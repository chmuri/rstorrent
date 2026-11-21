//! Rule editor (T9, plan §6.9): a slide-over drawer with every §6.1 rule
//! field — name, enabled, priority, scope selector (Global/Label/Tag/Tracker/
//! RSS with existing-label/tag selects + custom input), trigger (Periodic/
//! Event + EventKind), cond_mode, once, cooldown, allow_destructive — plus the
//! condition/action builders and the dry-run panel.
//!
//! Destructive gate (§5.5): when the actions contain Remove/RemoveWithData the
//! editor shows a red warning, requires the "I understand this deletes data"
//! checkbox AND a successful dry-run of the *current* values before Save
//! unlocks. Any edit after a test invalidates it (full-rule equality).
//!
//! Form state follows the RSS editor pattern: numeric fields stay strings so
//! partially typed values survive keystrokes; parsing/validation happens on
//! save/test. Field-level `Memo`s keep view-building closures from rebuilding
//! inputs on unrelated keystrokes (focus preservation).

use leptos::task::spawn_local;
use leptos::{ev, prelude::*};

use crate::components::rules::{ActionBuilder, ConditionBuilder, DryRunPanel};
use crate::components::ui::{Badge, BadgeKind, Button, ButtonVariant};
use crate::state::rules::{
    rules_create, rules_test, rules_update, Action, CondMode, Condition, EventKind, Rule, RuleId,
    RulesPageStore, Scope, Trigger,
};
use crate::state::store::{AppState, ToastKind};

use super::action_builder::{action_drafts_from, actions_from_drafts, ActionDraft};
use super::condition_builder::{condition_drafts_from, conditions_from_drafts, ConditionDraft};
use super::{parse_optional_u64, required_text};

const INPUT_CLASS: &str = "w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all";

const SELECT_CLASS: &str = "cursor-pointer rounded-xl border border-white/[0.08] bg-zinc-950/80 px-3 py-2 text-sm text-zinc-200 transition-all focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20";

const LABEL_CLASS: &str = "mb-1.5 block text-sm font-medium text-zinc-200";

/// Sentinel option value for the "Custom…" entry of the label/tag selects.
const CUSTOM_SCOPE_VALUE: &str = "__custom__";

/// What the editor is opened for; `None` in the handle signal means closed.
#[derive(Clone)]
pub enum EditorTarget {
    New,
    Edit(Rule),
}

/// Shared open/close handle, provided by the rules page. The list opens the
/// editor through it; the editor closes itself after a successful save.
#[derive(Clone, Copy)]
pub struct RuleEditorHandle {
    pub target: RwSignal<Option<EditorTarget>>,
}

impl RuleEditorHandle {
    pub fn new() -> Self {
        Self {
            target: RwSignal::new(None),
        }
    }
}

impl Default for RuleEditorHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ScopeKind {
    #[default]
    Global,
    Label,
    Tag,
    Tracker,
    Rss,
}

impl ScopeKind {
    const ALL: [ScopeKind; 5] = [
        ScopeKind::Global,
        ScopeKind::Label,
        ScopeKind::Tag,
        ScopeKind::Tracker,
        ScopeKind::Rss,
    ];

    fn value(self) -> &'static str {
        match self {
            ScopeKind::Global => "global",
            ScopeKind::Label => "label",
            ScopeKind::Tag => "tag",
            ScopeKind::Tracker => "tracker",
            ScopeKind::Rss => "rss",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ScopeKind::Global => "Global (every torrent)",
            ScopeKind::Label => "Label",
            ScopeKind::Tag => "Tag",
            ScopeKind::Tracker => "Tracker",
            ScopeKind::Rss => "RSS",
        }
    }

    fn from_value(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.value() == value)
    }
}

/// EventKind select options: (DTO variant, serde value, display label).
const EVENT_KINDS: [(EventKind, &str, &str); 5] = [
    (EventKind::Inserted, "inserted", "Inserted"),
    (EventKind::Finished, "finished", "Finished"),
    (EventKind::Paused, "paused", "Paused"),
    (EventKind::Resumed, "resumed", "Resumed"),
    (EventKind::HashDone, "hash_done", "Hash done"),
];

fn event_kind_value(kind: EventKind) -> &'static str {
    EVENT_KINDS
        .iter()
        .find(|(candidate, _, _)| *candidate == kind)
        .map(|(_, value, _)| *value)
        .unwrap_or("inserted")
}

/// Form state for one rule. Numeric fields stay as strings so partially typed
/// values survive keystrokes; they are parsed (and validated) on save/test.
#[derive(Clone)]
struct RuleDraft {
    /// None = creating a new rule; Some(id) = editing that rule.
    id: Option<RuleId>,
    name: String,
    enabled: bool,
    priority: String,
    scope_kind: ScopeKind,
    /// Label name / tag / tracker host, depending on `scope_kind`.
    scope_value: String,
    /// Label/Tag select is on the "Custom…" entry (free-text input shown).
    scope_custom: bool,
    rss_feed_id: String,
    rss_rule_id: String,
    trigger_event: bool,
    event_kind: EventKind,
    cond_mode: CondMode,
    once: bool,
    cooldown_secs: String,
    allow_destructive: bool,
}

impl Default for RuleDraft {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            enabled: true,
            priority: "0".to_string(),
            scope_kind: ScopeKind::default(),
            scope_value: String::new(),
            scope_custom: false,
            rss_feed_id: String::new(),
            rss_rule_id: String::new(),
            trigger_event: false,
            event_kind: EventKind::Inserted,
            cond_mode: CondMode::default(),
            once: false,
            cooldown_secs: String::new(),
            allow_destructive: false,
        }
    }
}

fn draft_from_rule(rule: &Rule, known_labels: &[String], known_tags: &[String]) -> RuleDraft {
    let (scope_kind, scope_value, scope_custom) = match &rule.scope {
        Scope::Global => (ScopeKind::Global, String::new(), false),
        Scope::Label(name) => (ScopeKind::Label, name.clone(), !known_labels.contains(name)),
        Scope::Tag(tag) => (ScopeKind::Tag, tag.clone(), !known_tags.contains(tag)),
        Scope::Tracker(host) => (ScopeKind::Tracker, host.clone(), false),
        Scope::Rss { .. } => (ScopeKind::Rss, String::new(), false),
    };
    let (rss_feed_id, rss_rule_id) = match &rule.scope {
        Scope::Rss { feed_id, rule_id } => (
            feed_id.map(|value| value.to_string()).unwrap_or_default(),
            rule_id.map(|value| value.to_string()).unwrap_or_default(),
        ),
        _ => (String::new(), String::new()),
    };
    let (trigger_event, event_kind) = match rule.trigger {
        Trigger::Periodic => (false, EventKind::Inserted),
        Trigger::Event(kind) => (true, kind),
    };
    RuleDraft {
        id: Some(rule.id),
        name: rule.name.clone(),
        enabled: rule.enabled,
        priority: rule.priority.to_string(),
        scope_kind,
        scope_value,
        scope_custom,
        rss_feed_id,
        rss_rule_id,
        trigger_event,
        event_kind,
        cond_mode: rule.cond_mode,
        once: rule.once,
        cooldown_secs: rule
            .cooldown_secs
            .map(|value| value.to_string())
            .unwrap_or_default(),
        allow_destructive: rule.allow_destructive,
    }
}

fn draft_to_rule(
    draft: &RuleDraft,
    conditions: &[ConditionDraft],
    actions: &[ActionDraft],
) -> Result<Rule, String> {
    let name = required_text(&draft.name, "Name")?;
    let priority = draft
        .priority
        .trim()
        .parse::<i32>()
        .map_err(|_| "Priority must be a whole number, e.g. 0 or 10.".to_string())?;
    let scope = match draft.scope_kind {
        ScopeKind::Global => Scope::Global,
        ScopeKind::Label => Scope::Label(required_text(&draft.scope_value, "Label")?),
        ScopeKind::Tag => Scope::Tag(required_text(&draft.scope_value, "Tag")?),
        ScopeKind::Tracker => Scope::Tracker(required_text(&draft.scope_value, "Tracker host")?),
        ScopeKind::Rss => Scope::Rss {
            feed_id: parse_optional_u64(&draft.rss_feed_id, "RSS feed id")?,
            rule_id: parse_optional_u64(&draft.rss_rule_id, "RSS rule id")?,
        },
    };
    let trigger = if draft.trigger_event {
        Trigger::Event(draft.event_kind)
    } else {
        Trigger::Periodic
    };
    let conditions = conditions_from_drafts(conditions)?;
    if conditions.is_empty() {
        return Err("Add at least one condition.".to_string());
    }
    let actions = actions_from_drafts(actions)?;
    if actions.is_empty() {
        return Err("Add at least one action.".to_string());
    }
    Ok(Rule {
        // The backend assigns the id on create; 0 marks a new rule.
        id: draft.id.unwrap_or(0),
        name,
        enabled: draft.enabled,
        priority,
        scope,
        trigger,
        cond_mode: draft.cond_mode,
        conditions,
        actions,
        once: draft.once,
        cooldown_secs: parse_optional_u64(&draft.cooldown_secs, "Cooldown")?,
        allow_destructive: draft.allow_destructive,
    })
}

/// Tag candidates for the scope select and datalists. The UI `TorrentItem`
/// DTO does not carry `tags` yet (T7/T11 extension), so known tags are
/// collected from existing rules: Tag scopes, TagHas conditions and
/// AddTag/RemoveTag actions. Reported in the T9 report.
fn known_tags_from_rules(rules: &[Rule]) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for rule in rules {
        if let Scope::Tag(tag) = &rule.scope {
            tags.push(tag.clone());
        }
        for condition in &rule.conditions {
            if let Condition::TagHas(tag) = condition {
                tags.push(tag.clone());
            }
        }
        for action in &rule.actions {
            match action {
                Action::AddTag(tag) | Action::RemoveTag(tag) => tags.push(tag.clone()),
                _ => {}
            }
        }
    }
    tags.sort();
    tags.dedup();
    tags
}

/// Label/description row with the settings-page switch control (same shape as
/// the RSS editor's SwitchRow, private there).
#[component]
fn EditorSwitch(
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

/// Rule editor drawer. Reads the open target from [`RuleEditorHandle`] in
/// context; mutations bump `store.reload` so the page refetches (RSS pattern).
#[component]
pub fn RuleEditor() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = use_context::<RulesPageStore>().expect("rules page provides RulesPageStore");
    let handle = use_context::<RuleEditorHandle>().expect("rules page provides RuleEditorHandle");

    let draft = RwSignal::new(RuleDraft::default());
    let conditions = RwSignal::new(Vec::<ConditionDraft>::new());
    let actions = RwSignal::new(Vec::<ActionDraft>::new());
    let busy = RwSignal::new(false);
    // "I understand this deletes data" acknowledgement (destructive gate).
    let ack = RwSignal::new(false);
    // Snapshot of the last successfully tested rule; any edit invalidates it.
    let last_tested = RwSignal::new(None::<Rule>);

    let known_labels = Memo::new(move |_| {
        store
            .labels
            .get()
            .into_iter()
            .map(|label| label.name)
            .collect::<Vec<_>>()
    });
    let known_tags = Memo::new(move |_| known_tags_from_rules(&store.rules.get()));

    // Field-level memos: view-building closures below subscribe to these, not
    // to the whole draft, so typing in one field never rebuilds another input.
    let scope_kind = Memo::new(move |_| draft.get().scope_kind);
    let scope_custom = Memo::new(move |_| draft.get().scope_custom);
    let trigger_event = Memo::new(move |_| draft.get().trigger_event);

    let current_rule =
        Memo::new(move |_| draft_to_rule(&draft.get(), &conditions.get(), &actions.get()).ok());
    let has_destructive = Memo::new(move |_| {
        actions
            .get()
            .iter()
            .any(|draft| draft.kind.is_destructive())
    });
    let tested_current = Memo::new(move |_| match (last_tested.get(), current_rule.get()) {
        (Some(tested), Some(current)) => tested == current,
        _ => false,
    });

    // (Re)initialize the form whenever the editor opens for a target. Runs on
    // mount too, so switching tabs and back restores the open rule.
    Effect::new(move |_| {
        let Some(target) = handle.target.get() else {
            return;
        };
        let (next_draft, next_conditions, next_actions) = match target {
            EditorTarget::New => (RuleDraft::default(), Vec::new(), Vec::new()),
            EditorTarget::Edit(rule) => {
                let labels = known_labels.get_untracked();
                let tags = known_tags.get_untracked();
                (
                    draft_from_rule(&rule, &labels, &tags),
                    condition_drafts_from(&rule.conditions),
                    action_drafts_from(&rule.actions),
                )
            }
        };
        draft.set(next_draft);
        conditions.set(next_conditions);
        actions.set(next_actions);
        ack.set(false);
        last_tested.set(None);
        // The dry-run result belongs to the editor (T8 store contract).
        store.dry_run_result.set(None);
    });

    let validated_rule =
        move || match draft_to_rule(&draft.get(), &conditions.get(), &actions.get()) {
            Ok(rule) => Some(rule),
            Err(message) => {
                state.push_toast(ToastKind::Error, "Invalid rule", message);
                None
            }
        };

    let close = move || {
        if !busy.get_untracked() {
            handle.target.set(None);
        }
    };

    let save = move || {
        if busy.get() {
            return;
        }
        let Some(rule) = validated_rule() else {
            return;
        };
        if has_destructive.get() {
            if !ack.get() {
                state.push_toast(
                    ToastKind::Error,
                    "Destructive rule",
                    "Check \"I understand this deletes data\" before saving.",
                );
                return;
            }
            if !tested_current.get() {
                state.push_toast(
                    ToastKind::Error,
                    "Destructive rule",
                    "Run \"Test rule\" on the current values before saving.",
                );
                return;
            }
        }
        busy.set(true);
        let is_new = rule.id == 0;
        spawn_local(async move {
            let result = if is_new {
                rules_create(&state, &rule).await
            } else {
                rules_update(&state, rule.id, &rule).await
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
                    handle.target.set(None);
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Could not save rule", err.to_string());
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
            match rules_test(&state, &rule, Some(50)).await {
                Ok(result) => {
                    let count = result.matches.len();
                    store.dry_run_result.set(Some(result));
                    last_tested.set(Some(rule));
                    state.push_toast(
                        ToastKind::Info,
                        "Dry run complete",
                        format!("{count} torrent(s) matched. Nothing was changed or saved."),
                    );
                }
                Err(err) => {
                    store.dry_run_result.set(None);
                    last_tested.set(None);
                    state.push_toast(ToastKind::Error, "Rule test failed", err.to_string());
                }
            }
            busy.set(false);
        });
    };

    let save_disabled =
        move || busy.get() || (has_destructive.get() && (!ack.get() || !tested_current.get()));

    let key_handle = window_event_listener(ev::keydown, move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape" && handle.target.get_untracked().is_some() {
            close();
        }
    });
    on_cleanup(move || key_handle.remove());

    view! {
        <Show when=move || handle.target.get().is_some()>
            <div
                class="fixed inset-0 z-40 bg-black/60 backdrop-blur-sm"
                on:click=move |_| close()
                aria-hidden="true"
            ></div>
            <aside
                id="rule-editor"
                class="glass-surface fixed inset-y-0 right-0 z-50 flex w-full max-w-2xl flex-col border-l border-white/[0.08] bg-zinc-900/95 shadow-2xl backdrop-blur-xl"
                role="dialog"
                aria-modal="true"
                aria-label="Rule editor"
            >
                <div class="flex items-start justify-between gap-3 border-b border-white/[0.08] px-5 py-4">
                    <div class="min-w-0 flex-1">
                        <p class="text-[11px] uppercase tracking-wide text-zinc-500">
                            {move || if draft.get().id.is_some() { "Edit rule" } else { "New rule" }}
                        </p>
                        <h2 class="mt-1 truncate text-base font-semibold tracking-tight text-zinc-100">
                            {move || {
                                let name = draft.get().name;
                                if name.trim().is_empty() { "Untitled rule".to_string() } else { name }
                            }}
                        </h2>
                    </div>
                    <button
                        type="button"
                        class="cursor-pointer rounded-lg p-1.5 text-zinc-400 transition-colors hover:bg-white/[0.06] hover:text-zinc-100"
                        on:click=move |_| close()
                        aria-label="Close editor"
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                            <path d="M18 6 6 18" />
                            <path d="m6 6 12 12" />
                        </svg>
                    </button>
                </div>

                <div class="flex-1 space-y-5 overflow-y-auto px-5 py-5">
                    <div>
                        <label for="rule-name" class=LABEL_CLASS>"Name"</label>
                        <input
                            id="rule-name"
                            type="text"
                            class=INPUT_CLASS
                            placeholder="e.g. Private tracker cleanup"
                            autocomplete="off"
                            prop:value=move || draft.get().name
                            on:input=move |ev| draft.update(|d| d.name = event_target_value(&ev))
                        />
                    </div>

                    <div class="grid grid-cols-2 gap-3">
                        <div>
                            <label for="rule-priority" class=LABEL_CLASS>"Priority"</label>
                            <input
                                id="rule-priority"
                                type="number"
                                step="1"
                                class=INPUT_CLASS
                                prop:value=move || draft.get().priority
                                on:input=move |ev| draft.update(|d| d.priority = event_target_value(&ev))
                            />
                            <p class="mt-1.5 text-xs text-zinc-500">"Lower numbers are evaluated first."</p>
                        </div>
                        <div>
                            <label for="rule-cooldown" class=LABEL_CLASS>"Cooldown (seconds)"</label>
                            <input
                                id="rule-cooldown"
                                type="number"
                                min="0"
                                step="1"
                                class=INPUT_CLASS
                                placeholder="No cooldown"
                                prop:value=move || draft.get().cooldown_secs
                                on:input=move |ev| draft.update(|d| d.cooldown_secs = event_target_value(&ev))
                            />
                            <p class="mt-1.5 text-xs text-zinc-500">"Minimum spacing between firings per torrent."</p>
                        </div>
                    </div>

                    <EditorSwitch
                        title="Enabled"
                        description="Disabled rules are kept but skipped during evaluation."
                        checked=Signal::derive(move || draft.get().enabled)
                        on_toggle=Callback::new(move |_| draft.update(|d| d.enabled = !d.enabled))
                    />

                    <div>
                        <label for="rule-scope-kind" class=LABEL_CLASS>"Scope"</label>
                        <select
                            id="rule-scope-kind"
                            class=SELECT_CLASS
                            prop:value=move || scope_kind.get().value()
                            on:change=move |ev| {
                                let Some(kind) = ScopeKind::from_value(&event_target_value(&ev)) else {
                                    return;
                                };
                                draft.update(|d| {
                                    d.scope_kind = kind;
                                    d.scope_value = String::new();
                                    d.scope_custom = false;
                                });
                            }
                        >
                            {ScopeKind::ALL.iter().map(|kind| {
                                view! { <option value=kind.value()>{kind.label()}</option> }
                            }).collect_view()}
                        </select>
                        <div class="mt-2">
                            {move || match scope_kind.get() {
                                ScopeKind::Global => view! {
                                    <p class="text-xs text-zinc-500">"Applies to every torrent."</p>
                                }.into_any(),
                                ScopeKind::Label => view! {
                                    <div class="space-y-2">
                                        <select
                                            id="rule-scope-label"
                                            class=SELECT_CLASS
                                            aria-label="Label"
                                            prop:value=move || {
                                                // Track labels so the value is reapplied once options exist.
                                                let _ = known_labels.get();
                                                let d = draft.get();
                                                if d.scope_custom { CUSTOM_SCOPE_VALUE.to_string() } else { d.scope_value.clone() }
                                            }
                                            on:change=move |ev| {
                                                let value = event_target_value(&ev);
                                                draft.update(|d| {
                                                    if value == CUSTOM_SCOPE_VALUE {
                                                        d.scope_custom = true;
                                                    } else {
                                                        d.scope_custom = false;
                                                        d.scope_value = value;
                                                    }
                                                });
                                            }
                                        >
                                            <option value="">"— choose a label —"</option>
                                            {move || known_labels.get().into_iter().map(|name| {
                                                view! { <option value=name.clone()>{name.clone()}</option> }
                                            }).collect_view()}
                                            <option value=CUSTOM_SCOPE_VALUE>"Custom…"</option>
                                        </select>
                                        {move || scope_custom.get().then(|| {
                                            view! {
                                                <input
                                                    id="rule-scope-label-custom"
                                                    type="text"
                                                    class=INPUT_CLASS
                                                    placeholder="Label name (d.custom1)"
                                                    autocomplete="off"
                                                    prop:value=move || draft.get().scope_value
                                                    on:input=move |ev| draft.update(|d| d.scope_value = event_target_value(&ev))
                                                />
                                            }
                                        })}
                                    </div>
                                }.into_any(),
                                ScopeKind::Tag => view! {
                                    <div class="space-y-2">
                                        <select
                                            id="rule-scope-tag"
                                            class=SELECT_CLASS
                                            aria-label="Tag"
                                            prop:value=move || {
                                                let _ = known_tags.get();
                                                let d = draft.get();
                                                if d.scope_custom { CUSTOM_SCOPE_VALUE.to_string() } else { d.scope_value.clone() }
                                            }
                                            on:change=move |ev| {
                                                let value = event_target_value(&ev);
                                                draft.update(|d| {
                                                    if value == CUSTOM_SCOPE_VALUE {
                                                        d.scope_custom = true;
                                                    } else {
                                                        d.scope_custom = false;
                                                        d.scope_value = value;
                                                    }
                                                });
                                            }
                                        >
                                            <option value="">"— choose a tag —"</option>
                                            {move || known_tags.get().into_iter().map(|tag| {
                                                view! { <option value=tag.clone()>{tag.clone()}</option> }
                                            }).collect_view()}
                                            <option value=CUSTOM_SCOPE_VALUE>"Custom…"</option>
                                        </select>
                                        {move || scope_custom.get().then(|| {
                                            view! {
                                                <input
                                                    id="rule-scope-tag-custom"
                                                    type="text"
                                                    class=INPUT_CLASS
                                                    placeholder="Tag name (d.tags)"
                                                    autocomplete="off"
                                                    prop:value=move || draft.get().scope_value
                                                    on:input=move |ev| draft.update(|d| d.scope_value = event_target_value(&ev))
                                                />
                                            }
                                        })}
                                        <p class="text-xs text-zinc-500">
                                            "Known tags come from existing rules; torrent tags are not in the UI item DTO yet."
                                        </p>
                                    </div>
                                }.into_any(),
                                ScopeKind::Tracker => view! {
                                    <input
                                        id="rule-scope-tracker"
                                        type="text"
                                        class=INPUT_CLASS
                                        placeholder="tracker.example.org (host, no scheme/port)"
                                        autocomplete="off"
                                        prop:value=move || draft.get().scope_value
                                        on:input=move |ev| draft.update(|d| d.scope_value = event_target_value(&ev))
                                    />
                                }.into_any(),
                                ScopeKind::Rss => view! {
                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label for="rule-scope-rss-feed" class=LABEL_CLASS>"Feed id (optional)"</label>
                                            <input
                                                id="rule-scope-rss-feed"
                                                type="number"
                                                min="0"
                                                step="1"
                                                class=INPUT_CLASS
                                                placeholder="Any feed"
                                                prop:value=move || draft.get().rss_feed_id
                                                on:input=move |ev| draft.update(|d| d.rss_feed_id = event_target_value(&ev))
                                            />
                                        </div>
                                        <div>
                                            <label for="rule-scope-rss-rule" class=LABEL_CLASS>"Rule id (optional)"</label>
                                            <input
                                                id="rule-scope-rss-rule"
                                                type="number"
                                                min="0"
                                                step="1"
                                                class=INPUT_CLASS
                                                placeholder="Any rule"
                                                prop:value=move || draft.get().rss_rule_id
                                                on:input=move |ev| draft.update(|d| d.rss_rule_id = event_target_value(&ev))
                                            />
                                        </div>
                                        <p class="col-span-2 text-xs text-zinc-500">
                                            "Matches torrents added via RSS; empty fields match any value, both empty = every RSS torrent."
                                        </p>
                                    </div>
                                }.into_any(),
                            }}
                        </div>
                    </div>

                    <div class="grid grid-cols-2 gap-3">
                        <div>
                            <label for="rule-trigger" class=LABEL_CLASS>"Trigger"</label>
                            <select
                                id="rule-trigger"
                                class=SELECT_CLASS
                                prop:value=move || if trigger_event.get() { "event" } else { "periodic" }
                                on:change=move |ev| {
                                    let event = event_target_value(&ev) == "event";
                                    draft.update(|d| d.trigger_event = event);
                                }
                            >
                                <option value="periodic">"Periodic (every check)"</option>
                                <option value="event">"Event (lifecycle)"</option>
                            </select>
                        </div>
                        <div>
                            <label for="rule-cond-mode" class=LABEL_CLASS>"Conditions mode"</label>
                            <select
                                id="rule-cond-mode"
                                class=SELECT_CLASS
                                prop:value=move || match draft.get().cond_mode {
                                    CondMode::All => "all",
                                    CondMode::Any => "any",
                                }
                                on:change=move |ev| {
                                    let mode = if event_target_value(&ev) == "any" { CondMode::Any } else { CondMode::All };
                                    draft.update(|d| d.cond_mode = mode);
                                }
                            >
                                <option value="all">"All (AND)"</option>
                                <option value="any">"Any (OR)"</option>
                            </select>
                        </div>
                    </div>

                    <Show when=move || trigger_event.get()>
                        <div>
                            <label for="rule-event-kind" class=LABEL_CLASS>"Event kind"</label>
                            <select
                                id="rule-event-kind"
                                class=SELECT_CLASS
                                prop:value=move || event_kind_value(draft.get().event_kind)
                                on:change=move |ev| {
                                    let value = event_target_value(&ev);
                                    if let Some(kind) = EVENT_KINDS.iter().find(|(_, candidate, _)| *candidate == value).map(|(kind, _, _)| *kind) {
                                        draft.update(|d| d.event_kind = kind);
                                    }
                                }
                            >
                                {EVENT_KINDS.iter().map(|(_, value, label)| {
                                    view! { <option value=*value>{*label}</option> }
                                }).collect_view()}
                            </select>
                        </div>
                    </Show>

                    <label class="flex cursor-pointer items-start gap-3 rounded-xl border border-white/[0.08] bg-white/[0.03] px-3 py-2.5 hover:bg-white/[0.05]">
                        <input
                            id="rule-once"
                            type="checkbox"
                            class="mt-0.5 h-4 w-4 cursor-pointer accent-[var(--accent)]"
                            prop:checked=move || draft.get().once
                            on:change=move |ev| draft.update(|d| d.once = event_target_checked(&ev))
                        />
                        <span class="text-sm">
                            <span class="font-medium text-zinc-100">"Once per torrent"</span>
                            <span class="block text-xs text-zinc-500">"Fire at most once per torrent, ever (sidecar ledger)."</span>
                        </span>
                    </label>

                    <div>
                        <div class=LABEL_CLASS>"Conditions"</div>
                        <ConditionBuilder conditions />
                    </div>

                    <div>
                        <div class=LABEL_CLASS>"Actions"</div>
                        <ActionBuilder actions />
                    </div>

                    <label class="flex cursor-pointer items-start gap-3 rounded-xl border border-white/[0.08] bg-white/[0.03] px-3 py-2.5 hover:bg-white/[0.05]">
                        <input
                            id="rule-allow-destructive"
                            type="checkbox"
                            class="mt-0.5 h-4 w-4 cursor-pointer accent-rose-500"
                            prop:checked=move || draft.get().allow_destructive
                            on:change=move |ev| draft.update(|d| d.allow_destructive = event_target_checked(&ev))
                        />
                        <span class="text-sm">
                            <span class="font-medium text-zinc-100">"Allow destructive actions"</span>
                            <span class="block text-xs text-zinc-500">"Per-rule gate for Remove/Remove with data; the global gate in Settings must also be on."</span>
                        </span>
                    </label>

                    <Show when=move || has_destructive.get()>
                        <div class="rounded-xl border border-rose-500/30 bg-rose-500/10 px-4 py-3">
                            <p class="text-sm font-semibold text-rose-300">"Destructive actions selected"</p>
                            <p class="mt-1 text-xs text-rose-200/80">
                                "Remove deletes the torrent from the client; Remove with data also deletes its files from disk. This cannot be undone."
                            </p>
                            {move || (!store.seed_settings.get().rules.allow_destructive).then(|| {
                                view! {
                                    <p class="mt-2 text-xs text-amber-300">
                                        "The global destructive gate is off — these actions will be skipped until it is enabled in Settings."
                                    </p>
                                }
                            })}
                            {move || (!draft.get().allow_destructive).then(|| {
                                view! {
                                    <p class="mt-2 text-xs text-amber-300">
                                        "„Allow destructive actions” is unchecked — the actions will be skipped."
                                    </p>
                                }
                            })}
                            <label class="mt-3 flex cursor-pointer items-start gap-3">
                                <input
                                    id="rule-destructive-ack"
                                    type="checkbox"
                                    class="mt-0.5 h-4 w-4 cursor-pointer accent-rose-500"
                                    prop:checked=move || ack.get()
                                    on:change=move |ev| ack.set(event_target_checked(&ev))
                                />
                                <span class="text-sm font-medium text-rose-100">"I understand this deletes data"</span>
                            </label>
                            <div class="mt-2 flex flex-wrap items-center gap-2">
                                {move || if tested_current.get() {
                                    view! { <Badge label="Tested" kind=BadgeKind::Success /> }.into_any()
                                } else {
                                    view! { <Badge label="Test required" kind=BadgeKind::Warning /> }.into_any()
                                }}
                                <span class="text-xs text-rose-200/70">
                                    "Run „Test rule” below on the current values before saving; edits invalidate the test."
                                </span>
                            </div>
                        </div>
                    </Show>

                    <DryRunPanel on_test=Callback::new(move |_| test()) busy />

                    // Datalists shared by the scope selects and the builders'
                    // label/tag inputs (rendered once to keep ids unique).
                    <datalist id="rule-editor-labels">
                        {move || known_labels.get().into_iter().map(|name| {
                            view! { <option value=name.clone() /> }
                        }).collect_view()}
                    </datalist>
                    <datalist id="rule-editor-tags">
                        {move || known_tags.get().into_iter().map(|tag| {
                            view! { <option value=tag.clone() /> }
                        }).collect_view()}
                    </datalist>
                </div>

                <div class="flex items-center justify-end gap-2 border-t border-white/[0.08] px-5 py-4">
                    <Button
                        label="Cancel"
                        variant=ButtonVariant::Ghost
                        on_click=Callback::new(move |_| close())
                    />
                    {move || {
                        let disabled = save_disabled();
                        let label = if busy.get() { "Saving…" } else { "Save" };
                        view! {
                            <Button
                                label=label
                                disabled=disabled
                                on_click=Callback::new(move |_| save())
                            />
                        }
                    }}
                </div>
            </aside>
        </Show>
    }
}
