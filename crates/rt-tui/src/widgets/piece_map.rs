//! Dynamic adaptive piece matrix and high-density swarm piece map widget.
//!
//! Provides two rendering modes:
//! 1. `AdaptiveMatrix`: Proportional density shading (`█`, `▓`, `▒`, `░`), active in-flight
//!    indicators (`⚙`, `▶`), and coordinate rulers (`0%..100%`, `#XXXX`). Fills the entire
//!    available viewport cleanly without empty voids.
//! 2. `HighResBraille`: Ultra-dense bitpacked Braille cells (8 pieces per character cell).

use crate::state::PieceState;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// Display mode for the piece map widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PieceMapMode {
    #[default]
    AdaptiveMatrix,
    HighResBraille,
}

/// Widget for rendering an intuitive, responsive piece heatmap.
pub struct PieceMapWidget<'a> {
    pieces: &'a [PieceState],
    theme: Theme,
    mode: PieceMapMode,
    show_rulers: bool,
    animation_tick: usize,
}

impl<'a> PieceMapWidget<'a> {
    pub fn new(pieces: &'a [PieceState], theme: Theme) -> Self {
        Self {
            pieces,
            theme,
            mode: PieceMapMode::AdaptiveMatrix,
            show_rulers: true,
            animation_tick: 0,
        }
    }

    /// Sets the rendering mode (Adaptive Matrix vs High-Res Braille).
    pub fn with_mode(mut self, mode: PieceMapMode) -> Self {
        self.mode = mode;
        self
    }

    /// Toggles the position coordinate rulers along the axes.
    pub fn with_rulers(mut self, show: bool) -> Self {
        self.show_rulers = show;
        self
    }

    /// Sets the animation tick counter for pulsating/spinning active transfers.
    pub fn with_animation_tick(mut self, tick: usize) -> Self {
        self.animation_tick = tick;
        self
    }

    /// Evaluates one Braille cell representing 8 pieces starting at piece_offset.
    /// Braille dots arrangement:
    ///   Col 0: Dot 1 (bit 0), Dot 2 (bit 1), Dot 3 (bit 2), Dot 7 (bit 6)
    ///   Col 1: Dot 4 (bit 3), Dot 5 (bit 4), Dot 6 (bit 5), Dot 8 (bit 7)
    #[inline]
    pub fn compute_braille_cell(&self, piece_offset: usize) -> (char, ratatui::style::Color) {
        let mut mask: u32 = 0;
        let mut has_downloading = false;
        let mut has_verified = false;
        let mut has_peer = false;

        let dot_map = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80];

        for (i, &dot_bit) in dot_map.iter().enumerate() {
            let idx = piece_offset + i;
            if idx < self.pieces.len() {
                match self.pieces[idx] {
                    PieceState::Verified => {
                        mask |= dot_bit;
                        has_verified = true;
                    }
                    PieceState::Downloading => {
                        mask |= dot_bit;
                        has_downloading = true;
                    }
                    PieceState::PeerAvailable => {
                        mask |= dot_bit;
                        has_peer = true;
                    }
                    PieceState::Missing => {}
                }
            }
        }

        // char from unicode codepoint 0x2800 + mask
        let ch = char::from_u32(0x2800 + mask).unwrap_or('⠀');

        let color = if has_downloading {
            self.theme.download
        } else if has_verified {
            self.theme.success
        } else if has_peer {
            self.theme.accent
        } else {
            self.theme.muted
        };

        (ch, color)
    }

    /// Renders the legacy/high-density Braille map.
    fn render_braille(&self, area: Rect, buf: &mut Buffer) {
        let mut piece_idx = 0;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if piece_idx >= self.pieces.len() {
                    return;
                }

                let (ch, color) = self.compute_braille_cell(piece_idx);
                let cell = &mut buf[(x, y)];
                cell.set_char(ch).set_fg(color);

                piece_idx += 8;
            }
        }
    }

    /// Renders the modern, responsive adaptive matrix filling the entire viewport.
    fn render_adaptive_matrix(&self, area: Rect, buf: &mut Buffer) {
        let total_pieces = self.pieces.len();
        if total_pieces == 0 {
            return;
        }

        let ruler_w = if self.show_rulers && area.width >= 24 {
            7u16 // " #0000 "
        } else {
            0u16
        };

        let has_top_ruler = self.show_rulers && area.height >= 4;
        let map_y = if has_top_ruler {
            area.y.saturating_add(1)
        } else {
            area.y
        };
        let map_x = area.x.saturating_add(ruler_w);
        let map_w = area.width.saturating_sub(ruler_w);
        let map_h = area.bottom().saturating_sub(map_y);

        if map_w == 0 || map_h == 0 {
            return;
        }

        // Render top percentage ruler if space permits
        if has_top_ruler {
            self.render_top_ruler(area.x, area.y, area.width, ruler_w, buf);
        }

        let total_cells = (map_w as usize).saturating_mul(map_h as usize);
        let pieces_per_cell = total_pieces.div_ceil(total_cells);
        let pieces_per_cell = pieces_per_cell.max(1);

        for row_idx in 0..map_h {
            let cur_y = map_y + row_idx;

            // Render left piece-index ruler
            if ruler_w > 0 {
                let row_start_piece = (row_idx as usize) * (map_w as usize) * pieces_per_cell;
                if row_start_piece < total_pieces {
                    let label = format!("#{:04} ", row_start_piece.min(99999));
                    buf.set_string(area.x, cur_y, &label, self.theme.muted_style());
                } else {
                    buf.set_string(area.x, cur_y, "       ", self.theme.muted_style());
                }
            }

            // Render cells across the row
            for col_idx in 0..map_w {
                let cur_x = map_x + col_idx;
                let cell_idx = (row_idx as usize) * (map_w as usize) + (col_idx as usize);
                let piece_start = cell_idx * pieces_per_cell;

                if piece_start >= total_pieces {
                    // Beyond total piece count
                    continue;
                }

                let piece_end = (piece_start + pieces_per_cell).min(total_pieces);
                let slice = &self.pieces[piece_start..piece_end];

                let mut verified = 0usize;
                let mut downloading = 0usize;
                let mut peer_available = 0usize;

                for &st in slice {
                    match st {
                        PieceState::Verified => verified += 1,
                        PieceState::Downloading => downloading += 1,
                        PieceState::PeerAvailable => peer_available += 1,
                        PieceState::Missing => {}
                    }
                }

                let (ch, color) = if downloading > 0 {
                    // Active in-flight piece indicator with animated tick
                    const ACTIVE_GLYPHS: [char; 4] = ['⚙', '▶', '●', '▲'];
                    let glyph = if self.theme.is_ascii {
                        '>'
                    } else {
                        ACTIVE_GLYPHS[(self.animation_tick + cell_idx) % ACTIVE_GLYPHS.len()]
                    };
                    (glyph, self.theme.download)
                } else if verified == slice.len() {
                    // 100% verified solid block
                    let c = if self.theme.is_ascii { '#' } else { '█' };
                    (c, self.theme.success)
                } else if verified > 0 {
                    // Partially verified density shade
                    let pct = (verified * 100) / slice.len();
                    if pct >= 75 {
                        let c = if self.theme.is_ascii { 'O' } else { '▓' };
                        (c, self.theme.success)
                    } else if pct >= 40 {
                        let c = if self.theme.is_ascii { 'o' } else { '▒' };
                        (c, self.theme.success)
                    } else {
                        let c = if self.theme.is_ascii { '.' } else { '░' };
                        (c, self.theme.accent)
                    }
                } else if peer_available > 0 {
                    // Available in swarm
                    let c = if self.theme.is_ascii { '.' } else { '·' };
                    (c, self.theme.accent)
                } else {
                    // Missing payload
                    let c = if self.theme.is_ascii { ' ' } else { '·' };
                    (c, self.theme.muted)
                };

                let cell = &mut buf[(cur_x, cur_y)];
                cell.set_char(ch).set_fg(color);
            }
        }
    }

    /// Renders the milestone ruler: `0% ─────── 25% ─────── 50% ─────── 75% ─────── 100%`
    fn render_top_ruler(&self, x: u16, y: u16, width: u16, ruler_offset: u16, buf: &mut Buffer) {
        let content_w = width.saturating_sub(ruler_offset) as usize;
        if content_w < 12 {
            return;
        }

        let start_x = x + ruler_offset;
        let mut ruler_str = vec!['─'; content_w];

        // Milestones at 0%, 25%, 50%, 75%, 100%
        let milestones = [
            (0.0, "0%"),
            (0.25, "25%"),
            (0.50, "50%"),
            (0.75, "75%"),
            (1.0, "100%"),
        ];

        for (frac, text) in milestones {
            let pos = (frac * (content_w.saturating_sub(text.len())) as f64).round() as usize;
            if pos + text.len() <= content_w {
                for (offset, ch) in text.chars().enumerate() {
                    ruler_str[pos + offset] = ch;
                }
            }
        }

        let s: String = ruler_str.into_iter().collect();
        buf.set_string(start_x, y, &s, Style::default().fg(self.theme.muted));
    }
}

impl<'a> Widget for PieceMapWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.pieces.is_empty() {
            return;
        }

        match self.mode {
            PieceMapMode::HighResBraille => self.render_braille(area, buf),
            PieceMapMode::AdaptiveMatrix => self.render_adaptive_matrix(area, buf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::THEME_TOKYO_NIGHT;

    #[test]
    fn test_all_verified_braille() {
        let pieces = [PieceState::Verified; 8];
        let widget = PieceMapWidget::new(&pieces, THEME_TOKYO_NIGHT);
        let (ch, color) = widget.compute_braille_cell(0);

        assert_eq!(ch, '⣿'); // 0x2800 + 0xFF = 0x28FF ('⣿')
        assert_eq!(color, THEME_TOKYO_NIGHT.success);
    }

    #[test]
    fn test_empty_braille() {
        let pieces = [PieceState::Missing; 8];
        let widget = PieceMapWidget::new(&pieces, THEME_TOKYO_NIGHT);
        let (ch, color) = widget.compute_braille_cell(0);

        assert_eq!(ch, '⠀'); // blank braille
        assert_eq!(color, THEME_TOKYO_NIGHT.muted);
    }

    #[test]
    fn test_downloading_priority_color() {
        let mut pieces = [PieceState::Verified; 8];
        pieces[3] = PieceState::Downloading;
        let widget = PieceMapWidget::new(&pieces, THEME_TOKYO_NIGHT);
        let (_ch, color) = widget.compute_braille_cell(0);

        assert_eq!(color, THEME_TOKYO_NIGHT.download);
    }

    #[test]
    fn test_adaptive_matrix_rendering() {
        let mut pieces = vec![PieceState::Missing; 100];
        for p in &mut pieces[0..50] {
            *p = PieceState::Verified;
        }
        pieces[75] = PieceState::Downloading;

        let widget = PieceMapWidget::new(&pieces, THEME_TOKYO_NIGHT)
            .with_mode(PieceMapMode::AdaptiveMatrix)
            .with_rulers(true);

        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // Ensure top ruler rendered "0%" and "100%"
        let top_row = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect::<String>();
        assert!(top_row.contains("0%"));
        assert!(top_row.contains("100%"));

        // Ensure left ruler rendered piece number
        let row1_label = (0..7)
            .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
            .collect::<String>();
        assert!(row1_label.contains("#0000"));
    }
}
