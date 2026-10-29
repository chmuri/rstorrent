//! Torrent details pane: tab bar plus the Info tab body.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::state::{DetailTab, DetailsInfo, DetailsState};
use crate::widgets::file_list::FileListWidget;
use crate::widgets::peer_list::PeerListWidget;
use crate::widgets::tracker_list::TrackerListWidget;

/// Details pane for one torrent.
pub struct DetailsWidget<'a> {
    state: &'a DetailsState,
    tab: DetailTab,
}

impl<'a> DetailsWidget<'a> {
    /// Builds the widget for `state`, rendering `tab`.
    pub fn new(state: &'a DetailsState, tab: DetailTab) -> Self {
        Self { state, tab }
    }
}

impl Widget for DetailsWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let [tabs_area, body_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);

        render_tab_bar(self.tab, tabs_area, buf);

        match self.tab {
            DetailTab::Info => render_info(&self.state.info, body_area, buf),
            DetailTab::Files => {
                FileListWidget::new(&self.state.files).render(body_area, buf);
            }
            DetailTab::Peers => {
                PeerListWidget::new(&self.state.peers).render(body_area, buf);
            }
            DetailTab::Trackers => {
                TrackerListWidget::new(&self.state.trackers).render(body_area, buf);
            }
            DetailTab::Chunks => {
                render_count(
                    "Chunks",
                    self.state.info.piece_count as usize,
                    body_area,
                    buf,
                );
            }
        }
    }
}

/// `[1:Info] [2:Files] …`, with the active tab inverted.
fn render_tab_bar(active: DetailTab, area: Rect, buf: &mut Buffer) {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(DetailTab::ALL.len() * 2);

    for tab in DetailTab::ALL {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }

        let label = format!("[{}:{}]", tab.number(), tab.title());
        if tab == active {
            spans.push(Span::styled(label, Style::new().reversed()));
        } else {
            spans.push(Span::raw(label));
        }
    }

    Paragraph::new(Line::from(spans)).render(area, buf);
}

/// Label/value lines of the Info tab.
fn render_info(info: &DetailsInfo, area: Rect, buf: &mut Buffer) {
    let pieces = info.piece_count.to_string();
    let private = if info.private { "yes" } else { "no" };

    let lines = vec![
        info_line("Hash", &info.hash),
        info_line("Name", &info.name),
        info_line("State", &info.state_name),
        info_line("Size", &info.size_display),
        info_line("Piece length", &info.piece_length_display),
        info_line("Pieces", &pieces),
        info_line("Directory", &info.directory),
        info_line("Private", private),
        info_line("Seed", &info.connection_seed),
        info_line("Throttle", &info.throttle_name),
        info_line("Message", &info.message),
    ];

    Paragraph::new(lines).render(area, buf);
}

/// One bold label followed by its value.
fn info_line<'a>(label: &'static str, value: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{label:<13}"),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw(value),
    ])
}

/// Placeholder body for the tabs that TASK-08/09 will fill.
fn render_count(title: &str, count: usize, area: Rect, buf: &mut Buffer) {
    let label = if count == 1 {
        format!("{title}: 1 entry")
    } else {
        format!("{title}: {count} entries")
    };

    Paragraph::new(label).render(area, buf);
}
