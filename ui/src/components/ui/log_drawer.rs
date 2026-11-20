use leptos::prelude::*;

use crate::state::*;

#[component]
pub fn LogDrawer() -> impl IntoView {
    let state = use_context::<AppState>().expect("AppState in context");
    let filter_level = RwSignal::new("all".to_string());
    let filter_query = RwSignal::new(String::new());

    let is_open = Signal::derive(move || state.log_drawer_open.get());
    let logs = Signal::derive(move || state.logs.get());

    let filtered_logs = Signal::derive(move || {
        let level = filter_level.get().to_lowercase();
        let query = filter_query.get().to_lowercase();
        logs.get()
            .into_iter()
            .filter(|entry| {
                if level != "all" && entry.level.to_lowercase() != level {
                    return false;
                }
                if !query.is_empty() {
                    let text = format!("{} {} {}", entry.target, entry.message, entry.level)
                        .to_lowercase();
                    if !text.contains(&query) {
                        return false;
                    }
                }
                true
            })
            .collect::<Vec<_>>()
    });

    view! {
        {move || {
            if !is_open.get() {
                return None;
            }

            Some(view! {
                <div class="fixed inset-x-0 bottom-0 z-40 h-96 flex flex-col border-t border-cyan-500/30 bg-zinc-950/95 shadow-[0_-10px_30px_rgba(0,0,0,0.8)] backdrop-blur-xl cyber-scanlines">
                    // Drawer Header
                    <div class="flex items-center justify-between px-4 py-2 border-b border-white/10 bg-zinc-900/80 font-mono text-xs">
                        <div class="flex items-center gap-3">
                            <span class="relative flex h-2.5 w-2.5">
                                <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-cyan-400 opacity-75"></span>
                                <span class="relative inline-flex rounded-full h-2.5 w-2.5 bg-cyan-500"></span>
                            </span>
                            <span class="font-bold text-white tracking-wider">
                                "SYSTEM CONSOLE // DIAGNOSTIC LOGS"
                            </span>
                            <span class="text-zinc-500 text-[11px]">
                                {move || format!("({} entries)", filtered_logs.get().len())}
                            </span>
                        </div>

                        // Filters
                        <div class="flex items-center gap-2">
                            <input
                                type="text"
                                placeholder="Search logs..."
                                prop:value=move || filter_query.get()
                                on:input=move |ev| filter_query.set(event_target_value(&ev))
                                class="bg-black/60 border border-white/10 rounded px-2.5 py-1 text-xs text-white placeholder-zinc-500 focus:outline-none focus:border-cyan-500/50 w-44"
                            />

                            <div class="flex items-center rounded border border-white/10 p-0.5 bg-black/40">
                                {["all", "info", "warn", "error"].into_iter().map(|lvl| {
                                    let active = move || filter_level.get() == lvl;
                                    let label = lvl.to_uppercase();
                                    view! {
                                        <button
                                            on:click=move |_| filter_level.set(lvl.to_string())
                                            class=move || format!(
                                                "px-2 py-0.5 rounded text-[10px] font-bold uppercase transition-colors {}",
                                                if active() {
                                                    "bg-cyan-500 text-black shadow-sm"
                                                } else {
                                                    "text-zinc-400 hover:text-white"
                                                }
                                            )
                                        >
                                            {label}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>

                            <button
                                on:click=move |_| state.log_drawer_open.set(false)
                                class="p-1 rounded text-zinc-400 hover:text-white hover:bg-white/10 ml-2"
                                title="Close console"
                            >
                                <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                    <line x1="18" y1="6" x2="6" y2="18"></line>
                                    <line x1="6" y1="6" x2="18" y2="18"></line>
                                </svg>
                            </button>
                        </div>
                    </div>

                    // Log output
                    <div class="flex-1 overflow-y-auto p-4 font-mono text-xs space-y-1 scrollbar-thin">
                        {move || {
                            let entries = filtered_logs.get();
                            if entries.is_empty() {
                                return view! {
                                    <div class="text-zinc-500 italic p-2">"No log events match filter criteria."</div>
                                }.into_any();
                            }
                            let reversed = entries.into_iter().rev().collect::<Vec<_>>();
                            view! {
                                <div class="flex flex-col-reverse space-y-reverse space-y-1">
                                    {reversed.into_iter().map(|entry| {
                                        let level_class = match entry.level.to_uppercase().as_str() {
                                            "ERROR" => "text-rose-400 bg-rose-950/60 border-rose-500/40",
                                            "WARN" => "text-amber-400 bg-amber-950/60 border-amber-500/40",
                                            "DEBUG" | "TRACE" => "text-zinc-500 bg-zinc-900 border-zinc-800",
                                            _ => "text-cyan-400 bg-cyan-950/60 border-cyan-500/40",
                                        };
                                        view! {
                                            <div class="flex items-start gap-2.5 px-2 py-0.5 rounded hover:bg-white/[0.03] transition-colors leading-relaxed">
                                                <span class="text-zinc-600 text-[10px] shrink-0 font-normal">
                                                    {format_timestamp(entry.timestamp * 1000)}
                                                </span>
                                                <span class=format!("text-[10px] font-bold px-1.5 py-0.2 rounded border {level_class}")>
                                                    {entry.level}
                                                </span>
                                                <span class="text-zinc-500 text-[11px] shrink-0 max-w-[180px] truncate">{entry.target}</span>
                                                <span class="text-zinc-200 break-all">{entry.message}</span>
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }}
                    </div>
                </div>
            })
        }}
    }
}
