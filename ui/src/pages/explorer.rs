use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::ui::*;
use crate::state::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StatusCounts {
    total: usize,
    completed: usize,
    downloading: usize,
    warning: usize,
    failed: usize,
}

const STATUS_FILTERS: [(&str, &str); 5] = [
    ("all", "All"),
    ("completed", "Completed"),
    ("downloading", "Downloading"),
    ("warning", "Warning"),
    ("failed", "Failed"),
];

#[component]
pub fn ExplorerPage() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();

    let counts = Memo::new(move |_| {
        let mut c = StatusCounts::default();
        state.items.with(|items| {
            for item in items.iter() {
                c.total += 1;
                match item.status() {
                    ItemStatus::Completed => c.completed += 1,
                    ItemStatus::Downloading | ItemStatus::Checking => c.downloading += 1,
                    ItemStatus::Warning => c.warning += 1,
                    ItemStatus::Failed => c.failed += 1,
                }
            }
        });
        c
    });

    let current_page = RwSignal::new(1usize);
    let page_size = RwSignal::new(50usize);

    Effect::new(move |_| {
        state.search_query.track();
        state.status_filter.track();
        state.label_filter.track();
        state.sort_key.track();
        state.sort_desc.track();
        current_page.set(1);
    });

    let filtered = Memo::new(move |_| state.filtered_items());
    let total_filtered = Memo::new(move |_| filtered.with(|items| items.len()));
    let total_pages = Memo::new(move |_| {
        let total = total_filtered.get();
        let size = page_size.get().max(1);
        total.div_ceil(size).max(1)
    });

    let paged_items = Memo::new(move |_| {
        let size = page_size.get().max(1);
        let pages = total_pages.get();
        let cur = current_page.get().clamp(1, pages);
        let start = (cur - 1) * size;
        filtered.with(|items| {
            items
                .iter()
                .skip(start)
                .take(size)
                .cloned()
                .collect::<Vec<_>>()
        })
    });

    view! {
        <div class="flex flex-col gap-4">
            {toolbar(state, counts)}
            {label_facet(state)}
            {bulk_bar(state)}
            <Show
                when=move || total_filtered.get() != 0
                fallback=move || empty_state(state)
            >
                <Show
                    when=move || state.view_mode.get() == ViewMode::Table
                    fallback=move || card_grid(state, paged_items)
                >
                    {table_view(state, paged_items)}
                </Show>
                {pagination_controls(current_page, page_size, total_filtered, total_pages)}
            </Show>
        </div>
        <seed_limits_dialog::SeedLimitsDialog/>
    }
}

/// Label facet (plan.md §6.9, T11): multi-select chips with live counters
/// over the distinct `category` values, plus a "No label" bucket
/// ([`LABEL_FILTER_NONE`]) and an "All" reset. Filtering itself lives in
/// `AppState::filtered_items`, so it survives WS refreshes.
fn label_facet(state: AppState) -> impl IntoView {
    let counts = Memo::new(move |_| state.items.with(|items| label_counts(items)));
    let total = Memo::new(move |_| state.items.with(|items| items.len()));
    let none_active = Signal::derive(move || {
        state
            .label_filter
            .get()
            .iter()
            .any(|s| s == LABEL_FILTER_NONE)
    });
    view! {
        <div class="flex flex-wrap items-center gap-2">
            <span class="mr-1 text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
                "Labels"
            </span>
            <button
                class=move || facet_pill_classes(state.label_filter.get().is_empty())
                on:click=move |_| state.label_filter.set(Vec::new())
                title="Show all labels"
            >
                "All"
                <span class="rounded-full bg-white/[0.08] px-1.5 py-0.5 font-mono text-[10px] tabular-nums leading-none">
                    {move || total.get()}
                </span>
            </button>
            <For
                each=move || counts.get().0
                key=|(label, _)| label.clone()
                let(item)
            >
                {
                    let (label, _) = item;
                    let active_label = label.clone();
                    let toggle_label = label.clone();
                    let count_label = label.clone();
                    let title_label = label.clone();
                    let count = Signal::derive(move || {
                        counts
                            .get()
                            .0
                            .iter()
                            .find(|(name, _)| name == &count_label)
                            .map(|(_, count)| *count)
                            .unwrap_or(0)
                    });
                    let active = Signal::derive(move || {
                        state.label_filter.get().contains(&active_label)
                    });
                    view! {
                        <button
                            class=move || facet_pill_classes(active.get())
                            on:click=move |_| state.toggle_label_filter(&toggle_label)
                            title=move || format!("Toggle label '{title_label}'")
                        >
                            {label}
                            <span class="rounded-full bg-white/[0.08] px-1.5 py-0.5 font-mono text-[10px] tabular-nums leading-none">
                                {move || count.get()}
                            </span>
                        </button>
                    }
                }
            </For>
            <button
                class=move || facet_pill_classes(none_active.get())
                on:click=move |_| state.toggle_label_filter(LABEL_FILTER_NONE)
                title="Show torrents without a label"
            >
                "No label"
                <span class="rounded-full bg-white/[0.08] px-1.5 py-0.5 font-mono text-[10px] tabular-nums leading-none">
                    {move || counts.get().1}
                </span>
            </button>
        </div>
    }
}

fn facet_pill_classes(active: bool) -> String {
    let base = "inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-3 py-1.5 text-xs font-medium transition-all duration-200 active:scale-[0.98]";
    let skin = if active {
        "border-[var(--accent)]/40 bg-[var(--accent)]/15 text-zinc-100"
    } else {
        "border-white/[0.08] bg-white/[0.04] text-zinc-400 hover:bg-white/[0.08] hover:text-zinc-200"
    };
    format!("{base} {skin}")
}

fn toolbar(state: AppState, counts: Memo<StatusCounts>) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-3">
            <div class="flex flex-wrap items-center gap-3">
                <div class="relative min-w-56 flex-1">
                    <svg
                        xmlns="http://www.w3.org/2000/svg"
                        class="pointer-events-none absolute left-3.5 top-1/2 h-4 w-4 -translate-y-1/2 text-zinc-500"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    >
                        <circle cx="11" cy="11" r="8"/>
                        <path d="m21 21-4.35-4.35"/>
                    </svg>
                    <input
                        type="text"
                        class="w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 pl-10 pr-9 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all"
                        placeholder="Search by name, hash, or category..."
                        prop:value=move || state.search_query.get()
                        on:input=move |ev| state.search_query.set(event_target_value(&ev))
                    />
                    <Show when=move || !state.search_query.get().is_empty()>
                        <button
                            class="absolute right-3 top-1/2 -translate-y-1/2 cursor-pointer rounded-md p-0.5 text-zinc-500 transition-all duration-150 hover:bg-white/[0.06] hover:text-zinc-200"
                            title="Clear search"
                            on:click=move |_| state.search_query.set(String::new())
                        >
                            <svg
                                xmlns="http://www.w3.org/2000/svg"
                                class="h-3.5 w-3.5"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="2"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                            >
                                <path d="M18 6 6 18"/>
                                <path d="m6 6 12 12"/>
                            </svg>
                        </button>
                    </Show>
                </div>

                <Show when=move || state.view_mode.get() == ViewMode::Cards>
                    <div class="flex items-center gap-2">
                        <select
                            class="cursor-pointer rounded-xl border border-white/[0.08] bg-zinc-950/80 px-3 py-2 text-sm text-zinc-200 transition-all focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20"
                            title="Sort by"
                            prop:value=move || sort_key_value(state.sort_key.get())
                            on:change=move |ev| {
                                state.sort_key.set(sort_key_from_value(&event_target_value(&ev)));
                            }
                        >
                            <option value="name">"Name"</option>
                            <option value="down_rate">"Down Rate"</option>
                            <option value="up_rate">"Up Rate"</option>
                            <option value="size">"Size"</option>
                            <option value="progress">"Progress"</option>
                            <option value="status">"Status"</option>
                            <option value="category">"Label"</option>
                            <option value="updated">"Updated"</option>
                        </select>
                        <button
                            class="inline-flex cursor-pointer items-center justify-center rounded-xl border border-white/[0.08] bg-white/[0.04] p-2 text-zinc-300 transition-all hover:bg-white/[0.08] active:scale-[0.98]"
                            title=move || {
                                if state.sort_desc.get() {
                                    "Sorted descending — click for ascending"
                                } else {
                                    "Sorted ascending — click for descending"
                                }
                            }
                            on:click=move |_| state.sort_desc.update(|d| *d = !*d)
                        >
                            <svg
                                xmlns="http://www.w3.org/2000/svg"
                                class="h-4 w-4 transition-transform duration-200"
                                class=("rotate-180", move || !state.sort_desc.get())
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="2"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                            >
                                <path d="M12 5v14"/>
                                <path d="m19 12-7 7-7-7"/>
                            </svg>
                        </button>
                    </div>
                </Show>

                <div class="flex items-center gap-1 rounded-xl border border-white/[0.08] bg-white/[0.04] p-1">
                    <button
                        class=move || view_toggle_classes(state.view_mode.get() == ViewMode::Table)
                        on:click=move |_| state.view_mode.set(ViewMode::Table)
                    >
                        <svg
                            xmlns="http://www.w3.org/2000/svg"
                            class="h-3.5 w-3.5"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        >
                            <path d="M12 3v18"/>
                            <rect width="18" height="18" x="3" y="3" rx="2"/>
                            <path d="M3 9h18"/>
                            <path d="M3 15h18"/>
                        </svg>
                        "Table"
                    </button>
                    <button
                        class=move || view_toggle_classes(state.view_mode.get() == ViewMode::Cards)
                        on:click=move |_| state.view_mode.set(ViewMode::Cards)
                    >
                        <svg
                            xmlns="http://www.w3.org/2000/svg"
                            class="h-3.5 w-3.5"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        >
                            <rect width="7" height="7" x="3" y="3" rx="1"/>
                            <rect width="7" height="7" x="14" y="3" rx="1"/>
                            <rect width="7" height="7" x="14" y="14" rx="1"/>
                            <rect width="7" height="7" x="3" y="14" rx="1"/>
                        </svg>
                        "Cards"
                    </button>
                </div>
            </div>

            <div class="flex flex-wrap items-center gap-2">
                {STATUS_FILTERS
                    .iter()
                    .map(|(value, label)| {
                        let value = *value;
                        let label = *label;
                        view! {
                            <button
                                class=move || pill_classes(value, state.status_filter.get() == value)
                                on:click=move |_| state.status_filter.set(value.to_string())
                            >
                                {label}
                                <span class="rounded-full bg-white/[0.08] px-1.5 py-0.5 font-mono text-[10px] tabular-nums leading-none">
                                    {move || pill_count(counts.get(), value)}
                                </span>
                            </button>
                        }
                    })
                    .collect_view()}
                <span class="ml-auto whitespace-nowrap font-mono text-xs tabular-nums text-zinc-500">
                    {move || {
                        format!(
                            "{} of {} torrents",
                            state.filtered_items().len(),
                            state.items.get().len()
                        )
                    }}
                </span>
            </div>
        </div>
    }
}

fn bulk_bar(state: AppState) -> impl IntoView {
    view! {
        <Show when=move || !state.selected_hashes.get().is_empty()>
            <div class="flex flex-wrap items-center gap-3 rounded-xl border border-[var(--accent)]/25 bg-[var(--accent)]/10 px-4 py-2.5 transition-all duration-200">
                <span class="font-mono text-sm font-medium tabular-nums text-zinc-100">
                    {move || format!("{} selected", state.selected_hashes.get().len())}
                </span>
                <div class="ml-auto flex flex-wrap items-center gap-2">
                    <Button
                        label="Start"
                        variant=ButtonVariant::Secondary
                        on_click=Callback::new(move |_| run_bulk(state, "start"))
                    />
                    <Button
                        label="Stop"
                        variant=ButtonVariant::Secondary
                        on_click=Callback::new(move |_| run_bulk(state, "stop"))
                    />
                    <Button
                        label="Remove"
                        variant=ButtonVariant::Danger
                        on_click=Callback::new(move |_| open_bulk_remove(state))
                    />
                    <Button
                        label="Clear"
                        variant=ButtonVariant::Ghost
                        on_click=Callback::new(move |_| state.selected_hashes.set(Vec::new()))
                    />
                </div>
            </div>
        </Show>
    }
}

fn sortable_th(state: AppState, key: SortKey, label: &'static str) -> impl IntoView {
    let on_click = move |_| {
        if state.sort_key.get() == key {
            state.sort_desc.update(|d| *d = !*d);
        } else {
            state.sort_key.set(key);
            state.sort_desc.set(!matches!(key, SortKey::Name));
        }
    };

    view! {
        <th
            class=move || {
                let active = state.sort_key.get() == key;
                if active {
                    "dense-row px-4 py-3 font-semibold text-zinc-100 select-none cursor-pointer group transition-colors duration-150"
                } else {
                    "dense-row px-4 py-3 font-medium text-zinc-500 hover:text-zinc-300 select-none cursor-pointer group transition-colors duration-150"
                }
            }
            on:click=on_click
            title=move || {
                if state.sort_key.get() == key {
                    if state.sort_desc.get() {
                        format!("Sorted by {label} descending — click for ascending")
                    } else {
                        format!("Sorted by {label} ascending — click for descending")
                    }
                } else {
                    format!("Sort by {label}")
                }
            }
        >
            <div class="inline-flex items-center gap-1.5">
                <span>{label}</span>
                <span class="inline-flex items-center">
                    <Show
                        when=move || state.sort_key.get() == key
                        fallback=move || {
                            view! {
                                <svg
                                    xmlns="http://www.w3.org/2000/svg"
                                    class="h-3 w-3 opacity-0 transition-opacity duration-150 group-hover:opacity-40"
                                    viewBox="0 0 24 24"
                                    fill="none"
                                    stroke="currentColor"
                                    stroke-width="2"
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                >
                                    <path d="m7 15 5 5 5-5"/>
                                    <path d="m7 9 5-5 5 5"/>
                                </svg>
                            }
                        }
                    >
                        <svg
                            xmlns="http://www.w3.org/2000/svg"
                            class="h-3.5 w-3.5 text-indigo-400 transition-transform duration-200"
                            class=("rotate-180", move || !state.sort_desc.get())
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        >
                            <path d="M12 5v14"/>
                            <path d="m19 12-7 7-7-7"/>
                        </svg>
                    </Show>
                </span>
            </div>
        </th>
    }
}

fn table_view(state: AppState, paged_items: Memo<Vec<TorrentItem>>) -> impl IntoView {
    view! {
        <div class="glass-surface relative overflow-hidden rounded-2xl border border-white/[0.08] bg-zinc-900/50 shadow-[0_8px_32px_rgba(0,0,0,0.4)] backdrop-blur-xl transition-all duration-200">
            <div class="overflow-x-auto">
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-white/[0.08] bg-zinc-950/40 text-[11px] uppercase tracking-wider text-zinc-500">
                            <th class="dense-row w-10 px-4 py-3">{select_all_checkbox(state)}</th>
                            {sortable_th(state, SortKey::Name, "Name")}
                            {sortable_th(state, SortKey::Status, "Status")}
                            {sortable_th(state, SortKey::Category, "Label")}
                            {sortable_th(state, SortKey::Size, "Size")}
                            {sortable_th(state, SortKey::Progress, "Progress")}
                            {sortable_th(state, SortKey::Seeds, "Seeds")}
                            {sortable_th(state, SortKey::Peers, "Peers")}
                            {sortable_th(state, SortKey::DownRate, "Down")}
                            {sortable_th(state, SortKey::UpRate, "Up")}
                            {sortable_th(state, SortKey::Ratio, "Ratio")}
                            // Seeded is intentionally not sortable: SortKey is
                            // matched exhaustively outside T11 ownership.
                            <th class="dense-row px-4 py-3 font-medium">"Seeded"</th>
                            {sortable_th(state, SortKey::Eta, "ETA")}
                            {sortable_th(state, SortKey::Finished, "Completed")}
                            {sortable_th(state, SortKey::Updated, "Updated")}
                            <th class="dense-row px-4 py-3">
                                <span class="sr-only">"Actions"</span>
                            </th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || paged_items.get()
                            key=|item| item.hash.clone()
                            let(item)
                        >
                            <ExplorerRow state hash=item.hash.clone()/>
                        </For>
                    </tbody>
                </table>
            </div>
        </div>
    }
}

fn card_grid(state: AppState, paged_items: Memo<Vec<TorrentItem>>) -> impl IntoView {
    view! {
        <div class="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
            <For
                each=move || paged_items.get()
                key=|item| item.hash.clone()
                let(item)
            >
                <ExplorerCard state hash=item.hash.clone()/>
            </For>
        </div>
    }
}

fn empty_state(state: AppState) -> impl IntoView {
    view! {
        <div class="glass-surface relative flex flex-col items-center justify-center gap-4 overflow-hidden rounded-2xl border border-white/[0.08] bg-zinc-900/50 px-6 py-16 text-center shadow-[0_8px_32px_rgba(0,0,0,0.4)] backdrop-blur-xl">
            <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-20 w-20"
                viewBox="0 0 96 96"
                fill="none"
            >
                <circle cx="48" cy="48" r="40" fill="var(--accent)" opacity="0.07"/>
                <circle cx="48" cy="48" r="30" stroke="white" stroke-opacity="0.08"/>
                <path
                    d="M28 42h40l-6 24a4 4 0 0 1-4 3H38a4 4 0 0 1-4-3l-6-24Z"
                    stroke="#71717a"
                    stroke-width="2"
                    stroke-linejoin="round"
                />
                <path
                    d="M24 42l8-14h32l8 14"
                    stroke="#71717a"
                    stroke-width="2"
                    stroke-linejoin="round"
                />
                <path d="M40 54h16" stroke="var(--accent)" stroke-width="2" stroke-linecap="round"/>
                <circle cx="64" cy="28" r="9" stroke="#a1a1aa" stroke-width="2"/>
                <path d="m71 35 6 6" stroke="#a1a1aa" stroke-width="2" stroke-linecap="round"/>
            </svg>
            <div>
                <p class="text-sm font-semibold tracking-tight text-zinc-200">
                    "No torrents match your filters"
                </p>
                <p class="mt-1 text-xs text-zinc-500">
                    "Try a different search term or reset the filters below."
                </p>
            </div>
            <Button
                label="Reset Filters"
                variant=ButtonVariant::Secondary
                on_click=Callback::new(move |_| {
                    state.search_query.set(String::new());
                    state.status_filter.set("all".to_string());
                    state.label_filter.set(Vec::new());
                })
            />
        </div>
    }
}

fn pagination_controls(
    current_page: RwSignal<usize>,
    page_size: RwSignal<usize>,
    total_filtered: Memo<usize>,
    total_pages: Memo<usize>,
) -> impl IntoView {
    let can_prev = move || current_page.get() > 1;
    let can_next = move || current_page.get() < total_pages.get();

    view! {
        <div class="flex flex-wrap items-center justify-between gap-3 px-2 py-2 text-xs text-zinc-400">
            <div class="flex items-center gap-2">
                <span>"Show"</span>
                <select
                    class="cursor-pointer rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2 py-1 text-xs text-zinc-200 transition-all focus:border-indigo-500/60 focus:outline-none"
                    prop:value=move || page_size.get().to_string()
                    on:change=move |ev| {
                        if let Ok(sz) = event_target_value(&ev).parse::<usize>() {
                            page_size.set(sz);
                            current_page.set(1);
                        }
                    }
                >
                    <option value="25">"25"</option>
                    <option value="50">"50"</option>
                    <option value="100">"100"</option>
                    <option value="200">"200"</option>
                    <option value="500">"500"</option>
                    <option value="1000">"1000"</option>
                </select>
                <span>"per page"</span>
                <span class="ml-2 font-mono tabular-nums text-zinc-500">
                    {move || {
                        let total = total_filtered.get();
                        if total == 0 {
                            "0 of 0".to_string()
                        } else {
                            let size = page_size.get();
                            let cur = current_page.get();
                            let start = (cur - 1) * size + 1;
                            let end = (cur * size).min(total);
                            format!("Showing {start}–{end} of {total}")
                        }
                    }}
                </span>
            </div>

            <div class="flex items-center gap-2">
                <button
                    class="cursor-pointer rounded-lg border border-white/[0.08] bg-white/[0.04] px-2.5 py-1 text-zinc-300 transition-all hover:bg-white/[0.08] disabled:cursor-not-allowed disabled:opacity-40"
                    disabled=move || !can_prev()
                    on:click=move |_| current_page.update(|p| *p = p.saturating_sub(1).max(1))
                >
                    "Previous"
                </button>
                <span class="px-2 font-mono text-zinc-300">
                    {move || format!("Page {} of {}", current_page.get(), total_pages.get())}
                </span>
                <button
                    class="cursor-pointer rounded-lg border border-white/[0.08] bg-white/[0.04] px-2.5 py-1 text-zinc-300 transition-all hover:bg-white/[0.08] disabled:cursor-not-allowed disabled:opacity-40"
                    disabled=move || !can_next()
                    on:click=move |_| current_page.update(|p| *p = (*p + 1).min(total_pages.get()))
                >
                    "Next"
                </button>
            </div>
        </div>
    }
}

// Rows resolve their item live from state.items by hash: For is keyed by hash,
// so a row instance always maps to the same torrent while its data keeps ticking.
fn item_memo(state: AppState, hash: String) -> Memo<Option<TorrentItem>> {
    Memo::new(move |_| {
        state
            .items
            .with(|items| items.iter().find(|i| i.hash == hash).cloned())
    })
}

#[component]
fn ExplorerRow(state: AppState, hash: String) -> impl IntoView {
    let item = item_memo(state, hash.clone());
    let open_item = item;
    let checkbox = selection_checkbox(state, hash.clone());
    let actions = quick_actions(state, hash);

    view! {
        <tr
            class="dense-row group cursor-pointer border-b border-white/[0.04] transition-colors duration-150 last:border-0 hover:bg-white/[0.03]"
            on:click=move |_| {
                if let Some(i) = open_item.get() {
                    state.selected_item.set(Some(i));
                }
            }
        >
            <td class="dense-row px-4 py-3">{checkbox}</td>
            <td class="dense-row px-4 py-3">
                <div class="flex flex-col gap-0.5">
                    <span class="max-w-64 truncate text-sm font-medium text-zinc-100 sm:max-w-96">
                        {move || item.get().map(|i| i.name).unwrap_or_default()}
                    </span>
                    <div class="flex items-center gap-2">
                        <span class="rounded bg-white/[0.06] px-1.5 py-0.5 font-mono text-[10px] text-zinc-500">
                            {move || item.get().map(|i| i.short_hash()).unwrap_or_default()}
                        </span>
                    </div>
                </div>
            </td>
            <td class="dense-row px-4 py-3">
                {move || {
                    item.get()
                        .map(|i| {
                            view! {
                                <Badge
                                    label=i.status_label()
                                    kind=status_kind(i.status())
                                    pulse=matches!(i.status(), ItemStatus::Downloading | ItemStatus::Checking)
                                />
                            }
                                .into_any()
                        })
                        .unwrap_or_else(|| ().into_any())
                }}
            </td>
            <td class="dense-row px-4 py-3">
                {move || {
                    item.get()
                        .map(|i| {
                            if i.category.is_empty() {
                                view! { <span class="text-xs text-zinc-600">"—"</span> }.into_any()
                            } else {
                                view! {
                                    <span class="inline-flex items-center rounded-md bg-purple-500/10 px-2 py-0.5 text-xs font-medium text-purple-400 border border-purple-500/20">
                                        {i.category}
                                    </span>
                                }
                                .into_any()
                            }
                        })
                        .unwrap_or_else(|| ().into_any())
                }}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-zinc-300">
                {move || item.get().map(|i| format_bytes(i.size_bytes)).unwrap_or_default()}
            </td>
            <td class="dense-row px-4 py-3">{progress_bar(item, "h-1.5 w-24 shrink-0")}</td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-emerald-400/90">
                {move || item.get().map(|i| format!("{}/{}", i.seeds_connected, i.seeds_total)).unwrap_or_default()}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-blue-400/90">
                {move || item.get().map(|i| format!("{}/{}", i.peers_connected, i.peers_total)).unwrap_or_default()}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-emerald-400/90">
                {move || item.get().map(|i| format_rate(i.down_rate)).unwrap_or_default()}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-zinc-400">
                {move || item.get().map(|i| format_rate(i.up_rate)).unwrap_or_default()}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums">
                {move || {
                    item.get().map(|i| {
                        let r = i.ratio();
                        match i.ratio_limit() {
                            // Progress toward the effective ratio limit
                            // (plan §6.9): "1.20 / 3.00" + mini bar.
                            Some(limit) => {
                                let pct = if limit > 0.0 {
                                    (r / limit * 100.0).clamp(0.0, 100.0)
                                } else {
                                    100.0
                                };
                                let bar_class = if r >= limit {
                                    "h-full rounded-full bg-emerald-400 transition-all duration-300"
                                } else {
                                    "h-full rounded-full bg-[var(--accent)] transition-all duration-300"
                                };
                                view! {
                                    <div class="flex flex-col gap-1">
                                        <span class="text-zinc-300">{format!("{r:.2} / {limit:.2}")}</span>
                                        <div class="h-1 w-16 overflow-hidden rounded-full bg-white/[0.06]">
                                            <div class=bar_class style=format!("width: {pct:.1}%")></div>
                                        </div>
                                    </div>
                                }
                                    .into_any()
                            }
                            None => {
                                let style = if r >= 2.0 {
                                    "font-semibold text-purple-400"
                                } else if r >= 1.0 {
                                    "text-emerald-400"
                                } else if r >= 0.5 {
                                    "text-amber-400"
                                } else {
                                    "text-zinc-400"
                                };
                                view! { <span class=style>{format!("{r:.2}")}</span> }.into_any()
                            }
                        }
                    })
                }}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-zinc-400">
                {move || item.get().map(|i| i.format_seeded()).unwrap_or_default()}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-zinc-400">
                {move || item.get().map(|i| i.format_eta()).unwrap_or_default()}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-zinc-500">
                {move || item.get().map(|i| i.format_completed()).unwrap_or_default()}
            </td>
            <td class="dense-row whitespace-nowrap px-4 py-3 font-mono text-xs tabular-nums text-zinc-500">
                {move || {
                    item.get()
                        .map(|i| format_timestamp(i.state_changed * 1000))
                        .unwrap_or_default()
                }}
            </td>
            <td class="dense-row px-4 py-3">{actions}</td>
        </tr>
    }
}

#[component]
fn ExplorerCard(state: AppState, hash: String) -> impl IntoView {
    let item = item_memo(state, hash.clone());
    let open_item = item;
    let checkbox = selection_checkbox(state, hash.clone());
    let actions = quick_actions(state, hash);

    view! {
        <div
            class="glass-surface relative flex cursor-pointer flex-col gap-3 overflow-hidden rounded-2xl border border-white/[0.08] bg-zinc-900/50 p-5 shadow-[0_8px_32px_rgba(0,0,0,0.4)] backdrop-blur-xl transition-all duration-200 hover:border-white/[0.14]"
            on:click=move |_| {
                if let Some(i) = open_item.get() {
                    state.selected_item.set(Some(i));
                }
            }
        >
            <div class="flex items-start justify-between gap-3">
                <div class="min-w-0 flex-1">
                    <p class="truncate text-sm font-semibold tracking-tight text-zinc-100">
                        {move || item.get().map(|i| i.name).unwrap_or_default()}
                    </p>
                    <div class="mt-1.5 flex items-center gap-2">
                        <span class="rounded bg-white/[0.06] px-1.5 py-0.5 font-mono text-[10px] text-zinc-500">
                            {move || item.get().map(|i| i.short_hash()).unwrap_or_default()}
                        </span>
                        {move || {
                            item.get().and_then(|i| {
                                if i.category.is_empty() {
                                    None
                                } else {
                                    Some(view! {
                                        <span class="inline-flex items-center rounded-md bg-purple-500/10 px-1.5 py-0.5 text-[10px] font-medium text-purple-400 border border-purple-500/20">
                                            {i.category}
                                        </span>
                                    })
                                }
                            })
                        }}
                    </div>
                </div>
                {checkbox}
            </div>

            <div class="flex items-center justify-between gap-2">
                {move || {
                    item.get()
                        .map(|i| {
                            view! {
                                <Badge
                                    label=i.status_label()
                                    kind=status_kind(i.status())
                                    pulse=matches!(i.status(), ItemStatus::Downloading | ItemStatus::Checking)
                                />
                            }
                                .into_any()
                        })
                        .unwrap_or_else(|| ().into_any())
                }}
                <span class="font-mono text-xs tabular-nums text-zinc-400">
                    {move || item.get().map(|i| format_bytes(i.size_bytes)).unwrap_or_default()}
                </span>
            </div>

            {progress_bar(item, "h-1.5 flex-1")}

            <div class="grid grid-cols-2 gap-2 rounded-xl border border-white/[0.06] bg-zinc-950/60 p-2.5">
                <div class="flex flex-col gap-0.5">
                    <span class="text-[10px] uppercase tracking-wider text-zinc-500">"Down"</span>
                    <span class="font-mono text-xs tabular-nums text-emerald-400/90">
                        {move || item.get().map(|i| format_rate(i.down_rate)).unwrap_or_default()}
                    </span>
                </div>
                <div class="flex flex-col gap-0.5">
                    <span class="text-[10px] uppercase tracking-wider text-zinc-500">"Up"</span>
                    <span class="font-mono text-xs tabular-nums text-zinc-400">
                        {move || item.get().map(|i| format_rate(i.up_rate)).unwrap_or_default()}
                    </span>
                </div>
                <div class="flex flex-col gap-0.5 border-t border-white/[0.04] pt-1.5">
                    <span class="text-[10px] uppercase tracking-wider text-zinc-500">"Swarm (S/P)"</span>
                    <span class="font-mono text-xs tabular-nums text-zinc-300">
                        {move || {
                            item.get()
                                .map(|i| format!("{}/{}s • {}/{}p", i.seeds_connected, i.seeds_total, i.peers_connected, i.peers_total))
                                .unwrap_or_default()
                        }}
                    </span>
                </div>
                <div class="flex flex-col gap-0.5 border-t border-white/[0.04] pt-1.5">
                    <span class="text-[10px] uppercase tracking-wider text-zinc-500">"Ratio / ETA"</span>
                    <span class="font-mono text-xs tabular-nums text-zinc-300">
                        {move || {
                            item.get()
                                .map(|i| format!("{:.2} • {}", i.ratio(), i.format_eta()))
                                .unwrap_or_default()
                        }}
                    </span>
                </div>
            </div>

            <div class="flex items-center justify-between border-t border-white/[0.06] pt-3 text-[10px] text-zinc-500 font-mono">
                <span>
                    {move || {
                        item.get()
                            .map(|i| {
                                if i.finished > 0 {
                                    format!("Done {}", i.format_completed())
                                } else {
                                    format!("Updated {}", format_timestamp(i.state_changed * 1000))
                                }
                            })
                            .unwrap_or_default()
                    }}
                </span>
                {actions}
            </div>
        </div>
    }
}

fn selection_checkbox(state: AppState, hash: String) -> impl IntoView {
    let checked_hash = hash.clone();
    let click_hash = hash;
    view! {
        <input
            type="checkbox"
            class="h-4 w-4 shrink-0 cursor-pointer rounded border-white/[0.15] bg-zinc-950/80 accent-[var(--accent)]"
            aria-label="Select torrent"
            prop:checked=move || state.selected_hashes.get().contains(&checked_hash)
            on:click=move |ev| {
                ev.stop_propagation();
                toggle_hash(state, &click_hash);
            }
        />
    }
}

fn select_all_checkbox(state: AppState) -> impl IntoView {
    let all_checked = move || {
        let selected = state.selected_hashes.get();
        let filtered = state.filtered_items();
        !filtered.is_empty() && filtered.iter().all(|i| selected.contains(&i.hash))
    };
    view! {
        <input
            type="checkbox"
            class="h-4 w-4 cursor-pointer rounded border-white/[0.15] bg-zinc-950/80 accent-[var(--accent)]"
            aria-label="Select all filtered torrents"
            prop:checked=all_checked
            on:click=move |ev| {
                ev.stop_propagation();
                let filtered: Vec<String> =
                    state.filtered_items().into_iter().map(|i| i.hash).collect();
                let all_in =
                    !filtered.is_empty() && filtered.iter().all(|h| state.selected_hashes.get().contains(h));
                state.selected_hashes.update(|sel| {
                    if all_in {
                        sel.retain(|h| !filtered.contains(h));
                    } else {
                        for h in filtered {
                            if !sel.contains(&h) {
                                sel.push(h);
                            }
                        }
                    }
                });
            }
        />
    }
}

fn quick_actions(state: AppState, hash: String) -> impl IntoView {
    let start_hash = hash.clone();
    let stop_hash = hash.clone();
    let limits_hash = hash;
    view! {
        <div class="flex items-center gap-1">
            <button
                class="cursor-pointer rounded-lg p-1.5 text-zinc-500 transition-all duration-150 hover:bg-emerald-500/10 hover:text-emerald-400 active:scale-[0.95]"
                title="Start"
                on:click=move |ev| {
                    ev.stop_propagation();
                    run_command(state, start_hash.clone(), "start");
                }
            >
                <svg
                    xmlns="http://www.w3.org/2000/svg"
                    class="h-3.5 w-3.5"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                >
                    <polygon points="5 3 19 12 5 21 5 3"/>
                </svg>
            </button>
            <button
                class="cursor-pointer rounded-lg p-1.5 text-zinc-500 transition-all duration-150 hover:bg-rose-500/10 hover:text-rose-400 active:scale-[0.95]"
                title="Stop"
                on:click=move |ev| {
                    ev.stop_propagation();
                    run_command(state, stop_hash.clone(), "stop");
                }
            >
                <svg
                    xmlns="http://www.w3.org/2000/svg"
                    class="h-3.5 w-3.5"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                >
                    <rect x="5" y="5" width="14" height="14" rx="2"/>
                </svg>
            </button>
            <button
                class="cursor-pointer rounded-lg p-1.5 text-zinc-500 transition-all duration-150 hover:bg-indigo-500/10 hover:text-indigo-400 active:scale-[0.95]"
                title="Seed limits…"
                on:click=move |ev| {
                    ev.stop_propagation();
                    state.seed_limits_hash.set(Some(limits_hash.clone()));
                }
            >
                <svg
                    xmlns="http://www.w3.org/2000/svg"
                    class="h-3.5 w-3.5"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                >
                    <line x1="4" x2="4" y1="21" y2="14"/>
                    <line x1="4" x2="4" y1="10" y2="3"/>
                    <line x1="12" x2="12" y1="21" y2="12"/>
                    <line x1="12" x2="12" y1="8" y2="3"/>
                    <line x1="20" x2="20" y1="21" y2="16"/>
                    <line x1="20" x2="20" y1="12" y2="3"/>
                    <line x1="2" x2="6" y1="14" y2="14"/>
                    <line x1="10" x2="14" y1="8" y2="8"/>
                    <line x1="18" x2="22" y1="16" y2="16"/>
                </svg>
            </button>
        </div>
    }
}

fn progress_bar(item: Memo<Option<TorrentItem>>, bar_class: &'static str) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2">
            <div class=move || {
                format!("{bar_class} overflow-hidden rounded-full bg-white/[0.06]")
            }>
                <div
                    class=move || {
                        let is_checking = item.get().map(|i| i.status() == ItemStatus::Checking).unwrap_or(false);
                        if is_checking {
                            "h-full rounded-full bg-gradient-to-r from-amber-500 via-yellow-300 to-amber-500 animate-laser-scan shadow-[0_0_8px_rgba(245,158,11,0.5)] transition-all duration-300"
                        } else {
                            "h-full rounded-full bg-[var(--accent)] transition-all duration-300"
                        }
                    }
                    style=move || {
                        let pct = item.get().map(|i| {
                            if i.status() == ItemStatus::Checking && i.chunks_total > 0 {
                                (i.chunks_hashed as f64 / i.chunks_total as f64) * 100.0
                            } else {
                                i.progress() * 100.0
                            }
                        }).unwrap_or_default();
                        format!("width: {pct:.1}%")
                    }
                ></div>
            </div>
            <span class="font-mono text-xs tabular-nums text-zinc-400">
                {move || item.get().map(|i| {
                    if i.status() == ItemStatus::Checking && i.chunks_total > 0 {
                        format!("{:.1}% [chk]", (i.chunks_hashed as f64 / i.chunks_total as f64) * 100.0)
                    } else {
                        format_percent(i.progress())
                    }
                }).unwrap_or_default()}
            </span>
        </div>
    }
}

fn run_command(state: AppState, hash: String, verb: &'static str) {
    spawn_local(async move {
        // send_command already surfaces success/error toasts.
        let _ = api::send_command(state, &hash, verb).await;
    });
}

/// Opens the remove-confirmation modal for the current bulk selection.
fn open_bulk_remove(state: AppState) {
    let hashes = state.selected_hashes.get();
    if hashes.is_empty() {
        return;
    }
    let label = if hashes.len() == 1 {
        state
            .items
            .get()
            .iter()
            .find(|i| i.hash == hashes[0])
            .map(|i| i.name.clone())
            .unwrap_or_else(|| hashes[0].clone())
    } else {
        format!("{} torrents", hashes.len())
    };
    state.remove_confirm_hashes.set(hashes);
    state.remove_confirm_label.set(label);
    state.remove_confirm_open.set(true);
}

fn run_bulk(state: AppState, verb: &'static str) {
    let hashes = state.selected_hashes.get();
    if hashes.is_empty() {
        return;
    }
    spawn_local(async move {
        for hash in &hashes {
            let _ = api::send_command(state, hash, verb).await;
        }
        if verb == "remove" {
            state.selected_hashes.set(Vec::new());
        }
    });
}

fn toggle_hash(state: AppState, hash: &str) {
    state.selected_hashes.update(|sel| {
        if let Some(pos) = sel.iter().position(|h| h == hash) {
            sel.remove(pos);
        } else {
            sel.push(hash.to_string());
        }
    });
}

fn sort_key_value(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "name",
        SortKey::DownRate => "down_rate",
        SortKey::UpRate => "up_rate",
        SortKey::Size => "size",
        SortKey::Progress => "progress",
        SortKey::Status => "status",
        SortKey::Category => "category",
        SortKey::Updated => "updated",
        SortKey::Seeds => "seeds",
        SortKey::Peers => "peers",
        SortKey::Ratio => "ratio",
        SortKey::Eta => "eta",
        SortKey::Finished => "finished",
    }
}

fn sort_key_from_value(value: &str) -> SortKey {
    match value {
        "name" => SortKey::Name,
        "up_rate" => SortKey::UpRate,
        "size" => SortKey::Size,
        "progress" => SortKey::Progress,
        "status" => SortKey::Status,
        "category" | "label" => SortKey::Category,
        "updated" => SortKey::Updated,
        "seeds" => SortKey::Seeds,
        "peers" => SortKey::Peers,
        "ratio" => SortKey::Ratio,
        "eta" => SortKey::Eta,
        "finished" | "completed" => SortKey::Finished,
        _ => SortKey::DownRate,
    }
}

fn pill_classes(value: &str, active: bool) -> String {
    let base = "inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-3 py-1.5 text-xs font-medium transition-all duration-200 active:scale-[0.98]";
    let skin = if active {
        match value {
            "completed" | "healthy" => "border-emerald-500/20 bg-emerald-500/10 text-emerald-400",
            "downloading" | "processing" => "border-indigo-500/20 bg-indigo-500/10 text-indigo-400",
            "warning" => "border-amber-500/20 bg-amber-500/10 text-amber-400",
            "failed" => "border-rose-500/20 bg-rose-500/10 text-rose-400",
            _ => "border-[var(--accent)]/40 bg-[var(--accent)]/15 text-zinc-100",
        }
    } else {
        "border-white/[0.08] bg-white/[0.04] text-zinc-400 hover:bg-white/[0.08] hover:text-zinc-200"
    };
    format!("{base} {skin}")
}

fn pill_count(counts: StatusCounts, value: &str) -> usize {
    match value {
        "completed" | "healthy" => counts.completed,
        "downloading" | "processing" => counts.downloading,
        "warning" => counts.warning,
        "failed" => counts.failed,
        _ => counts.total,
    }
}

fn view_toggle_classes(active: bool) -> &'static str {
    if active {
        "inline-flex cursor-pointer items-center gap-1.5 rounded-lg bg-[var(--accent)]/15 px-2.5 py-1.5 text-xs font-medium text-zinc-100 transition-all duration-200"
    } else {
        "inline-flex cursor-pointer items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-medium text-zinc-500 transition-all duration-200 hover:text-zinc-300"
    }
}
