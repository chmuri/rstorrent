use rt_core::ViewManager;
use rt_tui::display::{color_map::parse_color_definition, Canvas, ColorKind};
use rt_tui::ui::{ActiveScreen, InputPromptMode, RootUI};
use rt_tui::Window;
use std::sync::{Arc, RwLock};

#[test]
fn test_root_ui_initialization_and_layout() {
    let mut ui = RootUI::new();
    let vm = Arc::new(RwLock::new(ViewManager::new()));
    ui.set_view_manager(vm);

    assert_eq!(ui.active_screen, ActiveScreen::DownloadList);
    assert_eq!(ui.current_view_name, "main");

    let mut canvas = Canvas::new(0, 0, 80, 24);
    ui.balance_and_redraw(&mut canvas);

    let rows = canvas.to_string_rows();
    assert_eq!(rows.len(), 24);
    // Row 0 should be title
    assert!(rows[0].contains("rTorrent"));
    // Row 1 should be view
    assert!(rows[1].contains("[View: main]"));
    // Row 23 should be statusbar
    assert!(rows[23].contains("Throttle"));
}

#[test]
fn test_root_ui_input_prompt_lifecycle() {
    let mut ui = RootUI::new();
    assert_eq!(ui.input_mode, InputPromptMode::None);

    ui.enable_input("load.normal", InputPromptMode::LoadNormal);
    assert_eq!(ui.input_mode, InputPromptMode::LoadNormal);

    // Type "/path/to/test.torrent"
    for b in b"/path/to/test.torrent" {
        ui.handle_key(*b as i32);
    }

    // Submit with Enter (10)
    let res = ui.handle_key(10);
    assert!(res.is_some());
    let (mode, text) = res.unwrap();
    assert_eq!(mode, InputPromptMode::LoadNormal);
    assert_eq!(text, "/path/to/test.torrent");
    assert_eq!(ui.input_mode, InputPromptMode::None);
}

#[test]
fn test_root_ui_navigation_and_shortcuts() {
    let mut ui = RootUI::new();
    let vm = Arc::new(RwLock::new(ViewManager::new()));
    ui.set_view_manager(vm);

    // Switch view with '3' (started)
    ui.handle_key(b'3' as i32);
    assert_eq!(ui.current_view_name, "started");

    // Toggle log view with 'l'
    ui.handle_key(b'l' as i32);
    assert_eq!(ui.active_screen, ActiveScreen::Log);

    // Exit log view back to download list with 'l'
    ui.handle_key(b'l' as i32);
    assert_eq!(ui.active_screen, ActiveScreen::DownloadList);

    // Throttle adjustments
    assert_eq!(ui.throttle_up, 0);
    ui.handle_key(b'a' as i32); // +1
    assert_eq!(ui.throttle_up, 1);
    ui.handle_key(b's' as i32); // +5
    assert_eq!(ui.throttle_up, 6);
    ui.handle_key(b'x' as i32); // -5
    assert_eq!(ui.throttle_up, 1);

    // Quit request
    assert!(!ui.shutdown_requested);
    ui.handle_key(0x11); // ^Q
    assert!(ui.shutdown_requested);
}

#[test]
fn test_root_ui_colors_are_applied_on_redraw() {
    let mut ui = RootUI::new();
    ui.set_ui_color(ColorKind::Title, "bold white on blue")
        .unwrap();
    ui.set_ui_color(ColorKind::Footer, "black on yellow")
        .unwrap();

    let mut canvas = Canvas::new(0, 0, 80, 24);
    ui.balance_and_redraw(&mut canvas);

    let expected_title = parse_color_definition("bold white on blue").unwrap();
    let title = canvas.get_raw_cell(0, 0).unwrap();
    assert_eq!(title.style.fg, expected_title.fg);
    assert_eq!(title.style.bg, expected_title.bg);

    let expected_footer = parse_color_definition("black on yellow").unwrap();
    let footer = canvas.get_raw_cell(0, 23).unwrap();
    assert_eq!(footer.style.fg, expected_footer.fg);
    assert_eq!(footer.style.bg, expected_footer.bg);
}

#[test]
fn test_root_ui_invalid_color_does_not_change_map() {
    let mut ui = RootUI::new();
    ui.set_ui_color(ColorKind::Title, "red").unwrap();
    let before = ui
        .color_map
        .read()
        .unwrap()
        .get_style(ColorKind::Title, None);

    assert!(ui.set_ui_color(ColorKind::Title, "not-a-color").is_err());
    assert_eq!(
        ui.color_map
            .read()
            .unwrap()
            .get_style(ColorKind::Title, None),
        before
    );
    assert_eq!(ui.ui_color(ColorKind::Title), "red");
}

#[test]
fn test_root_ui_layout_variable_matches_window() {
    let mut ui = RootUI::new();
    assert_eq!(ui.torrent_list_layout, "full");

    ui.set_torrent_list_layout("compact");
    assert_eq!(ui.torrent_list_layout, "compact");
    assert_eq!(ui.window_download_list.lock().unwrap().layout, "compact");

    ui.toggle_layout();
    assert_eq!(ui.torrent_list_layout, "full");
    assert_eq!(ui.window_download_list.lock().unwrap().layout, "full");
}

#[test]
fn test_root_ui_dirty_marking_helpers() {
    let mut ui = RootUI::new();
    let mut canvas = Canvas::new(0, 0, 80, 24);
    ui.window_statusbar.lock().unwrap().redraw(&mut canvas);
    assert!(!ui.window_statusbar.lock().unwrap().is_dirty());

    ui.set_status_throttle_up("up");
    assert!(ui.window_statusbar.lock().unwrap().is_dirty());

    ui.window_input.lock().unwrap().redraw(&mut canvas);
    assert!(!ui.window_input.lock().unwrap().is_dirty());
    ui.set_input_history_size(10);
    assert!(ui.window_input.lock().unwrap().is_dirty());

    ui.window_title.lock().unwrap().redraw(&mut canvas);
    assert!(!ui.window_title.lock().unwrap().is_dirty());
    ui.mark_ui_dirty();
    assert!(ui.window_title.lock().unwrap().is_dirty());
}
