//! Application shell: key handling, refresh scheduling, rendering and the loop.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::Frame;

use crate::bridge::{BridgeError, GrpcBridge};
use crate::events::{AppEvent, EventQueue, EventQueueConfig};
use crate::state::{AppState, DetailTab, DetailsState};
use crate::terminal::ClientTerminal;
use crate::widgets::download_list::visible_rows;
use crate::widgets::{DetailsWidget, DownloadListWidget, StatusBarWidget, TitleBarWidget};

/// Tick used when the caller does not configure one.
pub const DEFAULT_TICK: Duration = Duration::from_millis(500);

/// Longest the loop blocks waiting for input, so refreshes stay on schedule.
const MAX_INPUT_WAIT: Duration = Duration::from_millis(50);

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
        }
    }

    /// Overrides the refresh interval.
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Dispatches one already-read terminal event.
    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key { code, modifiers } => self.handle_key(code, modifiers),
            AppEvent::Resize { cols, rows } => self.handle_resize(cols, rows),
        }
    }

    /// Handles a key press.
    ///
    /// `^Q` requests a graceful exit; a second `^Q` forces it. Everything else
    /// is dispatched to the list or the details pane, whichever is open.
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if let (KeyCode::Char('q'), KeyModifiers::CONTROL) = (code, modifiers) {
            if self.should_quit {
                self.force_quit = true;
            } else {
                self.should_quit = true;
            }
            return;
        }

        if self.state.details.is_some() {
            self.handle_details_key(code, modifiers);
        } else {
            self.handle_list_key(code, modifiers);
        }
    }

    /// Keys active while the torrent list is shown.
    fn handle_list_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match (code, modifiers) {
            (KeyCode::Down, _) => self.state.focus_down(),
            (KeyCode::Up, _) => self.state.focus_up(),
            (KeyCode::PageDown, _) => self.state.focus_page_down(self.page_size()),
            (KeyCode::PageUp, _) => self.state.focus_page_up(self.page_size()),
            (KeyCode::Home, _) => self.state.focus_home(),
            (KeyCode::End, _) => self.state.focus_end(),
            (KeyCode::Right, _) | (KeyCode::Char('l'), _) => {
                self.details_request = self.state.selected().map(|torrent| torrent.hash.clone());
            }
            _ => {}
        }
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
        Some(result)
    }

    /// Renders one frame: title bar, body (list or details), status bar.
    pub fn draw(&self, frame: &mut Frame) {
        let [title_area, body_area, status_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        frame.render_widget(TitleBarWidget::new(&self.state), title_area);

        match self.state.details.as_ref() {
            Some(details) => {
                frame.render_widget(DetailsWidget::new(details, details.active_tab), body_area)
            }
            None => frame.render_widget(DownloadListWidget::new(&self.state), body_area),
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
                self.handle_event(event);
            }

            if let Some(hash) = self.take_details_request() {
                if let Some(Err(error)) = self.open_details(&hash).await {
                    tracing::warn!(%error, "failed to open details for {hash}");
                }
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
