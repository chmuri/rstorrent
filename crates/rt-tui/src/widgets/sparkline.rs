//! Real-time rate telemetry sparkline backed by fixed-size ring buffer.

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Sparkline, Widget};

/// Rolling circular buffer for rate history with zero heap allocations during recording.
#[derive(Debug, Clone)]
pub struct RateHistory<const N: usize = 60> {
    samples: [u64; N],
    write_head: usize,
    count: usize,
}

impl<const N: usize> Default for RateHistory<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RateHistory<N> {
    pub const fn new() -> Self {
        Self {
            samples: [0; N],
            write_head: 0,
            count: 0,
        }
    }

    #[inline]
    pub fn push(&mut self, rate_bytes_per_sec: u64) {
        self.samples[self.write_head] = rate_bytes_per_sec;
        self.write_head = (self.write_head + 1) % N;
        if self.count < N {
            self.count += 1;
        }
    }

    #[inline]
    pub fn current(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            let last_idx = if self.write_head == 0 {
                N - 1
            } else {
                self.write_head - 1
            };
            self.samples[last_idx]
        }
    }

    #[inline]
    pub fn max(&self) -> u64 {
        self.samples.iter().copied().max().unwrap_or(0)
    }

    /// Returns ordered samples in chronological order.
    pub fn to_vec(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.count);
        let start = if self.count < N { 0 } else { self.write_head };
        for i in 0..self.count {
            out.push(self.samples[(start + i) % N]);
        }
        out
    }
}

/// Helper to format bandwidth into human-readable strings (e.g. `24.6 MB/s`).
pub fn format_rate(rate_bps: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let bytes = rate_bps as f64;
    if bytes >= GB {
        format!("{:.1} GB/s", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB/s", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB/s", bytes / KB)
    } else {
        format!("{} B/s", rate_bps)
    }
}

/// Helper to format data volume into human-readable strings (e.g. `4.8 GB`).
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let b = bytes as f64;
    if b >= TB {
        format!("{:.2} TB", b / TB)
    } else if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// Renders a labeled sparkline card with current speed and volume.
pub struct RateSparklineWidget<'a> {
    label: &'a str,
    history: &'a RateHistory<60>,
    total_bytes: u64,
    is_download: bool,
    theme: Theme,
}

impl<'a> RateSparklineWidget<'a> {
    pub fn new(
        label: &'a str,
        history: &'a RateHistory<60>,
        total_bytes: u64,
        is_download: bool,
        theme: Theme,
    ) -> Self {
        Self {
            label,
            history,
            total_bytes,
            is_download,
            theme,
        }
    }
}

impl<'a> Widget for RateSparklineWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height == 0 {
            return;
        }

        let rate_str = format!("{}: {}", self.label, format_rate(self.history.current()));
        let total_str = format!("({})", format_size(self.total_bytes));

        let style = if self.is_download {
            self.theme.download_style()
        } else {
            self.theme.upload_style()
        };

        // Split horizontally: Label on left, sparkline in middle, total on right
        let chunks = Layout::horizontal([
            Constraint::Length(rate_str.len() as u16 + 1),
            Constraint::Min(5),
            Constraint::Length(total_str.len() as u16 + 1),
        ])
        .split(area);

        buf.set_string(chunks[0].x, chunks[0].y, rate_str, style);

        let data = self.history.to_vec();
        let max = self.history.max().max(1);

        let sparkline = Sparkline::default().data(&data).max(max).style(style);

        sparkline.render(chunks[1], buf);

        buf.set_string(
            chunks[2].x,
            chunks[2].y,
            total_str,
            Style::default().fg(self.theme.muted),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_history_ring_buffer() {
        let mut hist: RateHistory<5> = RateHistory::new();
        assert_eq!(hist.current(), 0);

        hist.push(10);
        hist.push(20);
        hist.push(30);
        assert_eq!(hist.current(), 30);
        assert_eq!(hist.to_vec(), vec![10, 20, 30]);

        hist.push(40);
        hist.push(50);
        hist.push(60); // overwrites 10
        assert_eq!(hist.current(), 60);
        assert_eq!(hist.to_vec(), vec![20, 30, 40, 50, 60]);
        assert_eq!(hist.max(), 60);
    }

    #[test]
    fn test_format_rate_and_size() {
        assert_eq!(format_rate(1024), "1.0 KB/s");
        assert_eq!(format_rate(1024 * 1024 * 5), "5.0 MB/s");
        assert_eq!(format_size(1024 * 1024 * 1024 * 4), "4.0 GB");
    }
}
