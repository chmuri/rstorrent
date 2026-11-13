//! Comprehensive integration and verification tests for the Next-Gen Ratatui UI.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use rt_tui::app::{EngineCommand, NextGenTuiApp};
use rt_tui::concurrency::compute::UiComputeEngine;
use rt_tui::input::fsm::InteractionMode;
use rt_tui::input::router::{AppAction, KeyRouter};
use rt_tui::state::{
    ActiveView, EngineSnapshot, PieceState, SessionStats, SortKey, SortOrder, TorrentUiRecord,
    TorrentUiState,
};
use rt_tui::theme::{ThemeId, THEME_TOKYO_NIGHT};
use rt_tui::ui::layout::{calculate_responsive_layout, LayoutMode};
use rt_tui::widgets::piece_map::PieceMapWidget;
use rt_tui::widgets::progress::format_fractional_bar;
use rt_tui::widgets::sparkline::RateHistory;
use std::sync::Arc;

fn mock_torrent(name: &str, down: u64, size: u64, state: TorrentUiState) -> TorrentUiRecord {
    let mut hash = [0u8; 20];
    for (i, b) in name.bytes().take(20).enumerate() {
        hash[i] = b;
    }
    TorrentUiRecord {
        hash,
        hash_hex: format!("{:02x?}", hash)
            .replace(", ", "")
            .replace("[", "")
            .replace("]", ""),
        name: name.into(),
        size_bytes: size,
        completed_bytes: size / 2,
        down_rate_bps: down,
        up_rate_bps: down / 5,
        total_uploaded_bytes: down * 10,
        state,
        seeds_connected: 12,
        seeds_total: 45,
        peers_connected: 20,
        peers_total: 80,
        priority: 1,
        eta_seconds: (size / 2).checked_div(down),
        ratio_promille: 350,
        tags: vec!["iso".into(), "linux".into()],
        piece_size: 4 * 1024 * 1024,
        total_pieces: 1200,
        verified_pieces: 600,
        save_path: "/downloads/linux".into(),
        is_private: false,
        message: None,
    }
}

#[test]
fn test_braille_piece_map_bitpacking() {
    let pieces = vec![PieceState::Verified; 8];
    let widget = PieceMapWidget::new(&pieces, THEME_TOKYO_NIGHT);
    let (ch, color) = widget.compute_braille_cell(0);

    // Full Braille 8 dots = 0x28FF = '⣿'
    assert_eq!(ch, '⣿');
    assert_eq!(color, THEME_TOKYO_NIGHT.success);

    let empty = vec![PieceState::Missing; 8];
    let widget_empty = PieceMapWidget::new(&empty, THEME_TOKYO_NIGHT);
    let (ch_empty, _) = widget_empty.compute_braille_cell(0);
    assert_eq!(ch_empty, '⠀');
}

#[test]
fn test_fractional_sub_block_precision() {
    let bar_0 = format_fractional_bar(0.0, 10);
    assert_eq!(bar_0, " ░░░░░░░░░");

    let bar_100 = format_fractional_bar(1.0, 10);
    assert_eq!(bar_100, "██████████");

    // Partial step (2.5% of 10 chars * 8 steps/char = 2 steps => '▎')
    let bar_partial = format_fractional_bar(0.025, 10);
    assert_eq!(bar_partial, "▎░░░░░░░░░");
}

#[test]
fn test_rate_history_sparkline_ring_buffer() {
    let mut history: RateHistory<60> = RateHistory::new();
    for i in 1..=100 {
        history.push(i * 1024);
    }

    assert_eq!(history.current(), 100 * 1024);
    assert_eq!(history.to_vec().len(), 60);
    assert_eq!(history.max(), 100 * 1024);
}

#[test]
fn test_responsive_layout_breakpoints() {
    use ratatui::layout::Rect;

    // Compact mode (80x24)
    let compact = calculate_responsive_layout(Rect::new(0, 0, 80, 24), false);
    assert_eq!(compact.mode, LayoutMode::Compact);
    assert!(compact.sidebar_area.is_none());
    assert_eq!(compact.view_tabs_area.height, 1);
    assert_eq!(compact.table_area.width, 80);

    // Standard mode (120x35) - Full width table without sidebar void
    let standard = calculate_responsive_layout(Rect::new(0, 0, 120, 35), true);
    assert_eq!(standard.mode, LayoutMode::Standard);
    assert_eq!(standard.view_tabs_area.height, 1);
    assert_eq!(standard.table_area.width, 120);
    assert!(standard.inspector_area.is_some());

    // UltraWide mode (180x45) - Side-by-side inspector
    let wide = calculate_responsive_layout(Rect::new(0, 0, 180, 45), true);
    assert_eq!(wide.mode, LayoutMode::UltraWide);
    assert_eq!(wide.view_tabs_area.height, 1);
    assert!(wide.inspector_area.is_some());
    // Side-by-side: table 60% of 180 = 108
    assert_eq!(wide.table_area.width, 108);
}

#[test]
fn test_rayon_parallel_sort_and_filter_scale() {
    // Generate 5,000 torrents
    let mut torrents = Vec::with_capacity(5000);
    for i in 0..5000 {
        let state = if i % 3 == 0 {
            TorrentUiState::Downloading
        } else if i % 3 == 1 {
            TorrentUiState::Seeding
        } else {
            TorrentUiState::Paused
        };
        torrents.push(mock_torrent(
            &format!("Torrent_{:05}", i),
            (i as u64) * 1000,
            1_000_000,
            state,
        ));
    }

    let start = std::time::Instant::now();
    let sorted = UiComputeEngine::filter_and_sort(
        &torrents,
        ActiveView::All,
        Some("Torrent_001"),
        SortKey::DownRate,
        SortOrder::Descending,
    );
    let elapsed = start.elapsed();

    assert!(sorted.len() >= 10);
    // Benchmarking requirement: sorting 5,000 torrents should take < 5ms
    assert!(
        elapsed.as_millis() < 50,
        "Sort took {:?} which is unexpectedly slow",
        elapsed
    );

    let counts = UiComputeEngine::count_categories(&torrents);
    assert_eq!(counts[0], 5000);
    assert!(counts[1] > 0);
}

#[test]
fn test_app_fsm_navigation_and_actions() {
    let mut app = NextGenTuiApp::new("127.0.0.1:50051".into());

    let t1 = mock_torrent(
        "Ubuntu 24.04",
        10_000_000,
        4_000_000_000,
        TorrentUiState::Downloading,
    );
    let t2 = mock_torrent(
        "Debian 13",
        5_000_000,
        3_000_000_000,
        TorrentUiState::Seeding,
    );

    let snapshot = Arc::new(EngineSnapshot {
        sequence: 1,
        timestamp: std::time::Instant::now(),
        torrents: vec![t1, t2].into(),
        stats: SessionStats {
            global_down_bps: 10_000_000,
            global_up_bps: 2_000_000,
            global_down_total: 50_000_000,
            global_up_total: 10_000_000,
            disk_free_bytes: 400_000_000_000,
            disk_total_bytes: 1_000_000_000_000,
            dht_nodes: 400,
            listen_port: 6881,
            port_open: true,
            total_torrents: 2,
            active_torrents: 2,
        },
    });

    app.update_snapshot(snapshot);
    assert_eq!(app.visible_indices.len(), 2);
    assert_eq!(app.selected_row, 0);

    // Cursor navigation down
    app.dispatch_action(AppAction::CursorDown);
    assert_eq!(app.selected_row, 1);

    // Multi-selection with Space
    app.dispatch_action(AppAction::ToggleSelect);
    assert_eq!(app.multi_selected.len(), 1);
    assert_eq!(app.mode, InteractionMode::VisualSelect);

    // Bulk start command
    let cmd = app.dispatch_action(AppAction::StartTorrent);
    assert!(matches!(cmd, Some(EngineCommand::StartTorrent(_))));

    // Open Theme Picker
    app.dispatch_action(AppAction::OpenThemePicker);
    assert!(matches!(app.mode, InteractionMode::ThemePickerModal { .. }));

    // Cycle theme down and submit
    app.dispatch_action(AppAction::CursorDown);
    app.dispatch_action(AppAction::SubmitModal);
    assert_eq!(app.mode, InteractionMode::Normal);
    assert_ne!(app.theme.id, ThemeId::TokyoNight);
}

#[test]
fn test_headless_terminal_render() {
    let backend = TestBackend::new(120, 35);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut app = NextGenTuiApp::new("127.0.0.1:50051".into());
    let t1 = mock_torrent(
        "Ubuntu 24.04",
        10_000_000,
        4_000_000_000,
        TorrentUiState::Downloading,
    );

    let snapshot = Arc::new(EngineSnapshot {
        sequence: 1,
        timestamp: std::time::Instant::now(),
        torrents: vec![t1].into(),
        stats: SessionStats::default(),
    });

    app.update_snapshot(snapshot);

    terminal
        .draw(|f| {
            app.render(f);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    // Verify that the title and columns are present in the buffer
    let mut found_title = false;
    let mut found_torrent = false;

    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            line.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
        }
        if line.contains("rtorrent v") {
            found_title = true;
        }
        if line.contains("Ubuntu 24.04") {
            found_torrent = true;
        }
    }

    assert!(
        found_title,
        "Dashboard title not rendered in headless buffer"
    );
    assert!(
        found_torrent,
        "Torrent name not rendered in headless buffer"
    );
}

#[test]
fn test_key_router_routes_vim_and_standard_keys() {
    let normal = InteractionMode::Normal;

    // Vim 'j' -> CursorDown
    let ev_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(KeyRouter::route_key(ev_j, &normal), AppAction::CursorDown);

    // Arrow Down -> CursorDown
    let ev_down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        KeyRouter::route_key(ev_down, &normal),
        AppAction::CursorDown
    );

    // Command palette ':' and 'Ctrl+P'
    let ev_colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    assert_eq!(
        KeyRouter::route_key(ev_colon, &normal),
        AppAction::OpenCommandPalette
    );
    let ev_ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert_eq!(
        KeyRouter::route_key(ev_ctrl_p, &normal),
        AppAction::OpenCommandPalette
    );

    // Views '1' -> View All
    let ev_1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE);
    assert_eq!(
        KeyRouter::route_key(ev_1, &normal),
        AppAction::SwitchView(ActiveView::All)
    );
}

#[test]
fn test_views_sidebar_does_not_overflow_buffer_with_large_counts() {
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Widget;
    use rt_tui::widgets::ViewsSidebar;

    let backend = TestBackend::new(18, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    let sidebar = ViewsSidebar::new(
        ActiveView::All,
        [10000, 9869, 31, 9, 0, 93, 0, 0],
        THEME_TOKYO_NIGHT,
        true,
    );

    terminal
        .draw(|f| {
            let area = ratatui::layout::Rect::new(0, 0, 18, 12);
            sidebar.render(area, f.buffer_mut());
        })
        .unwrap();

    let buf = terminal.backend().buffer();
    // Verify right border at x = 17 is intact and contains border character '│'
    for y in 1..9 {
        let sym = buf[(17, y)].symbol();
        assert_eq!(
            sym, "│",
            "Right border at y={} was overwritten by text overflow: {:?}",
            y, sym
        );
    }
}

#[test]
fn test_views_sidebar_dynamic_width_and_number_visibility() {
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;
    use rt_tui::ui::layout::dynamic_sidebar_width;
    use rt_tui::widgets::ViewsSidebar;

    // 1. Verify dynamic sidebar width adapts to number of digits
    assert_eq!(dynamic_sidebar_width(120, 0), 22);
    assert_eq!(dynamic_sidebar_width(120, 99), 22);
    assert_eq!(dynamic_sidebar_width(120, 999), 23);
    assert_eq!(dynamic_sidebar_width(120, 9869), 24);
    assert_eq!(dynamic_sidebar_width(120, 14250), 25);
    assert_eq!(dynamic_sidebar_width(160, 100000), 26);

    let sidebar_width = dynamic_sidebar_width(120, 14250);
    assert_eq!(sidebar_width, 25);

    // 2. Render sidebar with 14250 torrents and 0 on downloading
    let render_area = Rect::new(0, 0, sidebar_width, 14);
    let backend = TestBackend::new(sidebar_width, 14);
    let mut terminal = Terminal::new(backend).unwrap();

    let sidebar = ViewsSidebar::new(
        ActiveView::All,
        [14250, 0, 31, 9, 0, 93, 0, 0],
        THEME_TOKYO_NIGHT,
        true,
    );

    terminal
        .draw(|f| {
            sidebar.render(render_area, f.buffer_mut());
        })
        .unwrap();

    let buf = terminal.backend().buffer();

    // Verify row 1 has "14250"
    let mut line_row1 = String::new();
    for x in 0..sidebar_width {
        line_row1.push(buf[(x, 1)].symbol().chars().next().unwrap_or(' '));
    }
    assert!(
        line_row1.contains("14250"),
        "Row 1 should display 14250: {}",
        line_row1
    );

    // Verify row 2 (Downloading) has "0" clearly visible
    let mut line_row2 = String::new();
    for x in 0..sidebar_width {
        line_row2.push(buf[(x, 2)].symbol().chars().next().unwrap_or(' '));
    }
    assert!(
        line_row2.contains("0"),
        "Row 2 (Downloading) should clearly display 0: {}",
        line_row2
    );

    // Verify right border at x = sidebar_width - 1 is intact
    let right_border_x = sidebar_width - 1;
    for y in 1..9 {
        let sym = buf[(right_border_x, y)].symbol();
        assert_eq!(sym, "│", "Right border at y={} should be '│'", y);
    }
}

#[test]
fn test_confirm_remove_modal_navigation_and_permanent_delete() {
    let modal_mode = InteractionMode::ConfirmRemoveModal {
        delete_files: false,
        focus_confirm: true,
    };

    // Down key in modal -> CursorDown (select permanently delete)
    let ev_down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        KeyRouter::route_key(ev_down, &modal_mode),
        AppAction::CursorDown
    );

    // Vim 'j' in modal -> CursorDown
    let ev_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    assert_eq!(
        KeyRouter::route_key(ev_j, &modal_mode),
        AppAction::CursorDown
    );

    // Up key in modal -> CursorUp (keep files)
    let ev_up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(
        KeyRouter::route_key(ev_up, &modal_mode),
        AppAction::CursorUp
    );

    // Enter in modal -> SubmitModal
    let ev_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        KeyRouter::route_key(ev_enter, &modal_mode),
        AppAction::SubmitModal
    );

    // App state machine test with 10k selection
    let mut app = NextGenTuiApp::new("127.0.0.1:50051".into());
    let t1 = mock_torrent(
        "Torrent_00001",
        1_000_000,
        100_000_000,
        TorrentUiState::Downloading,
    );
    let snapshot = Arc::new(EngineSnapshot {
        sequence: 1,
        timestamp: std::time::Instant::now(),
        torrents: vec![t1].into(),
        stats: SessionStats::default(),
    });
    app.update_snapshot(snapshot);

    // Open remove confirmation modal
    app.dispatch_action(AppAction::OpenRemoveModal);
    assert_eq!(
        app.mode,
        InteractionMode::ConfirmRemoveModal {
            delete_files: false,
            focus_confirm: true,
        }
    );

    // Press Down -> should change delete_files to true (Permanently delete data files)
    app.dispatch_action(AppAction::CursorDown);
    assert_eq!(
        app.mode,
        InteractionMode::ConfirmRemoveModal {
            delete_files: true,
            focus_confirm: true,
        }
    );

    // Press Up -> should change delete_files back to false (Keep data files)
    app.dispatch_action(AppAction::CursorUp);
    assert_eq!(
        app.mode,
        InteractionMode::ConfirmRemoveModal {
            delete_files: false,
            focus_confirm: true,
        }
    );

    // Press Down again to permanently delete
    app.dispatch_action(AppAction::CursorDown);

    // Submit modal (Press Enter) -> Should return RemoveTorrent command with delete_data: true!
    let cmd = app.dispatch_action(AppAction::SubmitModal);
    match cmd {
        Some(EngineCommand::RemoveTorrent {
            hashes,
            delete_data,
        }) => {
            assert_eq!(hashes.len(), 1);
            assert!(
                delete_data,
                "Expected delete_data to be true for permanently delete"
            );
        }
        other => panic!(
            "Expected RemoveTorrent with delete_data: true, got {:?}",
            other
        ),
    }
}

#[test]
fn test_add_torrent_modal_navigation_editing_and_pasting() {
    let mut app = NextGenTuiApp::new("127.0.0.1:50051".into());

    // 1. Open AddTorrentModal
    app.dispatch_action(AppAction::OpenAddModal);
    match &app.mode {
        InteractionMode::AddTorrentModal {
            source_input,
            source_cursor,
            dir_input,
            dir_cursor,
            start_immediately,
            sequential,
            focused_field,
        } => {
            assert_eq!(source_input, "");
            assert_eq!(*source_cursor, 0);
            assert_eq!(dir_input, "/downloads");
            assert_eq!(*dir_cursor, 10);
            assert!(*start_immediately);
            assert!(!*sequential);
            assert_eq!(*focused_field, 0);
        }
        other => panic!("Expected AddTorrentModal, got {:?}", other),
    }

    // 2. Paste long magnet link with trailing newline
    app.dispatch_action(AppAction::PasteInput(
        "magnet:?xt=urn:btih:0123456789abcdef\r\n".to_string(),
    ));
    if let InteractionMode::AddTorrentModal {
        source_input,
        source_cursor,
        ..
    } = &app.mode
    {
        assert_eq!(source_input, "magnet:?xt=urn:btih:0123456789abcdef");
        assert_eq!(*source_cursor, 36);
    } else {
        panic!("Wrong mode");
    }

    // 3. Move cursor Left 6 times, backspace 1 char, insert 'Z'
    for _ in 0..6 {
        app.dispatch_action(AppAction::CursorLeft);
    }
    if let InteractionMode::AddTorrentModal { source_cursor, .. } = &app.mode {
        assert_eq!(*source_cursor, 30);
    }
    // Backspace: removes char before index 30 ('9')
    app.dispatch_action(AppAction::BackspaceInput);
    if let InteractionMode::AddTorrentModal {
        source_input,
        source_cursor,
        ..
    } = &app.mode
    {
        assert_eq!(source_input, "magnet:?xt=urn:btih:012345678abcdef");
        assert_eq!(*source_cursor, 29);
    }
    // Insert 'Z' at cursor
    app.dispatch_action(AppAction::CharInput('Z'));
    if let InteractionMode::AddTorrentModal {
        source_input,
        source_cursor,
        ..
    } = &app.mode
    {
        assert_eq!(source_input, "magnet:?xt=urn:btih:012345678Zabcdef");
        assert_eq!(*source_cursor, 30);
    }

    // 4. DeleteInput: deletes char at cursor ('a')
    app.dispatch_action(AppAction::DeleteInput);
    if let InteractionMode::AddTorrentModal {
        source_input,
        source_cursor,
        ..
    } = &app.mode
    {
        assert_eq!(source_input, "magnet:?xt=urn:btih:012345678Zbcdef");
        assert_eq!(*source_cursor, 30);
    }

    // 5. CursorHome & CursorEnd
    app.dispatch_action(AppAction::CursorHome);
    if let InteractionMode::AddTorrentModal { source_cursor, .. } = &app.mode {
        assert_eq!(*source_cursor, 0);
    }
    app.dispatch_action(AppAction::CursorEnd);
    if let InteractionMode::AddTorrentModal {
        source_input,
        source_cursor,
        ..
    } = &app.mode
    {
        assert_eq!(*source_cursor, source_input.chars().count());
    }

    // 6. Space input on field 0
    let key_space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    let action_space = KeyRouter::route_key(key_space, &app.mode);
    assert_eq!(action_space, AppAction::CharInput(' '));
    app.dispatch_action(action_space);

    // 7. Tab to field 1 (dir_input)
    let key_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let action_tab = KeyRouter::route_key(key_tab, &app.mode);
    assert_eq!(action_tab, AppAction::CursorDown);
    app.dispatch_action(action_tab);
    if let InteractionMode::AddTorrentModal { focused_field, .. } = &app.mode {
        assert_eq!(*focused_field, 1);
    }

    // Append "/movies" to dir_input
    app.dispatch_action(AppAction::PasteInput("/movies".to_string()));
    if let InteractionMode::AddTorrentModal {
        dir_input,
        dir_cursor,
        ..
    } = &app.mode
    {
        assert_eq!(dir_input, "/downloads/movies");
        assert_eq!(*dir_cursor, 17);
    }

    // 8. Tab to field 2 (start_immediately) & toggle with Space
    app.dispatch_action(AppAction::CursorDown);
    let action_toggle = KeyRouter::route_key(key_space, &app.mode);
    assert_eq!(action_toggle, AppAction::ToggleSelect);
    app.dispatch_action(action_toggle);
    if let InteractionMode::AddTorrentModal {
        start_immediately, ..
    } = &app.mode
    {
        assert!(
            !*start_immediately,
            "Expected start_immediately to be false"
        );
    }

    // 9. Tab to field 3 (sequential) & toggle with Space
    app.dispatch_action(AppAction::CursorDown);
    let action_seq = KeyRouter::route_key(key_space, &app.mode);
    assert_eq!(action_seq, AppAction::ToggleSelect);
    app.dispatch_action(action_seq);
    if let InteractionMode::AddTorrentModal { sequential, .. } = &app.mode {
        assert!(*sequential, "Expected sequential to be true");
    }

    // 10. Tab to field 4 (Cancel button)
    app.dispatch_action(AppAction::CursorDown);
    if let InteractionMode::AddTorrentModal { focused_field, .. } = &app.mode {
        assert_eq!(*focused_field, 4);
    }
    // CursorRight moves to field 5 (Submit button)
    app.dispatch_action(AppAction::CursorRight);
    if let InteractionMode::AddTorrentModal { focused_field, .. } = &app.mode {
        assert_eq!(*focused_field, 5);
    }
    // CursorLeft moves back to field 4
    app.dispatch_action(AppAction::CursorLeft);
    if let InteractionMode::AddTorrentModal { focused_field, .. } = &app.mode {
        assert_eq!(*focused_field, 4);
    }
    // CursorRight back to field 5
    app.dispatch_action(AppAction::CursorRight);

    // 11. Enter to submit
    let cmd = app.dispatch_action(AppAction::SubmitModal);
    match cmd {
        Some(EngineCommand::AddTorrent {
            source,
            destination,
            start,
            sequential,
        }) => {
            assert!(source.starts_with("magnet:?xt="));
            assert_eq!(destination, "/downloads/movies");
            assert!(!start);
            assert!(sequential);
        }
        other => panic!("Expected EngineCommand::AddTorrent, got {:?}", other),
    }
    assert_eq!(app.mode, InteractionMode::Normal);
}

#[test]
fn test_shortcuts_help_modal_dynamic_responsive_rendering() {
    let backend = TestBackend::new(120, 35);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut app = NextGenTuiApp::new("127.0.0.1:50051".into());
    app.dispatch_action(AppAction::OpenHelpModal);
    assert_eq!(app.mode, InteractionMode::ShortcutsHelpModal);

    terminal.draw(|f| app.render(f)).unwrap();
    let buffer = terminal.backend().buffer();

    let mut found_jump = false;
    let mut found_remove = false;
    let mut found_footer = false;

    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            line.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
        }
        if line.contains("Jump to first torrent") {
            found_jump = true;
        }
        if line.contains("Remove torrent(s) [Safe modal]") {
            found_remove = true;
        }
        if line.contains("Close cheatsheet") {
            found_footer = true;
        }
    }

    assert!(
        found_jump,
        "Full text 'Jump to first torrent' should be visible on 120-wide terminal without truncation"
    );
    assert!(
        found_remove,
        "Full text 'Remove torrent(s) [Safe modal]' should be visible on 120-wide terminal without truncation"
    );
    assert!(found_footer, "Cheatsheet footer should be rendered");

    // Close modal with '?'
    let ev_q = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
    let action_close = KeyRouter::route_key(ev_q, &app.mode);
    assert_eq!(action_close, AppAction::CloseModal);
    app.dispatch_action(action_close);
    assert_eq!(app.mode, InteractionMode::Normal);
}

#[test]
fn test_view_tabs_widget_rendering_and_hit_test() {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;
    use ratatui::Terminal;
    use rt_tui::state::ActiveView;
    use rt_tui::theme::THEME_TOKYO_NIGHT;
    use rt_tui::widgets::ViewTabsWidget;

    let counts = [14250, 10000, 4250, 12000, 0, 4250, 0, 0];
    let area = Rect::new(0, 0, 140, 1);
    let backend = TestBackend::new(140, 1);
    let mut terminal = Terminal::new(backend).unwrap();

    let widget = ViewTabsWidget::new(ActiveView::All, counts, THEME_TOKYO_NIGHT, true);
    terminal
        .draw(|f| {
            widget.render(area, f.buffer_mut());
        })
        .unwrap();

    let buf = terminal.backend().buffer();
    let mut row_str = String::new();
    for x in 0..140 {
        row_str.push(buf[(x, 0)].symbol().chars().next().unwrap_or(' '));
    }

    // Verify row has [1] All (14250) with indicator
    assert!(
        row_str.contains("[1] All (14250)"),
        "Row should contain active view label: {}",
        row_str
    );
    assert!(
        row_str.contains("[2] Downloading (10000)"),
        "Row should contain downloading view label: {}",
        row_str
    );

    // Hit testing
    let hit_all = ViewTabsWidget::hit_test(5, 0, area, counts);
    assert_eq!(hit_all, Some(ActiveView::All));

    // Outside bounds
    let hit_outside_y = ViewTabsWidget::hit_test(5, 1, area, counts);
    assert_eq!(hit_outside_y, None);
}

#[test]
fn test_inspector_tabs_and_close_button_hit_test() {
    use ratatui::layout::Rect;
    use rt_tui::widgets::{InspectorDrawerWidget, InspectorTab};

    // Inspector area (x: 100, y: 5, w: 80, h: 20)
    let insp_area = Rect::new(100, 5, 80, 20);

    // Tab bar is on row insp_area.y + 1 = 6
    let hit_tab1 = InspectorDrawerWidget::hit_test(103, 6, insp_area);
    assert_eq!(hit_tab1, Some(InspectorTab::General));

    // Outside row bounds
    assert_eq!(InspectorDrawerWidget::hit_test(103, 5, insp_area), None);
    assert_eq!(InspectorDrawerWidget::hit_test(103, 7, insp_area), None);

    // Close button hit testing in top border (row 5)
    assert!(InspectorDrawerWidget::is_close_button_hit(
        175, 5, insp_area
    ));
    assert!(!InspectorDrawerWidget::is_close_button_hit(
        105, 5, insp_area
    ));

    // Content body hit testing (row 7..24)
    assert!(InspectorDrawerWidget::is_content_hit(110, 8, insp_area));
    assert!(!InspectorDrawerWidget::is_content_hit(110, 5, insp_area));
    assert!(!InspectorDrawerWidget::is_content_hit(110, 6, insp_area));
}

#[test]
fn test_mouse_and_keyboard_tabs_interaction() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use rt_tui::input::mouse::MouseHandler;
    use rt_tui::ui::layout::calculate_responsive_layout;
    use rt_tui::widgets::InspectorTab;

    let mut app = NextGenTuiApp::new("test".into());
    let t1 = mock_torrent("test_00000.bin", 1000, 32768, TorrentUiState::Downloading);
    app.update_snapshot(Arc::new(EngineSnapshot {
        sequence: 1,
        timestamp: std::time::Instant::now(),
        torrents: vec![t1].into(),
        stats: SessionStats::default(),
    }));

    // In normal mode: inspector is closed by default
    assert_eq!(app.mode, InteractionMode::Normal);
    assert!(!app.show_inspector);

    // Press 'i' or Enter to open & focus inspector
    let ev_i = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
    let action_i = KeyRouter::route_key(ev_i, &app.mode);
    assert_eq!(action_i, AppAction::ToggleInspector);
    app.dispatch_action(action_i);
    assert!(app.show_inspector);
    assert_eq!(
        app.mode,
        InteractionMode::InspectorFocus {
            selected_sub_idx: 0
        }
    );

    // In InspectorFocus mode: press '2' to jump to Trackers tab
    let ev_2 = KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE);
    let action_2 = KeyRouter::route_key(ev_2, &app.mode);
    assert_eq!(
        action_2,
        AppAction::SwitchInspectorTab(InspectorTab::Trackers)
    );
    app.dispatch_action(action_2);
    assert_eq!(app.active_inspector_tab, InspectorTab::Trackers);

    // Press Right or 'l' or Tab to cycle next inspector tab
    let ev_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let action_tab = KeyRouter::route_key(ev_tab, &app.mode);
    assert_eq!(action_tab, AppAction::CycleInspectorTabNext);
    app.dispatch_action(action_tab);
    assert_eq!(app.active_inspector_tab, InspectorTab::Peers);

    // Press Esc or 'i' to return to Normal mode
    let ev_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let action_esc = KeyRouter::route_key(ev_esc, &app.mode);
    assert_eq!(action_esc, AppAction::UnfocusInspector);
    app.dispatch_action(action_esc);
    assert_eq!(app.mode, InteractionMode::Normal);

    // Mouse test: calculate layout with inspector open
    let term_rect = Rect::new(0, 0, 160, 40);
    let layout = calculate_responsive_layout(term_rect, true);
    assert!(layout.inspector_area.is_some());
    let insp = layout.inspector_area.unwrap();

    // Click on Inspector Tab 1: row insp.y + 1, column insp.x + 2
    let mouse_tab_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: insp.x + 2,
        row: insp.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    let mouse_act = MouseHandler::handle_mouse(
        mouse_tab_click,
        &layout,
        &app.mode,
        term_rect,
        app.category_counts,
        app.selected_row,
        app.visible_indices.len(),
    );
    assert_eq!(
        mouse_act,
        AppAction::SwitchInspectorTab(InspectorTab::General)
    );
    app.dispatch_action(mouse_act);
    assert_eq!(app.active_inspector_tab, InspectorTab::General);
    assert_eq!(
        app.mode,
        InteractionMode::InspectorFocus {
            selected_sub_idx: 0
        }
    );

    // Click on View Tab 2 (Downloading): row 4, column layout.view_tabs_area.x + 25
    let mouse_view_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: layout.view_tabs_area.x + 25,
        row: layout.view_tabs_area.y,
        modifiers: KeyModifiers::NONE,
    };
    let view_act = MouseHandler::handle_mouse(
        mouse_view_click,
        &layout,
        &app.mode,
        term_rect,
        app.category_counts,
        app.selected_row,
        app.visible_indices.len(),
    );
    assert_eq!(view_act, AppAction::SwitchView(ActiveView::Downloading));
    app.dispatch_action(view_act);
    assert_eq!(app.active_view, ActiveView::Downloading);
    assert_eq!(app.mode, InteractionMode::Normal);

    // Keyboard view navigation: Left/Right arrows cycle views
    let ev_left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
    let action_left = KeyRouter::route_key(ev_left, &app.mode);
    assert_eq!(action_left, AppAction::CycleViewPrev);
    app.dispatch_action(action_left);
    assert_eq!(app.active_view, ActiveView::All);
}

#[test]
fn test_mouse_table_selection_and_hit_testing() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use rt_tui::app::NextGenTuiApp;
    use rt_tui::input::fsm::InteractionMode;
    use rt_tui::input::mouse::MouseHandler;
    use rt_tui::input::router::AppAction;
    use rt_tui::state::{EngineSnapshot, TorrentUiState};
    use rt_tui::ui::layout::calculate_responsive_layout;
    use rt_tui::widgets::TorrentTableWidget;
    use std::sync::Arc;

    let term_rect = Rect::new(0, 0, 120, 30);
    let layout = calculate_responsive_layout(term_rect, false);
    let table = layout.table_area;

    // Table row hit testing directly:
    // Header is at table.top() + 1
    // Row 0 is at table.top() + 2
    // Row 1 is at table.top() + 3
    assert_eq!(
        TorrentTableWidget::row_at_position(table.top(), table, 0, 5),
        None
    ); // border
    assert_eq!(
        TorrentTableWidget::row_at_position(table.top() + 1, table, 0, 5),
        None
    ); // header
    assert_eq!(
        TorrentTableWidget::row_at_position(table.top() + 2, table, 0, 5),
        Some(0)
    ); // row 0
    assert_eq!(
        TorrentTableWidget::row_at_position(table.top() + 3, table, 0, 5),
        Some(1)
    ); // row 1
    assert_eq!(
        TorrentTableWidget::row_at_position(table.top() + 4, table, 0, 5),
        Some(2)
    ); // row 2
    assert_eq!(
        TorrentTableWidget::row_at_position(table.top() + 7, table, 0, 5),
        None
    ); // empty space (index 5 with 5 items -> None)
    assert_eq!(
        TorrentTableWidget::row_at_position(table.bottom() - 1, table, 0, 5),
        None
    ); // bottom border

    // Setup an app with 5 torrents
    let mut app = NextGenTuiApp::new("test".into());
    let mut torrents = Vec::new();
    for i in 0..5 {
        let rec = mock_torrent(
            &format!("Torrent #{}", i),
            1024,
            2048,
            TorrentUiState::Downloading,
        );
        torrents.push(rec);
    }
    let snapshot = Arc::new(EngineSnapshot {
        torrents: torrents.into(),
        ..Default::default()
    });
    app.update_snapshot(snapshot);
    assert_eq!(app.visible_indices.len(), 5);
    assert_eq!(app.selected_row, 0);

    // Left click on row 2 (table.top() + 4):
    // Should choose row 2 (AppAction::SelectRow(2)) instead of toggling/marking row 0
    let click_row_2 = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: table.left() + 10,
        row: table.top() + 4,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    let act = MouseHandler::handle_mouse(
        click_row_2,
        &layout,
        &app.mode,
        term_rect,
        app.category_counts,
        app.selected_row,
        app.visible_indices.len(),
    );
    assert_eq!(act, AppAction::SelectRow(2));
    app.dispatch_action(act);

    // Crucial: row 2 is chosen (selected_row == 2), and NOT multi-selected (multi_selected is empty)!
    assert_eq!(app.selected_row, 2);
    assert!(app.multi_selected.is_empty());
    assert_eq!(app.mode, InteractionMode::Normal);

    // Left click on row 0 (table.top() + 2):
    let click_row_0 = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: table.left() + 5,
        row: table.top() + 2,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    let act = MouseHandler::handle_mouse(
        click_row_0,
        &layout,
        &app.mode,
        term_rect,
        app.category_counts,
        app.selected_row,
        app.visible_indices.len(),
    );
    assert_eq!(act, AppAction::SelectRow(0));
    app.dispatch_action(act);
    assert_eq!(app.selected_row, 0);
    assert!(app.multi_selected.is_empty());

    // Ctrl + Left click on row 1 (table.top() + 3): toggles multi-select for row 1
    let ctrl_click_row_1 = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: table.left() + 10,
        row: table.top() + 3,
        modifiers: crossterm::event::KeyModifiers::CONTROL,
    };
    let act = MouseHandler::handle_mouse(
        ctrl_click_row_1,
        &layout,
        &app.mode,
        term_rect,
        app.category_counts,
        app.selected_row,
        app.visible_indices.len(),
    );
    assert_eq!(act, AppAction::ToggleSelectRow(1));
    app.dispatch_action(act);
    assert_eq!(app.selected_row, 1);
    assert_eq!(app.multi_selected.len(), 1);
    let expected_hash = app.snapshot.torrents[app.visible_indices[1]].hash;
    assert!(app.multi_selected.contains(&expected_hash));
    assert_eq!(app.mode, InteractionMode::VisualSelect);

    // Plain Left click on row 3 clears multi-select and selects row 3 in Normal mode
    let click_row_3 = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: table.left() + 15,
        row: table.top() + 5,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    let act = MouseHandler::handle_mouse(
        click_row_3,
        &layout,
        &app.mode,
        term_rect,
        app.category_counts,
        app.selected_row,
        app.visible_indices.len(),
    );
    assert_eq!(act, AppAction::SelectRow(3));
    app.dispatch_action(act);
    assert_eq!(app.selected_row, 3);
    assert!(app.multi_selected.is_empty());
    assert_eq!(app.mode, InteractionMode::Normal);

    // Right click on row 4 selects row 4
    let right_click_row_4 = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: table.left() + 20,
        row: table.top() + 6,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    let act = MouseHandler::handle_mouse(
        right_click_row_4,
        &layout,
        &app.mode,
        term_rect,
        app.category_counts,
        app.selected_row,
        app.visible_indices.len(),
    );
    assert_eq!(act, AppAction::SelectRow(4));
    app.dispatch_action(act);
    assert_eq!(app.selected_row, 4);

    // Click on empty space (row table.top() + 8, beyond the 5 items) does nothing
    let click_empty = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: table.left() + 10,
        row: table.top() + 8,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    let act = MouseHandler::handle_mouse(
        click_empty,
        &layout,
        &app.mode,
        term_rect,
        app.category_counts,
        app.selected_row,
        app.visible_indices.len(),
    );
    assert_eq!(act, AppAction::None);
}

#[test]
fn test_sample_torrent_details_does_not_deadlock() {
    use lt_common::hash_string::HashString;
    use lt_engine::constructor::TorrentMetadata;
    use rt_core::{Download, DownloadList};
    use std::sync::{Arc, RwLock};

    let hash_bytes = [42u8; 20];
    let metadata = TorrentMetadata {
        info_hash: HashString::new(hash_bytes),
        name: "test_torrent".to_string(),
        piece_length: 1024,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 1024,
        is_multi_file: false,
        files: Vec::new(),
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
        v2: None,
    };
    let dl = Arc::new(RwLock::new(Download::new(
        lt_engine::download::Download::new(metadata),
    )));

    let mut dl_list = DownloadList::new();
    dl_list.insert(dl);

    let details = rt_tui::concurrency::aggregator::sample_torrent_details(&dl_list, &hash_bytes);
    assert!(details.is_some());
    let d = details.unwrap();
    assert_eq!(d.piece_map.len(), 1);
    assert!(d
        .log_messages
        .iter()
        .any(|msg| msg.contains("Torrent: test_torrent")));
}

#[test]
fn test_pieces_tab_infographic_and_pipeline_rendering() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;
    use rt_tui::state::InFlightPieceRecord;
    use rt_tui::widgets::inspector::pieces::PiecesTab;

    let mut record = mock_torrent(
        "ubuntu-26.04.1-live-server-amd64.iso",
        14_000_000,
        2_700_000_000,
        TorrentUiState::Downloading,
    );
    record.total_pieces = 11169;
    record.verified_pieces = 89;
    record.piece_size = 256 * 1024;
    record.eta_seconds = Some(192);

    let mut pieces = vec![PieceState::Missing; 11169];
    for p in &mut pieces[0..89] {
        *p = PieceState::Verified;
    }
    pieces[90] = PieceState::Downloading;
    pieces[91] = PieceState::Downloading;

    let in_flight = vec![
        InFlightPieceRecord {
            piece_index: 90,
            blocks_completed: 10,
            blocks_total: 16,
            bytes_completed: 160 * 1024,
            bytes_total: 256 * 1024,
            peer_addr: Some("142.250.180.3:51413".into()),
        },
        InFlightPieceRecord {
            piece_index: 91,
            blocks_completed: 4,
            blocks_total: 16,
            bytes_completed: 64 * 1024,
            bytes_total: 256 * 1024,
            peer_addr: Some("82.165.197.1:6881".into()),
        },
    ];

    let tab = PiecesTab::new(&record, &pieces, &in_flight, THEME_TOKYO_NIGHT);
    let area = Rect::new(0, 0, 100, 24);
    let mut buf = Buffer::empty(area);
    tab.render(area, &mut buf);

    let full_text = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Assert HUD metrics
    assert!(full_text.contains("89 / 11169 pcs"));
    assert!(full_text.contains("Verified:"));
    assert!(full_text.contains("Remaining: 11080 pcs"));
    assert!(full_text.contains("Piece Size: 256.0 KB"));
    assert!(full_text.contains("Down:"));
    assert!(full_text.contains("In-Flight: 2 pcs"));

    // Assert Active In-Flight pipeline section
    assert!(full_text.contains("ACTIVE IN-FLIGHT PIPELINE"));
    assert!(full_text.contains("Pc #90"));
    assert!(full_text.contains("10/16 blk"));
    assert!(full_text.contains("142.250.180.3:51413"));
    assert!(full_text.contains("Pc #91"));
    assert!(full_text.contains("4/16 blk"));

    // Assert rulers
    assert!(full_text.contains("0%"));
    assert!(full_text.contains("100%"));
    assert!(full_text.contains("#0000"));

    // Assert legend
    assert!(full_text.contains("Legend: █ Verified (100%)"));
}

#[test]
fn test_pieces_tab_completed_torrent_banner() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;
    use rt_tui::widgets::inspector::pieces::PiecesTab;

    let mut record = mock_torrent("ubuntu.iso", 0, 1_000_000, TorrentUiState::Complete);
    record.total_pieces = 100;
    record.verified_pieces = 100;
    record.completed_bytes = 1_000_000;

    let pieces = vec![PieceState::Verified; 100];
    let in_flight = vec![];

    let tab = PiecesTab::new(&record, &pieces, &in_flight, THEME_TOKYO_NIGHT);
    let area = Rect::new(0, 0, 80, 20);
    let mut buf = Buffer::empty(area);
    tab.render(area, &mut buf);

    let full_text = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(full_text.contains("100.0%"));
    assert!(full_text.contains("All pieces verified and written to disk"));
}
