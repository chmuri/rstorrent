//! Safe Remove Torrent Confirmation Modal with disk space calculation.

use crate::theme::Theme;
use crate::widgets::sparkline::format_size;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

pub struct ConfirmRemoveModal<'a> {
    torrent_names: &'a [String],
    total_bytes: u64,
    delete_files: bool,
    focus_confirm: bool,
    theme: Theme,
}

impl<'a> ConfirmRemoveModal<'a> {
    pub fn new(
        torrent_names: &'a [String],
        total_bytes: u64,
        delete_files: bool,
        focus_confirm: bool,
        theme: Theme,
    ) -> Self {
        Self {
            torrent_names,
            total_bytes,
            delete_files,
            focus_confirm,
            theme,
        }
    }
}

impl<'a> Widget for ConfirmRemoveModal<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let target_width = (area.width * 70 / 100).clamp(68, 90);
        let modal_width = target_width.min(area.width.saturating_sub(4));
        let modal_height = 15.min(area.height.saturating_sub(2));

        if modal_width < 30 || modal_height < 6 {
            return;
        }

        let modal_x = area.x + (area.width.saturating_sub(modal_width)) / 2;
        let modal_y = area.y + (area.height.saturating_sub(modal_height)) / 2;
        let modal_rect = Rect::new(modal_x, modal_y, modal_width, modal_height);

        // Clear backdrop
        Clear.render(modal_rect, buf);

        let border_type = if self.theme.is_ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(if self.delete_files {
                Style::default()
                    .fg(self.theme.error)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(self.theme.warning)
                    .add_modifier(Modifier::BOLD)
            })
            .style(self.theme.surface_style())
            .title(" ⚠ Confirm Torrent Removal ");

        let inner = block.inner(modal_rect);
        block.render(modal_rect, buf);

        let count = self.torrent_names.len();
        let prompt_line = format!("Are you sure you want to remove {} torrent(s)?", count);
        buf.set_string(
            inner.x + 2,
            inner.y + 1,
            prompt_line,
            Style::default()
                .fg(self.theme.foreground)
                .add_modifier(Modifier::BOLD),
        );

        // List up to 3 preview names
        for (i, name) in self.torrent_names.iter().take(3).enumerate() {
            let truncated = if name.len() > (inner.width as usize - 8) {
                format!("• {}…", &name[..inner.width as usize - 10])
            } else {
                format!("• {}", name)
            };
            buf.set_string(
                inner.x + 4,
                inner.y + 3 + i as u16,
                truncated,
                Style::default().fg(self.theme.muted),
            );
        }

        if count > 3 {
            let more = format!("  ... and {} more torrents", count - 3);
            buf.set_string(
                inner.x + 4,
                inner.y + 6,
                more,
                Style::default().fg(self.theme.muted),
            );
        }

        let size_str = format!("Total disk space: {}", format_size(self.total_bytes));
        buf.set_string(
            inner.x + 2,
            inner.y + 7,
            size_str,
            Style::default().fg(self.theme.accent),
        );

        // Destructive delete radio toggle with visual arrow indicator
        let radio_keep = if !self.delete_files {
            "  ▸ (•) Remove from session only (keep data on disk)"
        } else {
            "    ( ) Remove from session only (keep data on disk)"
        };
        let radio_del = if self.delete_files {
            "  ▸ (•) PERMANENTLY DELETE data files from disk"
        } else {
            "    ( ) PERMANENTLY DELETE data files from disk"
        };

        buf.set_string(
            inner.x + 2,
            inner.y + 9,
            radio_keep,
            if !self.delete_files {
                self.theme.success_style().add_modifier(Modifier::BOLD)
            } else {
                self.theme.muted_style()
            },
        );
        buf.set_string(
            inner.x + 2,
            inner.y + 10,
            radio_del,
            if self.delete_files {
                self.theme.error_style().add_modifier(Modifier::BOLD)
            } else {
                self.theme.muted_style()
            },
        );

        // Helper navigation hint line
        buf.set_string(
            inner.x + 3,
            inner.bottom().saturating_sub(2),
            "↑/↓ / j/k: Select Option   Space: Toggle   Esc: Cancel   Enter: Confirm",
            self.theme.muted_style(),
        );

        // Action Buttons: Default focus is on Confirm!
        let cancel_style = if !self.focus_confirm {
            Style::default()
                .bg(self.theme.accent)
                .fg(self.theme.background)
                .add_modifier(Modifier::BOLD)
        } else {
            self.theme.muted_style()
        };

        let confirm_style = if self.focus_confirm {
            if self.delete_files {
                Style::default()
                    .bg(self.theme.error)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .bg(self.theme.warning)
                    .fg(self.theme.background)
                    .add_modifier(Modifier::BOLD)
            }
        } else {
            self.theme.muted_style()
        };

        buf.set_string(
            inner.x + 4,
            inner.bottom().saturating_sub(1),
            " [ Cancel (Esc) ] ",
            cancel_style,
        );
        buf.set_string(
            inner.right().saturating_sub(28),
            inner.bottom().saturating_sub(1),
            " [ Confirm Remove (Enter) ] ",
            confirm_style,
        );
    }
}
