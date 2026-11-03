//! Inspector Tab 1: General Torrent Metadata and Transfer Summary.

use crate::state::TorrentUiRecord;
use crate::theme::Theme;
use crate::widgets::sparkline::{format_rate, format_size};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Widget;

pub struct GeneralInfoTab<'a> {
    record: &'a TorrentUiRecord,
    theme: Theme,
}

impl<'a> GeneralInfoTab<'a> {
    pub fn new(record: &'a TorrentUiRecord, theme: Theme) -> Self {
        Self { record, theme }
    }
}

impl<'a> Widget for GeneralInfoTab<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 20 || area.height < 4 {
            return;
        }

        let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        let col1_lines = [
            format!(
                "Downloaded:   {} / {} ({:.1}%)",
                format_size(self.record.completed_bytes),
                format_size(self.record.size_bytes),
                self.record.progress_percent()
            ),
            format!(
                "Uploaded:     {} (Ratio: {:.2})",
                format_size(self.record.total_uploaded_bytes),
                self.record.ratio()
            ),
            format!("Down Speed:   {}", format_rate(self.record.down_rate_bps)),
            format!("Up Speed:     {}", format_rate(self.record.up_rate_bps)),
            format!(
                "Pieces:       {} / {} verified ({} piece size)",
                self.record.verified_pieces,
                self.record.total_pieces,
                format_size(self.record.piece_size)
            ),
        ];

        let col2_lines = [
            format!("Hash:         {}", self.record.hash_hex),
            format!("Save Path:    {}", self.record.save_path),
            format!(
                "Swarm:        {} seeds / {} peers",
                self.record.seeds_connected, self.record.peers_connected
            ),
            format!(
                "Private:      {}",
                if self.record.is_private {
                    "Yes (DHT disabled)"
                } else {
                    "No (DHT / PEX active)"
                }
            ),
            format!(
                "Message:      {}",
                self.record.message.as_deref().unwrap_or("None")
            ),
        ];

        for (i, line) in col1_lines.iter().enumerate() {
            if i as u16 >= cols[0].height {
                break;
            }
            buf.set_string(
                cols[0].x,
                cols[0].y + i as u16,
                line,
                Style::default().fg(self.theme.foreground),
            );
        }

        for (i, line) in col2_lines.iter().enumerate() {
            if i as u16 >= cols[1].height {
                break;
            }
            buf.set_string(
                cols[1].x,
                cols[1].y + i as u16,
                line,
                Style::default().fg(self.theme.muted),
            );
        }
    }
}
