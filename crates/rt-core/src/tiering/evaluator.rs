use std::path::PathBuf;

use super::types::{TierCondition, TierIndex, TierTransitionRule, TieringConfig, TorrentTierState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierTransitionDecision {
    pub from_tier: TierIndex,
    pub to_tier: TierIndex,
    pub target_path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Copy)]
pub struct TorrentEvaluationMetrics {
    pub ratio: f64,
    pub is_done: bool,
    pub uploaded_bytes: u64,
    pub is_active: bool,
    pub last_active: u64,
}

pub struct TierEvaluator;

impl TierEvaluator {
    /// Evaluates if the given torrent should transition to another tier.
    pub fn evaluate_torrent(
        state: &TorrentTierState,
        config: &TieringConfig,
        now: u64,
        metrics: &TorrentEvaluationMetrics,
    ) -> Option<TierTransitionDecision> {
        if !config.enabled || config.tiers.is_empty() {
            return None;
        }

        let current_tier = state.current_tier?;

        for rule in &config.rules {
            if rule.from_tier != current_tier {
                continue;
            }

            if Self::rule_matches(rule, state, now, metrics) {
                // Find target tier definition
                if let Some(target_def) = config.tiers.get(rule.to_tier as usize) {
                    let reason = format!(
                        "Conditions met for transition from tier {} ({}) to tier {} ({})",
                        rule.from_tier,
                        config
                            .tiers
                            .get(rule.from_tier as usize)
                            .map(|t| t.name.as_str())
                            .unwrap_or("unknown"),
                        rule.to_tier,
                        target_def.name
                    );

                    return Some(TierTransitionDecision {
                        from_tier: rule.from_tier,
                        to_tier: rule.to_tier,
                        target_path: target_def.path.clone(),
                        reason,
                    });
                }
            }
        }

        None
    }

    fn rule_matches(
        rule: &TierTransitionRule,
        state: &TorrentTierState,
        now: u64,
        metrics: &TorrentEvaluationMetrics,
    ) -> bool {
        if rule.conditions.is_empty() {
            return false;
        }

        rule.conditions.iter().all(|cond| match cond {
            TierCondition::TimeOnTier(secs) => {
                let elapsed = now.saturating_sub(state.tier_entered_at);
                elapsed >= *secs
            }
            TierCondition::MinRatio(target_ratio) => metrics.ratio >= *target_ratio,
            TierCondition::IsComplete => metrics.is_done,
            TierCondition::MinUploaded(target_bytes) => metrics.uploaded_bytes >= *target_bytes,
            TierCondition::InactiveSince(secs) => {
                if metrics.is_active {
                    false
                } else {
                    let inactive_duration = now.saturating_sub(metrics.last_active);
                    inactive_duration >= *secs
                }
            }
        })
    }
}
