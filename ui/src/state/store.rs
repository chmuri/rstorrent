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
    #[serde(default)]
    pub seeds_connected: u32,
    #[serde(default)]
    pub seeds_total: u32,
    #[serde(default)]
    pub peers_connected: u32,
    #[serde(default)]
    pub peers_total: u32,
    #[serde(default)]
    pub chunks_hashed: u32,
    #[serde(default)]
    pub chunks_total: u32,
    #[serde(default)]
    pub hash_progress_pct: f32,
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

/// Per-tracker health snapshot from `GET /api/torrents/{hash}/trackers`.
/// Mirrors `webui::rest::TrackerStatus`; timestamps are unix seconds (0 = never).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackerInfo {
    pub url: String,
    pub tracker_type: String,
    pub group: u32,
    pub is_enabled: bool,
    pub is_usable: bool,
    pub is_in_use: bool,
    pub is_busy: bool,
    pub latest_event: String,
    pub success_counter: u32,
    pub failed_counter: u32,
    pub normal_interval: u64,
    pub min_interval: u64,
    pub success_time_last: u64,
    pub failed_time_last: u64,
    pub activity_time_next: u64,
    pub scrape_complete: u32,
    pub scrape_incomplete: u32,
    pub scrape_downloaded: u32,
    pub scrape_counter: u32,
    pub scrape_failed_counter: u32,
}

impl TrackerInfo {
    /// Short host:port label for compact display.
    pub fn host(&self) -> String {
        let rest = self
            .url
            .split_once("://")
            .map(|(_, r)| r)
            .unwrap_or(&self.url);
        rest.split('/').next().unwrap_or(rest).to_string()
    }

    /// Health classification for the status dot/badge.
    pub fn health(&self) -> &'static str {
        if !self.is_enabled {
            "disabled"
        } else if self.is_busy {
            "busy"
        } else if self.failed_counter > 0 && self.success_counter == 0 {
            "failed"
        } else if self.success_counter > 0 {
            "ok"
        } else {
            "pending"
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandResponse {
    pub success: bool,
    pub result: serde_json::Value,
    pub error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkTomlConfig {
    pub port_range: Option<String>,
    pub port: Option<u16>,
    pub bind_address: Option<String>,
    pub local_address: Option<String>,
    pub enable_upnp: Option<bool>,
    pub enable_utp: Option<bool>,
    pub proxy: Option<String>,
    pub download_throttle_kb: Option<u64>,
    pub upload_throttle_kb: Option<u64>,
    pub max_uploads: Option<u32>,
    pub max_downloads: Option<u32>,
    pub max_peers: Option<u32>,
    pub max_peers_seed: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageTomlConfig {
    pub directory: Option<String>,
    pub session: Option<String>,
    pub watch_directory: Option<String>,
    pub disk_threads: Option<usize>,
    pub max_open_files: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProtocolTomlConfig {
    pub encryption: Option<String>,
    pub peer_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RpcTomlConfig {
    pub scgi_port: Option<u16>,
    pub scgi_socket: Option<String>,
    pub grpc_port: Option<u16>,
    pub grpc_socket: Option<String>,
    pub scgi_token: Option<String>,
    pub grpc_token: Option<String>,
    pub grpc_tls_cert: Option<String>,
    pub grpc_tls_key: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DhtTomlConfig {
    pub mode: Option<String>,
    pub port: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LoggingTomlConfig {
    pub level: Option<String>,
    pub file: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricsTomlConfig {
    pub addr: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WebuiTomlConfig {
    pub addr: Option<String>,
    pub token: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TierTomlItem {
    pub name: String,
    pub path: String,
    pub capacity_gb: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TierRuleTomlItem {
    pub from: u8,
    pub to: u8,
    pub min_ratio: Option<f64>,
    pub time_on_tier_secs: Option<u64>,
    pub is_complete: Option<bool>,
    pub min_uploaded_bytes: Option<u64>,
    pub inactive_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TieringTomlConfig {
    pub enabled: Option<bool>,
    pub check_interval: Option<u64>,
    #[serde(default)]
    pub tiers: Vec<TierTomlItem>,
    #[serde(default)]
    pub rules: Vec<TierRuleTomlItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RssTomlConfig {
    pub enabled: Option<bool>,
    pub refresh_interval_secs: Option<u64>,
    pub max_items_per_feed: Option<usize>,
    pub request_timeout_secs: Option<u64>,
    pub max_response_mb: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default)]
    pub network: NetworkTomlConfig,
    #[serde(default)]
    pub storage: StorageTomlConfig,
    #[serde(default)]
    pub protocol: ProtocolTomlConfig,
    #[serde(default)]
    pub rpc: RpcTomlConfig,
    #[serde(default)]
    pub dht: DhtTomlConfig,
    #[serde(default)]
    pub logging: LoggingTomlConfig,
    #[serde(default)]
    pub metrics: MetricsTomlConfig,
    #[serde(default)]
    pub webui: WebuiTomlConfig,
    #[serde(default)]
    pub tiering: TieringTomlConfig,
    #[serde(default)]
    pub rss: RssTomlConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfigResponse {
    pub config: DaemonConfig,
    pub toml: String,
    pub config_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AddTorrentPayload {
    pub source: Option<String>,
    pub torrent_file_base64: Option<String>,
    pub download_dir: Option<String>,
    pub start_immediately: bool,
    pub category: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AddTorrentResult {
    pub success: bool,
    pub hash: String,
    pub name: String,
    pub error: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemStatus {
    Completed,
    Downloading,
    Checking,
    Warning,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: u64,
    pub timestamp: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "details")]
pub enum DaemonLifecycle {
    Initializing {
        phase: String,
        step: u32,
        total_steps: u32,
        current_item: Option<String>,
    },
    Ready,
    ShuttingDown,
    Unknown,
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
    UpRate,
    Size,
    Progress,
    Status,
    Category,
    Updated,
    Seeds,
    Peers,
    Ratio,
    Eta,
    Finished,
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
        } else if self.is_hash_checking || self.state == "Checking" {
            ItemStatus::Checking
        } else if self.state == "Downloading" || (self.is_active && !self.is_complete) {
            ItemStatus::Downloading
        } else if !self.message.is_empty() {
            ItemStatus::Warning
        } else {
            ItemStatus::Completed
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

    pub fn ratio(&self) -> f64 {
        if self.down_total > 0 {
            (self.up_total as f64 / self.down_total as f64).max(0.0)
        } else if self.completed_bytes > 0 {
            (self.up_total as f64 / self.completed_bytes as f64).max(0.0)
        } else {
            0.0
        }
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        if self.down_rate > 0 && self.completed_bytes < self.size_bytes {
            Some(((self.size_bytes - self.completed_bytes) / self.down_rate) as u64)
        } else {
            None
        }
    }

    pub fn format_eta(&self) -> String {
        match self.eta_seconds() {
            None => {
                if self.is_complete || self.status() == ItemStatus::Completed {
                    "Done".to_string()
                } else {
                    "—".to_string()
                }
            }
            Some(s) if s > 86400 => format!("{}d", s / 86400),
            Some(s) if s > 3600 => format!("{:02}h {:02}m", s / 3600, (s % 3600) / 60),
            Some(s) => format!("{:02}m {:02}s", s / 60, s % 60),
        }
    }

    pub fn format_completed(&self) -> String {
        if self.finished == 0 {
            "—".to_string()
        } else {
            format_timestamp(self.finished * 1000)
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
            version: "v0.23.0".to_string(),
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
    pub add_torrent_modal_open: RwSignal<bool>,
    pub logs: RwSignal<Vec<LogEntry>>,
    pub daemon_lifecycle: RwSignal<DaemonLifecycle>,
    pub log_drawer_open: RwSignal<bool>,
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
            add_torrent_modal_open: RwSignal::new(false),
            logs: RwSignal::new(Vec::new()),
            daemon_lifecycle: RwSignal::new(DaemonLifecycle::Unknown),
            log_drawer_open: RwSignal::new(false),
        }
    }

    pub fn push_log(self, entry: LogEntry) {
        self.logs.update(|list| {
            if list.len() >= 500 {
                list.remove(0);
            }
            list.push(entry);
        });
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
        self.items.with(|raw_items| {
            let mut items: Vec<TorrentItem> = raw_items
                .iter()
                .filter(|item| {
                    let matches_query = query.is_empty()
                        || item.name.to_lowercase().contains(&query)
                        || item.hash.to_lowercase().contains(&query)
                        || item.category.to_lowercase().contains(&query);
                    let matches_status = match filter.as_str() {
                        "completed" | "healthy" => item.status() == ItemStatus::Completed,
                        "downloading" | "processing" => item.status() == ItemStatus::Downloading,
                        "warning" => item.status() == ItemStatus::Warning,
                        "failed" => item.status() == ItemStatus::Failed,
                        _ => true,
                    };
                    matches_query && matches_status
                })
                .cloned()
                .collect();

            let sort_key = self.sort_key.get();
            let descending = self.sort_desc.get();
            items.sort_by(|a, b| {
                let ordering = match sort_key {
                    SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    SortKey::DownRate => a.down_rate.cmp(&b.down_rate),
                    SortKey::UpRate => a.up_rate.cmp(&b.up_rate),
                    SortKey::Size => a.size_bytes.cmp(&b.size_bytes),
                    SortKey::Progress => a
                        .progress()
                        .partial_cmp(&b.progress())
                        .unwrap_or(std::cmp::Ordering::Equal),
                    SortKey::Status => status_rank(a.status()).cmp(&status_rank(b.status())),
                    SortKey::Category => a.category.to_lowercase().cmp(&b.category.to_lowercase()),
                    SortKey::Updated => a.state_changed.cmp(&b.state_changed),
                    SortKey::Seeds => a.seeds_connected.cmp(&b.seeds_connected),
                    SortKey::Peers => a.peers_connected.cmp(&b.peers_connected),
                    SortKey::Ratio => a
                        .ratio()
                        .partial_cmp(&b.ratio())
                        .unwrap_or(std::cmp::Ordering::Equal),
                    SortKey::Eta => a.eta_seconds().cmp(&b.eta_seconds()),
                    SortKey::Finished => a.finished.cmp(&b.finished),
                };
                if descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            });
            items
        })
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
        ItemStatus::Checking => 0,
        ItemStatus::Completed => 1,
        ItemStatus::Downloading => 2,
        ItemStatus::Warning => 3,
        ItemStatus::Failed => 4,
    }
}
