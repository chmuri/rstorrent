//! Application shell: key handling, refresh scheduling, rendering and the loop.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::bridge::{BridgeError, GrpcBridge};
use crate::events::{AppEvent, EventQueue, EventQueueConfig};
use crate::state::{AppState, ConnectionStatus};
use crate::terminal::ClientTerminal;

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
    /// `^Q` requests a graceful exit; a second `^Q` forces it. Every other key
    /// stays unbound until TASK-10 adds the shortcut map.
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if let (KeyCode::Char('q'), KeyModifiers::CONTROL) = (code, modifiers) {
            if self.should_quit {
                self.force_quit = true;
            } else {
                self.should_quit = true;
            }
        }
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

    /// Renders one frame.
    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" rstorrent-tui ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let status = match &self.state.connection {
            ConnectionStatus::Connected => {
                format!("connected · {} torrents", self.state.torrents.len())
            }
            ConnectionStatus::Disconnected { reason } => format!("disconnected: {reason}"),
        };
        frame.render_widget(Paragraph::new(status), inner);
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
