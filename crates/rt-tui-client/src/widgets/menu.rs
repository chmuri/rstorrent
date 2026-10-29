//! Context menu popup.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::state::MenuState;
use crate::widgets::download_list::viewport;

/// Popup listing the actions available for the focused torrent.
pub struct MenuWidget<'a> {
    state: &'a MenuState,
}

impl<'a> MenuWidget<'a> {
    /// Builds the widget for the given menu state.
    pub fn new(state: &'a MenuState) -> Self {
        Self { state }
    }
}

impl Widget for MenuWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.state.is_empty() || area.width < 6 || area.height < 3 {
            return;
        }

        let label_width = self
            .state
            .entries()
            .iter()
            .map(|entry| entry.label.chars().count())
            .max()
            .unwrap_or(8) as u16;
        let width = (label_width + 4).min(area.width);
        let height = (self.state.len() as u16 + 2).min(area.height);

        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );

        Clear.render(popup, buf);

        let block = Block::default().borders(Borders::ALL).title(" menu ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        let range = viewport(
            self.state.selected(),
            self.state.len(),
            usize::from(inner.height),
        );

        let lines = range
            .map(|index| {
                let entry = &self.state.entries()[index];
                let style = if !entry.enabled {
                    Style::new().fg(Color::DarkGray)
                } else if index == self.state.selected() {
                    Style::new().reversed()
                } else {
                    Style::new()
                };

                Line::from(Span::styled(entry.label.clone(), style))
            })
            .collect::<Vec<_>>();

        Paragraph::new(lines).render(inner, buf);
    }
}
