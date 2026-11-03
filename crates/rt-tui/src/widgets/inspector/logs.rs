//! Inspector Tab 6: Filterable Daemon Events and Logs.

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

pub struct LogsTab<'a> {
    messages: &'a [String],
    theme: Theme,
}

impl<'a> LogsTab<'a> {
    pub fn new(messages: &'a [String], theme: Theme) -> Self {
        Self { messages, theme }
    }
}

impl<'a> Widget for LogsTab<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height == 0 {
            return;
        }

        if self.messages.is_empty() {
            buf.set_string(
                area.x,
                area.y,
                "No log events recorded for this session.",
                self.theme.muted_style(),
            );
            return;
        }

        let max_lines = area.height as usize;
        let start_idx = self.messages.len().saturating_sub(max_lines);

        for (i, msg) in self.messages.iter().skip(start_idx).enumerate() {
            let y = area.y + i as u16;
            if y >= area.bottom() {
                break;
            }

            let style = if msg.contains("[ERROR]") || msg.contains("ERR") {
                self.theme.error_style()
            } else if msg.contains("[WARN]") {
                self.theme.warning_style()
            } else {
                Style::default().fg(self.theme.foreground)
            };

            buf.set_string(area.x, y, msg, style);
        }
    }
}
