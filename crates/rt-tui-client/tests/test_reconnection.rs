//! TDD contract for TASK-15 — reconnection and connection health.

mod common;

use std::time::Duration;

use common::{spawn_tcp_daemon, MockDaemon};
use rt_tui_client::app::App;
use rt_tui_client::bridge::GrpcBridge;
use rt_tui_client::connection::{
    ConnectionManager, ConnectionState, HEALTH_CHECK_INTERVAL, MAX_BACKOFF,
};
use rt_tui_client::state::{AppState, ConnectionStatus};

const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn backoff_increases_exponentially() {
    let mut mgr = ConnectionManager::new();
    assert_eq!(mgr.next_backoff(), Duration::from_secs(1));
    mgr.record_failure();
    assert_eq!(mgr.next_backoff(), Duration::from_secs(2));
    mgr.record_failure();
    assert_eq!(mgr.next_backoff(), Duration::from_secs(4));
}

#[test]
fn backoff_caps_at_max() {
    let mut mgr = ConnectionManager::new();
    for _ in 0..10 {
        mgr.record_failure();
    }
    assert!(mgr.next_backoff() <= Duration::from_secs(30));
}

#[test]
fn successful_reconnect_resets_backoff() {
    let mut mgr = ConnectionManager::new();
    mgr.record_failure();
    mgr.record_failure();
    mgr.record_success();
    assert_eq!(mgr.next_backoff(), Duration::from_secs(1));
}

#[test]
fn state_transitions() {
    let mut mgr = ConnectionManager::new();
    assert_eq!(mgr.state(), ConnectionState::Disconnected);
    mgr.record_success();
    assert_eq!(mgr.state(), ConnectionState::Connected);
    mgr.record_failure();
    assert_eq!(mgr.state(), ConnectionState::Reconnecting);
}

// ---------------------------------------------------------------------------
// Backoff details
// ---------------------------------------------------------------------------

#[test]
fn backoff_sequence_is_one_two_four_eight_sixteen_thirty() {
    let mut mgr = ConnectionManager::new();
    let mut observed = Vec::new();
    for _ in 0..7 {
        observed.push(mgr.next_backoff().as_secs());
        mgr.record_failure();
    }

    assert_eq!(observed, vec![1, 2, 4, 8, 16, 30, 30]);
    assert_eq!(MAX_BACKOFF, Duration::from_secs(30));
}

#[test]
fn failures_are_counted() {
    let mut mgr = ConnectionManager::new();
    mgr.record_failure();
    mgr.record_failure();

    assert_eq!(mgr.failures(), 2);
}

#[test]
fn health_checks_run_every_ten_seconds() {
    assert_eq!(HEALTH_CHECK_INTERVAL, Duration::from_secs(10));

    let mut mgr = ConnectionManager::new();
    let start = std::time::Instant::now();
    assert!(mgr.health_due(start), "first health check runs immediately");
    mgr.mark_health_checked(start);
    assert!(!mgr.health_due(start + Duration::from_secs(9)));
    assert!(mgr.health_due(start + Duration::from_secs(10)));
}

// ---------------------------------------------------------------------------
// App integration
// ---------------------------------------------------------------------------

fn app_with(seeded: AppState, endpoint: &str) -> App {
    App::new_headless(seeded).with_endpoint(endpoint)
}

#[tokio::test]
async fn failed_refresh_moves_the_state_to_reconnecting() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        error: Some(tonic::Status::unavailable("daemon gone")),
        ..MockDaemon::default()
    })
    .await;
    let bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut app = App::new(AppState::new(), bridge, SHORT_TIMEOUT);

    let outcome = app.refresh().await.expect("bridge attached");

    assert!(outcome.is_err());
    assert!(matches!(
        app.state.connection,
        ConnectionStatus::Reconnecting
    ));
    assert!(
        app.state.reconnect_in.is_some(),
        "countdown must be published"
    );
    assert!(app.state.stale, "data must be marked stale");
}

#[tokio::test]
async fn successful_refresh_clears_the_reconnection_state() {
    let endpoint = spawn_tcp_daemon(MockDaemon::default()).await;
    let bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut app = App::new(AppState::new(), bridge, SHORT_TIMEOUT);

    let _ = app.refresh().await;
    let _ = app.refresh().await;

    assert_eq!(app.state.connection, ConnectionStatus::Connected);
    assert_eq!(app.state.reconnect_in, None);
    assert!(!app.state.stale);
}

#[tokio::test]
async fn health_check_reports_failure_while_the_daemon_is_down() {
    let mut app = app_with(AppState::new(), "127.0.0.1:1");

    let ok = app.check_health().await;

    assert_eq!(ok, Some(false));
    assert!(matches!(
        app.state.connection,
        ConnectionStatus::Reconnecting
    ));
}

#[test]
fn status_bar_shows_the_reconnection_countdown() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    let mut state = AppState::new();
    state.connection = ConnectionStatus::Reconnecting;
    state.reconnect_in = Some(3);
    let area = Rect::new(0, 0, 90, 1);
    let mut buffer = Buffer::empty(area);

    rt_tui_client::widgets::StatusBarWidget::new(&state).render(area, &mut buffer);

    let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("reconnecting"), "rendered: {text}");
    assert!(text.contains('3'), "countdown missing: {text}");
}
