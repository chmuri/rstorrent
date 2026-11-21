//! Action builder (T9, plan §6.9): ordered action rows plus an add row
//! covering every [`Action`] variant from §6.1 (18 variants — the task brief
//! says 19, but the frozen contract and the T8 DTO define 18; reported).
//! Actions execute in order; `Noop` is the exception marker that stops
//! evaluating. Drafts keep parameters as strings (RSS editor pattern) and are
//! parsed/validated on save/test. `Execute` argv is edited one argument per
//! line. Rows are keyed by a stable draft key so typing keeps input focus.

use leptos::prelude::*;

use crate::components::ui::{Button, ButtonVariant};
use crate::state::rules::Action;

use super::{next_draft_key, optional_text, parse_bool, parse_required_u8, required_text};

const SELECT_CLASS: &str = "cursor-pointer rounded-xl border border-white/[0.08] bg-zinc-950/80 px-3 py-2 text-sm text-zinc-200 transition-all focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20";

const CHIP_SELECT_CLASS: &str = "cursor-pointer rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 text-xs text-zinc-200 transition-all focus:border-indigo-500/60 focus:outline-none";

const CHIP_NUMBER_CLASS: &str = "w-24 rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 text-xs text-zinc-100 placeholder:text-zinc-600 transition-all focus:border-indigo-500/60 focus:outline-none";

const CHIP_TEXT_CLASS: &str = "w-44 rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 text-xs text-zinc-100 placeholder:text-zinc-600 transition-all focus:border-indigo-500/60 focus:outline-none";

const CHIP_WIDE_CLASS: &str = "w-64 rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 text-xs text-zinc-100 placeholder:text-zinc-600 transition-all focus:border-indigo-500/60 focus:outline-none";

const CHIP_TEXTAREA_CLASS: &str = "w-full rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 font-mono text-xs text-zinc-100 placeholder:text-zinc-600 transition-all focus:border-indigo-500/60 focus:outline-none";

const ROW_ICON_BUTTON_CLASS: &str = "shrink-0 cursor-pointer rounded-lg p-1.5 text-zinc-500 transition-colors hover:bg-white/[0.06] hover:text-zinc-100 disabled:opacity-40 disabled:pointer-events-none";

/// The 18 action variants of §6.1, in contract order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Noop,
    Stop,
    Pause,
    Start,
    Close,
    Remove,
    RemoveWithData,
    MoveToDir,
    MoveToTier,
    SetLabel,
    AddTag,
    RemoveTag,
    SetThrottle,
    SetPriority,
    SuperSeed,
    Execute,
    Webhook,
    Command,
}

impl ActionKind {
    pub const ALL: [ActionKind; 18] = [
        ActionKind::Noop,
        ActionKind::Stop,
        ActionKind::Pause,
        ActionKind::Start,
        ActionKind::Close,
        ActionKind::Remove,
        ActionKind::RemoveWithData,
        ActionKind::MoveToDir,
        ActionKind::MoveToTier,
        ActionKind::SetLabel,
        ActionKind::AddTag,
        ActionKind::RemoveTag,
        ActionKind::SetThrottle,
        ActionKind::SetPriority,
        ActionKind::SuperSeed,
        ActionKind::Execute,
        ActionKind::Webhook,
        ActionKind::Command,
    ];

    /// snake_case value, identical to the §6.1 serde tag.
    pub fn value(self) -> &'static str {
        match self {
            ActionKind::Noop => "noop",
            ActionKind::Stop => "stop",
            ActionKind::Pause => "pause",
            ActionKind::Start => "start",
            ActionKind::Close => "close",
            ActionKind::Remove => "remove",
            ActionKind::RemoveWithData => "remove_with_data",
            ActionKind::MoveToDir => "move_to_dir",
            ActionKind::MoveToTier => "move_to_tier",
            ActionKind::SetLabel => "set_label",
            ActionKind::AddTag => "add_tag",
            ActionKind::RemoveTag => "remove_tag",
            ActionKind::SetThrottle => "set_throttle",
            ActionKind::SetPriority => "set_priority",
            ActionKind::SuperSeed => "super_seed",
            ActionKind::Execute => "execute",
            ActionKind::Webhook => "webhook",
            ActionKind::Command => "command",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ActionKind::Noop => "Noop — exception, stop evaluating",
            ActionKind::Stop => "Stop",
            ActionKind::Pause => "Pause",
            ActionKind::Start => "Start",
            ActionKind::Close => "Close",
            ActionKind::Remove => "Remove (keep data)",
            ActionKind::RemoveWithData => "Remove with data (destructive)",
            ActionKind::MoveToDir => "Move to directory",
            ActionKind::MoveToTier => "Move to tier",
            ActionKind::SetLabel => "Set label",
            ActionKind::AddTag => "Add tag",
            ActionKind::RemoveTag => "Remove tag",
            ActionKind::SetThrottle => "Set throttle",
            ActionKind::SetPriority => "Set priority",
            ActionKind::SuperSeed => "Super seed",
            ActionKind::Execute => "Execute (argv)",
            ActionKind::Webhook => "Webhook",
            ActionKind::Command => "Command (d.*)",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.value() == value)
    }

    pub fn is_destructive(self) -> bool {
        matches!(self, ActionKind::Remove | ActionKind::RemoveWithData)
    }

    pub fn default_params(self) -> Vec<String> {
        match self {
            ActionKind::Noop
            | ActionKind::Stop
            | ActionKind::Pause
            | ActionKind::Start
            | ActionKind::Close
            | ActionKind::Remove
            | ActionKind::RemoveWithData => Vec::new(),
            ActionKind::SuperSeed => vec!["true".to_string()],
            _ => vec![String::new()],
        }
    }
}

/// One editable action row. Parameters stay strings until save/test.
#[derive(Debug, Clone)]
pub struct ActionDraft {
    pub key: u64,
    pub kind: ActionKind,
    pub params: Vec<String>,
}

impl ActionDraft {
    fn param(&self, index: usize) -> &str {
        self.params.get(index).map(String::as_str).unwrap_or("")
    }

    fn to_action(&self) -> Result<Action, String> {
        match self.kind {
            ActionKind::Noop => Ok(Action::Noop),
            ActionKind::Stop => Ok(Action::Stop),
            ActionKind::Pause => Ok(Action::Pause),
            ActionKind::Start => Ok(Action::Start),
            ActionKind::Close => Ok(Action::Close),
            ActionKind::Remove => Ok(Action::Remove),
            ActionKind::RemoveWithData => Ok(Action::RemoveWithData),
            ActionKind::MoveToDir => Ok(Action::MoveToDir(required_text(
                self.param(0),
                "Directory",
            )?)),
            ActionKind::MoveToTier => Ok(Action::MoveToTier(parse_required_u8(
                self.param(0),
                "Tier",
            )?)),
            ActionKind::SetLabel => Ok(Action::SetLabel(required_text(self.param(0), "Label")?)),
            ActionKind::AddTag => Ok(Action::AddTag(required_text(self.param(0), "Tag")?)),
            ActionKind::RemoveTag => Ok(Action::RemoveTag(required_text(self.param(0), "Tag")?)),
            ActionKind::SetThrottle => Ok(Action::SetThrottle(optional_text(self.param(0)))),
            ActionKind::SetPriority => {
                let priority = parse_required_u8(self.param(0), "Priority")?;
                if priority > 3 {
                    return Err("Priority must be between 0 and 3.".to_string());
                }
                Ok(Action::SetPriority(priority))
            }
            ActionKind::SuperSeed => Ok(Action::SuperSeed(parse_bool(self.param(0)))),
            ActionKind::Execute => {
                let argv: Vec<String> = self
                    .param(0)
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(String::from)
                    .collect();
                if argv.is_empty() {
                    return Err(
                        "Execute needs at least the program path (one argument per line)."
                            .to_string(),
                    );
                }
                Ok(Action::Execute(argv))
            }
            ActionKind::Webhook => {
                let url = required_text(self.param(0), "URL")?;
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err("Webhook URL must start with http:// or https://.".to_string());
                }
                Ok(Action::Webhook { url })
            }
            ActionKind::Command => Ok(Action::Command(required_text(self.param(0), "Command")?)),
        }
    }
}

/// Typed actions → editable drafts (editor open).
pub fn action_drafts_from(actions: &[Action]) -> Vec<ActionDraft> {
    actions
        .iter()
        .map(|action| {
            let (kind, params) = match action {
                Action::Noop => (ActionKind::Noop, Vec::new()),
                Action::Stop => (ActionKind::Stop, Vec::new()),
                Action::Pause => (ActionKind::Pause, Vec::new()),
                Action::Start => (ActionKind::Start, Vec::new()),
                Action::Close => (ActionKind::Close, Vec::new()),
                Action::Remove => (ActionKind::Remove, Vec::new()),
                Action::RemoveWithData => (ActionKind::RemoveWithData, Vec::new()),
                Action::MoveToDir(dir) => (ActionKind::MoveToDir, vec![dir.clone()]),
                Action::MoveToTier(tier) => (ActionKind::MoveToTier, vec![tier.to_string()]),
                Action::SetLabel(label) => (ActionKind::SetLabel, vec![label.clone()]),
                Action::AddTag(tag) => (ActionKind::AddTag, vec![tag.clone()]),
                Action::RemoveTag(tag) => (ActionKind::RemoveTag, vec![tag.clone()]),
                Action::SetThrottle(name) => (
                    ActionKind::SetThrottle,
                    vec![name.clone().unwrap_or_default()],
                ),
                Action::SetPriority(priority) => {
                    (ActionKind::SetPriority, vec![priority.to_string()])
                }
                Action::SuperSeed(value) => (ActionKind::SuperSeed, vec![value.to_string()]),
                Action::Execute(argv) => (ActionKind::Execute, vec![argv.join("\n")]),
                Action::Webhook { url } => (ActionKind::Webhook, vec![url.clone()]),
                Action::Command(command) => (ActionKind::Command, vec![command.clone()]),
            };
            ActionDraft {
                key: next_draft_key(),
                kind,
                params,
            }
        })
        .collect()
}

/// Drafts → typed actions in row order, with row-numbered errors (save/test).
pub fn actions_from_drafts(drafts: &[ActionDraft]) -> Result<Vec<Action>, String> {
    drafts
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            draft
                .to_action()
                .map_err(|err| format!("Action {} ({}): {err}", index + 1, draft.kind.label()))
        })
        .collect()
}

/// Human-readable payload summary for dry-run matches.
pub fn action_summary(action: &Action) -> String {
    match action {
        Action::Noop => "noop (stop evaluating)".to_string(),
        Action::Stop => "stop".to_string(),
        Action::Pause => "pause".to_string(),
        Action::Start => "start".to_string(),
        Action::Close => "close".to_string(),
        Action::Remove => "remove".to_string(),
        Action::RemoveWithData => "remove_with_data".to_string(),
        Action::MoveToDir(dir) => format!("move_to_dir: {dir}"),
        Action::MoveToTier(tier) => format!("move_to_tier: {tier}"),
        Action::SetLabel(label) => format!("set_label: {label}"),
        Action::AddTag(tag) => format!("add_tag: {tag}"),
        Action::RemoveTag(tag) => format!("remove_tag: {tag}"),
        Action::SetThrottle(None) => "set_throttle: clear".to_string(),
        Action::SetThrottle(Some(name)) => format!("set_throttle: {name}"),
        Action::SetPriority(priority) => format!("set_priority: {priority}"),
        Action::SuperSeed(value) => format!("super_seed: {value}"),
        Action::Execute(argv) => format!("execute: {}", argv.join(" ")),
        Action::Webhook { url } => format!("webhook: {url}"),
        Action::Command(command) => format!("command: {command}"),
    }
}

/// Destructive gate applies to these actions (plan §5.5).
pub fn action_is_destructive(action: &Action) -> bool {
    matches!(action, Action::Remove | Action::RemoveWithData)
}

fn set_param(actions: RwSignal<Vec<ActionDraft>>, key: u64, index: usize, value: String) {
    actions.update(|list| {
        if let Some(draft) = list.iter_mut().find(|draft| draft.key == key) {
            if let Some(param) = draft.params.get_mut(index) {
                *param = value;
            }
        }
    });
}

/// Ordered action rows (move up/down, remove) plus a kind select + add row.
#[component]
pub fn ActionBuilder(actions: RwSignal<Vec<ActionDraft>>) -> impl IntoView {
    let add_kind = RwSignal::new(ActionKind::Stop);

    view! {
        <div class="space-y-2">
            <For
                each={move || actions.get().into_iter().map(|draft| draft.key).collect::<Vec<_>>()}
                key=|key| *key
                let:key
            >
                <ActionRow actions key />
            </For>
            {move || {
                actions.get().is_empty().then(|| {
                    view! {
                        <p class="rounded-lg border border-dashed border-white/[0.08] bg-white/[0.02] px-3 py-2 text-xs text-zinc-500">
                            "No actions yet — add at least one below."
                        </p>
                    }
                })
            }}
            <div class="flex flex-wrap items-center gap-2 pt-1">
                <select
                    class=SELECT_CLASS
                    aria-label="Action to add"
                    prop:value=move || add_kind.get().value()
                    on:change=move |ev| {
                        if let Some(kind) = ActionKind::from_value(&event_target_value(&ev)) {
                            add_kind.set(kind);
                        }
                    }
                >
                    {ActionKind::ALL.iter().map(|kind| {
                        view! { <option value=kind.value()>{kind.label()}</option> }
                    }).collect_view()}
                </select>
                <Button
                    label="Add action"
                    variant=ButtonVariant::Secondary
                    on_click=Callback::new(move |_| {
                        let kind = add_kind.get();
                        actions.update(|list| {
                            list.push(ActionDraft {
                                key: next_draft_key(),
                                kind,
                                params: kind.default_params(),
                            });
                        });
                    })
                />
            </div>
            <p class="text-xs text-zinc-500">
                "Actions execute in order. Noop is an exception: it stops evaluating this torrent."
            </p>
        </div>
    }
}

/// One ordered action row. Like the condition chip, the kind is a `Memo` so
/// parameter edits never rebuild the inputs (focus stays).
#[component]
fn ActionRow(actions: RwSignal<Vec<ActionDraft>>, key: u64) -> impl IntoView {
    let row_kind = Memo::new(move |_| {
        actions.with(|list| {
            list.iter()
                .find(|draft| draft.key == key)
                .map(|draft| draft.kind)
        })
    });
    let position = move || {
        actions.with(|list| {
            list.iter()
                .position(|draft| draft.key == key)
                .map(|index| index + 1)
                .unwrap_or(0)
        })
    };
    let is_first =
        move || actions.with(|list| list.first().map(|draft| draft.key != key).unwrap_or(true));
    let is_last =
        move || actions.with(|list| list.last().map(|draft| draft.key != key).unwrap_or(true));
    let move_up = move || {
        actions.update(|list| {
            if let Some(index) = list.iter().position(|draft| draft.key == key) {
                if index > 0 {
                    list.swap(index, index - 1);
                }
            }
        });
    };
    let move_down = move || {
        actions.update(|list| {
            if let Some(index) = list.iter().position(|draft| draft.key == key) {
                if index + 1 < list.len() {
                    list.swap(index, index + 1);
                }
            }
        });
    };

    view! {
        <div class="rounded-xl border border-white/[0.08] bg-white/[0.02] px-3 py-2 transition-all duration-200 hover:border-white/[0.15]">
            <div class="flex flex-wrap items-center gap-2">
                <span class="w-6 shrink-0 text-center font-mono text-xs tabular-nums text-zinc-500">
                    {position}
                </span>
                <select
                    class=CHIP_SELECT_CLASS
                    aria-label="Action kind"
                    prop:value=move || row_kind.get().map(|kind| kind.value()).unwrap_or_default()
                    on:change=move |ev| {
                        let Some(kind) = ActionKind::from_value(&event_target_value(&ev)) else {
                            return;
                        };
                        actions.update(|list| {
                            if let Some(draft) = list.iter_mut().find(|draft| draft.key == key) {
                                draft.kind = kind;
                                draft.params = kind.default_params();
                            }
                        });
                    }
                >
                    {ActionKind::ALL.iter().map(|kind| {
                        view! { <option value=kind.value()>{kind.label()}</option> }
                    }).collect_view()}
                </select>
                <div class="flex min-w-0 flex-1 flex-wrap items-center gap-2">
                    {move || {
                        let Some(kind) = row_kind.get() else {
                            return view! { <div></div> }.into_any();
                        };
                        let value = move || {
                            actions.with(|list| {
                                list.iter()
                                    .find(|draft| draft.key == key)
                                    .and_then(|draft| draft.params.first().cloned())
                                    .unwrap_or_default()
                            })
                        };
                        let text_input = |class: &'static str, placeholder: &'static str, datalist: Option<&'static str>| {
                            view! {
                                <input
                                    type="text"
                                    class=class
                                    placeholder=placeholder
                                    autocomplete="off"
                                    list=datalist
                                    prop:value=value
                                    on:input=move |ev| set_param(actions, key, 0, event_target_value(&ev))
                                />
                            }.into_any()
                        };
                        match kind {
                            ActionKind::Noop => view! {
                                <span class="text-xs text-zinc-500">"exception — stop evaluating"</span>
                            }.into_any(),
                            ActionKind::Stop | ActionKind::Pause | ActionKind::Start | ActionKind::Close => {
                                view! { <span></span> }.into_any()
                            }
                            ActionKind::Remove | ActionKind::RemoveWithData => view! {
                                <span class="text-xs font-medium text-rose-400">
                                    "destructive — gated by „Allow destructive actions”"
                                </span>
                            }.into_any(),
                            ActionKind::MoveToDir => text_input(CHIP_TEXT_CLASS, "/downloads/cold", None),
                            ActionKind::MoveToTier => view! {
                                <label class="flex items-center gap-1.5 text-xs text-zinc-400">
                                    "Tier"
                                    <input
                                        type="number"
                                        min="0"
                                        max="255"
                                        step="1"
                                        class=CHIP_NUMBER_CLASS
                                        placeholder="e.g. 2"
                                        prop:value=value
                                        on:input=move |ev| set_param(actions, key, 0, event_target_value(&ev))
                                    />
                                </label>
                            }.into_any(),
                            ActionKind::SetLabel => text_input(CHIP_TEXT_CLASS, "movies", Some("rule-editor-labels")),
                            ActionKind::AddTag | ActionKind::RemoveTag => text_input(CHIP_TEXT_CLASS, "keep", Some("rule-editor-tags")),
                            ActionKind::SetThrottle => text_input(CHIP_TEXT_CLASS, "Empty = clear throttle", None),
                            ActionKind::SetPriority => view! {
                                <label class="flex items-center gap-1.5 text-xs text-zinc-400">
                                    "0–3"
                                    <input
                                        type="number"
                                        min="0"
                                        max="3"
                                        step="1"
                                        class=CHIP_NUMBER_CLASS
                                        placeholder="e.g. 3"
                                        prop:value=value
                                        on:input=move |ev| set_param(actions, key, 0, event_target_value(&ev))
                                    />
                                </label>
                            }.into_any(),
                            ActionKind::SuperSeed => view! {
                                <select
                                    class=CHIP_SELECT_CLASS
                                    aria-label="Super seed enabled"
                                    prop:value=value
                                    on:change=move |ev| set_param(actions, key, 0, event_target_value(&ev))
                                >
                                    <option value="true">"true"</option>
                                    <option value="false">"false"</option>
                                </select>
                            }.into_any(),
                            ActionKind::Execute => view! {
                                <textarea
                                    rows="3"
                                    class=CHIP_TEXTAREA_CLASS
                                    placeholder="One argument per line, e.g.\n/usr/bin/notify-send\nRule fired"
                                    prop:value=value
                                    on:input=move |ev| set_param(actions, key, 0, event_target_value(&ev))
                                ></textarea>
                            }.into_any(),
                            ActionKind::Webhook => text_input(CHIP_WIDE_CLASS, "https://hooks.example.org/…", None),
                            ActionKind::Command => text_input(CHIP_WIDE_CLASS, "d.stop=", None),
                        }
                    }}
                </div>
                <div class="flex shrink-0 items-center gap-0.5">
                    <button
                        type="button"
                        class=ROW_ICON_BUTTON_CLASS
                        aria-label="Move action up"
                        title="Move up"
                        disabled=is_first
                        on:click=move |_| move_up()
                    >
                        <span aria-hidden="true">"↑"</span>
                    </button>
                    <button
                        type="button"
                        class=ROW_ICON_BUTTON_CLASS
                        aria-label="Move action down"
                        title="Move down"
                        disabled=is_last
                        on:click=move |_| move_down()
                    >
                        <span aria-hidden="true">"↓"</span>
                    </button>
                    <button
                        type="button"
                        class="shrink-0 cursor-pointer rounded-lg p-1.5 text-zinc-500 transition-colors hover:bg-white/[0.06] hover:text-rose-300"
                        aria-label="Remove action"
                        on:click=move |_| actions.update(|list| list.retain(|draft| draft.key != key))
                    >
                        <span aria-hidden="true">"✕"</span>
                    </button>
                </div>
            </div>
        </div>
    }
}
