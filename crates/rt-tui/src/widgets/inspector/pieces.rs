//! Inspector Tab 5: Dynamic Swarm Piece Heatmap, Telemetry HUD, and In-Flight Pipeline.

use crate::state::{InFlightPieceRecord, PieceState, TorrentUiRecord, TorrentUiState};
use crate::theme::Theme;
use crate::widgets::piece_map::{PieceMapMode, PieceMapWidget};
use crate::widgets::progress::format_fractional_bar;
use crate::widgets::sparkline::{format_rate, format_size};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

pub struct PiecesTab<'a> {
    record: &'a TorrentUiRecord,
    pieces: &'a [PieceState],
    in_flight: &'a [InFlightPieceRecord],
    theme: Theme,
}

impl<'a> PiecesTab<'a> {
    pub fn new(
        record: &'a TorrentUiRecord,
        pieces: &'a [PieceState],
        in_flight: &'a [InFlightPieceRecord],
        theme: Theme,
    ) -> Self {
        Self {
            record,
            pieces,
            in_flight,
            theme,
        }
    }

    fn format_eta(eta_secs: Option<u64>, state: TorrentUiState) -> String {
        match state {
            TorrentUiState::Complete | TorrentUiState::Seeding => "Done".into(),
            TorrentUiState::Paused => "Paused".into(),
            TorrentUiState::Error => "Error".into(),
            TorrentUiState::Checking => "Checking".into(),
            _ => {
                if let Some(secs) = eta_secs {
                    if secs >= 86400 {
                        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
                    } else if secs >= 3600 {
                        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
                    } else if secs >= 60 {
                        format!("{}m {:02}s", secs / 60, secs % 60)
                    } else {
                        format!("{}s", secs)
                    }
                } else {
                    "∞".into()
                }
            }
        }
    }
}

impl<'a> Widget for PiecesTab<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 3 {
            return;
        }

        if self.pieces.is_empty() && self.record.total_pieces == 0 {
            buf.set_string(
                area.x,
                area.y,
                "Piece data not available or torrent not started.",
                self.theme.muted_style(),
            );
            return;
        }

        let verified_count = self.record.verified_pieces;
        let total_count = self.record.total_pieces;
        let remaining_pieces = total_count.saturating_sub(verified_count);
        let progress_frac = if total_count > 0 {
            (verified_count as f64 / total_count as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let verified_pct = progress_frac * 100.0;

        let remaining_bytes = self
            .record
            .size_bytes
            .saturating_sub(self.record.completed_bytes);

        // Layout zones:
        // Top: HUD metrics card (3 rows)
        // Middle: Adaptive Responsive Matrix (Min 3 rows)
        // Lower: In-Flight Pipeline (2..4 rows depending on space)
        // Bottom: Legend (1 row)
        let has_in_flight_panel = area.height >= 14;
        let in_flight_height = if has_in_flight_panel {
            if !self.in_flight.is_empty() {
                (self.in_flight.len() as u16 + 1).clamp(2, 4)
            } else {
                2
            }
        } else {
            0
        };

        let chunks = Layout::vertical([
            Constraint::Length(3),                // HUD Metrics
            Constraint::Min(4),                   // Adaptive Piece Matrix
            Constraint::Length(in_flight_height), // In-Flight Pipeline
            Constraint::Length(1),                // Legend
        ])
        .split(area);

        // --- Zone 1: HUD Metrics ---
        let hud_area = chunks[0];
        let bar_width = (hud_area.width as usize).saturating_sub(32).clamp(10, 48);
        let bar_str = format_fractional_bar(progress_frac, bar_width);

        // Row 0: Progress bar + Percentage
        let row0_left = format!(
            "[{}] {:>5.1}% ({} / {} pcs) ",
            bar_str, verified_pct, verified_count, total_count
        );
        let (badge_icon, badge_text) = self.record.state.badge();
        let badge_str = format!("[{} {}]", badge_icon, badge_text);

        buf.set_string(
            hud_area.x,
            hud_area.y,
            &row0_left,
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        let badge_x = hud_area.right().saturating_sub(badge_str.len() as u16);
        if badge_x > hud_area.x + row0_left.len() as u16 {
            buf.set_string(
                badge_x,
                hud_area.y,
                &badge_str,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            );
        }

        // Row 1: Bytes + Remaining + Piece Size
        let row1_str = if hud_area.width >= 90 {
            format!(
                "Verified: {} / {}  •  Remaining: {} pcs ({})  •  Piece Size: {}",
                format_size(self.record.completed_bytes),
                format_size(self.record.size_bytes),
                remaining_pieces,
                format_size(remaining_bytes),
                format_size(self.record.piece_size),
            )
        } else {
            format!(
                "Done: {} / {} • Left: {} pcs ({}) • Pc: {}",
                format_size(self.record.completed_bytes),
                format_size(self.record.size_bytes),
                remaining_pieces,
                format_size(remaining_bytes),
                format_size(self.record.piece_size),
            )
        };
        buf.set_string(
            hud_area.x,
            hud_area.y + 1,
            &row1_str,
            Style::default().fg(self.theme.foreground),
        );

        // Row 2: Speed + ETA + Active In-Flight summary
        let total_active_blocks: u32 = self
            .in_flight
            .iter()
            .map(|f| (f.blocks_total.saturating_sub(f.blocks_completed)) as u32)
            .sum();
        let row2_str = if hud_area.width >= 90 {
            format!(
                "Down: {}  •  Up: {}  •  ETA: {}  •  In-Flight: {} pcs ({} active blocks)",
                format_rate(self.record.down_rate_bps),
                format_rate(self.record.up_rate_bps),
                Self::format_eta(self.record.eta_seconds, self.record.state),
                self.in_flight.len(),
                total_active_blocks
            )
        } else {
            format!(
                "Down: {} • Up: {} • ETA: {} • In-Flight: {} pcs ({} blk)",
                format_rate(self.record.down_rate_bps),
                format_rate(self.record.up_rate_bps),
                Self::format_eta(self.record.eta_seconds, self.record.state),
                self.in_flight.len(),
                total_active_blocks
            )
        };
        buf.set_string(
            hud_area.x,
            hud_area.y + 2,
            &row2_str,
            Style::default().fg(self.theme.muted),
        );

        // --- Zone 2: Adaptive Piece Matrix ---
        PieceMapWidget::new(self.pieces, self.theme)
            .with_mode(PieceMapMode::AdaptiveMatrix)
            .with_rulers(true)
            .render(chunks[1], buf);

        // --- Zone 3: Active In-Flight Pipeline ---
        if has_in_flight_panel && in_flight_height > 0 {
            let pipe_area = chunks[2];
            let title = format!(
                "⚡ ACTIVE IN-FLIGHT PIPELINE ({} pieces downloading)",
                self.in_flight.len()
            );
            buf.set_string(
                pipe_area.x,
                pipe_area.y,
                &title,
                Style::default()
                    .fg(self.theme.download)
                    .add_modifier(Modifier::BOLD),
            );

            if self.in_flight.is_empty() {
                let note = if verified_count == total_count && total_count > 0 {
                    "✔ All pieces verified and written to disk. Seeding to swarm."
                } else {
                    "⏸ No active in-flight requests. Ready to request from peers."
                };
                buf.set_string(
                    pipe_area.x + 2,
                    pipe_area.y + 1,
                    note,
                    self.theme.muted_style(),
                );
            } else {
                let max_rows = (in_flight_height.saturating_sub(1)) as usize;
                for (idx, active) in self.in_flight.iter().take(max_rows).enumerate() {
                    let cur_y = pipe_area.y + 1 + idx as u16;
                    let frac = if active.blocks_total > 0 {
                        (active.blocks_completed as f64 / active.blocks_total as f64)
                            .clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let mini_bar = format_fractional_bar(frac, 12);
                    let peer_info = active.peer_addr.as_deref().unwrap_or("Swarm");
                    let line = format!(
                        "  ⚙ Pc #{:<5} [{}] {:>3.0}% ({}/{} blk) │ Peer: {}",
                        active.piece_index,
                        mini_bar,
                        frac * 100.0,
                        active.blocks_completed,
                        active.blocks_total,
                        peer_info
                    );
                    buf.set_string(
                        pipe_area.x,
                        cur_y,
                        &line,
                        Style::default().fg(self.theme.foreground),
                    );
                }
            }
        }

        // --- Zone 4: Legend & View Info ---
        let legend_area = chunks[3];
        let legend =
            "Legend: █ Verified (100%)  ▓ 75%  ▒ 50%  ░ 25%  ⚙ In-Flight  · Swarm Available";
        buf.set_string(
            legend_area.x,
            legend_area.y,
            legend,
            Style::default().fg(self.theme.muted),
        );
    }
}
