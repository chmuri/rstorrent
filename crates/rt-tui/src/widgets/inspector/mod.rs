//! Inspector Drawer widget (Zone 3) with interactive tabs.

pub mod files;
pub mod general;
pub mod logs;
pub mod peers;
pub mod pieces;
pub mod trackers;

use crate::state::{TorrentDetailSnapshot, TorrentUiRecord};
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectorTab {
    #[default]
    General = 1,
    Trackers = 2,
    Peers = 3,
    Files = 4,
    PieceMap = 5,
    Logs = 6,
}

impl InspectorTab {
    pub const ALL: [InspectorTab; 6] = [
        InspectorTab::General,
        InspectorTab::Trackers,
        InspectorTab::Peers,
        InspectorTab::Files,
        InspectorTab::PieceMap,
        InspectorTab::Logs,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Self::General => "1:General",
            Self::Trackers => "2:Trackers",
            Self::Peers => "3:Peers",
            Self::Files => "4:Files",
            Self::PieceMap => "★ 5:Piece Map",
            Self::Logs => "6:Logs",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::General => Self::Trackers,
            Self::Trackers => Self::Peers,
            Self::Peers => Self::Files,
            Self::Files => Self::PieceMap,
            Self::PieceMap => Self::Logs,
            Self::Logs => Self::General,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::General => Self::Logs,
            Self::Trackers => Self::General,
            Self::Peers => Self::Trackers,
            Self::Files => Self::Peers,
            Self::PieceMap => Self::Files,
            Self::Logs => Self::PieceMap,
        }
    }
}

pub struct InspectorDrawerWidget<'a> {
    record: &'a TorrentUiRecord,
    details: Option<&'a TorrentDetailSnapshot>,
    active_tab: InspectorTab,
    selected_sub_idx: usize,
    theme: Theme,
    is_focused: bool,
}

impl<'a> InspectorDrawerWidget<'a> {
    pub fn new(
        record: &'a TorrentUiRecord,
        details: Option<&'a TorrentDetailSnapshot>,
        active_tab: InspectorTab,
        selected_sub_idx: usize,
        theme: Theme,
        is_focused: bool,
    ) -> Self {
        Self {
            record,
            details,
            active_tab,
            selected_sub_idx,
            theme,
            is_focused,
        }
    }

    /// Spatial hit-test returning the InspectorTab if (x, y) falls inside an inspector tab pill.
    pub fn hit_test(x: u16, y: u16, area: Rect) -> Option<InspectorTab> {
        if area.width < 20 || area.height < 4 {
            return None;
        }

        // Tab bar is rendered on row area.y + 1 (inside top border)
        let tab_bar_y = area.y + 1;
        if y != tab_bar_y {
            return None;
        }

        let inner_x = area.x + 1;
        let inner_right = area.right().saturating_sub(1);
        let mut cur_x = inner_x;

        for (i, tab) in InspectorTab::ALL.into_iter().enumerate() {
            if cur_x >= inner_right {
                break;
            }
            let tab_text = format!(" [{}] ", tab.title());
            let tab_w = tab_text.chars().count() as u16;
            let next_x = cur_x + tab_w;
            let tab_end = if i < InspectorTab::ALL.len() - 1 {
                next_x + 1
            } else {
                next_x
            };
            if x >= cur_x && x < tab_end {
                return Some(tab);
            }
            cur_x = tab_end;
        }

        None
    }

    /// Checks if (x, y) clicked on the close button `[i: Close]` in title row.
    pub fn is_close_button_hit(x: u16, y: u16, area: Rect) -> bool {
        if area.width < 20 || area.height < 4 {
            return false;
        }
        y == area.y && x >= area.right().saturating_sub(14) && x < area.right()
    }

    /// Checks if (x, y) clicked inside the inspector content body.
    pub fn is_content_hit(x: u16, y: u16, area: Rect) -> bool {
        if area.width < 20 || area.height < 4 {
            return false;
        }
        x > area.left()
            && x < area.right().saturating_sub(1)
            && y >= area.top() + 2
            && y < area.bottom().saturating_sub(1)
    }
}

impl<'a> Widget for InspectorDrawerWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 20 || area.height < 4 {
            return;
        }

        let border_type = if self.theme.is_ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        };

        let title = format!(" Torrent Inspector: {} [i: Close] ", self.record.name);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(self.theme.border_style(self.is_focused))
            .style(self.theme.surface_style())
            .title(title);

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height <= 2 || inner.width == 0 {
            return;
        }

        // Split inner into Tab bar header and Tab content
        let chunks = Layout::vertical([
            Constraint::Length(1), // Tab Bar
            Constraint::Min(2),    // Tab Body
        ])
        .split(inner);

        // Render tab selector
        let mut x = chunks[0].x;
        for tab in InspectorTab::ALL {
            let is_active = tab == self.active_tab;
            let tab_text = format!(" [{}] ", tab.title());

            let style = if is_active {
                Style::default()
                    .fg(self.theme.accent)
                    .bg(self.theme.surface)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(self.theme.muted)
            };

            buf.set_string(x, chunks[0].y, &tab_text, style);
            x += tab_text.chars().count() as u16 + 1;
            if x >= chunks[0].right() {
                break;
            }
        }

        // Render tab body
        let empty_details = TorrentDetailSnapshot::default();
        let details = self.details.unwrap_or(&empty_details);

        match self.active_tab {
            InspectorTab::General => {
                general::GeneralInfoTab::new(self.record, self.theme).render(chunks[1], buf);
            }
            InspectorTab::Trackers => {
                trackers::TrackersTab::new(&details.trackers, self.theme).render(chunks[1], buf);
            }
            InspectorTab::Peers => {
                peers::PeersTab::new(&details.peers, self.theme).render(chunks[1], buf);
            }
            InspectorTab::Files => {
                files::FilesTab::new(&details.files, self.selected_sub_idx, self.theme)
                    .render(chunks[1], buf);
            }
            InspectorTab::PieceMap => {
                pieces::PiecesTab::new(
                    &details.piece_map,
                    self.record.total_pieces,
                    self.record.verified_pieces,
                    self.theme,
                )
                .render(chunks[1], buf);
            }
            InspectorTab::Logs => {
                logs::LogsTab::new(&details.log_messages, self.theme).render(chunks[1], buf);
            }
        }
    }
}
