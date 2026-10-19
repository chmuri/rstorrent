use lt_common::HashString;
use lt_engine::constructor::TorrentMetadata;
use rt_core::{Download, DownloadList};
use rt_tui::display::{Canvas, Window};
use rt_tui::ui::{ActiveScreen, RootUI};
use rt_tui::windows::tracker_list::{
    TrackerAction, TrackerActionOutcome, TrackerColumnWidths, TrackerListWindow, TrackerRow,
    TrackerTypeView,
};
use std::sync::{Arc, RwLock};

fn metadata(announce_list: Vec<Vec<String>>, announce: Option<String>) -> TorrentMetadata {
    TorrentMetadata {
        info_hash: HashString::new([9u8; 20]),
        name: "tracker_test".to_string(),
        piece_length: 262144,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 16,
        is_multi_file: false,
        files: Vec::new(),
        announce,
        announce_list,
        is_private: false,
        is_meta_download: false,
    }
}

fn make_download(announce_list: Vec<Vec<String>>) -> Arc<RwLock<Download>> {
    let engine = lt_engine::download::Download::new(metadata(announce_list, None));
    Arc::new(RwLock::new(Download::new(engine)))
}

fn http_row(index: usize, group: u32, group_index: usize, url: &str) -> TrackerRow {
    TrackerRow {
        group_index,
        tracker_type: TrackerTypeView::Http,
        ..TrackerRow::new(index, group, url)
    }
}

fn udp_row(index: usize, group: u32, group_index: usize, url: &str) -> TrackerRow {
    TrackerRow {
        group_index,
        tracker_type: TrackerTypeView::Udp,
        scrapable: false,
        ..TrackerRow::new(index, group, url)
    }
}

#[test]
fn test_http_udp_and_disabled_rows_render_with_groups() {
    let mut window = TrackerListWindow::new();
    window.set_download_name("sample");
    window.set_trackers(vec![
        http_row(0, 0, 0, "http://tracker.example.com/announce"),
        udp_row(1, 0, 1, "udp://tracker.example.org:6969/announce"),
        TrackerRow {
            enabled: false,
            usable: false,
            latest_event: 2,
            next_announce_in: Some(90),
            success_counter: 2,
            failed_counter: 1,
            scrape_counter: 1,
            scrape_complete: 12,
            scrape_incomplete: 3,
            scrape_downloaded: 7,
            ..http_row(2, 1, 0, "http://backup.example.net/announce")
        },
    ]);

    let mut canvas = Canvas::new(0, 0, 110, 20);
    window.redraw(&mut canvas);
    let lines = canvas.to_string_rows();

    assert!(lines[0].contains("[Trackers: sample]"));
    assert!(lines[0].contains("3"));
    assert!(lines[1].contains("Grp"));
    assert!(lines[1].contains("URL"));
    assert!(lines[1].contains("S/I/D"));

    // Group 0 contains two trackers; only the first row repeats the group.
    assert!(lines[2].contains("0:0"));
    assert!(lines[2].contains("HTTP"));
    assert!(lines[2].contains("on"));
    assert!(lines[3].contains(":1"));
    assert!(lines[3].contains("UDP"));
    assert!(lines[2].contains("http://tracker.example.com/announce"));

    // Disabled trackers still render their last known counters.
    assert!(lines[4].contains("1:0"));
    assert!(lines[4].contains("off"));
    assert!(lines[4].contains("started"));
    assert!(lines[4].contains("12/3/7"));
    assert!(lines[4].contains("2/1"));
    assert!(lines[4].contains("1m"));
}

#[test]
fn test_url_truncation_keeps_host_visible() {
    let mut window = TrackerListWindow::new();
    window.set_download_name("narrow");
    window.set_trackers(vec![http_row(
        0,
        0,
        0,
        "http://a.co/this/is/a/very/long/announce/path/that/does/not/fit",
    )]);

    let widths = TrackerColumnWidths::for_width(44);
    assert!(widths.url >= 11);
    let line = window.rows[0].render_line(&widths, false);
    assert!(line.contains("http://a.co"));
    // The host is kept and the path is replaced with an ellipsis.
    assert!(!line.contains("/very/long"));

    let mut canvas = Canvas::new(0, 0, 44, 8);
    window.redraw(&mut canvas);
    let rows = canvas.to_string_rows();
    assert!(rows[2].contains("http://a.co"));
}

#[test]
fn test_navigation_wraps_and_pages() {
    let mut window = TrackerListWindow::new();
    window.set_trackers(
        (0..6)
            .map(|i| http_row(i, 0, i, &format!("http://t{}.example.com/announce", i)))
            .collect(),
    );
    window.resize(0, 0, 80, 4); // two visible data rows

    assert_eq!(window.selected_index(), 0);
    window.select_next();
    assert_eq!(window.selected_index(), 1);
    window.select_prev();
    assert_eq!(window.selected_index(), 0);
    window.select_prev();
    assert_eq!(window.selected_index(), 5);
    window.select_next();
    assert_eq!(window.selected_index(), 0);

    window.page_down();
    assert_eq!(window.selected_index(), 2);
    window.page_down();
    assert_eq!(window.selected_index(), 4);
    window.page_down();
    assert_eq!(window.selected_index(), 5);
    window.page_up();
    assert_eq!(window.selected_index(), 3);

    assert!(window.select_index(4));
    assert!(!window.select_index(6));
    window.select_last();
    assert_eq!(window.selected_index(), 5);
    window.select_first();
    assert_eq!(window.selected_index(), 0);
}

#[test]
fn test_tracker_action_key_bindings() {
    assert_eq!(
        TrackerAction::for_key(b' ' as i32),
        Some(TrackerAction::ToggleEnabled)
    );
    assert_eq!(
        TrackerAction::for_key(b'*' as i32),
        Some(TrackerAction::ToggleEnabled)
    );
    assert_eq!(
        TrackerAction::for_key(b'a' as i32),
        Some(TrackerAction::ForceAnnounce)
    );
    assert_eq!(
        TrackerAction::for_key(b't' as i32),
        Some(TrackerAction::ForceAnnounce)
    );
    assert_eq!(
        TrackerAction::for_key(b's' as i32),
        Some(TrackerAction::ForceScrape)
    );
    assert_eq!(TrackerAction::for_key(b'x' as i32), None);
}

#[test]
fn test_live_download_snapshot_and_actions() {
    let download = make_download(vec![
        vec!["http://tracker.example.com/announce".to_string()],
        vec!["udp://tracker.example.org:6969/announce".to_string()],
    ]);

    let mut window = TrackerListWindow::new();
    window.set_download(Some(Arc::clone(&download)));

    assert_eq!(window.len(), 2);
    assert_eq!(window.rows[0].tracker_type, TrackerTypeView::Http);
    assert_eq!(window.rows[1].tracker_type, TrackerTypeView::Udp);
    assert!(window.rows[0].enabled);
    assert!(window.rows[0].usable);

    // Tracker indices can shift between snapshot and action; actions re-resolve
    // the live entry by group + URL.
    window.rows[0].index = 99;

    // t.disable / t.enable are used for the toggle action.
    assert_eq!(
        window.apply_selected_action(TrackerAction::ToggleEnabled),
        TrackerActionOutcome::Applied
    );
    assert_eq!(window.rows[0].state_label(), "off");
    assert_eq!(
        window.apply_selected_action(TrackerAction::ToggleEnabled),
        TrackerActionOutcome::Applied
    );
    assert!(window.rows[0].enabled);

    // The selected HTTP tracker accepts a forced announce and scrape.
    assert_eq!(
        window.apply_selected_action(TrackerAction::ForceAnnounce),
        TrackerActionOutcome::Applied
    );
    assert!(window.rows[0].busy);
    assert_eq!(window.rows[0].state_label(), "req");

    assert_eq!(
        window.apply_selected_action(TrackerAction::ForceScrape),
        TrackerActionOutcome::Applied
    );
    assert_eq!(window.rows[0].state_label(), "scr");

    // BEP 15 UDP trackers support scrape natively.
    assert!(window.select_index(1));
    assert_eq!(
        window.apply_selected_action(TrackerAction::ForceScrape),
        TrackerActionOutcome::Applied
    );
    assert_eq!(window.rows[1].state_label(), "scr");
}

#[test]
fn test_no_download_and_no_tracker_messages() {
    let mut window = TrackerListWindow::new();
    let mut canvas = Canvas::new(0, 0, 60, 8);
    window.redraw(&mut canvas);
    assert!(canvas.to_string_rows()[0].contains("[No download selected]"));

    let download = make_download(Vec::new());
    window.set_download(Some(download));
    assert!(window.is_empty());

    let mut canvas = Canvas::new(0, 0, 60, 8);
    window.redraw(&mut canvas);
    let lines = canvas.to_string_rows();
    assert!(lines[0].contains("[Trackers: tracker_test]"));
    assert!(lines[1].contains("[No trackers configured]"));
}

#[test]
fn test_very_narrow_terminal_does_not_panic() {
    let mut window = TrackerListWindow::new();
    window.set_download_name("tiny");
    window.set_trackers(vec![http_row(
        0,
        0,
        0,
        "http://tracker.example.com/announce",
    )]);

    for width in [8u32, 12, 20, 30] {
        let mut canvas = Canvas::new(0, 0, width, 5);
        window.redraw(&mut canvas);
        let lines = canvas.to_string_rows();
        assert_eq!(lines.len(), 5);
    }
}

#[test]
fn test_root_ui_tracker_view_integration() {
    let download = make_download(vec![vec![
        "http://tracker.example.com/announce".to_string(),
        "udp://tracker.example.org:6969/announce".to_string(),
    ]]);

    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));

    let mut ui = RootUI::new();
    ui.set_download_list(Arc::new(RwLock::new(list)));

    ui.handle_key(b'o' as i32);
    assert_eq!(ui.active_screen, ActiveScreen::TrackerList);
    assert_eq!(ui.window_tracker_list.lock().unwrap().len(), 2);

    let mut canvas = Canvas::new(0, 0, 90, 24);
    ui.balance_and_redraw(&mut canvas);
    let lines = canvas.to_string_rows();
    assert!(lines[1].contains("[Trackers: tracker_test]"));
    assert!(lines[3].contains("HTTP"));
    assert!(lines[4].contains("UDP"));

    // Navigation is routed to the tracker window while it is active.
    ui.handle_key(b'j' as i32);
    assert_eq!(ui.window_tracker_list.lock().unwrap().selected_index(), 1);

    // Close with 'o', then reopen with the file-list-safe 'T' binding.
    ui.handle_key(b'o' as i32);
    assert_eq!(ui.active_screen, ActiveScreen::DownloadList);
    ui.handle_key(b'T' as i32);
    assert_eq!(ui.active_screen, ActiveScreen::TrackerList);
}
