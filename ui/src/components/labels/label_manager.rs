//! Label registry manager (plan.md T10, §6.9): list of labels with color
//! badges and policy summaries, add form (name + color picker), per-label
//! default save path, remove-with-confirm (torrents keep `d.custom1`) and the
//! seed policy dialog. Mutations go through the T8 REST client
//! (`state/rules.rs`); list membership changes bump `store.reload`, in-place
//! updates apply the PUT response directly.
//!
//! The component owns a local [`RulesPageStore`] and provides it to the policy
//! dialog via context (the settings page does not provide the `/rules` store).

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::label_policy_dialog::{action_label, format_secs_short, LabelPolicyDialog};
use crate::components::ui::{Badge, BadgeKind, BentoCard, Button, ButtonVariant};
use crate::state::rules::{
    labels_create, labels_delete, labels_list, labels_update, LabelDef, LimitMode, LimitValue,
    RulesPageStore, SeedAction, SeedPolicy,
};
use crate::state::store::{AppState, ConnectionMode, ToastKind};

// Copied from pages/settings.rs (private there) to keep the shared input look.
const INPUT_CLASS: &str = "w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all";

// Mirrors ButtonVariant::Primary; the add button needs a QA id, which the
// shared Button component does not expose (feeds_panel.rs precedent).
const PRIMARY_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-medium text-white shadow-[inset_0_1px_0_0_rgba(255,255,255,0.2)] hover:brightness-110 active:scale-[0.98] transition-all cursor-pointer";

const DEFAULT_COLOR: &str = "#7c3aed";

/// Shared handles for the per-label actions. Everything is `Copy` (contexts
/// and signals), so rows and async handlers can capture it freely
/// (feeds_panel.rs FeedCtx pattern).
#[derive(Clone, Copy)]
struct LabelCtx {
    state: AppState,
    store: RulesPageStore,
    busy_names: RwSignal<Vec<String>>,
    /// Unsaved save-path drafts keyed by label name; survives list re-renders.
    path_drafts: RwSignal<HashMap<String, String>>,
    /// Snapshot of the label whose policy dialog is open (None = closed).
    editing_policy: RwSignal<Option<LabelDef>>,
    /// Name of the label pending remove confirmation (None = closed).
    confirm_remove: RwSignal<Option<String>>,
}

impl LabelCtx {
    fn is_busy(self, name: &str) -> bool {
        self.busy_names.get().iter().any(|busy| busy == name)
    }

    fn set_busy(self, name: &str, busy: bool) {
        self.busy_names.update(|names| {
            if busy {
                names.push(name.to_string());
            } else {
                names.retain(|busy_name| busy_name != name);
            }
        });
    }

    /// Always diff against the latest store snapshot, not the row's
    /// render-time copy, so concurrent reloads cannot produce stale updates.
    fn current_label(self, name: &str) -> Option<LabelDef> {
        self.store
            .labels
            .get()
            .into_iter()
            .find(|label| label.name == name)
    }

    fn open_policy(self, name: &str) {
        if let Some(label) = self.current_label(name) {
            self.editing_policy.set(Some(label));
        }
    }

    fn save_path(self, name: &str) {
        if self.is_busy(name) {
            return;
        }
        let Some(label) = self.current_label(name) else {
            return;
        };
        let Some(text) = self.path_drafts.get_untracked().get(name).cloned() else {
            return;
        };
        let trimmed = text.trim();
        let save_path = (!trimmed.is_empty()).then(|| trimmed.to_string());
        if save_path == label.save_path {
            self.path_drafts.update(|drafts| {
                drafts.remove(name);
            });
            return;
        }
        let mut updated = label.clone();
        updated.save_path = save_path;
        let name = name.to_string();
        self.set_busy(&name, true);
        spawn_local(async move {
            match labels_update(&self.state, &name, &updated).await {
                Ok(saved) => {
                    self.state.push_toast(
                        ToastKind::Success,
                        "Save path updated",
                        format!("\"{name}\" default directory saved."),
                    );
                    // Apply the PUT response in place; other rows keep their
                    // drafts and the list does not need a full refetch.
                    self.store.labels.update(|labels| {
                        if let Some(entry) = labels.iter_mut().find(|label| label.name == name) {
                            *entry = saved;
                        }
                    });
                    self.path_drafts.update(|drafts| {
                        drafts.remove(&name);
                    });
                }
                Err(err) => {
                    self.state.push_toast(
                        ToastKind::Error,
                        "Could not update label",
                        err.to_string(),
                    );
                }
            }
            self.set_busy(&name, false);
        });
    }

    fn remove_label(self, name: &str) {
        if self.is_busy(name) {
            return;
        }
        let name = name.to_string();
        self.set_busy(&name, true);
        spawn_local(async move {
            match labels_delete(&self.state, &name).await {
                Ok(()) => {
                    self.state.push_toast(
                        ToastKind::Success,
                        "Label removed",
                        format!(
                            "\"{name}\" was removed from the registry. Torrents keep their d.custom1 value."
                        ),
                    );
                    self.confirm_remove.set(None);
                    if self
                        .editing_policy
                        .get_untracked()
                        .is_some_and(|label| label.name == name)
                    {
                        self.editing_policy.set(None);
                    }
                    self.path_drafts.update(|drafts| {
                        drafts.remove(&name);
                    });
                    self.store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    self.state.push_toast(
                        ToastKind::Error,
                        "Could not remove label",
                        err.to_string(),
                    );
                }
            }
            self.set_busy(&name, false);
        });
    }
}

/// Badges summarizing the explicit parts of a policy; an all-inherit policy
/// shows a single "inherits global" badge.
fn policy_summary(policy: &SeedPolicy) -> Vec<(String, BadgeKind)> {
    let mut badges = Vec::new();
    match policy.ratio {
        LimitValue::Set(ratio) => badges.push((format!("ratio {ratio}"), BadgeKind::Info)),
        LimitValue::Unlimited => badges.push(("ratio unlimited".to_string(), BadgeKind::Neutral)),
        LimitValue::Inherit => {}
    }
    match policy.seeding_time_secs {
        LimitValue::Set(secs) => {
            badges.push((format!("seed {}", format_secs_short(secs)), BadgeKind::Info));
        }
        LimitValue::Unlimited => {
            badges.push(("seed time unlimited".to_string(), BadgeKind::Neutral));
        }
        LimitValue::Inherit => {}
    }
    match policy.inactive_secs {
        LimitValue::Set(secs) => {
            badges.push((format!("idle {}", format_secs_short(secs)), BadgeKind::Info));
        }
        LimitValue::Unlimited => badges.push(("idle unlimited".to_string(), BadgeKind::Neutral)),
        LimitValue::Inherit => {}
    }
    if let Some(action) = &policy.action {
        let kind = if matches!(action, SeedAction::Remove | SeedAction::RemoveWithData) {
            BadgeKind::Error
        } else {
            BadgeKind::Warning
        };
        badges.push((action_label(action), kind));
    }
    if badges.is_empty() {
        badges.push(("inherits global".to_string(), BadgeKind::Neutral));
    } else if policy.mode == LimitMode::All {
        badges.push(("mode all".to_string(), BadgeKind::Neutral));
    }
    badges
}

fn label_row(label: LabelDef, ctx: LabelCtx, idx: usize) -> impl IntoView {
    let name = label.name.clone();
    let color = label.color.clone();
    let stored_path = label.save_path.clone();
    let badges = policy_summary(&label.policy);
    let path_id = format!("label-path-{idx}");

    let input_name = name.clone();
    let button_name = name.clone();
    let button_path = stored_path.clone();
    let edit_name = name.clone();
    let delete_name = name.clone();

    view! {
        <div class="rounded-xl border border-white/[0.08] bg-white/[0.02] px-4 py-3 transition-all duration-200 hover:border-white/[0.15]">
            <div class="flex flex-wrap items-start justify-between gap-3">
                <div class="min-w-0 flex-1">
                    <div class="flex min-w-0 flex-wrap items-center gap-2">
                        <span
                            class="inline-flex items-center gap-1.5 rounded-full border border-white/[0.1] bg-white/[0.04] px-2.5 py-0.5 text-xs font-medium text-zinc-200"
                            title=color.as_deref().map(|color| format!("Color {color}")).unwrap_or_else(|| "No color".to_string())
                        >
                            <span
                                class="h-2 w-2 shrink-0 rounded-full"
                                style=format!("background-color: {}", color.as_deref().unwrap_or("#71717a"))
                            />
                            {label.name.clone()}
                        </span>
                        {badges.into_iter().map(|(text, kind)| view! { <Badge label=text kind=kind /> }).collect_view()}
                    </div>
                    <div class="mt-2.5 flex flex-wrap items-end gap-2">
                        <div class="min-w-[12rem] flex-1">
                            <label for=path_id.clone() class="mb-1 block text-xs font-medium text-zinc-400">
                                "Default save path"
                            </label>
                            <input
                                id=path_id
                                type="text"
                                class=INPUT_CLASS
                                placeholder="Empty uses the global download directory"
                                autocomplete="off"
                                prop:value=move || {
                                    ctx.path_drafts
                                        .get()
                                        .get(&name)
                                        .cloned()
                                        .unwrap_or_else(|| stored_path.clone().unwrap_or_default())
                                }
                                on:input=move |ev| {
                                    let value = event_target_value(&ev);
                                    ctx.path_drafts.update(|drafts| {
                                        drafts.insert(input_name.clone(), value);
                                    });
                                }
                            />
                        </div>
                        {move || {
                            let busy = ctx.is_busy(&button_name);
                            let changed = ctx
                                .path_drafts
                                .get()
                                .get(&button_name)
                                .map(|text| {
                                    let trimmed = text.trim();
                                    let value = (!trimmed.is_empty()).then(|| trimmed.to_string());
                                    value != button_path
                                })
                                .unwrap_or(false);
                            let click_name = button_name.clone();
                            view! {
                                <Button
                                    label="Save path"
                                    variant=ButtonVariant::Secondary
                                    disabled=busy || !changed
                                    on_click=Callback::new(move |_| ctx.save_path(&click_name))
                                />
                            }
                        }}
                    </div>
                </div>
                <div class="flex shrink-0 flex-wrap items-center gap-2">
                    {move || {
                        let busy = ctx.is_busy(&edit_name);
                        let click_name = edit_name.clone();
                        view! {
                            <Button
                                label="Edit policy"
                                variant=ButtonVariant::Secondary
                                disabled=busy
                                on_click=Callback::new(move |_| ctx.open_policy(&click_name))
                            />
                        }
                    }}
                    {move || {
                        let busy = ctx.is_busy(&delete_name);
                        let click_name = delete_name.clone();
                        view! {
                            <Button
                                label="Delete"
                                variant=ButtonVariant::Danger
                                disabled=busy
                                on_click=Callback::new(move |_| {
                                    ctx.confirm_remove.set(Some(click_name.clone()));
                                })
                            />
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

/// Labels card: registry list, add form, save-path editing, remove confirm
/// and the per-label seed policy dialog. Owns and provides a local
/// [`RulesPageStore`]; loads the label list on mount and on `reload` bumps
/// (live mode only, no polling — settings is not the `/rules` page).
#[component]
pub fn LabelManager() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = RulesPageStore::new();
    // The policy dialog reads the store from context (RSS panels pattern).
    provide_context(store);

    let request_gen = RwSignal::new(0u64);

    Effect::new(move |_| {
        let _reload = store.reload.get();
        // Invalidate in-flight responses on every run, including going offline.
        request_gen.update(|gen| *gen += 1);
        if state.connection.get() != ConnectionMode::Live {
            return;
        }
        let gen = request_gen.get_untracked();
        spawn_local(async move {
            store.loading.set(true);
            match labels_list(&state).await {
                Ok(labels) => {
                    if request_gen.get_untracked() == gen {
                        store.labels.set(labels);
                    }
                }
                Err(err) => {
                    if request_gen.get_untracked() == gen {
                        state.push_toast(
                            ToastKind::Error,
                            "Could not load labels",
                            err.to_string(),
                        );
                    }
                }
            }
            if request_gen.get_untracked() == gen {
                store.loading.set(false);
            }
        });
    });

    let ctx = LabelCtx {
        state,
        store,
        busy_names: RwSignal::new(Vec::new()),
        path_drafts: RwSignal::new(HashMap::new()),
        editing_policy: RwSignal::new(None),
        confirm_remove: RwSignal::new(None),
    };

    let name_draft = RwSignal::new(String::new());
    let color_draft = RwSignal::new(DEFAULT_COLOR.to_string());
    let adding = RwSignal::new(false);

    let add_label = move || {
        if adding.get() {
            return;
        }
        let name = name_draft.get().trim().to_string();
        if name.is_empty() {
            state.push_toast(
                ToastKind::Error,
                "Invalid label name",
                "Give the label a non-empty name.",
            );
            return;
        }
        if store
            .labels
            .get_untracked()
            .iter()
            .any(|label| label.name == name)
        {
            state.push_toast(
                ToastKind::Error,
                "Label already exists",
                format!("\"{name}\" is already registered (names are case-sensitive)."),
            );
            return;
        }
        let label = LabelDef {
            name: name.clone(),
            color: Some(color_draft.get()),
            policy: SeedPolicy::default(),
            save_path: None,
        };
        adding.set(true);
        spawn_local(async move {
            match labels_create(&state, &label).await {
                Ok(created) => {
                    state.push_toast(
                        ToastKind::Success,
                        "Label added",
                        format!("\"{}\" is now registered.", created.name),
                    );
                    name_draft.set(String::new());
                    color_draft.set(DEFAULT_COLOR.to_string());
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Could not add label", err.to_string());
                }
            }
            adding.set(false);
        });
    };

    let key_handle =
        window_event_listener(leptos::ev::keydown, move |ev: web_sys::KeyboardEvent| {
            if ev.key() != "Escape" {
                return;
            }
            let Some(name) = ctx.confirm_remove.get_untracked() else {
                return;
            };
            if !ctx.is_busy(&name) {
                ctx.confirm_remove.set(None);
            }
        });
    on_cleanup(move || key_handle.remove());

    view! {
        <BentoCard
            title="Labels"
            subtitle="Registry of d.custom1 labels — colors, seed policies and default save paths.".to_string()
            col_span="col-span-1 md:col-span-2"
            actions=view! {
                {move || {
                    let count = store.labels.get().len();
                    view! {
                        <Badge
                            label={if count == 1 { "1 label".to_string() } else { format!("{count} labels") }}
                            kind=BadgeKind::Neutral
                        />
                    }
                }}
                {move || {
                    let busy = store.loading.get();
                    view! {
                        <Button
                            label="Reload"
                            variant=ButtonVariant::Ghost
                            disabled=busy
                            on_click=Callback::new(move |_| store.reload.update(|n| *n += 1))
                        />
                    }
                }}
            }.into_any()
        >
            <Show
                when=move || state.connection.get() == ConnectionMode::Live
                fallback=move || {
                    view! {
                        <div class="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-white/[0.12] bg-white/[0.02] px-6 py-12 text-center">
                            <p class="text-sm font-medium text-zinc-300">
                                "Backend offline — labels require a live connection"
                            </p>
                        </div>
                    }
                }
            >
                <div class="space-y-4">
                    <div class="flex flex-wrap items-end gap-3">
                        <div class="min-w-[12rem] flex-1">
                            <label for="label-add-name" class="mb-1 block text-xs font-medium text-zinc-400">
                                "Name"
                            </label>
                            <input
                                id="label-add-name"
                                type="text"
                                class=INPUT_CLASS
                                placeholder="e.g. movies — matches d.custom1"
                                autocomplete="off"
                                prop:value=move || name_draft.get()
                                on:input=move |ev| name_draft.set(event_target_value(&ev))
                            />
                        </div>
                        <div class="shrink-0">
                            <label for="label-add-color" class="mb-1 block text-xs font-medium text-zinc-400">
                                "Color"
                            </label>
                            <div class="flex items-center gap-2">
                                <input
                                    id="label-add-color"
                                    type="color"
                                    class="h-9 w-14 cursor-pointer rounded-lg border border-white/[0.08] bg-zinc-950/80 p-1"
                                    prop:value=move || color_draft.get()
                                    on:input=move |ev| color_draft.set(event_target_value(&ev))
                                />
                                <span class="font-mono text-xs text-zinc-500">{move || color_draft.get()}</span>
                            </div>
                        </div>
                        {move || {
                            let busy = adding.get();
                            view! {
                                <button
                                    id="label-add-button"
                                    type="button"
                                    disabled=busy
                                    class=if busy {
                                        format!("{PRIMARY_BUTTON_CLASS} opacity-50 pointer-events-none")
                                    } else {
                                        PRIMARY_BUTTON_CLASS.to_string()
                                    }
                                    on:click=move |_| add_label()
                                >
                                    {if busy { "Adding…" } else { "Add label" }}
                                </button>
                            }
                        }}
                    </div>
                    <p class="text-xs text-zinc-500">
                        "Names are case-sensitive and map to d.custom1. The save path is the default directory for newly added torrents carrying the label."
                    </p>

                    {move || {
                        let labels = store.labels.get();
                        if labels.is_empty() {
                            view! {
                                <div class="rounded-xl border border-dashed border-white/[0.12] bg-white/[0.02] px-4 py-8 text-center">
                                    <p class="text-sm font-medium text-zinc-300">"No labels yet"</p>
                                    <p class="mt-1 text-xs text-zinc-500">
                                        "Add a label above to attach colors, seed policies and default save paths to torrents."
                                    </p>
                                </div>
                            }
                            .into_any()
                        } else {
                            view! {
                                <div class="space-y-3">
                                    {labels.into_iter().enumerate().map(|(idx, label)| label_row(label, ctx, idx)).collect_view()}
                                </div>
                            }
                            .into_any()
                        }
                    }}
                </div>
            </Show>
        </BentoCard>

        {move || {
            ctx.editing_policy.get().map(|label| {
                view! {
                    <LabelPolicyDialog
                        name=label.name.clone()
                        policy=label.policy.clone()
                        on_close=Callback::new(move |_| ctx.editing_policy.set(None))
                    />
                }
            })
        }}

        {move || {
            let name = ctx.confirm_remove.get()?;
            let busy = ctx.is_busy(&name);
            let backdrop_name = name.clone();
            let cancel_name = name.clone();
            let confirm_name = name.clone();
            Some(view! {
                <div
                    class="fixed inset-0 z-[60] flex items-center justify-center bg-black/75 backdrop-blur-sm p-4"
                    on:click=move |ev| {
                        if ev.target() == ev.current_target() && !ctx.is_busy(&backdrop_name) {
                            ctx.confirm_remove.set(None);
                        }
                    }
                >
                    <div
                        class="relative w-full max-w-md rounded-2xl border border-white/[0.12] bg-zinc-900 shadow-2xl p-6 text-zinc-100"
                        role="dialog"
                        aria-modal="true"
                        aria-label="Remove label"
                    >
                        <h2 class="text-base font-semibold tracking-tight">"Remove label"</h2>
                        <p class="mt-1 break-words text-sm text-zinc-400">{format!("\"{name}\"")}</p>
                        <p class="mt-3 text-xs text-zinc-500">
                            "Torrents keep their label value (d.custom1). Only the registry entry — color, seed policy and default save path — is removed."
                        </p>
                        <div class="mt-5 flex items-center justify-end gap-2">
                            <Button
                                label="Cancel"
                                variant=ButtonVariant::Ghost
                                on_click=Callback::new(move |_| {
                                    if !ctx.is_busy(&cancel_name) {
                                        ctx.confirm_remove.set(None);
                                    }
                                })
                            />
                            <Button
                                label={if busy { "Removing…" } else { "Remove label" }}
                                variant=ButtonVariant::Danger
                                disabled=busy
                                on_click=Callback::new(move |_| ctx.remove_label(&confirm_name))
                            />
                        </div>
                    </div>
                </div>
            })
        }}
    }
}
