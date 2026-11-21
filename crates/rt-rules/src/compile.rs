//! Rule validation and compilation (plan.md §6.1).
//!
//! `compile` validates structural invariants (non-empty conditions/actions,
//! webhook URL scheme, `NameMatches` regexes) and pre-compiles regexes once so
//! the per-tick evaluator never re-parses them.

use regex::Regex;

use crate::error::RuleError;
use crate::model::{Action, Condition, Rule};

/// A validated rule with pre-compiled matchers. Runtime-only (not serializable).
#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub rule: Rule,
    /// Pre-compiled `Condition::NameMatches` regexes (case-insensitive),
    /// in condition order.
    pub name_regexes: Vec<Regex>,
}

/// Validate and compile rules for evaluation. Fails on the first invalid rule.
///
/// The result is sorted by `(priority, id)` so the evaluator can apply
/// first-match-wins without re-sorting. Disabled rules are kept — the
/// evaluator skips them, which keeps indices stable for the store.
pub fn compile(rules: &[Rule]) -> Result<Vec<CompiledRule>, RuleError> {
    let mut compiled = rules
        .iter()
        .map(|rule| {
            if rule.conditions.is_empty() {
                return Err(RuleError::EmptyConditions(rule.id));
            }
            if rule.actions.is_empty() {
                return Err(RuleError::EmptyActions(rule.id));
            }
            for action in &rule.actions {
                if let Action::Webhook { url } = action {
                    validate_webhook_url(rule.id, url)?;
                }
            }
            let name_regexes = rule
                .conditions
                .iter()
                .filter_map(|condition| match condition {
                    Condition::NameMatches(pattern) => Some(compile_name_regex(rule.id, pattern)),
                    _ => None,
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CompiledRule {
                rule: rule.clone(),
                name_regexes,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    compiled.sort_unstable_by_key(|entry| (entry.rule.priority, entry.rule.id));
    Ok(compiled)
}

/// `NameMatches` is case-insensitive by contract: prefix the user pattern
/// with the `(?i)` flag and compile once.
fn compile_name_regex(rule_id: u64, pattern: &str) -> Result<Regex, RuleError> {
    Regex::new(&format!("(?i){pattern}"))
        .map_err(|source| RuleError::InvalidRegex { rule_id, source })
}

/// Webhooks may only target `http://` or `https://` with a non-empty host
/// (plan D14). Handles userinfo, ports and bracketed IPv6 literals.
fn validate_webhook_url(rule_id: u64, url: &str) -> Result<(), RuleError> {
    let invalid = || RuleError::InvalidUrl(rule_id, url.to_owned());
    let (scheme, rest) = url.split_once("://").ok_or_else(invalid)?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(invalid());
    }
    // Authority ends at the first path/query/fragment delimiter.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // Strip optional userinfo ("user:pass@").
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = match host_port.strip_prefix('[') {
        // IPv6 literal: "[::1]" or "[::1]:8080".
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => host_port.split(':').next().unwrap_or_default(),
    };
    if host.is_empty() {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CondMode, Scope, Trigger};

    fn rule(id: u64) -> Rule {
        Rule {
            id,
            name: format!("rule-{id}"),
            enabled: true,
            priority: 0,
            scope: Scope::Global,
            trigger: Trigger::Periodic,
            cond_mode: CondMode::All,
            conditions: vec![Condition::Complete(true)],
            actions: vec![Action::Stop],
            once: false,
            cooldown_secs: None,
            allow_destructive: false,
        }
    }

    #[test]
    fn empty_input_compiles_to_empty() {
        assert!(compile(&[]).unwrap().is_empty());
    }

    #[test]
    fn empty_conditions_rejected() {
        let mut r = rule(7);
        r.conditions = vec![];
        assert!(matches!(compile(&[r]), Err(RuleError::EmptyConditions(7))));
    }

    #[test]
    fn empty_actions_rejected() {
        let mut r = rule(7);
        r.actions = vec![];
        assert!(matches!(compile(&[r]), Err(RuleError::EmptyActions(7))));
    }

    #[test]
    fn invalid_regex_rejected_with_rule_id() {
        let mut r = rule(7);
        r.conditions = vec![Condition::NameMatches("(unclosed".into())];
        match compile(&[r]) {
            Err(RuleError::InvalidRegex { rule_id, .. }) => assert_eq!(rule_id, 7),
            other => panic!("expected InvalidRegex, got {other:?}"),
        }
    }

    #[test]
    fn name_regex_is_case_insensitive() {
        let mut r = rule(1);
        r.conditions = vec![Condition::NameMatches("UBUNTU.*ISO".into())];
        let compiled = compile(&[r]).unwrap();
        assert_eq!(compiled[0].name_regexes.len(), 1);
        assert!(compiled[0].name_regexes[0].is_match("ubuntu-24.04.iso"));
        assert!(!compiled[0].name_regexes[0].is_match("debian.iso"));
    }

    #[test]
    fn name_regexes_follow_condition_order() {
        let mut r = rule(1);
        r.conditions = vec![
            Condition::Complete(true),
            Condition::NameMatches("alpha".into()),
            Condition::SizeAtLeast(1),
            Condition::NameMatches("beta".into()),
        ];
        let compiled = compile(&[r]).unwrap();
        let regexes = &compiled[0].name_regexes;
        assert_eq!(regexes.len(), 2);
        assert!(regexes[0].is_match("ALPHA"));
        assert!(!regexes[0].is_match("beta"));
        assert!(regexes[1].is_match("BETA"));
        assert!(!regexes[1].is_match("alpha"));
    }

    #[test]
    fn sorted_by_priority_then_id() {
        let mut r1 = rule(1);
        r1.priority = 5;
        let mut r2 = rule(2);
        r2.priority = -1;
        let r3 = rule(3);
        let r4 = rule(4);
        let compiled = compile(&[r1, r2, r3, r4]).unwrap();
        let ids: Vec<u64> = compiled.iter().map(|c| c.rule.id).collect();
        assert_eq!(ids, vec![2, 3, 4, 1]);
    }

    #[test]
    fn disabled_rules_are_kept() {
        let mut r = rule(1);
        r.enabled = false;
        let compiled = compile(&[r]).unwrap();
        assert_eq!(compiled.len(), 1);
        assert!(!compiled[0].rule.enabled);
    }

    #[test]
    fn webhook_http_and_https_accepted() {
        for url in [
            "http://example.org/hook",
            "https://example.org/hook",
            "https://example.org",
            "https://example.org:8443/hook#frag",
            "http://localhost:9/hook",
        ] {
            let mut r = rule(1);
            r.actions = vec![Action::Webhook { url: url.into() }];
            assert!(compile(&[r]).is_ok(), "should accept {url}");
        }
    }

    #[test]
    fn webhook_uppercase_scheme_accepted() {
        let mut r = rule(1);
        r.actions = vec![Action::Webhook {
            url: "HTTP://example.org/hook".into(),
        }];
        assert!(compile(&[r]).is_ok());
    }

    #[test]
    fn webhook_other_schemes_rejected() {
        for url in [
            "ftp://example.org/hook",
            "file:///etc/passwd",
            "example.org/hook",
            "",
            "http:/example.org",
        ] {
            let mut r = rule(3);
            r.actions = vec![Action::Webhook { url: url.into() }];
            match compile(&[r]) {
                Err(RuleError::InvalidUrl(rule_id, bad)) => {
                    assert_eq!(rule_id, 3);
                    assert_eq!(bad, url);
                }
                other => panic!("expected InvalidUrl for {url:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn webhook_empty_host_rejected() {
        for url in [
            "http://",
            "https:///path",
            "http://?q=1",
            "http://:8080/hook",
            "http://user@/hook",
            "https://[]:9/hook",
        ] {
            let mut r = rule(1);
            r.actions = vec![Action::Webhook { url: url.into() }];
            assert!(
                matches!(compile(&[r]), Err(RuleError::InvalidUrl(1, _))),
                "should reject {url}"
            );
        }
    }

    #[test]
    fn webhook_userinfo_port_and_ipv6_accepted() {
        for url in [
            "http://user:pass@example.org:8080/hook",
            "https://[::1]:9/hook",
            "https://[2001:db8::1]/hook",
        ] {
            let mut r = rule(1);
            r.actions = vec![Action::Webhook { url: url.into() }];
            assert!(compile(&[r]).is_ok(), "should accept {url}");
        }
    }

    #[test]
    fn first_invalid_rule_aborts_compilation() {
        let valid = rule(1);
        let mut invalid = rule(2);
        invalid.conditions = vec![];
        assert!(matches!(
            compile(&[valid, invalid]),
            Err(RuleError::EmptyConditions(2))
        ));
    }
}
