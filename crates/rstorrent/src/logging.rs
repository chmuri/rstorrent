//! Configurable `tracing` subscriber initialization for daemon and interactive
//! modes.
//!
//! The level comes from the TOML `[logging] level` key (default `info`), the
//! per-target filter from `RUST_LOG` (`EnvFilter`). In interactive mode logs are
//! captured to a file rather than stdout/stderr so the TUI is not corrupted.

use std::path::{Path, PathBuf};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;
use crate::setup::find_config_file;
use crate::toml_config::RtorrentTomlConfig;

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
