use crate::display::{Canvas, ColorKind, ColorMap, Frame, FrameNode, Window};
use crate::input::{
    Bindings, InputHistory, InputManager, KeymapStyle, NavigationKey, PathInput, KEY_DOWN, KEY_UP,
    MAX_INPUT_HISTORY_SIZE,
};
use crate::windows::chunks_seen::ChunksSeenWindow;
use crate::windows::menu::{download_context_menu, Menu, MenuElement, MenuOutcome};
use crate::windows::peer_list::{PeerActionOutcome, PeerListWindow};
use crate::windows::tracker_list::{TrackerAction, TrackerActionOutcome, TrackerListWindow};
use crate::windows::transfer_list::TransferListWindow;
use crate::windows::{
    DetailTab, FilePriority, WindowDownloadDetails, WindowDownloadList, WindowFileList,
    WindowInput, WindowLog, WindowStatusbar, WindowTitle,
};
use lt_common::HashString;
use ratatui::style::{Color, Modifier, Style};
use rt_core::{Download, DownloadList, View, ViewManager};
use rt_rpc::CommandMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveScreen {
    DownloadList,
    DownloadDetails,
    FileList,
    PeerList,
    TrackerList,
    ChunksSeen,
    TransferList,
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPromptMode {
    None,
    LoadStart,
    LoadNormal,
    Command,
    Filter,
}

pub struct RootUI {
    pub window_title: Arc<Mutex<WindowTitle>>,
    pub window_download_list: Arc<Mutex<WindowDownloadList>>,
    pub window_details: Arc<Mutex<WindowDownloadDetails>>,
    pub window_file_list: Arc<Mutex<WindowFileList>>,
    pub window_peer_list: Arc<Mutex<PeerListWindow>>,
    pub window_tracker_list: Arc<Mutex<TrackerListWindow>>,
    pub window_chunks_seen: Arc<Mutex<ChunksSeenWindow>>,
    pub window_transfer_list: Arc<Mutex<TransferListWindow>>,
    pub window_menu: Option<Arc<Mutex<MenuElement>>>,
    pub command_map: Option<Arc<RwLock<CommandMap>>>,
    pub window_log: Arc<Mutex<WindowLog>>,
    pub window_input: Arc<Mutex<WindowInput>>,
    pub window_statusbar: Arc<Mutex<WindowStatusbar>>,

    pub root_frame: Frame,
    pub input_manager: Arc<Mutex<InputManager>>,
    pub global_bindings: Arc<Mutex<Bindings>>,

    pub active_screen: ActiveScreen,
    pub input_mode: InputPromptMode,
    active_path_input: Option<Arc<Mutex<PathInput>>>,

    pub view_manager: Option<Arc<RwLock<ViewManager>>>,
    pub download_list: Option<Arc<RwLock<DownloadList>>>,
    pub current_view_name: String,

    pub throttle_up: u32,
    pub throttle_down: u32,
    pub rate_up: f64,
    pub rate_down: f64,
    pub listen_port: u16,

    pub shutdown_requested: bool,
    pub log_messages: Vec<String>,

    pub color_map: Arc<RwLock<ColorMap>>,
    pub color_definitions: HashMap<String, String>,
    pub input_history: InputHistory,
    pub input_history_limit: usize,
    /// Query used by incremental history search (Ctrl-R/Ctrl-S).
    input_history_search: Option<String>,
    pub keymap_style: String,
    pub torrent_list_layout: String,
    pub focus_page_size: u32,
    pub status_throttle_up: String,
    pub status_throttle_down: String,
    pub throttle_step_small: u32,
    pub throttle_step_medium: u32,
    pub throttle_step_large: u32,
}

impl Default for RootUI {
    fn default() -> Self {
        Self::new()
    }
}

impl RootUI {
    pub fn new() -> Self {
        let window_title = Arc::new(Mutex::new(WindowTitle::new("rstorrent")));
        let window_download_list = Arc::new(Mutex::new(WindowDownloadList::new()));
        let window_details = Arc::new(Mutex::new(WindowDownloadDetails::new()));
        let window_file_list = Arc::new(Mutex::new(WindowFileList::new()));
        let window_peer_list = Arc::new(Mutex::new(PeerListWindow::new()));
        let window_tracker_list = Arc::new(Mutex::new(TrackerListWindow::new()));
        let window_chunks_seen = Arc::new(Mutex::new(ChunksSeenWindow::new()));
        let window_transfer_list = Arc::new(Mutex::new(TransferListWindow::new()));
        let window_log = Arc::new(Mutex::new(WindowLog::new()));
        let window_input = Arc::new(Mutex::new(WindowInput::new()));
        let window_statusbar = Arc::new(Mutex::new(WindowStatusbar::new()));

        let input_manager = Arc::new(Mutex::new(InputManager::new()));
        let global_bindings = Arc::new(Mutex::new(Bindings::new()));

        input_manager
            .lock()
            .unwrap()
            .push_bindings(global_bindings.clone());

        let root_frame = Frame::with_row(vec![
            Frame::with_window(window_title.clone()),
            Frame::with_window(window_download_list.clone()),
            Frame::with_window(window_input.clone()),
            Frame::with_window(window_statusbar.clone()),
        ]);

        let mut ui = Self {
            window_title,
            window_download_list,
            window_details,
            window_file_list,
            window_peer_list,
            window_tracker_list,
            window_chunks_seen,
            window_transfer_list,
            window_menu: None,
            command_map: None,
            window_log,
            window_input,
            window_statusbar,
            root_frame,
            input_manager,
            global_bindings,
            active_screen: ActiveScreen::DownloadList,
            input_mode: InputPromptMode::None,
            active_path_input: None,
            view_manager: None,
            download_list: None,
            current_view_name: "main".to_string(),
            throttle_up: 0,
            throttle_down: 0,
            rate_up: 0.0,
            rate_down: 0.0,
            listen_port: 6881,
            shutdown_requested: false,
            log_messages: Vec::new(),
            color_map: Arc::new(RwLock::new(ColorMap::new())),
            color_definitions: HashMap::new(),
            input_history: InputHistory::new(),
            input_history_limit: crate::input::DEFAULT_INPUT_HISTORY_SIZE,
            input_history_search: None,
            keymap_style: KeymapStyle::Default.as_str().to_string(),
            torrent_list_layout: "full".to_string(),
            focus_page_size: 0,
            status_throttle_up: String::new(),
            status_throttle_down: String::new(),
            throttle_step_small: 5,
            throttle_step_medium: 50,
            throttle_step_large: 500,
        };

        ui.update_statusbar();
        ui
    }

    pub fn input_history_size(&self) -> usize {
        self.input_history_limit.min(MAX_INPUT_HISTORY_SIZE)
    }

    /// Keeps the public `input_history_limit` field and the bounded history in
    /// sync, so direct field writes (the pre-existing API) still take effect.
    fn ensure_history_size(&mut self) {
        self.input_history_limit = self.input_history_limit.min(MAX_INPUT_HISTORY_SIZE);
        if self.input_history.size() != self.input_history_limit {
            self.input_history.set_size(self.input_history_limit);
        }
    }

    pub fn set_input_history_size(&mut self, size: usize) {
        self.input_history_limit = size.min(MAX_INPUT_HISTORY_SIZE);
        self.input_history.set_size(self.input_history_limit);
        self.window_input.lock().unwrap().mark_dirty();
    }

    pub fn push_input_history(&mut self, line: impl Into<String>) {
        self.ensure_history_size();
        self.input_history.push(line);
    }

    /// Adds a line to the history and reports whether it was stored.
    pub fn add_input_history(&mut self, line: impl Into<String>) -> bool {
        self.ensure_history_size();
        self.input_history.push(line)
    }

    pub fn clear_input_history(&mut self) {
        self.input_history.clear();
        self.input_history_search = None;
        self.window_input.lock().unwrap().mark_dirty();
    }

    /// Keymap style name as configured through `ui.keymap.style`.
    pub fn keymap_style(&self) -> &str {
        &self.keymap_style
    }

    /// Parsed keymap style.  Unknown values (which can only appear if the
    /// public field was assigned directly) fall back to the default style.
    pub fn keymap_style_enum(&self) -> KeymapStyle {
        KeymapStyle::parse(&self.keymap_style).unwrap_or_default()
    }

    /// Validates and switches the keymap style.
    ///
    /// Supported values are `default`, `emacs` and `vi` (`vim` is accepted as
    /// an alias for `vi`).  The stored name is normalised to the canonical
    /// spelling.
    pub fn set_keymap_style(&mut self, style: &str) -> Result<(), String> {
        let parsed = KeymapStyle::parse(style)?;
        self.keymap_style = parsed.as_str().to_string();
        self.input_manager.lock().unwrap().set_keymap_style(parsed);
        self.window_input.lock().unwrap().mark_dirty();
        Ok(())
    }

    /// Recalls the previous (older) input-history entry into the active
    /// prompt.  Returns `false` when no prompt is active or history is empty.
    pub fn input_history_prev(&mut self) -> bool {
        self.ensure_history_size();
        let Some(input) = self.active_path_input.clone() else {
            return false;
        };
        let current = input.lock().unwrap().text().to_string();
        let Some(text) = self.input_history.prev(&current) else {
            return false;
        };
        input.lock().unwrap().set_text(text);
        true
    }

    /// Recalls the next (newer) input-history entry, eventually restoring the
    /// line that was being edited.
    pub fn input_history_next(&mut self) -> bool {
        self.ensure_history_size();
        let Some(input) = self.active_path_input.clone() else {
            return false;
        };
        let current = input.lock().unwrap().text().to_string();
        let Some(text) = self.input_history.next(&current) else {
            return false;
        };
        input.lock().unwrap().set_text(text);
        true
    }

    /// Incremental history search towards older entries.
    pub fn input_history_search_prev(&mut self, query: &str) -> bool {
        self.ensure_history_size();
        let Some(input) = self.active_path_input.clone() else {
            return false;
        };
        let current = input.lock().unwrap().text().to_string();
        let Some(text) = self.input_history.search_prev(&current, query) else {
            return false;
        };
        input.lock().unwrap().set_text(text);
        true
    }

    /// Incremental history search towards newer entries.
    pub fn input_history_search_next(&mut self, query: &str) -> bool {
        self.ensure_history_size();
        let Some(input) = self.active_path_input.clone() else {
            return false;
        };
        let current = input.lock().unwrap().text().to_string();
        let Some(text) = self.input_history.search_next(&current, query) else {
            return false;
        };
        input.lock().unwrap().set_text(text);
        true
    }

    fn poll_active_completion(&self) {
        if let Some(input) = self.active_path_input.clone() {
            input.lock().unwrap().poll_completion();
        }
    }

    fn active_input_text(&self) -> String {
        self.active_path_input
            .as_ref()
            .map(|input| input.lock().unwrap().text().to_string())
            .unwrap_or_default()
    }

    pub fn set_ui_color(&mut self, kind: ColorKind, definition: &str) -> Result<(), String> {
        // Validate first: an invalid definition must not modify either the
        // stored variable or the active color map.
        let style = crate::display::color_map::parse_color_definition(definition)?;

        {
            let mut color_map = self.color_map.write().unwrap();
            color_map.set_style(kind, style);
        }

        if let Some(name) = kind.rpc_variable_name() {
            self.color_definitions
                .insert(name.to_string(), definition.to_string());
        }

        // Colors affect every visible window and the parity styles.
        self.mark_ui_dirty();
        Ok(())
    }

    pub fn ui_color(&self, kind: ColorKind) -> String {
        kind.rpc_variable_name()
            .and_then(|name| self.color_definitions.get(name))
            .cloned()
            .unwrap_or_default()
    }

    /// Rebuilds the active [`ColorMap`] from every stored `ui.color.*`
    /// definition.  Definitions are validated when they are stored, so a
    /// failure here can only come from a definition inserted directly into
    /// the public `color_definitions` map.
    pub fn reload_color_map(&mut self) {
        let definitions: Vec<(String, String)> = self
            .color_definitions
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        self.color_map.write().unwrap().build_from_definitions(
            definitions
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        );
        self.mark_ui_dirty();
    }

    /// Marks every window dirty, forcing the next render pass to repaint the
    /// complete interface.  Used by UI RPC commands that can be invoked from
    /// SCGI/XML-RPC threads while the render loop is running.
    pub fn mark_ui_dirty(&self) {
        self.window_title.lock().unwrap().mark_dirty();
        self.window_download_list.lock().unwrap().mark_dirty();
        self.window_details.lock().unwrap().mark_dirty();
        self.window_file_list.lock().unwrap().mark_dirty();
        self.window_peer_list.lock().unwrap().mark_dirty();
        self.window_tracker_list.lock().unwrap().mark_dirty();
        self.window_chunks_seen.lock().unwrap().mark_dirty();
        self.window_transfer_list.lock().unwrap().mark_dirty();
        self.window_log.lock().unwrap().mark_dirty();
        self.window_input.lock().unwrap().mark_dirty();
        self.window_statusbar.lock().unwrap().mark_dirty();
        if let Some(menu) = &self.window_menu {
            menu.lock().unwrap().mark_dirty();
        }
    }

    /// Marks the windows that display the download list and its view.
    pub fn mark_view_dirty(&self) {
        self.window_title.lock().unwrap().mark_dirty();
        self.window_download_list.lock().unwrap().mark_dirty();
        self.window_details.lock().unwrap().mark_dirty();
        self.window_file_list.lock().unwrap().mark_dirty();
        self.window_peer_list.lock().unwrap().mark_dirty();
        self.window_tracker_list.lock().unwrap().mark_dirty();
        self.window_chunks_seen.lock().unwrap().mark_dirty();
        self.window_transfer_list.lock().unwrap().mark_dirty();
        self.window_log.lock().unwrap().mark_dirty();
    }

    pub fn set_focus_page_size(&mut self, size: u32) {
        self.focus_page_size = size;
        self.mark_view_dirty();
    }

    pub fn set_status_throttle_up(&mut self, names: impl Into<String>) {
        self.status_throttle_up = names.into();
        self.window_statusbar.lock().unwrap().mark_dirty();
    }

    pub fn set_status_throttle_down(&mut self, names: impl Into<String>) {
        self.status_throttle_down = names.into();
        self.window_statusbar.lock().unwrap().mark_dirty();
    }

    /// Updates one of `ui.throttle.global.step.*` (0 = small, 1 = medium,
    /// 2 = large).  The values are consumed by the throttle-adjustment keys.
    pub fn set_throttle_step(&mut self, step: usize, value: u32) {
        match step {
            0 => self.throttle_step_small = value,
            1 => self.throttle_step_medium = value,
            _ => self.throttle_step_large = value,
        }
        self.window_statusbar.lock().unwrap().mark_dirty();
    }

    pub fn set_torrent_list_layout(&mut self, layout: impl Into<String>) {
        let layout = layout.into();
        self.torrent_list_layout = layout.clone();
        self.window_download_list.lock().unwrap().set_layout(layout);
    }

    /// Removes focus from a download as `ui.unfocus_download` does.
    ///
    /// With a download target the focus only changes when that download is
    /// currently focused (matching the legacy UI, where the command is driven
    /// by `event.download.erased`).  Without a target the current selection is
    /// simply cleared.
    pub fn unfocus_download(&mut self, hash: Option<&HashString>) -> bool {
        let mut window = self.window_download_list.lock().unwrap();
        let view = window.view.clone();
        let view_manager = window.view_manager.clone();
        let view_name = window.view_name.clone();

        let changed = if let Some(view) = view {
            let mut view = view.write().unwrap();
            Self::unfocus_in_view(&mut view, hash)
        } else if let Some(view_manager) = view_manager {
            let mut vm = view_manager.write().unwrap();
            vm.find_mut(&view_name)
                .map(|view| Self::unfocus_in_view(view, hash))
                .unwrap_or(false)
        } else {
            false
        };

        // Even a no-op target change should trigger a repaint so RPC clients
        // observe a consistent interface state.
        window.mark_dirty();
        changed
    }

    fn unfocus_in_view(view: &mut View, hash: Option<&HashString>) -> bool {
        let is_focused = match hash {
            Some(hash) => view
                .focus()
                .map(|download| *download.read().unwrap().info_hash() == *hash)
                .unwrap_or(false),
            None => true,
        };
        if !is_focused {
            return false;
        }

        if hash.is_none() {
            let end = view.size_visible();
            if view.focus_index() == end {
                return false;
            }
            view.set_focus_index(end);
        } else {
            view.next_focus(1);
        }
        true
    }

    pub fn set_view_manager(&mut self, vm: Arc<RwLock<ViewManager>>) {
        self.view_manager = Some(vm.clone());
        self.window_download_list
            .lock()
            .unwrap()
            .set_view_manager(Some(vm), &self.current_view_name);
        self.switch_view(&self.current_view_name.clone());
    }

    pub fn set_download_list(&mut self, dl: Arc<RwLock<DownloadList>>) {
        self.download_list = Some(dl);
        if self.active_screen == ActiveScreen::TrackerList {
            self.refresh_tracker_list();
        }
    }

    pub fn switch_view(&mut self, view_name: &str) {
        self.current_view_name = view_name.to_string();
        self.window_download_list
            .lock()
            .unwrap()
            .set_view_name(view_name);
        self.window_title
            .lock()
            .unwrap()
            .set_title(format!("rTorrent - View: {}", view_name));

        if self.active_screen == ActiveScreen::FileList {
            self.refresh_file_list();
        } else if self.active_screen == ActiveScreen::PeerList {
            self.refresh_peer_list();
        } else if self.active_screen == ActiveScreen::TrackerList {
            self.refresh_tracker_list();
        } else if self.active_screen == ActiveScreen::ChunksSeen {
            self.refresh_chunks_seen();
        } else if self.active_screen == ActiveScreen::TransferList {
            self.refresh_transfer_list();
        }

        // A view switch changes the title, list contents and every secondary
        // window that follows the focused download.
        self.mark_ui_dirty();
    }

    /// Returns the download currently focused in the active view.
    pub fn selected_download(&self) -> Option<Arc<RwLock<Download>>> {
        let win = self.window_download_list.lock().unwrap();

        if let Some(view_arc) = &win.view {
            return view_arc.read().unwrap().focus();
        }

        if let Some(vm_arc) = &win.view_manager {
            let vm = vm_arc.read().unwrap();
            return vm.find(&win.view_name).and_then(|view| view.focus());
        }

        // No view is configured: fall back to the first registered download.
        self.download_list
            .as_ref()
            .and_then(|list| list.read().unwrap().all().into_iter().next())
    }

    /// Rebinds the file-list window to the currently focused download.
    pub fn refresh_file_list(&mut self) {
        let download = self.selected_download();
        self.window_file_list.lock().unwrap().set_download(download);
    }

    /// Rebinds the peer-list window to the currently focused download.
    pub fn refresh_peer_list(&mut self) {
        let download = self.selected_download();
        self.window_peer_list.lock().unwrap().set_download(download);
    }

    /// Rebinds the tracker-list window to the currently focused download and
    /// takes a fresh snapshot of its tracker state.
    pub fn refresh_tracker_list(&mut self) {
        let download = self.selected_download();
        self.window_tracker_list
            .lock()
            .unwrap()
            .set_download(download);
    }

    /// Rebinds the chunks-seen window to the focused download.
    pub fn refresh_chunks_seen(&mut self) {
        let download = self.selected_download();
        self.window_chunks_seen
            .lock()
            .unwrap()
            .set_download(download);
    }

    /// Rebinds the transfer-list window to the focused download.
    pub fn refresh_transfer_list(&mut self) {
        let download = self.selected_download();
        self.window_transfer_list
            .lock()
            .unwrap()
            .set_download(download);
    }

    pub fn set_command_map(&mut self, map: Arc<RwLock<CommandMap>>) {
        self.command_map = Some(map);
    }

    pub fn open_menu(&mut self, menu: Menu) {
        self.window_menu = Some(Arc::new(Mutex::new(MenuElement::new(menu))));
    }

    pub fn close_menu(&mut self) {
        self.window_menu = None;
    }

    pub fn menu_is_open(&self) -> bool {
        self.window_menu.is_some()
    }

    /// Applies an action to the current tracker-list selection.
    pub fn tracker_list_action(&mut self, action: TrackerAction) -> TrackerActionOutcome {
        let outcome = self
            .window_tracker_list
            .lock()
            .unwrap()
            .apply_selected_action(action);
        if outcome.is_applied() {
            self.window_tracker_list.lock().unwrap().mark_dirty();
        }
        outcome
    }

    /// Applies an action to the current peer-list selection and refreshes it.
    pub fn peer_list_action(
        &mut self,
        action: crate::windows::peer_list::PeerAction,
    ) -> PeerActionOutcome {
        let outcome = self
            .window_peer_list
            .lock()
            .unwrap()
            .apply_selected_action(action);
        if let PeerActionOutcome::Applied = outcome {
            self.window_peer_list.lock().unwrap().mark_dirty();
        }
        outcome
    }

    /// Applies a priority to the current file-list selection.
    pub fn file_list_set_priority(&mut self, priority: FilePriority) -> bool {
        self.window_file_list
            .lock()
            .unwrap()
            .set_selected_priority(priority)
    }

    pub fn set_active_screen(&mut self, screen: ActiveScreen) {
        if screen == ActiveScreen::FileList {
            // Always follow the download that is focused in the current view.
            self.refresh_file_list();
        } else if screen == ActiveScreen::PeerList {
            self.refresh_peer_list();
        } else if screen == ActiveScreen::TrackerList {
            self.refresh_tracker_list();
        } else if screen == ActiveScreen::ChunksSeen {
            self.refresh_chunks_seen();
        } else if screen == ActiveScreen::TransferList {
            self.refresh_transfer_list();
        }

        self.active_screen = screen;

        self.window_download_list.lock().unwrap().set_active(false);
        self.window_details.lock().unwrap().set_active(false);
        self.window_file_list.lock().unwrap().set_active(false);
        self.window_peer_list.lock().unwrap().set_active(false);
        self.window_tracker_list.lock().unwrap().set_active(false);
        self.window_chunks_seen.lock().unwrap().set_active(false);
        self.window_transfer_list.lock().unwrap().set_active(false);
        self.window_log.lock().unwrap().set_active(false);

        let main_window: Arc<Mutex<dyn Window>> = match screen {
            ActiveScreen::DownloadList => {
                self.window_download_list.lock().unwrap().set_active(true);
                self.window_download_list.clone()
            }
            ActiveScreen::DownloadDetails => {
                self.window_details.lock().unwrap().set_active(true);
                self.window_details.clone()
            }
            ActiveScreen::FileList => {
                self.window_file_list.lock().unwrap().set_active(true);
                self.window_file_list.clone()
            }
            ActiveScreen::PeerList => {
                self.window_peer_list.lock().unwrap().set_active(true);
                self.window_peer_list.clone()
            }
            ActiveScreen::TrackerList => {
                self.window_tracker_list.lock().unwrap().set_active(true);
                self.window_tracker_list.clone()
            }
            ActiveScreen::ChunksSeen => {
                self.window_chunks_seen.lock().unwrap().set_active(true);
                self.window_chunks_seen.clone()
            }
            ActiveScreen::TransferList => {
                self.window_transfer_list.lock().unwrap().set_active(true);
                self.window_transfer_list.clone()
            }
            ActiveScreen::Log => {
                self.window_log.lock().unwrap().set_active(true);
                self.window_log.clone()
            }
        };

        // Update Row 1 in root frame
        if let FrameNode::Row(children) = &mut self.root_frame.node {
            if children.len() > 1 {
                children[1] = Frame::with_window(main_window);
            }
        }
    }

    pub fn open_selected_download_details(&mut self) {
        let focused_dl = {
            let win = self.window_download_list.lock().unwrap();
            if let Some(view_arc) = &win.view {
                let view = view_arc.read().unwrap();
                view.visible_downloads().get(view.focus_index()).cloned()
            } else if let Some(vm_arc) = &win.view_manager {
                let vm = vm_arc.read().unwrap();
                if let Some(view) = vm.find(&win.view_name) {
                    view.visible_downloads().get(view.focus_index()).cloned()
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(dl) = focused_dl {
            self.window_details.lock().unwrap().set_download(Some(dl));
            self.set_active_screen(ActiveScreen::DownloadDetails);
        }
    }

    pub fn next_download(&mut self) {
        let mut win = self.window_download_list.lock().unwrap();
        let view_arc = win.view.clone();
        let vm_arc = win.view_manager.clone();
        let view_name = win.view_name.clone();

        if let Some(v) = &view_arc {
            v.write().unwrap().next_focus(1);
            win.mark_dirty();
        } else if let Some(vm) = &vm_arc {
            let mut guard = vm.write().unwrap();
            if let Some(v) = guard.find_mut(&view_name) {
                v.next_focus(1);
                win.mark_dirty();
            }
        }
    }

    pub fn prev_download(&mut self) {
        let mut win = self.window_download_list.lock().unwrap();
        let view_arc = win.view.clone();
        let vm_arc = win.view_manager.clone();
        let view_name = win.view_name.clone();

        if let Some(v) = &view_arc {
            v.write().unwrap().prev_focus(1);
            win.mark_dirty();
        } else if let Some(vm) = &vm_arc {
            let mut guard = vm.write().unwrap();
            if let Some(v) = guard.find_mut(&view_name) {
                v.prev_focus(1);
                win.mark_dirty();
            }
        }
    }

    pub fn toggle_layout(&mut self) {
        let layout = {
            let win = self.window_download_list.lock().unwrap();
            if win.layout == "compact" {
                "full"
            } else {
                "compact"
            }
        };
        self.set_torrent_list_layout(layout);
    }

    pub fn push_log(&mut self, msg: impl Into<String>) {
        let s = msg.into();
        self.log_messages.push(s.clone());
        self.window_log.lock().unwrap().push_message(s.clone());
        self.window_statusbar
            .lock()
            .unwrap()
            .set_status(format!("[Log] {}", s));
    }

    pub fn update_statusbar(&mut self) {
        let throttle_str = format!(
            "[Throttle {} / {} KB] [Rate {:5.1} / {:5.1} KB] [Port: {}]",
            if self.throttle_up == 0 {
                "off".to_string()
            } else {
                self.throttle_up.to_string()
            },
            if self.throttle_down == 0 {
                "off".to_string()
            } else {
                self.throttle_down.to_string()
            },
            self.rate_up,
            self.rate_down,
            self.listen_port
        );
        self.window_statusbar
            .lock()
            .unwrap()
            .set_extra(throttle_str);
    }

    pub fn enable_input(&mut self, title: &str, mode: InputPromptMode) {
        self.input_mode = mode;
        self.ensure_history_size();
        self.input_history.reset_navigation();
        self.input_history_search = None;

        let path_input = Arc::new(Mutex::new(PathInput::new()));
        {
            let mut input = path_input.lock().unwrap();
            // Filesystem scans run off-thread so a slow mount cannot stall
            // the event loop.
            input.set_async_completion(true);
            if mode == InputPromptMode::Command {
                if let Some(map) = &self.command_map {
                    let names: Vec<String> = map
                        .read()
                        .unwrap()
                        .iter()
                        .map(|(name, _)| name.clone())
                        .collect();
                    input.set_command_completions(names);
                }
            }
        }
        self.active_path_input = Some(path_input.clone());

        self.input_manager
            .lock()
            .unwrap()
            .set_keymap_style(self.keymap_style_enum());

        {
            let mut win = self.window_input.lock().unwrap();
            win.set_title(title);
            win.set_input(Some(path_input.clone()));
            win.set_focus(true);
            win.set_active(true);
        }

        self.input_manager
            .lock()
            .unwrap()
            .set_text_input(Some(path_input));
    }

    pub fn disable_input(&mut self) -> (InputPromptMode, String) {
        let text = if let Some(pi) = &self.active_path_input {
            pi.lock().unwrap().text().to_string()
        } else {
            String::new()
        };

        let mode = self.input_mode;
        self.input_mode = InputPromptMode::None;
        self.active_path_input = None;
        self.input_history.reset_navigation();
        self.input_history_search = None;

        {
            let mut win = self.window_input.lock().unwrap();
            win.set_input(None);
            win.set_focus(false);
            win.set_active(false);
        }

        self.input_manager.lock().unwrap().set_text_input(None);
        (mode, text)
    }

    pub fn handle_key(&mut self, key: i32) -> Option<(InputPromptMode, String)> {
        // If text input is active
        if self.input_mode != InputPromptMode::None {
            // Apply a completion that finished on the worker thread.
            self.poll_active_completion();

            match key {
                10 | 13 => {
                    // Enter submitted input
                    let (mode, text) = self.disable_input();
                    self.push_input_history(text.as_str());
                    return Some((mode, text));
                }
                0x1B => {
                    // Esc cancelled input
                    self.disable_input();
                    return None;
                }
                KEY_UP | 0x10 => {
                    // Up / ^P: older history entry (the active keymap style
                    // only affects list navigation, matching legacy rTorrent
                    // which always binds these keys inside a prompt).
                    self.input_history_search = None;
                    self.input_history_prev();
                    return None;
                }
                KEY_DOWN | 0x0E => {
                    // Down / ^N: newer history entry.
                    self.input_history_search = None;
                    self.input_history_next();
                    return None;
                }
                0x12 => {
                    // ^R: incremental search towards older entries.  The
                    // current prompt text seeds the query; repeated ^R keeps
                    // using that query while the prompt shows a match.
                    let query = self
                        .input_history_search
                        .clone()
                        .unwrap_or_else(|| self.active_input_text());
                    self.input_history_search = Some(query.clone());
                    self.input_history_search_prev(&query);
                    return None;
                }
                0x13 => {
                    // ^S: incremental search towards newer entries.
                    let query = self
                        .input_history_search
                        .clone()
                        .unwrap_or_else(|| self.active_input_text());
                    self.input_history_search = Some(query.clone());
                    self.input_history_search_next(&query);
                    return None;
                }
                _ => {
                    // Any edit abandons the current search/navigation cursor.
                    self.input_history_search = None;
                    self.input_history.reset_navigation();
                    self.input_manager.lock().unwrap().pressed(key);
                    self.poll_active_completion();
                    return None;
                }
            }
        }

        // A popup menu owns keyboard input while it is open.
        if self.window_menu.is_some() {
            let Some(command_map) = self.command_map.clone() else {
                self.push_log("Menu unavailable: command map is not configured");
                self.close_menu();
                return None;
            };
            let outcome = {
                let mut element = self.window_menu.as_ref().unwrap().lock().unwrap();
                let map = command_map.read().unwrap();
                element.handle_key(key, &map)
            };
            if let Some(outcome) = outcome {
                if let Some(message) = outcome.message() {
                    self.push_log(message);
                }
                match outcome {
                    MenuOutcome::CloseRequested | MenuOutcome::Executed(_) => self.close_menu(),
                    _ => {}
                }
            }
            return None;
        }

        // The keymap style selects the character-based navigation keys.
        let keymap = self.keymap_style_enum();

        // Peer-list actions take precedence over the global throttle keys.
        if self.active_screen == ActiveScreen::PeerList {
            let has_selection = !self.window_peer_list.lock().unwrap().is_empty();
            if has_selection {
                if let Some(outcome) = self.window_peer_list.lock().unwrap().handle_action_key(key)
                {
                    if outcome.is_applied() {
                        self.window_peer_list.lock().unwrap().mark_dirty();
                    }
                    return None;
                }
            }
        }

        // Tracker-list actions take precedence over the global throttle keys.
        if self.active_screen == ActiveScreen::TrackerList {
            let has_selection = !self.window_tracker_list.lock().unwrap().is_empty();
            if has_selection {
                if let Some(outcome) = self
                    .window_tracker_list
                    .lock()
                    .unwrap()
                    .handle_action_key(key)
                {
                    if outcome.is_applied() {
                        self.window_tracker_list.lock().unwrap().mark_dirty();
                    }
                    return None;
                }
            }
        }

        // Global shortcuts
        let k = key;
        if k == 0x11 {
            // ^Q: quit
            self.shutdown_requested = true;
        } else if k == 0x0C {
            // ^L: redraw
            self.window_title.lock().unwrap().mark_dirty();
            self.window_download_list.lock().unwrap().mark_dirty();
            self.window_details.lock().unwrap().mark_dirty();
            self.window_file_list.lock().unwrap().mark_dirty();
            self.window_peer_list.lock().unwrap().mark_dirty();
            self.window_tracker_list.lock().unwrap().mark_dirty();
            self.window_chunks_seen.lock().unwrap().mark_dirty();
            self.window_transfer_list.lock().unwrap().mark_dirty();
            self.window_statusbar.lock().unwrap().mark_dirty();
        } else if k == 0x18 {
            // ^X: execute command
            self.enable_input("execute", InputPromptMode::Command);
        } else if k == 0x107 || k == 0x7F || k == 8 {
            // Backspace: collapse a directory or leave the current sub-view.
            if self.active_screen == ActiveScreen::FileList {
                let collapsed = self.window_file_list.lock().unwrap().collapse_selected();
                if !collapsed {
                    self.set_active_screen(ActiveScreen::DownloadList);
                }
            } else if self.active_screen != ActiveScreen::DownloadList {
                self.set_active_screen(ActiveScreen::DownloadList);
            } else {
                self.enable_input("load.start", InputPromptMode::LoadStart);
            }
        } else if k == 10 || k == 13 {
            // Enter: open details from the list, toggle directories in files.
            if self.active_screen == ActiveScreen::FileList {
                self.window_file_list.lock().unwrap().toggle_selected();
            } else if self.active_screen == ActiveScreen::DownloadList {
                self.open_selected_download_details();
            } else {
                self.set_active_screen(ActiveScreen::DownloadList);
            }
        } else if k == 0x1B {
            // Esc: return to DownloadList
            if self.active_screen != ActiveScreen::DownloadList {
                self.set_active_screen(ActiveScreen::DownloadList);
            }
        } else if k == 0x104 {
            // Left Arrow: return to DownloadList.
            if self.active_screen != ActiveScreen::DownloadList {
                self.set_active_screen(ActiveScreen::DownloadList);
            }
        } else if k == 0x105 {
            // Right Arrow: expand a directory or open details.
            if self.active_screen == ActiveScreen::FileList {
                self.window_file_list.lock().unwrap().expand_selected();
            } else if self.active_screen == ActiveScreen::DownloadList {
                self.open_selected_download_details();
            }
        } else if keymap.matches(NavigationKey::Up, k) {
            // Up arrow / style-specific key
            if self.active_screen == ActiveScreen::DownloadDetails {
                self.window_details.lock().unwrap().scroll_up();
            } else if self.active_screen == ActiveScreen::FileList {
                self.window_file_list.lock().unwrap().move_up();
            } else if self.active_screen == ActiveScreen::PeerList {
                self.window_peer_list.lock().unwrap().select_prev();
            } else if self.active_screen == ActiveScreen::TrackerList {
                self.window_tracker_list.lock().unwrap().select_prev();
            } else if self.active_screen == ActiveScreen::ChunksSeen {
                self.window_chunks_seen.lock().unwrap().move_up();
            } else if self.active_screen == ActiveScreen::TransferList {
                self.window_transfer_list.lock().unwrap().select_prev();
            } else {
                self.prev_download();
            }
        } else if keymap.matches(NavigationKey::Down, k) {
            // Down arrow / style-specific key
            if self.active_screen == ActiveScreen::DownloadDetails {
                self.window_details.lock().unwrap().scroll_down();
            } else if self.active_screen == ActiveScreen::FileList {
                self.window_file_list.lock().unwrap().move_down();
            } else if self.active_screen == ActiveScreen::PeerList {
                self.window_peer_list.lock().unwrap().select_next();
            } else if self.active_screen == ActiveScreen::TrackerList {
                self.window_tracker_list.lock().unwrap().select_next();
            } else if self.active_screen == ActiveScreen::ChunksSeen {
                self.window_chunks_seen.lock().unwrap().move_down();
            } else if self.active_screen == ActiveScreen::TransferList {
                self.window_transfer_list.lock().unwrap().select_next();
            } else {
                self.next_download();
            }
        } else if self.active_screen == ActiveScreen::FileList
            && keymap.matches(NavigationKey::PageUp, k)
        {
            // Page Up: 0x153, ^U or 'K'
            self.window_file_list.lock().unwrap().move_page_up();
        } else if self.active_screen == ActiveScreen::PeerList
            && keymap.matches(NavigationKey::PageUp, k)
        {
            self.window_peer_list.lock().unwrap().page_up();
        } else if self.active_screen == ActiveScreen::TrackerList
            && keymap.matches(NavigationKey::PageUp, k)
        {
            self.window_tracker_list.lock().unwrap().page_up();
        } else if self.active_screen == ActiveScreen::ChunksSeen
            && keymap.matches(NavigationKey::PageUp, k)
        {
            self.window_chunks_seen.lock().unwrap().page_up();
        } else if self.active_screen == ActiveScreen::TransferList
            && keymap.matches(NavigationKey::PageUp, k)
        {
            self.window_transfer_list.lock().unwrap().page_up();
        } else if self.active_screen == ActiveScreen::FileList
            && keymap.matches(NavigationKey::PageDown, k)
        {
            // Page Down: 0x156, ^D or 'J'
            self.window_file_list.lock().unwrap().move_page_down();
        } else if self.active_screen == ActiveScreen::PeerList
            && keymap.matches(NavigationKey::PageDown, k)
        {
            self.window_peer_list.lock().unwrap().page_down();
        } else if self.active_screen == ActiveScreen::TrackerList
            && keymap.matches(NavigationKey::PageDown, k)
        {
            self.window_tracker_list.lock().unwrap().page_down();
        } else if self.active_screen == ActiveScreen::ChunksSeen
            && keymap.matches(NavigationKey::PageDown, k)
        {
            self.window_chunks_seen.lock().unwrap().page_down();
        } else if self.active_screen == ActiveScreen::TransferList
            && keymap.matches(NavigationKey::PageDown, k)
        {
            self.window_transfer_list.lock().unwrap().page_down();
        } else if self.active_screen == ActiveScreen::FileList && k == 32 {
            // Space expands directories and cycles the priority of files.
            let mut win = self.window_file_list.lock().unwrap();
            if win.selected_row().map(|row| row.is_dir).unwrap_or(false) {
                win.toggle_selected();
            } else {
                win.cycle_selected_priority();
            }
        } else if self.active_screen == ActiveScreen::FileList && k == (b'/' as i32) {
            // '/' collapses/expands the selected directory.
            self.window_file_list.lock().unwrap().toggle_selected();
        } else if self.active_screen == ActiveScreen::FileList && k == (b'*' as i32) {
            // '*' cycles the priority of every file in the torrent.
            self.window_file_list.lock().unwrap().cycle_all_priorities();
        } else if self.active_screen == ActiveScreen::FileList && k == (b'o' as i32) {
            self.file_list_set_priority(FilePriority::Off);
        } else if self.active_screen == ActiveScreen::FileList && k == (b'n' as i32) {
            self.file_list_set_priority(FilePriority::Normal);
        } else if self.active_screen == ActiveScreen::FileList && k == (b'h' as i32) {
            self.file_list_set_priority(FilePriority::High);
        } else if self.active_screen == ActiveScreen::FileList && k == (b'r' as i32) {
            self.window_file_list.lock().unwrap().refresh();
        } else if self.active_screen == ActiveScreen::PeerList && k == (b'r' as i32) {
            self.window_peer_list.lock().unwrap().refresh();
        } else if self.active_screen == ActiveScreen::TrackerList && k == (b'r' as i32) {
            self.window_tracker_list.lock().unwrap().refresh();
        } else if self.active_screen == ActiveScreen::ChunksSeen && k == (b'r' as i32) {
            self.window_chunks_seen.lock().unwrap().refresh();
        } else if self.active_screen == ActiveScreen::TransferList && k == (b'r' as i32) {
            self.window_transfer_list.lock().unwrap().refresh();
        } else if k == (b'm' as i32) {
            if let Some(download) = self.selected_download() {
                let hash = *download.read().unwrap().info_hash();
                self.open_menu(download_context_menu(&hash));
            }
        } else if k == (b'u' as i32) {
            if self.active_screen == ActiveScreen::ChunksSeen {
                self.set_active_screen(ActiveScreen::DownloadList);
            } else {
                self.set_active_screen(ActiveScreen::ChunksSeen);
            }
        } else if k == (b't' as i32) {
            if self.active_screen == ActiveScreen::TransferList {
                self.set_active_screen(ActiveScreen::DownloadList);
            } else {
                self.set_active_screen(ActiveScreen::TransferList);
            }
        } else if k == (b'o' as i32) && self.active_screen != ActiveScreen::FileList {
            // Open/close the tracker view. The file list keeps 'o' for the
            // legacy "priority off" action.
            if self.active_screen == ActiveScreen::TrackerList {
                self.set_active_screen(ActiveScreen::DownloadList);
            } else {
                self.set_active_screen(ActiveScreen::TrackerList);
            }
        } else if k == (b'T' as i32) {
            // Alternate tracker-view toggle that also works from the file list.
            if self.active_screen == ActiveScreen::TrackerList {
                self.set_active_screen(ActiveScreen::DownloadList);
            } else {
                self.set_active_screen(ActiveScreen::TrackerList);
            }
        } else if k == 0x10 || k == (b'p' as i32) {
            // Toggle the peer-list view.
            if self.active_screen == ActiveScreen::PeerList {
                self.set_active_screen(ActiveScreen::DownloadList);
            } else {
                self.set_active_screen(ActiveScreen::PeerList);
            }
        } else if self.active_screen == ActiveScreen::FileList && k == (b'e' as i32) {
            self.window_file_list.lock().unwrap().expand_all();
        } else if self.active_screen == ActiveScreen::FileList && k == (b'E' as i32) {
            self.window_file_list.lock().unwrap().collapse_all();
        } else if k == (b'v' as i32) {
            // Toggle the file-list view.
            if self.active_screen == ActiveScreen::FileList {
                self.set_active_screen(ActiveScreen::DownloadList);
            } else {
                self.set_active_screen(ActiveScreen::FileList);
            }
        } else if k == (b'1' as i32) {
            if self.active_screen == ActiveScreen::DownloadDetails {
                self.window_details.lock().unwrap().set_tab(DetailTab::Info);
            } else {
                self.switch_view("main");
            }
        } else if k == (b'2' as i32) {
            if self.active_screen == ActiveScreen::DownloadDetails {
                self.window_details
                    .lock()
                    .unwrap()
                    .set_tab(DetailTab::Peers);
            } else {
                self.switch_view("name");
            }
        } else if k == (b'3' as i32) {
            if self.active_screen == ActiveScreen::DownloadDetails {
                self.window_details
                    .lock()
                    .unwrap()
                    .set_tab(DetailTab::Files);
            } else {
                self.switch_view("started");
            }
        } else if k == (b'4' as i32) {
            if self.active_screen == ActiveScreen::DownloadDetails {
                self.window_details
                    .lock()
                    .unwrap()
                    .set_tab(DetailTab::Trackers);
            } else {
                self.switch_view("stopped");
            }
        } else if k == (b'5' as i32) {
            self.switch_view("complete");
        } else if k == (b'6' as i32) {
            self.switch_view("incomplete");
        } else if k == (b'7' as i32) {
            self.switch_view("hashing");
        } else if k == (b'8' as i32) {
            self.switch_view("seeding");
        } else if k == (b'9' as i32) {
            self.switch_view("leeching");
        } else if k == (b'0' as i32) {
            self.switch_view("active");
        } else if k == (b'l' as i32) {
            if self.active_screen == ActiveScreen::Log {
                self.set_active_screen(ActiveScreen::DownloadList);
            } else {
                self.set_active_screen(ActiveScreen::Log);
            }
        } else if k == (b'L' as i32) {
            self.toggle_layout();
        } else if k == (b'F' as i32) || k == (b'f' as i32) {
            self.enable_input("filter", InputPromptMode::Filter);
        } else if k == (b'a' as i32) {
            self.adjust_up_throttle(1);
        } else if k == (b'z' as i32) {
            self.adjust_up_throttle(-1);
        } else if k == (b's' as i32) {
            self.adjust_up_throttle(5);
        } else if k == (b'x' as i32) {
            self.adjust_up_throttle(-5);
        } else if k == (b'd' as i32) {
            self.adjust_up_throttle(50);
        } else if k == (b'c' as i32) {
            self.adjust_up_throttle(-50);
        } else if k == (b'A' as i32) {
            self.adjust_down_throttle(1);
        } else if k == (b'Z' as i32) {
            self.adjust_down_throttle(-1);
        } else if k == (b'S' as i32) {
            self.adjust_down_throttle(5);
        } else if k == (b'X' as i32) {
            self.adjust_down_throttle(-5);
        } else if k == (b'D' as i32) {
            self.adjust_down_throttle(50);
        } else if k == (b'C' as i32) {
            self.adjust_down_throttle(-50);
        } else {
            self.input_manager.lock().unwrap().pressed(key);
        }
        None
    }

    pub fn balance_and_redraw(&mut self, canvas: &mut Canvas) {
        // Pick up asynchronous path completions even when no key arrives.
        self.poll_active_completion();

        let w = canvas.full_width();
        let h = canvas.full_height();
        self.root_frame.balance(0, 0, w, h);
        self.root_frame.redraw(canvas);

        if let Some(menu) = &self.window_menu {
            if w > 2 && h > 2 {
                let mut element = menu.lock().unwrap();
                let menu_width = (w - 2).min(48).max(12);
                let menu_height = (h - 2).min(element.menu().len() as u32 + 1).max(2);
                let x = (w - menu_width) / 2;
                let y = (h - menu_height) / 2;
                canvas.set_viewport(x, y, menu_width, menu_height);
                element.redraw(canvas);
                canvas.reset_viewport();
            }
        }

        // Apply the configured `ui.color.*` definitions on top of the
        // windows' built-in styles.  Keeping this in the root renderer means
        // RPC colour changes take effect on the very next frame without each
        // window needing its own copy of the colour map.
        self.apply_color_map(canvas);
    }

    fn apply_color_map(&self, canvas: &mut Canvas) {
        let width = canvas.full_width();
        let height = canvas.full_height();
        if width == 0 || height == 0 {
            return;
        }

        let color_map = self.color_map.read().unwrap();
        let is_active = |kind: ColorKind| {
            kind.rpc_variable_name()
                .map(|name| self.color_definitions.contains_key(name))
                .unwrap_or(false)
                || !style_is_default(&color_map.get_style(kind, None))
        };

        // Nothing has been configured: leave the renderer's defaults intact.
        if !ColorKind::ALL.iter().any(|kind| is_active(*kind)) {
            return;
        }

        for y in 0..height {
            let line = canvas.get_line(y);
            let (kind, parity) = if y == 0 {
                (ColorKind::Title, None)
            } else if y + 1 == height {
                (ColorKind::Footer, None)
            } else {
                (row_color_kind(&line), Some(y % 2 != 0))
            };

            let parity_active = parity
                .map(|odd| {
                    if odd {
                        is_active(ColorKind::Odd)
                    } else {
                        is_active(ColorKind::Even)
                    }
                })
                .unwrap_or(false);

            // Fall back to the plain foreground/background style when the
            // semantic kind for this row has not been configured.
            let row_kind = if is_active(kind) {
                kind
            } else {
                ColorKind::Default
            };

            for x in 0..width {
                let Some(cell_style) = canvas.get_raw_cell(x, y).map(|cell| cell.style) else {
                    continue;
                };
                let mut kind = cell_color_kind(cell_style, row_kind);
                let explicit = is_active(kind);
                if !explicit {
                    if !parity_active {
                        continue;
                    }
                    kind = ColorKind::Default;
                }
                let style = color_map.get_style(kind, parity);
                let merged = if explicit {
                    // A configured kind fully describes the colour (so
                    // assigning an empty definition resets the window's
                    // hard-coded colour instead of leaving it in place).
                    apply_style(cell_style, style)
                } else {
                    // Parity backgrounds only fill in the background.
                    merge_style(cell_style, style)
                };
                canvas.set_attr(x, y, 1, merged);
            }
        }
    }

    pub fn adjust_up_throttle(&mut self, delta: i32) {
        if delta < 0 {
            self.throttle_up = self.throttle_up.saturating_sub((-delta) as u32);
        } else {
            self.throttle_up = self.throttle_up.saturating_add(delta as u32);
        }
        self.update_statusbar();
    }

    pub fn adjust_down_throttle(&mut self, delta: i32) {
        if delta < 0 {
            self.throttle_down = self.throttle_down.saturating_sub((-delta) as u32);
        } else {
            self.throttle_down = self.throttle_down.saturating_add(delta as u32);
        }
        self.update_statusbar();
    }
}

/// Best-effort mapping from a rendered list row to the legacy `ui.color.*`
/// semantic kind.  Rows produced by the download list carry their state in
/// the text, so this keeps the renderer decoupled from the individual window
/// implementations.
fn row_color_kind(line: &str) -> ColorKind {
    if line.starts_with('*') {
        return ColorKind::Focus;
    }

    let upper = line.to_ascii_uppercase();
    if upper.contains("SEEDING") {
        ColorKind::Seeding
    } else if upper.contains("COMPLETED") {
        ColorKind::Complete
    } else if upper.contains("DOWNLOADING") {
        ColorKind::Leeching
    } else if upper.contains("STOPPED") || upper.contains("CLOSED") {
        ColorKind::Stopped
    } else if upper.contains("QUEUED") {
        ColorKind::Queued
    } else if upper.contains("CONNECTING")
        || upper.contains("INCOMPLETE")
        || upper.contains("HASHING")
    {
        ColorKind::Incomplete
    } else if upper.contains("ERROR") || upper.contains("FAILED") {
        ColorKind::Alarm
    } else {
        ColorKind::Default
    }
}

/// Maps a window's built-in style to a semantic colour kind.  This lets
/// `ui.color.*` replace the hard-coded ratatui colours used by individual
/// windows without coupling those windows to the colour map.
fn cell_color_kind(style: Style, fallback: ColorKind) -> ColorKind {
    if fallback != ColorKind::Default {
        return fallback;
    }
    if style.add_modifier.contains(Modifier::REVERSED)
        || (style.fg == Some(Color::Black) && style.bg == Some(Color::White))
    {
        return ColorKind::Focus;
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        return ColorKind::Label;
    }

    match style.fg {
        Some(Color::Red) | Some(Color::LightRed) => ColorKind::Alarm,
        Some(Color::Green) => ColorKind::Complete,
        Some(Color::LightGreen) => ColorKind::Seeding,
        Some(Color::Yellow) => ColorKind::Incomplete,
        Some(Color::LightYellow) => ColorKind::Queued,
        Some(Color::Cyan) | Some(Color::LightCyan) => ColorKind::Info,
        Some(Color::DarkGray) => ColorKind::Stopped,
        _ => ColorKind::Default,
    }
}

fn style_is_default(style: &Style) -> bool {
    style.fg.is_none()
        && style.bg.is_none()
        && style.underline_color.is_none()
        && style.add_modifier.is_empty()
        && style.sub_modifier.is_empty()
}

fn apply_style(base: Style, overlay: Style) -> Style {
    let mut style = base;
    style.fg = overlay.fg;
    style.bg = overlay.bg;
    style.underline_color = overlay.underline_color;
    style.add_modifier |= overlay.add_modifier;
    style.sub_modifier |= overlay.sub_modifier;
    style
}

fn merge_style(base: Style, overlay: Style) -> Style {
    let mut style = base;
    if overlay.fg.is_some() {
        style.fg = overlay.fg;
    }
    if overlay.bg.is_some() {
        style.bg = overlay.bg;
    }
    if overlay.underline_color.is_some() {
        style.underline_color = overlay.underline_color;
    }
    style.add_modifier |= overlay.add_modifier;
    style.sub_modifier |= overlay.sub_modifier;
    style
}
