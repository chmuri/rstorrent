//! Responsive layout calculations for Compact, Standard, and Ultra-Wide terminal sizes.

use ratatui::layout::{Constraint, Layout, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Compact,   // < 100 columns (e.g. 80x24)
    Standard,  // 100..160 columns (e.g. 120x35)
    UltraWide, // >= 160 columns (side-by-side)
}

#[derive(Debug, Clone, Copy)]
pub struct ComputedLayout {
    pub mode: LayoutMode,
    pub header_area: Rect,
    pub view_tabs_area: Rect,
    pub sidebar_area: Option<Rect>,
    pub table_area: Rect,
    pub inspector_area: Option<Rect>,
    pub footer_area: Rect,
}

pub fn dynamic_sidebar_width(terminal_width: u16, max_count: usize) -> u16 {
    let digits = if max_count == 0 {
        1
    } else {
        (max_count as f64).log10().floor() as usize + 1
    };

    // Label "[2] Downloading" needs 16 chars.
    // Plus 2 spaces padding, plus digits, plus 2 border chars.
    let needed = 16 + 2 + digits as u16 + 2;

    let base_width = if terminal_width >= 160 {
        24
    } else if terminal_width >= 120 {
        22
    } else {
        21
    };

    base_width.max(needed).clamp(21, 28)
}

pub fn calculate_responsive_layout(area: Rect, show_inspector: bool) -> ComputedLayout {
    calculate_responsive_layout_dynamic(area, show_inspector, 0)
}

pub fn calculate_responsive_layout_dynamic(
    area: Rect,
    show_inspector: bool,
    _max_count: usize,
) -> ComputedLayout {
    if area.width < 100 {
        // Compact (e.g. 80x24)
        let vertical = Layout::vertical([
            Constraint::Length(3), // Compact Header Dashboard
            Constraint::Length(1), // Category Segmented Tab Bar
            Constraint::Min(6),    // Body (Table + optional Inspector)
            Constraint::Length(1), // Footer
        ])
        .split(area);

        let header_area = vertical[0];
        let view_tabs_area = vertical[1];
        let body_area = vertical[2];
        let footer_area = vertical[3];

        let (table_area, inspector_area) = if show_inspector {
            let body_chunks = Layout::vertical([
                Constraint::Min(6),
                Constraint::Length(10), // Bottom Inspector drawer
            ])
            .split(body_area);
            (body_chunks[0], Some(body_chunks[1]))
        } else {
            (body_area, None)
        };

        ComputedLayout {
            mode: LayoutMode::Compact,
            header_area,
            view_tabs_area,
            sidebar_area: None,
            table_area,
            inspector_area,
            footer_area,
        }
    } else if area.width >= 140 {
        // UltraWide (>= 140 cols): side-by-side inspector if open
        let vertical = Layout::vertical([
            Constraint::Length(4), // Header Dashboard
            Constraint::Length(1), // Category Segmented Tab Bar
            Constraint::Min(8),    // Body (Table + optional Side Inspector)
            Constraint::Length(1), // Footer
        ])
        .split(area);

        let header_area = vertical[0];
        let view_tabs_area = vertical[1];
        let body_area = vertical[2];
        let footer_area = vertical[3];

        let (table_area, inspector_area) = if show_inspector {
            // Horizontal split: 60% table on left, 40% inspector on right
            let body_chunks = Layout::horizontal([
                Constraint::Percentage(60), // Left: Torrent Table
                Constraint::Percentage(40), // Right: Persistent Side Inspector
            ])
            .split(body_area);
            (body_chunks[0], Some(body_chunks[1]))
        } else {
            (body_area, None)
        };

        ComputedLayout {
            mode: LayoutMode::UltraWide,
            header_area,
            view_tabs_area,
            sidebar_area: None,
            table_area,
            inspector_area,
            footer_area,
        }
    } else {
        // Standard Desktop (100..139 cols): bottom inspector drawer if open
        let vertical = Layout::vertical([
            Constraint::Length(4), // Header Dashboard
            Constraint::Length(1), // Category Segmented Tab Bar
            Constraint::Min(8),    // Body (Table + optional Bottom Inspector)
            Constraint::Length(1), // Footer
        ])
        .split(area);

        let header_area = vertical[0];
        let view_tabs_area = vertical[1];
        let body_area = vertical[2];
        let footer_area = vertical[3];

        let (table_area, inspector_area) = if show_inspector {
            let body_chunks = Layout::vertical([
                Constraint::Min(8),
                Constraint::Length(14), // Bottom Inspector drawer
            ])
            .split(body_area);
            (body_chunks[0], Some(body_chunks[1]))
        } else {
            (body_area, None)
        };

        ComputedLayout {
            mode: LayoutMode::Standard,
            header_area,
            view_tabs_area,
            sidebar_area: None,
            table_area,
            inspector_area,
            footer_area,
        }
    }
}
