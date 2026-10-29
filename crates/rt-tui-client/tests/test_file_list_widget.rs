//! TDD contract for TASK-08 — file list view.

mod common;

use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use common::{spawn_tcp_daemon, MockDaemon};
use rt_grpc::{CommandResponse, FileInfo, TorrentDetails, TorrentSummary};
use rt_tui_client::app::{App, PriorityCommand};
use rt_tui_client::bridge::GrpcBridge;
use rt_tui_client::state::{AppState, DetailTab, DetailsState, FileListState};
use rt_tui_client::widgets::file_list::priority_style;
use rt_tui_client::widgets::FileListWidget;

const AREA: Rect = Rect::new(0, 0, 80, 10);
const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

fn make_file_info(index: u32, path: &str, size: i64, completed: i64, priority: u32) -> FileInfo {
    FileInfo {
        index,
        path: path.to_string(),
        size_bytes: size,
        completed_bytes: completed,
        priority,
    }
}

fn details_with(files: Vec<FileInfo>) -> TorrentDetails {
    TorrentDetails {
        summary: Some(TorrentSummary {
            hash: "abc123def".to_string(),
            name: "test.torrent".to_string(),
            ..TorrentSummary::default()
        }),
        files,
        ..TorrentDetails::default()
    }
}

fn buffer_to_string(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

fn render(state: &FileListState, area: Rect) -> String {
    let mut buffer = Buffer::empty(area);
    FileListWidget::new(state).render(area, &mut buffer);
    buffer_to_string(&buffer)
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn renders_file_path_and_priority() {
    let files = vec![
        make_file_info(0, "data/file1.txt", 1024, 512, 1),
        make_file_info(1, "data/file2.txt", 2048, 2048, 2),
    ];
    let state = FileListState::from_proto(&files);
    let widget = FileListWidget::new(&state);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
    widget.render(Rect::new(0, 0, 80, 10), &mut buf);
    let text = buffer_to_string(&buf);
    assert!(text.contains("file1.txt"));
    assert!(text.contains("file2.txt"));
    assert!(text.contains("Normal"), "priority label missing: {text}");
    assert!(text.contains("High"), "priority label missing: {text}");
}

#[test]
fn priority_cycles_through_values() {
    // 0 = off, 1 = normal, 2 = high, then back to off.
    assert_eq!(FileListState::next_priority(0), 1); // off -> normal
    assert_eq!(FileListState::next_priority(1), 2); // normal -> high
    assert_eq!(FileListState::next_priority(2), 0); // high -> off
}

// ---------------------------------------------------------------------------
// Priority display
// ---------------------------------------------------------------------------

#[test]
fn priority_labels_and_colours_differ() {
    let state = FileListState::from_proto(&[
        make_file_info(0, "off.bin", 10, 0, 0),
        make_file_info(1, "normal.bin", 10, 0, 1),
        make_file_info(2, "high.bin", 10, 0, 2),
    ]);

    let text = render(&state, AREA);
    for label in ["Off", "Normal", "High"] {
        assert!(text.contains(label), "missing {label}: {text}");
    }

    assert_eq!(
        priority_style(0).fg,
        Some(ratatui::style::Color::Red),
        "off should be red"
    );
    assert_eq!(
        priority_style(1).fg,
        Some(ratatui::style::Color::Green),
        "normal should be green"
    );
    assert_eq!(
        priority_style(2).fg,
        Some(ratatui::style::Color::Yellow),
        "high should be yellow"
    );
}

#[test]
fn rows_show_size_and_completed_percent() {
    let state = FileListState::from_proto(&[make_file_info(0, "half.bin", 2048, 1024, 1)]);

    let text = render(&state, AREA);

    assert!(text.contains("2.0 KB"), "size missing: {text}");
    assert!(text.contains("50%"), "completed missing: {text}");
}

// ---------------------------------------------------------------------------
// Directory rows
// ---------------------------------------------------------------------------

#[test]
fn shared_prefix_becomes_a_synthetic_directory_row() {
    let state = FileListState::from_proto(&[
        make_file_info(0, "data/sub/file1.txt", 1024, 512, 1),
        make_file_info(1, "data/sub/file2.txt", 2048, 2048, 1),
    ]);

    assert!(state.entries[0].is_directory);
    assert_eq!(state.entries[0].path, "data/sub/");
    assert_eq!(state.entries[1].path, "file1.txt");
    assert!(state.entries[1].size_bytes == 1024);
}

#[test]
fn files_without_shared_prefix_have_no_directory_row() {
    let state = FileListState::from_proto(&[
        make_file_info(0, "a.txt", 10, 0, 1),
        make_file_info(1, "b.txt", 10, 0, 1),
    ]);

    assert!(state.entries.iter().all(|entry| !entry.is_directory));
    assert_eq!(state.entries.len(), 2);
}

#[test]
fn focus_starts_on_the_first_file_and_skips_directories() {
    let mut state = FileListState::from_proto(&[
        make_file_info(0, "data/file1.txt", 1024, 512, 1),
        make_file_info(1, "data/file2.txt", 2048, 2048, 1),
    ]);

    assert_eq!(state.focus_index, 1, "directory row must not take focus");

    state.focus_down();
    assert_eq!(state.focus_index, 2);

    state.focus_up();
    assert_eq!(state.focus_index, 1, "focus must skip the directory row");
}

#[test]
fn paging_moves_focus_by_a_page_and_clamps() {
    let files = (0..10)
        .map(|i| make_file_info(i, &format!("f{i}.bin"), 10, 0, 1))
        .collect::<Vec<_>>();
    let mut state = FileListState::from_proto(&files);

    state.focus_page_down(4);
    assert_eq!(state.focus_index, 4);

    state.focus_page_down(4);
    assert_eq!(state.focus_index, 8);

    state.focus_page_down(4);
    assert_eq!(state.focus_index, 9, "must clamp at the last file");

    state.focus_page_up(4);
    assert_eq!(state.focus_index, 5);
}

// ---------------------------------------------------------------------------
// Priority commands
// ---------------------------------------------------------------------------

fn app_with_files(files: Vec<FileInfo>) -> App {
    let mut state = AppState::new();
    let mut details = DetailsState::from_proto(&details_with(files));
    details.active_tab = DetailTab::Files;
    state.details = Some(details);

    App::new_headless(state)
}

#[test]
fn space_cycles_the_focused_files_priority_and_requests_a_command() {
    let mut app = app_with_files(vec![
        make_file_info(0, "data/file1.txt", 1024, 512, 1),
        make_file_info(1, "data/file2.txt", 2048, 2048, 1),
    ]);

    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);

    let details = app.state.details.as_ref().expect("details open");
    let focused = details.files.focused().expect("a file is focused");
    assert_eq!(focused.priority, 2, "normal -> high");
    assert_eq!(focused.index, 0);

    let command = app.take_priority_command().expect("command requested");
    assert_eq!(
        command,
        PriorityCommand {
            file_index: 0,
            priority: 2,
            hash: "abc123def".to_string(),
        }
    );
    assert_eq!(app.take_priority_command(), None, "request is consumed once");
}

#[test]
fn star_also_cycles_priority() {
    let mut app = app_with_files(vec![make_file_info(0, "a.bin", 10, 0, 2)]);

    app.handle_key(KeyCode::Char('*'), KeyModifiers::NONE);

    let details = app.state.details.as_ref().expect("details open");
    assert_eq!(
        details.files.focused().map(|file| file.priority),
        Some(0),
        "high -> off"
    );
}

#[test]
fn space_on_a_directory_row_does_nothing() {
    let mut app = app_with_files(vec![
        make_file_info(0, "data/file1.txt", 1024, 512, 1),
        make_file_info(1, "data/file2.txt", 2048, 2048, 1),
    ]);
    if let Some(details) = app.state.details.as_mut() {
        details.files.focus_index = 0; // the synthetic directory row
    }

    app.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);

    assert_eq!(app.take_priority_command(), None);
}

#[test]
fn arrow_keys_move_the_file_focus_in_the_files_tab() {
    let mut app = app_with_files(vec![
        make_file_info(0, "data/file1.txt", 10, 0, 1),
        make_file_info(1, "data/file2.txt", 10, 0, 1),
    ]);

    app.handle_key(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        app.state
            .details
            .as_ref()
            .map(|details| details.files.focus_index),
        Some(2)
    );

    app.handle_key(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(
        app.state
            .details
            .as_ref()
            .map(|details| details.files.focus_index),
        Some(1)
    );
}

#[tokio::test]
async fn priority_command_is_sent_through_the_bridge() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        execute_response: Some(CommandResponse {
            success: true,
            result: None,
            error_message: String::new(),
        }),
        ..MockDaemon::default()
    })
    .await;
    let bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut app = App::new(AppState::new(), bridge, SHORT_TIMEOUT);
    let command = PriorityCommand {
        file_index: 3,
        priority: 2,
        hash: "abc123def".to_string(),
    };

    let outcome = app
        .send_priority(&command)
        .await
        .expect("bridge attached")
        .expect("command succeeds");

    assert!(outcome.success);
}
