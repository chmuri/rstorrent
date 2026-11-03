//! BEP 29: LEDBAT (Low Extra Delay Background Transport) Congestion Control.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Standard target one-way queuing delay (100 milliseconds in microseconds).
pub const TARGET_DELAY_MICROS: u32 = 100_000;

/// Standard Maximum Segment Size (MSS) for uTP over UDP.
pub const MSS: u32 = 1400;

/// Minimum congestion window (2 * MSS).
pub const MIN_CWND: u32 = 2 * MSS;

/// Initial congestion window (4 * MSS).
pub const INIT_CWND: u32 = 4 * MSS;

/// Default initial retransmission timeout (1000 ms).
pub const INITIAL_RTO: Duration = Duration::from_millis(1000);
pub const MIN_RTO: Duration = Duration::from_millis(500);
pub const MAX_RTO: Duration = Duration::from_millis(10000);

/// Sliding window entry for base delay tracking.
#[derive(Debug, Clone, Copy)]
struct DelaySample {
    timestamp: Instant,
    delay_micros: u32,
}

/// Base delay tracker over a sliding 2-minute window.
#[derive(Debug)]
pub struct BaseDelayTracker {
    window_duration: Duration,
    samples: VecDeque<DelaySample>,
}

impl BaseDelayTracker {
    pub fn new(window_duration: Duration) -> Self {
        Self {
            window_duration,
            samples: VecDeque::new(),
        }
    }

    pub fn add_sample(&mut self, now: Instant, delay_micros: u32) {
        // Purge expired samples
        while let Some(front) = self.samples.front() {
            if now.saturating_duration_since(front.timestamp) > self.window_duration {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        self.samples.push_back(DelaySample {
            timestamp: now,
            delay_micros,
        });
    }

    pub fn base_delay(&self) -> Option<u32> {
        self.samples.iter().map(|s| s.delay_micros).min()
    }
}

/// LEDBAT congestion controller.
#[derive(Debug)]
pub struct LedbatCongestion {
    /// Maximum allowed in-flight bytes (congestion window).
    max_window: u32,
    /// Currently unacknowledged in-flight bytes.
    cur_window: u32,
    /// Base delay tracker (minimum delay observed over the last 120s).
    base_delay_tracker: BaseDelayTracker,
    /// Smoothed Round Trip Time (SRTT).
    srtt: Option<Duration>,
    /// Round Trip Time Variation (RTTVAR).
    rttvar: Duration,
    /// Retransmission timeout (RTO).
    rto: Duration,
}

impl Default for LedbatCongestion {
    fn default() -> Self {
        Self::new()
    }
}

impl LedbatCongestion {
    pub fn new() -> Self {
        Self {
            max_window: INIT_CWND,
            cur_window: 0,
            base_delay_tracker: BaseDelayTracker::new(Duration::from_secs(120)),
            srtt: None,
            rttvar: Duration::from_millis(500),
            rto: INITIAL_RTO,
        }
    }

    pub fn max_window(&self) -> u32 {
        self.max_window
    }

    pub fn cur_window(&self) -> u32 {
        self.cur_window
    }

    pub fn rto(&self) -> Duration {
        self.rto
    }

    pub fn on_packet_sent(&mut self, bytes: u32) {
        self.cur_window = self.cur_window.saturating_add(bytes);
    }

    /// Can we send more bytes according to LEDBAT window and peer advertised window?
    pub fn can_send(&self, remote_wnd: u32) -> bool {
        let effective_wnd = self.max_window.min(remote_wnd);
        self.cur_window < effective_wnd
    }

    /// Available send quota in bytes.
    pub fn available_window(&self, remote_wnd: u32) -> u32 {
        let effective_wnd = self.max_window.min(remote_wnd);
        effective_wnd.saturating_sub(self.cur_window)
    }

    /// Called when an ACK arrives acknowledging `bytes_acked` with a measured delay.
    pub fn on_ack(
        &mut self,
        bytes_acked: u32,
        sample_delay_micros: u32,
        rtt_sample: Option<Duration>,
        now: Instant,
    ) {
        self.cur_window = self.cur_window.saturating_sub(bytes_acked);

        if let Some(rtt) = rtt_sample {
            self.update_rto(rtt);
        }

        if sample_delay_micros == 0 {
            return;
        }

        self.base_delay_tracker.add_sample(now, sample_delay_micros);
        let base_delay = match self.base_delay_tracker.base_delay() {
            Some(bd) => bd,
            None => return,
        };

        // Queuing delay is the delay over the lowest baseline
        let queuing_delay = sample_delay_micros.saturating_sub(base_delay);
        let off_target = (TARGET_DELAY_MICROS as i64) - (queuing_delay as i64);

        // Window adjustment
        // max_window += GAIN * (off_target / TARGET_DELAY) * (bytes_acked / max_window)
        // GAIN is 1 packet (MSS) per RTT
        let window_factor = (bytes_acked as f64) / (self.max_window.max(1) as f64);
        let delay_factor = (off_target as f64) / (TARGET_DELAY_MICROS as f64);
        let window_change = (MSS as f64) * delay_factor * window_factor;

        let new_wnd = (self.max_window as f64) + window_change;
        self.max_window = (new_wnd.round() as u32).max(MIN_CWND);
    }

    /// Called on packet loss or timeout event.
    pub fn on_loss(&mut self) {
        // Multiplicative decrease (drop window by 30%)
        let decreased = (self.max_window as f64 * 0.70) as u32;
        self.max_window = decreased.max(MIN_CWND);
        // Backoff RTO
        self.rto = (self.rto * 2).min(MAX_RTO);
    }

    /// Standard RFC 6298 RTT / RTO calculation.
    fn update_rto(&mut self, rtt: Duration) {
        match self.srtt {
            None => {
                self.srtt = Some(rtt);
                self.rttvar = rtt / 2;
                self.rto = (rtt + self.rttvar * 4).clamp(MIN_RTO, MAX_RTO);
            }
            Some(srtt) => {
                let rtt_diff = rtt.abs_diff(srtt);
                self.rttvar = (self.rttvar * 3 + rtt_diff) / 4;
                let new_srtt = (srtt * 7 + rtt) / 8;
                self.srtt = Some(new_srtt);
                self.rto = (new_srtt + self.rttvar * 4).clamp(MIN_RTO, MAX_RTO);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ledbat_window_growth_on_low_delay() {
        let mut ledbat = LedbatCongestion::new();
        let now = Instant::now();
        let initial_wnd = ledbat.max_window();

        // Feed baseline delay sample of 50ms (50,000 micros)
        ledbat.on_ack(1400, 50_000, Some(Duration::from_millis(50)), now);

        // Feed subsequent sample also at 50ms -> queuing delay is 0, off_target is +100ms
        for _ in 0..10 {
            ledbat.on_ack(1400, 50_000, Some(Duration::from_millis(50)), now);
        }

        assert!(
            ledbat.max_window() > initial_wnd,
            "Window must grow when queuing delay is below target"
        );
    }

    #[test]
    fn test_ledbat_window_shrinks_on_high_delay() {
        let mut ledbat = LedbatCongestion::new();
        let now = Instant::now();

        // Baseline 20ms
        ledbat.on_ack(1400, 20_000, Some(Duration::from_millis(20)), now);

        // Huge queuing delay of 250ms (delay 270ms - base 20ms = 250ms queuing delay > 100ms)
        let wnd_before = ledbat.max_window();
        for _ in 0..10 {
            ledbat.on_ack(1400, 270_000, Some(Duration::from_millis(270)), now);
        }

        assert!(
            ledbat.max_window() < wnd_before,
            "Window must shrink when queuing delay exceeds target"
        );
    }

    #[test]
    fn test_loss_event_decreases_window() {
        let mut ledbat = LedbatCongestion::new();
        ledbat.max_window = 100_000;
        ledbat.on_loss();
        assert_eq!(ledbat.max_window(), 70_000);
        assert!(ledbat.rto() >= INITIAL_RTO);
    }
}
