//! Unified Next-Gen Ratatui UI Application State and Drawing Engine.

use crate::concurrency::compute::UiComputeEngine;
use crate::input::fsm::InteractionMode;
use crate::input::router::AppAction;
use crate::state::{
    ActiveView, EngineSnapshot, SortKey, SortOrder, TorrentDetailSnapshot, TorrentUiRecord,
};
use crate::theme::{Theme, ThemeId};
use crate::ui::layout::calculate_responsive_layout_dynamic;
use crate::widgets::inspector::InspectorTab;
use crate::widgets::modals::*;
use crate::widgets::sidebar::ViewsSidebar;
use crate::widgets::sparkline::RateHistory;
use crate::widgets::status_bar::{FooterMode, StatusBarWidget};
use crate::widgets::torrent_table::TorrentTableWidget;
use crate::widgets::{DashboardHeader, InspectorDrawerWidget, ViewTabsWidget};
use ratatui::widgets::Widget;
use ratatui::Frame;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

/// Actions directed to the BitTorrent engine from the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineCommand {
    StartTorrent(Vec<[u8; 20]>),
    PauseTorrent(Vec<[u8; 20]>),
    RemoveTorrent {
        hashes: Vec<[u8; 20]>,
        delete_data: bool,
    },
    RecheckHash(Vec<[u8; 20]>),
    AdjustDownloadThrottle(i64),
    AdjustUploadThrottle(i64),
    ToggleUncappedThrottle,
    AddTorrent {
        source: String,
        destination: String,
        start: bool,
        sequential: bool,
    },
    ToggleSequential([u8; 20]),
    Quit,
}

pub struct NextGenTuiApp {
    pub theme: Theme,
    pub active_view: ActiveView,
    pub sort_key: SortKey,
    pub sort_order: SortOrder,
    pub selected_row: usize,
    pub multi_selected: HashSet<[u8; 20]>,
    pub mode: InteractionMode,
    pub show_inspector: bool,
    pub active_inspector_tab: InspectorTab,
    pub inspector_sub_idx: usize,
    pub snapshot: Arc<EngineSnapshot>,
    pub visible_indices: Vec<usize>,
    pub category_counts: [usize; 8],
    pub down_history: RateHistory<60>,
    pub up_history: RateHistory<60>,
    pub status_message: Option<(String, Instant)>,
    pub endpoint_label: String,
    pub active_torrent_details: Option<TorrentDetailSnapshot>,
}

impl Default for NextGenTuiApp {
    fn default() -> Self {
        Self::new("local".into())
    }
}

impl NextGenTuiApp {
    pub fn new(endpoint_label: String) -> Self {
        let initial_snapshot = Arc::new(EngineSnapshot::default());
        Self {
            theme: Theme::by_id(ThemeId::TokyoNight),
            active_view: ActiveView::All,
            sort_key: SortKey::DownRate,
            sort_order: SortOrder::Descending,
            selected_row: 0,
            multi_selected: HashSet::new(),
            mode: InteractionMode::Normal,
            show_inspector: false,
            active_inspector_tab: InspectorTab::General,
            inspector_sub_idx: 0,
            snapshot: initial_snapshot,
            visible_indices: Vec::new(),
            category_counts: [0; 8],
            down_history: RateHistory::new(),
            up_history: RateHistory::new(),
            status_message: None,
            endpoint_label,
            active_torrent_details: None,
        }
    }

    /// Sets an ephemeral status message displayed on the footer.
    pub fn set_status_message(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), Instant::now()));
    }

    /// Ingests a new engine snapshot and computes visible indices with Rayon.
    pub fn update_snapshot(&mut self, snapshot: Arc<EngineSnapshot>) {
        self.down_history.push(snapshot.stats.global_down_bps);
        self.up_history.push(snapshot.stats.global_up_bps);

        let filter_query = match &self.mode {
            InteractionMode::Filter { query } => Some(query.as_str()),
            _ => None,
        };

        // Parallel filter and sort across all torrents
        self.visible_indices = UiComputeEngine::filter_and_sort(
            &snapshot.torrents,
            self.active_view,
            filter_query,
            self.sort_key,
            self.sort_order,
        );

        // Parallel category count for badges
        self.category_counts = UiComputeEngine::count_categories(&snapshot.torrents);

        // Clamp selection cursor
        if !self.visible_indices.is_empty() && self.selected_row >= self.visible_indices.len() {
            self.selected_row = self.visible_indices.len() - 1;
        }

        self.snapshot = snapshot;
    }

    /// Gets currently focused torrent record if one exists.
    pub fn focused_torrent(&self) -> Option<&TorrentUiRecord> {
        if self.visible_indices.is_empty() || self.selected_row >= self.visible_indices.len() {
            None
        } else {
            let actual_idx = self.visible_indices[self.selected_row];
            self.snapshot.torrents.get(actual_idx)
        }
    }

    /// Collects hashes for action: multi-selected set if non-empty, otherwise currently focused item.
    pub fn target_hashes(&self) -> Vec<[u8; 20]> {
        if !self.multi_selected.is_empty() {
            self.multi_selected.iter().copied().collect()
        } else if let Some(focused) = self.focused_torrent() {
            vec![focused.hash]
        } else {
            Vec::new()
        }
    }

    /// Dispatches an action generated by keyboard or mouse router.
    pub fn dispatch_action(&mut self, action: AppAction) -> Option<EngineCommand> {
        match action {
            AppAction::Quit => Some(EngineCommand::Quit),

            // Navigation
            AppAction::CursorDown => {
                if let InteractionMode::ConfirmRemoveModal {
                    ref mut delete_files,
                    ..
                } = self.mode
                {
                    *delete_files = true;
                } else if let InteractionMode::InspectorFocus {
                    ref mut selected_sub_idx,
                } = self.mode
                {
                    *selected_sub_idx = selected_sub_idx.saturating_add(1);
                    self.inspector_sub_idx = *selected_sub_idx;
                } else if let InteractionMode::CommandPalette {
                    ref mut selected_idx,
                    ..
                } = self.mode
                {
                    *selected_idx = selected_idx.saturating_add(1);
                } else if let InteractionMode::ThemePickerModal {
                    ref mut selected_theme_id,
                    ..
                } = self.mode
                {
                    *selected_theme_id = selected_theme_id.next();
                } else if let InteractionMode::AddTorrentModal {
                    ref mut focused_field,
                    ..
                } = self.mode
                {
                    *focused_field = (*focused_field + 1) % 6;
                } else if !self.visible_indices.is_empty()
                    && self.selected_row + 1 < self.visible_indices.len()
                {
                    self.selected_row += 1;
                }
                None
            }

            AppAction::CursorUp => {
                if let InteractionMode::ConfirmRemoveModal {
                    ref mut delete_files,
                    ..
                } = self.mode
                {
                    *delete_files = false;
                } else if let InteractionMode::InspectorFocus {
                    ref mut selected_sub_idx,
                } = self.mode
                {
                    *selected_sub_idx = selected_sub_idx.saturating_sub(1);
                    self.inspector_sub_idx = *selected_sub_idx;
                } else if let InteractionMode::CommandPalette {
                    ref mut selected_idx,
                    ..
                } = self.mode
                {
                    *selected_idx = selected_idx.saturating_sub(1);
                } else if let InteractionMode::ThemePickerModal {
                    ref mut selected_theme_id,
                    ..
                } = self.mode
                {
                    *selected_theme_id = selected_theme_id.prev();
                } else if let InteractionMode::AddTorrentModal {
                    ref mut focused_field,
                    ..
                } = self.mode
                {
                    *focused_field = if *focused_field == 0 {
                        5
                    } else {
                        *focused_field - 1
                    };
                } else if self.selected_row > 0 {
                    self.selected_row -= 1;
                }
                None
            }

            AppAction::CursorLeft => {
                if let InteractionMode::AddTorrentModal {
                    ref mut source_cursor,
                    ref mut dir_cursor,
                    ref mut focused_field,
                    ..
                } = self.mode
                {
                    if *focused_field == 0 {
                        *source_cursor = source_cursor.saturating_sub(1);
                    } else if *focused_field == 1 {
                        *dir_cursor = dir_cursor.saturating_sub(1);
                    } else if *focused_field == 5 {
                        *focused_field = 4;
                    }
                }
                None
            }

            AppAction::CursorRight => {
                if let InteractionMode::AddTorrentModal {
                    ref source_input,
                    ref mut source_cursor,
                    ref dir_input,
                    ref mut dir_cursor,
                    ref mut focused_field,
                    ..
                } = self.mode
                {
                    if *focused_field == 0 {
                        *source_cursor = (*source_cursor + 1).min(source_input.chars().count());
                    } else if *focused_field == 1 {
                        *dir_cursor = (*dir_cursor + 1).min(dir_input.chars().count());
                    } else if *focused_field == 4 {
                        *focused_field = 5;
                    }
                }
                None
            }

            AppAction::CursorHome => {
                if let InteractionMode::AddTorrentModal {
                    ref mut source_cursor,
                    ref mut dir_cursor,
                    ref focused_field,
                    ..
                } = self.mode
                {
                    if *focused_field == 0 {
                        *source_cursor = 0;
                    } else if *focused_field == 1 {
                        *dir_cursor = 0;
                    }
                }
                None
            }

            AppAction::CursorEnd => {
                if let InteractionMode::AddTorrentModal {
                    ref source_input,
                    ref mut source_cursor,
                    ref dir_input,
                    ref mut dir_cursor,
                    ref focused_field,
                    ..
                } = self.mode
                {
                    if *focused_field == 0 {
                        *source_cursor = source_input.chars().count();
                    } else if *focused_field == 1 {
                        *dir_cursor = dir_input.chars().count();
                    }
                }
                None
            }

            AppAction::CursorTop => {
                self.selected_row = 0;
                None
            }

            AppAction::CursorBottom => {
                if !self.visible_indices.is_empty() {
                    self.selected_row = self.visible_indices.len() - 1;
                }
                None
            }

            AppAction::PageDown => {
                if !self.visible_indices.is_empty() {
                    self.selected_row =
                        (self.selected_row + 10).min(self.visible_indices.len() - 1);
                }
                None
            }

            AppAction::PageUp => {
                self.selected_row = self.selected_row.saturating_sub(10);
                None
            }

            // Selection
            AppAction::ToggleSelect => {
                match &mut self.mode {
                    InteractionMode::ConfirmRemoveModal { delete_files, .. } => {
                        *delete_files = !*delete_files;
                    }
                    InteractionMode::AddTorrentModal {
                        start_immediately,
                        sequential,
                        focused_field,
                        ..
                    } => {
                        if *focused_field == 2 {
                            *start_immediately = !*start_immediately;
                        } else if *focused_field == 3 {
                            *sequential = !*sequential;
                        } else if *focused_field == 4 {
                            self.mode = InteractionMode::Normal;
                        } else if *focused_field == 5 {
                            return self.dispatch_action(AppAction::SubmitModal);
                        }
                    }
                    _ => {
                        if let Some(t) = self.focused_torrent() {
                            let hash = t.hash;
                            if self.multi_selected.contains(&hash) {
                                self.multi_selected.remove(&hash);
                            } else {
                                self.multi_selected.insert(hash);
                            }
                            self.mode = if self.multi_selected.is_empty() {
                                InteractionMode::Normal
                            } else {
                                InteractionMode::VisualSelect
                            };
                            // Advance cursor down 1 row
                            if self.selected_row + 1 < self.visible_indices.len() {
                                self.selected_row += 1;
                            }
                        }
                    }
                }
                None
            }

            AppAction::ToggleSelectOnly => {
                if let InteractionMode::ConfirmRemoveModal {
                    ref mut focus_confirm,
                    ..
                } = self.mode
                {
                    *focus_confirm = !*focus_confirm;
                } else if let Some(t) = self.focused_torrent() {
                    let hash = t.hash;
                    if self.multi_selected.contains(&hash) {
                        self.multi_selected.remove(&hash);
                    } else {
                        self.multi_selected.insert(hash);
                    }
                    self.mode = if self.multi_selected.is_empty() {
                        InteractionMode::Normal
                    } else {
                        InteractionMode::VisualSelect
                    };
                }
                None
            }

            AppAction::SelectAll => {
                for &idx in &self.visible_indices {
                    if let Some(t) = self.snapshot.torrents.get(idx) {
                        self.multi_selected.insert(t.hash);
                    }
                }
                self.mode = InteractionMode::VisualSelect;
                None
            }

            AppAction::InvertSelection => {
                for &idx in &self.visible_indices {
                    if let Some(t) = self.snapshot.torrents.get(idx) {
                        if self.multi_selected.contains(&t.hash) {
                            self.multi_selected.remove(&t.hash);
                        } else {
                            self.multi_selected.insert(t.hash);
                        }
                    }
                }
                self.mode = if self.multi_selected.is_empty() {
                    InteractionMode::Normal
                } else {
                    InteractionMode::VisualSelect
                };
                None
            }

            AppAction::ClearSelection => {
                self.multi_selected.clear();
                self.mode = InteractionMode::Normal;
                None
            }

            // Torrent controls
            AppAction::StartTorrent => {
                let hashes = self.target_hashes();
                if hashes.is_empty() {
                    None
                } else {
                    Some(EngineCommand::StartTorrent(hashes))
                }
            }

            AppAction::PauseTorrent => {
                let hashes = self.target_hashes();
                if hashes.is_empty() {
                    None
                } else {
                    Some(EngineCommand::PauseTorrent(hashes))
                }
            }

            AppAction::RecheckHash => {
                let hashes = self.target_hashes();
                if hashes.is_empty() {
                    None
                } else {
                    Some(EngineCommand::RecheckHash(hashes))
                }
            }

            AppAction::ToggleSequential => self
                .focused_torrent()
                .map(|t| EngineCommand::ToggleSequential(t.hash)),

            // Bandwidth
            AppAction::DownloadThrottleUp => Some(EngineCommand::AdjustDownloadThrottle(50 * 1024)),
            AppAction::DownloadThrottleDown => {
                Some(EngineCommand::AdjustDownloadThrottle(-50 * 1024))
            }
            AppAction::UploadThrottleUp => Some(EngineCommand::AdjustUploadThrottle(50 * 1024)),
            AppAction::UploadThrottleDown => Some(EngineCommand::AdjustUploadThrottle(-50 * 1024)),
            AppAction::ToggleUncapped => Some(EngineCommand::ToggleUncappedThrottle),

            // View switching
            AppAction::SwitchView(view) => {
                self.active_view = view;
                self.selected_row = 0;
                self.mode = InteractionMode::Normal;
                // Recompute visible indices with Rayon
                self.visible_indices = UiComputeEngine::filter_and_sort(
                    &self.snapshot.torrents,
                    self.active_view,
                    None,
                    self.sort_key,
                    self.sort_order,
                );
                None
            }

            AppAction::CycleViewNext => {
                let next_view = self.active_view.next();
                self.dispatch_action(AppAction::SwitchView(next_view))
            }

            AppAction::CycleViewPrev => {
                let prev_view = self.active_view.prev();
                self.dispatch_action(AppAction::SwitchView(prev_view))
            }

            // Inspector Drawer
            AppAction::ToggleInspector => {
                if self.show_inspector {
                    self.show_inspector = false;
                    self.mode = InteractionMode::Normal;
                } else {
                    self.show_inspector = true;
                    self.mode = InteractionMode::InspectorFocus {
                        selected_sub_idx: self.inspector_sub_idx,
                    };
                }
                None
            }

            AppAction::FocusInspector => {
                self.show_inspector = true;
                self.mode = InteractionMode::InspectorFocus {
                    selected_sub_idx: self.inspector_sub_idx,
                };
                None
            }

            AppAction::FocusInspectorWithItem(sub_idx) => {
                self.show_inspector = true;
                self.inspector_sub_idx = sub_idx;
                self.mode = InteractionMode::InspectorFocus {
                    selected_sub_idx: sub_idx,
                };
                None
            }

            AppAction::UnfocusInspector => {
                self.mode = InteractionMode::Normal;
                None
            }

            AppAction::SwitchInspectorTab(tab) => {
                self.show_inspector = true;
                self.active_inspector_tab = tab;
                self.inspector_sub_idx = 0;
                self.mode = InteractionMode::InspectorFocus {
                    selected_sub_idx: 0,
                };
                None
            }

            AppAction::CycleInspectorTabNext => {
                self.active_inspector_tab = self.active_inspector_tab.next();
                self.inspector_sub_idx = 0;
                if let InteractionMode::InspectorFocus {
                    ref mut selected_sub_idx,
                } = self.mode
                {
                    *selected_sub_idx = 0;
                }
                None
            }

            AppAction::CycleInspectorTabPrev => {
                self.active_inspector_tab = self.active_inspector_tab.prev();
                self.inspector_sub_idx = 0;
                if let InteractionMode::InspectorFocus {
                    ref mut selected_sub_idx,
                } = self.mode
                {
                    *selected_sub_idx = 0;
                }
                None
            }

            // Modals
            AppAction::OpenRemoveModal => {
                if !self.target_hashes().is_empty() {
                    self.mode = InteractionMode::ConfirmRemoveModal {
                        delete_files: false,
                        focus_confirm: true,
                    };
                }
                None
            }

            AppAction::OpenCommandPalette => {
                self.mode = InteractionMode::CommandPalette {
                    query: String::new(),
                    selected_idx: 0,
                };
                None
            }

            AppAction::OpenThemePicker => {
                self.mode = InteractionMode::ThemePickerModal {
                    current_theme_id: self.theme.id,
                    selected_theme_id: self.theme.id,
                };
                None
            }

            AppAction::OpenHelpModal => {
                self.mode = InteractionMode::ShortcutsHelpModal;
                None
            }

            AppAction::OpenAddModal => {
                let default_dir = "/downloads".to_string();
                let dir_cursor = default_dir.chars().count();
                self.mode = InteractionMode::AddTorrentModal {
                    source_input: String::new(),
                    source_cursor: 0,
                    dir_input: default_dir,
                    dir_cursor,
                    start_immediately: true,
                    sequential: false,
                    focused_field: 0,
                };
                None
            }

            AppAction::CloseModal => {
                self.mode = if self.multi_selected.is_empty() {
                    InteractionMode::Normal
                } else {
                    InteractionMode::VisualSelect
                };
                None
            }

            AppAction::SubmitModal => {
                match self.mode.clone() {
                    InteractionMode::ConfirmRemoveModal {
                        delete_files,
                        focus_confirm,
                    } => {
                        if focus_confirm {
                            let hashes = self.target_hashes();
                            self.mode = InteractionMode::Normal;
                            self.multi_selected.clear();
                            Some(EngineCommand::RemoveTorrent {
                                hashes,
                                delete_data: delete_files,
                            })
                        } else {
                            // User hit Enter while on Cancel
                            self.mode = InteractionMode::Normal;
                            None
                        }
                    }

                    InteractionMode::ThemePickerModal {
                        selected_theme_id, ..
                    } => {
                        self.theme = Theme::by_id(selected_theme_id);
                        self.set_status_message(format!(
                            "Theme updated to {}",
                            selected_theme_id.display_name()
                        ));
                        self.mode = InteractionMode::Normal;
                        None
                    }

                    InteractionMode::AddTorrentModal {
                        source_input,
                        dir_input,
                        start_immediately,
                        sequential,
                        focused_field,
                        ..
                    } => {
                        if focused_field == 4 {
                            self.mode = InteractionMode::Normal;
                            None
                        } else if !source_input.trim().is_empty() {
                            let cmd = EngineCommand::AddTorrent {
                                source: source_input.trim().to_string(),
                                destination: dir_input.trim().to_string(),
                                start: start_immediately,
                                sequential,
                            };
                            self.mode = InteractionMode::Normal;
                            Some(cmd)
                        } else {
                            self.mode = InteractionMode::Normal;
                            None
                        }
                    }

                    InteractionMode::CommandPalette {
                        query,
                        selected_idx,
                    } => {
                        let modal = CommandPaletteModal::new(&query, selected_idx, self.theme);
                        let filtered = modal.filtered_commands();
                        if let Some(cmd) = filtered.get(selected_idx) {
                            self.mode = InteractionMode::Normal;
                            match cmd.id {
                                "torrent.start" => self.dispatch_action(AppAction::StartTorrent),
                                "torrent.pause" => self.dispatch_action(AppAction::PauseTorrent),
                                "torrent.remove" => {
                                    self.dispatch_action(AppAction::OpenRemoveModal)
                                }
                                "torrent.recheck" => self.dispatch_action(AppAction::RecheckHash),
                                "torrent.add" => self.dispatch_action(AppAction::OpenAddModal),
                                "torrent.sequential" => {
                                    self.dispatch_action(AppAction::ToggleSequential)
                                }
                                "view.all" => {
                                    self.dispatch_action(AppAction::SwitchView(ActiveView::All))
                                }
                                "view.down" => self.dispatch_action(AppAction::SwitchView(
                                    ActiveView::Downloading,
                                )),
                                "view.seed" => {
                                    self.dispatch_action(AppAction::SwitchView(ActiveView::Seeding))
                                }
                                "view.active" => {
                                    self.dispatch_action(AppAction::SwitchView(ActiveView::Active))
                                }
                                "view.paused" => {
                                    self.dispatch_action(AppAction::SwitchView(ActiveView::Paused))
                                }
                                "view.complete" => self
                                    .dispatch_action(AppAction::SwitchView(ActiveView::Complete)),
                                "bandwidth.down_up" => {
                                    self.dispatch_action(AppAction::DownloadThrottleUp)
                                }
                                "bandwidth.down_down" => {
                                    self.dispatch_action(AppAction::DownloadThrottleDown)
                                }
                                "theme.select" => self.dispatch_action(AppAction::OpenThemePicker),
                                "help.shortcuts" => self.dispatch_action(AppAction::OpenHelpModal),
                                "session.quit" => Some(EngineCommand::Quit),
                                _ => None,
                            }
                        } else {
                            self.mode = InteractionMode::Normal;
                            None
                        }
                    }

                    _ => None,
                }
            }

            // Filtering
            AppAction::OpenFilter => {
                self.mode = InteractionMode::Filter {
                    query: String::new(),
                };
                None
            }

            AppAction::CharInput(c) => {
                match &mut self.mode {
                    InteractionMode::Filter { query } => {
                        query.push(c);
                        self.visible_indices = UiComputeEngine::filter_and_sort(
                            &self.snapshot.torrents,
                            self.active_view,
                            Some(query.as_str()),
                            self.sort_key,
                            self.sort_order,
                        );
                        self.selected_row = 0;
                    }
                    InteractionMode::CommandPalette {
                        query,
                        selected_idx,
                    } => {
                        query.push(c);
                        *selected_idx = 0;
                    }
                    InteractionMode::AddTorrentModal {
                        source_input,
                        source_cursor,
                        dir_input,
                        dir_cursor,
                        focused_field,
                        ..
                    } => {
                        if *focused_field == 0 {
                            insert_char_at_index(source_input, *source_cursor, c);
                            *source_cursor += 1;
                        } else if *focused_field == 1 {
                            insert_char_at_index(dir_input, *dir_cursor, c);
                            *dir_cursor += 1;
                        }
                    }
                    _ => {}
                }
                None
            }

            AppAction::BackspaceInput => {
                match &mut self.mode {
                    InteractionMode::Filter { query } => {
                        query.pop();
                        self.visible_indices = UiComputeEngine::filter_and_sort(
                            &self.snapshot.torrents,
                            self.active_view,
                            Some(query.as_str()),
                            self.sort_key,
                            self.sort_order,
                        );
                        self.selected_row = 0;
                    }
                    InteractionMode::CommandPalette {
                        query,
                        selected_idx,
                    } => {
                        query.pop();
                        *selected_idx = 0;
                    }
                    InteractionMode::AddTorrentModal {
                        source_input,
                        source_cursor,
                        dir_input,
                        dir_cursor,
                        focused_field,
                        ..
                    } => {
                        if *focused_field == 0 && *source_cursor > 0 {
                            remove_char_at_index(source_input, *source_cursor - 1);
                            *source_cursor -= 1;
                        } else if *focused_field == 1 && *dir_cursor > 0 {
                            remove_char_at_index(dir_input, *dir_cursor - 1);
                            *dir_cursor -= 1;
                        }
                    }
                    _ => {}
                }
                None
            }

            AppAction::DeleteInput => {
                if let InteractionMode::AddTorrentModal {
                    source_input,
                    source_cursor,
                    dir_input,
                    dir_cursor,
                    focused_field,
                    ..
                } = &mut self.mode
                {
                    if *focused_field == 0 {
                        remove_char_at_index(source_input, *source_cursor);
                    } else if *focused_field == 1 {
                        remove_char_at_index(dir_input, *dir_cursor);
                    }
                }
                None
            }

            AppAction::PasteInput(mut text) => {
                if text.is_empty() {
                    if let Some(clip) = get_system_clipboard() {
                        text = clip;
                    }
                }
                let clean_text: String =
                    text.chars().filter(|c| *c != '\r' && *c != '\n').collect();
                if !clean_text.is_empty() {
                    match &mut self.mode {
                        InteractionMode::AddTorrentModal {
                            source_input,
                            source_cursor,
                            dir_input,
                            dir_cursor,
                            focused_field,
                            ..
                        } => {
                            if *focused_field == 0 {
                                insert_str_at_char_index(source_input, *source_cursor, &clean_text);
                                *source_cursor += clean_text.chars().count();
                            } else if *focused_field == 1 {
                                insert_str_at_char_index(dir_input, *dir_cursor, &clean_text);
                                *dir_cursor += clean_text.chars().count();
                            }
                        }
                        InteractionMode::Filter { query } => {
                            query.push_str(&clean_text);
                            self.visible_indices = UiComputeEngine::filter_and_sort(
                                &self.snapshot.torrents,
                                self.active_view,
                                Some(query.as_str()),
                                self.sort_key,
                                self.sort_order,
                            );
                            self.selected_row = 0;
                        }
                        InteractionMode::CommandPalette {
                            query,
                            selected_idx,
                        } => {
                            query.push_str(&clean_text);
                            *selected_idx = 0;
                        }
                        _ => {}
                    }
                }
                None
            }

            AppAction::CommitFilter => {
                self.mode = InteractionMode::Normal;
                None
            }

            AppAction::CancelFilter => {
                self.mode = InteractionMode::Normal;
                self.visible_indices = UiComputeEngine::filter_and_sort(
                    &self.snapshot.torrents,
                    self.active_view,
                    None,
                    self.sort_key,
                    self.sort_order,
                );
                self.selected_row = 0;
                None
            }

            _ => None,
        }
    }

    /// Renders the complete application frame using Ratatui 0.30 diffing engine.
    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 10 || area.height < 5 {
            return;
        }

        let max_count = self.category_counts.iter().copied().max().unwrap_or(0);
        let layout = calculate_responsive_layout_dynamic(area, self.show_inspector, max_count);

        // Zone 1: Header Dashboard
        DashboardHeader::new(
            &self.snapshot.stats,
            &self.down_history,
            &self.up_history,
            self.theme,
            &self.endpoint_label,
        )
        .render(layout.header_area, frame.buffer_mut());

        // Zone 2A: View Tabs (Segmented Category Bar)
        ViewTabsWidget::new(self.active_view, self.category_counts, self.theme, false)
            .render(layout.view_tabs_area, frame.buffer_mut());

        // Sidebar fallback (if layout specifies sidebar_area)
        if let Some(sidebar_area) = layout.sidebar_area {
            let is_focused = matches!(self.mode, InteractionMode::Normal);
            ViewsSidebar::new(
                self.active_view,
                self.category_counts,
                self.theme,
                is_focused,
            )
            .render(sidebar_area, frame.buffer_mut());
        }

        // Zone 2B: Main Torrent Table
        let filter_label = match &self.mode {
            InteractionMode::Filter { query } => Some(query.as_str()),
            _ => None,
        };

        TorrentTableWidget::new(
            &self.snapshot.torrents,
            &self.visible_indices,
            self.selected_row,
            &self.multi_selected,
            self.theme,
            true,
        )
        .with_filter_label(filter_label)
        .render(layout.table_area, frame.buffer_mut());

        // Zone 3: Inspector Drawer
        if let Some(insp_area) = layout.inspector_area {
            if let Some(focused) = self.focused_torrent() {
                let is_focused = matches!(self.mode, InteractionMode::InspectorFocus { .. });
                InspectorDrawerWidget::new(
                    focused,
                    self.active_torrent_details.as_ref(),
                    self.active_inspector_tab,
                    self.inspector_sub_idx,
                    self.theme,
                    is_focused,
                )
                .render(insp_area, frame.buffer_mut());
            }
        }

        // Zone 4: Footer Status Bar
        let footer_mode = match &self.mode {
            InteractionMode::Normal => FooterMode::Normal,
            InteractionMode::VisualSelect => FooterMode::VisualSelect(self.multi_selected.len()),
            InteractionMode::Filter { .. } => FooterMode::Filter,
            InteractionMode::CommandPalette { .. } => FooterMode::CommandPalette,
            InteractionMode::InspectorFocus { .. } => FooterMode::InspectorFocus,
            _ => FooterMode::Normal,
        };

        let filter_text = match &self.mode {
            InteractionMode::Filter { query } => Some(query.as_str()),
            _ => None,
        };

        let message = self.status_message.as_ref().and_then(|(msg, time)| {
            if time.elapsed().as_secs() < 5 {
                Some(msg.as_str())
            } else {
                None
            }
        });

        StatusBarWidget::new(footer_mode, self.theme)
            .with_filter(filter_text)
            .with_message(message)
            .render(layout.footer_area, frame.buffer_mut());

        // Overlay Modals
        match &self.mode {
            InteractionMode::ConfirmRemoveModal {
                delete_files,
                focus_confirm,
            } => {
                let mut names = Vec::new();
                let mut total_bytes = 0;
                let hashes = self.target_hashes();
                for h in &hashes {
                    if let Some(t) = self.snapshot.torrents.iter().find(|t| t.hash == *h) {
                        names.push(t.name.clone());
                        total_bytes += t.size_bytes;
                    }
                }
                ConfirmRemoveModal::new(
                    &names,
                    total_bytes,
                    *delete_files,
                    *focus_confirm,
                    self.theme,
                )
                .render(area, frame.buffer_mut());
            }

            InteractionMode::CommandPalette {
                query,
                selected_idx,
            } => {
                CommandPaletteModal::new(query, *selected_idx, self.theme)
                    .render(area, frame.buffer_mut());
            }

            InteractionMode::ThemePickerModal {
                current_theme_id,
                selected_theme_id,
            } => {
                ThemePickerModal::new(*current_theme_id, *selected_theme_id)
                    .render(area, frame.buffer_mut());
            }

            InteractionMode::ShortcutsHelpModal => {
                ShortcutsHelpModal::new(self.theme).render(area, frame.buffer_mut());
            }

            InteractionMode::AddTorrentModal {
                source_input,
                source_cursor,
                dir_input,
                dir_cursor,
                start_immediately,
                sequential,
                focused_field,
            } => {
                AddTorrentModal::new(
                    source_input,
                    *source_cursor,
                    dir_input,
                    *dir_cursor,
                    *start_immediately,
                    *sequential,
                    *focused_field,
                    self.theme,
                )
                .render(area, frame.buffer_mut());
            }

            _ => {}
        }
    }
}

fn insert_char_at_index(target: &mut String, char_idx: usize, c: char) {
    let byte_idx = target
        .char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(target.len());
    target.insert(byte_idx, c);
}

fn remove_char_at_index(target: &mut String, char_idx: usize) -> Option<char> {
    if let Some((byte_idx, _)) = target.char_indices().nth(char_idx) {
        Some(target.remove(byte_idx))
    } else {
        None
    }
}

fn insert_str_at_char_index(target: &mut String, char_idx: usize, to_insert: &str) {
    let byte_idx = target
        .char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(target.len());
    target.insert_str(byte_idx, to_insert);
}

fn get_system_clipboard() -> Option<String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Ok(out) = std::process::Command::new("wl-paste")
            .arg("--no-newline")
            .output()
        {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }
    }

    if std::env::var_os("DISPLAY").is_some() {
        if let Ok(out) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
        {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }

        if let Ok(out) = std::process::Command::new("xsel")
            .args(["--clipboard", "--output"])
            .output()
        {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }
    }

    None
}
