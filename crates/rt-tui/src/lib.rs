//! Crate `rt-tui`: Terminal User Interface for rstorrent

pub mod app;
pub mod concurrency;
pub mod display;
pub mod input;
pub mod state;
pub mod theme;
pub mod ui;
pub mod widgets;
pub mod windows;

pub use app::{EngineCommand, NextGenTuiApp};
pub use concurrency::{
    sample_engine_snapshot, sample_engine_snapshot_with_stats, EngineSnapshotBroadcaster,
    UiComputeEngine,
};
pub use state::{
    ActiveView, EngineSnapshot, FileUiRecord, PeerUiRecord, PieceState, SessionStats, SortKey,
    SortOrder, TorrentDetailSnapshot, TorrentUiRecord, TorrentUiState, TrackerUiRecord,
};
pub use theme::{Theme, ThemeId};
pub use widgets::*;

pub use display::{
    canvas::Canvas,
    color_map::{ColorKind, ColorMap},
    frame::{Bounds, Frame, FrameNode},
    terminal::{TerminalBackend, TerminalGuard},
    text_element::{
        TextElement, TextElementCommand, TextElementList, TextElementString, TextElementValue,
        FLAG_DATE, FLAG_ELAPSED, FLAG_ESCAPE_HEX, FLAG_ESCAPE_HTML, FLAG_FIXED_WIDTH, FLAG_KB,
        FLAG_MB, FLAG_NORMAL, FLAG_REMAINING, FLAG_TIME, FLAG_TIMER, FLAG_XB,
    },
    utils::*,
    window::{
        BaseWindow, Window, EXTENT_FULL, EXTENT_STATIC, FLAG_ACTIVE, FLAG_BOTTOM, FLAG_FOCUSED,
        FLAG_LEFT, FLAG_OFFSCREEN,
    },
};

pub use input::{
    Bindings, InputManager, InputTarget, PathInput, TextInput, KEY_BACKSPACE, KEY_DC, KEY_END,
    KEY_HOME, KEY_LEFT, KEY_RIGHT,
};

pub use ui::{
    create_filter_command, create_filter_pattern, ActiveScreen, DownloadListUI, InputPromptMode,
    RootUI,
};
pub use windows::{
    DetailTab, WindowDownloadDetails, WindowDownloadList, WindowInput, WindowLog, WindowStatusbar,
    WindowTitle,
};
