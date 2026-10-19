use crate::display::{Canvas, Window, EXTENT_FULL, FLAG_ACTIVE};
use rt_core::{View, ViewManager};
use std::sync::{Arc, RwLock};

pub struct WindowDownloadList {
    pub flags: u32,
    pub view: Option<Arc<RwLock<View>>>,
    pub view_manager: Option<Arc<RwLock<ViewManager>>>,
    pub view_name: String,
    pub layout: String,
    dirty: bool,
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

    pub fn page_size(&self, height: u32) -> usize {
        let h = match self.layout.as_str() {
            "compact" => 1,
            _ => 3,
        };
        (height / h) as usize
    }

    fn render_view(&self, view: &View, canvas: &mut Canvas) {
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
            format!("[{:5} of {:<5}]", focus_idx + 1, total)
        };

        if canvas.width() > (title_line.len() + counter_str.len() + 2) as u32 {
            let right_x = canvas.width() - counter_str.len() as u32;
            canvas.print_at(right_x, 0, &counter_str);
        }

        let visible = view.visible_downloads();
        if visible.is_empty() || canvas.height() < 2 {
            return;
        }

        let mut y = 1u32;
        if self.layout == "compact" {
            canvas.print_at(0, y, "  Name                                                            | Status     | Downloaded |    Size    | Progress |  Up Rate  | Down Rate |  Uploaded  |    ETA    | Ratio | Misc ");
            y += 1;

            for (idx, dl) in visible.iter().enumerate() {
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
                let percent = if dl_guard.metadata().total_size > 0 {
                    (dl_guard.downloaded_bytes() * 100) / dl_guard.metadata().total_size
                } else {
                    0
                };
                let rate_down_kb = (dl_guard.download().rate_down() as f64) / 1024.0;
                let rate_up_kb = (dl_guard.download().rate_up() as f64) / 1024.0;

                let status_str = if !dl_guard.is_open() {
                    "CLOSED"
                } else if !dl_guard.is_active() {
                    "STOPPED"
                } else if dl_guard.is_done() {
                    "COMPLETED"
                } else {
                    "ACTIVE"
                };
                let line = format!("{} {:<64}| {:<10} | {:7.1} MB | {:7.1} MB | {:3}% | {:6.1} KB | {:6.1} KB | {:7.1} MB | --d --:-- |  0.00 |",
                    mark, name, status_str, down_mb, size_mb, percent, rate_up_kb, rate_down_kb, up_mb);
                canvas.print_at(0, y, &line);
                y += 1;
            }
        } else {
            // Full 3-line layout per torrent
            for (idx, dl) in visible.iter().enumerate() {
                if y + 2 >= canvas.height() {
                    break;
                }
                let is_focused = idx == focus_idx;
                let mark = if is_focused { '*' } else { ' ' };
                let dl_guard = dl.read().unwrap();
                let name = dl_guard.name();
                let total_size_mb = dl_guard.metadata().total_size as f64 / (1024.0 * 1024.0);
                let down_mb = dl_guard.downloaded_bytes() as f64 / (1024.0 * 1024.0);
                let up_mb = dl_guard.uploaded_bytes() as f64 / (1024.0 * 1024.0);
                let percent = if dl_guard.metadata().total_size > 0 {
                    (dl_guard.downloaded_bytes() * 100) / dl_guard.metadata().total_size
                } else {
                    0
                };
                let rate_down_kb = (dl_guard.download().rate_down() as f64) / 1024.0;
                let rate_up_kb = (dl_guard.download().rate_up() as f64) / 1024.0;

                // Line 1: Title
                canvas.print_at(0, y, &format!("{} {}", mark, name));
                y += 1;

                // Line 2: Rates & Size (clear percentage, zero confusing 'done' strings)
                let size_str = format!(
                    "{:8.1} / {:8.1} MB [{:>3}%]",
                    down_mb, total_size_mb, percent
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
                        "{} {} Rate: {:5.1} / {:5.1} KB Uploaded: {:7.1} MB           [   R: {:4.2}]",
                        mark, size_str, rate_down_kb, rate_up_kb, up_mb, ratio
                    ),
                );
                y += 1;

                // Line 3: Clear Status and active peer count
                let peers_count = dl_guard.download().connection_list().size();
                let status_str = if !dl_guard.is_open() {
                    "[CLOSED]".to_string()
                } else if !dl_guard.is_active() {
                    "[STOPPED]".to_string()
                } else if dl_guard.is_done() {
                    format!("[COMPLETED]   Peers: {}", peers_count)
                } else if peers_count > 0 {
                    format!("[DOWNLOADING] Peers: {}", peers_count)
                } else {
                    format!("[CONNECTING]  Peers: 0")
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

        if let Some(v) = &self.view {
            let view_lock = v.read().unwrap();
            self.render_view(&view_lock, canvas);
        } else if let Some(vm_arc) = &self.view_manager {
            let vm = vm_arc.read().unwrap();
            if let Some(view) = vm.find(&self.view_name) {
                self.render_view(view, canvas);
            } else {
                canvas.print_at(0, 0, &format!("[View '{}' not found]", self.view_name));
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
