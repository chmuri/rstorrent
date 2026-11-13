//! REST handlers for the WebUI API.
//!
//! Read endpoints snapshot the download list under a short-lived lock. Command
//! execution mirrors the gRPC `CommandService`: it runs on a blocking worker
//! (commands can fsync or touch the filesystem) and uses the server-decided
//! trust level carried in the [`Trusted`] request extension.

use crate::webui::convert::{self, SessionStats, TorrentSummary, VersionInfo};
use crate::webui::{Trusted, WebuiState};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use lt_common::Object;
use rt_rpc::Target;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
