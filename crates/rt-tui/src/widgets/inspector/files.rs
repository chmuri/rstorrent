//! Inspector Tab 4: File Priority & Progress List.

use crate::state::FileUiRecord;
use crate::theme::Theme;
use crate::widgets::progress::format_fractional_bar;
use crate::widgets::sparkline::format_size;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Cell, Row, Table, Widget};

pub struct FilesTab<'a> {
    files: &'a [FileUiRecord],
    selected_idx: usize,
    theme: Theme,
}

impl<'a> FilesTab<'a> {
    pub fn new(files: &'a [FileUiRecord], selected_idx: usize, theme: Theme) -> Self {
        Self {
            files,
            selected_idx,
            theme,
        }
    }
}

impl<'a> Widget for FilesTab<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 2 {
            return;
        }

        if self.files.is_empty() {
            buf.set_string(area.x, area.y, "No files found.", self.theme.muted_style());
            return;
        }

        let header = Row::new(vec![
            Cell::from("#").style(self.theme.table_header_style()),
            Cell::from("Priority").style(self.theme.table_header_style()),
            Cell::from("Progress").style(self.theme.table_header_style()),
            Cell::from("Size").style(self.theme.table_header_style()),
            Cell::from("Path").style(self.theme.table_header_style()),
        ]);

        let rows: Vec<Row> = self
            .files
            .iter()
            .take(area.height.saturating_sub(1) as usize)
            .enumerate()
            .map(|(i, f)| {
                let prio_label = match f.priority {
                    0 => "[Off   ]",
                    1 => "[Normal]",
                    _ => "[High  ]",
                };

                let frac = if f.size_bytes == 0 {
                    0.0
                } else {
                    f.completed_bytes as f64 / f.size_bytes as f64
                };

                let bar = format_fractional_bar(frac, 10);
                let prog_str = format!("{} {:>5.1}%", bar, frac * 100.0);

                let is_selected = i == self.selected_idx;
                let row_style = if is_selected {
                    self.theme.selected_row_style()
                } else {
                    Style::default().fg(self.theme.foreground)
                };

                Row::new(vec![
                    Cell::from(f.index.to_string()),
                    Cell::from(prio_label),
                    Cell::from(prog_str),
                    Cell::from(format_size(f.size_bytes)),
                    Cell::from(f.path.clone()),
                ])
                .style(row_style)
            })
            .collect();

        let widths = [
            Constraint::Length(4),
            Constraint::Length(10),
            Constraint::Length(18),
            Constraint::Length(10),
            Constraint::Min(20),
        ];

        let table = Table::new(rows, widths).header(header);
        table.render(area, buf);
    }
}
