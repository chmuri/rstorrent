//! Interactive Theme Picker Modal with live color swatch preview.

use crate::theme::{Theme, ThemeId};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

pub struct ThemePickerModal {
    current_theme_id: ThemeId,
    selected_theme_id: ThemeId,
    active_theme: Theme,
}

impl ThemePickerModal {
    pub fn new(current_theme_id: ThemeId, selected_theme_id: ThemeId) -> Self {
        let active_theme = Theme::by_id(selected_theme_id);
        Self {
            current_theme_id,
            selected_theme_id,
            active_theme,
        }
    }
}

impl Widget for ThemePickerModal {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let modal_width = 65.min(area.width.saturating_sub(4));
        let modal_height = 14.min(area.height.saturating_sub(2));

        if modal_width < 30 || modal_height < 6 {
            return;
        }

        let modal_x = area.x + (area.width.saturating_sub(modal_width)) / 2;
        let modal_y = area.y + (area.height.saturating_sub(modal_height)) / 2;
        let modal_rect = Rect::new(modal_x, modal_y, modal_width, modal_height);

        Clear.render(modal_rect, buf);

        let border_type = if self.active_theme.is_ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(self.active_theme.border_style(true))
            .style(self.active_theme.surface_style())
            .title(" 🎨 Color Theme & Settings ");

        let inner = block.inner(modal_rect);
        block.render(modal_rect, buf);

        let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        // Column 1: Theme List
        buf.set_string(
            cols[0].x + 1,
            cols[0].y,
            "Select Theme:",
            Style::default()
                .fg(self.active_theme.accent)
                .add_modifier(Modifier::BOLD),
        );

        for (i, &theme_id) in ThemeId::ALL.iter().enumerate() {
            let y = cols[0].y + 2 + i as u16;
            if y >= cols[0].bottom().saturating_sub(1) {
                break;
            }

            let is_sel = theme_id == self.selected_theme_id;
            let is_active = theme_id == self.current_theme_id;

            let pointer = if is_sel { "▸ " } else { "  " };
            let active_badge = if is_active { " (Active)" } else { "" };
            let line = format!("{}{}{}", pointer, theme_id.display_name(), active_badge);

            let style = if is_sel {
                Style::default()
                    .fg(self.active_theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else if is_active {
                Style::default().fg(self.active_theme.foreground)
            } else {
                Style::default().fg(self.active_theme.muted)
            };

            buf.set_string(cols[0].x + 1, y, line, style);
        }

        // Column 2: Live Palette Preview
        buf.set_string(
            cols[1].x + 1,
            cols[1].y,
            "Live Palette Preview:",
            Style::default()
                .fg(self.active_theme.accent)
                .add_modifier(Modifier::BOLD),
        );

        let preview_lines = [
            (
                "Accent:",
                "████ Electric",
                Style::default().fg(self.active_theme.accent),
            ),
            (
                "Download:",
                "████ Cyan Speed",
                self.active_theme.download_style(),
            ),
            (
                "Upload:",
                "████ Purple Rate",
                self.active_theme.upload_style(),
            ),
            (
                "Success:",
                "████ Done 100%",
                self.active_theme.success_style(),
            ),
            (
                "Warning:",
                "████ Warning",
                self.active_theme.warning_style(),
            ),
            ("Error:", "████ Error", self.active_theme.error_style()),
        ];

        for (i, (label, val, style)) in preview_lines.iter().enumerate() {
            let y = cols[1].y + 2 + i as u16;
            if y >= cols[1].bottom().saturating_sub(1) {
                break;
            }
            buf.set_string(
                cols[1].x + 1,
                y,
                format!("{:<10} ", label),
                Style::default().fg(self.active_theme.muted),
            );
            buf.set_string(cols[1].x + 12, y, *val, *style);
        }

        // Footer hint
        let footer = "↑/↓: Preview  |  Enter: Save Theme  |  Esc: Cancel";
        buf.set_string(
            inner.x + 1,
            inner.bottom().saturating_sub(1),
            footer,
            Style::default().fg(self.active_theme.muted),
        );
    }
}
