//! Validation errors for rules, labels and limit values.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuleError {
    #[error("invalid regex in rule {rule_id}: {source}")]
    InvalidRegex {
        rule_id: u64,
        #[source]
        source: regex::Error,
    },

    #[error("rule {0} has no conditions")]
    EmptyConditions(u64),

    #[error("rule {0} has no actions")]
    EmptyActions(u64),

    #[error("rule {0} references unknown tier {1}")]
    UnknownTier(u64, u8),

    #[error("rule {0} has an invalid webhook URL: {1}")]
    InvalidUrl(u64, String),

    #[error("invalid limit value: {0}")]
    InvalidLimitValue(String),

    #[error("invalid action: {0}")]
    InvalidAction(String),

    #[error("unknown rule: {0}")]
    UnknownRule(u64),

    #[error("unknown label: {0}")]
    UnknownLabel(String),

    #[error("duplicate label: {0}")]
    DuplicateLabel(String),

    #[error("invalid rule: {0}")]
    Invalid(String),
}
