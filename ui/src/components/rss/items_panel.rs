//! Items tab: paged item browser with debounced search, feed filter and
//! manual download. The panel owns item fetching (the page only loads
//! feeds/rules), so pagination state lives here instead of in the store.

use gloo_timers::callback::Timeout;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsValue;

use crate::components::ui::{Badge, BadgeKind, BentoCard, Button, ButtonVariant};
use crate::state::rss::{
    rss_download_item, rss_list_items, DownloadStatus, ItemFilter, RssPageStore,
};
use crate::state::{AppState, ToastKind};

const PAGE_SIZE: usize = 50;
const SEARCH_DEBOUNCE_MS: u32 = 300;

// Copied from pages/settings.rs (private there) so inputs match the app styling.
const INPUT_CLASS: &str = "w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all";

const SELECT_CLASS: &str = "cursor-pointer rounded-xl border border-white/[0.08] bg-zinc-950/80 px-3 py-2 text-sm text-zinc-200 transition-all focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20";

/// Items tab: paged item list with search and download actions.
#[component]
pub fn ItemsPanel() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = use_context::<RssPageStore>().expect("RssPageStore provided by the RSS page");

    let offset = RwSignal::new(0usize);
    let query = RwSignal::new(store.search.get_untracked());
    // Timeout owns a JS closure and is !Send, so it needs thread-local storage.
    let debounce: RwSignal<Option<Timeout>, LocalStorage> = RwSignal::new_local(None);
    let downloading = RwSignal::new(false);
    let request_gen = RwSignal::new(0u64);

    Effect::new(move |_| {
        // Subscribe to mutation bumps from any panel.
        let _reload = store.reload.get();
        let search = store.search.get();
        let feed_id = store.feed_filter.get();
        let page_offset = offset.get();
        let filter = ItemFilter {
            feed_id,
            search: if search.is_empty() {
                None
            } else {
                Some(search)
            },
            limit: PAGE_SIZE,
            offset: page_offset,
        };
        request_gen.update(|gen| *gen += 1);
        let gen = request_gen.get_untracked();
        spawn_local(async move {
            match rss_list_items(&state, &filter).await {
                Ok(page) => {
                    // Drop stale responses superseded by a newer request.
                    if request_gen.get_untracked() == gen {
                        store.items.set(page.items);
                        store.items_total.set(page.total);
                    }
                }
                Err(err) => {
                    if request_gen.get_untracked() == gen {
                        state.push_toast(ToastKind::Error, "Could not load items", err.to_string());
                    }
                }
            }
        });
    });

    let download_item = Callback::new(move |id: u64| {
        if downloading.get_untracked() {
            return;
        }
        downloading.set(true);
        spawn_local(async move {
            match rss_download_item(&state, id).await {
                Ok(outcome) => {
                    match outcome.status {
                        DownloadStatus::Added => {
                            let message = match outcome.torrent_hash {
                                Some(hash) => format!("Added to rtorrent ({hash})."),
                                None => "Added to rtorrent.".to_string(),
                            };
                            state.push_toast(ToastKind::Success, "Torrent added", message);
                        }
                        DownloadStatus::Duplicate => {
                            state.push_toast(
                                ToastKind::Warning,
                                "Already downloaded",
                                "This item is already in rtorrent.",
                            );
                        }
                        DownloadStatus::Failed => {
                            let message = outcome.error.unwrap_or_else(|| {
                                "The backend could not download this item.".to_string()
                            });
                            state.push_toast(ToastKind::Error, "Download failed", message);
                        }
                    }
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Download failed", err.to_string());
                }
            }
            downloading.set(false);
        });
    });

    view! {
        <BentoCard
            title="Items"
            subtitle="Browse feed items and download them manually.".to_string()
        >
            <div class="flex flex-col gap-4">
                <div class="flex flex-wrap items-center gap-3">
                    <div class="min-w-56 flex-1">
                        <input
                            id="rss-items-search"
                            type="text"
                            class=INPUT_CLASS
                            placeholder="Search items by title..."
                            autocomplete="off"
                            prop:value=move || query.get()
                            on:input=move |ev| {
                                let value = event_target_value(&ev);
                                query.set(value.clone());
                                // Replacing the handle drops (and cancels) the pending timeout.
                                debounce.set(Some(Timeout::new(SEARCH_DEBOUNCE_MS, move || {
                                    batch(|| {
                                        offset.set(0);
                                        store.search.set(value);
                                    });
                                })));
                            }
                        />
                    </div>
                    <select
                        class=SELECT_CLASS
                        title="Filter by feed"
                        prop:value=move || {
                            // Track feeds so the value is reapplied once options exist.
                            let _ = store.feeds.get();
                            store.feed_filter.get().map(|id| id.to_string()).unwrap_or_default()
                        }
                        on:change=move |ev| {
                            let feed_id = event_target_value(&ev).parse::<u64>().ok();
                            batch(|| {
                                offset.set(0);
                                store.feed_filter.set(feed_id);
                            });
                        }
                    >
                        <option value="">"All feeds"</option>
                        <For each=move || store.feeds.get() key=|feed| feed.id let(feed)>
                            <option value=feed.id.to_string()>
                                {move || {
                                    store.feeds.get().into_iter()
                                        .find(|f| f.id == feed.id)
                                        .and_then(|f| f.alias)
                                        .unwrap_or_else(|| format!("Feed {}", feed.id))
                                }}
                            </option>
                        </For>
                    </select>
                </div>

                <Show
                    when=move || !store.items.get().is_empty()
                    fallback=move || empty_state(store)
                >
                    <div class="overflow-x-auto rounded-xl border border-white/[0.08] bg-zinc-950/40">
                        <table class="w-full text-left text-sm">
                            <thead>
                                <tr class="border-b border-white/[0.08] bg-zinc-950/40 text-[11px] uppercase tracking-wider text-zinc-500">
                                    <th class="dense-row px-4 py-3 font-medium">"Title"</th>
                                    <th class="dense-row px-4 py-3 font-medium">"Feed"</th>
                                    <th class="dense-row px-4 py-3 font-medium">"Size"</th>
                                    <th class="dense-row px-4 py-3 font-medium">"Published"</th>
                                    <th class="dense-row px-4 py-3 font-medium">"Status"</th>
                                    <th class="dense-row px-4 py-3 font-medium">"Rule"</th>
                                    <th class="dense-row px-4 py-3">
                                        <span class="sr-only">"Actions"</span>
                                    </th>
                                </tr>
                            </thead>
                            <tbody>
                                <For each=move || store.items.get() key=|item| item.id let(item)>
                                    <ItemRow store id=item.id downloading on_download=download_item />
                                </For>
                            </tbody>
                        </table>
                    </div>
                </Show>

                <div class="flex flex-wrap items-center justify-between gap-3">
                    <span class="font-mono text-xs tabular-nums text-zinc-500">
                        {move || {
                            let total = store.items_total.get();
                            let current = offset.get();
                            let shown = store.items.get().len();
                            let start = if total == 0 { 0 } else { current + 1 };
                            let end = (current + shown).min(total);
                            format!("Showing {start}–{end} of {total}")
                        }}
                    </span>
                    {move || {
                        let total = store.items_total.get();
                        let current = offset.get();
                        view! {
                            <div class="flex items-center gap-2">
                                <Button
                                    label="Prev"
                                    variant=ButtonVariant::Secondary
                                    disabled=current == 0
                                    on_click=Callback::new(move |_| {
                                        offset.update(|o| *o = o.saturating_sub(PAGE_SIZE));
                                    })
                                />
                                <Button
                                    label="Next"
                                    variant=ButtonVariant::Secondary
                                    disabled={current + PAGE_SIZE >= total}
                                    on_click=Callback::new(move |_| {
                                        offset.update(|o| *o += PAGE_SIZE);
                                    })
                                />
                            </div>
                        }
                    }}
                </div>
            </div>
        </BentoCard>
    }
}

// For reuses rows by id, so each row resolves its item live from the store to
// pick up download records after a reload (same pattern as ExplorerRow).
#[component]
fn ItemRow(
    store: RssPageStore,
    id: u64,
    downloading: RwSignal<bool>,
    on_download: Callback<u64>,
) -> impl IntoView {
    let item = Memo::new(move |_| store.items.get().into_iter().find(|i| i.id == id));

    view! {
        <tr class="dense-row border-b border-white/[0.04] transition-colors duration-150 last:border-0 hover:bg-white/[0.03]">
            <td class="dense-row px-4 py-3">
                <div class="flex flex-col gap-0.5">
                    {move || {
                        item.get()
                            .map(|i| match i.link {
                                Some(link) => view! {
                                    <a
                                        href=link
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        class="max-w-64 truncate text-sm font-medium text-zinc-100 transition-colors hover:text-[var(--accent)] sm:max-w-96"
                                        title=i.title.clone()
                                    >
                                        {i.title.clone()}
                                    </a>
                                }
                                    .into_any(),
                                None => view! {
                                    <span
                                        class="max-w-64 truncate text-sm font-medium text-zinc-100 sm:max-w-96"
                                        title=i.title.clone()
                                    >
                                        {i.title.clone()}
                                    </span>
                                }
                                    .into_any(),
                            })
                            .unwrap_or_else(|| ().into_any())
                    }}
                    {move || {
                        item.get()
                            .and_then(|i| i.download)
                            .and_then(|d| d.error)
                            .map(|error| {
                                view! {
                                    <span
                                        class="max-w-96 truncate text-[10px] text-rose-400/90"
                                        title=error.clone()
                                    >
                                        {error.clone()}
                                    </span>
                                }
                                    .into_any()
                            })
                            .unwrap_or_else(|| ().into_any())
                    }}
                </div>
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 text-xs text-zinc-400">
                {move || {
                    item.get()
                        .map(|i| {
                            store.feeds.get().into_iter()
                                .find(|f| f.id == i.feed_id)
                                .and_then(|f| f.alias)
                                .unwrap_or_else(|| format!("Feed {}", i.feed_id))
                        })
                        .unwrap_or_default()
                }}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-zinc-300">
                {move || item.get().map(|i| format_size(i.size_bytes)).unwrap_or_default()}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-zinc-500">
                {move || item.get().map(|i| format_published(i.published_at)).unwrap_or_default()}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3">
                {move || {
                    item.get()
                        .and_then(|i| i.download)
                        .map(|d| {
                            let (label, kind) = status_badge(d.status);
                            view! { <Badge label=label kind=kind /> }.into_any()
                        })
                        .unwrap_or_else(|| "—".into_any())
                }}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3">
                {move || {
                    item.get()
                        .and_then(|i| i.download)
                        .and_then(|d| d.rule_id)
                        .map(|rule_id| {
                            let name = store.rules.get().into_iter()
                                .find(|r| r.id == rule_id)
                                .map(|r| r.name)
                                .unwrap_or_else(|| format!("Rule {rule_id}"));
                            view! { <Badge label=name kind=BadgeKind::Info /> }.into_any()
                        })
                        .unwrap_or_else(|| "—".into_any())
                }}
            </td>
            <td class="dense-row px-4 py-3">
                {move || {
                    let Some(i) = item.get() else {
                        return ().into_any();
                    };
                    if i.download.is_some() {
                        return ().into_any();
                    }
                    // Button props are plain values, so re-render on busy changes.
                    let busy = downloading.get();
                    view! {
                        <Button
                            label="Download"
                            variant=ButtonVariant::Secondary
                            disabled=busy
                            on_click=Callback::new(move |_| on_download.run(id))
                        />
                    }
                        .into_any()
                }}
            </td>
        </tr>
    }
}

fn empty_state(store: RssPageStore) -> impl IntoView {
    let unfiltered = move || store.search.get().is_empty() && store.feed_filter.get().is_none();
    view! {
        <div class="flex flex-col items-center justify-center gap-2 rounded-xl border border-white/[0.08] bg-zinc-950/40 px-6 py-12 text-center">
            <p class="text-sm font-semibold tracking-tight text-zinc-200">
                {move || {
                    if unfiltered() { "No items yet" } else { "No items match your search" }
                }}
            </p>
            <p class="text-xs text-zinc-500">
                {move || {
                    if unfiltered() {
                        "Items appear here after a feed is refreshed."
                    } else {
                        "Try a different search term or feed filter."
                    }
                }}
            </p>
        </div>
    }
}

fn status_badge(status: DownloadStatus) -> (&'static str, BadgeKind) {
    match status {
        DownloadStatus::Added => ("Added", BadgeKind::Success),
        DownloadStatus::Duplicate => ("Duplicate", BadgeKind::Warning),
        DownloadStatus::Failed => ("Failed", BadgeKind::Error),
    }
}

fn format_size(bytes: Option<u64>) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let Some(bytes) = bytes else {
        return "—".to_string();
    };
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes} B");
    }
    let mut rendered = format!("{value:.1}");
    if rendered.ends_with(".0") {
        rendered.truncate(rendered.len() - 2);
    }
    format!("{rendered} {}", UNITS[unit])
}

// Manual UTC formatter: js-sys locale methods need explicit locale/options
// arguments, and a fixed format keeps the column sortable and unambiguous.
fn format_published(secs: Option<u64>) -> String {
    let Some(secs) = secs else {
        return "—".to_string();
    };
    let date = js_sys::Date::new(&JsValue::from_f64(secs as f64 * 1000.0));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        date.get_utc_full_year(),
        date.get_utc_month() + 1,
        date.get_utc_date(),
        date.get_utc_hours(),
        date.get_utc_minutes(),
    )
}
