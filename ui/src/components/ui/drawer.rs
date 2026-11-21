use leptos::task::spawn_local;
use leptos::{ev, prelude::*};

use crate::components::ui::{status_kind, Badge, Button, ButtonVariant};
use crate::state::*;

// Backend item timestamps are unix seconds; format_timestamp expects millis.
fn format_item_time(unix_secs: u64) -> String {
    if unix_secs == 0 {
        "—".to_string()
    } else {
        format_timestamp(unix_secs.saturating_mul(1000))
    }
}

fn severity_dot(kind: ToastKind) -> &'static str {
    match kind {
        ToastKind::Success => "bg-emerald-400",
        ToastKind::Warning => "bg-amber-400",
        ToastKind::Error => "bg-rose-400",
        ToastKind::Info => "bg-indigo-400",
    }
}

fn meta_cell(label: &'static str, value: String, mono: bool, wide: bool) -> impl IntoView {
    let wrapper = if wide {
        "col-span-2 min-w-0"
    } else {
        "min-w-0"
    };
    let value_class = match (mono, wide) {
        (true, true) => "mt-0.5 break-all font-mono text-xs text-zinc-200",
        (true, false) => "mt-0.5 truncate font-mono text-xs tabular-nums text-zinc-200",
        (false, true) => "mt-0.5 break-all text-xs text-zinc-200",
        (false, false) => "mt-0.5 truncate text-xs text-zinc-200",
    };
    let title = value.clone();
    view! {
        <div class=wrapper>
            <dt class="text-[11px] uppercase tracking-wide text-zinc-500">{label}</dt>
            <dd class=value_class title=title>{value}</dd>
        </div>
    }
}

#[component]
pub fn SlideOverDrawer() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let busy = RwSignal::new(false);

    // Refresh the selected snapshot from the live item list so rates stay current.
    let current_item = Memo::new(move |_| {
        state.selected_item.get().map(|snapshot| {
            match state
                .items
                .get()
                .iter()
                .find(|item| item.hash == snapshot.hash)
            {
                Some(live) => live.clone(),
                None => snapshot,
            }
        })
    });

    // Retain the last item so content stays visible during the slide-out animation.
    let shown = RwSignal::new(None::<TorrentItem>);
    Effect::new(move |_| {
        if let Some(item) = current_item.get() {
            if shown.get_untracked().as_ref() != Some(&item) {
                shown.set(Some(item));
            }
        }
    });

    let open = Memo::new(move |_| state.selected_item.get().is_some());
    let close = move || state.selected_item.set(None);

    // Tracker health for the selected torrent. Reload only when the selection
    // changes (not on every rate tick) plus an explicit refresh button.
    let trackers = RwSignal::new(None::<Result<Vec<TrackerInfo>, String>>);
    let selected_hash = Memo::new(move |_| current_item.get().map(|i| i.hash.clone()));
    let load_trackers = move |hash: String| {
        trackers.set(None);
        spawn_local(async move {
            let result = api::fetch_torrent_trackers(state, &hash).await;
            trackers.set(Some(result));
        });
    };
    Effect::new(move |_| {
        if let Some(hash) = selected_hash.get() {
            load_trackers(hash);
        }
    });

    let run_command = move |verb: &'static str| {
        if busy.get_untracked() {
            return;
        }
        let Some(item) = current_item.get_untracked() else {
            return;
        };
        busy.set(true);
        let hash = item.hash.clone();
        spawn_local(async move {
            let result = api::send_command(state, &hash, verb).await;
            busy.set(false);
            match result {
                Ok(resp) if resp.success => {
                    if verb == "remove" {
                        state.selected_item.set(None);
                    }
                }
                Ok(resp) => {
                    if !resp.error.is_empty() {
                        state.push_toast(ToastKind::Error, "Command failed", resp.error);
                    }
                }
                // the api layer already surfaced an error toast
                Err(_) => {}
            }
        });
    };

    // Open the shared remove-confirmation modal for the current torrent.
    let open_remove = move || {
        let Some(item) = current_item.get_untracked() else {
            return;
        };
        state.remove_confirm_hashes.set(vec![item.hash.clone()]);
        state.remove_confirm_label.set(item.name.clone());
        state.remove_confirm_open.set(true);
    };

    let key_handle = window_event_listener(ev::keydown, move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape"
            && !state.cmd_palette_open.get_untracked()
            && state.selected_item.get_untracked().is_some()
        {
            state.selected_item.set(None);
        }
    });
    on_cleanup(move || key_handle.remove());

    view! {
        <div
            class=move || format!(
                "fixed inset-0 z-40 bg-black/60 backdrop-blur-sm transition-opacity duration-300 {}",
                if open.get() { "opacity-100" } else { "pointer-events-none opacity-0" }
            )
            on:click=move |_| close()
            aria-hidden="true"
        ></div>
        <aside
            class=move || format!(
                "glass-surface fixed inset-y-0 right-0 z-50 flex w-full max-w-md flex-col bg-zinc-900/50 backdrop-blur-xl border border-white/[0.08] shadow-[0_8px_32px_rgba(0,0,0,0.4)] transition-all duration-300 ease-out {}",
                if open.get() { "translate-x-0 visible" } else { "translate-x-full invisible" }
            )
            role="dialog"
            aria-modal="true"
            aria-label="Torrent inspector"
        >
            <div class="flex items-start justify-between gap-3 border-b border-white/[0.08] px-5 py-4">
                <div class="min-w-0 flex-1">
                    <p class="text-[11px] uppercase tracking-wide text-zinc-500">"Torrent inspector"</p>
                    {move || shown.get().map(|item| view! {
                        <h2 class="mt-1 truncate text-base font-semibold tracking-tight text-zinc-100" title=item.name.clone()>
                            {item.name.clone()}
                        </h2>
                        <p class="mt-0.5 font-mono text-xs text-zinc-500">{item.short_hash()}</p>
                    })}
                </div>
                <button
                    type="button"
                    class="rounded-lg p-1.5 text-zinc-400 transition-colors hover:bg-white/[0.06] hover:text-zinc-100"
                    on:click=move |_| close()
                    aria-label="Close inspector"
                >
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                        <path d="M18 6 6 18" />
                        <path d="m6 6 12 12" />
                    </svg>
                </button>
            </div>
            <div class="flex-1 overflow-y-auto px-5 py-5">
                {move || {
                    let Some(item) = shown.get() else {
                        return view! { <div></div> }.into_any();
                    };
                    let status = item.status();
                    let progress = item.progress();
                    let message = item.message.clone();
                    let message_is_error = status == ItemStatus::Failed;
                    let item_hash = item.hash.clone();
                    let save_hash = item.hash.clone();
                    let label_input = RwSignal::new(item.category.clone());
                    let is_saving = RwSignal::new(false);
                    let json_preview = serde_json::to_string_pretty(&item);
                    view! {
                        <div class="space-y-6">
                            <div class="space-y-3">
                                <div class="flex items-center justify-between gap-3">
                                    <Badge
                                        label=item.status_label()
                                        kind=status_kind(status)
                                        pulse=status == ItemStatus::Downloading
                                    />
                                    <span class="font-mono text-xs tabular-nums text-zinc-400">{format_percent(progress)}</span>
                                </div>
                                <div class="h-2 w-full overflow-hidden rounded-full bg-white/[0.06]">
                                    <div
                                        class="h-full rounded-full bg-[var(--accent)] transition-all duration-500"
                                        style=format!("width: {:.2}%", progress * 100.0)
                                    ></div>
                                </div>
                                <p class="text-xs text-zinc-500">
                                    {format_bytes(item.completed_bytes)} " of " {format_bytes(item.size_bytes)}
                                </p>
                            </div>
                            <div class="grid grid-cols-2 gap-3">
                                <div class="rounded-xl bg-white/[0.04] border border-white/[0.08] px-3 py-2.5">
                                    <p class="text-[11px] uppercase tracking-wide text-zinc-500">"Download"</p>
                                    <p class="mt-1 font-mono text-sm tabular-nums text-zinc-100">{format_rate(item.down_rate)}</p>
                                </div>
                                <div class="rounded-xl bg-white/[0.04] border border-white/[0.08] px-3 py-2.5">
                                    <p class="text-[11px] uppercase tracking-wide text-zinc-500">"Upload"</p>
                                    <p class="mt-1 font-mono text-sm tabular-nums text-zinc-100">{format_rate(item.up_rate)}</p>
                                </div>
                            </div>
                            {(!message.is_empty()).then(|| view! {
                                <div class=if message_is_error {
                                    "rounded-xl border border-rose-500/20 bg-rose-500/10 px-3 py-2 text-xs text-rose-300"
                                } else {
                                    "rounded-xl border border-amber-500/20 bg-amber-500/10 px-3 py-2 text-xs text-amber-300"
                                }>
                                    {message}
                                </div>
                            })}
                            <div>
                                <h3 class="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">"Metadata"</h3>
                                <dl class="mt-2 grid grid-cols-2 gap-x-4 gap-y-3">
                                    {meta_cell("Hash", item.hash.clone(), true, true)}
                                    {meta_cell("Directory", item.directory.clone(), false, true)}
                                    <div class="col-span-2 rounded-xl border border-white/[0.08] bg-white/[0.03] p-3">
                                        <label class="text-[11px] font-semibold uppercase tracking-wider text-zinc-400">"Label / Category"</label>
                                        <div class="mt-1.5 flex items-center gap-2">
                                            <input
                                                type="text"
                                                class="flex-1 rounded-lg border border-white/[0.1] bg-black/40 px-3 py-1.5 text-xs text-zinc-100 placeholder-zinc-600 focus:border-[var(--accent)] focus:outline-none"
                                                placeholder="e.g. movies, linux, tv"
                                                prop:value=move || label_input.get()
                                                on:input=move |ev| label_input.set(event_target_value(&ev))
                                            />
                                            <button
                                                class="cursor-pointer rounded-lg bg-[var(--accent)] px-3 py-1.5 text-xs font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
                                                disabled=move || is_saving.get()
                                                on:click=move |_| {
                                                    let h = save_hash.clone();
                                                    let l = label_input.get();
                                                    is_saving.set(true);
                                                    spawn_local(async move {
                                                        let _ = update_torrent_label(state, &h, &l).await;
                                                        is_saving.set(false);
                                                    });
                                                }
                                            >
                                                {move || if is_saving.get() { "Saving..." } else { "Save" }}
                                            </button>
                                        </div>
                                    </div>
                                    {meta_cell("Priority", item.priority.to_string(), true, false)}
                                    {meta_cell("Size", format_bytes(item.size_bytes), true, false)}
                                    {meta_cell("Completed", format_bytes(item.completed_bytes), true, false)}
                                    {meta_cell("Uploaded", format_bytes(item.up_total), true, false)}
                                    {meta_cell("Downloaded", format_bytes(item.down_total), true, false)}
                                    {meta_cell("Started", format_item_time(item.started), true, false)}
                                    {meta_cell("Finished", format_item_time(item.finished), true, false)}
                                    {meta_cell("State changed", format_item_time(item.state_changed), true, false)}
                                </dl>
                            </div>
                            <div>
                                <div class="flex items-center justify-between">
                                    <h3 class="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">"Trackers"</h3>
                                    <button
                                        class="cursor-pointer rounded-md px-2 py-0.5 text-[10px] font-medium text-zinc-500 transition-colors hover:bg-white/[0.06] hover:text-zinc-200"
                                        title="Refresh tracker health"
                                        on:click=move |_| {
                                            if let Some(hash) = selected_hash.get_untracked() {
                                                load_trackers(hash);
                                            }
                                        }
                                    >
                                        "Refresh"
                                    </button>
                                </div>
                                {move || match trackers.get() {
                                    None => view! {
                                        <p class="mt-2 text-xs text-zinc-500">"Loading tracker health…"</p>
                                    }.into_any(),
                                    Some(Err(err)) => view! {
                                        <p class="mt-2 text-xs text-rose-400">"Tracker data unavailable: " {err}</p>
                                    }.into_any(),
                                    Some(Ok(list)) if list.is_empty() => view! {
                                        <p class="mt-2 text-xs text-zinc-500">"No trackers reported for this torrent."</p>
                                    }.into_any(),
                                    Some(Ok(list)) => view! {
                                        <ul class="mt-2 space-y-2">
                                            {list.into_iter().map(|t| {
                                                let dot = match t.health() {
                                                    "ok" => "bg-emerald-400",
                                                    "busy" => "bg-cyan-400 animate-pulse",
                                                    "failed" => "bg-rose-400",
                                                    "disabled" => "bg-zinc-600",
                                                    _ => "bg-amber-400",
                                                };
                                                let last = if t.success_time_last > 0 {
                                                    format_item_time(t.success_time_last)
                                                } else if t.failed_time_last > 0 {
                                                    format!("failed {}", format_item_time(t.failed_time_last))
                                                } else {
                                                    "never".to_string()
                                                };
                                                let swarm = if t.scrape_complete > 0 || t.scrape_incomplete > 0 {
                                                    format!("{}S / {}L", t.scrape_complete, t.scrape_incomplete)
                                                } else {
                                                    "—".to_string()
                                                };
                                                let fail_class = if t.failed_counter > 0 {
                                                    "text-rose-400/80"
                                                } else {
                                                    ""
                                                };
                                                view! {
                                                    <li class="rounded-xl border border-white/[0.08] bg-white/[0.03] px-3 py-2">
                                                        <div class="flex items-center gap-2">
                                                            <span class=format!("h-1.5 w-1.5 shrink-0 rounded-full {}", dot)></span>
                                                            <span class="min-w-0 flex-1 truncate font-mono text-xs text-zinc-200" title={t.url.clone()}>
                                                                {t.host()}
                                                            </span>
                                                            <span class="rounded bg-white/[0.06] px-1.5 py-0.5 font-mono text-[9px] uppercase text-zinc-500">
                                                                {t.tracker_type.clone()}
                                                            </span>
                                                            <span class="font-mono text-[10px] tabular-nums text-zinc-500">
                                                                {format!("tier {}", t.group)}
                                                            </span>
                                                        </div>
                                                        <div class="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 font-mono text-[10px] tabular-nums text-zinc-500">
                                                            <span class="text-emerald-400/80">{format!("✓{}", t.success_counter)}</span>
                                                            <span class=fail_class>
                                                                {format!("✗{}", t.failed_counter)}
                                                            </span>
                                                            <span>{format!("every {}s", t.normal_interval)}</span>
                                                            <span>{swarm}</span>
                                                            <span class="text-zinc-600">{last}</span>
                                                            <span class="text-zinc-600">{format!("event: {}", t.latest_event)}</span>
                                                        </div>
                                                    </li>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </ul>
                                    }.into_any(),
                                }}
                            </div>
                            <div>
                                <h3 class="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">"Activity"</h3>
                                {move || {
                                    let events: Vec<ActivityEvent> = state
                                        .events
                                        .get()
                                        .into_iter()
                                        .filter(|ev| ev.hash.as_deref() == Some(item_hash.as_str()))
                                        .rev()
                                        .collect();
                                    if events.is_empty() {
                                        return view! {
                                            <p class="mt-2 text-xs text-zinc-500">"No activity recorded for this torrent yet."</p>
                                        }.into_any();
                                    }
                                    view! {
                                        <ul class="mt-2 space-y-2.5">
                                            {events.into_iter().map(|ev| view! {
                                                <li class="flex items-start gap-2.5">
                                                    <span class=format!("mt-1 h-1.5 w-1.5 shrink-0 rounded-full {}", severity_dot(ev.severity))></span>
                                                    <div class="min-w-0 flex-1">
                                                        <p class="break-words text-xs text-zinc-300">{ev.message.clone()}</p>
                                                        <p class="mt-0.5 font-mono text-[10px] tabular-nums text-zinc-500">{format_timestamp(ev.timestamp)}</p>
                                                    </div>
                                                </li>
                                            }).collect::<Vec<_>>()}
                                        </ul>
                                    }.into_any()
                                }}
                            </div>
                            <div>
                                <h3 class="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">"Raw payload"</h3>
                                {match json_preview {
                                    Ok(json) => view! {
                                        <pre class="mt-2 max-h-64 overflow-auto rounded-xl bg-zinc-950/80 border border-white/[0.08] p-3 font-mono text-[11px] leading-relaxed text-zinc-300">{json}</pre>
                                    }.into_any(),
                                    Err(err) => view! {
                                        <p class="mt-2 text-xs text-rose-400">"Failed to serialize item: " {err.to_string()}</p>
                                    }.into_any(),
                                }}
                            </div>
                        </div>
                    }.into_any()
                }}
            </div>
            {move || {
                // Button props are plain values in 0.7, so re-render on busy changes.
                let is_busy = busy.get();
                view! {
                    <div class="flex items-center gap-2 border-t border-white/[0.08] px-5 py-4">
                        <Button label="Start" variant=ButtonVariant::Secondary disabled=is_busy on_click=move || run_command("start") />
                        <Button label="Stop" variant=ButtonVariant::Secondary disabled=is_busy on_click=move || run_command("stop") />
                        <Button label="Recheck" variant=ButtonVariant::Secondary disabled=is_busy on_click=move || run_command("recheck") />
                        <div class="flex-1"></div>
                        <Button label="Remove" variant=ButtonVariant::Danger disabled=is_busy on_click=move || open_remove() />
                    </div>
                }
            }}
        </aside>
    }
}
