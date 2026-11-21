//! Automation Rules page panels (T9, plan.md §6.9): rule list, drawer editor
//! with condition/action builders, dry-run preview and audit history.
//! Mirrors the `components/rss` panel pattern; DTOs come from `state/rules.rs`
//! (T8) and are never duplicated here.

pub mod action_builder;
pub mod condition_builder;
pub mod dry_run_panel;
pub mod history_panel;
pub mod rule_editor;
pub mod rule_list;

pub use action_builder::ActionBuilder;
pub use condition_builder::ConditionBuilder;
pub use dry_run_panel::DryRunPanel;
pub use history_panel::HistoryPanel;
pub use rule_editor::{RuleEditor, RuleEditorHandle};
pub use rule_list::RuleList;

use std::sync::atomic::{AtomicU64, Ordering};

/// Stable identity for condition/action draft rows so keyed `For` lists keep
/// input focus while values change (keys survive edits and reordering).
static NEXT_DRAFT_KEY: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_draft_key() -> u64 {
    NEXT_DRAFT_KEY.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Shared draft parsing helpers. Numeric form fields stay strings while typing
// (RSS editor pattern) and are parsed/validated on save or test. Sizes are
// edited in MB (1 MB = 1048576 bytes, the RSS rule editor convention) and
// converted to bytes for the §6.1 DTOs.
// ---------------------------------------------------------------------------

pub(crate) const MB_BYTES: f64 = 1_048_576.0;

pub(crate) fn parse_required_u64(text: &str, field: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is required."));
    }
    trimmed
        .parse::<u64>()
        .map_err(|_| format!("{field} must be a non-negative whole number."))
}

pub(crate) fn parse_required_u32(text: &str, field: &str) -> Result<u32, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is required."));
    }
    trimmed
        .parse::<u32>()
        .map_err(|_| format!("{field} must be a non-negative whole number."))
}

pub(crate) fn parse_required_u8(text: &str, field: &str) -> Result<u8, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is required."));
    }
    trimmed
        .parse::<u8>()
        .map_err(|_| format!("{field} must be a whole number between 0 and 255."))
}

/// Empty (or whitespace-only) input means "not set".
pub(crate) fn parse_optional_u64(text: &str, field: &str) -> Result<Option<u64>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .map_err(|_| format!("{field} must be a non-negative whole number."))
}

/// Share ratio: `f64`, `1.0` = 100% (§6.1 unit table).
pub(crate) fn parse_ratio(text: &str, field: &str) -> Result<f64, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is required."));
    }
    let value = trimmed
        .parse::<f64>()
        .map_err(|_| format!("{field} must be a number."))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{field} must be a non-negative number."));
    }
    Ok(value)
}

/// Required size in MB, converted to bytes for the DTO.
pub(crate) fn parse_mb(text: &str, field: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is required."));
    }
    let mb = trimmed
        .parse::<f64>()
        .map_err(|_| format!("{field} must be a number of MB."))?;
    if !mb.is_finite() || mb < 0.0 {
        return Err(format!("{field} must be a non-negative number of MB."));
    }
    Ok((mb * MB_BYTES).round() as u64)
}

/// Shortest round-tripping decimal so byte values that are not whole MB still
/// survive an edit round trip exactly (same helper as the RSS rule editor).
pub(crate) fn bytes_to_mb(bytes: u64) -> String {
    format!("{}", bytes as f64 / MB_BYTES)
}

pub(crate) fn required_text(text: &str, field: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Err(format!("{field} is required."))
    } else {
        Ok(trimmed.to_string())
    }
}

/// Empty (or whitespace-only) inputs become None; non-empty values are kept
/// verbatim so e.g. throttle names are not altered.
pub(crate) fn optional_text(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Bool form params are `"true"` / `"false"` select values.
pub(crate) fn parse_bool(text: &str) -> bool {
    text.trim() == "true"
}
