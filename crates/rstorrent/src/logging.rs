//! Configurable `tracing` subscriber initialization for daemon and interactive
//! modes.
//!
//! The level comes from the TOML `[logging] level` key (default `info`), the
//! per-target filter from `RUST_LOG` (`EnvFilter`). In interactive mode logs are
//! captured to a file rather than stdout/stderr so the TUI is not corrupted.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;
use crate::setup::find_config_file;
use crate::toml_config::RtorrentTomlConfig;

/// Structured log record captured from `tracing` events.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: u64,
    pub timestamp: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Central hub storing recent log entries in an in-memory ring buffer
/// and broadcasting live log events to subscribed WebSocket clients.
pub struct LogHub {
    capacity: usize,
    entries: Mutex<VecDeque<LogEntry>>,
    next_id: AtomicU64,
    sender: broadcast::Sender<LogEntry>,
}

impl LogHub {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self {
            capacity,
            entries: Mutex::new(VecDeque::with_capacity(capacity)),
            next_id: AtomicU64::new(1),
            sender,
        }
    }

    pub fn push(&self, level: &tracing::Level, target: &str, message: String) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let entry = LogEntry {
            id,
            timestamp,
            level: level.to_string(),
            target: target.to_string(),
            message,
        };

        if let Ok(mut entries) = self.entries.lock() {
            if entries.len() >= self.capacity {
                entries.pop_front();
            }
            entries.push_back(entry.clone());
        }

        let _ = self.sender.send(entry);
    }

    pub fn recent(&self, limit: usize) -> Vec<LogEntry> {
        if let Ok(entries) = self.entries.lock() {
            let start = entries.len().saturating_sub(limit);
            entries.iter().skip(start).cloned().collect()
        } else {
            Vec::new()
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.sender.subscribe()
    }
}

static GLOBAL_LOG_HUB: OnceLock<LogHub> = OnceLock::new();

pub fn log_hub() -> &'static LogHub {
    GLOBAL_LOG_HUB.get_or_init(|| LogHub::new(1000))
}

struct FieldVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl tracing::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let name = field.name();
        let formatted = format!("{value:?}");
        if name == "message" {
            let trimmed =
                if formatted.starts_with('"') && formatted.ends_with('"') && formatted.len() >= 2 {
                    &formatted[1..formatted.len() - 1]
                } else {
                    &formatted
                };
            self.message = trimmed.to_string();
        } else {
            self.fields.push((name.to_string(), formatted));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields
                .push((field.name().to_string(), value.to_string()));
        }
    }
}

struct LogBroadcastLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogBroadcastLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let mut visitor = FieldVisitor {
            message: String::new(),
            fields: Vec::new(),
        };
        event.record(&mut visitor);

        let formatted = if visitor.fields.is_empty() {
            visitor.message
        } else {
            let mut parts = Vec::new();
            if !visitor.message.is_empty() {
                parts.push(visitor.message);
            }
            let field_str = visitor
                .fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ");
            parts.push(format!("[{field_str}]"));
            parts.join(" ")
        };

        if !formatted.is_empty() {
            log_hub().push(metadata.level(), metadata.target(), formatted);
        }
    }
}

/// Resolve the logging level and optional file path from the TOML `[logging]`
/// section. The config file is located the same way `Control::initialize` does.
fn resolve_logging(cli: &Cli) -> (String, Option<PathBuf>) {
    let mut level = None;
    let mut file = None;
    if !cli.ignore_rc_file() {
        let config_path = cli
            .config_file
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| find_config_file(cli.no_rc));
        if let Some(path) = config_path {
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(cfg) = RtorrentTomlConfig::parse(&content) {
                        level = cfg.logging.level;
                        file = cfg.logging.file.map(PathBuf::from);
                    }
                }
            }
        }
    }
    (level.unwrap_or_else(|| "info".to_string()), file)
}

fn parse_max_level(level: &str) -> LevelFilter {
    match level.to_ascii_lowercase().as_str() {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        "off" => LevelFilter::OFF,
        _ => LevelFilter::INFO,
    }
}

fn default_log_path() -> PathBuf {
    std::env::temp_dir().join("rstorrent.log")
}

fn open_log_file(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

/// Install the global subscriber with two filter layers: a global `LevelFilter`
/// cap from config and the per-target `EnvFilter` from `RUST_LOG`. Both must
/// pass for an event to be recorded. Using separate layers (rather than the fmt
/// builder's `with_max_level`, which would replace the `EnvFilter`) keeps both.
fn install<W>(env_filter: EnvFilter, max_level: LevelFilter, writer: W)
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::registry()
        .with(max_level)
        .with(env_filter)
        .with(LogBroadcastLayer)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .init();
}

/// Install the global tracing subscriber. Called before `Control::initialize`
/// so startup diagnostics are captured in both daemon and interactive modes.
pub fn init_logging(cli: &Cli, is_interactive: bool) {
    let (level, file) = resolve_logging(cli);
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level.clone()));
    let max_level = parse_max_level(&level);

    // Interactive mode must never write to the terminal: it would corrupt the
    // TUI. Capture to a file instead. Daemon mode defaults to stderr.
    if is_interactive || file.is_some() {
        let path = file.unwrap_or_else(default_log_path);
        match open_log_file(&path) {
            Ok(log_file) => install(env_filter, max_level, std::sync::Mutex::new(log_file)),
            Err(e) => {
                eprintln!(
                    "rstorrent: cannot open log file {}: {e}; falling back to stderr",
                    path.display()
                );
                install(env_filter, max_level, std::io::stderr);
            }
        }
    } else {
        install(env_filter, max_level, std::io::stderr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_level_parses_known_levels_and_defaults_to_info() {
        assert_eq!(parse_max_level("trace"), LevelFilter::TRACE);
        assert_eq!(parse_max_level("DEBUG"), LevelFilter::DEBUG);
        assert_eq!(parse_max_level("warn"), LevelFilter::WARN);
        assert_eq!(parse_max_level("error"), LevelFilter::ERROR);
        assert_eq!(parse_max_level("off"), LevelFilter::OFF);
        assert_eq!(parse_max_level("info"), LevelFilter::INFO);
        assert_eq!(parse_max_level("nonsense"), LevelFilter::INFO);
    }
}
