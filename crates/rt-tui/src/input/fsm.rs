//! Finite State Machine (FSM) defining active UI interaction modes.

use crate::theme::ThemeId;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InteractionMode {
    #[default]
    Normal,
    VisualSelect,
    Filter {
        query: String,
    },
    CommandPalette {
        query: String,
        selected_idx: usize,
    },
    ConfirmRemoveModal {
        delete_files: bool,
        focus_confirm: bool,
    },
    AddTorrentModal {
        source_input: String,
        source_cursor: usize,
        dir_input: String,
        dir_cursor: usize,
        start_immediately: bool,
        sequential: bool,
        focused_field: usize,
    },
    ThemePickerModal {
        current_theme_id: ThemeId,
        selected_theme_id: ThemeId,
    },
    ShortcutsHelpModal,
    InspectorFocus {
        selected_sub_idx: usize,
    },
}

impl InteractionMode {
    pub fn is_modal(&self) -> bool {
        matches!(
            self,
            Self::CommandPalette { .. }
                | Self::ConfirmRemoveModal { .. }
                | Self::AddTorrentModal { .. }
                | Self::ThemePickerModal { .. }
                | Self::ShortcutsHelpModal
        )
    }

    pub fn is_filtering(&self) -> bool {
        matches!(self, Self::Filter { .. })
    }
}
