//! Log view: timestamped client messages with scrollback.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::state::LogState;

/// Scrollback viewer for client messages.
pub struct LogWidget<'a> {
    state: &'a LogState,
}

impl<'a> LogWidget<'a> {
    /// Builds the widget for the given log state.
    pub fn new(state: &'a LogState) -> Self {
        Self { state }
    }
}

impl Widget for LogWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.state.is_empty() {
            Paragraph::new("No log messages").render(area, buf);
            return;
        }

        let height = usize::from(area.height);
        let end = self.state.len().saturating_sub(self.state.scroll());
        let start = end.saturating_sub(height);

        let lines = self.state.entries()[start..end]
            .iter()
            .map(|entry| Line::from(format!("[{}] {}", entry.timestamp, entry.message)))
            .collect::<Vec<_>>();

        Paragraph::new(lines).render(area, buf);
    }
}
