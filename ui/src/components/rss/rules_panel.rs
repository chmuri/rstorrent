//! Rules tab: auto-download rule list, a full-field create/edit form and a
//! dry-run test against stored items. The list is rendered from
//! `RssPageStore::rules`; mutations bump `reload` so the page refetches.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::ui::{Badge, BadgeKind, BentoCard, Button, ButtonVariant};
use crate::state::rss::{
    rss_create_rule, rss_delete_rule, rss_test_rule, rss_update_rule, EpisodeFilter, FeedId,
    FeedItem, RssPageStore, RssRule, RuleId,
};
use crate::state::store::{AppState, ToastKind};

const INPUT_CLASS: &str = "w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all";

// Mirrors ButtonVariant::Secondary; the test button needs a QA id, which the
// shared Button component does not expose.
const SECONDARY_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-white/[0.04] hover:bg-white/[0.08] border border-white/[0.08] px-3.5 py-2 text-sm font-medium text-zinc-200 active:scale-[0.98] transition-all cursor-pointer";

const LABEL_CLASS: &str = "mb-1.5 block text-sm font-medium text-zinc-200";

const MB_BYTES: f64 = 1_048_576.0;

/// Form state for one rule. Numeric fields stay as strings so partially typed
/// values survive keystrokes; they are parsed (and validated) on save/test.
#[derive(Clone)]
struct RuleDraft {
    /// None = creating a new rule; Some(id) = editing that rule.
    id: Option<RuleId>,
    name: String,
    enabled: bool,
    feed_ids: Vec<FeedId>,
    priority: String,
    match_title: String,
    exclude_title: String,
    min_size_mb: String,
    max_size_mb: String,
    episode_enabled: bool,
    season: String,
    from_episode: String,
    to_episode: String,
    save_path: String,
    label: String,
    add_paused: bool,
    cooldown_secs: String,
}

impl Default for RuleDraft {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            enabled: true,
            feed_ids: Vec::new(),
            priority: "0".to_string(),
            match_title: String::new(),
            exclude_title: String::new(),
            min_size_mb: String::new(),
            max_size_mb: String::new(),
            episode_enabled: false,
            season: String::new(),
            from_episode: "1".to_string(),
            to_episode: "1".to_string(),
            save_path: String::new(),
            label: String::new(),
            add_paused: false,
            cooldown_secs: String::new(),
        }
    }
}

impl RuleDraft {
    fn from_rule(rule: &RssRule) -> Self {
        let (episode_enabled, season, from_episode, to_episode) = match &rule.episode {
            Some(filter) => (
                true,
                filter
                    .season
                    .map(|season| season.to_string())
                    .unwrap_or_default(),
                filter.from_episode.to_string(),
                filter.to_episode.to_string(),
            ),
            None => (false, String::new(), "1".to_string(), "1".to_string()),
        };
        Self {
            id: Some(rule.id),
            name: rule.name.clone(),
            enabled: rule.enabled,
            feed_ids: rule.feed_ids.clone(),
            priority: rule.priority.to_string(),
            match_title: rule.match_title.clone().unwrap_or_default(),
            exclude_title: rule.exclude_title.clone().unwrap_or_default(),
            min_size_mb: rule.min_size_bytes.map(bytes_to_mb).unwrap_or_default(),
            max_size_mb: rule.max_size_bytes.map(bytes_to_mb).unwrap_or_default(),
            episode_enabled,
            season,
            from_episode,
            to_episode,
            save_path: rule.save_path.clone().unwrap_or_default(),
            label: rule.label.clone().unwrap_or_default(),
            add_paused: rule.add_paused,
            cooldown_secs: rule
                .cooldown_secs
                .map(|secs| secs.to_string())
                .unwrap_or_default(),
        }
    }
}

/// Shortest round-tripping decimal so byte values that are not whole MB still
/// survive an edit round trip exactly (f64 is exact up to 2^53 bytes).
fn bytes_to_mb(bytes: u64) -> String {
    format!("{}", bytes as f64 / MB_BYTES)
}

fn parse_size_mb(text: &str, field: &str) -> Result<Option<u64>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let mb = trimmed
        .parse::<f64>()
        .map_err(|_| format!("{field} must be a number of MB."))?;
    if !mb.is_finite() || mb < 0.0 {
        return Err(format!("{field} must be a non-negative number of MB."));
    }
    Ok(Some((mb * MB_BYTES).round() as u64))
}

fn parse_optional<T: std::str::FromStr>(text: &str, field: &str) -> Result<Option<T>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<T>()
        .map(Some)
        .map_err(|_| format!("{field} must be a non-negative whole number."))
}

/// Empty (or whitespace-only) inputs become None; non-empty values are kept
/// verbatim so regex patterns are not altered by trimming.
fn optional_text(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn draft_to_rule(draft: &RuleDraft) -> Result<RssRule, String> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err("Give the rule a name.".to_string());
    }
    let priority = draft
        .priority
        .trim()
        .parse::<i32>()
        .map_err(|_| "Priority must be a whole number, e.g. 0 or 10.".to_string())?;
    let episode =
        if draft.episode_enabled {
            let from_episode = draft.from_episode.trim().parse::<u32>().map_err(|_| {
                "Episode range start must be a non-negative whole number.".to_string()
            })?;
            let to_episode = draft.to_episode.trim().parse::<u32>().map_err(|_| {
                "Episode range end must be a non-negative whole number.".to_string()
            })?;
            Some(EpisodeFilter {
                season: parse_optional(&draft.season, "Season")?,
                from_episode,
                to_episode,
            })
        } else {
            None
        };
    Ok(RssRule {
        // The backend assigns the id on create; 0 marks a new rule.
        id: draft.id.unwrap_or(0),
        name: name.to_string(),
        enabled: draft.enabled,
        feed_ids: draft.feed_ids.clone(),
        priority,
        match_title: optional_text(&draft.match_title),
        exclude_title: optional_text(&draft.exclude_title),
        min_size_bytes: parse_size_mb(&draft.min_size_mb, "Minimum size")?,
        max_size_bytes: parse_size_mb(&draft.max_size_mb, "Maximum size")?,
        episode,
        save_path: optional_text(&draft.save_path),
        label: optional_text(&draft.label),
        add_paused: draft.add_paused,
        cooldown_secs: parse_optional(&draft.cooldown_secs, "Cooldown")?,
    })
}

fn short_pattern(pattern: &str) -> String {
    const MAX_CHARS: usize = 28;
    if pattern.chars().count() <= MAX_CHARS {
        return pattern.to_string();
    }
    let mut shortened: String = pattern.chars().take(MAX_CHARS).collect();
    shortened.push_str("...");
    shortened
}

/// Label/description row with the settings-page switch control.
#[component]
fn SwitchRow(
    #[prop(into)] title: String,
    #[prop(into)] description: String,
    checked: Signal<bool>,
    on_toggle: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-4">
            <div>
                <div class="text-sm font-medium text-zinc-200">{title}</div>
                <div class="mt-0.5 text-xs text-zinc-500">{description}</div>
            </div>
            <button
                type="button"
                role="switch"
                aria-checked=move || if checked.get() { "true" } else { "false" }
                class=move || format!(
                    "relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 cursor-pointer {}",
                    if checked.get() { "bg-[var(--accent)]" } else { "bg-white/[0.15]" }
                )
                on:click=move |_| on_toggle.run(())
            >
                <span class=move || format!(
                    "pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform duration-200 {}",
                    if checked.get() { "translate-x-[22px]" } else { "translate-x-0.5" }
                ) />
            </button>
        </div>
    }
}

/// Rules tab: rule list, editor and dry-run test.
#[component]
pub fn RulesPanel() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = use_context::<RssPageStore>().unwrap_or_default();

    let draft = RwSignal::new(RuleDraft::default());
    let busy = RwSignal::new(false);
    let test_results = RwSignal::new(None::<Vec<FeedItem>>);

    let new_rule = move || {
        draft.set(RuleDraft::default());
        test_results.set(None);
    };

    let start_edit = move |rule: RssRule| {
        draft.set(RuleDraft::from_rule(&rule));
        test_results.set(None);
    };

    let validated_rule = move || match draft_to_rule(&draft.get()) {
        Ok(rule) => Some(rule),
        Err(message) => {
            state.push_toast(ToastKind::Error, "Invalid rule", message);
            None
        }
    };

    let save = move || {
        if busy.get() {
            return;
        }
        let Some(rule) = validated_rule() else {
            return;
        };
        busy.set(true);
        let is_new = rule.id == 0;
        spawn_local(async move {
            let result = if is_new {
                rss_create_rule(&state, &rule).await
            } else {
                rss_update_rule(&state, rule.id, &rule).await
            };
            match result {
                Ok(saved) => {
                    state.push_toast(
                        ToastKind::Success,
                        if is_new {
                            "Rule created"
                        } else {
                            "Rule updated"
                        },
                        format!("\"{}\" was saved.", saved.name),
                    );
                    draft.set(if is_new {
                        RuleDraft::default()
                    } else {
                        RuleDraft::from_rule(&saved)
                    });
                    test_results.set(None);
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Could not save rule", err.to_string());
                }
            }
            busy.set(false);
        });
    };

    let delete_rule = move |id: RuleId, name: String| {
        if busy.get() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match rss_delete_rule(&state, id).await {
                Ok(()) => {
                    state.push_toast(
                        ToastKind::Success,
                        "Rule deleted",
                        format!("\"{name}\" was removed."),
                    );
                    if draft.get().id == Some(id) {
                        draft.set(RuleDraft::default());
                        test_results.set(None);
                    }
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Could not delete rule", err.to_string());
                }
            }
            busy.set(false);
        });
    };

    let test = move || {
        if busy.get() {
            return;
        }
        let Some(rule) = validated_rule() else {
            return;
        };
        busy.set(true);
        spawn_local(async move {
            match rss_test_rule(&state, &rule, None, Some(50)).await {
                Ok(result) => {
                    let count = result.matches.len();
                    test_results.set(Some(result.matches));
                    state.push_toast(
                        ToastKind::Info,
                        "Dry run complete",
                        format!("{count} item(s) matched. Nothing was saved."),
                    );
                }
                Err(err) => {
                    test_results.set(None);
                    state.push_toast(ToastKind::Error, "Rule test failed", err.to_string());
                }
            }
            busy.set(false);
        });
    };

    view! {
        <div class="grid grid-cols-1 gap-4 md:grid-cols-12">
            <BentoCard
                title="Rules"
                subtitle="Lower priority number is evaluated first; the first matching rule wins.".to_string()
                col_span="col-span-12 md:col-span-5"
                actions=view! {
                    {move || {
                        let is_busy = busy.get();
                        view! {
                            <Button
                                label="New rule"
                                variant=ButtonVariant::Secondary
                                disabled=is_busy
                                on_click=Callback::new(move |_| new_rule())
                            />
                        }
                    }}
                }.into_any()
            >
                {move || {
                    let is_busy = busy.get();
                    let mut rules = store.rules.get();
                    rules.sort_unstable_by_key(|rule| (rule.priority, rule.id));
                    if rules.is_empty() {
                        return view! {
                            <div class="rounded-xl border border-dashed border-white/[0.08] bg-white/[0.02] px-4 py-8 text-center">
                                <p class="text-sm font-medium text-zinc-300">"No rules yet"</p>
                                <p class="mt-1 text-xs text-zinc-500">
                                    "Create a rule in the editor to auto-download matching items from your feeds."
                                </p>
                            </div>
                        }.into_any();
                    }
                    view! {
                        <div class="space-y-2">
                            {rules.into_iter().map(|rule| {
                                let rule_id = rule.id;
                                let rule_name = rule.name.clone();
                                let edit_rule = rule.clone();
                                let enabled = rule.enabled;
                                let priority = rule.priority;
                                let pattern = rule.match_title.clone();
                                view! {
                                    <div class="rounded-xl border border-white/[0.08] bg-white/[0.02] px-4 py-3 transition-all duration-200 hover:border-white/[0.15]">
                                        <div class="flex items-start justify-between gap-3">
                                            <div class="min-w-0 flex-1">
                                                <div class="truncate text-sm font-medium text-zinc-200" title=rule.name.clone()>
                                                    {rule.name.clone()}
                                                </div>
                                                <div class="mt-1.5 flex flex-wrap items-center gap-1.5">
                                                    <span class="rounded-md bg-white/[0.06] px-2 py-0.5 font-mono text-xs tabular-nums text-zinc-300">
                                                        {format!("priority {priority}")}
                                                    </span>
                                                    <Badge
                                                        label=if enabled { "Enabled" } else { "Disabled" }
                                                        kind=if enabled { BadgeKind::Success } else { BadgeKind::Neutral }
                                                    />
                                                    {pattern.map(|pattern| view! {
                                                        <Badge label=short_pattern(&pattern) kind=BadgeKind::Info />
                                                    })}
                                                </div>
                                            </div>
                                            <div class="flex shrink-0 items-center gap-2">
                                                <Button
                                                    label="Edit"
                                                    variant=ButtonVariant::Secondary
                                                    disabled=is_busy
                                                    on_click=Callback::new(move |_| start_edit(edit_rule.clone()))
                                                />
                                                <Button
                                                    label="Delete"
                                                    variant=ButtonVariant::Danger
                                                    disabled=is_busy
                                                    on_click=Callback::new(move |_| delete_rule(rule_id, rule_name.clone()))
                                                />
                                            </div>
                                        </div>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </BentoCard>

            <BentoCard
                title="Rule editor"
                subtitle="Save sends every field; empty optional fields are cleared.".to_string()
                col_span="col-span-12 md:col-span-7"
            >
                <div class="space-y-4">
                    {move || {
                        let current = draft.get();
                        match current.id {
                            Some(_) => view! {
                                <p class="rounded-lg bg-white/[0.04] px-3 py-2 text-xs text-zinc-400">
                                    {format!("Editing \"{}\". Save updates this rule in place.", current.name)}
                                </p>
                            }.into_any(),
                            None => view! {
                                <p class="rounded-lg bg-white/[0.04] px-3 py-2 text-xs text-zinc-400">
                                    "New rule: fill in the fields, then save. Test rule dry-runs the current values."
                                </p>
                            }.into_any(),
                        }
                    }}

                    <div>
                        <label for="rss-rule-name" class=LABEL_CLASS>"Name"</label>
                        <input
                            id="rss-rule-name"
                            type="text"
                            class=INPUT_CLASS
                            placeholder="e.g. Show 1080p"
                            autocomplete="off"
                            prop:value=move || draft.get().name
                            on:input=move |ev| draft.update(|d| d.name = event_target_value(&ev))
                        />
                    </div>

                    <div class="grid grid-cols-2 gap-3">
                        <div>
                            <label for="rss-rule-priority" class=LABEL_CLASS>"Priority"</label>
                            <input
                                id="rss-rule-priority"
                                type="number"
                                step="1"
                                class=INPUT_CLASS
                                prop:value=move || draft.get().priority
                                on:input=move |ev| draft.update(|d| d.priority = event_target_value(&ev))
                            />
                            <p class="mt-1.5 text-xs text-zinc-500">"Lower numbers are evaluated first."</p>
                        </div>
                        <div>
                            <label for="rss-rule-cooldown" class=LABEL_CLASS>"Cooldown (seconds)"</label>
                            <input
                                id="rss-rule-cooldown"
                                type="number"
                                min="0"
                                step="1"
                                class=INPUT_CLASS
                                placeholder="No cooldown"
                                prop:value=move || draft.get().cooldown_secs
                                on:input=move |ev| draft.update(|d| d.cooldown_secs = event_target_value(&ev))
                            />
                            <p class="mt-1.5 text-xs text-zinc-500">"Suppress re-downloads inside this window."</p>
                        </div>
                    </div>

                    <SwitchRow
                        title="Enabled"
                        description="Disabled rules are kept but skipped during evaluation."
                        checked=Signal::derive(move || draft.get().enabled)
                        on_toggle=Callback::new(move |_| draft.update(|d| d.enabled = !d.enabled))
                    />

                    <div>
                        <div class=LABEL_CLASS>"Feeds"</div>
                        <div class="space-y-1.5 rounded-xl border border-white/[0.08] bg-zinc-950/80 px-3.5 py-2.5">
                            {move || {
                                let feeds = store.feeds.get();
                                if feeds.is_empty() {
                                    return view! {
                                        <p class="text-xs text-zinc-500">
                                            "No feeds configured yet; the rule will apply to all feeds."
                                        </p>
                                    }.into_any();
                                }
                                view! {
                                    <div class="space-y-1.5">
                                        {feeds.into_iter().map(|feed| {
                                            let feed_id = feed.id;
                                            let feed_label = feed.alias.as_deref().unwrap_or(&feed.url).to_string();
                                            let feed_enabled = feed.enabled;
                                            view! {
                                                <label class="flex cursor-pointer items-center gap-2 text-sm text-zinc-300">
                                                    <input
                                                        type="checkbox"
                                                        class="h-4 w-4 shrink-0 cursor-pointer accent-[var(--accent)]"
                                                        prop:checked=move || draft.get().feed_ids.contains(&feed_id)
                                                        on:change=move |ev| {
                                                            let checked = event_target_checked(&ev);
                                                            draft.update(|d| {
                                                                if checked {
                                                                    if !d.feed_ids.contains(&feed_id) {
                                                                        d.feed_ids.push(feed_id);
                                                                    }
                                                                } else {
                                                                    d.feed_ids.retain(|id| *id != feed_id);
                                                                }
                                                            });
                                                        }
                                                    />
                                                    <span class="truncate">{feed_label}</span>
                                                    {(!feed_enabled).then(|| view! {
                                                        <span class="shrink-0 text-xs text-zinc-500">"(disabled)"</span>
                                                    })}
                                                </label>
                                            }
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            }}
                        </div>
                        <p class="mt-1.5 text-xs text-zinc-500">"Select none to apply the rule to all feeds."</p>
                    </div>

                    <div>
                        <label for="rss-rule-match" class=LABEL_CLASS>"Match title (regex)"</label>
                        <input
                            id="rss-rule-match"
                            type="text"
                            class=INPUT_CLASS
                            placeholder="e.g. Show.*1080p — empty matches everything"
                            autocomplete="off"
                            prop:value=move || draft.get().match_title
                            on:input=move |ev| draft.update(|d| d.match_title = event_target_value(&ev))
                        />
                        <p class="mt-1.5 text-xs text-zinc-500">"Case-insensitive regex; must match when set."</p>
                    </div>

                    <div>
                        <label for="rss-rule-exclude" class=LABEL_CLASS>"Exclude title (regex)"</label>
                        <input
                            id="rss-rule-exclude"
                            type="text"
                            class=INPUT_CLASS
                            placeholder="e.g. CAM|TELESYNC — empty excludes nothing"
                            autocomplete="off"
                            prop:value=move || draft.get().exclude_title
                            on:input=move |ev| draft.update(|d| d.exclude_title = event_target_value(&ev))
                        />
                        <p class="mt-1.5 text-xs text-zinc-500">"Case-insensitive regex; a hit rejects the item."</p>
                    </div>

                    <div>
                        <div class="grid grid-cols-2 gap-3">
                            <div>
                                <label for="rss-rule-min-size" class=LABEL_CLASS>"Min size (MB)"</label>
                                <input
                                    id="rss-rule-min-size"
                                    type="number"
                                    min="0"
                                    step="any"
                                    class=INPUT_CLASS
                                    placeholder="No minimum"
                                    prop:value=move || draft.get().min_size_mb
                                    on:input=move |ev| draft.update(|d| d.min_size_mb = event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label for="rss-rule-max-size" class=LABEL_CLASS>"Max size (MB)"</label>
                                <input
                                    id="rss-rule-max-size"
                                    type="number"
                                    min="0"
                                    step="any"
                                    class=INPUT_CLASS
                                    placeholder="No maximum"
                                    prop:value=move || draft.get().max_size_mb
                                    on:input=move |ev| draft.update(|d| d.max_size_mb = event_target_value(&ev))
                                />
                            </div>
                        </div>
                        <p class="mt-1.5 text-xs text-zinc-500">
                            "1 MB = 1048576 bytes. Items with unknown size are rejected when a bound is set."
                        </p>
                    </div>

                    <div>
                        <label class="flex cursor-pointer items-center gap-2 text-sm font-medium text-zinc-200">
                            <input
                                type="checkbox"
                                class="h-4 w-4 cursor-pointer accent-[var(--accent)]"
                                prop:checked=move || draft.get().episode_enabled
                                on:change=move |ev| {
                                    let checked = event_target_checked(&ev);
                                    draft.update(|d| {
                                        d.episode_enabled = checked;
                                        if checked {
                                            if d.from_episode.trim().is_empty() {
                                                d.from_episode = "1".to_string();
                                            }
                                            if d.to_episode.trim().is_empty() {
                                                d.to_episode = "1".to_string();
                                            }
                                        }
                                    });
                                }
                            />
                            "Episode filter"
                        </label>
                        <p class="mt-1 text-xs text-zinc-500">
                            "Parses SxxExx, 1x02, E03 or 'Episode 04' titles; the last two default to season 1."
                        </p>
                        <Show when=move || draft.get().episode_enabled>
                            <div class="mt-2 grid grid-cols-3 gap-3">
                                <div>
                                    <label for="rss-rule-season" class=LABEL_CLASS>"Season"</label>
                                    <input
                                        id="rss-rule-season"
                                        type="number"
                                        min="0"
                                        step="1"
                                        class=INPUT_CLASS
                                        placeholder="Any season"
                                        prop:value=move || draft.get().season
                                        on:input=move |ev| draft.update(|d| d.season = event_target_value(&ev))
                                    />
                                </div>
                                <div>
                                    <label for="rss-rule-from-episode" class=LABEL_CLASS>"From episode"</label>
                                    <input
                                        id="rss-rule-from-episode"
                                        type="number"
                                        min="0"
                                        step="1"
                                        class=INPUT_CLASS
                                        prop:value=move || draft.get().from_episode
                                        on:input=move |ev| draft.update(|d| d.from_episode = event_target_value(&ev))
                                    />
                                </div>
                                <div>
                                    <label for="rss-rule-to-episode" class=LABEL_CLASS>"To episode"</label>
                                    <input
                                        id="rss-rule-to-episode"
                                        type="number"
                                        min="0"
                                        step="1"
                                        class=INPUT_CLASS
                                        prop:value=move || draft.get().to_episode
                                        on:input=move |ev| draft.update(|d| d.to_episode = event_target_value(&ev))
                                    />
                                </div>
                            </div>
                        </Show>
                    </div>

                    <div>
                        <label for="rss-rule-save-path" class=LABEL_CLASS>"Save path"</label>
                        <input
                            id="rss-rule-save-path"
                            type="text"
                            class=INPUT_CLASS
                            placeholder="Default save path"
                            autocomplete="off"
                            prop:value=move || draft.get().save_path
                            on:input=move |ev| draft.update(|d| d.save_path = event_target_value(&ev))
                        />
                    </div>

                    <div>
                        <label for="rss-rule-label" class=LABEL_CLASS>"Label"</label>
                        <input
                            id="rss-rule-label"
                            type="text"
                            class=INPUT_CLASS
                            placeholder="No label"
                            autocomplete="off"
                            prop:value=move || draft.get().label
                            on:input=move |ev| draft.update(|d| d.label = event_target_value(&ev))
                        />
                    </div>

                    <SwitchRow
                        title="Add paused"
                        description="Add matched torrents paused instead of starting them."
                        checked=Signal::derive(move || draft.get().add_paused)
                        on_toggle=Callback::new(move |_| draft.update(|d| d.add_paused = !d.add_paused))
                    />

                    <div class="flex flex-wrap items-center gap-3 pt-1">
                        {move || {
                            let is_busy = busy.get();
                            view! {
                                <Button
                                    label="Save"
                                    disabled=is_busy
                                    on_click=Callback::new(move |_| save())
                                />
                                <button
                                    id="rss-rule-test"
                                    type="button"
                                    class=if is_busy {
                                        format!("{SECONDARY_BUTTON_CLASS} opacity-50 pointer-events-none")
                                    } else {
                                        SECONDARY_BUTTON_CLASS.to_string()
                                    }
                                    disabled=is_busy
                                    on:click=move |_| test()
                                >
                                    "Test rule"
                                </button>
                            }
                        }}
                        <span class="text-xs text-zinc-500">
                            "Test is a dry run against stored items; nothing is saved."
                        </span>
                    </div>

                    {move || {
                        let matches = test_results.get()?;
                        let count = matches.len();
                        Some(view! {
                            <div class="rounded-xl border border-white/[0.08] bg-white/[0.02] px-4 py-3">
                                <div class="text-sm font-medium text-zinc-200">
                                    {format!("Dry run: {count} matched item(s)")}
                                </div>
                                <p class="mt-0.5 text-xs text-zinc-500">
                                    "Tested the form values at test time. Nothing was saved or downloaded."
                                </p>
                                {if matches.is_empty() {
                                    view! {
                                        <p class="mt-2 text-xs text-zinc-500">"No stored items matched."</p>
                                    }.into_any()
                                } else {
                                    view! {
                                        <ul class="mt-2 max-h-64 space-y-1 overflow-y-auto">
                                            {matches.into_iter().map(|item| view! {
                                                <li class="flex items-baseline justify-between gap-3 rounded-lg bg-zinc-950/60 px-3 py-1.5">
                                                    <span class="min-w-0 break-words text-xs text-zinc-300">{item.title}</span>
                                                    {item.size_bytes.map(|bytes| view! {
                                                        <span class="shrink-0 font-mono text-[10px] tabular-nums text-zinc-500">
                                                            {format!("{:.1} MB", bytes as f64 / MB_BYTES)}
                                                        </span>
                                                    })}
                                                </li>
                                            }).collect_view()}
                                        </ul>
                                    }.into_any()
                                }}
                            </div>
                        })
                    }}
                </div>
            </BentoCard>
        </div>
    }
}
