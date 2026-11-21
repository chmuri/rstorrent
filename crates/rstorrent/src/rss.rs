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
            let result = self
                .add_torrent(&matched.item, &matched.action, Some(matched.rule_id))
                .await;
            self.rss
                .record_download(matched.item.id, Some(matched.rule_id), result);
        }
    }

    /// Evaluates any unprocessed feed items for a specific feed against current rules
    /// and auto-downloads matches.
    pub async fn apply_unprocessed_feed(&self, feed_id: FeedId) {
        let matches = self.rss.evaluate_unprocessed_for_feed(feed_id);
        for matched in &matches {
            let result = self
                .add_torrent(&matched.item, &matched.action, Some(matched.rule_id))
                .await;
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
        let result = self.add_torrent(&item, &action, rule_id).await;
        self.rss.record_download(item_id, rule_id, result.clone());
        Ok(result)
    }

    async fn apply_matches(&self, outcome: &RefreshOutcome) {
        for matched in &outcome.matches {
            let result = self
                .add_torrent(&matched.item, &matched.action, Some(matched.rule_id))
                .await;
            self.rss
                .record_download(matched.item.id, Some(matched.rule_id), result);
        }
    }

    /// Loads the item's torrent (magnet or fetched `.torrent` bytes), applies
    /// the rule action and inserts it into the download list. Runs after the
    /// manager lock is released; every lock section here is short and synchronous.
    async fn add_torrent(
        &self,
        item: &FeedItem,
        action: &DownloadAction,
        rule_id: Option<u64>,
    ) -> DownloadResult {
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
            if let Err(err) = tokio::fs::create_dir_all(path).await {
                tracing::warn!(path = %path, error = %err, "Could not create RSS save directory");
            }
            download.set_directory(path.as_str());
        }
        if let Some(label) = &action.label {
            // d.custom1 is custom field index 1 (the API is 1-based).
            download.set_custom_field(1, label.as_str());
        }
        // Rules-engine bridge (plan §6.10): provenance in the persisted custom
        // map plus tags / throttle / per-torrent seed-limit overrides.
        download.custom_set("rss.feed_id", item.feed_id.to_string());
        if let Some(rule_id) = rule_id {
            download.custom_set("rss.rule_id", rule_id.to_string());
        }
        download.custom_set("rss.item_id", item.id.to_string());
        for tag in &action.tags {
            download.add_tag(tag);
        }
        if let Some(throttle) = &action.throttle_name {
            download.set_throttle_name(throttle.as_str());
        }
        if let Some(limits) = &action.seed_limits {
            *download.seed_overrides_mut() = rss_seed_limits_to_overrides(limits);
        }
        let hash = download.info_hash_hex();
        let download = Arc::new(RwLock::new(download));
        let inserted = {
            let mut list = self
                .download_list
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            list.insert(Arc::clone(&download))
        };
        if !inserted {
            return DownloadResult::Duplicate(hash);
        }
        if !action.paused {
            let list = self
                .download_list
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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

/// Maps the RSS-local seed-limit shape onto `rt_rules::TorrentOverrides`
/// (plan §6.10): `None` = inherit, `Some(0)` = unlimited, `Some(v)` = set.
fn rss_seed_limits_to_overrides(limits: &rt_rss::RssSeedLimits) -> rt_rules::TorrentOverrides {
    use rt_rules::{LimitValue, TorrentOverrides};
    fn f64_limit(v: Option<f64>) -> LimitValue<f64> {
        match v {
            None => LimitValue::Inherit,
            Some(x) if x <= 0.0 => LimitValue::Unlimited,
            Some(x) => LimitValue::Set(x),
        }
    }
    fn u64_limit(v: Option<u64>) -> LimitValue<u64> {
        match v {
            None => LimitValue::Inherit,
            Some(0) => LimitValue::Unlimited,
            Some(x) => LimitValue::Set(x),
        }
    }
    let action = limits.action.as_deref().and_then(parse_seed_action);
    if limits.action.is_some() && action.is_none() {
        tracing::warn!(
            action = ?limits.action,
            "RSS rule seed_limits.action is invalid; ignoring"
        );
    }
    TorrentOverrides {
        ratio: f64_limit(limits.ratio),
        seeding_time_secs: u64_limit(limits.seeding_time_secs),
        inactive_secs: u64_limit(limits.inactive_secs),
        action,
    }
}

/// Parses the wire/TOML seed-action string form (plan §6.2).
fn parse_seed_action(value: &str) -> Option<rt_rules::SeedAction> {
    use rt_rules::SeedAction;
    match value {
        "stop" => Some(SeedAction::Stop),
        "close" => Some(SeedAction::Close),
        "remove" => Some(SeedAction::Remove),
        "remove_with_data" => Some(SeedAction::RemoveWithData),
        "super_seed" => Some(SeedAction::SuperSeed),
        "notify" => Some(SeedAction::Notify),
        _ => {
            if let Some(name) = value.strip_prefix("throttle:") {
                Some(SeedAction::Throttle(name.to_string()))
            } else {
                value
                    .strip_prefix("command:")
                    .map(|cmd| SeedAction::Command(cmd.to_string()))
            }
        }
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
            tags: Vec::new(),
            throttle_name: None,
            seed_limits: None,
        }
    }

    #[tokio::test]
    async fn add_torrent_without_source_fails() {
        let dir = TempDir::new("no_source");
        let svc = service(&dir);
        let mut item = magnet_item(HASH);
        item.magnet = None;
        let result = svc.add_torrent(&item, &default_action(), None).await;
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
            tags: Vec::new(),
            throttle_name: None,
            seed_limits: None,
        };
        let item = magnet_item(HASH);

        let result = svc.add_torrent(&item, &action, Some(7)).await;
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
        let again = svc.add_torrent(&item, &action, Some(7)).await;
        assert!(matches!(again, DownloadResult::Duplicate(ref hash) if hash == HASH));
        assert_eq!(svc.download_list.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn magnet_add_starts_when_not_paused() {
        let dir = TempDir::new("magnet_start");
        let svc = service(&dir);
        let result = svc
            .add_torrent(&magnet_item(HASH), &default_action(), None)
            .await;
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

    #[tokio::test]
    async fn bridge_applies_provenance_tags_throttle_and_seed_limits() {
        let dir = TempDir::new("bridge");
        let svc = service(&dir);
        let action = DownloadAction {
            save_path: None,
            label: Some("tv".into()),
            paused: true,
            tags: vec!["rss".into(), "keep".into()],
            throttle_name: Some("seed".into()),
            seed_limits: Some(rt_rss::RssSeedLimits {
                ratio: Some(3.0),
                seeding_time_secs: Some(0), // unlimited
                inactive_secs: None,        // inherit
                action: Some("throttle:slow".into()),
            }),
        };
        let result = svc.add_torrent(&magnet_item(HASH), &action, Some(7)).await;
        assert!(matches!(result, DownloadResult::Added(_)));
        let download = svc.download_list.read().unwrap().find_hex(HASH).unwrap();
        let guard = download.read().unwrap();
        assert_eq!(guard.custom_get("rss.feed_id"), Some("1"));
        assert_eq!(guard.custom_get("rss.rule_id"), Some("7"));
        assert_eq!(guard.custom_get("rss.item_id"), Some("1"));
        assert!(guard.has_tag("rss"));
        assert!(guard.has_tag("keep"));
        assert_eq!(guard.throttle_name(), "seed");
        let overrides = guard.seed_overrides();
        assert_eq!(overrides.ratio, rt_rules::LimitValue::Set(3.0));
        assert_eq!(overrides.seeding_time_secs, rt_rules::LimitValue::Unlimited);
        assert_eq!(overrides.inactive_secs, rt_rules::LimitValue::Inherit);
        assert_eq!(
            overrides.action,
            Some(rt_rules::SeedAction::Throttle("slow".into()))
        );
    }

    #[tokio::test]
    async fn manual_download_records_provenance_without_rule() {
        let dir = TempDir::new("manual_prov");
        let svc = service(&dir);
        let result = svc
            .add_torrent(&magnet_item(HASH), &default_action(), None)
            .await;
        assert!(matches!(result, DownloadResult::Added(_)));
        let download = svc.download_list.read().unwrap().find_hex(HASH).unwrap();
        let guard = download.read().unwrap();
        assert_eq!(guard.custom_get("rss.feed_id"), Some("1"));
        assert_eq!(guard.custom_get("rss.rule_id"), None);
        assert_eq!(guard.custom_get("rss.item_id"), Some("1"));
    }

    #[test]
    fn seed_action_parsing() {
        assert_eq!(parse_seed_action("stop"), Some(rt_rules::SeedAction::Stop));
        assert_eq!(
            parse_seed_action("remove_with_data"),
            Some(rt_rules::SeedAction::RemoveWithData)
        );
        assert_eq!(
            parse_seed_action("throttle:seed"),
            Some(rt_rules::SeedAction::Throttle("seed".into()))
        );
        assert_eq!(
            parse_seed_action("command:d.stop="),
            Some(rt_rules::SeedAction::Command("d.stop=".into()))
        );
        assert_eq!(parse_seed_action("bogus"), None);
        assert_eq!(parse_seed_action("inherit"), None);
    }

    #[test]
    fn rss_seed_limits_mapping() {
        let overrides = rss_seed_limits_to_overrides(&rt_rss::RssSeedLimits {
            ratio: Some(0.0),
            seeding_time_secs: Some(60),
            inactive_secs: None,
            action: Some("bogus".into()),
        });
        assert_eq!(overrides.ratio, rt_rules::LimitValue::Unlimited);
        assert_eq!(overrides.seeding_time_secs, rt_rules::LimitValue::Set(60));
        assert_eq!(overrides.inactive_secs, rt_rules::LimitValue::Inherit);
        assert_eq!(overrides.action, None);
    }
}
