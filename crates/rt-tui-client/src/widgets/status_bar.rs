//! Bottom bar: transfer rates, throttle limits, listen port and connection state.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::state::{AppState, ConnectionStatus};

/// `↑ 1.4 MB ↓ 146.5 KB` plus the indicator needs this many columns.
const RATES_MIN_WIDTH: u16 = 36;

/// Adding `:6881` needs this many columns.
const PORT_MIN_WIDTH: u16 = 50;

/// Adding the throttle limits needs this many columns.
const THROTTLE_MIN_WIDTH: u16 = 72;

/// Below this the indicator loses its label and keeps only the dot.
const LABEL_MIN_WIDTH: u16 = 20;

/// rTorrent-style bottom bar.
pub struct StatusBarWidget<'a> {
    state: &'a AppState,
}

impl<'a> StatusBarWidget<'a> {
    /// Builds the widget for the given state.
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    /// One line; optional fields drop out as the terminal narrows.
    fn line(&self, width: u16) -> Line<'static> {
        let stats = &self.state.session_stats;
        let mut spans: Vec<Span<'static>> = Vec::new();

        if let Some(notice) = &self.state.notice {
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(notice.clone(), Style::new().fg(Color::Red)));
        }

        if width >= RATES_MIN_WIDTH {
            push_segment(
                &mut spans,
                format!(
                    "↑ {} ↓ {}",
                    stats.up_rate_display.trim(),
                    stats.down_rate_display.trim()
                ),
            );
        }
        if width >= THROTTLE_MIN_WIDTH {
            push_segment(
                &mut spans,
                format!(
                    "thr ↑ {} ↓ {}",
                    stats.throttle_up_display.trim(),
                    stats.throttle_down_display.trim()
                ),
            );
        }
        if width >= PORT_MIN_WIDTH {
            push_segment(&mut spans, format!(":{}", stats.listen_port));
        }

        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }

        let (label, color) = connection_display(&self.state.connection);
        spans.push(Span::styled("●", Style::new().fg(color)));
        if width >= LABEL_MIN_WIDTH {
            spans.push(Span::raw(format!(" {label}")));
        }

        Line::from(spans)
    }
}

impl Widget for StatusBarWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(self.line(area.width)).render(area, buf);
    }
}

/// Appends `text` with a two-space separator when the line is not empty.
fn push_segment(spans: &mut Vec<Span<'static>>, text: String) {
    if !spans.is_empty() {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::raw(text));
}

/// Label and colour of the connection indicator.
fn connection_display(status: &ConnectionStatus) -> (&'static str, Color) {
    match status {
        ConnectionStatus::Connected => ("connected", Color::Green),
        ConnectionStatus::Reconnecting => ("reconnecting", Color::Yellow),
        ConnectionStatus::Disconnected { .. } => ("disconnected", Color::Red),
    }
}
