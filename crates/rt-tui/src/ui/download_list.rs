use crate::display::Window;
use crate::input::Bindings;
use crate::windows::WindowDownloadList;
use rt_core::View;
use std::sync::{Arc, Mutex, RwLock};

pub struct DownloadListUI {
    pub window: Arc<Mutex<WindowDownloadList>>,
    pub bindings: Bindings,
    view: Option<Arc<RwLock<View>>>,
}

impl Default for DownloadListUI {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadListUI {
    pub fn new() -> Self {
        let window = Arc::new(Mutex::new(WindowDownloadList::new()));
        let mut ui = Self {
            window,
            bindings: Bindings::new(),
            view: None,
        };
        ui.setup_bindings();
        ui
    }

    pub fn set_view(&mut self, view: Arc<RwLock<View>>) {
        self.view = Some(view.clone());
        self.window.lock().unwrap().set_view(Some(view));
    }

    pub fn current_view(&self) -> Option<Arc<RwLock<View>>> {
        self.view.clone()
    }

    fn setup_bindings(&mut self) {
        // Down / ^N: next torrent
        let win_clone = self.window.clone();
        self.bindings.bind(0x105, move || {
            if let Some(v) = &win_clone.lock().unwrap().view {
                v.write().unwrap().next_focus(1);
                win_clone.lock().unwrap().mark_dirty();
            }
        });

        // Up / ^P: prev torrent
        let win_clone2 = self.window.clone();
        self.bindings.bind(0x104, move || {
            if let Some(v) = &win_clone2.lock().unwrap().view {
                v.write().unwrap().prev_focus(1);
                win_clone2.lock().unwrap().mark_dirty();
            }
        });
    }

    pub fn next(&mut self) {
        if let Some(v) = &self.view {
            v.write().unwrap().next_focus(1);
            self.window.lock().unwrap().mark_dirty();
        }
    }

    pub fn prev(&mut self) {
        if let Some(v) = &self.view {
            v.write().unwrap().prev_focus(1);
            self.window.lock().unwrap().mark_dirty();
        }
    }
}
