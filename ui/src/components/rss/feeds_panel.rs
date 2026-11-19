use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlInputElement;

use crate::components::ui::{Badge, BadgeKind, BentoCard, Button, ButtonVariant};
use crate::state::rss::{
    rss_create_feed, rss_delete_feed, rss_refresh_feed, rss_update_feed, Feed, FeedPatch,
    RssPageStore,
};
use crate::state::store::{AppState, ToastKind};

// Copied from pages/settings.rs (private there) to keep the shared input look.
const INPUT_CLASS: &str = "w-full rounded-xl bg-zinc-950/80 border border-white/[0.08] px-3.5 py-2 text-sm text-zinc-100 placeholder:text-zinc-500 focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 transition-all";

// The Button component exposes no id prop, but #rss-add-feed is a binding QA
// hook, so the add button is raw markup styled like ButtonVariant::Primary.
const ADD_BUTTON_CLASS: &str = "inline-flex items-center justify-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-medium text-white shadow-[inset_0_1px_0_0_rgba(255,255,255,0.2)] hover:brightness-110 active:scale-[0.98] transition-all cursor-pointer";

fn input_value(ev: web_sys::Event) -> Option<String> {
    ev.target()
        .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
        .map(|input| input.value())
}

fn feed_name(feed: &Feed) -> String {
    feed.alias
        .as_deref()
        .filter(|alias| !alias.is_empty())
        .unwrap_or(&feed.url)
        .to_string()
}

/// The backend stores `0` for "use the global default interval".
fn format_interval(secs: u64) -> String {
    match secs {
        0 => "default interval".to_string(),
        60 => "every minute".to_string(),
        3600 => "every hour".to_string(),
        secs if secs % 3600 == 0 => format!("every {} h", secs / 3600),
        secs if secs % 60 == 0 => format!("every {} min", secs / 60),
        secs => format!("every {secs} s"),
    }
}

fn format_timestamp(secs: u64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(secs as f64 * 1000.0));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds(),
    )
}

/// Shared handles for the per-feed actions. Everything is `Copy` (contexts and
/// signals), so rows and async handlers can capture it freely.
#[derive(Clone, Copy)]
struct FeedCtx {
    state: AppState,
    store: RssPageStore,
    busy_ids: RwSignal<Vec<u64>>,
    editing: RwSignal<Option<u64>>,
    edit_alias: RwSignal<String>,
    edit_interval: RwSignal<String>,
}

impl FeedCtx {
    fn is_busy(self, id: u64) -> bool {
        self.busy_ids.get().contains(&id)
    }

    fn set_busy(self, id: u64, busy: bool) {
        self.busy_ids.update(|ids| {
            if busy {
                ids.push(id);
            } else {
                ids.retain(|busy_id| *busy_id != id);
            }
        });
    }

    /// Always diff/toggle against the latest store snapshot, not the row's
    /// render-time copy, so concurrent reloads cannot produce stale patches.
    fn current_feed(self, id: u64) -> Option<Feed> {
        self.store
            .feeds
            .get()
            .into_iter()
            .find(|feed| feed.id == id)
    }

    fn toggle(self, id: u64) {
        if self.is_busy(id) {
            return;
        }
        let Some(feed) = self.current_feed(id) else {
            return;
        };
        let enabled = !feed.enabled;
        let patch = FeedPatch {
            url: None,
            alias: None,
            enabled: Some(enabled),
            refresh_interval_secs: None,
        };
        self.set_busy(id, true);
        spawn_local(async move {
            match rss_update_feed(&self.state, id, &patch).await {
                Ok(_) => {
                    self.state.push_toast(
                        ToastKind::Success,
                        if enabled {
                            "Feed enabled"
                        } else {
                            "Feed disabled"
                        },
                        feed_name(&feed),
                    );
                    self.store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    self.state.push_toast(
                        ToastKind::Error,
                        "Could not update feed",
                        err.to_string(),
                    );
                }
            }
            self.set_busy(id, false);
        });
    }

    fn refresh(self, id: u64) {
        if self.is_busy(id) {
            return;
        }
        self.set_busy(id, true);
        spawn_local(async move {
            match rss_refresh_feed(&self.state, id).await {
                Ok(outcome) => {
                    let summary = format!(
                        "{} new items, {} rule matches.",
                        outcome.new_items,
                        outcome.matches.len()
                    );
                    match outcome.error {
                        Some(error) => self.state.push_toast(
                            ToastKind::Warning,
                            "Feed refreshed with errors",
                            format!("{summary} {error}"),
                        ),
                        None => {
                            self.state
                                .push_toast(ToastKind::Success, "Feed refreshed", summary);
                        }
                    }
                    self.store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    self.state
                        .push_toast(ToastKind::Error, "Refresh failed", err.to_string());
                }
            }
            self.set_busy(id, false);
        });
    }

    fn delete(self, id: u64) {
        if self.is_busy(id) {
            return;
        }
        let name = self
            .current_feed(id)
            .map(|feed| feed_name(&feed))
            .unwrap_or_else(|| format!("feed {id}"));
        self.set_busy(id, true);
        spawn_local(async move {
            match rss_delete_feed(&self.state, id).await {
                Ok(()) => {
                    self.state.push_toast(
                        ToastKind::Success,
                        "Feed deleted",
                        format!("{name} was removed."),
                    );
                    if self.editing.get() == Some(id) {
                        self.editing.set(None);
                    }
                    self.store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    self.state.push_toast(
                        ToastKind::Error,
                        "Could not delete feed",
                        err.to_string(),
                    );
                }
            }
            self.set_busy(id, false);
        });
    }

    fn start_edit(self, id: u64) {
        if self.is_busy(id) {
            return;
        }
        let Some(feed) = self.current_feed(id) else {
            return;
        };
        self.edit_alias.set(feed.alias.unwrap_or_default());
        // 0 means "global default"; show it as an empty (= keep current) field.
        self.edit_interval.set(if feed.refresh_interval_secs == 0 {
            String::new()
        } else {
            feed.refresh_interval_secs.to_string()
        });
        self.editing.set(Some(id));
    }

    fn save(self, id: u64) {
        if self.is_busy(id) {
            return;
        }
        let Some(feed) = self.current_feed(id) else {
            return;
        };
        let alias = self.edit_alias.get().trim().to_string();
        let interval_text = self.edit_interval.get().trim().to_string();
        let interval = if interval_text.is_empty() {
            None
        } else {
            match interval_text.parse::<u64>() {
                Ok(secs) => Some(secs),
                Err(_) => {
                    self.state.push_toast(
                        ToastKind::Error,
                        "Invalid refresh interval",
                        "Enter a whole number of seconds, or leave the field empty.",
                    );
                    return;
                }
            }
        };
        // Only changed fields go into the patch; None means "leave unchanged".
        let mut patch = FeedPatch {
            url: None,
            alias: None,
            enabled: None,
            refresh_interval_secs: None,
        };
        if alias != feed.alias.unwrap_or_default() {
            patch.alias = Some(alias);
        }
        if interval.is_some_and(|secs| secs != feed.refresh_interval_secs) {
            patch.refresh_interval_secs = interval;
        }
        if patch.alias.is_none() && patch.refresh_interval_secs.is_none() {
            self.editing.set(None);
            return;
        }
        self.set_busy(id, true);
        spawn_local(async move {
            match rss_update_feed(&self.state, id, &patch).await {
                Ok(_) => {
                    self.state.push_toast(
                        ToastKind::Success,
                        "Feed updated",
                        "Feed settings saved.",
                    );
                    self.editing.set(None);
                    self.store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    self.state.push_toast(
                        ToastKind::Error,
                        "Could not update feed",
                        err.to_string(),
                    );
                }
            }
            self.set_busy(id, false);
        });
    }
}

fn feed_row(feed: Feed, ctx: FeedCtx) -> impl IntoView {
    let id = feed.id;
    let enabled = feed.enabled;
    let name = feed_name(&feed);
    let url = feed.url.clone();
    let interval = format_interval(feed.refresh_interval_secs);
    let last_error = feed.last_error.clone();
    let last_success = feed.last_success_at.map(format_timestamp);
    let last_fetch = feed.last_fetch_at.map(format_timestamp);

    view! {
        <div class="rounded-xl border border-white/[0.08] bg-white/[0.02] px-4 py-3 transition-all duration-200 hover:border-white/[0.15]">
            <div class="flex flex-wrap items-center gap-3">
                <div class="min-w-0 flex-1">
                    <div class="flex min-w-0 flex-wrap items-center gap-2">
                        <span class="truncate text-sm font-medium text-zinc-200" title=name.clone()>
                            {name.clone()}
                        </span>
                        {(!enabled).then(|| {
                            view! { <Badge label="Paused" kind=BadgeKind::Neutral /> }
                        })}
                        {last_error.map(|error| {
                            view! {
                                <span class="inline-flex min-w-0 items-center gap-1.5" title=error.clone()>
                                    <Badge label="Error" kind=BadgeKind::Error />
                                    <span class="max-w-[16rem] truncate text-xs text-rose-400/80">
                                        {error.clone()}
                                    </span>
                                </span>
                            }
                        })}
                    </div>
                    <div class="mt-0.5 truncate font-mono text-xs text-zinc-500" title=url.clone()>
                        {url.clone()}
                    </div>
                    <div class="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-zinc-500">
                        <span>{interval}</span>
                        {last_success.map(|at| view! { <span>"Last success: " {at}</span> })}
                        {last_fetch.map(|at| view! { <span>"Last fetch: " {at}</span> })}
                    </div>
                </div>
                <div class="flex shrink-0 flex-wrap items-center gap-2">
                    {move || {
                        let busy = ctx.is_busy(id);
                        view! {
                            <button
                                type="button"
                                role="switch"
                                aria-checked={if enabled { "true" } else { "false" }}
                                title={if enabled { "Disable this feed" } else { "Enable this feed" }}
                                disabled=busy
                                class=format!(
                                    "relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 cursor-pointer {} {}",
                                    if enabled { "bg-[var(--accent)]" } else { "bg-white/[0.15]" },
                                    if busy { "opacity-50 pointer-events-none" } else { "" },
                                )
                                on:click=move |_| ctx.toggle(id)
                            >
                                <span class=format!(
                                    "pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform duration-200 {}",
                                    if enabled { "translate-x-[22px]" } else { "translate-x-0.5" },
                                ) />
                            </button>
                        }
                    }}
                    {move || {
                        let busy = ctx.is_busy(id);
                        view! {
                            <Button
                                label="Refresh"
                                variant=ButtonVariant::Secondary
                                disabled=busy
                                on_click=Callback::new(move |_| ctx.refresh(id))
                            />
                        }
                    }}
                    {move || {
                        if ctx.editing.get() == Some(id) {
                            ().into_any()
                        } else {
                            let busy = ctx.is_busy(id);
                            view! {
                                <Button
                                    label="Edit"
                                    variant=ButtonVariant::Ghost
                                    disabled=busy
                                    on_click=Callback::new(move |_| ctx.start_edit(id))
                                />
                            }
                            .into_any()
                        }
                    }}
                    {move || {
                        let busy = ctx.is_busy(id);
                        view! {
                            <Button
                                label="Delete"
                                variant=ButtonVariant::Danger
                                disabled=busy
                                on_click=Callback::new(move |_| ctx.delete(id))
                            />
                        }
                    }}
                </div>
            </div>
            {move || {
                if ctx.editing.get() != Some(id) {
                    return ().into_any();
                }
                view! {
                    <div class="mt-3 flex flex-wrap items-end gap-3 border-t border-white/[0.08] pt-3">
                        <div class="min-w-[10rem] flex-1">
                            <label
                                for=format!("rss-feed-edit-alias-{id}")
                                class="mb-1 block text-xs font-medium text-zinc-400"
                            >
                                "Alias"
                            </label>
                            <input
                                id=format!("rss-feed-edit-alias-{id}")
                                type="text"
                                class=INPUT_CLASS
                                placeholder="Display name"
                                autocomplete="off"
                                prop:value=move || ctx.edit_alias.get()
                                on:input=move |ev| {
                                    if let Some(value) = input_value(ev) {
                                        ctx.edit_alias.set(value);
                                    }
                                }
                            />
                        </div>
                        <div class="min-w-[10rem] flex-1">
                            <label
                                for=format!("rss-feed-edit-interval-{id}")
                                class="mb-1 block text-xs font-medium text-zinc-400"
                            >
                                "Refresh interval (seconds)"
                            </label>
                            <input
                                id=format!("rss-feed-edit-interval-{id}")
                                type="text"
                                inputmode="numeric"
                                class=INPUT_CLASS
                                placeholder="Empty keeps the current interval"
                                autocomplete="off"
                                prop:value=move || ctx.edit_interval.get()
                                on:input=move |ev| {
                                    if let Some(value) = input_value(ev) {
                                        ctx.edit_interval.set(value);
                                    }
                                }
                            />
                        </div>
                        {move || {
                            let busy = ctx.is_busy(id);
                            view! {
                                <div class="flex items-center gap-2">
                                    <Button
                                        label="Save"
                                        disabled=busy
                                        on_click=Callback::new(move |_| ctx.save(id))
                                    />
                                    <Button
                                        label="Cancel"
                                        variant=ButtonVariant::Ghost
                                        on_click=Callback::new(move |_| ctx.editing.set(None))
                                    />
                                </div>
                            }
                        }}
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}

/// Feeds tab: feed list, add form, inline editing and per-feed actions.
#[component]
pub fn FeedsPanel() -> impl IntoView {
    let state = use_context::<AppState>().unwrap_or_default();
    let store = use_context::<RssPageStore>().unwrap_or_default();

    let url_draft = RwSignal::new(String::new());
    let alias_draft = RwSignal::new(String::new());
    let interval_draft = RwSignal::new(String::new());
    let adding = RwSignal::new(false);

    let ctx = FeedCtx {
        state,
        store,
        busy_ids: RwSignal::new(Vec::new()),
        editing: RwSignal::new(None),
        edit_alias: RwSignal::new(String::new()),
        edit_interval: RwSignal::new(String::new()),
    };

    let add_feed = move || {
        if adding.get() {
            return;
        }
        let url = url_draft.get().trim().to_string();
        let valid_scheme = url.starts_with("http://") || url.starts_with("https://");
        if url.is_empty() || !valid_scheme {
            state.push_toast(
                ToastKind::Error,
                "Invalid feed URL",
                "The feed URL must start with http:// or https://.",
            );
            return;
        }
        let interval_text = interval_draft.get().trim().to_string();
        let interval = if interval_text.is_empty() {
            None
        } else {
            match interval_text.parse::<u64>() {
                Ok(secs) => Some(secs),
                Err(_) => {
                    state.push_toast(
                        ToastKind::Error,
                        "Invalid refresh interval",
                        "Enter a whole number of seconds, or leave the field empty.",
                    );
                    return;
                }
            }
        };
        let alias = alias_draft.get().trim().to_string();
        let alias = (!alias.is_empty()).then_some(alias);
        adding.set(true);
        spawn_local(async move {
            match rss_create_feed(&state, &url, alias.as_deref(), interval).await {
                Ok(feed) => {
                    state.push_toast(
                        ToastKind::Success,
                        "Feed added",
                        format!("{} is now tracked.", feed_name(&feed)),
                    );
                    url_draft.set(String::new());
                    alias_draft.set(String::new());
                    interval_draft.set(String::new());
                    store.reload.update(|n| *n += 1);
                }
                Err(err) => {
                    state.push_toast(ToastKind::Error, "Could not add feed", err.to_string());
                }
            }
            adding.set(false);
        });
    };

    view! {
        <div class="space-y-4">
            <BentoCard
                title="Add feed"
                subtitle="Register an RSS feed URL to watch for new torrents.".to_string()
            >
                <div class="space-y-4">
                    <div>
                        <label
                            for="rss-feed-url"
                            class="mb-1.5 block text-sm font-medium text-zinc-200"
                        >
                            "Feed URL"
                        </label>
                        <input
                            id="rss-feed-url"
                            type="text"
                            required=true
                            class=INPUT_CLASS
                            placeholder="https://example.com/rss.xml"
                            autocomplete="off"
                            prop:value=move || url_draft.get()
                            on:input=move |ev| {
                                if let Some(value) = input_value(ev) {
                                    url_draft.set(value);
                                }
                            }
                        />
                    </div>
                    <div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
                        <div>
                            <label
                                for="rss-feed-alias"
                                class="mb-1.5 block text-sm font-medium text-zinc-200"
                            >
                                "Alias (optional)"
                            </label>
                            <input
                                id="rss-feed-alias"
                                type="text"
                                class=INPUT_CLASS
                                placeholder="Display name"
                                autocomplete="off"
                                prop:value=move || alias_draft.get()
                                on:input=move |ev| {
                                    if let Some(value) = input_value(ev) {
                                        alias_draft.set(value);
                                    }
                                }
                            />
                        </div>
                        <div>
                            <label
                                for="rss-feed-interval"
                                class="mb-1.5 block text-sm font-medium text-zinc-200"
                            >
                                "Refresh interval (optional)"
                            </label>
                            <input
                                id="rss-feed-interval"
                                type="text"
                                inputmode="numeric"
                                class=INPUT_CLASS
                                placeholder=move || {
                                    format!(
                                        "Default: {} s",
                                        store.config.get().default_refresh_interval_secs
                                    )
                                }
                                autocomplete="off"
                                prop:value=move || interval_draft.get()
                                on:input=move |ev| {
                                    if let Some(value) = input_value(ev) {
                                        interval_draft.set(value);
                                    }
                                }
                            />
                            <p class="mt-1.5 text-xs text-zinc-500">
                                "Seconds between fetches; empty uses the server default."
                            </p>
                        </div>
                    </div>
                    <div class="flex flex-wrap items-center gap-3">
                        {move || {
                            let busy = adding.get();
                            view! {
                                <button
                                    id="rss-add-feed"
                                    type="button"
                                    disabled=busy
                                    class={if busy {
                                        format!("{ADD_BUTTON_CLASS} opacity-50 pointer-events-none")
                                    } else {
                                        ADD_BUTTON_CLASS.to_string()
                                    }}
                                    on:click=move |_| add_feed()
                                >
                                    "Add feed"
                                </button>
                            }
                        }}
                        <span class="text-xs text-zinc-500">
                            "New feeds are enabled immediately."
                        </span>
                    </div>
                </div>
            </BentoCard>

            <BentoCard
                title="Feeds"
                subtitle="Toggle, refresh, edit or remove tracked feeds.".to_string()
                actions=view! {
                    {move || {
                        let count = store.feeds.get().len();
                        view! {
                            <Badge
                                label={if count == 1 {
                                    "1 feed".to_string()
                                } else {
                                    format!("{count} feeds")
                                }}
                                kind=BadgeKind::Neutral
                            />
                        }
                    }}
                }
                .into_any()
            >
                {move || {
                    let feeds = store.feeds.get();
                    if feeds.is_empty() {
                        view! {
                            <div class="rounded-xl border border-dashed border-white/[0.12] bg-white/[0.02] px-4 py-8 text-center">
                                <p class="text-sm font-medium text-zinc-300">"No feeds yet"</p>
                                <p class="mt-1 text-xs text-zinc-500">
                                    "Add an RSS feed URL above to start tracking releases."
                                </p>
                            </div>
                        }
                        .into_any()
                    } else {
                        view! {
                            <div class="space-y-3">
                                {feeds.into_iter().map(|feed| feed_row(feed, ctx)).collect_view()}
                            </div>
                        }
                        .into_any()
                    }
                }}
            </BentoCard>
        </div>
    }
}
