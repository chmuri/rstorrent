use std::sync::atomic::{AtomicU64, Ordering};

use gloo_storage::{LocalStorage, Storage};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TorrentItem {
    pub hash: String,
    pub name: String,
    pub size_bytes: i64,
    pub completed_bytes: i64,
    pub up_rate: i64,
    pub down_rate: i64,
    pub up_total: i64,
    pub down_total: i64,
    // Raw backend state: "Downloading" | "Seeding" | "Paused" | "Stopped" | "Closed" |
    // "Checking" | Error("msg").
    pub state: String,
    pub is_active: bool,
    pub is_complete: bool,
    pub is_hash_checking: bool,
    pub priority: u32,
    pub directory: String,
    pub category: String,
    pub message: String,
    pub state_changed: u64,
    pub started: u64,
    pub finished: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionStats {
    pub up_rate: i64,
    pub down_rate: i64,
    pub up_total: i64,
    pub down_total: i64,
    pub active_torrents: u32,
    pub total_torrents: u32,
    pub listen_port: u32,
    pub throttle_up_limit: i64,
    pub throttle_down_limit: i64,
    pub dht_active: bool,
    pub dht_nodes: u32,
    pub dht_torrents: u32,
    pub dht_port: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub api_version: String,
    pub build_target: String,
    pub uptime_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandResponse {
    pub success: bool,
    pub result: serde_json::Value,
    pub error: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemStatus {
    Healthy,
    Processing,
    Warning,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Warning,
    Error,
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToastMessage {
    pub id: u64,
    pub kind: ToastKind,
    pub title: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityEvent {
    pub id: u64,
    pub timestamp: u64,
    pub severity: ToastKind,
    pub message: String,
    pub hash: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accent {
    Indigo,
    Emerald,
    Rose,
    Cyan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Compact,
    Comfortable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionMode {
    Live,
    Demo,
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Table,
    Cards,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    DownRate,
    Size,
    Status,
    Updated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeRange {
    OneMin,
    FifteenMin,
    OneHour,
    OneDay,
}

impl TorrentItem {
    pub fn status(&self) -> ItemStatus {
        if self.state.starts_with("Error") {
            ItemStatus::Failed
        } else if self.state == "Downloading"
            || self.state == "Checking"
            || self.is_hash_checking
            || (self.is_active && !self.is_complete)
        {
            ItemStatus::Processing
        } else if !self.message.is_empty() {
            ItemStatus::Warning
        } else {
            ItemStatus::Healthy
        }
    }

    pub fn progress(&self) -> f64 {
        if self.size_bytes <= 0 {
            return 0.0;
        }
        (self.completed_bytes as f64 / self.size_bytes as f64).clamp(0.0, 1.0)
    }

    pub fn short_hash(&self) -> String {
        self.hash.chars().take(8).collect()
    }

    pub fn status_label(&self) -> String {
        if self.state.starts_with("Error") {
            return "Error".to_string();
        }
        match self.state.as_str() {
            "Downloading" => "Downloading".to_string(),
            "Seeding" => "Seeding".to_string(),
            "Paused" => "Paused".to_string(),
            "Stopped" => "Stopped".to_string(),
            "Closed" => "Closed".to_string(),
            "Checking" => "Checking".to_string(),
            other => other.to_string(),
        }
    }
}

pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let sign = if bytes < 0 { "-" } else { "" };
    let mut value = (bytes as f64).abs();
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let text = if unit == 0 {
        format!("{value:.0}")
    } else {
        let mut rendered = format!("{value:.1}");
        if rendered.ends_with(".0") {
            rendered.truncate(rendered.len() - 2);
        }
        rendered
    };
    format!("{sign}{text} {}", UNITS[unit])
}

pub fn format_rate(bytes_per_sec: i64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

pub fn format_duration(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

pub fn format_timestamp(unix_millis: u64) -> String {
    let date = js_sys::Date::new_0();
    date.set_time(unix_millis as f64);
    format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}

pub fn format_percent(ratio: f64) -> String {
    format!("{:.1}%", ratio * 100.0)
}

pub fn accent_hex(accent: Accent) -> &'static str {
    match accent {
        Accent::Indigo => "#4f46e5",
        Accent::Emerald => "#059669",
        Accent::Rose => "#e11d48",
        Accent::Cyan => "#0891b2",
    }
}

impl Default for VersionInfo {
    fn default() -> Self {
        Self {
            version: "v0.22.0".to_string(),
            api_version: "1".to_string(),
            build_target: "wasm32-unknown-unknown".to_string(),
            uptime_seconds: 0,
        }
    }
}

static NEXT_TOAST_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_EVENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
pub struct AppState {
    pub items: RwSignal<Vec<TorrentItem>>,
    pub selected_item: RwSignal<Option<TorrentItem>>,
    pub search_query: RwSignal<String>,
    pub status_filter: RwSignal<String>,
    pub cmd_palette_open: RwSignal<bool>,
    pub sidebar_collapsed: RwSignal<bool>,
    pub live_stream_active: RwSignal<bool>,
    pub toasts: RwSignal<Vec<ToastMessage>>,
    pub theme: RwSignal<Theme>,
    pub accent: RwSignal<Accent>,
    pub density: RwSignal<Density>,
    pub animations_enabled: RwSignal<bool>,
    pub connection: RwSignal<ConnectionMode>,
    pub api_base_url: RwSignal<String>,
    pub api_token: RwSignal<String>,
    pub poll_interval_ms: RwSignal<u64>,
    pub stats: RwSignal<SessionStats>,
    pub version: RwSignal<VersionInfo>,
    pub down_history: RwSignal<Vec<f64>>,
    pub up_history: RwSignal<Vec<f64>>,
    pub events: RwSignal<Vec<ActivityEvent>>,
    pub view_mode: RwSignal<ViewMode>,
    pub sort_key: RwSignal<SortKey>,
    pub sort_desc: RwSignal<bool>,
    pub time_range: RwSignal<TimeRange>,
    pub selected_hashes: RwSignal<Vec<String>>,
}

impl AppState {
    pub fn new() -> Self {
        let theme = match LocalStorage::get::<String>("rt-ui.theme").ok().as_deref() {
            Some("light") => Theme::Light,
            _ => Theme::Dark,
        };
        let accent = match LocalStorage::get::<String>("rt-ui.accent").ok().as_deref() {
            Some("emerald") => Accent::Emerald,
            Some("rose") => Accent::Rose,
            Some("cyan") => Accent::Cyan,
            _ => Accent::Indigo,
        };
        let density = match LocalStorage::get::<String>("rt-ui.density").ok().as_deref() {
            Some("compact") => Density::Compact,
            _ => Density::Comfortable,
        };
        let animations_enabled = LocalStorage::get::<bool>("rt-ui.animations").unwrap_or(true);
        let sidebar_collapsed =
            LocalStorage::get::<bool>("rt-ui.sidebar_collapsed").unwrap_or(false);
        let api_base_url = LocalStorage::get::<String>("rt-ui.api_base_url").unwrap_or_default();
        let api_token = LocalStorage::get::<String>("rt-ui.api_token").unwrap_or_default();
        let poll_interval_ms = LocalStorage::get::<u64>("rt-ui.poll_interval_ms").unwrap_or(2000);

        Self {
            items: RwSignal::new(Vec::new()),
            selected_item: RwSignal::new(None),
            search_query: RwSignal::new(String::new()),
            status_filter: RwSignal::new("all".to_string()),
            cmd_palette_open: RwSignal::new(false),
            sidebar_collapsed: RwSignal::new(sidebar_collapsed),
            live_stream_active: RwSignal::new(true),
            toasts: RwSignal::new(Vec::new()),
            theme: RwSignal::new(theme),
            accent: RwSignal::new(accent),
            density: RwSignal::new(density),
            animations_enabled: RwSignal::new(animations_enabled),
            connection: RwSignal::new(ConnectionMode::Offline),
            api_base_url: RwSignal::new(api_base_url),
            api_token: RwSignal::new(api_token),
            poll_interval_ms: RwSignal::new(poll_interval_ms),
            stats: RwSignal::new(SessionStats::default()),
            version: RwSignal::new(VersionInfo::default()),
            down_history: RwSignal::new(Vec::new()),
            up_history: RwSignal::new(Vec::new()),
            events: RwSignal::new(Vec::new()),
            view_mode: RwSignal::new(ViewMode::Table),
            sort_key: RwSignal::new(SortKey::DownRate),
            sort_desc: RwSignal::new(true),
            time_range: RwSignal::new(TimeRange::FifteenMin),
            selected_hashes: RwSignal::new(Vec::new()),
        }
    }

    pub fn push_toast(self, kind: ToastKind, title: impl Into<String>, message: impl Into<String>) {
        let id = NEXT_TOAST_ID.fetch_add(1, Ordering::Relaxed);
        let toast = ToastMessage {
            id,
            kind,
            title: title.into(),
            message: message.into(),
        };
        self.toasts.update(|toasts| {
            toasts.push(toast);
            while toasts.len() > 5 {
                toasts.remove(0);
            }
        });
        let toasts = self.toasts;
        spawn_local(async move {
            TimeoutFuture::new(4500).await;
            toasts.update(|list| list.retain(|toast| toast.id != id));
        });
    }

    pub fn push_event(self, severity: ToastKind, message: impl Into<String>, hash: Option<String>) {
        let id = NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed);
        let event = ActivityEvent {
            id,
            timestamp: js_sys::Date::now() as u64,
            severity,
            message: message.into(),
            hash,
        };
        self.events.update(|events| {
            events.push(event);
            if events.len() > 60 {
                let excess = events.len() - 60;
                events.drain(0..excess);
            }
        });
    }

    pub fn filtered_items(self) -> Vec<TorrentItem> {
        let query = self.search_query.get().to_lowercase();
        let filter = self.status_filter.get();
        let mut items: Vec<TorrentItem> = self
            .items
            .get()
            .into_iter()
            .filter(|item| {
                let matches_query = query.is_empty()
                    || item.name.to_lowercase().contains(&query)
                    || item.hash.to_lowercase().contains(&query)
                    || item.category.to_lowercase().contains(&query);
                let matches_status = match filter.as_str() {
                    "healthy" => item.status() == ItemStatus::Healthy,
                    "processing" => item.status() == ItemStatus::Processing,
                    "warning" => item.status() == ItemStatus::Warning,
                    "failed" => item.status() == ItemStatus::Failed,
                    _ => true,
                };
                matches_query && matches_status
            })
            .collect();

        let sort_key = self.sort_key.get();
        let descending = self.sort_desc.get();
        items.sort_by(|a, b| {
            let ordering = match sort_key {
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::DownRate => a.down_rate.cmp(&b.down_rate),
                SortKey::Size => a.size_bytes.cmp(&b.size_bytes),
                SortKey::Status => status_rank(a.status()).cmp(&status_rank(b.status())),
                SortKey::Updated => a.state_changed.cmp(&b.state_changed),
            };
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
        items
    }

    pub fn persist_settings(self) {
        let theme = match self.theme.get() {
            Theme::Dark => "dark",
            Theme::Light => "light",
        };
        let accent = match self.accent.get() {
            Accent::Indigo => "indigo",
            Accent::Emerald => "emerald",
            Accent::Rose => "rose",
            Accent::Cyan => "cyan",
        };
        let density = match self.density.get() {
            Density::Compact => "compact",
            Density::Comfortable => "comfortable",
        };
        let _ = LocalStorage::set("rt-ui.theme", theme);
        let _ = LocalStorage::set("rt-ui.accent", accent);
        let _ = LocalStorage::set("rt-ui.density", density);
        let _ = LocalStorage::set("rt-ui.animations", self.animations_enabled.get());
        let _ = LocalStorage::set("rt-ui.sidebar_collapsed", self.sidebar_collapsed.get());
        let _ = LocalStorage::set("rt-ui.api_base_url", self.api_base_url.get());
        let _ = LocalStorage::set("rt-ui.api_token", self.api_token.get());
        let _ = LocalStorage::set("rt-ui.poll_interval_ms", self.poll_interval_ms.get());
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn status_rank(status: ItemStatus) -> u8 {
    match status {
        ItemStatus::Healthy => 0,
        ItemStatus::Processing => 1,
        ItemStatus::Warning => 2,
        ItemStatus::Failed => 3,
    }
}
