//! High-density swarm piece map widget packing 8 pieces per Braille character (U+2800..U+28FF).

use crate::state::PieceState;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

/// Widget for rendering an ultra-dense piece heatmap using Unicode Braille.
pub struct PieceMapWidget<'a> {
    pieces: &'a [PieceState],
    theme: Theme,
}

impl<'a> PieceMapWidget<'a> {
    pub fn new(pieces: &'a [PieceState], theme: Theme) -> Self {
        Self { pieces, theme }
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
}

impl<'a> Widget for PieceMapWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.pieces.is_empty() {
            return;
        }

        let mut piece_idx = 0;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if piece_idx >= self.pieces.len() {
                    return;
                }

                let (ch, color) = self.compute_braille_cell(piece_idx);
                let cell = &mut buf[(x, y)];
                cell.set_char(ch).set_fg(color);

                piece_idx += 8; // 8 pieces packed per character cell!
            }
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
}
