use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::state::*;

fn toast_icon(kind: ToastKind) -> AnyView {
    match kind {
        ToastKind::Success => view! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                <circle cx="12" cy="12" r="10" />
                <path d="m9 12 2 2 4-4" />
            </svg>
        }
        .into_any(),
        ToastKind::Warning => view! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
                <path d="M12 9v4" />
                <path d="M12 17h.01" />
            </svg>
        }
        .into_any(),
        ToastKind::Error => view! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                <circle cx="12" cy="12" r="10" />
                <path d="m15 9-6 6" />
                <path d="m9 9 6 6" />
            </svg>
        }
        .into_any(),
        ToastKind::Info => view! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
                <circle cx="12" cy="12" r="10" />
                <path d="M12 16v-4" />
                <path d="M12 8h.01" />
            </svg>
        }
        .into_any(),
    }
}

#[component]
pub fn ToastContainer() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    view! {
        <div class="fixed bottom-5 right-5 z-50 flex flex-col gap-2.5" aria-live="polite">
            <For each=move || state.toasts.get() key=|toast| toast.id let:toast>
                <ToastCard toast=toast.clone() />
            </For>
        </div>
    }
}

#[component]
fn ToastCard(toast: ToastMessage) -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let id = toast.id;
    let entered = RwSignal::new(false);
    // one frame gap so the enter transition actually animates
    spawn_local(async move {
        TimeoutFuture::new(16).await;
        entered.set(true);
    });
    let (icon, accent, border) = match toast.kind {
        ToastKind::Success => (
            toast_icon(ToastKind::Success),
            "text-emerald-400",
            "border-emerald-500/20",
        ),
        ToastKind::Warning => (
            toast_icon(ToastKind::Warning),
            "text-amber-400",
            "border-amber-500/20",
        ),
        ToastKind::Error => (
            toast_icon(ToastKind::Error),
            "text-rose-400",
            "border-rose-500/20",
        ),
        ToastKind::Info => (
            toast_icon(ToastKind::Info),
            "text-indigo-400",
            "border-indigo-500/20",
        ),
    };
    view! {
        <div
            class=move || format!(
                "pointer-events-auto flex w-80 cursor-pointer items-start gap-3 rounded-xl border bg-zinc-900/90 px-4 py-3 shadow-[0_8px_32px_rgba(0,0,0,0.4)] backdrop-blur-xl transition-all duration-200 ease-out {} {}",
                border,
                if entered.get() { "translate-y-0 opacity-100" } else { "translate-y-2 opacity-0" }
            )
            on:click=move |_| state.toasts.update(|toasts| toasts.retain(|t| t.id != id))
            role="status"
        >
            <span class=format!("mt-0.5 shrink-0 {accent}")>{icon}</span>
            <div class="min-w-0 flex-1">
                <p class="text-sm font-semibold text-zinc-100">{toast.title}</p>
                <p class="mt-0.5 text-xs leading-relaxed text-zinc-400">{toast.message}</p>
            </div>
        </div>
    }
}
