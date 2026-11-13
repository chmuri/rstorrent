//! Native WebUI: a small axum HTTP server exposing the session over REST (and,
//! in a later increment, WebSocket) plus an embedded frontend.
//!
//! The server is opt-in: it only starts when `--webui-addr` or the TOML
//! `[webui] addr` key is set, so a default install never opens an extra port.
//! Authentication mirrors the gRPC/SCGI transports: a configured bearer token
//! must be presented (`Authorization: Bearer <token>`, or `?token=<token>` for
//! the browser WebSocket handshake). A valid token marks the caller trusted;
//! with no token configured the caller stays untrusted and is restricted to
//! `FLAG_UNTRUSTED_SAFE` commands, exactly like an unauthenticated TCP peer.

mod convert;
mod rest;
mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rt_core::DownloadList;
use rt_rpc::CommandMap;
use tokio::sync::{broadcast, watch};

use crate::cli::Cli;
use crate::setup::find_config_file;
use crate::toml_config::{RtorrentTomlConfig, WebuiTomlConfig};

/// Server-decided trust level for a WebUI request, carried as a request
/// extension. Never taken from the client payload.
#[derive(Clone, Copy, Debug)]
pub struct Trusted(pub bool);

/// Shared state handed to every WebUI handler.
#[derive(Clone)]
pub struct WebuiState {
    pub download_list: Arc<RwLock<DownloadList>>,
    pub command_map: Arc<RwLock<CommandMap>>,
    pub token: Option<String>,
    pub start_time: Instant,
    pub event_broadcaster: broadcast::Sender<ws::WebuiTorrentEvent>,
}

impl WebuiState {
    pub fn new(
        download_list: Arc<RwLock<DownloadList>>,
        command_map: Arc<RwLock<CommandMap>>,
        token: Option<String>,
    ) -> Self {
        // Mirror `rt-grpc/src/server.rs`: hook the download-list event listener
        // once and fan every event out to subscribed WebSocket clients.
        let (event_broadcaster, _) = broadcast::channel(256);
        let tx = event_broadcaster.clone();
        download_list
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .add_event_listener(Arc::new(move |event, download| {
                let guard = download.read().unwrap_or_else(|e| e.into_inner());
                let _ = tx.send(ws::WebuiTorrentEvent {
                    event_name: event.to_string(),
                    hash: guard.info_hash_hex(),
                    summary: convert::summary_from(&guard),
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                });
            }));
        Self {
            download_list,
            command_map,
            token,
            start_time: Instant::now(),
            event_broadcaster,
        }
    }
}

/// Bearer-token gate. Rejects invalid/missing tokens with 401 when a token is
/// configured; otherwise records the (untrusted) caller and lets the request
/// through so read-only `FLAG_UNTRUSTED_SAFE` commands still work.
async fn require_auth(State(state): State<WebuiState>, mut req: Request, next: Next) -> Response {
    let expected = state.token.as_deref();
    let presented = presented_token(&req);
    if !rt_rpc::auth::bearer_authorized(presented.as_deref(), expected) {
        return (StatusCode::UNAUTHORIZED, "invalid or missing bearer token").into_response();
    }
    let trusted = expected.is_some_and(|token| !token.is_empty());
    req.extensions_mut().insert(Trusted(trusted));
    next.run(req).await
}

/// Extract the presented credential: the `Authorization` header wins; the
/// `?token=` query parameter is the fallback browsers need for WebSockets.
fn presented_token(req: &Request) -> Option<String> {
    if let Some(value) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        return Some(value.to_string());
    }
    let query = req.uri().query()?;
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if key == "token" {
                return Some(format!("Bearer {}", value));
            }
        }
    }
    None
}

/// Build the WebUI router. The `/api` subtree is gated by [`require_auth`].
pub fn router(state: WebuiState) -> Router {
    let api = Router::new()
        .route("/torrents", get(rest::list_torrents))
        .route("/torrents/{hash}", get(rest::get_torrent))
        .route("/torrents/{hash}/start", post(rest::start_torrent))
        .route("/torrents/{hash}/stop", post(rest::stop_torrent))
        .route("/torrents/{hash}/remove", post(rest::remove_torrent))
        .route("/torrents/{hash}/recheck", post(rest::recheck_torrent))
        .route("/command", post(rest::execute_command))
        .route("/stats", get(rest::get_stats))
        .route("/version", get(rest::get_version))
        .route("/events", get(ws::events_ws))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state);
    Router::new().route("/", get(frontend)).nest("/api", api)
}

/// Serve the embedded single-page frontend at the root. The shell carries no
/// secrets; the browser supplies the bearer token for the gated `/api` calls.
async fn frontend() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

/// TLS identity for the WebUI HTTPS listener, mirroring the gRPC `[grpc]`
/// `tls_cert`/`tls_key` keys. Both paths must be present to enable TLS.
#[derive(Debug, Clone)]
pub struct WebuiTls {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// Serve the WebUI on `addr` until `shutdown` flips to `true` (or its sender is
/// dropped), then drain in-flight requests gracefully. With `tls` set the
/// listener terminates HTTPS via rustls; otherwise it serves plain HTTP.
pub async fn serve(
    addr: SocketAddr,
    state: WebuiState,
    mut shutdown: watch::Receiver<bool>,
    tls: Option<WebuiTls>,
) -> std::io::Result<()> {
    let app = router(state).into_make_service();
    let handle = axum_server::Handle::<SocketAddr>::new();
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        while !*shutdown.borrow_and_update() {
            if shutdown.changed().await.is_err() {
                break;
            }
        }
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(5)));
    });
    match tls {
        Some(tls) => {
            let config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key).await?;
            axum_server::bind_rustls(addr, config)
                .handle(handle)
                .serve(app)
                .await
        }
        None => axum_server::bind(addr).handle(handle).serve(app).await,
    }
}

/// Load the TOML `[webui]` section, honouring `--no-rc` and `-c`.
fn load_webui_config(cli: &Cli) -> Option<WebuiTomlConfig> {
    if cli.ignore_rc_file() {
        return None;
    }
    let config_path = cli
        .config_file
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| find_config_file(cli.no_rc));
    let path = config_path?;
    if path.extension().and_then(|e| e.to_str()) != Some("toml") {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    RtorrentTomlConfig::parse(&content)
        .ok()
        .map(|cfg| cfg.webui)
}

/// Resolve the WebUI bind address: `--webui-addr` wins over `[webui] addr`.
/// Returns `None` when unset or unparseable so a bad address can never prevent
/// the client from starting.
pub fn resolve_webui_addr(cli: &Cli) -> Option<SocketAddr> {
    let raw = cli
        .webui_addr
        .clone()
        .or_else(|| load_webui_config(cli).and_then(|cfg| cfg.addr))?;
    match raw.parse::<SocketAddr>() {
        Ok(addr) => Some(addr),
        Err(err) => {
            tracing::warn!(address = %raw, error = %err, "Invalid WebUI address; WebUI server disabled");
            None
        }
    }
}

/// Resolve the WebUI bearer token: `--webui-token` wins over `[webui] token`.
/// An empty token disables authentication.
pub fn resolve_webui_token(cli: &Cli) -> Option<String> {
    cli.webui_token
        .clone()
        .or_else(|| load_webui_config(cli).and_then(|cfg| cfg.token))
        .filter(|token| !token.is_empty())
}

/// Resolve WebUI TLS from `[webui] tls_cert`/`tls_key`. Mirrors gRPC: TLS is
/// TOML-only (no CLI flag), so a default install never needs extra arguments.
pub fn resolve_webui_tls(cli: &Cli) -> Option<WebuiTls> {
    tls_from_config(load_webui_config(cli)?)
}

/// Pure helper: both paths → `Some`, none → `None`, partial → warn + `None`
/// (a half-configured TLS must never prevent the client from starting).
pub fn tls_from_config(cfg: WebuiTomlConfig) -> Option<WebuiTls> {
    match (cfg.tls_cert, cfg.tls_key) {
        (Some(cert), Some(key)) => Some(WebuiTls {
            cert: PathBuf::from(cert),
            key: PathBuf::from(key),
        }),
        (None, None) => None,
        _ => {
            tracing::warn!("WebUI TLS requires both tls_cert and tls_key; TLS disabled");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use lt_common::Object;
    use rt_rpc::command_flags::{FLAG_PUBLIC_RPC, FLAG_UNTRUSTED_SAFE};
    use serde_json::{json, Value};
    use std::time::Duration;
    use tower::ServiceExt;

    fn test_state(token: Option<&str>) -> WebuiState {
        let mut map = CommandMap::new();
        map.insert_func(
            "test.ping",
            |_target, _arg| Ok(Object::from("pong")),
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            "",
        )
        .unwrap();
        map.insert_func(
            "test.secret",
            |_target, _arg| Ok(Object::from(42i64)),
            FLAG_PUBLIC_RPC,
            "",
            "",
        )
        .unwrap();
        WebuiState::new(
            Arc::new(RwLock::new(DownloadList::new())),
            Arc::new(RwLock::new(map)),
            token.map(|t| t.to_string()),
        )
    }

    fn get(uri: &str, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token));
        }
        builder.body(Body::empty()).unwrap()
    }

    fn post_json(uri: &str, token: Option<&str>, body: &Value) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token));
        }
        builder
            .body(Body::from(serde_json::to_string(body).unwrap()))
            .unwrap()
    }

    async fn send(state: WebuiState, req: Request<Body>) -> (StatusCode, Value) {
        let response = router(state).oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn rejects_missing_and_wrong_token() {
        let (status, _) = send(test_state(Some("s3cret")), get("/api/version", None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = send(
            test_state(Some("s3cret")),
            get("/api/version", Some("wrong")),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_valid_token_and_query_token() {
        let (status, body) = send(
            test_state(Some("s3cret")),
            get("/api/version", Some("s3cret")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("version").is_some());

        let (status, _) = send(
            test_state(Some("s3cret")),
            get("/api/version?token=s3cret", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn no_token_allows_read_but_stays_untrusted() {
        let (status, _) = send(test_state(None), get("/api/version", None)).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send(
            test_state(None),
            post_json("/api/command", None, &json!({"command": "test.ping"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], json!(true));
        assert_eq!(body["result"], json!("pong"));

        let (_, body) = send(
            test_state(None),
            post_json("/api/command", None, &json!({"command": "test.secret"})),
        )
        .await;
        assert_eq!(body["success"], json!(false));
    }

    #[tokio::test]
    async fn valid_token_grants_trusted_command() {
        let (status, body) = send(
            test_state(Some("s3cret")),
            post_json(
                "/api/command",
                Some("s3cret"),
                &json!({"command": "test.secret"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], json!(true));
        assert_eq!(body["result"], json!(42));
    }

    #[tokio::test]
    async fn command_round_trips_args_and_target() {
        let mut map = CommandMap::new();
        map.insert_func(
            "test.echo",
            |_target, arg| Ok(arg.clone()),
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            "",
        )
        .unwrap();
        let state = WebuiState::new(
            Arc::new(RwLock::new(DownloadList::new())),
            Arc::new(RwLock::new(map)),
            None,
        );
        let (_, body) = send(
            state,
            post_json(
                "/api/command",
                None,
                &json!({"command": "test.echo", "args": [7, "hi"]}),
            ),
        )
        .await;
        assert_eq!(body["success"], json!(true));
        assert_eq!(body["result"], json!([7, "hi"]));
    }

    #[tokio::test]
    async fn lists_empty_torrents_and_stats() {
        let (status, body) = send(test_state(None), get("/api/torrents", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!([]));

        let (status, body) = send(test_state(None), get("/api/stats", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_torrents"], json!(0));
        assert_eq!(body["active_torrents"], json!(0));
    }

    #[tokio::test]
    async fn unknown_torrent_is_not_found() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let (status, _) = send(
            test_state(None),
            get(&format!("/api/torrents/{}", hash), None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn resolve_webui_addr_cli_overrides_and_validates() {
        let cli = Cli {
            webui_addr: Some("127.0.0.1:8080".to_string()),
            ..Default::default()
        };
        assert_eq!(
            resolve_webui_addr(&cli),
            Some("127.0.0.1:8080".parse().unwrap())
        );

        let bad = Cli {
            webui_addr: Some("not-an-address".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_webui_addr(&bad), None);
    }

    #[test]
    fn resolve_webui_token_cli_wins_and_empty_disables() {
        let cli = Cli {
            webui_token: Some("abc".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_webui_token(&cli).as_deref(), Some("abc"));

        let empty = Cli {
            webui_token: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(resolve_webui_token(&empty), None);
    }

    #[test]
    fn object_json_round_trip() {
        let obj = Object::create_list_with(vec![
            Object::from(5i64),
            Object::from("text"),
            Object::create_empty(),
        ]);
        let value = convert::object_to_json(&obj);
        assert_eq!(value, json!([5, "text", null]));
        let back = convert::json_to_object(&value);
        assert_eq!(convert::object_to_json(&back), value);
    }

    #[tokio::test]
    async fn events_route_requires_auth() {
        let (status, _) = send(test_state(Some("s3cret")), get("/api/events", None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn event_listener_broadcasts_metadata_events() {
        let state = test_state(None);

        let metadata = lt_engine::TorrentMetadata {
            info_hash: lt_common::HashString::from_hex("4141414141414141414141414141414141414141")
                .unwrap(),
            name: "ws-event".into(),
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
            v2: None,
        };
        let download = Arc::new(RwLock::new(rt_core::Download::from_metadata(metadata)));
        state
            .download_list
            .write()
            .unwrap()
            .insert(download.clone());
        // Subscribe after insert: the inserted event has no receiver yet, so
        // only the metadata event emitted below is captured.
        let mut rx = state.event_broadcaster.subscribe();
        state
            .download_list
            .write()
            .unwrap()
            .notify_metadata_updated(&download);

        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event did not arrive in time")
            .expect("broadcast receive");
        assert_eq!(event.event_name, "event.download.metadata");
        assert_eq!(event.hash, "4141414141414141414141414141414141414141");
        assert_eq!(event.summary.name, "ws-event");

        let value = serde_json::to_value(ws::WsMessage::Torrent(Box::new(event))).unwrap();
        assert_eq!(value["type"], json!("torrent"));
        assert_eq!(value["event_name"], json!("event.download.metadata"));
        assert_eq!(value["summary"]["name"], json!("ws-event"));
    }

    #[test]
    fn ws_stats_message_serializes_with_type_tag() {
        let stats = convert::stats_from(&DownloadList::new(), &CommandMap::new());
        let value = serde_json::to_value(ws::WsMessage::Stats {
            stats,
            timestamp: 7,
        })
        .unwrap();
        assert_eq!(value["type"], json!("stats"));
        assert_eq!(value["timestamp"], json!(7));
        assert!(value["stats"].is_object());
        assert_eq!(value["stats"]["total_torrents"], json!(0));
    }

    #[test]
    fn tls_from_config_requires_both_paths() {
        let both = WebuiTomlConfig {
            tls_cert: Some("cert.pem".into()),
            tls_key: Some("key.pem".into()),
            ..Default::default()
        };
        let tls = tls_from_config(both).unwrap();
        assert_eq!(tls.cert, PathBuf::from("cert.pem"));
        assert_eq!(tls.key, PathBuf::from("key.pem"));
        assert!(tls_from_config(WebuiTomlConfig::default()).is_none());
        assert!(tls_from_config(WebuiTomlConfig {
            tls_cert: Some("cert.pem".into()),
            ..Default::default()
        })
        .is_none());
    }

    #[tokio::test]
    async fn frontend_served_at_root() {
        let response = router(test_state(None))
            .oneshot(get("/", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let html = std::str::from_utf8(&bytes).unwrap();
        assert!(html.contains("<title>rstorrent</title>"));
    }
}
