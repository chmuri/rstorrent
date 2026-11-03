//! Inspector Tab 2: Trackers Table.

use crate::state::TrackerUiRecord;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Cell, Row, Table, Widget};

pub struct TrackersTab<'a> {
    trackers: &'a [TrackerUiRecord],
    theme: Theme,
}

impl<'a> TrackersTab<'a> {
    pub fn new(trackers: &'a [TrackerUiRecord], theme: Theme) -> Self {
        Self { trackers, theme }
    }
}

impl<'a> Widget for TrackersTab<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 2 {
            return;
        }

        if self.trackers.is_empty() {
            buf.set_string(
                area.x,
                area.y,
                "No active trackers.",
                self.theme.muted_style(),
            );
            return;
        }

        let header = Row::new(vec![
            Cell::from("#").style(self.theme.table_header_style()),
            Cell::from("Tier").style(self.theme.table_header_style()),
            Cell::from("Status").style(self.theme.table_header_style()),
            Cell::from("Seeds").style(self.theme.table_header_style()),
            Cell::from("Peers").style(self.theme.table_header_style()),
            Cell::from("Next In").style(self.theme.table_header_style()),
            Cell::from("URL").style(self.theme.table_header_style()),
        ]);

        let rows: Vec<Row> = self
            .trackers
            .iter()
            .take(area.height.saturating_sub(1) as usize)
            .map(|t| {
                let status_style = if t.status.contains("Active") || t.status.contains("200") {
                    self.theme.success_style()
                } else if t.status.contains("Error") {
                    self.theme.error_style()
                } else {
                    Style::default().fg(self.theme.foreground)
                };

                Row::new(vec![
                    Cell::from(t.index.to_string()),
                    Cell::from(t.tier.to_string()),
                    Cell::from(t.status.clone()).style(status_style),
                    Cell::from(t.seeds.to_string()),
                    Cell::from(t.peers.to_string()),
                    Cell::from(format!("{}s", t.next_announce_secs)),
                    Cell::from(t.url.clone()),
                ])
                .style(Style::default().fg(self.theme.foreground))
            })
            .collect();

        let widths = [
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(14),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Min(20),
        ];

        let table = Table::new(rows, widths).header(header);
        table.render(area, buf);
    }
}
