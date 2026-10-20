use super::storer::{is_correct_format, DirectoryEntry, DownloadStorer};
use crate::commands::system::resolve_path;
use lt_sys::lockfile::Lockfile;
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::Path;

/// One download's already-serialized session streams.
///
/// Keeping serialization outside `SessionManager` means the manager owns only
/// persistence concerns, while the caller (normally `rt_core::Manager::commit`)
/// owns the download/session-data mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCommitEntry {
    pub info_hash_hex: String,
    pub torrent_stream: Option<Vec<u8>>,
    pub rtorrent_stream: Vec<u8>,
    pub libtorrent_stream: Vec<u8>,
}

impl SessionCommitEntry {
    pub fn new(
        info_hash_hex: impl Into<String>,
        torrent_stream: Option<Vec<u8>>,
        rtorrent_stream: Vec<u8>,
        libtorrent_stream: Vec<u8>,
    ) -> Self {
        Self {
            info_hash_hex: info_hash_hex.into(),
            torrent_stream,
            rtorrent_stream,
            libtorrent_stream,
        }
    }
}

pub struct SessionManager {
    path: String,
    use_fsyncdisk: bool,
    use_lock: bool,
    on_completion: bool,
    active: bool,
    freeze_info: bool,
    lockfile: Option<Lockfile>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            path: String::new(),
            use_fsyncdisk: true,
            use_lock: true,
            on_completion: true,
            active: false,
            freeze_info: false,
            lockfile: None,
        }
    }

    pub fn is_used(&self) -> bool {
        !self.path.is_empty()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn set_path(&mut self, path: &str) -> io::Result<()> {
        if self.freeze_info {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Session path cannot be changed after startup.",
            ));
        }

        if path.is_empty() {
            self.path.clear();
            return Ok(());
        }

        let expanded = crate::commands::system::expand_tilde(path)
            .unwrap_or_else(|_| path.to_string());
        let _ = fs::create_dir_all(&expanded);

        let resolved = resolve_path(&expanded);
        if resolved.is_empty() {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!(
                    "Could not resolve session path '{}'. The directory must exist and be accessible.",
                    path
                ),
            ));
        }

        if resolved.ends_with('/') {
            self.path = resolved;
        } else {
            self.path = format!("{}/", resolved);
        }

        Ok(())
    }

    fn session_lock_path(&self) -> String {
        format!("{}rtorrent.lock", self.path)
    }

    /// Returns the path of the session lock file, if a path is configured.
    pub fn lock_path(&self) -> Option<String> {
        if self.path.is_empty() {
            None
        } else {
            Some(self.session_lock_path())
        }
    }

    pub fn is_locked(&self) -> bool {
        self.lockfile
            .as_ref()
            .map(Lockfile::is_locked)
            .unwrap_or(false)
    }

    pub fn use_fsyncdisk(&self) -> bool {
        self.use_fsyncdisk
    }

    pub fn set_use_fsyncdisk(&mut self, use_fsyncdisk: bool) -> io::Result<()> {
        if self.freeze_info {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Session fsyncdisk option cannot be changed after startup.",
            ));
        }
        self.use_fsyncdisk = use_fsyncdisk;
        Ok(())
    }

    /// Whether finishing a download triggers an immediate session save.
    pub fn on_completion(&self) -> bool {
        self.on_completion
    }

    pub fn set_on_completion(&mut self, on_completion: bool) {
        self.on_completion = on_completion;
    }

    pub fn use_lock(&self) -> bool {
        self.use_lock
    }

    pub fn set_use_lock(&mut self, use_lock: bool) -> io::Result<()> {
        if self.freeze_info {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Session lock option cannot be changed after startup.",
            ));
        }
        self.use_lock = use_lock;
        Ok(())
    }

    pub fn start(&mut self) -> io::Result<()> {
        if self.active || self.freeze_info {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                "SessionManager::start() called while already started.",
            ));
        }

        if self.path.is_empty() {
            self.active = true;
            self.freeze_info = true;
            return Ok(());
        }

        let path = Path::new(&self.path);
        match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                let err = Error::new(
                    ErrorKind::NotADirectory,
                    format!("Session path is not a directory: {}", self.path),
                );
                return Err(err);
            }
            Err(err) => {
                return Err(Error::new(
                    err.kind(),
                    format!("Could not access session directory {}: {}", self.path, err),
                ));
            }
        }

        if self.use_lock {
            let lock_path = self.session_lock_path();
            let mut lock = Lockfile::with_path(lock_path.clone());
            if !lock.try_lock() {
                let locked_by = lock.locked_by_as_string();
                let lock_exists = Path::new(&lock_path).exists();
                return Err(if lock_exists {
                    Error::new(
                        ErrorKind::AddrInUse,
                        format!(
                            "Could not lock session directory, held by: {} : {}",
                            locked_by, self.path
                        ),
                    )
                } else {
                    Error::new(
                        ErrorKind::PermissionDenied,
                        format!(
                            "Could not lock session directory (create '{}' failed) : {}",
                            lock_path, self.path
                        ),
                    )
                });
            }

            self.lockfile = Some(lock);
        }

        self.active = true;
        self.freeze_info = true;
        Ok(())
    }

    pub fn stop(&mut self) -> io::Result<()> {
        if !self.active {
            return Err(Error::new(
                ErrorKind::Other,
                "SessionManager::stop() called while not active.",
            ));
        }

        if let Some(mut lock) = self.lockfile.take() {
            if !lock.unlock() {
                tracing::warn!(
                    path = %lock.path(),
                    "Could not remove session lock file while stopping."
                );
            }
        }

        self.active = false;
        Ok(())
    }

    pub fn save_streams(
        &self,
        info_hash_hex: &str,
        torrent_stream: Option<&[u8]>,
        rtorrent_stream: &[u8],
        libtorrent_stream: &[u8],
    ) -> io::Result<()> {
        if self.path.is_empty() {
            return Ok(());
        }

        if !self.active {
            return Err(Error::new(
                ErrorKind::Other,
                "SessionManager::save_streams() called while not active.",
            ));
        }

        let file_path = DownloadStorer::build_path(&self.path, info_hash_hex)?;
        DownloadStorer::save_and_move_streams(
            &file_path,
            self.use_fsyncdisk,
            torrent_stream,
            rtorrent_stream,
            libtorrent_stream,
        )
    }

    /// Commits every entry, continuing past individual failures so one broken
    /// download cannot prevent the remaining downloads from being persisted.
    ///
    /// Each download is published atomically (staged temp files are fsynced
    /// before the rename). The first error is returned after all entries have
    /// been attempted.
    pub fn commit(&self, entries: &[SessionCommitEntry]) -> io::Result<usize> {
        if self.path.is_empty() {
            return Ok(0);
        }

        if !self.active {
            return Err(Error::new(
                ErrorKind::Other,
                "SessionManager::commit() called while not active.",
            ));
        }

        let mut saved = 0usize;
        let mut first_error = None;

        for entry in entries {
            let result = self.save_streams(
                &entry.info_hash_hex,
                entry.torrent_stream.as_deref(),
                &entry.rtorrent_stream,
                &entry.libtorrent_stream,
            );

            match result {
                Ok(()) => saved += 1,
                Err(err) => {
                    tracing::error!(
                        hash = %entry.info_hash_hex,
                        path = %self.path,
                        error = %err,
                        "Could not commit download session data."
                    );
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
        }

        match first_error {
            Some(err) => Err(err),
            None => Ok(saved),
        }
    }

    /// Persist only the resume and `.rtorrent` streams for one download.
    ///
    /// This mirrors the legacy `d.save_resume` action: the static torrent file
    /// is left untouched.
    pub fn save_resume(&self, entry: &SessionCommitEntry) -> io::Result<()> {
        self.save_streams(
            &entry.info_hash_hex,
            None,
            &entry.rtorrent_stream,
            &entry.libtorrent_stream,
        )
    }

    /// Persist the torrent, resume and `.rtorrent` streams for one download.
    ///
    /// This mirrors the legacy `d.save_full_session` action.
    pub fn save_full_session(&self, entry: &SessionCommitEntry) -> io::Result<()> {
        self.save_streams(
            &entry.info_hash_hex,
            entry.torrent_stream.as_deref(),
            &entry.rtorrent_stream,
            &entry.libtorrent_stream,
        )
    }

    /// Returns the valid session entries in deterministic restore order.
    ///
    /// Symlinks, malformed names and vanished files are skipped. A single bad
    /// entry must not prevent the rest of the session from being restored.
    pub fn get_valid_entries(&self) -> io::Result<Vec<DirectoryEntry>> {
        let mut entries = self.get_entries()?;
        entries.retain(|entry| {
            entry.is_file
                && is_correct_format(&entry.s_name)
                && Path::new(&format!("{}{}", self.path, entry.s_name)).is_file()
        });
        entries.sort_by(|a, b| a.s_name.cmp(&b.s_name));
        Ok(entries)
    }

    pub fn remove_session_files(&self, info_hash_hex: &str) -> io::Result<()> {
        if self.path.is_empty() {
            return Ok(());
        }

        if !self.active {
            return Err(Error::new(
                ErrorKind::Other,
                "SessionManager::remove_session_files() called while not active.",
            ));
        }

        let file_path = DownloadStorer::build_path(&self.path, info_hash_hex)?;
        DownloadStorer::unlink_files(&file_path);
        Ok(())
    }

    /// Removes the session files belonging to one download.
    ///
    /// Called when a download is erased so stale session entries cannot be
    /// restored on the next startup. User data files are never touched.
    pub fn erase_entry(&self, info_hash_hex: &str) -> io::Result<bool> {
        if self.path.is_empty() || !self.active {
            return Ok(false);
        }

        let base = DownloadStorer::build_path(&self.path, info_hash_hex)?;
        DownloadStorer::unlink_files(&base);
        for suffix in [
            ".new",
            ".libtorrent_resume.new",
            ".rtorrent.new",
            ".meta.new",
        ] {
            let _ = fs::remove_file(format!("{base}{suffix}"));
        }
        Ok(true)
    }

    pub fn get_entries(&self) -> io::Result<Vec<DirectoryEntry>> {
        if self.path.is_empty() {
            return Ok(Vec::new());
        }
        let mut entries = DownloadStorer::get_formatted_entries(&self.path)?;
        // Restore in a stable order rather than relying on readdir order.
        entries.sort_by(|a, b| a.s_name.cmp(&b.s_name));
        Ok(entries)
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        // `Lockfile` also removes the file on drop, but doing it explicitly here
        // makes the release ordering obvious and guarantees the lock is gone
        // before the manager is considered dead.
        if let Some(mut lock) = self.lockfile.take() {
            if !lock.unlock() {
                tracing::warn!(
                    path = %lock.path(),
                    "Could not remove session lock file while dropping manager."
                );
            }
        }
        self.active = false;
    }
}
