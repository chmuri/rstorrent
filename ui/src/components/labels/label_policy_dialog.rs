//! Per-label seed policy dialog (plan.md T10, §6.9) plus the shared
//! seed-control vocabulary — limit kinds (inherit/unlimited/set), human time
//! units and the action select — reused by the settings "Seeding limits"
//! card. Sources badges are deliberately NOT here: they belong to the
//! per-torrent effective-limits dialog (T11).
//!
//! JSON shapes are binding (§6.1): [`LimitValue`] serializes as `"inherit"`,
//! `"unlimited"` or `{"set": …}`; [`SeedAction`] is externally tagged, e.g.
//! `{"throttle": "seed"}`. Saving goes through `labels_update_policy`
//! (`PUT /api/labels/{name}/policy`).

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::ui::{Button, ButtonVariant};
use crate::state::rules::{
    labels_update_policy, LimitMode, LimitValue, RulesPageStore, SeedAction, SeedPolicy,
};
use crate::state::store::{AppState, ToastKind};

// Copied from pages/settings.rs (private there) to keep the shared input look.
const INPUT_CLASS: &str = "w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all";

const LABEL_CLASS: &str = "mb-1.5 block text-sm font-medium text-zinc-200";

// Mirrors ButtonVariant::Primary; the save button needs a QA id, which the
// shared Button component does not expose (feeds_panel.rs precedent).
const PRIMARY_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-medium text-white shadow-[inset_0_1px_0_0_rgba(255,255,255,0.2)] hover:brightness-110 active:scale-[0.98] transition-all cursor-pointer";

/// Human-friendly duration units; the backend contract is always seconds
/// (plan.md §6.1 unit table), so values are converted on save.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl TimeUnit {
    pub fn multiplier(self) -> u64 {
        match self {
            Self::Seconds => 1,
            Self::Minutes => 60,
            Self::Hours => 3_600,
            Self::Days => 86_400,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seconds => "seconds",
            Self::Minutes => "minutes",
            Self::Hours => "hours",
            Self::Days => "days",
        }
    }

    pub fn from_value(value: &str) -> Self {
        match value {
            "minutes" => Self::Minutes,
            "hours" => Self::Hours,
            "days" => Self::Days,
            _ => Self::Seconds,
        }
    }
}

/// Selectable [`SeedAction`] variants; `throttle` and `command` carry an extra
/// text argument. The select value is the snake_case variant name, the JSON
/// payload stays externally tagged (the store DTO handles serialization).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Stop,
    Close,
    Remove,
    RemoveWithData,
    Throttle,
    SuperSeed,
    Notify,
    Command,
}

impl ActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
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

    pub fn from_value(value: &str) -> Self {
        match value {
            "close" => Self::Close,
            "remove" => Self::Remove,
            "remove_with_data" => Self::RemoveWithData,
            "throttle" => Self::Throttle,
            "super_seed" => Self::SuperSeed,
            "notify" => Self::Notify,
            "command" => Self::Command,
            _ => Self::Stop,
        }
    }

    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Remove | Self::RemoveWithData)
    }

    pub fn needs_arg(self) -> bool {
        matches!(self, Self::Throttle | Self::Command)
    }

    /// Split an action into its select kind and the extra argument (empty for
    /// argument-less variants).
    pub fn from_action(action: &SeedAction) -> (Self, String) {
        match action {
            SeedAction::Stop => (Self::Stop, String::new()),
            SeedAction::Close => (Self::Close, String::new()),
            SeedAction::Remove => (Self::Remove, String::new()),
            SeedAction::RemoveWithData => (Self::RemoveWithData, String::new()),
            SeedAction::Throttle(name) => (Self::Throttle, name.clone()),
            SeedAction::SuperSeed => (Self::SuperSeed, String::new()),
            SeedAction::Notify => (Self::Notify, String::new()),
            SeedAction::Command(command) => (Self::Command, command.clone()),
        }
    }
}

/// The inherit/unlimited/set choice of one [`LimitValue`] field.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    Inherit,
    Unlimited,
    Set,
}

impl LimitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Unlimited => "unlimited",
            Self::Set => "set",
        }
    }

    pub fn from_value(value: &str) -> Self {
        match value {
            "unlimited" => Self::Unlimited,
            "set" => Self::Set,
            _ => Self::Inherit,
        }
    }

    pub fn from_limit<T>(value: &LimitValue<T>) -> Self {
        match value {
            LimitValue::Inherit => Self::Inherit,
            LimitValue::Unlimited => Self::Unlimited,
            LimitValue::Set(_) => Self::Set,
        }
    }
}

/// Build the externally tagged action from the select kind + extra argument.
pub fn build_action(kind: ActionKind, arg: &str) -> Result<SeedAction, String> {
    let arg = arg.trim();
    match kind {
        ActionKind::Stop => Ok(SeedAction::Stop),
        ActionKind::Close => Ok(SeedAction::Close),
        ActionKind::Remove => Ok(SeedAction::Remove),
        ActionKind::RemoveWithData => Ok(SeedAction::RemoveWithData),
        ActionKind::Throttle => {
            if arg.is_empty() {
                return Err("Enter the throttle name for the throttle action.".to_string());
            }
            Ok(SeedAction::Throttle(arg.to_string()))
        }
        ActionKind::SuperSeed => Ok(SeedAction::SuperSeed),
        ActionKind::Notify => Ok(SeedAction::Notify),
        ActionKind::Command => {
            if arg.is_empty() {
                return Err("Enter the d.* command for the command action.".to_string());
            }
            Ok(SeedAction::Command(arg.to_string()))
        }
    }
}

/// Compact human label for badges and summaries.
pub fn action_label(action: &SeedAction) -> String {
    match action {
        SeedAction::Stop => "stop".to_string(),
        SeedAction::Close => "close".to_string(),
        SeedAction::Remove => "remove".to_string(),
        SeedAction::RemoveWithData => "remove with data".to_string(),
        SeedAction::Throttle(name) => format!("throttle:{name}"),
        SeedAction::SuperSeed => "super-seed".to_string(),
        SeedAction::Notify => "notify".to_string(),
        SeedAction::Command(command) => format!("command:{}", short_text(command, 24)),
    }
}

/// Red warning shown for the destructive `remove*` actions (plan §5.5).
pub fn destructive_warning(kind: ActionKind) -> Option<&'static str> {
    match kind {
        ActionKind::Remove => Some(
            "Destructive action: torrents that reach a limit are removed from the client \
             (downloaded data stays on disk). Requires the global destructive gate \
             ([rules] allow_destructive).",
        ),
        ActionKind::RemoveWithData => Some(
            "Destructive action: torrents that reach a limit are removed AND their downloaded \
             data is deleted from disk. This cannot be undone. Requires the global destructive \
             gate ([rules] allow_destructive).",
        ),
        _ => None,
    }
}

/// Split seconds into the largest whole unit for editing (7 d, 2 h, 30 m, 45 s).
pub fn split_duration(secs: u64) -> (String, TimeUnit) {
    if secs > 0 && secs.is_multiple_of(86_400) {
        ((secs / 86_400).to_string(), TimeUnit::Days)
    } else if secs > 0 && secs.is_multiple_of(3_600) {
        ((secs / 3_600).to_string(), TimeUnit::Hours)
    } else if secs > 0 && secs.is_multiple_of(60) {
        ((secs / 60).to_string(), TimeUnit::Minutes)
    } else {
        (secs.to_string(), TimeUnit::Seconds)
    }
}

/// Compact duration for badges: "7d", "2h 30m", "45s".
pub fn format_secs_short(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 {
        parts.push(format!("{seconds}s"));
    }
    parts.join(" ")
}

/// Parse a ratio value (`1.0` = 100%); must be finite and non-negative.
pub fn parse_ratio_value(text: &str) -> Result<f64, String> {
    let value = text
        .trim()
        .parse::<f64>()
        .map_err(|_| "Ratio must be a number, e.g. 2.0 (1.0 = 100%).".to_string())?;
    if !value.is_finite() || value < 0.0 {
        return Err("Ratio must be a non-negative number.".to_string());
    }
    Ok(value)
}

/// Parse a duration in the given unit into whole seconds (fractional inputs
/// like `1.5 hours` are allowed and rounded).
pub fn parse_duration_value(text: &str, unit: TimeUnit) -> Result<u64, String> {
    let value = text
        .trim()
        .parse::<f64>()
        .map_err(|_| "Duration must be a number, e.g. 30 minutes or 7 days.".to_string())?;
    if !value.is_finite() || value < 0.0 {
        return Err("Duration must be a non-negative number.".to_string());
    }
    let secs = value * unit.multiplier() as f64;
    if secs > u64::MAX as f64 {
        return Err("Duration is too large.".to_string());
    }
    Ok(secs.round() as u64)
}

fn short_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(max_chars).collect();
    shortened.push_str("...");
    shortened
}

/// Form state for one label policy. Numeric fields stay as strings so
/// partially typed values survive keystrokes; they are parsed on save
/// (rss/rules_panel.rs RuleDraft pattern).
#[derive(Clone)]
struct PolicyDraft {
    ratio_kind: LimitKind,
    ratio_value: String,
    time_kind: LimitKind,
    time_value: String,
    time_unit: TimeUnit,
    idle_kind: LimitKind,
    idle_value: String,
    idle_unit: TimeUnit,
    mode: LimitMode,
    /// None = inherit the action from the level above.
    action_kind: Option<ActionKind>,
    action_arg: String,
}

impl PolicyDraft {
    fn from_policy(policy: &SeedPolicy) -> Self {
        let ratio_value = match policy.ratio {
            LimitValue::Set(ratio) => ratio.to_string(),
            _ => String::new(),
        };
        let (time_value, time_unit) = match policy.seeding_time_secs {
            LimitValue::Set(secs) => split_duration(secs),
            _ => ("0".to_string(), TimeUnit::Seconds),
        };
        let (idle_value, idle_unit) = match policy.inactive_secs {
            LimitValue::Set(secs) => split_duration(secs),
            _ => ("0".to_string(), TimeUnit::Seconds),
        };
        let (action_kind, action_arg) = policy.action.as_ref().map(ActionKind::from_action).unzip();
        Self {
            ratio_kind: LimitKind::from_limit(&policy.ratio),
            ratio_value,
            time_kind: LimitKind::from_limit(&policy.seeding_time_secs),
            time_value,
            time_unit,
            idle_kind: LimitKind::from_limit(&policy.inactive_secs),
            idle_value,
            idle_unit,
            mode: policy.mode,
            action_kind,
            action_arg: action_arg.unwrap_or_default(),
        }
    }
}

fn draft_to_policy(draft: &PolicyDraft) -> Result<SeedPolicy, String> {
    let ratio = match draft.ratio_kind {
        LimitKind::Inherit => LimitValue::Inherit,
        LimitKind::Unlimited => LimitValue::Unlimited,
        LimitKind::Set => LimitValue::Set(parse_ratio_value(&draft.ratio_value)?),
    };
    let seeding_time_secs = match draft.time_kind {
        LimitKind::Inherit => LimitValue::Inherit,
        LimitKind::Unlimited => LimitValue::Unlimited,
        LimitKind::Set => {
            LimitValue::Set(parse_duration_value(&draft.time_value, draft.time_unit)?)
        }
    };
    let inactive_secs = match draft.idle_kind {
        LimitKind::Inherit => LimitValue::Inherit,
        LimitKind::Unlimited => LimitValue::Unlimited,
        LimitKind::Set => {
            LimitValue::Set(parse_duration_value(&draft.idle_value, draft.idle_unit)?)
        }
    };
    let action = match draft.action_kind {
        None => None,
        Some(kind) => Some(build_action(kind, &draft.action_arg)?),
    };
    Ok(SeedPolicy {
        ratio,
        seeding_time_secs,
        inactive_secs,
        mode: draft.mode,
        action,
    })
}

/// Modal editor for one label's [`SeedPolicy`]. Each limit field offers
/// inherit / unlimited / set; `set` reveals the value input (times with a
/// human unit select). The action select includes `inherit` (None). Saves via
/// `PUT /api/labels/{name}/policy`, then bumps the store reload and closes.
#[component]
pub fn LabelPolicyDialog(
    #[prop(into)] name: String,
    policy: SeedPolicy,
    on_close: Callback<()>,
) -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = use_context::<RulesPageStore>().unwrap_or_default();

    let draft = RwSignal::new(PolicyDraft::from_policy(&policy));
    let busy = RwSignal::new(false);

    let close = move || {
        if !busy.get_untracked() {
            on_close.run(());
        }
    };

    let key_handle =
        window_event_listener(leptos::ev::keydown, move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" {
                close();
            }
        });
    on_cleanup(move || key_handle.remove());

    let save_name = name.clone();
    let save = StoredValue::new(move || {
        if busy.get() {
            return;
        }
        let policy = match draft_to_policy(&draft.get()) {
            Ok(policy) => policy,
            Err(message) => {
                state.push_toast(ToastKind::Error, "Invalid label policy", message);
                return;
            }
        };
        busy.set(true);
        let label_name = save_name.clone();
        spawn_local(async move {
            match labels_update_policy(&state, &label_name, &policy).await {
                Ok(_) => {
                    state.push_toast(
                        ToastKind::Success,
                        "Label policy saved",
                        format!("\"{label_name}\" seed policy updated."),
                    );
                    store.reload.update(|n| *n += 1);
                    on_close.run(());
                }
                Err(err) => {
                    state.push_toast(
                        ToastKind::Error,
                        "Could not save label policy",
                        err.to_string(),
                    );
                }
            }
            busy.set(false);
        });
    });

    let title = format!("Seed policy — {name}");

    view! {
        <div
            class="fixed inset-0 z-[60] flex items-center justify-center bg-black/75 backdrop-blur-sm p-4"
            on:click=move |ev| {
                if ev.target() == ev.current_target() {
                    close();
                }
            }
        >
            <div
                class="relative max-h-[85vh] w-full max-w-lg overflow-y-auto rounded-2xl border border-white/[0.12] bg-zinc-900 shadow-2xl p-6 text-zinc-100"
                role="dialog"
                aria-modal="true"
                aria-label=title.clone()
            >
                <h2 class="text-base font-semibold tracking-tight">{title.clone()}</h2>
                <p class="mt-1 text-sm text-zinc-400">
                    "Inherit defers to the global defaults; unlimited opts this label out of a limit."
                </p>

                <div class="mt-4 space-y-4">
                    <div>
                        <label for="label-policy-ratio-kind" class=LABEL_CLASS>"Ratio limit"</label>
                        <div class="grid grid-cols-2 gap-3">
                            <select
                                id="label-policy-ratio-kind"
                                class=INPUT_CLASS
                                prop:value=move || draft.get().ratio_kind.as_str()
                                on:change=move |ev| {
                                    let kind = LimitKind::from_value(&event_target_value(&ev));
                                    draft.update(|d| d.ratio_kind = kind);
                                }
                            >
                                <option value="inherit">"Inherit"</option>
                                <option value="unlimited">"Unlimited"</option>
                                <option value="set">"Set value"</option>
                            </select>
                            {move || {
                                (draft.get().ratio_kind == LimitKind::Set).then(|| {
                                    view! {
                                        <input
                                            id="label-policy-ratio-value"
                                            type="number"
                                            min="0"
                                            step="0.1"
                                            class=INPUT_CLASS
                                            placeholder="e.g. 2.0"
                                            prop:value=move || draft.get().ratio_value.clone()
                                            on:input=move |ev| {
                                                let value = event_target_value(&ev);
                                                draft.update(|d| d.ratio_value = value);
                                            }
                                        />
                                    }
                                })
                            }}
                        </div>
                        <p class="mt-1.5 text-xs text-zinc-500">"1.0 = 100% uploaded / downloaded."</p>
                    </div>

                    <div>
                        <label for="label-policy-time-kind" class=LABEL_CLASS>"Seeding time limit"</label>
                        <div class="grid grid-cols-3 gap-3">
                            <select
                                id="label-policy-time-kind"
                                class=INPUT_CLASS
                                prop:value=move || draft.get().time_kind.as_str()
                                on:change=move |ev| {
                                    let kind = LimitKind::from_value(&event_target_value(&ev));
                                    draft.update(|d| d.time_kind = kind);
                                }
                            >
                                <option value="inherit">"Inherit"</option>
                                <option value="unlimited">"Unlimited"</option>
                                <option value="set">"Set value"</option>
                            </select>
                            {move || {
                                (draft.get().time_kind == LimitKind::Set).then(|| {
                                    view! {
                                        <input
                                            id="label-policy-time-value"
                                            type="number"
                                            min="0"
                                            step="any"
                                            class=INPUT_CLASS
                                            placeholder="e.g. 7"
                                            prop:value=move || draft.get().time_value.clone()
                                            on:input=move |ev| {
                                                let value = event_target_value(&ev);
                                                draft.update(|d| d.time_value = value);
                                            }
                                        />
                                    }
                                })
                            }}
                            {move || {
                                (draft.get().time_kind == LimitKind::Set).then(|| {
                                    view! {
                                        <select
                                            id="label-policy-time-unit"
                                            class=INPUT_CLASS
                                            prop:value=move || draft.get().time_unit.as_str()
                                            on:change=move |ev| {
                                                let unit = TimeUnit::from_value(&event_target_value(&ev));
                                                draft.update(|d| d.time_unit = unit);
                                            }
                                        >
                                            <option value="seconds">"seconds"</option>
                                            <option value="minutes">"minutes"</option>
                                            <option value="hours">"hours"</option>
                                            <option value="days">"days"</option>
                                        </select>
                                    }
                                })
                            }}
                        </div>
                        <p class="mt-1.5 text-xs text-zinc-500">
                            "Accumulated active-seeding time; stored as seconds."
                        </p>
                    </div>

                    <div>
                        <label for="label-policy-idle-kind" class=LABEL_CLASS>"Inactive limit"</label>
                        <div class="grid grid-cols-3 gap-3">
                            <select
                                id="label-policy-idle-kind"
                                class=INPUT_CLASS
                                prop:value=move || draft.get().idle_kind.as_str()
                                on:change=move |ev| {
                                    let kind = LimitKind::from_value(&event_target_value(&ev));
                                    draft.update(|d| d.idle_kind = kind);
                                }
                            >
                                <option value="inherit">"Inherit"</option>
                                <option value="unlimited">"Unlimited"</option>
                                <option value="set">"Set value"</option>
                            </select>
                            {move || {
                                (draft.get().idle_kind == LimitKind::Set).then(|| {
                                    view! {
                                        <input
                                            id="label-policy-idle-value"
                                            type="number"
                                            min="0"
                                            step="any"
                                            class=INPUT_CLASS
                                            placeholder="e.g. 30"
                                            prop:value=move || draft.get().idle_value.clone()
                                            on:input=move |ev| {
                                                let value = event_target_value(&ev);
                                                draft.update(|d| d.idle_value = value);
                                            }
                                        />
                                    }
                                })
                            }}
                            {move || {
                                (draft.get().idle_kind == LimitKind::Set).then(|| {
                                    view! {
                                        <select
                                            id="label-policy-idle-unit"
                                            class=INPUT_CLASS
                                            prop:value=move || draft.get().idle_unit.as_str()
                                            on:change=move |ev| {
                                                let unit = TimeUnit::from_value(&event_target_value(&ev));
                                                draft.update(|d| d.idle_unit = unit);
                                            }
                                        >
                                            <option value="seconds">"seconds"</option>
                                            <option value="minutes">"minutes"</option>
                                            <option value="hours">"hours"</option>
                                            <option value="days">"days"</option>
                                        </select>
                                    }
                                })
                            }}
                        </div>
                        <p class="mt-1.5 text-xs text-zinc-500">
                            "Seconds without upload while seeding; stored as seconds."
                        </p>
                    </div>

                    <div>
                        <label for="label-policy-mode" class=LABEL_CLASS>"Limit mode"</label>
                        <select
                            id="label-policy-mode"
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
                                draft.update(|d| d.mode = mode);
                            }
                        >
                            <option value="any">"Any — first reached limit fires"</option>
                            <option value="all">"All — every active limit must be reached"</option>
                        </select>
                    </div>

                    <div>
                        <label for="label-policy-action" class=LABEL_CLASS>"Action on limit"</label>
                        <select
                            id="label-policy-action"
                            class=INPUT_CLASS
                            prop:value=move || {
                                draft.get().action_kind.map(ActionKind::as_str).unwrap_or("inherit")
                            }
                            on:change=move |ev| {
                                let value = event_target_value(&ev);
                                let kind = (value != "inherit").then(|| ActionKind::from_value(&value));
                                draft.update(|d| d.action_kind = kind);
                            }
                        >
                            <option value="inherit">"Inherit"</option>
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
                            let kind = draft.get().action_kind;
                            kind.filter(|kind| kind.needs_arg()).map(|kind| {
                                let (id, label_text, placeholder) = if kind == ActionKind::Throttle {
                                    ("label-policy-action-arg", "Throttle name", "e.g. seed")
                                } else {
                                    ("label-policy-action-arg", "d.* command", "e.g. d.stop=")
                                };
                                view! {
                                    <div class="mt-3">
                                        <label for=id class=LABEL_CLASS>{label_text}</label>
                                        <input
                                            id=id
                                            type="text"
                                            class=INPUT_CLASS
                                            placeholder=placeholder
                                            autocomplete="off"
                                            prop:value=move || draft.get().action_arg.clone()
                                            on:input=move |ev| {
                                                let value = event_target_value(&ev);
                                                draft.update(|d| d.action_arg = value);
                                            }
                                        />
                                    </div>
                                }
                            })
                        }}
                        {move || {
                            draft.get().action_kind.and_then(destructive_warning).map(|warning| {
                                view! {
                                    <div class="mt-3 rounded-xl border border-rose-500/30 bg-rose-500/10 px-3.5 py-2.5 text-xs text-rose-300">
                                        {warning}
                                    </div>
                                }
                            })
                        }}
                    </div>
                </div>

                <div class="mt-5 flex items-center justify-end gap-2">
                    <Button
                        label="Cancel"
                        variant=ButtonVariant::Ghost
                        on_click=Callback::new(move |_| close())
                    />
                    {move || {
                        let is_busy = busy.get();
                        view! {
                            <button
                                id="label-policy-save"
                                type="button"
                                disabled=is_busy
                                class=if is_busy {
                                    format!("{PRIMARY_BUTTON_CLASS} opacity-50 pointer-events-none")
                                } else {
                                    PRIMARY_BUTTON_CLASS.to_string()
                                }
                                on:click=move |_| save.with_value(|save| save())
                            >
                                {if is_busy { "Saving…" } else { "Save policy" }}
                            </button>
                        }
                    }}
                </div>
            </div>
        </div>
    }
}
