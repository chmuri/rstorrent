use crate::download::Download;
use crate::download_factory::DownloadFactory;
use crate::download_list::DownloadList;
use crate::session::{SessionCommitEntry, SessionManager};
use lt_common::Object;
use std::collections::HashMap;
use std::io::{self, Error, ErrorKind};
use std::sync::{Arc, RwLock};

pub struct Manager {
    download_list: Arc<RwLock<DownloadList>>,
    session_manager: Arc<RwLock<SessionManager>>,
    magnet_path: String,
    throttles: HashMap<String, (u32, u32)>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

impl Manager {
    pub fn new() -> Self {
        let download_list = Arc::new(RwLock::new(DownloadList::new()));
        let session_manager = Arc::new(RwLock::new(SessionManager::new()));

        Self {
            download_list,
            session_manager,
            magnet_path: String::new(),
            throttles: HashMap::new(),
        }
    }

    pub fn download_list(&self) -> &Arc<RwLock<DownloadList>> {
        &self.download_list
    }

    pub fn session_manager(&self) -> &Arc<RwLock<SessionManager>> {
        &self.session_manager
    }

    pub fn magnet_path(&self) -> &str {
        &self.magnet_path
    }

    pub fn set_magnet_path(&mut self, path: impl Into<String>) {
        self.magnet_path = path.into();
    }

    pub fn set_throttle(&mut self, name: impl Into<String>, up_limit: u32, down_limit: u32) {
        self.throttles.insert(name.into(), (up_limit, down_limit));
    }

    pub fn get_throttle(&self, name: &str) -> Option<(u32, u32)> {
        self.throttles.get(name).copied()
    }

    pub fn load_torrent_object(
        &self,
        object: Object,
        start: bool,
    ) -> io::Result<Arc<RwLock<Download>>> {
        let mut factory = DownloadFactory::new();
        factory.load_object(object);
        factory.set_start(start);

        let mut dl_list = self.download_list.write().unwrap();
        factory.commit(&mut dl_list)
    }

    pub fn load_torrent_file(&self, path: &str, start: bool) -> io::Result<Arc<RwLock<Download>>> {
        let mut factory = DownloadFactory::new();
        factory.load_file(path)?;
        factory.set_start(start);

        let mut dl_list = self.download_list.write().unwrap();
        factory.commit(&mut dl_list)
    }

    pub fn build_commit_entry(download: &Download) -> io::Result<SessionCommitEntry> {
        let mut rtorrent_obj = Object::create_map();
        let _ = rtorrent_obj.insert_key("priority", Object::from(download.priority() as i64));
        let _ = rtorrent_obj.insert_key(
            "total_uploaded",
            Object::from(download.uploaded_bytes() as i64),
        );
        let _ = rtorrent_obj.insert_key(
            "total_downloaded",
            Object::from(download.downloaded_bytes() as i64),
        );
        let _ = rtorrent_obj.insert_key("directory", Object::from(download.directory()));
        let _ = rtorrent_obj.insert_key(
            "state",
            Object::from(if download.is_active() { 1i64 } else { 0i64 }),
        );
        let _ = rtorrent_obj.insert_key(
            "state_changed",
            Object::from(download.state_changed() as i64),
        );
        let _ = rtorrent_obj.insert_key(
            "state_counter",
            Object::from(download.state_counter() as i64),
        );
        if let Some(uri) = download.magnet_uri() {
            // Kept even after metadata arrives: if the `.torrent` file is ever
            // missing, the placeholder can still be recreated from this URI.
            let _ = rtorrent_obj.insert_key("magnet_uri", Object::from(uri));
        }

        let resume = download.to_resume_data();
        let mut resume_obj = resume.to_object();
        crate::session::resume_extras(&resume, download.metadata()).write_into(&mut resume_obj);
        let rtorrent_bytes = lt_common::object::bencode::encode(&rtorrent_obj)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?;
        let resume_bytes = lt_common::object::bencode::encode(&resume_obj)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?;

        let torrent_bytes = match download.bencode() {
            Some(object) => Some(
                lt_common::object::bencode::encode(object)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?,
            ),
            None => None,
        };

        Ok(SessionCommitEntry::new(
            download.info_hash_hex(),
            torrent_bytes,
            rtorrent_bytes,
            resume_bytes,
        ))
    }

    /// Persists all downloads in the session using the session manager's
    /// atomic per-download commit. This is the single entry point used by
    /// `session_save`, the scheduler and shutdown.
    pub fn commit(&self) -> io::Result<()> {
        let session = self.session_manager.read().unwrap();
        if !session.is_used() || !session.is_active() {
            return Ok(());
        }

        let downloads = self.download_list.read().unwrap().all();
        let mut entries = Vec::with_capacity(downloads.len());
        let mut first_error = None;

        for download in downloads {
            let mut guard = download.write().unwrap();
            // Magnet metadata may have been applied since the last network
            // supervisor tick. Refresh the wrapper (and its bencode) before
            // serializing so a shutdown never loses a freshly fetched torrent.
            guard.sync_metadata_from_engine();
            match Self::build_commit_entry(&guard) {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    tracing::error!(
                        error = %err,
                        "Could not serialize download session data."
                    );
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
        }

        // No downloads also means there is nothing to commit; avoid creating an
        // empty session directory side effect.
        if !entries.is_empty() {
            if let Err(err) = session.commit(&entries) {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }

        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Backwards-compatible name used by existing callers and tests.
    pub fn session_save(&self) -> io::Result<()> {
        self.commit()
    }

    /// Explicit alias for callers that prefer commit-oriented naming.
    pub fn session_commit(&self) -> io::Result<()> {
        self.commit()
    }

    /// Save one download's resume and `.rtorrent` state without rewriting the
    /// static torrent (legacy `d.save_resume`).
    ///
    /// An unconfigured session is a no-op, matching the legacy implementation.
    /// A configured but inactive session is an error rather than a silent
    /// success, so callers cannot believe resume data was persisted when it was
    /// not.
    pub fn save_download_resume(&self, download: &Download) -> io::Result<()> {
        let session = self.session_manager.read().unwrap();
        if !session.is_used() {
            return Ok(());
        }
        if !session.is_active() {
            return Err(Error::new(
                ErrorKind::Other,
                "d.save_resume: session manager is not active.",
            ));
        }

        let entry = Self::build_commit_entry(download)?;
        session.save_resume(&entry)
    }

    /// Save one download's full session streams, including the torrent
    /// (legacy `d.save_full_session`).
    pub fn save_download_full_session(&self, download: &Download) -> io::Result<()> {
        let session = self.session_manager.read().unwrap();
        if !session.is_used() {
            return Ok(());
        }
        if !session.is_active() {
            return Err(Error::new(
                ErrorKind::Other,
                "d.save_full_session: session manager is not active.",
            ));
        }

        let entry = Self::build_commit_entry(download)?;
        session.save_full_session(&entry)
    }

    /// Restores valid session entries, applying resume data where available.
    ///
    /// Invalid/corrupt entries are logged and skipped; startup is never aborted
    /// because one entry in the session directory is unusable.
    pub fn restore_session(&self) -> io::Result<usize> {
        let entries = {
            let session = self.session_manager.read().unwrap();
            if !session.is_used() || !session.is_active() {
                return Ok(0);
            }
            match session.get_restorable_entries() {
                Ok(entries) => entries,
                Err(err) => {
                    tracing::error!(
                        path = %session.path(),
                        error = %err,
                        "Could not enumerate session directory; continuing startup without restored downloads."
                    );
                    return Ok(0);
                }
            }
        };

        let session_path = {
            let session = self.session_manager.read().unwrap();
            session.path().to_string()
        };

        let mut restored = 0usize;

        for entry in entries {
            let torrent_path = format!("{}{}", session_path, entry.base_name);
            let expected_hash = &entry.base_name[..40];

            if self
                .download_list
                .read()
                .unwrap()
                .find_hex(expected_hash)
                .is_some()
            {
                tracing::debug!(
                    path = %torrent_path,
                    "Session entry already loaded; skipping duplicate."
                );
                continue;
            }

            let restored_entry = match crate::session::restore_session_entry(
                &torrent_path,
                expected_hash,
                entry.has_torrent,
            ) {
                Ok(entry) => entry,
                Err(err) => {
                    tracing::warn!(
                        path = %torrent_path,
                        error = %err,
                        "Skipping invalid session torrent entry."
                    );
                    continue;
                }
            };

            if let Some(err) = &restored_entry.resume_error {
                tracing::warn!(
                    path = %torrent_path,
                    error = %err,
                    "Session resume data was rejected; torrent restored without saved progress."
                );
            }

            let was_active = restored_entry.was_active;
            let resume_accepted = restored_entry.resume_error.is_none();
            let download = Arc::new(RwLock::new(restored_entry.download));
            {
                let mut download_list = self.download_list.write().unwrap();
                if !download_list.insert(Arc::clone(&download)) {
                    tracing::warn!(
                        path = %torrent_path,
                        "Skipping duplicate session torrent entry."
                    );
                    continue;
                }
            }

            // Only start (and therefore emit `event.download.resumed`) after
            // the resume record was accepted. Invalid resume data is left for
            // the normal hash-check path instead of being treated as a resume.
            if was_active && resume_accepted {
                self.download_list.read().unwrap().start(&download);
            }

            restored += 1;
        }

        Ok(restored)
    }

    pub fn shutdown(&self) -> io::Result<()> {
        let commit_result = self.commit();

        let stop_result = {
            let mut session = self.session_manager.write().unwrap();
            if session.is_active() {
                session.stop()
            } else {
                Ok(())
            }
        };

        self.download_list.write().unwrap().clear();

        commit_result.and(stop_result)
    }
}
