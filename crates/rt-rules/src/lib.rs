//! Pure domain logic for torrent lifecycle management: seed policies with
//! inheritance (global → label → torrent), automation rules (scope + trigger +
//! conditions → actions), and a side-effect-free evaluator that doubles as the
//! dry-run engine.
//!
//! This crate deliberately has **no dependency on `rt-core`/`lt-engine`/tokio**
//! (the `rt-rss` precedent): everything here is testable in isolation over
//! [`TorrentMetrics`] snapshots. The daemon glue (store, worker, action
//! executor) lives in `crates/rstorrent/src/rules.rs`.
//!
//! Unit conventions (plan.md §6.1): new APIs use `f64` ratios where `1.0` means
//! 100%, and `u64` seconds. Legacy `ratio.*` fixed-point (200 = 2.00) and
//! `d.ratio` permille stay untouched at their own boundaries.

pub mod compile;
pub mod error;
pub mod evaluate;
pub mod model;
pub mod resolve;

pub use compile::{compile, CompiledRule};
pub use error::RuleError;
pub use evaluate::{dry_run, evaluate_pass, policy_decision};
pub use model::{
    Action, CondMode, Condition, Decision, EffectiveLimits, EventKind, LabelDef, LimitMode,
    LimitSources, LimitValue, RssProvenance, Rule, RuleLedger, Scope, SeedAction, SeedPolicy,
    TorrentMetrics, TorrentOverrides, Trigger, POLICY_RULE_ID,
};
pub use resolve::resolve_limits;
