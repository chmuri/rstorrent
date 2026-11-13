//! Command palette popup: a query line over the matching commands.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::palette::PaletteState;
use crate::widgets::download_list::viewport;

/// Popup listing the commands that match the current query.
pub struct PaletteWidget<'a> {
    state: &'a PaletteState,
}

impl<'a> PaletteWidget<'a> {
    /// Builds the widget for the given palette state.
    pub fn new(state: &'a PaletteState) -> Self {
        Self { state }
    }
}

impl Widget for PaletteWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 20 || area.height < 5 {
            return;
        }

        let commands = self.state.filtered();
        let width = 60.min(area.width);
        let height = (commands.len() as u16 + 3).min(area.height).max(5);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );

        Clear.render(popup, buf);

        let block = Block::default().borders(Borders::ALL).title(" commands ");
        let inner = block.inner(popup);
        block.render(popup, buf);

        if inner.height < 2 {
            return;
        }

        let query = Line::from(vec![
            Span::styled("> ", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(self.state.query.clone()),
        ]);
        Paragraph::new(query).render(Rect::new(inner.x, inner.y, inner.width, 1), buf);

        let list_area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
        let range = viewport(
            self.state.selected,
            commands.len(),
            usize::from(list_area.height),
        );
        let lines = range
            .map(|index| {
                let command = commands[index];
                let style = if index == self.state.selected {
                    Style::new().reversed()
                } else {
                    Style::new()
                };
                Line::from(Span::styled(
                    format!(
                        "{:<24} {:<9} {:>6}",
                        command.title, command.category, command.shortcut
                    ),
                    style,
                ))
            })
            .collect::<Vec<_>>();

        Paragraph::new(lines).render(list_area, buf);
    }
}
