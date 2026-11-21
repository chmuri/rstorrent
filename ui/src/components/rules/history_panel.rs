//! History panel (T9, plan §6.9): audit table When/Rule/Torrent/Actions/
//! Status/Reason with ok/failed/skipped badges and a Clear history action
//! behind a confirmation modal. Data comes from `RulesPageStore::history`,
//! which the page reload effect refreshes on every mutation bump and on the
//! 15 s live-mode poll — the panel itself is purely reactive (RSS pattern).
//! `rule_id == 0` marks the built-in seed-policy enforcement.

use leptos::task::spawn_local;
use leptos::{ev, prelude::*};
use wasm_bindgen::JsValue;

use crate::components::ui::{Badge, BadgeKind, BentoCard, Button, ButtonVariant};
use crate::state::rules::{rules_clear_history, HistoryEntry, HistoryStatus, RulesPageStore};
use crate::state::store::{AppState, ToastKind};

// Mirrors ButtonVariant::Danger; the clear button needs a QA id, which the
// shared Button component does not expose (same rationale as RSS history).
const DANGER_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-rose-600/90 hover:bg-rose-500 px-3.5 py-2 text-sm font-medium text-white active:scale-[0.98] transition-all cursor-pointer";

/// History tab: audit trail with a guarded clear action.
#[component]
pub fn HistoryPanel() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = use_context::<RulesPageStore>().expect("rules page provides RulesPageStore");
    let busy = RwSignal::new(false);
    let confirm_open = RwSignal::new(false);

    let clear_history = move || {
        if busy.get() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match rules_clear_history(&state).await {
                Ok(()) => {
                    state.push_toast(
                        ToastKind::Success,
                        "History cleared",
                        "All rule audit entries were removed.",
                    );
                    confirm_open.set(false);
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Could not clear history", err.to_string());
                }
            }
            busy.set(false);
        });
    };

    let cancel_clear = move || {
        if !busy.get_untracked() {
            confirm_open.set(false);
        }
    };

    let key_handle = window_event_listener(ev::keydown, move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape" && confirm_open.get_untracked() {
            cancel_clear();
        }
    });
    on_cleanup(move || key_handle.remove());

    view! {
        <BentoCard
            title="Rule History"
            subtitle="Audit trail of every decision (cap 500). Refreshes with the page poll and after Check now.".to_string()
            actions=view! {
                <button
                    id="rules-history-clear"
                    type="button"
                    class=move || {
                        if busy.get() {
                            format!("{DANGER_BUTTON_CLASS} opacity-50 pointer-events-none")
                        } else {
                            DANGER_BUTTON_CLASS.to_string()
                        }
                    }
                    disabled=move || busy.get()
                    on:click=move |_| confirm_open.set(true)
                >
                    "Clear history"
                </button>
            }.into_any()
        >
            <Show
                when=move || !store.history.get().is_empty()
                fallback=move || {
                    view! {
                        <div class="flex flex-col items-center justify-center gap-1 px-6 py-12 text-center">
                            <p class="text-sm font-medium text-zinc-300">"No rule decisions recorded yet"</p>
                            <p class="text-xs text-zinc-500">
                                "Fired, failed and skipped decisions appear here with their reasons. Use „Check now” to force a pass."
                            </p>
                        </div>
                    }
                }
            >
                <div class="overflow-x-auto">
                    <table class="w-full text-left">
                        <thead>
                            <tr class="border-b border-white/[0.08] text-xs text-zinc-500">
                                <th class="px-3 py-2 font-medium">"When"</th>
                                <th class="px-3 py-2 font-medium">"Rule"</th>
                                <th class="px-3 py-2 font-medium">"Torrent"</th>
                                <th class="px-3 py-2 font-medium">"Actions"</th>
                                <th class="px-3 py-2 font-medium">"Status"</th>
                                <th class="px-3 py-2 font-medium">"Reason"</th>
                            </tr>
                        </thead>
                        <tbody>
                            <For
                                each=move || store.history.get()
                                key=|entry| entry.id
                                let:entry
                            >
                                <HistoryRow entry />
                            </For>
                        </tbody>
                    </table>
                </div>
            </Show>

            <Show when=move || confirm_open.get()>
                <div
                    class="fixed inset-0 z-[60] flex items-center justify-center bg-black/75 backdrop-blur-sm p-4"
                    on:click=move |ev| {
                        if ev.target() == ev.current_target() {
                            cancel_clear();
                        }
                    }
                >
                    <div class="relative w-full max-w-md rounded-2xl border border-white/[0.12] bg-zinc-900 shadow-2xl p-6 text-zinc-100">
                        <h2 class="text-base font-semibold tracking-tight">"Clear rule history"</h2>
                        <p class="mt-1 text-sm text-zinc-400">
                            "All audit entries will be removed. This cannot be undone."
                        </p>
                        <div class="mt-5 flex items-center justify-end gap-2">
                            <Button
                                label="Cancel"
                                variant=ButtonVariant::Ghost
                                on_click=Callback::new(move |_| cancel_clear())
                            />
                            {move || {
                                let is_busy = busy.get();
                                view! {
                                    <button
                                        id="rules-history-clear-confirm"
                                        type="button"
                                        class=if is_busy {
                                            format!("{DANGER_BUTTON_CLASS} opacity-50 pointer-events-none")
                                        } else {
                                            DANGER_BUTTON_CLASS.to_string()
                                        }
                                        disabled=is_busy
                                        on:click=move |_| clear_history()
                                    >
                                        {if is_busy { "Clearing…" } else { "Clear history" }}
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

#[component]
fn HistoryRow(entry: HistoryEntry) -> impl IntoView {
    let when = format_history_date(entry.at);
    let rule = if entry.rule_id == 0 {
        "Seed policy".to_string()
    } else if entry.rule_name.is_empty() {
        format!("Rule {}", entry.rule_id)
    } else {
        entry.rule_name.clone()
    };
    let torrent_name = entry.torrent_name.clone();
    let hash = entry.torrent_hash.clone();
    let actions = if entry.actions.is_empty() {
        "—".to_string()
    } else {
        entry.actions.join(", ")
    };
    let reason = entry.reason.clone();
    let error = entry.error.clone();
    let (label, kind) = status_badge(entry.status);

    view! {
        <tr class="border-b border-white/[0.04] transition-colors duration-150 last:border-0 hover:bg-white/[0.03]">
            <td class="whitespace-nowrap px-3 py-2.5 font-mono text-xs tabular-nums text-zinc-500">
                {when}
            </td>
            <td class="whitespace-nowrap px-3 py-2.5 text-xs text-zinc-400">{rule}</td>
            <td class="max-w-72 px-3 py-2.5">
                <div class="truncate text-sm font-medium text-zinc-100" title=torrent_name.clone()>
                    {torrent_name.clone()}
                </div>
                <span
                    class="rounded bg-white/[0.06] px-1.5 py-0.5 font-mono text-[10px] text-zinc-500"
                    title=hash.clone()
                >
                    {short_hash(&hash)}
                </span>
            </td>
            <td class="max-w-56 px-3 py-2.5">
                <span class="block truncate font-mono text-xs text-zinc-300" title=actions.clone()>
                    {actions.clone()}
                </span>
            </td>
            <td class="px-3 py-2.5">
                <Badge label=label kind=kind />
            </td>
            <td class="max-w-72 px-3 py-2.5">
                <div class="truncate text-xs text-zinc-400" title=reason.clone()>
                    {reason.clone()}
                </div>
                {error.map(|error| {
                    view! {
                        <div class="mt-0.5 truncate text-xs text-rose-400" title=error.clone()>
                            {error.clone()}
                        </div>
                    }
                })}
            </td>
        </tr>
    }
}

/// Local-time "YYYY-MM-DD HH:MM:SS" for a unix-seconds timestamp (copied from
/// the RSS history panel, private there).
fn format_history_date(unix_secs: u64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(unix_secs as f64 * 1000.0));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds(),
    )
}

fn status_badge(status: HistoryStatus) -> (&'static str, BadgeKind) {
    match status {
        HistoryStatus::Ok => ("ok", BadgeKind::Success),
        HistoryStatus::Failed => ("failed", BadgeKind::Error),
        HistoryStatus::Skipped => ("skipped", BadgeKind::Warning),
    }
}

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
