use crate::display::{Canvas, Window, EXTENT_FULL};
use ratatui::style::{Color, Modifier, Style};
use rt_core::Download;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailTab {
    Info,
    Peers,
    Files,
    Trackers,
}

pub struct WindowDownloadDetails {
    pub flags: u32,
    pub download: Option<Arc<RwLock<Download>>>,
    pub current_tab: DetailTab,
    pub scroll_offset: usize,
    dirty: bool,
}

impl Default for WindowDownloadDetails {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowDownloadDetails {
    pub fn new() -> Self {
        Self {
            flags: 0,
            download: None,
            current_tab: DetailTab::Info,
            scroll_offset: 0,
            dirty: true,
        }
    }

    pub fn set_download(&mut self, download: Option<Arc<RwLock<Download>>>) {
        self.download = download;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    pub fn set_tab(&mut self, tab: DetailTab) {
        self.current_tab = tab;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    pub fn scroll_up(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
            self.dirty = true;
        }
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset += 1;
        self.dirty = true;
    }
}

impl Window for WindowDownloadDetails {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        20
    }

    fn min_height(&self) -> u32 {
        5
    }

    fn max_width(&self) -> u32 {
        EXTENT_FULL
    }

    fn max_height(&self) -> u32 {
        EXTENT_FULL
    }

    fn resize(&mut self, _x: u32, _y: u32, _width: u32, _height: u32) {
        self.dirty = true;
    }

    fn redraw(&mut self, canvas: &mut Canvas) {
        if canvas.is_daemon() {
            return;
        }
        canvas.erase();

        let dl_arc = match &self.download {
            Some(d) => d.clone(),
            None => {
                canvas.print_at(0, 0, "[No download selected]");
                return;
            }
        };

        let dl = dl_arc.read().unwrap();
        let meta = dl.metadata();

        // Header bar with tab navigation
        let header_style = Style::default().bg(Color::Blue).fg(Color::White);
        let tab_indicator = format!(
            " [1: Info{}] [2: Peers{}] [3: Files{}] [4: Trackers{}]  (Left Arrow / Backspace to return) ",
            if self.current_tab == DetailTab::Info { " *" } else { "" },
            if self.current_tab == DetailTab::Peers { " *" } else { "" },
            if self.current_tab == DetailTab::Files { " *" } else { "" },
            if self.current_tab == DetailTab::Trackers { " *" } else { "" },
        );
        let w = canvas.width() as usize;
        let padded_header = format!("{:<width$}", tab_indicator, width = w);
        canvas.print_at_styled(0, 0, &padded_header, header_style);

        let mut y = 2u32;
        let max_y = canvas.height().saturating_sub(1);

        match self.current_tab {
            DetailTab::Info => {
                canvas.print_at_styled(
                    0,
                    y,
                    &format!("Torrent Name:    {}", meta.name),
                    Style::default().add_modifier(Modifier::BOLD),
                );
                y += 1;
                canvas.print_at(0, y, &format!("InfoHash:        {}", dl.info_hash_hex()));
                y += 1;
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "Total Size:      {:.2} MB ({} bytes)",
                        meta.total_size as f64 / (1024.0 * 1024.0),
                        meta.total_size
                    ),
                );
                y += 1;
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "Pieces:          {} chunks x {:.1} KB",
                        meta.pieces.len(),
                        meta.piece_length as f64 / 1024.0
                    ),
                );
                y += 1;
                canvas.print_at(0, y, &format!("Save Directory:  {}", dl.directory()));
                y += 1;
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "State:           {:?} (Active: {}, Done: {})",
                        dl.state(),
                        dl.is_active(),
                        dl.is_done()
                    ),
                );
                y += 1;
                canvas.print_at(0, y, &format!("Priority:        {}", dl.priority()));
                y += 1;
                if !dl.message().is_empty() {
                    canvas.print_at(0, y, &format!("Status Message:  {}", dl.message()));
                }
            }
            DetailTab::Peers => {
                canvas.print_at_styled(
                    0,
                    y,
                    "  IP Address             | Port  | Client               | Flags | Rate Up   | Rate Down | Done % ",
                    Style::default().add_modifier(Modifier::UNDERLINED),
                );
                y += 1;
                canvas.print_at(0, y, "  [No peers currently connected]");
            }
            DetailTab::Files => {
                canvas.print_at_styled(
                    0,
                    y,
                    "  #  | Priority | Size (MB)  | Path ",
                    Style::default().add_modifier(Modifier::UNDERLINED),
                );
                y += 1;

                if meta.files.is_empty() {
                    canvas.print_at(
                        0,
                        y,
                        &format!(
                            "  0  | normal   | {:>10.2} | {}",
                            meta.total_size as f64 / (1024.0 * 1024.0),
                            meta.name
                        ),
                    );
                } else {
                    for (idx, file) in meta.files.iter().enumerate().skip(self.scroll_offset) {
                        if y >= max_y {
                            break;
                        }
                        let path = file.path.components().join("/");
                        let size_mb = file.length as f64 / (1024.0 * 1024.0);
                        canvas.print_at(
                            0,
                            y,
                            &format!(" {:<3} | normal   | {:>10.2} | {}", idx, size_mb, path),
                        );
                        y += 1;
                    }
                }
            }
            DetailTab::Trackers => {
                canvas.print_at_styled(
                    0,
                    y,
                    "  Group | URL                                                                    | Status ",
                    Style::default().add_modifier(Modifier::UNDERLINED),
                );
                y += 1;
                if let Some(announce) = &meta.announce {
                    canvas.print_at(0, y, &format!("    0   | {:<70} | Active ", announce));
                    y += 1;
                }
                for (tier_idx, tier) in meta
                    .announce_list
                    .iter()
                    .enumerate()
                    .skip(self.scroll_offset)
                {
                    for url in tier {
                        if y >= max_y {
                            break;
                        }
                        canvas.print_at(
                            0,
                            y,
                            &format!("   {:>2}   | {:<70} | Active ", tier_idx, url),
                        );
                        y += 1;
                    }
                }
            }
        }

        self.dirty = false;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
