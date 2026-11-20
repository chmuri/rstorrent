use leptos::prelude::*;

use crate::components::ui::*;
use crate::state::*;

const STATUS_BAR_COLORS: [&str; 4] = [
    "bg-emerald-500",
    "bg-indigo-500",
    "bg-amber-500",
    "bg-rose-500",
];

const STATUS_PILLS: [(&str, &str, &str); 4] = [
    (
        "Completed",
        "inline-flex items-center gap-1.5 rounded-full bg-emerald-500/10 px-2.5 py-0.5 text-xs font-medium text-emerald-400 border border-emerald-500/20",
        "bg-emerald-400",
    ),
    (
        "Downloading",
        "inline-flex items-center gap-1.5 rounded-full bg-indigo-500/10 px-2.5 py-0.5 text-xs font-medium text-indigo-400 border border-indigo-500/20",
        "bg-indigo-400",
    ),
    (
        "Warning",
        "inline-flex items-center gap-1.5 rounded-full bg-amber-500/10 px-2.5 py-0.5 text-xs font-medium text-amber-400 border border-amber-500/20",
        "bg-amber-400",
    ),
    (
        "Failed",
        "inline-flex items-center gap-1.5 rounded-full bg-rose-500/10 px-2.5 py-0.5 text-xs font-medium text-rose-400 border border-rose-500/20",
        "bg-rose-400",
    ),
];

const CATEGORY_OPACITY: [f64; 6] = [1.0, 0.85, 0.7, 0.55, 0.4, 0.3];

#[derive(Clone, Debug, PartialEq)]
struct HeatRow {
    hash: String,
    name: String,
    cells: Vec<f64>,
}

#[derive(Clone, Debug)]
struct HeatTip {
    hash: String,
    name: String,
    offset: String,
    pct: u32,
    x: f64,
    y: f64,
}

// Deterministic per-torrent series so the matrix is stable across ticks.
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
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn seed_from_hash(hash: &str) -> u64 {
    let hex: String = hash
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(16)
        .collect();
    u64::from_str_radix(&hex, 16).unwrap_or_else(|_| {
        hash.bytes().fold(0xcbf29ce484222325u64, |acc, b| {
            (acc ^ b as u64).wrapping_mul(0x100000001b3)
        })
    })
}

fn percentile(data: &[f64], p: f64) -> Option<f64> {
    if data.is_empty() {
        return None;
    }
    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = sorted.len();
    if n == 1 {
        return Some(sorted[0]);
    }
    let rank = (p / 100.0) * (n as f64 - 1.0);
    let lo = (rank.floor() as usize).min(n - 1);
    let hi = (rank.ceil() as usize).min(n - 1);
    let frac = rank - lo as f64;
    Some(sorted[lo] + (sorted[hi] - sorted[lo]) * frac)
}

fn percentile_card(
    state: AppState,
    p: f64,
    title: &'static str,
    subtitle: &'static str,
) -> impl IntoView {
    let value = move || {
        percentile(&state.down_history.get(), p)
            .map_or_else(|| "—".to_string(), |v| format_rate(v as i64))
    };
    view! {
        <BentoCard title=title subtitle=subtitle.to_string() col_span="col-span-12 md:col-span-4">
            <div class="font-mono text-3xl font-semibold tracking-tight tabular-nums text-zinc-100">
                {value}
            </div>
            <div class="mt-2 font-mono tabular-nums text-xs text-zinc-500">
                {move || format!("{} samples buffered", state.down_history.get().len())}
            </div>
        </BentoCard>
    }
}

#[component]
pub fn AnalyticsPage() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();

    let status_counts = Memo::new(move |_| {
        let mut counts = [0usize; 4];
        state.items.with(|items| {
            for item in items {
                let idx = match item.status() {
                    ItemStatus::Completed => 0,
                    ItemStatus::Downloading | ItemStatus::Checking => 1,
                    ItemStatus::Warning => 2,
                    ItemStatus::Failed => 3,
                };
                counts[idx] += 1;
            }
        });
        counts
    });

    let categories = Memo::new(move |_| {
        let mut counts: Vec<(String, usize)> = Vec::new();
        state.items.with(|items| {
            for item in items {
                let mut name = item.category.trim().to_lowercase();
                if name.is_empty() {
                    name = "uncategorized".to_string();
                }
                if let Some(entry) = counts.iter_mut().find(|(n, _)| *n == name) {
                    entry.1 += 1;
                } else {
                    counts.push((name, 1));
                }
            }
        });
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut top: Vec<(String, usize)> = counts.iter().take(6).cloned().collect();
        let rest: usize = counts.iter().skip(6).map(|(_, c)| c).sum();
        if rest > 0 {
            top.push(("other".to_string(), rest));
        }
        top
    });

    let matrix = Memo::new(move |_| -> Vec<HeatRow> {
        let items = state.items.with(|items| {
            let mut top = items.clone();
            top.sort_by(|a, b| {
                b.down_rate
                    .cmp(&a.down_rate)
                    .then_with(|| a.name.cmp(&b.name))
            });
            top.truncate(12);
            top
        });
        let history = state.down_history.get();
        let mode = state.connection.get();
        let global_rate = state
            .stats
            .get()
            .down_rate
            .max(items.iter().map(|i| i.down_rate).sum::<i64>());

        // Right-align the last 120 samples into 24 buckets of 5s each.
        let recent: &[f64] = if history.len() > 120 {
            &history[history.len() - 120..]
        } else {
            &history[..]
        };
        let skip = 120usize.saturating_sub(recent.len());
        let mut bucket_globals = [0.5f64; 24];
        for (b, slot) in bucket_globals.iter_mut().enumerate() {
            let lo = (b * 5).saturating_sub(skip);
            let hi = (b * 5 + 5).saturating_sub(skip).min(recent.len());
            let slice = recent.get(lo..hi).unwrap_or(&[]);
            if slice.is_empty() {
                continue;
            }
            let max = slice.iter().copied().fold(0.0f64, f64::max);
            let avg = slice.iter().sum::<f64>() / slice.len() as f64;
            *slot = if max <= 0.0 {
                0.0
            } else {
                (avg / max).clamp(0.0, 1.0)
            };
        }

        items
            .iter()
            .map(|item| {
                let mut lcg = Lcg(seed_from_hash(&item.hash));
                let share = if global_rate > 0 {
                    (item.down_rate as f64 / global_rate as f64).min(1.0)
                } else {
                    0.0
                };
                let cells = (0..24)
                    .map(|b| {
                        let base = 0.15 + 0.85 * lcg.next_f64();
                        let v = match mode {
                            ConnectionMode::Live => base * (0.2 + 0.8 * share),
                            _ => base * (0.3 + 0.7 * bucket_globals[b]),
                        };
                        v.clamp(0.0, 1.0)
                    })
                    .collect();
                HeatRow {
                    hash: item.hash.clone(),
                    name: item.name.clone(),
                    cells,
                }
            })
            .collect()
    });

    let heat_tip = RwSignal::new(None::<HeatTip>);

    // Rows dropped from the top 12 never fire mouseleave; clear their tooltip.
    Effect::new(move |_| {
        let rows = matrix.get();
        let stale = heat_tip.with_untracked(|tip| {
            tip.as_ref()
                .is_some_and(|t| !rows.iter().any(|r| r.hash == t.hash))
        });
        if stale {
            heat_tip.set(None);
        }
    });

    view! {
        <div class="space-y-6">
            <div class="flex flex-wrap items-center justify-between gap-3">
                <div>
                    <h1 class="text-2xl font-semibold tracking-tight text-zinc-100">"Analytics"</h1>
                    <p class="mt-1 text-sm text-zinc-500">
                        "Distribution, throughput percentiles and per-torrent activity."
                    </p>
                </div>
                <div class="flex items-center gap-2">
                    {move || {
                        let (label, kind, pulse) = match state.connection.get() {
                            ConnectionMode::Live => ("Live", BadgeKind::Success, true),
                            ConnectionMode::Demo => ("Demo", BadgeKind::Warning, false),
                            ConnectionMode::Offline => ("Offline", BadgeKind::Error, false),
                        };
                        view! { <Badge label=label kind=kind pulse=pulse/> }
                    }}
                    {move || {
                        (!state.live_stream_active.get()).then(|| {
                            view! { <Badge label="Stream paused" kind=BadgeKind::Neutral/> }
                        })
                    }}
                </div>
            </div>

            <div class="grid grid-cols-1 md:grid-cols-12 gap-4">
                <BentoCard
                    title="Status Distribution"
                    subtitle="Share of torrents by current status.".to_string()
                    col_span="col-span-12 lg:col-span-6"
                >
                    // Persistent segments so width changes animate instead of re-rendering.
                    <div class="flex h-3 w-full overflow-hidden rounded-full bg-white/[0.04]">
                        {(0..4)
                            .map(|i| {
                                view! {
                                    <div
                                        class=format!(
                                            "h-full transition-all duration-200 ease-out {}",
                                            STATUS_BAR_COLORS[i],
                                        )
                                        style=move || {
                                            let counts = status_counts.get();
                                            let total: usize = counts.iter().sum();
                                            let pct = if total == 0 {
                                                0.0
                                            } else {
                                                counts[i] as f64 / total as f64 * 100.0
                                            };
                                            format!("width: {pct:.2}%")
                                        }
                                    />
                                }
                            })
                            .collect::<Vec<_>>()}
                    </div>
                    <div class="mt-4 flex flex-wrap items-center gap-2">
                        {move || {
                            let counts = status_counts.get();
                            STATUS_PILLS
                                .iter()
                                .enumerate()
                                .map(|(i, &(label, pill, dot))| {
                                    view! {
                                        <span class=pill>
                                            <span class=format!("h-1.5 w-1.5 rounded-full {dot}") />
                                            {label}
                                            <span class="font-mono tabular-nums">{counts[i]}</span>
                                        </span>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                    {move || {
                        let total: usize = status_counts.get().iter().sum();
                        (total == 0).then(|| {
                            view! {
                                <p class="mt-3 text-xs text-zinc-500">
                                    "No torrents yet — the distribution appears once items arrive."
                                </p>
                            }
                        })
                    }}
                </BentoCard>

                <BentoCard
                    title="Category Distribution"
                    subtitle="Top categories across the session.".to_string()
                    col_span="col-span-12 lg:col-span-6"
                >
                    <div class="flex h-3 w-full overflow-hidden rounded-full bg-white/[0.04]">
                        {(0..7)
                            .map(|i| {
                                view! {
                                    <div
                                        class="h-full transition-all duration-200 ease-out"
                                        style=move || {
                                            let cats = categories.get();
                                            let total: usize = cats.iter().map(|(_, c)| c).sum();
                                            let count = cats.get(i).map_or(0, |(_, c)| *c);
                                            let pct = if total == 0 {
                                                0.0
                                            } else {
                                                count as f64 / total as f64 * 100.0
                                            };
                                            if i == 6 {
                                                format!(
                                                    "width: {pct:.2}%; background-color: #71717a; opacity: 0.55",
                                                )
                                            } else {
                                                format!(
                                                    "width: {pct:.2}%; background-color: var(--accent); opacity: {}",
                                                    CATEGORY_OPACITY[i],
                                                )
                                            }
                                        }
                                    />
                                }
                            })
                            .collect::<Vec<_>>()}
                    </div>
                    <div class="mt-4 flex flex-wrap items-center gap-2">
                        {move || {
                            categories
                                .get()
                                .into_iter()
                                .enumerate()
                                .map(|(i, (name, count))| {
                                    let dot = if i == 6 { "#71717a" } else { "var(--accent)" };
                                    view! {
                                        <span class="inline-flex items-center gap-1.5 rounded-full border border-white/[0.08] bg-white/[0.03] px-2.5 py-0.5 text-xs text-zinc-300">
                                            <span
                                                class="h-1.5 w-1.5 rounded-full"
                                                style=format!("background-color: {dot}")
                                            />
                                            {name}
                                            <span class="font-mono tabular-nums text-zinc-400">{count}</span>
                                        </span>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                    {move || {
                        categories.get().is_empty().then(|| {
                            view! {
                                <p class="mt-3 text-xs text-zinc-500">
                                    "No categories yet — they appear once torrents are loaded."
                                </p>
                            }
                        })
                    }}
                </BentoCard>

                {percentile_card(
                    state,
                    50.0,
                    "p50 Throughput",
                    "Median download rate across buffered samples.",
                )}
                {percentile_card(
                    state,
                    95.0,
                    "p95 Throughput",
                    "95% of samples stayed below this rate.",
                )}
                {percentile_card(
                    state,
                    99.0,
                    "p99 Throughput",
                    "Tail rate — only 1% of samples ran hotter.",
                )}

                <BentoCard
                    title="Activity Matrix"
                    subtitle="Top 12 torrents by download rate — last 2 minutes in 5s buckets.".to_string()
                    col_span="col-span-12"
                    actions=view! {
                        <div class="flex items-center gap-2 font-mono text-[10px] text-zinc-500">
                            <span>"low"</span>
                            <span
                                class="h-2 w-24 rounded-full"
                                style="background: linear-gradient(to right, rgba(113,113,122,0.25), var(--accent));"
                            />
                            <span>"high"</span>
                        </div>
                    }
                        .into_any()
                >
                    <div class="space-y-3">
                        <div class="flex items-center gap-2">
                            <div class="w-44 shrink-0" />
                            <div class="flex flex-1 items-center justify-between font-mono text-[10px] text-zinc-500">
                                <span>"-2m"</span>
                                <span>"-90s"</span>
                                <span>"-60s"</span>
                                <span>"-30s"</span>
                                <span>"now"</span>
                            </div>
                            <div class="w-20 shrink-0" />
                        </div>
                        <div class="space-y-1.5">
                            <For
                                each=move || matrix.get()
                                key=|row| row.hash.clone()
                                children=move |row| {
                                    let row_data = StoredValue::new(row);
                                    let row_hash = row_data.with_value(|r| r.hash.clone());
                                    view! {
                                        <div class="flex items-center gap-2">
                                            <button
                                                type="button"
                                                class="flex w-44 shrink-0 cursor-pointer items-center gap-2 rounded-lg px-2 py-1 text-left transition-all duration-200 hover:bg-white/[0.04]"
                                                on:click=move |_| {
                                                    let hash = row_data.with_value(|r| r.hash.clone());
                                                    let item = state.items.with(|items| {
                                                        items.iter().find(|i| i.hash == hash).cloned()
                                                    });
                                                    if let Some(item) = item {
                                                        state.selected_item.set(Some(item));
                                                    }
                                                }
                                            >
                                                <span class=move || {
                                                    let hash = row_data.with_value(|r| r.hash.clone());
                                                    let color = state.items.with(|items| {
                                                        items
                                                            .iter()
                                                            .find(|i| i.hash == hash)
                                                            .map_or("bg-zinc-500", |i| {
                                                                match status_kind(i.status()) {
                                                                    BadgeKind::Success => "bg-emerald-500",
                                                                    BadgeKind::Info => "bg-indigo-500",
                                                                    BadgeKind::Warning => "bg-amber-500",
                                                                    BadgeKind::Error => "bg-rose-500",
                                                                    BadgeKind::Neutral => "bg-zinc-500",
                                                                    BadgeKind::Checking => "bg-cyan-400 animate-pulse",
                                                                }
                                                            })
                                                    });
                                                    format!("h-1.5 w-1.5 shrink-0 rounded-full {color}")
                                                } />
                                                <span class="truncate text-xs text-zinc-300">
                                                    {row_data.with_value(|r| r.name.clone())}
                                                </span>
                                            </button>
                                            <div class="flex flex-1 items-center gap-[3px]">
                                                {(0..24usize)
                                                    .map(|b| {
                                                        let hash = row_hash.clone();
                                                        let intensity = move || {
                                                            matrix.with(|rows| {
                                                                rows.iter()
                                                                    .find(|r| r.hash == hash)
                                                                    .and_then(|r| r.cells.get(b))
                                                                    .copied()
                                                                    .unwrap_or(0.0)
                                                            })
                                                        };
                                                        let style_value = intensity.clone();
                                                        let title_value = intensity.clone();
                                                        view! {
                                                            <div
                                                                class="relative h-5 flex-1 cursor-crosshair rounded-[4px] transition-all duration-200 hover:ring-1 hover:ring-white/40"
                                                                style=move || {
                                                                    let pct = (style_value() * 100.0).round() as u32;
                                                                    format!(
                                                                        "background-color: rgba(113,113,122,0.15); background-color: color-mix(in srgb, var(--accent) {pct}%, rgba(255,255,255,0.05))",
                                                                    )
                                                                }
                                                                title=move || {
                                                                    let pct = (title_value() * 100.0).round() as u32;
                                                                    let name = row_data.with_value(|r| r.name.clone());
                                                                    format!("{name} · -{}s · {pct}% intensity", (24 - b) * 5)
                                                                }
                                                                on:mouseenter=move |ev: leptos::ev::MouseEvent| {
                                                                    let (hash, name) = row_data
                                                                        .with_value(|r| (r.hash.clone(), r.name.clone()));
                                                                    heat_tip.set(Some(HeatTip {
                                                                        hash,
                                                                        name,
                                                                        offset: format!("-{}s", (24 - b) * 5),
                                                                        pct: (intensity() * 100.0).round() as u32,
                                                                        x: ev.client_x() as f64,
                                                                        y: ev.client_y() as f64,
                                                                    }));
                                                                }
                                                                on:mousemove=move |ev: leptos::ev::MouseEvent| {
                                                                    heat_tip.update(|tip| {
                                                                        if let Some(t) = tip {
                                                                            t.x = ev.client_x() as f64;
                                                                            t.y = ev.client_y() as f64;
                                                                        }
                                                                    });
                                                                }
                                                                on:mouseleave=move |_| heat_tip.set(None)
                                                            />
                                                        }
                                                    })
                                                    .collect::<Vec<_>>()}
                                            </div>
                                            <span class="w-20 shrink-0 text-right font-mono tabular-nums text-[10px] text-zinc-500">
                                                {move || {
                                                    let hash = row_data.with_value(|r| r.hash.clone());
                                                    state.items.with(|items| {
                                                        items
                                                            .iter()
                                                            .find(|i| i.hash == hash)
                                                            .map_or_else(
                                                                || "—".to_string(),
                                                                |i| format_rate(i.down_rate),
                                                            )
                                                    })
                                                }}
                                            </span>
                                        </div>
                                    }
                                }
                            />
                        </div>
                        {move || {
                            matrix.get().is_empty().then(|| {
                                view! {
                                    <p class="text-xs text-zinc-500">
                                        "No torrents to chart yet — the matrix fills in as items arrive."
                                    </p>
                                }
                            })
                        }}
                    </div>
                </BentoCard>
            </div>

            {move || {
                heat_tip.get().map(|tip| {
                    let win_w = web_sys::window()
                        .and_then(|w| w.inner_width().ok())
                        .and_then(|v| v.as_f64())
                        .unwrap_or(1280.0);
                    let left = (tip.x + 14.0).min(win_w - 240.0).max(8.0);
                    let top = (tip.y - 12.0).max(8.0);
                    let offset = tip.offset;
                    let pct = tip.pct;
                    let meta = format!("{offset} · {pct}% intensity");
                    view! {
                        <div
                            class="pointer-events-none fixed z-50 -translate-y-full rounded-lg border border-white/[0.12] bg-zinc-900/95 px-3 py-2 shadow-[0_8px_32px_rgba(0,0,0,0.5)] backdrop-blur-xl"
                            style=format!("left: {left}px; top: {top}px;")
                        >
                            <div class="max-w-56 truncate text-xs font-medium text-zinc-100">{tip.name}</div>
                            <div class="mt-0.5 font-mono tabular-nums text-[10px] text-zinc-400">{meta}</div>
                        </div>
                    }
                })
            }}
        </div>
    }
}
