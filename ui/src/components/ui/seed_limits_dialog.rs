//! Per-torrent seed limits dialog (plan.md §6.9, task T11).
//!
//! Shows the resolved [`EffectiveLimits`] with per-field source badges
//! (`global` / `label:<name>` / `torrent`), progress bars toward each limit
//! (ratio / seeding time / idle) and override controls
//! (inherit / unlimited / set + value, plus the seed action). Saving PUTs
//! `/api/torrents/{hash}/seed-limits` through the T8 client wrapped by
//! `state::api::save_torrent_seed_limits` and refreshes the displayed values.
//!
//! Open state lives in `AppState.seed_limits_hash` so any page (dashboard,
//! explorer, …) can open the dialog; each page mounts one `<SeedLimitsDialog/>`.
//!
//! The GET endpoint returns *effective* limits, not the raw overrides, so the
//! form is initialized by reconstructing them from `sources`: the resolver
//! (`rt-rules/src/resolve.rs`) attributes a field to `"torrent"` exactly when
//! the torrent override is explicit (`Set(v)` → `Some(v)`, `Unlimited` →
//! `None`); any other source means the override is `Inherit`. The same logic
//! applies to the action (`None` = inherit).

use leptos::task::spawn_local;
use leptos::{ev, prelude::*};

use crate::components::ui::{Badge, BadgeKind, Button, ButtonVariant};
use crate::state::rules::{EffectiveLimits, LimitMode, LimitValue, SeedAction, TorrentSeedLimits};
use crate::state::{api, format_duration, AppState, TorrentItem};

const SELECT_CLASS: &str = "cursor-pointer rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 text-xs text-zinc-200 transition-all focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20";
const INPUT_CLASS: &str = "w-36 rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 font-mono text-xs text-zinc-100 placeholder:text-zinc-600 transition-all focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20";

/// The three override choices of a [`LimitValue`] field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverrideMode {
    Inherit,
    Unlimited,
    Set,
}

impl OverrideMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Unlimited => "unlimited",
            Self::Set => "set",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "unlimited" => Self::Unlimited,
            "set" => Self::Set,
            _ => Self::Inherit,
        }
    }
}

/// Selectable seed actions; `Inherit` maps to `None` (defer upward), the
/// argument-carrying variants (`Throttle`/`Command`) use a shared text input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionChoice {
    Inherit,
    Stop,
    Close,
    Remove,
    RemoveWithData,
    Throttle,
    SuperSeed,
    Notify,
    Command,
}

impl ActionChoice {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Stop => "stop",
            Self::Close => "close",
            Self::Remove => "remove",
            Self::RemoveWithData => "remove_with_data",
            Self::Throttle => "throttle",
            Self::SuperSeed => "super_seed",
            Self::Notify => "notify",
            Self::Command => "command",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "stop" => Self::Stop,
            "close" => Self::Close,
            "remove" => Self::Remove,
            "remove_with_data" => Self::RemoveWithData,
            "throttle" => Self::Throttle,
            "super_seed" => Self::SuperSeed,
            "notify" => Self::Notify,
            "command" => Self::Command,
            _ => Self::Inherit,
        }
    }
}

/// Editable form state; all signals are `Copy`, so the struct can be captured
/// by the load/save closures and the view alike.
#[derive(Clone, Copy)]
struct LimitForm {
    ratio_mode: RwSignal<OverrideMode>,
    ratio_value: RwSignal<String>,
    time_mode: RwSignal<OverrideMode>,
    time_value: RwSignal<String>,
    idle_mode: RwSignal<OverrideMode>,
    idle_value: RwSignal<String>,
    action: RwSignal<ActionChoice>,
    action_arg: RwSignal<String>,
    error: RwSignal<Option<String>>,
}

impl LimitForm {
    fn new() -> Self {
        Self {
            ratio_mode: RwSignal::new(OverrideMode::Inherit),
            ratio_value: RwSignal::new(String::new()),
            time_mode: RwSignal::new(OverrideMode::Inherit),
            time_value: RwSignal::new(String::new()),
            idle_mode: RwSignal::new(OverrideMode::Inherit),
            idle_value: RwSignal::new(String::new()),
            action: RwSignal::new(ActionChoice::Inherit),
            action_arg: RwSignal::new(String::new()),
            error: RwSignal::new(None),
        }
    }

    /// Initialize the controls from the fetched effective limits (see the
    /// module docs for the reconstruction rationale). Value inputs are
    /// prefilled with the effective value even when inheriting, so switching
    /// to "Set" starts from a sane number.
    fn load_from(self, limits: &EffectiveLimits) {
        let overrides = overrides_from_effective(limits);
        self.ratio_mode.set(mode_of(&overrides.ratio));
        self.ratio_value.set(match &overrides.ratio {
            LimitValue::Set(v) => format!("{v:.2}"),
            _ => limits
                .ratio
                .map(|v| format!("{v:.2}"))
                .unwrap_or_else(|| "2.0".to_string()),
        });
        self.time_mode.set(mode_of(&overrides.seeding_time_secs));
        self.time_value.set(match &overrides.seeding_time_secs {
            LimitValue::Set(v) => format_duration(*v),
            _ => limits
                .seeding_time_secs
                .map(format_duration)
                .unwrap_or_else(|| "7d".to_string()),
        });
        self.idle_mode.set(mode_of(&overrides.inactive_secs));
        self.idle_value.set(match &overrides.inactive_secs {
            LimitValue::Set(v) => format_duration(*v),
            _ => limits
                .inactive_secs
                .map(format_duration)
                .unwrap_or_else(|| "30m".to_string()),
        });
        let (choice, arg) = match overrides.action {
            Some(SeedAction::Throttle(name)) => (ActionChoice::Throttle, name),
            Some(SeedAction::Command(cmd)) => (ActionChoice::Command, cmd),
            Some(SeedAction::Stop) => (ActionChoice::Stop, String::new()),
            Some(SeedAction::Close) => (ActionChoice::Close, String::new()),
            Some(SeedAction::Remove) => (ActionChoice::Remove, String::new()),
            Some(SeedAction::RemoveWithData) => (ActionChoice::RemoveWithData, String::new()),
            Some(SeedAction::SuperSeed) => (ActionChoice::SuperSeed, String::new()),
            Some(SeedAction::Notify) => (ActionChoice::Notify, String::new()),
            None => (ActionChoice::Inherit, String::new()),
        };
        // Prefill the argument from the resolved action when inheriting.
        let arg = if arg.is_empty() {
            match &limits.action {
                SeedAction::Throttle(name) => name.clone(),
                SeedAction::Command(cmd) => cmd.clone(),
                _ => String::new(),
            }
        } else {
            arg
        };
        self.action.set(choice);
        self.action_arg.set(arg);
        self.error.set(None);
    }

    /// Validate the controls and build the PUT body.
    fn build(self) -> Result<TorrentSeedLimits, String> {
        let ratio = match self.ratio_mode.get() {
            OverrideMode::Inherit => LimitValue::Inherit,
            OverrideMode::Unlimited => LimitValue::Unlimited,
            OverrideMode::Set => {
                let value: f64 = self
                    .ratio_value
                    .get()
                    .trim()
                    .parse()
                    .map_err(|_| "Ratio must be a number, e.g. 2.0.".to_string())?;
                if !value.is_finite() || value < 0.0 {
                    return Err("Ratio must be a non-negative number.".to_string());
                }
                LimitValue::Set(value)
            }
        };
        let seeding_time_secs = build_time(self.time_mode.get(), &self.time_value.get())
            .map_err(|err| format!("Seeding time: {err}"))?;
        let inactive_secs = build_time(self.idle_mode.get(), &self.idle_value.get())
            .map_err(|err| format!("Idle time: {err}"))?;
        let action = match self.action.get() {
            ActionChoice::Inherit => None,
            ActionChoice::Stop => Some(SeedAction::Stop),
            ActionChoice::Close => Some(SeedAction::Close),
            ActionChoice::Remove => Some(SeedAction::Remove),
            ActionChoice::RemoveWithData => Some(SeedAction::RemoveWithData),
            ActionChoice::SuperSeed => Some(SeedAction::SuperSeed),
            ActionChoice::Notify => Some(SeedAction::Notify),
            ActionChoice::Throttle => {
                let name = self.action_arg.get().trim().to_string();
                if name.is_empty() {
                    return Err("Enter a throttle name for the throttle action.".to_string());
                }
                Some(SeedAction::Throttle(name))
            }
            ActionChoice::Command => {
                let cmd = self.action_arg.get().trim().to_string();
                if cmd.is_empty() {
                    return Err("Enter a d.* command for the command action.".to_string());
                }
                Some(SeedAction::Command(cmd))
            }
        };
        Ok(TorrentSeedLimits {
            ratio,
            seeding_time_secs,
            inactive_secs,
            action,
        })
    }
}

fn mode_of<T>(value: &LimitValue<T>) -> OverrideMode {
    match value {
        LimitValue::Inherit => OverrideMode::Inherit,
        LimitValue::Unlimited => OverrideMode::Unlimited,
        LimitValue::Set(_) => OverrideMode::Set,
    }
}

fn build_time(mode: OverrideMode, text: &str) -> Result<LimitValue<u64>, String> {
    match mode {
        OverrideMode::Inherit => Ok(LimitValue::Inherit),
        OverrideMode::Unlimited => Ok(LimitValue::Unlimited),
        OverrideMode::Set => parse_duration(text).map(LimitValue::Set),
    }
}

/// Reconstruct the per-torrent overrides from effective limits + sources.
/// Exact per `rt-rules/src/resolve.rs`: source `"torrent"` ⟺ explicit
/// override (`Set` when the effective value is `Some`, `Unlimited` when it is
/// `None`); any other source means `Inherit`.
fn overrides_from_effective(limits: &EffectiveLimits) -> TorrentSeedLimits {
    fn field<T>(source: &str, value: Option<T>) -> LimitValue<T> {
        if source == "torrent" {
            match value {
                Some(value) => LimitValue::Set(value),
                None => LimitValue::Unlimited,
            }
        } else {
            LimitValue::Inherit
        }
    }
    TorrentSeedLimits {
        ratio: field(&limits.sources.ratio, limits.ratio),
        seeding_time_secs: field(&limits.sources.seeding_time_secs, limits.seeding_time_secs),
        inactive_secs: field(&limits.sources.inactive_secs, limits.inactive_secs),
        action: if limits.sources.action == "torrent" {
            Some(limits.action.clone())
        } else {
            None
        },
    }
}

/// Parse a human duration into seconds: `7d`, `2h 30m`, `2d4h`, `35m 12s`,
/// or a bare number (seconds). Units: d/h/m/s; whitespace is optional.
fn parse_duration(text: &str) -> Result<u64, String> {
    let mut total = 0u64;
    let mut current = 0u64;
    let mut have_digits = false;
    let mut have_token = false;
    let too_large = || "Duration is too large.".to_string();
    for ch in text.trim().chars() {
        match ch {
            '0'..='9' => {
                current = current
                    .checked_mul(10)
                    .and_then(|v| v.checked_add(u64::from(ch as u8 - b'0')))
                    .ok_or_else(too_large)?;
                have_digits = true;
            }
            'd' | 'h' | 'm' | 's' => {
                if !have_digits {
                    return Err(format!("Missing number before '{ch}' in duration."));
                }
                let mult = match ch {
                    'd' => 86_400,
                    'h' => 3_600,
                    'm' => 60,
                    _ => 1,
                };
                total = total
                    .checked_add(current.checked_mul(mult).ok_or_else(too_large)?)
                    .ok_or_else(too_large)?;
                current = 0;
                have_digits = false;
                have_token = true;
            }
            c if c.is_whitespace() => {
                if have_digits {
                    // A bare number means seconds.
                    total = total.checked_add(current).ok_or_else(too_large)?;
                    current = 0;
                    have_digits = false;
                    have_token = true;
                }
            }
            _ => {
                return Err(format!(
                    "Invalid character '{ch}' in duration (use e.g. 7d, 2h 30m or 3600)."
                ))
            }
        }
    }
    if have_digits {
        total = total.checked_add(current).ok_or_else(too_large)?;
        have_token = true;
    }
    if !have_token {
        return Err("Enter a duration, e.g. 7d, 2h 30m or 3600.".to_string());
    }
    Ok(total)
}

/// Compact label of a resolved [`SeedAction`] (TOML colon style, plan §6.5).
fn action_label(action: &SeedAction) -> String {
    match action {
        SeedAction::Stop => "stop".to_string(),
        SeedAction::Close => "close".to_string(),
        SeedAction::Remove => "remove".to_string(),
        SeedAction::RemoveWithData => "remove_with_data".to_string(),
        SeedAction::Throttle(name) => format!("throttle:{name}"),
        SeedAction::SuperSeed => "super_seed".to_string(),
        SeedAction::Notify => "notify".to_string(),
        SeedAction::Command(cmd) => format!("command:{cmd}"),
    }
}

/// Badge showing where an effective value came from.
fn source_badge(source: &str) -> AnyView {
    let (label, kind) = if source == "torrent" {
        ("Torrent".to_string(), BadgeKind::Success)
    } else if let Some(name) = source.strip_prefix("label:") {
        (format!("Label: {name}"), BadgeKind::Info)
    } else {
        ("Global".to_string(), BadgeKind::Neutral)
    };
    view! { <Badge label=label kind=kind/> }.into_any()
}

/// Progress bar toward a limit (`None` = unlimited → current value only).
/// `current`/`limit` are normalized to f64; `text` is the human-readable
/// "current / limit" caption. A zero limit renders as a full bar.
fn limit_progress(current: f64, limit: Option<f64>, text: String) -> AnyView {
    match limit {
        Some(limit) => {
            let pct = if limit > 0.0 {
                (current / limit * 100.0).clamp(0.0, 100.0)
            } else {
                100.0
            };
            let bar_class = if current >= limit {
                "h-full rounded-full bg-emerald-400 transition-all duration-300"
            } else {
                "h-full rounded-full bg-[var(--accent)] transition-all duration-300"
            };
            view! {
                <div class="flex items-center gap-2">
                    <div class="h-1.5 flex-1 overflow-hidden rounded-full bg-white/[0.06]">
                        <div class=bar_class style=format!("width: {pct:.1}%")></div>
                    </div>
                    <span class="shrink-0 font-mono text-[10px] tabular-nums text-zinc-500">{text}</span>
                </div>
            }
            .into_any()
        }
        None => view! {
            <p class="font-mono text-[10px] tabular-nums text-zinc-600">{text} " · no limit"</p>
        }
        .into_any(),
    }
}

/// One limit block: effective value + source badge, progress bar and the
/// override controls. `current_of` extracts the live current value from the
/// torrent snapshot; `caption` renders the "current / limit" text; `input`
/// builds the value input on demand (Show children are re-invocable, and
/// `AnyView` is not `Clone`).
#[allow(clippy::too_many_arguments)]
fn limit_section(
    title: &'static str,
    effective_text: String,
    source: String,
    limit: Option<f64>,
    item: Memo<Option<TorrentItem>>,
    current_of: impl Fn(&TorrentItem) -> f64 + Copy + Send + Sync + 'static,
    caption: impl Fn(f64, Option<f64>) -> String + Copy + Send + Sync + 'static,
    mode: RwSignal<OverrideMode>,
    input: impl Fn() -> AnyView + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="rounded-xl border border-white/[0.06] bg-white/[0.02] p-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
                <span class="text-xs font-semibold uppercase tracking-wider text-zinc-400">{title}</span>
                <div class="flex items-center gap-2">
                    <span class="font-mono text-xs tabular-nums text-zinc-200">{effective_text}</span>
                    {source_badge(&source)}
                </div>
            </div>
            <div class="mt-2">
                {move || {
                    let current = item.get().map(|i| current_of(&i)).unwrap_or(0.0);
                    limit_progress(current, limit, caption(current, limit))
                }}
            </div>
            <div class="mt-2.5 flex flex-wrap items-center gap-2">
                <span class="text-[10px] uppercase tracking-wider text-zinc-600">"Override"</span>
                <select
                    class=SELECT_CLASS
                    prop:value=move || mode.get().as_str()
                    on:change=move |ev| mode.set(OverrideMode::parse(&event_target_value(&ev)))
                    aria-label=format!("{title} override")
                >
                    <option value="inherit">"Inherit"</option>
                    <option value="unlimited">"Unlimited"</option>
                    <option value="set">"Set"</option>
                </select>
                <Show when=move || mode.get() == OverrideMode::Set fallback=|| ()>
                    {input()}
                </Show>
            </div>
        </div>
    }
}

#[component]
pub fn SeedLimitsDialog() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let form = LimitForm::new();
    let effective = RwSignal::new(None::<Result<EffectiveLimits, String>>);
    let saving = RwSignal::new(false);
    let load_gen = RwSignal::new(0u64);

    let open = Memo::new(move |_| state.seed_limits_hash.get().is_some());
    // Live torrent snapshot for the progress bars and the header.
    let dialog_item = Memo::new(move |_| {
        let hash = state.seed_limits_hash.get()?;
        state
            .items
            .with(|items| items.iter().find(|i| i.hash == hash).cloned())
    });

    let close = move || state.seed_limits_hash.set(None);

    // Fetch effective limits whenever the dialog opens (or switches torrent);
    // the generation guard drops stale responses (RSS request_gen pattern).
    let load = move |hash: String| {
        form.error.set(None);
        effective.set(None);
        load_gen.update(|gen| *gen += 1);
        let gen = load_gen.get_untracked();
        spawn_local(async move {
            let result = api::fetch_torrent_seed_limits(state, &hash).await;
            if load_gen.get_untracked() != gen {
                return;
            }
            match result {
                Ok(limits) => {
                    form.load_from(&limits);
                    effective.set(Some(Ok(limits)));
                }
                Err(err) => effective.set(Some(Err(err))),
            }
        });
    };
    Effect::new(move |_| {
        if let Some(hash) = state.seed_limits_hash.get() {
            load(hash);
        }
    });

    let on_save = move || {
        if saving.get_untracked() {
            return;
        }
        let Some(hash) = state.seed_limits_hash.get_untracked() else {
            return;
        };
        let overrides = match form.build() {
            Ok(overrides) => overrides,
            Err(message) => {
                form.error.set(Some(message));
                return;
            }
        };
        form.error.set(None);
        saving.set(true);
        spawn_local(async move {
            let result = api::save_torrent_seed_limits(state, &hash, &overrides).await;
            saving.set(false);
            match result {
                // PUT returns the recomputed effective limits: refresh the
                // display (sources included) without closing the dialog.
                Ok(limits) => effective.set(Some(Ok(limits))),
                Err(err) => form.error.set(Some(err)),
            }
        });
    };

    let key_handle = window_event_listener(ev::keydown, move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape"
            && state.seed_limits_hash.get_untracked().is_some()
            && !state.cmd_palette_open.get_untracked()
        {
            ev.prevent_default();
            // Keep the drawer's Escape listener from also firing.
            ev.stop_immediate_propagation();
            state.seed_limits_hash.set(None);
        }
    });
    on_cleanup(move || key_handle.remove());

    // Value-input builders: Show children re-invoke on every toggle, and
    // AnyView is not Clone, so build a fresh view per invocation.
    let ratio_input = move || {
        view! {
            <input
                type="number"
                step="0.01"
                min="0"
                class=INPUT_CLASS
                placeholder="e.g. 2.0"
                aria-label="Ratio override value"
                prop:value=move || form.ratio_value.get()
                on:input=move |ev| form.ratio_value.set(event_target_value(&ev))
            />
        }
        .into_any()
    };
    let time_input = move || {
        view! {
            <input
                type="text"
                class=INPUT_CLASS
                placeholder="7d / 2h 30m / 3600"
                aria-label="Seeding time override value"
                prop:value=move || form.time_value.get()
                on:input=move |ev| form.time_value.set(event_target_value(&ev))
            />
        }
        .into_any()
    };
    let idle_input = move || {
        view! {
            <input
                type="text"
                class=INPUT_CLASS
                placeholder="30m / 2h / 1800"
                aria-label="Idle time override value"
                prop:value=move || form.idle_value.get()
                on:input=move |ev| form.idle_value.set(event_target_value(&ev))
            />
        }
        .into_any()
    };

    view! {
        <Show when=move || open.get() fallback=|| ()>
            <div
                class="fixed inset-0 z-[60] flex items-start justify-center overflow-y-auto px-4 py-[8vh]"
                role="dialog"
                aria-modal="true"
                aria-label="Seed limits"
            >
                <div
                    class="absolute inset-0 bg-black/60 backdrop-blur-sm"
                    on:click=move |_| close()
                ></div>
                <div class="glass-surface relative w-full max-w-2xl rounded-2xl border border-white/15 bg-zinc-900/90 shadow-2xl backdrop-blur-2xl">
                    <div class="flex items-start justify-between gap-3 border-b border-white/[0.08] px-5 py-4">
                        <div class="min-w-0 flex-1">
                            <p class="text-[11px] uppercase tracking-wide text-zinc-500">"Seed limits"</p>
                            <h2 class="mt-1 truncate text-base font-semibold tracking-tight text-zinc-100">
                                {move || {
                                    dialog_item
                                        .get()
                                        .map(|i| i.name)
                                        .unwrap_or_else(|| {
                                            state
                                                .seed_limits_hash
                                                .get()
                                                .map(|h| h.chars().take(8).collect())
                                                .unwrap_or_default()
                                        })
                                }}
                            </h2>
                            <p class="mt-0.5 font-mono text-xs text-zinc-500">
                                {move || {
                                    state
                                        .seed_limits_hash
                                        .get()
                                        .map(|h| h.chars().take(8).collect::<String>())
                                        .unwrap_or_default()
                                }}
                            </p>
                        </div>
                        <button
                            type="button"
                            class="cursor-pointer rounded-lg p-1.5 text-zinc-400 transition-colors hover:bg-white/[0.06] hover:text-zinc-100"
                            on:click=move |_| close()
                            aria-label="Close seed limits dialog"
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                                <path d="M18 6 6 18"/>
                                <path d="m6 6 12 12"/>
                            </svg>
                        </button>
                    </div>
                    <div class="max-h-[70vh] space-y-4 overflow-y-auto px-5 py-4">
                        {move || match effective.get() {
                            None => view! {
                                <p class="py-6 text-center text-xs text-zinc-500">"Loading effective limits…"</p>
                            }
                            .into_any(),
                            Some(Err(err)) => view! {
                                <div class="flex flex-col items-center gap-3 py-6">
                                    <p class="text-center text-xs text-rose-400">{err}</p>
                                    <Button
                                        label="Retry"
                                        variant=ButtonVariant::Secondary
                                        on_click=Callback::new(move |_| {
                                            if let Some(hash) = state.seed_limits_hash.get_untracked() {
                                                load(hash);
                                            }
                                        })
                                    />
                                </div>
                            }
                            .into_any(),
                            Some(Ok(limits)) => {
                                let ratio_limit = limits.ratio;
                                let time_limit = limits.seeding_time_secs;
                                let idle_limit = limits.inactive_secs;
                                let mode_label = match limits.mode {
                                    LimitMode::Any => "any — first limit fires",
                                    LimitMode::All => "all — every limit must fire",
                                };
                                view! {
                                    <div class="space-y-4">
                                        <div class="flex flex-wrap items-center gap-2">
                                            <Badge label=format!("Mode: {mode_label}") kind=BadgeKind::Neutral/>
                                            <Badge label=format!("Action: {}", action_label(&limits.action)) kind=BadgeKind::Info/>
                                            {source_badge(&limits.sources.action)}
                                        </div>
                                        <p class="text-[11px] leading-relaxed text-zinc-600">
                                            "Effective values resolve global → label → torrent. Overrides below apply to this torrent only; mode comes from the label/global policy."
                                        </p>
                                        {limit_section(
                                            "Ratio",
                                            limits.ratio.map(|v| format!("{v:.2}")).unwrap_or_else(|| "Unlimited".to_string()),
                                            limits.sources.ratio.clone(),
                                            ratio_limit,
                                            dialog_item,
                                            |i| i.ratio(),
                                            move |current, limit| match limit {
                                                Some(limit) => format!("{current:.2} / {limit:.2}"),
                                                None => format!("{current:.2}"),
                                            },
                                            form.ratio_mode,
                                            ratio_input,
                                        )}
                                        {limit_section(
                                            "Seeding time",
                                            limits.seeding_time_secs.map(format_duration).unwrap_or_else(|| "Unlimited".to_string()),
                                            limits.sources.seeding_time_secs.clone(),
                                            time_limit.map(|v| v as f64),
                                            dialog_item,
                                            |i| i.seeding_time_secs as f64,
                                            move |current, limit| {
                                                let current = format_duration(current as u64);
                                                match limit {
                                                    Some(limit) => format!("{current} / {}", format_duration(limit as u64)),
                                                    None => current,
                                                }
                                            },
                                            form.time_mode,
                                            time_input,
                                        )}
                                        {limit_section(
                                            "Idle (no upload)",
                                            limits.inactive_secs.map(format_duration).unwrap_or_else(|| "Unlimited".to_string()),
                                            limits.sources.inactive_secs.clone(),
                                            idle_limit.map(|v| v as f64),
                                            dialog_item,
                                            |i| i.idle_secs as f64,
                                            move |current, limit| {
                                                let current = format_duration(current as u64);
                                                match limit {
                                                    Some(limit) => format!("{current} / {}", format_duration(limit as u64)),
                                                    None => current,
                                                }
                                            },
                                            form.idle_mode,
                                            idle_input,
                                        )}
                                        <div class="rounded-xl border border-white/[0.06] bg-white/[0.02] p-3">
                                            <div class="flex flex-wrap items-center gap-2">
                                                <span class="text-xs font-semibold uppercase tracking-wider text-zinc-400">"Action"</span>
                                                <select
                                                    class=SELECT_CLASS
                                                    prop:value=move || form.action.get().as_str()
                                                    on:change=move |ev| form.action.set(ActionChoice::parse(&event_target_value(&ev)))
                                                    aria-label="Seed action override"
                                                >
                                                    <option value="inherit">"Inherit"</option>
                                                    <option value="stop">"Stop"</option>
                                                    <option value="close">"Close"</option>
                                                    <option value="remove">"Remove"</option>
                                                    <option value="remove_with_data">"Remove with data"</option>
                                                    <option value="throttle">"Throttle…"</option>
                                                    <option value="super_seed">"Super-seed"</option>
                                                    <option value="notify">"Notify only"</option>
                                                    <option value="command">"Command…"</option>
                                                </select>
                                                <Show
                                                    when=move || matches!(form.action.get(), ActionChoice::Throttle | ActionChoice::Command)
                                                    fallback=|| ()
                                                >
                                                    <input
                                                        type="text"
                                                        class=INPUT_CLASS
                                                        placeholder=move || {
                                                            if form.action.get() == ActionChoice::Throttle {
                                                                "throttle name"
                                                            } else {
                                                                "d.* command"
                                                            }
                                                        }
                                                        aria-label="Seed action argument"
                                                        prop:value=move || form.action_arg.get()
                                                        on:input=move |ev| form.action_arg.set(event_target_value(&ev))
                                                    />
                                                </Show>
                                            </div>
                                            {move || {
                                                matches!(form.action.get(), ActionChoice::Remove | ActionChoice::RemoveWithData)
                                                    .then(|| {
                                                        view! {
                                                            <p class="mt-2 text-xs text-rose-400">
                                                                "Destructive action — it only runs when the destructive gate (global and rule) allows it."
                                                            </p>
                                                        }
                                                    })
                                            }}
                                        </div>
                                    </div>
                                }
                                .into_any()
                            }
                        }}
                    </div>
                    <div class="flex items-center justify-between gap-3 border-t border-white/[0.08] px-5 py-4">
                        <div class="min-w-0 flex-1">
                            {move || {
                                form.error
                                    .get()
                                    .map(|err| view! { <p class="text-xs text-rose-400">{err}</p> })
                            }}
                        </div>
                        <div class="flex shrink-0 items-center gap-2">
                            <Button
                                label="Cancel"
                                variant=ButtonVariant::Ghost
                                on_click=Callback::new(move |_| close())
                            />
                            {move || {
                                let busy = saving.get();
                                let label = if busy { "Saving…" } else { "Save" };
                                view! {
                                    <Button
                                        label=label
                                        variant=ButtonVariant::Primary
                                        disabled=busy
                                        on_click=Callback::new(move |_| on_save())
                                    />
                                }
                            }}
                        </div>
                    </div>
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::rules::LimitSources;

    #[test]
    fn parse_duration_units_and_bare_seconds() {
        assert_eq!(parse_duration("7d").unwrap(), 7 * 86_400);
        assert_eq!(parse_duration("2h 30m").unwrap(), 9_000);
        assert_eq!(parse_duration("2d4h").unwrap(), 2 * 86_400 + 4 * 3_600);
        assert_eq!(parse_duration("35m 12s").unwrap(), 35 * 60 + 12);
        assert_eq!(parse_duration("120").unwrap(), 120);
        assert_eq!(
            parse_duration("  1d  2h  3m  4s  ").unwrap(),
            86_400 + 7_200 + 180 + 4
        );
        assert_eq!(parse_duration("0s").unwrap(), 0);
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("   ").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("1.5h").is_err());
        assert!(parse_duration("d").is_err());
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("99999999999999999999d").is_err());
    }

    fn effective(ratio_source: &str, ratio: Option<f64>) -> EffectiveLimits {
        EffectiveLimits {
            ratio,
            seeding_time_secs: Some(86_400),
            inactive_secs: None,
            mode: LimitMode::Any,
            action: SeedAction::Throttle("seed".to_string()),
            sources: LimitSources {
                ratio: ratio_source.to_string(),
                seeding_time_secs: "label:movies".to_string(),
                inactive_secs: "torrent".to_string(),
                mode: "global".to_string(),
                action: "global".to_string(),
            },
        }
    }

    #[test]
    fn overrides_reconstructed_from_sources() {
        // Explicit torrent Set wins and is attributed to "torrent".
        let limits = effective("torrent", Some(3.0));
        let overrides = overrides_from_effective(&limits);
        assert_eq!(overrides.ratio, LimitValue::Set(3.0));
        // Inactive: source torrent + None effective = explicit Unlimited.
        assert_eq!(overrides.inactive_secs, LimitValue::Unlimited);
        // Label/global sources mean the torrent override is Inherit.
        assert_eq!(overrides.seeding_time_secs, LimitValue::Inherit);
        // Action source global → inherit (None), even though resolved.
        assert_eq!(overrides.action, None);

        let limits = effective("label:movies", Some(2.0));
        assert_eq!(overrides_from_effective(&limits).ratio, LimitValue::Inherit);
        let limits = effective("global", None);
        assert_eq!(overrides_from_effective(&limits).ratio, LimitValue::Inherit);
    }

    #[test]
    fn overrides_action_torrent_source_round_trips() {
        let mut limits = effective("global", None);
        limits.sources.action = "torrent".to_string();
        limits.action = SeedAction::RemoveWithData;
        let overrides = overrides_from_effective(&limits);
        assert_eq!(overrides.action, Some(SeedAction::RemoveWithData));
    }

    #[test]
    fn action_choice_select_values_round_trip() {
        for choice in [
            ActionChoice::Inherit,
            ActionChoice::Stop,
            ActionChoice::Close,
            ActionChoice::Remove,
            ActionChoice::RemoveWithData,
            ActionChoice::Throttle,
            ActionChoice::SuperSeed,
            ActionChoice::Notify,
            ActionChoice::Command,
        ] {
            assert_eq!(ActionChoice::parse(choice.as_str()), choice);
        }
        for mode in [
            OverrideMode::Inherit,
            OverrideMode::Unlimited,
            OverrideMode::Set,
        ] {
            assert_eq!(OverrideMode::parse(mode.as_str()), mode);
        }
    }
}
