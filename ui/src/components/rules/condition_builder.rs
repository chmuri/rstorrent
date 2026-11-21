//! Condition builder (T9, plan §6.9): editable chips plus an add row covering
//! all 19 [`Condition`] variants from §6.1. Drafts keep parameters as strings
//! so partially typed values survive keystrokes (RSS editor pattern); they are
//! parsed and validated on save/test. Size parameters are edited in MB and
//! converted to bytes for the DTO (shared helpers in `super`).
//!
//! Rows are keyed by a stable draft key (not by value) so typing into a chip
//! never recreates the input and loses focus; only a kind change rebuilds the
//! parameter area.

use leptos::prelude::*;

use crate::components::ui::{Button, ButtonVariant};
use crate::state::rules::Condition;

use super::{
    bytes_to_mb, next_draft_key, optional_text, parse_bool, parse_mb, parse_ratio,
    parse_required_u32, parse_required_u64, required_text,
};

const SELECT_CLASS: &str = "cursor-pointer rounded-xl border border-white/[0.08] bg-zinc-950/80 px-3 py-2 text-sm text-zinc-200 transition-all focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20";

const CHIP_SELECT_CLASS: &str = "cursor-pointer rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 text-xs text-zinc-200 transition-all focus:border-indigo-500/60 focus:outline-none";

const CHIP_NUMBER_CLASS: &str = "w-28 rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 text-xs text-zinc-100 placeholder:text-zinc-600 transition-all focus:border-indigo-500/60 focus:outline-none";

const CHIP_TEXT_CLASS: &str = "w-44 rounded-lg border border-white/[0.08] bg-zinc-950/80 px-2.5 py-1.5 text-xs text-zinc-100 placeholder:text-zinc-600 transition-all focus:border-indigo-500/60 focus:outline-none";

/// The 19 condition variants of §6.1, in contract order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionKind {
    RatioAtLeast,
    RatioBelow,
    SeedingTimeAtLeast,
    IdleAtLeast,
    AgeAtLeast,
    Complete,
    Active,
    Stopped,
    SizeAtLeast,
    SizeAtMost,
    SeedsAtLeast,
    SeedsAtMost,
    LeechersAtLeast,
    LabelIs,
    TagHas,
    TrackerIs,
    NameMatches,
    AddedByRss,
    FreeSpaceBelow,
}

/// How one draft parameter is edited inside a chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamInput {
    Text,
    Number,
    Bool,
    SizeMb,
}

#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    pub label: &'static str,
    pub placeholder: &'static str,
    pub input: ParamInput,
}

const RATIO_PARAM: ParamSpec = ParamSpec {
    label: "Ratio",
    placeholder: "e.g. 2.0",
    input: ParamInput::Number,
};
const SECONDS_PARAM: ParamSpec = ParamSpec {
    label: "Seconds",
    placeholder: "e.g. 86400",
    input: ParamInput::Number,
};
const BOOL_PARAM: ParamSpec = ParamSpec {
    label: "Is",
    placeholder: "",
    input: ParamInput::Bool,
};
const SIZE_MB_PARAM: ParamSpec = ParamSpec {
    label: "MB",
    placeholder: "e.g. 1024",
    input: ParamInput::SizeMb,
};
const COUNT_PARAM: ParamSpec = ParamSpec {
    label: "Count",
    placeholder: "e.g. 10",
    input: ParamInput::Number,
};

impl ConditionKind {
    pub const ALL: [ConditionKind; 19] = [
        ConditionKind::RatioAtLeast,
        ConditionKind::RatioBelow,
        ConditionKind::SeedingTimeAtLeast,
        ConditionKind::IdleAtLeast,
        ConditionKind::AgeAtLeast,
        ConditionKind::Complete,
        ConditionKind::Active,
        ConditionKind::Stopped,
        ConditionKind::SizeAtLeast,
        ConditionKind::SizeAtMost,
        ConditionKind::SeedsAtLeast,
        ConditionKind::SeedsAtMost,
        ConditionKind::LeechersAtLeast,
        ConditionKind::LabelIs,
        ConditionKind::TagHas,
        ConditionKind::TrackerIs,
        ConditionKind::NameMatches,
        ConditionKind::AddedByRss,
        ConditionKind::FreeSpaceBelow,
    ];

    /// snake_case value, identical to the §6.1 serde tag.
    pub fn value(self) -> &'static str {
        match self {
            ConditionKind::RatioAtLeast => "ratio_at_least",
            ConditionKind::RatioBelow => "ratio_below",
            ConditionKind::SeedingTimeAtLeast => "seeding_time_at_least",
            ConditionKind::IdleAtLeast => "idle_at_least",
            ConditionKind::AgeAtLeast => "age_at_least",
            ConditionKind::Complete => "complete",
            ConditionKind::Active => "active",
            ConditionKind::Stopped => "stopped",
            ConditionKind::SizeAtLeast => "size_at_least",
            ConditionKind::SizeAtMost => "size_at_most",
            ConditionKind::SeedsAtLeast => "seeds_at_least",
            ConditionKind::SeedsAtMost => "seeds_at_most",
            ConditionKind::LeechersAtLeast => "leechers_at_least",
            ConditionKind::LabelIs => "label_is",
            ConditionKind::TagHas => "tag_has",
            ConditionKind::TrackerIs => "tracker_is",
            ConditionKind::NameMatches => "name_matches",
            ConditionKind::AddedByRss => "added_by_rss",
            ConditionKind::FreeSpaceBelow => "free_space_below",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ConditionKind::RatioAtLeast => "Ratio at least",
            ConditionKind::RatioBelow => "Ratio below",
            ConditionKind::SeedingTimeAtLeast => "Seeding time at least",
            ConditionKind::IdleAtLeast => "Idle at least",
            ConditionKind::AgeAtLeast => "Age at least",
            ConditionKind::Complete => "Complete",
            ConditionKind::Active => "Active",
            ConditionKind::Stopped => "Stopped",
            ConditionKind::SizeAtLeast => "Size at least",
            ConditionKind::SizeAtMost => "Size at most",
            ConditionKind::SeedsAtLeast => "Seeds at least",
            ConditionKind::SeedsAtMost => "Seeds at most",
            ConditionKind::LeechersAtLeast => "Leechers at least",
            ConditionKind::LabelIs => "Label is",
            ConditionKind::TagHas => "Tag has",
            ConditionKind::TrackerIs => "Tracker is",
            ConditionKind::NameMatches => "Name matches (regex)",
            ConditionKind::AddedByRss => "Added by RSS",
            ConditionKind::FreeSpaceBelow => "Free space below",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.value() == value)
    }

    fn param_specs(self) -> &'static [ParamSpec] {
        match self {
            ConditionKind::RatioAtLeast | ConditionKind::RatioBelow => &[RATIO_PARAM],
            ConditionKind::SeedingTimeAtLeast
            | ConditionKind::IdleAtLeast
            | ConditionKind::AgeAtLeast => &[SECONDS_PARAM],
            ConditionKind::Complete | ConditionKind::Active | ConditionKind::Stopped => {
                &[BOOL_PARAM]
            }
            ConditionKind::SizeAtLeast | ConditionKind::SizeAtMost => &[SIZE_MB_PARAM],
            ConditionKind::SeedsAtLeast
            | ConditionKind::SeedsAtMost
            | ConditionKind::LeechersAtLeast => &[COUNT_PARAM],
            ConditionKind::LabelIs => &[ParamSpec {
                label: "Label",
                placeholder: "movies",
                input: ParamInput::Text,
            }],
            ConditionKind::TagHas => &[ParamSpec {
                label: "Tag",
                placeholder: "keep",
                input: ParamInput::Text,
            }],
            ConditionKind::TrackerIs => &[ParamSpec {
                label: "Host",
                placeholder: "tracker.example.org",
                input: ParamInput::Text,
            }],
            ConditionKind::NameMatches => &[ParamSpec {
                label: "Regex",
                placeholder: "show.*1080p",
                input: ParamInput::Text,
            }],
            ConditionKind::AddedByRss => &[],
            ConditionKind::FreeSpaceBelow => &[
                ParamSpec {
                    label: "MB",
                    placeholder: "e.g. 5120",
                    input: ParamInput::SizeMb,
                },
                ParamSpec {
                    label: "Path",
                    placeholder: "Torrent dir",
                    input: ParamInput::Text,
                },
            ],
        }
    }

    pub fn default_params(self) -> Vec<String> {
        self.param_specs()
            .iter()
            .map(|spec| match spec.input {
                ParamInput::Bool => "true".to_string(),
                _ => String::new(),
            })
            .collect()
    }
}

/// One editable condition row. Parameters stay strings until save/test.
#[derive(Debug, Clone)]
pub struct ConditionDraft {
    pub key: u64,
    pub kind: ConditionKind,
    pub params: Vec<String>,
}

impl ConditionDraft {
    fn param(&self, index: usize) -> &str {
        self.params.get(index).map(String::as_str).unwrap_or("")
    }

    fn to_condition(&self) -> Result<Condition, String> {
        match self.kind {
            ConditionKind::RatioAtLeast => Ok(Condition::RatioAtLeast(parse_ratio(
                self.param(0),
                "Ratio",
            )?)),
            ConditionKind::RatioBelow => {
                Ok(Condition::RatioBelow(parse_ratio(self.param(0), "Ratio")?))
            }
            ConditionKind::SeedingTimeAtLeast => Ok(Condition::SeedingTimeAtLeast(
                parse_required_u64(self.param(0), "Seconds")?,
            )),
            ConditionKind::IdleAtLeast => Ok(Condition::IdleAtLeast(parse_required_u64(
                self.param(0),
                "Seconds",
            )?)),
            ConditionKind::AgeAtLeast => Ok(Condition::AgeAtLeast(parse_required_u64(
                self.param(0),
                "Seconds",
            )?)),
            ConditionKind::Complete => Ok(Condition::Complete(parse_bool(self.param(0)))),
            ConditionKind::Active => Ok(Condition::Active(parse_bool(self.param(0)))),
            ConditionKind::Stopped => Ok(Condition::Stopped(parse_bool(self.param(0)))),
            ConditionKind::SizeAtLeast => {
                Ok(Condition::SizeAtLeast(parse_mb(self.param(0), "Size")?))
            }
            ConditionKind::SizeAtMost => {
                Ok(Condition::SizeAtMost(parse_mb(self.param(0), "Size")?))
            }
            ConditionKind::SeedsAtLeast => Ok(Condition::SeedsAtLeast(parse_required_u32(
                self.param(0),
                "Count",
            )?)),
            ConditionKind::SeedsAtMost => Ok(Condition::SeedsAtMost(parse_required_u32(
                self.param(0),
                "Count",
            )?)),
            ConditionKind::LeechersAtLeast => Ok(Condition::LeechersAtLeast(parse_required_u32(
                self.param(0),
                "Count",
            )?)),
            ConditionKind::LabelIs => {
                Ok(Condition::LabelIs(required_text(self.param(0), "Label")?))
            }
            ConditionKind::TagHas => Ok(Condition::TagHas(required_text(self.param(0), "Tag")?)),
            ConditionKind::TrackerIs => {
                Ok(Condition::TrackerIs(required_text(self.param(0), "Host")?))
            }
            ConditionKind::NameMatches => {
                // Keep the pattern verbatim (regex whitespace can be meaningful).
                let pattern = self.param(0);
                if pattern.trim().is_empty() {
                    return Err("Regex is required.".to_string());
                }
                Ok(Condition::NameMatches(pattern.to_string()))
            }
            ConditionKind::AddedByRss => Ok(Condition::AddedByRss),
            ConditionKind::FreeSpaceBelow => Ok(Condition::FreeSpaceBelow {
                bytes: parse_mb(self.param(0), "Free space")?,
                path: optional_text(self.param(1)),
            }),
        }
    }
}

/// Typed conditions → editable drafts (editor open).
pub fn condition_drafts_from(conditions: &[Condition]) -> Vec<ConditionDraft> {
    conditions
        .iter()
        .map(|condition| {
            let (kind, params) = match condition {
                Condition::RatioAtLeast(value) => {
                    (ConditionKind::RatioAtLeast, vec![format!("{value}")])
                }
                Condition::RatioBelow(value) => {
                    (ConditionKind::RatioBelow, vec![format!("{value}")])
                }
                Condition::SeedingTimeAtLeast(value) => {
                    (ConditionKind::SeedingTimeAtLeast, vec![value.to_string()])
                }
                Condition::IdleAtLeast(value) => {
                    (ConditionKind::IdleAtLeast, vec![value.to_string()])
                }
                Condition::AgeAtLeast(value) => {
                    (ConditionKind::AgeAtLeast, vec![value.to_string()])
                }
                Condition::Complete(value) => (ConditionKind::Complete, vec![value.to_string()]),
                Condition::Active(value) => (ConditionKind::Active, vec![value.to_string()]),
                Condition::Stopped(value) => (ConditionKind::Stopped, vec![value.to_string()]),
                Condition::SizeAtLeast(bytes) => {
                    (ConditionKind::SizeAtLeast, vec![bytes_to_mb(*bytes)])
                }
                Condition::SizeAtMost(bytes) => {
                    (ConditionKind::SizeAtMost, vec![bytes_to_mb(*bytes)])
                }
                Condition::SeedsAtLeast(value) => {
                    (ConditionKind::SeedsAtLeast, vec![value.to_string()])
                }
                Condition::SeedsAtMost(value) => {
                    (ConditionKind::SeedsAtMost, vec![value.to_string()])
                }
                Condition::LeechersAtLeast(value) => {
                    (ConditionKind::LeechersAtLeast, vec![value.to_string()])
                }
                Condition::LabelIs(value) => (ConditionKind::LabelIs, vec![value.clone()]),
                Condition::TagHas(value) => (ConditionKind::TagHas, vec![value.clone()]),
                Condition::TrackerIs(value) => (ConditionKind::TrackerIs, vec![value.clone()]),
                Condition::NameMatches(value) => (ConditionKind::NameMatches, vec![value.clone()]),
                Condition::AddedByRss => (ConditionKind::AddedByRss, Vec::new()),
                Condition::FreeSpaceBelow { bytes, path } => (
                    ConditionKind::FreeSpaceBelow,
                    vec![bytes_to_mb(*bytes), path.clone().unwrap_or_default()],
                ),
            };
            ConditionDraft {
                key: next_draft_key(),
                kind,
                params,
            }
        })
        .collect()
}

/// Drafts → typed conditions, with row-numbered validation errors (save/test).
pub fn conditions_from_drafts(drafts: &[ConditionDraft]) -> Result<Vec<Condition>, String> {
    drafts
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            draft
                .to_condition()
                .map_err(|err| format!("Condition {} ({}): {err}", index + 1, draft.kind.label()))
        })
        .collect()
}

fn set_param(conditions: RwSignal<Vec<ConditionDraft>>, key: u64, index: usize, value: String) {
    conditions.update(|list| {
        if let Some(draft) = list.iter_mut().find(|draft| draft.key == key) {
            if let Some(param) = draft.params.get_mut(index) {
                *param = value;
            }
        }
    });
}

/// Chips for the current conditions plus a kind select + "Add condition" row.
#[component]
pub fn ConditionBuilder(conditions: RwSignal<Vec<ConditionDraft>>) -> impl IntoView {
    let add_kind = RwSignal::new(ConditionKind::RatioAtLeast);

    view! {
        <div class="space-y-2">
            <For
                each={move || conditions.get().into_iter().map(|draft| draft.key).collect::<Vec<_>>()}
                key=|key| *key
                let:key
            >
                <ConditionChip conditions key />
            </For>
            {move || {
                conditions.get().is_empty().then(|| {
                    view! {
                        <p class="rounded-lg border border-dashed border-white/[0.08] bg-white/[0.02] px-3 py-2 text-xs text-zinc-500">
                            "No conditions yet — add at least one below."
                        </p>
                    }
                })
            }}
            <div class="flex flex-wrap items-center gap-2 pt-1">
                <select
                    class=SELECT_CLASS
                    aria-label="Condition to add"
                    prop:value=move || add_kind.get().value()
                    on:change=move |ev| {
                        if let Some(kind) = ConditionKind::from_value(&event_target_value(&ev)) {
                            add_kind.set(kind);
                        }
                    }
                >
                    {ConditionKind::ALL.iter().map(|kind| {
                        view! { <option value=kind.value()>{kind.label()}</option> }
                    }).collect_view()}
                </select>
                <Button
                    label="Add condition"
                    variant=ButtonVariant::Secondary
                    on_click=Callback::new(move |_| {
                        let kind = add_kind.get();
                        conditions.update(|list| {
                            list.push(ConditionDraft {
                                key: next_draft_key(),
                                kind,
                                params: kind.default_params(),
                            });
                        });
                    })
                />
            </div>
            <p class="text-xs text-zinc-500">
                "Swarm conditions (seeds/leechers) need scrape data; without it they do not match and the dry run reports „no data”."
            </p>
        </div>
    }
}

/// One editable chip. The kind is a `Memo` so the parameter area rebuilds only
/// when the kind actually changes — parameter keystrokes update attributes in
/// place and keep input focus.
#[component]
fn ConditionChip(conditions: RwSignal<Vec<ConditionDraft>>, key: u64) -> impl IntoView {
    let chip_kind = Memo::new(move |_| {
        conditions.with(|list| {
            list.iter()
                .find(|draft| draft.key == key)
                .map(|draft| draft.kind)
        })
    });

    view! {
        <div class="rounded-xl border border-white/[0.08] bg-white/[0.02] px-3 py-2 transition-all duration-200 hover:border-white/[0.15]">
            <div class="flex flex-wrap items-center gap-2">
                <select
                    class=CHIP_SELECT_CLASS
                    aria-label="Condition kind"
                    prop:value=move || chip_kind.get().map(|kind| kind.value()).unwrap_or_default()
                    on:change=move |ev| {
                        let Some(kind) = ConditionKind::from_value(&event_target_value(&ev)) else {
                            return;
                        };
                        conditions.update(|list| {
                            if let Some(draft) = list.iter_mut().find(|draft| draft.key == key) {
                                draft.kind = kind;
                                draft.params = kind.default_params();
                            }
                        });
                    }
                >
                    {ConditionKind::ALL.iter().map(|kind| {
                        view! { <option value=kind.value()>{kind.label()}</option> }
                    }).collect_view()}
                </select>
                <div class="flex min-w-0 flex-1 flex-wrap items-center gap-2">
                    {move || {
                        let Some(kind) = chip_kind.get() else {
                            return view! { <div></div> }.into_any();
                        };
                        // Datalists are provided once by the rule editor.
                        let datalist = match kind {
                            ConditionKind::LabelIs => Some("rule-editor-labels"),
                            ConditionKind::TagHas => Some("rule-editor-tags"),
                            _ => None,
                        };
                        kind.param_specs().iter().enumerate().map(|(index, spec)| {
                            let value = move || {
                                conditions.with(|list| {
                                    list.iter()
                                        .find(|draft| draft.key == key)
                                        .and_then(|draft| draft.params.get(index).cloned())
                                        .unwrap_or_default()
                                })
                            };
                            match spec.input {
                                ParamInput::Bool => view! {
                                    <label class="flex items-center gap-1.5 text-xs text-zinc-400">
                                        {spec.label}
                                        <select
                                            class=CHIP_SELECT_CLASS
                                            prop:value=value
                                            on:change=move |ev| set_param(conditions, key, index, event_target_value(&ev))
                                        >
                                            <option value="true">"true"</option>
                                            <option value="false">"false"</option>
                                        </select>
                                    </label>
                                }.into_any(),
                                ParamInput::Text => view! {
                                    <label class="flex items-center gap-1.5 text-xs text-zinc-400">
                                        {spec.label}
                                        <input
                                            type="text"
                                            class=CHIP_TEXT_CLASS
                                            placeholder=spec.placeholder
                                            autocomplete="off"
                                            list=datalist
                                            prop:value=value
                                            on:input=move |ev| set_param(conditions, key, index, event_target_value(&ev))
                                        />
                                    </label>
                                }.into_any(),
                                ParamInput::Number | ParamInput::SizeMb => view! {
                                    <label class="flex items-center gap-1.5 text-xs text-zinc-400">
                                        {spec.label}
                                        <input
                                            type="number"
                                            min="0"
                                            step="any"
                                            class=CHIP_NUMBER_CLASS
                                            placeholder=spec.placeholder
                                            prop:value=value
                                            on:input=move |ev| set_param(conditions, key, index, event_target_value(&ev))
                                        />
                                    </label>
                                }.into_any(),
                            }
                        }).collect_view().into_any()
                    }}
                </div>
                <button
                    type="button"
                    class="shrink-0 cursor-pointer rounded-lg p-1.5 text-zinc-500 transition-colors hover:bg-white/[0.06] hover:text-rose-300"
                    aria-label="Remove condition"
                    on:click=move |_| conditions.update(|list| list.retain(|draft| draft.key != key))
                >
                    <span aria-hidden="true">"✕"</span>
                </button>
            </div>
        </div>
    }
}
