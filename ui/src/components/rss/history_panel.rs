use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsValue;

use crate::components::ui::{Badge, BadgeKind, BentoCard};
use crate::state::rss::{
    rss_clear_history, DownloadStatus, HistoryEntry, RssPageStore, RssRule, RuleId,
};
use crate::state::store::{AppState, ToastKind};

/// History tab: auto-download history list with a clear action.
#[component]
pub fn HistoryPanel() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = use_context::<RssPageStore>().expect("RSS page provides RssPageStore");
    let busy = RwSignal::new(false);

    let clear_history = move || {
        if busy.get() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match rss_clear_history(&state).await {
                Ok(()) => {
                    state.push_toast(
                        ToastKind::Success,
                        "History cleared",
                        "All download history entries were removed.",
                    );
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Could not clear history", err.to_string());
                }
            }
            busy.set(false);
        });
    };

    view! {
        <BentoCard
            title="Download History"
            subtitle="Torrents added by RSS rules and manual downloads.".to_string()
            actions=view! {
                // Native button instead of the kit Button: the QA contract needs
                // id="rss-history-clear" on the button element itself and Button
                // exposes no id prop. Classes mirror ButtonVariant::Danger.
                <button
                    id="rss-history-clear"
                    type="button"
                    class=move || {
                        let base = "inline-flex items-center justify-center gap-2 rounded-xl bg-rose-600/90 hover:bg-rose-500 px-3.5 py-2 text-sm font-medium text-white active:scale-[0.98] transition-all cursor-pointer";
                        if busy.get() {
                            format!("{base} opacity-50 pointer-events-none")
                        } else {
                            base.to_string()
                        }
                    }
                    disabled=move || busy.get()
                    on:click=move |_| clear_history()
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
                            <p class="text-sm font-medium text-zinc-300">
                                "No downloads recorded yet"
                            </p>
                            <p class="text-xs text-zinc-500">
                                "Torrents added by RSS rules or manual downloads will appear here."
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
                                <th class="px-3 py-2 font-medium">"Title"</th>
                                <th class="px-3 py-2 font-medium">"Rule"</th>
                                <th class="px-3 py-2 font-medium">"Hash"</th>
                                <th class="px-3 py-2 font-medium">"Status"</th>
                            </tr>
                        </thead>
                        <tbody>
                            <For
                                each=move || store.history.get()
                                key=|entry| entry.id
                                let:entry
                            >
                                <HistoryRow store entry=entry />
                            </For>
                        </tbody>
                    </table>
                </div>
            </Show>
        </BentoCard>
    }
}

#[component]
fn HistoryRow(store: RssPageStore, entry: HistoryEntry) -> impl IntoView {
    let when = format_history_date(entry.at);
    let title = entry.title;
    let error = entry.error;
    let hash = entry.torrent_hash;
    let rule_id = entry.rule_id;
    let (label, kind) = status_badge(entry.status);

    view! {
        <tr class="border-b border-white/[0.04] transition-colors duration-150 last:border-0 hover:bg-white/[0.03]">
            <td class="whitespace-nowrap px-3 py-2.5 font-mono text-xs tabular-nums text-zinc-500">
                {when}
            </td>
            <td class="max-w-96 px-3 py-2.5">
                <div class="truncate text-sm font-medium text-zinc-100" title=title.clone()>
                    {title.clone()}
                </div>
                {error.map(|error| {
                    view! {
                        <div class="mt-0.5 truncate text-xs text-rose-400" title=error.clone()>
                            {error.clone()}
                        </div>
                    }
                })}
            </td>
            <td class="whitespace-nowrap px-3 py-2.5 text-xs text-zinc-400">
                {move || rule_name(&store.rules.get(), rule_id)}
            </td>
            <td class="px-3 py-2.5">
                {match hash {
                    Some(hash) => {
                        view! {
                            <span
                                class="rounded bg-white/[0.06] px-1.5 py-0.5 font-mono text-[10px] text-zinc-400"
                                title=hash.clone()
                            >
                                {short_hash(&hash)}
                            </span>
                        }
                            .into_any()
                    }
                    None => view! { <span class="text-xs text-zinc-600">"—"</span> }.into_any(),
                }}
            </td>
            <td class="px-3 py-2.5">
                <Badge label=label kind=kind />
            </td>
        </tr>
    }
}

/// Local-time "YYYY-MM-DD HH:MM:SS" for a unix-seconds timestamp.
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

fn status_badge(status: DownloadStatus) -> (&'static str, BadgeKind) {
    match status {
        DownloadStatus::Added => ("Added", BadgeKind::Success),
        DownloadStatus::Duplicate => ("Duplicate", BadgeKind::Warning),
        DownloadStatus::Failed => ("Failed", BadgeKind::Error),
    }
}

fn rule_name(rules: &[RssRule], rule_id: Option<RuleId>) -> String {
    match rule_id {
        None => "Manual".to_string(),
        Some(id) => rules
            .iter()
            .find(|rule| rule.id == id)
            .map(|rule| rule.name.clone())
            .unwrap_or_else(|| format!("Rule {id}")),
    }
}

/// First 10 characters with an ellipsis when truncated; the full hash stays
/// available through the title attribute.
fn short_hash(hash: &str) -> String {
    let short: String = hash.chars().take(10).collect();
    if short.len() == hash.len() {
        short
    } else {
        format!("{short}…")
    }
}
