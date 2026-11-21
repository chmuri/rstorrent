//! Pure evaluation: seed-policy decisions (layer A) and automation rules
//! (layer B). No I/O, no mutation — the same functions power the dry-run.
//!
//! Semantics (plan.md §6.1):
//! - rules are sorted `(priority, id)`; **first match wins** per torrent/pass;
//! - `Action::Noop` is returned as a decision (executor stops, audits);
//! - `once`/`cooldown_secs` consult the per-torrent [`RuleLedger`];
//! - swarm conditions fail when scrape data is `None` (never guess);
//! - ratio with base `0` is **not** satisfied (protects fresh magnets).

use std::collections::HashMap;

use regex::Regex;

use crate::compile::CompiledRule;
use crate::model::{
    Action, CondMode, Condition, Decision, EffectiveLimits, EventKind, LimitMode, Rule, RuleLedger,
    Scope, SeedAction, TorrentMetrics, Trigger, POLICY_RULE_ID,
};

/// Evaluate automation rules for one torrent. `event` is `Some(kind)` when
/// triggered by a lifecycle event, `None` on a periodic tick.
///
/// `rules` must come from [`crate::compile::compile`] (sorted by
/// `(priority, id)`); the first fully matching rule wins. Torrents with
/// `ignore_commands` are skipped entirely (rtorrent compatibility, plan D11).
pub fn evaluate_pass(
    rules: &[CompiledRule],
    m: &TorrentMetrics,
    now: u64,
    ledger: &RuleLedger,
    event: Option<EventKind>,
) -> Option<Decision> {
    if m.ignore_commands {
        return None;
    }
    rules.iter().find_map(|compiled| {
        let rule = &compiled.rule;
        if !rule.enabled
            || !scope_matches(&rule.scope, m)
            || !trigger_matches(&rule.trigger, event)
            || !conditions_match(compiled, m)
            || !ledger_allows(rule, now, ledger)
        {
            return None;
        }
        Some(Decision {
            rule_id: rule.id,
            actions: rule.actions.clone(),
            reason: decision_reason(compiled, m),
        })
    })
}

/// Built-in seed-policy enforcement (layer A). Returns a decision with
/// `rule_id == POLICY_RULE_ID` when the effective limits are reached.
///
/// Only `Some` limits are active; `LimitMode::Any` fires when at least one
/// active limit is reached, `All` when every active limit is reached. The
/// ratio limit additionally requires a non-zero base (fresh magnets with no
/// downloaded data never satisfy it, plan §6.1 note).
pub fn policy_decision(limits: &EffectiveLimits, m: &TorrentMetrics) -> Option<Decision> {
    let mut active = 0usize;
    let mut satisfied = 0usize;
    let mut reasons: Vec<String> = Vec::new();
    if let Some(limit) = limits.ratio {
        active += 1;
        if ratio_base(m) > 0 && m.ratio >= limit {
            satisfied += 1;
            reasons.push(format!("ratio {:.2} >= {limit:.2}", m.ratio));
        }
    }
    if let Some(limit) = limits.seeding_time_secs {
        active += 1;
        if m.seeding_time_secs >= limit {
            satisfied += 1;
            reasons.push(format!("seeding_time {}s >= {limit}s", m.seeding_time_secs));
        }
    }
    if let Some(limit) = limits.inactive_secs {
        active += 1;
        if m.idle_secs >= limit {
            satisfied += 1;
            reasons.push(format!("idle {}s >= {limit}s", m.idle_secs));
        }
    }
    if active == 0 {
        return None;
    }
    let reached = match limits.mode {
        LimitMode::Any => satisfied > 0,
        LimitMode::All => satisfied == active,
    };
    if !reached {
        return None;
    }
    Some(Decision {
        rule_id: POLICY_RULE_ID,
        actions: vec![seed_action_to_action(&limits.action)],
        reason: reasons.join("; "),
    })
}

/// Dry-run: evaluate rules against many torrents **without touching ledgers**.
/// Returns matched torrents paired with their decision. Each torrent uses the
/// ledger registered under its hash (empty when absent); evaluation always
/// runs as a periodic pass (`event = None`).
pub fn dry_run(
    rules: &[CompiledRule],
    metrics: &[TorrentMetrics],
    now: u64,
    ledgers: &HashMap<String, RuleLedger>,
) -> Vec<(TorrentMetrics, Decision)> {
    let empty = RuleLedger::default();
    metrics
        .iter()
        .filter_map(|m| {
            let ledger = ledgers.get(&m.hash).unwrap_or(&empty);
            evaluate_pass(rules, m, now, ledger, None).map(|decision| (m.clone(), decision))
        })
        .collect()
}

/// Ratio base: `completed` → `downloaded` → `total_size` when complete
/// (consistent with tiering, plan D5). Zero means "no real data yet".
fn ratio_base(m: &TorrentMetrics) -> u64 {
    if m.completed > 0 {
        m.completed
    } else if m.downloaded > 0 {
        m.downloaded
    } else if m.is_complete {
        m.total_size
    } else {
        0
    }
}

/// Map a resolved [`SeedAction`] onto the executor's [`Action`] vocabulary.
fn seed_action_to_action(action: &SeedAction) -> Action {
    match action {
        SeedAction::Stop => Action::Stop,
        SeedAction::Close => Action::Close,
        SeedAction::Remove => Action::Remove,
        SeedAction::RemoveWithData => Action::RemoveWithData,
        SeedAction::Throttle(name) => Action::SetThrottle(Some(name.clone())),
        SeedAction::SuperSeed => Action::SuperSeed(true),
        SeedAction::Notify => Action::Noop,
        SeedAction::Command(command) => Action::Command(command.clone()),
    }
}

fn scope_matches(scope: &Scope, m: &TorrentMetrics) -> bool {
    match scope {
        Scope::Global => true,
        Scope::Label(label) => m.label.as_deref() == Some(label.as_str()),
        Scope::Tag(tag) => m.tags.contains(tag),
        Scope::Tracker(host) => tracker_matches(m, host),
        Scope::Rss { feed_id, rule_id } => m.rss.as_ref().is_some_and(|rss| {
            feed_id.is_none_or(|id| rss.feed_id == id)
                && rule_id.is_none_or(|id| rss.rule_id == Some(id))
        }),
    }
}

/// Tracker hosts compare case-insensitively (DNS hosts are ASCII).
fn tracker_matches(m: &TorrentMetrics, host: &str) -> bool {
    m.tracker_hosts
        .iter()
        .any(|tracker| tracker.eq_ignore_ascii_case(host))
}

fn trigger_matches(trigger: &Trigger, event: Option<EventKind>) -> bool {
    match trigger {
        Trigger::Periodic => event.is_none(),
        Trigger::Event(kind) => event == Some(*kind),
    }
}

/// `once`: any ledger entry blocks forever. `cooldown_secs`: block while
/// `now - last_fired_at < cooldown` (the boundary `==` is allowed).
fn ledger_allows(rule: &Rule, now: u64, ledger: &RuleLedger) -> bool {
    let Some(&last_fired_at) = ledger.0.get(&rule.id) else {
        return true;
    };
    if rule.once {
        return false;
    }
    match rule.cooldown_secs {
        Some(cooldown) => now.saturating_sub(last_fired_at) >= cooldown,
        None => true,
    }
}

/// Pair each condition with its pre-compiled regex (`NameMatches` only, in
/// condition order) so evaluation never re-parses patterns.
fn condition_pairs(compiled: &CompiledRule) -> impl Iterator<Item = (&Condition, Option<&Regex>)> {
    let mut regexes = compiled.name_regexes.iter();
    compiled.rule.conditions.iter().map(move |condition| {
        let regex = match condition {
            Condition::NameMatches(_) => regexes.next(),
            _ => None,
        };
        (condition, regex)
    })
}

fn conditions_match(compiled: &CompiledRule, m: &TorrentMetrics) -> bool {
    let mut pairs = condition_pairs(compiled);
    match compiled.rule.cond_mode {
        CondMode::All => pairs.all(|(condition, regex)| condition_matches(condition, m, regex)),
        CondMode::Any => pairs.any(|(condition, regex)| condition_matches(condition, m, regex)),
    }
}

fn condition_matches(condition: &Condition, m: &TorrentMetrics, regex: Option<&Regex>) -> bool {
    match condition {
        Condition::RatioAtLeast(limit) => m.ratio >= *limit,
        Condition::RatioBelow(limit) => m.ratio < *limit,
        Condition::SeedingTimeAtLeast(limit) => m.seeding_time_secs >= *limit,
        Condition::IdleAtLeast(limit) => m.idle_secs >= *limit,
        Condition::AgeAtLeast(limit) => m.age_secs >= *limit,
        Condition::Complete(expected) => m.is_complete == *expected,
        Condition::Active(expected) => m.is_active == *expected,
        Condition::Stopped(expected) => m.is_stopped == *expected,
        Condition::SizeAtLeast(limit) => m.total_size >= *limit,
        Condition::SizeAtMost(limit) => m.total_size <= *limit,
        // Swarm data: `None` means "no scrape" — never guess (plan AC-014).
        Condition::SeedsAtLeast(limit) => m.seeds.is_some_and(|seeds| seeds >= *limit),
        Condition::SeedsAtMost(limit) => m.seeds.is_some_and(|seeds| seeds <= *limit),
        Condition::LeechersAtLeast(limit) => m.leechers.is_some_and(|leechers| leechers >= *limit),
        Condition::LabelIs(label) => m.label.as_deref() == Some(label.as_str()),
        Condition::TagHas(tag) => m.tags.contains(tag),
        Condition::TrackerIs(host) => tracker_matches(m, host),
        Condition::NameMatches(_) => regex.is_some_and(|regex| regex.is_match(&m.name)),
        Condition::AddedByRss => m.rss.is_some(),
        Condition::FreeSpaceBelow { bytes, path } => {
            let free = match path {
                Some(path) => m.path_free_bytes.get(path).copied(),
                None => m.dir_free_bytes,
            };
            free.is_some_and(|free| free < *bytes)
        }
    }
}

/// Concise audit reason: the satisfied conditions, in rule order.
fn decision_reason(compiled: &CompiledRule, m: &TorrentMetrics) -> String {
    condition_pairs(compiled)
        .filter(|(condition, regex)| condition_matches(condition, m, *regex))
        .map(|(condition, _)| describe_condition(condition, m))
        .collect::<Vec<_>>()
        .join("; ")
}

fn describe_condition(condition: &Condition, m: &TorrentMetrics) -> String {
    match condition {
        Condition::RatioAtLeast(limit) => format!("ratio {:.2} >= {limit:.2}", m.ratio),
        Condition::RatioBelow(limit) => format!("ratio {:.2} < {limit:.2}", m.ratio),
        Condition::SeedingTimeAtLeast(limit) => {
            format!("seeding_time {}s >= {limit}s", m.seeding_time_secs)
        }
        Condition::IdleAtLeast(limit) => format!("idle {}s >= {limit}s", m.idle_secs),
        Condition::AgeAtLeast(limit) => format!("age {}s >= {limit}s", m.age_secs),
        Condition::Complete(expected) => format!("complete == {expected}"),
        Condition::Active(expected) => format!("active == {expected}"),
        Condition::Stopped(expected) => format!("stopped == {expected}"),
        Condition::SizeAtLeast(limit) => format!("size {} >= {limit}", m.total_size),
        Condition::SizeAtMost(limit) => format!("size {} <= {limit}", m.total_size),
        Condition::SeedsAtLeast(limit) => format!("seeds {} >= {limit}", m.seeds.unwrap_or(0)),
        Condition::SeedsAtMost(limit) => format!("seeds {} <= {limit}", m.seeds.unwrap_or(0)),
        Condition::LeechersAtLeast(limit) => {
            format!("leechers {} >= {limit}", m.leechers.unwrap_or(0))
        }
        Condition::LabelIs(label) => format!("label == {label}"),
        Condition::TagHas(tag) => format!("tags contains {tag}"),
        Condition::TrackerIs(host) => format!("tracker == {host}"),
        Condition::NameMatches(pattern) => format!("name matches /{pattern}/i"),
        Condition::AddedByRss => "added by RSS".to_owned(),
        Condition::FreeSpaceBelow { bytes, path } => match path {
            Some(path) => format!("free space on {path} < {bytes}"),
            None => format!("free space < {bytes}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile;
    use crate::model::{LimitSources, RssProvenance};

    fn base_metrics() -> TorrentMetrics {
        TorrentMetrics {
            hash: "hash-1".into(),
            name: "Ubuntu 24.04 ISO".into(),
            is_complete: true,
            is_active: true,
            is_seeding: true,
            is_stopped: false,
            uploaded: 250,
            downloaded: 100,
            completed: 100,
            total_size: 100,
            ratio: 2.5,
            seeding_time_secs: 3600,
            idle_secs: 600,
            age_secs: 86_400,
            label: Some("movies".into()),
            tags: vec!["keep".into(), "hd".into()],
            tracker_hosts: vec!["Tracker.Example.ORG".into()],
            seeds: Some(10),
            leechers: Some(4),
            rss: None,
            tier: Some(0),
            ignore_commands: false,
            dir_free_bytes: Some(1 << 30),
            path_free_bytes: HashMap::new(),
        }
    }

    fn rule(id: u64, conditions: Vec<Condition>, actions: Vec<Action>) -> Rule {
        Rule {
            id,
            name: format!("rule-{id}"),
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

    fn compiled(rules: Vec<Rule>) -> Vec<CompiledRule> {
        compile(&rules).expect("test rules must compile")
    }

    fn matching_rule(id: u64) -> Rule {
        rule(id, vec![Condition::Complete(true)], vec![Action::Stop])
    }

    fn scoped(id: u64, scope: Scope) -> Rule {
        Rule {
            scope,
            ..matching_rule(id)
        }
    }

    fn ledger_with(rule_id: u64, at: u64) -> RuleLedger {
        let mut ledger = RuleLedger::default();
        ledger.0.insert(rule_id, at);
        ledger
    }

    fn fires(rules: &[CompiledRule], m: &TorrentMetrics) -> bool {
        evaluate_pass(rules, m, 1_000, &RuleLedger::default(), None).is_some()
    }

    fn effective_limits(
        ratio: Option<f64>,
        seeding_time_secs: Option<u64>,
        inactive_secs: Option<u64>,
        mode: LimitMode,
        action: SeedAction,
    ) -> EffectiveLimits {
        EffectiveLimits {
            ratio,
            seeding_time_secs,
            inactive_secs,
            mode,
            action,
            sources: LimitSources {
                ratio: "global".into(),
                seeding_time_secs: "global".into(),
                inactive_secs: "global".into(),
                mode: "global".into(),
                action: "global".into(),
            },
        }
    }

    #[test]
    fn ignore_commands_skips_everything() {
        let mut m = base_metrics();
        m.ignore_commands = true;
        let rules = compiled(vec![matching_rule(1)]);
        assert!(!fires(&rules, &m));
    }

    #[test]
    fn first_match_wins_by_priority() {
        let mut low = matching_rule(1);
        low.priority = 10;
        let mut high = rule(2, vec![Condition::Complete(true)], vec![Action::Noop]);
        high.priority = -5;
        let rules = compiled(vec![low, high]);
        let decision = evaluate_pass(&rules, &base_metrics(), 1_000, &RuleLedger::default(), None)
            .expect("a rule must match");
        assert_eq!(decision.rule_id, 2);
        assert_eq!(decision.actions, vec![Action::Noop]);
    }

    #[test]
    fn first_match_wins_by_id_when_priority_ties() {
        let rules = compiled(vec![matching_rule(2), matching_rule(1)]);
        let decision = evaluate_pass(&rules, &base_metrics(), 1_000, &RuleLedger::default(), None)
            .expect("a rule must match");
        assert_eq!(decision.rule_id, 1);
    }

    #[test]
    fn noop_exception_stops_evaluation() {
        let noop = rule(1, vec![Condition::Complete(true)], vec![Action::Noop]);
        let mut later = matching_rule(2);
        later.priority = 1;
        let rules = compiled(vec![later, noop]);
        let decision = evaluate_pass(&rules, &base_metrics(), 1_000, &RuleLedger::default(), None)
            .expect("the Noop rule must match");
        assert_eq!(decision.rule_id, 1);
        assert_eq!(decision.actions, vec![Action::Noop]);
    }

    #[test]
    fn disabled_rules_are_skipped() {
        let mut disabled = matching_rule(1);
        disabled.enabled = false;
        let enabled = rule(2, vec![Condition::Complete(true)], vec![Action::Noop]);
        let rules = compiled(vec![disabled, enabled]);
        let decision = evaluate_pass(&rules, &base_metrics(), 1_000, &RuleLedger::default(), None)
            .expect("the enabled rule must match");
        assert_eq!(decision.rule_id, 2);
    }

    #[test]
    fn cond_mode_all_requires_every_condition() {
        let r = rule(
            1,
            vec![Condition::Complete(true), Condition::RatioAtLeast(3.0)],
            vec![Action::Stop],
        );
        assert!(!fires(&compiled(vec![r]), &base_metrics()));
    }

    #[test]
    fn cond_mode_any_needs_only_one() {
        let r = Rule {
            cond_mode: CondMode::Any,
            ..rule(
                1,
                vec![Condition::Complete(false), Condition::RatioAtLeast(2.0)],
                vec![Action::Stop],
            )
        };
        assert!(fires(&compiled(vec![r]), &base_metrics()));
    }

    #[test]
    fn cond_mode_any_all_false_fails() {
        let r = Rule {
            cond_mode: CondMode::Any,
            ..rule(
                1,
                vec![Condition::Complete(false), Condition::RatioAtLeast(3.0)],
                vec![Action::Stop],
            )
        };
        assert!(!fires(&compiled(vec![r]), &base_metrics()));
    }

    #[test]
    fn once_fires_only_without_ledger_entry() {
        let r = Rule {
            once: true,
            ..matching_rule(1)
        };
        let rules = compiled(vec![r]);
        let m = base_metrics();
        assert!(evaluate_pass(&rules, &m, 1_000, &RuleLedger::default(), None).is_some());
        assert!(evaluate_pass(&rules, &m, 1_000, &ledger_with(1, 500), None).is_none());
    }

    #[test]
    fn cooldown_blocks_inside_window() {
        let r = Rule {
            cooldown_secs: Some(100),
            ..matching_rule(1)
        };
        let rules = compiled(vec![r]);
        let ledger = ledger_with(1, 1_000);
        assert!(evaluate_pass(&rules, &base_metrics(), 1_099, &ledger, None).is_none());
    }

    #[test]
    fn cooldown_boundary_is_inclusive() {
        let r = Rule {
            cooldown_secs: Some(100),
            ..matching_rule(1)
        };
        let rules = compiled(vec![r]);
        let ledger = ledger_with(1, 1_000);
        // now - last == cooldown is allowed; one second past is too.
        assert!(evaluate_pass(&rules, &base_metrics(), 1_100, &ledger, None).is_some());
        assert!(evaluate_pass(&rules, &base_metrics(), 1_101, &ledger, None).is_some());
    }

    #[test]
    fn cooldown_zero_always_allows() {
        let r = Rule {
            cooldown_secs: Some(0),
            ..matching_rule(1)
        };
        let rules = compiled(vec![r]);
        assert!(
            evaluate_pass(&rules, &base_metrics(), 1_000, &ledger_with(1, 1_000), None).is_some()
        );
    }

    #[test]
    fn once_takes_precedence_over_cooldown() {
        let r = Rule {
            once: true,
            cooldown_secs: Some(10),
            ..matching_rule(1)
        };
        let rules = compiled(vec![r]);
        assert!(evaluate_pass(&rules, &base_metrics(), 1_000, &ledger_with(1, 0), None).is_none());
    }

    #[test]
    fn ledger_entry_without_once_or_cooldown_allows() {
        let rules = compiled(vec![matching_rule(1)]);
        assert!(
            evaluate_pass(&rules, &base_metrics(), 1_000, &ledger_with(1, 999), None).is_some()
        );
    }

    #[test]
    fn periodic_trigger_only_on_tick() {
        let rules = compiled(vec![matching_rule(1)]);
        let m = base_metrics();
        assert!(evaluate_pass(&rules, &m, 1_000, &RuleLedger::default(), None).is_some());
        assert!(evaluate_pass(
            &rules,
            &m,
            1_000,
            &RuleLedger::default(),
            Some(EventKind::Finished)
        )
        .is_none());
    }

    #[test]
    fn event_trigger_requires_matching_kind() {
        let r = Rule {
            trigger: Trigger::Event(EventKind::Finished),
            ..matching_rule(1)
        };
        let rules = compiled(vec![r]);
        let m = base_metrics();
        let ledger = RuleLedger::default();
        assert!(evaluate_pass(&rules, &m, 1_000, &ledger, Some(EventKind::Finished)).is_some());
        assert!(evaluate_pass(&rules, &m, 1_000, &ledger, Some(EventKind::Paused)).is_none());
        assert!(evaluate_pass(&rules, &m, 1_000, &ledger, None).is_none());
    }

    #[test]
    fn event_and_periodic_rules_split_by_pass_kind() {
        let periodic = matching_rule(2);
        let event = Rule {
            trigger: Trigger::Event(EventKind::Finished),
            ..rule(1, vec![Condition::Complete(true)], vec![Action::Noop])
        };
        let rules = compiled(vec![periodic, event]);
        let m = base_metrics();
        let ledger = RuleLedger::default();
        let on_event = evaluate_pass(&rules, &m, 1_000, &ledger, Some(EventKind::Finished))
            .expect("event rule");
        assert_eq!(on_event.rule_id, 1);
        let on_tick = evaluate_pass(&rules, &m, 1_000, &ledger, None).expect("periodic rule");
        assert_eq!(on_tick.rule_id, 2);
    }

    #[test]
    fn scope_global_matches_any_torrent() {
        let mut m = base_metrics();
        m.label = None;
        m.tags = vec![];
        m.tracker_hosts = vec![];
        m.rss = None;
        assert!(fires(&compiled(vec![scoped(1, Scope::Global)]), &m));
    }

    #[test]
    fn scope_label_equality() {
        let m = base_metrics();
        assert!(fires(
            &compiled(vec![scoped(1, Scope::Label("movies".into()))]),
            &m
        ));
        // Labels are case-sensitive.
        assert!(!fires(
            &compiled(vec![scoped(1, Scope::Label("Movies".into()))]),
            &m
        ));
        assert!(!fires(
            &compiled(vec![scoped(1, Scope::Label("other".into()))]),
            &m
        ));
        let mut unlabeled = base_metrics();
        unlabeled.label = None;
        assert!(!fires(
            &compiled(vec![scoped(1, Scope::Label("movies".into()))]),
            &unlabeled
        ));
    }

    #[test]
    fn scope_tag_membership() {
        let m = base_metrics();
        assert!(fires(
            &compiled(vec![scoped(1, Scope::Tag("keep".into()))]),
            &m
        ));
        assert!(fires(
            &compiled(vec![scoped(1, Scope::Tag("hd".into()))]),
            &m
        ));
        assert!(!fires(
            &compiled(vec![scoped(1, Scope::Tag("missing".into()))]),
            &m
        ));
        let mut untagged = base_metrics();
        untagged.tags = vec![];
        assert!(!fires(
            &compiled(vec![scoped(1, Scope::Tag("keep".into()))]),
            &untagged
        ));
    }

    #[test]
    fn scope_tracker_case_insensitive() {
        let m = base_metrics(); // tracker_hosts: ["Tracker.Example.ORG"]
        assert!(fires(
            &compiled(vec![scoped(
                1,
                Scope::Tracker("tracker.example.org".into())
            )]),
            &m
        ));
        assert!(fires(
            &compiled(vec![scoped(
                1,
                Scope::Tracker("TRACKER.EXAMPLE.ORG".into())
            )]),
            &m
        ));
        assert!(!fires(
            &compiled(vec![scoped(1, Scope::Tracker("other.org".into()))]),
            &m
        ));
        let mut no_trackers = base_metrics();
        no_trackers.tracker_hosts = vec![];
        assert!(!fires(
            &compiled(vec![scoped(
                1,
                Scope::Tracker("tracker.example.org".into())
            )]),
            &no_trackers
        ));
    }

    #[test]
    fn scope_rss_wildcards_and_exact() {
        let mut m = base_metrics();
        m.rss = Some(RssProvenance {
            feed_id: 1,
            rule_id: Some(2),
            item_id: 3,
        });
        for scope in [
            Scope::Rss {
                feed_id: Some(1),
                rule_id: Some(2),
            },
            Scope::Rss {
                feed_id: Some(1),
                rule_id: None,
            },
            Scope::Rss {
                feed_id: None,
                rule_id: Some(2),
            },
            Scope::Rss {
                feed_id: None,
                rule_id: None,
            },
        ] {
            assert!(fires(&compiled(vec![scoped(1, scope)]), &m), "must match");
        }
        for scope in [
            Scope::Rss {
                feed_id: Some(9),
                rule_id: None,
            },
            Scope::Rss {
                feed_id: None,
                rule_id: Some(9),
            },
            Scope::Rss {
                feed_id: Some(9),
                rule_id: Some(9),
            },
        ] {
            assert!(
                !fires(&compiled(vec![scoped(1, scope)]), &m),
                "must not match"
            );
        }
        // No RSS provenance: even the fully wildcard scope fails.
        m.rss = None;
        assert!(!fires(
            &compiled(vec![scoped(
                1,
                Scope::Rss {
                    feed_id: None,
                    rule_id: None
                }
            )]),
            &m
        ));
    }

    #[test]
    fn scope_rss_manual_download_without_rule() {
        let mut m = base_metrics();
        m.rss = Some(RssProvenance {
            feed_id: 1,
            rule_id: None,
            item_id: 3,
        });
        assert!(!fires(
            &compiled(vec![scoped(
                1,
                Scope::Rss {
                    feed_id: Some(1),
                    rule_id: Some(2)
                }
            )]),
            &m
        ));
        assert!(fires(
            &compiled(vec![scoped(
                1,
                Scope::Rss {
                    feed_id: Some(1),
                    rule_id: None
                }
            )]),
            &m
        ));
        assert!(!fires(
            &compiled(vec![scoped(
                1,
                Scope::Rss {
                    feed_id: None,
                    rule_id: Some(2)
                }
            )]),
            &m
        ));
    }

    #[test]
    fn ratio_condition_boundaries() {
        let m = base_metrics(); // ratio 2.5
        for (condition, expected) in [
            (Condition::RatioAtLeast(2.5), true),
            (Condition::RatioAtLeast(2.6), false),
            (Condition::RatioBelow(2.5), false),
            (Condition::RatioBelow(2.6), true),
        ] {
            let rules = compiled(vec![rule(1, vec![condition], vec![Action::Stop])]);
            assert_eq!(fires(&rules, &m), expected);
        }
    }

    #[test]
    fn time_condition_boundaries() {
        let m = base_metrics(); // seeding 3600, idle 600, age 86400
        for (condition, expected) in [
            (Condition::SeedingTimeAtLeast(3600), true),
            (Condition::SeedingTimeAtLeast(3601), false),
            (Condition::IdleAtLeast(600), true),
            (Condition::IdleAtLeast(601), false),
            (Condition::AgeAtLeast(86_400), true),
            (Condition::AgeAtLeast(86_401), false),
        ] {
            let rules = compiled(vec![rule(1, vec![condition], vec![Action::Stop])]);
            assert_eq!(fires(&rules, &m), expected);
        }
    }

    #[test]
    fn state_condition_equality() {
        let m = base_metrics(); // complete, active, not stopped
        for (condition, expected) in [
            (Condition::Complete(true), true),
            (Condition::Complete(false), false),
            (Condition::Active(true), true),
            (Condition::Active(false), false),
            (Condition::Stopped(false), true),
            (Condition::Stopped(true), false),
        ] {
            let rules = compiled(vec![rule(1, vec![condition], vec![Action::Stop])]);
            assert_eq!(fires(&rules, &m), expected);
        }
    }

    #[test]
    fn size_condition_boundaries() {
        let m = base_metrics(); // total_size 100
        for (condition, expected) in [
            (Condition::SizeAtLeast(100), true),
            (Condition::SizeAtLeast(101), false),
            (Condition::SizeAtMost(100), true),
            (Condition::SizeAtMost(99), false),
        ] {
            let rules = compiled(vec![rule(1, vec![condition], vec![Action::Stop])]);
            assert_eq!(fires(&rules, &m), expected);
        }
    }

    #[test]
    fn label_tag_tracker_conditions() {
        let m = base_metrics();
        for (condition, expected) in [
            (Condition::LabelIs("movies".into()), true),
            (Condition::LabelIs("other".into()), false),
            (Condition::TagHas("hd".into()), true),
            (Condition::TagHas("nope".into()), false),
            (Condition::TrackerIs("tracker.example.org".into()), true),
            (Condition::TrackerIs("other.org".into()), false),
        ] {
            let rules = compiled(vec![rule(1, vec![condition], vec![Action::Stop])]);
            assert_eq!(fires(&rules, &m), expected);
        }
    }

    #[test]
    fn name_matches_case_insensitive() {
        let m = base_metrics(); // name "Ubuntu 24.04 ISO"
        let matching = rule(
            1,
            vec![Condition::NameMatches("ubuntu.*iso".into())],
            vec![Action::Stop],
        );
        assert!(fires(&compiled(vec![matching]), &m));
        let other = rule(
            1,
            vec![Condition::NameMatches("debian".into())],
            vec![Action::Stop],
        );
        assert!(!fires(&compiled(vec![other]), &m));
    }

    #[test]
    fn added_by_rss_condition() {
        let mut m = base_metrics();
        let rules = compiled(vec![rule(
            1,
            vec![Condition::AddedByRss],
            vec![Action::Stop],
        )]);
        assert!(!fires(&rules, &m));
        m.rss = Some(RssProvenance {
            feed_id: 1,
            rule_id: None,
            item_id: 3,
        });
        assert!(fires(&rules, &m));
    }

    #[test]
    fn swarm_conditions_fail_without_data() {
        let mut m = base_metrics();
        m.seeds = None;
        m.leechers = None;
        // Even trivially-true bounds must fail: the engine never guesses.
        for condition in [
            Condition::SeedsAtLeast(0),
            Condition::SeedsAtMost(u32::MAX),
            Condition::LeechersAtLeast(0),
        ] {
            let rules = compiled(vec![rule(1, vec![condition.clone()], vec![Action::Stop])]);
            assert!(
                !fires(&rules, &m),
                "no scrape data must fail: {condition:?}"
            );
        }
    }

    #[test]
    fn swarm_conditions_compare_when_data_present() {
        let mut m = base_metrics();
        m.seeds = Some(5);
        m.leechers = Some(3);
        for (condition, expected) in [
            (Condition::SeedsAtLeast(5), true),
            (Condition::SeedsAtLeast(6), false),
            (Condition::SeedsAtMost(5), true),
            (Condition::SeedsAtMost(4), false),
            (Condition::LeechersAtLeast(3), true),
            (Condition::LeechersAtLeast(4), false),
        ] {
            let rules = compiled(vec![rule(1, vec![condition.clone()], vec![Action::Stop])]);
            assert_eq!(fires(&rules, &m), expected, "{condition:?}");
        }
    }

    #[test]
    fn free_space_below_dir_and_path() {
        let mut m = base_metrics();
        m.dir_free_bytes = Some(500);
        m.path_free_bytes.insert("/downloads".into(), 100);
        for (condition, expected) in [
            (
                Condition::FreeSpaceBelow {
                    bytes: 1000,
                    path: None,
                },
                true,
            ),
            // Strict `<`: equal free space does not fire.
            (
                Condition::FreeSpaceBelow {
                    bytes: 500,
                    path: None,
                },
                false,
            ),
            (
                Condition::FreeSpaceBelow {
                    bytes: 200,
                    path: Some("/downloads".into()),
                },
                true,
            ),
            (
                Condition::FreeSpaceBelow {
                    bytes: 100,
                    path: Some("/downloads".into()),
                },
                false,
            ),
            // Unknown path: no data, no match.
            (
                Condition::FreeSpaceBelow {
                    bytes: u64::MAX,
                    path: Some("/missing".into()),
                },
                false,
            ),
        ] {
            let rules = compiled(vec![rule(1, vec![condition.clone()], vec![Action::Stop])]);
            assert_eq!(fires(&rules, &m), expected, "{condition:?}");
        }
    }

    #[test]
    fn free_space_without_data_is_false() {
        let mut m = base_metrics();
        m.dir_free_bytes = None;
        m.path_free_bytes = HashMap::new();
        for condition in [
            Condition::FreeSpaceBelow {
                bytes: u64::MAX,
                path: None,
            },
            Condition::FreeSpaceBelow {
                bytes: u64::MAX,
                path: Some("/x".into()),
            },
        ] {
            let rules = compiled(vec![rule(1, vec![condition], vec![Action::Stop])]);
            assert!(!fires(&rules, &m));
        }
    }

    #[test]
    fn decision_reason_lists_satisfied_conditions() {
        let m = base_metrics(); // ratio 2.5, complete
        let any = Rule {
            cond_mode: CondMode::Any,
            ..rule(
                1,
                vec![Condition::RatioAtLeast(2.0), Condition::RatioBelow(1.0)],
                vec![Action::Stop],
            )
        };
        let decision = evaluate_pass(
            &compiled(vec![any]),
            &m,
            1_000,
            &RuleLedger::default(),
            None,
        )
        .expect("must match");
        assert_eq!(decision.reason, "ratio 2.50 >= 2.00");

        let all = rule(
            1,
            vec![Condition::Complete(true), Condition::RatioAtLeast(2.0)],
            vec![Action::Stop],
        );
        let decision = evaluate_pass(
            &compiled(vec![all]),
            &m,
            1_000,
            &RuleLedger::default(),
            None,
        )
        .expect("must match");
        assert_eq!(decision.reason, "complete == true; ratio 2.50 >= 2.00");
    }

    #[test]
    fn policy_without_active_limits_returns_none() {
        let limits = effective_limits(None, None, None, LimitMode::Any, SeedAction::Stop);
        assert!(policy_decision(&limits, &base_metrics()).is_none());
    }

    #[test]
    fn policy_ratio_any_fires_with_reason() {
        let limits = effective_limits(Some(2.0), None, None, LimitMode::Any, SeedAction::Stop);
        let decision = policy_decision(&limits, &base_metrics()).expect("ratio reached");
        assert_eq!(decision.rule_id, POLICY_RULE_ID);
        assert_eq!(decision.actions, vec![Action::Stop]);
        assert!(decision.reason.contains("ratio"));
    }

    #[test]
    fn policy_reason_formats_ratio() {
        let limits = effective_limits(Some(2.0), None, None, LimitMode::Any, SeedAction::Stop);
        let decision = policy_decision(&limits, &base_metrics()).expect("ratio reached");
        assert_eq!(decision.reason, "ratio 2.50 >= 2.00");
    }

    #[test]
    fn policy_ratio_base_zero_is_not_reached() {
        let mut m = base_metrics();
        m.completed = 0;
        m.downloaded = 0;
        m.is_complete = false;
        m.total_size = 100;
        m.ratio = 99.0; // even an absurd ratio cannot fire without a base
        let limits = effective_limits(Some(1.0), None, None, LimitMode::Any, SeedAction::Stop);
        assert!(policy_decision(&limits, &m).is_none());
    }

    #[test]
    fn policy_ratio_base_zero_even_when_complete_with_zero_size() {
        let mut m = base_metrics();
        m.completed = 0;
        m.downloaded = 0;
        m.is_complete = true;
        m.total_size = 0;
        m.ratio = 5.0;
        let limits = effective_limits(Some(1.0), None, None, LimitMode::Any, SeedAction::Stop);
        assert!(policy_decision(&limits, &m).is_none());
    }

    #[test]
    fn policy_ratio_base_fallback_downloaded() {
        let mut m = base_metrics();
        m.completed = 0;
        m.downloaded = 100;
        m.ratio = 2.0;
        let limits = effective_limits(Some(2.0), None, None, LimitMode::Any, SeedAction::Stop);
        assert!(policy_decision(&limits, &m).is_some());
    }

    #[test]
    fn policy_ratio_base_fallback_total_size_when_complete() {
        let mut m = base_metrics();
        m.completed = 0;
        m.downloaded = 0;
        m.is_complete = true;
        m.total_size = 100;
        m.ratio = 1.0;
        let limits = effective_limits(Some(1.0), None, None, LimitMode::Any, SeedAction::Stop);
        assert!(policy_decision(&limits, &m).is_some());
    }

    #[test]
    fn policy_mode_any_fires_on_one_reached_limit() {
        let m = base_metrics(); // ratio 2.5, seeding 3600
        let limits = effective_limits(
            Some(3.0),
            Some(3600),
            None,
            LimitMode::Any,
            SeedAction::Stop,
        );
        let decision = policy_decision(&limits, &m).expect("seeding time reached");
        assert!(decision.reason.contains("seeding_time"));
        assert!(!decision.reason.contains("ratio"));
    }

    #[test]
    fn policy_mode_all_requires_every_active_limit() {
        let m = base_metrics(); // ratio 2.5 < 3.0, seeding 3600 reached
        let limits = effective_limits(
            Some(3.0),
            Some(3600),
            None,
            LimitMode::All,
            SeedAction::Stop,
        );
        assert!(policy_decision(&limits, &m).is_none());
    }

    #[test]
    fn policy_mode_all_fires_when_all_reached() {
        let m = base_metrics(); // ratio 2.5, seeding 3600, idle 600
        let limits = effective_limits(
            Some(2.0),
            Some(3600),
            Some(600),
            LimitMode::All,
            SeedAction::Stop,
        );
        let decision = policy_decision(&limits, &m).expect("all limits reached");
        assert!(decision.reason.contains("ratio"));
        assert!(decision.reason.contains("seeding_time"));
        assert!(decision.reason.contains("idle"));
    }

    #[test]
    fn policy_time_limits_boundary() {
        let m = base_metrics(); // seeding 3600, idle 600
        let seeding = effective_limits(None, Some(3600), None, LimitMode::Any, SeedAction::Stop);
        assert!(policy_decision(&seeding, &m).is_some());
        let seeding_over =
            effective_limits(None, Some(3601), None, LimitMode::Any, SeedAction::Stop);
        assert!(policy_decision(&seeding_over, &m).is_none());
        let idle = effective_limits(None, None, Some(600), LimitMode::Any, SeedAction::Stop);
        assert!(policy_decision(&idle, &m).is_some());
        let idle_over = effective_limits(None, None, Some(601), LimitMode::Any, SeedAction::Stop);
        assert!(policy_decision(&idle_over, &m).is_none());
    }

    #[test]
    fn policy_maps_every_seed_action() {
        let cases = [
            (SeedAction::Stop, Action::Stop),
            (SeedAction::Close, Action::Close),
            (SeedAction::Remove, Action::Remove),
            (SeedAction::RemoveWithData, Action::RemoveWithData),
            (
                SeedAction::Throttle("slow".into()),
                Action::SetThrottle(Some("slow".into())),
            ),
            (SeedAction::SuperSeed, Action::SuperSeed(true)),
            (SeedAction::Notify, Action::Noop),
            (
                SeedAction::Command("d.stop=".into()),
                Action::Command("d.stop=".into()),
            ),
        ];
        for (seed_action, expected) in cases {
            // seeding_time limit 0 is always reached by the fixture.
            let limits = effective_limits(None, Some(0), None, LimitMode::Any, seed_action);
            let decision = policy_decision(&limits, &base_metrics()).expect("limit reached");
            assert_eq!(decision.rule_id, POLICY_RULE_ID);
            assert_eq!(decision.actions, vec![expected]);
        }
    }

    #[test]
    fn dry_run_returns_matches_with_cloned_metrics() {
        let m1 = base_metrics();
        let mut m2 = base_metrics();
        m2.hash = "hash-2".into();
        m2.is_complete = false;
        let rules = compiled(vec![matching_rule(1)]);
        let result = dry_run(&rules, &[m1.clone(), m2], 1_000, &HashMap::new());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, m1);
        assert_eq!(result[0].1.rule_id, 1);
    }

    #[test]
    fn dry_run_never_mutates_ledgers() {
        let mut m1 = base_metrics();
        m1.hash = "a".into();
        let mut m2 = base_metrics();
        m2.hash = "b".into();
        let rules = compiled(vec![Rule {
            once: true,
            ..matching_rule(1)
        }]);
        let mut ledgers: HashMap<String, RuleLedger> = HashMap::new();
        // "a" already fired once → blocked; "b" has no ledger → would fire.
        ledgers.insert("a".into(), ledger_with(1, 500));
        let before = ledgers.clone();
        let result = dry_run(&rules, &[m1, m2], 1_000, &ledgers);
        assert_eq!(ledgers, before, "dry-run must not touch ledgers");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0.hash, "b");
    }

    #[test]
    fn dry_run_uses_periodic_trigger_only() {
        let event_rule = Rule {
            trigger: Trigger::Event(EventKind::Finished),
            ..matching_rule(1)
        };
        let rules = compiled(vec![event_rule]);
        assert!(dry_run(&rules, &[base_metrics()], 1_000, &HashMap::new()).is_empty());
    }

    #[test]
    fn dry_run_respects_ignore_commands() {
        let mut m = base_metrics();
        m.ignore_commands = true;
        let rules = compiled(vec![matching_rule(1)]);
        assert!(dry_run(&rules, &[m], 1_000, &HashMap::new()).is_empty());
    }
}
