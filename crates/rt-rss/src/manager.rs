//! Orchestration of the RSS subsystem: feed/rule CRUD, scheduled and forced
//! refreshes, rule matching of new items, manual download hand-off and
//! history/cooldown bookkeeping.
//!
//! Locking discipline: the store and config mutexes are only held for short
//! synchronous sections and are never nested; HTTP fetches and XML parsing
//! always run outside any lock, so no guard is ever held across an `.await`.
//! The manager matches items but never downloads: the service layer consumes
//! [`RefreshOutcome::matches`] and reports results via [`record_download`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::RssError;
use crate::fetch::{self, FeedResponse};
use crate::model::{
    DownloadAction, DownloadRecord, DownloadResult, DownloadStatus, Feed, FeedId, FeedItem,
    FeedPatch, HistoryEntry, ItemFilter, RefreshOutcome, RssConfig, RssRule, RuleId,
};
use crate::parse;
use crate::rules;
use crate::store::{unix_secs, Store};

/// Shared handle to the RSS state. Cheap to clone via `Arc` in the service
/// layer; all interior state is mutex-protected.
pub struct RssManager {
    store: Arc<Mutex<Store>>,
    store_path: PathBuf,
    client: reqwest::Client,
    config: Arc<Mutex<RssConfig>>,
}

impl RssManager {
    /// Loads (or initializes) the store at `store_path` and builds the HTTP
    /// client from `config`. A corrupt store file is quarantined by the
    /// store layer and replaced with fresh state.
    pub fn new(store_path: PathBuf, config: RssConfig) -> Result<Self, RssError> {
        let store = Store::load(&store_path)?;
        let client = fetch::build_client(config.request_timeout_secs, 5);
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            store_path,
            client,
            config: Arc::new(Mutex::new(config)),
        })
    }

    pub fn config(&self) -> RssConfig {
        self.lock_config().clone()
    }

    /// Replaces the in-memory config and persists the store. The config
    /// itself is not part of the `rss.json` schema; its durable home is the
    /// daemon configuration handled by the service layer.
    pub fn set_config(&self, config: RssConfig) {
        *self.lock_config() = config;
        if let Err(err) = self.persist() {
            tracing::warn!(error = %err, "Failed to persist RSS store after config change");
        }
    }

    pub fn feeds(&self) -> Vec<Feed> {
        self.lock_store().feeds.clone()
    }

    /// Registers a feed after URL validation. `interval` of `None` stores
    /// `0`, meaning "use the global default". Duplicate URLs are rejected.
    pub fn add_feed(
        &self,
        url: &str,
        alias: Option<String>,
        interval: Option<u64>,
    ) -> Result<Feed, RssError> {
        fetch::validate_url(url).map_err(|_| RssError::InvalidUrl(url.to_string()))?;
        let mut store = self.lock_store();
        if store.feeds.iter().any(|feed| feed.url == url) {
            return Err(RssError::DuplicateFeed(url.to_string()));
        }
        let feed = Feed {
            id: 0,
            url: url.to_string(),
            alias,
            enabled: true,
            refresh_interval_secs: interval.unwrap_or_default(),
            last_fetch_at: None,
            last_success_at: None,
            last_error: None,
            etag: None,
            last_modified: None,
            created_at: unix_secs(),
        };
        let feed = store.insert_feed(feed);
        store.save(&self.store_path)?;
        Ok(feed)
    }

    /// Applies a partial feed update; `None` patch fields stay unchanged.
    pub fn update_feed(&self, id: FeedId, patch: FeedPatch) -> Result<Feed, RssError> {
        if let Some(url) = &patch.url {
            fetch::validate_url(url).map_err(|_| RssError::InvalidUrl(url.clone()))?;
        }
        let mut store = self.lock_store();
        if let Some(url) = &patch.url {
            if store
                .feeds
                .iter()
                .any(|feed| feed.id != id && feed.url == *url)
            {
                return Err(RssError::DuplicateFeed(url.clone()));
            }
        }
        let feed = store.feed_mut(id).ok_or(RssError::NotFound("feed", id))?;
        if let Some(url) = patch.url {
            feed.url = url;
        }
        if let Some(alias) = patch.alias {
            feed.alias = Some(alias);
        }
        if let Some(enabled) = patch.enabled {
            feed.enabled = enabled;
        }
        if let Some(interval) = patch.refresh_interval_secs {
            feed.refresh_interval_secs = interval;
        }
        let updated = feed.clone();
        store.save(&self.store_path)?;
        Ok(updated)
    }

    /// Removes a feed together with all its stored items.
    pub fn remove_feed(&self, id: FeedId) -> Result<(), RssError> {
        let mut store = self.lock_store();
        if !store.remove_feed(id) {
            return Err(RssError::NotFound("feed", id));
        }
        store.save(&self.store_path)
    }

    /// Lists stored items: optional feed scope and case-insensitive title
    /// search, sorted by publication time (falling back to fetch time)
    /// descending. Returns the requested page and the total match count.
    pub fn items(&self, filter: &ItemFilter) -> (Vec<FeedItem>, usize) {
        let store = self.lock_store();
        let search = filter.search.as_deref().map(str::to_lowercase);
        let mut matched: Vec<FeedItem> = store
            .items
            .iter()
            .filter(|item| filter.feed_id.is_none_or(|id| item.feed_id == id))
            .filter(|item| {
                search
                    .as_deref()
                    .is_none_or(|needle| item.title.to_lowercase().contains(needle))
            })
            .cloned()
            .collect();
        sort_items_desc(&mut matched);
        let total = matched.len();
        let page = matched
            .into_iter()
            .skip(filter.offset)
            .take(filter.limit)
            .collect();
        (page, total)
    }

    pub fn rules(&self) -> Vec<RssRule> {
        self.lock_store().rules.clone()
    }

    /// Validates the rule's regexes and stores it with a freshly assigned
    /// id (any id on the incoming rule is ignored).
    pub fn add_rule(&self, rule: RssRule) -> Result<RssRule, RssError> {
        rules::compile(&rule)?;
        let mut store = self.lock_store();
        let rule = store.insert_rule(rule);
        store.save(&self.store_path)?;
        Ok(rule)
    }

    /// Validates and replaces a stored rule, keeping its original id.
    pub fn update_rule(&self, id: RuleId, rule: RssRule) -> Result<RssRule, RssError> {
        rules::compile(&rule)?;
        let mut store = self.lock_store();
        let existing = store.rule_mut(id).ok_or(RssError::NotFound("rule", id))?;
        *existing = rule;
        existing.id = id;
        let updated = existing.clone();
        store.save(&self.store_path)?;
        Ok(updated)
    }

    pub fn remove_rule(&self, id: RuleId) -> Result<(), RssError> {
        let mut store = self.lock_store();
        if !store.remove_rule(id) {
            return Err(RssError::NotFound("rule", id));
        }
        store.save(&self.store_path)
    }

    /// Dry-runs a rule against stored items: no cooldown checks, no writes.
    /// An invalid regex yields [`RssError::InvalidRule`].
    pub fn test_rule(
        &self,
        rule: &RssRule,
        feed_id: Option<FeedId>,
        limit: usize,
    ) -> Result<Vec<FeedItem>, RssError> {
        let compiled = rules::compile(rule)?;
        let now = unix_secs();
        let store = self.lock_store();
        let mut matched: Vec<FeedItem> = store
            .items
            .iter()
            .filter(|item| feed_id.is_none_or(|id| item.feed_id == id))
            .filter(|item| rules::matches(&compiled, item, now, None))
            .cloned()
            .collect();
        sort_items_desc(&mut matched);
        matched.truncate(limit);
        Ok(matched)
    }

    /// Returns up to `limit` history entries, newest first.
    pub fn history(&self, limit: usize) -> Vec<HistoryEntry> {
        self.lock_store()
            .history
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn clear_history(&self) {
        let mut store = self.lock_store();
        store.history.clear();
        if let Err(err) = store.save(&self.store_path) {
            tracing::warn!(error = %err, "Failed to persist RSS store after clearing history");
        }
    }

    /// Fetches and parses one feed, then applies new items under the store
    /// lock: guid dedup, per-feed caps and rule matching (with cooldowns)
    /// on items that have no download record yet. Fetch/parse errors are
    /// reported in the outcome, not as `Err`; only a missing feed or a
    /// persistence failure yields `Err`. Explicit refreshes run even for
    /// disabled feeds.
    pub async fn refresh_feed(&self, id: FeedId) -> Result<RefreshOutcome, RssError> {
        let feed = {
            let store = self.lock_store();
            store
                .feed(id)
                .ok_or(RssError::NotFound("feed", id))?
                .clone()
        };
        let (max_bytes, max_items) = {
            let config = self.lock_config();
            (config.max_response_bytes, config.max_items_per_feed)
        };
        let now = unix_secs();
        // Network and parsing happen with no lock held.
        let response = fetch::fetch_feed(
            &self.client,
            &feed.url,
            feed.etag.as_deref(),
            feed.last_modified.as_deref(),
            max_bytes,
        )
        .await;
        let (parsed, etag, last_modified) = match response {
            Ok(FeedResponse::NotModified) => return self.apply_not_modified(id, now),
            Ok(FeedResponse::Body {
                bytes,
                etag,
                last_modified,
            }) => match parse::parse_feed(&bytes) {
                Ok(parsed) => (parsed, etag, last_modified),
                Err(err) => return self.apply_fetch_error(id, now, &err.to_string()),
            },
            Err(err) => return self.apply_fetch_error(id, now, &err.to_string()),
        };

        let mut store = self.lock_store();
        let Some(feed) = store.feed_mut(id) else {
            // The feed was removed while the fetch was in flight.
            return Ok(RefreshOutcome {
                feed_id: id,
                new_items: 0,
                matches: Vec::new(),
                error: None,
            });
        };
        feed.last_fetch_at = Some(now);
        feed.last_success_at = Some(now);
        feed.last_error = None;
        feed.etag = etag;
        feed.last_modified = last_modified;

        let mut new_items: Vec<FeedItem> = Vec::new();
        for parsed_item in parsed.items {
            // The parser already falls back to link/torrent URL; a title
            // guid is the last resort so every item remains deduplicable.
            let guid = parsed_item
                .guid
                .unwrap_or_else(|| parsed_item.title.clone());
            let info_hash = parsed_item
                .magnet
                .as_deref()
                .and_then(parse::normalize_info_hash);
            let item = FeedItem {
                id: 0,
                feed_id: id,
                guid,
                title: parsed_item.title,
                link: parsed_item.link,
                torrent_url: parsed_item.torrent_url,
                magnet: parsed_item.magnet,
                info_hash,
                size_bytes: parsed_item.size_bytes,
                published_at: parsed_item.published_at,
                fetched_at: now,
                download: None,
            };
            if let Some(stored) = store.insert_item(item, max_items) {
                new_items.push(stored);
            }
        }
        let mut matches = Vec::new();
        for item in &new_items {
            if let Some(matched) = rules::best_match(&store.rules, item, now, &store.cooldowns) {
                matches.push(matched);
            }
        }
        let outcome = RefreshOutcome {
            feed_id: id,
            new_items: new_items.len(),
            matches,
            error: None,
        };
        store.save(&self.store_path)?;
        Ok(outcome)
    }

    /// Refreshes enabled feeds whose interval (plus a per-feed jitter of
    /// `id % 60` seconds) has elapsed since the last fetch. Feeds never
    /// fetched are always due. Runs sequentially; a feed error lands in its
    /// outcome and does not stop the remaining feeds.
    pub async fn refresh_due(&self, now: u64) -> Vec<RefreshOutcome> {
        let default_interval = self.lock_config().default_refresh_interval_secs;
        let due: Vec<FeedId> = {
            let store = self.lock_store();
            store
                .feeds
                .iter()
                .filter(|feed| feed.enabled)
                .filter(|feed| {
                    let interval = if feed.refresh_interval_secs > 0 {
                        feed.refresh_interval_secs
                    } else {
                        default_interval
                    };
                    let threshold = interval + feed.id % 60;
                    feed.last_fetch_at
                        .is_none_or(|last| now.saturating_sub(last) >= threshold)
                })
                .map(|feed| feed.id)
                .collect()
        };
        self.refresh_sequential(due).await
    }

    /// Force-refreshes every enabled feed, ignoring intervals.
    pub async fn refresh_all(&self) -> Vec<RefreshOutcome> {
        let ids: Vec<FeedId> = {
            let store = self.lock_store();
            store
                .feeds
                .iter()
                .filter(|feed| feed.enabled)
                .map(|feed| feed.id)
                .collect()
        };
        self.refresh_sequential(ids).await
    }

    /// Hands an unprocessed item to the caller for a manual download. The
    /// action comes from the best matching rule with cooldowns ignored
    /// (manual override), or defaults when no rule matches. The item is
    /// only marked processed later, via [`record_download`].
    pub fn take_manual(
        &self,
        item_id: u64,
    ) -> Result<(FeedItem, DownloadAction, Option<RuleId>), RssError> {
        let store = self.lock_store();
        let item = store
            .item(item_id)
            .ok_or(RssError::NotFound("item", item_id))?;
        if item.download.is_some() {
            return Err(RssError::ItemAlreadyProcessed(item_id));
        }
        let now = unix_secs();
        let matched = rules::best_match(&store.rules, item, now, &HashMap::new());
        Ok(match matched {
            Some(matched) => (item.clone(), matched.action, Some(matched.rule_id)),
            None => (
                item.clone(),
                DownloadAction {
                    save_path: None,
                    label: None,
                    paused: false,
                },
                None,
            ),
        })
    }

    /// Records the engine's result on the item, appends a history entry
    /// and arms the rule cooldown (only for rule-driven `Added` results),
    /// then persists. Unknown item ids are logged and ignored.
    pub fn record_download(&self, item_id: u64, rule_id: Option<RuleId>, result: DownloadResult) {
        let now = unix_secs();
        let (status, torrent_hash, error) = match result {
            DownloadResult::Added(hash) => (DownloadStatus::Added, Some(hash), None),
            DownloadResult::Duplicate(hash) => (DownloadStatus::Duplicate, Some(hash), None),
            DownloadResult::Failed(message) => (DownloadStatus::Failed, None, Some(message)),
        };
        let mut store = self.lock_store();
        let Some(item) = store.item_mut(item_id) else {
            tracing::warn!(item_id, "RSS download result for unknown item");
            return;
        };
        item.download = Some(DownloadRecord {
            torrent_hash: torrent_hash.clone(),
            rule_id,
            status,
            error: error.clone(),
            at: now,
        });
        let entry = HistoryEntry {
            id: 0,
            at: now,
            feed_id: item.feed_id,
            item_id,
            title: item.title.clone(),
            rule_id,
            torrent_hash,
            status,
            error,
        };
        store.insert_history(entry);
        if let Some(rule_id) = rule_id {
            if status == DownloadStatus::Added {
                store.cooldowns.insert(rule_id, now);
            }
        }
        if let Err(err) = store.save(&self.store_path) {
            tracing::warn!(error = %err, "Failed to persist RSS store after download");
        }
    }

    /// Writes the current store state to disk atomically.
    pub fn persist(&self) -> Result<(), RssError> {
        self.lock_store().save(&self.store_path)
    }

    /// 304 path: the cached copy is current, so only fetch bookkeeping is
    /// updated; validators are kept for the next conditional request.
    fn apply_not_modified(&self, id: FeedId, now: u64) -> Result<RefreshOutcome, RssError> {
        let mut store = self.lock_store();
        if let Some(feed) = store.feed_mut(id) {
            feed.last_fetch_at = Some(now);
            feed.last_success_at = Some(now);
            feed.last_error = None;
        }
        store.save(&self.store_path)?;
        Ok(RefreshOutcome {
            feed_id: id,
            new_items: 0,
            matches: Vec::new(),
            error: None,
        })
    }

    /// Fetch/parse failure path: record the error on the feed but keep all
    /// previously stored items and validators.
    fn apply_fetch_error(
        &self,
        id: FeedId,
        now: u64,
        error: &str,
    ) -> Result<RefreshOutcome, RssError> {
        let mut store = self.lock_store();
        if let Some(feed) = store.feed_mut(id) {
            feed.last_fetch_at = Some(now);
            feed.last_error = Some(error.to_string());
        }
        store.save(&self.store_path)?;
        Ok(RefreshOutcome {
            feed_id: id,
            new_items: 0,
            matches: Vec::new(),
            error: Some(error.to_string()),
        })
    }

    async fn refresh_sequential(&self, ids: Vec<FeedId>) -> Vec<RefreshOutcome> {
        let mut outcomes = Vec::with_capacity(ids.len());
        for id in ids {
            let outcome = match self.refresh_feed(id).await {
                Ok(outcome) => outcome,
                Err(err) => RefreshOutcome {
                    feed_id: id,
                    new_items: 0,
                    matches: Vec::new(),
                    error: Some(err.to_string()),
                },
            };
            outcomes.push(outcome);
        }
        outcomes
    }

    fn lock_store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn lock_config(&self) -> MutexGuard<'_, RssConfig> {
        self.config.lock().unwrap_or_else(|err| err.into_inner())
    }
}

/// Newest first: publication time (fetch time when unpublished), then fetch
/// time, then id as a deterministic tie-breaker.
fn sort_items_desc(items: &mut [FeedItem]) {
    items.sort_by_key(|item| {
        std::cmp::Reverse((
            item.published_at.unwrap_or(item.fetched_at),
            item.fetched_at,
            item.id,
        ))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("rt_rss_manager_unit_{name}_{}", std::process::id()));
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

    fn item(id: u64, title: &str, published: Option<u64>, fetched: u64) -> FeedItem {
        FeedItem {
            id,
            feed_id: 1,
            guid: format!("g-{id}"),
            title: title.to_string(),
            link: None,
            torrent_url: None,
            magnet: None,
            info_hash: None,
            size_bytes: None,
            published_at: published,
            fetched_at: fetched,
            download: None,
        }
    }

    fn manager_with(name: &str, items: Vec<FeedItem>) -> (TempDir, RssManager) {
        let dir = TempDir::new(name);
        let store = Store {
            items,
            next_item_id: 100,
            ..Store::default()
        };
        store.save(&dir.path.join("rss.json")).unwrap();
        let manager = RssManager::new(dir.path.join("rss.json"), RssConfig::default()).unwrap();
        (dir, manager)
    }

    fn filter(limit: usize, offset: usize) -> ItemFilter {
        ItemFilter {
            feed_id: None,
            search: None,
            limit,
            offset,
        }
    }

    #[test]
    fn items_sort_uses_published_then_fetched_then_id() {
        let (_dir, manager) = manager_with(
            "sort",
            vec![
                item(1, "old published", Some(100), 900),
                item(2, "unpublished", None, 500),
                item(3, "new published", Some(300), 100),
                item(4, "unpublished newer", None, 500),
            ],
        );
        let (page, total) = manager.items(&filter(10, 0));
        assert_eq!(total, 4);
        let ids: Vec<u64> = page.iter().map(|item| item.id).collect();
        // Unpublished items sort by fetched_at: 500 > 300 > 100; the two
        // unpublished tie on both time keys and fall back to descending id.
        assert_eq!(ids, [4, 2, 3, 1]);
    }

    #[test]
    fn items_search_is_case_insensitive_and_paginates() {
        let (_dir, manager) = manager_with(
            "search",
            vec![
                item(1, "Show ONE 1080p", Some(300), 1),
                item(2, "movie", Some(200), 1),
                item(3, "show two 720p", Some(100), 1),
            ],
        );
        let search = ItemFilter {
            search: Some("SHOW".to_string()),
            ..filter(10, 0)
        };
        let (page, total) = manager.items(&search);
        assert_eq!(total, 2);
        assert_eq!(page[0].id, 1);
        assert_eq!(page[1].id, 3);
        let (page, total) = manager.items(&ItemFilter {
            search: Some("show".to_string()),
            ..filter(1, 1)
        });
        assert_eq!(total, 2);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, 3);
        let (page, total) = manager.items(&ItemFilter {
            search: Some("show".to_string()),
            ..filter(1, 5)
        });
        assert_eq!(total, 2);
        assert!(page.is_empty());
    }

    #[test]
    fn take_manual_without_rules_returns_defaults() {
        let (_dir, manager) = manager_with("manual", vec![item(1, "anything", Some(100), 1)]);
        let (item, action, rule_id) = manager.take_manual(1).unwrap();
        assert_eq!(item.id, 1);
        assert!(action.save_path.is_none());
        assert!(action.label.is_none());
        assert!(!action.paused);
        assert!(rule_id.is_none());
        assert!(matches!(
            manager.take_manual(999),
            Err(RssError::NotFound("item", 999))
        ));
    }
}
