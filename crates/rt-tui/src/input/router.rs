//! Comprehensive keybinding router mapping Vim and standard cursor keys to AppAction.

use crate::input::fsm::InteractionMode;
use crate::state::ActiveView;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    // Navigation
    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,
    CursorTop,
    CursorBottom,
    PageUp,
    PageDown,

    // Selection
    SelectRow(usize),
    ToggleSelectRow(usize),
    SelectRange(usize),
    ToggleSelect,
    ToggleSelectOnly,
    SelectAll,
    InvertSelection,
    ClearSelection,

    // Torrent actions
    StartTorrent,
    PauseTorrent,
    OpenRemoveModal,
    PriorityUp,
    PriorityDown,
    RecheckHash,
    ToggleSequential,

    // Bandwidth
    DownloadThrottleUp,
    DownloadThrottleDown,
    UploadThrottleUp,
    UploadThrottleDown,
    ToggleUncapped,

    // Views
    SwitchView(ActiveView),
    CycleViewNext,
    CycleViewPrev,

    // Inspector
    ToggleInspector,
    CycleInspectorTabNext,
    CycleInspectorTabPrev,
    SwitchInspectorTab(crate::widgets::InspectorTab),
    FocusInspector,
    FocusInspectorWithItem(usize),
    UnfocusInspector,

    // Modals
    OpenCommandPalette,
    OpenAddModal,
    OpenThemePicker,
    OpenHelpModal,
    CloseModal,
    SubmitModal,

    // Filter & Text Input
    OpenFilter,
    CharInput(char),
    PasteInput(String),
    BackspaceInput,
    DeleteInput,
    CommitFilter,
    CancelFilter,

    // Session
    Quit,
    None,
}

pub struct KeyRouter;

impl KeyRouter {
    pub fn route_key(event: KeyEvent, mode: &InteractionMode) -> AppAction {
        match mode {
            InteractionMode::Normal | InteractionMode::VisualSelect => {
                Self::route_normal_or_visual(event, mode)
            }
            InteractionMode::Filter { .. } => Self::route_filter(event),
            InteractionMode::CommandPalette { .. } => Self::route_command_palette(event),
            InteractionMode::ConfirmRemoveModal { .. } => Self::route_confirm_remove(event),
            InteractionMode::AddTorrentModal { focused_field, .. } => {
                Self::route_add_torrent(event, *focused_field)
            }
            InteractionMode::ThemePickerModal { .. } => Self::route_theme_picker(event),
            InteractionMode::ShortcutsHelpModal => Self::route_help_modal(event),
            InteractionMode::InspectorFocus { .. } => Self::route_inspector(event),
        }
    }

    fn route_normal_or_visual(event: KeyEvent, mode: &InteractionMode) -> AppAction {
        let is_visual = matches!(mode, InteractionMode::VisualSelect);

        match (event.code, event.modifiers) {
            // Global exits
            (KeyCode::Char('q'), KeyModifiers::NONE)
            | (KeyCode::Char('q'), KeyModifiers::CONTROL) => AppAction::Quit,
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if is_visual {
                    AppAction::ClearSelection
                } else {
                    AppAction::Quit
                }
            }

            // Command Palette / Help / Theme
            (KeyCode::Char(':'), KeyModifiers::NONE)
            | (KeyCode::Char('p'), KeyModifiers::CONTROL) => AppAction::OpenCommandPalette,
            (KeyCode::Char('?'), KeyModifiers::NONE) | (KeyCode::F(1), KeyModifiers::NONE) => {
                AppAction::OpenHelpModal
            }
            (KeyCode::Char('t'), KeyModifiers::NONE) | (KeyCode::Char('S'), KeyModifiers::NONE) => {
                AppAction::OpenThemePicker
            }

            // Filter
            (KeyCode::Char('/'), KeyModifiers::NONE) | (KeyCode::Char('f'), KeyModifiers::NONE) => {
                AppAction::OpenFilter
            }

            // Views: 1 .. 8
            (KeyCode::Char('1'), KeyModifiers::NONE) => AppAction::SwitchView(ActiveView::All),
            (KeyCode::Char('2'), KeyModifiers::NONE) => {
                AppAction::SwitchView(ActiveView::Downloading)
            }
            (KeyCode::Char('3'), KeyModifiers::NONE) => AppAction::SwitchView(ActiveView::Seeding),
            (KeyCode::Char('4'), KeyModifiers::NONE) => AppAction::SwitchView(ActiveView::Active),
            (KeyCode::Char('5'), KeyModifiers::NONE) => AppAction::SwitchView(ActiveView::Paused),
            (KeyCode::Char('6'), KeyModifiers::NONE) => AppAction::SwitchView(ActiveView::Complete),
            (KeyCode::Char('7'), KeyModifiers::NONE) => AppAction::SwitchView(ActiveView::Error),
            (KeyCode::Char('8'), KeyModifiers::NONE) => AppAction::SwitchView(ActiveView::Hashing),

            // Navigation: Vim & Cursor
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
                AppAction::CursorDown
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, KeyModifiers::NONE) => {
                AppAction::CursorUp
            }
            (KeyCode::Char('g'), KeyModifiers::NONE) | (KeyCode::Home, KeyModifiers::NONE) => {
                AppAction::CursorTop
            }
            (KeyCode::Char('G'), KeyModifiers::NONE) | (KeyCode::End, KeyModifiers::NONE) => {
                AppAction::CursorBottom
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL)
            | (KeyCode::PageDown, KeyModifiers::NONE) => AppAction::PageDown,
            (KeyCode::Char('u'), KeyModifiers::CONTROL) | (KeyCode::PageUp, KeyModifiers::NONE) => {
                AppAction::PageUp
            }

            // Selection
            (KeyCode::Char(' '), KeyModifiers::NONE) => AppAction::ToggleSelect,
            (KeyCode::Char('v'), KeyModifiers::NONE) | (KeyCode::Char('V'), KeyModifiers::NONE) => {
                AppAction::ToggleSelectOnly
            }
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => AppAction::SelectAll,
            (KeyCode::Char('*'), KeyModifiers::NONE) => AppAction::InvertSelection,
            (KeyCode::Esc, KeyModifiers::NONE) => {
                if is_visual {
                    AppAction::ClearSelection
                } else {
                    AppAction::None
                }
            }

            // Torrent actions
            (KeyCode::Char('s'), KeyModifiers::NONE) => AppAction::StartTorrent,
            (KeyCode::Char('p'), KeyModifiers::NONE) => AppAction::PauseTorrent,
            (KeyCode::Char('x'), KeyModifiers::NONE) | (KeyCode::Delete, KeyModifiers::NONE) => {
                AppAction::OpenRemoveModal
            }
            (KeyCode::Char('+'), KeyModifiers::NONE) | (KeyCode::Char('='), KeyModifiers::NONE) => {
                AppAction::PriorityUp
            }
            (KeyCode::Char('-'), KeyModifiers::NONE) | (KeyCode::Char('_'), KeyModifiers::NONE) => {
                AppAction::PriorityDown
            }
            (KeyCode::Char('r'), KeyModifiers::NONE) => AppAction::RecheckHash,
            (KeyCode::Char('a'), KeyModifiers::NONE)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL) => AppAction::OpenAddModal,
            (KeyCode::Char('m'), KeyModifiers::NONE) => AppAction::ToggleSequential,

            // Bandwidth throttling
            (KeyCode::Char(']'), KeyModifiers::NONE) => AppAction::DownloadThrottleUp,
            (KeyCode::Char('['), KeyModifiers::NONE) => AppAction::DownloadThrottleDown,
            (KeyCode::Char('}'), KeyModifiers::NONE) => AppAction::UploadThrottleUp,
            (KeyCode::Char('{'), KeyModifiers::NONE) => AppAction::UploadThrottleDown,
            (KeyCode::Char('0'), KeyModifiers::NONE) => AppAction::ToggleUncapped,

            // Tab / View navigation
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, KeyModifiers::NONE) => {
                AppAction::CycleViewPrev
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, KeyModifiers::NONE) => {
                AppAction::CycleViewNext
            }
            (KeyCode::Tab, KeyModifiers::NONE) => AppAction::FocusInspector,

            // Inspector drawer toggle
            (KeyCode::Char('i'), KeyModifiers::NONE) | (KeyCode::Enter, KeyModifiers::NONE) => {
                AppAction::ToggleInspector
            }

            _ => AppAction::None,
        }
    }

    fn route_filter(event: KeyEvent) -> AppAction {
        match (event.code, event.modifiers) {
            (KeyCode::Esc, _) => AppAction::CancelFilter,
            (KeyCode::Enter, _) => AppAction::CommitFilter,
            (KeyCode::Backspace, _) => AppAction::BackspaceInput,
            (KeyCode::Char('v'), KeyModifiers::CONTROL)
            | (KeyCode::Insert, KeyModifiers::SHIFT) => AppAction::PasteInput(String::new()),
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => AppAction::CharInput(c),
            _ => AppAction::None,
        }
    }

    fn route_command_palette(event: KeyEvent) -> AppAction {
        match (event.code, event.modifiers) {
            (KeyCode::Esc, _) => AppAction::CloseModal,
            (KeyCode::Enter, _) => AppAction::SubmitModal,
            (KeyCode::Up, _) | (KeyCode::BackTab, _) => AppAction::CursorUp,
            (KeyCode::Down, _) | (KeyCode::Tab, _) => AppAction::CursorDown,
            (KeyCode::Backspace, _) => AppAction::BackspaceInput,
            (KeyCode::Char('v'), KeyModifiers::CONTROL)
            | (KeyCode::Insert, KeyModifiers::SHIFT) => AppAction::PasteInput(String::new()),
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => AppAction::CharInput(c),
            _ => AppAction::None,
        }
    }

    fn route_confirm_remove(event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => AppAction::CloseModal,
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => AppAction::SubmitModal,
            KeyCode::Down | KeyCode::Char('j') => AppAction::CursorDown,
            KeyCode::Up | KeyCode::Char('k') => AppAction::CursorUp,
            KeyCode::Char('d') | KeyCode::Char('D') => AppAction::CursorDown, // 'd' for delete
            KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Char('r') | KeyCode::Char('R') => {
                AppAction::CursorUp // 's' for session only
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char(' ') => AppAction::ToggleSelect, // toggle delete data
            KeyCode::Left | KeyCode::Right => AppAction::ToggleSelectOnly, // toggle button focus
            _ => AppAction::None,
        }
    }

    fn route_add_torrent(event: KeyEvent, focused_field: usize) -> AppAction {
        match (event.code, event.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                AppAction::CloseModal
            }
            (KeyCode::Enter, _) => AppAction::SubmitModal,
            (KeyCode::Tab, _) | (KeyCode::Down, KeyModifiers::NONE) => AppAction::CursorDown, // Next field
            (KeyCode::BackTab, _) | (KeyCode::Up, KeyModifiers::NONE) => AppAction::CursorUp, // Prev field
            (KeyCode::Left, KeyModifiers::NONE) => AppAction::CursorLeft,
            (KeyCode::Right, KeyModifiers::NONE) => AppAction::CursorRight,
            (KeyCode::Home, KeyModifiers::NONE) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                AppAction::CursorHome
            }
            (KeyCode::End, KeyModifiers::NONE) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                AppAction::CursorEnd
            }
            (KeyCode::Delete, KeyModifiers::NONE) => AppAction::DeleteInput,
            (KeyCode::Backspace, KeyModifiers::NONE) => AppAction::BackspaceInput,
            (KeyCode::Char('v'), KeyModifiers::CONTROL)
            | (KeyCode::Insert, KeyModifiers::SHIFT) => AppAction::PasteInput(String::new()),
            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                if focused_field == 0 || focused_field == 1 {
                    AppAction::CharInput(' ')
                } else {
                    AppAction::ToggleSelect
                }
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => AppAction::CharInput(c),
            _ => AppAction::None,
        }
    }

    fn route_theme_picker(event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Esc => AppAction::CloseModal,
            KeyCode::Enter => AppAction::SubmitModal,
            KeyCode::Up | KeyCode::Char('k') => AppAction::CursorUp,
            KeyCode::Down | KeyCode::Char('j') => AppAction::CursorDown,
            _ => AppAction::None,
        }
    }

    fn route_help_modal(event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Enter => {
                AppAction::CloseModal
            }
            _ => AppAction::None,
        }
    }

    fn route_inspector(event: KeyEvent) -> AppAction {
        match (event.code, event.modifiers) {
            (KeyCode::Esc, _)
            | (KeyCode::Char('i'), KeyModifiers::NONE)
            | (KeyCode::Char('q'), KeyModifiers::NONE) => AppAction::UnfocusInspector,

            // Direct jump to inspector tabs 1..6
            (KeyCode::Char('1'), KeyModifiers::NONE) => {
                AppAction::SwitchInspectorTab(crate::widgets::InspectorTab::General)
            }
            (KeyCode::Char('2'), KeyModifiers::NONE) => {
                AppAction::SwitchInspectorTab(crate::widgets::InspectorTab::Trackers)
            }
            (KeyCode::Char('3'), KeyModifiers::NONE) => {
                AppAction::SwitchInspectorTab(crate::widgets::InspectorTab::Peers)
            }
            (KeyCode::Char('4'), KeyModifiers::NONE) => {
                AppAction::SwitchInspectorTab(crate::widgets::InspectorTab::Files)
            }
            (KeyCode::Char('5'), KeyModifiers::NONE) => {
                AppAction::SwitchInspectorTab(crate::widgets::InspectorTab::PieceMap)
            }
            (KeyCode::Char('6'), KeyModifiers::NONE) => {
                AppAction::SwitchInspectorTab(crate::widgets::InspectorTab::Logs)
            }

            // Tab cycling
            (KeyCode::Left, KeyModifiers::NONE)
            | (KeyCode::Char('h'), KeyModifiers::NONE)
            | (KeyCode::BackTab, _) => AppAction::CycleInspectorTabPrev,

            (KeyCode::Right, KeyModifiers::NONE)
            | (KeyCode::Char('l'), KeyModifiers::NONE)
            | (KeyCode::Tab, KeyModifiers::NONE) => AppAction::CycleInspectorTabNext,

            // Item navigation inside active tab
            (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                AppAction::CursorUp
            }
            (KeyCode::Down, KeyModifiers::NONE) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                AppAction::CursorDown
            }

            // Toggle priority on file in Files tab
            (KeyCode::Char(' '), KeyModifiers::NONE) => AppAction::PriorityUp,
            (KeyCode::Enter, KeyModifiers::NONE) => AppAction::UnfocusInspector,

            _ => AppAction::None,
        }
    }
}
