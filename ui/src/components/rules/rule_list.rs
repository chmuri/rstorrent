//! Rule list (T9, plan §6.9): rules sorted by `(priority, id)` with visible
//! priority, enabled/scope/trigger badges, an enable/disable switch, Edit
//! (opens the shared editor drawer) and Delete behind a confirmation modal.
//! Mutations bump `store.reload` so the page refetches (RSS pattern); the
//! toggle always diffs against the latest store snapshot, not the row's
//! render-time copy (feeds-panel pattern).

use leptos::task::spawn_local;
use leptos::{ev, prelude::*};

use crate::components::rules::rule_editor::{EditorTarget, RuleEditorHandle};
use crate::components::ui::{Badge, BadgeKind, BentoCard, Button, ButtonVariant};
use crate::state::rules::{
    rules_delete, rules_disable, rules_enable, Action, EventKind, Rule, RuleId, RulesPageStore,
    Scope, Trigger,
};
use crate::state::store::{AppState, ToastKind};

// Mirrors ButtonVariant::Secondary; the New rule button needs a QA id, which
// the shared Button component does not expose (same rationale as RSS panels).
const SECONDARY_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-white/[0.04] hover:bg-white/[0.08] border border-white/[0.08] px-3.5 py-2 text-sm font-medium text-zinc-200 active:scale-[0.98] transition-all cursor-pointer";

// Mirrors ButtonVariant::Danger for the confirm button (needs a QA id).
const DANGER_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-rose-600/90 hover:bg-rose-500 px-3.5 py-2 text-sm font-medium text-white active:scale-[0.98] transition-all cursor-pointer";

fn scope_label(scope: &Scope) -> String {
    match scope {
        Scope::Global => "Global".to_string(),
        Scope::Label(name) => format!("Label: {name}"),
        Scope::Tag(tag) => format!("Tag: {tag}"),
        Scope::Tracker(host) => format!("Tracker: {host}"),
        Scope::Rss { feed_id, rule_id } => match (feed_id, rule_id) {
            (Some(feed), Some(rule)) => format!("RSS: feed {feed} / rule {rule}"),
            (Some(feed), None) => format!("RSS: feed {feed}"),
            (None, Some(rule)) => format!("RSS: rule {rule}"),
            (None, None) => "RSS (any)".to_string(),
        },
    }
}

fn event_kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Inserted => "inserted",
        EventKind::Finished => "finished",
        EventKind::Paused => "paused",
        EventKind::Resumed => "resumed",
        EventKind::HashDone => "hash_done",
    }
}

fn trigger_label(trigger: &Trigger) -> String {
    match trigger {
        Trigger::Periodic => "Periodic".to_string(),
        Trigger::Event(kind) => format!("Event: {}", event_kind_name(*kind)),
    }
}

fn rule_is_destructive(rule: &Rule) -> bool {
    rule.actions
        .iter()
        .any(|action| matches!(action, Action::Remove | Action::RemoveWithData))
}

/// Rules tab: the sorted rule list with toggle/edit/delete and "New rule".
#[component]
pub fn RuleList() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = use_context::<RulesPageStore>().expect("rules page provides RulesPageStore");
    let editor = use_context::<RuleEditorHandle>().expect("rules page provides RuleEditorHandle");

    let busy_ids = RwSignal::new(Vec::<RuleId>::new());
    let delete_target = RwSignal::new(None::<Rule>);
    let deleting = RwSignal::new(false);

    let toggle = move |id: RuleId| {
        if busy_ids.with(|ids| ids.contains(&id)) {
            return;
        }
        // Diff against the latest store snapshot, not the row's render copy.
        let Some(rule) = store
            .rules
            .get_untracked()
            .into_iter()
            .find(|rule| rule.id == id)
        else {
            return;
        };
        let enabled = !rule.enabled;
        busy_ids.update(|ids| ids.push(id));
        spawn_local(async move {
            let result = if enabled {
                rules_enable(&state, id).await
            } else {
                rules_disable(&state, id).await
            };
            match result {
                Ok(()) => {
                    state.push_toast(
                        ToastKind::Success,
                        if enabled {
                            "Rule enabled"
                        } else {
                            "Rule disabled"
                        },
                        format!("\"{}\".", rule.name),
                    );
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Could not update rule", err.to_string());
                }
            }
            busy_ids.update(|ids| ids.retain(|busy_id| *busy_id != id));
        });
    };

    let confirm_delete = move || {
        if deleting.get() {
            return;
        }
        let Some(rule) = delete_target.get() else {
            return;
        };
        deleting.set(true);
        spawn_local(async move {
            match rules_delete(&state, rule.id).await {
                Ok(()) => {
                    state.push_toast(
                        ToastKind::Success,
                        "Rule deleted",
                        format!("\"{}\" was removed.", rule.name),
                    );
                    // Close the editor if it was editing the deleted rule.
                    if let Some(EditorTarget::Edit(open)) = editor.target.get_untracked() {
                        if open.id == rule.id {
                            editor.target.set(None);
                        }
                    }
                    delete_target.set(None);
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Could not delete rule", err.to_string());
                }
            }
            deleting.set(false);
        });
    };

    let cancel_delete = move || {
        if !deleting.get_untracked() {
            delete_target.set(None);
        }
    };

    let key_handle = window_event_listener(ev::keydown, move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape" && delete_target.get_untracked().is_some() {
            cancel_delete();
        }
    });
    on_cleanup(move || key_handle.remove());

    view! {
        <BentoCard
            title="Rules"
            subtitle="First match wins: lower priority number is evaluated first (sort: priority, id).".to_string()
            actions=view! {
                <button
                    id="rule-new"
                    type="button"
                    class=SECONDARY_BUTTON_CLASS
                    on:click=move |_| editor.target.set(Some(EditorTarget::New))
                >
                    "New rule"
                </button>
            }.into_any()
        >
            {move || {
                let busy = busy_ids.get();
                let mut rules = store.rules.get();
                rules.sort_unstable_by_key(|rule| (rule.priority, rule.id));
                if rules.is_empty() {
                    return view! {
                        <div class="rounded-xl border border-dashed border-white/[0.08] bg-white/[0.02] px-4 py-8 text-center">
                            <p class="text-sm font-medium text-zinc-300">"No rules yet"</p>
                            <p class="mt-1 text-xs text-zinc-500">
                                "Create a rule to automate your torrents: scope, trigger, conditions and actions."
                            </p>
                        </div>
                    }.into_any();
                }
                view! {
                    <div class="space-y-2">
                        {rules.into_iter().map(|rule| {
                            let rule_id = rule.id;
                            let row_busy = busy.contains(&rule_id);
                            let enabled = rule.enabled;
                            let priority = rule.priority;
                            let scope = scope_label(&rule.scope);
                            let trigger = trigger_label(&rule.trigger);
                            let trigger_kind = match &rule.trigger {
                                Trigger::Periodic => BadgeKind::Neutral,
                                Trigger::Event(_) => BadgeKind::Warning,
                            };
                            let destructive = rule_is_destructive(&rule);
                            let once = rule.once;
                            let edit_rule = rule.clone();
                            let delete_rule = rule.clone();
                            let switch_title = if enabled { "Disable this rule" } else { "Enable this rule" };
                            view! {
                                <div
                                    id=format!("rule-row-{rule_id}")
                                    class="rounded-xl border border-white/[0.08] bg-white/[0.02] px-4 py-3 transition-all duration-200 hover:border-white/[0.15]"
                                >
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
                                                <Badge label=scope kind=BadgeKind::Info />
                                                <Badge label=trigger kind=trigger_kind />
                                                {once.then(|| view! { <Badge label="Once" kind=BadgeKind::Neutral /> })}
                                                {destructive.then(|| view! { <Badge label="Destructive" kind=BadgeKind::Error /> })}
                                            </div>
                                        </div>
                                        <div class="flex shrink-0 items-center gap-2">
                                            <button
                                                type="button"
                                                role="switch"
                                                aria-checked={if enabled { "true" } else { "false" }}
                                                title=switch_title
                                                disabled=row_busy
                                                class=format!(
                                                    "relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 cursor-pointer {} {}",
                                                    if enabled { "bg-[var(--accent)]" } else { "bg-white/[0.15]" },
                                                    if row_busy { "opacity-50 pointer-events-none" } else { "" },
                                                )
                                                on:click=move |_| toggle(rule_id)
                                            >
                                                <span class=format!(
                                                    "pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform duration-200 {}",
                                                    if enabled { "translate-x-[22px]" } else { "translate-x-0.5" },
                                                ) />
                                            </button>
                                            <Button
                                                label="Edit"
                                                variant=ButtonVariant::Secondary
                                                disabled=row_busy
                                                on_click=Callback::new(move |_| {
                                                    editor.target.set(Some(EditorTarget::Edit(edit_rule.clone())));
                                                })
                                            />
                                            <Button
                                                label="Delete"
                                                variant=ButtonVariant::Danger
                                                disabled=row_busy
                                                on_click=Callback::new(move |_| delete_target.set(Some(delete_rule.clone())))
                                            />
                                        </div>
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}

            <Show when=move || delete_target.get().is_some()>
                <div
                    class="fixed inset-0 z-[60] flex items-center justify-center bg-black/75 backdrop-blur-sm p-4"
                    on:click=move |ev| {
                        if ev.target() == ev.current_target() {
                            cancel_delete();
                        }
                    }
                >
                    <div class="relative w-full max-w-md rounded-2xl border border-white/[0.12] bg-zinc-900 shadow-2xl p-6 text-zinc-100">
                        <h2 class="text-base font-semibold tracking-tight">"Delete rule"</h2>
                        <p class="mt-1 break-words text-sm text-zinc-400">
                            {move || {
                                delete_target.get().map(|rule| {
                                    format!("\"{}\" will be removed from the store. History entries are kept.", rule.name)
                                }).unwrap_or_default()
                            }}
                        </p>
                        <div class="mt-5 flex items-center justify-end gap-2">
                            <Button
                                label="Cancel"
                                variant=ButtonVariant::Ghost
                                on_click=Callback::new(move |_| cancel_delete())
                            />
                            {move || {
                                let is_deleting = deleting.get();
                                view! {
                                    <button
                                        id="rule-delete-confirm"
                                        type="button"
                                        class=if is_deleting {
                                            format!("{DANGER_BUTTON_CLASS} opacity-50 pointer-events-none")
                                        } else {
                                            DANGER_BUTTON_CLASS.to_string()
                                        }
                                        disabled=is_deleting
                                        on:click=move |_| confirm_delete()
                                    >
                                        {if is_deleting { "Deleting…" } else { "Delete" }}
                                    </button>
                                }
                            }}
                        </div>
                    </div>
                </div>
            </Show>
        </BentoCard>
    }
}
