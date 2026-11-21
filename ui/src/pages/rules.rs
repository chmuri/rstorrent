//! Automation Rules page (T9, plan.md §6.9): tabbed shell around the Rules
//! (list + editor drawer + dry-run) and History panels with a shared reload
//! effect, 15 s live-mode polling and the header actions — "Rules enabled"
//! kill switch (PUT /api/rules/settings), "Check now", export (blob download)
//! and import (file input). The pattern mirrors `pages/rss.rs`.

use gloo_net::http::{Method, RequestBuilder};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::{File, HtmlInputElement};

use crate::components::rules::{HistoryPanel, RuleEditor, RuleEditorHandle, RuleList};
use crate::components::ui::{Button, ButtonVariant};
use crate::state::api::{download_json, ApiError};
use crate::state::rules::{
    rules_check, rules_export, rules_import, RulesExport, RulesFlags, RulesPageStore, RulesTab,
};
use crate::state::{AppState, ConnectionMode, ToastKind};

const POLL_INTERVAL_MS: u32 = 15_000;

// Copied from pages/rss.rs (private there) to keep the segment look.
const SEGMENT_BASE: &str = "inline-flex items-center gap-1.5 rounded-lg px-3.5 py-1.5 text-sm font-medium transition-all duration-200 cursor-pointer";

fn segment_class(active: bool) -> &'static str {
    if active {
        "bg-[var(--accent)] text-white shadow-[inset_0_1px_0_0_rgba(255,255,255,0.2)]"
    } else {
        "text-zinc-400 hover:text-zinc-100 hover:bg-white/[0.04]"
    }
}

fn tab_button(store: RulesPageStore, tab: RulesTab, label: &'static str) -> impl IntoView {
    view! {
        <button
            type="button"
            aria-pressed=move || if store.tab.get() == tab { "true" } else { "false" }
            class=move || format!("{SEGMENT_BASE} {}", segment_class(store.tab.get() == tab))
            on:click=move |_| store.tab.set(tab)
        >
            {label}
        </button>
    }
}

/// PUT `/api/rules/settings` (plan §6.6, E1/T8 bridge for the kill switch).
/// GAP (reported, not fixed here): the T8 client in `state/rules.rs` exposes
/// no `rules_*_settings` function and its generic request helpers
/// (`rules_send`/`rules_json`) are private, so this minimal call lives in the
/// page (T9 ownership) and reuses the `RulesFlags` DTO. The T8 owner should
/// consolidate it into the client. Accepts a bare `{enabled, allow_destructive}`
/// response, a full `{seed, rules}` document, or an empty 204-style body.
async fn save_rules_flags(state: &AppState, flags: &RulesFlags) -> Result<RulesFlags, ApiError> {
    let base = state.api_base_url.get();
    let token = state.api_token.get();
    let url = format!("{base}/api/rules/settings");
    let mut builder = RequestBuilder::new(&url).method(Method::PUT);
    if !token.is_empty() {
        builder = builder.header("Authorization", &format!("Bearer {token}"));
    }
    let body = serde_json::to_string(flags)
        .map_err(|err| ApiError::Other(format!("Invalid request payload: {err}")))?;
    let request = builder
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|err| ApiError::Other(format!("Request failed: {err}")))?;
    let response = request
        .send()
        .await
        .map_err(|err| ApiError::Other(format!("Request failed: {err}")))?;
    if !response.ok() {
        let status = response.status();
        if status == 401 || status == 403 {
            return Err(ApiError::Unauthorized(status));
        }
        return Err(ApiError::Other(format!("Server returned HTTP {status}")));
    }
    let text = response
        .text()
        .await
        .map_err(|err| ApiError::Other(format!("Invalid payload: {err}")))?;
    if text.trim().is_empty() {
        return Ok(flags.clone());
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| ApiError::Other(format!("Invalid payload: {err}")))?;
    let flags_value = value.get("rules").unwrap_or(&value);
    serde_json::from_value::<RulesFlags>(flags_value.clone())
        .map_err(|err| ApiError::Other(format!("Invalid payload: {err}")))
}

/// Automation Rules page: provides the shared store and editor handle, loads
/// rules/labels/seed settings/history and mounts the active tab panel.
#[component]
pub fn RulesPage() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = RulesPageStore::new();
    // Panels read the store from context, so it must exist before they render.
    provide_context(store);
    let editor = RuleEditorHandle::new();
    provide_context(editor);

    let checking = RwSignal::new(false);
    let saving_flags = RwSignal::new(false);
    let exporting = RwSignal::new(false);
    let importing = RwSignal::new(false);
    let import_input = NodeRef::<leptos::html::Input>::new();

    Effect::new(move |_| {
        let _reload = store.reload.get();
        let live = state.connection.get() == ConnectionMode::Live;
        if !live {
            // Invalidate in-flight responses when going offline.
            store.invalidate();
            return;
        }
        spawn_local(async move {
            if let Err(err) = store.load_all(&state).await {
                state.push_toast(
                    ToastKind::Error,
                    "Could not load rules data",
                    err.to_string(),
                );
            }
        });
    });

    // Interval owns a JS closure and is !Send, so it needs thread-local storage.
    // Created on mount, dropped on unmount: polling runs only while /rules is
    // mounted, and only ticks in live mode.
    let poll_interval: RwSignal<Option<Interval>, LocalStorage> = RwSignal::new_local(None);
    poll_interval.set(Some(Interval::new(POLL_INTERVAL_MS, move || {
        if state.connection.get_untracked() == ConnectionMode::Live {
            store.reload.update(|n| *n += 1);
        }
    })));
    // Replacing the handle drops (and cancels) the interval on unmount.
    on_cleanup(move || poll_interval.set(None));

    let check_now = move || {
        if checking.get() {
            return;
        }
        checking.set(true);
        spawn_local(async move {
            match rules_check(&state).await {
                Ok(decisions) => {
                    state.push_toast(
                        ToastKind::Success,
                        "Rules check finished",
                        format!("{} decisions fired.", decisions.len()),
                    );
                    // Refresh the lists so the History tab shows the new audit
                    // entries without waiting for the next poll.
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Rules check failed", err.to_string());
                }
            }
            checking.set(false);
        });
    };

    // Kill switch: flip only the [rules] flags via PUT /api/rules/settings;
    // the store keeps the old value on failure so the switch snaps back.
    let toggle_rules_enabled = move || {
        if saving_flags.get() {
            return;
        }
        let mut flags = store.seed_settings.get().rules;
        flags.enabled = !flags.enabled;
        saving_flags.set(true);
        spawn_local(async move {
            match save_rules_flags(&state, &flags).await {
                Ok(saved) => {
                    let enabled = saved.enabled;
                    store
                        .seed_settings
                        .update(|settings| settings.rules = saved);
                    state.push_toast(
                        ToastKind::Success,
                        "Rules settings saved",
                        if enabled {
                            "Automation rules are enabled."
                        } else {
                            "Automation rules are disabled (kill switch)."
                        },
                    );
                }
                Err(err) => {
                    state.push_toast(
                        ToastKind::Error,
                        "Could not save rules settings",
                        err.to_string(),
                    );
                }
            }
            saving_flags.set(false);
        });
    };

    let export_rules = move || {
        if exporting.get() {
            return;
        }
        exporting.set(true);
        spawn_local(async move {
            match rules_export(&state).await {
                Ok(blob) => match serde_json::to_string_pretty(&blob) {
                    Ok(json) => {
                        download_json("rstorrent-rules-backup.json", &json);
                        state.push_toast(
                            ToastKind::Success,
                            "Export ready",
                            "rstorrent-rules-backup.json download started.",
                        );
                    }
                    Err(err) => {
                        state.push_toast(ToastKind::Error, "Export failed", err.to_string());
                    }
                },
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Export failed", err.to_string());
                }
            }
            exporting.set(false);
        });
    };

    // Import: read the picked JSON file as text (FileReader pattern from
    // AddTorrentModal), parse the export blob and merge it via the API.
    let handle_import_file = move |file: File| {
        let reader = match web_sys::FileReader::new() {
            Ok(reader) => reader,
            Err(_) => {
                state.push_toast(
                    ToastKind::Error,
                    "Import failed",
                    "Could not read the selected file.",
                );
                importing.set(false);
                return;
            }
        };
        let reader_clone = reader.clone();
        let on_load =
            wasm_bindgen::closure::Closure::wrap(Box::new(move |_: web_sys::ProgressEvent| {
                let text = match reader_clone.result() {
                    Ok(result) => result.as_string(),
                    Err(_) => None,
                };
                let Some(text) = text else {
                    state.push_toast(
                        ToastKind::Error,
                        "Import failed",
                        "Could not read the selected file.",
                    );
                    importing.set(false);
                    return;
                };
                spawn_local(async move {
                    match serde_json::from_str::<RulesExport>(&text) {
                        Err(err) => {
                            state.push_toast(
                                ToastKind::Error,
                                "Invalid backup file",
                                format!("Expected a rules export JSON: {err}"),
                            );
                        }
                        Ok(blob) => match rules_import(&state, &blob).await {
                            Ok(()) => {
                                state.push_toast(
                                    ToastKind::Success,
                                    "Rules imported",
                                    format!(
                                        "{} rule(s) and {} label(s) merged.",
                                        blob.rules.len(),
                                        blob.labels.len()
                                    ),
                                );
                                store.reload.update(|n| *n += 1);
                            }
                            Err(err) => {
                                state.push_toast(
                                    ToastKind::Error,
                                    "Import failed",
                                    err.to_string(),
                                );
                            }
                        },
                    }
                    importing.set(false);
                });
            }) as Box<dyn FnMut(_)>);
        reader.set_onload(Some(on_load.as_ref().unchecked_ref()));
        if reader.read_as_text(&file).is_err() {
            state.push_toast(
                ToastKind::Error,
                "Import failed",
                "Could not read the selected file.",
            );
            importing.set(false);
        }
        on_load.forget();
    };

    let pick_import_file = move || {
        if importing.get() {
            return;
        }
        if let Some(input) = import_input.get() {
            // Allow re-importing the same file after a failed attempt.
            input.set_value("");
            input.click();
        }
    };

    view! {
        <div>
            <div class="mb-6 flex flex-wrap items-start justify-between gap-4">
                <div>
                    <h1 class="text-2xl font-semibold tracking-tight">"Automation Rules"</h1>
                    <p class="mt-1 text-sm text-zinc-500">
                        "Seed limits and condition-based automation for your torrents."
                    </p>
                </div>
                <div class="flex flex-wrap items-center gap-3">
                    <div class="flex items-center gap-2">
                        <span class="text-xs font-medium text-zinc-400">"Rules enabled"</span>
                        {move || {
                            let enabled = store.seed_settings.get().rules.enabled;
                            let busy = saving_flags.get();
                            view! {
                                <button
                                    id="rules-enabled-switch"
                                    type="button"
                                    role="switch"
                                    aria-checked={if enabled { "true" } else { "false" }}
                                    title={if enabled { "Disable automation rules" } else { "Enable automation rules" }}
                                    disabled=busy
                                    class=format!(
                                        "relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 cursor-pointer {} {}",
                                        if enabled { "bg-[var(--accent)]" } else { "bg-white/[0.15]" },
                                        if busy { "opacity-50 pointer-events-none" } else { "" },
                                    )
                                    on:click=move |_| toggle_rules_enabled()
                                >
                                    <span class=format!(
                                        "pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform duration-200 {}",
                                        if enabled { "translate-x-[22px]" } else { "translate-x-0.5" },
                                    ) />
                                </button>
                            }
                        }}
                    </div>
                    // Button props are plain values, so re-render on busy changes.
                    {move || {
                        let busy = checking.get();
                        view! {
                            <Button
                                label={if busy { "Checking..." } else { "Check now" }}
                                variant=ButtonVariant::Secondary
                                disabled=busy
                                on_click=Callback::new(move |_| check_now())
                            />
                        }
                    }}
                    {move || {
                        let busy = exporting.get();
                        view! {
                            <Button
                                label={if busy { "Exporting..." } else { "Export" }}
                                variant=ButtonVariant::Secondary
                                disabled=busy
                                on_click=Callback::new(move |_| export_rules())
                            />
                        }
                    }}
                    {move || {
                        let busy = importing.get();
                        view! {
                            <Button
                                label={if busy { "Importing..." } else { "Import" }}
                                variant=ButtonVariant::Secondary
                                disabled=busy
                                on_click=Callback::new(move |_| pick_import_file())
                            />
                        }
                    }}
                    <input
                        node_ref=import_input
                        type="file"
                        accept=".json,application/json"
                        class="hidden"
                        aria-label="Import rules backup file"
                        on:change=move |ev: web_sys::Event| {
                            let Some(target) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) else {
                                return;
                            };
                            let Some(files) = target.files() else {
                                return;
                            };
                            let Some(file) = files.get(0) else {
                                return;
                            };
                            importing.set(true);
                            handle_import_file(file);
                        }
                    />
                </div>
            </div>

            <Show
                when=move || state.connection.get() == ConnectionMode::Live
                fallback=move || {
                    view! {
                        <div class="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-white/[0.12] bg-white/[0.02] px-6 py-12 text-center">
                            <p class="text-sm font-medium text-zinc-300">
                                "Backend offline — rules require a live connection"
                            </p>
                        </div>
                    }
                }
            >
                <div class="mb-4 inline-flex gap-1 rounded-xl border border-white/[0.08] bg-zinc-950/80 p-1">
                    {tab_button(store, RulesTab::Rules, "Rules")}
                    {tab_button(store, RulesTab::History, "History")}
                </div>
                <div class="grid grid-cols-1 gap-4 md:grid-cols-12">
                    {move || match store.tab.get() {
                        RulesTab::Rules => view! {
                            <RuleList />
                            <RuleEditor />
                        }.into_any(),
                        RulesTab::History => view! { <HistoryPanel /> }.into_any(),
                    }}
                </div>
            </Show>
        </div>
    }
}
