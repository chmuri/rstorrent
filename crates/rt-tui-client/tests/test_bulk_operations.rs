//! Integration tests for bulk operations across torrents and files.

mod common;

use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};

use common::{spawn_tcp_daemon, MockDaemon};
use rt_grpc::{FileInfo, TorrentDetails, TorrentSummary};
use rt_tui_client::actions::{TorrentAction, TuiAction};
use rt_tui_client::app::App;
use rt_tui_client::bridge::GrpcBridge;
use rt_tui_client::state::{AppState, DetailTab, DetailsState, TorrentViewModel};

const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

fn make_vm(hash: &str, name: &str, active: bool, priority: u32) -> TorrentViewModel {
    TorrentViewModel::from_proto(&TorrentSummary {
        hash: hash.to_string(),
        name: name.to_string(),
        is_active: active,
        priority,
        ..TorrentSummary::default()
    })
}

fn app_with_three_torrents() -> App {
    let mut state = AppState::new();
    state.torrents = vec![
        make_vm("hash1", "torrent1", true, 2),
        make_vm("hash2", "torrent2", false, 1),
        make_vm("hash3", "torrent3", true, 3),
    ];
    App::new_headless(state)
}

#[test]
fn space_toggles_selection_of_focused_torrent() {
    let mut app = app_with_three_torrents();

    assert_eq!(app.state.selection_count(), 0);

    // Focus is on row 0 ("hash1")
    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.state.selection_count(), 1);
    assert!(app.state.is_selected("hash1"));

    // Toggle again removes selection
    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.state.selection_count(), 0);
    assert!(!app.state.is_selected("hash1"));
}

#[test]
fn ctrl_a_selects_all_and_esc_clears_selection() {
    let mut app = app_with_three_torrents();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(app.state.selection_count(), 3);
    assert!(app.state.is_selected("hash1"));
    assert!(app.state.is_selected("hash2"));
    assert!(app.state.is_selected("hash3"));

    // Esc clears selection
    app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(app.state.selection_count(), 0);
}

#[test]
fn star_inverts_selection() {
    let mut app = app_with_three_torrents();

    // Select hash1
    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.state.selection_count(), 1);

    // Invert -> hash2 and hash3 should be selected, hash1 unselected
    app.handle_key(KeyCode::Char('*'), KeyModifiers::NONE);
    assert_eq!(app.state.selection_count(), 2);
    assert!(!app.state.is_selected("hash1"));
    assert!(app.state.is_selected("hash2"));
    assert!(app.state.is_selected("hash3"));

    // Invert with SHIFT (standard keyboard layout: Shift+8 produces '*')
    app.handle_key(KeyCode::Char('*'), KeyModifiers::SHIFT);
    assert_eq!(app.state.selection_count(), 1);
    assert!(app.state.is_selected("hash1"));
    assert!(!app.state.is_selected("hash2"));
    assert!(!app.state.is_selected("hash3"));
}

#[test]
fn ctrl_capital_a_and_uppercase_ctrl_shortcuts() {
    let mut app = app_with_three_torrents();

    // Ctrl+A with uppercase 'A'
    app.handle_key(KeyCode::Char('A'), KeyModifiers::CONTROL);
    assert_eq!(app.state.selection_count(), 3);

    // Ctrl+S with uppercase 'S'
    let action = app.handle_key(KeyCode::Char('S'), KeyModifiers::CONTROL);
    match action {
        Some(TuiAction::BulkControlTorrents { hashes, action }) => {
            assert_eq!(hashes.len(), 3);
            assert_eq!(action, TorrentAction::Start);
        }
        other => panic!("expected BulkControlTorrents, got {other:?}"),
    }

    // Ctrl+K with uppercase 'K'
    let action = app.handle_key(KeyCode::Char('K'), KeyModifiers::CONTROL);
    match action {
        Some(TuiAction::BulkControlTorrents { hashes, action }) => {
            assert_eq!(hashes.len(), 3);
            assert_eq!(action, TorrentAction::Close);
        }
        other => panic!("expected BulkControlTorrents, got {other:?}"),
    }
}

#[test]
fn bulk_start_action_when_items_selected() {
    let mut app = app_with_three_torrents();

    // Select all torrents
    app.handle_key(KeyCode::Char('a'), KeyModifiers::CONTROL);

    // Ctrl+S produces BulkControlTorrents
    let action = app.handle_key(KeyCode::Char('s'), KeyModifiers::CONTROL);
    match action {
        Some(TuiAction::BulkControlTorrents { hashes, action }) => {
            assert_eq!(hashes.len(), 3);
            assert_eq!(action, TorrentAction::Start);
        }
        other => panic!("expected BulkControlTorrents, got {other:?}"),
    }
}

#[test]
fn bulk_stop_and_remove_action() {
    let mut app = app_with_three_torrents();

    // Select all (contains active and stopped torrents)
    app.handle_key(KeyCode::Char('a'), KeyModifiers::CONTROL);

    // Since some are active, Ctrl+D produces BulkControlTorrents::Stop
    let action = app.handle_key(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert!(matches!(
        action,
        Some(TuiAction::BulkControlTorrents {
            action: TorrentAction::Stop,
            ..
        })
    ));

    // Clear selection, select only hash2 (which is stopped)
    app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
    app.state.focus_index = 1; // row with hash2
    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.state.selection_count(), 1);

    // Since hash2 is stopped and only it is selected, Ctrl+D produces BulkRemoveTorrents
    let action = app.handle_key(KeyCode::Char('d'), KeyModifiers::CONTROL);
    match action {
        Some(TuiAction::BulkRemoveTorrents { hashes }) => {
            assert_eq!(hashes, vec!["hash2".to_string()]);
        }
        other => panic!("expected BulkRemoveTorrents, got {other:?}"),
    }
}

#[test]
fn bulk_priority_change() {
    let mut app = app_with_three_torrents();

    // Select all
    app.handle_key(KeyCode::Char('a'), KeyModifiers::CONTROL);

    // '+' produces BulkSetPriority
    let action = app.handle_key(KeyCode::Char('+'), KeyModifiers::NONE);
    match action {
        Some(TuiAction::BulkSetPriority { hashes, priority }) => {
            assert_eq!(hashes.len(), 3);
            assert_eq!(priority, 3); // row 0 priority was 2 -> next is 3
        }
        other => panic!("expected BulkSetPriority, got {other:?}"),
    }
}

#[test]
fn bulk_menu_entries_displayed_and_activated() {
    let mut app = app_with_three_torrents();

    // Select 2 torrents
    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);
    app.state.focus_index = 1;
    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.state.selection_count(), 2);

    // Open menu with 'm'
    app.handle_key(KeyCode::Char('m'), KeyModifiers::NONE);
    let menu = app.state.menu.as_ref().expect("menu is open");
    let labels: Vec<&str> = menu.entries().iter().map(|e| e.label.as_str()).collect();

    assert!(labels.contains(&"Start (2 torrents)"));
    assert!(labels.contains(&"Stop (2 torrents)"));
    assert!(labels.contains(&"Close (2 torrents)"));
    assert!(labels.contains(&"Check Hash (2 torrents)"));
    assert!(labels.contains(&"Priority High (2 torrents)"));
    assert!(labels.contains(&"Remove (2 torrents)"));

    // Press Enter to activate the first entry ("Start (2 torrents)")
    let action = app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
    match action {
        Some(TuiAction::BulkControlTorrents { hashes, action }) => {
            assert_eq!(hashes.len(), 2);
            assert_eq!(action, TorrentAction::Start);
        }
        other => panic!("expected BulkControlTorrents::Start, got {other:?}"),
    }
}

#[test]
fn files_tab_selection_and_bulk_priority() {
    let mut state = AppState::new();
    let details = TorrentDetails {
        summary: Some(TorrentSummary {
            hash: "testhash".to_string(),
            name: "torrent".to_string(),
            ..TorrentSummary::default()
        }),
        files: vec![
            FileInfo {
                index: 0,
                path: "file0.txt".to_string(),
                priority: 1, // normal
                ..FileInfo::default()
            },
            FileInfo {
                index: 1,
                path: "file1.txt".to_string(),
                priority: 1, // normal
                ..FileInfo::default()
            },
        ],
        ..TorrentDetails::default()
    };
    let mut details_state = DetailsState::from_proto(&details);
    details_state.active_tab = DetailTab::Files;
    state.details = Some(details_state);

    let mut app = App::new_headless(state);

    // Select file 0 with 'x'
    app.handle_key(KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(
        app.state.details.as_ref().unwrap().files.selection_count(),
        1
    );

    // Select all files with Ctrl+A
    app.handle_key(KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(
        app.state.details.as_ref().unwrap().files.selection_count(),
        2
    );

    // Press Space to cycle priority for all selected files
    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);

    // Both files should have priority 2 (normal -> high)
    let files = &app.state.details.as_ref().unwrap().files;
    assert_eq!(files.entries[0].priority, 2);
    assert_eq!(files.entries[1].priority, 2);

    // Priority commands queued for both files
    let cmd1 = app.take_priority_command().expect("cmd 1");
    let cmd2 = app.take_priority_command().expect("cmd 2");
    assert_eq!(cmd1.priority, 2);
    assert_eq!(cmd2.priority, 2);
    assert_eq!(app.take_priority_command(), None);

    // Esc clears file selection without closing the details pane
    app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.state.details.is_some());
    assert_eq!(
        app.state.details.as_ref().unwrap().files.selection_count(),
        0
    );
}

#[tokio::test]
async fn grpc_bridge_bulk_methods() {
    let endpoint = spawn_tcp_daemon(MockDaemon::default()).await;
    let mut bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");

    let hashes = vec!["hashA".to_string(), "hashB".to_string()];

    // bulk control
    let count = bridge
        .bulk_control_torrents(&hashes, TorrentAction::Start)
        .await
        .expect("bulk control succeeds");
    assert_eq!(count, 2);

    // bulk set priority
    let count = bridge
        .bulk_set_torrent_priority(&hashes, 3)
        .await
        .expect("bulk priority succeeds");
    assert_eq!(count, 2);

    // bulk remove
    let count = bridge
        .bulk_remove_torrents(&hashes, false)
        .await
        .expect("bulk remove succeeds");
    assert_eq!(count, 2);
}
