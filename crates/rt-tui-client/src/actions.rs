//! User intents produced by key presses.
//!
//! Actions are plain data: the key handler decides *what* the user asked for and
//! [`crate::app::App`] executes it against the bridge. Keeping them apart makes
//! every shortcut testable without a daemon.

/// Torrent lifecycle action understood by `TorrentService.ControlTorrent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentAction {
    /// Start the torrent.
    Start,
    /// Stop the torrent.
    Stop,
    /// Close the torrent without removing its data.
    Close,
    /// Re-check the downloaded data.
    CheckHash,
}

/// Prompt the client can open; TASK-11 renders them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// `Backspace` — load a `.torrent` file or magnet link.
    Load,
    /// `^X` — free-form command prompt.
    Command,
    /// `/` or `f` — filter torrents by name.
    Filter,
}

/// One user intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiAction {
    /// Does nothing; used for menu entries without behaviour.
    Noop,
    /// Start/stop/close/check a torrent.
    ControlTorrent {
        /// Info-hash of the target torrent.
        hash: String,
        /// What to do with it.
        action: TorrentAction,
    },
    /// Remove a torrent from the session.
    RemoveTorrent {
        /// Info-hash of the target torrent.
        hash: String,
    },
    /// Change a torrent's priority (`0` off … `3` high).
    SetPriority {
        /// Info-hash of the target torrent.
        hash: String,
        /// New priority.
        priority: u32,
    },
    /// Run an rTorrent command, optionally scoped to one torrent.
    Execute {
        /// rTorrent command name, e.g. `d.ignore_commands.set`.
        command: String,
        /// Integer arguments.
        args: Vec<i64>,
        /// Torrent the command applies to, if any.
        hash: Option<String>,
    },
    /// Load a `.torrent` file or magnet link.
    Load {
        /// Path or URI to load.
        path: String,
        /// Whether to start the torrent right away.
        start: bool,
    },
    /// Open an input prompt.
    OpenPrompt(PromptKind),
    /// Several actions that must run in order.
    Batch(Vec<TuiAction>),
}
