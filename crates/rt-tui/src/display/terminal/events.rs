//! Translation of crossterm events and bounded event queueing.
//!
//! This module deliberately lives next to [`crate::display::terminal`] instead
//! of inside the UI/input layers so that the terminal event loop can be tested
//! without a real terminal. It has no dependency on the window/UI code.
//!
//! The main loop is expected to use [`TerminalEventReader`], which combines
//! [`EventTranslator`] with an [`EventQueue`]:
//!
//! ```no_run
//! use rt_tui::display::terminal::{EventQueueConfig, TerminalEventReader};
//! use std::time::Duration;
//!
//! let mut events = TerminalEventReader::new(80, 24, EventQueueConfig::default());
//! # fn render() {}
//! loop {
//!     events.poll(Duration::from_millis(50)).unwrap();
//!     while let Some(event) = events.pop() {
//!         // Dispatch `event`, ignore `TerminalEvent::Unknown`.
//!     }
//!     render();
//! }
//! ```

use crate::input::{KEY_BACKSPACE, KEY_DC, KEY_END, KEY_HOME, KEY_LEFT, KEY_RIGHT};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};
use std::collections::VecDeque;
use std::io;
use std::time::Duration;

/// Key code for the Up arrow, matching the rest of the TUI.
pub const KEY_UP: i32 = 0x102;
/// Key code for the Down arrow, matching the rest of the TUI.
pub const KEY_DOWN: i32 = 0x103;
/// Key code for Page Up.
pub const KEY_PAGE_UP: i32 = 0x153;
/// Key code for Page Down.
pub const KEY_PAGE_DOWN: i32 = 0x156;
/// Key code for Insert (ncurses `KEY_IC`).
pub const KEY_INSERT: i32 = 0x14b;
/// First function key code (ncurses `KEY_F1`).
pub const KEY_F1: i32 = 0x109;

/// A terminal event translated into the form consumed by the UI.
///
/// [`Unknown`](TerminalEvent::Unknown) is deliberately non-fatal: the main loop
/// can ignore it and keep running. Some raw events (for example key-release
/// events on terminals that report them) are ignored entirely by the
/// translator and never reach this enum.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalEvent {
    /// A key translated to the legacy integer key code used by the UI.
    Key(i32),
    /// A mouse event whose coordinates have been clamped to the canvas.
    Mouse(MouseEvent),
    /// The terminal window gained focus.
    FocusGained,
    /// The terminal window lost focus.
    FocusLost,
    /// Bracketed-paste payload. May contain newlines.
    Paste(String),
    /// The terminal was resized to the given `(columns, rows)`.
    Resize(u32, u32),
    /// An event the backend does not understand. Safe to ignore.
    Unknown,
}

impl TerminalEvent {
    /// Returns `true` for [`TerminalEvent::Resize`].
    pub fn is_resize(&self) -> bool {
        matches!(self, TerminalEvent::Resize(..))
    }

    /// Returns the new size for a resize event.
    pub fn resize(&self) -> Option<(u32, u32)> {
        match self {
            TerminalEvent::Resize(width, height) => Some((*width, *height)),
            _ => None,
        }
    }

    /// Returns `true` for [`TerminalEvent::Unknown`].
    pub fn is_unknown(&self) -> bool {
        matches!(self, TerminalEvent::Unknown)
    }
}

/// Translate a crossterm key event into the legacy integer key code.
///
/// Key-release events are ignored (`None`), as are key codes the UI does not
/// understand. This function never panics and treats unsupported keys as a
/// safe no-op.
pub fn translate_key_event(key_event: KeyEvent) -> Option<i32> {
    // Terminals using the kitty protocol (and Windows) can report releases.
    // Feeding them to the UI would duplicate every key press.
    if key_event.kind == KeyEventKind::Release {
        return None;
    }

    // Handle Ctrl modifier. Ctrl+A..Ctrl+Z map to 1..26, Ctrl+Space and
    // Ctrl+@ map to NUL (0).
    if key_event.modifiers.contains(KeyModifiers::CONTROL) {
        match key_event.code {
            KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                let upper = c.to_ascii_uppercase();
                return Some((upper as u8 - b'@') as i32);
            }
            KeyCode::Char(' ') | KeyCode::Char('@') => return Some(0),
            _ => {}
        }
    }

    match key_event.code {
        KeyCode::Up => Some(KEY_UP),
        KeyCode::Down => Some(KEY_DOWN),
        KeyCode::Left => Some(KEY_LEFT),
        KeyCode::Right => Some(KEY_RIGHT),
        KeyCode::Home => Some(KEY_HOME),
        KeyCode::End => Some(KEY_END),
        KeyCode::PageUp => Some(KEY_PAGE_UP),
        KeyCode::PageDown => Some(KEY_PAGE_DOWN),
        KeyCode::Insert => Some(KEY_INSERT),
        KeyCode::Backspace => Some(KEY_BACKSPACE),
        KeyCode::Delete => Some(KEY_DC),
        KeyCode::Enter => Some(10), // '\n'
        KeyCode::Tab => Some(9),    // '\t'
        KeyCode::Esc => Some(0x1B),
        KeyCode::F(n) if (1..=12).contains(&n) => Some(KEY_F1 + (n as i32 - 1)),
        KeyCode::Char(c) => Some(c as i32),
        // Unsupported key codes (media keys, modifiers, keypad begin, ...)
        // must not terminate the event loop.
        _ => None,
    }
}

/// A stateful translator that keeps track of the terminal bounds.
///
/// Mouse coordinates reported by the terminal are cell coordinates. They are
/// clamped to `[0, width - 1]` / `[0, height - 1]` before being delivered to
/// the UI so an out-of-range report (common during a resize) can never index
/// outside the canvas. If no valid bounds are known, mouse events are ignored.
#[derive(Debug, Clone, Default)]
pub struct EventTranslator {
    bounds: Option<(u32, u32)>,
}

impl EventTranslator {
    /// Create a translator for a terminal of `width` x `height` cells.
    ///
    /// A zero width or height means "bounds unknown"; mouse events are then
    /// ignored until [`set_bounds`](Self::set_bounds) is called.
    pub fn new(width: u32, height: u32) -> Self {
        let mut translator = Self::default();
        translator.set_bounds(width, height);
        translator
    }

    /// Update the terminal bounds. Called by the main loop on resize.
    pub fn set_bounds(&mut self, width: u32, height: u32) {
        self.bounds = if width == 0 || height == 0 {
            None
        } else {
            Some((width, height))
        };
    }

    /// Forget the terminal bounds; subsequent mouse events are ignored.
    pub fn clear_bounds(&mut self) {
        self.bounds = None;
    }

    /// Current `(width, height)`, if known.
    pub fn bounds(&self) -> Option<(u32, u32)> {
        self.bounds
    }

    /// Translate one crossterm event.
    ///
    /// Returns `None` only for events that are intentionally dropped (key
    /// releases and unsupported key codes). Unsupported *event kinds* map to
    /// [`TerminalEvent::Unknown`] so the caller can count or ignore them
    /// without exiting the loop.
    pub fn translate(&mut self, event: Event) -> Option<TerminalEvent> {
        match event {
            Event::Key(key_event) => translate_key_event(key_event).map(TerminalEvent::Key),
            Event::Mouse(mouse_event) => self.translate_mouse(mouse_event),
            Event::FocusGained => Some(TerminalEvent::FocusGained),
            Event::FocusLost => Some(TerminalEvent::FocusLost),
            Event::Paste(text) => Some(TerminalEvent::Paste(text)),
            Event::Resize(width, height) => {
                let width = width as u32;
                let height = height as u32;
                // Keep clamping correct during the resize before the caller
                // gets a chance to update its own canvas.
                self.set_bounds(width, height);
                Some(TerminalEvent::Resize(width, height))
            }
            // New event kinds must be a safe no-op rather than a panic or loop
            // termination. crossterm may add variants in a future release; the
            // arm is unreachable with the variants present in 0.28.
            #[allow(unreachable_patterns)]
            _ => Some(TerminalEvent::Unknown),
        }
    }

    /// Translate a mouse event, clamping its cell coordinates to the bounds.
    ///
    /// Returns `None` when the bounds are unknown or zero-sized.
    pub fn translate_mouse(&self, mouse_event: MouseEvent) -> Option<TerminalEvent> {
        let (width, height) = self.bounds?;
        let column = clamp_coordinate(mouse_event.column, width);
        let row = clamp_coordinate(mouse_event.row, height);

        if column == mouse_event.column && row == mouse_event.row {
            Some(TerminalEvent::Mouse(mouse_event))
        } else {
            Some(TerminalEvent::Mouse(MouseEvent {
                column,
                row,
                ..mouse_event
            }))
        }
    }
}

/// Clamp a zero-based cell coordinate to a positive axis length.
fn clamp_coordinate(value: u16, axis_len: u32) -> u16 {
    // `axis_len` is at least 1 here (bounds are only stored when non-zero).
    // crossterm coordinates are u16, so very large terminals saturate.
    let max = axis_len.saturating_sub(1).min(u16::MAX as u32) as u16;
    value.min(max)
}

/// How to treat a resize event when the queue already contains one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizePolicy {
    /// Replace the previously queued resize with the newest one. Resize storms
    /// therefore occupy at most one slot in the queue.
    Coalesce,
    /// Keep resize events as separate entries; they are dropped normally when
    /// the queue is full.
    Drop,
}

impl Default for ResizePolicy {
    fn default() -> Self {
        Self::Coalesce
    }
}

/// Configuration for [`EventQueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventQueueConfig {
    /// Maximum number of queued events. Zero drops every event.
    pub capacity: usize,
    /// Resize handling when the queue already holds a resize event.
    pub resize_policy: ResizePolicy,
}

impl EventQueueConfig {
    /// Create a config with the default coalescing policy.
    pub const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            resize_policy: ResizePolicy::Coalesce,
        }
    }
}

impl Default for EventQueueConfig {
    fn default() -> Self {
        Self::new(256)
    }
}

/// What happened to an event offered to [`EventQueue::push`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPushOutcome {
    /// The event was appended to the queue.
    Enqueued,
    /// The event replaced an already queued resize.
    Coalesced,
    /// The event was discarded because the queue was full (or capacity was 0).
    Dropped,
    /// The translator intentionally ignored the raw event (for example a key
    /// release). Only produced by [`TerminalEventReader::push_event`].
    Ignored,
}

/// Snapshot of queue counters. Cheap to copy and safe to log periodically.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventQueueStats {
    /// Configured maximum length.
    pub capacity: usize,
    /// Current number of queued events.
    pub len: usize,
    /// Events accepted into the queue (includes coalesced resizes).
    pub pushed: u64,
    /// Events removed with [`EventQueue::pop`] or [`EventQueue::drain`].
    pub popped: u64,
    /// Events rejected because the queue was full.
    pub dropped: u64,
    /// Resize events merged into an already queued resize.
    pub coalesced_resizes: u64,
}

impl EventQueueStats {
    /// Returns `true` when at least one event has been dropped or coalesced.
    pub fn has_pressure(&self) -> bool {
        self.dropped > 0 || self.coalesced_resizes > 0
    }
}

/// A bounded FIFO queue with resize coalescing.
///
/// The queue never grows past [`EventQueueConfig::capacity`]. When it is full,
/// the newest event is dropped and counted in [`EventQueueStats::dropped`];
/// this keeps the event loop responsive under paste/scroll storms instead of
/// allocating without bound.
#[derive(Debug)]
pub struct EventQueue {
    buffer: VecDeque<TerminalEvent>,
    config: EventQueueConfig,
    pushed: u64,
    popped: u64,
    dropped: u64,
    coalesced_resizes: u64,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::with_config(EventQueueConfig::default())
    }
}

impl EventQueue {
    /// Create a queue with the given capacity and coalescing enabled.
    pub fn new(capacity: usize) -> Self {
        Self::with_config(EventQueueConfig::new(capacity))
    }

    /// Create a queue from an explicit configuration.
    pub fn with_config(config: EventQueueConfig) -> Self {
        Self {
            buffer: VecDeque::with_capacity(config.capacity),
            config,
            pushed: 0,
            popped: 0,
            dropped: 0,
            coalesced_resizes: 0,
        }
    }

    /// Current configuration.
    pub fn config(&self) -> EventQueueConfig {
        self.config
    }

    /// Replace the configuration. If the new capacity is smaller than the
    /// current length, the oldest events are discarded and counted as dropped.
    pub fn set_config(&mut self, config: EventQueueConfig) {
        self.config = config;
        self.enforce_capacity();
    }

    /// Change the capacity (see [`set_config`](Self::set_config)).
    pub fn set_capacity(&mut self, capacity: usize) {
        self.config.capacity = capacity;
        self.enforce_capacity();
    }

    /// Change the resize policy.
    pub fn set_resize_policy(&mut self, policy: ResizePolicy) {
        self.config.resize_policy = policy;
    }

    /// Maximum number of events the queue can hold.
    pub fn capacity(&self) -> usize {
        self.config.capacity
    }

    /// Number of currently queued events.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Returns `true` when no events are queued.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Offer an event to the queue.
    ///
    /// With [`ResizePolicy::Coalesce`], a resize replaces the most recently
    /// queued resize instead of consuming another slot. With
    /// [`ResizePolicy::Drop`], resizes are enqueued normally.
    pub fn push(&mut self, event: TerminalEvent) -> EventPushOutcome {
        let is_resize = matches!(&event, TerminalEvent::Resize(..));

        if is_resize && self.config.resize_policy == ResizePolicy::Coalesce {
            if let Some(index) = self.buffer.iter().rposition(TerminalEvent::is_resize) {
                self.buffer[index] = event;
                self.pushed += 1;
                self.coalesced_resizes += 1;
                return EventPushOutcome::Coalesced;
            }
        }

        if self.buffer.len() >= self.config.capacity {
            self.dropped += 1;
            return EventPushOutcome::Dropped;
        }

        self.buffer.push_back(event);
        self.pushed += 1;
        EventPushOutcome::Enqueued
    }

    /// Remove and return the oldest event.
    pub fn pop(&mut self) -> Option<TerminalEvent> {
        let event = self.buffer.pop_front();
        if event.is_some() {
            self.popped += 1;
        }
        event
    }

    /// Remove and return all queued events in FIFO order.
    pub fn drain(&mut self) -> Vec<TerminalEvent> {
        let events: Vec<_> = self.buffer.drain(..).collect();
        self.popped += events.len() as u64;
        events
    }

    /// Discard all queued events without counting them as popped.
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Current counters.
    pub fn stats(&self) -> EventQueueStats {
        EventQueueStats {
            capacity: self.config.capacity,
            len: self.buffer.len(),
            pushed: self.pushed,
            popped: self.popped,
            dropped: self.dropped,
            coalesced_resizes: self.coalesced_resizes,
        }
    }

    fn enforce_capacity(&mut self) {
        while self.buffer.len() > self.config.capacity {
            self.buffer.pop_front();
            self.dropped += 1;
        }
    }
}

/// Polling facade for the terminal event loop.
///
/// `TerminalEventReader` owns the translator (including current bounds) and the
/// bounded queue. The main loop should call [`poll`](Self::poll) once per frame
/// and then drain [`pop`](Self::pop) until it returns `None`.
///
/// Only a bounded number of raw events are read per poll (64 by default, see
/// [`set_max_events_per_poll`](Self::set_max_events_per_poll)). This guarantees
/// the renderer gets a chance to run even during an event storm.
#[derive(Debug)]
pub struct TerminalEventReader {
    translator: EventTranslator,
    queue: EventQueue,
    max_events_per_poll: usize,
}

impl Default for TerminalEventReader {
    fn default() -> Self {
        Self::new(80, 24, EventQueueConfig::default())
    }
}

impl TerminalEventReader {
    /// Create a reader for a terminal of `width` x `height` cells.
    pub fn new(width: u32, height: u32, config: EventQueueConfig) -> Self {
        Self {
            translator: EventTranslator::new(width, height),
            queue: EventQueue::with_config(config),
            max_events_per_poll: 64,
        }
    }

    /// Update the bounds used to clamp mouse coordinates. The translator also
    /// updates itself automatically when it sees a resize event.
    pub fn set_bounds(&mut self, width: u32, height: u32) {
        self.translator.set_bounds(width, height);
    }

    /// Current bounds, if known.
    pub fn bounds(&self) -> Option<(u32, u32)> {
        self.translator.bounds()
    }

    /// Borrow the translator (useful for tests or manual translation).
    pub fn translator(&self) -> &EventTranslator {
        &self.translator
    }

    /// Borrow the underlying queue.
    pub fn queue(&self) -> &EventQueue {
        &self.queue
    }

    /// Mutably borrow the underlying queue.
    pub fn queue_mut(&mut self) -> &mut EventQueue {
        &mut self.queue
    }

    /// Current queue statistics.
    pub fn stats(&self) -> EventQueueStats {
        self.queue.stats()
    }

    /// Number of queued events.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Returns `true` when no events are queued.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Set the maximum number of raw events read by one [`poll`](Self::poll)
    /// call. Values below 1 are treated as 1.
    pub fn set_max_events_per_poll(&mut self, max: usize) {
        self.max_events_per_poll = max.max(1);
    }

    /// Translate and queue a single raw crossterm event.
    ///
    /// This is the testable half of [`poll`](Self::poll) and can also be used
    /// by integrations that read events themselves.
    pub fn push_event(&mut self, event: Event) -> EventPushOutcome {
        match self.translator.translate(event) {
            Some(event) => self.queue.push(event),
            None => EventPushOutcome::Ignored,
        }
    }

    /// Poll for events, translate them, and enqueue the result.
    ///
    /// `timeout` is passed to crossterm. Once the first event arrives, up to
    /// [`max_events_per_poll`](Self::set_max_events_per_poll) raw events are
    /// drained without blocking. Returns the number of *translated* events
    /// offered to the queue (dropped/coalesced events are included; ignored
    /// raw events are not).
    pub fn poll(&mut self, timeout: Duration) -> io::Result<usize> {
        if !crossterm::event::poll(timeout)? {
            return Ok(0);
        }

        let mut read_count = 0;
        let mut translated = 0;
        while read_count < self.max_events_per_poll
            && crossterm::event::poll(Duration::ZERO)?
        {
            read_count += 1;
            let raw = crossterm::event::read()?;
            if let Some(event) = self.translator.translate(raw) {
                self.queue.push(event);
                translated += 1;
            }
        }
        Ok(translated)
    }

    /// Remove the oldest queued event.
    pub fn pop(&mut self) -> Option<TerminalEvent> {
        self.queue.pop()
    }

    /// Remove all queued events in FIFO order.
    pub fn drain(&mut self) -> Vec<TerminalEvent> {
        self.queue.drain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_release_is_ignored() {
        let mut key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(translate_key_event(key), None);
    }

    #[test]
    fn queue_coalesces_resizes() {
        let mut queue = EventQueue::new(4);
        assert_eq!(
            queue.push(TerminalEvent::Resize(10, 10)),
            EventPushOutcome::Enqueued
        );
        assert_eq!(
            queue.push(TerminalEvent::Resize(20, 20)),
            EventPushOutcome::Coalesced
        );
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.stats().coalesced_resizes, 1);
        assert_eq!(queue.pop(), Some(TerminalEvent::Resize(20, 20)));
    }

    #[test]
    fn queue_drops_when_full() {
        let mut queue = EventQueue::new(1);
        assert_eq!(
            queue.push(TerminalEvent::Key(1)),
            EventPushOutcome::Enqueued
        );
        assert_eq!(
            queue.push(TerminalEvent::Key(2)),
            EventPushOutcome::Dropped
        );
        assert_eq!(queue.stats().dropped, 1);
        assert_eq!(queue.pop(), Some(TerminalEvent::Key(1)));
    }
}
