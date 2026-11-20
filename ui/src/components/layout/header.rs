use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::components::ui::{Button, ButtonVariant};
use crate::state::*;

#[component]
pub fn Header() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let pathname = use_location().pathname;

    let section = move || match pathname.get().as_str() {
        "/explorer" => "Explorer",
        "/analytics" => "Analytics",
        "/settings" => "Settings",
        _ => "Dashboard",
    };

    view! {
        <header class="glass-surface sticky top-0 z-30 flex shrink-0 items-center gap-4 overflow-hidden rounded-2xl border border-white/[0.08] bg-zinc-900/50 px-6 py-3 shadow-[0_8px_32px_rgba(0,0,0,0.4)] backdrop-blur-xl transition-all duration-200 hover:border-white/[0.14]">
            <nav aria-label="Breadcrumb" class="hidden min-w-0 items-center gap-2 text-sm lg:flex">
                <span class="text-zinc-500">"rstorrent"</span>
                <svg
                    class="h-3.5 w-3.5 shrink-0 text-zinc-600"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                >
                    <path d="m9 18 6-6-6-6"/>
                </svg>
                <span class="truncate font-medium text-zinc-100">{section}</span>
            </nav>

            <button
                type="button"
                class="hidden max-w-md flex-1 items-center gap-2 rounded-xl border border-white/[0.08] bg-zinc-950/80 px-3.5 py-2 text-sm text-zinc-500 transition-all duration-200 hover:border-white/[0.15] hover:text-zinc-300 active:scale-[0.98] md:flex"
                on:click=move |_| state.cmd_palette_open.set(true)
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
                    <circle cx="11" cy="11" r="8"/>
                    <path d="m21 21-4.35-4.35"/>
                </svg>
                <span class="flex-1 truncate text-left">"Search or jump to..."</span>
                <kbd class="rounded bg-white/10 px-1.5 py-0.5 text-[10px] font-mono text-zinc-400">
                    "⌘K"
                </kbd>
            </button>

            <button
                type="button"
                aria-label="Open command palette"
                class="inline-flex h-9 w-9 items-center justify-center rounded-xl border border-white/[0.08] bg-white/[0.04] text-zinc-400 transition-all duration-200 hover:bg-white/[0.08] hover:text-zinc-100 active:scale-[0.98] md:hidden"
                on:click=move |_| state.cmd_palette_open.set(true)
            >
                <svg
                    class="h-4 w-4"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                >
                    <circle cx="11" cy="11" r="8"/>
                    <path d="m21 21-4.35-4.35"/>
                </svg>
            </button>

            <div class="ml-auto flex items-center gap-2.5">
                {connection_pill(state)}
                <button
                    type="button"
                    title=move || match state.theme.get() {
                        Theme::Dark => "Switch to light mode",
                        Theme::Light => "Switch to dark mode",
                    }
                    class="inline-flex h-9 w-9 items-center justify-center rounded-xl border border-white/[0.08] bg-white/[0.04] text-zinc-400 transition-all duration-200 hover:bg-white/[0.08] hover:text-zinc-100 active:scale-[0.98]"
                    on:click=move |_| {
                        state.theme.update(|theme| {
                            *theme = match theme {
                                Theme::Dark => Theme::Light,
                                Theme::Light => Theme::Dark,
                            };
                        });
                        state.persist_settings();
                    }
                >
                    {move || match state.theme.get() {
                        Theme::Dark => {
                            view! {
                                <svg
                                    class="h-4 w-4"
                                    viewBox="0 0 24 24"
                                    fill="none"
                                    stroke="currentColor"
                                    stroke-width="2"
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                    aria-hidden="true"
                                >
                                    <circle cx="12" cy="12" r="4"/>
                                    <path d="M12 2v2"/>
                                    <path d="M12 20v2"/>
                                    <path d="m4.93 4.93 1.41 1.41"/>
                                    <path d="m17.66 17.66 1.41 1.41"/>
                                    <path d="M2 12h2"/>
                                    <path d="M20 12h2"/>
                                    <path d="m6.34 17.66-1.41 1.41"/>
                                    <path d="m19.07 4.93-1.41 1.41"/>
                                </svg>
                            }
                                .into_any()
                        }
                        Theme::Light => {
                            view! {
                                <svg
                                    class="h-4 w-4"
                                    viewBox="0 0 24 24"
                                    fill="none"
                                    stroke="currentColor"
                                    stroke-width="2"
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                    aria-hidden="true"
                                >
                                    <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>
                                </svg>
                            }
                                .into_any()
                        }
                    }}
                </button>
                <Button
                    label="New Torrent"
                    variant=ButtonVariant::Primary
                    icon=view! {
                        <svg
                            class="h-4 w-4"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            aria-hidden="true"
                        >
                            <path d="M5 12h14"/>
                            <path d="M12 5v14"/>
                        </svg>
                    }
                        .into_any()
                    on_click=Callback::new(move |()| {
                        state.add_torrent_modal_open.set(true);
                    })
                />
            </div>
        </header>
    }
}

fn connection_pill(state: AppState) -> impl IntoView {
    let pill_class = move || {
        match state.connection.get() {
        ConnectionMode::Live => {
            "inline-flex items-center gap-1.5 rounded-full bg-emerald-500/10 px-2.5 py-0.5 text-xs font-medium text-emerald-400 border border-emerald-500/20 transition-all duration-200 active:scale-[0.98] cursor-pointer"
        }
        ConnectionMode::Demo => {
            "inline-flex items-center gap-1.5 rounded-full bg-amber-500/10 px-2.5 py-0.5 text-xs font-medium text-amber-400 border border-amber-500/20 transition-all duration-200 active:scale-[0.98] cursor-pointer"
        }
        ConnectionMode::Offline => {
            "inline-flex items-center gap-1.5 rounded-full bg-rose-500/10 px-2.5 py-0.5 text-xs font-medium text-rose-400 border border-rose-500/20 transition-all duration-200 active:scale-[0.98] cursor-pointer"
        }
    }
    };
    let dot_class = move || match state.connection.get() {
        ConnectionMode::Live => "bg-emerald-400",
        ConnectionMode::Demo => "bg-amber-400",
        ConnectionMode::Offline => "bg-rose-400",
    };
    let label = move || match state.connection.get() {
        ConnectionMode::Live => "Live",
        ConnectionMode::Demo => "Demo",
        ConnectionMode::Offline => "Offline",
    };

    view! {
        <button
            type="button"
            title="Toggle live updates"
            class=pill_class
            on:click=move |_| {
                let active = !state.live_stream_active.get();
                state.live_stream_active.set(active);
                if active {
                    state.push_toast(
                        ToastKind::Success,
                        "Live updates resumed",
                        "Real-time telemetry is streaming again.",
                    );
                } else {
                    state.push_toast(
                        ToastKind::Info,
                        "Live updates paused",
                        "Telemetry is frozen. Click the status pill to resume.",
                    );
                }
            }
        >
            <span class="relative flex h-2 w-2">
                <span class=move || {
                    format!(
                        "absolute inline-flex h-full w-full rounded-full opacity-75 {} {}",
                        dot_class(),
                        if state.live_stream_active.get() { "animate-ping" } else { "" },
                    )
                }/>
                <span class=move || {
                    format!("relative inline-flex h-2 w-2 rounded-full {}", dot_class())
                }/>
            </span>
            {label}
            <span class=move || {
                if state.live_stream_active.get() { "hidden" } else { "text-[10px] opacity-80" }
            }>"paused"</span>
        </button>
    }
}
