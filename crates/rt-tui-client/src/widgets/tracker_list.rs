//! Trackers tab: group, index, enabled flag and announce URL.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Paragraph, Row, Table, Widget};

use crate::state::TrackerListState;
use crate::widgets::download_list::{viewport, visible_rows};

/// Tracker table.
pub struct TrackerListWidget<'a> {
    state: &'a TrackerListState,
}

impl<'a> TrackerListWidget<'a> {
    /// Builds the widget for the given state.
    pub fn new(state: &'a TrackerListState) -> Self {
        Self { state }
    }
}

impl Widget for TrackerListWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.state.entries.is_empty() {
            Paragraph::new("No trackers").render(area, buf);
            return;
        }

        let header = Row::new(vec!["Group", "Index", "Enabled", "URL"])
            .style(Style::new().add_modifier(Modifier::BOLD));

        let range = viewport(
            self.state.focus_index,
            self.state.entries.len(),
            visible_rows(area.height),
        );

        let rows = range
            .map(|index| {
                let tracker = &self.state.entries[index];
                let (label, colour) = if tracker.enabled {
                    ("yes", Color::Green)
                } else {
                    ("no", Color::Red)
                };

                let row = Row::new(vec![
                    Cell::from(tracker.group.to_string()),
                    Cell::from(tracker.index.to_string()),
                    Cell::from(Span::styled(label, Style::new().fg(colour))),
                    Cell::from(tracker.url.as_str()),
                ]);

                if index == self.state.focus_index {
                    row.style(Style::new().reversed())
                } else {
                    row
                }
            })
            .collect::<Vec<_>>();

        let widths = [
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Min(20),
        ];

        Widget::render(Table::new(rows, widths).header(header), area, buf);
    }
}
