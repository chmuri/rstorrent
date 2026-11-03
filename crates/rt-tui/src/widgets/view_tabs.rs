//! Horizontal category views segmented tab bar (Zone 2A).

use crate::state::ActiveView;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

pub struct ViewTabsWidget {
    active_view: ActiveView,
    counts: [usize; 8],
    theme: Theme,
    is_focused: bool,
}

impl ViewTabsWidget {
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

    /// Spatial hit-test returning the ActiveView if (x, y) falls inside a tab pill.
    pub fn hit_test(x: u16, y: u16, area: Rect, counts: [usize; 8]) -> Option<ActiveView> {
        if y != area.y || x < area.left() || x >= area.right() {
            return None;
        }

        let tabs = Self::compute_tabs(area.width as usize, counts);
        let mut cur_x = area.x;

        for (i, (view, text, _)) in tabs.into_iter().enumerate() {
            if cur_x >= area.right() {
                break;
            }
            // Count total pill length including padding and pointer
            let pill_len = text.chars().count() + 4; // "   " (3) + " " (1)
            let next_x = cur_x + pill_len as u16;
            let tab_end = if i < 7 { next_x + 1 } else { next_x };
            if x >= cur_x && x < tab_end {
                return Some(view);
            }
            cur_x = tab_end;
        }

        None
    }

    fn compute_tabs(
        available_width: usize,
        counts: [usize; 8],
    ) -> Vec<(ActiveView, String, usize)> {
        let views = ActiveView::ALL;

        let use_full_labels = available_width >= 140;
        let use_medium_labels = available_width >= 105;

        views
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let count = counts[i];
                let count_str = if available_width < 90 && count >= 1000 {
                    format!("{:.0}k", count as f64 / 1000.0)
                } else if available_width < 115 && count >= 10_000 {
                    format!("{:.1}k", count as f64 / 1000.0)
                } else {
                    format!("{}", count)
                };

                let name = match v {
                    ActiveView::All => "All",
                    ActiveView::Downloading => {
                        if use_full_labels {
                            "Downloading"
                        } else if use_medium_labels {
                            "Download"
                        } else {
                            "Down"
                        }
                    }
                    ActiveView::Seeding => {
                        if use_medium_labels {
                            "Seeding"
                        } else {
                            "Seed"
                        }
                    }
                    ActiveView::Active => {
                        if use_medium_labels {
                            "Active"
                        } else {
                            "Act"
                        }
                    }
                    ActiveView::Paused => {
                        if use_medium_labels {
                            "Paused"
                        } else {
                            "Pause"
                        }
                    }
                    ActiveView::Complete => {
                        if use_medium_labels {
                            "Complete"
                        } else {
                            "Done"
                        }
                    }
                    ActiveView::Error => "Error",
                    ActiveView::Hashing => {
                        if use_medium_labels {
                            "Hashing"
                        } else {
                            "Hash"
                        }
                    }
                };

                let num = i + 1;
                let text = format!("[{}] {} ({})", num, name, count_str);
                let text_len = text.chars().count();
                (v, text, text_len)
            })
            .collect()
    }
}

impl Widget for ViewTabsWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 1 {
            return;
        }

        let width = area.width as usize;
        let row_y = area.y;

        // Fill background with theme surface style
        buf.set_stringn(
            area.x,
            row_y,
            " ".repeat(width),
            width,
            self.theme.surface_style(),
        );

        let tabs = Self::compute_tabs(width, self.counts);
        let mut cur_x = area.x;

        let sep = if self.theme.is_ascii { "|" } else { "│" };
        let sep_style = Style::default().fg(self.theme.border);

        for (i, (view, text, _)) in tabs.into_iter().enumerate() {
            if cur_x >= area.right() {
                break;
            }

            let is_selected = view == self.active_view;
            let count = self.counts[i];

            let (pill_text, pill_style) = if is_selected {
                let pointer = if self.is_focused { "▶" } else { "▸" };
                (
                    format!(" {} {} ", pointer, text),
                    Style::default()
                        .bg(self.theme.accent)
                        .fg(self.theme.background)
                        .add_modifier(Modifier::BOLD),
                )
            } else if count == 0 {
                (
                    format!("   {} ", text),
                    Style::default().fg(self.theme.muted).bg(self.theme.surface),
                )
            } else {
                (
                    format!("   {} ", text),
                    Style::default()
                        .fg(self.theme.foreground)
                        .bg(self.theme.surface)
                        .add_modifier(Modifier::BOLD),
                )
            };

            let pill_len = pill_text.chars().count();
            let max_w = (area.right().saturating_sub(cur_x)) as usize;
            buf.set_stringn(cur_x, row_y, &pill_text, max_w, pill_style);
            cur_x += pill_len as u16;

            // Separator between tabs
            if i < 7 && cur_x < area.right() {
                buf.set_string(cur_x, row_y, sep, sep_style);
                cur_x += 1;
            }
        }
    }
}
