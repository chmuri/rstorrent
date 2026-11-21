//! UI-side mirror of the automation-rules backend contract (plan.md §6.1/§6.6):
//! DTO shapes, the `/rules` page store and a REST client for `/api/rules`,
//! `/api/labels`, `/api/seed` and `/api/torrents/{hash}/seed-limits`. DTOs are
//! duplicated here by repo convention (see `state/rss.rs`) instead of sharing
//! a common crate with the backend.
//!
//! Serde shapes are binding: enums are externally tagged with `snake_case`
//! names, so [`LimitValue::Inherit`] serializes as `"inherit"`,
//! [`LimitValue::Set`] as `{"set": 2.0}`, and struct variants as
//! `{"free_space_below": {"bytes": …, "path": …}}`. The shapes mirror
//! `crates/rt-rules/src/model.rs` and `crates/rstorrent/src/rules_store.rs`.

use gloo_net::http::{Method, RequestBuilder, Response};
use leptos::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::api::ApiError;
use super::store::AppState;

pub type RuleId = u64;

fn default_true() -> bool {
    true
}

fn default_check_interval_secs() -> u64 {
    10
}

/// A limit value with explicit inheritance.
/// JSON: `"inherit"` | `"unlimited"` | `{"set": <value>}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LimitValue<T> {
    /// Defer to the next level up (torrent → label → global).
    #[default]
    Inherit,
    /// Explicitly no limit at this level.
    Unlimited,
    /// An explicit value.
    Set(T),
}

impl<T> LimitValue<T> {
    /// Whether this level expresses an explicit choice (no inheritance).
    #[allow(dead_code)]
    pub fn is_explicit(&self) -> bool {
        !matches!(self, Self::Inherit)
    }
}

/// How the active seed limits combine: any one reached fires the action,
/// or all of them must be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LimitMode {
    /// OR — the first reached limit fires (default, rtorrent-like).
    #[default]
    Any,
    /// AND — every active limit must be reached.
    All,
}

/// What to do when a seed limit is reached.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SeedAction {
    /// `d.stop` — keep the torrent in the session, resumable (default).
    #[default]
    Stop,
    /// `d.try_close` + `d.ignore_commands.set=1` (legacy ratio semantics).
    Close,
    /// `d.erase` — remove the torrent, keep the data.
    Remove,
    /// `d.delete_data` + `d.erase` — destructive, double-gated (plan §5.5).
    RemoveWithData,
    /// `d.throttle_name.set` — soft degradation instead of stopping.
    Throttle(String),
    /// `d.connection_seed.set=initial_seed` (BEP 16 super-seeding).
    SuperSeed,
    /// Only emit the event/audit entry, do not change torrent state.
    Notify,
    /// Arbitrary `d.*` command string, executed with `Target::Download`.
    Command(String),
}

/// Declarative seed limits at one inheritance level (label policy).
/// `Inherit` fields defer upward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SeedPolicy {
    /// Share ratio limit, `1.0` = 100%.
    #[serde(default)]
    pub ratio: LimitValue<f64>,
    /// Accumulated active-seeding time limit, seconds.
    #[serde(default)]
    pub seeding_time_secs: LimitValue<u64>,
    /// No-upload-while-seeding limit, seconds.
    #[serde(default)]
    pub inactive_secs: LimitValue<u64>,
    #[serde(default)]
    pub mode: LimitMode,
    /// `None` = inherit the action from the level above.
    #[serde(default)]
    pub action: Option<SeedAction>,
}

/// Per-torrent overrides: request body of `PUT /api/torrents/{hash}/seed-limits`
/// (plan §6.6). Mirrors `rt_rules::TorrentOverrides` — same shape as
/// [`SeedPolicy`] minus `mode` (mode resolves from label/global only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TorrentSeedLimits {
    #[serde(default)]
    pub ratio: LimitValue<f64>,
    #[serde(default)]
    pub seeding_time_secs: LimitValue<u64>,
    #[serde(default)]
    pub inactive_secs: LimitValue<u64>,
    #[serde(default)]
    pub action: Option<SeedAction>,
}

/// Where each effective value came from: `"global"`, `"label:<name>"` or
/// `"torrent"`. Powers the UI "effective limits" insight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitSources {
    pub ratio: String,
    pub seeding_time_secs: String,
    pub inactive_secs: String,
    pub mode: String,
    pub action: String,
}

/// Resolved limits for one torrent; response of
/// `GET /api/torrents/{hash}/seed-limits`. `None` means unlimited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveLimits {
    pub ratio: Option<f64>,
    pub seeding_time_secs: Option<u64>,
    pub inactive_secs: Option<u64>,
    pub mode: LimitMode,
    pub action: SeedAction,
    pub sources: LimitSources,
}

/// A first-class label: registry entry with color, seed policy and an
/// optional default save path for newly added torrents carrying the label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelDef {
    /// Matches `d.custom1`; non-empty, trimmed, case-sensitive.
    pub name: String,
    /// `"#rrggbb"` for the UI.
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub policy: SeedPolicy,
    #[serde(default)]
    pub save_path: Option<String>,
}

/// Which torrents a rule applies to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    /// Primary label (`d.custom1`) equality.
    Label(String),
    /// Membership in `d.tags`.
    Tag(String),
    /// Any tracker host matches (case-insensitive, no scheme/port).
    Tracker(String),
    /// RSS provenance from `d.custom`; `None` fields match any value.
    /// Both `None` = every torrent added via RSS.
    Rss {
        feed_id: Option<u64>,
        rule_id: Option<u64>,
    },
}

/// Lifecycle events a rule can trigger on (mapped from `event.download.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Inserted,
    Finished,
    Paused,
    Resumed,
    HashDone,
}

/// When a rule is evaluated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Every worker tick.
    #[default]
    Periodic,
    /// Immediately when the matching lifecycle event fires.
    Event(EventKind),
}

/// How a rule's conditions combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CondMode {
    #[default]
    All,
    Any,
}

/// A condition over a torrent metrics snapshot. Swarm conditions (`Seeds*`,
/// `Leechers*`) evaluate to **false when scrape data is unavailable** — the
/// engine never guesses; dry-run reports those hashes in `no_data`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    RatioAtLeast(f64),
    RatioBelow(f64),
    SeedingTimeAtLeast(u64),
    IdleAtLeast(u64),
    /// Seconds since the torrent was added (`load_date`).
    AgeAtLeast(u64),
    Complete(bool),
    Active(bool),
    Stopped(bool),
    SizeAtLeast(u64),
    SizeAtMost(u64),
    SeedsAtLeast(u32),
    SeedsAtMost(u32),
    LeechersAtLeast(u32),
    LabelIs(String),
    TagHas(String),
    TrackerIs(String),
    /// Case-insensitive regex over the torrent name (compiled once).
    NameMatches(String),
    AddedByRss,
    /// Free space on `path` (or the torrent's directory when `None`).
    FreeSpaceBelow {
        bytes: u64,
        path: Option<String>,
    },
}

/// An imperative action executed on the matching torrent, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Exception marker: stop evaluating this torrent, do nothing (audit only).
    Noop,
    Stop,
    Pause,
    Start,
    Close,
    /// Destructive gate applies (plan §5.5).
    Remove,
    /// Destructive gate applies (plan §5.5).
    RemoveWithData,
    MoveToDir(String),
    MoveToTier(u8),
    SetLabel(String),
    AddTag(String),
    RemoveTag(String),
    /// `None` clears the throttle.
    SetThrottle(Option<String>),
    /// 0..3
    SetPriority(u8),
    SuperSeed(bool),
    /// argv, no shell, timeout + kill in the executor.
    Execute(Vec<String>),
    /// POST JSON `{event, rule_id, hash, name, label, at}`; http/https only.
    Webhook {
        url: String,
    },
    /// Raw `d.*` command string executed with `Target::Download`.
    Command(String),
}

/// A user-defined automation rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Assigned by the store (≥ 1).
    pub id: RuleId,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Lower values are evaluated first; sort key is `(priority, id)`.
    #[serde(default)]
    pub priority: i32,
    pub scope: Scope,
    #[serde(default)]
    pub trigger: Trigger,
    #[serde(default)]
    pub cond_mode: CondMode,
    /// Non-empty (validated on create/update).
    pub conditions: Vec<Condition>,
    /// Non-empty; executed in order.
    pub actions: Vec<Action>,
    /// Fire at most once per torrent (sidecar ledger).
    #[serde(default)]
    pub once: bool,
    /// Minimum spacing between firings on the same torrent.
    #[serde(default)]
    pub cooldown_secs: Option<u64>,
    /// Per-rule destructive gate, ANDed with the global gate.
    #[serde(default)]
    pub allow_destructive: bool,
}

/// One evaluation outcome; element of the `POST /api/rules/check` response.
/// `rule_id == 0` marks the built-in seed-policy enforcement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub rule_id: RuleId,
    pub actions: Vec<Action>,
    pub reason: String,
}

/// Outcome status of one executed (or skipped) rule decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    Ok,
    Failed,
    Skipped,
}

/// One immutable audit entry (`GET /api/rules/history`). Mirrors
/// `rstorrent::rules_store::HistoryEntry`: `actions` are action *names* in
/// execution order (snake_case of [`Action`]), not full action payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: u64,
    pub at: u64,
    /// `0` = built-in seed-policy enforcement.
    pub rule_id: RuleId,
    pub rule_name: String,
    pub torrent_hash: String,
    pub torrent_name: String,
    pub actions: Vec<String>,
    pub status: HistoryStatus,
    pub reason: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// One dry-run match (`POST /api/rules/test`, plan §6.6 REST shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DryRunMatch {
    pub torrent_hash: String,
    pub name: String,
    pub actions: Vec<Action>,
    pub reason: String,
}

/// Response of `POST /api/rules/test`: preview only, zero mutations.
/// `no_data` lists torrent hashes whose swarm conditions could not be
/// evaluated because scrape data is missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DryRunResult {
    pub matches: Vec<DryRunMatch>,
    pub no_data: Vec<String>,
}

/// Request body of `POST /api/rules/test`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleTestRequest {
    pub rule: Rule,
    pub limit: Option<usize>,
}

/// Backup blob: response of `GET /api/rules/export`, body of
/// `POST /api/rules/import`. Mirrors `rstorrent::rules_store::ExportBlob`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RulesExport {
    pub labels: Vec<LabelDef>,
    pub rules: Vec<Rule>,
}

/// Global seed settings — the `[seed]` TOML section (plan §6.5). Numeric
/// sentinels follow the TOML contract: `ratio == 0` and `*_secs == 0` mean
/// unlimited. `action` uses the §6.1 [`SeedAction`] JSON shape (externally
/// tagged), not the TOML colon string (`throttle:<name>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeedSettings {
    /// Master switch of the seed policies; `false` = legacy `ratio.*` only.
    #[serde(default)]
    pub enabled: bool,
    /// Global default ratio, `1.0` = 100%; `0` = unlimited.
    #[serde(default)]
    pub ratio: f64,
    /// Global seeding time limit, seconds; `0` = unlimited.
    #[serde(default)]
    pub seeding_time_secs: u64,
    /// Global inactive limit, seconds; `0` = unlimited.
    #[serde(default)]
    pub inactive_secs: u64,
    #[serde(default)]
    pub mode: LimitMode,
    #[serde(default)]
    pub action: SeedAction,
    #[serde(default = "default_check_interval_secs")]
    pub check_interval_secs: u64,
}

impl Default for SeedSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ratio: 0.0,
            seeding_time_secs: 0,
            inactive_secs: 0,
            mode: LimitMode::default(),
            action: SeedAction::default(),
            check_interval_secs: default_check_interval_secs(),
        }
    }
}

/// The `[rules]` TOML section (plan §6.5): automation kill switch and the
/// global destructive gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RulesFlags {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allow_destructive: bool,
}

impl Default for RulesFlags {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_destructive: false,
        }
    }
}

/// DTO of `GET/PUT /api/seed` — the TOML shape (`[seed]` + `[rules]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RulesSettings {
    #[serde(default)]
    pub seed: SeedSettings,
    #[serde(default)]
    pub rules: RulesFlags,
}

/// Request body of `PUT /api/seed` (`{config, save_file?}` like RSS settings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveSeedSettingsRequest {
    pub config: RulesSettings,
    pub save_file: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RulesTab {
    #[default]
    Rules,
    History,
}

/// Reactive state of the `/rules` page. `reload` is bumped after a mutation
/// (or by the 15 s poll) so the page refetches the lists. `dry_run_result` is
/// owned by the rule editor (T9) and is deliberately never touched by polling.
#[derive(Clone, Copy)]
pub struct RulesPageStore {
    pub tab: RwSignal<RulesTab>,
    pub rules: RwSignal<Vec<Rule>>,
    pub labels: RwSignal<Vec<LabelDef>>,
    pub seed_settings: RwSignal<RulesSettings>,
    pub history: RwSignal<Vec<HistoryEntry>>,
    pub dry_run_result: RwSignal<Option<DryRunResult>>,
    pub loading: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub reload: RwSignal<u64>,
    /// Guards against stale `load_all` responses (RSS `request_gen` pattern,
    /// kept inside the store so the page effect stays thin).
    request_gen: RwSignal<u64>,
}

impl RulesPageStore {
    pub fn new() -> Self {
        Self {
            tab: RwSignal::new(RulesTab::default()),
            rules: RwSignal::new(Vec::new()),
            labels: RwSignal::new(Vec::new()),
            seed_settings: RwSignal::new(RulesSettings::default()),
            history: RwSignal::new(Vec::new()),
            dry_run_result: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            reload: RwSignal::new(0),
            request_gen: RwSignal::new(0),
        }
    }

    /// Discard in-flight `load_all` responses (e.g. when going offline).
    pub fn invalidate(&self) {
        self.request_gen.update(|gen| *gen += 1);
        self.loading.set(false);
    }

    /// Fetch rules, labels, seed settings and history, then apply them to the
    /// store. A response superseded by a newer run (or by [`Self::invalidate`])
    /// is dropped and reported as `Ok(())`; `loading`/`error` track the newest
    /// run only.
    pub async fn load_all(&self, state: &AppState) -> Result<(), ApiError> {
        self.request_gen.update(|gen| *gen += 1);
        let gen = self.request_gen.get_untracked();
        self.loading.set(true);
        self.error.set(None);
        let result = async {
            let rules = rules_list(state).await?;
            let labels = labels_list(state).await?;
            let seed_settings = seed_get_settings(state).await?;
            let history = rules_list_history(state, None).await?;
            Ok::<_, ApiError>((rules, labels, seed_settings, history))
        }
        .await;
        // Drop stale responses superseded by a newer run.
        if self.request_gen.get_untracked() != gen {
            return Ok(());
        }
        self.loading.set(false);
        match result {
            Ok((rules, labels, seed_settings, history)) => {
                self.rules.set(rules);
                self.labels.set(labels);
                self.seed_settings.set(seed_settings);
                self.history.set(history);
                Ok(())
            }
            Err(err) => {
                self.error.set(Some(err.to_string()));
                Err(err)
            }
        }
    }
}

impl Default for RulesPageStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// REST client for /api/rules, /api/labels, /api/seed and per-torrent seed
// limits (Bearer auth, same pattern as state/rss.rs and api.rs::fetch_json)
// ---------------------------------------------------------------------------

fn serialize_body<T: Serialize>(value: &T) -> Result<String, ApiError> {
    serde_json::to_string(value)
        .map_err(|err| ApiError::Other(format!("Invalid request payload: {err}")))
}

async fn rules_send(
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

async fn rules_json<T: DeserializeOwned>(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<String>,
) -> Result<T, ApiError> {
    let response = rules_send(state, method, path, body).await?;
    response
        .json::<T>()
        .await
        .map_err(|err| ApiError::Other(format!("Invalid payload: {err}")))
}

/// For endpoints answering 204 No Content.
async fn rules_empty(state: &AppState, method: Method, path: &str) -> Result<(), ApiError> {
    rules_send(state, method, path, None).await?;
    Ok(())
}

/// Percent-encodes a path/query value (RFC 3986 unreserved set stays literal).
/// Copied from `state/rss.rs` (private there); label names may contain spaces
/// or non-ASCII characters and must be escaped as a single path segment.
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

fn rules_history_path(limit: Option<usize>) -> String {
    match limit {
        Some(limit) => format!("/api/rules/history?limit={limit}"),
        None => "/api/rules/history".to_string(),
    }
}

fn label_path(name: &str) -> String {
    format!("/api/labels/{}", percent_encode(name))
}

// --- /api/rules -------------------------------------------------------------

pub async fn rules_list(state: &AppState) -> Result<Vec<Rule>, ApiError> {
    rules_json(state, Method::GET, "/api/rules", None).await
}

/// Single-rule fetch — part of the API client surface for editor flows.
#[allow(dead_code)]
pub async fn rules_get(state: &AppState, id: RuleId) -> Result<Rule, ApiError> {
    rules_json(state, Method::GET, &format!("/api/rules/{id}"), None).await
}

pub async fn rules_create(state: &AppState, rule: &Rule) -> Result<Rule, ApiError> {
    rules_json(
        state,
        Method::POST,
        "/api/rules",
        Some(serialize_body(rule)?),
    )
    .await
}

pub async fn rules_update(state: &AppState, id: RuleId, rule: &Rule) -> Result<Rule, ApiError> {
    rules_json(
        state,
        Method::PUT,
        &format!("/api/rules/{id}"),
        Some(serialize_body(rule)?),
    )
    .await
}

pub async fn rules_delete(state: &AppState, id: RuleId) -> Result<(), ApiError> {
    rules_empty(state, Method::DELETE, &format!("/api/rules/{id}")).await
}

pub async fn rules_enable(state: &AppState, id: RuleId) -> Result<(), ApiError> {
    rules_empty(state, Method::POST, &format!("/api/rules/{id}/enable")).await
}

pub async fn rules_disable(state: &AppState, id: RuleId) -> Result<(), ApiError> {
    rules_empty(state, Method::POST, &format!("/api/rules/{id}/disable")).await
}

/// Dry-run: preview matches without mutating anything (plan §6.6).
pub async fn rules_test(
    state: &AppState,
    rule: &Rule,
    limit: Option<usize>,
) -> Result<DryRunResult, ApiError> {
    let body = RuleTestRequest {
        rule: rule.clone(),
        limit,
    };
    rules_json(
        state,
        Method::POST,
        "/api/rules/test",
        Some(serialize_body(&body)?),
    )
    .await
}

/// Force an evaluation pass; returns the fired decisions.
pub async fn rules_check(state: &AppState) -> Result<Vec<Decision>, ApiError> {
    rules_json(state, Method::POST, "/api/rules/check", None).await
}

pub async fn rules_list_history(
    state: &AppState,
    limit: Option<usize>,
) -> Result<Vec<HistoryEntry>, ApiError> {
    rules_json(state, Method::GET, &rules_history_path(limit), None).await
}

pub async fn rules_clear_history(state: &AppState) -> Result<(), ApiError> {
    rules_empty(state, Method::DELETE, "/api/rules/history").await
}

pub async fn rules_export(state: &AppState) -> Result<RulesExport, ApiError> {
    rules_json(state, Method::GET, "/api/rules/export", None).await
}

/// Merge a backup blob with validation (conflicts → error).
pub async fn rules_import(state: &AppState, blob: &RulesExport) -> Result<(), ApiError> {
    rules_send(
        state,
        Method::POST,
        "/api/rules/import",
        Some(serialize_body(blob)?),
    )
    .await?;
    Ok(())
}

// --- /api/labels ------------------------------------------------------------

pub async fn labels_list(state: &AppState) -> Result<Vec<LabelDef>, ApiError> {
    rules_json(state, Method::GET, "/api/labels", None).await
}

/// Duplicate names are rejected by the backend with 409.
pub async fn labels_create(state: &AppState, label: &LabelDef) -> Result<LabelDef, ApiError> {
    rules_json(
        state,
        Method::POST,
        "/api/labels",
        Some(serialize_body(label)?),
    )
    .await
}

/// Update color/policy/save_path of a registered label.
pub async fn labels_update(
    state: &AppState,
    name: &str,
    label: &LabelDef,
) -> Result<LabelDef, ApiError> {
    rules_json(
        state,
        Method::PUT,
        &label_path(name),
        Some(serialize_body(label)?),
    )
    .await
}

/// Removes the registry entry only — torrent `d.custom1` values stay intact.
pub async fn labels_delete(state: &AppState, name: &str) -> Result<(), ApiError> {
    rules_empty(state, Method::DELETE, &label_path(name)).await
}

/// Policy fetch — part of the API client surface for dialog flows.
#[allow(dead_code)]
pub async fn labels_get_policy(state: &AppState, name: &str) -> Result<SeedPolicy, ApiError> {
    rules_json(
        state,
        Method::GET,
        &format!("{}/policy", label_path(name)),
        None,
    )
    .await
}

pub async fn labels_update_policy(
    state: &AppState,
    name: &str,
    policy: &SeedPolicy,
) -> Result<SeedPolicy, ApiError> {
    rules_json(
        state,
        Method::PUT,
        &format!("{}/policy", label_path(name)),
        Some(serialize_body(policy)?),
    )
    .await
}

// --- /api/seed --------------------------------------------------------------

pub async fn seed_get_settings(state: &AppState) -> Result<RulesSettings, ApiError> {
    rules_json(state, Method::GET, "/api/seed", None).await
}

/// `save_file` also persists the TOML (RSS settings precedent); `false`
/// applies in-memory only.
pub async fn seed_save_settings(
    state: &AppState,
    config: &RulesSettings,
    save_file: bool,
) -> Result<RulesSettings, ApiError> {
    let body = SaveSeedSettingsRequest {
        config: config.clone(),
        save_file,
    };
    rules_json(
        state,
        Method::PUT,
        "/api/seed",
        Some(serialize_body(&body)?),
    )
    .await
}

// --- /api/torrents/{hash}/seed-limits ----------------------------------------

/// Effective (resolved) limits with per-field sources.
pub async fn torrent_seed_limits(
    state: &AppState,
    hash: &str,
) -> Result<EffectiveLimits, ApiError> {
    rules_json(
        state,
        Method::GET,
        &format!("/api/torrents/{hash}/seed-limits"),
        None,
    )
    .await
}

/// Replace the per-torrent overrides; returns the recomputed effective limits.
pub async fn update_torrent_seed_limits(
    state: &AppState,
    hash: &str,
    overrides: &TorrentSeedLimits,
) -> Result<EffectiveLimits, ApiError> {
    rules_json(
        state,
        Method::PUT,
        &format!("/api/torrents/{hash}/seed-limits"),
        Some(serialize_body(overrides)?),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rule() -> Rule {
        Rule {
            id: 7,
            name: "private cleanup".to_string(),
            enabled: false,
            priority: -5,
            scope: Scope::Rss {
                feed_id: Some(2),
                rule_id: Some(3),
            },
            trigger: Trigger::Event(EventKind::Finished),
            cond_mode: CondMode::Any,
            conditions: vec![
                Condition::SeedingTimeAtLeast(86_400),
                Condition::SeedsAtLeast(10),
                Condition::FreeSpaceBelow {
                    bytes: 1 << 30,
                    path: None,
                },
            ],
            actions: vec![
                Action::AddTag("done".to_string()),
                Action::MoveToTier(2),
                Action::Webhook {
                    url: "https://hooks.example.org/x".to_string(),
                },
                Action::Execute(vec!["/usr/bin/notify".to_string(), "done".to_string()]),
            ],
            once: true,
            cooldown_secs: Some(3600),
            allow_destructive: true,
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
    fn limit_value_serde_shapes() {
        assert_eq!(
            serde_json::to_string(&LimitValue::<f64>::Inherit).unwrap(),
            "\"inherit\""
        );
        assert_eq!(
            serde_json::to_string(&LimitValue::<f64>::Unlimited).unwrap(),
            "\"unlimited\""
        );
        assert_eq!(
            serde_json::to_string(&LimitValue::Set(2.0f64)).unwrap(),
            "{\"set\":2.0}"
        );
        let parsed: LimitValue<u64> = serde_json::from_str("{\"set\":3600}").unwrap();
        assert_eq!(parsed, LimitValue::Set(3600));
        assert_eq!(LimitValue::<f64>::default(), LimitValue::Inherit);
        assert!(!LimitValue::<f64>::Inherit.is_explicit());
        assert!(LimitValue::Set(1.0f64).is_explicit());
    }

    #[test]
    fn seed_action_serde_shapes() {
        assert_eq!(
            serde_json::to_string(&SeedAction::Stop).unwrap(),
            "\"stop\""
        );
        assert_eq!(
            serde_json::to_string(&SeedAction::RemoveWithData).unwrap(),
            "\"remove_with_data\""
        );
        assert_eq!(
            serde_json::to_string(&SeedAction::Throttle("slow".to_string())).unwrap(),
            "{\"throttle\":\"slow\"}"
        );
        assert_eq!(SeedAction::default(), SeedAction::Stop);
        round_trip(&SeedAction::Command("d.stop=".to_string()));
    }

    #[test]
    fn seed_policy_defaults_and_round_trip() {
        let empty: SeedPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, SeedPolicy::default());
        assert_eq!(empty.mode, LimitMode::Any);
        assert_eq!(empty.action, None);
        round_trip(&SeedPolicy {
            ratio: LimitValue::Set(3.0),
            seeding_time_secs: LimitValue::Set(7 * 24 * 3600),
            inactive_secs: LimitValue::Unlimited,
            mode: LimitMode::All,
            action: Some(SeedAction::Throttle("seed".to_string())),
        });
    }

    #[test]
    fn scope_and_trigger_serde_shapes() {
        assert_eq!(serde_json::to_string(&Scope::Global).unwrap(), "\"global\"");
        assert_eq!(
            serde_json::to_string(&Scope::Label("movies".to_string())).unwrap(),
            "{\"label\":\"movies\"}"
        );
        assert_eq!(
            serde_json::to_string(&Scope::Rss {
                feed_id: Some(1),
                rule_id: None
            })
            .unwrap(),
            "{\"rss\":{\"feed_id\":1,\"rule_id\":null}}"
        );
        assert_eq!(
            serde_json::to_string(&Trigger::Periodic).unwrap(),
            "\"periodic\""
        );
        assert_eq!(
            serde_json::to_string(&Trigger::Event(EventKind::HashDone)).unwrap(),
            "{\"event\":\"hash_done\"}"
        );
        assert_eq!(Trigger::default(), Trigger::Periodic);
        round_trip(&Scope::Tracker("tracker.example.org".to_string()));
        round_trip(&Scope::Tag("keep".to_string()));
    }

    #[test]
    fn rule_backward_compatible_defaults() {
        // Minimal rule JSON: optional fields must default, not fail.
        let json = r#"{
            "id": 1, "name": "cleanup", "scope": "global",
            "conditions": [{"ratio_at_least": 2.0}],
            "actions": ["stop"]
        }"#;
        let rule: Rule = serde_json::from_str(json).unwrap();
        assert!(rule.enabled);
        assert_eq!(rule.priority, 0);
        assert_eq!(rule.trigger, Trigger::Periodic);
        assert_eq!(rule.cond_mode, CondMode::All);
        assert!(!rule.once);
        assert_eq!(rule.cooldown_secs, None);
        assert!(!rule.allow_destructive);
        round_trip(&rule);
    }

    #[test]
    fn rule_full_round_trip() {
        round_trip(&sample_rule());
    }

    #[test]
    fn condition_and_action_variant_round_trips() {
        let conditions = vec![
            Condition::RatioAtLeast(1.0),
            Condition::RatioBelow(0.5),
            Condition::SeedingTimeAtLeast(60),
            Condition::IdleAtLeast(60),
            Condition::AgeAtLeast(60),
            Condition::Complete(true),
            Condition::Active(false),
            Condition::Stopped(true),
            Condition::SizeAtLeast(1),
            Condition::SizeAtMost(2),
            Condition::SeedsAtLeast(3),
            Condition::SeedsAtMost(4),
            Condition::LeechersAtLeast(5),
            Condition::LabelIs("movies".to_string()),
            Condition::TagHas("keep".to_string()),
            Condition::TrackerIs("example.org".to_string()),
            Condition::NameMatches("show.*1080p".to_string()),
            Condition::AddedByRss,
            Condition::FreeSpaceBelow {
                bytes: 10,
                path: Some("/downloads".to_string()),
            },
        ];
        for cond in &conditions {
            round_trip(cond);
        }
        let actions = vec![
            Action::Noop,
            Action::Stop,
            Action::Pause,
            Action::Start,
            Action::Close,
            Action::Remove,
            Action::RemoveWithData,
            Action::MoveToDir("/cold".to_string()),
            Action::MoveToTier(1),
            Action::SetLabel("movies".to_string()),
            Action::AddTag("x".to_string()),
            Action::RemoveTag("x".to_string()),
            Action::SetThrottle(None),
            Action::SetThrottle(Some("seed".to_string())),
            Action::SetPriority(3),
            Action::SuperSeed(true),
            Action::Execute(vec!["/bin/true".to_string()]),
            Action::Webhook {
                url: "http://localhost:9/hook".to_string(),
            },
            Action::Command("d.stop=".to_string()),
        ];
        for action in &actions {
            round_trip(action);
        }
    }

    #[test]
    fn label_def_defaults_and_round_trip() {
        let minimal: LabelDef = serde_json::from_str(r#"{"name":"movies"}"#).unwrap();
        assert_eq!(minimal.color, None);
        assert_eq!(minimal.policy, SeedPolicy::default());
        assert_eq!(minimal.save_path, None);
        round_trip(&LabelDef {
            name: "movies".to_string(),
            color: Some("#7c3aed".to_string()),
            policy: SeedPolicy {
                ratio: LimitValue::Set(3.0),
                ..Default::default()
            },
            save_path: Some("/disk/movies".to_string()),
        });
    }

    #[test]
    fn effective_limits_and_overrides_round_trip() {
        round_trip(&EffectiveLimits {
            ratio: Some(3.0),
            seeding_time_secs: None,
            inactive_secs: Some(1800),
            mode: LimitMode::Any,
            action: SeedAction::Remove,
            sources: LimitSources {
                ratio: "label:movies".to_string(),
                seeding_time_secs: "torrent".to_string(),
                inactive_secs: "torrent".to_string(),
                mode: "global".to_string(),
                action: "torrent".to_string(),
            },
        });
        let overrides = TorrentSeedLimits {
            ratio: LimitValue::Unlimited,
            seeding_time_secs: LimitValue::Set(60),
            inactive_secs: LimitValue::Inherit,
            action: Some(SeedAction::Stop),
        };
        round_trip(&overrides);
        let empty: TorrentSeedLimits = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, TorrentSeedLimits::default());
    }

    #[test]
    fn history_entry_round_trip() {
        // `actions` are names (strings), mirroring rules_store::HistoryEntry.
        let entry = HistoryEntry {
            id: 7,
            at: 1_700_000_200,
            rule_id: 3,
            rule_name: "private cleanup".to_string(),
            torrent_hash: "aabbccdd".to_string(),
            torrent_name: "Show S02E05 1080p".to_string(),
            actions: vec!["add_tag".to_string(), "move_to_tier".to_string()],
            status: HistoryStatus::Ok,
            reason: "seeding_time 86400 >= 86400".to_string(),
            error: None,
        };
        round_trip(&entry);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
        round_trip(&HistoryEntry {
            id: 8,
            at: 1_700_000_300,
            rule_id: 0,
            rule_name: "seed policy".to_string(),
            torrent_hash: "eeff0011".to_string(),
            torrent_name: "Movie".to_string(),
            actions: vec!["stop".to_string()],
            status: HistoryStatus::Failed,
            reason: "ratio 3.0 >= 3.0".to_string(),
            error: Some("command failed".to_string()),
        });
        let skipped: HistoryEntry = serde_json::from_str(
            r#"{
                "id": 9, "at": 1, "rule_id": 2, "rule_name": "r",
                "torrent_hash": "h", "torrent_name": "n",
                "actions": [], "status": "skipped", "reason": "cooldown"
            }"#,
        )
        .unwrap();
        assert_eq!(skipped.status, HistoryStatus::Skipped);
        assert_eq!(skipped.error, None);
    }

    #[test]
    fn dry_run_and_decision_round_trips() {
        round_trip(&DryRunResult {
            matches: vec![DryRunMatch {
                torrent_hash: "aabbccdd".to_string(),
                name: "Show S02E05 1080p".to_string(),
                actions: vec![Action::Stop],
                reason: "ratio 2.0 >= 2.0".to_string(),
            }],
            no_data: vec!["eeff0011".to_string()],
        });
        round_trip(&DryRunResult {
            matches: Vec::new(),
            no_data: Vec::new(),
        });
        round_trip(&Decision {
            rule_id: 0,
            actions: vec![Action::Stop],
            reason: "ratio 2.0 >= 2.0".to_string(),
        });
        round_trip(&RuleTestRequest {
            rule: sample_rule(),
            limit: Some(50),
        });
    }

    #[test]
    fn rules_settings_defaults_and_round_trip() {
        let settings = RulesSettings::default();
        assert!(!settings.seed.enabled);
        assert_eq!(settings.seed.ratio, 0.0);
        assert_eq!(settings.seed.check_interval_secs, 10);
        assert_eq!(settings.seed.action, SeedAction::Stop);
        assert!(settings.rules.enabled);
        assert!(!settings.rules.allow_destructive);
        round_trip(&settings);
        round_trip(&RulesSettings {
            seed: SeedSettings {
                enabled: true,
                ratio: 2.0,
                seeding_time_secs: 7 * 24 * 3600,
                inactive_secs: 1800,
                mode: LimitMode::All,
                action: SeedAction::Throttle("seed".to_string()),
                check_interval_secs: 30,
            },
            rules: RulesFlags {
                enabled: false,
                allow_destructive: true,
            },
        });
        round_trip(&SaveSeedSettingsRequest {
            config: settings,
            save_file: true,
        });
        // Partial documents default field-by-field (TOML shape tolerance).
        let partial: RulesSettings =
            serde_json::from_str(r#"{"seed":{"enabled":true},"rules":{}}"#).unwrap();
        assert!(partial.seed.enabled);
        assert_eq!(partial.seed.check_interval_secs, 10);
        assert!(partial.rules.enabled);
    }

    #[test]
    fn rules_export_round_trip() {
        round_trip(&RulesExport {
            labels: vec![LabelDef {
                name: "movies".to_string(),
                color: None,
                policy: SeedPolicy::default(),
                save_path: None,
            }],
            rules: vec![sample_rule()],
        });
    }

    #[test]
    fn percent_encode_and_paths() {
        assert_eq!(percent_encode("AZaz09-_.~"), "AZaz09-_.~");
        assert_eq!(percent_encode("hello world"), "hello%20world");
        assert_eq!(percent_encode("łódź"), "%C5%82%C3%B3d%C5%BA");
        assert_eq!(label_path("movies"), "/api/labels/movies");
        assert_eq!(label_path("my label"), "/api/labels/my%20label");
        assert_eq!(label_path("a/b"), "/api/labels/a%2Fb");
        assert_eq!(rules_history_path(None), "/api/rules/history");
        assert_eq!(
            rules_history_path(Some(250)),
            "/api/rules/history?limit=250"
        );
    }

    #[test]
    fn rules_tab_default() {
        assert_eq!(RulesTab::default(), RulesTab::Rules);
    }
}
