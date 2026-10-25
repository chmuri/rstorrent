//! Verifies the opt-in startup/shutdown metrics contract: valid JSONL, one
//! line per event, and a disabled recorder that touches no file.

use rstorrent::control::Control;
use rstorrent::metrics::{Metrics, SCHEMA_VERSION};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_metrics_path(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "rstorrent-metrics-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("metrics.jsonl")
}

fn cleanup(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

fn read_events(path: &Path) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(path).expect("metrics file exists");
    raw.lines()
        .map(|line| serde_json::from_str(line).expect("every line is valid JSON"))
        .collect()
}

#[test]
fn phase_and_summary_lines_are_valid_json() {
    let path = temp_metrics_path("format");
    let metrics = Metrics::open(&path).unwrap();
    assert!(metrics.is_enabled());

    let started = std::time::Instant::now();
    metrics.phase("startup.phase", "restore_entries", started, Some(10_000));
    metrics.phase_with(
        "shutdown.phase",
        "session_save",
        started,
        None,
        &[("completed", 1), ("timed_out", 0)],
    );
    metrics.summary("shutdown.summary", &[("total_ms", 42)]);
    metrics.failure("startup.failed", "bad \"quote\"\nand newline");

    let events = read_events(&path);
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|e| e["v"] == SCHEMA_VERSION));

    assert_eq!(events[0]["event"], "startup.phase");
    assert_eq!(events[0]["phase"], "restore_entries");
    assert_eq!(events[0]["n"], 10_000);
    assert!(events[0]["ms"].is_u64());

    assert_eq!(events[1]["phase"], "session_save");
    assert_eq!(events[1]["completed"], 1);
    assert_eq!(events[1]["timed_out"], 0);

    assert_eq!(events[2]["event"], "shutdown.summary");
    assert_eq!(events[2]["total_ms"], 42);

    assert_eq!(events[3]["error"], "bad \"quote\"\nand newline");

    cleanup(&path);
}

#[test]
fn disabled_recorder_creates_no_file() {
    let dir = temp_metrics_path("disabled");
    let path = dir.with_file_name("should-not-exist.jsonl");

    let metrics = Metrics::disabled();
    metrics.phase("startup.phase", "views", std::time::Instant::now(), None);
    metrics.summary("startup.summary", &[("torrents", 1)]);

    assert!(!path.exists());
    cleanup(&dir);
}

#[tokio::test]
async fn shutdown_emits_every_phase_and_a_summary() {
    let path = temp_metrics_path("shutdown");
    let mut control = Control::new();
    control.set_metrics(Metrics::open(&path).unwrap());

    control.shutdown().await;

    let events = read_events(&path);
    for phase in [
        "network_drain",
        "flush_downloads",
        "disk_thread",
        "session_save",
        "lock_release",
    ] {
        assert!(
            events
                .iter()
                .any(|e| e["event"] == "shutdown.phase" && e["phase"] == phase),
            "missing shutdown phase {phase}"
        );
    }

    let summary = events
        .iter()
        .find(|e| e["event"] == "shutdown.summary")
        .expect("shutdown summary recorded");
    assert!(summary["total_ms"].is_u64());

    #[cfg(target_os = "linux")]
    assert!(summary["proc_write_bytes"].is_u64());

    cleanup(&path);
}
