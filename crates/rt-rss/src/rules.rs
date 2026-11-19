//! Rule engine: compiles rule regexes, evaluates rules against feed items
//! (scope, exclude/match title, size, episode range, cooldown) and selects
//! the download action for the winning rule.
//!
//! Evaluation order in [`matches`] is binding: enabled, feed scope,
//! `exclude_title` (hit rejects), `match_title` (must hit when set), size,
//! episode, cooldown. Season packs (`S01` without an episode number) are
//! intentionally invisible to episode filters: [`parse_episode`] returns
//! `None` for them.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use crate::error::RssError;
use crate::model::{DownloadAction, EpisodeFilter, FeedId, FeedItem, MatchedItem, RssRule, RuleId};

/// A rule with its title regexes compiled once, plus copies of the fields
/// needed for matching. Build with [`compile`].
#[derive(Debug, Clone)]
pub struct CompiledRule {
    enabled: bool,
    feed_ids: Vec<FeedId>,
    match_title: Option<Regex>,
    exclude_title: Option<Regex>,
    min_size_bytes: Option<u64>,
    max_size_bytes: Option<u64>,
    episode: Option<EpisodeFilter>,
    cooldown_secs: Option<u64>,
}

/// Compiles a rule's regexes case-insensitively. An invalid pattern yields
/// [`RssError::InvalidRule`] naming the rule and the offending field.
pub fn compile(rule: &RssRule) -> Result<CompiledRule, RssError> {
    Ok(CompiledRule {
        enabled: rule.enabled,
        feed_ids: rule.feed_ids.clone(),
        match_title: compile_pattern(rule.match_title.as_deref(), "match_title", rule.id)?,
        exclude_title: compile_pattern(rule.exclude_title.as_deref(), "exclude_title", rule.id)?,
        min_size_bytes: rule.min_size_bytes,
        max_size_bytes: rule.max_size_bytes,
        episode: rule.episode.clone(),
        cooldown_secs: rule.cooldown_secs,
    })
}

fn compile_pattern(
    pattern: Option<&str>,
    field: &str,
    rule_id: RuleId,
) -> Result<Option<Regex>, RssError> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map(Some)
        .map_err(|err| {
            RssError::InvalidRule(format!("rule {rule_id}: invalid {field} regex: {err}"))
        })
}

/// Derives the download parameters from a rule.
pub fn select_action(rule: &RssRule) -> DownloadAction {
    DownloadAction {
        save_path: rule.save_path.clone(),
        label: rule.label.clone(),
        paused: rule.add_paused,
    }
}

// Static patterns are known-valid; `Option` keeps the initializers panic-free.
fn static_regex(pattern: &str) -> Option<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .ok()
}

static SXXEXX_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| static_regex(r"S(\d{1,2})\s*\.?\s*E(\d{1,3})"));
static NXNN_RE: LazyLock<Option<Regex>> = LazyLock::new(|| static_regex(r"(\d{1,2})x(\d{1,3})"));
// "Standalone" E: a leading word boundary keeps it from firing inside SxxExx
// or glued to other word characters; no trailing boundary so `E05v2` still
// yields episode 5.
static STANDALONE_E_RE: LazyLock<Option<Regex>> = LazyLock::new(|| static_regex(r"\bE(\d{1,3})"));
static EPISODE_WORD_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| static_regex(r"\bEpisode\s+(\d{1,3})\b"));

/// Extracts `(season, episode)` from a release title, case-insensitively.
/// Tries `S01E02` (optionally dot/space separated), then `1x02`, then a
/// standalone `E05` and `Episode 7` (both season 1). Season packs and titles
/// without a recognizable episode return `None`.
pub fn parse_episode(title: &str) -> Option<(u32, u32)> {
    if let Some(pair) = capture_pair(SXXEXX_RE.as_ref(), title) {
        return Some(pair);
    }
    if let Some(pair) = capture_pair(NXNN_RE.as_ref(), title) {
        return Some(pair);
    }
    if let Some(episode) = capture_single(STANDALONE_E_RE.as_ref(), title) {
        return Some((1, episode));
    }
    capture_single(EPISODE_WORD_RE.as_ref(), title).map(|episode| (1, episode))
}

fn capture_pair(re: Option<&Regex>, title: &str) -> Option<(u32, u32)> {
    let caps = re?.captures(title)?;
    // `\d` also matches non-ASCII digits, which `u32::from_str` rejects;
    // treat those as "no episode" instead of panicking.
    Some((
        caps.get(1)?.as_str().parse().ok()?,
        caps.get(2)?.as_str().parse().ok()?,
    ))
}

fn capture_single(re: Option<&Regex>, title: &str) -> Option<u32> {
    re?.captures(title)?.get(1)?.as_str().parse().ok()
}

/// Evaluates a compiled rule against an item. `last_rule_download` is the
/// Unix time of the rule's most recent download, if any.
pub fn matches(
    compiled: &CompiledRule,
    item: &FeedItem,
    now: u64,
    last_rule_download: Option<u64>,
) -> bool {
    if !compiled.enabled {
        return false;
    }
    if !compiled.feed_ids.is_empty() && !compiled.feed_ids.contains(&item.feed_id) {
        return false;
    }
    if let Some(re) = &compiled.exclude_title {
        if re.is_match(&item.title) {
            return false;
        }
    }
    if let Some(re) = &compiled.match_title {
        if !re.is_match(&item.title) {
            return false;
        }
    }
    if compiled.min_size_bytes.is_some() || compiled.max_size_bytes.is_some() {
        // A size filter cannot be satisfied by an item of unknown size.
        let Some(size) = item.size_bytes else {
            return false;
        };
        if let Some(min) = compiled.min_size_bytes {
            if size < min {
                return false;
            }
        }
        if let Some(max) = compiled.max_size_bytes {
            if size > max {
                return false;
            }
        }
    }
    if let Some(filter) = &compiled.episode {
        let Some((season, episode)) = parse_episode(&item.title) else {
            return false;
        };
        if let Some(wanted) = filter.season {
            if season != wanted {
                return false;
            }
        }
        if !(filter.from_episode..=filter.to_episode).contains(&episode) {
            return false;
        }
    }
    if let Some(cooldown) = compiled.cooldown_secs {
        if let Some(last) = last_rule_download {
            // Saturating: a future `last` (clock skew) must not underflow.
            if now.saturating_sub(last) < cooldown {
                return false;
            }
        }
    }
    true
}

/// Picks the first matching rule in ascending `(priority, id)` order and
/// pairs the item with that rule's action. Rules are compiled on the fly
/// (rule sets are small); a rule that fails to compile is skipped with a
/// warning rather than aborting the whole evaluation.
pub fn best_match(
    rules: &[RssRule],
    item: &FeedItem,
    now: u64,
    cooldowns: &HashMap<RuleId, u64>,
) -> Option<MatchedItem> {
    let mut ordered: Vec<&RssRule> = rules.iter().collect();
    ordered.sort_by_key(|rule| (rule.priority, rule.id));
    for rule in ordered {
        let compiled = match compile(rule) {
            Ok(compiled) => compiled,
            Err(err) => {
                tracing::warn!(rule_id = rule.id, error = %err, "Skipping RSS rule with invalid regex");
                continue;
            }
        };
        let last_download = cooldowns.get(&rule.id).copied();
        if matches(&compiled, item, now, last_download) {
            return Some(MatchedItem {
                item: item.clone(),
                rule_id: rule.id,
                action: select_action(rule),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 10_000;

    fn base_rule() -> RssRule {
        RssRule {
            id: 1,
            name: "test rule".to_string(),
            enabled: true,
            feed_ids: Vec::new(),
            priority: 0,
            match_title: None,
            exclude_title: None,
            min_size_bytes: None,
            max_size_bytes: None,
            episode: None,
            save_path: None,
            label: None,
            add_paused: false,
            cooldown_secs: None,
        }
    }

    fn item_with_title(title: &str) -> FeedItem {
        FeedItem {
            id: 100,
            feed_id: 1,
            guid: "guid-100".to_string(),
            title: title.to_string(),
            link: None,
            torrent_url: Some("https://example.com/file.torrent".to_string()),
            magnet: None,
            info_hash: None,
            size_bytes: Some(1_000_000_000),
            published_at: None,
            fetched_at: 1_000,
            download: None,
        }
    }

    fn matches_rule(rule: &RssRule, item: &FeedItem) -> bool {
        matches(&compile(rule).unwrap(), item, NOW, None)
    }

    #[test]
    fn match_title_hit() {
        let rule = RssRule {
            match_title: Some("show.*1080p".to_string()),
            ..base_rule()
        };
        assert!(matches_rule(
            &rule,
            &item_with_title("My Show S01E02 1080p")
        ));
    }

    #[test]
    fn match_title_miss() {
        let rule = RssRule {
            match_title: Some("show.*1080p".to_string()),
            ..base_rule()
        };
        assert!(!matches_rule(&rule, &item_with_title("Other Movie 720p")));
    }

    #[test]
    fn match_title_case_insensitive() {
        let rule = RssRule {
            match_title: Some("SHOW".to_string()),
            ..base_rule()
        };
        assert!(matches_rule(&rule, &item_with_title("my show 1080p")));
        let rule = RssRule {
            match_title: Some("show".to_string()),
            ..base_rule()
        };
        assert!(matches_rule(&rule, &item_with_title("MY SHOW 1080p")));
    }

    #[test]
    fn exclude_overrides_match() {
        let rule = RssRule {
            match_title: Some("show".to_string()),
            exclude_title: Some("cam|telesync".to_string()),
            ..base_rule()
        };
        assert!(!matches_rule(&rule, &item_with_title("Show CAM 1080p")));
        assert!(matches_rule(&rule, &item_with_title("Show 1080p")));
    }

    #[test]
    fn exclude_case_insensitive() {
        let rule = RssRule {
            exclude_title: Some("CAM".to_string()),
            ..base_rule()
        };
        assert!(!matches_rule(&rule, &item_with_title("show cam rip")));
    }

    #[test]
    fn no_filters_matches_any_item() {
        assert!(matches_rule(
            &base_rule(),
            &item_with_title("Anything at all")
        ));
    }

    #[test]
    fn size_within_bounds() {
        let rule = RssRule {
            min_size_bytes: Some(500_000_000),
            max_size_bytes: Some(2_000_000_000),
            ..base_rule()
        };
        assert!(matches_rule(&rule, &item_with_title("Show 1080p")));
    }

    #[test]
    fn size_boundaries_inclusive() {
        let rule = RssRule {
            min_size_bytes: Some(1_000),
            max_size_bytes: Some(2_000),
            ..base_rule()
        };
        let mut item = item_with_title("Show");
        item.size_bytes = Some(1_000);
        assert!(matches_rule(&rule, &item));
        item.size_bytes = Some(2_000);
        assert!(matches_rule(&rule, &item));
    }

    #[test]
    fn size_below_min_rejected() {
        let rule = RssRule {
            min_size_bytes: Some(2_000_000_000),
            ..base_rule()
        };
        assert!(!matches_rule(&rule, &item_with_title("Show")));
    }

    #[test]
    fn size_above_max_rejected() {
        let rule = RssRule {
            max_size_bytes: Some(500_000_000),
            ..base_rule()
        };
        assert!(!matches_rule(&rule, &item_with_title("Show")));
    }

    #[test]
    fn size_filter_rejects_unknown_size() {
        let mut item = item_with_title("Show");
        item.size_bytes = None;
        let min_only = RssRule {
            min_size_bytes: Some(1),
            ..base_rule()
        };
        assert!(!matches_rule(&min_only, &item));
        let max_only = RssRule {
            max_size_bytes: Some(u64::MAX),
            ..base_rule()
        };
        assert!(!matches_rule(&max_only, &item));
    }

    #[test]
    fn no_size_filter_allows_unknown_size() {
        let mut item = item_with_title("Show");
        item.size_bytes = None;
        assert!(matches_rule(&base_rule(), &item));
    }

    #[test]
    fn parse_episode_sxxexx_variants() {
        assert_eq!(parse_episode("Show S01E02 1080p"), Some((1, 2)));
        assert_eq!(parse_episode("Show s12e123"), Some((12, 123)));
        assert_eq!(parse_episode("Show S01.E02"), Some((1, 2)));
        assert_eq!(parse_episode("Show S01 . E02"), Some((1, 2)));
        assert_eq!(parse_episode("Show S01 E02"), Some((1, 2)));
    }

    #[test]
    fn parse_episode_nxnn() {
        assert_eq!(parse_episode("Show 1x02"), Some((1, 2)));
        assert_eq!(parse_episode("Show 12x123"), Some((12, 123)));
    }

    #[test]
    fn parse_episode_standalone_e() {
        assert_eq!(parse_episode("Show E05"), Some((1, 5)));
        assert_eq!(parse_episode("Show e7 1080p"), Some((1, 7)));
        assert_eq!(parse_episode("Show.E05.1080p"), Some((1, 5)));
    }

    #[test]
    fn parse_episode_episode_word() {
        assert_eq!(parse_episode("Show Episode 7"), Some((1, 7)));
        assert_eq!(parse_episode("Show episode 123"), Some((1, 123)));
        assert_eq!(parse_episode("Show EPISODE 4"), Some((1, 4)));
    }

    #[test]
    fn parse_episode_season_pack_is_none() {
        assert_eq!(parse_episode("Show S01 1080p"), None);
        assert_eq!(parse_episode("Show S02 COMPLETE"), None);
    }

    #[test]
    fn parse_episode_weird_titles_do_not_panic() {
        assert_eq!(parse_episode(""), None);
        assert_eq!(parse_episode("   "), None);
        assert_eq!(parse_episode("żółć 日本語 🎬"), None);
        assert_eq!(parse_episode("S"), None);
        assert_eq!(parse_episode("E"), None);
        assert_eq!(parse_episode("Episode"), None);
        // Non-ASCII digits match `\d` but cannot parse as u32.
        assert_eq!(parse_episode("S٠١E٠٢"), None);
        assert_eq!(parse_episode("S99E999"), Some((99, 999)));
    }

    #[test]
    fn episode_filter_range_inclusive() {
        let rule = RssRule {
            episode: Some(EpisodeFilter {
                season: Some(1),
                from_episode: 2,
                to_episode: 5,
            }),
            ..base_rule()
        };
        assert!(matches_rule(&rule, &item_with_title("Show S01E02")));
        assert!(matches_rule(&rule, &item_with_title("Show S01E05")));
        assert!(!matches_rule(&rule, &item_with_title("Show S01E01")));
        assert!(!matches_rule(&rule, &item_with_title("Show S01E06")));
    }

    #[test]
    fn episode_filter_season_mismatch() {
        let rule = RssRule {
            episode: Some(EpisodeFilter {
                season: Some(2),
                from_episode: 1,
                to_episode: 12,
            }),
            ..base_rule()
        };
        assert!(!matches_rule(&rule, &item_with_title("Show S01E03")));
        assert!(matches_rule(&rule, &item_with_title("Show S02E03")));
    }

    #[test]
    fn episode_filter_any_season_when_none() {
        let rule = RssRule {
            episode: Some(EpisodeFilter {
                season: None,
                from_episode: 1,
                to_episode: 10,
            }),
            ..base_rule()
        };
        assert!(matches_rule(&rule, &item_with_title("Show S03E04")));
    }

    #[test]
    fn episode_filter_rejects_titles_without_episode() {
        let rule = RssRule {
            episode: Some(EpisodeFilter {
                season: None,
                from_episode: 1,
                to_episode: 100,
            }),
            ..base_rule()
        };
        assert!(!matches_rule(&rule, &item_with_title("Show 1080p")));
        assert!(!matches_rule(&rule, &item_with_title("Show S01")));
    }

    #[test]
    fn cooldown_blocks_recent_download() {
        let rule = RssRule {
            cooldown_secs: Some(3600),
            ..base_rule()
        };
        let compiled = compile(&rule).unwrap();
        let item = item_with_title("Show");
        assert!(!matches(&compiled, &item, NOW, Some(NOW - 1_000)));
    }

    #[test]
    fn cooldown_allows_after_interval() {
        let rule = RssRule {
            cooldown_secs: Some(3600),
            ..base_rule()
        };
        let compiled = compile(&rule).unwrap();
        let item = item_with_title("Show");
        // Exactly at the boundary is allowed.
        assert!(matches(&compiled, &item, NOW, Some(NOW - 3600)));
        assert!(matches(&compiled, &item, NOW, Some(NOW - 7200)));
    }

    #[test]
    fn cooldown_without_last_download() {
        let rule = RssRule {
            cooldown_secs: Some(3600),
            ..base_rule()
        };
        let compiled = compile(&rule).unwrap();
        assert!(matches(&compiled, &item_with_title("Show"), NOW, None));
    }

    #[test]
    fn cooldown_none_or_zero_allows() {
        let item = item_with_title("Show");
        let no_cooldown = compile(&base_rule()).unwrap();
        assert!(matches(&no_cooldown, &item, NOW, Some(NOW)));
        let zero_cooldown = RssRule {
            cooldown_secs: Some(0),
            ..base_rule()
        };
        let compiled = compile(&zero_cooldown).unwrap();
        assert!(matches(&compiled, &item, NOW, Some(NOW)));
    }

    #[test]
    fn cooldown_future_timestamp_does_not_panic() {
        let rule = RssRule {
            cooldown_secs: Some(3600),
            ..base_rule()
        };
        let compiled = compile(&rule).unwrap();
        let item = item_with_title("Show");
        assert!(!matches(&compiled, &item, 100, Some(200)));
    }

    #[test]
    fn feed_scope_empty_matches_all_feeds() {
        let mut item = item_with_title("Show");
        item.feed_id = 7;
        assert!(matches_rule(&base_rule(), &item));
    }

    #[test]
    fn feed_scope_restricts_to_listed_feeds() {
        let rule = RssRule {
            feed_ids: vec![1, 2],
            ..base_rule()
        };
        let mut item = item_with_title("Show");
        item.feed_id = 3;
        assert!(!matches_rule(&rule, &item));
        item.feed_id = 2;
        assert!(matches_rule(&rule, &item));
    }

    #[test]
    fn disabled_rule_never_matches() {
        let rule = RssRule {
            enabled: false,
            ..base_rule()
        };
        let item = item_with_title("Show");
        assert!(!matches_rule(&rule, &item));
        assert!(best_match(&[rule], &item, NOW, &HashMap::new()).is_none());
    }

    #[test]
    fn invalid_match_regex_returns_invalid_rule() {
        let rule = RssRule {
            match_title: Some("(unclosed".to_string()),
            ..base_rule()
        };
        let err = compile(&rule).unwrap_err();
        assert!(matches!(err, RssError::InvalidRule(_)));
        assert!(err.to_string().contains("match_title"));
    }

    #[test]
    fn invalid_exclude_regex_returns_invalid_rule() {
        let rule = RssRule {
            exclude_title: Some("[bad".to_string()),
            ..base_rule()
        };
        let err = compile(&rule).unwrap_err();
        assert!(matches!(err, RssError::InvalidRule(_)));
        assert!(err.to_string().contains("exclude_title"));
    }

    #[test]
    fn select_action_maps_rule_fields() {
        let rule = RssRule {
            save_path: Some("/downloads/show".to_string()),
            label: Some("show".to_string()),
            add_paused: true,
            ..base_rule()
        };
        let action = select_action(&rule);
        assert_eq!(action.save_path.as_deref(), Some("/downloads/show"));
        assert_eq!(action.label.as_deref(), Some("show"));
        assert!(action.paused);
    }

    #[test]
    fn select_action_defaults() {
        let action = select_action(&base_rule());
        assert_eq!(action.save_path, None);
        assert_eq!(action.label, None);
        assert!(!action.paused);
    }

    #[test]
    fn best_match_lower_priority_wins() {
        let high = RssRule {
            id: 1,
            priority: 5,
            match_title: Some("show".to_string()),
            ..base_rule()
        };
        let low = RssRule {
            id: 2,
            priority: 1,
            match_title: Some("show".to_string()),
            ..base_rule()
        };
        let item = item_with_title("Show 1080p");
        // Input order must not matter.
        let matched =
            best_match(&[high.clone(), low.clone()], &item, NOW, &HashMap::new()).unwrap();
        assert_eq!(matched.rule_id, 2);
        let matched = best_match(&[low, high], &item, NOW, &HashMap::new()).unwrap();
        assert_eq!(matched.rule_id, 2);
    }

    #[test]
    fn best_match_tie_breaks_by_id() {
        let later = RssRule {
            id: 3,
            priority: 0,
            match_title: Some("show".to_string()),
            ..base_rule()
        };
        let earlier = RssRule {
            id: 1,
            priority: 0,
            match_title: Some("show".to_string()),
            ..base_rule()
        };
        let item = item_with_title("Show 1080p");
        let matched = best_match(&[later, earlier], &item, NOW, &HashMap::new()).unwrap();
        assert_eq!(matched.rule_id, 1);
    }

    #[test]
    fn best_match_skips_invalid_rule() {
        let broken = RssRule {
            id: 1,
            priority: 0,
            match_title: Some("(unclosed".to_string()),
            ..base_rule()
        };
        let valid = RssRule {
            id: 2,
            priority: 1,
            match_title: Some("show".to_string()),
            ..base_rule()
        };
        let item = item_with_title("Show 1080p");
        let matched = best_match(&[broken, valid], &item, NOW, &HashMap::new()).unwrap();
        assert_eq!(matched.rule_id, 2);
    }

    #[test]
    fn best_match_respects_cooldowns() {
        let rule = RssRule {
            cooldown_secs: Some(3600),
            ..base_rule()
        };
        let item = item_with_title("Show");
        let recent = HashMap::from([(1, NOW - 100)]);
        assert!(best_match(std::slice::from_ref(&rule), &item, NOW, &recent).is_none());
        let old = HashMap::from([(1, NOW - 7200)]);
        assert!(best_match(std::slice::from_ref(&rule), &item, NOW, &old).is_some());
        assert!(best_match(&[rule], &item, NOW, &HashMap::new()).is_some());
    }

    #[test]
    fn best_match_returns_item_and_action() {
        let rule = RssRule {
            match_title: Some("show".to_string()),
            save_path: Some("/downloads/show".to_string()),
            label: Some("show".to_string()),
            add_paused: true,
            ..base_rule()
        };
        let item = item_with_title("Show S01E02 1080p");
        let matched = best_match(&[rule], &item, NOW, &HashMap::new()).unwrap();
        assert_eq!(matched.rule_id, 1);
        assert_eq!(matched.item.guid, item.guid);
        assert_eq!(matched.item.title, item.title);
        assert_eq!(matched.action.save_path.as_deref(), Some("/downloads/show"));
        assert_eq!(matched.action.label.as_deref(), Some("show"));
        assert!(matched.action.paused);
    }

    #[test]
    fn best_match_empty_rules_none() {
        let item = item_with_title("Show");
        assert!(best_match(&[], &item, NOW, &HashMap::new()).is_none());
    }
}
