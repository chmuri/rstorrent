//! Disk storage gauge widget with capacity metrics and alert thresholds.

use crate::theme::Theme;
use crate::widgets::sparkline::format_size;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Gauge, Widget};

pub struct StorageGaugeWidget {
    used_bytes: u64,
    total_bytes: u64,
    free_bytes: u64,
    theme: Theme,
}

impl StorageGaugeWidget {
    pub fn new(free_bytes: u64, total_bytes: u64, theme: Theme) -> Self {
        let used_bytes = total_bytes.saturating_sub(free_bytes);
        Self {
            used_bytes,
            total_bytes,
            free_bytes,
            theme,
        }
    }
}

impl Widget for StorageGaugeWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height == 0 {
            return;
        }

        let ratio = if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        };

        let label = format!(
            "Disk: {}% ({} free)",
            (ratio * 100.0).round() as u64,
            format_size(self.free_bytes)
        );

        let color = if ratio >= 0.90 {
            self.theme.error
        } else if ratio >= 0.75 {
            self.theme.warning
        } else {
            self.theme.success
        };

        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(color).bg(self.theme.surface))
            .ratio(ratio)
            .label(label);

        gauge.render(area, buf);
    }
}
