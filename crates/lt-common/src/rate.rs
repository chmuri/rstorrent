use std::time::{SystemTime, UNIX_EPOCH};

/// A rate meter using a sliding window of second-buckets.
/// Matches libtorrent's `Rate` algorithm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rate {
    span: u32,
    startup_span: u32,
    min_active_seconds: u32,
    idle_timeout: u32,

    buckets: Vec<u64>,
    current: u64,
    total: u64,

    start_insert: Option<u32>,
    last_insert: Option<u32>,
    last_query: Option<u32>,
}

impl Rate {
    pub fn new(span: u32) -> Self {
        Self::with_params(span, 3, 3, span / 3)
    }

    pub fn with_startup_span(span: u32, startup_span: u32) -> Self {
        Self::with_params(span, startup_span, 3, span / 3)
    }

    pub fn with_params(
        span: u32,
        startup_span: u32,
        min_active_seconds: u32,
        idle_timeout: u32,
    ) -> Self {
        assert!(span > 0);
        let startup_span = startup_span.min(span).max(1);
        let min_active_seconds = min_active_seconds.min(span).max(1);
        let idle_timeout = idle_timeout.min(span).max(1);

        Self {
            span,
            startup_span,
            min_active_seconds,
            idle_timeout,
            buckets: vec![0u64; span as usize],
            current: 0,
            total: 0,
            start_insert: None,
            last_insert: None,
            last_query: None,
        }
    }

    pub fn span(&self) -> u32 {
        self.span
    }

    pub fn startup_span(&self) -> u32 {
        self.startup_span
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn set_total(&mut self, total: u64) {
        self.total = total;
    }

    fn bucket_index(&self, second: u32) -> usize {
        (second as usize) % self.buckets.len()
    }

    fn clear_rate(&mut self) {
        if self.current != 0 {
            self.buckets.fill(0);
        }
        self.current = 0;
        self.start_insert = None;
        self.last_insert = None;
        self.last_query = None;
    }

    pub fn advance_to(&mut self, now: u32) {
        let last_q = match self.last_query {
            None => {
                self.last_query = Some(now);
                return;
            }
            Some(q) => q,
        };

        if now < last_q {
            self.clear_rate();
            self.last_query = Some(now);
            return;
        }

        if let Some(last_in) = self.last_insert {
            if now.saturating_sub(last_in) > self.idle_timeout {
                self.clear_rate();
                self.last_query = Some(now);
                return;
            }
        }

        let elapsed = now - last_q;
        if elapsed == 0 {
            return;
        }

        if elapsed >= self.span {
            self.clear_rate();
            self.last_query = Some(now);
            return;
        }

        for offset in 1..=elapsed {
            let b_idx = self.bucket_index(last_q + offset);
            self.current = self.current.saturating_sub(self.buckets[b_idx]);
            self.buckets[b_idx] = 0;
        }

        if self.current == 0 {
            self.start_insert = None;
            self.last_insert = None;
        }

        self.last_query = Some(now);
    }

    /// Insert bytes at explicit timestamp `now` (in seconds).
    pub fn insert_at(&mut self, bytes: u64, now: u32) {
        self.advance_to(now);

        if bytes == 0 {
            return;
        }

        if let Some(last_in) = self.last_insert {
            if now.saturating_sub(last_in) > self.startup_span {
                self.clear_rate();
                self.advance_to(now);
            }
        }

        if self.start_insert.is_none() {
            self.start_insert = Some(now);
        }

        let b_idx = self.bucket_index(now);
        self.buckets[b_idx] += bytes;
        self.current += bytes;
        self.total += bytes;
        self.last_insert = Some(now);
    }

    /// Calculate rate at explicit timestamp `now` (in seconds).
    pub fn rate_at(&mut self, now: u32) -> u64 {
        self.advance_to(now);

        if self.current == 0 {
            return 0;
        }

        let start = self.start_insert.unwrap_or(now);
        let active_seconds = now.saturating_sub(start) + 1;
        let divisor = self.min_active_seconds.max(self.span.min(active_seconds));

        self.current / (divisor as u64)
    }

    fn current_timestamp_secs() -> u32 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0)
    }

    /// Insert bytes transferred at current time.
    pub fn insert(&mut self, bytes: u64) {
        self.insert_at(bytes, Self::current_timestamp_secs());
    }

    /// Get current transfer rate in bytes per second.
    pub fn rate(&mut self) -> u64 {
        self.rate_at(Self::current_timestamp_secs())
    }

    pub fn reset_rate(&mut self) {
        self.clear_rate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_startup_and_filling_window() {
        let mut rate = Rate::with_params(4, 3, 3, 4);

        rate.insert_at(100, 0);
        assert_eq!(rate.rate_at(0), 33);

        rate.insert_at(100, 1);
        assert_eq!(rate.rate_at(1), 66);

        rate.insert_at(100, 2);
        assert_eq!(rate.rate_at(2), 100);

        rate.insert_at(100, 3);
        assert_eq!(rate.rate_at(3), 100);

        rate.insert_at(100, 4);
        assert_eq!(rate.rate_at(4), 100);
    }

    #[test]
    fn test_rate_decays_after_span() {
        let mut rate = Rate::new(5);

        rate.insert_at(500, 0);
        assert_eq!(rate.rate_at(5), 0);
    }

    #[test]
    fn test_restart_after_idle() {
        let mut rate = Rate::with_params(30, 3, 3, 10);

        rate.insert_at(1200, 0);
        rate.insert_at(1200, 1);

        assert!(rate.rate_at(10) < 1200);

        rate.insert_at(1200, 10);
        assert_eq!(rate.rate_at(10), 400);

        assert_eq!(rate.rate_at(21), 0);
    }

    #[test]
    fn test_zero_bytes_do_not_prevent_idle_restart() {
        let mut rate = Rate::with_params(30, 3, 3, 10);

        rate.insert_at(1200, 0);
        rate.insert_at(0, 9);
        rate.insert_at(0, 18);
        rate.insert_at(1200, 27);

        assert_eq!(rate.rate_at(27), 400);
    }

    #[test]
    fn test_zero_bytes_do_not_discard_samples() {
        let mut rate = Rate::with_params(30, 3, 3, 10);

        rate.insert_at(1200, 0);
        rate.insert_at(0, 4);

        assert_eq!(rate.rate_at(4), 240);
    }

    #[test]
    fn test_clock_rollback_resets_rate() {
        let mut rate = Rate::with_params(30, 3, 3, 10);

        rate.insert_at(1200, 10);
        rate.insert_at(300, 9);

        assert_eq!(rate.rate_at(9), 100);
        assert_eq!(rate.total(), 1500);
    }

    #[test]
    fn test_startup_span() {
        let mut rate = Rate::with_params(30, 5, 6, 10);

        assert_eq!(rate.startup_span(), 5);
        rate.insert_at(600, 0);
        assert_eq!(rate.rate_at(0), 100);

        let clamped_rate = Rate::with_params(30, 30, 30, 30);
        assert_eq!(clamped_rate.startup_span(), 30);
    }
}
