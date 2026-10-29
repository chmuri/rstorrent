//! Torrent list widget: columns, progress bar, focus highlight and viewport.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Widget};

use crate::state::{AppState, TorrentViewModel};

/// Number of text rows the widget spends on the header.
pub const HEADER_ROWS: u16 = 1;

/// Width at which the progress bar and the ratio column appear.
const FULL_WIDTH: u16 = 80;

/// Width at which progress, rates and state appear.
const COMPACT_WIDTH: u16 = 60;

/// Width at which the size column appears.
const SIZE_WIDTH: u16 = 40;

/// Cells used by the progress bar column.
const BAR_WIDTH: usize = 8;

/// One column of the torrent table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// Torrent name.
    Name,
    /// Total size, e.g. `2.0 GB`.
    Size,
    /// Completion in percent.
    Done,
    /// Inline progress bar.
    Bar,
    /// Down rate.
    Down,
    /// Up rate.
    Up,
    /// Human-readable state.
    State,
    /// Share ratio.
    Ratio,
}

impl Column {
    /// Header label for the column.
    pub fn title(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Done => "Done",
            Self::Bar => "Progress",
            Self::Down => "↓ Rate",
            Self::Up => "↑ Rate",
            Self::State => "State",
            Self::Ratio => "Ratio",
        }
    }

    /// Width request handed to the table layout.
    pub fn constraint(self) -> Constraint {
        match self {
            Self::Name => Constraint::Min(12),
            Self::Size => Constraint::Length(8),
            Self::Done => Constraint::Length(5),
            Self::Bar => Constraint::Length(BAR_WIDTH as u16),
            Self::Down | Self::Up => Constraint::Length(9),
            Self::State => Constraint::Length(11),
            Self::Ratio => Constraint::Length(6),
        }
    }
}

/// Columns that fit into a terminal `width`, in display order.
///
/// Narrow terminals lose the least important columns first: bar and ratio below
/// 80 columns, progress and rates below 60, size below 40.
pub fn columns_for_width(width: u16) -> Vec<Column> {
    let full = width >= FULL_WIDTH;
    let compact = width >= COMPACT_WIDTH;

    let mut columns = vec![Column::Name];

    if width >= SIZE_WIDTH {
        columns.push(Column::Size);
    }
    if compact {
        columns.push(Column::Done);
    }
    if full {
        columns.push(Column::Bar);
    }
    if compact {
        columns.push(Column::Down);
        columns.push(Column::Up);
    }
    columns.push(Column::State);
    if full {
        columns.push(Column::Ratio);
    }

    columns
}

/// Number of torrent rows that fit into `height` (header excluded).
pub fn visible_rows(height: u16) -> usize {
    height.saturating_sub(HEADER_ROWS) as usize
}

/// Half-open row range that keeps `focus` inside a `height`-row viewport.
pub fn viewport(focus: usize, len: usize, height: usize) -> std::ops::Range<usize> {
    if height == 0 || len == 0 {
        return 0..0;
    }
    if len <= height {
        return 0..len;
    }

    let start = focus.saturating_sub(height / 2).min(len - height);
    start..(start + height)
}

/// Torrent list.
pub struct DownloadListWidget<'a> {
    state: &'a AppState,
}

impl<'a> DownloadListWidget<'a> {
    /// Builds the widget for the given state.
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl Widget for DownloadListWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let visible = self.state.visible_torrents();
        if visible.is_empty() {
            render_empty(area, buf);
            return;
        }

        let columns = columns_for_width(area.width);
        let width = columns.iter().map(|column| column.constraint());
        let header = Row::new(
            columns
                .iter()
                .map(|column| Cell::from(column.title()))
                .collect::<Vec<_>>(),
        )
        .style(Style::new().add_modifier(Modifier::BOLD));

        let range = viewport(
            self.state.focus_index,
            visible.len(),
            visible_rows(area.height),
        );

        let rows = range
            .map(|index| {
                let torrent = visible[index];
                let cells = columns
                    .iter()
                    .map(|column| cell_for(*column, torrent))
                    .collect::<Vec<_>>();
                let row = Row::new(cells);

                if index == self.state.focus_index {
                    row.style(Style::new().reversed())
                } else {
                    row
                }
            })
            .collect::<Vec<_>>();

        Widget::render(Table::new(rows, width).header(header), area, buf);
    }
}

/// Builds one cell of the torrent table.
fn cell_for(column: Column, torrent: &TorrentViewModel) -> Cell<'_> {
    match column {
        Column::Name => Cell::from(torrent.name.as_str()),
        Column::Size => Cell::from(torrent.size_display.as_str()),
        Column::Done => Cell::from(format!("{:>3.0}%", torrent.progress_percent)),
        Column::Bar => Cell::from(progress_bar(torrent.progress_percent)),
        Column::Down => Cell::from(torrent.down_rate_display.as_str()),
        Column::Up => Cell::from(torrent.up_rate_display.as_str()),
        Column::State => Cell::from(torrent.state_name.as_str()),
        Column::Ratio => Cell::from(torrent.ratio_display.as_str()),
    }
}

/// Rounded progress bar of [`BAR_WIDTH`] cells.
fn progress_bar(percent: f64) -> Line<'static> {
    let ratio = (percent / 100.0).clamp(0.0, 1.0);
    let filled = ((ratio * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);

    Line::from(vec![
        Span::styled("█".repeat(filled), Style::new().fg(Color::Green)),
        Span::styled(
            "░".repeat(BAR_WIDTH - filled),
            Style::new().fg(Color::DarkGray),
        ),
    ])
}

/// Centred "No torrents" placeholder.
fn render_empty(area: Rect, buf: &mut Buffer) {
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);

    Paragraph::new("No torrents")
        .alignment(Alignment::Center)
        .render(middle, buf);
}
