use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};

use crate::components::labels::{
    build_action, destructive_warning, parse_duration_value, parse_ratio_value, split_duration,
    ActionKind, LabelManager, TimeUnit,
};
use crate::components::ui::*;
use crate::state::api;
use crate::state::rules::{
    seed_get_settings, seed_save_settings, LimitMode, RulesPageStore, SeedSettings,
};
use crate::state::*;

const INPUT_CLASS: &str = "w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all";

const SEGMENT_BASE: &str = "inline-flex items-center gap-1.5 rounded-lg px-3.5 py-1.5 text-sm font-medium transition-all duration-200 cursor-pointer";

const ACCENT_PRESETS: [(Accent, &str, &str); 4] = [
    (Accent::Indigo, "Indigo", "ring-indigo-500"),
    (Accent::Emerald, "Emerald", "ring-emerald-500"),
    (Accent::Rose, "Rose", "ring-rose-500"),
    (Accent::Cyan, "Cyan", "ring-cyan-500"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    ClientUI,
    DaemonConfig,
    RawToml,
}

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

fn render_accent_swatches(state: AppState) -> impl IntoView {
    ACCENT_PRESETS
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
        .collect_view()
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let current_tab = RwSignal::new(SettingsTab::ClientUI);
    let testing = RwSignal::new(false);

    let daemon_config = RwSignal::new(DaemonConfig::default());
    let raw_toml = RwSignal::new(String::new());
    let config_path = RwSignal::new(Option::<String>::None);
    let loading_config = RwSignal::new(false);
    let saving_config = RwSignal::new(false);

    let load_server_config = move || {
        let base = state.api_base_url.get();
        let token = state.api_token.get();
        loading_config.set(true);

        spawn_local(async move {
            match api::fetch_daemon_config(&base, &token).await {
                Ok(resp) => {
                    daemon_config.set(resp.config);
                    raw_toml.set(resp.toml);
                    config_path.set(resp.config_path);
                    state.push_toast(
                        ToastKind::Success,
                        "Config loaded",
                        "Daemon configuration retrieved",
                    );
                }
                Err(err) => {
                    let template_toml = r#"# rtorrent.toml - default template
[network]
port = 6881
port_range = "6881-6889"
download_throttle_kb = 0
upload_throttle_kb = 0
max_uploads = 50
max_downloads = 50
max_peers = 100
max_peers_seed = 50
enable_upnp = true
enable_utp = true

[storage]
directory = "~/Downloads"
session = "~/.local/share/rstorrent/session"
disk_threads = 4
max_open_files = 256

[protocol]
encryption = "allow"

[dht]
mode = "auto"
port = 6881

[logging]
level = "info"

[tiering]
enabled = false
check_interval = 60

[rss]
enabled = true
refresh_interval_secs = 1800
"#;
                    raw_toml.set(template_toml.to_string());
                    config_path.set(Some("rstorrent.toml (Demo)".to_string()));
                    if state.connection.get() == ConnectionMode::Live {
                        state.push_toast(ToastKind::Warning, "Remote config unavailable", err);
                    }
                }
            }
            loading_config.set(false);
        });
    };

    let save_config = move |save_file: bool| {
        if saving_config.get() {
            return;
        }
        saving_config.set(true);
        let base = state.api_base_url.get();
        let token = state.api_token.get();
        let cfg = daemon_config.get();

        spawn_local(async move {
            if state.connection.get() == ConnectionMode::Live {
                match api::update_daemon_config(&base, &token, Some(cfg), None, save_file).await {
                    Ok(resp) if resp.success => {
                        let msg = if save_file {
                            "Saved to rstorrent.toml and applied to runtime."
                        } else {
                            "Applied to runtime (in-memory)."
                        };
                        state.push_toast(ToastKind::Success, "Configuration updated", msg);
                        load_server_config();
                    }
                    Ok(resp) => {
                        state.push_toast(ToastKind::Error, "Config update failed", resp.error);
                    }
                    Err(e) => {
                        state.push_toast(ToastKind::Error, "Config update failed", e);
                    }
                }
            } else {
                state.push_toast(
                    ToastKind::Success,
                    "Config updated (Demo)",
                    "Settings applied locally",
                );
            }
            saving_config.set(false);
        });
    };

    let save_raw_toml = move |save_file: bool| {
        if saving_config.get() {
            return;
        }
        saving_config.set(true);
        let base = state.api_base_url.get();
        let token = state.api_token.get();
        let toml_str = raw_toml.get();

        spawn_local(async move {
            if state.connection.get() == ConnectionMode::Live {
                match api::update_daemon_config(&base, &token, None, Some(toml_str), save_file)
                    .await
                {
                    Ok(resp) if resp.success => {
                        state.push_toast(
                            ToastKind::Success,
                            "TOML applied",
                            "Raw TOML configuration parsed and saved.",
                        );
                        load_server_config();
                    }
                    Ok(resp) => {
                        state.push_toast(ToastKind::Error, "TOML parse error", resp.error);
                    }
                    Err(e) => {
                        state.push_toast(ToastKind::Error, "Update failed", e);
                    }
                }
            } else {
                state.push_toast(
                    ToastKind::Success,
                    "TOML applied (Demo)",
                    "Raw configuration verified",
                );
            }
            saving_config.set(false);
        });
    };

    let download_toml = move || {
        let text = raw_toml.get();
        if text.is_empty() {
            state.push_toast(
                ToastKind::Warning,
                "Export warning",
                "No TOML content to export",
            );
            return;
        }
        api::download_json("rstorrent.toml", &text);
        state.push_toast(ToastKind::Success, "Downloaded", "rstorrent.toml exported");
    };

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

    view! {
        <div>
            <div class="mb-6 flex flex-wrap items-center justify-between gap-4">
                <div>
                    <h1 class="text-2xl font-semibold tracking-tight">"Settings"</h1>
                    <p class="mt-1 text-sm text-zinc-500">
                        "Manage UI appearance, backend connection, and daemon rtorrent.toml parameters."
                    </p>
                </div>
                <div class="inline-flex gap-1 rounded-xl border border-white/[0.08] bg-zinc-950/80 p-1">
                    <button
                        type="button"
                        class=move || format!("{SEGMENT_BASE} {}", segment_class(current_tab.get() == SettingsTab::ClientUI))
                        on:click=move |_| current_tab.set(SettingsTab::ClientUI)
                    >
                        "Client UI"
                    </button>
                    <button
                        type="button"
                        class=move || format!("{SEGMENT_BASE} {}", segment_class(current_tab.get() == SettingsTab::DaemonConfig))
                        on:click=move |_| {
                            current_tab.set(SettingsTab::DaemonConfig);
                            load_server_config();
                        }
                    >
                        "Daemon Config (rstorrent.toml)"
                    </button>
                    <button
                        type="button"
                        class=move || format!("{SEGMENT_BASE} {}", segment_class(current_tab.get() == SettingsTab::RawToml))
                        on:click=move |_| {
                            current_tab.set(SettingsTab::RawToml);
                            if raw_toml.get().is_empty() {
                                load_server_config();
                            }
                        }
                    >
                        "Raw TOML Editor"
                    </button>
                </div>
            </div>

            {move || match current_tab.get() {
                SettingsTab::ClientUI => view! {
                    <div class="grid grid-cols-1 gap-4 md:grid-cols-12 animate-in fade-in duration-150">
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
                                    <label for="settings-api-base" class="mb-1.5 block text-sm font-medium text-zinc-200">
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
                                    <label for="settings-api-token" class="mb-1.5 block text-sm font-medium text-zinc-200">
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
                                        <label for="settings-poll-interval" class="text-sm font-medium text-zinc-200">
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
                                            "Light"
                                        </button>
                                    </div>
                                </div>

                                <div>
                                    <div class="mb-2 text-sm font-medium text-zinc-200">"Accent color"</div>
                                    <div class="grid grid-cols-4 gap-2">{render_accent_swatches(state)}</div>
                                </div>

                                <div>
                                    <div class="mb-2 text-sm font-medium text-zinc-200">"Density"</div>
                                    <div class="inline-flex gap-1 rounded-xl border border-white/[0.08] bg-zinc-950/80 p-1">
                                        <button
                                            type="button"
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
                                <InfoRow label="Version">{move || state.version.get().version}</InfoRow>
                                <InfoRow label="API version">{move || state.version.get().api_version}</InfoRow>
                                <InfoRow label="Build target">{move || state.version.get().build_target}</InfoRow>
                                <InfoRow label="Uptime">{move || format_duration(state.version.get().uptime_seconds)}</InfoRow>
                                <div class="my-2 border-t border-white/[0.08]" />
                                <InfoRow label="Endpoint">
                                    {move || {
                                        let base = state.api_base_url.get();
                                        if base.is_empty() { "same origin".to_string() } else { base }
                                    }}
                                </InfoRow>
                                <InfoRow label="Auth">
                                    {move || if state.api_token.get().is_empty() { "No token".to_string() } else { "Bearer token".to_string() }}
                                </InfoRow>
                                <InfoRow label="Polling">{move || format!("{} ms", state.poll_interval_ms.get())}</InfoRow>
                                <InfoRow label="Mode">{move || connection_badge(state.connection.get()).0}</InfoRow>
                            </div>
                        </BentoCard>
                    </div>
                }.into_any(),

                SettingsTab::DaemonConfig => view! {
                    <div class="space-y-4 animate-in fade-in duration-150">
                        <div class="flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-white/[0.08] bg-zinc-900/60 p-4">
                            <div class="flex items-center gap-2">
                                <span class="text-xs text-zinc-400">"Config File:"</span>
                                <span class="rounded bg-white/[0.06] px-2 py-1 font-mono text-xs text-zinc-200">
                                    {move || config_path.get().unwrap_or_else(|| "rstorrent.toml".to_string())}
                                </span>
                            </div>
                            <div class="flex items-center gap-2">
                                <Button
                                    label="Reload"
                                    variant=ButtonVariant::Ghost
                                    disabled=loading_config.get()
                                    on_click=Callback::new(move |_| load_server_config())
                                />
                                <Button
                                    label="Apply (Runtime)"
                                    variant=ButtonVariant::Secondary
                                    disabled=saving_config.get()
                                    on_click=Callback::new(move |_| save_config(false))
                                />
                                <Button
                                    label="Save to rstorrent.toml"
                                    variant=ButtonVariant::Primary
                                    disabled=saving_config.get()
                                    on_click=Callback::new(move |_| save_config(true))
                                />
                            </div>
                        </div>

                        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                            <BentoCard
                                title="Network & Bandwidth Throttles"
                                subtitle="Ports, IP bindings, UPnP/uTP and speed caps [network]".to_string()
                                col_span="col-span-1 md:col-span-1"
                            >
                                <div class="space-y-3.5">
                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Listen Port"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="6881"
                                                prop:value=move || daemon_config.get().network.port.map(|p| p.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u16>().ok());
                                                    daemon_config.update(|c| c.network.port = val);
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Port Range"</label>
                                            <input
                                                type="text"
                                                class=INPUT_CLASS
                                                placeholder="6881-6999"
                                                prop:value=move || daemon_config.get().network.port_range.unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).filter(|s| !s.is_empty());
                                                    daemon_config.update(|c| c.network.port_range = val);
                                                }
                                            />
                                        </div>
                                    </div>

                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Bind Address"</label>
                                            <input
                                                type="text"
                                                class=INPUT_CLASS
                                                placeholder="0.0.0.0"
                                                prop:value=move || daemon_config.get().network.bind_address.unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).filter(|s| !s.is_empty());
                                                    daemon_config.update(|c| c.network.bind_address = val);
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Local Address"</label>
                                            <input
                                                type="text"
                                                class=INPUT_CLASS
                                                placeholder="outgoing IP"
                                                prop:value=move || daemon_config.get().network.local_address.unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).filter(|s| !s.is_empty());
                                                    daemon_config.update(|c| c.network.local_address = val);
                                                }
                                            />
                                        </div>
                                    </div>

                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Download Throttle (KiB/s)"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="0 = unlimited"
                                                prop:value=move || daemon_config.get().network.download_throttle_kb.unwrap_or(0).to_string()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u64>().ok());
                                                    daemon_config.update(|c| c.network.download_throttle_kb = val);
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Upload Throttle (KiB/s)"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="0 = unlimited"
                                                prop:value=move || daemon_config.get().network.upload_throttle_kb.unwrap_or(0).to_string()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u64>().ok());
                                                    daemon_config.update(|c| c.network.upload_throttle_kb = val);
                                                }
                                            />
                                        </div>
                                    </div>

                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Max Upload Slots"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="50"
                                                prop:value=move || daemon_config.get().network.max_uploads.map(|v| v.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u32>().ok());
                                                    daemon_config.update(|c| c.network.max_uploads = val);
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Max Download Slots"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="50"
                                                prop:value=move || daemon_config.get().network.max_downloads.map(|v| v.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u32>().ok());
                                                    daemon_config.update(|c| c.network.max_downloads = val);
                                                }
                                            />
                                        </div>
                                    </div>

                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Max Peers (Leech)"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="100"
                                                prop:value=move || daemon_config.get().network.max_peers.map(|v| v.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u32>().ok());
                                                    daemon_config.update(|c| c.network.max_peers = val);
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Max Peers (Seed)"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="50"
                                                prop:value=move || daemon_config.get().network.max_peers_seed.map(|v| v.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u32>().ok());
                                                    daemon_config.update(|c| c.network.max_peers_seed = val);
                                                }
                                            />
                                        </div>
                                    </div>

                                    <div>
                                        <label class="block text-xs font-medium text-zinc-300 mb-1">"Outbound Proxy URL"</label>
                                        <input
                                            type="text"
                                            class=INPUT_CLASS
                                            placeholder="socks5://127.0.0.1:1080"
                                            prop:value=move || daemon_config.get().network.proxy.unwrap_or_default()
                                            on:input=move |ev| {
                                                let val = input_value(ev).filter(|s| !s.is_empty());
                                                daemon_config.update(|c| c.network.proxy = val);
                                            }
                                        />
                                    </div>

                                    <div class="flex items-center gap-6 pt-1">
                                        <label class="flex items-center gap-2 cursor-pointer select-none">
                                            <input
                                                type="checkbox"
                                                class="rounded border-white/20 bg-zinc-950 text-indigo-600 focus:ring-0 cursor-pointer h-4 w-4"
                                                prop:checked=move || daemon_config.get().network.enable_upnp.unwrap_or(true)
                                                on:change=move |ev| {
                                                    let checked = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).map(|i| i.checked());
                                                    daemon_config.update(|c| c.network.enable_upnp = checked);
                                                }
                                            />
                                            <span class="text-xs text-zinc-300 font-medium">"Enable UPnP / NAT-PMP"</span>
                                        </label>
                                        <label class="flex items-center gap-2 cursor-pointer select-none">
                                            <input
                                                type="checkbox"
                                                class="rounded border-white/20 bg-zinc-950 text-indigo-600 focus:ring-0 cursor-pointer h-4 w-4"
                                                prop:checked=move || daemon_config.get().network.enable_utp.unwrap_or(true)
                                                on:change=move |ev| {
                                                    let checked = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).map(|i| i.checked());
                                                    daemon_config.update(|c| c.network.enable_utp = checked);
                                                }
                                            />
                                            <span class="text-xs text-zinc-300 font-medium">"Enable uTP (BEP 29)"</span>
                                        </label>
                                    </div>
                                </div>
                            </BentoCard>

                            <BentoCard
                                title="Storage & Disk Management"
                                subtitle="Download paths, session state, and disk worker threads [storage]".to_string()
                                col_span="col-span-1 md:col-span-1"
                            >
                                <div class="space-y-3.5">
                                    <div>
                                        <label class="block text-xs font-medium text-zinc-300 mb-1">"Default Download Directory"</label>
                                        <input
                                            type="text"
                                            class=INPUT_CLASS
                                            placeholder="~/Downloads"
                                            prop:value=move || daemon_config.get().storage.directory.unwrap_or_default()
                                            on:input=move |ev| {
                                                let val = input_value(ev).filter(|s| !s.is_empty());
                                                daemon_config.update(|c| c.storage.directory = val);
                                            }
                                        />
                                    </div>

                                    <div>
                                        <label class="block text-xs font-medium text-zinc-300 mb-1">"Session Directory (Resume Data)"</label>
                                        <input
                                            type="text"
                                            class=INPUT_CLASS
                                            placeholder="~/.local/share/rstorrent/session"
                                            prop:value=move || daemon_config.get().storage.session.unwrap_or_default()
                                            on:input=move |ev| {
                                                let val = input_value(ev).filter(|s| !s.is_empty());
                                                daemon_config.update(|c| c.storage.session = val);
                                            }
                                        />
                                    </div>

                                    <div>
                                        <label class="block text-xs font-medium text-zinc-300 mb-1">"Watch Directory (Auto-Load)"</label>
                                        <input
                                            type="text"
                                            class=INPUT_CLASS
                                            placeholder="~/Downloads/watch"
                                            prop:value=move || daemon_config.get().storage.watch_directory.unwrap_or_default()
                                            on:input=move |ev| {
                                                let val = input_value(ev).filter(|s| !s.is_empty());
                                                daemon_config.update(|c| c.storage.watch_directory = val);
                                            }
                                        />
                                    </div>

                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Disk Worker Threads"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="4"
                                                prop:value=move || daemon_config.get().storage.disk_threads.map(|v| v.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<usize>().ok());
                                                    daemon_config.update(|c| c.storage.disk_threads = val);
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Max Open Files Per Thread"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="256"
                                                prop:value=move || daemon_config.get().storage.max_open_files.map(|v| v.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<usize>().ok());
                                                    daemon_config.update(|c| c.storage.max_open_files = val);
                                                }
                                            />
                                        </div>
                                    </div>
                                </div>
                            </BentoCard>

                            <BentoCard
                                title="BitTorrent Protocol & DHT"
                                subtitle="Wire encryption, custom peer ID, and Kademlia DHT [protocol, dht]".to_string()
                                col_span="col-span-1 md:col-span-1"
                            >
                                <div class="space-y-3.5">
                                    <div>
                                        <label class="block text-xs font-medium text-zinc-300 mb-1">"Protocol Encryption (MSE/PE)"</label>
                                        <select
                                            class=INPUT_CLASS
                                            prop:value=move || daemon_config.get().protocol.encryption.unwrap_or_else(|| "allow".to_string())
                                            on:change=move |ev| {
                                                if let Some(val) = ev.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()).map(|s| s.value()) {
                                                    daemon_config.update(|c| c.protocol.encryption = Some(val));
                                                }
                                            }
                                        >
                                            <option value="allow">"Allow encryption (allow / allow)"</option>
                                            <option value="prefer">"Prefer encryption (try_outgoing)"</option>
                                            <option value="require">"Require encryption (require / require)"</option>
                                            <option value="deny">"Deny encryption (plaintext only)"</option>
                                        </select>
                                    </div>

                                    <div>
                                        <label class="block text-xs font-medium text-zinc-300 mb-1">"Custom Peer ID"</label>
                                        <input
                                            type="text"
                                            class=INPUT_CLASS
                                            placeholder="Generated automatically"
                                            prop:value=move || daemon_config.get().protocol.peer_id.unwrap_or_default()
                                            on:input=move |ev| {
                                                let val = input_value(ev).filter(|s| !s.is_empty());
                                                daemon_config.update(|c| c.protocol.peer_id = val);
                                            }
                                        />
                                    </div>

                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"DHT Mode"</label>
                                            <select
                                                class=INPUT_CLASS
                                                prop:value=move || daemon_config.get().dht.mode.unwrap_or_else(|| "auto".to_string())
                                                on:change=move |ev| {
                                                    if let Some(val) = ev.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()).map(|s| s.value()) {
                                                        daemon_config.update(|c| c.dht.mode = Some(val));
                                                    }
                                                }
                                            >
                                                <option value="auto">"Auto (On demand)"</option>
                                                <option value="on">"On (Always enabled)"</option>
                                                <option value="off">"Off (Disabled)"</option>
                                            </select>
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"DHT UDP Port"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="6881"
                                                prop:value=move || daemon_config.get().dht.port.map(|p| p.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u16>().ok());
                                                    daemon_config.update(|c| c.dht.port = val);
                                                }
                                            />
                                        </div>
                                    </div>
                                </div>
                            </BentoCard>

                            <BentoCard
                                title="RPC, Logging & Metrics"
                                subtitle="gRPC, SCGI endpoints, log levels, and Prometheus [rpc, logging, metrics]".to_string()
                                col_span="col-span-1 md:col-span-1"
                            >
                                <div class="space-y-3.5">
                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"gRPC Port"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="50051"
                                                prop:value=move || daemon_config.get().rpc.grpc_port.map(|p| p.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u16>().ok());
                                                    daemon_config.update(|c| c.rpc.grpc_port = val);
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"SCGI Port"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="5000"
                                                prop:value=move || daemon_config.get().rpc.scgi_port.map(|p| p.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u16>().ok());
                                                    daemon_config.update(|c| c.rpc.scgi_port = val);
                                                }
                                            />
                                        </div>
                                    </div>

                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Tracing Log Level"</label>
                                            <select
                                                class=INPUT_CLASS
                                                prop:value=move || daemon_config.get().logging.level.unwrap_or_else(|| "info".to_string())
                                                on:change=move |ev| {
                                                    if let Some(val) = ev.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()).map(|s| s.value()) {
                                                        daemon_config.update(|c| c.logging.level = Some(val));
                                                    }
                                                }
                                            >
                                                <option value="trace">"trace"</option>
                                                <option value="debug">"debug"</option>
                                                <option value="info">"info"</option>
                                                <option value="warn">"warn"</option>
                                                <option value="error">"error"</option>
                                                <option value="off">"off"</option>
                                            </select>
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Prometheus Metrics Bind"</label>
                                            <input
                                                type="text"
                                                class=INPUT_CLASS
                                                placeholder="127.0.0.1:9090"
                                                prop:value=move || daemon_config.get().metrics.addr.unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).filter(|s| !s.is_empty());
                                                    daemon_config.update(|c| c.metrics.addr = val);
                                                }
                                            />
                                        </div>
                                    </div>

                                    <div>
                                        <label class="block text-xs font-medium text-zinc-300 mb-1">"Log File Destination"</label>
                                        <input
                                            type="text"
                                            class=INPUT_CLASS
                                            placeholder="/var/log/rstorrent.log"
                                            prop:value=move || daemon_config.get().logging.file.unwrap_or_default()
                                            on:input=move |ev| {
                                                let val = input_value(ev).filter(|s| !s.is_empty());
                                                daemon_config.update(|c| c.logging.file = val);
                                            }
                                        />
                                    </div>
                                </div>
                            </BentoCard>

                            <BentoCard
                                title="RSS Automation"
                                subtitle="Syndication feed polling and item thresholds [rss]".to_string()
                                col_span="col-span-1 md:col-span-1"
                            >
                                <div class="space-y-3.5">
                                    <div class="flex items-center gap-3">
                                        <label class="flex items-center gap-2 cursor-pointer select-none">
                                            <input
                                                type="checkbox"
                                                class="rounded border-white/20 bg-zinc-950 text-indigo-600 focus:ring-0 cursor-pointer h-4 w-4"
                                                prop:checked=move || daemon_config.get().rss.enabled.unwrap_or(true)
                                                on:change=move |ev| {
                                                    let checked = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).map(|i| i.checked());
                                                    daemon_config.update(|c| c.rss.enabled = checked);
                                                }
                                            />
                                            <span class="text-xs text-zinc-300 font-medium">"Enable RSS Background Service"</span>
                                        </label>
                                    </div>

                                    <div class="grid grid-cols-2 gap-3">
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Refresh Interval (s)"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="1800"
                                                prop:value=move || daemon_config.get().rss.refresh_interval_secs.map(|v| v.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<u64>().ok());
                                                    daemon_config.update(|c| c.rss.refresh_interval_secs = val);
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-xs font-medium text-zinc-300 mb-1">"Max Items Per Feed"</label>
                                            <input
                                                type="number"
                                                class=INPUT_CLASS
                                                placeholder="200"
                                                prop:value=move || daemon_config.get().rss.max_items_per_feed.map(|v| v.to_string()).unwrap_or_default()
                                                on:input=move |ev| {
                                                    let val = input_value(ev).and_then(|s| s.parse::<usize>().ok());
                                                    daemon_config.update(|c| c.rss.max_items_per_feed = val);
                                                }
                                            />
                                        </div>
                                    </div>
                                </div>
                            </BentoCard>

                            <BentoCard
                                title="Storage Tiering"
                                subtitle="Automatic lifecycle between fast NVMe, SSD, and HDD [tiering]".to_string()
                                col_span="col-span-1 md:col-span-1"
                            >
                                <div class="space-y-3.5">
                                    <div class="flex items-center gap-3">
                                        <label class="flex items-center gap-2 cursor-pointer select-none">
                                            <input
                                                type="checkbox"
                                                class="rounded border-white/20 bg-zinc-950 text-indigo-600 focus:ring-0 cursor-pointer h-4 w-4"
                                                prop:checked=move || daemon_config.get().tiering.enabled.unwrap_or(false)
                                                on:change=move |ev| {
                                                    let checked = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).map(|i| i.checked());
                                                    daemon_config.update(|c| c.tiering.enabled = checked);
                                                }
                                            />
                                            <span class="text-xs text-zinc-300 font-medium">"Enable Automated Storage Tiering"</span>
                                        </label>
                                    </div>

                                    <div>
                                        <label class="block text-xs font-medium text-zinc-300 mb-1">"Migration Check Interval (s)"</label>
                                        <input
                                            type="number"
                                            class=INPUT_CLASS
                                            placeholder="60"
                                            prop:value=move || daemon_config.get().tiering.check_interval.map(|v| v.to_string()).unwrap_or_default()
                                            on:input=move |ev| {
                                                let val = input_value(ev).and_then(|s| s.parse::<u64>().ok());
                                                daemon_config.update(|c| c.tiering.check_interval = val);
                                            }
                                        />
                                    </div>
                                </div>
                            </BentoCard>

                            <SeedingLimitsCard />
                            <LabelManager />
                        </div>
                    </div>
                }.into_any(),

                SettingsTab::RawToml => view! {
                    <div class="space-y-4 animate-in fade-in duration-150">
                        <div class="flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-white/[0.08] bg-zinc-900/60 p-4">
                            <div>
                                <h3 class="text-sm font-semibold text-zinc-100">"Direct rtorrent.toml Source"</h3>
                                <p class="text-xs text-zinc-500">"Inspect or paste raw TOML configuration with comments and annotations."</p>
                            </div>
                            <div class="flex items-center gap-2">
                                <Button
                                    label="Reload"
                                    variant=ButtonVariant::Ghost
                                    disabled=loading_config.get()
                                    on_click=Callback::new(move |_| load_server_config())
                                />
                                <Button
                                    label="Download TOML"
                                    variant=ButtonVariant::Secondary
                                    on_click=Callback::new(move |_| download_toml())
                                />
                                <Button
                                    label="Apply & Save TOML"
                                    variant=ButtonVariant::Primary
                                    disabled=saving_config.get()
                                    on_click=Callback::new(move |_| save_raw_toml(true))
                                />
                            </div>
                        </div>

                        <div class="rounded-2xl border border-white/[0.08] bg-zinc-950 p-4 shadow-xl">
                            <textarea
                                rows="24"
                                class="w-full bg-transparent font-mono text-xs text-zinc-200 focus:outline-none resize-y leading-relaxed"
                                placeholder="# rtorrent.toml content..."
                                prop:value=move || raw_toml.get()
                                on:input=move |ev| {
                                    if let Some(target) = ev.target().and_then(|t| t.dyn_into::<HtmlTextAreaElement>().ok()) {
                                        raw_toml.set(target.value());
                                    }
                                }
                            />
                        </div>
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

// ---------------------------------------------------------------------------
// T10: "Seeding limits" card — global [seed] defaults via /api/seed (T8
// client, state/rules.rs). The DaemonConfig toolbar cannot carry [seed]
// (DaemonConfig has no seed section), so the card owns Apply / Save buttons
// using the RSS settings {config, save_file} pattern.
// ---------------------------------------------------------------------------

// Mirror ButtonVariant::Secondary / Primary; the apply/save buttons need QA
// ids, which the shared Button component does not expose (feeds_panel.rs
// precedent).
const SECONDARY_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-white/[0.04] hover:bg-white/[0.08] border border-white/[0.08] px-3.5 py-2 text-sm font-medium text-zinc-200 active:scale-[0.98] transition-all cursor-pointer";

const PRIMARY_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-medium text-white shadow-[inset_0_1px_0_0_rgba(255,255,255,0.2)] hover:brightness-110 active:scale-[0.98] transition-all cursor-pointer";

/// Form state of the "Seeding limits" card. Numeric fields stay as strings so
/// partially typed values survive keystrokes; they are parsed on Apply/Save
/// (rss/rules_panel.rs RuleDraft pattern). The `*_unlimited` toggles map to
/// the TOML sentinel `0` (plan §6.5).
#[derive(Clone)]
struct SeedDraft {
    enabled: bool,
    ratio_unlimited: bool,
    ratio: String,
    time_unlimited: bool,
    time_value: String,
    time_unit: TimeUnit,
    idle_unlimited: bool,
    idle_value: String,
    idle_unit: TimeUnit,
    mode: LimitMode,
    action_kind: ActionKind,
    action_arg: String,
    check_interval: String,
}

impl SeedDraft {
    fn from_settings(settings: &SeedSettings) -> Self {
        let (time_value, time_unit) = split_duration(settings.seeding_time_secs);
        let (idle_value, idle_unit) = split_duration(settings.inactive_secs);
        let (action_kind, action_arg) = ActionKind::from_action(&settings.action);
        Self {
            enabled: settings.enabled,
            ratio_unlimited: settings.ratio == 0.0,
            // Keep a sensible number ready for when "unlimited" is toggled off.
            ratio: if settings.ratio == 0.0 {
                "1.0".to_string()
            } else {
                settings.ratio.to_string()
            },
            time_unlimited: settings.seeding_time_secs == 0,
            time_value,
            time_unit,
            idle_unlimited: settings.inactive_secs == 0,
            idle_value,
            idle_unit,
            mode: settings.mode,
            action_kind,
            action_arg,
            check_interval: settings.check_interval_secs.to_string(),
        }
    }
}

fn draft_to_seed(draft: &SeedDraft) -> Result<SeedSettings, String> {
    let ratio = if draft.ratio_unlimited {
        0.0
    } else {
        parse_ratio_value(&draft.ratio)?
    };
    let seeding_time_secs = if draft.time_unlimited {
        0
    } else {
        parse_duration_value(&draft.time_value, draft.time_unit)?
    };
    let inactive_secs = if draft.idle_unlimited {
        0
    } else {
        parse_duration_value(&draft.idle_value, draft.idle_unit)?
    };
    let action = build_action(draft.action_kind, &draft.action_arg)?;
    let check_interval_secs = draft
        .check_interval
        .trim()
        .parse::<u64>()
        .map_err(|_| "Check interval must be a whole number of seconds.".to_string())?;
    if check_interval_secs == 0 {
        return Err("Check interval must be at least 1 second.".to_string());
    }
    Ok(SeedSettings {
        enabled: draft.enabled,
        ratio,
        seeding_time_secs,
        inactive_secs,
        mode: draft.mode,
        action,
        check_interval_secs,
    })
}

/// Apply one edit and mark the draft dirty, so store reloads never clobber
/// unsaved input.
fn edit_seed(
    draft: RwSignal<SeedDraft>,
    dirty: RwSignal<bool>,
    update: impl FnOnce(&mut SeedDraft),
) {
    draft.update(update);
    dirty.set(true);
}

/// Global seed policy defaults ([seed] TOML section): enabled switch, ratio /
/// seeding time / inactive limits with unlimited toggles and human time units,
/// mode, action select (extra input for throttle/command, red warning for the
/// destructive remove* actions) and check interval. Apply keeps changes
/// in-memory; Save also persists the TOML (`save_file: true`). The [rules]
/// flags are preserved untouched on PUT.
#[component]
fn SeedingLimitsCard() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    // The settings page does not provide the /rules store; own a local one.
    let store = RulesPageStore::new();
    provide_context(store);

    let request_gen = RwSignal::new(0u64);
    let draft = RwSignal::new(SeedDraft::from_settings(
        &store.seed_settings.get_untracked().seed,
    ));
    let dirty = RwSignal::new(false);
    let saving = RwSignal::new(false);

    Effect::new(move |_| {
        let _reload = store.reload.get();
        // Invalidate in-flight responses on every run, including going offline.
        request_gen.update(|gen| *gen += 1);
        if state.connection.get() != ConnectionMode::Live {
            return;
        }
        let gen = request_gen.get_untracked();
        spawn_local(async move {
            store.loading.set(true);
            match seed_get_settings(&state).await {
                Ok(settings) => {
                    if request_gen.get_untracked() == gen {
                        store.seed_settings.set(settings);
                    }
                }
                Err(err) => {
                    if request_gen.get_untracked() == gen {
                        state.push_toast(
                            ToastKind::Error,
                            "Could not load seeding limits",
                            err.to_string(),
                        );
                    }
                }
            }
            if request_gen.get_untracked() == gen {
                store.loading.set(false);
            }
        });
    });

    // Keep the draft in sync with the store until the user edits; a successful
    // save resets `dirty`, so the server response becomes the source of truth.
    Effect::new(move |_| {
        let settings = store.seed_settings.get();
        if !dirty.get() {
            draft.set(SeedDraft::from_settings(&settings.seed));
        }
    });

    let save = move |save_file: bool| {
        if saving.get() {
            return;
        }
        let seed = match draft_to_seed(&draft.get()) {
            Ok(seed) => seed,
            Err(message) => {
                state.push_toast(ToastKind::Error, "Invalid seeding limits", message);
                return;
            }
        };
        // Preserve the [rules] flags; this card only edits the [seed] section.
        let mut config = store.seed_settings.get_untracked();
        config.seed = seed;
        saving.set(true);
        spawn_local(async move {
            if state.connection.get() != ConnectionMode::Live {
                state.push_toast(
                    ToastKind::Warning,
                    "Backend offline",
                    "Seeding limits require a live connection.",
                );
                saving.set(false);
                return;
            }
            match seed_save_settings(&state, &config, save_file).await {
                Ok(saved) => {
                    store.seed_settings.set(saved);
                    dirty.set(false);
                    state.push_toast(
                        ToastKind::Success,
                        "Seeding limits updated",
                        if save_file {
                            "Saved to rstorrent.toml and applied to runtime."
                        } else {
                            "Applied to runtime (in-memory)."
                        },
                    );
                }
                Err(err) => {
                    state.push_toast(
                        ToastKind::Error,
                        "Could not save seeding limits",
                        err.to_string(),
                    );
                }
            }
            saving.set(false);
        });
    };

    view! {
        <BentoCard
            title="Seeding limits"
            subtitle="Global seed policy defaults [seed] — labels and per-torrent overrides inherit from here.".to_string()
            col_span="col-span-1 md:col-span-2"
            actions=view! {
                {move || {
                    let enabled = store.seed_settings.get().seed.enabled;
                    view! {
                        <Badge
                            label={if enabled { "Enabled" } else { "Disabled" }}
                            kind={if enabled { BadgeKind::Success } else { BadgeKind::Neutral }}
                        />
                    }
                }}
                {move || {
                    let busy = store.loading.get();
                    view! {
                        <Button
                            label="Reload"
                            variant=ButtonVariant::Ghost
                            disabled=busy
                            on_click=Callback::new(move |_| store.reload.update(|n| *n += 1))
                        />
                    }
                }}
            }.into_any()
        >
            <Show
                when=move || state.connection.get() == ConnectionMode::Live
                fallback=move || {
                    view! {
                        <div class="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-white/[0.12] bg-white/[0.02] px-6 py-12 text-center">
                            <p class="text-sm font-medium text-zinc-300">
                                "Backend offline — seeding limits require a live connection"
                            </p>
                        </div>
                    }
                }
            >
                <div class="space-y-3.5">
                    <div class="flex items-center justify-between gap-4">
                        <div>
                            <div class="text-sm font-medium text-zinc-200">"Enable seed policies"</div>
                            <div class="mt-0.5 text-xs text-zinc-500">
                                "Master switch [seed] enabled — when off, legacy ratio.* settings stay in charge."
                            </div>
                        </div>
                        <button
                            id="seed-enabled"
                            type="button"
                            role="switch"
                            aria-checked=move || if draft.get().enabled { "true" } else { "false" }
                            class=move || format!(
                                "relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 cursor-pointer {}",
                                if draft.get().enabled { "bg-[var(--accent)]" } else { "bg-white/[0.15]" }
                            )
                            on:click=move |_| edit_seed(draft, dirty, |d| d.enabled = !d.enabled)
                        >
                            <span class=move || format!(
                                "pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform duration-200 {}",
                                if draft.get().enabled { "translate-x-[22px]" } else { "translate-x-0.5" }
                            ) />
                        </button>
                    </div>

                    <div class="grid grid-cols-1 gap-3.5 md:grid-cols-2">
                        <div>
                            <label for="seed-ratio" class="block text-xs font-medium text-zinc-300 mb-1">"Ratio limit"</label>
                            <div class="flex flex-wrap items-center gap-3">
                                <div class="min-w-[8rem] flex-1">
                                    <input
                                        id="seed-ratio"
                                        type="number"
                                        min="0"
                                        step="0.1"
                                        class=INPUT_CLASS
                                        disabled=move || draft.get().ratio_unlimited
                                        prop:value=move || draft.get().ratio.clone()
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            edit_seed(draft, dirty, move |d| d.ratio = value);
                                        }
                                    />
                                </div>
                                <label class="flex shrink-0 items-center gap-2 cursor-pointer select-none">
                                    <input
                                        id="seed-ratio-unlimited"
                                        type="checkbox"
                                        class="rounded border-white/20 bg-zinc-950 text-indigo-600 focus:ring-0 cursor-pointer h-4 w-4"
                                        prop:checked=move || draft.get().ratio_unlimited
                                        on:change=move |ev| {
                                            let checked = event_target_checked(&ev);
                                            edit_seed(draft, dirty, move |d| d.ratio_unlimited = checked);
                                        }
                                    />
                                    <span class="text-xs text-zinc-300 font-medium">"Unlimited"</span>
                                </label>
                            </div>
                            <p class="mt-1.5 text-xs text-zinc-500">"1.0 = 100%; unlimited (0) disables the ratio limit."</p>
                        </div>

                        <div>
                            <label for="seed-time-value" class="block text-xs font-medium text-zinc-300 mb-1">"Seeding time limit"</label>
                            <div class="flex flex-wrap items-center gap-3">
                                <div class="min-w-[6rem] flex-1">
                                    <input
                                        id="seed-time-value"
                                        type="number"
                                        min="0"
                                        step="any"
                                        class=INPUT_CLASS
                                        disabled=move || draft.get().time_unlimited
                                        prop:value=move || draft.get().time_value.clone()
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            edit_seed(draft, dirty, move |d| d.time_value = value);
                                        }
                                    />
                                </div>
                                <div class="w-32 shrink-0">
                                    <select
                                        id="seed-time-unit"
                                        class=INPUT_CLASS
                                        disabled=move || draft.get().time_unlimited
                                        prop:value=move || draft.get().time_unit.as_str()
                                        on:change=move |ev| {
                                            let unit = TimeUnit::from_value(&event_target_value(&ev));
                                            edit_seed(draft, dirty, move |d| d.time_unit = unit);
                                        }
                                    >
                                        <option value="seconds">"seconds"</option>
                                        <option value="minutes">"minutes"</option>
                                        <option value="hours">"hours"</option>
                                        <option value="days">"days"</option>
                                    </select>
                                </div>
                                <label class="flex shrink-0 items-center gap-2 cursor-pointer select-none">
                                    <input
                                        id="seed-time-unlimited"
                                        type="checkbox"
                                        class="rounded border-white/20 bg-zinc-950 text-indigo-600 focus:ring-0 cursor-pointer h-4 w-4"
                                        prop:checked=move || draft.get().time_unlimited
                                        on:change=move |ev| {
                                            let checked = event_target_checked(&ev);
                                            edit_seed(draft, dirty, move |d| d.time_unlimited = checked);
                                        }
                                    />
                                    <span class="text-xs text-zinc-300 font-medium">"Unlimited"</span>
                                </label>
                            </div>
                            <p class="mt-1.5 text-xs text-zinc-500">"Accumulated active seeding time; unlimited (0) disables the limit."</p>
                        </div>

                        <div>
                            <label for="seed-idle-value" class="block text-xs font-medium text-zinc-300 mb-1">"Inactive limit"</label>
                            <div class="flex flex-wrap items-center gap-3">
                                <div class="min-w-[6rem] flex-1">
                                    <input
                                        id="seed-idle-value"
                                        type="number"
                                        min="0"
                                        step="any"
                                        class=INPUT_CLASS
                                        disabled=move || draft.get().idle_unlimited
                                        prop:value=move || draft.get().idle_value.clone()
                                        on:input=move |ev| {
                                            let value = event_target_value(&ev);
                                            edit_seed(draft, dirty, move |d| d.idle_value = value);
                                        }
                                    />
                                </div>
                                <div class="w-32 shrink-0">
                                    <select
                                        id="seed-idle-unit"
                                        class=INPUT_CLASS
                                        disabled=move || draft.get().idle_unlimited
                                        prop:value=move || draft.get().idle_unit.as_str()
                                        on:change=move |ev| {
                                            let unit = TimeUnit::from_value(&event_target_value(&ev));
                                            edit_seed(draft, dirty, move |d| d.idle_unit = unit);
                                        }
                                    >
                                        <option value="seconds">"seconds"</option>
                                        <option value="minutes">"minutes"</option>
                                        <option value="hours">"hours"</option>
                                        <option value="days">"days"</option>
                                    </select>
                                </div>
                                <label class="flex shrink-0 items-center gap-2 cursor-pointer select-none">
                                    <input
                                        id="seed-idle-unlimited"
                                        type="checkbox"
                                        class="rounded border-white/20 bg-zinc-950 text-indigo-600 focus:ring-0 cursor-pointer h-4 w-4"
                                        prop:checked=move || draft.get().idle_unlimited
                                        on:change=move |ev| {
                                            let checked = event_target_checked(&ev);
                                            edit_seed(draft, dirty, move |d| d.idle_unlimited = checked);
                                        }
                                    />
                                    <span class="text-xs text-zinc-300 font-medium">"Unlimited"</span>
                                </label>
                            </div>
                            <p class="mt-1.5 text-xs text-zinc-500">"Time without upload while seeding; unlimited (0) disables the limit."</p>
                        </div>

                        <div>
                            <label for="seed-mode" class="block text-xs font-medium text-zinc-300 mb-1">"Limit mode"</label>
                            <select
                                id="seed-mode"
                                class=INPUT_CLASS
                                prop:value=move || match draft.get().mode {
                                    LimitMode::Any => "any",
                                    LimitMode::All => "all",
                                }
                                on:change=move |ev| {
                                    let mode = if event_target_value(&ev) == "all" {
                                        LimitMode::All
                                    } else {
                                        LimitMode::Any
                                    };
                                    edit_seed(draft, dirty, move |d| d.mode = mode);
                                }
                            >
                                <option value="any">"Any — first reached limit fires"</option>
                                <option value="all">"All — every active limit must be reached"</option>
                            </select>
                            <p class="mt-1.5 text-xs text-zinc-500">"How the active limits combine."</p>
                        </div>

                        <div class="md:col-span-2">
                            <label for="seed-action" class="block text-xs font-medium text-zinc-300 mb-1">"Action when a limit is reached"</label>
                            <div class="grid grid-cols-1 items-end gap-3 sm:grid-cols-2">
                                <select
                                    id="seed-action"
                                    class=INPUT_CLASS
                                    prop:value=move || draft.get().action_kind.as_str()
                                    on:change=move |ev| {
                                        let kind = ActionKind::from_value(&event_target_value(&ev));
                                        edit_seed(draft, dirty, move |d| d.action_kind = kind);
                                    }
                                >
                                    <option value="stop">"Stop (d.stop)"</option>
                                    <option value="close">"Close (legacy ratio semantics)"</option>
                                    <option value="remove">"Remove (keep data)"</option>
                                    <option value="remove_with_data">"Remove with data (destructive)"</option>
                                    <option value="throttle">"Throttle…"</option>
                                    <option value="super_seed">"Super-seed (initial seeding)"</option>
                                    <option value="notify">"Notify only"</option>
                                    <option value="command">"Custom command…"</option>
                                </select>
                                {move || {
                                    draft.get().action_kind.needs_arg().then(|| {
                                        let (label_text, placeholder) =
                                            if draft.get().action_kind == ActionKind::Throttle {
                                                ("Throttle name", "e.g. seed")
                                            } else {
                                                ("d.* command", "e.g. d.stop=")
                                            };
                                        view! {
                                            <div>
                                                <label for="seed-action-arg" class="block text-xs font-medium text-zinc-300 mb-1">{label_text}</label>
                                                <input
                                                    id="seed-action-arg"
                                                    type="text"
                                                    class=INPUT_CLASS
                                                    placeholder=placeholder
                                                    autocomplete="off"
                                                    prop:value=move || draft.get().action_arg.clone()
                                                    on:input=move |ev| {
                                                        let value = event_target_value(&ev);
                                                        edit_seed(draft, dirty, move |d| d.action_arg = value);
                                                    }
                                                />
                                            </div>
                                        }
                                    })
                                }}
                            </div>
                            {move || {
                                destructive_warning(draft.get().action_kind).map(|warning| {
                                    view! {
                                        <div class="mt-3 rounded-xl border border-rose-500/30 bg-rose-500/10 px-3.5 py-2.5 text-xs text-rose-300">
                                            {warning}
                                        </div>
                                    }
                                })
                            }}
                            {move || {
                                let kind = draft.get().action_kind;
                                let gate_off = !store.seed_settings.get().rules.allow_destructive;
                                (kind.is_destructive() && gate_off).then(|| {
                                    view! {
                                        <p class="mt-2 text-xs text-amber-400">
                                            "The global destructive gate ([rules] allow_destructive) is currently off — remove actions are skipped until it is enabled on the Rules page."
                                        </p>
                                    }
                                })
                            }}
                        </div>

                        <div>
                            <label for="seed-check-interval" class="block text-xs font-medium text-zinc-300 mb-1">"Check interval (s)"</label>
                            <input
                                id="seed-check-interval"
                                type="number"
                                min="1"
                                step="1"
                                class=INPUT_CLASS
                                placeholder="10"
                                prop:value=move || draft.get().check_interval.clone()
                                on:input=move |ev| {
                                    let value = event_target_value(&ev);
                                    edit_seed(draft, dirty, move |d| d.check_interval = value);
                                }
                            />
                            <p class="mt-1.5 text-xs text-zinc-500">"Seconds between seed policy evaluation passes."</p>
                        </div>
                    </div>

                    <div class="flex flex-wrap items-center gap-3 pt-1">
                        {move || {
                            let busy = saving.get();
                            view! {
                                <button
                                    id="seed-apply"
                                    type="button"
                                    disabled=busy
                                    class=if busy {
                                        format!("{SECONDARY_BUTTON_CLASS} opacity-50 pointer-events-none")
                                    } else {
                                        SECONDARY_BUTTON_CLASS.to_string()
                                    }
                                    on:click=move |_| save(false)
                                >
                                    {if busy { "Applying…" } else { "Apply (Runtime)" }}
                                </button>
                            }
                        }}
                        {move || {
                            let busy = saving.get();
                            view! {
                                <button
                                    id="seed-save"
                                    type="button"
                                    disabled=busy
                                    class=if busy {
                                        format!("{PRIMARY_BUTTON_CLASS} opacity-50 pointer-events-none")
                                    } else {
                                        PRIMARY_BUTTON_CLASS.to_string()
                                    }
                                    on:click=move |_| save(true)
                                >
                                    {if busy { "Saving…" } else { "Save to TOML" }}
                                </button>
                            }
                        }}
                        <span class="text-xs text-zinc-500">
                            "Apply keeps changes in memory; Save also persists the [seed] section to rstorrent.toml."
                        </span>
                    </div>
                </div>
            </Show>
        </BentoCard>
    }
}
