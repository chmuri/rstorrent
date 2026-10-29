//! Connection health: state machine, exponential backoff and health checks.

use std::time::{Duration, Instant};

/// Longest wait between reconnection attempts.
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// How often the client pings the daemon while idle.
pub const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(10);

/// Where the client thinks the daemon is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Nothing has been attempted yet, or the session ended.
    Disconnected,
    /// The last call succeeded.
    Connected,
    /// The last call failed; the client keeps retrying.
    Reconnecting,
}

/// Tracks failures, backoff and health-check timing.
#[derive(Debug, Clone)]
pub struct ConnectionManager {
    failures: u32,
    state: ConnectionState,
    last_health: Option<Instant>,
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionManager {
    /// Fresh manager: no attempts yet, first backoff is one second.
    pub fn new() -> Self {
        Self {
            failures: 0,
            state: ConnectionState::Disconnected,
            last_health: None,
        }
    }

    /// Current state.
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// Consecutive failures since the last success.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Wait before the next attempt: `1s, 2s, 4s, 8s, 16s, 30s…`.
    pub fn next_backoff(&self) -> Duration {
        let exponent = self.failures.min(31);
        Duration::from_secs(1u64 << exponent).min(MAX_BACKOFF)
    }

    /// Records a failed call.
    pub fn record_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
        self.state = ConnectionState::Reconnecting;
    }

    /// Records a successful call and resets the backoff.
    pub fn record_success(&mut self) {
        self.failures = 0;
        self.state = ConnectionState::Connected;
    }

    /// `true` when a health check is due at `now`.
    pub fn health_due(&self, now: Instant) -> bool {
        match self.last_health {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= HEALTH_CHECK_INTERVAL,
        }
    }

    /// Records that a health check ran at `now`.
    pub fn mark_health_checked(&mut self, now: Instant) {
        self.last_health = Some(now);
    }
}
