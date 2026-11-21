//! REST handlers for the rules engine (`/api/rules`, `/api/labels`,
//! `/api/seed`, `/api/torrents/{hash}/seed-limits`).
//!
//! Same gate as RSS: shared `require_auth` plus a trusted caller (configured,
//! non-empty bearer token); untrusted callers get 403 even on reads. When the
//! daemon runs without the rules service every endpoint answers 503.
//!
//! Query strings are parsed by hand: the workspace axum build disables the
//! `query` feature (see `rss.rs`).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use rt_rules::{LabelDef, LimitMode, LimitValue, Rule, SeedAction, SeedPolicy, TorrentOverrides};
use serde::{Deserialize, Serialize};

use crate::rules::{DryRunResult, RuleService, RulesSettings, SeedSettings};
use crate::rules_store::{ExportBlob, HistoryEntry, RuleStoreError};
use crate::toml_config::set_toml_value;
use crate::webui::{Trusted, WebuiState};

const DEFAULT_HISTORY_LIMIT: usize = 100;
const MAX_HISTORY_LIMIT: usize = 500;
const DEFAULT_RULE_TEST_LIMIT: usize = 50;

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

type ApiError = (StatusCode, Json<ErrorBody>);

fn error(status: StatusCode, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
}

fn rules_error(err: RuleStoreError) -> ApiError {
    let status = match &err {
        RuleStoreError::Validation(inner) => match inner {
            rt_rules::RuleError::UnknownRule(_) | rt_rules::RuleError::UnknownLabel(_) => {
                StatusCode::NOT_FOUND
            }
            rt_rules::RuleError::DuplicateLabel(_) => StatusCode::CONFLICT,
            _ => StatusCode::BAD_REQUEST,
        },
        RuleStoreError::Io(_) | RuleStoreError::Serde(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, err.to_string())
}

/// Trust gate plus service availability, shared by every handler.
fn service(state: &WebuiState, trusted: Trusted) -> Result<Arc<RuleService>, ApiError> {
    if !trusted.0 {
        return Err(error(
            StatusCode::FORBIDDEN,
            "trusted authentication required for rules endpoints",
        ));
    }
    state.rules().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "rules service is unavailable",
        )
    })
}

// ---- DTOs (contract §6.6 + E1/T8 refinement) ----

/// Global seed settings in the REST shape: scalars with `0 = unlimited`,
/// `action` as the externally tagged `SeedAction` JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedDto {
    pub enabled: bool,
    pub ratio: f64,
    pub seeding_time_secs: u64,
    pub inactive_secs: u64,
    pub mode: String,
    pub action: SeedAction,
    pub check_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesDto {
    pub enabled: bool,
    pub allow_destructive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesSettingsDto {
    pub seed: SeedDto,
    pub rules: RulesDto,
}

#[derive(Debug, Deserialize)]
pub struct SaveSeedRequest {
    pub config: RulesSettingsDto,
    #[serde(default)]
    pub save_file: bool,
}

#[derive(Debug, Deserialize)]
pub struct RuleTestRequest {
    pub rule: Rule,
    pub limit: Option<usize>,
}

fn limit_to_scalar<T: Copy>(value: &LimitValue<T>, zero: T) -> T {
    match value {
        LimitValue::Set(v) => *v,
        _ => zero,
    }
}

fn scalar_to_limit_f64(v: f64) -> LimitValue<f64> {
    if v > 0.0 {
        LimitValue::Set(v)
    } else {
        LimitValue::Unlimited
    }
}

fn scalar_to_limit_u64(v: u64) -> LimitValue<u64> {
    if v > 0 {
        LimitValue::Set(v)
    } else {
        LimitValue::Unlimited
    }
}

fn settings_to_dto(seed: &SeedSettings, rules: &RulesSettings) -> RulesSettingsDto {
    RulesSettingsDto {
        seed: SeedDto {
            enabled: seed.enabled,
            ratio: limit_to_scalar(&seed.policy.ratio, 0.0),
            seeding_time_secs: limit_to_scalar(&seed.policy.seeding_time_secs, 0),
            inactive_secs: limit_to_scalar(&seed.policy.inactive_secs, 0),
            mode: match seed.policy.mode {
                LimitMode::Any => "any".into(),
                LimitMode::All => "all".into(),
            },
            action: seed.policy.action.clone().unwrap_or_default(),
            check_interval_secs: seed.check_interval_secs,
        },
        rules: RulesDto {
            enabled: rules.enabled,
            allow_destructive: rules.allow_destructive,
        },
    }
}

fn dto_to_settings(dto: &RulesSettingsDto) -> (SeedSettings, RulesSettings) {
    let mode = if dto.seed.mode.eq_ignore_ascii_case("all") {
        LimitMode::All
    } else {
        LimitMode::Any
    };
    (
        SeedSettings {
            enabled: dto.seed.enabled,
            policy: SeedPolicy {
                ratio: scalar_to_limit_f64(dto.seed.ratio),
                seeding_time_secs: scalar_to_limit_u64(dto.seed.seeding_time_secs),
                inactive_secs: scalar_to_limit_u64(dto.seed.inactive_secs),
                mode,
                action: Some(dto.seed.action.clone()),
            },
            check_interval_secs: dto.seed.check_interval_secs.max(1),
        },
        RulesSettings {
            enabled: dto.rules.enabled,
            allow_destructive: dto.rules.allow_destructive,
        },
    )
}

/// `SeedAction` → the TOML/CLI string form (`throttle:<name>`, `command:<str>`).
fn action_to_toml_string(action: &SeedAction) -> String {
    match action {
        SeedAction::Stop => "stop".into(),
        SeedAction::Close => "close".into(),
        SeedAction::Remove => "remove".into(),
        SeedAction::RemoveWithData => "remove_with_data".into(),
        SeedAction::Throttle(name) => format!("throttle:{name}"),
        SeedAction::SuperSeed => "super_seed".into(),
        SeedAction::Notify => "notify".into(),
        SeedAction::Command(cmd) => format!("command:{cmd}"),
    }
}

/// Writes the `[seed]`/`[rules]` sections into the daemon TOML file without
/// touching other sections (dotted-key updates, `rest::update_config` style).
fn save_seed_sections(state: &WebuiState, dto: &RulesSettingsDto) -> Result<(), ApiError> {
    let path = state
        .config_path()
        .unwrap_or_else(|| PathBuf::from("rstorrent.toml"));
    let io_error = |err: std::io::Error| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{}: {err}", path.display()),
        )
    };
    let mut content = if path.exists() {
        std::fs::read_to_string(&path).map_err(io_error)?
    } else {
        crate::toml_config::RtorrentTomlConfig::template().generate_annotated_toml()
    };
    let ratio_str = if dto.seed.ratio.fract() == 0.0 {
        format!("{:.1}", dto.seed.ratio)
    } else {
        dto.seed.ratio.to_string()
    };
    let updates: [(&str, String); 9] = [
        ("seed.enabled", dto.seed.enabled.to_string()),
        ("seed.ratio", ratio_str),
        (
            "seed.seeding_time_secs",
            dto.seed.seeding_time_secs.to_string(),
        ),
        ("seed.inactive_secs", dto.seed.inactive_secs.to_string()),
        ("seed.mode", format!("\"{}\"", dto.seed.mode)),
        (
            "seed.action",
            format!("\"{}\"", action_to_toml_string(&dto.seed.action)),
        ),
        (
            "seed.check_interval_secs",
            dto.seed.check_interval_secs.to_string(),
        ),
        ("rules.enabled", dto.rules.enabled.to_string()),
        (
            "rules.allow_destructive",
            dto.rules.allow_destructive.to_string(),
        ),
    ];
    for (key, value) in updates {
        content = set_toml_value(&content, key, &value).map_err(|err| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("set {key}: {err}"),
            )
        })?;
    }
    std::fs::write(&path, content).map_err(io_error)?;
    Ok(())
}

/// Reads a numeric query parameter (hand-rolled: axum `query` feature is off).
fn query_limit(raw: Option<&str>, key: &str, default: usize, max: usize) -> usize {
    let Some(raw) = raw else {
        return default;
    };
    for pair in raw.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next() == Some(key) {
            if let Some(value) = kv.next() {
                if let Ok(parsed) = value.parse::<usize>() {
                    return parsed.min(max);
                }
            }
        }
    }
    default
}

// ---- Seed settings ----

async fn get_seed(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<RulesSettingsDto>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(settings_to_dto(
        &svc.seed_settings(),
        &svc.rules_settings(),
    )))
}

async fn put_seed(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(req): Json<SaveSeedRequest>,
) -> Result<Json<RulesSettingsDto>, ApiError> {
    let svc = service(&state, trusted)?;
    let (seed, rules) = dto_to_settings(&req.config);
    svc.set_seed_settings(seed);
    svc.set_rules_settings(rules);
    if req.save_file {
        save_seed_sections(&state, &req.config)?;
    }
    Ok(Json(settings_to_dto(
        &svc.seed_settings(),
        &svc.rules_settings(),
    )))
}

async fn get_rules_settings(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<RulesDto>, ApiError> {
    let svc = service(&state, trusted)?;
    let settings = svc.rules_settings();
    Ok(Json(RulesDto {
        enabled: settings.enabled,
        allow_destructive: settings.allow_destructive,
    }))
}

async fn put_rules_settings(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(req): Json<RulesDto>,
) -> Result<Json<RulesDto>, ApiError> {
    let svc = service(&state, trusted)?;
    svc.set_rules_settings(RulesSettings {
        enabled: req.enabled,
        allow_destructive: req.allow_destructive,
    });
    Ok(Json(req))
}

// ---- Rules CRUD ----

async fn list_rules(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<Vec<Rule>>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.list_rules()))
}

async fn create_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(rule): Json<Rule>,
) -> Result<(StatusCode, Json<Rule>), ApiError> {
    let svc = service(&state, trusted)?;
    let inserted = svc.add_rule(rule).map_err(rules_error)?;
    Ok((StatusCode::CREATED, Json(inserted)))
}

async fn get_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<u64>,
) -> Result<Json<Rule>, ApiError> {
    let svc = service(&state, trusted)?;
    svc.get_rule(id)
        .map(Json)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("unknown rule {id}")))
}

async fn update_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<u64>,
    Json(mut rule): Json<Rule>,
) -> Result<Json<Rule>, ApiError> {
    let svc = service(&state, trusted)?;
    rule.id = id;
    svc.update_rule(rule.clone()).map_err(rules_error)?;
    Ok(Json(rule))
}

async fn delete_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<u64>,
) -> Result<StatusCode, ApiError> {
    let svc = service(&state, trusted)?;
    svc.remove_rule(id).map_err(rules_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn enable_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<u64>,
) -> Result<StatusCode, ApiError> {
    let svc = service(&state, trusted)?;
    svc.set_rule_enabled(id, true).map_err(rules_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn disable_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(id): Path<u64>,
) -> Result<StatusCode, ApiError> {
    let svc = service(&state, trusted)?;
    svc.set_rule_enabled(id, false).map_err(rules_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn test_rule(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(req): Json<RuleTestRequest>,
) -> Result<Json<DryRunResult>, ApiError> {
    let svc = service(&state, trusted)?;
    let mut result = svc.test_rule(&req.rule).map_err(rules_error)?;
    let limit = req.limit.unwrap_or(DEFAULT_RULE_TEST_LIMIT);
    result.matches.truncate(limit);
    Ok(Json(result))
}

async fn check_rules(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<Vec<HistoryEntry>>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.check_now().await))
}

async fn list_history(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    RawQuery(raw): RawQuery,
) -> Result<Json<Vec<HistoryEntry>>, ApiError> {
    let svc = service(&state, trusted)?;
    let limit = query_limit(
        raw.as_deref(),
        "limit",
        DEFAULT_HISTORY_LIMIT,
        MAX_HISTORY_LIMIT,
    );
    Ok(Json(svc.history(limit)))
}

async fn clear_history(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<StatusCode, ApiError> {
    let svc = service(&state, trusted)?;
    svc.clear_history().map_err(rules_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn export_rules(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<ExportBlob>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.export()))
}

async fn import_rules(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(blob): Json<ExportBlob>,
) -> Result<StatusCode, ApiError> {
    let svc = service(&state, trusted)?;
    svc.import(blob).map_err(rules_error)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Labels ----

async fn list_labels(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
) -> Result<Json<Vec<LabelDef>>, ApiError> {
    let svc = service(&state, trusted)?;
    Ok(Json(svc.list_labels()))
}

async fn create_label(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Json(label): Json<LabelDef>,
) -> Result<(StatusCode, Json<LabelDef>), ApiError> {
    let svc = service(&state, trusted)?;
    let inserted = svc.add_label(label).map_err(rules_error)?;
    Ok((StatusCode::CREATED, Json(inserted)))
}

async fn update_label(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(name): Path<String>,
    Json(mut label): Json<LabelDef>,
) -> Result<Json<LabelDef>, ApiError> {
    let svc = service(&state, trusted)?;
    label.name = name;
    svc.update_label(label.clone()).map_err(rules_error)?;
    Ok(Json(label))
}

async fn delete_label(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let svc = service(&state, trusted)?;
    svc.remove_label(&name).map_err(rules_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_label_policy(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(name): Path<String>,
) -> Result<Json<SeedPolicy>, ApiError> {
    let svc = service(&state, trusted)?;
    svc.list_labels()
        .into_iter()
        .find(|l| l.name == name)
        .map(|l| Json(l.policy))
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("unknown label {name}")))
}

async fn put_label_policy(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(name): Path<String>,
    Json(policy): Json<SeedPolicy>,
) -> Result<Json<SeedPolicy>, ApiError> {
    let svc = service(&state, trusted)?;
    let mut label = svc
        .list_labels()
        .into_iter()
        .find(|l| l.name == name)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("unknown label {name}")))?;
    label.policy = policy.clone();
    svc.update_label(label).map_err(rules_error)?;
    Ok(Json(policy))
}

// ---- Per-torrent seed limits ----

async fn get_seed_limits(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(hash): Path<String>,
) -> Result<Json<rt_rules::EffectiveLimits>, ApiError> {
    let svc = service(&state, trusted)?;
    svc.effective_limits(&hash)
        .map(Json)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("unknown torrent {hash}")))
}

async fn put_seed_limits(
    State(state): State<WebuiState>,
    Extension(trusted): Extension<Trusted>,
    Path(hash): Path<String>,
    Json(overrides): Json<TorrentOverrides>,
) -> Result<Json<rt_rules::EffectiveLimits>, ApiError> {
    let svc = service(&state, trusted)?;
    svc.set_torrent_overrides(&hash, overrides)
        .await
        .map_err(rules_error)?;
    svc.effective_limits(&hash)
        .map(Json)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("unknown torrent {hash}")))
}

/// Rules routes, merged into the gated `/api` router by [`super::router`].
pub fn routes() -> Router<WebuiState> {
    Router::new()
        .route("/seed", get(get_seed).put(put_seed))
        .route(
            "/rules/settings",
            get(get_rules_settings).put(put_rules_settings),
        )
        .route("/rules", get(list_rules).post(create_rule))
        .route("/rules/test", post(test_rule))
        .route("/rules/check", post(check_rules))
        .route("/rules/history", get(list_history).delete(clear_history))
        .route("/rules/export", get(export_rules))
        .route("/rules/import", post(import_rules))
        .route(
            "/rules/{id}",
            get(get_rule).put(update_rule).delete(delete_rule),
        )
        .route("/rules/{id}/enable", post(enable_rule))
        .route("/rules/{id}/disable", post(disable_rule))
        .route("/labels", get(list_labels).post(create_label))
        .route("/labels/{name}", put(update_label).delete(delete_label))
        .route(
            "/labels/{name}/policy",
            get(get_label_policy).put(put_label_policy),
        )
        .route(
            "/torrents/{hash}/seed-limits",
            get(get_seed_limits).put(put_seed_limits),
        )
}
