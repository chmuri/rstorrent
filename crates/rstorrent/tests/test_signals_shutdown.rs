use rstorrent::cli::Cli;
use rstorrent::control::Control;
use rstorrent::signals::{
    begin_terminal_session, restore_terminal_once, terminal_is_restored, SigintOutcome,
    SignalState, TerminalSessionGuard,
};
use std::sync::Mutex;
use std::time::Duration;

/// Terminal state is process-global, so tests that touch it must not overlap.
static TERMINAL_TEST_LOCK: Mutex<()> = Mutex::new(());

struct TempDir {
    path: String,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = format!("/tmp/rtorrent_test_signals_{}_{}", name, std::process::id());
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn free_port() -> u16 {
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    probe.local_addr().unwrap().port()
}

fn daemon_cli(session: &str, directory: &str, port: u16) -> Cli {
    Cli {
        bind_address: Some("127.0.0.1".to_string()),
        directory: Some(directory.to_string()),
        local_address: None,
        port_range: Some(format!("{}-{}", port, port)),
        session: Some(session.to_string()),
        no_rc: true,
        config_file: None,
        daemon: true,
        grpc_port: None,
        grpc_socket: None,
        options: Vec::new(),
        command_options: Vec::new(),
        deprecated: false,
        disable_intermediate: false,
        intermediate_no_xmlrpc: false,
        files: Vec::new(),
    }
}

#[test]
fn double_sigint_escalates_to_immediate_exit() {
    let state = SignalState::new();

    assert_eq!(state.note_sigint(), SigintOutcome::GracefulShutdown);
    assert!(state.is_shutdown());
    assert!(!state.is_quick());
    assert_eq!(state.sigint_count(), 1);

    assert_eq!(state.note_sigint(), SigintOutcome::ImmediateExit);
    assert!(state.is_shutdown());
    assert!(state.is_quick());
    assert_eq!(state.sigint_count(), 2);

    // Further interrupts remain immediate-exit requests.
    assert_eq!(state.note_sigint(), SigintOutcome::ImmediateExit);
}

#[test]
fn sigwinch_only_sets_a_consumable_redraw_flag() {
    let state = SignalState::new();

    assert!(!state.take_sigwinch());
    state.note_sigwinch();
    assert!(state.take_sigwinch());
    assert!(!state.take_sigwinch());
}

#[test]
fn terminal_restore_is_idempotent() {
    let _guard = TERMINAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    begin_terminal_session();
    assert!(!terminal_is_restored());

    assert!(restore_terminal_once());
    assert!(terminal_is_restored());

    // A second restore must be a no-op, not a second escape sequence.
    assert!(!restore_terminal_once());
    assert!(terminal_is_restored());
}

#[test]
fn terminal_session_guard_restores_on_drop_and_is_idempotent() {
    let _guard = TERMINAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    {
        let mut session = TerminalSessionGuard::enter();
        assert!(!terminal_is_restored());

        assert!(session.restore());
        assert!(terminal_is_restored());
        assert!(!session.restore());
    }

    // Dropping the already-restored guard must not touch the terminal again.
    assert!(terminal_is_restored());
}

#[tokio::test]
async fn graceful_shutdown_drains_network_and_is_idempotent() {
    let session_dir = TempDir::new("session");
    let download_dir = TempDir::new("download");
    let port = free_port();

    let cli = daemon_cli(&session_dir.path, &download_dir.path, port);
    let mut control = Control::new();
    control.listen_port = port;

    control
        .initialize(&cli)
        .expect("control initialization failed");
    assert!(control.network_runtime.lock().unwrap().is_some());

    let lock_path = format!("{}/rtorrent.lock", session_dir.path);
    assert!(std::path::Path::new(&lock_path).exists());

    tokio::time::timeout(Duration::from_secs(15), control.shutdown())
        .await
        .expect("graceful shutdown exceeded its outer test deadline");

    assert!(control.is_shutdown_completed());
    assert!(control.network_runtime.lock().unwrap().is_none());
    assert!(!control.session_manager.lock().unwrap().is_active());
    assert!(!std::path::Path::new(&lock_path).exists());

    // A second shutdown must be a no-op and must not panic or block.
    tokio::time::timeout(Duration::from_secs(2), control.shutdown())
        .await
        .expect("repeated shutdown blocked");
    assert!(control.is_shutdown_completed());
}
