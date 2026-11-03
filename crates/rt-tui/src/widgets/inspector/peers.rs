//! Inspector Tab 3: Swarm Peers Table.

use crate::state::PeerUiRecord;
use crate::theme::Theme;
use crate::widgets::sparkline::format_rate;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Cell, Row, Table, Widget};

pub struct PeersTab<'a> {
    peers: &'a [PeerUiRecord],
    theme: Theme,
}

impl<'a> PeersTab<'a> {
    pub fn new(peers: &'a [PeerUiRecord], theme: Theme) -> Self {
        Self { peers, theme }
    }
}

impl<'a> Widget for PeersTab<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 2 {
            return;
        }

        if self.peers.is_empty() {
            buf.set_string(
                area.x,
                area.y,
                "No connected peers in swarm.",
                self.theme.muted_style(),
            );
            return;
        }

        let header = Row::new(vec![
            Cell::from("IP:Port").style(self.theme.table_header_style()),
            Cell::from("Flags").style(self.theme.table_header_style()),
            Cell::from("Client").style(self.theme.table_header_style()),
            Cell::from("Down Rate").style(self.theme.table_header_style()),
            Cell::from("Up Rate").style(self.theme.table_header_style()),
            Cell::from("Progress").style(self.theme.table_header_style()),
        ]);

        let rows: Vec<Row> = self
            .peers
            .iter()
            .take(area.height.saturating_sub(1) as usize)
            .map(|p| {
                Row::new(vec![
                    Cell::from(p.ip_port.clone()),
                    Cell::from(p.flags.clone()),
                    Cell::from(p.client_name.clone()),
                    Cell::from(format_rate(p.down_rate_bps)).style(self.theme.download_style()),
                    Cell::from(format_rate(p.up_rate_bps)).style(self.theme.upload_style()),
                    Cell::from(format!("{}%", p.progress_percent)),
                ])
                .style(Style::default().fg(self.theme.foreground))
            })
            .collect();

        let widths = [
            Constraint::Length(22),
            Constraint::Length(8),
            Constraint::Length(20),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
        ];

        let table = Table::new(rows, widths).header(header);
        table.render(area, buf);
    }
}
