use gloo_timers::future::TimeoutFuture;
use leptos::task::spawn_local;
use leptos::{ev, prelude::*};
use leptos_router::hooks::use_navigate;

use crate::state::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaletteAction {
    CreateItem,
    ToggleLive,
    Diagnostics,
    ExportJson,
    ClearFilters,
}

#[derive(Clone, PartialEq)]
enum EntryKind {
    Navigate(&'static str),
    Action(PaletteAction),
    Item(String),
}

#[derive(Clone, PartialEq)]
struct PaletteEntry {
    id: String,
    group: &'static str,
    title: String,
    subtitle: String,
    kind: EntryKind,
}

fn entry_icon(kind: &EntryKind) -> AnyView {
    match kind {
        EntryKind::Navigate(_) => view! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                <path d="M5 12h14" />
                <path d="m12 5 7 7-7 7" />
            </svg>
        }
        .into_any(),
        EntryKind::Action(_) => view! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
            </svg>
        }
        .into_any(),
        EntryKind::Item(_) => view! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                <polyline points="7 10 12 15 17 10" />
                <line x1="12" x2="12" y1="15" y2="3" />
            </svg>
        }
        .into_any(),
    }
}

#[component]
pub fn CommandPalette() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let navigate = StoredValue::new(use_navigate());

    let query = RwSignal::new(String::new());
    let selected = RwSignal::new(0usize);
    let entered = RwSignal::new(false);

    let input_ref = NodeRef::<leptos::html::Input>::new();
    input_ref.on_load(|el| {
        let _ = el.focus();
    });

    let entries = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        let hit = |text: &str| q.is_empty() || text.to_lowercase().contains(&q);
        let mut list = Vec::new();

        for (path, title, subtitle) in [
            ("/", "Go to Dashboard", "Overview, KPIs and live telemetry"),
            (
                "/explorer",
                "Go to Explorer",
                "Browse, filter and control torrents",
            ),
            (
                "/analytics",
                "Go to Analytics",
                "Distributions, percentiles and heatmap",
            ),
            (
                "/settings",
                "Go to Settings",
                "Connection, appearance and data tools",
            ),
        ] {
            if hit(title) || hit(path) {
                list.push(PaletteEntry {
                    id: format!("nav:{path}"),
                    group: "Navigation",
                    title: title.to_string(),
                    subtitle: subtitle.to_string(),
                    kind: EntryKind::Navigate(path),
                });
            }
        }

        let live_active = state.live_stream_active.get();
        for (action, title, subtitle) in [
            (
                PaletteAction::CreateItem,
                "Create New Item",
                "Add a fresh demo torrent",
            ),
            (
                PaletteAction::ToggleLive,
                "Toggle Live Stream",
                if live_active {
                    "Pause real-time updates"
                } else {
                    "Resume real-time updates"
                },
            ),
            (
                PaletteAction::Diagnostics,
                "Run Diagnostics",
                "Probe backend health and emit events",
            ),
            (
                PaletteAction::ExportJson,
                "Export JSON",
                "Download the current state as JSON",
            ),
            (
                PaletteAction::ClearFilters,
                "Clear Filters",
                "Reset search and status filters",
            ),
        ] {
            if hit(title) {
                list.push(PaletteEntry {
                    id: format!("action:{title}"),
                    group: "Quick Actions",
                    title: title.to_string(),
                    subtitle: subtitle.to_string(),
                    kind: EntryKind::Action(action),
                });
            }
        }

        for item in state
            .items
            .get()
            .into_iter()
            .filter(|item| {
                q.is_empty()
                    || item.name.to_lowercase().contains(&q)
                    || item.hash.to_lowercase().contains(&q)
                    || item.category.to_lowercase().contains(&q)
            })
            .take(50)
        {
            list.push(PaletteEntry {
                id: format!("item:{}", item.hash),
                group: "Torrents",
                title: item.name.clone(),
                subtitle: format!("{} · {}", item.short_hash(), item.status_label()),
                kind: EntryKind::Item(item.hash.clone()),
            });
        }

        list
    });

    let execute = move |entry: &PaletteEntry| {
        match &entry.kind {
            EntryKind::Navigate(path) => {
                navigate.with_value(|nav| nav(path, Default::default()));
            }
            EntryKind::Action(action) => match action {
                PaletteAction::CreateItem => api::create_demo_item(state),
                PaletteAction::ToggleLive => {
                    let active = !state.live_stream_active.get_untracked();
                    state.live_stream_active.set(active);
                    let (title, message) = if active {
                        (
                            "Live stream resumed",
                            "Real-time updates are running again.",
                        )
                    } else {
                        ("Live stream paused", "Real-time updates are frozen.")
                    };
                    state.push_toast(ToastKind::Info, title, message);
                }
                PaletteAction::Diagnostics => api::run_diagnostics(state),
                PaletteAction::ExportJson => {
                    let json = api::export_state_json(state);
                    api::download_json("rt-ui-state.json", &json);
                    state.push_toast(
                        ToastKind::Success,
                        "Export ready",
                        "rt-ui-state.json download started.",
                    );
                }
                PaletteAction::ClearFilters => {
                    state.search_query.set(String::new());
                    state.status_filter.set("all".to_string());
                    state.push_toast(
                        ToastKind::Info,
                        "Filters cleared",
                        "Search and status filters were reset.",
                    );
                }
            },
            EntryKind::Item(hash) => {
                let found = state
                    .items
                    .get_untracked()
                    .into_iter()
                    .find(|item| &item.hash == hash);
                match found {
                    Some(item) => state.selected_item.set(Some(item)),
                    None => state.push_toast(
                        ToastKind::Warning,
                        "Item unavailable",
                        "That torrent is no longer in the list.",
                    ),
                }
            }
        }
        state.cmd_palette_open.set(false);
    };

    Effect::new(move |_| {
        if state.cmd_palette_open.get() {
            query.set(String::new());
            selected.set(0);
            entered.set(false);
            // one frame gap so the enter transition actually animates
            spawn_local(async move {
                TimeoutFuture::new(16).await;
                // The palette may have closed again within the frame gap.
                if state.cmd_palette_open.get() {
                    entered.set(true);
                }
            });
        }
    });

    Effect::new(move |_| {
        let len = entries.get().len();
        selected.maybe_update(|idx| {
            if *idx >= len {
                *idx = len.saturating_sub(1);
                true
            } else {
                false
            }
        });
    });

    Effect::new(move |_| {
        let idx = selected.get();
        if state.cmd_palette_open.get() {
            if let Some(el) = document().get_element_by_id(&format!("palette-entry-{idx}")) {
                el.scroll_into_view();
            }
        }
    });

    let key_handle = window_event_listener(ev::keydown, move |ev: web_sys::KeyboardEvent| {
        let key = ev.key();
        if (ev.meta_key() || ev.ctrl_key()) && (key == "k" || key == "K") {
            ev.prevent_default();
            state.cmd_palette_open.update(|open| *open = !*open);
        } else if key == "Escape" && state.cmd_palette_open.get_untracked() {
            ev.prevent_default();
            // keep the drawer's Escape listener (registered later) from also firing
            ev.stop_immediate_propagation();
            state.cmd_palette_open.set(false);
        }
    });
    on_cleanup(move || key_handle.remove());

    view! {
        <Show when=move || state.cmd_palette_open.get() fallback=|| ()>
            <div
                class="fixed inset-0 z-50 flex items-start justify-center px-4 pt-[15vh]"
                role="dialog"
                aria-modal="true"
                aria-label="Command palette"
            >
                <div
                    class="absolute inset-0 bg-black/60 backdrop-blur-sm"
                    on:click=move |_| state.cmd_palette_open.set(false)
                ></div>
                <div class=move || format!(
                    "glass-surface relative w-full max-w-xl overflow-hidden rounded-2xl bg-zinc-900/90 backdrop-blur-2xl border border-white/15 shadow-2xl transition-all duration-150 ease-out {}",
                    if entered.get() { "translate-y-0 opacity-100 scale-100" } else { "-translate-y-2 opacity-0 scale-95" }
                )>
                    <div class="flex items-center gap-3 border-b border-white/[0.08] px-4 py-3">
                        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4 shrink-0 text-zinc-500">
                            <circle cx="11" cy="11" r="8" />
                            <path d="m21 21-4.3-4.3" />
                        </svg>
                        <input
                            node_ref=input_ref
                            type="text"
                            class="w-full bg-transparent text-sm text-zinc-100 placeholder:text-zinc-500 focus:outline-none"
                            placeholder="Search routes, actions, torrents..."
                            prop:value=move || query.get()
                            on:input=move |ev| {
                                query.set(event_target_value(&ev));
                                selected.set(0);
                            }
                            on:keydown=move |ev: web_sys::KeyboardEvent| {
                                let list = entries.get_untracked();
                                let len = list.len();
                                match ev.key().as_str() {
                                    "ArrowDown" => {
                                        ev.prevent_default();
                                        if len > 0 {
                                            selected.update(|idx| *idx = (*idx + 1) % len);
                                        }
                                    }
                                    "ArrowUp" => {
                                        ev.prevent_default();
                                        if len > 0 {
                                            selected.update(|idx| *idx = if *idx == 0 { len - 1 } else { *idx - 1 });
                                        }
                                    }
                                    "Enter" => {
                                        ev.prevent_default();
                                        let idx = selected.get_untracked().min(len.saturating_sub(1));
                                        if let Some(entry) = list.get(idx) {
                                            execute(entry);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        />
                        <kbd class="shrink-0 rounded bg-white/10 px-1.5 py-0.5 text-[10px] font-mono text-zinc-400">"esc"</kbd>
                    </div>
                    <div class="max-h-[320px] overflow-y-auto p-2">
                        {move || {
                            let list = entries.get();
                            if list.is_empty() {
                                return view! {
                                    <p class="px-4 py-8 text-center text-sm text-zinc-500">
                                        "No results for "
                                        <span class="font-mono text-zinc-300">{query.get()}</span>
                                    </p>
                                }
                                .into_any();
                            }
                            let idx = selected.get();
                            let mut rows = Vec::new();
                            let mut last_group = "";
                            for (i, entry) in list.iter().enumerate() {
                                if entry.group != last_group {
                                    last_group = entry.group;
                                    rows.push(view! {
                                        <p class="px-3 pb-1 pt-3 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">
                                            {entry.group}
                                        </p>
                                    }.into_any());
                                }
                                let is_active = i == idx;
                                let row_id = format!("palette-entry-{i}");
                                let click_entry = entry.clone();
                                let icon = entry_icon(&entry.kind);
                                let path_hint = match &entry.kind {
                                    EntryKind::Navigate(path) => Some(*path),
                                    _ => None,
                                };
                                rows.push(view! {
                                    <button
                                        id=row_id
                                        type="button"
                                        class=format!(
                                            "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left transition-colors duration-100 {}",
                                            if is_active { "bg-white/[0.08]" } else { "hover:bg-white/[0.04]" }
                                        )
                                        on:click=move |_| execute(&click_entry)
                                        on:mouseenter=move |_| selected.set(i)
                                    >
                                        <span class=if is_active { "shrink-0 text-zinc-100" } else { "shrink-0 text-zinc-500" }>
                                            {icon}
                                        </span>
                                        <span class="min-w-0 flex-1">
                                            <span class="block truncate text-sm font-medium text-zinc-100">{entry.title.clone()}</span>
                                            <span class="block truncate text-xs text-zinc-500">{entry.subtitle.clone()}</span>
                                        </span>
                                        {path_hint.map(|path| view! {
                                            <span class="shrink-0 font-mono text-[10px] text-zinc-500">{path}</span>
                                        })}
                                        {is_active.then(|| view! {
                                            <kbd class="shrink-0 rounded bg-white/10 px-1.5 py-0.5 text-[10px] font-mono text-zinc-400">"↵"</kbd>
                                        })}
                                    </button>
                                }.into_any());
                            }
                            rows.into_any()
                        }}
                    </div>
                    <div class="flex items-center gap-4 border-t border-white/[0.08] px-4 py-2.5 text-[11px] text-zinc-500">
                        <span class="flex items-center gap-1.5">
                            <kbd class="rounded bg-white/10 px-1.5 py-0.5 text-[10px] font-mono text-zinc-400">"↑↓"</kbd>
                            "navigate"
                        </span>
                        <span class="flex items-center gap-1.5">
                            <kbd class="rounded bg-white/10 px-1.5 py-0.5 text-[10px] font-mono text-zinc-400">"↵"</kbd>
                            "select"
                        </span>
                        <span class="flex items-center gap-1.5">
                            <kbd class="rounded bg-white/10 px-1.5 py-0.5 text-[10px] font-mono text-zinc-400">"esc"</kbd>
                            "close"
                        </span>
                    </div>
                </div>
            </div>
        </Show>
    }
}
