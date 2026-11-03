//! Category views sidebar and tab bar (Zone 2A).

use crate::state::ActiveView;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

pub struct ViewsSidebar {
    active_view: ActiveView,
    counts: [usize; 8],
    theme: Theme,
    is_focused: bool,
}

impl ViewsSidebar {
    pub fn new(
        active_view: ActiveView,
        counts: [usize; 8],
        theme: Theme,
        is_focused: bool,
    ) -> Self {
        Self {
            active_view,
            counts,
            theme,
            is_focused,
        }
    }
}

impl Widget for ViewsSidebar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 8 || area.height < 4 {
            return;
        }

        let border_type = if self.theme.is_ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(self.theme.border_style(self.is_focused))
            .style(self.theme.surface_style())
            .title(" Views ");

        let inner = block.inner(area);
        Clear.render(area, buf);
        block.render(area, buf);

        let views = ActiveView::ALL;
        let inner_width = inner.width as usize;

        for (i, view) in views.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }

            let is_selected = *view == self.active_view;
            let count = self.counts[i];
            let pointer = if is_selected { "▸" } else { " " };
            let num = i + 1;

            let right = format!("{}", count);
            let right_len = right.len();

            // Characters needed for prefix: pointer (1) + '[' (1) + num (1) + '] ' (2) = 5
            let max_label_width = inner_width.saturating_sub(right_len + 5);

            let view_label = match view {
                ActiveView::All => "All",
                ActiveView::Downloading => {
                    if max_label_width >= 11 {
                        "Downloading"
                    } else if max_label_width >= 8 {
                        "Download"
                    } else {
                        "Down"
                    }
                }
                ActiveView::Seeding => {
                    if max_label_width >= 7 {
                        "Seeding"
                    } else {
                        "Seed"
                    }
                }
                ActiveView::Active => "Active",
                ActiveView::Paused => {
                    if max_label_width >= 6 {
                        "Paused"
                    } else {
                        "Pause"
                    }
                }
                ActiveView::Complete => {
                    if max_label_width >= 8 {
                        "Complete"
                    } else {
                        "Done"
                    }
                }
                ActiveView::Error => "Error",
                ActiveView::Hashing => {
                    if max_label_width >= 7 {
                        "Hashing"
                    } else {
                        "Hash"
                    }
                }
            };

            let left = format!("{}[{}] {}", pointer, num, view_label);
            let row_y = inner.y + i as u16;

            let row_style = if is_selected {
                Style::default()
                    .fg(self.theme.accent)
                    .bg(self.theme.surface)
                    .add_modifier(Modifier::BOLD)
            } else if count == 0 {
                Style::default().fg(self.theme.muted)
            } else {
                Style::default().fg(self.theme.foreground)
            };

            // 1. Fill entire row background
            buf.set_stringn(
                inner.x,
                row_y,
                " ".repeat(inner_width),
                inner_width,
                row_style,
            );

            // 2. Left label (guaranteed to end at least 1 space before the right count)
            let max_left = inner_width.saturating_sub(right_len + 1);
            buf.set_stringn(inner.x, row_y, &left, max_left, row_style);

            // 3. Right number (anchored to the right border of inner, guaranteed visible)
            let right_x = inner.right().saturating_sub(right_len as u16);
            buf.set_stringn(right_x, row_y, &right, right_len, row_style);
        }
    }
}
