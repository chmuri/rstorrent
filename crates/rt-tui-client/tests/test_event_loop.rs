//! TDD contract for TASK-04 — event loop, terminal backend and app shell.

mod common;

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use common::{spawn_tcp_daemon, MockDaemon};
use rt_tui_client::app::App;
use rt_tui_client::bridge::GrpcBridge;
use rt_tui_client::events::{AppEvent, EventPushOutcome, EventQueue, EventQueueConfig};
use rt_tui_client::state::{AppState, ConnectionStatus};

const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

fn render_to_string(app: &App, cols: u16, rows: u16) -> String {
    let backend = TestBackend::new(cols, rows);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| app.draw(frame)).expect("draw frame");

    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn app_quits_on_ctrl_q() {
    let mut app = App::new_headless(AppState::new()); // no terminal
    app.handle_key(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert!(app.should_quit);
}

#[test]
fn app_double_ctrl_q_sets_force_quit() {
    let mut app = App::new_headless(AppState::new());
    app.handle_key(KeyCode::Char('q'), KeyModifiers::CONTROL);
    app.handle_key(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert!(app.force_quit);
}

#[test]
fn app_resize_updates_state_dimensions() {
    let mut app = App::new_headless(AppState::new());
    app.handle_resize(120, 40);
    assert_eq!(app.state.terminal_cols, 120);
    assert_eq!(app.state.terminal_rows, 40);
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

#[test]
fn plain_q_does_not_quit() {
    let mut app = App::new_headless(AppState::new());

    app.handle_key(KeyCode::Char('q'), KeyModifiers::NONE);

    assert!(!app.should_quit);
    assert!(!app.force_quit);
}

#[test]
fn ctrl_q_followed_by_other_keys_stays_a_graceful_quit() {
    let mut app = App::new_headless(AppState::new());

    app.handle_key(KeyCode::Char('q'), KeyModifiers::CONTROL);
    app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);

    assert!(app.should_quit);
    assert!(!app.force_quit);
}

#[test]
fn key_events_are_dispatched_through_handle_event() {
    let mut app = App::new_headless(AppState::new());

    app.handle_event(AppEvent::key(KeyCode::Char('q'), KeyModifiers::CONTROL));

    assert!(app.should_quit);
}

#[test]
fn resize_events_are_dispatched_through_handle_event() {
    let mut app = App::new_headless(AppState::new());

    app.handle_event(AppEvent::resize(99, 33));

    assert_eq!(app.state.terminal_cols, 99);
    assert_eq!(app.state.terminal_rows, 33);
}

// ---------------------------------------------------------------------------
// Bounded event queue with resize coalescing
// ---------------------------------------------------------------------------

#[test]
fn event_queue_coalesces_resizes_to_the_latest() {
    let mut queue = EventQueue::new(EventQueueConfig::default());

    assert_eq!(
        queue.push(AppEvent::resize(80, 24)),
        EventPushOutcome::Enqueued
    );
    assert_eq!(
        queue.push(AppEvent::resize(100, 30)),
        EventPushOutcome::Coalesced
    );
    assert_eq!(
        queue.push(AppEvent::resize(120, 40)),
        EventPushOutcome::Coalesced
    );

    assert_eq!(queue.len(), 1);
    assert_eq!(queue.coalesced_resizes(), 2);
    assert_eq!(queue.pop(), Some(AppEvent::resize(120, 40)));
    assert_eq!(queue.pop(), None);
}

#[test]
fn event_queue_is_bounded_and_drops_new_events_when_full() {
    let mut queue = EventQueue::new(EventQueueConfig::new(2));

    queue.push(AppEvent::key(KeyCode::Char('a'), KeyModifiers::NONE));
    queue.push(AppEvent::key(KeyCode::Char('b'), KeyModifiers::NONE));
    assert_eq!(
        queue.push(AppEvent::key(KeyCode::Char('c'), KeyModifiers::NONE)),
        EventPushOutcome::Dropped
    );

    assert_eq!(queue.len(), 2);
    assert_eq!(queue.dropped(), 1);
}

#[test]
fn event_queue_with_zero_capacity_drops_everything() {
    let mut queue = EventQueue::new(EventQueueConfig::new(0));

    assert_eq!(
        queue.push(AppEvent::key(KeyCode::Char('a'), KeyModifiers::NONE)),
        EventPushOutcome::Dropped
    );
    assert!(queue.is_empty());
}

// ---------------------------------------------------------------------------
// Refresh scheduling
// ---------------------------------------------------------------------------

#[test]
fn refresh_is_due_before_the_first_fetch_and_after_the_tick() {
    let mut app = App::new_headless(AppState::new()).with_tick(Duration::from_millis(500));
    let start = Instant::now();

    assert!(app.refresh_due(start), "first refresh must run immediately");

    app.mark_refreshed(start);
    assert!(!app.refresh_due(start + Duration::from_millis(499)));
    assert!(app.refresh_due(start + Duration::from_millis(500)));
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[test]
fn draw_renders_title_and_connection_status() {
    let app = App::new_headless(AppState::new());

    let rendered = render_to_string(&app, 60, 10);

    assert!(rendered.contains("rstorrent-tui"), "rendered: {rendered}");
    assert!(rendered.contains("disconnected"), "rendered: {rendered}");
}

#[test]
fn draw_reports_torrent_count_when_connected() {
    let mut state = AppState::new();
    state.connection = ConnectionStatus::Connected;
    let app = App::new_headless(state);

    let rendered = render_to_string(&app, 60, 10);

    assert!(rendered.contains("0 torrents"), "rendered: {rendered}");
}

// ---------------------------------------------------------------------------
// Refresh through the bridge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refresh_updates_state_from_the_daemon() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        summaries: vec![MockDaemon::summary("aaa", "first")],
        ..MockDaemon::default()
    })
    .await;
    let bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut app = App::new(AppState::new(), bridge, Duration::from_millis(10));

    let outcome = app.refresh().await.expect("bridge is attached");

    assert!(outcome.is_ok());
    assert_eq!(app.state.connection, ConnectionStatus::Connected);
    assert_eq!(app.state.torrents.len(), 1);
}

#[tokio::test]
async fn headless_app_has_no_bridge_to_refresh_from() {
    let mut app = App::new_headless(AppState::new());

    assert!(app.refresh().await.is_none());
}
