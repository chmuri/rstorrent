//! TDD contract for TASK-01 — startup path behaviour.

use rt_tui_client::cli::{Cli, DEFAULT_REFRESH_MS};
use rt_tui_client::run::{run, StartupError};

fn cli_for(endpoint: String) -> Cli {
    Cli {
        endpoint,
        theme: "default".to_string(),
        refresh_ms: DEFAULT_REFRESH_MS,
    }
}

#[tokio::test]
async fn run_reports_success_on_listening_endpoint() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let port = listener.local_addr().expect("listener address").port();

    let result = run(cli_for(format!("127.0.0.1:{port}"))).await;

    assert!(result.is_ok(), "startup failed: {result:?}");
}

#[tokio::test]
async fn run_rejects_malformed_endpoint() {
    let result = run(cli_for("http://localhost:5000".to_string())).await;

    let error = result.expect_err("malformed endpoint must be rejected");
    assert!(matches!(error, StartupError::InvalidEndpoint { .. }));
    assert_eq!(error.exit_code(), 2);
}

#[tokio::test]
async fn run_reports_unreachable_daemon() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    drop(listener); // nothing listens on this port any more

    let result = run(cli_for(format!("127.0.0.1:{port}"))).await;

    let error = result.expect_err("unreachable daemon must be reported");
    assert!(matches!(error, StartupError::Unreachable { .. }));
    assert_eq!(error.exit_code(), 1);
}
