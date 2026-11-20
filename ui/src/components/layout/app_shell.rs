use leptos::prelude::*;

use super::{Header, Sidebar};
use crate::components::ui::{AddTorrentModal, CommandPalette, SlideOverDrawer, ToastContainer};
use crate::state::*;

#[component]
pub fn AppShell(children: Children) -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();

    view! {
        <div
            data-theme=move || match state.theme.get() {
                Theme::Dark => "dark",
                Theme::Light => "light",
            }
            data-density=move || match state.density.get() {
                Density::Comfortable => "comfortable",
                Density::Compact => "compact",
            }
            data-animations=move || if state.animations_enabled.get() { "on" } else { "off" }
            style=move || format!("--accent: {}", accent_hex(state.accent.get()))
            class=move || match state.theme.get() {
                Theme::Dark => {
                    "min-h-screen bg-[#09090B] bg-grid-pattern text-zinc-100 antialiased selection:bg-indigo-500/30 selection:text-indigo-200"
                }
                Theme::Light => {
                    "min-h-screen bg-[#FAFAFA] bg-grid-pattern text-zinc-900 antialiased selection:bg-indigo-500/30 selection:text-indigo-200"
                }
            }
        >
            <div
                aria-hidden="true"
                class="pointer-events-none fixed -top-40 -left-40 h-96 w-96 rounded-full bg-indigo-600/15 blur-[128px]"
            />
            <div
                aria-hidden="true"
                class="pointer-events-none fixed -bottom-40 -right-40 h-96 w-96 rounded-full bg-violet-600/10 blur-[128px]"
            />
            <div class="relative flex h-screen">
                <Sidebar/>
                <div class="flex min-w-0 flex-1 flex-col">
                    <Header/>
                    <main class="flex-1 overflow-y-auto px-6 py-6">{children()}</main>
                </div>
            </div>
            <CommandPalette/>
            <AddTorrentModal/>
            <SlideOverDrawer/>
            <ToastContainer/>
        </div>
    }
}
