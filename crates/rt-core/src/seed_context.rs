//! Global seed-policy context slot (plan.md §6.2, task T3b).
//!
//! The daemon glue (`RuleService`) publishes the runtime `[seed]` globals and
//! the label registry once at startup; the `d.seed_limits` builtin resolves
//! effective limits through this slot. Before publication the builtin falls
//! back to default globals and no labels, so it works in a bare command map
//! (tests, early startup) without any glue wiring.
//!
//! Mirrors the `install_ratio_context` / `install_throttle_manager` pattern in
//! `network_runtime`: a `OnceLock` published once, later installs ignored.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use rt_rules::{LabelDef, SeedPolicy};

/// Shared seed-policy state: the global `[seed]` policy plus the label
/// registry used to match `d.custom1` against label policies.
pub struct SeedContext {
    pub globals: SeedPolicy,
    pub labels: Vec<LabelDef>,
}

static SEED_CONTEXT: OnceLock<Arc<RwLock<SeedContext>>> = OnceLock::new();

/// Publish the seed-policy context. Called once by the daemon control plane
/// after the rule store is loaded; subsequent installations are ignored.
pub fn install_seed_context(ctx: Arc<RwLock<SeedContext>>) {
    let _ = SEED_CONTEXT.set(ctx);
}

/// Handle to the published context, or `None` before publication.
pub fn seed_context() -> Option<Arc<RwLock<SeedContext>>> {
    SEED_CONTEXT.get().map(Arc::clone)
}

/// Master switch: while the seed-policy engine is enabled it owns seeding
/// torrents, and the legacy group ratio enforcement in `network_runtime`
/// stands down (plan §5.4.6: one action owner).
static SEED_POLICIES_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set whether the seed-policy engine is active (`[seed] enabled`).
pub fn set_seed_policies_active(active: bool) {
    SEED_POLICIES_ACTIVE.store(active, Ordering::Relaxed);
}

/// Whether the seed-policy engine is active. Read by the network runtime to
/// skip legacy ratio enforcement.
pub fn seed_policies_active() -> bool {
    SEED_POLICIES_ACTIVE.load(Ordering::Relaxed)
}
