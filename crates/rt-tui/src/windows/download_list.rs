use crate::display::{Canvas, Window, EXTENT_FULL, FLAG_ACTIVE};
use rt_core::{View, ViewManager};
use std::sync::{Arc, RwLock};

/// Percentage of payload bytes covered by verified pieces.
///
/// Uses completed bytes rather than the session transfer counter, which is
/// restored independently and restarts at zero.
pub fn progress_percent(completed: u64, total: u64) -> u64 {
    if total == 0 {
        0
    } else {
        completed.saturating_mul(100).saturating_div(total).min(100)
    }
}

/// Status label shared by the compact and full layouts.
pub fn status_label(
    is_open: bool,
    is_active: bool,
    is_done: bool,
    is_hash_checking: bool,
    chunks_hashed: u32,
    num_pieces: usize,
) -> String {
    if is_hash_checking {
        format!("HASHING {}/{}", chunks_hashed, num_pieces)
    } else if !is_open {
        "CLOSED".to_string()
    } else if !is_active {
        "STOPPED".to_string()
    } else if is_done {
        "COMPLETED".to_string()
    } else {
        "ACTIVE".to_string()
    }
}

/// First item to render so `focus` is visible in a window of `capacity` rows.
///
/// The window moves only when the focus leaves it, which keeps the list stable
/// while moving inside the visible page, and never scrolls past the point where
/// the last page is full.
pub fn scroll_window(focus: usize, current: usize, capacity: usize, total: usize) -> usize {
    if capacity == 0 || total == 0 {
        return 0;
    }
    let capacity = capacity.min(total);
    let max_start = total - capacity;
    let mut start = current.min(max_start);
    if focus < start {
        start = focus;
    } else if focus >= start + capacity {
        start = focus + 1 - capacity;
    }
    start.min(max_start)
}

pub struct WindowDownloadList {
    pub flags: u32,
    pub view: Option<Arc<RwLock<View>>>,
    pub view_manager: Option<Arc<RwLock<ViewManager>>>,
    pub view_name: String,
    pub layout: String,
    dirty: bool,
    /// Index of the first rendered download. Adjusted on every render so the
    /// focused row stays on screen.
    scroll: usize,
    /// Rows that fit in the last render; used by PageUp/PageDown.
    page: usize,
}

impl Default for WindowDownloadList {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowDownloadList {
    pub fn new() -> Self {
        Self {
            flags: FLAG_ACTIVE,
            view: None,
            view_manager: None,
            view_name: "main".to_string(),
            layout: "full".to_string(),
            dirty: true,
            scroll: 0,
            page: 1,
        }
    }

    pub fn set_view(&mut self, view: Option<Arc<RwLock<View>>>) {
        self.view = view;
        self.dirty = true;
    }

    pub fn set_view_manager(
        &mut self,
        vm: Option<Arc<RwLock<ViewManager>>>,
        name: impl Into<String>,
    ) {
        self.view_manager = vm;
        self.view_name = name.into();
        self.dirty = true;
    }

    pub fn set_view_name(&mut self, name: impl Into<String>) {
        self.view_name = name.into();
        self.dirty = true;
    }

    pub fn set_layout(&mut self, layout: impl Into<String>) {
        self.layout = layout.into();
        self.dirty = true;
    }

    /// Rows visible in the last render. Never zero, so it is safe as a page
    /// step for PageUp/PageDown.
    pub fn page_len(&self) -> usize {
        self.page.max(1)
    }

    /// First rendered download index.
    pub fn scroll_offset(&self) -> usize {
        self.scroll
    }

    fn render_view(&mut self, view: &View, canvas: &mut Canvas) {
        let view_name = view.name();
        let filtered_str = if !view.get_filter_temp().is_empty() {
            " (filtered)"
        } else {
            ""
        };
        let title_line = format!("[View: {}{}]", view_name, filtered_str);
        canvas.print_at(0, 0, &title_line);

        let total = view.size();
        let focus_idx = view.focus_index();
        let counter_str = if total == 0 {
            "[ none of 0    ]".to_string()
        } else {
            // `focus == total` is the "no selection" sentinel; never render it
            // as position N+1.
            format!("[{:5} of {:<5}]", (focus_idx + 1).min(total), total)
        };

        if canvas.width() > (title_line.len() + counter_str.len() + 2) as u32 {
            let right_x = canvas.width() - counter_str.len() as u32;
            canvas.print_at(right_x, 0, &counter_str);
        }

        let visible = view.visible_downloads();
        if visible.is_empty() || canvas.height() < 2 {
            return;
        }

        let item_height = if self.layout == "compact" { 1 } else { 3 };
        let mut y = 1u32;
        if self.layout == "compact" {
            canvas.print_at(0, y, "  Name                                                            | Status     | Downloaded |    Size    | Progress |  Up Rate  | Down Rate |  Uploaded  |    ETA    | Ratio | Misc ");
            y += 1;
        }

        // Keep the focused row on screen: move the window only when the focus
        // leaves it, then clamp so the last page stays full.
        let capacity = (canvas.height().saturating_sub(y) / item_height) as usize;
        self.page = capacity.max(1);
        // The sentinel focus (nothing selected) still scrolls to the end, but
        // must not paint a focus marker on the last row.
        let scroll_focus = focus_idx.min(visible.len() - 1);
        let start = scroll_window(scroll_focus, self.scroll, self.page, visible.len());
        self.scroll = start;
        let end = (start + self.page).min(visible.len());

        if self.layout == "compact" {
            for (offset, dl) in visible[start..end].iter().enumerate() {
                let idx = start + offset;
                if y >= canvas.height() {
                    break;
                }
                let is_focused = idx == focus_idx;
                let mark = if is_focused { '*' } else { ' ' };
                let dl_guard = dl.read().unwrap();
                let name = dl_guard.name();
                let size_mb = dl_guard.metadata().total_size as f64 / (1024.0 * 1024.0);
                let down_mb = dl_guard.downloaded_bytes() as f64 / (1024.0 * 1024.0);
                let up_mb = dl_guard.uploaded_bytes() as f64 / (1024.0 * 1024.0);
                let percent =
                    progress_percent(dl_guard.completed_bytes(), dl_guard.metadata().total_size);
                let rate_down_kb = (dl_guard.download().rate_down() as f64) / 1024.0;
                let rate_up_kb = (dl_guard.download().rate_up() as f64) / 1024.0;

                let status_str = status_label(
                    dl_guard.is_open(),
                    dl_guard.is_active(),
                    dl_guard.is_done(),
                    dl_guard.is_hash_checking(),
                    dl_guard.chunks_hashed(),
                    dl_guard.metadata().num_pieces,
                );
                let line = format!("{} {:<64}| {:<10} | {:7.1} MB | {:7.1} MB | {:3}% | {:6.1} KB | {:6.1} KB | {:7.1} MB | --d --:-- |  0.00 |",
                    mark, name, status_str, down_mb, size_mb, percent, rate_up_kb, rate_down_kb, up_mb);
                canvas.print_at(0, y, &line);
                y += 1;
            }
        } else {
            // Full 3-line layout per torrent
            for (offset, dl) in visible[start..end].iter().enumerate() {
                let idx = start + offset;
                if y + 2 >= canvas.height() {
                    break;
                }
                let is_focused = idx == focus_idx;
                let mark = if is_focused { '*' } else { ' ' };
                let dl_guard = dl.read().unwrap();
                let name = dl_guard.name();
                let total_size_mb = dl_guard.metadata().total_size as f64 / (1024.0 * 1024.0);
                let done_mb = dl_guard.completed_bytes() as f64 / (1024.0 * 1024.0);
                let up_mb = dl_guard.uploaded_bytes() as f64 / (1024.0 * 1024.0);
                let percent =
                    progress_percent(dl_guard.completed_bytes(), dl_guard.metadata().total_size);
                let rate_down_kb = (dl_guard.download().rate_down() as f64) / 1024.0;
                let rate_up_kb = (dl_guard.download().rate_up() as f64) / 1024.0;

                // Line 1: Title
                canvas.print_at(0, y, &format!("{} {}", mark, name));
                y += 1;

                // Line 2: verified progress, rates and uploaded total.
                let size_str = format!(
                    "{:8.1} / {:8.1} MB [{:>3}%]",
                    done_mb, total_size_mb, percent
                );
                let ratio = if dl_guard.downloaded_bytes() > 0 {
                    (dl_guard.uploaded_bytes() as f64) / (dl_guard.downloaded_bytes() as f64)
                } else {
                    0.0
                };
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "{} {} Rate: {:5.1} / {:5.1} KB Uploaded: {:7.1} MB Downloaded: {:7.1} MB [   R: {:4.2}]",
                        mark, size_str, rate_down_kb, rate_up_kb, up_mb,
                        dl_guard.downloaded_bytes() as f64 / (1024.0 * 1024.0), ratio
                    ),
                );
                y += 1;

                // Line 3: Clear Status and active peer count
                let peers_count = dl_guard.download().connection_list().size();
                let label = status_label(
                    dl_guard.is_open(),
                    dl_guard.is_active(),
                    dl_guard.is_done(),
                    dl_guard.is_hash_checking(),
                    dl_guard.chunks_hashed(),
                    dl_guard.metadata().num_pieces,
                );
                let status_str = if label == "HASHING" || label.starts_with("HASHING ") {
                    format!("[{}] Peers: {}", label, peers_count)
                } else if !dl_guard.is_open() {
                    "[CLOSED]".to_string()
                } else if !dl_guard.is_active() {
                    "[STOPPED]".to_string()
                } else if dl_guard.is_done() {
                    format!("[COMPLETED]   Peers: {}", peers_count)
                } else if peers_count > 0 {
                    format!("[DOWNLOADING] Peers: {}", peers_count)
                } else {
                    "[CONNECTING]  Peers: 0".to_string()
                };
                canvas.print_at(0, y, &format!("{} {}", mark, status_str));
                y += 1;
            }
        }
    }
}

impl Window for WindowDownloadList {
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

        let view = self.view.clone();
        let view_manager = self.view_manager.clone();
        let view_name = self.view_name.clone();

        if let Some(v) = view {
            let view_lock = v.read().unwrap();
            self.render_view(&view_lock, canvas);
        } else if let Some(vm_arc) = view_manager {
            let vm = vm_arc.read().unwrap();
            if let Some(view) = vm.find(&view_name) {
                self.render_view(view, canvas);
            } else {
                canvas.print_at(0, 0, &format!("[View '{}' not found]", view_name));
            }
        } else {
            canvas.print_at(0, 0, "[No View]");
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
