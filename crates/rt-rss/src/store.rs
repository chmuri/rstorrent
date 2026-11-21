//! Persistent JSON storage for the RSS subsystem: feeds, items, rules,
//! download history and rule cooldowns in a single `rss.json` document.
//!
//! Writes are atomic (temporary file + rename) with 0600 permissions on
//! unix. A missing file loads as the default store; a corrupt or
//! unsupported-version document is quarantined as
//! `rss.json.corrupt-<unixsecs>` and replaced by a fresh store. The store is
//! pure data: no HTTP and no rule evaluation happen here.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::RssError;
use crate::model::{Feed, FeedId, FeedItem, HistoryEntry, RssRule, RuleId};

/// Schema version of the on-disk document.
const STORE_VERSION: u32 = 1;
/// Maximum number of history entries kept; the oldest are dropped first.
const MAX_HISTORY: usize = 500;

/// The full persisted state of the RSS subsystem. Field layout mirrors the
/// binding `rss.json` schema; ids are allocated from the `next_*` counters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    pub next_feed_id: FeedId,
    pub next_rule_id: RuleId,
    pub next_item_id: u64,
    pub next_history_id: u64,
    pub feeds: Vec<Feed>,
    pub items: Vec<FeedItem>,
    pub rules: Vec<RssRule>,
    pub history: Vec<HistoryEntry>,
    /// Unix time of each rule's most recent `Added` download.
    pub cooldowns: HashMap<RuleId, u64>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            next_feed_id: 1,
            next_rule_id: 1,
            next_item_id: 1,
            next_history_id: 1,
            feeds: Vec::new(),
            items: Vec::new(),
            rules: Vec::new(),
            history: Vec::new(),
            cooldowns: HashMap::new(),
        }
    }
}

impl Store {
    /// Loads the store from `path`. A missing file yields the default
    /// store; a corrupt or unsupported-version document is quarantined
    /// next to the original and a fresh default store is returned.
    pub fn load(path: &Path) -> Result<Self, RssError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => {
                return Err(RssError::Store(format!("read {}: {err}", path.display())));
            }
        };
        match serde_json::from_slice::<Store>(&bytes) {
            Ok(store) if store.version == STORE_VERSION => Ok(store),
            Ok(store) => {
                Self::quarantine(
                    path,
                    &format!("unsupported store version {}", store.version),
                );
                Ok(Self::default())
            }
            Err(err) => {
                Self::quarantine(path, &err.to_string());
                Ok(Self::default())
            }
        }
    }

    /// Atomically writes the store to `path`: serialize, write
    /// `<name>.tmp` with 0600 permissions (unix), fsync, then rename over
    /// the target. Missing parent directories are created.
    pub fn save(&self, path: &Path) -> Result<(), RssError> {
        let io_err =
            |err: std::io::Error| RssError::Store(format!("write {}: {err}", path.display()));
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(io_err)?;
            }
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|err| RssError::Store(format!("serialize store: {err}")))?;
        let tmp = sibling_path(path, ".tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(io_err)?;
        file.write_all(&bytes).map_err(io_err)?;
        file.sync_all().map_err(io_err)?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The mode above only applies on creation; a leftover tmp file
            // from a crashed run must be tightened too.
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(io_err)?;
        }
        std::fs::rename(&tmp, path).map_err(io_err)?;
        Ok(())
    }

    /// Moves a corrupt document aside so the next save starts clean.
    /// Quarantine failures are logged, never fatal: a fresh store is still
    /// returned to the caller.
    fn quarantine(path: &Path, reason: &str) {
        let target = sibling_path(path, &format!(".corrupt-{}", unix_secs()));
        tracing::warn!(
            path = %path.display(),
            reason,
            "Quarantining corrupt RSS store; starting fresh"
        );
        if let Err(err) = std::fs::rename(path, &target) {
            tracing::warn!(error = %err, "Failed to quarantine corrupt RSS store");
        }
    }

    /// Appends a feed, assigning the next feed id.
    pub fn insert_feed(&mut self, mut feed: Feed) -> Feed {
        feed.id = self.next_feed_id;
        self.next_feed_id += 1;
        self.feeds.push(feed.clone());
        feed
    }

    /// Appends a rule, assigning the next rule id.
    pub fn insert_rule(&mut self, mut rule: RssRule) -> RssRule {
        rule.id = self.next_rule_id;
        self.next_rule_id += 1;
        self.rules.push(rule.clone());
        rule
    }

    /// Inserts an item unless its `(feed_id, guid)` already exists; an
    /// existing item is never updated. Assigns the next item id and
    /// enforces the per-feed cap. Returns the stored item, or `None` when
    /// deduplicated (or evicted immediately by a zero cap).
    pub fn insert_item(
        &mut self,
        mut item: FeedItem,
        max_items_per_feed: usize,
    ) -> Option<FeedItem> {
        let duplicate = self
            .items
            .iter()
            .any(|existing| existing.feed_id == item.feed_id && existing.guid == item.guid);
        if duplicate {
            return None;
        }
        item.id = self.next_item_id;
        self.next_item_id += 1;
        self.items.push(item.clone());
        self.evict_feed_items(item.feed_id, max_items_per_feed);
        self.items
            .iter()
            .any(|stored| stored.id == item.id)
            .then_some(item)
    }

    /// Appends a history entry, assigning the next history id and keeping
    /// at most [`MAX_HISTORY`] entries (oldest dropped first).
    pub fn insert_history(&mut self, mut entry: HistoryEntry) {
        entry.id = self.next_history_id;
        self.next_history_id += 1;
        self.history.push(entry);
        if self.history.len() > MAX_HISTORY {
            let excess = self.history.len() - MAX_HISTORY;
            self.history.drain(..excess);
        }
    }

    /// Enforces the per-feed item cap: oldest unprocessed items are evicted
    /// first, then oldest processed ones. "Oldest" means lowest id
    /// (insertion order), which stays stable regardless of publication
    /// dates and only affects the given feed.
    fn evict_feed_items(&mut self, feed_id: FeedId, cap: usize) {
        let count = self
            .items
            .iter()
            .filter(|item| item.feed_id == feed_id)
            .count();
        let Some(mut excess) = count.checked_sub(cap) else {
            return;
        };
        let mut remove_ids: Vec<u64> = Vec::new();
        for processed in [false, true] {
            if excess == 0 {
                break;
            }
            let mut candidates: Vec<u64> = self
                .items
                .iter()
                .filter(|item| item.feed_id == feed_id && item.download.is_some() == processed)
                .map(|item| item.id)
                .collect();
            candidates.sort_unstable();
            for id in candidates.into_iter().take(excess) {
                remove_ids.push(id);
                excess -= 1;
            }
        }
        self.items.retain(|item| !remove_ids.contains(&item.id));
    }

    pub fn feed(&self, id: FeedId) -> Option<&Feed> {
        self.feeds.iter().find(|feed| feed.id == id)
    }

    pub fn feed_mut(&mut self, id: FeedId) -> Option<&mut Feed> {
        self.feeds.iter_mut().find(|feed| feed.id == id)
    }

    pub fn item(&self, id: u64) -> Option<&FeedItem> {
        self.items.iter().find(|item| item.id == id)
    }

    pub fn item_mut(&mut self, id: u64) -> Option<&mut FeedItem> {
        self.items.iter_mut().find(|item| item.id == id)
    }

    pub fn rule(&self, id: RuleId) -> Option<&RssRule> {
        self.rules.iter().find(|rule| rule.id == id)
    }

    pub fn rule_mut(&mut self, id: RuleId) -> Option<&mut RssRule> {
        self.rules.iter_mut().find(|rule| rule.id == id)
    }

    /// Removes a feed together with all its items. Returns `false` when
    /// the feed does not exist.
    pub fn remove_feed(&mut self, id: FeedId) -> bool {
        let before = self.feeds.len();
        self.feeds.retain(|feed| feed.id != id);
        self.items.retain(|item| item.feed_id != id);
        self.feeds.len() < before
    }

    /// Removes a rule and its cooldown entry. Returns `false` when the
    /// rule does not exist.
    pub fn remove_rule(&mut self, id: RuleId) -> bool {
        let before = self.rules.len();
        self.rules.retain(|rule| rule.id != id);
        self.cooldowns.remove(&id);
        self.rules.len() < before
    }
}

/// Current Unix time in seconds; falls back to 0 if the clock is before
/// the epoch (never happens in practice, but keeps the code panic-free).
pub(crate) fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Appends `suffix` to the file name of `path`, keeping the directory.
fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_else(|| std::ffi::OsString::from("rss.json"));
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DownloadRecord, DownloadStatus};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("rt_rss_store_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn store_path(&self) -> PathBuf {
            self.path.join("rss.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn sample_feed(id: FeedId) -> Feed {
        Feed {
            id,
            url: format!("https://example.com/rss/{id}.xml"),
            alias: Some(format!("feed {id}")),
            enabled: true,
            refresh_interval_secs: 0,
            last_fetch_at: Some(1_700_000_000),
            last_success_at: Some(1_700_000_000),
            last_error: None,
            etag: Some("\"v1\"".to_string()),
            last_modified: None,
            created_at: 1_699_000_000,
        }
    }

    fn sample_item(feed_id: FeedId, guid: &str, downloaded: bool) -> FeedItem {
        FeedItem {
            id: 0,
            feed_id,
            guid: guid.to_string(),
            title: format!("title {guid}"),
            link: None,
            torrent_url: Some(format!("https://example.com/t/{guid}.torrent")),
            magnet: None,
            info_hash: None,
            size_bytes: Some(1_000_000),
            published_at: Some(1_700_000_000),
            fetched_at: 1_700_000_000,
            download: downloaded.then(|| DownloadRecord {
                torrent_hash: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
                rule_id: Some(1),
                status: DownloadStatus::Added,
                error: None,
                at: 1_700_000_100,
            }),
        }
    }

    fn sample_rule(id: RuleId) -> RssRule {
        RssRule {
            tags: Vec::new(),
            throttle_name: None,
            seed_limits: None,
            id,
            name: format!("rule {id}"),
            enabled: true,
            feed_ids: Vec::new(),
            priority: 0,
            match_title: Some("show.*1080p".to_string()),
            exclude_title: None,
            min_size_bytes: None,
            max_size_bytes: None,
            episode: None,
            save_path: Some("/downloads".to_string()),
            label: None,
            add_paused: false,
            cooldown_secs: Some(3600),
        }
    }

    fn sample_history(item_id: u64) -> HistoryEntry {
        HistoryEntry {
            id: 0,
            at: 1_700_000_000 + item_id,
            feed_id: 1,
            item_id,
            title: format!("title {item_id}"),
            rule_id: Some(1),
            torrent_hash: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            status: DownloadStatus::Added,
            error: None,
        }
    }

    #[test]
    fn missing_file_loads_default_without_creating_it() {
        let dir = TempDir::new("missing");
        let path = dir.store_path();
        let store = Store::load(&path).unwrap();
        assert_eq!(store.version, 1);
        assert_eq!(store.next_feed_id, 1);
        assert_eq!(store.next_rule_id, 1);
        assert_eq!(store.next_item_id, 1);
        assert_eq!(store.next_history_id, 1);
        assert!(store.feeds.is_empty());
        assert!(store.items.is_empty());
        assert!(store.rules.is_empty());
        assert!(store.history.is_empty());
        assert!(store.cooldowns.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn save_load_round_trip_preserves_every_field() {
        let dir = TempDir::new("round_trip");
        let path = dir.store_path();
        let mut store = Store::default();
        store.insert_feed(sample_feed(0));
        store.insert_rule(sample_rule(0));
        store.insert_item(sample_item(1, "g-1", true), 200).unwrap();
        store
            .insert_item(sample_item(1, "g-2", false), 200)
            .unwrap();
        store.insert_history(sample_history(1));
        store.cooldowns.insert(1, 1_700_000_500);

        store.save(&path).unwrap();
        let loaded = Store::load(&path).unwrap();
        // Models have no PartialEq; the serialized form is the stable
        // full-field comparison (same trick as model.rs tests).
        assert_eq!(
            serde_json::to_string(&loaded).unwrap(),
            serde_json::to_string(&store).unwrap()
        );
        assert_eq!(loaded.next_feed_id, 2);
        assert_eq!(loaded.next_rule_id, 2);
        assert_eq!(loaded.next_item_id, 3);
        assert_eq!(loaded.next_history_id, 2);
        assert_eq!(loaded.cooldowns.get(&1), Some(&1_700_000_500));
        // Atomic write leaves no temporary file behind.
        assert!(!path.with_file_name("rss.json.tmp").exists());
    }

    #[test]
    fn save_sets_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("permissions");
        let path = dir.store_path();
        Store::default().save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // A pre-existing world-readable tmp file must be tightened too.
        let tmp = path.with_file_name("rss.json.tmp");
        std::fs::write(&tmp, b"stale").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        Store::default().save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = TempDir::new("parents");
        let path = dir.path.join("nested").join("deeper").join("rss.json");
        Store::default().save(&path).unwrap();
        assert!(path.exists());
        assert!(Store::load(&path).unwrap().feeds.is_empty());
    }

    #[test]
    fn corrupt_file_is_quarantined_and_store_starts_fresh() {
        let dir = TempDir::new("corrupt");
        let path = dir.store_path();
        std::fs::write(&path, b"{ this is not json").unwrap();
        let store = Store::load(&path).unwrap();
        assert!(store.feeds.is_empty());
        assert_eq!(store.next_feed_id, 1);
        assert!(!path.exists());
        let quarantined: Vec<PathBuf> = std::fs::read_dir(&dir.path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|entry| {
                entry
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("rss.json.corrupt-")
            })
            .collect();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(
            std::fs::read(&quarantined[0]).unwrap(),
            b"{ this is not json"
        );
    }

    #[test]
    fn wrong_schema_is_quarantined() {
        let dir = TempDir::new("bad_schema");
        let path = dir.store_path();
        // Valid JSON but missing required fields.
        std::fs::write(&path, br#"{"version": 1}"#).unwrap();
        let store = Store::load(&path).unwrap();
        assert!(store.feeds.is_empty());
        assert_eq!(store.version, 1);
        assert!(!path.exists());
        assert_eq!(quarantine_count(&dir), 1);
    }

    #[test]
    fn unsupported_version_is_quarantined() {
        let dir = TempDir::new("bad_version");
        let path = dir.store_path();
        // Full schema but a version this build does not understand.
        let mut store = Store::default();
        store.insert_feed(sample_feed(0));
        let store = Store {
            version: 99,
            ..store
        };
        std::fs::write(&path, serde_json::to_vec(&store).unwrap()).unwrap();
        let fresh = Store::load(&path).unwrap();
        assert!(fresh.feeds.is_empty());
        assert_eq!(fresh.version, 1);
        assert!(!path.exists());
        assert_eq!(quarantine_count(&dir), 1);
    }

    fn quarantine_count(dir: &TempDir) -> usize {
        std::fs::read_dir(&dir.path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("rss.json.corrupt-"))
            .count()
    }

    #[test]
    fn insert_item_dedups_by_feed_and_guid_without_updating() {
        let mut store = Store::default();
        let first = store
            .insert_item(sample_item(1, "g-1", false), 200)
            .unwrap();
        assert_eq!(first.id, 1);
        let mut changed = sample_item(1, "g-1", false);
        changed.title = "updated title".to_string();
        assert!(store.insert_item(changed, 200).is_none());
        assert_eq!(store.items.len(), 1);
        assert_eq!(store.items[0].title, "title g-1");
        assert_eq!(store.next_item_id, 2);
        // Same guid on another feed is a distinct item.
        let other = store
            .insert_item(sample_item(2, "g-1", false), 200)
            .unwrap();
        assert_eq!(other.id, 2);
        assert_eq!(store.items.len(), 2);
    }

    #[test]
    fn eviction_drops_oldest_unprocessed_first_then_processed() {
        let mut store = Store::default();
        // Insertion order: u1, p1(downloaded), u2, u3, u4 with cap 3.
        store.insert_item(sample_item(1, "u1", false), 3).unwrap();
        store.insert_item(sample_item(1, "p1", true), 3).unwrap();
        store.insert_item(sample_item(1, "u2", false), 3).unwrap();
        let guids = |store: &Store| -> Vec<String> {
            store.items.iter().map(|item| item.guid.clone()).collect()
        };
        assert_eq!(guids(&store), ["u1", "p1", "u2"]);
        // u3 overflows: oldest unprocessed (u1) goes, downloaded p1 stays.
        store.insert_item(sample_item(1, "u3", false), 3).unwrap();
        assert_eq!(guids(&store), ["p1", "u2", "u3"]);
        // u4 overflows: u2 goes.
        store.insert_item(sample_item(1, "u4", false), 3).unwrap();
        assert_eq!(guids(&store), ["p1", "u3", "u4"]);
        // p2 overflows: u3 goes (unprocessed still preferred).
        store.insert_item(sample_item(1, "p2", true), 3).unwrap();
        assert_eq!(guids(&store), ["p1", "u4", "p2"]);
        // p3 overflows: u4 goes, leaving only downloaded items.
        store.insert_item(sample_item(1, "p3", true), 3).unwrap();
        assert_eq!(guids(&store), ["p1", "p2", "p3"]);
        // p4 overflows with no unprocessed left: oldest downloaded (p1) goes.
        store.insert_item(sample_item(1, "p4", true), 3).unwrap();
        assert_eq!(guids(&store), ["p2", "p3", "p4"]);
    }

    #[test]
    fn eviction_only_touches_the_capped_feed_and_zero_cap_keeps_nothing() {
        let mut store = Store::default();
        store.insert_item(sample_item(1, "a", false), 1).unwrap();
        store.insert_item(sample_item(2, "b", false), 1).unwrap();
        store.insert_item(sample_item(1, "c", false), 1).unwrap();
        let guids: Vec<String> = store.items.iter().map(|item| item.guid.clone()).collect();
        assert_eq!(guids, ["b", "c"]);
        // A zero cap evicts even the freshly inserted item.
        assert!(store.insert_item(sample_item(2, "d", false), 0).is_none());
        let guids: Vec<String> = store.items.iter().map(|item| item.guid.clone()).collect();
        assert_eq!(guids, ["c"]);
    }

    #[test]
    fn history_is_capped_at_500_oldest_first() {
        let mut store = Store::default();
        for item_id in 1..=505u64 {
            store.insert_history(sample_history(item_id));
        }
        assert_eq!(store.history.len(), 500);
        assert_eq!(store.history[0].item_id, 6);
        assert_eq!(store.history[499].item_id, 505);
        assert_eq!(store.history[0].id, 6);
        assert_eq!(store.next_history_id, 506);
    }

    #[test]
    fn insert_methods_allocate_sequential_ids() {
        let mut store = Store::default();
        assert_eq!(store.insert_feed(sample_feed(0)).id, 1);
        assert_eq!(store.insert_feed(sample_feed(0)).id, 2);
        assert_eq!(store.insert_rule(sample_rule(0)).id, 1);
        assert_eq!(store.insert_rule(sample_rule(0)).id, 2);
        assert_eq!(
            store
                .insert_item(sample_item(1, "a", false), 200)
                .unwrap()
                .id,
            1
        );
        store.insert_history(sample_history(1));
        store.insert_history(sample_history(2));
        assert_eq!(store.history[0].id, 1);
        assert_eq!(store.history[1].id, 2);
    }

    #[test]
    fn remove_feed_deletes_its_items_and_remove_rule_clears_cooldown() {
        let mut store = Store::default();
        store.insert_feed(sample_feed(0));
        store.insert_feed(sample_feed(0));
        store.insert_item(sample_item(1, "a", false), 200).unwrap();
        store.insert_item(sample_item(2, "b", false), 200).unwrap();
        store.insert_rule(sample_rule(0));
        store.cooldowns.insert(1, 1_700_000_000);

        assert!(store.remove_feed(1));
        assert!(!store.remove_feed(1));
        assert_eq!(store.feeds.len(), 1);
        assert_eq!(store.items.len(), 1);
        assert_eq!(store.items[0].feed_id, 2);

        assert!(store.remove_rule(1));
        assert!(!store.remove_rule(1));
        assert!(store.rules.is_empty());
        assert!(store.cooldowns.is_empty());
    }
}
