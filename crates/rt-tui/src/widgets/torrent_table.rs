//! Main torrent table widget (Zone 2B) with smooth fractional progress and rich styling.

use crate::state::{TorrentUiRecord, TorrentUiState};
use crate::theme::Theme;
use crate::widgets::progress::format_fractional_bar;
use crate::widgets::sparkline::{format_rate, format_size};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Clear, Row, Table, Widget};
use std::collections::HashSet;

pub struct TorrentTableWidget<'a> {
    torrents: &'a [TorrentUiRecord],
    visible_indices: &'a [usize],
    selected_row: usize,
    multi_selected: &'a HashSet<[u8; 20]>,
    theme: Theme,
    is_focused: bool,
    filter_label: Option<&'a str>,
}

impl<'a> TorrentTableWidget<'a> {
    pub fn new(
        torrents: &'a [TorrentUiRecord],
        visible_indices: &'a [usize],
        selected_row: usize,
        multi_selected: &'a HashSet<[u8; 20]>,
        theme: Theme,
        is_focused: bool,
    ) -> Self {
        Self {
            torrents,
            visible_indices,
            selected_row,
            multi_selected,
            theme,
            is_focused,
            filter_label: None,
        }
    }

    pub fn with_filter_label(mut self, label: Option<&'a str>) -> Self {
        self.filter_label = label;
        self
    }

    fn format_eta(eta_secs: Option<u64>, state: TorrentUiState) -> String {
        match state {
            TorrentUiState::Complete | TorrentUiState::Seeding => "Seeding".into(),
            TorrentUiState::Paused => "Paused".into(),
            TorrentUiState::Checking => "Checking".into(),
            TorrentUiState::Error => "Error".into(),
            _ => match eta_secs {
                None => "-".into(),
                Some(s) if s > 86400 => format!("{}d", s / 86400),
                Some(s) if s > 3600 => format!("{:02}h {:02}m", s / 3600, (s % 3600) / 60),
                Some(s) => format!("{:02}m {:02}s", s / 60, s % 60),
            },
        }
    }

    fn ratio_style(&self, ratio: f64) -> Style {
        if ratio >= 2.0 {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD)
        } else if ratio >= 1.0 {
            self.theme.success_style()
        } else if ratio >= 0.5 {
            self.theme.warning_style()
        } else {
            self.theme.error_style()
        }
    }
}

impl<'a> Widget for TorrentTableWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 20 || area.height < 4 {
            return;
        }

        let is_compact = area.width < 110;
        let border_type = if self.theme.is_ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        };

        let title = match self.filter_label {
            Some(f) if !f.is_empty() => format!(
                " Torrents [Filter: {} | Total: {}] ",
                f,
                self.visible_indices.len()
            ),
            _ => format!(" Torrents [Total: {}] ", self.visible_indices.len()),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(self.theme.border_style(self.is_focused))
            .style(self.theme.surface_style())
            .title(title);

        let inner = block.inner(area);
        block.render(area, buf);
        Clear.render(inner, buf);

        if inner.height <= 1 || inner.width == 0 {
            return;
        }

        // Build header
        let header = if is_compact {
            Row::new(vec![
                Cell::from("S").style(self.theme.table_header_style()),
                Cell::from("Name").style(self.theme.table_header_style()),
                Cell::from("Progress").style(self.theme.table_header_style()),
                Cell::from("Size").style(self.theme.table_header_style()),
                Cell::from("Down").style(self.theme.table_header_style()),
                Cell::from("ETA").style(self.theme.table_header_style()),
            ])
        } else {
            Row::new(vec![
                Cell::from("S").style(self.theme.table_header_style()),
                Cell::from("Name").style(self.theme.table_header_style()),
                Cell::from("Progress").style(self.theme.table_header_style()),
                Cell::from("Size").style(self.theme.table_header_style()),
                Cell::from("Down").style(self.theme.table_header_style()),
                Cell::from("Up").style(self.theme.table_header_style()),
                Cell::from("S/P").style(self.theme.table_header_style()),
                Cell::from("ETA").style(self.theme.table_header_style()),
                Cell::from("Ratio").style(self.theme.table_header_style()),
            ])
        };

        let visible_rows_count = (inner.height.saturating_sub(1)) as usize;
        let scroll_offset = if self.selected_row >= visible_rows_count {
            self.selected_row - visible_rows_count + 1
        } else {
            0
        };

        let rows: Vec<Row> = self
            .visible_indices
            .iter()
            .skip(scroll_offset)
            .take(visible_rows_count)
            .enumerate()
            .map(|(display_idx, &torrent_idx)| {
                let actual_row_idx = scroll_offset + display_idx;
                let rec = &self.torrents[torrent_idx];
                let is_current = actual_row_idx == self.selected_row;
                let is_checked = self.multi_selected.contains(&rec.hash);

                let (status_icon, _) = rec.state.badge();
                let status_style = match rec.state {
                    TorrentUiState::Downloading => self.theme.download_style(),
                    TorrentUiState::Seeding | TorrentUiState::Complete => {
                        self.theme.success_style()
                    }
                    TorrentUiState::Active => self.theme.accent.into(),
                    TorrentUiState::Paused => self.theme.muted_style(),
                    TorrentUiState::Checking => self.theme.warning_style(),
                    TorrentUiState::Error => self.theme.error_style(),
                };

                let prefix = if is_checked {
                    "[✔] "
                } else if is_current {
                    "▸ "
                } else {
                    "  "
                };

                let name_cell = format!("{}{}", prefix, rec.name);
                let bar_width = if is_compact { 10 } else { 12 };
                let bar_str = format_fractional_bar(rec.progress_fraction(), bar_width);
                let progress_cell = format!("{} {:>5.1}%", bar_str, rec.progress_percent());

                let size_cell = format_size(rec.size_bytes);
                let down_cell = if rec.down_rate_bps > 0 {
                    format_rate(rec.down_rate_bps)
                } else {
                    "0 B/s".into()
                };
                let up_cell = if rec.up_rate_bps > 0 {
                    format_rate(rec.up_rate_bps)
                } else {
                    "0 B/s".into()
                };
                let sp_cell = format!("{}/{}", rec.seeds_connected, rec.seeds_total);
                let eta_cell = Self::format_eta(rec.eta_seconds, rec.state);
                let ratio_cell = format!("{:.2}", rec.ratio());

                let row_style = if is_current {
                    self.theme.selected_row_style()
                } else if is_checked {
                    self.theme.visual_select_style()
                } else {
                    Style::default().fg(self.theme.foreground)
                };

                if is_compact {
                    Row::new(vec![
                        Cell::from(status_icon).style(status_style),
                        Cell::from(name_cell),
                        Cell::from(progress_cell).style(status_style),
                        Cell::from(size_cell),
                        Cell::from(down_cell).style(self.theme.download_style()),
                        Cell::from(eta_cell),
                    ])
                    .style(row_style)
                } else {
                    Row::new(vec![
                        Cell::from(status_icon).style(status_style),
                        Cell::from(name_cell),
                        Cell::from(progress_cell).style(status_style),
                        Cell::from(size_cell),
                        Cell::from(down_cell).style(self.theme.download_style()),
                        Cell::from(up_cell).style(self.theme.upload_style()),
                        Cell::from(sp_cell),
                        Cell::from(eta_cell),
                        Cell::from(ratio_cell).style(self.ratio_style(rec.ratio())),
                    ])
                    .style(row_style)
                }
            })
            .collect();

        let widths = if is_compact {
            vec![
                Constraint::Length(2),
                Constraint::Min(30),
                Constraint::Length(18),
                Constraint::Length(9),
                Constraint::Length(10),
                Constraint::Length(9),
            ]
        } else {
            vec![
                Constraint::Length(2),
                Constraint::Min(36),
                Constraint::Length(20),
                Constraint::Length(9),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Length(7),
            ]
        };

        let table = Table::new(rows, widths).header(header);
        table.render(inner, buf);
    }
}
