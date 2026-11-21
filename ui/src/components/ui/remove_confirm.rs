use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::ui::{Button, ButtonVariant};
use crate::state::*;

/// Shared confirmation modal for removing torrents (single from the drawer,
/// bulk from the explorer). Offers explicit opt-in checkboxes for deleting
/// downloaded data and the source `.torrent` file; both default to unchecked
/// so the default action only removes the torrent from the client.
#[component]
pub fn RemoveConfirm() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let delete_data = RwSignal::new(false);
    let delete_source = RwSignal::new(false);

    // Reset checkboxes each time the modal opens.
    Effect::new(move |_| {
        if state.remove_confirm_open.get() {
            delete_data.set(false);
            delete_source.set(false);
        }
    });

    let close = move || {
        if !state.remove_busy.get_untracked() {
            state.remove_confirm_open.set(false);
        }
    };

    let confirm = move || {
        if state.remove_busy.get_untracked() {
            return;
        }
        let hashes = state.remove_confirm_hashes.get_untracked();
        if hashes.is_empty() {
            state.remove_confirm_open.set(false);
            return;
        }
        let dd = delete_data.get_untracked();
        let ds = delete_source.get_untracked();
        state.remove_busy.set(true);
        spawn_local(async move {
            for hash in &hashes {
                let _ = api::remove_torrent(state, hash, dd, ds).await;
            }
            state.remove_busy.set(false);
            state.remove_confirm_open.set(false);
            state.selected_hashes.set(Vec::new());
            // Close the details drawer if the removed torrent was selected.
            if let Some(sel) = state.selected_item.get_untracked() {
                if hashes.iter().any(|h| h == &sel.hash) {
                    state.selected_item.set(None);
                }
            }
            state.remove_confirm_hashes.set(Vec::new());
        });
    };

    let key_handle =
        window_event_listener(leptos::ev::keydown, move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" && state.remove_confirm_open.get_untracked() {
                close();
            }
        });
    on_cleanup(move || key_handle.remove());

    let count_label = move || {
        let n = state.remove_confirm_hashes.get().len();
        if n <= 1 {
            state.remove_confirm_label.get()
        } else {
            format!("{n} torrents")
        }
    };

    view! {
        <Show when=move || state.remove_confirm_open.get()>
            <div
                class="fixed inset-0 z-[60] flex items-center justify-center bg-black/75 backdrop-blur-sm p-4"
                on:click=move |ev| {
                    if ev.target() == ev.current_target() {
                        close();
                    }
                }
            >
                <div class="relative w-full max-w-md rounded-2xl border border-white/[0.12] bg-zinc-900 shadow-2xl p-6 text-zinc-100">
                    <h2 class="text-base font-semibold tracking-tight">"Remove torrent"</h2>
                    <p class="mt-1 break-words text-sm text-zinc-400">{count_label}</p>

                    <div class="mt-4 space-y-3">
                        <label class="flex cursor-pointer items-start gap-3 rounded-xl border border-white/[0.08] bg-white/[0.03] px-3 py-2.5 hover:bg-white/[0.05]">
                            <input
                                type="checkbox"
                                class="mt-0.5 h-4 w-4 cursor-pointer accent-rose-500"
                                prop:checked=move || delete_data.get()
                                on:change=move |ev| delete_data.set(event_target_checked(&ev))
                            />
                            <span class="text-sm">
                                <span class="font-medium text-zinc-100">"Delete downloaded data"</span>
                                <span class="block text-xs text-zinc-500">"Permanently remove this torrent's files from disk."</span>
                            </span>
                        </label>
                        <label class="flex cursor-pointer items-start gap-3 rounded-xl border border-white/[0.08] bg-white/[0.03] px-3 py-2.5 hover:bg-white/[0.05]">
                            <input
                                type="checkbox"
                                class="mt-0.5 h-4 w-4 cursor-pointer accent-rose-500"
                                prop:checked=move || delete_source.get()
                                on:change=move |ev| delete_source.set(event_target_checked(&ev))
                            />
                            <span class="text-sm">
                                <span class="font-medium text-zinc-100">"Delete source .torrent file"</span>
                                <span class="block text-xs text-zinc-500">"Remove the tied .torrent file (e.g. from the watch directory)."</span>
                            </span>
                        </label>
                    </div>

                    <p class="mt-3 text-xs text-zinc-500">
                        "Without these options the torrent is only removed from the client; data and source files stay on disk."
                    </p>

                    <div class="mt-5 flex items-center justify-end gap-2">
                        <Button
                            label="Cancel"
                            variant=ButtonVariant::Ghost
                            on_click=Callback::new(move |_| close())
                        />
                        {move || {
                            let busy = state.remove_busy.get();
                            let label = if busy { "Removing…" } else { "Remove" };
                            view! {
                                <Button
                                    label=label
                                    variant=ButtonVariant::Danger
                                    disabled=busy
                                    on_click=Callback::new(move |_| confirm())
                                />
                            }
                        }}
                    </div>
                </div>
            </div>
        </Show>
    }
}
