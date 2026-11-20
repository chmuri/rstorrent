use leptos::prelude::*;

use crate::state::*;

#[component]
pub fn BootSplash() -> impl IntoView {
    let state = use_context::<AppState>().expect("AppState in context");
    let is_dismissed = RwSignal::new(false);

    let lifecycle = Signal::derive(move || state.daemon_lifecycle.get());
    let connection = Signal::derive(move || state.connection.get());
    let logs = Signal::derive(move || state.logs.get());
    let items = Signal::derive(move || state.items.get());

    let is_initializing = Signal::derive(move || {
        if is_dismissed.get() {
            return false;
        }
        match lifecycle.get() {
            DaemonLifecycle::Initializing { .. } => true,
            DaemonLifecycle::Unknown => connection.get() == ConnectionMode::Offline,
            _ => false,
        }
    });

    let checking_items = Signal::derive(move || {
        items
            .get()
            .into_iter()
            .filter(|i| i.status() == ItemStatus::Checking)
            .collect::<Vec<_>>()
    });

    view! {
        {move || {
            if !is_initializing.get() {
                return None;
            }

            let (phase_title, step, total_steps, current_item) = match lifecycle.get() {
                DaemonLifecycle::Initializing { phase, step, total_steps, current_item } => {
                    (phase, step, total_steps, current_item)
                }
                _ => (
                    "Connecting to rtorrent engine...".to_string(),
                    0,
                    10,
                    None,
                ),
            };

            let progress_pct = if total_steps > 0 {
                ((step as f64 / total_steps as f64) * 100.0).clamp(5.0, 100.0)
            } else {
                15.0
            };

            Some(view! {
                <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/90 backdrop-blur-xl p-4 overflow-y-auto">
                    <div class="relative w-full max-w-4xl rounded-2xl border border-cyan-500/40 bg-zinc-950/95 p-6 shadow-[0_0_50px_rgba(6,182,212,0.25)] neon-border-cyan cyber-scanlines">
                        // Top status banner
                        <div class="flex items-center justify-between border-b border-cyan-500/20 pb-4 mb-6">
                            <div class="flex items-center gap-3">
                                <span class="relative flex h-3 w-3">
                                    <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-cyan-400 opacity-75"></span>
                                    <span class="relative inline-flex rounded-full h-3 w-3 bg-cyan-500 shadow-[0_0_10px_#06b6d4]"></span>
                                </span>
                                <div>
                                    <span class="font-mono text-xs uppercase tracking-widest text-cyan-400 font-bold">
                                        "RSTORRENT // CORE SUBSYSTEM"
                                    </span>
                                    <h1 class="text-xl font-bold tracking-tight text-white flex items-center gap-2">
                                        "Engine Initialization"
                                        <span class="text-xs font-mono font-normal px-2 py-0.5 rounded bg-cyan-950 text-cyan-300 border border-cyan-500/30">
                                            {format!("PHASE {:02}/{:02}", step, total_steps)}
                                        </span>
                                    </h1>
                                </div>
                            </div>
                            <button
                                on:click=move |_| is_dismissed.set(true)
                                class="rounded-lg border border-zinc-800 bg-zinc-900/80 px-3 py-1.5 font-mono text-xs text-zinc-400 hover:text-cyan-300 hover:border-cyan-500/50 transition-colors"
                            >
                                "[DISMISS_OVERLAY ->]"
                            </button>
                        </div>

                        <div class="grid grid-cols-1 md:grid-cols-12 gap-6">
                            // Radar Scanner & Status
                            <div class="md:col-span-5 flex flex-col items-center justify-center p-4 rounded-xl border border-white/5 bg-zinc-900/40">
                                <div class="relative w-48 h-48 flex items-center justify-center mb-4">
                                    // Circular SVG Radar Grid
                                    <svg class="absolute inset-0 w-full h-full text-cyan-500/20" viewBox="0 0 100 100">
                                        <circle cx="50" cy="50" r="48" fill="none" stroke="currentColor" stroke-width="0.75" />
                                        <circle cx="50" cy="50" r="35" fill="none" stroke="currentColor" stroke-width="0.75" stroke-dasharray="2 2" />
                                        <circle cx="50" cy="50" r="22" fill="none" stroke="currentColor" stroke-width="0.75" />
                                        <circle cx="50" cy="50" r="8" fill="none" stroke="currentColor" stroke-width="0.75" />
                                        <line x1="50" y1="2" x2="50" y2="98" stroke="currentColor" stroke-width="0.5" />
                                        <line x1="2" y1="50" x2="98" y2="50" stroke="currentColor" stroke-width="0.5" />
                                    </svg>

                                    // Rotating radar beam
                                    <div class="absolute inset-0 rounded-full animate-radar-sweep pointer-events-none"
                                         style="background: conic-gradient(from 0deg, transparent 0deg, transparent 270deg, rgba(6,182,212,0.15) 320deg, rgba(6,182,212,0.7) 360deg);" />

                                    // Center readout
                                    <div class="relative z-10 flex flex-col items-center justify-center text-center">
                                        <span class="font-mono text-3xl font-extrabold text-cyan-400 tracking-wider shadow-cyan-500/50 drop-shadow-[0_0_12px_rgba(6,182,212,0.8)]">
                                            {format!("{:.0}%", progress_pct)}
                                        </span>
                                        <span class="font-mono text-[10px] uppercase text-zinc-400 tracking-widest mt-0.5">
                                            "DIAGNOSTIC"
                                        </span>
                                    </div>
                                </div>

                                <div class="w-full text-center">
                                    <div class="font-mono text-sm font-semibold text-zinc-100 mb-1 truncate px-2">
                                        {phase_title}
                                    </div>
                                    {current_item.map(|item| {
                                        view! {
                                            <div class="font-mono text-xs text-amber-400/90 truncate px-2 bg-amber-950/40 border border-amber-500/30 rounded py-1 mb-2">
                                                <span class="text-zinc-500">"Target: "</span>{item}
                                            </div>
                                        }
                                    })}
                                    // Progress bar with laser scan
                                    <div class="w-full bg-zinc-900 rounded-full h-2.5 overflow-hidden border border-white/10 mt-2">
                                        <div
                                            class="h-full bg-gradient-to-r from-cyan-600 via-cyan-400 to-emerald-400 animate-laser-scan transition-all duration-300"
                                            style=format!("width: {:.1}%", progress_pct)
                                        />
                                    </div>
                                </div>
                            </div>

                            // Live Log Console & Checking Items
                            <div class="md:col-span-7 flex flex-col gap-4">
                                // Active Hash Verification Panel (if checking)
                                {move || {
                                    let checking = checking_items.get();
                                    if checking.is_empty() {
                                        return None;
                                    }
                                    Some(view! {
                                        <div class="rounded-xl border border-amber-500/40 bg-amber-950/20 p-3 shadow-[0_0_15px_rgba(245,158,11,0.15)]">
                                            <div class="flex items-center gap-2 mb-2 text-amber-400 font-mono text-xs font-bold uppercase tracking-wider">
                                                <span class="relative flex h-2 w-2">
                                                    <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-amber-400 opacity-75"></span>
                                                    <span class="relative inline-flex rounded-full h-2 w-2 bg-amber-500"></span>
                                                </span>
                                                "Active Integrity Verification ("{checking.len()}" torrents)"
                                            </div>
                                            <div class="space-y-2 max-h-36 overflow-y-auto pr-1">
                                                {checking.into_iter().map(|item| {
                                                    let pct = item.hash_progress_pct;
                                                    view! {
                                                        <div class="p-2 rounded bg-black/50 border border-white/5">
                                                            <div class="flex justify-between text-xs font-mono mb-1">
                                                                <span class="text-white truncate max-w-[200px]">{item.name}</span>
                                                                <span class="text-amber-400 font-bold">{format!("{:.1}%", pct)}</span>
                                                            </div>
                                                            <div class="w-full bg-zinc-900 rounded-full h-1.5 overflow-hidden border border-white/5">
                                                                <div
                                                                    class="h-full bg-gradient-to-r from-amber-500 via-yellow-300 to-amber-500 animate-laser-scan"
                                                                    style=format!("width: {:.1}%", pct)
                                                                />
                                                            </div>
                                                            <div class="flex justify-between text-[10px] font-mono text-zinc-500 mt-1">
                                                                <span>{format!("Chunks: {} / {}", item.chunks_hashed, item.chunks_total)}</span>
                                                                <span>{format_rate(item.completed_bytes)}</span>
                                                            </div>
                                                        </div>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        </div>
                                    })
                                }}

                                // Live Terminal Window
                                <div class="flex-1 flex flex-col rounded-xl border border-zinc-800 bg-black/80 overflow-hidden min-h-[220px] max-h-[300px]">
                                    <div class="flex items-center justify-between px-3 py-1.5 bg-zinc-900/80 border-b border-zinc-800 font-mono text-[11px] text-zinc-400">
                                        <div class="flex items-center gap-2">
                                            <div class="flex gap-1.5">
                                                <div class="w-2.5 h-2.5 rounded-full bg-rose-500/80" />
                                                <div class="w-2.5 h-2.5 rounded-full bg-amber-500/80" />
                                                <div class="w-2.5 h-2.5 rounded-full bg-emerald-500/80" />
                                            </div>
                                            <span class="text-zinc-500 ml-1">"SYS_DIAGNOSTICS // STREAM_ACTIVE"</span>
                                        </div>
                                        <span class="text-cyan-400 text-[10px]">{format!("{} EVENTS", logs.get().len())}</span>
                                    </div>
                                    <div class="flex-1 p-3 overflow-y-auto font-mono text-xs space-y-1.5 scrollbar-thin">
                                        {move || {
                                            let entries = logs.get();
                                            if entries.is_empty() {
                                                return view! {
                                                    <div class="text-zinc-600 italic">"Waiting for daemon tracing stream..."</div>
                                                }.into_any();
                                            }
                                            let recent = entries.into_iter().rev().take(40).collect::<Vec<_>>();
                                            view! {
                                                <div class="flex flex-col-reverse space-y-reverse space-y-1">
                                                    {recent.into_iter().map(|entry| {
                                                        let level_class = match entry.level.to_uppercase().as_str() {
                                                            "ERROR" => "text-rose-400 bg-rose-950/60 border-rose-500/40",
                                                            "WARN" => "text-amber-400 bg-amber-950/60 border-amber-500/40",
                                                            "DEBUG" | "TRACE" => "text-zinc-500 bg-zinc-900 border-zinc-800",
                                                            _ => "text-cyan-400 bg-cyan-950/60 border-cyan-500/40",
                                                        };
                                                        view! {
                                                            <div class="leading-relaxed flex items-start gap-2 hover:bg-white/[0.02] px-1 py-0.5 rounded">
                                                                <span class=format!("text-[10px] font-bold px-1.5 py-0.2 rounded border {level_class}")>
                                                                    {entry.level}
                                                                </span>
                                                                <span class="text-zinc-500 text-[11px] shrink-0">{entry.target}</span>
                                                                <span class="text-zinc-200 break-all">{entry.message}</span>
                                                            </div>
                                                        }
                                                    }).collect_view()}
                                                </div>
                                            }.into_any()
                                        }}
                                    </div>
                                </div>
                            </div>
                        </div>

                        // Footer hint
                        <div class="mt-4 pt-3 border-t border-white/5 flex items-center justify-between font-mono text-[11px] text-zinc-500">
                            <span>"Press ~ or Ctrl+L anytime to toggle system log console"</span>
                            <span class="text-cyan-500/70">"STATUS: ONLINE_ACTIVE"</span>
                        </div>
                    </div>
                </div>
            })
        }}
    }
}
