use std::sync::{Arc, RwLock};
use lt_common::hash_string::HashString;
use crate::download::Download;

pub type DownloadEventListener = Arc<dyn Fn(&str, &Arc<RwLock<Download>>) + Send + Sync>;

#[derive(Default)]
pub struct DownloadList {
    downloads: Vec<Arc<RwLock<Download>>>,
    listeners: Vec<DownloadEventListener>,
}

impl DownloadList {
    pub fn new() -> Self {
        Self {
            downloads: Vec::new(),
            listeners: Vec::new(),
        }
    }

    pub fn add_event_listener(&mut self, listener: DownloadEventListener) {
        self.listeners.push(listener);
    }

    fn emit(&self, event: &str, download: &Arc<RwLock<Download>>) {
        for listener in &self.listeners {
            listener(event, download);
        }
    }

    pub fn len(&self) -> usize {
        self.downloads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.downloads.is_empty()
    }

    pub fn all(&self) -> Vec<Arc<RwLock<Download>>> {
        self.downloads.clone()
    }

    pub fn clear(&mut self) {
        for d in &self.downloads {
            let mut d_guard = d.write().unwrap();
            d_guard.close();
        }
        self.downloads.clear();
    }

    pub fn find(&self, hash: &HashString) -> Option<Arc<RwLock<Download>>> {
        self.downloads
            .iter()
            .find(|d| d.read().unwrap().info_hash() == hash)
            .cloned()
    }

    pub fn find_hex(&self, hash: &str) -> Option<Arc<RwLock<Download>>> {
        if hash.len() != 40 {
            return None;
        }

        let key = HashString::from_hex(hash)?;
        self.find(&key)
    }

    pub fn insert(&mut self, download: Arc<RwLock<Download>>) -> bool {
        let hash = *download.read().unwrap().info_hash();
        if self.find(&hash).is_some() {
            return false;
        }

        self.downloads.push(Arc::clone(&download));
        self.emit("event.download.inserted", &download);
        true
    }

    pub fn erase(&mut self, hash: &HashString) -> Option<Arc<RwLock<Download>>> {
        let pos = self
            .downloads
            .iter()
            .position(|d| d.read().unwrap().info_hash() == hash)?;

        let download = self.downloads[pos].clone();
        let (already_erasing, was_active, was_open) = {
            let guard = download.read().unwrap();
            (guard.is_erasing(), guard.is_active(), guard.is_open())
        };

        // Re-entrant erase (e.g. from an event handler) must not emit a
        // second set of lifecycle events.
        if already_erasing {
            return None;
        }

        {
            let mut d_guard = download.write().unwrap();
            d_guard.set_erasing(true);
            d_guard.erase();
        }

        self.downloads.remove(pos);

        if was_active {
            self.emit("event.download.paused", &download);
        }
        if was_open {
            self.emit("event.download.closed", &download);
        }
        self.emit("event.download.erased", &download);
        Some(download)
    }

    pub fn erase_hex(&mut self, hash: &str) -> Option<Arc<RwLock<Download>>> {
        let key = HashString::from_hex(hash)?;
        self.erase(&key)
    }

    /// Opens a download, emitting `event.download.opened` only on the first
    /// closed -> open transition.
    pub fn open(&self, download: &Arc<RwLock<Download>>) {
        let was_open = download.read().unwrap().is_open();
        {
            let mut d_guard = download.write().unwrap();
            let _ = d_guard.open();
        }
        if !was_open {
            self.emit("event.download.opened", download);
        }
    }

    /// Closes a download, emitting `paused`/`closed` only for transitions that
    /// actually occurred.
    pub fn close(&self, download: &Arc<RwLock<Download>>) {
        let (was_active, was_open) = {
            let guard = download.read().unwrap();
            (guard.is_active(), guard.is_open())
        };
        {
            let mut d_guard = download.write().unwrap();
            d_guard.close();
        }
        if was_active {
            self.emit("event.download.paused", download);
        }
        if was_open {
            self.emit("event.download.closed", download);
        }
    }

    /// Force-closes files/network state without a tracker stop announce.
    pub fn close_directly(&self, download: &Arc<RwLock<Download>>) {
        let (was_active, was_open) = {
            let guard = download.read().unwrap();
            (guard.is_active(), guard.is_open())
        };
        {
            let mut d_guard = download.write().unwrap();
            d_guard.close_directly();
        }
        if was_active {
            self.emit("event.download.paused", download);
        }
        if was_open {
            self.emit("event.download.closed", download);
        }
    }

    /// Opens (if needed) and starts a download. Emits `resumed` for a
    /// stopped/paused -> active transition only.
    pub fn start(&self, download: &Arc<RwLock<Download>>) {
        let was_open = download.read().unwrap().is_open();
        if !was_open {
            self.open(download);
        }

        let (was_active, was_done, pending_recheck) = {
            let guard = download.read().unwrap();
            (
                guard.is_active(),
                guard.is_done(),
                guard.has_pending_hash_pieces(),
            )
        };

        if pending_recheck {
            // Resume validation deferred these pieces. Mirror the legacy
            // `hash_queue` event before the synchronous pass.
            self.emit("event.download.hash_queued", download);
        }

        {
            let mut d_guard = download.write().unwrap();
            d_guard.start();
        }

        let (is_active, is_done, hash_failed, hash_checked) = {
            let guard = download.read().unwrap();
            (
                guard.is_active(),
                guard.is_done(),
                guard.is_hash_failed(),
                guard.is_hash_checked(),
            )
        };

        if !was_active && is_active {
            self.emit("event.download.resumed", download);
        }

        if pending_recheck {
            if hash_failed || !hash_checked {
                self.emit("event.download.hash_failed", download);
            } else {
                self.emit("event.download.hash_done", download);
                if is_done && !was_done {
                    self.emit("event.download.finished", download);
                }
            }
        }
    }

    /// Strict start used by `d.try_start`: a closed download is rejected
    /// without emitting any event.
    pub fn try_start(&self, download: &Arc<RwLock<Download>>) -> Result<(), String> {
        let (was_active, was_done, hash_failed, erasing, pending_recheck) = {
            let guard = download.read().unwrap();
            (
                guard.is_active(),
                guard.is_done(),
                guard.is_hash_failed(),
                guard.is_erasing(),
                guard.has_pending_hash_pieces(),
            )
        };
        if hash_failed {
            return Err("download hash check failed".into());
        }
        if erasing {
            return Err("download is being erased".into());
        }

        if pending_recheck {
            self.emit("event.download.hash_queued", download);
        }

        let start_result = {
            let mut d_guard = download.write().unwrap();
            d_guard.try_start()
        };

        let (is_active, is_done, hash_failed, hash_checked) = {
            let guard = download.read().unwrap();
            (
                guard.is_active(),
                guard.is_done(),
                guard.is_hash_failed(),
                guard.is_hash_checked(),
            )
        };

        if start_result.is_ok() && !was_active && is_active {
            self.emit("event.download.resumed", download);
        }

        if pending_recheck {
            if hash_failed || !hash_checked {
                self.emit("event.download.hash_failed", download);
            } else {
                self.emit("event.download.hash_done", download);
                if is_done && !was_done {
                    self.emit("event.download.finished", download);
                }
            }
        }

        start_result
    }

    /// Pauses an active download, emitting `paused` only on a transition.
    pub fn pause(&self, download: &Arc<RwLock<Download>>) {
        let was_active = download.read().unwrap().is_active();
        {
            let mut d_guard = download.write().unwrap();
            d_guard.pause();
        }
        if was_active && download.read().unwrap().is_paused() {
            self.emit("event.download.paused", download);
        }
    }

    /// Resumes a paused/stopped download, opening it first when necessary.
    pub fn resume(&self, download: &Arc<RwLock<Download>>) {
        self.start(download);
    }

    /// Stops a download, emitting `paused` only when it was active.
    pub fn stop(&self, download: &Arc<RwLock<Download>>) {
        let _ = self.try_stop(download);
    }

    pub fn try_stop(&self, download: &Arc<RwLock<Download>>) -> Result<(), String> {
        let was_active = download.read().unwrap().is_active();
        {
            let mut d_guard = download.write().unwrap();
            d_guard.try_stop()?;
        }
        if was_active {
            self.emit("event.download.paused", download);
        }
        Ok(())
    }

    /// rTorrent's `d.try_close`: close only when the download is not being
    /// erased/ignored.
    pub fn try_close(&self, download: &Arc<RwLock<Download>>) {
        if download.read().unwrap().is_erasing() {
            return;
        }
        self.close(download);
    }

    pub fn check_hash(&self, download: &Arc<RwLock<Download>>) {
        self.check_hash_internal(download, false);
    }

    /// Hash check run when a download reaches completion.
    ///
    /// Mirrors `variable_hashing_last`: bad chunks emit
    /// `event.download.hash_final_failed` instead of `hash_done`.
    pub fn check_hash_final(&self, download: &Arc<RwLock<Download>>) {
        self.check_hash_internal(download, true);
    }

    fn check_hash_internal(
        &self,
        download: &Arc<RwLock<Download>>,
        final_check: bool,
    ) {
        let (already_checking, was_done, is_open, erasing) = {
            let guard = download.read().unwrap();
            (
                guard.is_hash_checking(),
                guard.is_done(),
                guard.is_open(),
                guard.is_erasing(),
            )
        };
        if already_checking || erasing {
            return;
        }
        if !is_open {
            // Hashing needs the engine open, but rTorrent does not emit an
            // `opened` event as part of a hash queue.
            let _ = download.write().unwrap().open();
        }

        {
            let mut guard = download.write().unwrap();
            guard.set_hash_checking(true);
            guard.set_hash_checked(false);
            guard.set_hash_failed(false);
        }

        self.emit("event.download.hash_queued", download);

        let engine = download.read().unwrap().engine_download();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let prepared = {
            let mut eng = engine.lock().unwrap();
            eng.prepare_recheck(None, cancel)
        };

        let result = match prepared {
            Ok(prep) => {
                let res = prep.execute();
                let mut eng = engine.lock().unwrap();
                eng.finish_recheck(prep, res)
            }
            Err(err) => Err(err),
        };

        let (hash_checked, is_done, is_checking) = {
            let (is_checked, is_failed, done) = {
                let engine = download.read().unwrap().engine_download();
                let eng = engine.lock().unwrap();
                let cancelled = result.as_ref().map(|r| r.cancelled).unwrap_or(true);
                (
                    eng.is_hash_checked() && !cancelled,
                    eng.is_hash_failed() || cancelled,
                    eng.is_done(),
                )
            };
            let mut guard = download.write().unwrap();
            guard.set_hash_checking(false);
            guard.set_hash_checked(is_checked);
            guard.set_hash_failed(is_failed);
            (
                is_checked,
                done,
                guard.is_hash_checking(),
            )
        };

        if result.is_err() || is_checking || !hash_checked {
            // The pass could not complete (I/O error, cancellation or an
            // empty/unavailable file set). The legacy engine marks the
            // download as failed and skips `hash_done`.
            download.write().unwrap().set_hash_failed(true);
            self.emit("event.download.hash_failed", download);
            return;
        }

        if final_check && !is_done {
            // Completion hash found bad chunks. The download stays
            // incomplete so the scheduler can fetch them again.
            download
                .write()
                .unwrap()
                .set_message("Hash check on download completion found bad chunks.");
            self.emit("event.download.hash_final_failed", download);
            return;
        }

        // A normal rehash with bad chunks still completed. `is_hash_failed`
        // records the per-piece failures, matching the legacy `hash_done`
        // path for `variable_hashing_rehash`.
        self.emit("event.download.hash_done", download);

        if is_done && !was_done {
            self.emit("event.download.finished", download);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_engine::constructor::TorrentMetadata;
    use std::sync::Mutex;

    fn download(name: &str) -> Arc<RwLock<Download>> {
        let metadata = TorrentMetadata {
            info_hash: HashString::from_hex(
                "4242424242424242424242424242424242424242",
            )
            .unwrap(),
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
        };
        Arc::new(RwLock::new(Download::from_metadata(metadata)))
    }

    #[test]
    fn lifecycle_events_are_not_duplicated() {
        let mut list = DownloadList::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        list.add_event_listener(Arc::new(move |event, _| {
            sink.lock().unwrap().push(event.to_string());
        }));

        let dl = download("events");
        let hash = *dl.read().unwrap().info_hash();
        assert!(list.insert(Arc::clone(&dl)));

        list.open(&dl);
        list.open(&dl);
        list.start(&dl);
        list.start(&dl);
        list.stop(&dl);
        list.stop(&dl);
        list.close(&dl);
        list.close(&dl);
        list.erase(&hash);

        let expected: Vec<String> = [
            "event.download.inserted",
            "event.download.opened",
            "event.download.resumed",
            "event.download.paused",
            "event.download.closed",
            "event.download.erased",
        ]
        .iter()
        .map(|event| event.to_string())
        .collect();

        let events = events.lock().unwrap();
        assert_eq!(events.as_slice(), expected.as_slice());
    }

    #[test]
    fn pause_and_resume_emit_once() {
        let mut list = DownloadList::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        list.add_event_listener(Arc::new(move |event, _| {
            sink.lock().unwrap().push(event.to_string());
        }));

        let dl = download("pause-resume");
        list.insert(Arc::clone(&dl));
        list.start(&dl);

        list.pause(&dl);
        list.pause(&dl);
        assert!(dl.read().unwrap().is_paused());

        list.resume(&dl);
        list.resume(&dl);
        assert!(dl.read().unwrap().is_active());

        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "event.download.paused")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "event.download.resumed")
                .count(),
            2
        );
    }

    #[test]
    fn try_start_rejects_closed_download_without_event() {
        let mut list = DownloadList::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        list.add_event_listener(Arc::new(move |event, _| {
            sink.lock().unwrap().push(event.to_string());
        }));

        let dl = download("closed");
        list.insert(Arc::clone(&dl));

        assert!(list.try_start(&dl).is_err());
        assert!(!dl.read().unwrap().is_open());

        list.open(&dl);
        list.try_start(&dl).unwrap();
        assert!(dl.read().unwrap().is_active());

        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "event.download.resumed")
                .count(),
            1
        );
        assert!(events
            .iter()
            .any(|event| event.as_str() == "event.download.opened"));
    }
}
