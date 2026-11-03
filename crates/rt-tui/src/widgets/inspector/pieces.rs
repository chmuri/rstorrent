//! Inspector Tab 5: Swarm Braille Piece Heatmap with piece summary.

use crate::state::PieceState;
use crate::theme::Theme;
use crate::widgets::piece_map::PieceMapWidget;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Widget;

pub struct PiecesTab<'a> {
    pieces: &'a [PieceState],
    total_pieces: u32,
    verified_pieces: u32,
    theme: Theme,
}

impl<'a> PiecesTab<'a> {
    pub fn new(
        pieces: &'a [PieceState],
        total_pieces: u32,
        verified_pieces: u32,
        theme: Theme,
    ) -> Self {
        Self {
            pieces,
            total_pieces,
            verified_pieces,
            theme,
        }
    }
}

impl<'a> Widget for PiecesTab<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 3 {
            return;
        }

        if self.pieces.is_empty() {
            buf.set_string(
                area.x,
                area.y,
                "Piece data not available or torrent not started.",
                self.theme.muted_style(),
            );
            return;
        }

        let chunks = Layout::vertical([
            Constraint::Length(1), // Summary / Legend
            Constraint::Min(2),    // Braille Map
        ])
        .split(area);

        let verified_pct = if self.total_pieces > 0 {
            (self.verified_pieces as f64 / self.total_pieces as f64) * 100.0
        } else {
            0.0
        };

        let summary = format!(
            "Total Pieces: {} | Verified: {} ({:.1}%) | Legend: ⣿ Verified (Green)  ⣯ In-Flight (Cyan)  ⠀ Missing",
            self.total_pieces, self.verified_pieces, verified_pct
        );
        buf.set_string(
            chunks[0].x,
            chunks[0].y,
            summary,
            Style::default().fg(self.theme.accent),
        );

        PieceMapWidget::new(self.pieces, self.theme).render(chunks[1], buf);
    }
}
