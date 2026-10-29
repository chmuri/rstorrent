//! File list widget for the Files tab of the details pane.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Paragraph, Row, Table, Widget};

use crate::state::{FileEntry, FileListState};
use crate::widgets::download_list::{viewport, visible_rows};

/// Colour used for each priority level.
pub fn priority_style(priority: u32) -> Style {
    match priority {
        0 => Style::new().fg(Color::Red),
        1 => Style::new().fg(Color::Green),
        _ => Style::new().fg(Color::Yellow),
    }
}

/// Per-file list with priority colouring.
pub struct FileListWidget<'a> {
    state: &'a FileListState,
}

impl<'a> FileListWidget<'a> {
    /// Builds the widget for the given state.
    pub fn new(state: &'a FileListState) -> Self {
        Self { state }
    }
}

impl Widget for FileListWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.state.entries.is_empty() {
            Paragraph::new("No files").render(area, buf);
            return;
        }

        let header = Row::new(vec!["#", "Path", "Size", "Done", "Priority"])
            .style(Style::new().add_modifier(Modifier::BOLD));

        let range = viewport(
            self.state.focus_index,
            self.state.entries.len(),
            visible_rows(area.height),
        );

        let rows = range
            .map(|index| {
                let entry = &self.state.entries[index];
                let row = Row::new(cells_for(entry));

                if index == self.state.focus_index && !entry.is_directory {
                    row.style(Style::new().reversed())
                } else {
                    row
                }
            })
            .collect::<Vec<_>>();

        let widths = [
            Constraint::Length(5),
            Constraint::Min(12),
            Constraint::Length(9),
            Constraint::Length(6),
            Constraint::Length(9),
        ];

        Widget::render(Table::new(rows, widths).header(header), area, buf);
    }
}

/// Cells of one file row; directory rows only carry a path and a total size.
fn cells_for(entry: &FileEntry) -> Vec<Cell<'_>> {
    if entry.is_directory {
        return vec![
            Cell::from(""),
            Cell::from(entry.path.as_str()).style(Style::new().add_modifier(Modifier::BOLD)),
            Cell::from(entry.size_display.as_str()),
            Cell::from(""),
            Cell::from(""),
        ];
    }

    vec![
        Cell::from(entry.index.to_string()),
        Cell::from(entry.path.as_str()),
        Cell::from(entry.size_display.as_str()),
        Cell::from(format!("{:>3.0}%", entry.completed_percent)),
        Cell::from(Span::styled(
            entry.priority_label(),
            priority_style(entry.priority),
        )),
    ]
}
