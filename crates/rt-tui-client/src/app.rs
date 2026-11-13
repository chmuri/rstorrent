//! Application shell: key handling, refresh scheduling, rendering and the loop.

use std::collections::HashSet;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::Frame;
use rt_grpc::{object_value, CommandResponse, ObjectValue};

use crate::actions::{PromptKind, TorrentAction, TuiAction};
use crate::bridge::{BridgeError, GrpcBridge, DEFAULT_RPC_TIMEOUT};
use crate::config::Theme;
use crate::connection::ConnectionManager;
use crate::events::{AppEvent, EventQueue, EventQueueConfig};
use crate::key_handler::{self, global_action, torrent_action};
use crate::palette::PaletteState;
use crate::state::{
    ActiveView, AppState, ConnectionStatus, DetailTab, DetailsState, FileListState, MenuEntry,
    MenuResult, MenuState,
};
use crate::stream::{StreamUpdate, Streams};
use crate::terminal::ClientTerminal;
use crate::throttle::{ThrottleAction, ThrottleKeyMap};
use crate::widgets::download_list::{viewport, visible_rows, HEADER_ROWS};
use crate::widgets::input::{InputResult, InputState, PromptMode};
use crate::widgets::InputWidget;
use crate::widgets::{
    DetailsWidget, DownloadListWidget, LogWidget, MenuWidget, PaletteWidget, StatusBarWidget,
    TitleBarWidget,
};
use rt_tui::theme::ThemeId;
use rt_tui::widgets::ThemePickerModal;

/// Tick used when the caller does not configure one.
pub const DEFAULT_TICK: Duration = Duration::from_millis(500);

/// Longest the loop blocks waiting for input, so refreshes stay on schedule.
const MAX_INPUT_WAIT: Duration = Duration::from_millis(50);

/// Number of prompt lines remembered across prompt openings.
const PROMPT_HISTORY_LIMIT: usize = 100;

/// rTorrent command used to change a file's priority.
pub const PRIORITY_COMMAND: &str = "f.priority.set";

/// Priority change waiting to be sent to the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorityCommand {
    /// Index of the file inside the torrent.
    pub file_index: u32,
    /// New priority: `0` off, `1` normal, `2` high.
    pub priority: u32,
    /// Torrent the file belongs to.
    pub hash: String,
}

/// Owns the state, the daemon bridge, the refresh interval and the terminal loop.
pub struct App {
    /// Current view model.
    pub state: AppState,
    /// First `^Q` asks the loop to leave.
    pub should_quit: bool,
    /// Second `^Q` leaves without a final render.
    pub force_quit: bool,
    bridge: Option<GrpcBridge>,
    tick: Duration,
    last_refresh: Option<Instant>,
    details_request: Option<String>,
    priority_command: Option<PriorityCommand>,
    priority_commands: Vec<PriorityCommand>,
    ignored: HashSet<String>,
    /// Open input prompt, if any.
    pub prompt: Option<InputState>,
    prompt_history: Vec<String>,
    streams: Option<Streams>,
    connection: ConnectionManager,
    endpoint: Option<String>,
    timeout: Duration,
    throttle: ThrottleKeyMap,
}

impl App {
    /// Builds an app that can talk to a daemon.
    pub fn new(state: AppState, bridge: GrpcBridge, tick: Duration) -> Self {
        Self {
            state,
            should_quit: false,
            force_quit: false,
            bridge: Some(bridge),
            tick,
            last_refresh: None,
            details_request: None,
            priority_command: None,
            priority_commands: Vec::new(),
            ignored: HashSet::new(),
            prompt: None,
            prompt_history: Vec::new(),
            streams: None,
            connection: ConnectionManager::new(),
            endpoint: None,
            timeout: DEFAULT_RPC_TIMEOUT,
            throttle: ThrottleKeyMap::new("qwerty"),
        }
    }

    /// Builds an app without a terminal or a daemon (tests, headless mode).
    pub fn new_headless(state: AppState) -> Self {
        Self {
            state,
            should_quit: false,
            force_quit: false,
            bridge: None,
            tick: DEFAULT_TICK,
            last_refresh: None,
            details_request: None,
            priority_command: None,
            priority_commands: Vec::new(),
            ignored: HashSet::new(),
            prompt: None,
            prompt_history: Vec::new(),
            streams: None,
            connection: ConnectionManager::new(),
            endpoint: None,
            timeout: DEFAULT_RPC_TIMEOUT,
            throttle: ThrottleKeyMap::new("qwerty"),
        }
    }

    /// Overrides the refresh interval.
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Remembers the endpoint so health checks can reconnect lazily.
    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        self.endpoint = Some(endpoint.to_string());
        self
    }

    /// Starts the background event and stats streams for `endpoint`.
    pub fn with_streams(mut self, endpoint: &str) -> Self {
        self.endpoint = Some(endpoint.to_string());
        self.streams = Some(Streams::spawn(endpoint, self.timeout));
        self
    }

    /// Applies the configured theme.
    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.state.theme = theme;
        self
    }

    /// Records the active theme id so the picker can highlight it.
    pub fn with_theme_id(mut self, id: ThemeId) -> Self {
        self.state.theme_id = id;
        self
    }

    /// Overrides the per-RPC timeout used for lazy reconnects.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Appends a message to the log view.
    pub fn log(&mut self, message: &str) {
        self.state.log.push(message);
    }

    /// Applies everything the background streams buffered so far.
    pub fn poll_streams(&mut self) -> usize {
        let mut applied = 0;

        while let Some(update) = self.streams.as_mut().and_then(Streams::try_recv) {
            match update {
                StreamUpdate::Event(event) => self.state.apply_event(&event),
                StreamUpdate::Stats(stats) => self.state.apply_stats(&stats),
            }
            applied += 1;
        }

        applied
    }

    /// Pings the daemon, reconnecting lazily when the channel is gone.
    pub async fn check_health(&mut self) -> Option<bool> {
        let endpoint = self.endpoint.clone()?;
        self.connection.mark_health_checked(Instant::now());

        if self.bridge.is_none() {
            return Some(match GrpcBridge::connect(&endpoint, self.timeout).await {
                Ok(bridge) => {
                    self.bridge = Some(bridge);
                    self.log("reconnected to the daemon");
                    let _ = self.refresh().await;
                    true
                }
                Err(error) => {
                    self.record_failure(&error);
                    false
                }
            });
        }

        let version = self.bridge.as_mut()?.version().await;
        match version {
            Ok(_) => {
                self.connection.record_success();
                self.state.connection = ConnectionStatus::Connected;
                self.state.reconnect_in = None;
                Some(true)
            }
            Err(error) => {
                self.record_failure(&error);
                Some(false)
            }
        }
    }

    /// Uses the throttle key layout (`qwerty`, `azerty`, `qwertz`, `dvorak`).
    pub fn with_layout(mut self, layout: &str) -> Self {
        self.throttle = ThrottleKeyMap::new(layout);
        self
    }

    /// Dispatches one already-read terminal event.
    ///
    /// Returns the action the front end still has to execute, if any.
    pub fn handle_event(&mut self, event: AppEvent) -> Option<TuiAction> {
        match event {
            AppEvent::Key { code, modifiers } => self.handle_key(code, modifiers),
            AppEvent::Resize { cols, rows } => {
                self.handle_resize(cols, rows);
                None
            }
            AppEvent::Mouse {
                kind,
                column,
                row,
                modifiers,
            } => self.handle_mouse(kind, column, row, modifiers),
        }
    }

    /// Handles a key press.
    ///
    /// `^Q` requests a graceful exit; a second `^Q` forces it. Everything else
    /// is dispatched to the list or the details pane, whichever is open.
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<TuiAction> {
        if let (KeyCode::Char('q'), KeyModifiers::CONTROL) = (code, modifiers) {
            if self.should_quit {
                self.force_quit = true;
            } else {
                self.should_quit = true;
            }
            return None;
        }

        if self.state.show_log {
            return self.handle_log_key(code, modifiers);
        }

        if self.prompt.is_some() {
            return self.handle_prompt_key(code, modifiers);
        }

        if self.state.theme_picker.is_some() {
            return self.handle_picker_key(code, modifiers);
        }

        if self.state.palette.is_some() {
            return self.handle_palette_key(code, modifiers);
        }

        if self.state.menu.is_some() {
            return self.handle_menu_key(code, modifiers);
        }

        if self.state.details.is_some() {
            self.handle_details_key(code, modifiers);
            return None;
        }

        self.handle_list_key(code, modifiers)
    }

    /// Keys active while the log view is shown.
    fn handle_log_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<TuiAction> {
        match (code, modifiers) {
            (KeyCode::Char(' '), _)
            | (KeyCode::Esc, _)
            | (KeyCode::Char('L'), _)
            | (KeyCode::Char('l'), _) => {
                self.state.show_log = false;
            }
            (KeyCode::Down, _) => self.state.log.scroll_down(),
            (KeyCode::Up, _) => self.state.log.scroll_up(),
            _ => {}
        }

        None
    }

    /// Keys active while a prompt is open.
    fn handle_prompt_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<TuiAction> {
        let result = self
            .prompt
            .as_mut()
            .map(|prompt| (prompt.mode(), prompt.handle_key(code, modifiers)));

        match result {
            None | Some((_, InputResult::Pending)) => None,
            Some((_, InputResult::Cancelled)) => {
                self.prompt = None;
                None
            }
            Some((mode, InputResult::Submitted(text))) => {
                self.remember_prompt(&text);
                self.prompt = None;
                self.submit_prompt(mode, &text)
            }
        }
    }

    /// Opens a prompt seeded with the command history.
    fn open_prompt(&mut self, mode: PromptMode) {
        let mut prompt = InputState::new(mode);
        for entry in &self.prompt_history {
            prompt.add_history(entry);
        }
        self.prompt = Some(prompt);
    }

    /// Remembers a submitted prompt line, bounded to the newest 100.
    fn remember_prompt(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        self.prompt_history.push(text.to_string());
        if self.prompt_history.len() > PROMPT_HISTORY_LIMIT {
            self.prompt_history.remove(0);
        }
    }

    /// Turns submitted prompt text into an action.
    fn submit_prompt(&mut self, mode: PromptMode, text: &str) -> Option<TuiAction> {
        match mode {
            PromptMode::Command => self.parse_command(text),
            PromptMode::LoadStart => Some(TuiAction::Load {
                path: text.to_string(),
                start: true,
            }),
            PromptMode::LoadNormal => Some(TuiAction::Load {
                path: text.to_string(),
                start: false,
            }),
            PromptMode::Filter => {
                let clean = text.trim();
                self.state.filter = if clean.is_empty() {
                    None
                } else {
                    Some(clean.to_string())
                };
                self.state.clamp_focus();
                None
            }
        }
    }

    /// Parses `command` or `command=1,2` into an executable action.
    fn parse_command(&mut self, text: &str) -> Option<TuiAction> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }

        let (command, raw_args) = match text.split_once('=') {
            Some((command, args)) => (command.trim(), args),
            None => (text, ""),
        };

        if command.is_empty() {
            self.state.notice = Some(format!("invalid command `{text}`"));
            return None;
        }

        let mut args = Vec::new();
        for token in raw_args
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            match token.parse::<i64>() {
                Ok(value) => args.push(value),
                Err(_) => {
                    self.state.notice = Some(format!("invalid argument `{token}`"));
                    return None;
                }
            }
        }

        Some(TuiAction::Execute {
            command: command.to_string(),
            args,
            hash: None,
        })
    }

    /// Keys active while the context menu is open.
    fn handle_menu_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<TuiAction> {
        let result = self
            .state
            .menu
            .as_mut()
            .map(|menu| menu.handle_key(code, modifiers));

        match result {
            None | Some(MenuResult::Pending) => None,
            Some(MenuResult::Closed) => {
                self.state.menu = None;
                None
            }
            Some(MenuResult::Activated(action)) => {
                self.state.menu = None;
                Some(action)
            }
        }
    }

    /// Opens the theme picker over the current palette.
    fn open_theme_picker(&mut self) {
        self.state.theme_picker = Some(self.state.theme_id);
    }

    /// Keys active while the theme picker is open.
    fn handle_picker_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<TuiAction> {
        let selected = self.state.theme_picker?;
        match (code, modifiers) {
            (KeyCode::Down, _) => self.state.theme_picker = Some(selected.next()),
            (KeyCode::Up, _) => self.state.theme_picker = Some(selected.prev()),
            (KeyCode::Enter, _) => {
                self.state.set_theme_id(selected);
                self.state.theme_picker = None;
                self.state.notice = Some(format!("theme updated to {}", selected.display_name()));
                self.log(&format!("theme changed to {}", selected.display_name()));
            }
            (KeyCode::Esc, _) | (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
                self.state.theme_picker = None;
            }
            _ => {}
        }
        None
    }

    /// Opens the command palette.
    fn open_palette(&mut self) {
        self.state.palette = Some(PaletteState::new());
    }

    /// Keys active while the command palette is open.
    fn handle_palette_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<TuiAction> {
        match (code, modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
                self.state.palette = None;
            }
            (KeyCode::Enter, _) => {
                let command = self
                    .state
                    .palette
                    .as_ref()
                    .and_then(|palette| palette.selected_command());
                self.state.palette = None;
                if let Some(command) = command {
                    return self.activate_command(command.id);
                }
            }
            (KeyCode::Up, _) => {
                if let Some(palette) = self.state.palette.as_mut() {
                    palette.move_up();
                }
            }
            (KeyCode::Down, _) => {
                if let Some(palette) = self.state.palette.as_mut() {
                    palette.move_down();
                }
            }
            (KeyCode::Backspace, _) => {
                if let Some(palette) = self.state.palette.as_mut() {
                    palette.backspace();
                }
            }
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                if let Some(palette) = self.state.palette.as_mut() {
                    palette.insert(ch);
                }
            }
            _ => {}
        }
        None
    }

    /// Runs the palette command identified by `id`.
    fn activate_command(&mut self, id: &str) -> Option<TuiAction> {
        match id {
            "torrent.start" => self.control_focused(TorrentAction::Start),
            "torrent.stop" => self.control_focused(TorrentAction::Stop),
            "torrent.recheck" => self.control_focused(TorrentAction::CheckHash),
            "torrent.remove" => self.remove_focused(),
            "torrent.add" => Some(TuiAction::OpenPrompt(PromptKind::Load)),
            "torrent.command" => Some(TuiAction::OpenPrompt(PromptKind::Command)),
            "filter" => Some(TuiAction::OpenPrompt(PromptKind::Filter)),
            "view.main" => {
                self.switch_view(ActiveView::Main);
                None
            }
            "view.name" => {
                self.switch_view(ActiveView::Name);
                None
            }
            "view.started" => {
                self.switch_view(ActiveView::Started);
                None
            }
            "view.stopped" => {
                self.switch_view(ActiveView::Stopped);
                None
            }
            "view.complete" => {
                self.switch_view(ActiveView::Complete);
                None
            }
            "view.incomplete" => {
                self.switch_view(ActiveView::Incomplete);
                None
            }
            "view.hashing" => {
                self.switch_view(ActiveView::Hashing);
                None
            }
            "view.seeding" => {
                self.switch_view(ActiveView::Seeding);
                None
            }
            "view.leeching" => {
                self.switch_view(ActiveView::Leeching);
                None
            }
            "view.active" => {
                self.switch_view(ActiveView::Active);
                None
            }
            "theme.select" => {
                self.open_theme_picker();
                None
            }
            "log.toggle" => {
                self.state.show_log = !self.state.show_log;
                None
            }
            "session.quit" => {
                self.should_quit = true;
                None
            }
            _ => None,
        }
    }

    /// Controls the focused torrent, or every selected one when a selection exists.
    fn control_focused(&mut self, action: TorrentAction) -> Option<TuiAction> {
        if self.state.selection_count() > 0 {
            Some(TuiAction::BulkControlTorrents {
                hashes: self.state.target_hashes(),
                action,
            })
        } else {
            self.state
                .selected()
                .map(|torrent| TuiAction::ControlTorrent {
                    hash: torrent.hash.clone(),
                    action,
                })
        }
    }

    /// Removes the focused torrent, or every selected one when a selection exists.
    fn remove_focused(&mut self) -> Option<TuiAction> {
        if self.state.selection_count() > 0 {
            Some(TuiAction::BulkRemoveTorrents {
                hashes: self.state.target_hashes(),
            })
        } else {
            self.state
                .selected()
                .map(|torrent| TuiAction::RemoveTorrent {
                    hash: torrent.hash.clone(),
                })
        }
    }

    /// The body rectangle the list/details widgets occupy, mirroring `draw`.
    fn body_rect(&self) -> Rect {
        let prompt_rows = if self.prompt.is_some() { 1 } else { 0 };
        let height = self.state.terminal_rows.saturating_sub(2 + prompt_rows);
        Rect::new(0, 1, self.state.terminal_cols, height)
    }

    /// Routes a mouse event to whichever pane is open.
    fn handle_mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        modifiers: KeyModifiers,
    ) -> Option<TuiAction> {
        if self.prompt.is_some() {
            return None;
        }
        if self.state.theme_picker.is_some() {
            match kind {
                MouseEventKind::ScrollDown => {
                    if let Some(id) = self.state.theme_picker {
                        self.state.theme_picker = Some(id.next());
                    }
                }
                MouseEventKind::ScrollUp => {
                    if let Some(id) = self.state.theme_picker {
                        self.state.theme_picker = Some(id.prev());
                    }
                }
                _ => {}
            }
            return None;
        }
        if self.state.palette.is_some() {
            match kind {
                MouseEventKind::ScrollDown => {
                    if let Some(palette) = self.state.palette.as_mut() {
                        palette.move_down();
                    }
                }
                MouseEventKind::ScrollUp => {
                    if let Some(palette) = self.state.palette.as_mut() {
                        palette.move_up();
                    }
                }
                _ => {}
            }
            return None;
        }

        let body = self.body_rect();
        if column < body.x || column >= body.right() || row < body.y || row >= body.bottom() {
            return None;
        }

        if self.state.show_log {
            match kind {
                MouseEventKind::ScrollDown => self.state.log.scroll_down(),
                MouseEventKind::ScrollUp => self.state.log.scroll_up(),
                _ => {}
            }
            return None;
        }

        if self.state.menu.is_some() {
            return self.mouse_menu(kind, column, row, body);
        }
        if self.state.details.is_some() {
            self.mouse_details(kind, column, row, body);
            return None;
        }
        self.mouse_list(kind, column, row, modifiers, body)
    }

    /// Mouse handling while the context menu is open.
    fn mouse_menu(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        body: Rect,
    ) -> Option<TuiAction> {
        match kind {
            MouseEventKind::ScrollDown => {
                if let Some(menu) = self.state.menu.as_mut() {
                    menu.handle_key(KeyCode::Down, KeyModifiers::NONE);
                }
            }
            MouseEventKind::ScrollUp => {
                if let Some(menu) = self.state.menu.as_mut() {
                    menu.handle_key(KeyCode::Up, KeyModifiers::NONE);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let index = self
                    .state
                    .menu
                    .as_ref()
                    .and_then(|menu| menu_entry_at(menu, column, row, body));
                if let Some(index) = index {
                    let action = self.state.menu.as_mut().and_then(|menu| {
                        menu.select(index);
                        menu.activate()
                    });
                    self.state.menu = None;
                    return action;
                }
            }
            _ => {}
        }
        None
    }

    /// Mouse handling for the torrent list.
    fn mouse_list(
        &mut self,
        kind: MouseEventKind,
        _column: u16,
        row: u16,
        modifiers: KeyModifiers,
        body: Rect,
    ) -> Option<TuiAction> {
        match kind {
            MouseEventKind::ScrollDown => {
                self.state.focus_down();
                return None;
            }
            MouseEventKind::ScrollUp => {
                self.state.focus_up();
                return None;
            }
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right) => {}
            _ => return None,
        }

        let visible = self.state.visible_torrents();
        let rows = visible_rows(body.height);
        let range = viewport(self.state.focus_index, visible.len(), rows);
        let offset = row.saturating_sub(body.y + HEADER_ROWS) as usize;
        let index = range.start + offset;
        if offset >= rows || index >= visible.len() {
            return None;
        }

        let hash = visible[index].hash.clone();
        let was_focused = self.state.focus_index == index;
        self.state.focus_index = index;

        match kind {
            MouseEventKind::Down(MouseButton::Right) => self.open_menu(),
            MouseEventKind::Down(MouseButton::Left)
                if modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.state.toggle_select_focused();
            }
            MouseEventKind::Down(MouseButton::Left) if was_focused => {
                self.details_request = Some(hash);
            }
            _ => {}
        }
        None
    }

    /// Mouse handling for the details pane: tab-bar clicks and list scrolling.
    fn mouse_details(&mut self, kind: MouseEventKind, column: u16, row: u16, body: Rect) {
        if row == body.y {
            if let MouseEventKind::Down(MouseButton::Left) = kind {
                let offset = column.saturating_sub(body.x) as usize;
                if let Some(tab) = tab_at_offset(offset) {
                    if let Some(details) = self.state.details.as_mut() {
                        details.active_tab = tab;
                    }
                }
            }
            return;
        }

        let active_tab = match self.state.details.as_ref() {
            Some(details) => details.active_tab,
            None => return,
        };
        let down = matches!(kind, MouseEventKind::ScrollDown);
        let up = matches!(kind, MouseEventKind::ScrollUp);
        if !down && !up {
            return;
        }
        if active_tab == DetailTab::Logs {
            if down {
                self.state.log.scroll_down();
            } else {
                self.state.log.scroll_up();
            }
            return;
        }
        if let Some(details) = self.state.details.as_mut() {
            match active_tab {
                DetailTab::Files => {
                    if down {
                        details.files.focus_down();
                    } else {
                        details.files.focus_up();
                    }
                }
                DetailTab::Peers => {
                    if down {
                        details.peers.focus_down();
                    } else {
                        details.peers.focus_up();
                    }
                }
                DetailTab::Trackers => {
                    if down {
                        details.trackers.focus_down();
                    } else {
                        details.trackers.focus_up();
                    }
                }
                _ => {}
            }
        }
    }

    /// Opens the context menu for the focused torrent, or bulk menu if multiple selected.
    fn open_menu(&mut self) {
        let entries = if self.state.selection_count() > 0 {
            let hashes = self.state.target_hashes();
            let count = hashes.len();
            bulk_menu_entries(hashes, count)
        } else {
            match self.state.selected() {
                Some(torrent) => menu_entries(torrent),
                None => return,
            }
        };

        self.state.menu = Some(MenuState::new(entries));
    }

    /// Replaces the prompt text (used by tests and TASK-16 scripting).
    pub fn set_prompt_text(&mut self, text: &str) {
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.set_text(text);
        }
    }

    /// Changes the active view, resets focus, and invalidates the last refresh.
    pub fn switch_view(&mut self, view: ActiveView) {
        if self.state.active_view != view {
            self.state.active_view = view;
            self.state.focus_index = 0;
            self.last_refresh = None;
        }
    }

    /// Keys active while the torrent list is shown.
    fn handle_list_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<TuiAction> {
        match (code, modifiers) {
            (KeyCode::Char(d @ '0'..='9'), KeyModifiers::NONE) => {
                let view = match d {
                    '1' => ActiveView::Main,
                    '2' => ActiveView::Name,
                    '3' => ActiveView::Started,
                    '4' => ActiveView::Stopped,
                    '5' => ActiveView::Complete,
                    '6' => ActiveView::Incomplete,
                    '7' => ActiveView::Hashing,
                    '8' => ActiveView::Seeding,
                    '9' => ActiveView::Leeching,
                    '0' => ActiveView::Active,
                    _ => unreachable!(),
                };
                self.switch_view(view);
                return None;
            }
            (KeyCode::Down, _) => {
                self.state.focus_down();
                return None;
            }
            (KeyCode::Up, _) => {
                self.state.focus_up();
                return None;
            }
            (KeyCode::PageDown, _) => {
                let page = self.page_size();
                self.state.focus_page_down(page);
                return None;
            }
            (KeyCode::PageUp, _) => {
                let page = self.page_size();
                self.state.focus_page_up(page);
                return None;
            }
            (KeyCode::Home, _) => {
                self.state.focus_home();
                return None;
            }
            (KeyCode::End, _) => {
                self.state.focus_end();
                return None;
            }
            (KeyCode::Right, _) | (KeyCode::Enter, _) | (KeyCode::Char('l'), _) => {
                self.details_request = self.state.selected().map(|torrent| torrent.hash.clone());
                return None;
            }
            (KeyCode::Char('m'), KeyModifiers::NONE) => {
                self.open_menu();
                return None;
            }
            (KeyCode::Char('L'), _) => {
                self.state.show_log = true;
                return None;
            }
            (KeyCode::Char(' '), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.state.toggle_select_focused();
                return None;
            }
            (KeyCode::Char('a') | KeyCode::Char('A'), KeyModifiers::CONTROL) => {
                self.state.select_all();
                return None;
            }
            (KeyCode::Char('*'), _) => {
                self.state.invert_selection();
                return None;
            }
            (KeyCode::Esc, _) if self.state.filter.is_some() => {
                self.state.filter = None;
                self.state.clamp_focus();
                return None;
            }
            (KeyCode::Esc, _) if self.state.selection_count() > 0 => {
                self.state.deselect_all();
                return None;
            }
            (KeyCode::Char('t'), KeyModifiers::NONE) => {
                self.open_theme_picker();
                return None;
            }
            (KeyCode::Char(':'), KeyModifiers::NONE)
            | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.open_palette();
                return None;
            }
            _ => {}
        }

        if let Some(action) = global_action(code, modifiers) {
            return Some(action);
        }

        if self.state.selection_count() > 0 {
            let hashes = self.state.target_hashes();
            let current_priority = self.state.selected().map_or(2, |t| t.priority);
            let any_active = self
                .state
                .torrents
                .iter()
                .any(|t| self.state.selected_hashes.contains(&t.hash) && t.is_active);

            if let Some(action) = key_handler::bulk_torrent_action(
                code,
                modifiers,
                hashes,
                current_priority,
                any_active,
            ) {
                return Some(action);
            }
        }

        if let Some(torrent) = self.state.selected() {
            let ignored = self.ignored.contains(&torrent.hash);

            if let Some(action) = torrent_action(code, modifiers, torrent, ignored) {
                // Reflect `d.ignore_commands` locally as soon as the user asks for it.
                for (hash, value) in ignore_updates(&action) {
                    if value {
                        self.ignored.insert(hash);
                    } else {
                        self.ignored.remove(&hash);
                    }
                }

                return Some(action);
            }
        }

        match code {
            KeyCode::Char(key) if !modifiers.contains(KeyModifiers::CONTROL) => self
                .throttle
                .lookup(key)
                .and_then(|action| self.throttle_action(action)),
            _ => None,
        }
    }

    /// Builds the throttle command for one key press.
    fn throttle_action(&mut self, action: ThrottleAction) -> Option<TuiAction> {
        let current = match action {
            ThrottleAction::UploadUp(_) | ThrottleAction::UploadDown(_) => {
                self.state.session.throttle_up_limit
            }
            ThrottleAction::DownloadUp(_) | ThrottleAction::DownloadDown(_) => {
                self.state.session.throttle_down_limit
            }
        };

        let limit = u64::try_from(current).unwrap_or(0);
        let next = ThrottleKeyMap::adjust(limit, action);

        Some(TuiAction::Execute {
            command: action.command().to_string(),
            args: vec![i64::try_from(next).unwrap_or(i64::MAX)],
            hash: None,
        })
    }

    /// Keys active while the details pane is shown.
    ///
    /// `h`, `^B` and Left on the first tab return to the list; Left/Right (and
    /// `h`'s counterpart `l`) move between tabs; `1`–`6` select a tab directly.
    fn handle_details_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let active_tab = match self.state.details.as_ref() {
            Some(details) => details.active_tab,
            None => return,
        };

        match (code, modifiers) {
            (KeyCode::Char('x'), _) | (KeyCode::Char('v'), _) | (KeyCode::Insert, _)
                if active_tab == DetailTab::Files =>
            {
                if let Some(details) = self.state.details.as_mut() {
                    details.files.toggle_select_focused();
                }
            }
            (KeyCode::Char('a'), KeyModifiers::CONTROL) if active_tab == DetailTab::Files => {
                if let Some(details) = self.state.details.as_mut() {
                    details.files.select_all();
                }
            }
            (KeyCode::Char('i'), _) | (KeyCode::Char('I'), _) if active_tab == DetailTab::Files => {
                if let Some(details) = self.state.details.as_mut() {
                    details.files.invert_selection();
                }
            }
            (KeyCode::Esc, _)
                if active_tab == DetailTab::Files
                    && self
                        .state
                        .details
                        .as_ref()
                        .map_or(0, |d| d.files.selection_count())
                        > 0 =>
            {
                if let Some(details) = self.state.details.as_mut() {
                    details.files.deselect_all();
                }
            }
            (KeyCode::Esc, _) | (KeyCode::Backspace, _) => {
                self.state.details = None;
            }
            (KeyCode::Char('h'), _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.state.details = None;
            }
            (KeyCode::Left, _) if active_tab == DetailTab::Info => {
                self.state.details = None;
            }
            (KeyCode::Left, _) => {
                if let Some(details) = self.state.details.as_mut() {
                    details.previous_tab();
                }
            }
            (KeyCode::Right, _) | (KeyCode::Char('l'), _) => {
                if let Some(details) = self.state.details.as_mut() {
                    details.next_tab();
                }
            }
            (KeyCode::Char(number @ '1'..='6'), _) => {
                if let Some(details) = self.state.details.as_mut() {
                    details.select_tab(number as u8 - b'0');
                }
            }
            (KeyCode::Down, _) if active_tab == DetailTab::Logs => {
                self.state.log.scroll_down();
            }
            (KeyCode::Up, _) if active_tab == DetailTab::Logs => {
                self.state.log.scroll_up();
            }
            (KeyCode::Down, _) => {
                if let Some(details) = self.state.details.as_mut() {
                    match active_tab {
                        DetailTab::Files => details.files.focus_down(),
                        DetailTab::Peers => details.peers.focus_down(),
                        DetailTab::Trackers => details.trackers.focus_down(),
                        _ => {}
                    }
                }
            }
            (KeyCode::Up, _) => {
                if let Some(details) = self.state.details.as_mut() {
                    match active_tab {
                        DetailTab::Files => details.files.focus_up(),
                        DetailTab::Peers => details.peers.focus_up(),
                        DetailTab::Trackers => details.trackers.focus_up(),
                        _ => {}
                    }
                }
            }
            (KeyCode::PageDown, _) => {
                let page = self.page_size();
                if let Some(details) = self.state.details.as_mut() {
                    match active_tab {
                        DetailTab::Files => details.files.focus_page_down(page),
                        DetailTab::Peers => details.peers.focus_page_down(page),
                        DetailTab::Trackers => details.trackers.focus_page_down(page),
                        _ => {}
                    }
                }
            }
            (KeyCode::PageUp, _) => {
                let page = self.page_size();
                if let Some(details) = self.state.details.as_mut() {
                    match active_tab {
                        DetailTab::Files => details.files.focus_page_up(page),
                        DetailTab::Peers => details.peers.focus_page_up(page),
                        DetailTab::Trackers => details.trackers.focus_page_up(page),
                        _ => {}
                    }
                }
            }
            (KeyCode::Char(' '), _) | (KeyCode::Char('*'), _) if active_tab == DetailTab::Files => {
                self.cycle_file_priority();
            }
            _ => {}
        }
    }

    /// Takes the hash of a torrent whose details were requested by a key press.
    pub fn take_details_request(&mut self) -> Option<String> {
        self.details_request.take()
    }

    /// Fetches details for `hash` and opens the details pane.
    pub async fn open_details(&mut self, hash: &str) -> Option<Result<(), BridgeError>> {
        let bridge = self.bridge.as_mut()?;

        match bridge.get_torrent(hash).await {
            Ok(details) => {
                self.state.details = Some(DetailsState::from_proto(&details));
                Some(Ok(()))
            }
            Err(error) => Some(Err(error)),
        }
    }

    /// Rows of the torrent list that fit into the current terminal.
    fn page_size(&self) -> usize {
        visible_rows(self.state.terminal_rows).max(1)
    }

    /// Records the new terminal dimensions.
    pub fn handle_resize(&mut self, cols: u16, rows: u16) {
        self.state.terminal_cols = cols.max(1);
        self.state.terminal_rows = rows.max(1);
    }

    /// `true` when a refresh is due at `now`.
    pub fn refresh_due(&self, now: Instant) -> bool {
        match self.last_refresh {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= self.tick,
        }
    }

    /// Records that a refresh attempt started at `now`.
    pub fn mark_refreshed(&mut self, now: Instant) {
        self.last_refresh = Some(now);
    }

    /// Refreshes through the bridge, if one is attached.
    pub async fn refresh(&mut self) -> Option<Result<(), BridgeError>> {
        let bridge = self.bridge.as_mut()?;
        let result = self.state.refresh(bridge).await;
        self.mark_refreshed(Instant::now());

        match &result {
            Ok(()) => {
                let was_disconnected = self.state.connection != ConnectionStatus::Connected;
                self.connection.record_success();
                self.state.connection = ConnectionStatus::Connected;
                self.state.reconnect_in = None;
                self.state.stale = false;
                if was_disconnected {
                    self.log("connected to the daemon");
                }
            }
            Err(error) => {
                let message = error.to_string();
                self.record_failure(error);
                self.state.notice = Some(message.clone());
                self.log(&format!("refresh failed: {message}"));
            }
        }

        Some(result)
    }

    /// Records a failed call and publishes the reconnection countdown.
    fn record_failure(&mut self, error: &BridgeError) {
        self.connection.record_failure();
        self.state.connection = ConnectionStatus::Reconnecting;
        self.state.reconnect_in = Some(self.connection.next_backoff().as_secs());
        self.state.stale = true;
        let message = error.to_string();
        self.state.notice = Some(message);
    }

    /// Cycles the focused or selected file(s) priority and queues the matching command(s).
    fn cycle_file_priority(&mut self) {
        let Some(details) = self.state.details.as_mut() else {
            return;
        };
        let hash = details.info.hash.clone();

        if details.files.selection_count() > 0 {
            let target_indices = details.files.selected_indices.clone();
            let base_prio = details.files.focused().map_or(1, |f| f.priority);
            let next_prio = FileListState::next_priority(base_prio);

            for entry in &mut details.files.entries {
                if !entry.is_directory && target_indices.contains(&entry.index) {
                    entry.priority = next_prio;
                    self.priority_commands.push(PriorityCommand {
                        file_index: entry.index,
                        priority: next_prio,
                        hash: hash.clone(),
                    });
                }
            }
            if self.priority_command.is_none() && !self.priority_commands.is_empty() {
                self.priority_command = Some(self.priority_commands.remove(0));
            }
        } else {
            let Some(file) = details.files.focused_mut() else {
                return;
            };

            file.priority = FileListState::next_priority(file.priority);
            self.priority_command = Some(PriorityCommand {
                file_index: file.index,
                priority: file.priority,
                hash,
            });
        }
    }

    /// Takes the priority change waiting to be sent to the daemon.
    pub fn take_priority_command(&mut self) -> Option<PriorityCommand> {
        if let Some(cmd) = self.priority_command.take() {
            return Some(cmd);
        }
        if !self.priority_commands.is_empty() {
            return Some(self.priority_commands.remove(0));
        }
        None
    }

    /// Sends a priority change through `CommandService`.
    pub async fn send_priority(
        &mut self,
        command: &PriorityCommand,
    ) -> Option<Result<CommandResponse, BridgeError>> {
        let bridge = self.bridge.as_mut()?;
        let args = vec![
            ObjectValue {
                kind: Some(object_value::Kind::IntValue(i64::from(command.file_index))),
            },
            ObjectValue {
                kind: Some(object_value::Kind::IntValue(i64::from(command.priority))),
            },
        ];

        Some(
            bridge
                .execute(PRIORITY_COMMAND, args, Some(&command.hash))
                .await,
        )
    }

    /// Executes one action against the bridge, if one is attached.
    pub async fn execute_action(&mut self, action: TuiAction) -> Option<Result<(), BridgeError>> {
        for step in flatten(action) {
            match self.execute_step(step).await {
                Some(Ok(())) => self.state.notice = None,
                Some(Err(error)) => {
                    self.state.notice = Some(error.to_string());
                    return Some(Err(error));
                }
                None => return None,
            }
        }

        Some(Ok(()))
    }

    /// Executes a single (non-batch) action.
    async fn execute_step(&mut self, action: TuiAction) -> Option<Result<(), BridgeError>> {
        match action {
            TuiAction::Noop | TuiAction::Batch(_) => Some(Ok(())),
            TuiAction::ControlTorrent { hash, action } => {
                let bridge = self.bridge.as_mut()?;
                Some(bridge.control_torrent(&hash, action).await)
            }
            TuiAction::RemoveTorrent { hash } => {
                let bridge = self.bridge.as_mut()?;
                Some(bridge.remove_torrent(&hash).await)
            }
            TuiAction::SetPriority { hash, priority } => {
                let bridge = self.bridge.as_mut()?;
                Some(bridge.set_torrent_priority(&hash, priority).await)
            }
            TuiAction::BulkControlTorrents { hashes, action } => {
                let bridge = self.bridge.as_mut()?;
                Some(
                    bridge
                        .bulk_control_torrents(&hashes, action)
                        .await
                        .map(|_| ()),
                )
            }
            TuiAction::BulkRemoveTorrents { hashes } => {
                let bridge = self.bridge.as_mut()?;
                for hash in &hashes {
                    self.state.selected_hashes.remove(hash);
                }
                Some(
                    bridge
                        .bulk_remove_torrents(&hashes, false)
                        .await
                        .map(|_| ()),
                )
            }
            TuiAction::BulkSetPriority { hashes, priority } => {
                let bridge = self.bridge.as_mut()?;
                Some(
                    bridge
                        .bulk_set_torrent_priority(&hashes, priority)
                        .await
                        .map(|_| ()),
                )
            }
            TuiAction::Execute {
                command,
                args,
                hash,
            } => {
                let bridge = self.bridge.as_mut()?;
                Some(
                    bridge
                        .execute_ints(&command, &args, hash.as_deref())
                        .await
                        .map(|_response| ()),
                )
            }
            TuiAction::Load { path, start } => {
                let bridge = self.bridge.as_mut()?;
                let command = if start { "load.start" } else { "load.normal" };

                Some(
                    bridge
                        .execute_strings(command, &[path], None)
                        .await
                        .map(|_response| ()),
                )
            }
            TuiAction::OpenPrompt(kind) => {
                self.open_prompt(prompt_mode(kind));
                Some(Ok(()))
            }
        }
    }

    /// Renders one frame: title bar, body, optional prompt line and status bar.
    pub fn draw(&self, frame: &mut Frame) {
        let prompt_rows = if self.prompt.is_some() { 1 } else { 0 };
        let [title_area, body_area, prompt_area, status_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(prompt_rows),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        frame.render_widget(TitleBarWidget::new(&self.state), title_area);

        if self.state.show_log {
            frame.render_widget(LogWidget::new(&self.state.log), body_area);
        } else {
            match self.state.details.as_ref() {
                Some(details) => frame.render_widget(
                    DetailsWidget::new(details, details.active_tab, &self.state.log),
                    body_area,
                ),
                None => frame.render_widget(DownloadListWidget::new(&self.state), body_area),
            }
        }

        if let Some(menu) = self.state.menu.as_ref() {
            frame.render_widget(MenuWidget::new(menu), body_area);
        }

        if let Some(selected) = self.state.theme_picker {
            frame.render_widget(
                ThemePickerModal::new(self.state.theme_id, selected),
                body_area,
            );
        }

        if let Some(palette) = self.state.palette.as_ref() {
            frame.render_widget(PaletteWidget::new(palette), body_area);
        }

        if let Some(prompt) = self.prompt.as_ref() {
            frame.render_widget(InputWidget::new(prompt), prompt_area);
        }

        frame.render_widget(StatusBarWidget::new(&self.state), status_area);
    }

    /// Runs the loop until the user quits.
    ///
    /// Each iteration reads terminal input (off the async workers), dispatches
    /// it, refreshes on schedule and renders one frame.
    pub async fn run(&mut self, terminal: &mut ClientTerminal) -> io::Result<()> {
        loop {
            if self.force_quit {
                return Ok(());
            }
            if self.should_quit {
                terminal.draw(|frame| self.draw(frame))?;
                return Ok(());
            }

            for event in read_events(self.tick.min(MAX_INPUT_WAIT)).await? {
                if let Some(action) = self.handle_event(event) {
                    if let Some(Err(error)) = self.execute_action(action).await {
                        tracing::warn!(%error, "torrent action failed");
                    }
                }
            }

            if let Some(hash) = self.take_details_request() {
                if let Some(Err(error)) = self.open_details(&hash).await {
                    tracing::warn!(%error, "failed to open details for {hash}");
                }
            }

            while let Some(command) = self.take_priority_command() {
                match self.send_priority(&command).await {
                    Some(Ok(response)) if !response.success => {
                        tracing::warn!(message = %response.error_message, "priority change rejected");
                    }
                    Some(Err(error)) => tracing::warn!(%error, "priority change failed"),
                    _ => {}
                }
            }

            self.poll_streams();

            if self.connection.health_due(Instant::now()) && !self.refresh_due(Instant::now()) {
                let _ = self.check_health().await;
            }

            if self.refresh_due(Instant::now()) {
                self.mark_refreshed(Instant::now());
                let _ = self.refresh().await;
            }

            terminal.draw(|frame| self.draw(frame))?;
        }
    }
}

/// Reads pending terminal events without blocking a runtime worker.
async fn read_events(timeout: Duration) -> io::Result<Vec<AppEvent>> {
    tokio::task::spawn_blocking(move || {
        let mut queue = EventQueue::new(EventQueueConfig::default());
        poll_input(&mut queue, timeout)?;

        let mut events = Vec::with_capacity(queue.len());
        while let Some(event) = queue.pop() {
            events.push(event);
        }
        Ok(events)
    })
    .await
    .map_err(io::Error::other)?
}

/// Fills `queue` with the events already waiting, blocking at most `timeout`.
fn poll_input(queue: &mut EventQueue, timeout: Duration) -> io::Result<()> {
    let mut wait = timeout;

    loop {
        if !event::poll(wait)? {
            return Ok(());
        }

        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                queue.push(AppEvent::key(key.code, key.modifiers));
            }
            Event::Resize(cols, rows) => {
                queue.push(AppEvent::resize(cols, rows));
            }
            Event::Mouse(mouse) => {
                queue.push(AppEvent::mouse(
                    mouse.kind,
                    mouse.column,
                    mouse.row,
                    mouse.modifiers,
                ));
            }
            // Paste and focus reporting are not bound yet.
            _ => {}
        }

        wait = Duration::ZERO;
    }
}

/// Expands nested [`TuiAction::Batch`] values into a flat list.
fn flatten(action: TuiAction) -> Vec<TuiAction> {
    match action {
        TuiAction::Batch(actions) => actions.into_iter().flat_map(flatten).collect(),
        action => vec![action],
    }
}

/// Extracts the `d.ignore_commands` values an action wants to set.
fn ignore_updates(action: &TuiAction) -> Vec<(String, bool)> {
    match action {
        TuiAction::Execute {
            command,
            args,
            hash: Some(hash),
        } if *command == key_handler::IGNORE_COMMANDS => match args.first() {
            Some(value) => vec![(hash.clone(), *value != 0)],
            None => Vec::new(),
        },
        TuiAction::Batch(steps) => steps.iter().flat_map(ignore_updates).collect(),
        _ => Vec::new(),
    }
}

/// Maps a prompt request onto the input mode.
fn prompt_mode(kind: PromptKind) -> PromptMode {
    match kind {
        PromptKind::Command => PromptMode::Command,
        PromptKind::Load => PromptMode::LoadStart,
        PromptKind::Filter => PromptMode::Filter,
    }
}

/// Menu entries available for `torrent`.
fn menu_entries(torrent: &crate::state::TorrentViewModel) -> Vec<MenuEntry> {
    let hash = torrent.hash.clone();
    let active = torrent.is_active;
    let priority = torrent.priority;

    vec![
        MenuEntry::new(
            "Start",
            TuiAction::ControlTorrent {
                hash: hash.clone(),
                action: crate::actions::TorrentAction::Start,
            },
            !active,
        ),
        MenuEntry::new(
            "Stop",
            TuiAction::ControlTorrent {
                hash: hash.clone(),
                action: crate::actions::TorrentAction::Stop,
            },
            active,
        ),
        MenuEntry::new(
            "Close",
            TuiAction::ControlTorrent {
                hash: hash.clone(),
                action: crate::actions::TorrentAction::Close,
            },
            true,
        ),
        MenuEntry::new(
            "Check Hash",
            TuiAction::ControlTorrent {
                hash: hash.clone(),
                action: crate::actions::TorrentAction::CheckHash,
            },
            true,
        ),
        MenuEntry::new(
            "Priority Low",
            TuiAction::SetPriority {
                hash: hash.clone(),
                priority: 1,
            },
            priority != 1,
        ),
        MenuEntry::new(
            "Priority Normal",
            TuiAction::SetPriority {
                hash: hash.clone(),
                priority: 2,
            },
            priority != 2,
        ),
        MenuEntry::new(
            "Priority High",
            TuiAction::SetPriority {
                hash: hash.clone(),
                priority: 3,
            },
            priority != 3,
        ),
        MenuEntry::new("Remove", TuiAction::RemoveTorrent { hash }, true),
    ]
}

/// Menu entries available when multiple torrents are selected.
fn bulk_menu_entries(hashes: Vec<String>, count: usize) -> Vec<MenuEntry> {
    vec![
        MenuEntry::new(
            format!("Start ({count} torrents)"),
            TuiAction::BulkControlTorrents {
                hashes: hashes.clone(),
                action: crate::actions::TorrentAction::Start,
            },
            true,
        ),
        MenuEntry::new(
            format!("Stop ({count} torrents)"),
            TuiAction::BulkControlTorrents {
                hashes: hashes.clone(),
                action: crate::actions::TorrentAction::Stop,
            },
            true,
        ),
        MenuEntry::new(
            format!("Close ({count} torrents)"),
            TuiAction::BulkControlTorrents {
                hashes: hashes.clone(),
                action: crate::actions::TorrentAction::Close,
            },
            true,
        ),
        MenuEntry::new(
            format!("Check Hash ({count} torrents)"),
            TuiAction::BulkControlTorrents {
                hashes: hashes.clone(),
                action: crate::actions::TorrentAction::CheckHash,
            },
            true,
        ),
        MenuEntry::new(
            format!("Priority Low ({count} torrents)"),
            TuiAction::BulkSetPriority {
                hashes: hashes.clone(),
                priority: 1,
            },
            true,
        ),
        MenuEntry::new(
            format!("Priority Normal ({count} torrents)"),
            TuiAction::BulkSetPriority {
                hashes: hashes.clone(),
                priority: 2,
            },
            true,
        ),
        MenuEntry::new(
            format!("Priority High ({count} torrents)"),
            TuiAction::BulkSetPriority {
                hashes: hashes.clone(),
                priority: 3,
            },
            true,
        ),
        MenuEntry::new(
            format!("Remove ({count} torrents)"),
            TuiAction::BulkRemoveTorrents { hashes },
            true,
        ),
    ]
}

/// Maps a tab-bar column offset to the tab it falls on, mirroring `render_tab_bar`.
fn tab_at_offset(offset: usize) -> Option<DetailTab> {
    let mut cursor = 0;
    for tab in DetailTab::ALL {
        let width = 4 + tab.title().chars().count();
        if offset < cursor + width {
            return Some(tab);
        }
        cursor += width + 1;
    }
    None
}

/// Maps a menu click to the entry index, mirroring `MenuWidget` geometry.
fn menu_entry_at(menu: &MenuState, column: u16, row: u16, area: Rect) -> Option<usize> {
    if menu.is_empty() || area.width < 6 || area.height < 3 {
        return None;
    }
    let label_width = menu
        .entries()
        .iter()
        .map(|entry| entry.label.chars().count())
        .max()
        .unwrap_or(8) as u16;
    let width = (label_width + 4).min(area.width);
    let height = (menu.len() as u16 + 2).min(area.height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let inner_x = popup.x + 1;
    let inner_y = popup.y + 1;
    let inner_width = popup.width.saturating_sub(2);
    let inner_height = popup.height.saturating_sub(2);
    if column < inner_x
        || column >= inner_x + inner_width
        || row < inner_y
        || row >= inner_y + inner_height
    {
        return None;
    }
    let range = viewport(menu.selected(), menu.len(), inner_height as usize);
    let index = range.start + (row - inner_y) as usize;
    if index < menu.len() {
        Some(index)
    } else {
        None
    }
}
