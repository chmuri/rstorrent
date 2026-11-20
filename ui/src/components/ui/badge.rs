use leptos::prelude::*;

use crate::state::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeKind {
    Success,
    Warning,
    Error,
    Info,
    Neutral,
    Checking,
}

pub fn status_kind(status: ItemStatus) -> BadgeKind {
    match status {
        ItemStatus::Completed => BadgeKind::Success,
        ItemStatus::Downloading => BadgeKind::Info,
        ItemStatus::Checking => BadgeKind::Checking,
        ItemStatus::Warning => BadgeKind::Warning,
        ItemStatus::Failed => BadgeKind::Error,
    }
}

#[component]
pub fn Badge(
    #[prop(into)] label: String,
    kind: BadgeKind,
    #[prop(default = false)] pulse: bool,
) -> impl IntoView {
    let (pill, dot) = match kind {
        BadgeKind::Success => (
            "bg-emerald-500/10 text-emerald-400 border-emerald-500/20",
            "bg-emerald-400",
        ),
        BadgeKind::Warning => (
            "bg-amber-500/10 text-amber-400 border-amber-500/20",
            "bg-amber-400",
        ),
        BadgeKind::Error => (
            "bg-rose-500/10 text-rose-400 border-rose-500/20",
            "bg-rose-400",
        ),
        BadgeKind::Info => (
            "bg-indigo-500/10 text-indigo-400 border-indigo-500/20",
            "bg-indigo-400",
        ),
        BadgeKind::Neutral => (
            "bg-zinc-500/10 text-zinc-400 border-zinc-500/20",
            "bg-zinc-400",
        ),
        BadgeKind::Checking => (
            "bg-cyan-500/15 text-cyan-300 border-cyan-400/40 shadow-[0_0_10px_rgba(6,182,212,0.35)]",
            "bg-cyan-400",
        ),
    };

    view! {
        <span class=format!(
            "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium {pill}",
        )>
            {pulse.then(|| {
                view! {
                    <span class="relative flex h-1.5 w-1.5">
                        <span class=format!(
                            "absolute inline-flex h-full w-full animate-ping rounded-full opacity-75 {dot}",
                        ) />
                        <span class=format!(
                            "relative inline-flex h-1.5 w-1.5 rounded-full {dot}",
                        ) />
                    </span>
                }
            })}
            {label}
        </span>
    }
}
