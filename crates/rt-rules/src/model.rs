//! Data models for seed policies and automation rules (plan.md §6.1).
//!
//! Serde shapes are binding contracts: enums are externally tagged with
//! `snake_case` names, so [`LimitValue::Inherit`] serializes as `"inherit"`,
//! [`LimitValue::Set`] as `{"set": 2.0}`, and struct variants as
//! `{"free_space_below": {"bytes": …, "path": …}}`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Synthetic rule id used by the built-in seed-policy enforcement (layer A).
/// User rules always get ids ≥ 1 from the store.
pub const POLICY_RULE_ID: u64 = 0;

fn default_true() -> bool {
    true
}

/// A limit value with explicit inheritance. Replaces numeric sentinels
/// (qBittorrent's -2/-1) with a type: impossible states are unrepresentable.
///
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

/// What to do when a seed limit is reached (or as a rule action subset).
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

/// Declarative seed limits at one inheritance level. `Inherit` fields defer
/// upward; the global level (TOML `[seed]`) uses `Unlimited`/`Set` only.
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

/// Per-torrent overrides stored in the `.rtorrent` sidecar. Same shape as
/// [`SeedPolicy`] minus `mode` (mode resolves from label/global only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TorrentOverrides {
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
/// `"torrent"`. Powers the UI/CLI "effective limits" insight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitSources {
    pub ratio: String,
    pub seeding_time_secs: String,
    pub inactive_secs: String,
    pub mode: String,
    pub action: String,
}

/// Resolved limits for one torrent. `None` means unlimited.
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

impl EventKind {
    /// The `event.download.*` name emitted by `DownloadList`.
    pub fn event_name(self) -> &'static str {
        match self {
            Self::Inserted => "event.download.inserted",
            Self::Finished => "event.download.finished",
            Self::Paused => "event.download.paused",
            Self::Resumed => "event.download.resumed",
            Self::HashDone => "event.download.hash_done",
        }
    }

    /// Parse an `event.download.*` name back into a kind.
    pub fn from_event_name(name: &str) -> Option<Self> {
        match name {
            "event.download.inserted" => Some(Self::Inserted),
            "event.download.finished" => Some(Self::Finished),
            "event.download.paused" => Some(Self::Paused),
            "event.download.resumed" => Some(Self::Resumed),
            "event.download.hash_done" => Some(Self::HashDone),
            _ => None,
        }
    }
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

/// A condition over [`TorrentMetrics`]. Swarm conditions (`Seeds*`,
/// `Leechers*`) evaluate to **false when scrape data is unavailable** —
/// the engine never guesses (plan AC-014).
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

/// A user-defined automation rule (layer B).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Assigned by the store (≥ 1).
    pub id: u64,
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

/// RSS provenance recorded in `d.custom` when an item is auto-downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RssProvenance {
    pub feed_id: u64,
    /// `None` for manual downloads with no matching rule.
    pub rule_id: Option<u64>,
    pub item_id: u64,
}

/// The only input to the evaluator: a cheap snapshot of one torrent.
/// Ratio base: `uploaded / completed` (fallback `downloaded`, then
/// `total_size` when complete) — consistent with tiering (plan D5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TorrentMetrics {
    pub hash: String,
    pub name: String,
    pub is_complete: bool,
    pub is_active: bool,
    pub is_seeding: bool,
    pub is_stopped: bool,
    pub uploaded: u64,
    pub downloaded: u64,
    pub completed: u64,
    pub total_size: u64,
    pub ratio: f64,
    pub seeding_time_secs: u64,
    pub idle_secs: u64,
    pub age_secs: u64,
    pub label: Option<String>,
    pub tags: Vec<String>,
    pub tracker_hosts: Vec<String>,
    /// Best scrape across trackers; `None` = no data (conditions fail).
    pub seeds: Option<u32>,
    pub leechers: Option<u32>,
    pub rss: Option<RssProvenance>,
    pub tier: Option<u8>,
    pub ignore_commands: bool,
    /// Free space on the torrent's directory (statvfs); `None` = unknown.
    /// Filled by the glue so the evaluator stays pure.
    #[serde(default)]
    pub dir_free_bytes: Option<u64>,
    /// Free space for extra paths referenced by `FreeSpaceBelow { path: Some }`
    /// conditions (glue stats each distinct rule path once per tick).
    #[serde(default)]
    pub path_free_bytes: HashMap<String, u64>,
}

/// Per-torrent firing ledger: `rule_id → last_fired_at` (unix seconds).
/// Persisted in the sidecar; erased together with the torrent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleLedger(pub HashMap<u64, u64>);

/// One evaluation outcome. `rule_id == POLICY_RULE_ID` marks the built-in
/// seed-policy enforcement (layer A).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub rule_id: u64,
    pub actions: Vec<Action>,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T>(value: &T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
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
        roundtrip(&LimitValue::Set(1.5f64));
    }

    #[test]
    fn limit_value_default_is_inherit() {
        assert_eq!(LimitValue::<f64>::default(), LimitValue::Inherit);
        assert!(!LimitValue::<f64>::Inherit.is_explicit());
        assert!(LimitValue::<f64>::Unlimited.is_explicit());
        assert!(LimitValue::Set(1.0f64).is_explicit());
    }

    #[test]
    fn seed_policy_defaults_and_roundtrip() {
        let empty: SeedPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, SeedPolicy::default());
        assert_eq!(empty.mode, LimitMode::Any);
        assert_eq!(empty.action, None);

        let policy = SeedPolicy {
            ratio: LimitValue::Set(3.0),
            seeding_time_secs: LimitValue::Set(7 * 24 * 3600),
            inactive_secs: LimitValue::Unlimited,
            mode: LimitMode::All,
            action: Some(SeedAction::Throttle("seed".into())),
        };
        roundtrip(&policy);
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
            serde_json::to_string(&SeedAction::Throttle("slow".into())).unwrap(),
            "{\"throttle\":\"slow\"}"
        );
        for action in [
            SeedAction::Stop,
            SeedAction::Close,
            SeedAction::Remove,
            SeedAction::RemoveWithData,
            SeedAction::SuperSeed,
            SeedAction::Notify,
            SeedAction::Command("d.stop=".into()),
        ] {
            roundtrip(&action);
        }
    }

    #[test]
    fn scope_serde_shapes() {
        assert_eq!(serde_json::to_string(&Scope::Global).unwrap(), "\"global\"");
        assert_eq!(
            serde_json::to_string(&Scope::Label("movies".into())).unwrap(),
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
        roundtrip(&Scope::Tracker("tracker.example.org".into()));
        roundtrip(&Scope::Tag("keep".into()));
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
        roundtrip(&rule);
    }

    #[test]
    fn rule_full_roundtrip() {
        let rule = Rule {
            id: 7,
            name: "private cleanup".into(),
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
                Action::AddTag("done".into()),
                Action::MoveToTier(2),
                Action::Webhook {
                    url: "https://hooks.example.org/x".into(),
                },
                Action::Execute(vec!["/usr/bin/notify".into(), "done".into()]),
            ],
            once: true,
            cooldown_secs: Some(3600),
            allow_destructive: true,
        };
        roundtrip(&rule);
    }

    #[test]
    fn condition_and_action_variant_roundtrips() {
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
            Condition::LabelIs("movies".into()),
            Condition::TagHas("keep".into()),
            Condition::TrackerIs("example.org".into()),
            Condition::NameMatches("show.*1080p".into()),
            Condition::AddedByRss,
            Condition::FreeSpaceBelow {
                bytes: 10,
                path: Some("/downloads".into()),
            },
        ];
        for cond in &conditions {
            roundtrip(cond);
        }
        let actions = vec![
            Action::Noop,
            Action::Stop,
            Action::Pause,
            Action::Start,
            Action::Close,
            Action::Remove,
            Action::RemoveWithData,
            Action::MoveToDir("/cold".into()),
            Action::MoveToTier(1),
            Action::SetLabel("movies".into()),
            Action::AddTag("x".into()),
            Action::RemoveTag("x".into()),
            Action::SetThrottle(None),
            Action::SetThrottle(Some("seed".into())),
            Action::SetPriority(3),
            Action::SuperSeed(true),
            Action::Execute(vec!["/bin/true".into()]),
            Action::Webhook {
                url: "http://localhost:9/hook".into(),
            },
            Action::Command("d.stop=".into()),
        ];
        for action in &actions {
            roundtrip(action);
        }
    }

    #[test]
    fn label_def_defaults_and_roundtrip() {
        let minimal: LabelDef = serde_json::from_str(r#"{"name":"movies"}"#).unwrap();
        assert_eq!(minimal.color, None);
        assert_eq!(minimal.policy, SeedPolicy::default());
        assert_eq!(minimal.save_path, None);
        roundtrip(&LabelDef {
            name: "movies".into(),
            color: Some("#7c3aed".into()),
            policy: SeedPolicy {
                ratio: LimitValue::Set(3.0),
                ..Default::default()
            },
            save_path: Some("/disk/movies".into()),
        });
    }

    #[test]
    fn effective_limits_and_sources_roundtrip() {
        let limits = EffectiveLimits {
            ratio: Some(3.0),
            seeding_time_secs: None,
            inactive_secs: Some(1800),
            mode: LimitMode::Any,
            action: SeedAction::Remove,
            sources: LimitSources {
                ratio: "label:movies".into(),
                seeding_time_secs: "torrent".into(),
                inactive_secs: "torrent".into(),
                mode: "global".into(),
                action: "torrent".into(),
            },
        };
        roundtrip(&limits);
    }

    #[test]
    fn overrides_and_ledger_roundtrip() {
        roundtrip(&TorrentOverrides {
            ratio: LimitValue::Unlimited,
            seeding_time_secs: LimitValue::Set(60),
            inactive_secs: LimitValue::Inherit,
            action: Some(SeedAction::Stop),
        });
        let mut ledger = RuleLedger::default();
        ledger.0.insert(3, 1_700_000_000);
        let json = serde_json::to_string(&ledger).unwrap();
        assert_eq!(json, r#"{"3":1700000000}"#);
        roundtrip(&ledger);
    }

    #[test]
    fn metrics_and_provenance_roundtrip() {
        let metrics = TorrentMetrics {
            hash: "0123456789abcdef0123456789abcdef01234567".into(),
            name: "example".into(),
            is_complete: true,
            is_active: true,
            is_seeding: true,
            is_stopped: false,
            uploaded: 200,
            downloaded: 100,
            completed: 100,
            total_size: 100,
            ratio: 2.0,
            seeding_time_secs: 3600,
            idle_secs: 60,
            age_secs: 7200,
            label: Some("movies".into()),
            tags: vec!["keep".into()],
            tracker_hosts: vec!["tracker.example.org".into()],
            seeds: Some(10),
            leechers: None,
            rss: Some(RssProvenance {
                feed_id: 1,
                rule_id: Some(2),
                item_id: 3,
            }),
            tier: Some(0),
            ignore_commands: false,
            dir_free_bytes: Some(1 << 30),
            path_free_bytes: HashMap::new(),
        };
        roundtrip(&metrics);
        roundtrip(&Decision {
            rule_id: POLICY_RULE_ID,
            actions: vec![Action::Stop],
            reason: "ratio 2.0 >= 2.0".into(),
        });
    }

    #[test]
    fn event_kind_names_roundtrip() {
        for kind in [
            EventKind::Inserted,
            EventKind::Finished,
            EventKind::Paused,
            EventKind::Resumed,
            EventKind::HashDone,
        ] {
            assert_eq!(EventKind::from_event_name(kind.event_name()), Some(kind));
        }
        assert_eq!(EventKind::from_event_name("event.download.erased"), None);
        assert_eq!(EventKind::from_event_name("bogus"), None);
    }
}
