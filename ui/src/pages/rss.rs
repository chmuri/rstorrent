//! RSS page: tabbed shell around the RSS panels with a shared reload effect,
//! 15 s polling and the global refresh / enabled actions.

use gloo_timers::callback::Interval;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::rss::{FeedsPanel, HistoryPanel, ItemsPanel, RulesPanel};
use crate::components::ui::{Button, ButtonVariant};
use crate::state::api::ApiError;
use crate::state::rss::{
    rss_get_settings, rss_list_feeds, rss_list_history, rss_list_rules, rss_refresh_all,
    rss_save_settings, RssPageStore, RssTab,
};
use crate::state::{AppState, ConnectionMode, ToastKind};

const POLL_INTERVAL_MS: u32 = 15_000;

// Copied from pages/settings.rs (private there) to keep the segment look.
const SEGMENT_BASE: &str = "inline-flex items-center gap-1.5 rounded-lg px-3.5 py-1.5 text-sm font-medium transition-all duration-200 cursor-pointer";

fn segment_class(active: bool) -> &'static str {
    if active {
        "bg-[var(--accent)] text-white shadow-[inset_0_1px_0_0_rgba(255,255,255,0.2)]"
    } else {
        "text-zinc-400 hover:text-zinc-100 hover:bg-white/[0.04]"
    }
}

fn tab_button(store: RssPageStore, tab: RssTab, label: &'static str) -> impl IntoView {
    view! {
        <button
            type="button"
            aria-pressed=move || if store.tab.get() == tab { "true" } else { "false" }
            class=move || format!("{SEGMENT_BASE} {}", segment_class(store.tab.get() == tab))
            on:click=move |_| store.tab.set(tab)
        >
            {label}
        </button>
    }
}

/// RSS page: provides the shared store, loads feeds/rules/history/config and
/// mounts the active tab panel.
#[component]
pub fn RssPage() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = RssPageStore::new();
    // Panels read the store from context, so it must exist before they render.
    provide_context(store);

    let saving_config = RwSignal::new(false);
    let request_gen = RwSignal::new(0u64);

    Effect::new(move |_| {
        let _reload = store.reload.get();
        let live = state.connection.get() == ConnectionMode::Live;
        // Invalidate in-flight responses on every run, including going offline.
        request_gen.update(|gen| *gen += 1);
        if !live {
            return;
        }
        let gen = request_gen.get_untracked();
        // Items are deliberately not fetched here: ItemsPanel owns item loading
        // and pagination, and a page-level fetch would fight its offset.
        spawn_local(async move {
            let load = async {
                let feeds = rss_list_feeds(&state).await?;
                let rules = rss_list_rules(&state).await?;
                let history = rss_list_history(&state, None).await?;
                let config = rss_get_settings(&state).await?;
                Ok::<_, ApiError>((feeds, rules, history, config))
            };
            match load.await {
                Ok((feeds, rules, history, config)) => {
                    // Drop stale responses superseded by a newer run.
                    if request_gen.get_untracked() == gen {
                        store.feeds.set(feeds);
                        store.rules.set(rules);
                        store.history.set(history);
                        store.config.set(config);
                    }
                }
                Err(err) => {
                    if request_gen.get_untracked() == gen {
                        state.push_toast(
                            ToastKind::Error,
                            "Could not load RSS data",
                            err.to_string(),
                        );
                    }
                }
            }
        });
    });

    // Interval owns a JS closure and is !Send, so it needs thread-local storage.
    let poll_interval: RwSignal<Option<Interval>, LocalStorage> = RwSignal::new_local(None);
    poll_interval.set(Some(Interval::new(POLL_INTERVAL_MS, move || {
        if state.connection.get_untracked() == ConnectionMode::Live {
            store.reload.update(|n| *n += 1);
        }
    })));
    // Replacing the handle drops (and cancels) the interval on unmount.
    on_cleanup(move || poll_interval.set(None));

    let refresh_all = move || {
        if store.busy.get() {
            return;
        }
        store.busy.set(true);
        spawn_local(async move {
            match rss_refresh_all(&state).await {
                Ok(outcomes) => {
                    let new_items: usize = outcomes.iter().map(|o| o.new_items).sum();
                    let matches: usize = outcomes.iter().map(|o| o.matches.len()).sum();
                    let errors = outcomes.iter().filter(|o| o.error.is_some()).count();
                    let summary = format!(
                        "{new_items} new items, {matches} rule matches, {errors} errors across {} feeds.",
                        outcomes.len()
                    );
                    match outcomes.iter().find_map(|o| o.error.as_deref()) {
                        Some(first_error) => state.push_toast(
                            ToastKind::Warning,
                            "Refresh finished with errors",
                            format!("{summary} First error: {first_error}"),
                        ),
                        None => {
                            state.push_toast(ToastKind::Success, "All feeds refreshed", summary);
                        }
                    }
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Refresh failed", err.to_string());
                }
            }
            store.busy.set(false);
        });
    };

    let toggle_enabled = move || {
        if saving_config.get() {
            return;
        }
        let mut config = store.config.get();
        config.enabled = !config.enabled;
        saving_config.set(true);
        spawn_local(async move {
            match rss_save_settings(&state, &config, true).await {
                Ok(saved) => {
                    let enabled = saved.enabled;
                    store.config.set(saved);
                    state.push_toast(
                        ToastKind::Success,
                        "RSS settings saved",
                        if enabled {
                            "RSS auto-downloads are enabled."
                        } else {
                            "RSS auto-downloads are disabled."
                        },
                    );
                }
                Err(err) => {
                    // The store keeps the old config, so the switch stays in
                    // its previous position after a failed save.
                    state.push_toast(
                        ToastKind::Error,
                        "Could not save RSS settings",
                        err.to_string(),
                    );
                }
            }
            saving_config.set(false);
        });
    };

    view! {
        <div>
            <div class="mb-6 flex flex-wrap items-start justify-between gap-4">
                <div>
                    <h1 class="text-2xl font-semibold tracking-tight">"RSS"</h1>
                    <p class="mt-1 text-sm text-zinc-500">
                        "Track RSS feeds and auto-download matching torrents."
                    </p>
                </div>
                <div class="flex flex-wrap items-center gap-3">
                    <div class="flex items-center gap-2">
                        <span class="text-xs font-medium text-zinc-400">"RSS enabled"</span>
                        {move || {
                            let enabled = store.config.get().enabled;
                            let busy = saving_config.get();
                            view! {
                                <button
                                    type="button"
                                    role="switch"
                                    aria-checked={if enabled { "true" } else { "false" }}
                                    title={if enabled { "Disable RSS" } else { "Enable RSS" }}
                                    disabled=busy
                                    class=format!(
                                        "relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 cursor-pointer {} {}",
                                        if enabled { "bg-[var(--accent)]" } else { "bg-white/[0.15]" },
                                        if busy { "opacity-50 pointer-events-none" } else { "" },
                                    )
                                    on:click=move |_| toggle_enabled()
                                >
                                    <span class=format!(
                                        "pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform duration-200 {}",
                                        if enabled { "translate-x-[22px]" } else { "translate-x-0.5" },
                                    ) />
                                </button>
                            }
                        }}
                    </div>
                    // Button props are plain values, so re-render on busy changes.
                    {move || {
                        let busy = store.busy.get();
                        view! {
                            <Button
                                label={if busy { "Refreshing..." } else { "Refresh all" }}
                                variant=ButtonVariant::Secondary
                                disabled=busy
                                on_click=Callback::new(move |_| refresh_all())
                            />
                        }
                    }}
                </div>
            </div>

            <Show
                when=move || state.connection.get() == ConnectionMode::Live
                fallback=move || {
                    view! {
                        <div class="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-white/[0.12] bg-white/[0.02] px-6 py-12 text-center">
                            <p class="text-sm font-medium text-zinc-300">
                                "Backend offline — RSS requires a live connection"
                            </p>
                        </div>
                    }
                }
            >
                <div class="mb-4 inline-flex gap-1 rounded-xl border border-white/[0.08] bg-zinc-950/80 p-1">
                    {tab_button(store, RssTab::Feeds, "Feeds")}
                    {tab_button(store, RssTab::Items, "Items")}
                    {tab_button(store, RssTab::Rules, "Rules")}
                    {tab_button(store, RssTab::History, "History")}
                </div>
                {move || match store.tab.get() {
                    RssTab::Feeds => view! { <FeedsPanel /> }.into_any(),
                    RssTab::Items => view! { <ItemsPanel /> }.into_any(),
                    RssTab::Rules => view! { <RulesPanel /> }.into_any(),
                    RssTab::History => view! { <HistoryPanel /> }.into_any(),
                }}
            </Show>
        </div>
    }
}
