use std::sync::{Arc, RwLock};
use lt_common::Object;
use rt_rpc::CommandMap;
use crate::download::Download;
use crate::view::View;

pub struct ViewManager {
    views: Vec<View>,
}

impl Default for ViewManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewManager {
    pub fn new() -> Self {
        let mut vm = Self { views: Vec::new() };
        vm.initialize_default_views();
        vm
    }

    fn initialize_default_views(&mut self) {
        let default_names = [
            "main",
            "default",
            "name",
            "active",
            "started",
            "stopped",
            "complete",
            "incomplete",
            "hashing",
            "seeding",
            "leeching",
        ];

        for name in default_names {
            self.insert(name);
        }
    }

    pub fn len(&self) -> usize {
        self.views.len()
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    pub fn clear(&mut self) {
        self.views.clear();
    }

    pub fn insert(&mut self, name: &str) -> &mut View {
        if let Some(pos) = self.views.iter().position(|v| v.name() == name) {
            return &mut self.views[pos];
        }

        let view = View::with_name(name);
        self.views.push(view);
        self.views.last_mut().unwrap()
    }

    pub fn find(&self, name: &str) -> Option<&View> {
        self.views.iter().find(|v| v.name() == name)
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut View> {
        self.views.iter_mut().find(|v| v.name() == name)
    }

    pub fn all(&self) -> &[View] {
        &self.views
    }

    pub fn all_mut(&mut self) -> &mut [View] {
        &mut self.views
    }

    pub fn insert_download_to_all(&mut self, download: Arc<RwLock<Download>>) {
        let hash = *download.read().unwrap().info_hash();
        for view in &mut self.views {
            view.insert(Arc::clone(&download));
            if view.get_filter().is_empty() {
                view.set_visible(&hash, None);
            }
        }
    }

    pub fn erase_download_from_all(&mut self, hash: &lt_common::HashString, map: Option<&CommandMap>) {
        for view in &mut self.views {
            view.erase(hash, map);
        }
    }

    pub fn filter_on_trigger(&mut self, trigger: &str, map: Option<&CommandMap>) {
        for view in &mut self.views {
            if view.filter_on().iter().any(|t| t == trigger) {
                view.filter(map);
            }
        }
    }

    pub fn sort(&mut self, name: &str, map: Option<&CommandMap>) {
        if let Some(view) = self.find_mut(name) {
            view.sort(map);
        }
    }

    pub fn set_sort_new(&mut self, name: &str, cmd: Object) {
        if let Some(view) = self.find_mut(name) {
            view.set_sort_new(cmd);
        }
    }

    pub fn set_sort_current(&mut self, name: &str, cmd: Object) {
        if let Some(view) = self.find_mut(name) {
            view.set_sort_current(cmd);
        }
    }

    pub fn set_filter(&mut self, name: &str, cmd: Object) {
        if let Some(view) = self.find_mut(name) {
            view.set_filter(cmd);
        }
    }

    pub fn set_filter_temp(&mut self, name: &str, cmd: Object) {
        if let Some(view) = self.find_mut(name) {
            view.set_filter_temp(cmd);
        }
    }

    pub fn set_event_added(&mut self, name: &str, cmd: Object) {
        if let Some(view) = self.find_mut(name) {
            view.set_event_added(cmd);
        }
    }

    pub fn set_event_removed(&mut self, name: &str, cmd: Object) {
        if let Some(view) = self.find_mut(name) {
            view.set_event_removed(cmd);
        }
    }

    pub fn set_filter_on(&mut self, name: &str, triggers: Vec<String>) {
        if let Some(view) = self.find_mut(name) {
            view.set_filter_on(triggers);
        }
    }
}
