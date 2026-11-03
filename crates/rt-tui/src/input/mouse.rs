//! Mouse event handler and spatial hit-testing.

use crate::input::fsm::InteractionMode;
use crate::input::router::AppAction;
use crate::state::ActiveView;
use crate::ui::layout::ComputedLayout;
use crate::widgets::{InspectorDrawerWidget, ViewTabsWidget};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

pub struct MouseHandler;

impl MouseHandler {
    pub fn handle_mouse(
        event: MouseEvent,
        layout: &ComputedLayout,
        mode: &InteractionMode,
        term_area: Rect,
        counts: [usize; 8],
    ) -> AppAction {
        match event.kind {
            MouseEventKind::ScrollDown => {
                if let Some(insp) = layout.inspector_area {
                    if event.column >= insp.left()
                        && event.column < insp.right()
                        && event.row >= insp.top()
                        && event.row < insp.bottom()
                    {
                        return AppAction::CursorDown;
                    }
                }
                AppAction::CursorDown
            }
            MouseEventKind::ScrollUp => {
                if let Some(insp) = layout.inspector_area {
                    if event.column >= insp.left()
                        && event.column < insp.right()
                        && event.row >= insp.top()
                        && event.row < insp.bottom()
                    {
                        return AppAction::CursorUp;
                    }
                }
                AppAction::CursorUp
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let x = event.column;
                let y = event.row;

                // Handle ConfirmRemoveModal clicks
                if matches!(mode, InteractionMode::ConfirmRemoveModal { .. }) {
                    let modal_width = 68.min(term_area.width.saturating_sub(4));
                    let modal_height = 15.min(term_area.height.saturating_sub(2));
                    let modal_x = term_area.x + (term_area.width.saturating_sub(modal_width)) / 2;
                    let modal_y = term_area.y + (term_area.height.saturating_sub(modal_height)) / 2;

                    // Option 0: Keep data row (modal_y + 9)
                    if y == modal_y + 9 && x >= modal_x + 2 && x < modal_x + modal_width - 2 {
                        return AppAction::CursorUp;
                    }
                    // Option 1: Permanently delete row (modal_y + 10)
                    if y == modal_y + 10 && x >= modal_x + 2 && x < modal_x + modal_width - 2 {
                        return AppAction::CursorDown;
                    }
                    // Buttons row: modal_y + modal_height - 2
                    if y == modal_y + modal_height - 2 {
                        // Cancel button
                        if x >= modal_x + 3 && x <= modal_x + 22 {
                            return AppAction::CloseModal;
                        }
                        // Confirm button
                        if x >= modal_x + modal_width.saturating_sub(30)
                            && x <= modal_x + modal_width - 2
                        {
                            return AppAction::SubmitModal;
                        }
                    }

                    // Clicked elsewhere on the modal: consume event without affecting table
                    if x >= modal_x
                        && x < modal_x + modal_width
                        && y >= modal_y
                        && y < modal_y + modal_height
                    {
                        return AppAction::None;
                    }
                }

                // If any modal is active, do not forward click to background table/sidebar
                if mode.is_modal() {
                    return AppAction::None;
                }

                // 1. Check if click was in view tabs
                if let Some(view) = ViewTabsWidget::hit_test(x, y, layout.view_tabs_area, counts) {
                    return AppAction::SwitchView(view);
                }

                // 2. Check if click was in sidebar (fallback)
                if let Some(sidebar) = layout.sidebar_area {
                    if x >= sidebar.left()
                        && x < sidebar.right()
                        && y > sidebar.top()
                        && y < sidebar.bottom()
                    {
                        let row_offset = (y - sidebar.top() - 1) as usize;
                        if let Some(view) = ActiveView::from_number(row_offset + 1) {
                            return AppAction::SwitchView(view);
                        }
                    }
                }

                // 3. Check if click was in inspector area
                if let Some(insp) = layout.inspector_area {
                    if x >= insp.left() && x < insp.right() && y >= insp.top() && y < insp.bottom()
                    {
                        if InspectorDrawerWidget::is_close_button_hit(x, y, insp) {
                            return AppAction::ToggleInspector;
                        }
                        if let Some(tab) = InspectorDrawerWidget::hit_test(x, y, insp) {
                            return AppAction::SwitchInspectorTab(tab);
                        }
                        if InspectorDrawerWidget::is_content_hit(x, y, insp) {
                            let sub_idx = (y.saturating_sub(insp.top() + 2)) as usize;
                            return AppAction::FocusInspectorWithItem(sub_idx);
                        }
                        return AppAction::FocusInspector;
                    }
                }

                // 4. Check if click was in main table
                let table = layout.table_area;
                if x >= table.left() && x < table.right() && y > table.top() && y < table.bottom() {
                    return AppAction::ToggleSelect;
                }

                // 5. Check if click was in footer
                let footer = layout.footer_area;
                if y == footer.y && x >= footer.right().saturating_sub(10) {
                    return AppAction::Quit;
                }

                AppAction::None
            }
            _ => AppAction::None,
        }
    }
}
