use std::sync::{Arc, RwLock};
use lt_common::hash_string::HashString;
use lt_common::Object;
use rt_rpc::parse_commands::parse_command;
use rt_rpc::{CommandMap, Target};
use crate::download::Download;

pub type SlotChanged = Box<dyn Fn() + Send + Sync>;

pub struct View {
    name: String,
    downloads: Vec<Arc<RwLock<Download>>>,
    size_visible: usize,
    focus: usize,

    filter: Object,
    temp_filter: Object,
    event_added: Object,
    event_removed: Object,
    sort_new: Object,
    sort_current: Object,

    filter_on: Vec<String>,
    changed_listeners: Vec<SlotChanged>,
}

impl Default for View {
    fn default() -> Self {
        Self::new()
    }
}

impl View {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            downloads: Vec::new(),
            size_visible: 0,
            focus: 0,
            filter: Object::create_empty(),
            temp_filter: Object::create_empty(),
            event_added: Object::create_empty(),
            event_removed: Object::create_empty(),
            sort_new: Object::create_empty(),
            sort_current: Object::create_empty(),
            filter_on: Vec::new(),
            changed_listeners: Vec::new(),
        }
    }

    pub fn with_name(name: impl Into<String>) -> Self {
        let mut v = Self::new();
        v.initialize(name);
        v
    }

    pub fn initialize(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn size(&self) -> usize {
        self.size_visible
    }

    pub fn size_visible(&self) -> usize {
        self.size_visible
    }

    pub fn size_not_visible(&self) -> usize {
        self.downloads.len().saturating_sub(self.size_visible)
    }

    pub fn empty_visible(&self) -> bool {
        self.size_visible == 0
    }

    pub fn total_size(&self) -> usize {
        self.downloads.len()
    }

    pub fn focus_index(&self) -> usize {
        self.focus
    }

    pub fn focus(&self) -> Option<Arc<RwLock<Download>>> {
        if self.focus < self.size_visible {
            self.downloads.get(self.focus).cloned()
        } else {
            None
        }
    }

    pub fn set_focus_index(&mut self, idx: usize) {
        if self.size_visible == 0 {
            self.focus = 0;
        } else {
            self.focus = idx.min(self.size_visible);
        }
        self.emit_changed();
    }

    pub fn next_focus(&mut self, count: usize) {
        if self.empty_visible() {
            return;
        }
        if self.focus >= self.size_visible {
            self.focus = 0;
        } else {
            self.focus = (self.focus + count).min(self.size_visible);
        }
        self.emit_changed();
    }

    pub fn prev_focus(&mut self, count: usize) {
        if self.empty_visible() {
            return;
        }
        if self.focus == 0 {
            self.focus = self.size_visible.saturating_sub(1);
        } else {
            self.focus = self.focus.saturating_sub(count);
        }
        self.emit_changed();
    }

    pub fn visible_downloads(&self) -> &[Arc<RwLock<Download>>] {
        &self.downloads[..self.size_visible]
    }

    pub fn filtered_downloads(&self) -> &[Arc<RwLock<Download>>] {
        &self.downloads[self.size_visible..]
    }

    pub fn all_downloads(&self) -> &[Arc<RwLock<Download>>] {
        &self.downloads
    }

    pub fn add_changed_listener<F>(&mut self, f: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.changed_listeners.push(Box::new(f));
    }

    fn emit_changed(&self) {
        for listener in &self.changed_listeners {
            listener();
        }
    }

    pub fn insert(&mut self, download: Arc<RwLock<Download>>) {
        let hash = *download.read().unwrap().info_hash();
        if self
            .downloads
            .iter()
            .any(|d| *d.read().unwrap().info_hash() == hash)
        {
            return;
        }
        self.downloads.push(download);
        self.emit_changed();
    }

    pub fn erase(&mut self, hash: &HashString, map: Option<&CommandMap>) {
        if let Some(pos) = self
            .downloads
            .iter()
            .position(|d| *d.read().unwrap().info_hash() == *hash)
        {
            let is_vis = pos < self.size_visible;
            let dl = self.downloads.remove(pos);

            if is_vis {
                self.size_visible = self.size_visible.saturating_sub(1);
                if self.focus > pos {
                    self.focus = self.focus.saturating_sub(1);
                }
                self.focus = self.focus.min(self.size_visible);

                if let Some(m) = map {
                    self.call_event(&self.event_removed, &dl, m);
                }
            }
            self.emit_changed();
        }
    }

    pub fn set_visible(&mut self, hash: &HashString, map: Option<&CommandMap>) {
        if let Some(filtered_idx) = self.downloads[self.size_visible..]
            .iter()
            .position(|d| *d.read().unwrap().info_hash() == *hash)
        {
            let actual_idx = self.size_visible + filtered_idx;
            let dl = self.downloads.remove(actual_idx);
            self.downloads.insert(self.size_visible, Arc::clone(&dl));
            self.size_visible += 1;

            if let Some(m) = map {
                self.call_event(&self.event_added, &dl, m);
            }
            self.emit_changed();
        }
    }

    pub fn set_not_visible(&mut self, hash: &HashString, map: Option<&CommandMap>) {
        if let Some(vis_idx) = self.downloads[..self.size_visible]
            .iter()
            .position(|d| *d.read().unwrap().info_hash() == *hash)
        {
            let dl = self.downloads.remove(vis_idx);
            self.size_visible = self.size_visible.saturating_sub(1);
            if self.focus > vis_idx {
                self.focus = self.focus.saturating_sub(1);
            }
            self.focus = self.focus.min(self.size_visible);

            self.downloads.push(Arc::clone(&dl));

            if let Some(m) = map {
                self.call_event(&self.event_removed, &dl, m);
            }
            self.emit_changed();
        }
    }

    // Setters for commands
    pub fn set_filter(&mut self, cmd: Object) {
        self.filter = cmd;
    }

    pub fn get_filter(&self) -> &Object {
        &self.filter
    }

    pub fn set_filter_temp(&mut self, cmd: Object) {
        self.temp_filter = cmd;
    }

    pub fn get_filter_temp(&self) -> &Object {
        &self.temp_filter
    }

    pub fn set_event_added(&mut self, cmd: Object) {
        self.event_added = cmd;
    }

    pub fn get_event_added(&self) -> &Object {
        &self.event_added
    }

    pub fn set_event_removed(&mut self, cmd: Object) {
        self.event_removed = cmd;
    }

    pub fn get_event_removed(&self) -> &Object {
        &self.event_removed
    }

    pub fn set_sort_new(&mut self, cmd: Object) {
        self.sort_new = cmd;
    }

    pub fn set_sort_current(&mut self, cmd: Object) {
        self.sort_current = cmd;
    }

    pub fn set_filter_on(&mut self, triggers: Vec<String>) {
        self.filter_on = triggers;
    }

    pub fn filter_on(&self) -> &[String] {
        &self.filter_on
    }

    fn eval_filter_cmd(cmd: &Object, dl: &Arc<RwLock<Download>>, map: &CommandMap) -> bool {
        if cmd.is_empty() {
            return true;
        }

        let hash = *dl.read().unwrap().info_hash();
        let target = Target::Download(hash);

        let cmd_str = if let Ok(s) = cmd.as_str() {
            s.to_string()
        } else if let Ok(bytes) = cmd.as_string() {
            String::from_utf8_lossy(bytes).to_string()
        } else {
            return true;
        };

        let cmd_to_run = if cmd_str.contains('=') {
            cmd_str
        } else {
            format!("{}=", cmd_str)
        };

        match parse_command(&cmd_to_run, &target, map, true) {
            Ok((res, _)) => {
                if let Ok(v) = res.as_value() {
                    v != 0
                } else if let Ok(s) = res.as_str() {
                    !s.is_empty()
                } else {
                    true
                }
            }
            Err(_) => false,
        }
    }

    pub fn filter_matches(&self, dl: &Arc<RwLock<Download>>, map: Option<&CommandMap>) -> bool {
        if let Some(m) = map {
            Self::eval_filter_cmd(&self.filter, dl, m)
                && Self::eval_filter_cmd(&self.temp_filter, dl, m)
        } else {
            true
        }
    }

    pub fn filter(&mut self, map: Option<&CommandMap>) {
        if self.name == "started" || self.name == "stopped" {
            return;
        }

        let cur_visible = self.downloads[..self.size_visible].to_vec();
        let cur_filtered = self.downloads[self.size_visible..].to_vec();

        let mut matching_visible = Vec::new();
        let mut non_matching_visible = Vec::new();
        for dl in cur_visible {
            if self.filter_matches(&dl, map) {
                matching_visible.push(dl);
            } else {
                non_matching_visible.push(dl);
            }
        }

        let mut matching_filtered = Vec::new();
        let mut non_matching_filtered = Vec::new();
        for dl in cur_filtered {
            if self.filter_matches(&dl, map) {
                matching_filtered.push(dl);
            } else {
                non_matching_filtered.push(dl);
            }
        }

        // Newly added: matching_filtered
        // Newly removed: non_matching_visible
        let mut new_downloads = Vec::with_capacity(self.downloads.len());
        new_downloads.extend(matching_visible);
        new_downloads.extend(matching_filtered.clone());
        let new_size_visible = new_downloads.len();

        new_downloads.extend(non_matching_visible.clone());
        new_downloads.extend(non_matching_filtered);

        self.downloads = new_downloads;
        self.size_visible = new_size_visible;
        self.focus = self.focus.min(self.size_visible);

        if let Some(m) = map {
            for dl in &non_matching_visible {
                self.call_event(&self.event_removed, dl, m);
            }
            for dl in &matching_filtered {
                self.call_event(&self.event_added, dl, m);
            }
        }

        self.emit_changed();
    }

    pub fn sort(&mut self, _map: Option<&CommandMap>) {
        let cur_focus = self.focus();

        let (visible, _filtered) = self.downloads.split_at_mut(self.size_visible);
        visible.sort_by(|a, b| {
            let name_a = a.read().unwrap().name().to_string();
            let name_b = b.read().unwrap().name().to_string();
            name_a.cmp(&name_b)
        });

        if let Some(target_dl) = cur_focus {
            let target_hash = *target_dl.read().unwrap().info_hash();
            if let Some(new_pos) = self.downloads[..self.size_visible]
                .iter()
                .position(|d| *d.read().unwrap().info_hash() == target_hash)
            {
                self.focus = new_pos;
            }
        }

        self.emit_changed();
    }

    fn call_event(&self, event_cmd: &Object, dl: &Arc<RwLock<Download>>, map: &CommandMap) {
        if event_cmd.is_empty() {
            return;
        }

        let hash = *dl.read().unwrap().info_hash();
        let target = Target::Download(hash);

        let cmd_str = if let Ok(s) = event_cmd.as_str() {
            s.to_string()
        } else if let Ok(bytes) = event_cmd.as_string() {
            String::from_utf8_lossy(bytes).to_string()
        } else {
            return;
        };

        let cmd_to_run = if cmd_str.contains('=') {
            cmd_str
        } else {
            format!("{}=", cmd_str)
        };

        let _ = parse_command(&cmd_to_run, &target, map, true);
    }
}
