use rstorrent::control::Control;
use rstorrent::signals::install_posix_signal_defaults;

#[test]
fn posix_signal_defaults_are_installed_without_panicking() {
    install_posix_signal_defaults();
}

#[tokio::test]
async fn graceful_shutdown_completes_in_order() {
    let control = Control::new();

    assert!(!control.is_shutdown_completed());
    control.shutdown().await;
    assert!(control.is_shutdown_completed());
}
