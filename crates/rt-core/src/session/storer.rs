use lt_common::object::bencode;
use lt_storage::resume::ResumeData;
use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;
#[cfg(target_os = "linux")]
use nix::unistd::fdatasync;
use nix::unistd::{fsync, write};
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub s_name: String,
    pub is_file: bool,
}

impl DirectoryEntry {
    pub fn is_file(&self) -> bool {
        self.is_file
    }
}

pub fn is_correct_format(f: &str) -> bool {
    if f.len() != 48 || !f.ends_with(".torrent") {
        return false;
    }

    // Work on bytes: slicing a UTF-8 string at a fixed byte offset can panic
    // when the directory contains a non-ASCII file name.
    f.as_bytes()[..40]
        .iter()
        .all(|b| b.is_ascii_hexdigit())
}

/// Loads a `.libtorrent_resume` file.
pub fn load_resume_file(path: &str) -> io::Result<ResumeData> {
    let bytes = fs::read(path)?;
    let object = bencode::decode(&bytes)
        .map_err(|err| Error::new(ErrorKind::InvalidData, err.to_string()))?;
    ResumeData::from_object(&object)
        .map_err(|err| Error::new(ErrorKind::InvalidData, err.to_string()))
}

/// Promote a staged ``.new`` file left behind by a crash between fsync and
/// rename.
///
/// The session writer fsyncs every staged stream before publishing it, so a
/// successfully written ``.new`` file is complete. We only promote it when the
/// destination is missing and the staged bytes still decode as bencode; a
/// partially written file is discarded instead. If the destination already
/// exists it is the last committed generation and the stale temporary file is
/// simply removed.
fn recover_staged_entry(base_path: &str) {
    let torrent_path = base_path.to_string();
    let suffixes = [".libtorrent_resume", ".rtorrent"];

    // The torrent file is the anchor for the whole entry; recover it first.
    let torrent_tmp = temp_path(&torrent_path);
    let torrent_present = recover_one(&torrent_path, &torrent_tmp);

    if !torrent_present {
        // Without a torrent the other streams cannot be restored, but stale
        // temporaries should still not accumulate forever.
        for suffix in suffixes {
            let path = format!("{}{}", base_path, suffix);
            let tmp = temp_path(&path);
            if fs::symlink_metadata(&tmp).is_ok() {
                let _ = fs::remove_file(&tmp);
            }
        }
        return;
    }

    let mut changed = false;
    for suffix in suffixes {
        let path = format!("{}{}", base_path, suffix);
        let tmp = temp_path(&path);
        if let Ok(meta) = fs::symlink_metadata(&tmp) {
            if !meta.is_file() {
                continue;
            }
            if Path::new(&path).is_file() {
                let _ = fs::remove_file(&tmp);
            } else if is_valid_bencode_file(&tmp) {
                if fs::rename(&tmp, &path).is_ok() {
                    changed = true;
                }
            } else {
                let _ = fs::remove_file(&tmp);
            }
        }
    }

    if changed {
        let _ = sync_directory_of(base_path);
    }
}

fn recover_one(path: &str, tmp: &str) -> bool {
    let tmp_meta = match fs::symlink_metadata(tmp) {
        Ok(meta) => meta,
        Err(_) => return Path::new(path).is_file(),
    };

    if !tmp_meta.is_file() {
        return Path::new(path).is_file();
    }

    if Path::new(path).is_file() {
        let _ = fs::remove_file(tmp);
        return true;
    }

    if is_valid_bencode_file(tmp) && fs::rename(tmp, path).is_ok() {
        let _ = sync_directory_of(path);
        return true;
    }

    let _ = fs::remove_file(tmp);
    Path::new(path).is_file()
}

fn is_valid_bencode_file(path: &str) -> bool {
    match fs::read(path) {
        Ok(bytes) => bencode::decode(&bytes).is_ok(),
        Err(_) => false,
    }
}

/// Recover staged session files left behind by interrupted writes.
pub fn recover_staged_entries(session_path: &str) -> io::Result<()> {
    if session_path.is_empty() {
        return Ok(());
    }

    let read_dir = fs::read_dir(session_path)?;
    for entry in read_dir {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();

        let base_name = match name.strip_suffix(".torrent.new") {
            Some(base) => format!("{}.torrent", base),
            None => continue,
        };

        if !is_correct_format(&base_name) {
            continue;
        }

        let separator = if session_path.ends_with('/') { "" } else { "/" };
        recover_staged_entry(&format!("{}{}{}", session_path, separator, base_name));
    }

    Ok(())
}

pub fn get_formatted_entries(session_path: &str) -> io::Result<Vec<DirectoryEntry>> {
    if session_path.is_empty() {
        return Ok(Vec::new());
    }

    // Complete a session write that was interrupted between fsync and rename
    // before enumerating what can be restored.
    recover_staged_entries(session_path)?;

    let read_dir = fs::read_dir(session_path)?;
    let mut entries = Vec::new();

    for entry in read_dir {
        // A single unreadable/vanishing directory entry must not abort restore
        // of the remaining session entries.
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if name_str.starts_with('.') || !is_correct_format(&name_str) {
            continue;
        }

        // Must check symlink_metadata so symlinks do not report is_file == true
        let is_file = match entry.path().symlink_metadata() {
            Ok(meta) => meta.is_file(),
            Err(_) => false,
        };

        entries.push(DirectoryEntry {
            s_name: name_str.to_string(),
            is_file,
        });
    }

    Ok(entries)
}

fn save_stream(path: &str, use_fsyncdisk: bool, data: &[u8]) -> io::Result<()> {
    // Remove any leftover temporary file first so that O_EXCL only ever fails
    // on an entry that appeared after the unlink, and O_NOFOLLOW keeps a symlink
    // planted in the session directory from redirecting the write.
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    let fd = open(
        path,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(|e| Error::from_raw_os_error(e as i32))?;

    let mut cursor = 0;
    while cursor < data.len() {
        match write(&fd, &data[cursor..]) {
            Ok(n) => cursor += n,
            Err(nix::errno::Errno::EINTR) => continue,
            // The descriptor is owned, so dropping it closes it even when a
            // write fails. The caller removes all staged `.new` files.
            Err(e) => return Err(Error::from_raw_os_error(e as i32)),
        }
    }

    if use_fsyncdisk {
        #[cfg(target_os = "macos")]
        let sync_res = fsync(&fd);
        #[cfg(not(target_os = "macos"))]
        let sync_res = fdatasync(&fd);

        if let Err(e) = sync_res {
            return Err(Error::from_raw_os_error(e as i32));
        }
    }

    drop(fd);
    Ok(())
}

fn temp_path(path: &str) -> String {
    format!("{}.new", path)
}

fn remove_staged_files(paths: &[String]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn sync_directory_of(path: &str) -> io::Result<()> {
    let dir = match path.rfind('/') {
        Some(0) => "/",
        Some(idx) => &path[..idx],
        None => ".",
    };

    let fd = open(dir, OFlag::O_RDONLY | OFlag::O_DIRECTORY, Mode::empty())
        .map_err(|e| Error::from_raw_os_error(e as i32))?;

    fsync(&fd).map_err(|e| Error::from_raw_os_error(e as i32))?;
    drop(fd);
    Ok(())
}

pub struct DownloadStorer;

impl DownloadStorer {
    pub fn build_path(session_path: &str, info_hash_hex: &str) -> io::Result<String> {
        if session_path.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "DownloadStorer::build_path() called with empty session path.",
            ));
        }
        if !session_path.ends_with('/') {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "DownloadStorer::build_path() session path missing trailing slash.",
            ));
        }
        if info_hash_hex.len() != 40 || !info_hash_hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "DownloadStorer::build_path() requires a 40-character hexadecimal info hash.",
            ));
        }

        Ok(format!(
            "{}{}.torrent",
            session_path,
            info_hash_hex.to_uppercase()
        ))
    }

    pub fn unlink_files(base_path: &str) {
        let torrent_path = base_path.to_string();
        let libtorrent_path = format!("{}.libtorrent_resume", base_path);
        let rtorrent_path = format!("{}.rtorrent", base_path);
        let meta_path = format!("{}.meta", base_path);

        let _ = fs::remove_file(libtorrent_path);
        let _ = fs::remove_file(rtorrent_path);
        let _ = fs::remove_file(torrent_path);
        let _ = fs::remove_file(meta_path);
    }

    pub fn save_and_move_streams(
        path: &str,
        use_fsyncdisk: bool,
        torrent_stream: Option<&[u8]>,
        rtorrent_stream: &[u8],
        libtorrent_stream: &[u8],
    ) -> io::Result<()> {
        let torrent_path = path.to_string();
        let libtorrent_path = format!("{}.libtorrent_resume", path);
        let rtorrent_path = format!("{}.rtorrent", path);

        let torrent_tmp = temp_path(&torrent_path);
        let libtorrent_tmp = temp_path(&libtorrent_path);
        let rtorrent_tmp = temp_path(&rtorrent_path);

        // Stage every stream first. Until all three writes have completed (and
        // have been fsynced, when requested) the previously committed files are
        // left untouched, so a crash or write failure cannot destroy the last
        // good session state.
        let stage_result = (|| {
            if let Some(data) = torrent_stream {
                save_stream(&torrent_tmp, use_fsyncdisk, data)?;
            }
            save_stream(&libtorrent_tmp, use_fsyncdisk, libtorrent_stream)?;
            save_stream(&rtorrent_tmp, use_fsyncdisk, rtorrent_stream)?;
            Ok::<(), io::Error>(())
        })();

        if let Err(err) = stage_result {
            remove_staged_files(&[torrent_tmp, libtorrent_tmp, rtorrent_tmp]);
            return Err(err);
        }

        // A directory (or other non-replaceable object) at a destination would
        // fail halfway through the rename sequence. Reject it before publishing
        // anything so the previously committed session stays fully intact.
        let targets = if torrent_stream.is_some() {
            vec![&torrent_path, &libtorrent_path, &rtorrent_path]
        } else {
            vec![&libtorrent_path, &rtorrent_path]
        };
        for target in targets {
            if let Ok(meta) = fs::symlink_metadata(target) {
                if meta.is_dir() {
                    remove_staged_files(&[torrent_tmp, libtorrent_tmp, rtorrent_tmp]);
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("Session destination is a directory: {}", target),
                    ));
                }
            }
        }

        // Publish the staged files. A failure here still leaves the old files in
        // place for any stream that has not been renamed yet.
        let publish_result = (|| {
            if torrent_stream.is_some() {
                fs::rename(&torrent_tmp, &torrent_path)?;
            }
            fs::rename(&libtorrent_tmp, &libtorrent_path)?;
            fs::rename(&rtorrent_tmp, &rtorrent_path)?;
            Ok::<(), io::Error>(())
        })();

        if let Err(err) = publish_result {
            remove_staged_files(&[torrent_tmp, libtorrent_tmp, rtorrent_tmp]);
            return Err(err);
        }

        // Syncing the files themselves does not persist the renames; the
        // directory holding them has to be synced for the new names to survive
        // a crash.
        if use_fsyncdisk {
            sync_directory_of(path)?;
        }

        Ok(())
    }

    /// Loads a `.libtorrent_resume` file.
    pub fn load_resume_file(path: &str) -> io::Result<ResumeData> {
        let bytes = fs::read(path)?;
        let object = bencode::decode(&bytes)
            .map_err(|err| Error::new(ErrorKind::InvalidData, err.to_string()))?;
        ResumeData::from_object(&object)
            .map_err(|err| Error::new(ErrorKind::InvalidData, err.to_string()))
    }

    pub fn get_formatted_entries(session_path: &str) -> io::Result<Vec<DirectoryEntry>> {
        crate::session::storer::get_formatted_entries(session_path)
    }
}
