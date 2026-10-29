//! TDD contract for TASK-14 — event streaming and real-time updates.

mod common;

use std::time::Duration;

use common::{spawn_tcp_daemon, MockDaemon};
use rt_grpc::{SessionStatsResponse, TorrentEvent, TorrentSummary};
use rt_tui_client::state::{AppState, TorrentViewModel};
use rt_tui_client::stream::{StreamUpdate, Streams, STREAM_CAPACITY};

const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

fn summary(hash: &str, complete: bool) -> TorrentSummary {
    TorrentSummary {
        hash: hash.to_string(),
        name: format!("{hash}.iso"),
        size_bytes: 1024,
        completed_bytes: if complete { 1024 } else { 512 },
        is_complete: complete,
        ..TorrentSummary::default()
    }
}

fn make_vm_with_hash(hash: &str) -> TorrentViewModel {
    TorrentViewModel::from_proto(&summary(hash, false))
}

fn make_proto_summary_complete() -> TorrentSummary {
    summary("abc123", true)
}

fn make_proto_summary_with_hash(hash: &str) -> TorrentSummary {
    summary(hash, false)
}

fn event(name: &str, hash: &str, summary: Option<TorrentSummary>) -> TorrentEvent {
    TorrentEvent {
        event_name: name.to_string(),
        hash: hash.to_string(),
        summary,
        timestamp: 1_234_567_890,
    }
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn apply_event_updates_torrent_state() {
    let mut state = AppState::new();
    state.torrents = vec![make_vm_with_hash("abc123")];

    let event = TorrentEvent {
        event_name: "event.download.finished".into(),
        hash: "abc123".into(),
        summary: Some(make_proto_summary_complete()),
        timestamp: 1234567890,
    };

    state.apply_event(&event);
    assert!(state.torrents[0].is_complete);
}

#[test]
fn apply_event_for_unknown_hash_adds_torrent() {
    let mut state = AppState::new();
    assert!(state.torrents.is_empty());

    let event = TorrentEvent {
        event_name: "event.download.inserted".into(),
        hash: "new123".into(),
        summary: Some(make_proto_summary_with_hash("new123")),
        timestamp: 1234567890,
    };

    state.apply_event(&event);
    assert_eq!(state.torrents.len(), 1);
    assert_eq!(state.torrents[0].hash, "new123");
}

#[test]
fn apply_stats_updates_rates() {
    let mut state = AppState::new();
    let stats = SessionStatsResponse {
        up_rate_bytes_per_sec: 50_000,
        down_rate_bytes_per_sec: 100_000,
        ..Default::default()
    };
    state.apply_stats(&stats);
    assert_eq!(state.session.up_rate, 50_000);
    assert_eq!(state.session.down_rate, 100_000);
}

// ---------------------------------------------------------------------------
// Event semantics
// ---------------------------------------------------------------------------

#[test]
fn apply_event_replaces_an_existing_row() {
    let mut state = AppState::new();
    state.torrents = vec![make_vm_with_hash("abc123")];

    state.apply_event(&event(
        "event.download.inserted",
        "abc123",
        Some(summary("abc123", true)),
    ));

    assert_eq!(state.torrents.len(), 1, "no duplicate row");
    assert!(state.torrents[0].is_complete);
}

#[test]
fn apply_event_removes_erased_torrents() {
    let mut state = AppState::new();
    state.torrents = vec![make_vm_with_hash("abc123"), make_vm_with_hash("def456")];

    state.apply_event(&event("event.download.erased", "abc123", None));

    assert_eq!(state.torrents.len(), 1);
    assert_eq!(state.torrents[0].hash, "def456");
}

#[test]
fn apply_event_ignores_events_without_summary() {
    let mut state = AppState::new();
    state.torrents = vec![make_vm_with_hash("abc123")];

    state.apply_event(&event("event.download.inserted", "zzz", None));

    assert_eq!(state.torrents.len(), 1);
}

// ---------------------------------------------------------------------------
// Streaming plumbing
// ---------------------------------------------------------------------------

#[test]
fn stream_channel_is_bounded_to_256() {
    assert_eq!(STREAM_CAPACITY, 256);
}

#[tokio::test]
async fn streams_forward_torrent_events() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        events: vec![event(
            "event.download.finished",
            "abc123",
            Some(summary("abc123", true)),
        )],
        ..MockDaemon::default()
    })
    .await;
    let mut streams = Streams::spawn(&endpoint, SHORT_TIMEOUT);

    let update = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(update) = streams.try_recv() {
                return update;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("an event arrives");

    match update {
        StreamUpdate::Event(event) => assert_eq!(event.hash, "abc123"),
        other => panic!("expected an event, got {other:?}"),
    }
}

#[tokio::test]
async fn streams_forward_session_stats() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        stats_stream: vec![SessionStatsResponse {
            up_rate_bytes_per_sec: 1_234,
            ..SessionStatsResponse::default()
        }],
        ..MockDaemon::default()
    })
    .await;
    let mut streams = Streams::spawn(&endpoint, SHORT_TIMEOUT);

    let update = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(update) = streams.try_recv() {
                if matches!(update, StreamUpdate::Stats(_)) {
                    return update;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a stats update arrives");

    match update {
        StreamUpdate::Stats(stats) => assert_eq!(stats.up_rate_bytes_per_sec, 1_234),
        other => panic!("expected stats, got {other:?}"),
    }
}

#[tokio::test]
async fn app_applies_stream_updates() {
    let endpoint = spawn_tcp_daemon(MockDaemon {
        events: vec![event(
            "event.download.inserted",
            "new123",
            Some(summary("new123", false)),
        )],
        ..MockDaemon::default()
    })
    .await;
    let mut app = rt_tui_client::app::App::new_headless(AppState::new()).with_streams(&endpoint);

    let applied = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if app.poll_streams() > 0 {
                return true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stream updates are applied");

    assert!(applied);
    assert_eq!(app.state.torrents.len(), 1);
    assert_eq!(app.state.torrents[0].hash, "new123");
}
