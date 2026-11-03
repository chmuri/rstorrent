//! Command Palette fuzzy finder modal (Zone : / Ctrl+P).

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

#[derive(Debug, Clone)]
pub struct CommandItem {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub shortcut: &'static str,
}

pub const ALL_COMMANDS: [CommandItem; 17] = [
    CommandItem {
        id: "torrent.start",
        title: "Start / Resume Torrent",
        category: "Torrent",
        shortcut: "s",
    },
    CommandItem {
        id: "torrent.pause",
        title: "Pause / Stop Torrent",
        category: "Torrent",
        shortcut: "p",
    },
    CommandItem {
        id: "torrent.remove",
        title: "Remove Torrent...",
        category: "Torrent",
        shortcut: "x",
    },
    CommandItem {
        id: "torrent.recheck",
        title: "Force Hash Recheck",
        category: "Torrent",
        shortcut: "r",
    },
    CommandItem {
        id: "torrent.add",
        title: "Add New Torrent / Magnet...",
        category: "Torrent",
        shortcut: "a",
    },
    CommandItem {
        id: "torrent.sequential",
        title: "Toggle Sequential Download",
        category: "Torrent",
        shortcut: "m",
    },
    CommandItem {
        id: "view.all",
        title: "Switch View: All",
        category: "View",
        shortcut: "1",
    },
    CommandItem {
        id: "view.down",
        title: "Switch View: Downloading",
        category: "View",
        shortcut: "2",
    },
    CommandItem {
        id: "view.seed",
        title: "Switch View: Seeding",
        category: "View",
        shortcut: "3",
    },
    CommandItem {
        id: "view.active",
        title: "Switch View: Active",
        category: "View",
        shortcut: "4",
    },
    CommandItem {
        id: "view.paused",
        title: "Switch View: Paused",
        category: "View",
        shortcut: "5",
    },
    CommandItem {
        id: "view.complete",
        title: "Switch View: Complete",
        category: "View",
        shortcut: "6",
    },
    CommandItem {
        id: "bandwidth.down_up",
        title: "Increase Download Throttle (+50 KB/s)",
        category: "Bandwidth",
        shortcut: "]",
    },
    CommandItem {
        id: "bandwidth.down_down",
        title: "Decrease Download Throttle (-50 KB/s)",
        category: "Bandwidth",
        shortcut: "[",
    },
    CommandItem {
        id: "theme.select",
        title: "Open Theme Picker...",
        category: "Theme",
        shortcut: "t",
    },
    CommandItem {
        id: "help.shortcuts",
        title: "Open Shortcuts Cheatsheet...",
        category: "Help",
        shortcut: "?",
    },
    CommandItem {
        id: "session.quit",
        title: "Quit rtorrent",
        category: "Session",
        shortcut: "q",
    },
];

pub struct CommandPaletteModal<'a> {
    query: &'a str,
    selected_idx: usize,
    theme: Theme,
}

impl<'a> CommandPaletteModal<'a> {
    pub fn new(query: &'a str, selected_idx: usize, theme: Theme) -> Self {
        Self {
            query,
            selected_idx,
            theme,
        }
    }

    pub fn filtered_commands(&self) -> Vec<&'static CommandItem> {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            ALL_COMMANDS.iter().collect()
        } else {
            ALL_COMMANDS
                .iter()
                .filter(|c| {
                    c.title.to_lowercase().contains(&q)
                        || c.category.to_lowercase().contains(&q)
                        || c.shortcut.to_lowercase().contains(&q)
                })
                .collect()
        }
    }
}

impl<'a> Widget for CommandPaletteModal<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let target_width = (area.width * 60 / 100).clamp(65, 84);
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
            .title(" ❯ Command Palette ");

        let inner = block.inner(modal_rect);
        block.render(modal_rect, buf);

        if inner.height < 3 || inner.width < 10 {
            return;
        }

        // Input prompt line
        let prompt_line = format!("❯ {}█", self.query);
        buf.set_string(
            inner.x + 1,
            inner.y,
            prompt_line,
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD),
        );

        // Horizontal separator line
        for x in inner.left()..inner.right() {
            buf[(x, inner.y + 1)]
                .set_char('─')
                .set_fg(self.theme.border);
        }

        let filtered = self.filtered_commands();
        let max_items = (inner.height.saturating_sub(3)) as usize;
        let selected = self.selected_idx.min(filtered.len().saturating_sub(1));

        for (i, cmd) in filtered.iter().take(max_items).enumerate() {
            let y = inner.y + 2 + i as u16;
            let is_sel = i == selected;

            let line = format!("[{:<9}] {}", cmd.category, cmd.title);
            let style = if is_sel {
                Style::default()
                    .bg(self.theme.surface)
                    .fg(self.theme.foreground)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.muted)
            };

            buf.set_string(inner.x + 1, y, line, style);

            let shortcut_hint = format!("[ {} ]", cmd.shortcut);
            let sx = inner.right().saturating_sub(shortcut_hint.len() as u16 + 1);
            buf.set_string(sx, y, shortcut_hint, Style::default().fg(self.theme.accent));
        }

        // Footer hint
        let footer = "↑/↓: Select  |  Enter: Execute  |  Esc: Close";
        buf.set_string(
            inner.x + 1,
            inner.bottom().saturating_sub(1),
            footer,
            Style::default().fg(self.theme.muted),
        );
    }
}
