//! Add New Torrent / Magnet link modal dialog (a / Ctrl+N).

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

pub struct AddTorrentModal<'a> {
    source_input: &'a str,
    source_cursor: usize,
    dir_input: &'a str,
    dir_cursor: usize,
    start_immediately: bool,
    sequential: bool,
    focused_field: usize, // 0 = source, 1 = dir, 2 = start_opt, 3 = seq_opt, 4 = cancel_btn, 5 = submit_btn
    theme: Theme,
}

impl<'a> AddTorrentModal<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source_input: &'a str,
        source_cursor: usize,
        dir_input: &'a str,
        dir_cursor: usize,
        start_immediately: bool,
        sequential: bool,
        focused_field: usize,
        theme: Theme,
    ) -> Self {
        Self {
            source_input,
            source_cursor,
            dir_input,
            dir_cursor,
            start_immediately,
            sequential,
            focused_field,
            theme,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_text_field(
        buf: &mut Buffer,
        x: u16,
        y: u16,
        max_width: usize,
        text: &str,
        cursor: usize,
        is_active: bool,
        theme: &Theme,
    ) {
        let prefix = if is_active { "❯ " } else { "  " };
        let prefix_style = if is_active {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            theme.muted_style()
        };
        buf.set_string(x, y, prefix, prefix_style);

        let content_x = x + 2;
        let content_width = max_width.saturating_sub(2);
        if content_width == 0 {
            return;
        }

        let char_count = text.chars().count();
        let cur = cursor.min(char_count);

        let scroll_offset = if cur >= content_width {
            cur - content_width + 1
        } else {
            0
        };

        let visible: Vec<char> = text
            .chars()
            .skip(scroll_offset)
            .take(content_width)
            .collect();

        for (i, &ch) in visible.iter().enumerate() {
            let col = content_x + i as u16;
            let is_cursor = is_active && (scroll_offset + i == cur);
            let style = if is_cursor {
                Style::default()
                    .bg(theme.foreground)
                    .fg(theme.background)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            buf.set_string(col, y, ch.to_string(), style);
        }

        if is_active && cur == char_count {
            let cursor_col = content_x + (char_count - scroll_offset) as u16;
            if (cursor_col as usize) < (content_x as usize + content_width) {
                buf.set_string(
                    cursor_col,
                    y,
                    "█",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
    }
}

impl<'a> Widget for AddTorrentModal<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let target_width = (area.width * 75 / 100).clamp(70, 96);
        let modal_width = target_width.min(area.width.saturating_sub(4));
        let modal_height = 14.min(area.height.saturating_sub(2));

        if modal_width < 30 || modal_height < 6 {
            return;
        }

        let modal_x = area.x + (area.width.saturating_sub(modal_width)) / 2;
        let modal_y = area.y + (area.height.saturating_sub(modal_height)) / 2;
        let modal_rect = Rect::new(modal_x, modal_y, modal_width, modal_height);

        Clear.render(modal_rect, buf);

        let border_type = if self.theme.is_ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(self.theme.border_style(true))
            .style(self.theme.surface_style())
            .title(" ➕ Add New Torrent / Magnet ");

        let inner = block.inner(modal_rect);
        block.render(modal_rect, buf);

        // Field 0: Source Input
        let src_active = self.focused_field == 0;
        buf.set_string(
            inner.x + 2,
            inner.y + 1,
            "Source URL, Magnet or .torrent path:",
            Style::default()
                .fg(if src_active {
                    self.theme.accent
                } else {
                    self.theme.foreground
                })
                .add_modifier(Modifier::BOLD),
        );
        Self::render_text_field(
            buf,
            inner.x + 2,
            inner.y + 2,
            (inner.width as usize).saturating_sub(4),
            self.source_input,
            self.source_cursor,
            src_active,
            &self.theme,
        );

        // Field 1: Save Dir Input
        let dir_active = self.focused_field == 1;
        buf.set_string(
            inner.x + 2,
            inner.y + 4,
            "Destination Directory:",
            Style::default()
                .fg(if dir_active {
                    self.theme.accent
                } else {
                    self.theme.foreground
                })
                .add_modifier(Modifier::BOLD),
        );
        Self::render_text_field(
            buf,
            inner.x + 2,
            inner.y + 5,
            (inner.width as usize).saturating_sub(4),
            self.dir_input,
            self.dir_cursor,
            dir_active,
            &self.theme,
        );

        // Field 2 & 3: Options
        let start_box = if self.start_immediately {
            "[x] Start torrent immediately"
        } else {
            "[ ] Start torrent immediately"
        };
        let seq_box = if self.sequential {
            "[x] Sequential download (piece streaming)"
        } else {
            "[ ] Sequential download (piece streaming)"
        };

        let start_style = if self.focused_field == 2 {
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.muted)
        };
        let seq_style = if self.focused_field == 3 {
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.muted)
        };

        buf.set_string(inner.x + 2, inner.y + 7, start_box, start_style);
        buf.set_string(inner.x + 2, inner.y + 8, seq_box, seq_style);

        // Buttons
        let cancel_style = if self.focused_field == 4 {
            Style::default()
                .bg(self.theme.accent)
                .fg(self.theme.background)
                .add_modifier(Modifier::BOLD)
        } else {
            self.theme.muted_style()
        };

        let submit_style = if self.focused_field == 5 {
            Style::default()
                .bg(self.theme.success)
                .fg(self.theme.background)
                .add_modifier(Modifier::BOLD)
        } else {
            self.theme.success_style()
        };

        buf.set_string(
            inner.x + 4,
            inner.bottom().saturating_sub(1),
            " [ Cancel (Esc) ] ",
            cancel_style,
        );
        buf.set_string(
            inner.right().saturating_sub(25),
            inner.bottom().saturating_sub(1),
            " [ ✓ Add Torrent (Enter) ] ",
            submit_style,
        );
    }
}
