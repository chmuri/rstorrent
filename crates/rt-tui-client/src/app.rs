//! Application shell: key handling, refresh scheduling, rendering and the loop.

use std::collections::HashSet;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::Frame;
use rt_grpc::{object_value, CommandResponse, ObjectValue};

use crate::actions::{PromptKind, TuiAction};
use crate::bridge::{BridgeError, GrpcBridge, DEFAULT_RPC_TIMEOUT};
use crate::config::Theme;
use crate::connection::ConnectionManager;
use crate::events::{AppEvent, EventQueue, EventQueueConfig};
use crate::key_handler::{self, global_action, torrent_action};
use crate::state::{
    ActiveView, AppState, ConnectionStatus, DetailTab, DetailsState, FileListState, MenuEntry,
    MenuResult, MenuState,
};
use crate::stream::{StreamUpdate, Streams};
use crate::terminal::ClientTerminal;
use crate::throttle::{ThrottleAction, ThrottleKeyMap};
use crate::widgets::download_list::visible_rows;
use crate::widgets::input::{InputResult, InputState, PromptMode};
use crate::widgets::InputWidget;
use crate::widgets::{
    DetailsWidget, DownloadListWidget, LogWidget, MenuWidget, StatusBarWidget, TitleBarWidget,
};

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

    /// Opens the context menu for the focused torrent.
    fn open_menu(&mut self) {
        let entries = match self.state.selected() {
            Some(torrent) => menu_entries(torrent),
            None => return,
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
            (KeyCode::Esc, _) if self.state.filter.is_some() => {
                self.state.filter = None;
                self.state.clamp_focus();
                return None;
            }
            _ => {}
        }

        if let Some(action) = global_action(code, modifiers) {
            return Some(action);
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
    /// `h`'s counterpart `l`) move between tabs; `1`–`5` select a tab directly.
    fn handle_details_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let active_tab = match self.state.details.as_ref() {
            Some(details) => details.active_tab,
            None => return,
        };

        match (code, modifiers) {
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
            (KeyCode::Char(number @ '1'..='5'), _) => {
                if let Some(details) = self.state.details.as_mut() {
                    details.select_tab(number as u8 - b'0');
                }
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

    /// Cycles the focused file's priority and queues the matching command.
    fn cycle_file_priority(&mut self) {
        let Some(details) = self.state.details.as_mut() else {
            return;
        };
        let hash = details.info.hash.clone();
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

    /// Takes the priority change waiting to be sent to the daemon.
    pub fn take_priority_command(&mut self) -> Option<PriorityCommand> {
        self.priority_command.take()
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
                Some(details) => {
                    frame.render_widget(DetailsWidget::new(details, details.active_tab), body_area)
                }
                None => frame.render_widget(DownloadListWidget::new(&self.state), body_area),
            }
        }

        if let Some(menu) = self.state.menu.as_ref() {
            frame.render_widget(MenuWidget::new(menu), body_area);
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

            if let Some(command) = self.take_priority_command() {
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
            // Mouse, paste and focus reporting are not bound yet (TASK-10).
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
