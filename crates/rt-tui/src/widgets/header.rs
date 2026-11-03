//! Dashboard telemetry header widget (Zone 1).

use crate::state::SessionStats;
use crate::theme::Theme;
use crate::widgets::sparkline::{RateHistory, RateSparklineWidget};
use crate::widgets::storage_gauge::StorageGaugeWidget;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Widget};

pub struct DashboardHeader<'a> {
    stats: &'a SessionStats,
    down_history: &'a RateHistory<60>,
    up_history: &'a RateHistory<60>,
    theme: Theme,
    endpoint_label: &'a str,
}

impl<'a> DashboardHeader<'a> {
    pub fn new(
        stats: &'a SessionStats,
        down_history: &'a RateHistory<60>,
        up_history: &'a RateHistory<60>,
        theme: Theme,
        endpoint_label: &'a str,
    ) -> Self {
        Self {
            stats,
            down_history,
            up_history,
            theme,
            endpoint_label,
        }
    }
}

impl<'a> Widget for DashboardHeader<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 20 {
            return;
        }

        let title = format!(" rtorrent v{} ", env!("CARGO_PKG_VERSION"));
        let border_type = if self.theme.is_ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(self.theme.border_style(false))
            .style(self.theme.surface_style())
            .title(title)
            .title_alignment(ratatui::layout::Alignment::Left);

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 {
            return;
        }

        if inner.height == 1 {
            // Compact single line header
            let summary = format!(
                "DL: {} | UL: {} | Torrents: {} ({}) | DHT: {} | Port: {}",
                crate::widgets::sparkline::format_rate(self.stats.global_down_bps),
                crate::widgets::sparkline::format_rate(self.stats.global_up_bps),
                self.stats.total_torrents,
                self.stats.active_torrents,
                self.stats.dht_nodes,
                self.stats.listen_port
            );
            buf.set_string(inner.x, inner.y, summary, self.theme.base_style());
            return;
        }

        // Multi-line rich telemetry dashboard
        let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);

        // Row 1: Down sparkline | Up sparkline | Torrent & DHT stats
        let row1_cols = Layout::horizontal([
            Constraint::Percentage(35),
            Constraint::Percentage(35),
            Constraint::Percentage(30),
        ])
        .split(rows[0]);

        RateSparklineWidget::new(
            "DL",
            self.down_history,
            self.stats.global_down_total,
            true,
            self.theme,
        )
        .render(row1_cols[0], buf);

        RateSparklineWidget::new(
            "UL",
            self.up_history,
            self.stats.global_up_total,
            false,
            self.theme,
        )
        .render(row1_cols[1], buf);

        let swarm_str = format!(
            "Torrents: {} ({})  DHT: {}",
            self.stats.total_torrents, self.stats.active_torrents, self.stats.dht_nodes
        );
        buf.set_string(
            row1_cols[2].x,
            row1_cols[2].y,
            swarm_str,
            Style::default().fg(self.theme.foreground),
        );

        // Row 2: Disk Gauge | Connection Info
        if rows.len() > 1 && rows[1].height > 0 {
            let row2_cols =
                Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .split(rows[1]);

            StorageGaugeWidget::new(
                self.stats.disk_free_bytes,
                self.stats.disk_total_bytes,
                self.theme,
            )
            .render(row2_cols[0], buf);

            let port_status = if self.stats.port_open {
                "TCP/UTP OK"
            } else {
                "Checking"
            };
            let port_str = format!(
                "Port: {} [{}]  Endpoint: {}",
                self.stats.listen_port, port_status, self.endpoint_label
            );
            buf.set_string(
                row2_cols[1].x,
                row2_cols[1].y,
                port_str,
                Style::default().fg(self.theme.muted),
            );
        }
    }
}
