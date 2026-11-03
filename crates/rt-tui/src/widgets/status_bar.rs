//! Status and command bar footer widget (Zone 4).

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterMode {
    Normal,
    VisualSelect(usize),
    Filter,
    CommandPalette,
    InspectorFocus,
}

pub struct StatusBarWidget<'a> {
    mode: FooterMode,
    theme: Theme,
    filter_text: Option<&'a str>,
    message: Option<&'a str>,
}

impl<'a> StatusBarWidget<'a> {
    pub fn new(mode: FooterMode, theme: Theme) -> Self {
        Self {
            mode,
            theme,
            filter_text: None,
            message: None,
        }
    }

    pub fn with_filter(mut self, text: Option<&'a str>) -> Self {
        self.filter_text = text;
        self
    }

    pub fn with_message(mut self, msg: Option<&'a str>) -> Self {
        self.message = msg;
        self
    }
}

impl<'a> Widget for StatusBarWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height == 0 {
            return;
        }

        let bg_style = Style::default()
            .bg(self.theme.surface)
            .fg(self.theme.foreground);

        // Fill background
        for x in area.left()..area.right() {
            buf[(x, area.y)].set_style(bg_style);
        }

        let mode_label = match self.mode {
            FooterMode::Normal => (
                " NORMAL ",
                Style::default()
                    .bg(self.theme.accent)
                    .fg(self.theme.background)
                    .add_modifier(Modifier::BOLD),
            ),
            FooterMode::VisualSelect(count) => {
                let s = format!(" VISUAL ({count}) ");
                // leak string or use fixed buffer:
                let style = Style::default()
                    .bg(self.theme.warning)
                    .fg(self.theme.background)
                    .add_modifier(Modifier::BOLD);
                buf.set_string(area.x, area.y, s, style);
                let shortcuts = " [s] Start  [p] Pause  [x] Bulk Delete  [*] Invert  [Esc] Clear ";
                buf.set_string(
                    area.x + 16,
                    area.y,
                    shortcuts,
                    Style::default().fg(self.theme.foreground),
                );
                return;
            }
            FooterMode::Filter => (
                " FILTER ",
                Style::default()
                    .bg(self.theme.download)
                    .fg(self.theme.background)
                    .add_modifier(Modifier::BOLD),
            ),
            FooterMode::CommandPalette => (
                " COMMAND ",
                Style::default()
                    .bg(self.theme.upload)
                    .fg(self.theme.background)
                    .add_modifier(Modifier::BOLD),
            ),
            FooterMode::InspectorFocus => (
                " INSPECT ",
                Style::default()
                    .bg(self.theme.success)
                    .fg(self.theme.background)
                    .add_modifier(Modifier::BOLD),
            ),
        };

        buf.set_string(area.x, area.y, mode_label.0, mode_label.1);
        let x_offset = area.x + mode_label.0.len() as u16 + 1;

        if let Some(msg) = self.message {
            buf.set_string(
                x_offset,
                area.y,
                msg,
                Style::default().fg(self.theme.accent),
            );
            return;
        }

        if let Some(filt) = self.filter_text {
            let filter_display = format!("/{}█", filt);
            buf.set_string(
                x_offset,
                area.y,
                filter_display,
                Style::default()
                    .fg(self.theme.download)
                    .add_modifier(Modifier::BOLD),
            );
            return;
        }

        let hints = match self.mode {
            FooterMode::Normal => " [1-8/h/l] Views  [s] Start  [p] Pause  [x] Remove  [Space] Select  [a] Add  [i/Enter] Inspect  [t] Theme  [/] Filter  [:] Cmd  [?] Help  [q] Quit",
            FooterMode::InspectorFocus => " [1-6/h/l/Tab] Tabs  [j/k] Select  [Space] Priority  [i/Esc] Close/Back",
            _ => "",
        };

        buf.set_string(
            x_offset,
            area.y,
            hints,
            Style::default().fg(self.theme.muted),
        );
    }
}
