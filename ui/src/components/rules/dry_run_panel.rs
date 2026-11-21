//! Dry-run panel (T9, plan §6.9): "Test rule" button plus the preview of
//! `POST /api/rules/test` results from `RulesPageStore::dry_run_result`.
//! The test itself is run by the rule editor (which owns the draft and the
//! destructive gate); this panel renders matches (hash/name/actions/reason),
//! the `no_data` "no scrape data" badges and the purity notice. The store
//! signal is never touched by polling (T8 contract).

use leptos::prelude::*;

use crate::components::rules::action_builder::{action_is_destructive, action_summary};
use crate::components::ui::{Badge, BadgeKind};
use crate::state::rules::RulesPageStore;

// Mirrors ButtonVariant::Secondary; the test button needs a QA id, which the
// shared Button component does not expose (same rationale as the RSS panel).
const SECONDARY_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-white/[0.04] hover:bg-white/[0.08] border border-white/[0.08] px-3.5 py-2 text-sm font-medium text-zinc-200 active:scale-[0.98] transition-all cursor-pointer";

/// First 10 characters with an ellipsis; the full hash stays in the title
/// attribute (copied from the RSS history panel, private there).
fn short_hash(hash: &str) -> String {
    let short: String = hash.chars().take(10).collect();
    if short.len() == hash.len() {
        short
    } else {
        format!("{short}…")
    }
}

/// Dry-run preview. `on_test` runs the editor's validated draft through
/// `rules_test`; `busy` is shared with the editor so Save/Test disable together.
#[component]
pub fn DryRunPanel(on_test: Callback<()>, busy: RwSignal<bool>) -> impl IntoView {
    let store = use_context::<RulesPageStore>().expect("rules page provides RulesPageStore");

    view! {
        <div class="rounded-xl border border-white/[0.08] bg-white/[0.02] px-4 py-3">
            <div class="flex flex-wrap items-center justify-between gap-3">
                <div>
                    <div class="text-sm font-medium text-zinc-200">"Dry run"</div>
                    <p class="mt-0.5 text-xs text-zinc-500">
                        "Preview the current form values against live torrents. Nothing was changed or saved."
                    </p>
                </div>
                <button
                    id="rule-test"
                    type="button"
                    class=move || {
                        if busy.get() {
                            format!("{SECONDARY_BUTTON_CLASS} opacity-50 pointer-events-none")
                        } else {
                            SECONDARY_BUTTON_CLASS.to_string()
                        }
                    }
                    disabled=move || busy.get()
                    on:click=move |_| on_test.run(())
                >
                    {move || if busy.get() { "Testing…" } else { "Test rule" }}
                </button>
            </div>
            {move || {
                store.dry_run_result.get().map(|result| {
                    let count = result.matches.len();
                    view! {
                        <div class="mt-3 border-t border-white/[0.06] pt-3">
                            <div class="text-sm font-medium text-zinc-200">
                                {format!("Dry run: {count} matched torrent(s)")}
                            </div>
                            <p class="mt-0.5 text-xs text-zinc-500">
                                "Tested the form values at test time. Nothing was changed or saved."
                            </p>
                            {if result.matches.is_empty() {
                                view! {
                                    <p class="mt-2 text-xs text-zinc-500">"No torrents matched."</p>
                                }.into_any()
                            } else {
                                view! {
                                    <ul class="mt-2 max-h-64 space-y-1 overflow-y-auto">
                                        {result.matches.into_iter().map(|matched| {
                                            view! {
                                                <li class="rounded-lg bg-zinc-950/60 px-3 py-2">
                                                    <div class="flex items-baseline justify-between gap-3">
                                                        <span class="min-w-0 break-words text-xs font-medium text-zinc-200">
                                                            {matched.name.clone()}
                                                        </span>
                                                        <span
                                                            class="shrink-0 rounded bg-white/[0.06] px-1.5 py-0.5 font-mono text-[10px] text-zinc-500"
                                                            title=matched.torrent_hash.clone()
                                                        >
                                                            {short_hash(&matched.torrent_hash)}
                                                        </span>
                                                    </div>
                                                    <div class="mt-1 flex flex-wrap items-center gap-1">
                                                        {matched.actions.iter().map(|action| {
                                                            let class = if action_is_destructive(action) {
                                                                "rounded bg-rose-500/10 px-1.5 py-0.5 font-mono text-[10px] text-rose-300"
                                                            } else {
                                                                "rounded bg-white/[0.06] px-1.5 py-0.5 font-mono text-[10px] text-zinc-300"
                                                            };
                                                            view! {
                                                                <span class=class title=action_summary(action)>
                                                                    {action_summary(action)}
                                                                </span>
                                                            }
                                                        }).collect_view()}
                                                    </div>
                                                    <p class="mt-1 text-xs text-zinc-500">{matched.reason.clone()}</p>
                                                </li>
                                            }
                                        }).collect_view()}
                                    </ul>
                                }.into_any()
                            }}
                            {(!result.no_data.is_empty()).then(|| {
                                view! {
                                    <div class="mt-2 rounded-lg border border-amber-500/20 bg-amber-500/10 px-3 py-2">
                                        <div class="flex flex-wrap items-center gap-1.5">
                                            <Badge label="no scrape data" kind=BadgeKind::Warning />
                                            <span class="text-xs text-amber-200/80">
                                                "Swarm conditions could not be evaluated for:"
                                            </span>
                                        </div>
                                        <div class="mt-1 flex flex-wrap gap-1">
                                            {result.no_data.iter().map(|hash| {
                                                view! {
                                                    <span
                                                        class="rounded bg-white/[0.06] px-1.5 py-0.5 font-mono text-[10px] text-zinc-400"
                                                        title=hash.clone()
                                                    >
                                                        {short_hash(hash)}
                                                    </span>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                }
                            })}
                        </div>
                    }
                })
            }}
        </div>
    }
}
