//! REST handlers for the WebUI API.
//!
//! Read endpoints snapshot the download list under a short-lived lock. Command
//! execution mirrors the gRPC `CommandService`: it runs on a blocking worker
//! (commands can fsync or touch the filesystem) and uses the server-decided
//! trust level carried in the [`Trusted`] request extension.

use crate::toml_config::RtorrentTomlConfig;
use crate::webui::convert::{self, SessionStats, TorrentSummary, VersionInfo};
use crate::webui::{Trusted, WebuiState};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use lt_common::Object;
use rt_rpc::Target;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// A generic command invocation, equivalent to the gRPC `CommandRequest`.
#[derive(Debug, Deserialize)]
pub struct CommandRequest {
    pub command: String,
    /// Hex info-hash of the target torrent; empty means no target.
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub args: Vec<Value>,
}

/// Outcome of a command invocation, equivalent to the gRPC `CommandResponse`.
#[derive(Debug, Serialize)]
pub struct CommandResponse {
    pub success: bool,
    pub result: Value,
    pub error: String,
}

impl CommandResponse {
    fn failure(error: String) -> Self {
        Self {
            success: false,
            result: Value::Null,
            error,
        }
    }
}

pub async fn list_torrents(State(state): State<WebuiState>) -> Json<Vec<TorrentSummary>> {
    let list = state
        .download_list
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let summaries = list
        .all()
        .iter()
        .map(|dl| {
            let guard = dl.read().unwrap_or_else(|e| e.into_inner());
            convert::summary_from(&guard)
        })
        .collect();
    Json(summaries)
}

pub async fn get_torrent(
    State(state): State<WebuiState>,
    Path(hash): Path<String>,
) -> Result<Json<TorrentSummary>, (StatusCode, String)> {
    let hs = lt_common::HashString::from_hex(&hash)
        .ok_or((StatusCode::BAD_REQUEST, "invalid info hash".to_string()))?;
    let list = state
        .download_list
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let dl = list
        .find(&hs)
        .ok_or((StatusCode::NOT_FOUND, "torrent not found".to_string()))?;
    let guard = dl.read().unwrap_or_else(|e| e.into_inner());
    Ok(Json(convert::summary_from(&guard)))
}

pub async fn get_stats(State(state): State<WebuiState>) -> Json<SessionStats> {
    let stats = {
        let list = state
            .download_list
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let map = state.command_map.read().unwrap_or_else(|e| e.into_inner());
        convert::stats_from(&list, &map)
    };
    Json(stats)
}

pub async fn get_version(State(state): State<WebuiState>) -> Json<VersionInfo> {
    Json(VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: lt_common::version::VERSION.to_string(),
        build_target: std::env::consts::ARCH.to_string(),
        uptime_seconds: state.start_time.elapsed().as_secs(),
    })
}

pub async fn execute_command(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(req): Json<CommandRequest>,
) -> Json<CommandResponse> {
    let target = if req.target.is_empty() {
        Target::None
    } else {
        match lt_common::HashString::from_hex(&req.target) {
            Some(hash) => Target::Download(hash),
            None => {
                return Json(CommandResponse::failure(format!(
                    "invalid target hash {}",
                    req.target
                )))
            }
        }
    };

    let arg = match req.args.len() {
        0 => Object::create_empty(),
        1 => convert::json_to_object(&req.args[0]),
        _ => Object::create_list_with(req.args.iter().map(convert::json_to_object).collect()),
    };

    Json(run_command(state, trusted.0, req.command, target, arg).await)
}

pub async fn start_torrent(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(hash): Path<String>,
) -> Json<CommandResponse> {
    torrent_action(state, trusted.0, hash, "d.start").await
}

pub async fn stop_torrent(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(hash): Path<String>,
) -> Json<CommandResponse> {
    torrent_action(state, trusted.0, hash, "d.stop").await
}

pub async fn remove_torrent(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(hash): Path<String>,
) -> Json<CommandResponse> {
    torrent_action(state, trusted.0, hash, "d.erase").await
}

pub async fn recheck_torrent(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(hash): Path<String>,
) -> Json<CommandResponse> {
    torrent_action(state, trusted.0, hash, "d.check_hash").await
}

async fn torrent_action(
    state: WebuiState,
    trusted: bool,
    hash: String,
    command: &str,
) -> Json<CommandResponse> {
    let target = match lt_common::HashString::from_hex(&hash) {
        Some(hash) => Target::Download(hash),
        None => {
            return Json(CommandResponse::failure(format!(
                "invalid info hash {}",
                hash
            )))
        }
    };
    Json(
        run_command(
            state,
            trusted,
            command.to_string(),
            target,
            Object::create_empty(),
        )
        .await,
    )
}

/// Run a command on a blocking worker so filesystem/fsync work never stalls the
/// Tokio reactor, mirroring `rt-grpc/src/command_service.rs`.
async fn run_command(
    state: WebuiState,
    trusted: bool,
    command: String,
    target: Target,
    arg: Object,
) -> CommandResponse {
    let map = state.command_map.clone();
    tokio::task::spawn_blocking(move || {
        let map = map.read().unwrap_or_else(|e| e.into_inner());
        match map.call_command(&command, &arg, &target, trusted) {
            Ok(obj) => CommandResponse {
                success: true,
                result: convert::object_to_json(&obj),
                error: String::new(),
            },
            Err(e) => CommandResponse::failure(e.to_string()),
        }
    })
    .await
    .unwrap_or_else(|_| CommandResponse::failure("command task failed".to_string()))
}

#[derive(Debug, Deserialize)]
pub struct MoveTorrentRequest {
    pub target: String,
}

#[derive(Debug, Deserialize)]
pub struct SetTorrentTierRequest {
    pub tier: u8,
}

#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    pub config: RtorrentTomlConfig,
    pub toml: String,
    pub config_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    pub config: Option<RtorrentTomlConfig>,
    pub toml: Option<String>,
    #[serde(default)]
    pub save_file: bool,
}

pub async fn move_torrent(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(hash): Path<String>,
    Json(payload): Json<MoveTorrentRequest>,
) -> Json<CommandResponse> {
    let target = match lt_common::HashString::from_hex(&hash) {
        Some(hash) => Target::Download(hash),
        None => {
            return Json(CommandResponse::failure(format!(
                "invalid info hash {}",
                hash
            )))
        }
    };
    Json(
        run_command(
            state,
            trusted.0,
            "d.move".to_string(),
            target,
            Object::from(payload.target.as_str()),
        )
        .await,
    )
}

#[derive(Debug, Deserialize)]
pub struct SetTorrentLabelRequest {
    #[serde(alias = "category")]
    pub label: Option<String>,
}

pub async fn set_torrent_label(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(hash): Path<String>,
    Json(payload): Json<SetTorrentLabelRequest>,
) -> Json<CommandResponse> {
    let target = match lt_common::HashString::from_hex(&hash) {
        Some(hash) => Target::Download(hash),
        None => {
            return Json(CommandResponse::failure(format!(
                "invalid info hash {}",
                hash
            )))
        }
    };
    let label = payload.label.unwrap_or_default().trim().to_string();
    Json(
        run_command(
            state,
            trusted.0,
            "d.custom1.set".to_string(),
            target,
            Object::from(label.as_str()),
        )
        .await,
    )
}

pub async fn set_torrent_tier(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(hash): Path<String>,
    Json(payload): Json<SetTorrentTierRequest>,
) -> Json<CommandResponse> {
    let target = match lt_common::HashString::from_hex(&hash) {
        Some(hash) => Target::Download(hash),
        None => {
            return Json(CommandResponse::failure(format!(
                "invalid info hash {}",
                hash
            )))
        }
    };
    Json(
        run_command(
            state,
            trusted.0,
            "d.tier.set".to_string(),
            target,
            Object::from(payload.tier as i64),
        )
        .await,
    )
}

pub async fn get_config(State(state): State<WebuiState>) -> Json<ConfigResponse> {
    let map = state.command_map.read().unwrap_or_else(|e| e.into_inner());
    let cfg = RtorrentTomlConfig::from_runtime(&map, &state.tiering_manager, &state.dht_config);
    let toml = cfg.generate_annotated_toml();
    let config_path = state.config_path().map(|p| p.to_string_lossy().to_string());
    Json(ConfigResponse {
        config: cfg,
        toml,
        config_path,
    })
}

pub async fn update_config(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(payload): Json<UpdateConfigRequest>,
) -> Json<CommandResponse> {
    if !trusted.0 {
        return Json(CommandResponse::failure(
            "trusted authentication required to modify configuration".into(),
        ));
    }

    let parsed_cfg = if let Some(cfg) = payload.config {
        cfg
    } else if let Some(toml_str) = payload.toml {
        match RtorrentTomlConfig::parse(&toml_str) {
            Ok(c) => c,
            Err(e) => return Json(CommandResponse::failure(format!("TOML parse error: {e}"))),
        }
    } else {
        return Json(CommandResponse::failure(
            "missing config or toml in request".into(),
        ));
    };

    // Apply config
    {
        let map = state.command_map.read().unwrap_or_else(|e| e.into_inner());
        let mut dummy_net = lt_net::NetworkConfig::default();
        parsed_cfg.apply(&map, &mut dummy_net, &state.dht_config);
    }

    if payload.save_file {
        let path = state
            .config_path()
            .unwrap_or_else(|| PathBuf::from("rstorrent.toml"));
        if let Err(e) = parsed_cfg.save_to_file(&path) {
            return Json(CommandResponse::failure(format!(
                "Applied settings, but saving file failed: {e}"
            )));
        }
    }

    Json(CommandResponse {
        success: true,
        result: serde_json::Value::String("Configuration updated successfully".into()),
        error: String::new(),
    })
}

pub async fn export_config_file(State(state): State<WebuiState>) -> Response {
    let map = state.command_map.read().unwrap_or_else(|e| e.into_inner());
    let cfg = RtorrentTomlConfig::from_runtime(&map, &state.tiering_manager, &state.dht_config);
    let toml = cfg.generate_annotated_toml();

    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"rstorrent.toml\"",
            ),
        ],
        toml,
    )
        .into_response()
}

pub async fn save_config_file(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Json<CommandResponse> {
    if !trusted.0 {
        return Json(CommandResponse::failure(
            "trusted authentication required to save configuration".into(),
        ));
    }
    let map = state.command_map.read().unwrap_or_else(|e| e.into_inner());
    let cfg = RtorrentTomlConfig::from_runtime(&map, &state.tiering_manager, &state.dht_config);
    let path = state
        .config_path()
        .unwrap_or_else(|| PathBuf::from("rstorrent.toml"));
    match cfg.save_to_file(&path) {
        Ok(()) => Json(CommandResponse {
            success: true,
            result: serde_json::Value::String(format!("Configuration saved to {}", path.display())),
            error: String::new(),
        }),
        Err(e) => Json(CommandResponse::failure(format!(
            "Failed to save configuration: {e}"
        ))),
    }
}

#[derive(Debug, Deserialize)]
pub struct AddTorrentRequest {
    /// Magnet URI, HTTP(S) URL, or local file path.
    pub source: Option<String>,
    /// Raw base64-encoded .torrent file payload (alternative to source).
    pub torrent_file_base64: Option<String>,
    /// Custom download destination directory (overrides default).
    pub download_dir: Option<String>,
    /// Whether to start the download immediately or leave it paused. Defaults to true.
    #[serde(default = "default_start_true")]
    pub start_immediately: bool,
    /// Category / label (stored in custom1).
    pub category: Option<String>,
}

fn default_start_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct AddTorrentResponse {
    pub success: bool,
    pub hash: String,
    pub name: String,
    pub error: String,
}

pub async fn add_torrent(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(payload): Json<AddTorrentRequest>,
) -> Json<AddTorrentResponse> {
    if !trusted.0 {
        return Json(AddTorrentResponse {
            success: false,
            hash: String::new(),
            name: String::new(),
            error: "trusted authentication required to add torrents".into(),
        });
    }

    let source_or_data = if let Some(b64) = payload
        .torrent_file_base64
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
        format!("data:application/x-bittorrent;base64,{cleaned}")
    } else if let Some(src) = payload
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        src.to_string()
    } else {
        return Json(AddTorrentResponse {
            success: false,
            hash: String::new(),
            name: String::new(),
            error: "either source URL/magnet or torrent_file_base64 must be provided".into(),
        });
    };

    let command_name = if payload.start_immediately {
        "load.start"
    } else {
        "load.normal"
    };

    let mut args = vec![Object::from(source_or_data.as_str())];
    if let Some(dir) = payload
        .download_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        args.push(Object::from(format!("d.directory.set=\"{dir}\"").as_str()));
    }
    if let Some(cat) = payload
        .category
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        args.push(Object::from(format!("d.custom1.set=\"{cat}\"").as_str()));
    }

    let before_hashes: std::collections::HashSet<String> = {
        let list = state
            .download_list
            .read()
            .unwrap_or_else(|e| e.into_inner());
        list.all()
            .iter()
            .map(|d| d.read().unwrap_or_else(|e| e.into_inner()).info_hash_hex())
            .collect()
    };

    let map = state.command_map.clone();
    let cmd = command_name.to_string();
    let arg_obj = Object::create_list_with(args);

    let res = tokio::task::spawn_blocking(move || {
        let map = map.read().unwrap_or_else(|e| e.into_inner());
        map.call_command(&cmd, &arg_obj, &Target::None, true)
    })
    .await;

    match res {
        Ok(Ok(_)) => {
            let list = state
                .download_list
                .read()
                .unwrap_or_else(|e| e.into_inner());

            let found = list
                .all()
                .iter()
                .find(|d| {
                    let guard = d.read().unwrap_or_else(|e| e.into_inner());
                    !before_hashes.contains(&guard.info_hash_hex())
                })
                .cloned()
                .or_else(|| {
                    if let Some(pos) = source_or_data.find("urn:btih:") {
                        let hash_part = &source_or_data[pos + 9..];
                        let hex_str: String = hash_part
                            .chars()
                            .take_while(|c| c.is_ascii_hexdigit())
                            .collect();
                        if let Some(hs) = lt_common::HashString::from_hex(&hex_str) {
                            return list.find(&hs);
                        }
                    }
                    list.all().last().cloned()
                });

            if let Some(dl) = found {
                {
                    let mut guard = dl.write().unwrap_or_else(|e| e.into_inner());
                    if let Some(dir) = payload
                        .download_dir
                        .as_deref()
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                    {
                        guard.set_directory(dir);
                    }
                    if let Some(cat) = payload
                        .category
                        .as_deref()
                        .map(str::trim)
                        .filter(|c| !c.is_empty())
                    {
                        guard.set_custom_field(1, cat);
                    }
                }
                let guard = dl.read().unwrap_or_else(|e| e.into_inner());
                Json(AddTorrentResponse {
                    success: true,
                    hash: guard.info_hash_hex(),
                    name: guard.name().to_string(),
                    error: String::new(),
                })
            } else {
                Json(AddTorrentResponse {
                    success: true,
                    hash: String::new(),
                    name: String::new(),
                    error: String::new(),
                })
            }
        }
        Ok(Err(e)) => Json(AddTorrentResponse {
            success: false,
            hash: String::new(),
            name: String::new(),
            error: format!("Failed to load torrent: {e}"),
        }),
        Err(e) => Json(AddTorrentResponse {
            success: false,
            hash: String::new(),
            name: String::new(),
            error: format!("Task execution failed: {e}"),
        }),
    }
}

#[derive(Serialize)]
pub struct DaemonStatusResponse {
    pub lifecycle: super::DaemonLifecycle,
    pub uptime_seconds: u64,
    pub active_torrents: usize,
    pub checking_torrents: usize,
}

pub async fn get_status(State(state): State<WebuiState>) -> Json<DaemonStatusResponse> {
    let lifecycle = state.lifecycle();
    let (active_torrents, checking_torrents) = {
        let list = state
            .download_list
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let active = list
            .all()
            .iter()
            .filter(|d| d.read().map(|g| g.is_active()).unwrap_or(false))
            .count();
        let checking = list
            .all()
            .iter()
            .filter(|d| d.read().map(|g| g.is_hash_checking()).unwrap_or(false))
            .count();
        (active, checking)
    };
    Json(DaemonStatusResponse {
        lifecycle,
        uptime_seconds: state.start_time.elapsed().as_secs(),
        active_torrents,
        checking_torrents,
    })
}

pub async fn get_logs(
    State(_state): State<WebuiState>,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
) -> Json<Vec<crate::logging::LogEntry>> {
    let limit = raw_query
        .as_deref()
        .and_then(|q| {
            q.split('&')
                .find_map(|pair| pair.strip_prefix("limit="))
                .and_then(|v| v.parse::<usize>().ok())
        })
        .unwrap_or(200);
    Json(crate::logging::log_hub().recent(limit))
}
