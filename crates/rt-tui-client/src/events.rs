//! Bounded input-event queue with resize coalescing.
//!
//! Mirrors the `EventQueueConfig` pattern already used by the in-process
//! `rt-tui` renderer, without depending on it (this crate must stay free of
//! `rt-core`). A burst of resize events collapses into the newest geometry, and
//! the queue never grows past its configured capacity.

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyModifiers};

/// Something the event loop has to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    /// A key press with its modifiers.
    Key {
        /// Which key was pressed.
        code: KeyCode,
        /// Modifier keys held during the press.
        modifiers: KeyModifiers,
    },
    /// The terminal was resized to `cols` × `rows`.
    Resize {
        /// New width in columns.
        cols: u16,
        /// New height in rows.
        rows: u16,
    },
}

impl AppEvent {
    /// Builds a key event.
    pub fn key(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self::Key { code, modifiers }
    }

    /// Builds a resize event.
    pub fn resize(cols: u16, rows: u16) -> Self {
        Self::Resize { cols, rows }
    }

    /// `true` for [`AppEvent::Resize`].
    pub fn is_resize(&self) -> bool {
        matches!(self, Self::Resize { .. })
    }
}

/// What to do when a resize arrives while one is already queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizePolicy {
    /// Replace the queued resize with the newest geometry.
    Coalesce,
    /// Queue every resize in arrival order.
    KeepAll,
}

/// Capacity and resize policy of an [`EventQueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventQueueConfig {
    /// Maximum number of queued events; `0` drops every event.
    pub capacity: usize,
    /// Resize handling policy.
    pub resize_policy: ResizePolicy,
}

impl EventQueueConfig {
    /// Config with the default coalescing policy and the given capacity.
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
    /// The event was appended.
    Enqueued,
    /// The event replaced an already queued resize.
    Coalesced,
    /// The event was discarded because the queue was full.
    Dropped,
}

/// Bounded FIFO of [`AppEvent`]s.
#[derive(Debug)]
pub struct EventQueue {
    config: EventQueueConfig,
    buffer: VecDeque<AppEvent>,
    dropped: u64,
    coalesced_resizes: u64,
}

impl EventQueue {
    /// Creates an empty queue with the given configuration.
    pub fn new(config: EventQueueConfig) -> Self {
        Self {
            config,
            buffer: VecDeque::with_capacity(config.capacity.min(256)),
            dropped: 0,
            coalesced_resizes: 0,
        }
    }

    /// Offers `event` to the queue.
    pub fn push(&mut self, event: AppEvent) -> EventPushOutcome {
        if event.is_resize() && self.config.resize_policy == ResizePolicy::Coalesce {
            if let Some(index) = self.buffer.iter().rposition(AppEvent::is_resize) {
                self.buffer[index] = event;
                self.coalesced_resizes += 1;
                return EventPushOutcome::Coalesced;
            }
        }

        if self.buffer.len() >= self.config.capacity {
            self.dropped += 1;
            return EventPushOutcome::Dropped;
        }

        self.buffer.push_back(event);
        EventPushOutcome::Enqueued
    }

    /// Removes and returns the oldest event.
    pub fn pop(&mut self) -> Option<AppEvent> {
        self.buffer.pop_front()
    }

    /// Number of queued events.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// `true` when nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Events discarded because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Resizes merged into an already queued resize.
    pub fn coalesced_resizes(&self) -> u64 {
        self.coalesced_resizes
    }
}
