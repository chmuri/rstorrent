//! Label registry panels (plan.md T10, §6.9): the label manager and the
//! per-label seed policy dialog, plus the shared seed-control vocabulary
//! (limit kinds, time units, action select) reused by the settings
//! "Seeding limits" card.

pub mod label_manager;
pub mod label_policy_dialog;

pub use label_manager::LabelManager;
pub use label_policy_dialog::{
    build_action, destructive_warning, parse_duration_value, parse_ratio_value, split_duration,
    ActionKind, TimeUnit,
};
