//! Daemon glue for the torrent rules engine (plan.md §6.3): the `rules.json`
//! store, the label registry, the periodic/event worker and the two-phase
//! action executor. Domain logic (types, inheritance resolution, evaluation,
//! dry-run) lives in the pure `rt-rules` crate; this module bridges it to the
//! session-backed download list, mirroring the `rss.rs` service pattern.
//!
//! Lock discipline (plan §5.4): metrics are collected under short read locks
//! (list snapshot first, then per-torrent), evaluation runs without locks, and
//! actions run without holding the list lock because `d.try_close`/`d.erase`
//! re-enter the list. The download-list event listener only `try_send`s into a
//! bounded channel — it never blocks the emitter.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lt_common::{HashString, Object};
use rt_core::session::SessionManager;
use rt_core::tiering::file_mover::FileMover;
use rt_core::tiering::TieringManager;
use rt_core::{seed_context::SeedContext, Download, DownloadList};
use rt_rpc::parse_commands::parse_command_multiple;
use rt_rpc::{CommandMap, Target};
use rt_rules::compile::{compile, CompiledRule};
use rt_rules::evaluate::{dry_run, evaluate_pass, policy_decision};
use rt_rules::resolve::resolve_limits;
use rt_rules::{
    Action, Condition, Decision, EffectiveLimits, EventKind, LabelDef, Rule, RuleLedger,
    SeedPolicy, TorrentMetrics, TorrentOverrides, POLICY_RULE_ID,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::control::Control;
use crate::rules_store::{HistoryEntry, HistoryStatus, RuleStore, RuleStoreError};

/// Default worker cadence; overridden by `[seed] check_interval_secs`.
const DEFAULT_CHECK_INTERVAL_SECS: u64 = 10;
/// Bounded event channel; a full channel drops events (never blocks emitters).
const EVENT_CHANNEL_CAP: usize = 1024;
/// Timeout for `Action::Execute` child processes.
const EXECUTE_TIMEOUT_SECS: u64 = 60;
/// Webhook request timeout.
const WEBHOOK_TIMEOUT_SECS: u64 = 10;

/// Global seed-policy settings (TOML `[seed]`, runtime shape).
#[derive(Debug, Clone)]
pub struct SeedSettings {
    pub enabled: bool,
    /// Global defaults; `Inherit` at this level resolves to unlimited.
    pub policy: SeedPolicy,
    pub check_interval_secs: u64,
}

impl Default for SeedSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            policy: SeedPolicy::default(),
            check_interval_secs: DEFAULT_CHECK_INTERVAL_SECS,
        }
    }
}

/// Automation-rule settings (TOML `[rules]`, runtime shape).
#[derive(Debug, Clone)]
pub struct RulesSettings {
    pub enabled: bool,
    /// Global destructive gate, ANDed with each rule's `allow_destructive`.
    pub allow_destructive: bool,
}

impl Default for RulesSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_destructive: false,
        }
    }
}

/// A lifecycle event forwarded from the download-list listener.
#[derive(Debug, Clone)]
pub struct EventMsg {
    pub kind: EventKind,
    pub hash: String,
}

/// One torrent snapshot plus the state needed for evaluation and execution.
pub struct CollectedItem {
    pub metrics: TorrentMetrics,
    pub overrides: TorrentOverrides,
    pub ledger: RuleLedger,
    pub download: Arc<RwLock<Download>>,
}

/// Dry-run outcome returned by `rules.test` / `POST /api/rules/test`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DryRunResult {
    pub matches: Vec<DryRunMatch>,
    /// Torrents where a swarm condition could not be evaluated (no scrape).
    pub no_data: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DryRunMatch {
    pub torrent_hash: String,
    pub name: String,
    pub actions: Vec<Action>,
    pub reason: String,
}

pub struct RuleService {
    store: Mutex<RuleStore>,
    store_path: PathBuf,
    compiled: RwLock<Vec<CompiledRule>>,
    seed_settings: RwLock<SeedSettings>,
    rules_settings: RwLock<RulesSettings>,
    /// Shared with the `d.seed_limits` builtin command (installed at startup).
    seed_context: Arc<RwLock<SeedContext>>,
    /// Effective limits at the last policy firing, per torrent. Suppresses
    /// re-firing while limits are unchanged (throttle/notify stay idempotent).
    last_policy: Mutex<HashMap<String, EffectiveLimits>>,
    download_list: Arc<RwLock<DownloadList>>,
    session_manager: Arc<Mutex<SessionManager>>,
    command_map: Arc<RwLock<CommandMap>>,
    tiering_manager: Arc<TieringManager>,
    event_tx: mpsc::Sender<EventMsg>,
    event_rx: Mutex<Option<mpsc::Receiver<EventMsg>>>,
    client: reqwest::Client,
}

impl RuleService {
    pub fn new(
        store_path: PathBuf,
        seed_settings: SeedSettings,
        rules_settings: RulesSettings,
        download_list: Arc<RwLock<DownloadList>>,
        session_manager: Arc<Mutex<SessionManager>>,
        command_map: Arc<RwLock<CommandMap>>,
        tiering_manager: Arc<TieringManager>,
    ) -> Result<Arc<Self>, RuleStoreError> {
        let store = RuleStore::load(&store_path)?;
        let compiled = compile(&store.rules_sorted())?;
        let seed_context = Arc::new(RwLock::new(SeedContext {
            globals: seed_settings.policy.clone(),
            labels: store.labels.clone(),
        }));
        rt_core::seed_context::install_seed_context(Arc::clone(&seed_context));
        rt_core::seed_context::set_seed_policies_active(seed_settings.enabled);

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        // Listener runs in the emitter's context (hash worker / RPC): only a
        // non-blocking try_send, never a lock held across the send.
        {
            let tx = event_tx.clone();
            let mut list = download_list.write().unwrap_or_else(|e| e.into_inner());
            list.add_event_listener(Arc::new(move |event, download| {
                let Some(kind) = EventKind::from_event_name(event) else {
                    return;
                };
                let hash = download
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .info_hash_hex();
                if tx.try_send(EventMsg { kind, hash }).is_err() {
                    tracing::debug!(event, "rules event channel full; dropping event");
                }
            }));
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(WEBHOOK_TIMEOUT_SECS))
            .build()
            .map_err(|e| RuleStoreError::Io(format!("webhook client: {e}")))?;

        Ok(Arc::new(Self {
            store: Mutex::new(store),
            store_path,
            compiled: RwLock::new(compiled),
            seed_settings: RwLock::new(seed_settings),
            rules_settings: RwLock::new(rules_settings),
            seed_context,
            last_policy: Mutex::new(HashMap::new()),
            download_list,
            session_manager,
            command_map,
            tiering_manager,
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
            client,
        }))
    }

    /// Sender clone for wiring tests or extra producers.
    pub fn event_sender(&self) -> mpsc::Sender<EventMsg> {
        self.event_tx.clone()
    }

    fn persist(&self) -> Result<(), RuleStoreError> {
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        store.save(&self.store_path)
    }

    fn recompile(&self) -> Result<(), RuleStoreError> {
        let rules = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rules_sorted();
        let compiled = compile(&rules)?;
        *self.compiled.write().unwrap_or_else(|e| e.into_inner()) = compiled;
        Ok(())
    }

    fn refresh_seed_context(&self) {
        let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let settings = self.seed_settings.read().unwrap_or_else(|e| e.into_inner());
        let mut ctx = self.seed_context.write().unwrap_or_else(|e| e.into_inner());
        ctx.globals = settings.policy.clone();
        ctx.labels = store.labels.clone();
    }

    // ---- Settings ----

    pub fn seed_settings(&self) -> SeedSettings {
        self.seed_settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_seed_settings(&self, settings: SeedSettings) {
        rt_core::seed_context::set_seed_policies_active(settings.enabled);
        *self
            .seed_settings
            .write()
            .unwrap_or_else(|e| e.into_inner()) = settings;
        self.refresh_seed_context();
    }

    pub fn rules_settings(&self) -> RulesSettings {
        self.rules_settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_rules_settings(&self, settings: RulesSettings) {
        *self
            .rules_settings
            .write()
            .unwrap_or_else(|e| e.into_inner()) = settings;
    }

    // ---- Rule CRUD ----

    pub fn list_rules(&self) -> Vec<Rule> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rules_sorted()
    }

    pub fn get_rule(&self, id: u64) -> Option<Rule> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rule(id)
            .cloned()
    }

    pub fn add_rule(&self, rule: Rule) -> Result<Rule, RuleStoreError> {
        compile(std::slice::from_ref(&rule))?;
        let inserted = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert_rule(rule);
        self.recompile()?;
        self.persist()?;
        Ok(inserted)
    }

    pub fn update_rule(&self, rule: Rule) -> Result<(), RuleStoreError> {
        compile(std::slice::from_ref(&rule))?;
        let updated = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update_rule(rule);
        if !updated {
            return Err(rt_rules::RuleError::UnknownRule(0).into());
        }
        self.recompile()?;
        self.persist()
    }

    pub fn remove_rule(&self, id: u64) -> Result<(), RuleStoreError> {
        let removed = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove_rule(id);
        if !removed {
            return Err(rt_rules::RuleError::UnknownRule(id).into());
        }
        self.recompile()?;
        self.persist()
    }

    pub fn set_rule_enabled(&self, id: u64, enabled: bool) -> Result<(), RuleStoreError> {
        {
            let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            let rule = store
                .rules
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or(rt_rules::RuleError::UnknownRule(id))?;
            rule.enabled = enabled;
        }
        self.recompile()?;
        self.persist()
    }

    // ---- Labels ----

    pub fn list_labels(&self) -> Vec<LabelDef> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .labels
            .clone()
    }

    pub fn add_label(&self, label: LabelDef) -> Result<LabelDef, RuleStoreError> {
        let inserted = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert_label(label)?;
        self.refresh_seed_context();
        self.persist()?;
        Ok(inserted)
    }

    pub fn remove_label(&self, name: &str) -> Result<(), RuleStoreError> {
        let removed = self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove_label(name);
        if !removed {
            return Err(rt_rules::RuleError::UnknownLabel(name.to_string()).into());
        }
        self.refresh_seed_context();
        self.persist()
    }

    pub fn update_label(&self, label: LabelDef) -> Result<(), RuleStoreError> {
        {
            let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            let slot = store
                .label_mut(&label.name)
                .ok_or_else(|| rt_rules::RuleError::UnknownLabel(label.name.clone()))?;
            *slot = label;
        }
        self.refresh_seed_context();
        self.persist()
    }

    // ---- History / export / import ----

    pub fn history(&self, limit: usize) -> Vec<HistoryEntry> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .history_newest(limit)
    }

    pub fn clear_history(&self) -> Result<(), RuleStoreError> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear_history();
        self.persist()
    }

    pub fn export(&self) -> crate::rules_store::ExportBlob {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .export()
    }

    pub fn import(&self, blob: crate::rules_store::ExportBlob) -> Result<(), RuleStoreError> {
        compile(&blob.rules)?;
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .import(blob)?;
        self.recompile()?;
        self.refresh_seed_context();
        self.persist()
    }

    // ---- Per-torrent limits ----

    pub fn effective_limits(&self, hash: &str) -> Option<EffectiveLimits> {
        let download = self
            .download_list
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .find_hex(hash)?;
        let guard = download.read().unwrap_or_else(|e| e.into_inner());
        Some(self.resolve_for(&guard))
    }

    pub async fn set_torrent_overrides(
        &self,
        hash: &str,
        overrides: TorrentOverrides,
    ) -> Result<(), RuleStoreError> {
        let download = self
            .download_list
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .find_hex(hash)
            .ok_or_else(|| rt_rules::RuleError::Invalid(format!("unknown torrent {hash}")))?;
        {
            let mut guard = download.write().unwrap_or_else(|e| e.into_inner());
            *guard.seed_overrides_mut() = overrides;
        }
        // Persist promptly: overrides are user intent, not telemetry.
        let session = Arc::clone(&self.session_manager);
        let list = Arc::clone(&self.download_list);
        let _ = tokio::task::spawn_blocking(move || {
            Control::save_session_state_with_abort(session, list, None)
        })
        .await;
        Ok(())
    }

    fn resolve_for(&self, guard: &Download) -> EffectiveLimits {
        let settings = self.seed_settings.read().unwrap_or_else(|e| e.into_inner());
        let ctx = self.seed_context.read().unwrap_or_else(|e| e.into_inner());
        let label_name = guard.custom_field(1).trim().to_string();
        let label = ctx
            .labels
            .iter()
            .find(|l| l.name == label_name)
            .map(|l| (l.name.as_str(), &l.policy));
        resolve_limits(&settings.policy, label, guard.seed_overrides())
    }

    // ---- Dry-run ----

    pub fn test_rule(&self, rule: &Rule) -> Result<DryRunResult, RuleStoreError> {
        let compiled = compile(std::slice::from_ref(rule))?;
        let items = self.collect_items(None);
        let now = unix_secs();
        let ledgers: HashMap<String, RuleLedger> = items
            .iter()
            .map(|i| (i.metrics.hash.clone(), i.ledger.clone()))
            .collect();
        let metrics: Vec<TorrentMetrics> = items.iter().map(|i| i.metrics.clone()).collect();
        let matched = dry_run(&compiled, &metrics, now, &ledgers);
        let has_swarm = rule.conditions.iter().any(|c| {
            matches!(
                c,
                Condition::SeedsAtLeast(_)
                    | Condition::SeedsAtMost(_)
                    | Condition::LeechersAtLeast(_)
            )
        });
        let no_data = if has_swarm {
            metrics
                .iter()
                .filter(|m| m.seeds.is_none() || m.leechers.is_none())
                .map(|m| m.hash.clone())
                .collect()
        } else {
            Vec::new()
        };
        Ok(DryRunResult {
            matches: matched
                .into_iter()
                .map(|(m, d)| DryRunMatch {
                    torrent_hash: m.hash,
                    name: m.name,
                    actions: d.actions,
                    reason: d.reason,
                })
                .collect(),
            no_data,
        })
    }

    // ---- Metrics collection ----

    /// Snapshot all torrents (or one by hex hash). Phase A clones the Arc list
    /// under the list read lock; phase B locks each torrent briefly. No lock is
    /// held while calling into the command map (`d.ignore_commands`).
    pub fn collect_items(&self, only_hash: Option<&str>) -> Vec<CollectedItem> {
        let arcs: Vec<Arc<RwLock<Download>>> = {
            let list = self.download_list.read().unwrap_or_else(|e| e.into_inner());
            match only_hash {
                Some(hash) => list.find_hex(hash).into_iter().collect(),
                None => list.all(),
            }
        };
        let now = unix_secs();
        let mut free_cache: HashMap<String, Option<u64>> = HashMap::new();
        let rule_paths = self.free_space_rule_paths();
        let mut items = Vec::with_capacity(arcs.len());
        for arc in arcs {
            let ignore = self.query_ignore_commands(&arc);
            let guard = arc.read().unwrap_or_else(|e| e.into_inner());
            let metrics = self.build_metrics(&guard, now, ignore, &mut free_cache, &rule_paths);
            items.push(CollectedItem {
                metrics,
                overrides: guard.seed_overrides().clone(),
                ledger: guard.rule_ledger().clone(),
                download: Arc::clone(&arc),
            });
        }
        items
    }

    fn query_ignore_commands(&self, arc: &Arc<RwLock<Download>>) -> bool {
        let hash = arc
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .info_hash_hex();
        let Some(parsed) = HashString::from_hex(&hash) else {
            return false;
        };
        let map = self.command_map.read().unwrap_or_else(|e| e.into_inner());
        map.call_command(
            "d.ignore_commands",
            &Object::create_empty(),
            &Target::Download(parsed),
            true,
        )
        .ok()
        .and_then(|obj| obj.as_value().ok())
        .map(|v| v != 0)
        .unwrap_or(false)
    }

    /// Distinct `Some(path)` values from `FreeSpaceBelow` conditions across all
    /// compiled rules — statvfs'd once per pass.
    fn free_space_rule_paths(&self) -> Vec<String> {
        let compiled = self.compiled.read().unwrap_or_else(|e| e.into_inner());
        let mut paths: Vec<String> = Vec::new();
        for rule in compiled.iter() {
            for cond in &rule.rule.conditions {
                if let Condition::FreeSpaceBelow { path: Some(p), .. } = cond {
                    if !paths.contains(p) {
                        paths.push(p.clone());
                    }
                }
            }
        }
        paths
    }

    fn build_metrics(
        &self,
        guard: &Download,
        now: u64,
        ignore_commands: bool,
        free_cache: &mut HashMap<String, Option<u64>>,
        rule_paths: &[String],
    ) -> TorrentMetrics {
        let uploaded = guard.uploaded_bytes();
        let downloaded = guard.downloaded_bytes();
        let completed = guard.completed_bytes();
        let total_size = guard.metadata().total_size;
        let is_complete = guard.is_done();
        let base = if completed > 0 {
            completed
        } else if downloaded > 0 {
            downloaded
        } else if is_complete {
            total_size
        } else {
            0
        };
        let ratio = if base > 0 {
            uploaded as f64 / base as f64
        } else {
            0.0
        };

        // Swarm health from tracker scrape; None when no tracker ever scraped.
        let mut tracker_hosts = Vec::new();
        let mut seeds: Option<u32> = None;
        let mut leechers: Option<u32> = None;
        {
            let engine = guard.download();
            for tracker in &engine.tracker_controller().tracker_list.trackers {
                if let Some(host) = url_host(tracker.url()) {
                    if !tracker_hosts.contains(&host) {
                        tracker_hosts.push(host);
                    }
                }
                let state = tracker.state();
                if state.counters.scrape_time_last > 0 {
                    seeds = Some(seeds.unwrap_or(0).max(state.scrape_complete));
                    leechers = Some(leechers.unwrap_or(0).max(state.scrape_incomplete));
                }
            }
        }

        let rss = guard
            .custom_get("rss.feed_id")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|feed_id| rt_rules::RssProvenance {
                feed_id,
                rule_id: guard.custom_get("rss.rule_id").and_then(|v| v.parse().ok()),
                item_id: guard
                    .custom_get("rss.item_id")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            });

        let dir = guard.directory().to_string();
        let dir_free = *free_cache
            .entry(dir.clone())
            .or_insert_with(|| FileMover::available_space(&dir).ok());
        let mut path_free = HashMap::new();
        for p in rule_paths {
            let free = *free_cache
                .entry(p.clone())
                .or_insert_with(|| FileMover::available_space(p).ok());
            if let Some(free) = free {
                path_free.insert(p.clone(), free);
            }
        }

        let label = guard.custom_field(1).trim().to_string();
        TorrentMetrics {
            hash: guard.info_hash_hex(),
            name: guard.name().to_string(),
            is_complete,
            is_active: guard.is_active(),
            is_seeding: guard.is_seeding(),
            is_stopped: guard.is_stopped(),
            uploaded,
            downloaded,
            completed,
            total_size,
            ratio,
            seeding_time_secs: guard.seeding_time(),
            idle_secs: guard.idle_secs(now),
            age_secs: now.saturating_sub(guard.load_date()),
            label: if label.is_empty() { None } else { Some(label) },
            tags: guard.tags().to_vec(),
            tracker_hosts,
            seeds,
            leechers,
            rss,
            tier: guard.tier_state().current_tier,
            ignore_commands,
            dir_free_bytes: dir_free,
            path_free_bytes: path_free,
        }
    }

    // ---- Evaluation ----

    /// Decide for every collected item. Periodic pass (`event == None`):
    /// seed policies first (layer A), then periodic rules (layer B). Event
    /// pass: only rules with the matching event trigger. One decision per
    /// item; the policy wins over rules.
    fn decide(
        &self,
        items: &[CollectedItem],
        event: Option<EventKind>,
        now: u64,
    ) -> Vec<(usize, Decision)> {
        let seed = self.seed_settings();
        let rules_enabled = self.rules_settings().enabled;
        let compiled = self.compiled.read().unwrap_or_else(|e| e.into_inner());
        let last_policy = self.last_policy.lock().unwrap_or_else(|e| e.into_inner());
        let mut decisions = Vec::new();
        for (idx, item) in items.iter().enumerate() {
            let m = &item.metrics;
            if m.ignore_commands {
                continue;
            }
            if event.is_none() && seed.enabled && m.is_seeding {
                let limits = self.resolve_limits_for(item);
                let suppressed = last_policy.get(&m.hash) == Some(&limits);
                if !suppressed {
                    if let Some(decision) = policy_decision(&limits, m) {
                        decisions.push((idx, decision));
                        continue;
                    }
                }
            }
            if rules_enabled {
                if let Some(decision) = evaluate_pass(&compiled, m, now, &item.ledger, event) {
                    decisions.push((idx, decision));
                }
            }
        }
        decisions
    }

    fn resolve_limits_for(&self, item: &CollectedItem) -> EffectiveLimits {
        let settings = self.seed_settings.read().unwrap_or_else(|e| e.into_inner());
        let ctx = self.seed_context.read().unwrap_or_else(|e| e.into_inner());
        let label = item.metrics.label.as_deref().and_then(|name| {
            ctx.labels
                .iter()
                .find(|l| l.name == name)
                .map(|l| (l.name.as_str(), &l.policy))
        });
        resolve_limits(&settings.policy, label, &item.overrides)
    }

    /// Force one evaluation pass now (`rules.check`); returns audit entries.
    pub async fn check_now(&self) -> Vec<HistoryEntry> {
        self.run_pass(None).await
    }

    /// One worker pass: collect → decide → execute → audit. Returns the audit
    /// entries produced (empty when nothing fired).
    pub async fn run_pass(&self, event: Option<EventMsg>) -> Vec<HistoryEntry> {
        let seed_enabled = self.seed_settings().enabled;
        let rules_enabled = self.rules_settings().enabled;
        if !seed_enabled && !rules_enabled {
            return Vec::new();
        }
        let items = self.collect_items(event.as_ref().map(|e| e.hash.as_str()));
        if items.is_empty() {
            return Vec::new();
        }
        let now = unix_secs();
        let decisions = self.decide(&items, event.as_ref().map(|e| e.kind), now);
        let mut entries = Vec::new();
        for (idx, decision) in decisions {
            let item = &items[idx];
            let (status, error) = self.execute_decision(item, &decision, now).await;
            let entry = self.audit(item, &decision, status, error, now);
            entries.push(entry);
        }
        if !entries.is_empty() {
            // Persist rule ledger / state changes through the normal writer
            // snapshot path (cached: unchanged entries are skipped).
            let session = Arc::clone(&self.session_manager);
            let list = Arc::clone(&self.download_list);
            let _ = tokio::task::spawn_blocking(move || {
                Control::save_session_state_with_abort(session, list, None)
            })
            .await;
        }
        entries
    }

    // ---- Execution ----

    async fn execute_decision(
        &self,
        item: &CollectedItem,
        decision: &Decision,
        now: u64,
    ) -> (HistoryStatus, Option<String>) {
        // Destructive gate: global flag AND (for user rules) the rule flag.
        let destructive = decision
            .actions
            .iter()
            .any(|a| matches!(a, Action::Remove | Action::RemoveWithData));
        if destructive && !self.destructive_allowed(decision.rule_id) {
            return (
                HistoryStatus::Skipped,
                Some("destructive actions are gated (allow_destructive)".into()),
            );
        }

        let mut status = HistoryStatus::Ok;
        let mut error = None;
        for action in &decision.actions {
            if matches!(action, Action::Noop) {
                break;
            }
            if let Err(err) = self.execute_action(item, action).await {
                status = HistoryStatus::Failed;
                error = Some(err);
                break;
            }
        }

        if status == HistoryStatus::Ok {
            // Record the firing: user rules in the persisted ledger, policies
            // in the in-memory suppression map (idempotent across restarts).
            if decision.rule_id == POLICY_RULE_ID {
                let limits = self.resolve_limits_for(item);
                self.last_policy
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(item.metrics.hash.clone(), limits);
            } else {
                let mut guard = item.download.write().unwrap_or_else(|e| e.into_inner());
                guard.rule_ledger_mut().0.insert(decision.rule_id, now);
            }
        }
        (status, error)
    }

    fn destructive_allowed(&self, rule_id: u64) -> bool {
        if !self.rules_settings().allow_destructive {
            return false;
        }
        if rule_id == POLICY_RULE_ID {
            return true;
        }
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rule(rule_id)
            .map(|r| r.allow_destructive)
            .unwrap_or(false)
    }

    async fn execute_action(&self, item: &CollectedItem, action: &Action) -> Result<(), String> {
        let arc = &item.download;
        let hash = item.metrics.hash.clone();
        match action {
            Action::Noop => Ok(()),
            Action::Stop => {
                let list = self.download_list.read().unwrap_or_else(|e| e.into_inner());
                list.stop(arc);
                Ok(())
            }
            Action::Pause => {
                let list = self.download_list.read().unwrap_or_else(|e| e.into_inner());
                list.pause(arc);
                Ok(())
            }
            Action::Start => {
                let list = self.download_list.read().unwrap_or_else(|e| e.into_inner());
                list.start(arc);
                Ok(())
            }
            Action::Close => {
                {
                    let list = self.download_list.read().unwrap_or_else(|e| e.into_inner());
                    list.close(arc);
                }
                self.run_command(&hash, "d.ignore_commands.set=1")
            }
            Action::Remove => {
                self.erase(&hash);
                Ok(())
            }
            Action::RemoveWithData => {
                {
                    let list = self.download_list.read().unwrap_or_else(|e| e.into_inner());
                    list.stop(arc);
                }
                self.run_command(&hash, "d.delete_data=")?;
                self.erase(&hash);
                Ok(())
            }
            Action::MoveToDir(dir) => self.live_move(arc, PathBuf::from(dir)).await,
            Action::MoveToTier(tier) => {
                let path = {
                    let config = self.tiering_manager.config();
                    let config = config.read().unwrap_or_else(|e| e.into_inner());
                    config
                        .tiers
                        .get(*tier as usize)
                        .map(|t| t.path.clone())
                        .ok_or_else(|| format!("unknown tier {tier}"))?
                };
                self.live_move(arc, path).await
            }
            Action::SetLabel(label) => {
                let mut guard = arc.write().unwrap_or_else(|e| e.into_inner());
                guard.set_custom_field(1, label.as_str());
                Ok(())
            }
            Action::AddTag(tag) => {
                let mut guard = arc.write().unwrap_or_else(|e| e.into_inner());
                guard.add_tag(tag);
                Ok(())
            }
            Action::RemoveTag(tag) => {
                let mut guard = arc.write().unwrap_or_else(|e| e.into_inner());
                guard.remove_tag(tag);
                Ok(())
            }
            Action::SetThrottle(name) => {
                let mut guard = arc.write().unwrap_or_else(|e| e.into_inner());
                guard.set_throttle_name(name.clone().unwrap_or_default());
                Ok(())
            }
            Action::SetPriority(priority) => {
                let mut guard = arc.write().unwrap_or_else(|e| e.into_inner());
                guard.set_priority(u32::from(*priority).min(3));
                Ok(())
            }
            Action::SuperSeed(enable) => {
                let mut guard = arc.write().unwrap_or_else(|e| e.into_inner());
                guard
                    .set_connection_seed(if *enable { "initial_seed" } else { "seed" })
                    .map_err(|e| e.to_string())
            }
            Action::Execute(argv) => self.execute_process(argv).await,
            Action::Webhook { url } => self.webhook(url, item).await,
            Action::Command(cmd) => self.run_command(&hash, cmd),
        }
    }

    fn erase(&self, hash: &str) {
        if let Some(parsed) = HashString::from_hex(hash) {
            let mut list = self
                .download_list
                .write()
                .unwrap_or_else(|e| e.into_inner());
            list.erase(&parsed);
        }
    }

    fn run_command(&self, hash: &str, cmd: &str) -> Result<(), String> {
        if cmd.trim().is_empty() {
            return Ok(());
        }
        let parsed =
            HashString::from_hex(hash).ok_or_else(|| format!("invalid info hash {hash}"))?;
        let target = Target::Download(parsed);
        let map = self.command_map.read().unwrap_or_else(|e| e.into_inner());
        parse_command_multiple(cmd, &target, &map, true)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn live_move(&self, arc: &Arc<RwLock<Download>>, path: PathBuf) -> Result<(), String> {
        let arc = Arc::clone(arc);
        tokio::task::spawn_blocking(move || {
            let mut guard = arc.write().unwrap_or_else(|e| e.into_inner());
            TieringManager::live_move(&mut guard, &path).map(|_| ())
        })
        .await
        .map_err(|e| format!("move task: {e}"))?
        .map_err(|e| e.to_string())
    }

    async fn execute_process(&self, argv: &[String]) -> Result<(), String> {
        let argv = argv.to_vec();
        tokio::task::spawn_blocking(move || {
            let Some((program, args)) = argv.split_first() else {
                return Err("empty execute argv".to_string());
            };
            let mut child = std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| format!("spawn {program}: {e}"))?;
            let deadline = std::time::Instant::now() + Duration::from_secs(EXECUTE_TIMEOUT_SECS);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) if status.success() => return Ok(()),
                    Ok(Some(status)) => {
                        return Err(format!("{program} exited with {status}"));
                    }
                    Ok(None) => {
                        if std::time::Instant::now() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(format!(
                                "{program} timed out after {EXECUTE_TIMEOUT_SECS}s"
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(e) => return Err(format!("wait {program}: {e}")),
                }
            }
        })
        .await
        .map_err(|e| format!("execute task: {e}"))?
    }

    async fn webhook(&self, url: &str, item: &CollectedItem) -> Result<(), String> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(format!("webhook URL must be http/https: {url}"));
        }
        let payload = serde_json::json!({
            "event": "rule.fired",
            "hash": item.metrics.hash,
            "name": item.metrics.name,
            "label": item.metrics.label,
            "at": unix_secs(),
        });
        let response = self
            .client
            .post(url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("webhook: {e}"))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("webhook status {}", response.status()))
        }
    }

    // ---- Audit ----

    fn audit(
        &self,
        item: &CollectedItem,
        decision: &Decision,
        status: HistoryStatus,
        error: Option<String>,
        now: u64,
    ) -> HistoryEntry {
        let rule_name = if decision.rule_id == POLICY_RULE_ID {
            "seed-policy".to_string()
        } else {
            self.get_rule(decision.rule_id)
                .map(|r| r.name)
                .unwrap_or_else(|| format!("rule#{}", decision.rule_id))
        };
        let actions: Vec<String> = decision
            .actions
            .iter()
            .map(|a| {
                serde_json::to_value(a)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_else(|| format!("{a:?}"))
            })
            .collect();
        let entry = HistoryEntry {
            id: 0,
            at: now,
            rule_id: decision.rule_id,
            rule_name,
            torrent_hash: item.metrics.hash.clone(),
            torrent_name: item.metrics.name.clone(),
            actions,
            status,
            reason: decision.reason.clone(),
            error,
        };
        {
            let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store.insert_history(entry.clone());
        }
        if let Err(err) = self.persist() {
            tracing::warn!(error = %err, "could not persist rules history");
        }
        let event_name = if decision.rule_id == POLICY_RULE_ID {
            "event.seed_limit.reached"
        } else {
            "event.rule.fired"
        };
        {
            let list = self.download_list.read().unwrap_or_else(|e| e.into_inner());
            list.emit_event(event_name, &item.download);
        }
        tracing::info!(
            rule_id = decision.rule_id,
            hash = %item.metrics.hash,
            status = ?status,
            reason = %decision.reason,
            "rules engine decision"
        );
        entry
    }
}

/// Extracts the lowercase host from a tracker URL (no scheme/port/path).
fn url_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .or_else(|| url.strip_prefix("udp://"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(rest.split(['/', '?', '#']).next().unwrap_or(""));
    let host = host.trim().to_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Spawns the rules worker: periodic ticks plus immediate event-driven passes.
/// Exits when the shutdown watch fires (or its sender is dropped).
pub fn spawn_worker(
    service: Arc<RuleService>,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    let mut event_rx = service
        .event_rx
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .expect("rules worker spawned twice");
    tokio::spawn(async move {
        if *shutdown.borrow_and_update() {
            return;
        }
        let mut interval_secs = service.seed_settings().check_interval_secs.max(1);
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tracing::info!(interval_secs, "rules worker started");
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let entries = service.run_pass(None).await;
                    if !entries.is_empty() {
                        tracing::debug!(fired = entries.len(), "rules periodic pass");
                    }
                    let current = service.seed_settings().check_interval_secs.max(1);
                    if current != interval_secs {
                        interval_secs = current;
                        interval = tokio::time::interval(Duration::from_secs(interval_secs));
                        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    }
                }
                msg = event_rx.recv() => {
                    match msg {
                        Some(msg) => {
                            let entries = service.run_pass(Some(msg)).await;
                            if !entries.is_empty() {
                                tracing::debug!(fired = entries.len(), "rules event pass");
                            }
                        }
                        None => break,
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow_and_update() {
                        break;
                    }
                }
            }
        }
        tracing::info!("rules worker stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rt_rules::{CondMode, Condition, LimitValue, Rule, Scope, Trigger};

    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "rt_rules_service_unit_{name}_{}",
                std::process::id()
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

    fn service(dir: &TempDir) -> Arc<RuleService> {
        RuleService::new(
            dir.path.join("rules.json"),
            SeedSettings::default(),
            RulesSettings::default(),
            Arc::new(RwLock::new(DownloadList::new())),
            Arc::new(Mutex::new(SessionManager::new())),
            Arc::new(RwLock::new(CommandMap::new())),
            Arc::new(TieringManager::new(Default::default())),
        )
        .unwrap()
    }

    fn rule(name: &str, conditions: Vec<Condition>, actions: Vec<Action>) -> Rule {
        Rule {
            id: 0,
            name: name.into(),
            enabled: true,
            priority: 0,
            scope: Scope::Global,
            trigger: Trigger::Periodic,
            cond_mode: CondMode::All,
            conditions,
            actions,
            once: false,
            cooldown_secs: None,
            allow_destructive: false,
        }
    }

    fn item(metrics: TorrentMetrics) -> CollectedItem {
        let mut factory = rt_core::DownloadFactory::new();
        factory
            .load_magnet(&format!("magnet:?xt=urn:btih:{}&dn=fixture", metrics.hash))
            .expect("fixture magnet");
        let download = factory.create_download().expect("fixture download");
        CollectedItem {
            metrics,
            overrides: TorrentOverrides::default(),
            ledger: RuleLedger::default(),
            download: Arc::new(RwLock::new(download)),
        }
    }

    fn seeding_metrics(hash: &str) -> TorrentMetrics {
        TorrentMetrics {
            hash: hash.into(),
            name: "fixture".into(),
            is_complete: true,
            is_active: true,
            is_seeding: true,
            is_stopped: false,
            uploaded: 300,
            downloaded: 100,
            completed: 100,
            total_size: 100,
            ratio: 3.0,
            seeding_time_secs: 3600,
            idle_secs: 0,
            age_secs: 7200,
            label: None,
            tags: vec![],
            tracker_hosts: vec![],
            seeds: None,
            leechers: None,
            rss: None,
            tier: None,
            ignore_commands: false,
            dir_free_bytes: Some(1 << 30),
            path_free_bytes: HashMap::new(),
        }
    }

    #[test]
    fn url_host_parsing() {
        assert_eq!(
            url_host("https://Tracker.Example.org:443/announce"),
            Some("tracker.example.org".into())
        );
        assert_eq!(
            url_host("udp://t.example.org:6969"),
            Some("t.example.org".into())
        );
        assert_eq!(
            url_host("http://example.org/announce?x=1"),
            Some("example.org".into())
        );
        assert_eq!(url_host("ftp://example.org"), None);
        assert_eq!(url_host("https://"), None);
    }

    #[test]
    fn rule_crud_validates_and_persists() {
        let dir = TempDir::new("crud");
        let svc = service(&dir);
        let bad = rule("bad", vec![], vec![Action::Stop]);
        assert!(svc.add_rule(bad).is_err());
        let good = rule(
            "good",
            vec![Condition::RatioAtLeast(2.0)],
            vec![Action::Stop],
        );
        let inserted = svc.add_rule(good).unwrap();
        assert_eq!(inserted.id, 1);
        assert_eq!(svc.list_rules().len(), 1);
        assert!(svc.set_rule_enabled(1, false).is_ok());
        assert!(!svc.get_rule(1).unwrap().enabled);
        assert!(svc.remove_rule(1).is_ok());
        assert!(svc.remove_rule(1).is_err());
        // Reload from disk: empty again.
        let reloaded = RuleStore::load(&dir.path.join("rules.json")).unwrap();
        assert!(reloaded.rules.is_empty());
    }

    #[test]
    fn label_registry_updates_seed_context() {
        let dir = TempDir::new("labels");
        let svc = service(&dir);
        svc.add_label(LabelDef {
            name: "movies".into(),
            color: None,
            policy: SeedPolicy {
                ratio: LimitValue::Set(3.0),
                ..Default::default()
            },
            save_path: None,
        })
        .unwrap();
        let ctx = svc.seed_context.read().unwrap();
        assert_eq!(ctx.labels.len(), 1);
        assert_eq!(ctx.labels[0].policy.ratio, LimitValue::Set(3.0));
    }

    #[tokio::test]
    async fn dry_run_is_pure_and_reports_no_data() {
        let dir = TempDir::new("dryrun");
        let svc = service(&dir);
        let r = rule(
            "swarm",
            vec![Condition::SeedsAtLeast(5)],
            vec![Action::Stop],
        );
        // No torrents registered: empty result, but the store must be untouched.
        let before = svc.export();
        let result = svc.test_rule(&r).unwrap();
        assert!(result.matches.is_empty());
        assert_eq!(svc.export(), before);
        assert!(svc.history(10).is_empty());
    }

    #[test]
    fn decide_policy_fires_once_per_limits() {
        let dir = TempDir::new("policy");
        let svc = service(&dir);
        svc.set_seed_settings(SeedSettings {
            enabled: true,
            policy: SeedPolicy {
                ratio: LimitValue::Set(2.0),
                ..Default::default()
            },
            check_interval_secs: 10,
        });
        let items = [item(seeding_metrics(HASH))];
        let decisions = svc.decide(&items, None, unix_secs());
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].1.rule_id, POLICY_RULE_ID);
        assert_eq!(decisions[0].1.actions, vec![Action::Stop]);
        // Simulate the firing being recorded.
        svc.last_policy
            .lock()
            .unwrap()
            .insert(HASH.into(), svc.resolve_limits_for(&items[0]));
        assert!(svc.decide(&items, None, unix_secs()).is_empty());
        // Changing the limit re-enables evaluation.
        svc.set_seed_settings(SeedSettings {
            enabled: true,
            policy: SeedPolicy {
                ratio: LimitValue::Set(5.0),
                ..Default::default()
            },
            check_interval_secs: 10,
        });
        // ratio 3.0 < 5.0 → no decision, but not suppressed either.
        assert!(svc.decide(&items, None, unix_secs()).is_empty());
    }

    #[test]
    fn decide_respects_ignore_commands_and_event_triggers() {
        let dir = TempDir::new("triggers");
        let svc = service(&dir);
        svc.add_rule(rule(
            "periodic",
            vec![Condition::Complete(true)],
            vec![Action::AddTag("x".into())],
        ))
        .unwrap();
        let mut event_rule = rule(
            "on_finished",
            vec![Condition::Complete(true)],
            vec![Action::Pause],
        );
        event_rule.trigger = Trigger::Event(EventKind::Finished);
        svc.add_rule(event_rule).unwrap();

        let mut m = seeding_metrics(HASH);
        m.is_seeding = false;
        let items = vec![item(m)];
        // Periodic pass: only the periodic rule.
        let periodic = svc.decide(&items, None, unix_secs());
        assert_eq!(periodic.len(), 1);
        assert_eq!(periodic[0].1.actions, vec![Action::AddTag("x".into())]);
        // Event pass: only the event rule.
        let event = svc.decide(&items, Some(EventKind::Finished), unix_secs());
        assert_eq!(event.len(), 1);
        assert_eq!(event[0].1.actions, vec![Action::Pause]);
        // ignore_commands disables everything.
        let mut ignored = seeding_metrics(HASH);
        ignored.is_seeding = false;
        ignored.ignore_commands = true;
        let items = vec![item(ignored)];
        assert!(svc.decide(&items, None, unix_secs()).is_empty());
    }

    #[tokio::test]
    async fn destructive_gate_skips_and_audits() {
        let dir = TempDir::new("gate");
        let svc = service(&dir);
        let mut r = rule(
            "cleanup",
            vec![Condition::RatioAtLeast(1.0)],
            vec![Action::RemoveWithData],
        );
        r.allow_destructive = true; // rule allows, global gate does not
        let inserted = svc.add_rule(r).unwrap();
        let items = [item(seeding_metrics(HASH))];
        let decision = Decision {
            rule_id: inserted.id,
            actions: vec![Action::RemoveWithData],
            reason: "test".into(),
        };
        let (status, error) = svc
            .execute_decision(&items[0], &decision, unix_secs())
            .await;
        assert_eq!(status, HistoryStatus::Skipped);
        assert!(error.is_some());
        svc.audit(&items[0], &decision, status, error, unix_secs());
        let history = svc.history(10);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, HistoryStatus::Skipped);
    }

    #[tokio::test]
    async fn noop_exception_stops_execution() {
        let dir = TempDir::new("noop");
        let svc = service(&dir);
        let items = [item(seeding_metrics(HASH))];
        let decision = Decision {
            rule_id: 1,
            actions: vec![Action::Noop, Action::AddTag("x".into())],
            reason: "exception".into(),
        };
        let (status, _) = svc
            .execute_decision(&items[0], &decision, unix_secs())
            .await;
        assert_eq!(status, HistoryStatus::Ok);
        // AddTag after Noop never ran.
        let guard = items[0].download.read().unwrap();
        assert!(!guard.has_tag("x"));
    }
}
