pub mod manager;
pub mod storer;

use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::Path;
use std::time::UNIX_EPOCH;

use lt_common::object::bencode;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_storage::file::Priority;
use lt_storage::resume::{
    ResumeData, ResumeExtras, MTIME_DOWNLOADING, MTIME_RECHECK, MTIME_SKIPPED, MTIME_UNCHECKED,
};

use crate::download::Download;
use crate::download_factory::DownloadFactory;

pub use manager::{SessionCommitEntry, SessionManager};
pub use storer::{
    get_formatted_entries, is_correct_format, load_resume_file, recover_staged_entries,
    DirectoryEntry, DownloadStorer,
};

/// Per-file decisions made while validating persisted resume data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeFileAction {
    /// File exists with the expected size and mtime; its saved pieces are kept.
    Trust,
    /// File metadata changed (or was never available); clear its piece range.
    Recheck,
    /// File was never created (legacy mtime `~0`); clear its piece range.
    Create,
    /// File was intentionally skipped (legacy mtime `~1`); clear its pieces.
    Skip,
}

/// Summary of the validation applied to one resume record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeValidation {
    /// The saved bitfield did not match the torrent and was discarded.
    pub bitfield_discarded: bool,
    /// `completed` disagreed with the bitfield and was recomputed.
    pub completed_recomputed: bool,
    pub files_trusted: usize,
    pub files_rechecked: usize,
    pub files_missing: usize,
    /// Kept for callers that want to report per-file outcomes in order.
    pub file_actions: Vec<ResumeFileAction>,
}

/// Result of loading and applying a session resume file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeApplyOutcome {
    /// A resume record was found, validated and accepted.
    pub resume_loaded: bool,
    /// Number of pieces restored from an accepted bitfield.
    pub completed_chunks: usize,
    pub validation: ResumeValidation,
}

/// A session download plus the resume state that was accepted for it.
pub struct RestoredSessionDownload {
    pub download: Download,
    pub resume_loaded: bool,
    /// The download was active when the session was saved. Callers should only
    /// start it after accepted resume data has been applied.
    pub was_active: bool,
    /// Diagnostics for an unreadable/invalid resume record. Such a record is
    /// ignored and the torrent is restored without saved progress.
    pub resume_error: Option<String>,
    pub outcome: ResumeApplyOutcome,
}

/// Restore the `.rtorrent` sidecar (directory, priority, transfer totals).
///
/// Missing sidecars are not an error: the download is simply restored with its
/// defaults. The directory must be restored before resume validation because
/// per-file mtimes are checked relative to it.
///
/// Returns whether the download was active when the sidecar was written.
pub fn restore_rtorrent_state(base_path: &str, download: &mut Download) -> bool {
    let path = format!("{}.rtorrent", base_path);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => {
            if err.kind() != ErrorKind::NotFound {
                tracing::warn!(path = %path, error = %err, "Could not read session state file.");
            }
            return false;
        }
    };

    let object = match bencode::decode(&bytes) {
        Ok(object) => object,
        Err(err) => {
            tracing::warn!(path = %path, error = %err, "Could not decode session state file.");
            return false;
        }
    };

    if let Ok(directory) = object.get_key_string("directory") {
        let directory = String::from_utf8_lossy(directory);
        if !directory.is_empty() {
            download.set_directory(directory.into_owned());
        }
    }

    if let Ok(priority) = object.get_key_value("priority") {
        if let Ok(priority) = u32::try_from(priority) {
            download.set_priority(priority);
        }
    }

    object
        .get_key_value("state")
        .map(|state| state > 0)
        .unwrap_or(false)
}

/// Load a `.libtorrent_resume` file, validate it against the download's
/// metadata and apply it.
///
/// Validation is deliberately file-local: a changed or missing payload file
/// only clears the pieces belonging to that file. The torrent itself is still
/// restored, and a bad bitfield only means saved progress is discarded.
pub fn load_resume_for_download(
    path: &str,
    download: &mut Download,
) -> io::Result<ResumeApplyOutcome> {
    let bytes = fs::read(path)?;
    let object = bencode::decode(&bytes)
        .map_err(|err| Error::new(ErrorKind::InvalidData, err.to_string()))?;

    let metadata = download.metadata().clone();
    let directory = download.directory().to_string();

    let mut resume = ResumeData::from_object_with_piece_count(&object, metadata.num_pieces)
        .map_err(|err| Error::new(ErrorKind::InvalidData, err.to_string()))?;

    let validation = prepare_resume_data(&mut resume, &metadata, &directory);
    let completed_chunks = download
        .apply_resume(&resume)
        .map_err(|err| Error::new(ErrorKind::InvalidData, err))?;

    Ok(ResumeApplyOutcome {
        resume_loaded: true,
        completed_chunks,
        validation,
    })
}

/// Validate a parsed resume record against torrent metadata and the payload
/// files on disk, clearing only the affected file ranges.
pub fn prepare_resume_data(
    resume: &mut ResumeData,
    metadata: &TorrentMetadata,
    save_directory: &str,
) -> ResumeValidation {
    let mut validation = ResumeValidation::default();

    // A bitfield from a different torrent revision (or a corrupt/torn write)
    // has to be discarded. File priorities and mtimes remain useful.
    if !resume.bitfield_is_valid(metadata.num_pieces) {
        resume.bitfield = None;
        validation.bitfield_discarded = true;
    }

    // Older writers could leave the files list short. Treat missing entries as
    // "recheck" rather than aborting the whole torrent.
    if resume.file_mtimes.len() < metadata.files.len() {
        validation.files_rechecked += metadata.files.len() - resume.file_mtimes.len();
        resume
            .file_mtimes
            .resize(metadata.files.len(), MTIME_RECHECK);
    }
    if resume.file_priorities.len() < metadata.files.len() {
        resume
            .file_priorities
            .resize(metadata.files.len(), Priority::Normal);
    }

    for (index, file) in metadata.files.iter().enumerate() {
        let mtime = resume.file_mtimes[index];
        let action = match mtime {
            MTIME_UNCHECKED => ResumeFileAction::Create,
            MTIME_SKIPPED => ResumeFileAction::Skip,
            // The bitfield was written while the file was actively
            // downloading; it is trusted and uncertain pieces are handled by
            // the engine's pending-hash list.
            MTIME_DOWNLOADING => ResumeFileAction::Trust,
            MTIME_RECHECK => ResumeFileAction::Recheck,
            actual_mtime => {
                if save_directory.is_empty() {
                    // No directory yet (e.g. a session torrent without a
                    // `.rtorrent` sidecar). Keep the saved progress; it will be
                    // rechecked later if the files are not where expected.
                    ResumeFileAction::Trust
                } else {
                    let relative = file.path.as_string();
                    let path = Path::new(save_directory).join(relative.trim_start_matches('/'));
                    match fs::metadata(&path) {
                        Ok(file_meta) => {
                            let size_matches = file_meta.len() == file.length;
                            let mtime_matches = file_meta
                                .modified()
                                .ok()
                                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                                .map(|duration| duration.as_secs() as i64)
                                == Some(actual_mtime);

                            if size_matches && mtime_matches {
                                ResumeFileAction::Trust
                            } else {
                                resume.file_mtimes[index] = MTIME_RECHECK;
                                ResumeFileAction::Recheck
                            }
                        }
                        Err(_) => {
                            resume.file_mtimes[index] = MTIME_RECHECK;
                            ResumeFileAction::Recheck
                        }
                    }
                }
            }
        };

        match action {
            ResumeFileAction::Trust => validation.files_trusted += 1,
            ResumeFileAction::Recheck => validation.files_rechecked += 1,
            ResumeFileAction::Create => {
                validation.files_missing += 1;
                clear_file_range(&mut resume.bitfield, file, metadata.piece_length);
            }
            ResumeFileAction::Skip => {
                validation.files_missing += 1;
                clear_file_range(&mut resume.bitfield, file, metadata.piece_length);
            }
        }

        if action == ResumeFileAction::Recheck {
            clear_file_range(&mut resume.bitfield, file, metadata.piece_length);
        }

        validation.file_actions.push(action);
    }

    validation.completed_recomputed =
        resume.normalize_completed(metadata.total_size, metadata.piece_length);

    validation
}

fn clear_file_range(
    bitfield: &mut Option<lt_common::bitfield::Bitfield>,
    file: &TorrentFileEntry,
    piece_length: u32,
) {
    if file.length == 0 || piece_length == 0 {
        return;
    }

    let Some(bitfield) = bitfield.as_mut() else {
        return;
    };
    let piece_length = piece_length as u64;
    let first = (file.offset / piece_length) as usize;
    let last = ((file.offset + file.length - 1) / piece_length) as usize;

    let Some(max_index) = bitfield.size_bits().checked_sub(1) else {
        return;
    };
    for index in first..=last.min(max_index) {
        bitfield.unset(index);
    }
}

/// Build the legacy-only parts of a resume record for a snapshot.
///
/// The engine derives completion from the global bitfield, but older rtorrent
/// clients also read the per-file `completed` counter. Computing it here keeps
/// the saved file readable by both implementations without adding state to
/// `ResumeData` itself.
pub fn resume_extras(resume: &ResumeData, metadata: &TorrentMetadata) -> ResumeExtras {
    let file_completed_chunks = metadata
        .files
        .iter()
        .map(|file| -> Option<u64> {
            let Some(bitfield) = resume.bitfield.as_ref() else {
                return None;
            };
            if file.length == 0 || metadata.piece_length == 0 {
                return Some(0);
            }

            let piece_length = metadata.piece_length as u64;
            let first = (file.offset / piece_length) as usize;
            let last = ((file.offset + file.length - 1) / piece_length) as usize;
            let last = last.min(bitfield.size_bits().saturating_sub(1));

            let mut completed = 0u64;
            for index in first..=last {
                if bitfield.get(index) {
                    completed += 1;
                }
            }
            Some(completed)
        })
        .collect();

    ResumeExtras {
        file_completed_chunks,
        trackers: Vec::new(),
    }
}

/// Load one session entry before it is inserted into a download list.
///
/// Ordering is intentional: the torrent is created first, the `.rtorrent`
/// sidecar restores the save directory, and only then is resume data validated
/// and applied. Invalid/corrupt session entries return `Err` so the caller can
/// skip them without aborting startup.
pub fn load_session_download(
    torrent_path: &str,
    expected_hash: &str,
) -> io::Result<RestoredSessionDownload> {
    let mut factory = DownloadFactory::new();
    factory.set_session(true);
    factory.set_init_load(true);
    factory.set_print_log(false);
    factory.load_file(torrent_path)?;

    let mut download = factory.create_download()?;

    let actual_hash = download.info_hash_hex();
    if !actual_hash.eq_ignore_ascii_case(expected_hash) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "session torrent hash {} does not match file name hash {}",
                actual_hash, expected_hash
            ),
        ));
    }

    let was_active = restore_rtorrent_state(torrent_path, &mut download);

    let resume_path = format!("{}.libtorrent_resume", torrent_path);
    let (resume_loaded, resume_error, outcome) =
        match load_resume_for_download(&resume_path, &mut download) {
            Ok(outcome) => (true, None, outcome),
            Err(err) if err.kind() == ErrorKind::NotFound => {
                (false, None, ResumeApplyOutcome::default())
            }
            Err(err) => {
                tracing::warn!(
                    path = %resume_path,
                    error = %err,
                    "Could not apply session resume data; restoring torrent without saved progress."
                );
                (false, Some(err.to_string()), ResumeApplyOutcome::default())
            }
        };

    // Restored downloads must not be active before the manager decides to
    // start them; otherwise `DownloadList::start` cannot emit the legacy
    // `event.download.resumed` transition.
    if was_active {
        download.stop();
    }

    Ok(RestoredSessionDownload {
        download,
        resume_loaded,
        was_active,
        resume_error,
        outcome,
    })
}
