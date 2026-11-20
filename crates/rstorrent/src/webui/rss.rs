//! REST handlers for the RSS subsystem (`/api/rss`).
//!
//! Every endpoint sits behind the shared `require_auth` layer and additionally
//! demands a trusted caller (a configured, non-empty bearer token); untrusted
//! callers get 403 even on reads. When the daemon runs without RSS
//! (`WebuiState::rss` is `None`) every endpoint answers 503.
//!
//! The manager is internally locked with short synchronous sections, so CRUD
//! handlers call it directly (no `spawn_blocking`); refresh/download handlers
//! await the service, which never holds a lock across an `.await`.
//!
//! Error mapping: `NotFound` -> 404, `InvalidUrl`/`InvalidRule` -> 400,
//! `DuplicateFeed`/`ItemAlreadyProcessed` -> 409, `Store` -> 500 and
//! `Fetch`/`Parse` -> 502 (upstream failures). Refresh endpoints usually report
//! fetch/parse errors inside a 200 [`RefreshOutcome`] instead of failing.
//!
//! Query strings are parsed by hand: the workspace axum build disables the
//! `query` feature, so `axum::extract::Query` is unavailable and this layer
//! must not add dependencies.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use rt_rss::{
    DownloadResult, DownloadStatus, Feed, FeedId, FeedItem, FeedPatch, HistoryEntry, ItemFilter,
    RefreshOutcome, RssConfig, RssError, RssRule, RuleId,
};
use serde::{Deserialize, Serialize};

use crate::rss::RssService;
use crate::toml_config::{RssTomlConfig, RtorrentTomlConfig};
use crate::webui::{Trusted, WebuiState};

/// Default page size for `GET /rss/items` (mirrors the UI client default).
const DEFAULT_ITEMS_LIMIT: usize = 50;
/// Hard cap on the items page size; larger requests are clamped, not rejected.
const MAX_ITEMS_LIMIT: usize = 200;
/// Default number of history entries returned by `GET /rss/history`.
const DEFAULT_HISTORY_LIMIT: usize = 100;
/// Hard cap on the history page size.
const MAX_HISTORY_LIMIT: usize = 500;
/// Default match cap for `POST /rss/rules/test` when `limit` is omitted.
const DEFAULT_RULE_TEST_LIMIT: usize = 50;

/// JSON error body shared by every RSS endpoint.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

type ApiError = (StatusCode, Json<ErrorBody>);

/// Paged response of `GET /rss/items` (mirrors the UI client `ItemsPage`).
#[derive(Debug, Serialize)]
pub struct ItemsPage {
    pub items: Vec<FeedItem>,
    pub total: usize,
}

/// Response of `POST /rss/items/{id}/download` (mirrors `DownloadOutcome`).
#[derive(Debug, Serialize)]
pub struct DownloadOutcome {
    pub status: DownloadStatus,
    pub torrent_hash: Option<String>,
    pub error: Option<String>,
}

impl From<DownloadResult> for DownloadOutcome {
    fn from(result: DownloadResult) -> Self {
        match result {
            DownloadResult::Added(hash) => Self {
                status: DownloadStatus::Added,
                torrent_hash: Some(hash),
                error: None,
            },
            DownloadResult::Duplicate(hash) => Self {
                status: DownloadStatus::Duplicate,
                torrent_hash: Some(hash),
                error: None,
            },
            DownloadResult::Failed(message) => Self {
                status: DownloadStatus::Failed,
                torrent_hash: None,
                error: Some(message),
            },
        }
    }
}

/// Response of `POST /rss/rules/test` (mirrors `RuleTestResult`).
#[derive(Debug, Serialize)]
pub struct RuleTestResult {
    pub matches: Vec<FeedItem>,
}

/// Request body of `POST /rss/feeds` (mirrors `CreateFeedRequest`).
#[derive(Debug, Deserialize)]
pub struct CreateFeedRequest {
    pub url: String,
    pub alias: Option<String>,
    pub refresh_interval_secs: Option<u64>,
}

/// Request body of `POST /rss/rules/test` (mirrors `RuleTestRequest`).
#[derive(Debug, Deserialize)]
pub struct RuleTestRequest {
    pub rule: RssRule,
    pub feed_id: Option<FeedId>,
    pub limit: Option<usize>,
}

/// Request body of `PUT /rss/settings` (mirrors `SaveSettingsRequest`).
#[derive(Debug, Deserialize)]
pub struct SaveSettingsRequest {
    pub config: RssConfig,
    #[serde(default)]
    pub save_file: bool,
}

fn error(status: StatusCode, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
}

fn rss_error(err: RssError) -> ApiError {
    let status = match &err {
        RssError::NotFound(..) => StatusCode::NOT_FOUND,
        RssError::InvalidUrl(_) | RssError::InvalidRule(_) => StatusCode::BAD_REQUEST,
        RssError::DuplicateFeed(_) | RssError::ItemAlreadyProcessed(_) => StatusCode::CONFLICT,
        RssError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
        RssError::Fetch(_) | RssError::Parse(_) => StatusCode::BAD_GATEWAY,
    };
    error(status, err.to_string())
}

/// Trust gate plus service availability, shared by every handler. The trust
/// check runs first so untrusted callers never learn whether RSS is enabled.
fn service(state: &WebuiState, trusted: Trusted) -> Result<Arc<RssService>, ApiError> {
    if !trusted.0 {
        return Err(error(
            StatusCode::FORBIDDEN,
            "trusted authentication required for RSS endpoints",
        ));
    }
    state.rss().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "RSS service is unavailable",
        )
    })
}

/// RSS routes, merged into the gated `/api` router by [`super::router`].
pub fn routes() -> Router<WebuiState> {
    Router::new()
        .route("/rss/feeds", get(list_feeds).post(create_feed))
        .route("/rss/feeds/{id}", put(update_feed).delete(delete_feed))
        .route("/rss/feeds/{id}/refresh", post(refresh_feed))
        .route("/rss/refresh", post(refresh_all))
        .route("/rss/items", get(list_items))
        .route("/rss/items/{id}/download", post(download_item))
        .route("/rss/rules", get(list_rules).post(create_rule))
        .route("/rss/rules/test", post(test_rule))
        .route("/rss/rules/{id}", put(update_rule).delete(delete_rule))
        .route("/rss/history", get(list_history).delete(clear_history))
        .route("/rss/settings", get(get_settings).put(update_settings))
}

async fn list_feeds(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<Vec<Feed>>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.rss.feeds()))
}

async fn create_feed(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(req): Json<CreateFeedRequest>,
) -> Result<(StatusCode, Json<Feed>), ApiError> {
    let svc = service(&state, trusted)?;
    let feed = svc
        .rss
        .add_feed(&req.url, req.alias, req.refresh_interval_secs)
        .map_err(rss_error)?;
    Ok((StatusCode::CREATED, Json(feed)))
}

async fn update_feed(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<FeedId>,
    Json(patch): Json<FeedPatch>,
) -> Result<Json<Feed>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.rss.update_feed(id, patch).map_err(rss_error)?))
}

async fn delete_feed(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<FeedId>,
) -> Result<StatusCode, ApiError> {
    let svc = service(&state, trusted)?;
    svc.rss.remove_feed(id).map_err(rss_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn refresh_feed(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<FeedId>,
) -> Result<Json<RefreshOutcome>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.refresh_feed(id).await.map_err(rss_error)?))
}

async fn refresh_all(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<Vec<RefreshOutcome>>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.refresh_all().await))
}

async fn list_items(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    RawQuery(raw): RawQuery,
) -> Result<Json<ItemsPage>, ApiError> {
    let svc = service(&state, trusted)?;
    let params = parse_query(raw.as_deref());
    let filter = ItemFilter {
        feed_id: params.get("feed_id").and_then(|value| value.parse().ok()),
        search: params
            .get("search")
            .filter(|value| !value.is_empty())
            .cloned(),
        limit: query_limit(&params, "limit", DEFAULT_ITEMS_LIMIT, MAX_ITEMS_LIMIT),
        offset: params
            .get("offset")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
    };
    let (items, total) = svc.rss.items(&filter);
    Ok(Json(ItemsPage { items, total }))
}

async fn download_item(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<u64>,
) -> Result<Json<DownloadOutcome>, ApiError> {
    let svc = service(&state, trusted)?;
    // The service records the outcome (history + cooldown) itself.
    let result = svc.download_item(id).await.map_err(rss_error)?;
    Ok(Json(DownloadOutcome::from(result)))
}

async fn list_rules(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<Vec<RssRule>>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.rss.rules()))
}

async fn create_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(rule): Json<RssRule>,
) -> Result<(StatusCode, Json<RssRule>), ApiError> {
    let svc = service(&state, trusted)?;
    let created = svc.rss.add_rule(rule).map_err(rss_error)?;
    svc.apply_unprocessed().await;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn update_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<RuleId>,
    Json(rule): Json<RssRule>,
) -> Result<Json<RssRule>, ApiError> {
    let svc = service(&state, trusted)?;
    let updated = svc.rss.update_rule(id, rule).map_err(rss_error)?;
    svc.apply_unprocessed().await;
    Ok(Json(updated))
}

async fn delete_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<RuleId>,
) -> Result<StatusCode, ApiError> {
    let svc = service(&state, trusted)?;
    svc.rss.remove_rule(id).map_err(rss_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn test_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(req): Json<RuleTestRequest>,
) -> Result<Json<RuleTestResult>, ApiError> {
    let svc = service(&state, trusted)?;
    let limit = req.limit.unwrap_or(DEFAULT_RULE_TEST_LIMIT);
    let matches = svc
        .rss
        .test_rule(&req.rule, req.feed_id, limit)
        .map_err(rss_error)?;
    Ok(Json(RuleTestResult { matches }))
}

async fn list_history(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    RawQuery(raw): RawQuery,
) -> Result<Json<Vec<HistoryEntry>>, ApiError> {
    let svc = service(&state, trusted)?;
    let params = parse_query(raw.as_deref());
    let limit = query_limit(&params, "limit", DEFAULT_HISTORY_LIMIT, MAX_HISTORY_LIMIT);
    Ok(Json(svc.rss.history(limit)))
}

async fn clear_history(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<StatusCode, ApiError> {
    let svc = service(&state, trusted)?;
    svc.rss.clear_history();
    Ok(StatusCode::NO_CONTENT)
}

async fn get_settings(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<RssConfig>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.rss.config()))
}

async fn update_settings(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(req): Json<SaveSettingsRequest>,
) -> Result<Json<RssConfig>, ApiError> {
    let svc = service(&state, trusted)?;
    svc.rss.set_config(req.config.clone());
    if req.save_file {
        save_rss_section(&state, &req.config)?;
    }
    Ok(Json(req.config))
}

/// Writes the `[rss]` section into the daemon TOML file, mirroring
/// `rest::update_config`. A missing file starts from the default config so
/// `save_file` can create it; read/parse/save failures are server-side file
/// problems and surface as 500.
fn save_rss_section(state: &WebuiState, config: &RssConfig) -> Result<(), ApiError> {
    let path = state
        .config_path()
        .unwrap_or_else(|| PathBuf::from("rstorrent.toml"));
    let io_error = |err: std::io::Error| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{}: {err}", path.display()),
        )
    };
    let mut cfg = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(io_error)?;
        RtorrentTomlConfig::parse(&content).map_err(|err| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("parse {}: {err}", path.display()),
            )
        })?
    } else {
        RtorrentTomlConfig::default()
    };
    cfg.rss = RssTomlConfig::from_rss_config(config);
    cfg.save_to_file(&path).map_err(io_error)?;
    Ok(())
}

/// Reads a numeric query parameter, falling back to `default` when missing or
/// unparseable and clamping to `max`.
fn query_limit(params: &HashMap<String, String>, key: &str, default: usize, max: usize) -> usize {
    params
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
        .min(max)
}

/// Parses an `x-www-form-urlencoded` query string into decoded key/value pairs
/// (last occurrence wins). Hand-rolled because the workspace axum build
/// disables the `query` feature (see the module docs).
fn parse_query(raw: Option<&str>) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in raw.unwrap_or_default().split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key), percent_decode(value));
    }
    params
}

/// Decodes `%XX` escapes and `+` as space; invalid escapes stay literal.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi << 4) | lo);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webui::router;
    use axum::body::Body;
    use axum::http::{header, Request};
    use rt_core::session::SessionManager;
    use rt_core::DownloadList;
    use rt_rpc::CommandMap;
    use rt_rss::store::Store;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, RwLock};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tower::ServiceExt;

    const MAGNET_HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    /// Self-cleaning temporary directory, unique per test invocation.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "rstorrent_webui_rss_{}_{}_{}",
                label,
                std::process::id(),
                id
            ));
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

    struct Harness {
        state: WebuiState,
        service: Arc<RssService>,
        list: Arc<RwLock<DownloadList>>,
    }

    fn plain_session() -> Arc<Mutex<SessionManager>> {
        Arc::new(Mutex::new(SessionManager::new()))
    }

    fn active_session(dir: &TempDir) -> Arc<Mutex<SessionManager>> {
        let session_dir = dir.path.join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        let mut sm = SessionManager::new();
        sm.set_path(session_dir.to_str().unwrap()).unwrap();
        sm.start().unwrap();
        Arc::new(Mutex::new(sm))
    }

    /// Builds a WebUI state backed by a real RSS service sharing one download
    /// list, so tests can assert on both the HTTP responses and the engine.
    fn harness(dir: &TempDir, token: Option<&str>, session: Arc<Mutex<SessionManager>>) -> Harness {
        let list = Arc::new(RwLock::new(DownloadList::new()));
        let service = RssService::new(
            dir.path.join("rss.json"),
            RssConfig::default(),
            Arc::clone(&list),
            session,
            None,
        )
        .unwrap();
        let state = WebuiState::new_test(
            Arc::clone(&list),
            Arc::new(RwLock::new(CommandMap::new())),
            token.map(str::to_string),
            Arc::new(rt_core::tiering::TieringManager::new(Default::default())),
            Arc::new(RwLock::new(rt_core::commands::tracker::DhtConfig::default())),
            None,
            Some(Arc::clone(&service)),
        );
        Harness {
            state,
            service,
            list,
        }
    }

    /// Seeds the store file before the service loads it.
    fn seed_items(dir: &TempDir, items: Vec<FeedItem>) {
        let next_item_id = items.iter().map(|item| item.id).max().unwrap_or(0) + 1;
        let store = Store {
            items,
            next_item_id,
            ..Store::default()
        };
        store.save(&dir.path.join("rss.json")).unwrap();
    }

    fn item(id: u64, title: &str, published: u64) -> FeedItem {
        FeedItem {
            id,
            feed_id: 1,
            guid: format!("guid-{id}"),
            title: title.to_string(),
            link: None,
            torrent_url: None,
            magnet: None,
            info_hash: None,
            size_bytes: None,
            published_at: Some(published),
            fetched_at: published,
            download: None,
        }
    }

    fn rule_json(match_title: &str) -> Value {
        json!({
            "id": 0,
            "name": "test rule",
            "enabled": true,
            "feed_ids": [],
            "priority": 0,
            "match_title": match_title,
            "exclude_title": null,
            "min_size_bytes": null,
            "max_size_bytes": null,
            "episode": null,
            "save_path": null,
            "label": null,
            "add_paused": false,
            "cooldown_secs": null
        })
    }

    fn config_json(interval: u64) -> Value {
        json!({
            "enabled": false,
            "default_refresh_interval_secs": interval,
            "max_items_per_feed": 10,
            "request_timeout_secs": 5,
            "max_response_bytes": 2 * 1024 * 1024
        })
    }

    fn request(
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token));
        }
        match body {
            Some(value) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(value).unwrap()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        }
    }

    async fn send(state: WebuiState, req: Request<Body>) -> (StatusCode, Value) {
        let response = router(state).oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    // -- Mock HTTP (one canned reply per connection) -------------------------

    fn http_response(status: &str, body: &[u8]) -> Vec<u8> {
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let mut out = head.into_bytes();
        out.extend_from_slice(body);
        out
    }

    async fn read_request_head(socket: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = socket.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..read]);
            if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    async fn spawn_server(replies: Vec<Vec<u8>>) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let mut heads = Vec::new();
            for reply in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                heads.push(read_request_head(&mut socket).await);
                let _ = socket.write_all(&reply).await;
            }
            heads
        });
        (base, handle)
    }

    async fn finish(handle: JoinHandle<Vec<String>>) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("mock server did not finish")
            .unwrap()
    }

    /// Minimal valid single-file torrent fixture.
    fn make_torrent(name: &str, payload: &[u8]) -> Vec<u8> {
        const PIECE_LENGTH: usize = 16384;
        let mut pieces = Vec::new();
        for chunk in payload.chunks(PIECE_LENGTH) {
            pieces.extend_from_slice(&lt_crypto::sha1::sha1_hash(chunk));
        }
        let mut out = Vec::new();
        out.push(b'd');
        out.extend_from_slice(b"4:infod");
        out.extend_from_slice(format!("6:lengthi{}e", payload.len()).as_bytes());
        out.extend_from_slice(format!("4:name{}:{}", name.len(), name).as_bytes());
        out.extend_from_slice(format!("12:piece lengthi{}e", PIECE_LENGTH).as_bytes());
        out.extend_from_slice(format!("6:pieces{}:", pieces.len()).as_bytes());
        out.extend_from_slice(&pieces);
        out.extend_from_slice(b"ee");
        out
    }

    fn torrent_info_hash(torrent: &[u8]) -> String {
        let marker = b"4:info";
        let start = torrent
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("fixture has an info dict")
            + marker.len();
        let info = &torrent[start..torrent.len() - 1];
        lt_crypto::sha1::sha1_hash(info)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn feed_xml(torrent_url: &str, guid: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>RSS Test</title>
<item><title>My Show S01E01 1080p</title><guid>{guid}</guid>
<pubDate>Wed, 15 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="{torrent_url}" length="1500000000" type="application/x-bittorrent"/></item>
</channel></rss>"#
        )
    }

    fn magnet_xml() -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>RSS Test</title>
<item><title>Other Movie 720p</title><guid>movie-1</guid>
<pubDate>Tue, 14 Nov 2023 06:00:00 GMT</pubDate>
<enclosure url="magnet:?xt=urn:btih:{MAGNET_HASH}&amp;dn=movie" type="application/x-bittorrent"/></item>
</channel></rss>"#
        )
    }

    // -- Feeds ---------------------------------------------------------------

    #[tokio::test]
    async fn feeds_crud_round_trip() {
        let dir = TempDir::new("feeds_crud");
        let h = harness(&dir, Some("t"), plain_session());

        let (status, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/feeds", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!([]));

        let (status, feed) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/feeds",
                Some("t"),
                Some(&json!({
                    "url": "https://example.com/rss.xml",
                    "alias": "Example",
                    "refresh_interval_secs": 600
                })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(feed["id"], json!(1));
        assert_eq!(feed["url"], json!("https://example.com/rss.xml"));
        assert_eq!(feed["alias"], json!("Example"));
        assert_eq!(feed["enabled"], json!(true));
        assert_eq!(feed["refresh_interval_secs"], json!(600));
        let id = feed["id"].as_u64().unwrap();

        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/feeds", Some("t"), None),
        )
        .await;
        assert_eq!(body.as_array().unwrap().len(), 1);

        let (status, updated) = send(
            h.state.clone(),
            request(
                "PUT",
                &format!("/api/rss/feeds/{id}"),
                Some("t"),
                Some(&json!({"enabled": false, "alias": "Renamed"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["enabled"], json!(false));
        assert_eq!(updated["alias"], json!("Renamed"));
        assert_eq!(updated["url"], json!("https://example.com/rss.xml"));

        let (status, _) = send(
            h.state.clone(),
            request(
                "PUT",
                "/api/rss/feeds/999",
                Some("t"),
                Some(&json!({"enabled": false})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = send(
            h.state.clone(),
            request("DELETE", &format!("/api/rss/feeds/{id}"), Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(body, Value::Null);

        let (status, _) = send(
            h.state.clone(),
            request("DELETE", &format!("/api/rss/feeds/{id}"), Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/feeds", Some("t"), None),
        )
        .await;
        assert_eq!(body, json!([]));
    }

    #[tokio::test]
    async fn feed_validation_rejects_bad_and_duplicate_urls() {
        let dir = TempDir::new("feed_validation");
        let h = harness(&dir, Some("t"), plain_session());

        for bad in ["not a url", "ftp://example.com/feed.xml"] {
            let (status, body) = send(
                h.state.clone(),
                request(
                    "POST",
                    "/api/rss/feeds",
                    Some("t"),
                    Some(&json!({"url": bad})),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "url {bad}");
            assert!(body["error"].as_str().unwrap().contains("invalid feed URL"));
        }

        let (status, feed) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/feeds",
                Some("t"),
                Some(&json!({"url": "https://example.com/rss.xml"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = feed["id"].as_u64().unwrap();

        let (status, body) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/feeds",
                Some("t"),
                Some(&json!({"url": "https://example.com/rss.xml"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["error"].is_string());

        let (status, _) = send(
            h.state.clone(),
            request(
                "PUT",
                &format!("/api/rss/feeds/{id}"),
                Some("t"),
                Some(&json!({"url": "bad url"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn refresh_feed_auto_adds_matched_torrent() {
        let dir = TempDir::new("refresh_add");
        let torrent = make_torrent("rss-file", b"webui rss payload");
        let hash = torrent_info_hash(&torrent);
        let (torrent_base, torrent_server) =
            spawn_server(vec![http_response("200 OK", &torrent)]).await;
        let torrent_url = format!("{torrent_base}/file.torrent");
        let xml = feed_xml(&torrent_url, "show-1");
        let (feed_base, feed_server) = spawn_server(vec![
            http_response("200 OK", xml.as_bytes()),
            http_response("200 OK", xml.as_bytes()),
        ])
        .await;

        let h = harness(&dir, Some("t"), active_session(&dir));
        let (_, feed) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/feeds",
                Some("t"),
                Some(&json!({"url": format!("{feed_base}/rss.xml")})),
            ),
        )
        .await;
        let feed_id = feed["id"].as_u64().unwrap();

        let save_path = dir.path.join("data");
        let mut rule = rule_json("show.*1080p");
        rule["save_path"] = json!(save_path.to_str().unwrap());
        rule["label"] = json!("show");
        rule["add_paused"] = json!(true);
        let (status, rule) = send(
            h.state.clone(),
            request("POST", "/api/rss/rules", Some("t"), Some(&rule)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let rule_id = rule["id"].as_u64().unwrap();

        let (status, outcome) = send(
            h.state.clone(),
            request(
                "POST",
                &format!("/api/rss/feeds/{feed_id}/refresh"),
                Some("t"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(outcome["feed_id"], json!(feed_id));
        assert_eq!(outcome["new_items"], json!(1));
        assert_eq!(outcome["error"], Value::Null);
        assert_eq!(outcome["matches"].as_array().unwrap().len(), 1);
        assert_eq!(outcome["matches"][0]["rule_id"], json!(rule_id));

        {
            let guard = h.list.read().unwrap();
            assert_eq!(guard.len(), 1);
            let download = guard.find_hex(&hash).expect("torrent must be in the list");
            let dl = download.read().unwrap();
            assert_eq!(dl.directory(), save_path.to_str().unwrap());
            assert_eq!(dl.custom_field(1), "show");
            assert!(
                !dl.is_active(),
                "add_paused rule must not start the torrent"
            );
        }

        let (_, history) = send(
            h.state.clone(),
            request("GET", "/api/rss/history", Some("t"), None),
        )
        .await;
        assert_eq!(history.as_array().unwrap().len(), 1);
        assert_eq!(history[0]["status"], json!("added"));
        assert_eq!(history[0]["torrent_hash"], json!(hash));
        assert_eq!(history[0]["rule_id"], json!(rule_id));

        let (_, items) = send(
            h.state.clone(),
            request("GET", "/api/rss/items", Some("t"), None),
        )
        .await;
        assert_eq!(items["items"][0]["download"]["status"], json!("added"));

        // Guid dedup: a second refresh stores nothing new.
        let (status, again) = send(
            h.state.clone(),
            request(
                "POST",
                &format!("/api/rss/feeds/{feed_id}/refresh"),
                Some("t"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(again["new_items"], json!(0));
        assert!(again["matches"].as_array().unwrap().is_empty());

        let (status, _) = send(
            h.state.clone(),
            request("POST", "/api/rss/feeds/999/refresh", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        assert_eq!(finish(feed_server).await.len(), 2);
        assert_eq!(finish(torrent_server).await.len(), 1);
    }

    #[tokio::test]
    async fn refresh_all_is_forced_and_skips_disabled_feeds() {
        let dir = TempDir::new("refresh_all");
        let (feed_base, feed_server) =
            spawn_server(vec![http_response("200 OK", magnet_xml().as_bytes())]).await;
        let h = harness(&dir, Some("t"), plain_session());
        let (_, feed) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/feeds",
                Some("t"),
                Some(&json!({"url": format!("{feed_base}/rss.xml")})),
            ),
        )
        .await;
        let feed_id = feed["id"].as_u64().unwrap();

        // Forced refresh ignores the (never fetched) schedule.
        let (status, outcomes) = send(
            h.state.clone(),
            request("POST", "/api/rss/refresh", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(outcomes.as_array().unwrap().len(), 1);
        assert_eq!(outcomes[0]["new_items"], json!(1));
        assert_eq!(outcomes[0]["error"], Value::Null);

        let (status, _) = send(
            h.state.clone(),
            request(
                "PUT",
                &format!("/api/rss/feeds/{feed_id}"),
                Some("t"),
                Some(&json!({"enabled": false})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, outcomes) = send(
            h.state.clone(),
            request("POST", "/api/rss/refresh", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(outcomes, json!([]));
        assert_eq!(finish(feed_server).await.len(), 1);
    }

    // -- Items ---------------------------------------------------------------

    #[tokio::test]
    async fn items_search_pagination_and_feed_filter() {
        let dir = TempDir::new("items_query");
        seed_items(
            &dir,
            vec![
                item(1, "Show ONE 1080p", 300),
                item(2, "movie", 200),
                item(3, "show two 720p", 100),
            ],
        );
        let h = harness(&dir, Some("t"), plain_session());

        let (status, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/items", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total"], json!(3));
        let ids: Vec<u64> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, [1, 2, 3]);

        // Case-insensitive search, percent-encoded and '+'-encoded spaces.
        for query in ["search=SHOW", "search=Show%20ONE", "search=Show+ONE"] {
            let (_, body) = send(
                h.state.clone(),
                request("GET", &format!("/api/rss/items?{query}"), Some("t"), None),
            )
            .await;
            if query == "search=SHOW" {
                assert_eq!(body["total"], json!(2));
                assert_eq!(body["items"][0]["id"], json!(1));
                assert_eq!(body["items"][1]["id"], json!(3));
            } else {
                assert_eq!(body["total"], json!(1), "query {query}");
                assert_eq!(body["items"][0]["id"], json!(1));
            }
        }

        let (_, body) = send(
            h.state.clone(),
            request(
                "GET",
                "/api/rss/items?search=show&limit=1&offset=1",
                Some("t"),
                None,
            ),
        )
        .await;
        assert_eq!(body["total"], json!(2));
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["id"], json!(3));

        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/items?feed_id=1", Some("t"), None),
        )
        .await;
        assert_eq!(body["total"], json!(3));
        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/items?feed_id=2", Some("t"), None),
        )
        .await;
        assert_eq!(body["total"], json!(0));
        assert!(body["items"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn items_limit_defaults_and_clamps() {
        let dir = TempDir::new("items_limit");
        let items: Vec<FeedItem> = (1..=201)
            .map(|id| item(id, &format!("Item {id}"), id))
            .collect();
        seed_items(&dir, items);
        let h = harness(&dir, Some("t"), plain_session());

        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/items", Some("t"), None),
        )
        .await;
        assert_eq!(body["total"], json!(201));
        assert_eq!(body["items"].as_array().unwrap().len(), 50);
        assert_eq!(body["items"][0]["id"], json!(201));

        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/items?limit=1000", Some("t"), None),
        )
        .await;
        assert_eq!(body["total"], json!(201));
        assert_eq!(body["items"].as_array().unwrap().len(), 200);

        // Unparseable numbers fall back to the defaults.
        let (_, body) = send(
            h.state.clone(),
            request(
                "GET",
                "/api/rss/items?limit=abc&offset=xyz",
                Some("t"),
                None,
            ),
        )
        .await;
        assert_eq!(body["items"].as_array().unwrap().len(), 50);
    }

    // -- Rules ---------------------------------------------------------------

    #[tokio::test]
    async fn rules_crud_round_trip() {
        let dir = TempDir::new("rules_crud");
        let h = harness(&dir, Some("t"), plain_session());

        let mut rule = rule_json("show.*1080p");
        rule["id"] = json!(999); // ignored: the store assigns the id
        let (status, created) = send(
            h.state.clone(),
            request("POST", "/api/rss/rules", Some("t"), Some(&rule)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["id"], json!(1));
        assert_eq!(created["match_title"], json!("show.*1080p"));
        let id = created["id"].as_u64().unwrap();

        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/rules", Some("t"), None),
        )
        .await;
        assert_eq!(body.as_array().unwrap().len(), 1);

        let mut renamed = rule_json("movie.*720p");
        renamed["name"] = json!("renamed");
        renamed["id"] = json!(42); // the path id wins
        let (status, updated) = send(
            h.state.clone(),
            request(
                "PUT",
                &format!("/api/rss/rules/{id}"),
                Some("t"),
                Some(&renamed),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["id"], json!(id));
        assert_eq!(updated["name"], json!("renamed"));
        assert_eq!(updated["match_title"], json!("movie.*720p"));

        let (status, _) = send(
            h.state.clone(),
            request(
                "PUT",
                "/api/rss/rules/999",
                Some("t"),
                Some(&rule_json("anything")),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = send(
            h.state.clone(),
            request("DELETE", &format!("/api/rss/rules/{id}"), Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(
            h.state.clone(),
            request("DELETE", &format!("/api/rss/rules/{id}"), Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/rules", Some("t"), None),
        )
        .await;
        assert_eq!(body, json!([]));
    }

    #[tokio::test]
    async fn rules_reject_invalid_regex() {
        let dir = TempDir::new("rules_regex");
        let h = harness(&dir, Some("t"), plain_session());

        let (status, body) = send(
            h.state.clone(),
            request("POST", "/api/rss/rules", Some("t"), Some(&rule_json("("))),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("invalid rule"));

        let (_, created) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/rules",
                Some("t"),
                Some(&rule_json("show")),
            ),
        )
        .await;
        let id = created["id"].as_u64().unwrap();

        let (status, _) = send(
            h.state.clone(),
            request(
                "PUT",
                &format!("/api/rss/rules/{id}"),
                Some("t"),
                Some(&rule_json("[")),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (_, rules) = send(
            h.state.clone(),
            request("GET", "/api/rss/rules", Some("t"), None),
        )
        .await;
        assert_eq!(rules[0]["match_title"], json!("show"));

        let (status, _) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/rules/test",
                Some("t"),
                Some(&json!({"rule": rule_json("("), "feed_id": null, "limit": null})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rules_test_is_a_dry_run() {
        let dir = TempDir::new("rules_test");
        seed_items(
            &dir,
            vec![
                item(1, "Show A 1080p", 300),
                item(2, "Show B 1080p", 200),
                item(3, "movie", 100),
            ],
        );
        let h = harness(&dir, Some("t"), plain_session());

        let (status, body) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/rules/test",
                Some("t"),
                Some(&json!({"rule": rule_json("show.*1080p"), "feed_id": null, "limit": null})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let matches = body["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0]["id"], json!(1));
        assert_eq!(matches[1]["id"], json!(2));

        let (_, body) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/rules/test",
                Some("t"),
                Some(&json!({"rule": rule_json("show.*1080p"), "feed_id": null, "limit": 1})),
            ),
        )
        .await;
        assert_eq!(body["matches"].as_array().unwrap().len(), 1);
        assert_eq!(body["matches"][0]["id"], json!(1));

        let (_, body) = send(
            h.state.clone(),
            request(
                "POST",
                "/api/rss/rules/test",
                Some("t"),
                Some(&json!({"rule": rule_json("show.*1080p"), "feed_id": 2, "limit": null})),
            ),
        )
        .await;
        assert!(body["matches"].as_array().unwrap().is_empty());

        // Dry run: nothing was downloaded or recorded.
        let (_, items) = send(
            h.state.clone(),
            request("GET", "/api/rss/items", Some("t"), None),
        )
        .await;
        for item in items["items"].as_array().unwrap() {
            assert_eq!(item["download"], Value::Null);
        }
        assert!(h.list.read().unwrap().is_empty());
    }

    // -- Download / history --------------------------------------------------

    #[tokio::test]
    async fn manual_download_added_duplicate_and_conflict() {
        let dir = TempDir::new("manual_download");
        let torrent = make_torrent("manual-file", b"manual payload");
        let hash = torrent_info_hash(&torrent);
        let reply = http_response("200 OK", &torrent);
        let (base, torrent_server) = spawn_server(vec![reply.clone(), reply]).await;
        let url = format!("{base}/file.torrent");
        let mut a = item(1, "Manual A", 300);
        a.torrent_url = Some(url.clone());
        let mut b = item(2, "Manual B", 200);
        b.torrent_url = Some(url);
        seed_items(&dir, vec![a, b]);
        let h = harness(&dir, Some("t"), active_session(&dir));

        let (status, body) = send(
            h.state.clone(),
            request("POST", "/api/rss/items/1/download", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], json!("added"));
        assert_eq!(body["torrent_hash"], json!(hash));
        assert_eq!(body["error"], Value::Null);

        // Same torrent via a second item: the engine dedups by info hash.
        let (status, body) = send(
            h.state.clone(),
            request("POST", "/api/rss/items/2/download", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], json!("duplicate"));
        assert_eq!(body["torrent_hash"], json!(hash));

        // Re-downloading a processed item is a conflict, not a second attempt.
        let (status, body) = send(
            h.state.clone(),
            request("POST", "/api/rss/items/1/download", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["error"].is_string());

        let (status, _) = send(
            h.state.clone(),
            request("POST", "/api/rss/items/999/download", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        assert_eq!(h.list.read().unwrap().len(), 1);
        let (_, history) = send(
            h.state.clone(),
            request("GET", "/api/rss/history", Some("t"), None),
        )
        .await;
        let entries = history.as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["status"], json!("duplicate"));
        assert_eq!(entries[1]["status"], json!("added"));
        assert_eq!(finish(torrent_server).await.len(), 2);
    }

    #[tokio::test]
    async fn history_list_limit_and_clear() {
        let dir = TempDir::new("history");
        seed_items(
            &dir,
            vec![
                item(1, "First", 100),
                item(2, "Second", 200),
                item(3, "Third", 300),
            ],
        );
        let h = harness(&dir, Some("t"), plain_session());
        for id in 1..=3 {
            h.service
                .rss
                .record_download(id, None, DownloadResult::Added(format!("{id:040x}")));
        }

        let (status, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/history", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let entries = body.as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["item_id"], json!(3));
        assert_eq!(entries[2]["item_id"], json!(1));
        assert_eq!(entries[0]["status"], json!("added"));

        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/history?limit=2", Some("t"), None),
        )
        .await;
        assert_eq!(body.as_array().unwrap().len(), 2);
        assert_eq!(body[0]["item_id"], json!(3));

        // Over-limit requests are clamped to the 500-entry cap.
        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/history?limit=1000", Some("t"), None),
        )
        .await;
        assert_eq!(body.as_array().unwrap().len(), 3);

        let (status, _) = send(
            h.state.clone(),
            request("DELETE", "/api/rss/history", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/history", Some("t"), None),
        )
        .await;
        assert_eq!(body, json!([]));
    }

    // -- Settings ------------------------------------------------------------

    #[tokio::test]
    async fn settings_round_trip_and_save_file() {
        let dir = TempDir::new("settings");
        let h = harness(&dir, Some("t"), plain_session());
        let toml_path = dir.path.join("rstorrent.toml");
        // Pre-existing file with another section: save_file must preserve it.
        std::fs::write(&toml_path, "[network]\nport = 6881\n").unwrap();
        *h.state.config_path.write().unwrap() = Some(toml_path.clone());

        let (status, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/settings", Some("t"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["enabled"], json!(true));
        assert_eq!(body["default_refresh_interval_secs"], json!(1800));

        let config = config_json(123);
        let (status, body) = send(
            h.state.clone(),
            request(
                "PUT",
                "/api/rss/settings",
                Some("t"),
                Some(&json!({"config": config, "save_file": true})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, config);

        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/settings", Some("t"), None),
        )
        .await;
        assert_eq!(body, config);

        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(content.contains("[rss]"));
        assert!(content.contains("enabled = false"));
        assert!(content.contains("refresh_interval_secs = 123"));
        assert!(content.contains("max_response_mb = 2"));
        let parsed = RtorrentTomlConfig::parse(&content).unwrap();
        assert_eq!(parsed.network.port, Some(6881));
        let applied = parsed.rss.to_rss_config();
        assert!(!applied.enabled);
        assert_eq!(applied.default_refresh_interval_secs, 123);
        assert_eq!(applied.max_items_per_feed, 10);
        assert_eq!(applied.request_timeout_secs, 5);
        assert_eq!(applied.max_response_bytes, 2 * 1024 * 1024);

        // save_file=false applies in memory but leaves the file untouched.
        let (status, _) = send(
            h.state.clone(),
            request(
                "PUT",
                "/api/rss/settings",
                Some("t"),
                Some(&json!({"config": config_json(456), "save_file": false})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), content);
        let (_, body) = send(
            h.state.clone(),
            request("GET", "/api/rss/settings", Some("t"), None),
        )
        .await;
        assert_eq!(body["default_refresh_interval_secs"], json!(456));

        // A missing file is created from the default config.
        let created = dir.path.join("created.toml");
        *h.state.config_path.write().unwrap() = Some(created.clone());
        let (status, _) = send(
            h.state.clone(),
            request(
                "PUT",
                "/api/rss/settings",
                Some("t"),
                Some(&json!({"config": config_json(789), "save_file": true})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created_content = std::fs::read_to_string(&created).unwrap();
        assert!(created_content.contains("[rss]"));
        assert!(created_content.contains("refresh_interval_secs = 789"));
    }

    // -- Auth / availability -------------------------------------------------

    #[tokio::test]
    async fn untrusted_caller_is_forbidden_on_every_endpoint() {
        let dir = TempDir::new("untrusted");
        // No token configured: require_auth lets the request through but
        // marks it untrusted, and every RSS endpoint must answer 403.
        let h = harness(&dir, None, plain_session());
        let rule = rule_json("show");
        let cases: Vec<(&str, &str, Option<Value>)> = vec![
            ("GET", "/api/rss/feeds", None),
            (
                "POST",
                "/api/rss/feeds",
                Some(json!({"url": "https://example.com/rss.xml"})),
            ),
            ("PUT", "/api/rss/feeds/1", Some(json!({"enabled": false}))),
            ("DELETE", "/api/rss/feeds/1", None),
            ("POST", "/api/rss/feeds/1/refresh", None),
            ("POST", "/api/rss/refresh", None),
            ("GET", "/api/rss/items", None),
            ("POST", "/api/rss/items/1/download", None),
            ("GET", "/api/rss/rules", None),
            ("POST", "/api/rss/rules", Some(rule.clone())),
            ("PUT", "/api/rss/rules/1", Some(rule.clone())),
            ("DELETE", "/api/rss/rules/1", None),
            (
                "POST",
                "/api/rss/rules/test",
                Some(json!({"rule": rule, "feed_id": null, "limit": null})),
            ),
            ("GET", "/api/rss/history", None),
            ("DELETE", "/api/rss/history", None),
            ("GET", "/api/rss/settings", None),
            (
                "PUT",
                "/api/rss/settings",
                Some(json!({"config": config_json(60), "save_file": false})),
            ),
        ];
        assert_eq!(cases.len(), 17);
        for (method, uri, body) in cases {
            let (status, body) =
                send(h.state.clone(), request(method, uri, None, body.as_ref())).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
            assert!(body["error"].is_string(), "{method} {uri}");
        }
        // Nothing was mutated by the rejected calls.
        assert!(h.service.rss.feeds().is_empty());
        assert!(h.service.rss.rules().is_empty());
    }

    #[tokio::test]
    async fn missing_or_wrong_token_is_unauthorized() {
        let dir = TempDir::new("auth");
        let h = harness(&dir, Some("s3cret"), plain_session());
        let (status, _) = send(
            h.state.clone(),
            request("GET", "/api/rss/feeds", None, None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = send(
            h.state.clone(),
            request("GET", "/api/rss/feeds", Some("wrong"), None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = send(
            h.state.clone(),
            request("GET", "/api/rss/feeds", Some("s3cret"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_service_is_unavailable() {
        let state = WebuiState::new_test(
            Arc::new(RwLock::new(DownloadList::new())),
            Arc::new(RwLock::new(CommandMap::new())),
            Some("t".to_string()),
            Arc::new(rt_core::tiering::TieringManager::new(Default::default())),
            Arc::new(RwLock::new(rt_core::commands::tracker::DhtConfig::default())),
            None,
            None,
        );
        for (method, uri) in [
            ("GET", "/api/rss/feeds"),
            ("POST", "/api/rss/refresh"),
            ("GET", "/api/rss/settings"),
        ] {
            let (status, body) = send(state.clone(), request(method, uri, Some("t"), None)).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{method} {uri}");
            assert!(body["error"].is_string(), "{method} {uri}");
        }
    }
}
