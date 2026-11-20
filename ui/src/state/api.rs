use futures_util::future::{select, Either};
use futures_util::StreamExt;
use gloo_net::http::Request;
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::Message;
use gloo_storage::{LocalStorage, Storage};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsCast;

use super::store::*;

const MIB: i64 = 1024 * 1024;
const GIB: i64 = 1024 * MIB;
const HISTORY_CAP: usize = 900;

// Deterministic LCG so the demo simulation never needs a rand crate.
struct Lcg(u64);

impl Lcg {
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range_i64(&mut self, min: i64, max: i64) -> i64 {
        if max <= min {
            return min;
        }
        min + (self.next_u64() % (max - min) as u64) as i64
    }
}

/// Why a backend request failed. Auth rejection is called out separately so the
/// UI can point the user at the API token setting instead of claiming the server
/// is unreachable.
#[derive(Debug)]
pub enum ApiError {
    Unauthorized(u16),
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized(status) => {
                write!(f, "Server returned HTTP {status} (unauthorized)")
            }
            ApiError::Other(message) => write!(f, "{message}"),
        }
    }
}

impl ApiError {
    fn is_auth(&self) -> bool {
        matches!(self, ApiError::Unauthorized(_))
    }
}

pub fn spawn_backend_bridge(state: AppState) {
    spawn_local(async move {
        loop {
            let base = state.api_base_url.get();
            let token = state.api_token.get();
            match fetch_items(&base, &token).await {
                Ok(items) => {
                    state.items.set(items);
                    set_connection(state, ConnectionMode::Live, false);
                    refresh_stats_version(state, &base, &token).await;
                    live_phase(state, &base, &token).await;
                }
                Err(err) => {
                    if state.connection.get() != ConnectionMode::Demo {
                        seed_demo(state);
                    }
                    set_connection(state, ConnectionMode::Demo, err.is_auth());
                    demo_phase(state).await;
                }
            }
        }
    });
}

pub async fn send_command(
    state: AppState,
    hash: &str,
    verb: &str,
) -> Result<CommandResponse, String> {
    if !matches!(verb, "start" | "stop" | "remove" | "recheck") {
        let message = format!("Unknown command: {verb}");
        state.push_toast(ToastKind::Error, "Command failed", message.clone());
        return Err(message);
    }
    let short: String = hash.chars().take(8).collect();

    if state.connection.get() == ConnectionMode::Live {
        let base = state.api_base_url.get();
        let token = state.api_token.get();
        let url = format!("{base}/api/torrents/{hash}/{verb}");
        let mut request = Request::post(&url);
        if !token.is_empty() {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let outcome = async {
            let response = request
                .send()
                .await
                .map_err(|err| format!("Request failed: {err}"))?;
            if !response.ok() {
                return Err(format!("Server returned HTTP {}", response.status()));
            }
            response
                .json::<CommandResponse>()
                .await
                .map_err(|err| format!("Invalid command response: {err}"))
        }
        .await;
        return match outcome {
            Ok(response) if response.success => {
                apply_command_to_items(state, hash, verb);
                state.push_event(
                    ToastKind::Success,
                    format!("Command {verb} succeeded"),
                    Some(hash.to_string()),
                );
                state.push_toast(
                    ToastKind::Success,
                    "Command sent",
                    format!("{verb} applied to {short}"),
                );
                Ok(response)
            }
            Ok(response) => {
                let message = if response.error.is_empty() {
                    "Backend rejected the command".to_string()
                } else {
                    response.error.clone()
                };
                state.push_toast(ToastKind::Error, "Command failed", message.clone());
                Err(message)
            }
            Err(message) => {
                state.push_toast(ToastKind::Error, "Command failed", message.clone());
                Err(message)
            }
        };
    }

    // Demo (and pre-connection Offline): mutate the local dataset.
    let name = state
        .items
        .get()
        .iter()
        .find(|item| item.hash == hash)
        .map(|item| item.name.clone());
    let Some(name) = name else {
        let message = "Torrent not found".to_string();
        state.push_toast(ToastKind::Error, "Command failed", message.clone());
        return Err(message);
    };
    apply_command_to_items(state, hash, verb);
    let (kind, title, detail) = match verb {
        "start" => (
            ToastKind::Success,
            "Torrent started",
            format!("{name} is downloading"),
        ),
        "stop" => (
            ToastKind::Info,
            "Torrent paused",
            format!("{name} was paused"),
        ),
        "remove" => (
            ToastKind::Warning,
            "Torrent removed",
            format!("{name} was removed"),
        ),
        _ => (
            ToastKind::Info,
            "Recheck started",
            format!("Hash check queued for {name}"),
        ),
    };
    state.push_toast(kind, title, detail);
    state.push_event(kind, format!("{verb}: {name}"), Some(hash.to_string()));
    Ok(CommandResponse {
        success: true,
        result: serde_json::Value::Null,
        error: String::new(),
    })
}

pub async fn test_connection(base_url: &str, token: &str) -> Result<VersionInfo, String> {
    let base = base_url.trim_end_matches('/');
    fetch_json::<VersionInfo>(base, token, "/api/version")
        .await
        .map_err(|err| match err {
            ApiError::Unauthorized(status) => format!(
                "Connection test failed: the backend rejected the API token (HTTP {status}). \
                 Check the token in the Connection section."
            ),
            ApiError::Other(message) => format!("Connection test failed: {message}"),
        })
}

pub async fn update_torrent_label(
    state: AppState,
    hash: &str,
    label: &str,
) -> Result<CommandResponse, String> {
    let clean_label = label.trim().to_string();
    if state.connection.get() == ConnectionMode::Live {
        let base = state.api_base_url.get();
        let token = state.api_token.get();
        let url = format!("{base}/api/torrents/{hash}/label");
        let mut request = Request::post(&url);
        if !token.is_empty() {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let payload = serde_json::json!({ "label": clean_label });
        let outcome = async {
            let response = request
                .json(&payload)
                .map_err(|e| format!("Serialization error: {e}"))?
                .send()
                .await
                .map_err(|err| format!("Request failed: {err}"))?;
            if !response.ok() {
                return Err(format!("Server returned HTTP {}", response.status()));
            }
            response
                .json::<CommandResponse>()
                .await
                .map_err(|err| format!("Invalid JSON: {err}"))
        }
        .await;

        match outcome {
            Ok(cmd) => {
                if cmd.success {
                    let h = hash.to_string();
                    let l = clean_label.clone();
                    state.items.update(|items| {
                        if let Some(item) = items.iter_mut().find(|i| i.hash == h) {
                            item.category = l;
                        }
                    });
                    state.selected_item.update(|selected| {
                        if let Some(ref mut item) = selected {
                            if item.hash == hash {
                                item.category = clean_label.clone();
                            }
                        }
                    });
                    state.push_toast(
                        ToastKind::Success,
                        "Label updated",
                        format!("Label set to '{}'", clean_label),
                    );
                    Ok(cmd)
                } else {
                    state.push_toast(ToastKind::Error, "Update failed", cmd.error.clone());
                    Err(cmd.error)
                }
            }
            Err(err) => {
                state.push_toast(ToastKind::Error, "Update failed", err.clone());
                Err(err)
            }
        }
    } else {
        let h = hash.to_string();
        let l = clean_label.clone();
        state.items.update(|items| {
            if let Some(item) = items.iter_mut().find(|i| i.hash == h) {
                item.category = l;
            }
        });
        state.selected_item.update(|selected| {
            if let Some(ref mut item) = selected {
                if item.hash == hash {
                    item.category = clean_label.clone();
                }
            }
        });
        state.push_toast(
            ToastKind::Success,
            "Label updated",
            format!("Label set to '{}'", clean_label),
        );
        Ok(CommandResponse {
            success: true,
            result: serde_json::Value::Null,
            error: String::new(),
        })
    }
}

pub fn download_json(filename: &str, json: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(body) = document.body() else {
        return;
    };
    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(json));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/json");
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    let Ok(element) = document.create_element("a") else {
        return;
    };
    let Ok(anchor) = element.dyn_into::<web_sys::HtmlAnchorElement>() else {
        return;
    };
    anchor.set_href(&url);
    anchor.set_download(filename);
    let _ = body.append_child(&anchor);
    anchor.click();
    let _ = body.remove_child(&anchor);
    let _ = web_sys::Url::revoke_object_url(&url);
}

pub fn export_state_json(state: AppState) -> String {
    let payload = serde_json::json!({
        "items": state.items.get(),
        "stats": state.stats.get(),
        "exported_at": js_sys::Date::now() as u64,
    });
    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
}

pub fn reset_demo_data(state: AppState) {
    seed_demo(state);
    state.push_toast(
        ToastKind::Success,
        "Demo data reset",
        "Simulation dataset reseeded",
    );
    state.push_event(ToastKind::Info, "Demo dataset reset", None);
}

pub fn clear_local_storage(state: AppState) {
    for key in [
        "rt-ui.theme",
        "rt-ui.accent",
        "rt-ui.density",
        "rt-ui.animations",
        "rt-ui.sidebar_collapsed",
        "rt-ui.api_base_url",
        "rt-ui.api_token",
        "rt-ui.poll_interval_ms",
    ] {
        LocalStorage::delete(key);
    }
    state.push_toast(
        ToastKind::Success,
        "Storage cleared",
        "All rt-ui settings removed from local storage",
    );
}

pub fn create_demo_item(state: AppState) {
    let mut rng = Lcg(js_sys::Date::now() as u64);
    const CANDIDATES: [(&str, &str, i64); 6] = [
        ("ubuntu-26.04-beta-desktop-amd64.iso", "linux", 6 * GIB),
        ("debian-13-trixie-amd64-netinst.iso", "linux", 700 * MIB),
        (
            "archlinux-bootstrap-2026.11-x86_64.tar.zst",
            "linux",
            450 * MIB,
        ),
        ("wiktionary-en-all-2026-11.zim", "datasets", 8 * GIB),
        ("librivox-short-stories-vol03.zip", "books", GIB + 200 * MIB),
        ("tears-of-steel-4k-remaster.mkv", "media", 12 * GIB),
    ];
    let index = (rng.next_u64() % CANDIDATES.len() as u64) as usize;
    let (name, category, size) = CANDIDATES[index];
    // TorrentItem timestamps are unix seconds, matching the backend contract.
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let item = TorrentItem {
        hash: demo_hash(&mut rng),
        name: name.to_string(),
        size_bytes: size,
        completed_bytes: 0,
        up_rate: 0,
        down_rate: rng.range_i64(2 * MIB, 14 * MIB),
        up_total: 0,
        down_total: 0,
        state: "Downloading".to_string(),
        is_active: true,
        is_complete: false,
        is_hash_checking: false,
        priority: 1,
        directory: format!("/downloads/{category}"),
        category: category.to_string(),
        message: String::new(),
        state_changed: now,
        started: now,
        finished: 0,
        seeds_connected: 12,
        seeds_total: 45,
        peers_connected: 5,
        peers_total: 28,
    };
    let name = item.name.clone();
    let hash = item.hash.clone();
    state.items.update(|items| items.push(item));
    state.push_toast(
        ToastKind::Success,
        "Torrent added",
        format!("{name} started downloading"),
    );
    state.push_event(ToastKind::Success, format!("Added {name}"), Some(hash));
}

pub fn run_diagnostics(state: AppState) {
    let items = state.items.get();
    let stats = state.stats.get();
    let mode = state.connection.get();
    let failed = items
        .iter()
        .filter(|item| item.status() == ItemStatus::Failed)
        .count();
    state.push_event(
        ToastKind::Info,
        format!("Diagnostics: connection mode {mode:?}"),
        None,
    );
    state.push_event(
        ToastKind::Info,
        format!(
            "Diagnostics: {} torrents tracked, {} active",
            items.len(),
            stats.active_torrents
        ),
        None,
    );
    if failed > 0 {
        state.push_event(
            ToastKind::Warning,
            format!("Diagnostics: {failed} torrent(s) in error state"),
            None,
        );
    } else {
        state.push_event(
            ToastKind::Success,
            "Diagnostics: no torrents in error state",
            None,
        );
    }
    state.push_event(
        ToastKind::Success,
        format!(
            "Diagnostics: history buffer {}/{HISTORY_CAP} samples",
            state.down_history.get().len()
        ),
        None,
    );
    state.push_toast(
        ToastKind::Success,
        "Diagnostics complete",
        format!(
            "{} torrents, {} active, {} errors",
            items.len(),
            stats.active_torrents,
            failed
        ),
    );
}

async fn fetch_json<T: DeserializeOwned>(
    base: &str,
    token: &str,
    path: &str,
) -> Result<T, ApiError> {
    let url = format!("{base}{path}");
    let mut request = Request::get(&url);
    if !token.is_empty() {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
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
    response
        .json::<T>()
        .await
        .map_err(|err| ApiError::Other(format!("Invalid payload: {err}")))
}

async fn fetch_items(base: &str, token: &str) -> Result<Vec<TorrentItem>, ApiError> {
    fetch_json::<Vec<TorrentItem>>(base, token, "/api/torrents").await
}

async fn refresh_stats_version(state: AppState, base: &str, token: &str) {
    if let Ok(stats) = fetch_json::<SessionStats>(base, token, "/api/stats").await {
        push_sample(state, stats.down_rate as f64, stats.up_rate as f64);
        state.stats.set(stats);
    }
    if let Ok(version) = fetch_json::<VersionInfo>(base, token, "/api/version").await {
        state.version.set(version);
    }
}

fn settings_changed(state: AppState, base: &str, token: &str) -> bool {
    state.api_base_url.get() != base || state.api_token.get() != token
}

fn set_connection(state: AppState, mode: ConnectionMode, auth_failed: bool) {
    if state.connection.get() == mode {
        return;
    }
    state.connection.set(mode);
    match mode {
        ConnectionMode::Live => {
            state.push_event(ToastKind::Success, "Connected to backend", None);
            state.push_toast(
                ToastKind::Success,
                "Connected",
                "Live backend connection established",
            );
        }
        ConnectionMode::Demo => {
            if auth_failed {
                state.push_event(
                    ToastKind::Warning,
                    "Backend rejected credentials, demo simulation active",
                    None,
                );
                state.push_toast(
                    ToastKind::Warning,
                    "Authentication required",
                    "The backend rejected the API token. Set it in Settings → Connection to \
                     go live. Showing simulated data meanwhile.",
                );
            } else {
                state.push_event(
                    ToastKind::Warning,
                    "Backend unreachable, demo simulation active",
                    None,
                );
                state.push_toast(
                    ToastKind::Warning,
                    "Demo mode",
                    "Backend unreachable — showing simulated data",
                );
            }
        }
        ConnectionMode::Offline => {}
    }
}

async fn live_phase(state: AppState, base: &str, token: &str) {
    let ws_url = websocket_url(base, token);
    if let Ok(mut ws) = WebSocket::open(&ws_url) {
        loop {
            if settings_changed(state, base, token) {
                let _ = ws.close(None, None);
                return;
            }
            // The timeout keeps the loop responsive to pause/settings changes even
            // when the server goes quiet; dropping the pending next() is safe.
            match select(ws.next(), TimeoutFuture::new(5_000)).await {
                Either::Left((Some(Ok(message)), _)) => {
                    if !state.live_stream_active.get() {
                        continue;
                    }
                    match message {
                        Message::Text(text) => handle_envelope(state, &text),
                        Message::Bytes(bytes) => {
                            if let Ok(text) = String::from_utf8(bytes) {
                                handle_envelope(state, &text);
                            }
                        }
                    }
                }
                Either::Left((Some(Err(_)), _)) | Either::Left((None, _)) => break,
                Either::Right((_, _)) => continue,
            }
        }
        let _ = ws.close(None, None);
        state.push_event(
            ToastKind::Warning,
            "Event stream closed, falling back to polling",
            None,
        );
    }
    // Fallback polling loop when WebSocket disconnects: poll a few times, then
    // return so the outer supervisor loop re-establishes the WebSocket connection.
    for _ in 0..5 {
        if settings_changed(state, base, token) {
            return;
        }
        let interval = state.poll_interval_ms.get().clamp(500, 60_000) as u32;
        TimeoutFuture::new(interval).await;
        if settings_changed(state, base, token) || !state.live_stream_active.get() {
            continue;
        }
        match fetch_items(base, token).await {
            Ok(items) => {
                state.items.set(items);
                refresh_stats_version(state, base, token).await;
            }
            Err(_) => return,
        }
    }
}

async fn demo_phase(state: AppState) {
    let base = state.api_base_url.get();
    let token = state.api_token.get();
    let mut rng = Lcg(0x2545_F491_4F6C_DD1D);
    // 30 one-second ticks, then the supervisor re-probes the backend.
    for _ in 0..30 {
        if settings_changed(state, &base, &token) {
            return;
        }
        if state.live_stream_active.get() {
            demo_tick(state, &mut rng);
        }
        TimeoutFuture::new(1_000).await;
    }
}

fn websocket_url(base: &str, token: &str) -> String {
    let origin = if base.is_empty() {
        web_sys::window()
            .and_then(|window| window.location().origin().ok())
            .unwrap_or_default()
    } else {
        base.trim_end_matches('/').to_string()
    };
    // A bare host:port base needs a scheme before the ws/wss conversion.
    let origin = if !origin.is_empty()
        && !origin.starts_with("http://")
        && !origin.starts_with("https://")
    {
        format!("http://{origin}")
    } else {
        origin
    };
    let ws_origin = if let Some(rest) = origin.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = origin.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        origin
    };
    if token.is_empty() {
        format!("{ws_origin}/api/events")
    } else {
        format!(
            "{ws_origin}/api/events?token={}",
            js_sys::encode_uri_component(token)
        )
    }
}

fn handle_envelope(state: AppState, text: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    match value.get("type").and_then(|kind| kind.as_str()) {
        Some("torrent") => {
            if let Some(summary) = value
                .get("summary")
                .and_then(|raw| serde_json::from_value::<TorrentItem>(raw.clone()).ok())
            {
                upsert_item(state, summary);
                return;
            }
            let event_name = value
                .get("event_name")
                .and_then(|raw| raw.as_str())
                .unwrap_or("");
            let hash = value.get("hash").and_then(|raw| raw.as_str()).unwrap_or("");
            if (event_name.contains("erase") || event_name.contains("remove")) && !hash.is_empty() {
                remove_item(state, hash);
            }
        }
        Some("stats") => {
            if let Some(stats) = value
                .get("stats")
                .and_then(|raw| serde_json::from_value::<SessionStats>(raw.clone()).ok())
            {
                push_sample(state, stats.down_rate as f64, stats.up_rate as f64);
                state.stats.set(stats);
            }
        }
        _ => {}
    }
}

fn upsert_item(state: AppState, summary: TorrentItem) {
    let hash = summary.hash.clone();
    let refreshed = summary.clone();
    state.items.update(
        |items| match items.iter_mut().find(|item| item.hash == hash) {
            Some(existing) => *existing = summary,
            None => items.push(summary),
        },
    );
    state.selected_item.update(|selected| {
        if matches!(selected, Some(item) if item.hash == hash) {
            *selected = Some(refreshed);
        }
    });
}

fn remove_item(state: AppState, hash: &str) {
    state
        .items
        .update(|items| items.retain(|item| item.hash != hash));
    state
        .selected_hashes
        .update(|hashes| hashes.retain(|selected| selected != hash));
    state.selected_item.update(|selected| {
        if matches!(selected, Some(item) if item.hash == hash) {
            *selected = None;
        }
    });
}

fn apply_command_to_items(state: AppState, hash: &str, verb: &str) {
    // TorrentItem timestamps are unix seconds, matching the backend contract.
    let now = (js_sys::Date::now() / 1000.0) as u64;
    if verb == "remove" {
        remove_item(state, hash);
        return;
    }
    state.items.update(|items| {
        if let Some(item) = items.iter_mut().find(|item| item.hash == hash) {
            match verb {
                "start" => {
                    item.is_active = true;
                    item.message.clear();
                    item.state_changed = now;
                    if item.is_complete {
                        item.state = "Seeding".to_string();
                        item.up_rate = 512 * 1024;
                    } else {
                        item.state = "Downloading".to_string();
                        item.down_rate = 2 * MIB;
                    }
                }
                "stop" => {
                    item.is_active = false;
                    item.state = "Paused".to_string();
                    item.down_rate = 0;
                    item.up_rate = 0;
                    item.state_changed = now;
                }
                "recheck" => {
                    item.state = "Checking".to_string();
                    item.is_hash_checking = true;
                    item.state_changed = now;
                }
                _ => {}
            }
        }
    });
    let fresh = state.items.get().into_iter().find(|item| item.hash == hash);
    state.selected_item.update(|selected| {
        if matches!(selected, Some(item) if item.hash == hash) {
            *selected = fresh;
        }
    });
}

fn push_sample(state: AppState, down: f64, up: f64) {
    state.down_history.update(|history| {
        history.push(down);
        if history.len() > HISTORY_CAP {
            let excess = history.len() - HISTORY_CAP;
            history.drain(0..excess);
        }
    });
    state.up_history.update(|history| {
        history.push(up);
        if history.len() > HISTORY_CAP {
            let excess = history.len() - HISTORY_CAP;
            history.drain(0..excess);
        }
    });
}

fn demo_hash(rng: &mut Lcg) -> String {
    let mut hash = String::with_capacity(40);
    for _ in 0..20 {
        hash.push_str(&format!("{:02x}", (rng.next_u64() & 0xff) as u8));
    }
    hash
}

fn build_demo_items(rng: &mut Lcg) -> Vec<TorrentItem> {
    // TorrentItem timestamps are unix seconds, matching the backend contract.
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let seeds: [(&str, i64, &str, &str, f64, i64, i64); 14] = [
        (
            "ubuntu-24.04.2-desktop-amd64.iso",
            5 * GIB + 800 * MIB,
            "linux",
            "Downloading",
            0.62,
            12 * MIB,
            300 * 1024,
        ),
        (
            "debian-12.9.0-amd64-netinst.iso",
            650 * MIB,
            "linux",
            "Seeding",
            1.0,
            0,
            2 * MIB,
        ),
        (
            "archlinux-2026.11.01-x86_64.iso",
            GIB + 100 * MIB,
            "linux",
            "Downloading",
            0.35,
            8 * MIB,
            128 * 1024,
        ),
        (
            "linux-kernel-6.12.tar.xz",
            320 * MIB,
            "linux",
            "Paused",
            0.78,
            0,
            0,
        ),
        (
            "wikipedia-en-all-2026-11.zim",
            90 * GIB,
            "datasets",
            "Downloading",
            0.18,
            24 * MIB,
            512 * 1024,
        ),
        (
            "openstreetmap-planet-2026-11.osm.pbf",
            78 * GIB,
            "datasets",
            "Checking",
            0.91,
            0,
            0,
        ),
        (
            "common-crawl-2026-45-subset.warc.gz",
            24 * GIB,
            "datasets",
            "Downloading",
            0.44,
            16 * MIB,
            256 * 1024,
        ),
        (
            "blender-open-movie-4k.mkv",
            8 * GIB + 500 * MIB,
            "media",
            "Seeding",
            1.0,
            0,
            5 * MIB,
        ),
        (
            "librivox-classics-vol12.zip",
            2 * GIB + 400 * MIB,
            "books",
            "Seeding",
            1.0,
            0,
            MIB,
        ),
        (
            "gutenberg-ebooks-batch-042.tar",
            GIB + 600 * MIB,
            "books",
            "Paused",
            0.52,
            0,
            0,
        ),
        (
            "fedora-workstation-41-x86_64.iso",
            2 * GIB + 200 * MIB,
            "linux",
            "Downloading",
            0.83,
            10 * MIB,
            200 * 1024,
        ),
        (
            "nixos-24.11-minimal.iso",
            980 * MIB,
            "linux",
            "Seeding",
            1.0,
            0,
            900 * 1024,
        ),
        (
            "internet-archive-books-2026.tar",
            12 * GIB,
            "books",
            "Error(\"tracker timeout\")",
            0.27,
            0,
            0,
        ),
        (
            "open-images-v7-validation.tar.gz",
            4 * GIB + 700 * MIB,
            "datasets",
            "Downloading",
            0.09,
            6 * MIB,
            64 * 1024,
        ),
    ];
    seeds
        .iter()
        .map(
            |&(name, size, category, state, progress, down_rate, up_rate)| {
                let completed = (size as f64 * progress) as i64;
                let is_complete = progress >= 1.0;
                TorrentItem {
                    hash: demo_hash(rng),
                    name: name.to_string(),
                    size_bytes: size,
                    completed_bytes: completed,
                    up_rate,
                    down_rate,
                    up_total: rng.range_i64(0, 40 * GIB),
                    down_total: completed + rng.range_i64(0, 2 * GIB),
                    state: state.to_string(),
                    is_active: matches!(state, "Downloading" | "Seeding" | "Checking"),
                    is_complete,
                    is_hash_checking: state == "Checking",
                    priority: rng.range_i64(0, 3) as u32,
                    directory: format!("/downloads/{category}"),
                    category: category.to_string(),
                    message: if state.starts_with("Error") {
                        "tracker timeout".to_string()
                    } else {
                        String::new()
                    },
                    state_changed: if state == "Checking" {
                        now
                    } else {
                        now.saturating_sub(rng.range_i64(30, 21_600) as u64)
                    },
                    started: now.saturating_sub(rng.range_i64(3_600, 259_200) as u64),
                    finished: if is_complete {
                        now.saturating_sub(rng.range_i64(60, 86_400) as u64)
                    } else {
                        0
                    },
                    seeds_connected: if is_complete {
                        rng.range_i64(5, 30) as u32
                    } else {
                        rng.range_i64(1, 15) as u32
                    },
                    seeds_total: rng.range_i64(20, 80) as u32,
                    peers_connected: if is_complete {
                        rng.range_i64(0, 5) as u32
                    } else {
                        rng.range_i64(2, 25) as u32
                    },
                    peers_total: rng.range_i64(10, 50) as u32,
                }
            },
        )
        .collect()
}

fn derive_stats(items: &[TorrentItem], previous: &SessionStats, rng: &mut Lcg) -> SessionStats {
    let up_rate: i64 = items.iter().map(|item| item.up_rate).sum();
    let down_rate: i64 = items.iter().map(|item| item.down_rate).sum();
    let up_total: i64 = items.iter().map(|item| item.up_total).sum();
    let down_total: i64 = items.iter().map(|item| item.down_total).sum();
    let active_torrents = items.iter().filter(|item| item.is_active).count() as u32;
    let dht_nodes = if previous.dht_nodes == 0 {
        320
    } else {
        (previous.dht_nodes as i64 + rng.range_i64(-8, 9)).clamp(180, 520) as u32
    };
    SessionStats {
        up_rate,
        down_rate,
        up_total,
        down_total,
        active_torrents,
        total_torrents: items.len() as u32,
        listen_port: 51413,
        throttle_up_limit: 0,
        throttle_down_limit: 0,
        dht_active: true,
        dht_nodes,
        dht_torrents: active_torrents,
        dht_port: 6881,
    }
}

fn seed_demo(state: AppState) {
    let mut rng = Lcg(0x5EED_C0DE_1234_ABCD);
    let items = build_demo_items(&mut rng);
    let stats = derive_stats(&items, &SessionStats::default(), &mut rng);
    state.items.set(items);
    state.selected_item.set(None);
    state.selected_hashes.set(Vec::new());
    // Prefill ~90 samples so charts render immediately on first paint.
    let mut wave = Lcg(0x000A_11CE_5EED_0001);
    let mut down = Vec::with_capacity(90);
    let mut up = Vec::with_capacity(90);
    for _ in 0..90 {
        down.push((stats.down_rate as f64 * (0.7 + 0.6 * wave.next_f64())).max(0.0));
        up.push((stats.up_rate as f64 * (0.7 + 0.6 * wave.next_f64())).max(0.0));
    }
    state.down_history.set(down);
    state.up_history.set(up);
    state.stats.set(stats);
    state.events.set(Vec::new());
    state.push_event(ToastKind::Info, "Demo simulation initialized", None);
    state.push_event(ToastKind::Success, "Loaded 14 demo torrents", None);
}

fn demo_tick(state: AppState, rng: &mut Lcg) {
    // TorrentItem timestamps are unix seconds, matching the backend contract.
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let mut items = state.items.get();
    let mut pending: Vec<(ToastKind, String, Option<String>)> = Vec::new();

    for item in items.iter_mut() {
        if item.state == "Downloading" {
            let drift = rng.range_i64(-768 * 1024, 768 * 1024);
            item.down_rate = (item.down_rate + drift).clamp(256 * 1024, 40 * MIB);
            let gained = item
                .down_rate
                .min(item.size_bytes - item.completed_bytes)
                .max(0);
            item.completed_bytes += gained;
            item.down_total += gained;
            if item.completed_bytes >= item.size_bytes {
                item.completed_bytes = item.size_bytes;
                item.is_complete = true;
                item.state = "Seeding".to_string();
                item.down_rate = 0;
                item.up_rate = rng.range_i64(256 * 1024, 4 * MIB);
                item.finished = now;
                item.state_changed = now;
                pending.push((
                    ToastKind::Success,
                    format!("{} finished downloading", item.name),
                    Some(item.hash.clone()),
                ));
            } else if rng.next_f64() < 0.004 {
                item.state = "Error(\"tracker timeout\")".to_string();
                item.message = "tracker timeout".to_string();
                item.is_active = false;
                item.down_rate = 0;
                item.state_changed = now;
                pending.push((
                    ToastKind::Error,
                    format!("{} hit a tracker timeout", item.name),
                    Some(item.hash.clone()),
                ));
            }
        } else if item.state == "Seeding" {
            let drift = rng.range_i64(-192 * 1024, 192 * 1024);
            item.up_rate = (item.up_rate + drift).clamp(64 * 1024, 8 * MIB);
            item.up_total += item.up_rate;
        } else if item.state == "Checking" {
            // Timestamps are unix seconds, so the hash check runs for ~8s.
            if now.saturating_sub(item.state_changed) > 8 {
                item.is_hash_checking = false;
                item.state_changed = now;
                if item.is_complete {
                    item.state = "Seeding".to_string();
                    item.up_rate = rng.range_i64(256 * 1024, 4 * MIB);
                } else {
                    item.state = "Downloading".to_string();
                    item.down_rate = rng.range_i64(MIB, 12 * MIB);
                }
                pending.push((
                    ToastKind::Info,
                    format!("Hash check complete for {}", item.name),
                    Some(item.hash.clone()),
                ));
            }
        } else if item.state.starts_with("Error") && rng.next_f64() < 0.01 {
            item.state = "Downloading".to_string();
            item.is_active = true;
            item.message.clear();
            item.down_rate = rng.range_i64(512 * 1024, 6 * MIB);
            item.state_changed = now;
            pending.push((
                ToastKind::Success,
                format!("{} recovered from error", item.name),
                Some(item.hash.clone()),
            ));
        }
    }

    if rng.next_f64() < 0.03 && !items.is_empty() {
        let index = (rng.next_u64() % items.len() as u64) as usize;
        let item = &mut items[index];
        if item.state == "Downloading" {
            item.state = "Paused".to_string();
            item.is_active = false;
            item.down_rate = 0;
            item.state_changed = now;
            pending.push((
                ToastKind::Info,
                format!("{} paused", item.name),
                Some(item.hash.clone()),
            ));
        } else if item.state == "Paused" {
            item.state = "Downloading".to_string();
            item.is_active = true;
            item.down_rate = rng.range_i64(512 * 1024, 12 * MIB);
            item.state_changed = now;
            pending.push((
                ToastKind::Info,
                format!("{} resumed", item.name),
                Some(item.hash.clone()),
            ));
        }
    }

    if rng.next_f64() < 0.06 && !items.is_empty() {
        let index = (rng.next_u64() % items.len() as u64) as usize;
        let item = &items[index];
        pending.push((
            ToastKind::Info,
            format!("Tracker announce OK for {}", item.name),
            Some(item.hash.clone()),
        ));
    }

    let stats = derive_stats(&items, &state.stats.get(), rng);
    state.items.set(items);
    push_sample(state, stats.down_rate as f64, stats.up_rate as f64);
    state.stats.set(stats);
    for (severity, message, hash) in pending {
        state.push_event(severity, message, hash);
    }
    let selected_hash = state.selected_item.get().map(|item| item.hash);
    if let Some(hash) = selected_hash {
        let fresh = state.items.get().into_iter().find(|item| item.hash == hash);
        // Only notify when the snapshot actually changed, so the drawer does
        // not re-render on every demo tick.
        if fresh != state.selected_item.get() {
            state.selected_item.set(fresh);
        }
    }
}

pub async fn fetch_daemon_config(
    base_url: &str,
    token: &str,
) -> Result<DaemonConfigResponse, String> {
    let base = base_url.trim_end_matches('/');
    fetch_json::<DaemonConfigResponse>(base, token, "/api/config")
        .await
        .map_err(|err| format!("{err}"))
}

pub async fn update_daemon_config(
    base_url: &str,
    token: &str,
    config: Option<DaemonConfig>,
    toml: Option<String>,
    save_file: bool,
) -> Result<CommandResponse, String> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/config");
    let mut request = Request::post(&url);
    if !token.is_empty() {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let body = serde_json::json!({
        "config": config,
        "toml": toml,
        "save_file": save_file,
    });
    let response = request
        .json(&body)
        .map_err(|err| format!("Serialization error: {err}"))?
        .send()
        .await
        .map_err(|err| format!("Request failed: {err}"))?;
    if !response.ok() {
        return Err(format!("Server returned HTTP {}", response.status()));
    }
    response
        .json::<CommandResponse>()
        .await
        .map_err(|err| format!("Invalid payload: {err}"))
}

#[allow(dead_code)]
pub async fn save_daemon_config_file(
    base_url: &str,
    token: &str,
) -> Result<CommandResponse, String> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/config/save");
    let mut request = Request::post(&url);
    if !token.is_empty() {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let response = request
        .send()
        .await
        .map_err(|err| format!("Request failed: {err}"))?;
    if !response.ok() {
        return Err(format!("Server returned HTTP {}", response.status()));
    }
    response
        .json::<CommandResponse>()
        .await
        .map_err(|err| format!("Invalid payload: {err}"))
}

pub async fn add_torrent(
    state: AppState,
    payload: AddTorrentPayload,
) -> Result<AddTorrentResult, String> {
    if state.connection.get() == ConnectionMode::Live {
        let base = state.api_base_url.get();
        let token = state.api_token.get();
        let url = format!("{}/api/torrents/add", base.trim_end_matches('/'));
        let mut request = Request::post(&url);
        if !token.is_empty() {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let body = serde_json::json!({
            "source": payload.source,
            "torrent_file_base64": payload.torrent_file_base64,
            "download_dir": payload.download_dir,
            "start_immediately": payload.start_immediately,
            "category": payload.category,
        });
        let outcome = async {
            let response = request
                .json(&body)
                .map_err(|err| format!("Serialization failed: {err}"))?
                .send()
                .await
                .map_err(|err| format!("Request failed: {err}"))?;
            if !response.ok() {
                return Err(format!("Server returned HTTP {}", response.status()));
            }
            response
                .json::<AddTorrentResult>()
                .await
                .map_err(|err| format!("Invalid response: {err}"))
        }
        .await;

        match outcome {
            Ok(res) if res.success => {
                let name = if res.name.is_empty() {
                    "New Torrent".to_string()
                } else {
                    res.name.clone()
                };
                state.push_toast(
                    ToastKind::Success,
                    "Torrent added",
                    format!("{name} queued successfully"),
                );
                state.push_event(
                    ToastKind::Success,
                    format!("Added: {name}"),
                    Some(res.hash.clone()),
                );
                spawn_local(async move {
                    if let Ok(items) = fetch_items(&base, &token).await {
                        state.items.set(items);
                    }
                });
                Ok(res)
            }
            Ok(res) => {
                let msg = if res.error.is_empty() {
                    "Failed to add torrent".to_string()
                } else {
                    res.error.clone()
                };
                state.push_toast(ToastKind::Error, "Add torrent failed", msg.clone());
                Err(msg)
            }
            Err(e) => {
                state.push_toast(ToastKind::Error, "Add torrent failed", e.clone());
                Err(e)
            }
        }
    } else {
        // Demo mode
        let name = payload
            .source
            .as_deref()
            .and_then(|s| {
                if s.starts_with("magnet:") {
                    s.split("&dn=").nth(1).and_then(|dn| dn.split('&').next())
                } else {
                    s.rsplit('/').next()
                }
            })
            .filter(|n| !n.is_empty())
            .unwrap_or("Custom Added Torrent")
            .to_string();

        let hash: String = (0..40).map(|i| format!("{:x}", (i * 7 + 3) % 16)).collect();
        let now = (js_sys::Date::now() / 1000.0) as u64;
        let item = TorrentItem {
            hash: hash.clone(),
            name: name.clone(),
            size_bytes: 2 * 1024 * 1024 * 1024,
            completed_bytes: 0,
            up_rate: 0,
            down_rate: if payload.start_immediately {
                5 * 1024 * 1024
            } else {
                0
            },
            up_total: 0,
            down_total: 0,
            state: if payload.start_immediately {
                "Downloading".to_string()
            } else {
                "Paused".to_string()
            },
            is_active: payload.start_immediately,
            is_complete: false,
            is_hash_checking: false,
            priority: 2,
            directory: payload
                .download_dir
                .unwrap_or_else(|| "/downloads".to_string()),
            category: payload.category.unwrap_or_default(),
            message: String::new(),
            state_changed: now,
            started: if payload.start_immediately { now } else { 0 },
            finished: 0,
            seeds_connected: if payload.start_immediately { 8 } else { 0 },
            seeds_total: 35,
            peers_connected: if payload.start_immediately { 4 } else { 0 },
            peers_total: 22,
        };
        state.items.update(|list| list.insert(0, item));
        state.push_toast(
            ToastKind::Success,
            "Torrent added (Demo)",
            format!("{name} created"),
        );
        state.push_event(
            ToastKind::Success,
            format!("Added (Demo): {name}"),
            Some(hash.clone()),
        );
        Ok(AddTorrentResult {
            success: true,
            hash,
            name,
            error: String::new(),
        })
    }
}
