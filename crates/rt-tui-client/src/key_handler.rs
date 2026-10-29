//! Download-list shortcut map.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::actions::{PromptKind, TorrentAction, TuiAction};
use crate::state::TorrentViewModel;

/// rTorrent command that flips a torrent's ignore flag.
pub const IGNORE_COMMANDS: &str = "d.ignore_commands.set";

/// rTorrent command that resets the completed flag before a re-check.
pub const COMPLETE_SET: &str = "d.complete.set";

/// Highest torrent priority understood by the daemon.
pub const MAX_PRIORITY: u32 = 3;

/// Next priority in the `0 → 1 → 2 → 3 → 0` cycle.
pub fn next_priority(priority: u32) -> u32 {
    (priority + 1) % (MAX_PRIORITY + 1)
}

/// Previous priority in the `0 → 3 → 2 → 1 → 0` cycle.
pub fn previous_priority(priority: u32) -> u32 {
    (priority + MAX_PRIORITY) % (MAX_PRIORITY + 1)
}

/// Keys that act on the client rather than on one torrent.
pub fn global_action(code: KeyCode, modifiers: KeyModifiers) -> Option<TuiAction> {
    match (code, modifiers) {
        (KeyCode::Backspace, _) => Some(TuiAction::OpenPrompt(PromptKind::Load)),
        (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
            Some(TuiAction::OpenPrompt(PromptKind::Command))
        }
        (KeyCode::Char('/'), KeyModifiers::NONE)
        | (KeyCode::Char('f'), KeyModifiers::NONE)
        | (KeyCode::Char('F'), KeyModifiers::NONE) => {
            Some(TuiAction::OpenPrompt(PromptKind::Filter))
        }
        _ => None,
    }
}

/// Maps a key press onto a torrent action.
///
/// `ignored` is the client's local view of `d.ignore_commands`, because the
/// daemon does not report it in `TorrentSummary`.
pub fn torrent_action(
    code: KeyCode,
    modifiers: KeyModifiers,
    torrent: &TorrentViewModel,
    ignored: bool,
) -> Option<TuiAction> {
    let hash = torrent.hash.clone();

    match (code, modifiers) {
        (KeyCode::Char('s'), KeyModifiers::CONTROL) => Some(TuiAction::ControlTorrent {
            hash,
            action: TorrentAction::Start,
        }),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            if torrent.is_active {
                Some(TuiAction::ControlTorrent {
                    hash,
                    action: TorrentAction::Stop,
                })
            } else {
                Some(TuiAction::RemoveTorrent { hash })
            }
        }
        (KeyCode::Char('k'), KeyModifiers::CONTROL) => Some(TuiAction::Batch(vec![
            TuiAction::Execute {
                command: IGNORE_COMMANDS.to_string(),
                args: vec![1],
                hash: Some(hash.clone()),
            },
            TuiAction::ControlTorrent {
                hash: hash.clone(),
                action: TorrentAction::Stop,
            },
            TuiAction::ControlTorrent {
                hash,
                action: TorrentAction::Close,
            },
        ])),
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => Some(TuiAction::Batch(vec![
            TuiAction::Execute {
                command: COMPLETE_SET.to_string(),
                args: vec![0],
                hash: Some(hash.clone()),
            },
            TuiAction::ControlTorrent {
                hash,
                action: TorrentAction::CheckHash,
            },
        ])),
        (KeyCode::Char('+'), _) => Some(TuiAction::SetPriority {
            hash,
            priority: next_priority(torrent.priority),
        }),
        (KeyCode::Char('-'), _) => Some(TuiAction::SetPriority {
            hash,
            priority: previous_priority(torrent.priority),
        }),
        (KeyCode::Char('I'), _) => Some(TuiAction::Execute {
            command: IGNORE_COMMANDS.to_string(),
            args: vec![if ignored { 0 } else { 1 }],
            hash: Some(hash),
        }),
        _ => None,
    }
}
