//! Top bar: client version, active view and torrent count.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::state::AppState;

/// Width needed for the full `rstorrent vX — [view] N torrents` line.
const VERSION_MIN_WIDTH: u16 = 50;

/// Width needed for `[view] N torrents`.
const COUNT_MIN_WIDTH: u16 = 25;

/// rTorrent-style top bar.
pub struct TitleBarWidget<'a> {
    state: &'a AppState,
}

impl<'a> TitleBarWidget<'a> {
    /// Builds the widget for the given state.
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    /// One line, trimmed from the left as the terminal narrows.
    fn line(&self, width: u16) -> Line<'static> {
        let view = match &self.state.filter {
            Some(filter) if !filter.trim().is_empty() => {
                format!("[{}:{}]", self.state.active_view.as_str(), filter.trim())
            }
            _ => format!("[{}]", self.state.active_view.as_str()),
        };

        let count = self.state.visible_torrents().len();
        if width >= VERSION_MIN_WIDTH {
            Line::from(format!(
                "rstorrent v{} — {view} {}",
                env!("CARGO_PKG_VERSION"),
                torrent_count(count)
            ))
        } else if width >= COUNT_MIN_WIDTH {
            Line::from(format!("{view} {}", torrent_count(count)))
        } else {
            Line::from(view)
        }
    }
}

impl Widget for TitleBarWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(self.line(area.width))
            .style(Style::new().add_modifier(Modifier::BOLD))
            .render(area, buf);
    }
}

/// `1 torrent` / `N torrents`.
fn torrent_count(count: usize) -> String {
    if count == 1 {
        "1 torrent".to_string()
    } else {
        format!("{count} torrents")
    }
}
