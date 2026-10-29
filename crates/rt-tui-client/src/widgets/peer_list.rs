//! Peers tab: address, client, transfer rates and the encryption marker.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Paragraph, Row, Table, Widget};

use crate::state::PeerListState;
use crate::widgets::download_list::{viewport, visible_rows};

/// Peer table.
pub struct PeerListWidget<'a> {
    state: &'a PeerListState,
}

impl<'a> PeerListWidget<'a> {
    /// Builds the widget for the given state.
    pub fn new(state: &'a PeerListState) -> Self {
        Self { state }
    }
}

impl Widget for PeerListWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.state.entries.is_empty() {
            Paragraph::new("No peers").render(area, buf);
            return;
        }

        let header = Row::new(vec!["Address", "Client", "↓ Rate", "↑ Rate", "🔒"])
            .style(Style::new().add_modifier(Modifier::BOLD));

        let range = viewport(
            self.state.focus_index,
            self.state.entries.len(),
            visible_rows(area.height),
        );

        let rows = range
            .map(|index| {
                let peer = &self.state.entries[index];
                let lock = if peer.encrypted {
                    Span::styled("🔒", Style::new().fg(Color::Green))
                } else {
                    Span::raw("")
                };

                let row = Row::new(vec![
                    Cell::from(peer.address.as_str()),
                    Cell::from(peer.client.as_str()),
                    Cell::from(peer.down_rate_display.trim()),
                    Cell::from(peer.up_rate_display.trim()),
                    Cell::from(lock),
                ]);

                if index == self.state.focus_index {
                    row.style(Style::new().reversed())
                } else {
                    row
                }
            })
            .collect::<Vec<_>>();

        let widths = [
            Constraint::Min(18),
            Constraint::Length(13),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(3),
        ];

        Widget::render(Table::new(rows, widths).header(header), area, buf);
    }
}
