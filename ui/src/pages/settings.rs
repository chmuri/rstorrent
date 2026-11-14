use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

use crate::components::ui::*;
use crate::state::api;
use crate::state::*;

const INPUT_CLASS: &str = "w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all";

const SEGMENT_BASE: &str = "inline-flex items-center gap-1.5 rounded-lg px-3.5 py-1.5 text-sm font-medium transition-all duration-200 cursor-pointer";

const ACCENT_PRESETS: [(Accent, &str, &str); 4] = [
    (Accent::Indigo, "Indigo", "ring-indigo-500"),
    (Accent::Emerald, "Emerald", "ring-emerald-500"),
    (Accent::Rose, "Rose", "ring-rose-500"),
    (Accent::Cyan, "Cyan", "ring-cyan-500"),
];

fn input_value(ev: web_sys::Event) -> Option<String> {
    ev.target()
        .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
        .map(|input| input.value())
}

fn segment_class(active: bool) -> &'static str {
    if active {
        "bg-[var(--accent)] text-white shadow-[inset_0_1px_0_0_rgba(255,255,255,0.2)]"
    } else {
        "text-zinc-400 hover:text-zinc-100 hover:bg-white/[0.04]"
    }
}

fn connection_badge(mode: ConnectionMode) -> (&'static str, BadgeKind, bool) {
    match mode {
        ConnectionMode::Live => ("Live", BadgeKind::Success, true),
        ConnectionMode::Demo => ("Demo", BadgeKind::Warning, false),
        ConnectionMode::Offline => ("Offline", BadgeKind::Error, false),
    }
}

#[component]
fn DataActionRow(
    #[prop(into)] title: String,
    #[prop(into)] description: String,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-white/[0.08] bg-white/[0.02] px-4 py-3 transition-all duration-200 hover:border-white/[0.15]">
            <div class="min-w-0 flex-1">
                <div class="text-sm font-medium text-zinc-200">{title}</div>
                <div class="mt-0.5 text-xs text-zinc-500">{description}</div>
            </div>
            <div class="shrink-0">{children()}</div>
        </div>
    }
}

#[component]
fn InfoRow(#[prop(into)] label: String, children: Children) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-4 py-1.5">
            <span class="shrink-0 text-xs text-zinc-500">{label}</span>
            <span class="truncate text-right font-mono text-xs tabular-nums text-zinc-200">
                {children()}
            </span>
        </div>
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let testing = RwSignal::new(false);

    let run_test = move || {
        if testing.get() {
            return;
        }
        testing.set(true);
        let base = state.api_base_url.get();
        let token = state.api_token.get();
        spawn_local(async move {
            match api::test_connection(&base, &token).await {
                Ok(info) => {
                    state.push_toast(
                        ToastKind::Success,
                        "Connection successful",
                        format!(
                            "rstorrent {} (API {}) responded.",
                            info.version, info.api_version
                        ),
                    );
                    state.version.set(info);
                }
                Err(message) => {
                    state.push_toast(ToastKind::Error, "Connection failed", message);
                }
            }
            testing.set(false);
        });
    };

    let export_state = move || {
        let json = api::export_state_json(state);
        // export_state_json returns "{}" only on serialization failure.
        if json == "{}" {
            state.push_toast(
                ToastKind::Error,
                "Export failed",
                "Could not serialize the current state.",
            );
            return;
        }
        api::download_json("rstorrent-state.json", &json);
        state.push_toast(
            ToastKind::Success,
            "State exported",
            "Saved as rstorrent-state.json.",
        );
    };

    let accent_swatches = ACCENT_PRESETS
        .into_iter()
        .map(|(accent, name, ring)| {
            view! {
                <button
                    type="button"
                    title=name
                    aria-pressed=move || if state.accent.get() == accent { "true" } else { "false" }
                    class=move || {
                        let base = "flex flex-col items-center gap-2 rounded-xl border px-2 py-3 transition-all duration-200 active:scale-[0.98] cursor-pointer";
                        if state.accent.get() == accent {
                            format!("{base} border-white/[0.15] bg-white/[0.06] ring-2 {ring}")
                        } else {
                            format!("{base} border-white/[0.08] bg-white/[0.02] hover:bg-white/[0.05] hover:border-white/[0.15]")
                        }
                    }
                    on:click=move |_| {
                        state.accent.set(accent);
                        state.persist_settings();
                    }
                >
                    <span
                        class="h-6 w-6 rounded-full shadow-[inset_0_1px_0_0_rgba(255,255,255,0.25)]"
                        style=format!("background-color: {}", accent_hex(accent))
                    />
                    <span class=move || {
                        if state.accent.get() == accent {
                            "text-xs font-medium text-zinc-100"
                        } else {
                            "text-xs font-medium text-zinc-500"
                        }
                    }>{name}</span>
                </button>
            }
        })
        .collect_view();

    view! {
        <div>
            <div class="mb-6">
                <h1 class="text-2xl font-semibold tracking-tight">"Settings"</h1>
                <p class="mt-1 text-sm text-zinc-500">
                    "Backend connection, appearance, and local data. Changes are saved automatically."
                </p>
            </div>

            <div class="grid grid-cols-1 gap-4 md:grid-cols-12">
                <BentoCard
                    title="Backend Connection"
                    subtitle="Where the UI fetches torrents, stats, and events.".to_string()
                    col_span="col-span-12 md:col-span-7"
                    actions=view! {
                        {move || {
                            let (label, kind, pulse) = connection_badge(state.connection.get());
                            view! { <Badge label=label kind=kind pulse=pulse /> }
                        }}
                    }.into_any()
                >
                    <div class="space-y-5">
                        <div>
                            <label
                                for="settings-api-base"
                                class="mb-1.5 block text-sm font-medium text-zinc-200"
                            >
                                "API base URL"
                            </label>
                            <input
                                id="settings-api-base"
                                type="text"
                                class=INPUT_CLASS
                                placeholder="same origin"
                                autocomplete="off"
                                prop:value=move || state.api_base_url.get()
                                on:input=move |ev| {
                                    if let Some(value) = input_value(ev) {
                                        state.api_base_url.set(value);
                                        state.persist_settings();
                                    }
                                }
                            />
                            <p class="mt-1.5 text-xs text-zinc-500">
                                "Leave empty for same origin, e.g. http://127.0.0.1:8080"
                            </p>
                        </div>

                        <div>
                            <label
                                for="settings-api-token"
                                class="mb-1.5 block text-sm font-medium text-zinc-200"
                            >
                                "API token"
                            </label>
                            <input
                                id="settings-api-token"
                                type="password"
                                class=INPUT_CLASS
                                placeholder="Optional bearer token"
                                autocomplete="off"
                                prop:value=move || state.api_token.get()
                                on:input=move |ev| {
                                    if let Some(value) = input_value(ev) {
                                        state.api_token.set(value);
                                        state.persist_settings();
                                    }
                                }
                            />
                            <p class="mt-1.5 text-xs text-zinc-500">
                                "Sent as an Authorization: Bearer header when set."
                            </p>
                        </div>

                        <div>
                            <div class="flex items-center justify-between gap-3">
                                <label
                                    for="settings-poll-interval"
                                    class="text-sm font-medium text-zinc-200"
                                >
                                    "Polling interval"
                                </label>
                                <span class="rounded-md bg-white/[0.06] px-2 py-0.5 font-mono text-xs tabular-nums text-zinc-200">
                                    {move || format!("{} ms", state.poll_interval_ms.get())}
                                </span>
                            </div>
                            <input
                                id="settings-poll-interval"
                                type="range"
                                min="500"
                                max="10000"
                                step="250"
                                class="mt-2.5 w-full cursor-pointer accent-[var(--accent)]"
                                prop:value=move || state.poll_interval_ms.get().to_string()
                                on:input=move |ev| {
                                    if let Some(value) = input_value(ev) {
                                        if let Ok(ms) = value.parse::<u64>() {
                                            state.poll_interval_ms.set(ms.clamp(500, 10000));
                                            state.persist_settings();
                                        }
                                    }
                                }
                            />
                            <div class="mt-1 flex justify-between font-mono text-[10px] text-zinc-500">
                                <span>"500 ms"</span>
                                <span>"10 s"</span>
                            </div>
                            <p class="mt-1.5 text-xs text-zinc-500">
                                "Fallback polling cadence when the live WebSocket stream is unavailable."
                            </p>
                        </div>

                        <div class="flex flex-wrap items-center gap-3 pt-1">
                            // Button props are plain values in 0.7, so re-render on busy changes.
                            {move || {
                                let busy = testing.get();
                                view! {
                                    <Button
                                        label={if busy { "Testing..." } else { "Test Connection" }}
                                        variant=ButtonVariant::Secondary
                                        disabled=busy
                                        on_click=Callback::new(move |_| run_test())
                                    />
                                }
                            }}
                            <span class="text-xs text-zinc-500">
                                "Probes GET /api/version with the configured credentials."
                            </span>
                        </div>
                    </div>
                </BentoCard>

                <BentoCard
                    title="Appearance & UX"
                    subtitle="Theme, accent, density, and motion.".to_string()
                    col_span="col-span-12 md:col-span-5"
                >
                    <div class="space-y-5">
                        <div>
                            <div class="mb-2 text-sm font-medium text-zinc-200">"Theme"</div>
                            <div class="inline-flex gap-1 rounded-xl border border-white/[0.08] bg-zinc-950/80 p-1">
                                <button
                                    type="button"
                                    aria-pressed=move || if state.theme.get() == Theme::Dark { "true" } else { "false" }
                                    class=move || format!("{SEGMENT_BASE} {}", segment_class(state.theme.get() == Theme::Dark))
                                    on:click=move |_| {
                                        state.theme.set(Theme::Dark);
                                        state.persist_settings();
                                    }
                                >
                                    <svg
                                        xmlns="http://www.w3.org/2000/svg"
                                        viewBox="0 0 24 24"
                                        fill="none"
                                        stroke="currentColor"
                                        stroke-width="2"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        class="h-3.5 w-3.5"
                                    >
                                        <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
                                    </svg>
                                    "Dark"
                                </button>
                                <button
                                    type="button"
                                    aria-pressed=move || if state.theme.get() == Theme::Light { "true" } else { "false" }
                                    class=move || format!("{SEGMENT_BASE} {}", segment_class(state.theme.get() == Theme::Light))
                                    on:click=move |_| {
                                        state.theme.set(Theme::Light);
                                        state.persist_settings();
                                    }
                                >
                                    <svg
                                        xmlns="http://www.w3.org/2000/svg"
                                        viewBox="0 0 24 24"
                                        fill="none"
                                        stroke="currentColor"
                                        stroke-width="2"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        class="h-3.5 w-3.5"
                                    >
                                        <circle cx="12" cy="12" r="4" />
                                        <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
                                    </svg>
                                    "Light"
                                </button>
                            </div>
                        </div>

                        <div>
                            <div class="mb-2 text-sm font-medium text-zinc-200">"Accent color"</div>
                            <div class="grid grid-cols-4 gap-2">{accent_swatches}</div>
                        </div>

                        <div>
                            <div class="mb-2 text-sm font-medium text-zinc-200">"Density"</div>
                            <div class="inline-flex gap-1 rounded-xl border border-white/[0.08] bg-zinc-950/80 p-1">
                                <button
                                    type="button"
                                    aria-pressed=move || if state.density.get() == Density::Compact { "true" } else { "false" }
                                    class=move || format!("{SEGMENT_BASE} {}", segment_class(state.density.get() == Density::Compact))
                                    on:click=move |_| {
                                        state.density.set(Density::Compact);
                                        state.persist_settings();
                                    }
                                >
                                    "Compact"
                                </button>
                                <button
                                    type="button"
                                    aria-pressed=move || if state.density.get() == Density::Comfortable { "true" } else { "false" }
                                    class=move || format!("{SEGMENT_BASE} {}", segment_class(state.density.get() == Density::Comfortable))
                                    on:click=move |_| {
                                        state.density.set(Density::Comfortable);
                                        state.persist_settings();
                                    }
                                >
                                    "Comfortable"
                                </button>
                            </div>
                        </div>

                        <div class="flex items-center justify-between gap-4">
                            <div>
                                <div class="text-sm font-medium text-zinc-200">"Animations"</div>
                                <div class="mt-0.5 text-xs text-zinc-500">
                                    "Transitions and motion effects across the UI."
                                </div>
                            </div>
                            <button
                                type="button"
                                role="switch"
                                aria-checked=move || if state.animations_enabled.get() { "true" } else { "false" }
                                class=move || format!(
                                    "relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 cursor-pointer {}",
                                    if state.animations_enabled.get() { "bg-[var(--accent)]" } else { "bg-white/[0.15]" }
                                )
                                on:click=move |_| {
                                    state.animations_enabled.update(|enabled| *enabled = !*enabled);
                                    state.persist_settings();
                                }
                            >
                                <span class=move || format!(
                                    "pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform duration-200 {}",
                                    if state.animations_enabled.get() { "translate-x-[22px]" } else { "translate-x-0.5" }
                                ) />
                            </button>
                        </div>
                    </div>
                </BentoCard>

                <BentoCard
                    title="Data Management"
                    subtitle="Snapshots and local browser data.".to_string()
                    col_span="col-span-12 md:col-span-7"
                >
                    <div class="space-y-3">
                        <DataActionRow
                            title="Export State as JSON"
                            description="Download the current items and session stats as a JSON snapshot."
                        >
                            <Button
                                label="Export JSON"
                                variant=ButtonVariant::Secondary
                                on_click=Callback::new(move |_| export_state())
                            />
                        </DataActionRow>
                        <DataActionRow
                            title="Reset Demo Data"
                            description="Reseed the simulated dataset used when the backend is unreachable."
                        >
                            <Button
                                label="Reset Demo Data"
                                variant=ButtonVariant::Danger
                                on_click=Callback::new(move |_| api::reset_demo_data(state))
                            />
                        </DataActionRow>
                        <DataActionRow
                            title="Clear Local Storage"
                            description="Remove all saved rt-ui settings from this browser."
                        >
                            <Button
                                label="Clear Local Storage"
                                variant=ButtonVariant::Danger
                                on_click=Callback::new(move |_| api::clear_local_storage(state))
                            />
                        </DataActionRow>
                    </div>
                </BentoCard>

                <BentoCard
                    title="About"
                    subtitle="Build runtime and connection summary.".to_string()
                    col_span="col-span-12 md:col-span-5"
                >
                    <div>
                        <InfoRow label="Version">
                            {move || state.version.get().version}
                        </InfoRow>
                        <InfoRow label="API version">
                            {move || state.version.get().api_version}
                        </InfoRow>
                        <InfoRow label="Build target">
                            {move || state.version.get().build_target}
                        </InfoRow>
                        <InfoRow label="Uptime">
                            {move || format_duration(state.version.get().uptime_seconds)}
                        </InfoRow>
                        <div class="my-2 border-t border-white/[0.08]" />
                        <InfoRow label="Endpoint">
                            {move || {
                                let base = state.api_base_url.get();
                                if base.is_empty() {
                                    "same origin".to_string()
                                } else {
                                    base
                                }
                            }}
                        </InfoRow>
                        <InfoRow label="Auth">
                            {move || {
                                if state.api_token.get().is_empty() {
                                    "No token".to_string()
                                } else {
                                    "Bearer token".to_string()
                                }
                            }}
                        </InfoRow>
                        <InfoRow label="Polling">
                            {move || format!("{} ms", state.poll_interval_ms.get())}
                        </InfoRow>
                        <InfoRow label="Mode">
                            {move || connection_badge(state.connection.get()).0}
                        </InfoRow>
                    </div>
                </BentoCard>
            </div>
        </div>
    }
}
