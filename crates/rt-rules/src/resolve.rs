//! Inheritance resolution: global → label → torrent (plan.md §5.3, §6.1).
//!
//! Per field, the last explicit (`Unlimited`/`Set`) value wins; `Inherit`
//! defers upward. `mode` resolves label → global (no torrent override);
//! `action` resolves torrent → label → global → `Stop`.

use crate::model::{
    EffectiveLimits, LimitSources, LimitValue, SeedAction, SeedPolicy, TorrentOverrides,
};

/// Resolve the effective seed limits for one torrent.
///
/// `label` is the registered primary label's name and policy (`d.custom1`),
/// if any. `sources` records where each value came from: `"global"`,
/// `"label:<name>"` or `"torrent"` — the level of the **last explicit**
/// choice (`Unlimited` counts as explicit). A global `Inherit` has no higher
/// level to defer to and resolves to unlimited.
pub fn resolve_limits(
    global: &SeedPolicy,
    label: Option<(&str, &SeedPolicy)>,
    overrides: &TorrentOverrides,
) -> EffectiveLimits {
    let (ratio, ratio_source) = resolve_field(
        &overrides.ratio,
        label.map(|(name, policy)| (name, &policy.ratio)),
        &global.ratio,
    );
    let (seeding_time_secs, seeding_time_source) = resolve_field(
        &overrides.seeding_time_secs,
        label.map(|(name, policy)| (name, &policy.seeding_time_secs)),
        &global.seeding_time_secs,
    );
    let (inactive_secs, inactive_source) = resolve_field(
        &overrides.inactive_secs,
        label.map(|(name, policy)| (name, &policy.inactive_secs)),
        &global.inactive_secs,
    );
    // `LimitMode` has no `Inherit`: a present label always supplies the mode.
    let (mode, mode_source) = match label {
        Some((name, policy)) => (policy.mode, format!("label:{name}")),
        None => (global.mode, "global".to_owned()),
    };
    let (action, action_source) = resolve_action(global, label, overrides);
    EffectiveLimits {
        ratio,
        seeding_time_secs,
        inactive_secs,
        mode,
        action,
        sources: LimitSources {
            ratio: ratio_source,
            seeding_time_secs: seeding_time_source,
            inactive_secs: inactive_source,
            mode: mode_source,
            action: action_source,
        },
    }
}

/// Resolve one limit field: torrent → label → global. Returns the effective
/// value (`None` = unlimited) plus the source of the last explicit choice.
fn resolve_field<T: Clone>(
    torrent: &LimitValue<T>,
    label: Option<(&str, &LimitValue<T>)>,
    global: &LimitValue<T>,
) -> (Option<T>, String) {
    if let Some(value) = explicit_value(torrent) {
        return (value, "torrent".to_owned());
    }
    if let Some((name, label_value)) = label {
        if let Some(value) = explicit_value(label_value) {
            return (value, format!("label:{name}"));
        }
    }
    // Global `Inherit` has no higher level: treat as unlimited.
    (explicit_value(global).flatten(), "global".to_owned())
}

/// The explicit choice at one level, if any: `Unlimited` → `Some(None)`,
/// `Set(v)` → `Some(Some(v))`, `Inherit` → `None` (defer upward).
fn explicit_value<T: Clone>(value: &LimitValue<T>) -> Option<Option<T>> {
    match value {
        LimitValue::Inherit => None,
        LimitValue::Unlimited => Some(None),
        LimitValue::Set(value) => Some(Some(value.clone())),
    }
}

/// Resolve the action: torrent → label → global → `Stop` (plan §6.1).
fn resolve_action(
    global: &SeedPolicy,
    label: Option<(&str, &SeedPolicy)>,
    overrides: &TorrentOverrides,
) -> (SeedAction, String) {
    if let Some(action) = &overrides.action {
        return (action.clone(), "torrent".to_owned());
    }
    if let Some((name, policy)) = label {
        if let Some(action) = &policy.action {
            return (action.clone(), format!("label:{name}"));
        }
    }
    match &global.action {
        Some(action) => (action.clone(), "global".to_owned()),
        None => (SeedAction::Stop, "global".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LimitMode;

    fn policy(ratio: LimitValue<f64>, time: LimitValue<u64>, idle: LimitValue<u64>) -> SeedPolicy {
        SeedPolicy {
            ratio,
            seeding_time_secs: time,
            inactive_secs: idle,
            ..Default::default()
        }
    }

    #[test]
    fn global_set_values_win_without_label() {
        let global = policy(
            LimitValue::Set(2.0),
            LimitValue::Set(3600),
            LimitValue::Set(60),
        );
        let effective = resolve_limits(&global, None, &TorrentOverrides::default());
        assert_eq!(effective.ratio, Some(2.0));
        assert_eq!(effective.seeding_time_secs, Some(3600));
        assert_eq!(effective.inactive_secs, Some(60));
        assert_eq!(effective.sources.ratio, "global");
        assert_eq!(effective.sources.seeding_time_secs, "global");
        assert_eq!(effective.sources.inactive_secs, "global");
        assert_eq!(effective.mode, LimitMode::Any);
        assert_eq!(effective.sources.mode, "global");
        assert_eq!(effective.action, SeedAction::Stop);
        assert_eq!(effective.sources.action, "global");
    }

    #[test]
    fn global_inherit_resolves_to_unlimited() {
        let global = policy(
            LimitValue::Inherit,
            LimitValue::Inherit,
            LimitValue::Inherit,
        );
        let effective = resolve_limits(&global, None, &TorrentOverrides::default());
        assert_eq!(effective.ratio, None);
        assert_eq!(effective.seeding_time_secs, None);
        assert_eq!(effective.inactive_secs, None);
        assert_eq!(effective.sources.ratio, "global");
        assert_eq!(effective.sources.seeding_time_secs, "global");
        assert_eq!(effective.sources.inactive_secs, "global");
    }

    #[test]
    fn global_unlimited_resolves_to_unlimited() {
        let global = policy(
            LimitValue::Unlimited,
            LimitValue::Unlimited,
            LimitValue::Unlimited,
        );
        let effective = resolve_limits(&global, None, &TorrentOverrides::default());
        assert_eq!(effective.ratio, None);
        assert_eq!(effective.seeding_time_secs, None);
        assert_eq!(effective.inactive_secs, None);
        assert_eq!(effective.sources.ratio, "global");
    }

    #[test]
    fn label_set_beats_global() {
        let global = policy(
            LimitValue::Set(2.0),
            LimitValue::Set(100),
            LimitValue::Set(10),
        );
        let label = policy(
            LimitValue::Set(3.0),
            LimitValue::Set(200),
            LimitValue::Set(20),
        );
        let effective = resolve_limits(
            &global,
            Some(("movies", &label)),
            &TorrentOverrides::default(),
        );
        assert_eq!(effective.ratio, Some(3.0));
        assert_eq!(effective.seeding_time_secs, Some(200));
        assert_eq!(effective.inactive_secs, Some(20));
        assert_eq!(effective.sources.ratio, "label:movies");
        assert_eq!(effective.sources.seeding_time_secs, "label:movies");
        assert_eq!(effective.sources.inactive_secs, "label:movies");
    }

    #[test]
    fn label_inherit_defers_to_global() {
        let global = policy(
            LimitValue::Set(2.0),
            LimitValue::Set(100),
            LimitValue::Set(10),
        );
        let label = policy(
            LimitValue::Inherit,
            LimitValue::Inherit,
            LimitValue::Inherit,
        );
        let effective = resolve_limits(
            &global,
            Some(("movies", &label)),
            &TorrentOverrides::default(),
        );
        assert_eq!(effective.ratio, Some(2.0));
        assert_eq!(effective.seeding_time_secs, Some(100));
        assert_eq!(effective.inactive_secs, Some(10));
        assert_eq!(effective.sources.ratio, "global");
        assert_eq!(effective.sources.seeding_time_secs, "global");
        assert_eq!(effective.sources.inactive_secs, "global");
    }

    #[test]
    fn label_unlimited_beats_global_set() {
        let global = policy(
            LimitValue::Set(2.0),
            LimitValue::Set(100),
            LimitValue::Set(10),
        );
        let label = policy(
            LimitValue::Unlimited,
            LimitValue::Unlimited,
            LimitValue::Unlimited,
        );
        let effective = resolve_limits(
            &global,
            Some(("movies", &label)),
            &TorrentOverrides::default(),
        );
        assert_eq!(effective.ratio, None);
        assert_eq!(effective.seeding_time_secs, None);
        assert_eq!(effective.inactive_secs, None);
        // Unlimited is an explicit choice: the source stays at the label.
        assert_eq!(effective.sources.ratio, "label:movies");
        assert_eq!(effective.sources.seeding_time_secs, "label:movies");
        assert_eq!(effective.sources.inactive_secs, "label:movies");
    }

    #[test]
    fn torrent_set_beats_label_and_global() {
        let global = policy(
            LimitValue::Set(2.0),
            LimitValue::Set(100),
            LimitValue::Set(10),
        );
        let label = policy(
            LimitValue::Set(3.0),
            LimitValue::Set(200),
            LimitValue::Set(20),
        );
        let overrides = TorrentOverrides {
            ratio: LimitValue::Set(5.0),
            seeding_time_secs: LimitValue::Set(300),
            inactive_secs: LimitValue::Set(30),
            ..Default::default()
        };
        let effective = resolve_limits(&global, Some(("movies", &label)), &overrides);
        assert_eq!(effective.ratio, Some(5.0));
        assert_eq!(effective.seeding_time_secs, Some(300));
        assert_eq!(effective.inactive_secs, Some(30));
        assert_eq!(effective.sources.ratio, "torrent");
        assert_eq!(effective.sources.seeding_time_secs, "torrent");
        assert_eq!(effective.sources.inactive_secs, "torrent");
    }

    #[test]
    fn torrent_inherit_defers_to_label() {
        let global = policy(
            LimitValue::Set(2.0),
            LimitValue::Set(100),
            LimitValue::Set(10),
        );
        let label = policy(
            LimitValue::Set(3.0),
            LimitValue::Set(200),
            LimitValue::Set(20),
        );
        let effective = resolve_limits(
            &global,
            Some(("movies", &label)),
            &TorrentOverrides::default(),
        );
        assert_eq!(effective.ratio, Some(3.0));
        assert_eq!(effective.seeding_time_secs, Some(200));
        assert_eq!(effective.inactive_secs, Some(20));
        assert_eq!(effective.sources.ratio, "label:movies");
    }

    #[test]
    fn torrent_and_label_inherit_defer_to_global() {
        let global = policy(
            LimitValue::Set(2.0),
            LimitValue::Set(100),
            LimitValue::Set(10),
        );
        let label = policy(
            LimitValue::Inherit,
            LimitValue::Inherit,
            LimitValue::Inherit,
        );
        let effective = resolve_limits(
            &global,
            Some(("movies", &label)),
            &TorrentOverrides::default(),
        );
        assert_eq!(effective.ratio, Some(2.0));
        assert_eq!(effective.seeding_time_secs, Some(100));
        assert_eq!(effective.inactive_secs, Some(10));
        assert_eq!(effective.sources.ratio, "global");
    }

    #[test]
    fn torrent_unlimited_beats_label_and_global() {
        let global = policy(
            LimitValue::Set(2.0),
            LimitValue::Set(100),
            LimitValue::Set(10),
        );
        let label = policy(
            LimitValue::Set(3.0),
            LimitValue::Set(200),
            LimitValue::Set(20),
        );
        let overrides = TorrentOverrides {
            ratio: LimitValue::Unlimited,
            seeding_time_secs: LimitValue::Unlimited,
            inactive_secs: LimitValue::Unlimited,
            ..Default::default()
        };
        let effective = resolve_limits(&global, Some(("movies", &label)), &overrides);
        assert_eq!(effective.ratio, None);
        assert_eq!(effective.seeding_time_secs, None);
        assert_eq!(effective.inactive_secs, None);
        assert_eq!(effective.sources.ratio, "torrent");
        assert_eq!(effective.sources.seeding_time_secs, "torrent");
        assert_eq!(effective.sources.inactive_secs, "torrent");
    }

    /// Full 3-level × (inherit/unlimited/set) matrix for one field, checked
    /// against an independent "first explicit level wins" oracle.
    fn inheritance_matrix<T: Clone + PartialEq + std::fmt::Debug>(
        values: [T; 3],
        make_policy: impl Fn(LimitValue<T>) -> SeedPolicy,
        make_overrides: impl Fn(LimitValue<T>) -> TorrentOverrides,
        effective_field: impl Fn(&EffectiveLimits) -> Option<T>,
        source_field: impl Fn(&LimitSources) -> &str,
    ) {
        let [global_value, label_value, torrent_value] = values;
        let globals = [
            LimitValue::Inherit,
            LimitValue::Unlimited,
            LimitValue::Set(global_value),
        ];
        let labels = [
            None,
            Some(LimitValue::Inherit),
            Some(LimitValue::Unlimited),
            Some(LimitValue::Set(label_value)),
        ];
        let torrents = [
            LimitValue::Inherit,
            LimitValue::Unlimited,
            LimitValue::Set(torrent_value),
        ];
        for global in &globals {
            for label in &labels {
                for torrent in &torrents {
                    let global_policy = make_policy(global.clone());
                    let label_policy = label.clone().map(&make_policy);
                    let overrides = make_overrides(torrent.clone());
                    let effective = resolve_limits(
                        &global_policy,
                        label_policy.as_ref().map(|policy| ("movies", policy)),
                        &overrides,
                    );
                    let candidates = [
                        (Some(torrent), "torrent"),
                        (label.as_ref(), "label:movies"),
                        (Some(global), "global"),
                    ];
                    let winner = candidates
                        .into_iter()
                        .find(|(value, _)| value.is_some_and(LimitValue::is_explicit));
                    let (expected, expected_source) = match winner {
                        Some((Some(LimitValue::Set(v)), source)) => (Some(v.clone()), source),
                        Some((Some(LimitValue::Unlimited), source)) => (None, source),
                        // All levels inherit: only possible with global Inherit.
                        _ => (None, "global"),
                    };
                    let context = format!("global={global:?} label={label:?} torrent={torrent:?}");
                    assert_eq!(effective_field(&effective), expected, "{context}");
                    assert_eq!(
                        source_field(&effective.sources),
                        expected_source,
                        "{context}"
                    );
                }
            }
        }
    }

    #[test]
    fn ratio_inheritance_matrix() {
        inheritance_matrix(
            [1.0, 2.0, 3.0],
            |v| SeedPolicy {
                ratio: v,
                ..Default::default()
            },
            |v| TorrentOverrides {
                ratio: v,
                ..Default::default()
            },
            |e| e.ratio,
            |s| s.ratio.as_str(),
        );
    }

    #[test]
    fn seeding_time_inheritance_matrix() {
        inheritance_matrix(
            [100, 200, 300],
            |v| SeedPolicy {
                seeding_time_secs: v,
                ..Default::default()
            },
            |v| TorrentOverrides {
                seeding_time_secs: v,
                ..Default::default()
            },
            |e| e.seeding_time_secs,
            |s| s.seeding_time_secs.as_str(),
        );
    }

    #[test]
    fn inactive_secs_inheritance_matrix() {
        inheritance_matrix(
            [100, 200, 300],
            |v| SeedPolicy {
                inactive_secs: v,
                ..Default::default()
            },
            |v| TorrentOverrides {
                inactive_secs: v,
                ..Default::default()
            },
            |e| e.inactive_secs,
            |s| s.inactive_secs.as_str(),
        );
    }

    #[test]
    fn mode_comes_from_label_when_present() {
        let global = SeedPolicy {
            mode: LimitMode::Any,
            ..Default::default()
        };
        let label = SeedPolicy {
            mode: LimitMode::All,
            ..Default::default()
        };
        let effective = resolve_limits(
            &global,
            Some(("movies", &label)),
            &TorrentOverrides::default(),
        );
        assert_eq!(effective.mode, LimitMode::All);
        assert_eq!(effective.sources.mode, "label:movies");
    }

    #[test]
    fn mode_comes_from_global_without_label() {
        let global = SeedPolicy {
            mode: LimitMode::All,
            ..Default::default()
        };
        let effective = resolve_limits(&global, None, &TorrentOverrides::default());
        assert_eq!(effective.mode, LimitMode::All);
        assert_eq!(effective.sources.mode, "global");
    }

    #[test]
    fn action_torrent_wins_over_label_and_global() {
        let global = SeedPolicy {
            action: Some(SeedAction::Close),
            ..Default::default()
        };
        let label = SeedPolicy {
            action: Some(SeedAction::Stop),
            ..Default::default()
        };
        let overrides = TorrentOverrides {
            action: Some(SeedAction::Remove),
            ..Default::default()
        };
        let effective = resolve_limits(&global, Some(("movies", &label)), &overrides);
        assert_eq!(effective.action, SeedAction::Remove);
        assert_eq!(effective.sources.action, "torrent");
    }

    #[test]
    fn action_label_wins_over_global() {
        let global = SeedPolicy {
            action: Some(SeedAction::Stop),
            ..Default::default()
        };
        let label = SeedPolicy {
            action: Some(SeedAction::Throttle("slow".into())),
            ..Default::default()
        };
        let effective = resolve_limits(
            &global,
            Some(("movies", &label)),
            &TorrentOverrides::default(),
        );
        assert_eq!(effective.action, SeedAction::Throttle("slow".into()));
        assert_eq!(effective.sources.action, "label:movies");
    }

    #[test]
    fn action_inherits_to_global() {
        let global = SeedPolicy {
            action: Some(SeedAction::Close),
            ..Default::default()
        };
        let label = SeedPolicy::default();
        let effective = resolve_limits(
            &global,
            Some(("movies", &label)),
            &TorrentOverrides::default(),
        );
        assert_eq!(effective.action, SeedAction::Close);
        assert_eq!(effective.sources.action, "global");
    }

    #[test]
    fn action_falls_back_to_stop() {
        let effective = resolve_limits(
            &SeedPolicy::default(),
            Some(("movies", &SeedPolicy::default())),
            &TorrentOverrides::default(),
        );
        assert_eq!(effective.action, SeedAction::Stop);
        assert_eq!(effective.sources.action, "global");
    }

    #[test]
    fn plan_5_3_inheritance_example() {
        // global: ratio=2.0, time=unlimited, idle=unlimited, mode=any, action=stop
        let global = SeedPolicy {
            ratio: LimitValue::Set(2.0),
            seeding_time_secs: LimitValue::Unlimited,
            inactive_secs: LimitValue::Unlimited,
            mode: LimitMode::Any,
            action: Some(SeedAction::Stop),
        };
        // label "movies": ratio=3.0, time=7d, idle=inherit, action=inherit
        let label = SeedPolicy {
            ratio: LimitValue::Set(3.0),
            seeding_time_secs: LimitValue::Set(7 * 24 * 3600),
            ..Default::default()
        };
        // torrent X: ratio=inherit, time=unlimited, idle=30m, action=remove
        let overrides = TorrentOverrides {
            ratio: LimitValue::Inherit,
            seeding_time_secs: LimitValue::Unlimited,
            inactive_secs: LimitValue::Set(1800),
            action: Some(SeedAction::Remove),
        };
        let effective = resolve_limits(&global, Some(("movies", &label)), &overrides);
        assert_eq!(effective.ratio, Some(3.0));
        assert_eq!(effective.sources.ratio, "label:movies");
        assert_eq!(effective.seeding_time_secs, None);
        assert_eq!(effective.sources.seeding_time_secs, "torrent");
        assert_eq!(effective.inactive_secs, Some(1800));
        assert_eq!(effective.sources.inactive_secs, "torrent");
        assert_eq!(effective.mode, LimitMode::Any);
        // LimitMode has no Inherit: with a label present the mode source is
        // the label (value matches the plan example's "any").
        assert_eq!(effective.sources.mode, "label:movies");
        assert_eq!(effective.action, SeedAction::Remove);
        assert_eq!(effective.sources.action, "torrent");
    }
}
