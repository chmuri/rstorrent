//! Interactive Keyboard Shortcuts Cheatsheet Modal (? / F1).

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

pub struct ShortcutsHelpModal {
    theme: Theme,
}

impl ShortcutsHelpModal {
    pub fn new(theme: Theme) -> Self {
        Self { theme }
    }
}

impl Widget for ShortcutsHelpModal {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Dynamic responsive width & height based on available terminal area
        let target_width = (area.width * 85 / 100).clamp(80, 106);
        let modal_width = target_width.min(area.width.saturating_sub(2));

        let target_height = 20;
        let modal_height = target_height.min(area.height.saturating_sub(2));

        if modal_width < 40 || modal_height < 8 {
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
            .title(" ⌨ Keyboard Shortcuts Cheatsheet ");

        let inner = block.inner(modal_rect);
        block.render(modal_rect, buf);

        let col_gap = if inner.width >= 80 { 3 } else { 1 };
        let cols = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(col_gap),
            Constraint::Fill(1),
        ])
        .split(inner);

        // Optional subtle vertical divider in the middle
        if col_gap >= 3 {
            let divider_x = cols[1].x + 1;
            let divider_symbol = if self.theme.is_ascii { "|" } else { "│" };
            let divider_style = Style::default().fg(self.theme.border);
            for y in inner.y + 1..inner.bottom().saturating_sub(2) {
                buf.set_string(divider_x, y, divider_symbol, divider_style);
            }
        }

        let col1_entries = [
            ("NAVIGATION", ""),
            ("  j / ↓", "Move cursor down"),
            ("  k / ↑", "Move cursor up"),
            ("  g / Home", "Jump to first torrent"),
            ("  G / End", "Jump to last torrent"),
            ("  Ctrl+D / PgDn", "Half page down"),
            ("  Ctrl+U / PgUp", "Half page up"),
            ("  Enter / i", "Inspect torrent details"),
            ("", ""),
            ("SELECTION & BULK", ""),
            ("  Space", "Toggle select & move down"),
            ("  v", "Toggle selection in-place"),
            ("  Ctrl+A", "Select all in view"),
            ("  *", "Invert selection"),
            ("  Esc", "Clear selection / Cancel"),
        ];

        let col2_entries = [
            ("TORRENT ACTIONS", ""),
            ("  s", "Start / Resume torrent(s)"),
            ("  p", "Pause / Stop torrent(s)"),
            ("  x / Del", "Remove torrent(s) [Safe modal]"),
            ("  + / -", "Increase / decrease priority"),
            ("  r", "Force hash recheck"),
            ("  a", "Add new torrent / magnet"),
            ("  m", "Toggle sequential download"),
            ("", ""),
            ("VIEWS & CONTROLS", ""),
            ("  1 .. 8", "Quick switch views"),
            ("  [ / ]", "Throttle download (±50 KB/s)"),
            ("  { / }", "Throttle upload (±50 KB/s)"),
            ("  : / Ctrl+P", "Open Command Palette"),
            ("  t", "Open Theme Picker"),
            ("  ?", "Toggle this cheatsheet"),
        ];

        let max_content_y = inner.bottom().saturating_sub(2);

        // Render Column 1
        let col1_w = cols[0].width as usize;
        let col1_key_w = 16.min(col1_w.saturating_sub(12));
        for (i, (k, desc)) in col1_entries.iter().enumerate() {
            let y = cols[0].y + i as u16;
            if y > max_content_y {
                break;
            }
            if desc.is_empty() {
                buf.set_string(
                    cols[0].x + 1,
                    y,
                    *k,
                    Style::default()
                        .fg(self.theme.accent)
                        .add_modifier(Modifier::BOLD),
                );
            } else {
                let key_str = format!("{:<width$}", k, width = col1_key_w);
                buf.set_stringn(
                    cols[0].x + 1,
                    y,
                    &key_str,
                    col1_key_w,
                    Style::default()
                        .fg(self.theme.foreground)
                        .add_modifier(Modifier::BOLD),
                );
                let desc_x = cols[0].x + 1 + col1_key_w as u16 + 1;
                let desc_max_w = (cols[0].right()).saturating_sub(desc_x) as usize;
                buf.set_stringn(
                    desc_x,
                    y,
                    *desc,
                    desc_max_w,
                    Style::default().fg(self.theme.muted),
                );
            }
        }

        // Render Column 2
        let col2_w = cols[2].width as usize;
        let col2_key_w = 14.min(col2_w.saturating_sub(12));
        for (i, (k, desc)) in col2_entries.iter().enumerate() {
            let y = cols[2].y + i as u16;
            if y > max_content_y {
                break;
            }
            if desc.is_empty() {
                buf.set_string(
                    cols[2].x + 1,
                    y,
                    *k,
                    Style::default()
                        .fg(self.theme.accent)
                        .add_modifier(Modifier::BOLD),
                );
            } else {
                let key_str = format!("{:<width$}", k, width = col2_key_w);
                buf.set_stringn(
                    cols[2].x + 1,
                    y,
                    &key_str,
                    col2_key_w,
                    Style::default()
                        .fg(self.theme.foreground)
                        .add_modifier(Modifier::BOLD),
                );
                let desc_x = cols[2].x + 1 + col2_key_w as u16 + 1;
                let desc_max_w = (cols[2].right()).saturating_sub(desc_x) as usize;
                buf.set_stringn(
                    desc_x,
                    y,
                    *desc,
                    desc_max_w,
                    Style::default().fg(self.theme.muted),
                );
            }
        }

        let footer = " [ Esc / ? / q / Enter ] Close cheatsheet ";
        buf.set_string(
            inner.x + 2,
            inner.bottom().saturating_sub(1),
            footer,
            Style::default().fg(self.theme.muted),
        );
    }
}
