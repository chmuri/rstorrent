//! TDD contract for TASK-10 — keyboard shortcuts and torrent actions.

mod common;

use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};

use common::{spawn_tcp_daemon, MockDaemon};
use rt_grpc::TorrentSummary;
use rt_tui_client::actions::{PromptKind, TorrentAction, TuiAction};
use rt_tui_client::app::App;
use rt_tui_client::bridge::GrpcBridge;
use rt_tui_client::state::{AppState, TorrentViewModel};

const SHORT_TIMEOUT: Duration = Duration::from_secs(1);
const HASH: &str = "abc123";

fn torrent(hash: &str, active: bool, priority: u32) -> TorrentViewModel {
    TorrentViewModel::from_proto(&TorrentSummary {
        hash: hash.to_string(),
        name: "test.torrent".to_string(),
        is_active: active,
        priority,
        ..TorrentSummary::default()
    })
}

fn state_with_one(active: bool, priority: u32) -> AppState {
    let mut state = AppState::new();
    state.torrents = vec![torrent(HASH, active, priority)];
    state
}

fn state_with_torrents(count: usize) -> AppState {
    let mut state = AppState::new();
    state.torrents = (0..count)
        .map(|index| torrent(HASH, true, 1 + index as u32))
        .collect();
    state
}

fn state_with_active_torrent() -> AppState {
    state_with_one(true, 2)
}

fn state_with_stopped_torrent() -> AppState {
    state_with_one(false, 1)
}

fn state_with_priority(priority: u32) -> AppState {
    state_with_one(true, priority)
}

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn ctrl_s_produces_start_action() {
    let mut app = App::new_headless(state_with_torrents(1));
    let action = app.handle_key(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert_eq!(
        action,
        Some(TuiAction::ControlTorrent {
            hash: HASH.to_string(),
            action: TorrentAction::Start,
        })
    );
}

#[test]
fn ctrl_d_on_active_produces_stop() {
    let mut app = App::new_headless(state_with_active_torrent());
    let action = app.handle_key(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert_eq!(
        action,
        Some(TuiAction::ControlTorrent {
            hash: HASH.to_string(),
            action: TorrentAction::Stop,
        })
    );
}

#[test]
fn ctrl_d_on_stopped_produces_remove() {
    let mut app = App::new_headless(state_with_stopped_torrent());
    let action = app.handle_key(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert!(matches!(action, Some(TuiAction::RemoveTorrent { .. })));
}

#[test]
fn plus_cycles_priority_up() {
    let mut app = App::new_headless(state_with_priority(2));
    let action = app.handle_key(KeyCode::Char('+'), KeyModifiers::NONE);
    assert_eq!(
        action,
        Some(TuiAction::SetPriority {
            hash: HASH.to_string(),
            priority: 3,
        })
    );
}

#[test]
fn shortcuts_noop_on_empty_list() {
    let mut app = App::new_headless(AppState::new());
    let action = app.handle_key(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert_eq!(action, None);
}

// ---------------------------------------------------------------------------
// Remaining shortcuts
// ---------------------------------------------------------------------------

#[test]
fn minus_cycles_priority_down_and_wraps() {
    let mut app = App::new_headless(state_with_priority(0));

    let action = app.handle_key(KeyCode::Char('-'), KeyModifiers::NONE);

    assert_eq!(
        action,
        Some(TuiAction::SetPriority {
            hash: HASH.to_string(),
            priority: 3,
        }),
        "0 -> 3 wraps"
    );
}

#[test]
fn ctrl_k_ignores_stops_and_closes() {
    let mut app = App::new_headless(state_with_active_torrent());

    let action = app.handle_key(KeyCode::Char('k'), KeyModifiers::CONTROL);

    match action {
        Some(TuiAction::Batch(steps)) => {
            assert_eq!(steps.len(), 3);
            assert!(
                matches!(&steps[0], TuiAction::Execute { command, .. } if command == "d.ignore_commands.set"),
                "unexpected first step: {:?}",
                steps[0]
            );
            assert!(matches!(
                steps[1],
                TuiAction::ControlTorrent {
                    action: TorrentAction::Stop,
                    ..
                }
            ));
            assert!(matches!(
                steps[2],
                TuiAction::ControlTorrent {
                    action: TorrentAction::Close,
                    ..
                }
            ));
        }
        other => panic!("expected a batch, got {other:?}"),
    }
}

#[test]
fn ctrl_r_resets_completion_and_checks_the_hash() {
    let mut app = App::new_headless(state_with_active_torrent());

    let action = app.handle_key(KeyCode::Char('r'), KeyModifiers::CONTROL);

    match action {
        Some(TuiAction::Batch(steps)) => {
            assert_eq!(steps.len(), 2);
            assert!(
                matches!(&steps[0], TuiAction::Execute { command, .. } if command == "d.complete.set"),
                "unexpected first step: {:?}",
                steps[0]
            );
            assert!(matches!(
                steps[1],
                TuiAction::ControlTorrent {
                    action: TorrentAction::CheckHash,
                    ..
                }
            ));
        }
        other => panic!("expected a batch, got {other:?}"),
    }
}

#[test]
fn i_toggles_ignore_commands_between_on_and_off() {
    let mut app = App::new_headless(state_with_active_torrent());

    let first = app.handle_key(KeyCode::Char('I'), KeyModifiers::SHIFT);
    match first {
        Some(TuiAction::Execute { args, .. }) => assert_eq!(args, vec![1]),
        other => panic!("expected Execute, got {other:?}"),
    }

    let second = app.handle_key(KeyCode::Char('I'), KeyModifiers::SHIFT);
    match second {
        Some(TuiAction::Execute { args, .. }) => assert_eq!(args, vec![0]),
        other => panic!("expected Execute, got {other:?}"),
    }
}

#[test]
fn backspace_and_ctrl_x_and_slash_open_prompts() {
    let mut app = App::new_headless(state_with_active_torrent());

    assert_eq!(
        app.handle_key(KeyCode::Backspace, KeyModifiers::NONE),
        Some(TuiAction::OpenPrompt(PromptKind::Load))
    );
    assert_eq!(
        app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL),
        Some(TuiAction::OpenPrompt(PromptKind::Command))
    );
    assert_eq!(
        app.handle_key(KeyCode::Char('/'), KeyModifiers::NONE),
        Some(TuiAction::OpenPrompt(PromptKind::Filter))
    );
    assert_eq!(
        app.handle_key(KeyCode::Char('f'), KeyModifiers::NONE),
        Some(TuiAction::OpenPrompt(PromptKind::Filter))
    );
}

#[test]
fn number_keys_switch_active_views() {
    use rt_tui_client::state::ActiveView;
    let mut app = App::new_headless(state_with_active_torrent());

    app.handle_key(KeyCode::Char('1'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Main);

    app.handle_key(KeyCode::Char('2'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Name);

    app.handle_key(KeyCode::Char('3'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Started);

    app.handle_key(KeyCode::Char('4'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Stopped);

    app.handle_key(KeyCode::Char('5'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Complete);

    app.handle_key(KeyCode::Char('6'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Incomplete);

    app.handle_key(KeyCode::Char('7'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Hashing);

    app.handle_key(KeyCode::Char('8'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Seeding);

    app.handle_key(KeyCode::Char('9'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Leeching);

    app.handle_key(KeyCode::Char('0'), KeyModifiers::NONE);
    assert_eq!(app.state.active_view, ActiveView::Active);
}

#[test]
fn escape_clears_active_filter() {
    let mut app = App::new_headless(state_with_active_torrent());
    app.state.filter = Some("test".to_string());

    app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(app.state.filter, None);
}

#[test]
fn ctrl_q_still_quits_without_producing_an_action() {
    let mut app = App::new_headless(state_with_active_torrent());

    let action = app.handle_key(KeyCode::Char('q'), KeyModifiers::CONTROL);

    assert_eq!(action, None);
    assert!(app.should_quit);
}

// ---------------------------------------------------------------------------
// Execution through the bridge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn control_action_is_executed_through_the_bridge() {
    let endpoint = spawn_tcp_daemon(MockDaemon::default()).await;
    let bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut app = App::new(AppState::new(), bridge, SHORT_TIMEOUT);
    let action = TuiAction::ControlTorrent {
        hash: HASH.to_string(),
        action: TorrentAction::Start,
    };

    let outcome = app.execute_action(action).await.expect("bridge attached");

    assert!(outcome.is_ok(), "control failed: {outcome:?}");
}

#[tokio::test]
async fn batch_actions_execute_every_step() {
    let endpoint = spawn_tcp_daemon(MockDaemon::default()).await;
    let bridge = GrpcBridge::connect(&endpoint, SHORT_TIMEOUT)
        .await
        .expect("bridge connects");
    let mut app = App::new(AppState::new(), bridge, SHORT_TIMEOUT);
    let action = TuiAction::Batch(vec![
        TuiAction::Execute {
            command: "d.ignore_commands.set".to_string(),
            args: vec![1],
            hash: Some(HASH.to_string()),
        },
        TuiAction::ControlTorrent {
            hash: HASH.to_string(),
            action: TorrentAction::Stop,
        },
        TuiAction::ControlTorrent {
            hash: HASH.to_string(),
            action: TorrentAction::Close,
        },
    ]);

    let outcome = app.execute_action(action).await.expect("bridge attached");

    assert!(outcome.is_ok(), "batch failed: {outcome:?}");
}

#[tokio::test]
async fn prompt_actions_open_the_input_line() {
    let mut app = App::new_headless(AppState::new());

    let outcome = app
        .execute_action(TuiAction::OpenPrompt(PromptKind::Load))
        .await;

    assert!(outcome.is_some(), "prompts need no bridge");
    assert_eq!(
        app.prompt.as_ref().map(|prompt| prompt.mode()),
        Some(rt_tui_client::widgets::input::PromptMode::LoadStart)
    );
}

#[tokio::test]
async fn headless_app_cannot_execute_bridge_actions() {
    let mut app = App::new_headless(AppState::new());

    let outcome = app
        .execute_action(TuiAction::ControlTorrent {
            hash: HASH.to_string(),
            action: TorrentAction::Start,
        })
        .await;

    assert!(outcome.is_none());
}
