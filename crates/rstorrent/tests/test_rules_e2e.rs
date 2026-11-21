//! End-to-end rules engine tests with a real seeding torrent: policy
//! enforcement, rule firing, once-ledger, event triggers, dry-run purity and
//! the destructive gate (plan §9, AC-003/006/007/008).

use std::sync::{Arc, Mutex, RwLock};

use lt_common::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::TorrentMetadata;
use rstorrent::rules::{EventMsg, RuleService, RulesSettings, SeedSettings};
use rstorrent::rules_store::HistoryStatus;
use rt_core::session::SessionManager;
use rt_core::{Download, DownloadList};
use rt_rpc::CommandMap;
use rt_rules::{Action, Condition, EventKind, LimitValue, Rule, Scope, SeedPolicy, Trigger};

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rt_rules_e2e_{name}_{}", std::process::id()));
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

/// Real single-piece seeding torrent (network_runtime test fixture pattern):
/// the only piece has arrived, transfer counters are set explicitly.
fn seeding_fixture(
    dir: &TempDir,
    uploaded: u64,
    downloaded: u64,
) -> (Arc<RwLock<DownloadList>>, Arc<RwLock<Download>>, String) {
    let payload = b"rules-e2e-payload";
    let hash = sha1_hash(payload);
    let meta = TorrentMetadata {
        info_hash: HashString::new(hash),
        name: "rules_e2e".into(),
        piece_length: payload.len() as u32,
        pieces: hash.to_vec(),
        num_pieces: 1,
        total_size: payload.len() as u64,
        is_multi_file: false,
        files: Vec::new(),
        announce: None,
        announce_list: vec![],
        is_private: false,
        is_meta_download: false,
        v2: None,
    };
    let mut dl = Download::from_metadata(meta);
    {
        let mut engine = dl.download_mut();
        engine.start();
        assert!(engine.receive_block(0, 0, payload).unwrap());
    }
    dl.set_downloaded_bytes(downloaded);
    dl.set_uploaded_bytes(uploaded);
    dl.set_directory(dir.path.to_str().unwrap());
    assert!(dl.is_seeding());
    let hash_hex = dl.info_hash_hex();
    let item = Arc::new(RwLock::new(dl));
    let list = Arc::new(RwLock::new(DownloadList::new()));
    list.write().unwrap().insert(Arc::clone(&item));
    (list, item, hash_hex)
}

fn service(dir: &TempDir, list: Arc<RwLock<DownloadList>>, seed: SeedSettings) -> Arc<RuleService> {
    // Full builtin command map so command-based actions (d.delete_data,
    // d.ignore_commands, raw Command) behave exactly as in the daemon.
    let mut map = CommandMap::new();
    let tiering = Arc::new(rt_core::tiering::TieringManager::new(Default::default()));
    rt_core::commands::initialize_builtin_commands(
        &mut map,
        rt_core::commands::CommandRegistry {
            storage: Arc::new(RwLock::new(rt_rpc::ObjectStorage::new())),
            ip_manager: Arc::new(RwLock::new(rt_core::IpTableManager::new())),
            download_list: Arc::clone(&list),
            view_manager: Arc::new(RwLock::new(rt_core::ViewManager::new())),
            group_manager: Arc::new(RwLock::new(rt_core::ChokeGroupManager::new())),
            throttle_manager: Arc::new(RwLock::new(rt_core::ThrottleManager::new())),
            dht_config: Arc::new(RwLock::new(rt_core::commands::tracker::DhtConfig::default())),
            network_config: Default::default(),
            tiering_manager: Arc::clone(&tiering),
        },
    )
    .unwrap();
    RuleService::new(
        dir.path.join("rules.json"),
        seed,
        RulesSettings::default(),
        list,
        Arc::new(Mutex::new(SessionManager::new())),
        Arc::new(RwLock::new(map)),
        tiering,
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
        cond_mode: Default::default(),
        conditions,
        actions,
        once: false,
        cooldown_secs: None,
        allow_destructive: false,
    }
}

fn seed_enabled(ratio: f64) -> SeedSettings {
    SeedSettings {
        enabled: true,
        policy: SeedPolicy {
            ratio: LimitValue::Set(ratio),
            ..Default::default()
        },
        check_interval_secs: 10,
    }
}

#[tokio::test]
async fn policy_stops_seeding_torrent_and_audits() {
    let dir = TempDir::new("policy_stop");
    let (list, item, hash) = seeding_fixture(&dir, 300, 100); // ratio 3.0
    let svc = service(&dir, Arc::clone(&list), seed_enabled(2.0));

    let entries = svc.check_now().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, HistoryStatus::Ok);
    assert_eq!(entries[0].rule_id, rt_rules::POLICY_RULE_ID);
    assert_eq!(entries[0].torrent_hash, hash);
    assert!(!item.read().unwrap().is_active());

    // Second pass: the torrent is no longer seeding — nothing fires.
    let entries = svc.check_now().await;
    assert!(entries.is_empty());
    assert_eq!(svc.history(10).len(), 1);
}

#[tokio::test]
async fn policy_throttle_suppressed_until_limits_change() {
    let dir = TempDir::new("policy_throttle");
    let (list, item, _hash) = seeding_fixture(&dir, 300, 100);
    let mut seed = seed_enabled(2.0);
    seed.policy.action = Some(rt_rules::SeedAction::Throttle("slow".into()));
    let svc = service(&dir, Arc::clone(&list), seed);

    let entries = svc.check_now().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(item.read().unwrap().throttle_name(), "slow");
    assert!(item.read().unwrap().is_seeding());

    // Still seeding, limits unchanged: suppressed, no audit spam.
    assert!(svc.check_now().await.is_empty());

    // Raise the limit above the current ratio (300 uploaded / 17 completed
    // bytes ≈ 17.6): re-evaluates, no fire.
    let mut changed = svc.seed_settings();
    changed.policy.ratio = LimitValue::Set(20.0);
    svc.set_seed_settings(changed);
    assert!(svc.check_now().await.is_empty());

    // Lower it again: fires once more with the new limits.
    let mut changed = svc.seed_settings();
    changed.policy.ratio = LimitValue::Set(1.0);
    svc.set_seed_settings(changed);
    assert_eq!(svc.check_now().await.len(), 1);
}

#[tokio::test]
async fn once_rule_fires_exactly_once_across_passes() {
    let dir = TempDir::new("once");
    let (list, item, _hash) = seeding_fixture(&dir, 300, 100);
    let svc = service(&dir, Arc::clone(&list), SeedSettings::default());
    let mut r = rule(
        "tag-once",
        vec![Condition::RatioAtLeast(1.0)],
        vec![Action::AddTag("done".into())],
    );
    r.once = true;
    svc.add_rule(r).unwrap();

    assert_eq!(svc.check_now().await.len(), 1);
    assert!(item.read().unwrap().has_tag("done"));
    // Ledger persisted on the torrent: second pass is silent.
    assert!(svc.check_now().await.is_empty());
    assert!(item.read().unwrap().rule_ledger().0.contains_key(&1));
}

#[tokio::test]
async fn event_trigger_fires_on_finished_only() {
    let dir = TempDir::new("event");
    let (list, item, hash) = seeding_fixture(&dir, 300, 100);
    let svc = service(&dir, Arc::clone(&list), SeedSettings::default());
    let mut r = rule(
        "on-finished",
        vec![Condition::Complete(true)],
        vec![Action::AddTag("finished".into())],
    );
    r.trigger = Trigger::Event(EventKind::Finished);
    svc.add_rule(r).unwrap();

    // Periodic pass: event rules do not fire.
    assert!(svc.check_now().await.is_empty());
    // Event pass: fires.
    let entries = svc
        .run_pass(Some(EventMsg {
            kind: EventKind::Finished,
            hash: hash.clone(),
        }))
        .await;
    assert_eq!(entries.len(), 1);
    assert!(item.read().unwrap().has_tag("finished"));
}

#[tokio::test]
async fn dry_run_does_not_touch_state() {
    let dir = TempDir::new("dryrun");
    let (list, item, _hash) = seeding_fixture(&dir, 300, 100);
    let svc = service(&dir, Arc::clone(&list), SeedSettings::default());
    let r = rule(
        "probe",
        vec![Condition::RatioAtLeast(1.0)],
        vec![Action::AddTag("x".into())],
    );
    let result = svc.test_rule(&r).unwrap();
    assert_eq!(result.matches.len(), 1);
    assert!(!item.read().unwrap().has_tag("x"));
    assert!(item.read().unwrap().rule_ledger().0.is_empty());
    assert!(svc.history(10).is_empty());
}

#[tokio::test]
async fn destructive_gate_blocks_remove_with_data() {
    let dir = TempDir::new("gate_block");
    let (list, item, _hash) = seeding_fixture(&dir, 300, 100);
    let svc = service(&dir, Arc::clone(&list), SeedSettings::default());
    let mut r = rule(
        "cleanup",
        vec![Condition::RatioAtLeast(1.0)],
        vec![Action::RemoveWithData],
    );
    r.allow_destructive = true; // rule allows; global gate (default) does not
    svc.add_rule(r).unwrap();

    let entries = svc.check_now().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, HistoryStatus::Skipped);
    assert_eq!(list.read().unwrap().len(), 1);
    assert!(item.read().unwrap().is_seeding());
}

#[tokio::test]
async fn gated_remove_with_data_deletes_payload_and_erases() {
    let dir = TempDir::new("gate_allow");
    let (list, _item, hash) = seeding_fixture(&dir, 300, 100);
    // Materialize the payload so d.delete_data has something to remove.
    std::fs::write(dir.path.join("rules_e2e"), b"rules-e2e-payload").unwrap();
    let svc = service(&dir, Arc::clone(&list), SeedSettings::default());
    svc.set_rules_settings(RulesSettings {
        enabled: true,
        allow_destructive: true,
    });
    let mut r = rule(
        "cleanup",
        vec![Condition::RatioAtLeast(1.0)],
        vec![Action::RemoveWithData],
    );
    r.allow_destructive = true;
    svc.add_rule(r).unwrap();

    let entries = svc.check_now().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, HistoryStatus::Ok);
    assert_eq!(list.read().unwrap().len(), 0);
    assert!(!dir.path.join("rules_e2e").exists());
    let _ = hash;
}

#[tokio::test]
async fn noop_exception_rule_protects_torrent() {
    let dir = TempDir::new("noop");
    let (list, item, _hash) = seeding_fixture(&dir, 300, 100);
    let svc = service(&dir, Arc::clone(&list), seed_enabled(2.0));
    // Exception rule with higher priority (lower number) than the policy?
    // Policies run first, so instead protect via a tag-scoped Noop rule and
    // verify the rule layer honors Noop ordering between user rules.
    let mut guard = rule(
        "protect",
        vec![Condition::TagHas("keep".into())],
        vec![Action::Noop],
    );
    guard.priority = -10;
    svc.add_rule(guard).unwrap();
    svc.add_rule(rule(
        "stop-all",
        vec![Condition::Complete(true)],
        vec![Action::Stop],
    ))
    .unwrap();
    item.write().unwrap().add_tag("keep");

    // Policy still fires (layer A wins over rules) — stop is idempotent here;
    // the assertion targets rule-layer ordering: with the policy disabled the
    // Noop guard must prevent the stop rule from firing.
    let mut off = svc.seed_settings();
    off.enabled = false;
    svc.set_seed_settings(off);
    let entries = svc.check_now().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].actions, vec!["noop".to_string()]);
    assert!(item.read().unwrap().is_seeding());
}
