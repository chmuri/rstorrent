//! TDD contract for TASK-13 — throttle key maps and layouts.

use crossterm::event::{KeyCode, KeyModifiers};

use rt_grpc::SessionStatsResponse;
use rt_grpc::TorrentSummary;
use rt_tui_client::actions::TuiAction;
use rt_tui_client::app::App;
use rt_tui_client::state::{AppState, SessionStats, TorrentViewModel};
use rt_tui_client::throttle::{StepSize, ThrottleAction, ThrottleKeyMap};

// ---------------------------------------------------------------------------
// Plan contract
// ---------------------------------------------------------------------------

#[test]
fn qwerty_layout_maps_a_to_upload_small_up() {
    let map = ThrottleKeyMap::new("qwerty");
    let action = map.lookup('a');
    assert_eq!(action, Some(ThrottleAction::UploadUp(StepSize::Small)));
}

#[test]
fn qwerty_layout_maps_uppercase_a_to_download_small_up() {
    let map = ThrottleKeyMap::new("qwerty");
    let action = map.lookup('A');
    assert_eq!(action, Some(ThrottleAction::DownloadUp(StepSize::Small)));
}

#[test]
fn dvorak_layout_maps_semicolon_to_upload_small_down() {
    let map = ThrottleKeyMap::new("dvorak");
    let action = map.lookup(';');
    assert_eq!(action, Some(ThrottleAction::UploadDown(StepSize::Small)));
}

#[test]
fn throttle_adjustment_saturates_at_zero() {
    let mut throttle_up: u64 = 1000; // 1 KB
    throttle_up = throttle_up.saturating_sub(5000); // medium step
    assert_eq!(throttle_up, 0);
}

#[test]
fn unknown_layout_falls_back_to_qwerty() {
    let map = ThrottleKeyMap::new("unknown_layout");
    let action = map.lookup('a');
    assert_eq!(action, Some(ThrottleAction::UploadUp(StepSize::Small)));
}

// ---------------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------------

#[test]
fn qwerty_covers_small_medium_large_in_both_directions() {
    let map = ThrottleKeyMap::new("qwerty");

    assert_eq!(map.lookup('s'), Some(ThrottleAction::UploadUp(StepSize::Medium)));
    assert_eq!(map.lookup('d'), Some(ThrottleAction::UploadUp(StepSize::Large)));
    assert_eq!(map.lookup('z'), Some(ThrottleAction::UploadDown(StepSize::Small)));
    assert_eq!(map.lookup('x'), Some(ThrottleAction::UploadDown(StepSize::Medium)));
    assert_eq!(map.lookup('c'), Some(ThrottleAction::UploadDown(StepSize::Large)));
    assert_eq!(map.lookup('S'), Some(ThrottleAction::DownloadUp(StepSize::Medium)));
    assert_eq!(map.lookup('C'), Some(ThrottleAction::DownloadDown(StepSize::Large)));
}

#[test]
fn azerty_and_qwertz_remap_the_letter_keys() {
    let azerty = ThrottleKeyMap::new("azerty");
    assert_eq!(azerty.lookup('q'), Some(ThrottleAction::UploadUp(StepSize::Small)));
    assert_eq!(azerty.lookup('w'), Some(ThrottleAction::UploadDown(StepSize::Small)));

    let qwertz = ThrottleKeyMap::new("qwertz");
    assert_eq!(qwertz.lookup('y'), Some(ThrottleAction::UploadDown(StepSize::Small)));
    assert_eq!(qwertz.lookup('a'), Some(ThrottleAction::UploadUp(StepSize::Small)));
}

#[test]
fn unrelated_keys_are_not_throttle_keys() {
    let map = ThrottleKeyMap::new("qwerty");

    assert_eq!(map.lookup('q'), None);
    assert_eq!(map.lookup('1'), None);
}

#[test]
fn step_sizes_are_kilobyte_based() {
    assert_eq!(StepSize::Small.bytes(), 1024);
    assert_eq!(StepSize::Medium.bytes(), 5 * 1024);
    assert_eq!(StepSize::Large.bytes(), 50 * 1024);
}

#[test]
fn adjust_moves_the_limit_and_saturates() {
    assert_eq!(
        ThrottleKeyMap::adjust(1000, ThrottleAction::UploadUp(StepSize::Small)),
        2024
    );
    assert_eq!(
        ThrottleKeyMap::adjust(1000, ThrottleAction::UploadDown(StepSize::Small)),
        0
    );
    assert_eq!(
        ThrottleKeyMap::adjust(u64::MAX, ThrottleAction::UploadUp(StepSize::Large)),
        u64::MAX
    );
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

fn app_with_limits(up: i64, down: i64) -> App {
    let mut state = AppState::new();
    state.torrents = vec![TorrentViewModel::from_proto(&TorrentSummary {
        hash: "abc123".to_string(),
        name: "test.torrent".to_string(),
        ..TorrentSummary::default()
    })];
    state.session_stats = SessionStats::from_proto(&SessionStatsResponse {
        throttle_up_limit: up,
        throttle_down_limit: down,
        ..SessionStatsResponse::default()
    });

    App::new_headless(state)
}

#[test]
fn a_raises_the_upload_limit_through_a_command() {
    let mut app = app_with_limits(10_000, 20_000);

    let action = app.handle_key(KeyCode::Char('a'), KeyModifiers::NONE);

    assert_eq!(
        action,
        Some(TuiAction::Execute {
            command: "throttle.global_up.max_rate.set".to_string(),
            args: vec![11_024],
            hash: None,
        })
    );
}

#[test]
fn uppercase_a_lowers_the_download_limit() {
    let mut app = app_with_limits(10_000, 20_000);

    let action = app.handle_key(KeyCode::Char('A'), KeyModifiers::NONE);

    assert_eq!(
        action,
        Some(TuiAction::Execute {
            command: "throttle.global_down.max_rate.set".to_string(),
            args: vec![18_976],
            hash: None,
        })
    );
}

#[test]
fn lowering_below_zero_saturates() {
    let mut app = app_with_limits(500, 500);

    let action = app.handle_key(KeyCode::Char('c'), KeyModifiers::NONE);

    assert_eq!(
        action,
        Some(TuiAction::Execute {
            command: "throttle.global_up.max_rate.set".to_string(),
            args: vec![0],
            hash: None,
        })
    );
}

#[test]
fn throttle_keys_work_without_a_focused_torrent() {
    let mut state = AppState::new();
    state.session_stats = SessionStats::from_proto(&SessionStatsResponse {
        throttle_up_limit: 4096,
        ..SessionStatsResponse::default()
    });
    let mut app = App::new_headless(state);

    let action = app.handle_key(KeyCode::Char('a'), KeyModifiers::NONE);

    assert!(matches!(action, Some(TuiAction::Execute { .. })));
}

#[test]
fn app_can_use_another_layout() {
    let mut app = app_with_limits(0, 0).with_layout("dvorak");

    let action = app.handle_key(KeyCode::Char(';'), KeyModifiers::NONE);

    assert_eq!(
        action,
        Some(TuiAction::Execute {
            command: "throttle.global_up.max_rate.set".to_string(),
            args: vec![0],
            hash: None,
        })
    );
}
