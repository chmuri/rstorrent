use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::state::*;

#[component]
pub fn Sidebar() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let pathname = use_location().pathname;

    let explorer_count = Signal::derive(move || state.items.with(|items| items.len()));
    // Demo telemetry: deterministic pseudo-load derived from session stats, not real measurements.
    let cpu_pct = Signal::derive(move || {
        let (down_rate, active_torrents) = state
            .stats
            .with(|stats| (stats.down_rate, stats.active_torrents));
        let mbps = down_rate as f64 / (1024.0 * 1024.0);
        (6.0 + active_torrents as f64 * 4.5 + mbps * 0.8).clamp(2.0, 97.0)
    });
    let mem_pct = Signal::derive(move || {
        let down_total = state.stats.with(|stats| stats.down_total);
        let items_len = state.items.with(|items| items.len());
        let gib = down_total as f64 / (1024.0 * 1024.0 * 1024.0);
        (18.0 + items_len as f64 * 2.5 + gib * 0.4).clamp(8.0, 93.0)
    });

    view! {
        <aside class=move || {
            format!(
                "glass-surface relative overflow-hidden rounded-2xl bg-zinc-900/50 backdrop-blur-xl border border-white/[0.08] shadow-[0_8px_32px_rgba(0,0,0,0.4)] transition-all duration-200 hover:border-white/[0.14] flex h-screen shrink-0 flex-col {}",
                if state.sidebar_collapsed.get() { "w-16" } else { "w-64" },
            )
        }>
            <div class=move || {
                format!(
                    "flex items-center gap-3 border-b border-white/[0.08] py-4 {}",
                    if state.sidebar_collapsed.get() { "justify-center px-2" } else { "px-4" },
                )
            }>
                <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-[var(--accent)] shadow-[0_0_24px_rgba(79,70,229,0.4)]">
                    <svg
                        class="h-5 w-5 text-white"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        aria-hidden="true"
                    >
                        <path d="M12 3v12"/>
                        <path d="m7 10 5 5 5-5"/>
                        <path d="M5 21h14"/>
                    </svg>
                </div>
                <div class=move || {
                    if state.sidebar_collapsed.get() { "hidden" } else { "flex min-w-0 flex-col" }
                }>
                    <span class="truncate text-sm font-semibold tracking-tight text-zinc-100">
                        "rstorrent"
                    </span>
                    <span class="mt-1 w-fit rounded-full border border-white/[0.08] bg-white/[0.06] px-1.5 py-0.5 font-mono text-[10px] text-zinc-400">
                        {move || state.version.get().version}
                    </span>
                </div>
            </div>

            <nav
                aria-label="Primary"
                class=move || {
                    format!(
                        "flex-1 space-y-1 overflow-y-auto py-4 {}",
                        if state.sidebar_collapsed.get() { "px-2" } else { "px-3" },
                    )
                }
            >
                {nav_link(state, pathname, "/", "Dashboard", dashboard_icon(), None)}
                {nav_link(state, pathname, "/explorer", "Explorer", folder_icon(), Some(explorer_count))}
                {nav_link(state, pathname, "/rss", "RSS", rss_icon(), None)}
                {nav_link(state, pathname, "/analytics", "Analytics", chart_icon(), None)}
                {nav_link(state, pathname, "/settings", "Settings", settings_icon(), None)}
            </nav>

            <div class="mt-auto border-t border-white/[0.08] px-3 py-3">
                <div class=move || {
                    if state.sidebar_collapsed.get()
                        || state.connection.get() != ConnectionMode::Demo
                    {
                        "hidden"
                    } else {
                        "mb-3 space-y-2 px-1"
                    }
                }>
                    <p class="text-[10px] font-medium uppercase tracking-wider text-zinc-600">
                        "Simulated load"
                    </p>
                    {resource_meter("CPU", cpu_pct)}
                    {resource_meter("Memory", mem_pct)}
                </div>
                <button
                    type="button"
                    title=move || {
                        if state.sidebar_collapsed.get() { "Expand sidebar" } else { "Collapse sidebar" }
                    }
                    class="flex w-full items-center justify-center gap-2 rounded-xl border border-white/[0.08] bg-white/[0.04] px-2 py-2 text-xs font-medium text-zinc-400 transition-all duration-200 hover:bg-white/[0.08] hover:text-zinc-100 active:scale-[0.98]"
                    on:click=move |_| {
                        state.sidebar_collapsed.update(|collapsed| *collapsed = !*collapsed);
                        state.persist_settings();
                    }
                >
                    <svg
                        class="h-4 w-4 shrink-0"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        aria-hidden="true"
                    >
                        {move || {
                            if state.sidebar_collapsed.get() {
                                view! {
                                    <path d="m6 17 5-5-5-5"/>
                                    <path d="m13 17 5-5-5-5"/>
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <path d="m11 17-5-5 5-5"/>
                                    <path d="m18 17-5-5 5-5"/>
                                }
                                    .into_any()
                            }
                        }}
                    </svg>
                    <span class=move || if state.sidebar_collapsed.get() { "hidden" } else { "" }>
                        "Collapse"
                    </span>
                </button>
            </div>
        </aside>
    }
}

fn nav_link(
    state: AppState,
    pathname: Memo<String>,
    href: &'static str,
    label: &'static str,
    icon: AnyView,
    count: Option<Signal<usize>>,
) -> impl IntoView {
    view! {
        <A
            href=href
            attr:title=label
            attr:class=move || {
                let active = pathname.get() == href;
                format!(
                    "group flex items-center gap-3 rounded-xl py-2 text-sm font-medium transition-all duration-200 active:scale-[0.98] {} {}",
                    if state.sidebar_collapsed.get() { "justify-center px-2" } else { "px-3" },
                    if active {
                        "bg-[var(--accent)]/15 text-[var(--accent)]"
                    } else {
                        "text-zinc-400 hover:bg-white/[0.04] hover:text-zinc-100"
                    },
                )
            }
        >
            {icon}
            <span class=move || if state.sidebar_collapsed.get() { "hidden" } else { "truncate" }>
                {label}
            </span>
            {count.map(|count| {
                view! {
                    <span class=move || {
                        if state.sidebar_collapsed.get() {
                            "hidden"
                        } else {
                            "ml-auto rounded-full border border-white/[0.08] bg-white/[0.06] px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-zinc-400"
                        }
                    }>{move || count.get()}</span>
                }
            })}
        </A>
    }
}

fn resource_meter(label: &'static str, pct: Signal<f64>) -> impl IntoView {
    view! {
        <div class="space-y-1">
            <div class="flex items-center justify-between text-[10px] font-medium uppercase tracking-wider text-zinc-500">
                <span>{label}</span>
                <span class="font-mono tabular-nums text-zinc-400">
                    {move || format!("{:.0}%", pct.get())}
                </span>
            </div>
            <div class="h-1 overflow-hidden rounded-full bg-white/[0.06]">
                <div
                    class="h-full rounded-full bg-[var(--accent)] transition-all duration-500"
                    style=move || format!("width: {:.1}%", pct.get())
                />
            </div>
        </div>
    }
}

fn dashboard_icon() -> AnyView {
    view! {
        <svg
            class="h-[18px] w-[18px] shrink-0"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <rect x="3" y="3" width="7" height="9" rx="1"/>
            <rect x="14" y="3" width="7" height="5" rx="1"/>
            <rect x="14" y="12" width="7" height="9" rx="1"/>
            <rect x="3" y="16" width="7" height="5" rx="1"/>
        </svg>
    }
    .into_any()
}

fn folder_icon() -> AnyView {
    view! {
        <svg
            class="h-[18px] w-[18px] shrink-0"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>
        </svg>
    }
        .into_any()
}

fn rss_icon() -> AnyView {
    view! {
        <svg
            class="h-[18px] w-[18px] shrink-0"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <path d="M4 11a9 9 0 0 1 9 9"/>
            <path d="M4 4a16 16 0 0 1 16 16"/>
            <circle cx="5" cy="19" r="1"/>
        </svg>
    }
    .into_any()
}

fn chart_icon() -> AnyView {
    view! {
        <svg
            class="h-[18px] w-[18px] shrink-0"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <path d="M3 3v18h18"/>
            <path d="M18 17V9"/>
            <path d="M13 17V5"/>
            <path d="M8 17v-3"/>
        </svg>
    }
    .into_any()
}

fn settings_icon() -> AnyView {
    view! {
        <svg
            class="h-[18px] w-[18px] shrink-0"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/>
            <circle cx="12" cy="12" r="3"/>
        </svg>
    }
        .into_any()
}
