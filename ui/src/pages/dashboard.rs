use leptos::prelude::*;

use crate::components::ui::*;
use crate::state::api;
use crate::state::*;

const COUNT_HISTORY_CAP: usize = 60;
const SPARK_SAMPLES: usize = 60;
const MIB: f64 = 1024.0 * 1024.0;

const TIME_RANGES: [(TimeRange, &str); 4] = [
    (TimeRange::OneMin, "1m"),
    (TimeRange::FifteenMin, "15m"),
    (TimeRange::OneHour, "1h"),
    (TimeRange::OneDay, "24h"),
];

const TILE_CLASS: &str = "group flex cursor-pointer flex-col items-start gap-2.5 rounded-xl border border-white/[0.08] bg-white/[0.03] p-4 text-left transition-all duration-200 hover:border-white/[0.15] hover:bg-white/[0.06] active:scale-[0.98]";

#[component]
pub fn DashboardPage() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();

    // Count KPIs have no backend history series, so accumulate local samples.
    let active_history = RwSignal::new(Vec::<f64>::new());
    let seeding_history = RwSignal::new(Vec::<f64>::new());
    Effect::new(move |_| {
        let active = f64::from(state.stats.get().active_torrents);
        let seeding = seeding_count(state) as f64;
        active_history.update(|history| push_sample(history, active, COUNT_HISTORY_CAP));
        seeding_history.update(|history| push_sample(history, seeding, COUNT_HISTORY_CAP));
    });

    let chart_down = Signal::derive(move || chart_series(state, state.down_history, false));
    let chart_up = Signal::derive(move || chart_series(state, state.up_history, true));
    let buffer_note = Signal::derive(move || {
        let long_range = matches!(
            state.time_range.get(),
            TimeRange::OneHour | TimeRange::OneDay
        );
        (long_range && state.connection.get() == ConnectionMode::Live)
            .then(|| "live buffer limited to 15m".to_string())
    });
    let top_hashes = Signal::derive(move || {
        let mut items = state.items.get();
        items.sort_by_key(|a| std::cmp::Reverse(a.down_rate));
        items.truncate(5);
        items.into_iter().map(|item| item.hash).collect::<Vec<_>>()
    });
    let events_rev = Memo::new(move |_| state.events.get().into_iter().rev().collect::<Vec<_>>());

    let range_pills = view! {
        <div class="flex items-center gap-1 rounded-full border border-white/[0.06] bg-white/[0.03] p-1">
            {TIME_RANGES
                .into_iter()
                .map(|(range, label)| {
                    let active = Signal::derive(move || state.time_range.get() == range);
                    view! {
                        <button
                            type="button"
                            class=move || {
                                if active.get() {
                                    "cursor-pointer rounded-full bg-[var(--accent)]/15 px-2.5 py-1 text-xs font-medium text-[var(--accent)] transition-all duration-200"
                                } else {
                                    "cursor-pointer rounded-full px-2.5 py-1 text-xs font-medium text-zinc-500 transition-all duration-200 hover:text-zinc-200"
                                }
                            }
                            on:click=move |_| state.time_range.set(range)
                        >
                            {label}
                        </button>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
    .into_any();

    let trigger_sync = move |_| {
        state.push_event(ToastKind::Info, "Manual sync triggered", None);
        state.push_toast(
            ToastKind::Success,
            "Sync triggered",
            "Requested a fresh telemetry tick.",
        );
    };
    let flush_cache = move |_| {
        state.events.set(Vec::new());
        state.push_toast(ToastKind::Info, "Cache flushed", "Activity buffer cleared.");
    };
    let export_snapshot = move |_| {
        let json = api::export_state_json(state);
        if json == "{}" {
            state.push_toast(
                ToastKind::Error,
                "Export failed",
                "Could not serialize the current state.",
            );
        } else {
            api::download_json("rstorrent-snapshot.json", &json);
            state.push_toast(
                ToastKind::Success,
                "Snapshot exported",
                "Saved rstorrent-snapshot.json.",
            );
        }
    };
    let run_healthcheck = move |_| {
        api::run_diagnostics(state);
    };

    view! {
        <div class="grid grid-cols-1 md:grid-cols-12 gap-4">
            <KpiCard
                title="Active Torrents"
                value=Signal::derive(move || state.stats.get().active_torrents.to_string())
                delta=Signal::derive(move || count_delta(&active_history.get()))
                spark=Signal::derive(move || active_history.get())
                color="#6366f1"
            />
            <KpiCard
                title="Download Rate"
                value=Signal::derive(move || format_rate(state.stats.get().down_rate))
                delta=Signal::derive(move || rate_delta(&state.down_history.get()))
                spark=Signal::derive(move || tail(&state.down_history.get(), SPARK_SAMPLES))
                color="#6366f1"
            />
            <KpiCard
                title="Upload Rate"
                value=Signal::derive(move || format_rate(state.stats.get().up_rate))
                delta=Signal::derive(move || rate_delta(&state.up_history.get()))
                spark=Signal::derive(move || tail(&state.up_history.get(), SPARK_SAMPLES))
                color="#10b981"
            />
            <KpiCard
                title="Seeding"
                value=Signal::derive(move || seeding_count(state).to_string())
                delta=Signal::derive(move || count_delta(&seeding_history.get()))
                spark=Signal::derive(move || seeding_history.get())
                color="#10b981"
            />

            <BentoCard
                title="Network Throughput"
                subtitle="Download and upload, bytes/sec".to_string()
                col_span="col-span-12 lg:col-span-8"
                actions=range_pills
            >
                <div class="flex flex-col gap-3">
                    // BentoCard's subtitle prop is static, so the dynamic live-buffer
                    // note renders as a body caption instead.
                    {move || {
                        buffer_note
                            .get()
                            .map(|note| view! { <p class="text-xs text-zinc-500">{note}</p> })
                    }}
                    <AreaChart
                        data=chart_down
                        color="#6366f1"
                        height=240.0
                        secondary=chart_up
                        secondary_color="#10b981"
                    />
                    <div class="flex items-center gap-4 text-xs text-zinc-500">
                        <span class="inline-flex items-center gap-1.5">
                            <span class="h-1.5 w-1.5 rounded-full bg-[#6366f1]"></span>
                            "Download"
                        </span>
                        <span class="inline-flex items-center gap-1.5">
                            <span class="h-1.5 w-1.5 rounded-full bg-[#10b981]"></span>
                            "Upload"
                        </span>
                        <span class="ml-auto font-mono tabular-nums text-zinc-400">
                            {move || format_rate(state.stats.get().down_rate)}
                        </span>
                    </div>
                </div>
            </BentoCard>

            <BentoCard
                title="Live Activity"
                subtitle="Newest first".to_string()
                col_span="col-span-12 lg:col-span-4"
            >
                <div class="flex h-[280px] flex-col gap-1 overflow-y-auto pr-1">
                    <For
                        each=move || events_rev.get()
                        key=|event| event.id
                        let:event
                    >
                        {
                            let dot = severity_dot(event.severity);
                            let timestamp = format_timestamp(event.timestamp);
                            let message = event.message.clone();
                            let hash_link = event.hash.clone();
                            view! {
                                <div class="dense-row flex items-start gap-2.5 rounded-lg px-2 py-2 transition-colors duration-200 hover:bg-white/[0.03]">
                                    <span class=format!("mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full {dot}")></span>
                                    <div class="min-w-0 flex-1">
                                        <div class="flex items-baseline justify-between gap-2">
                                            <span class="truncate text-xs text-zinc-300">{message}</span>
                                            <span class="shrink-0 font-mono text-[10px] tabular-nums text-zinc-500">{timestamp}</span>
                                        </div>
                                        {hash_link.map(|hash| {
                                            let short = hash.chars().take(8).collect::<String>();
                                            view! {
                                                <button
                                                    type="button"
                                                    class="mt-0.5 cursor-pointer font-mono text-[10px] text-[var(--accent)] transition-colors duration-200 hover:underline"
                                                    on:click=move |_| {
                                                        match state.items.get().into_iter().find(|torrent| torrent.hash == hash) {
                                                            Some(torrent) => state.selected_item.set(Some(torrent)),
                                                            None => state.push_toast(ToastKind::Info, "Item unavailable", "That torrent is no longer in the list."),
                                                        }
                                                    }
                                                >
                                                    {short}
                                                </button>
                                            }
                                        })}
                                    </div>
                                </div>
                            }
                        }
                    </For>
                    {move || {
                        state
                            .events
                            .get()
                            .is_empty()
                            .then(|| {
                                view! {
                                    <p class="px-2 py-6 text-center text-xs text-zinc-500">
                                        "No activity yet. Events will appear here as they happen."
                                    </p>
                                }
                            })
                    }}
                </div>
            </BentoCard>

            <BentoCard
                title="Top Active"
                subtitle="Highest download rate".to_string()
                col_span="col-span-12 lg:col-span-7"
            >
                <div class="overflow-x-auto">
                    <table class="w-full text-left text-sm">
                        <thead>
                            <tr class="border-b border-white/[0.06] text-[10px] uppercase tracking-wider text-zinc-500">
                                <th class="px-3 py-2 font-medium">"Torrent"</th>
                                <th class="px-3 py-2 font-medium">"Progress"</th>
                                <th class="px-3 py-2 font-medium">"Down"</th>
                                <th class="px-3 py-2 font-medium">"Status"</th>
                                <th class="px-3 py-2"></th>
                            </tr>
                        </thead>
                        <tbody>
                            <For
                                each=move || top_hashes.get()
                                key=|hash| hash.clone()
                                children=move |hash| {
                                    let inspect_hash = hash.clone();
                                    let item = Signal::derive(move || {
                                        state.items.get().into_iter().find(|torrent| torrent.hash == hash)
                                    });
                                    view! {
                                        <tr class="border-b border-white/[0.04] transition-colors duration-200 last:border-0 hover:bg-white/[0.03]">
                                            <td class="dense-row max-w-[220px] px-3 py-2.5">
                                                <div class="truncate text-sm font-medium text-zinc-200">
                                                    {move || item.get().map(|torrent| torrent.name).unwrap_or_default()}
                                                </div>
                                                <div class="font-mono text-[10px] text-zinc-500">
                                                    {move || item.get().map(|torrent| torrent.short_hash()).unwrap_or_default()}
                                                </div>
                                            </td>
                                            <td class="dense-row px-3 py-2.5">
                                                <div class="flex items-center gap-2">
                                                    <div class="h-1.5 w-24 overflow-hidden rounded-full bg-white/[0.06]">
                                                        <div
                                                            class="h-full rounded-full bg-[var(--accent)] transition-all duration-300"
                                                            style=move || format!(
                                                                "width: {:.1}%",
                                                                item.get().map(|torrent| torrent.progress() * 100.0).unwrap_or(0.0),
                                                            )
                                                        ></div>
                                                    </div>
                                                    <span class="font-mono text-[10px] tabular-nums text-zinc-500">
                                                        {move || format_percent(item.get().map(|torrent| torrent.progress()).unwrap_or(0.0))}
                                                    </span>
                                                </div>
                                            </td>
                                            <td class="dense-row whitespace-nowrap px-3 py-2.5 font-mono text-xs tabular-nums text-zinc-300">
                                                {move || format_rate(item.get().map(|torrent| torrent.down_rate).unwrap_or(0))}
                                            </td>
                                            <td class="dense-row px-3 py-2.5">
                                                {move || {
                                                    item.get().map(|torrent| {
                                                        let is_pulsing = matches!(torrent.status(), ItemStatus::Downloading | ItemStatus::Checking);
                                                        view! { <Badge label=torrent.status_label() kind=status_kind(torrent.status()) pulse=is_pulsing/> }
                                                    })
                                                }}
                                            </td>
                                            <td class="dense-row px-3 py-2.5 text-right">
                                                <Button
                                                    label="Inspect"
                                                    variant=ButtonVariant::Ghost
                                                    on_click=Callback::new(move |_| {
                                                        if let Some(torrent) = state.items.get().into_iter().find(|torrent| torrent.hash == inspect_hash) {
                                                            state.selected_item.set(Some(torrent));
                                                        }
                                                    })
                                                />
                                            </td>
                                        </tr>
                                    }
                                }
                            />
                        </tbody>
                    </table>
                    {move || {
                        state
                            .items
                            .get()
                            .is_empty()
                            .then(|| {
                                if state.connection.get() == ConnectionMode::Live {
                                    view! {
                                        <div class="flex flex-col gap-2 py-3">
                                            <Skeleton class="h-9 w-full"/>
                                            <Skeleton class="h-9 w-full"/>
                                            <Skeleton class="h-9 w-full"/>
                                        </div>
                                    }
                                        .into_any()
                                } else {
                                    view! {
                                        <p class="py-6 text-center text-xs text-zinc-500">
                                            "No torrents to show yet."
                                        </p>
                                    }
                                        .into_any()
                                }
                            })
                    }}
                </div>
            </BentoCard>

            <BentoCard
                title="Quick Controls"
                subtitle="One-tap operations".to_string()
                col_span="col-span-12 lg:col-span-5"
            >
                <div class="grid grid-cols-2 gap-3">
                    <button type="button" class=TILE_CLASS on:click=trigger_sync>
                        <span class="rounded-lg bg-[var(--accent)]/10 p-2 text-[var(--accent)]">{sync_icon()}</span>
                        <span class="text-sm font-semibold text-zinc-200">"Trigger Sync"</span>
                        <span class="text-xs leading-relaxed text-zinc-500">"Request a fresh telemetry tick"</span>
                    </button>
                    <button type="button" class=TILE_CLASS on:click=flush_cache>
                        <span class="rounded-lg bg-amber-500/10 p-2 text-amber-400">{flush_icon()}</span>
                        <span class="text-sm font-semibold text-zinc-200">"Flush Cache"</span>
                        <span class="text-xs leading-relaxed text-zinc-500">"Clear the activity buffer"</span>
                    </button>
                    <button type="button" class=TILE_CLASS on:click=export_snapshot>
                        <span class="rounded-lg bg-emerald-500/10 p-2 text-emerald-400">{export_icon()}</span>
                        <span class="text-sm font-semibold text-zinc-200">"Export Snapshot"</span>
                        <span class="text-xs leading-relaxed text-zinc-500">"Download state as JSON"</span>
                    </button>
                    <button type="button" class=TILE_CLASS on:click=run_healthcheck>
                        <span class="rounded-lg bg-cyan-500/10 p-2 text-cyan-400">{healthcheck_icon()}</span>
                        <span class="text-sm font-semibold text-zinc-200">"Run Healthcheck"</span>
                        <span class="text-xs leading-relaxed text-zinc-500">"Diagnostics and connectivity"</span>
                    </button>
                </div>
            </BentoCard>
        </div>
    }
}

#[component]
fn KpiCard(
    #[prop(into)] title: String,
    value: Signal<String>,
    delta: Signal<(String, BadgeKind)>,
    spark: Signal<Vec<f64>>,
    color: &'static str,
) -> impl IntoView {
    view! {
        <BentoCard title=title col_span="col-span-12 sm:col-span-6 lg:col-span-3">
            <div class="flex flex-col gap-3">
                <div class="flex items-end justify-between gap-2">
                    <span class="font-mono text-3xl font-semibold tracking-tight tabular-nums text-zinc-100">
                        {move || value.get()}
                    </span>
                    {move || {
                        let (label, kind) = delta.get();
                        view! { <Badge label=label kind=kind/> }
                    }}
                </div>
                // width is only the internal viewBox coordinate space; w-full stretches the SVG.
                <Sparkline data=spark color=color height=36.0 width=240.0/>
            </div>
        </BentoCard>
    }
}

fn push_sample(history: &mut Vec<f64>, sample: f64, cap: usize) {
    history.push(sample);
    if history.len() > cap {
        history.remove(0);
    }
}

fn tail(samples: &[f64], count: usize) -> Vec<f64> {
    let start = samples.len().saturating_sub(count);
    samples[start..].to_vec()
}

fn seeding_count(state: AppState) -> usize {
    state
        .items
        .get()
        .iter()
        .filter(|torrent| torrent.state == "Seeding")
        .count()
}

fn count_delta(history: &[f64]) -> (String, BadgeKind) {
    let (Some(&first), Some(&last)) = (history.first(), history.last()) else {
        return ("—".to_string(), BadgeKind::Neutral);
    };
    let diff = last - first;
    if diff.abs() < 0.5 {
        return ("±0".to_string(), BadgeKind::Neutral);
    }
    let kind = if diff > 0.0 {
        BadgeKind::Success
    } else {
        BadgeKind::Warning
    };
    (format!("{diff:+.0}"), kind)
}

fn rate_delta(history: &[f64]) -> (String, BadgeKind) {
    let (Some(&first), Some(&last)) = (history.first(), history.last()) else {
        return ("—".to_string(), BadgeKind::Neutral);
    };
    let diff = last - first;
    // Guard the zero-base case: report an absolute delta instead of dividing.
    if first.abs() < f64::EPSILON {
        if diff.abs() < f64::EPSILON {
            return ("0.0%".to_string(), BadgeKind::Neutral);
        }
        let kind = if diff > 0.0 {
            BadgeKind::Success
        } else {
            BadgeKind::Warning
        };
        let sign = if diff > 0.0 { "+" } else { "-" };
        return (format!("{sign}{}", format_bytes(diff.abs() as i64)), kind);
    }
    let pct = diff / first * 100.0;
    let kind = if pct > 0.05 {
        BadgeKind::Success
    } else if pct < -0.05 {
        BadgeKind::Warning
    } else {
        BadgeKind::Neutral
    };
    (format!("{pct:+.1}%"), kind)
}

fn severity_dot(severity: ToastKind) -> &'static str {
    match severity {
        ToastKind::Success => "bg-emerald-400",
        ToastKind::Warning => "bg-amber-400",
        ToastKind::Error => "bg-rose-400",
        ToastKind::Info => "bg-indigo-400",
    }
}

fn chart_series(state: AppState, history: RwSignal<Vec<f64>>, upload: bool) -> Vec<f64> {
    let range = state.time_range.get();
    let long_range = matches!(range, TimeRange::OneHour | TimeRange::OneDay);
    if long_range && state.connection.get() == ConnectionMode::Demo {
        let seed = range_seed(range).wrapping_add(if upload { 0x9E37_79B9_7F4A_7C15 } else { 0 });
        let base = if upload { 12.0 * MIB } else { 48.0 * MIB };
        return synthetic_series(range, seed, base);
    }
    let samples = history.get();
    match range {
        TimeRange::OneMin => tail(&samples, 60),
        _ => samples,
    }
}

fn range_seed(range: TimeRange) -> u64 {
    match range {
        TimeRange::OneMin => 60,
        TimeRange::FifteenMin => 900,
        TimeRange::OneHour => 3600,
        TimeRange::OneDay => 86400,
    }
}

// Deterministic LCG so Demo long-range charts are stable across re-renders.
struct Lcg(u64);

impl Lcg {
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

fn synthetic_series(range: TimeRange, seed: u64, base: f64) -> Vec<f64> {
    let points = match range {
        TimeRange::OneHour => 360,
        TimeRange::OneDay => 1440,
        _ => 900,
    };
    let mut rng = Lcg(seed);
    let mut drift = 0.0_f64;
    let mut series = Vec::with_capacity(points);
    for index in 0..points {
        drift = (drift + (rng.next_f64() - 0.5) * 0.08).clamp(-0.35, 0.35);
        let wave = (index as f64 / points as f64 * std::f64::consts::TAU * 3.0).sin() * 0.25;
        let noise = (rng.next_f64() - 0.5) * 0.2;
        series.push(base * (1.0 + drift + wave + noise).max(0.05));
    }
    series
}

fn sync_icon() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class="h-4 w-4"
        >
            <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/>
            <path d="M21 3v5h-5"/>
            <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/>
            <path d="M8 16H3v5"/>
        </svg>
    }
}

fn flush_icon() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class="h-4 w-4"
        >
            <path d="M3 6h18"/>
            <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/>
            <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>
            <line x1="10" x2="10" y1="11" y2="17"/>
            <line x1="14" x2="14" y1="11" y2="17"/>
        </svg>
    }
}

fn export_icon() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class="h-4 w-4"
        >
            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>
            <polyline points="7 10 12 15 17 10"/>
            <line x1="12" x2="12" y1="15" y2="3"/>
        </svg>
    }
}

fn healthcheck_icon() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class="h-4 w-4"
        >
            <path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"/>
        </svg>
    }
}
