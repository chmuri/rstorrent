//! Daemon glue for the RSS subsystem: hands matched feed items to the
//! download engine, records the outcomes in the RSS store, and runs the
//! periodic refresh worker. The domain logic (fetching, parsing, rules,
//! persistence) lives in `rt-rss`; this module only bridges it to the
//! session-backed download list.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lt_common::Object;
use rt_core::session::SessionManager;
use rt_core::{DownloadFactory, DownloadList};
use rt_rpc::{CommandMap, Target};
use rt_rss::manager::RssManager;
use rt_rss::{
    fetch, DownloadAction, DownloadResult, FeedId, FeedItem, RefreshOutcome, RssConfig, RssError,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::control::Control;

/// How often the background worker wakes up to check which feeds are due.
/// Each feed carries its own (jittered) refresh interval; this tick only
/// drives the scheduler, it is not the fetch interval itself.
const WORKER_TICK: Duration = Duration::from_secs(30);

pub struct RssService {
    pub rss: Arc<RssManager>,
    download_list: Arc<RwLock<DownloadList>>,
    session_manager: Arc<Mutex<SessionManager>>,
    command_map: Option<Arc<RwLock<CommandMap>>>,
    client: reqwest::Client,
}

impl RssService {
    pub fn new(
        store_path: PathBuf,
        config: RssConfig,
        download_list: Arc<RwLock<DownloadList>>,
        session_manager: Arc<Mutex<SessionManager>>,
        command_map: Option<Arc<RwLock<CommandMap>>>,
    ) -> Result<Arc<Self>, RssError> {
        let client = fetch::build_client(config.request_timeout_secs, 5);
        let manager = RssManager::new(store_path, config)?;
        Ok(Arc::new(Self {
            rss: Arc::new(manager),
            download_list,
            session_manager,
            command_map,
            client,
        }))
    }

    /// Refreshes one feed and auto-adds every rule match. Works for disabled
    /// feeds too (explicit refresh is a manual override).
    pub async fn refresh_feed(&self, id: FeedId) -> Result<RefreshOutcome, RssError> {
        let outcome = self.rss.refresh_feed(id).await?;
        self.apply_matches(&outcome).await;
        self.apply_unprocessed_feed(id).await;
        Ok(outcome)
    }

    /// Refreshes every enabled feed whose interval has elapsed at `now`
    /// (unix seconds) and auto-adds the matches.
    pub async fn refresh_due(&self, now: u64) -> Vec<RefreshOutcome> {
        let outcomes = self.rss.refresh_due(now).await;
        for outcome in &outcomes {
            self.apply_matches(outcome).await;
            self.apply_unprocessed_feed(outcome.feed_id).await;
        }
        outcomes
    }

    /// Refreshes every enabled feed regardless of its schedule.
    pub async fn refresh_all(&self) -> Vec<RefreshOutcome> {
        let outcomes = self.rss.refresh_all().await;
        for outcome in &outcomes {
            self.apply_matches(outcome).await;
            self.apply_unprocessed_feed(outcome.feed_id).await;
        }
        outcomes
    }

    /// Evaluates any unprocessed feed items across all feeds against current rules
    /// and auto-downloads matches.
    pub async fn apply_unprocessed(&self) {
        let matches = self.rss.evaluate_unprocessed();
        for matched in &matches {
            let result = self.add_torrent(&matched.item, &matched.action).await;
            self.rss
                .record_download(matched.item.id, Some(matched.rule_id), result);
        }
    }

    /// Evaluates any unprocessed feed items for a specific feed against current rules
    /// and auto-downloads matches.
    pub async fn apply_unprocessed_feed(&self, feed_id: FeedId) {
        let matches = self.rss.evaluate_unprocessed_for_feed(feed_id);
        for matched in &matches {
            let result = self.add_torrent(&matched.item, &matched.action).await;
            self.rss
                .record_download(matched.item.id, Some(matched.rule_id), result);
        }
    }

    fn default_directory(&self) -> Option<String> {
        let map_arc = self.command_map.as_ref()?;
        let map = map_arc.read().unwrap_or_else(|e| e.into_inner());
        map.call_command(
            "directory.default",
            &Object::create_empty(),
            &Target::None,
            false,
        )
        .ok()
        .and_then(|obj| obj.as_str().map(|s| s.to_string()).ok())
        .filter(|d| !d.is_empty())
    }

    /// Manual download of a single item. Rule matching still applies (with
    /// cooldowns ignored), but an item without a matching rule is added with
    /// the default action. Already-processed items are rejected.
    pub async fn download_item(&self, item_id: u64) -> Result<DownloadResult, RssError> {
        let (item, action, rule_id) = self.rss.take_manual(item_id)?;
        let result = self.add_torrent(&item, &action).await;
        self.rss.record_download(item_id, rule_id, result.clone());
        Ok(result)
    }

    async fn apply_matches(&self, outcome: &RefreshOutcome) {
        for matched in &outcome.matches {
            let result = self.add_torrent(&matched.item, &matched.action).await;
            self.rss
                .record_download(matched.item.id, Some(matched.rule_id), result);
        }
    }

    /// Loads the item's torrent (magnet or fetched `.torrent` bytes), applies
    /// the rule action and inserts it into the download list. Runs after the
    /// manager lock is released; every lock section here is short and synchronous.
    async fn add_torrent(&self, item: &FeedItem, action: &DownloadAction) -> DownloadResult {
        let mut factory = DownloadFactory::new();
        if let Some(magnet) = &item.magnet {
            if let Err(err) = factory.load_magnet(magnet) {
                return DownloadResult::Failed(err.to_string());
            }
        } else if let Some(url) = &item.torrent_url {
            let max_bytes = self.rss.config().max_response_bytes;
            let bytes = match fetch::fetch_torrent_bytes(&self.client, url, max_bytes).await {
                Ok(bytes) => bytes,
                Err(err) => return DownloadResult::Failed(err.to_string()),
            };
            if let Err(err) = factory.load_raw_data(&bytes) {
                return DownloadResult::Failed(err.to_string());
            }
        } else {
            return DownloadResult::Failed("no torrent source".to_string());
        }

        let mut download = match factory.create_download() {
            Ok(download) => download,
            Err(err) => return DownloadResult::Failed(err.to_string()),
        };
        let save_path = action
            .save_path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .or_else(|| self.default_directory());
        if let Some(path) = &save_path {
            if let Err(err) = std::fs::create_dir_all(path) {
                tracing::warn!(path = %path, error = %err, "Could not create RSS save directory");
            }
            download.set_directory(path.as_str());
        }
        if let Some(label) = &action.label {
            // d.custom1 is custom field index 1 (the API is 1-based).
            download.set_custom_field(1, label.as_str());
        }
        let hash = download.info_hash_hex();
        let download = Arc::new(RwLock::new(download));
        let inserted = {
            let mut list = self
                .download_list
                .write()
                .unwrap_or_else(|e| e.into_inner());
            list.insert(Arc::clone(&download))
        };
        if !inserted {
            return DownloadResult::Duplicate(hash);
        }
        if !action.paused {
            let list = self.download_list.read().unwrap_or_else(|e| e.into_inner());
            list.start(&download);
        }
        // Persist through the same writer-snapshot path as d.save_full_session
        // so the new torrent survives a restart even if the daemon dies before
        // the next periodic checkpoint.
        if let Err(err) = Control::save_session_state_with_abort(
            Arc::clone(&self.session_manager),
            Arc::clone(&self.download_list),
            None,
        ) {
            tracing::warn!(error = %err, "Could not save session after RSS download");
        }
        DownloadResult::Added(hash)
    }
}

/// Spawns the periodic RSS refresh worker. It ticks every 30 seconds,
/// refreshes due feeds while RSS is enabled, and exits when the shutdown
/// watch fires (or its sender is dropped).
pub fn spawn_worker(
    service: Arc<RssService>,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if *shutdown.borrow_and_update() {
            return;
        }
        let mut interval = tokio::time::interval(WORKER_TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!("RSS refresh worker started");
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if service.rss.config().enabled {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let outcomes = service.refresh_due(now).await;
                        let matches: usize =
                            outcomes.iter().map(|outcome| outcome.matches.len()).sum();
                        tracing::debug!(
                            feeds = outcomes.len(),
                            matches,
                            "RSS refresh tick"
                        );
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow_and_update() {
                        break;
                    }
                }
            }
        }
        tracing::info!("RSS refresh worker stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("rt_rss_service_unit_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn service(dir: &TempDir) -> Arc<RssService> {
        RssService::new(
            dir.path.join("rss.json"),
            RssConfig::default(),
            Arc::new(RwLock::new(DownloadList::new())),
            Arc::new(Mutex::new(SessionManager::new())),
            None,
        )
        .unwrap()
    }

    fn magnet_item(hash: &str) -> FeedItem {
        FeedItem {
            id: 1,
            feed_id: 1,
            guid: format!("guid-{hash}"),
            title: "Magnet Item".to_string(),
            link: None,
            torrent_url: None,
            magnet: Some(format!("magnet:?xt=urn:btih:{hash}&dn=test")),
            info_hash: Some(hash.to_string()),
            size_bytes: None,
            published_at: None,
            fetched_at: 0,
            download: None,
        }
    }

    fn default_action() -> DownloadAction {
        DownloadAction {
            save_path: None,
            label: None,
            paused: false,
        }
    }

    #[tokio::test]
    async fn add_torrent_without_source_fails() {
        let dir = TempDir::new("no_source");
        let svc = service(&dir);
        let mut item = magnet_item(HASH);
        item.magnet = None;
        let result = svc.add_torrent(&item, &default_action()).await;
        assert!(matches!(result, DownloadResult::Failed(ref msg) if msg == "no torrent source"));
        assert_eq!(svc.download_list.read().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn magnet_add_applies_action_and_dedups() {
        let dir = TempDir::new("magnet_action");
        let svc = service(&dir);
        let save_path = dir.path.join("data");
        let action = DownloadAction {
            save_path: Some(save_path.to_string_lossy().into_owned()),
            label: Some("rss".to_string()),
            paused: true,
        };
        let item = magnet_item(HASH);

        let result = svc.add_torrent(&item, &action).await;
        assert!(matches!(result, DownloadResult::Added(ref hash) if hash == HASH));
        assert!(save_path.exists());
        {
            let list = svc.download_list.read().unwrap();
            assert_eq!(list.len(), 1);
            let download = list.find_hex(HASH).unwrap();
            let guard = download.read().unwrap();
            assert_eq!(guard.directory(), save_path.to_str().unwrap());
            assert_eq!(guard.custom_field(1), "rss");
            assert!(!guard.is_active());
        }

        // The same info hash a second time is a duplicate, not a second entry.
        let again = svc.add_torrent(&item, &action).await;
        assert!(matches!(again, DownloadResult::Duplicate(ref hash) if hash == HASH));
        assert_eq!(svc.download_list.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn magnet_add_starts_when_not_paused() {
        let dir = TempDir::new("magnet_start");
        let svc = service(&dir);
        let result = svc.add_torrent(&magnet_item(HASH), &default_action()).await;
        assert!(matches!(result, DownloadResult::Added(_)));
        let download = svc.download_list.read().unwrap().find_hex(HASH).unwrap();
        assert!(download.read().unwrap().is_active());
    }

    #[tokio::test]
    async fn download_item_unknown_id_is_not_found() {
        let dir = TempDir::new("manual_missing");
        let svc = service(&dir);
        assert!(matches!(
            svc.download_item(42).await,
            Err(RssError::NotFound("item", 42))
        ));
    }
}
