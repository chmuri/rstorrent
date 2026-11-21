//! Data models for the RSS subsystem: feeds, parsed items, rules, and the
//! records exchanged between the store, the rule engine, and the download
//! pipeline. All timestamps are Unix seconds.

use serde::{Deserialize, Serialize};

pub type FeedId = u64;
pub type RuleId = u64;

/// A registered RSS feed together with its fetch bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feed {
    pub id: FeedId,
    pub url: String,
    pub alias: Option<String>,
    pub enabled: bool,
    /// Per-feed override; `0` means use the global default from [`RssConfig`].
    pub refresh_interval_secs: u64,
    pub last_fetch_at: Option<u64>,
    pub last_success_at: Option<u64>,
    pub last_error: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub created_at: u64,
}

/// A single item parsed from a feed. A `Some` [`download`](FeedItem::download)
/// marks the item as processed so it is never downloaded again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    /// Assigned by the store.
    pub id: u64,
    pub feed_id: FeedId,
    /// guid/id from the XML; falls back to the link or torrent URL.
    pub guid: String,
    pub title: String,
    pub link: Option<String>,
    pub torrent_url: Option<String>,
    pub magnet: Option<String>,
    /// 40-char lowercase hex, extracted from a magnet `xt=urn:btih:`.
    pub info_hash: Option<String>,
    pub size_bytes: Option<u64>,
    pub published_at: Option<u64>,
    pub fetched_at: u64,
    pub download: Option<DownloadRecord>,
}

/// Outcome of processing an item, recorded on the item itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub torrent_hash: Option<String>,
    /// `None` means the download was triggered manually.
    pub rule_id: Option<RuleId>,
    pub status: DownloadStatus,
    pub error: Option<String>,
    pub at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Added,
    Duplicate,
    Failed,
}

/// Seed-limit overrides applied to a torrent when an RSS rule adds it
/// (plan §6.10). Kept local to `rt-rss` so the crate stays independent of
/// `rt-rules`; the daemon glue maps this onto `rt_rules::TorrentOverrides`.
///
/// Per field: `None` = leave the torrent default (inherit), `Some(0)` /
/// `Some(0.0)` = unlimited, `Some(v > 0)` = explicit limit. `action` uses the
/// wire string form (`stop`, `throttle:<name>`, `command:<str>`, …); `None`
/// inherits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RssSeedLimits {
    #[serde(default)]
    pub ratio: Option<f64>,
    #[serde(default)]
    pub seeding_time_secs: Option<u64>,
    #[serde(default)]
    pub inactive_secs: Option<u64>,
    #[serde(default)]
    pub action: Option<String>,
}

/// A user-defined rule selecting items for automatic download.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssRule {
    pub id: RuleId,
    pub name: String,
    pub enabled: bool,
    /// Empty means the rule applies to all feeds.
    pub feed_ids: Vec<FeedId>,
    /// Lower values are evaluated first.
    pub priority: i32,
    /// Regex, matched case-insensitively.
    pub match_title: Option<String>,
    /// Regex, matched case-insensitively.
    pub exclude_title: Option<String>,
    pub min_size_bytes: Option<u64>,
    pub max_size_bytes: Option<u64>,
    pub episode: Option<EpisodeFilter>,
    pub save_path: Option<String>,
    /// Maps to rTorrent's `d.custom1`.
    pub label: Option<String>,
    pub add_paused: bool,
    /// Minimum spacing between downloads triggered by this rule.
    pub cooldown_secs: Option<u64>,
    /// Tags applied to the torrent on add (plan §6.10, additive).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Named throttle applied on add.
    #[serde(default)]
    pub throttle_name: Option<String>,
    /// Per-torrent seed-limit overrides applied on add.
    #[serde(default)]
    pub seed_limits: Option<RssSeedLimits>,
}

/// Inclusive episode range, optionally scoped to a season.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeFilter {
    pub season: Option<u32>,
    pub from_episode: u32,
    pub to_episode: u32,
}

/// Download parameters derived from a matching rule (or manual defaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadAction {
    pub save_path: Option<String>,
    pub label: Option<String>,
    pub paused: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub throttle_name: Option<String>,
    #[serde(default)]
    pub seed_limits: Option<RssSeedLimits>,
}

/// An item that matched a rule, paired with the action to perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedItem {
    pub item: FeedItem,
    pub rule_id: RuleId,
    pub action: DownloadAction,
}

/// An immutable record of a download attempt, kept for the history view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: u64,
    pub at: u64,
    pub feed_id: FeedId,
    pub item_id: u64,
    pub title: String,
    pub rule_id: Option<RuleId>,
    pub torrent_hash: Option<String>,
    pub status: DownloadStatus,
    pub error: Option<String>,
}

/// Global RSS settings. With no feeds registered the subsystem is inert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssConfig {
    pub enabled: bool,
    pub default_refresh_interval_secs: u64,
    pub max_items_per_feed: usize,
    pub request_timeout_secs: u64,
    pub max_response_bytes: u64,
}

impl Default for RssConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_refresh_interval_secs: 1800,
            max_items_per_feed: 200,
            request_timeout_secs: 30,
            max_response_bytes: 5 * 1024 * 1024,
        }
    }
}

/// Result of refreshing one feed. A fetch/parse error is reported here
/// instead of failing the whole run; the feed is never deleted on error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshOutcome {
    pub feed_id: FeedId,
    pub new_items: usize,
    pub matches: Vec<MatchedItem>,
    pub error: Option<String>,
}

/// Partial update of a feed; `None` fields are left unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedPatch {
    pub url: Option<String>,
    pub alias: Option<String>,
    pub enabled: Option<bool>,
    pub refresh_interval_secs: Option<u64>,
}

/// Query parameters for listing stored items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemFilter {
    pub feed_id: Option<FeedId>,
    pub search: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

/// Result of handing a match to the engine: the torrent hash for
/// `Added`/`Duplicate`, or the error message for `Failed`.
#[derive(Debug, Clone)]
pub enum DownloadResult {
    Added(String),
    Duplicate(String),
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_feed() -> Feed {
        Feed {
            id: 1,
            url: "https://example.com/rss.xml".to_string(),
            alias: Some("Example feed".to_string()),
            enabled: true,
            refresh_interval_secs: 0,
            last_fetch_at: Some(1_700_000_000),
            last_success_at: Some(1_699_999_000),
            last_error: Some("HTTP status 503".to_string()),
            etag: Some("\"abc123\"".to_string()),
            last_modified: Some("Wed, 15 Nov 2023 06:00:00 GMT".to_string()),
            created_at: 1_699_000_000,
        }
    }

    fn sample_rule() -> RssRule {
        RssRule {
            tags: Vec::new(),
            throttle_name: None,
            seed_limits: None,
            id: 7,
            name: "weekly show".to_string(),
            enabled: true,
            feed_ids: vec![1, 2],
            priority: 10,
            match_title: Some("show.*1080p".to_string()),
            exclude_title: Some("cam|telesync".to_string()),
            min_size_bytes: Some(100 * 1024 * 1024),
            max_size_bytes: Some(8 * 1024 * 1024 * 1024),
            episode: Some(EpisodeFilter {
                season: Some(2),
                from_episode: 1,
                to_episode: 12,
            }),
            save_path: Some("/downloads/show".to_string()),
            label: Some("show".to_string()),
            add_paused: false,
            cooldown_secs: Some(3600),
        }
    }

    // Structs serialize fields in declaration order, so comparing the
    // re-serialized JSON is a stable full-field round-trip check.
    fn assert_round_trip<T: Serialize + serde::de::DeserializeOwned>(value: &T) {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn feed_serde_round_trip() {
        assert_round_trip(&sample_feed());
    }

    #[test]
    fn feed_optional_fields_round_trip() {
        let feed = Feed {
            alias: None,
            last_fetch_at: None,
            last_success_at: None,
            last_error: None,
            etag: None,
            last_modified: None,
            ..sample_feed()
        };
        assert_round_trip(&feed);
    }

    #[test]
    fn rss_rule_serde_round_trip() {
        assert_round_trip(&sample_rule());
    }

    #[test]
    fn rss_rule_minimal_serde_round_trip() {
        let rule = RssRule {
            tags: Vec::new(),
            throttle_name: None,
            seed_limits: None,
            feed_ids: Vec::new(),
            match_title: None,
            exclude_title: None,
            min_size_bytes: None,
            max_size_bytes: None,
            episode: None,
            save_path: None,
            label: None,
            cooldown_secs: None,
            ..sample_rule()
        };
        assert_round_trip(&rule);
    }

    #[test]
    fn rss_config_serde_round_trip() {
        assert_round_trip(&RssConfig::default());
        let custom = RssConfig {
            enabled: false,
            default_refresh_interval_secs: 60,
            max_items_per_feed: 10,
            request_timeout_secs: 5,
            max_response_bytes: 1024,
        };
        assert_round_trip(&custom);
    }

    #[test]
    fn rss_config_default_values() {
        let config = RssConfig::default();
        assert!(config.enabled);
        assert_eq!(config.default_refresh_interval_secs, 1800);
        assert_eq!(config.max_items_per_feed, 200);
        assert_eq!(config.request_timeout_secs, 30);
        assert_eq!(config.max_response_bytes, 5 * 1024 * 1024);
    }

    #[test]
    fn download_status_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&DownloadStatus::Added).unwrap(),
            "\"added\""
        );
        assert_eq!(
            serde_json::to_string(&DownloadStatus::Duplicate).unwrap(),
            "\"duplicate\""
        );
        assert_eq!(
            serde_json::to_string(&DownloadStatus::Failed).unwrap(),
            "\"failed\""
        );
        let back: DownloadStatus = serde_json::from_str("\"duplicate\"").unwrap();
        assert_eq!(back, DownloadStatus::Duplicate);
    }

    #[test]
    fn feed_item_with_download_record_round_trip() {
        let item = FeedItem {
            id: 42,
            feed_id: 1,
            guid: "guid-42".to_string(),
            title: "Show S02E05 1080p".to_string(),
            link: Some("https://example.com/items/42".to_string()),
            torrent_url: Some("https://example.com/items/42.torrent".to_string()),
            magnet: None,
            info_hash: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            size_bytes: Some(1_500_000_000),
            published_at: Some(1_700_000_000),
            fetched_at: 1_700_000_100,
            download: Some(DownloadRecord {
                torrent_hash: Some("0123456789ABCDEF0123456789ABCDEF01234567".to_string()),
                rule_id: Some(7),
                status: DownloadStatus::Added,
                error: None,
                at: 1_700_000_200,
            }),
        };
        assert_round_trip(&item);
    }

    #[test]
    fn old_rule_json_without_bridge_fields_loads_defaults() {
        // rss.json store version 1 predates the rules-engine bridge fields;
        // they must default, not fail (plan §6.10, AC-009).
        let json = r#"{
            "id": 3, "name": "weekly", "enabled": true, "feed_ids": [],
            "priority": 0, "match_title": "show", "exclude_title": null,
            "min_size_bytes": null, "max_size_bytes": null, "episode": null,
            "save_path": "/downloads", "label": "tv", "add_paused": false,
            "cooldown_secs": 3600
        }"#;
        let rule: RssRule = serde_json::from_str(json).unwrap();
        assert!(rule.tags.is_empty());
        assert_eq!(rule.throttle_name, None);
        assert_eq!(rule.seed_limits, None);
    }

    #[test]
    fn old_download_action_json_loads_defaults() {
        let json = r#"{"save_path":null,"label":"tv","paused":true}"#;
        let action: DownloadAction = serde_json::from_str(json).unwrap();
        assert!(action.tags.is_empty());
        assert_eq!(action.throttle_name, None);
        assert_eq!(action.seed_limits, None);
    }

    #[test]
    fn bridge_fields_round_trip() {
        let limits = RssSeedLimits {
            ratio: Some(3.0),
            seeding_time_secs: Some(0),
            inactive_secs: None,
            action: Some("throttle:slow".to_string()),
        };
        let json = serde_json::to_string(&limits).unwrap();
        let back: RssSeedLimits = serde_json::from_str(&json).unwrap();
        assert_eq!(back, limits);
        let action = DownloadAction {
            save_path: None,
            label: Some("tv".to_string()),
            paused: false,
            tags: vec!["rss".to_string()],
            throttle_name: Some("seed".to_string()),
            seed_limits: Some(limits),
        };
        assert_round_trip(&action);
    }
}
