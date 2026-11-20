use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::{DragEvent, Event, File, HtmlInputElement};

use crate::components::ui::button::{Button, ButtonVariant};
use crate::state::api::add_torrent;
use crate::state::store::{AddTorrentPayload, AppState, ToastKind};

const INPUT_CLASS: &str = "w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all";

#[derive(Clone, Copy, PartialEq, Eq)]
enum AddMode {
    UrlOrMagnet,
    FileUpload,
}

#[component]
pub fn AddTorrentModal() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();

    let mode = RwSignal::new(AddMode::UrlOrMagnet);
    let source_input = RwSignal::new(String::new());
    let download_dir = RwSignal::new(String::new());
    let category = RwSignal::new(String::new());
    let start_immediately = RwSignal::new(true);

    let file_name = RwSignal::new(String::new());
    let file_size_str = RwSignal::new(String::new());
    let file_base64 = RwSignal::new(String::new());
    let is_dragging = RwSignal::new(false);
    let is_submitting = RwSignal::new(false);

    let close_modal = move || {
        state.add_torrent_modal_open.set(false);
        source_input.set(String::new());
        download_dir.set(String::new());
        category.set(String::new());
        file_name.set(String::new());
        file_size_str.set(String::new());
        file_base64.set(String::new());
        is_submitting.set(false);
    };

    let handle_file = move |file: File| {
        let name = file.name();
        let size = file.size() as u64;
        file_name.set(name);
        file_size_str.set(format!("{:.1} KiB", size as f64 / 1024.0));

        let reader = match web_sys::FileReader::new() {
            Ok(r) => r,
            Err(_) => return,
        };

        let reader_clone = reader.clone();
        let file_b64 = file_base64;
        let on_load =
            wasm_bindgen::closure::Closure::wrap(Box::new(move |_: web_sys::ProgressEvent| {
                if let Ok(res) = reader_clone.result() {
                    if let Some(data_url) = res.as_string() {
                        // Extract base64 part
                        if let Some((_, b64)) = data_url.split_once("base64,") {
                            file_b64.set(b64.to_string());
                        } else {
                            file_b64.set(data_url);
                        }
                    }
                }
            }) as Box<dyn FnMut(_)>);

        reader.set_onload(Some(on_load.as_ref().unchecked_ref()));
        let _ = reader.read_as_data_url(&file);
        on_load.forget();
    };

    let submit = move || {
        if is_submitting.get() {
            return;
        }

        let m = mode.get();
        let src = source_input.get().trim().to_string();
        let b64 = file_base64.get().trim().to_string();

        if m == AddMode::UrlOrMagnet && src.is_empty() {
            state.push_toast(
                ToastKind::Error,
                "Validation error",
                "Please enter a Magnet link or torrent URL",
            );
            return;
        }
        if m == AddMode::FileUpload && b64.is_empty() {
            state.push_toast(
                ToastKind::Error,
                "Validation error",
                "Please select a .torrent file to upload",
            );
            return;
        }

        is_submitting.set(true);

        let payload = AddTorrentPayload {
            source: if m == AddMode::UrlOrMagnet {
                Some(src)
            } else {
                None
            },
            torrent_file_base64: if m == AddMode::FileUpload {
                Some(b64)
            } else {
                None
            },
            download_dir: if download_dir.get().trim().is_empty() {
                None
            } else {
                Some(download_dir.get().trim().to_string())
            },
            start_immediately: start_immediately.get(),
            category: if category.get().trim().is_empty() {
                None
            } else {
                Some(category.get().trim().to_string())
            },
        };

        spawn_local(async move {
            match add_torrent(state, payload).await {
                Ok(_) => {
                    close_modal();
                }
                Err(_) => {
                    is_submitting.set(false);
                }
            }
        });
    };

    // Global Escape handler
    let window = web_sys::window();
    if let Some(win) = window {
        let close_cb = close_modal;
        let key_closure =
            wasm_bindgen::closure::Closure::wrap(Box::new(move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "Escape" && state.add_torrent_modal_open.get_untracked() {
                    close_cb();
                }
            }) as Box<dyn FnMut(_)>);
        let _ =
            win.add_event_listener_with_callback("keydown", key_closure.as_ref().unchecked_ref());
        key_closure.forget();
    }

    view! {
        <Show when=move || state.add_torrent_modal_open.get()>
            <div
                class="fixed inset-0 z-50 flex items-center justify-center bg-black/75 backdrop-blur-sm p-4 transition-all duration-200"
                on:click=move |ev| {
                    if ev.target() == ev.current_target() {
                        close_modal();
                    }
                }
            >
                <div class="relative w-full max-w-lg rounded-2xl border border-white/[0.12] bg-zinc-900 shadow-2xl p-6 text-zinc-100 animate-in fade-in zoom-in-95 duration-150">
                    <div class="flex items-center justify-between pb-4 border-b border-white/[0.08]">
                        <div class="flex items-center gap-2.5">
                            <span class="flex h-8 w-8 items-center justify-center rounded-lg bg-[var(--accent)]/15 text-[var(--accent)]">
                                <svg class="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                    <path d="M5 12h14"/>
                                    <path d="M12 5v14"/>
                                </svg>
                            </span>
                            <div>
                                <h3 class="text-base font-semibold text-zinc-100">"Add New Torrent"</h3>
                                <p class="text-xs text-zinc-400">"Add via Magnet link, URL, or upload a .torrent file"</p>
                            </div>
                        </div>
                        <button
                            type="button"
                            class="rounded-lg p-1.5 text-zinc-400 hover:bg-white/[0.08] hover:text-zinc-100 transition-colors cursor-pointer"
                            on:click=move |_| close_modal()
                        >
                            <svg class="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                <path d="M18 6 6 18M6 6l12 12"/>
                            </svg>
                        </button>
                    </div>

                    <div class="mt-4 flex gap-1 rounded-xl border border-white/[0.08] bg-zinc-950/80 p-1">
                        <button
                            type="button"
                            class=move || {
                                let active = mode.get() == AddMode::UrlOrMagnet;
                                if active {
                                    "flex-1 rounded-lg py-1.5 text-xs font-medium bg-[var(--accent)] text-white shadow-sm transition-all cursor-pointer"
                                } else {
                                    "flex-1 rounded-lg py-1.5 text-xs font-medium text-zinc-400 hover:text-zinc-200 hover:bg-white/[0.04] transition-all cursor-pointer"
                                }
                            }
                            on:click=move |_| mode.set(AddMode::UrlOrMagnet)
                        >
                            "Magnet / URL"
                        </button>
                        <button
                            type="button"
                            class=move || {
                                let active = mode.get() == AddMode::FileUpload;
                                if active {
                                    "flex-1 rounded-lg py-1.5 text-xs font-medium bg-[var(--accent)] text-white shadow-sm transition-all cursor-pointer"
                                } else {
                                    "flex-1 rounded-lg py-1.5 text-xs font-medium text-zinc-400 hover:text-zinc-200 hover:bg-white/[0.04] transition-all cursor-pointer"
                                }
                            }
                            on:click=move |_| mode.set(AddMode::FileUpload)
                        >
                            "Upload .torrent"
                        </button>
                    </div>

                    <div class="mt-4 space-y-4">
                        {move || match mode.get() {
                            AddMode::UrlOrMagnet => view! {
                                <div>
                                    <label class="block text-xs font-medium text-zinc-300 mb-1.5">
                                        "Magnet link or .torrent URL"
                                    </label>
                                    <textarea
                                        rows="3"
                                        class=INPUT_CLASS
                                        placeholder="magnet:?xt=urn:btih:... or https://example.com/torrent.torrent"
                                        prop:value=move || source_input.get()
                                        on:input=move |ev| {
                                            if let Some(target) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                                                source_input.set(target.value());
                                            } else if let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok()) {
                                                source_input.set(target.value());
                                            }
                                        }
                                    />
                                </div>
                            }.into_any(),
                            AddMode::FileUpload => {
                                let input_ref = NodeRef::<leptos::html::Input>::new();
                                view! {
                                    <div>
                                        <label class="block text-xs font-medium text-zinc-300 mb-1.5">
                                            "Select or drop .torrent file"
                                        </label>
                                        <div
                                            class=move || {
                                                let base = "flex flex-col items-center justify-center rounded-xl border-2 border-dashed p-6 text-center transition-all cursor-pointer";
                                                if is_dragging.get() {
                                                    format!("{base} border-[var(--accent)] bg-[var(--accent)]/10")
                                                } else {
                                                    format!("{base} border-white/[0.15] bg-white/[0.02] hover:border-white/[0.3] hover:bg-white/[0.04]")
                                                }
                                            }
                                            on:click=move |_| {
                                                if let Some(input) = input_ref.get() {
                                                    input.click();
                                                }
                                            }
                                            on:dragover=move |ev: DragEvent| {
                                                ev.prevent_default();
                                                is_dragging.set(true);
                                            }
                                            on:dragleave=move |ev: DragEvent| {
                                                ev.prevent_default();
                                                is_dragging.set(false);
                                            }
                                            on:drop=move |ev: DragEvent| {
                                                ev.prevent_default();
                                                is_dragging.set(false);
                                                if let Some(dt) = ev.data_transfer() {
                                                    if let Some(files) = dt.files() {
                                                        if let Some(file) = files.get(0) {
                                                            handle_file(file);
                                                        }
                                                    }
                                                }
                                            }
                                        >
                                            <input
                                                type="file"
                                                accept=".torrent"
                                                class="hidden"
                                                node_ref=input_ref
                                                on:change=move |ev: Event| {
                                                    if let Some(target) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                                                        if let Some(files) = target.files() {
                                                            if let Some(file) = files.get(0) {
                                                                handle_file(file);
                                                            }
                                                        }
                                                    }
                                                }
                                            />
                                            {move || {
                                                let name = file_name.get();
                                                if name.is_empty() {
                                                    view! {
                                                        <svg class="h-8 w-8 text-zinc-500 mb-2" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                                                            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>
                                                            <polyline points="17 8 12 3 7 8"/>
                                                            <line x1="12" y1="3" x2="12" y2="15"/>
                                                        </svg>
                                                        <p class="text-xs text-zinc-300 font-medium">"Click to browse or drag & drop .torrent file"</p>
                                                        <p class="text-[10px] text-zinc-500 mt-1">"Bencoded torrent descriptor"</p>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <svg class="h-8 w-8 text-emerald-400 mb-2" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                                                            <path d="M20 6 9 17l-5-5"/>
                                                        </svg>
                                                        <p class="text-xs text-zinc-100 font-medium truncate max-w-[280px]">{name}</p>
                                                        <p class="text-[10px] text-zinc-400 mt-0.5">{file_size_str.get()}</p>
                                                    }.into_any()
                                                }
                                            }}
                                        </div>
                                    </div>
                                }.into_any()
                            }
                        }}

                        <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
                            <div>
                                <label class="block text-xs font-medium text-zinc-300 mb-1.5">
                                    "Save Directory (optional)"
                                </label>
                                <input
                                    type="text"
                                    class=INPUT_CLASS
                                    placeholder="Default directory"
                                    prop:value=move || download_dir.get()
                                    on:input=move |ev| {
                                        if let Some(val) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).map(|i| i.value()) {
                                            download_dir.set(val);
                                        }
                                    }
                                />
                            </div>
                            <div>
                                <label class="block text-xs font-medium text-zinc-300 mb-1.5">
                                    "Category / Tag (optional)"
                                </label>
                                <input
                                    type="text"
                                    class=INPUT_CLASS
                                    placeholder="e.g. linux, iso, media"
                                    prop:value=move || category.get()
                                    on:input=move |ev| {
                                        if let Some(val) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).map(|i| i.value()) {
                                            category.set(val);
                                        }
                                    }
                                />
                            </div>
                        </div>

                        <div class="flex items-center gap-3 pt-1">
                            <label class="flex items-center gap-2 cursor-pointer select-none">
                                <input
                                    type="checkbox"
                                    class="rounded border-white/20 bg-zinc-950 text-indigo-600 focus:ring-0 focus:ring-offset-0 cursor-pointer h-4 w-4"
                                    prop:checked=move || start_immediately.get()
                                    on:change=move |ev| {
                                        if let Some(checked) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).map(|i| i.checked()) {
                                            start_immediately.set(checked);
                                        }
                                    }
                                />
                                <span class="text-xs text-zinc-300 font-medium">"Start download immediately"</span>
                            </label>
                        </div>
                    </div>

                    <div class="mt-6 flex items-center justify-end gap-3 pt-3 border-t border-white/[0.08]">
                        <Button
                            label="Cancel"
                            variant=ButtonVariant::Secondary
                            disabled=is_submitting.get()
                            on_click=Callback::new(move |_| close_modal())
                        />
                        <Button
                            label=if is_submitting.get() { "Adding..." } else { "Add Torrent" }
                            variant=ButtonVariant::Primary
                            disabled=is_submitting.get()
                            on_click=Callback::new(move |_| submit())
                        />
                    </div>
                </div>
            </div>
        </Show>
    }
}
