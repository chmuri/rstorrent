use lt_common::hash_string::HashString;
use lt_common::Object;
use lt_engine::constructor::TorrentMetadata;
use lt_engine::download::{Download as EngineDownload, DownloadState};
use lt_storage::resume::ResumeData;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub const DEFAULT_RESUME_FLAGS: u32 = !0;

pub struct Download {
    download: Arc<Mutex<EngineDownload>>,
    metadata: TorrentMetadata,
    bencode: Option<Object>,
    /// Original magnet URI for downloads that were added before their info
    /// dictionary was available. Persisted in the `.rtorrent` sidecar so the
    /// placeholder can be recreated after a restart.
    magnet_uri: Option<String>,
    message: String,
    priority: u32,
    resume_flags: u32,
    group: u32,
    directory: String,
    throttle_name: String,
    hash_failed: bool,
    hash_checked: bool,
    erasing: bool,
    is_open: bool,
    connection_seed: String,
    accepting_seeders: bool,
    chunks_hashed: Arc<AtomicU32>,
    hash_checking: Arc<AtomicBool>,
    last_active: u64,
}

impl Download {
    pub fn new(download: EngineDownload) -> Self {
        let metadata = download.metadata().clone();
        let chunks_hashed = download.chunks_hashed_arc();
        Self {
            download: Arc::new(Mutex::new(download)),
            metadata,
            bencode: None,
            magnet_uri: None,
            message: String::new(),
            priority: 2, // normal
            resume_flags: DEFAULT_RESUME_FLAGS,
            group: 0,
            directory: String::new(),
            throttle_name: String::new(),
            hash_failed: false,
            hash_checked: false,
            erasing: false,
            is_open: false,
            connection_seed: "leech".to_string(),
            accepting_seeders: true,
            chunks_hashed,
            hash_checking: Arc::new(AtomicBool::new(false)),
            last_active: 0,
        }
    }

    pub fn from_metadata(metadata: TorrentMetadata) -> Self {
        Self::new(EngineDownload::new(metadata))
    }

    pub fn engine_download(&self) -> Arc<Mutex<EngineDownload>> {
        Arc::clone(&self.download)
    }

    pub fn download(&self) -> MutexGuard<'_, EngineDownload> {
        self.download.lock().unwrap()
    }

    pub fn download_mut(&mut self) -> MutexGuard<'_, EngineDownload> {
        self.download.lock().unwrap()
    }

    pub fn metadata(&self) -> &TorrentMetadata {
        &self.metadata
    }

    /// The magnet URI this download was created from, if any.
    pub fn magnet_uri(&self) -> Option<&str> {
        self.magnet_uri.as_deref()
    }

    pub fn set_magnet_uri(&mut self, uri: impl Into<String>) {
        self.magnet_uri = Some(uri.into());
    }

    pub fn is_metadata_download(&self) -> bool {
        self.metadata.is_meta_download
    }

    /// Updates the wrapper's metadata snapshot once the network layer has
    /// fetched the info dictionary for a magnet download.
    ///
    /// When metadata becomes available this also reconstructs the torrent's
    /// bencode (`info` + trackers) so the session writer can persist the
    /// missing `<info-hash>.torrent` file. Without it a magnet download is
    /// never restorable after a restart.
    pub fn sync_metadata_from_engine(&mut self) -> bool {
        if !self.metadata.is_meta_download {
            return false;
        }

        let (metadata, info_bytes) = {
            let mut engine = self.download.lock().unwrap();
            if engine.is_metadata_download() {
                return false;
            }

            let metadata = engine.metadata().clone();
            let info_bytes = lt_engine::network::download_info_bytes(&engine);
            // Once the real file layout is known, an already present payload
            // (e.g. an ISO downloaded earlier) must be verified before it is
            // used for seeding.
            let _ = engine.queue_recheck_if_files_exist();
            (metadata, info_bytes)
        };

        self.metadata = metadata;
        self.hash_checked = false;

        if self.bencode.is_none() {
            if let Some(bytes) = info_bytes {
                self.bencode = Self::bencode_from_info(&self.metadata, bytes.as_slice());
            }
        }

        true
    }

    /// Builds the top-level torrent dictionary from the verified raw `info`
    /// bytes. Re-encoding [`TorrentMetadata`] directly is not safe because
    /// unknown extension keys would change the info-hash.
    fn bencode_from_info(metadata: &TorrentMetadata, info_bytes: &[u8]) -> Option<Object> {
        let info = lt_common::object::bencode::decode(info_bytes).ok()?;

        let mut root = Object::create_map();
        root.insert_key("info", info).ok()?;

        if let Some(announce) = metadata.announce.as_deref().filter(|url| !url.is_empty()) {
            root.insert_key("announce", Object::from(announce)).ok()?;
        }

        if !metadata.announce_list.is_empty() {
            let mut tiers = Object::create_list();
            for tier in &metadata.announce_list {
                let mut urls = Object::create_list();
                for url in tier {
                    urls.insert_back(Object::from(url.as_str())).ok()?;
                }
                if !urls.as_list().map(|list| list.is_empty()).unwrap_or(true) {
                    tiers.insert_back(urls).ok()?;
                }
            }
            if !tiers.as_list().map(|list| list.is_empty()).unwrap_or(true) {
                root.insert_key("announce-list", tiers).ok()?;
            }
        }

        Some(root)
    }

    pub fn is_sequential(&self) -> bool {
        self.download.lock().unwrap().is_sequential()
    }

    pub fn set_sequential(&mut self, value: bool) {
        self.download.lock().unwrap().set_sequential(value);
    }

    pub fn last_active(&self) -> u64 {
        self.last_active
    }

    pub fn set_last_active(&mut self, value: u64) {
        self.last_active = value;
    }

    pub fn info_hash(&self) -> &HashString {
        &self.metadata.info_hash
    }

    pub fn info_hash_hex(&self) -> String {
        self.metadata.info_hash.to_hex()
    }

    pub fn name(&self) -> &str {
        &self.metadata.name
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn set_open(&mut self, open: bool) {
        if open {
            let _ = self.open();
        } else {
            self.close();
        }
    }

    pub fn state(&self) -> DownloadState {
        if self.is_hash_checking() {
            return DownloadState::Checking;
        }
        self.download.lock().unwrap().state().clone()
    }

    /// Human-readable state name (used by `d.state`/`d.connection_current`).
    pub fn state_name(&self) -> String {
        format!("{:?}", self.state())
    }

    /// rTorrent-compatible numeric state: 0 stopped, 1 started.
    pub fn state_value(&self) -> i64 {
        self.state().as_rpc_value()
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.state(),
            DownloadState::Downloading | DownloadState::Seeding
        )
    }

    pub fn is_closed(&self) -> bool {
        matches!(self.state(), DownloadState::Closed)
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self.state(), DownloadState::Stopped)
    }

    pub fn is_paused(&self) -> bool {
        matches!(self.state(), DownloadState::Paused)
    }

    pub fn is_error(&self) -> bool {
        matches!(self.state(), DownloadState::Error(_))
    }

    pub fn is_done(&self) -> bool {
        if self.is_hash_checking() {
            return false;
        }
        self.download.lock().unwrap().is_done()
    }

    pub fn is_downloading(&self) -> bool {
        self.is_active() && !self.is_done()
    }

    pub fn is_seeding(&self) -> bool {
        self.is_active() && self.is_done()
    }

    pub fn is_initial_seeding(&self) -> bool {
        if self.is_hash_checking() {
            return false;
        }
        self.download.lock().unwrap().is_initial_seeding()
    }

    /// Preferred connection type: `leech`, `seed` or `initial_seed`.
    pub fn connection_seed(&self) -> &str {
        &self.connection_seed
    }

    /// Selects the connection type used when the download starts.
    ///
    /// Switching to `initial_seed` while already seeding enables BEP 16
    /// immediately; switching away disables it.
    pub fn set_connection_seed(&mut self, value: &str) -> Result<(), String> {
        let value = value.trim().to_ascii_lowercase();
        if !matches!(value.as_str(), "leech" | "seed" | "initial_seed") {
            return Err(format!("invalid connection seed type '{value}'"));
        }

        self.connection_seed = value;
        let active = self.is_active();
        let mut engine = self.download.lock().unwrap();
        if self.connection_seed == "initial_seed" {
            if active {
                engine.start_initial_seeding();
            }
        } else {
            engine.stop_initial_seeding();
        }
        Ok(())
    }

    pub fn accepting_seeders(&self) -> bool {
        self.accepting_seeders
    }

    pub fn set_accepting_seeders(&mut self, value: bool) {
        self.accepting_seeders = value;
        self.download.lock().unwrap().set_accepting_seeders(value);
    }

    pub fn is_hash_checked(&self) -> bool {
        if !self.is_open || !self.hash_checked || self.is_hash_checking() {
            return false;
        }

        // A resume validation may have queued a partial recheck. Until it
        // completes the persisted bits are not trustworthy.
        let engine = self.download.lock().unwrap();
        !engine.is_checking() && !engine.has_pending_hash_pieces()
    }

    pub fn is_hash_checking(&self) -> bool {
        self.hash_checking.load(Ordering::Relaxed)
    }

    pub fn set_hash_checking(&mut self, checking: bool) {
        self.hash_checking.store(checking, Ordering::Relaxed);
    }

    pub fn chunks_hashed(&self) -> u32 {
        self.chunks_hashed.load(Ordering::Relaxed)
    }

    pub fn set_hash_checked(&mut self, checked: bool) {
        self.hash_checked = checked;
    }

    pub fn flush_disk(&mut self) -> Result<(), lt_storage::StorageError> {
        self.download.lock().unwrap().flush_disk()
    }

    pub fn to_resume_data(&self) -> ResumeData {
        self.download.lock().unwrap().to_resume_data()
    }

    pub fn apply_resume(&mut self, resume: &ResumeData) -> Result<usize, String> {
        let mut engine = self.download.lock().unwrap();
        let result = engine.apply_resume(resume);
        match &result {
            Ok(_) => {
                // Trust only what the engine accepted. Pieces that resume
                // validation marked for a recheck keep this false until the
                // pass finishes.
                self.hash_checked = engine.is_hash_checked();
                self.hash_failed = engine.is_hash_failed();
            }
            Err(_) => {
                self.hash_checked = false;
                self.hash_failed = false;
            }
        }
        result
    }

    /// Rehashes the download, returning a diagnostic when it is not legal.
    pub fn try_check_hash(&mut self) -> Result<(), String> {
        self.try_check_hash_internal()
    }

    /// Runs the final hash check used on completion.
    ///
    /// A completed download whose files no longer match the torrent is left
    /// incomplete; the caller distinguishes this case with
    /// [`Download::is_hash_failed`] and emits `hash_final_failed`.
    pub fn try_check_hash_final(&mut self) -> Result<(), String> {
        self.try_check_hash_internal()
    }

    fn try_check_hash_internal(&mut self) -> Result<(), String> {
        if !self.is_open {
            return Err("download is not open".into());
        }
        if self.is_hash_checking() {
            return Err("download is already hash checking".into());
        }

        let result = {
            let mut engine = self.download.lock().unwrap();
            engine.recheck()
        };

        match result {
            Ok(result) => {
                let engine = self.download.lock().unwrap();
                self.hash_checked = engine.is_hash_checked() && !result.cancelled;
                self.hash_failed = engine.is_hash_failed() || result.cancelled;
                Ok(())
            }
            Err(err) => {
                self.hash_checked = false;
                self.hash_failed = true;
                Err(err)
            }
        }
    }

    pub fn check_hash(&mut self) {
        let _ = self.try_check_hash();
    }

    pub fn check_hash_final(&mut self) {
        let _ = self.try_check_hash_final();
    }

    pub fn is_hash_failed(&self) -> bool {
        self.hash_failed
    }

    pub fn set_hash_failed(&mut self, val: bool) {
        self.hash_failed = val;
    }

    /// Pieces that resume validation queued for a partial recheck.
    pub fn pending_hash_pieces(&self) -> Vec<u32> {
        self.download.lock().unwrap().pending_hash_pieces().to_vec()
    }

    /// True while a resume-triggered recheck has not completed.
    pub fn has_pending_hash_pieces(&self) -> bool {
        if self.is_hash_checking() {
            return false;
        }
        self.download.lock().unwrap().has_pending_hash_pieces()
    }

    /// Takes the pieces queued by resume validation for an explicit recheck.
    pub fn take_pending_hash_pieces(&mut self) -> Vec<u32> {
        self.download.lock().unwrap().take_pending_hash_pieces()
    }

    /// Rechecks only the pieces selected by resume validation.
    pub fn recheck_pending_pieces(&mut self) -> Result<(), String> {
        let pending = self.take_pending_hash_pieces();
        if pending.is_empty() {
            let engine = self.download.lock().unwrap();
            self.hash_checked = engine.is_hash_checked();
            self.hash_failed = engine.is_hash_failed();
            return Ok(());
        }

        let result = {
            let mut engine = self.download.lock().unwrap();
            engine.recheck_pieces(&pending)
        };

        match result {
            Ok(result) => {
                let engine = self.download.lock().unwrap();
                self.hash_checked = engine.is_hash_checked() && !result.cancelled;
                self.hash_failed = engine.is_hash_failed() || result.cancelled;
                Ok(())
            }
            Err(err) => {
                self.hash_checked = false;
                self.hash_failed = true;
                Err(err)
            }
        }
    }

    pub fn state_changed(&self) -> u64 {
        if self.is_hash_checking() {
            return 0;
        }
        self.download.lock().unwrap().state_changed()
    }

    pub fn set_state_changed(&mut self, value: u64) {
        self.download.lock().unwrap().set_state_changed(value);
    }

    pub fn state_counter(&self) -> u64 {
        if self.is_hash_checking() {
            return 0;
        }
        self.download.lock().unwrap().state_counter()
    }

    pub fn set_state_counter(&mut self, value: u64) {
        self.download.lock().unwrap().set_state_counter(value);
    }

    pub fn timestamp_started(&self) -> u64 {
        if self.is_hash_checking() {
            return 0;
        }
        self.download.lock().unwrap().timestamp_started()
    }

    pub fn set_timestamp_started(&mut self, value: u64) {
        self.download.lock().unwrap().set_timestamp_started(value);
    }

    pub fn timestamp_finished(&self) -> u64 {
        if self.is_hash_checking() {
            return 0;
        }
        self.download.lock().unwrap().timestamp_finished()
    }

    pub fn set_timestamp_finished(&mut self, value: u64) {
        self.download.lock().unwrap().set_timestamp_finished(value);
    }

    pub fn load_date(&self) -> u64 {
        if self.is_hash_checking() {
            return 0;
        }
        self.download.lock().unwrap().load_date()
    }

    pub fn set_load_date(&mut self, value: u64) {
        self.download.lock().unwrap().set_load_date(value);
    }

    pub fn is_erasing(&self) -> bool {
        self.erasing
    }

    pub fn set_erasing(&mut self, val: bool) {
        self.erasing = val;
    }

    pub fn bencode(&self) -> Option<&Object> {
        self.bencode.as_ref()
    }

    pub fn bencode_mut(&mut self) -> Option<&mut Object> {
        self.bencode.as_mut()
    }

    pub fn set_bencode(&mut self, bencode: Object) {
        self.bencode = Some(bencode);
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn set_message(&mut self, msg: impl Into<String>) {
        self.message = msg.into();
    }

    pub fn priority(&self) -> u32 {
        self.priority
    }

    pub fn set_priority(&mut self, priority: u32) {
        self.priority = priority;
    }

    pub fn resume_flags(&self) -> u32 {
        self.resume_flags
    }

    pub fn set_resume_flags(&mut self, flags: u32) {
        self.resume_flags = flags;
    }

    pub fn group(&self) -> u32 {
        self.group
    }

    pub fn set_group(&mut self, group: u32) {
        self.group = group;
    }

    pub fn directory(&self) -> &str {
        &self.directory
    }

    pub fn set_directory(&mut self, dir: impl Into<String>) {
        self.directory = dir.into();
        self.download
            .lock()
            .unwrap()
            .set_save_directory(&self.directory);
    }

    pub fn has_existing_files(&self) -> bool {
        self.download.lock().unwrap().has_existing_files()
    }

    pub fn queue_recheck_if_files_exist(&mut self) -> bool {
        self.download.lock().unwrap().queue_recheck_if_files_exist()
    }

    pub fn throttle_name(&self) -> &str {
        &self.throttle_name
    }

    pub fn set_throttle_name(&mut self, name: impl Into<String>) {
        self.throttle_name = name.into();
    }

    pub fn rate_up(&self) -> u64 {
        if self.is_hash_checking() {
            return 0;
        }
        self.download.lock().unwrap().rate_up()
    }

    pub fn rate_down(&self) -> u64 {
        if self.is_hash_checking() {
            return 0;
        }
        self.download.lock().unwrap().rate_down()
    }

    pub fn uploaded_bytes(&self) -> u64 {
        if self.is_hash_checking() {
            return 0;
        }
        self.download.lock().unwrap().uploaded_bytes()
    }

    pub fn downloaded_bytes(&self) -> u64 {
        if self.is_hash_checking() {
            return 0;
        }
        self.download.lock().unwrap().downloaded_bytes()
    }

    /// Opens the download. Idempotent; emits no events itself.
    pub fn open(&mut self) -> Result<(), String> {
        if self.is_open && !self.is_closed() {
            return Ok(());
        }

        self.download.lock().unwrap().open()?;
        self.is_open = true;
        Ok(())
    }

    /// Starts an open download.
    ///
    /// Closed downloads are rejected (silently, matching the legacy engine
    /// entry point). `DownloadList::start`/`d.start` open first when needed.
    pub fn start(&mut self) {
        // rTorrent's `d.start` clears the sticky hash-failure flag before
        // asking the scheduler to start the download.
        self.hash_failed = false;
        let _ = self.try_start();
    }

    /// Starts an already-open download.
    ///
    /// Fails while the download is closed, hash checking, has failed hashing,
    /// or is in an error state. Repeated starts are no-ops.
    pub fn try_start(&mut self) -> Result<(), String> {
        if !self.is_open {
            return Err("download is closed".into());
        }
        if self.hash_failed {
            return Err("download hash check failed".into());
        }
        if self.erasing {
            return Err("download is being erased".into());
        }

        let mut engine = self.download.lock().unwrap();
        let had_pending_recheck = engine.has_pending_hash_pieces();
        match engine.try_start() {
            Ok(()) => {
                self.hash_checked = engine.is_hash_checked();
                if had_pending_recheck {
                    // Only a just-finished deferred recheck may publish a new
                    // per-piece failure result. A normal start keeps the
                    // caller-cleared sticky flag.
                    self.hash_failed = engine.is_hash_failed();
                }
                if self.connection_seed == "initial_seed" {
                    engine.start_initial_seeding();
                }
                Ok(())
            }
            Err(err) => {
                // A deferred resume recheck may have run and failed before
                // returning the start error; publish its result.
                self.hash_checked = engine.is_hash_checked();
                if had_pending_recheck {
                    self.hash_failed = engine.is_hash_failed();
                }
                Err(err)
            }
        }
    }

    /// Stops the download. Idempotent.
    pub fn stop(&mut self) {
        let _ = self.try_stop();
    }

    pub fn try_stop(&mut self) -> Result<(), String> {
        self.download.lock().unwrap().try_stop()
    }

    pub fn pause(&mut self) {
        self.download.lock().unwrap().pause();
    }

    pub fn resume(&mut self) {
        self.download.lock().unwrap().resume();
    }

    /// Stops and closes the download. Idempotent.
    pub fn close(&mut self) {
        let _ = self.download.lock().unwrap().close();
        self.is_open = false;
        self.hash_checked = false;
    }

    /// Closes files/network state directly without a tracker stop announce.
    pub fn close_directly(&mut self) {
        let _ = self.download.lock().unwrap().close_directly();
        self.is_open = false;
        self.hash_checked = false;
    }

    /// Releases runtime state while the download is being erased.
    pub fn erase(&mut self) {
        self.erasing = true;
        self.download.lock().unwrap().erase();
        self.is_open = false;
        self.hash_checked = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(name: &str) -> TorrentMetadata {
        TorrentMetadata {
            info_hash: HashString::from_hex("4141414141414141414141414141414141414141").unwrap(),
            name: name.into(),
            piece_length: 16384,
            pieces: vec![0u8; 20],
            num_pieces: 1,
            total_size: 16384,
            is_multi_file: false,
            files: Vec::new(),
            announce: None,
            announce_list: Vec::new(),
            is_private: false,
            is_meta_download: false,
        }
    }

    #[test]
    fn try_start_rejects_closed_download() {
        let mut download = Download::from_metadata(metadata("closed"));
        download.close();

        assert!(download.is_closed());
        assert!(!download.is_open());
        assert!(download.try_start().is_err());

        download.open().unwrap();
        assert!(!download.is_closed());
        download.try_start().unwrap();
        assert!(download.is_active());
    }

    #[test]
    fn lifecycle_counters_and_timestamps_are_exposed() {
        let mut download = Download::from_metadata(metadata("timestamps"));

        download.open().unwrap();
        download.start();
        assert!(download.timestamp_started() > 0);

        let started_counter = download.state_counter();
        download.start();
        assert_eq!(started_counter, download.state_counter());

        download.set_timestamp_started(123);
        download.set_timestamp_finished(456);
        download.set_state_changed(789);
        download.set_state_counter(42);
        download.set_load_date(1000);

        assert_eq!(download.timestamp_started(), 123);
        assert_eq!(download.timestamp_finished(), 456);
        assert_eq!(download.state_changed(), 789);
        assert_eq!(download.state_counter(), 42);
        assert_eq!(download.load_date(), 1000);
    }

    #[test]
    fn stop_is_idempotent() {
        let mut download = Download::from_metadata(metadata("idempotent"));
        download.open().unwrap();
        download.try_start().unwrap();

        download.try_stop().unwrap();
        let counter = download.state_counter();
        download.try_stop().unwrap();

        assert_eq!(counter, download.state_counter());
        assert!(download.is_stopped());
    }
}
