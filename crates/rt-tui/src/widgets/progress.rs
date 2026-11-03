//! Smooth fractional sub-block progress bar generator (8 increments per character).

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// Formats a smooth sub-block progress bar string for a given width in cells.
/// Uses Unicode Left-to-Right 1/8th blocks: ` ▏▎▍▌▋▊▉█`.
pub fn format_fractional_bar(fraction: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let fraction = fraction.clamp(0.0, 1.0);
    let total_steps = (width * 8) as f64;
    let filled_steps = (fraction * total_steps).round() as usize;

    let full_chars = filled_steps / 8;
    let remainder = filled_steps % 8;

    let mut out = String::with_capacity(width * 4);
    for _ in 0..full_chars.min(width) {
        out.push('█');
    }

    if full_chars < width {
        let partial = match remainder {
            0 => ' ',
            1 => '▏',
            2 => '▎',
            3 => '▍',
            4 => '▌',
            5 => '▋',
            6 => '▊',
            7 => '▉',
            _ => '█',
        };
        out.push(partial);

        for _ in (full_chars + 1)..width {
            out.push('░');
        }
    }

    out
}

/// Widget that renders a smooth fractional progress bar with theme colors.
pub struct SmoothProgressBar {
    fraction: f64,
    theme: Theme,
}

impl SmoothProgressBar {
    pub fn new(fraction: f64, theme: Theme) -> Self {
        Self {
            fraction: fraction.clamp(0.0, 1.0),
            theme,
        }
    }

    pub fn color_for_fraction(&self) -> Style {
        if self.fraction >= 1.0 {
            self.theme.success_style()
        } else if self.fraction >= 0.75 {
            Style::default().fg(self.theme.download)
        } else if self.fraction >= 0.25 {
            Style::default().fg(self.theme.accent)
        } else {
            self.theme.warning_style()
        }
    }
}

impl Widget for SmoothProgressBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let bar_str = format_fractional_bar(self.fraction, area.width as usize);
        let style = self.color_for_fraction();
        buf.set_string(area.x, area.y, bar_str, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_bar() {
        let bar = format_fractional_bar(0.0, 10);
        assert_eq!(bar, " ░░░░░░░░░");
    }

    #[test]
    fn test_full_bar() {
        let bar = format_fractional_bar(1.0, 10);
        assert_eq!(bar, "██████████");
    }

    #[test]
    fn test_half_bar() {
        let bar = format_fractional_bar(0.5, 10);
        assert_eq!(bar, "█████ ░░░░");
    }

    #[test]
    fn test_fractional_step() {
        let bar = format_fractional_bar(0.025, 10);
        // 0.025 * 80 = 2 steps => '▎'
        assert_eq!(bar, "▎░░░░░░░░░");
    }
}
