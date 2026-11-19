//! UI-side mirror of the RSS backend contract (`rt-rss`): DTO shapes, the RSS
//! page store and a REST client for `/api/rss`. DTOs are duplicated here by
//! repo convention (see `store.rs` vs the backend conversion layer) instead of
//! sharing a common crate.
//!
//! T8 scaffolding: the panels and this client are wired into the app by
//! T9-T13, so nothing here is reachable from `main` yet.
#![allow(dead_code)]

use gloo_net::http::{Method, RequestBuilder, Response};
use leptos::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::api::ApiError;
use super::store::AppState;

pub type FeedId = u64;
pub type RuleId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feed {
    pub id: FeedId,
    pub url: String,
    pub alias: Option<String>,
    pub enabled: bool,
    pub refresh_interval_secs: u64,
    pub last_fetch_at: Option<u64>,
    pub last_success_at: Option<u64>,
    pub last_error: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedItem {
    pub id: u64,
    pub feed_id: FeedId,
    pub guid: String,
    pub title: String,
    pub link: Option<String>,
    pub torrent_url: Option<String>,
    pub magnet: Option<String>,
    pub info_hash: Option<String>,
    pub size_bytes: Option<u64>,
    pub published_at: Option<u64>,
    pub fetched_at: u64,
    pub download: Option<DownloadRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub torrent_hash: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RssRule {
    pub id: RuleId,
    pub name: String,
    pub enabled: bool,
    pub feed_ids: Vec<FeedId>,
    pub priority: i32,
    pub match_title: Option<String>,
    pub exclude_title: Option<String>,
    pub min_size_bytes: Option<u64>,
    pub max_size_bytes: Option<u64>,
    pub episode: Option<EpisodeFilter>,
    pub save_path: Option<String>,
    pub label: Option<String>,
    pub add_paused: bool,
    pub cooldown_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeFilter {
    pub season: Option<u32>,
    pub from_episode: u32,
    pub to_episode: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadAction {
    pub save_path: Option<String>,
    pub label: Option<String>,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchedItem {
    pub item: FeedItem,
    pub rule_id: RuleId,
    pub action: DownloadAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefreshOutcome {
    pub feed_id: FeedId,
    pub new_items: usize,
    pub matches: Vec<MatchedItem>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedPatch {
    pub url: Option<String>,
    pub alias: Option<String>,
    pub enabled: Option<bool>,
    pub refresh_interval_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemFilter {
    pub feed_id: Option<FeedId>,
    pub search: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

impl Default for ItemFilter {
    fn default() -> Self {
        // Mirrors the backend default page size for GET /rss/items.
        Self {
            feed_id: None,
            search: None,
            limit: 50,
            offset: 0,
        }
    }
}

/// Paged response of `GET /rss/items`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemsPage {
    pub items: Vec<FeedItem>,
    pub total: usize,
}

/// Response of `POST /rss/items/{id}/download`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadOutcome {
    pub status: DownloadStatus,
    pub torrent_hash: Option<String>,
    pub error: Option<String>,
}

/// Response of `POST /rss/rules/test`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleTestResult {
    pub matches: Vec<FeedItem>,
}

/// Request body of `POST /rss/feeds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFeedRequest {
    pub url: String,
    pub alias: Option<String>,
    pub refresh_interval_secs: Option<u64>,
}

/// Request body of `POST /rss/rules/test`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleTestRequest {
    pub rule: RssRule,
    pub feed_id: Option<FeedId>,
    pub limit: Option<usize>,
}

/// Request body of `PUT /rss/settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveSettingsRequest {
    pub config: RssConfig,
    pub save_file: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RssTab {
    #[default]
    Feeds,
    Items,
    Rules,
    History,
}

/// Reactive state of the RSS page. `reload` is bumped after a mutation so the
/// page can refetch the affected lists.
#[derive(Clone, Copy)]
pub struct RssPageStore {
    pub tab: RwSignal<RssTab>,
    pub feeds: RwSignal<Vec<Feed>>,
    pub items: RwSignal<Vec<FeedItem>>,
    pub items_total: RwSignal<usize>,
    pub rules: RwSignal<Vec<RssRule>>,
    pub history: RwSignal<Vec<HistoryEntry>>,
    pub config: RwSignal<RssConfig>,
    pub busy: RwSignal<bool>,
    pub search: RwSignal<String>,
    pub feed_filter: RwSignal<Option<u64>>,
    pub reload: RwSignal<u64>,
}

impl RssPageStore {
    pub fn new() -> Self {
        Self {
            tab: RwSignal::new(RssTab::default()),
            feeds: RwSignal::new(Vec::new()),
            items: RwSignal::new(Vec::new()),
            items_total: RwSignal::new(0),
            rules: RwSignal::new(Vec::new()),
            history: RwSignal::new(Vec::new()),
            config: RwSignal::new(RssConfig::default()),
            busy: RwSignal::new(false),
            search: RwSignal::new(String::new()),
            feed_filter: RwSignal::new(None),
            reload: RwSignal::new(0),
        }
    }
}

impl Default for RssPageStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// REST client for /api/rss (Bearer auth, same pattern as api.rs::fetch_json)
// ---------------------------------------------------------------------------

fn serialize_body<T: Serialize>(value: &T) -> Result<String, ApiError> {
    serde_json::to_string(value)
        .map_err(|err| ApiError::Other(format!("Invalid request payload: {err}")))
}

async fn rss_send(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<String>,
) -> Result<Response, ApiError> {
    let base = state.api_base_url.get();
    let token = state.api_token.get();
    let url = format!("{base}{path}");
    let mut builder = RequestBuilder::new(&url).method(method);
    if !token.is_empty() {
        builder = builder.header("Authorization", &format!("Bearer {token}"));
    }
    let request = match body {
        Some(json) => builder
            .header("Content-Type", "application/json")
            .body(json),
        None => builder.build(),
    }
    .map_err(|err| ApiError::Other(format!("Request failed: {err}")))?;
    let response = request
        .send()
        .await
        .map_err(|err| ApiError::Other(format!("Request failed: {err}")))?;
    if !response.ok() {
        let status = response.status();
        if status == 401 || status == 403 {
            return Err(ApiError::Unauthorized(status));
        }
        return Err(ApiError::Other(format!("Server returned HTTP {status}")));
    }
    Ok(response)
}

async fn rss_json<T: DeserializeOwned>(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<String>,
) -> Result<T, ApiError> {
    let response = rss_send(state, method, path, body).await?;
    response
        .json::<T>()
        .await
        .map_err(|err| ApiError::Other(format!("Invalid payload: {err}")))
}

/// For endpoints answering 204 No Content.
async fn rss_empty(state: &AppState, method: Method, path: &str) -> Result<(), ApiError> {
    rss_send(state, method, path, None).await?;
    Ok(())
}

/// Percent-encodes a query value (RFC 3986 unreserved set stays literal).
/// Hand-rolled instead of `js_sys::encode_uri_component` so query building is
/// unit-testable outside the browser.
fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[usize::from(byte >> 4)] as char);
            encoded.push(HEX[usize::from(byte & 0x0f)] as char);
        }
    }
    encoded
}

fn items_query(filter: &ItemFilter) -> String {
    let mut params = Vec::new();
    if let Some(feed_id) = filter.feed_id {
        params.push(format!("feed_id={feed_id}"));
    }
    if let Some(search) = filter.search.as_deref().filter(|s| !s.is_empty()) {
        params.push(format!("search={}", percent_encode(search)));
    }
    params.push(format!("limit={}", filter.limit));
    params.push(format!("offset={}", filter.offset));
    format!("?{}", params.join("&"))
}

fn history_path(limit: Option<usize>) -> String {
    match limit {
        Some(limit) => format!("/api/rss/history?limit={limit}"),
        None => "/api/rss/history".to_string(),
    }
}

pub async fn rss_list_feeds(state: &AppState) -> Result<Vec<Feed>, ApiError> {
    rss_json(state, Method::GET, "/api/rss/feeds", None).await
}

pub async fn rss_create_feed(
    state: &AppState,
    url: &str,
    alias: Option<&str>,
    refresh_interval_secs: Option<u64>,
) -> Result<Feed, ApiError> {
    let body = CreateFeedRequest {
        url: url.to_string(),
        alias: alias.map(str::to_string),
        refresh_interval_secs,
    };
    rss_json(
        state,
        Method::POST,
        "/api/rss/feeds",
        Some(serialize_body(&body)?),
    )
    .await
}

pub async fn rss_update_feed(
    state: &AppState,
    id: FeedId,
    patch: &FeedPatch,
) -> Result<Feed, ApiError> {
    rss_json(
        state,
        Method::PUT,
        &format!("/api/rss/feeds/{id}"),
        Some(serialize_body(patch)?),
    )
    .await
}

pub async fn rss_delete_feed(state: &AppState, id: FeedId) -> Result<(), ApiError> {
    rss_empty(state, Method::DELETE, &format!("/api/rss/feeds/{id}")).await
}

pub async fn rss_refresh_feed(state: &AppState, id: FeedId) -> Result<RefreshOutcome, ApiError> {
    rss_json(
        state,
        Method::POST,
        &format!("/api/rss/feeds/{id}/refresh"),
        None,
    )
    .await
}

pub async fn rss_refresh_all(state: &AppState) -> Result<Vec<RefreshOutcome>, ApiError> {
    rss_json(state, Method::POST, "/api/rss/refresh", None).await
}

pub async fn rss_list_items(state: &AppState, filter: &ItemFilter) -> Result<ItemsPage, ApiError> {
    let path = format!("/api/rss/items{}", items_query(filter));
    rss_json(state, Method::GET, &path, None).await
}

pub async fn rss_download_item(state: &AppState, id: u64) -> Result<DownloadOutcome, ApiError> {
    rss_json(
        state,
        Method::POST,
        &format!("/api/rss/items/{id}/download"),
        None,
    )
    .await
}

pub async fn rss_list_rules(state: &AppState) -> Result<Vec<RssRule>, ApiError> {
    rss_json(state, Method::GET, "/api/rss/rules", None).await
}

pub async fn rss_create_rule(state: &AppState, rule: &RssRule) -> Result<RssRule, ApiError> {
    rss_json(
        state,
        Method::POST,
        "/api/rss/rules",
        Some(serialize_body(rule)?),
    )
    .await
}

pub async fn rss_update_rule(
    state: &AppState,
    id: RuleId,
    rule: &RssRule,
) -> Result<RssRule, ApiError> {
    rss_json(
        state,
        Method::PUT,
        &format!("/api/rss/rules/{id}"),
        Some(serialize_body(rule)?),
    )
    .await
}

pub async fn rss_delete_rule(state: &AppState, id: RuleId) -> Result<(), ApiError> {
    rss_empty(state, Method::DELETE, &format!("/api/rss/rules/{id}")).await
}

pub async fn rss_test_rule(
    state: &AppState,
    rule: &RssRule,
    feed_id: Option<FeedId>,
    limit: Option<usize>,
) -> Result<RuleTestResult, ApiError> {
    let body = RuleTestRequest {
        rule: rule.clone(),
        feed_id,
        limit,
    };
    rss_json(
        state,
        Method::POST,
        "/api/rss/rules/test",
        Some(serialize_body(&body)?),
    )
    .await
}

pub async fn rss_list_history(
    state: &AppState,
    limit: Option<usize>,
) -> Result<Vec<HistoryEntry>, ApiError> {
    rss_json(state, Method::GET, &history_path(limit), None).await
}

pub async fn rss_clear_history(state: &AppState) -> Result<(), ApiError> {
    rss_empty(state, Method::DELETE, "/api/rss/history").await
}

pub async fn rss_get_settings(state: &AppState) -> Result<RssConfig, ApiError> {
    rss_json(state, Method::GET, "/api/rss/settings", None).await
}

pub async fn rss_save_settings(
    state: &AppState,
    config: &RssConfig,
    save_file: bool,
) -> Result<RssConfig, ApiError> {
    let body = SaveSettingsRequest {
        config: config.clone(),
        save_file,
    };
    rss_json(
        state,
        Method::PUT,
        "/api/rss/settings",
        Some(serialize_body(&body)?),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_feed() -> Feed {
        Feed {
            id: 1,
            url: "https://example.com/rss.xml".to_string(),
            alias: Some("Example".to_string()),
            enabled: true,
            refresh_interval_secs: 1800,
            last_fetch_at: Some(1_700_000_000),
            last_success_at: Some(1_700_000_000),
            last_error: None,
            etag: Some("\"abc123\"".to_string()),
            last_modified: Some("Wed, 01 Jan 2025 00:00:00 GMT".to_string()),
            created_at: 1_699_000_000,
        }
    }

    fn sample_item() -> FeedItem {
        FeedItem {
            id: 10,
            feed_id: 1,
            guid: "guid-10".to_string(),
            title: "Show S02E05 1080p".to_string(),
            link: Some("https://example.com/items/10".to_string()),
            torrent_url: Some("https://example.com/items/10.torrent".to_string()),
            magnet: None,
            info_hash: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            size_bytes: Some(1_500_000_000),
            published_at: Some(1_699_999_000),
            fetched_at: 1_700_000_000,
            download: Some(DownloadRecord {
                torrent_hash: Some("aabbccdd".to_string()),
                rule_id: Some(3),
                status: DownloadStatus::Added,
                error: None,
                at: 1_700_000_100,
            }),
        }
    }

    fn sample_rule() -> RssRule {
        RssRule {
            id: 3,
            name: "Show 1080p".to_string(),
            enabled: true,
            feed_ids: vec![1, 2],
            priority: 10,
            match_title: Some("Show.*1080p".to_string()),
            exclude_title: Some("CAM".to_string()),
            min_size_bytes: Some(500_000_000),
            max_size_bytes: Some(5_000_000_000),
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

    fn sample_matched_item() -> MatchedItem {
        MatchedItem {
            item: sample_item(),
            rule_id: 3,
            action: DownloadAction {
                save_path: Some("/downloads/show".to_string()),
                label: None,
                paused: true,
            },
        }
    }

    fn round_trip<T>(value: &T)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value);
    }

    #[test]
    fn feed_round_trip() {
        round_trip(&sample_feed());
        let minimal = Feed {
            id: 2,
            url: "https://example.com/other.xml".to_string(),
            alias: None,
            enabled: false,
            refresh_interval_secs: 900,
            last_fetch_at: None,
            last_success_at: None,
            last_error: None,
            etag: None,
            last_modified: None,
            created_at: 1_699_000_001,
        };
        let json = serde_json::to_string(&minimal).unwrap();
        assert!(json.contains("\"alias\":null"));
        round_trip(&minimal);
    }

    #[test]
    fn feed_item_round_trip() {
        round_trip(&sample_item());
        let mut bare = sample_item();
        bare.download = None;
        bare.magnet = Some("magnet:?xt=urn:btih:0123456789abcdef".to_string());
        bare.link = None;
        bare.torrent_url = None;
        round_trip(&bare);
    }

    #[test]
    fn download_status_snake_case() {
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
    fn rss_rule_round_trip() {
        round_trip(&sample_rule());
        let minimal = RssRule {
            id: 0,
            name: "Catch all".to_string(),
            enabled: true,
            feed_ids: Vec::new(),
            priority: 0,
            match_title: None,
            exclude_title: None,
            min_size_bytes: None,
            max_size_bytes: None,
            episode: None,
            save_path: None,
            label: None,
            add_paused: false,
            cooldown_secs: None,
        };
        round_trip(&minimal);
    }

    #[test]
    fn matched_item_and_action_round_trip() {
        round_trip(&sample_matched_item());
        round_trip(&DownloadAction {
            save_path: None,
            label: Some("rss".to_string()),
            paused: false,
        });
        round_trip(&EpisodeFilter {
            season: None,
            from_episode: 3,
            to_episode: 24,
        });
    }

    #[test]
    fn history_entry_round_trip() {
        round_trip(&HistoryEntry {
            id: 7,
            at: 1_700_000_200,
            feed_id: 1,
            item_id: 10,
            title: "Show S02E05 1080p".to_string(),
            rule_id: Some(3),
            torrent_hash: Some("aabbccdd".to_string()),
            status: DownloadStatus::Duplicate,
            error: None,
        });
        round_trip(&HistoryEntry {
            id: 8,
            at: 1_700_000_300,
            feed_id: 1,
            item_id: 11,
            title: "Show S02E06 1080p".to_string(),
            rule_id: None,
            torrent_hash: None,
            status: DownloadStatus::Failed,
            error: Some("connection refused".to_string()),
        });
    }

    #[test]
    fn rss_config_defaults_and_round_trip() {
        let config = RssConfig::default();
        assert!(config.enabled);
        assert_eq!(config.default_refresh_interval_secs, 1800);
        assert_eq!(config.max_items_per_feed, 200);
        assert_eq!(config.request_timeout_secs, 30);
        assert_eq!(config.max_response_bytes, 5 * 1024 * 1024);
        round_trip(&config);
    }

    #[test]
    fn refresh_outcome_round_trip() {
        round_trip(&RefreshOutcome {
            feed_id: 1,
            new_items: 2,
            matches: vec![sample_matched_item()],
            error: None,
        });
        round_trip(&RefreshOutcome {
            feed_id: 2,
            new_items: 0,
            matches: Vec::new(),
            error: Some("HTTP 503".to_string()),
        });
    }

    #[test]
    fn feed_patch_round_trip() {
        let patch = FeedPatch {
            url: Some("https://example.com/new.xml".to_string()),
            alias: None,
            enabled: Some(false),
            refresh_interval_secs: None,
        };
        let json = serde_json::to_string(&patch).unwrap();
        assert!(json.contains("\"alias\":null"));
        round_trip(&patch);
        round_trip(&FeedPatch {
            url: None,
            alias: None,
            enabled: None,
            refresh_interval_secs: None,
        });
    }

    #[test]
    fn item_filter_defaults() {
        let filter = ItemFilter::default();
        assert_eq!(filter.feed_id, None);
        assert_eq!(filter.search, None);
        assert_eq!(filter.limit, 50);
        assert_eq!(filter.offset, 0);
        round_trip(&filter);
    }

    #[test]
    fn response_dto_round_trips() {
        round_trip(&ItemsPage {
            items: vec![sample_item()],
            total: 42,
        });
        round_trip(&DownloadOutcome {
            status: DownloadStatus::Failed,
            torrent_hash: None,
            error: Some("no torrent link".to_string()),
        });
        round_trip(&DownloadOutcome {
            status: DownloadStatus::Added,
            torrent_hash: Some("aabbccdd".to_string()),
            error: None,
        });
        round_trip(&RuleTestResult {
            matches: vec![sample_item()],
        });
    }

    #[test]
    fn request_dto_round_trips() {
        round_trip(&CreateFeedRequest {
            url: "https://example.com/rss.xml".to_string(),
            alias: Some("Example".to_string()),
            refresh_interval_secs: None,
        });
        round_trip(&RuleTestRequest {
            rule: sample_rule(),
            feed_id: Some(1),
            limit: Some(25),
        });
        round_trip(&SaveSettingsRequest {
            config: RssConfig::default(),
            save_file: true,
        });
    }

    #[test]
    fn percent_encode_ascii() {
        assert_eq!(percent_encode("AZaz09-_.~"), "AZaz09-_.~");
        assert_eq!(percent_encode("hello world"), "hello%20world");
        assert_eq!(percent_encode("a+b&c=d/e?f"), "a%2Bb%26c%3Dd%2Fe%3Ff");
        assert_eq!(percent_encode(""), "");
    }

    #[test]
    fn percent_encode_multibyte_utf8() {
        assert_eq!(percent_encode("łódź"), "%C5%82%C3%B3d%C5%BA");
        assert_eq!(percent_encode("🚀"), "%F0%9F%9A%80");
    }

    #[test]
    fn items_query_variants() {
        let filter = ItemFilter {
            feed_id: Some(7),
            search: Some("show 1080p".to_string()),
            limit: 25,
            offset: 50,
        };
        assert_eq!(
            items_query(&filter),
            "?feed_id=7&search=show%201080p&limit=25&offset=50"
        );
        assert_eq!(items_query(&ItemFilter::default()), "?limit=50&offset=0");
        let empty_search = ItemFilter {
            search: Some(String::new()),
            ..ItemFilter::default()
        };
        assert_eq!(items_query(&empty_search), "?limit=50&offset=0");
    }

    #[test]
    fn history_path_variants() {
        assert_eq!(history_path(None), "/api/rss/history");
        assert_eq!(history_path(Some(250)), "/api/rss/history?limit=250");
    }

    #[test]
    fn rss_tab_default() {
        assert_eq!(RssTab::default(), RssTab::Feeds);
    }
}
