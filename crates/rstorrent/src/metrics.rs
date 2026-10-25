//! Structured startup and shutdown metrics.
//!
//! The interactive TUI installs no `tracing` subscriber, so phase timings have
//! to leave the process through a side channel. Setting
//! [`METRICS_ENV`] to a file path makes every recorded event append one JSON
//! object per line to that file. Without the variable the recorder is
//! disabled and every method returns immediately.
//!
//! Events are emitted only on phase boundaries — a handful per process
//! lifetime — so the recorder never adds work to a per-torrent loop.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Environment variable naming the JSONL output file.
pub const METRICS_ENV: &str = "RSTORRENT_METRICS_FILE";

/// Version of the line schema. Bump when a field changes meaning.
pub const SCHEMA_VERSION: u64 = 1;

/// Process-wide I/O counters read from `/proc/self/io`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoSnapshot {
    pub write_bytes: u64,
    pub wchar: u64,
}

/// Appends structured events to a file, or does nothing when disabled.
pub struct Metrics {
    file: Option<Mutex<File>>,
    path: Option<PathBuf>,
    start: Instant,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::disabled()
    }
}

impl Metrics {
    /// A recorder that discards every event.
    pub fn disabled() -> Self {
        Self {
            file: None,
            path: None,
            start: Instant::now(),
        }
    }

    /// Enables recording to `path`, creating it when missing.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            file: Some(Mutex::new(file)),
            path: Some(path),
            start: Instant::now(),
        })
    }

    /// Reads [`METRICS_ENV`]. Any failure yields a disabled recorder so that
    /// observability can never prevent the client from starting.
    pub fn from_env() -> Self {
        let Some(raw) = std::env::var_os(METRICS_ENV) else {
            return Self::disabled();
        };
        if raw.is_empty() {
            return Self::disabled();
        }
        match Self::open(PathBuf::from(raw)) {
            Ok(metrics) => metrics,
            Err(err) => {
                tracing::warn!(error = %err, "Could not open metrics file; metrics disabled");
                Self::disabled()
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.file.is_some()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Milliseconds since this recorder was created.
    pub fn elapsed_ms(&self) -> u64 {
        duration_ms(self.start.elapsed())
    }

    /// Milliseconds between `started` and now.
    pub fn ms_since(&self, started: Instant) -> u64 {
        duration_ms(started.elapsed())
    }

    /// Records the duration of a named phase.
    pub fn phase(
        &self,
        event: &'static str,
        phase: &'static str,
        started: Instant,
        n: Option<u64>,
    ) {
        self.phase_with(event, phase, started, n, &[]);
    }

    /// Records a phase duration plus extra numeric flags (e.g. `completed`).
    pub fn phase_with(
        &self,
        event: &'static str,
        phase: &'static str,
        started: Instant,
        n: Option<u64>,
        flags: &[(&'static str, u64)],
    ) {
        if !self.is_enabled() {
            return;
        }
        let mut line = self.base(event);
        line.push_str(",\"phase\":\"");
        line.push_str(phase);
        line.push_str("\",\"ms\":");
        line.push_str(&self.ms_since(started).to_string());
        if let Some(n) = n {
            line.push_str(",\"n\":");
            line.push_str(&n.to_string());
        }
        for (key, value) in flags {
            line.push_str(",\"");
            line.push_str(key);
            line.push_str("\":");
            line.push_str(&value.to_string());
        }
        line.push('}');
        self.write_line(line);
    }

    /// Records a single counter value.
    pub fn counter(&self, name: &'static str, value: u64) {
        self.summary("counter", &[(name, value)]);
    }

    /// Records a set of counters under one event name.
    pub fn summary(&self, event: &'static str, fields: &[(&'static str, u64)]) {
        if !self.is_enabled() {
            return;
        }
        let mut line = self.base(event);
        for (key, value) in fields {
            line.push_str(",\"");
            line.push_str(key);
            line.push_str("\":");
            line.push_str(&value.to_string());
        }
        line.push('}');
        self.write_line(line);
    }

    /// Records a failure with a free-form message.
    pub fn failure(&self, event: &'static str, message: &str) {
        if !self.is_enabled() {
            return;
        }
        let mut line = self.base(event);
        line.push_str(",\"error\":\"");
        escape_into(&mut line, message);
        line.push_str("\"}");
        self.write_line(line);
    }

    fn base(&self, event: &'static str) -> String {
        format!(
            "{{\"v\":{SCHEMA_VERSION},\"t_ms\":{},\"event\":\"{event}\"",
            self.elapsed_ms()
        )
    }

    fn write_line(&self, mut line: String) {
        let Some(file) = &self.file else {
            return;
        };
        line.push('\n');
        if let Ok(mut guard) = file.lock() {
            // A metrics write must never affect the client's own I/O path:
            // failures are dropped rather than propagated.
            let _ = guard.write_all(line.as_bytes());
            let _ = guard.flush();
        }
    }
}

/// Reads the calling process's cumulative write volume.
///
/// Used as an independent cross-check against the session writer's own
/// counters: a large gap means something outside the session path is writing.
pub fn io_snapshot() -> Option<IoSnapshot> {
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read_to_string("/proc/self/io").ok()?;
        let mut snapshot = IoSnapshot {
            write_bytes: 0,
            wchar: 0,
        };
        for line in raw.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let Ok(value) = value.trim().parse::<u64>() else {
                continue;
            };
            match key {
                "write_bytes" => snapshot.write_bytes = value,
                "wchar" => snapshot.wchar = value,
                _ => {}
            }
        }
        Some(snapshot)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Appends `value` to `out` with JSON string escaping.
fn escape_into(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_handles_quotes_and_control_characters() {
        let mut out = String::new();
        escape_into(&mut out, "a\"b\\c\nd\te\u{1}");
        assert_eq!(out, "a\\\"b\\\\c\\nd\\te\\u0001");
    }

    #[test]
    fn disabled_recorder_writes_nothing() {
        let metrics = Metrics::disabled();
        assert!(!metrics.is_enabled());
        assert!(metrics.path().is_none());
        metrics.phase("startup.phase", "views", Instant::now(), Some(1));
        metrics.summary("startup.summary", &[("torrents", 1)]);
        metrics.failure("startup.failed", "boom");
    }
}
