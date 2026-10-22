//! Daemon-mode smoke test.
//!
//! Drives the real binary through `run_app`'s non-interactive path: startup,
//! the signal-driven loop, and graceful shutdown. This is the only coverage for
//! the binary entrypoint that unit tests cannot reach.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn daemon_starts_and_shuts_down_on_sigterm() {
    let base = std::env::temp_dir().join(format!("rstorrent_daemon_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let session = base.join("session");
    let data = base.join("data");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::create_dir_all(&data).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_rstorrent"))
        .arg("--daemon")
        .arg("-n")
        .arg("-s")
        .arg(&session)
        .arg("-d")
        .arg(&data)
        .arg("-p")
        .arg("55123-55133")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon should spawn");

    // Give the daemon time to initialize the session directory.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut started = false;
    while Instant::now() < deadline {
        if session.join("dht.cache").exists() || session.join("rstorrent.lock").exists() {
            started = true;
            break;
        }
        if let Ok(Some(_)) = child.try_wait() {
            panic!("daemon exited before initializing");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(started, "daemon never initialized its session directory");

    let status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill should run");
    assert!(status.success(), "could not signal the daemon");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait().expect("wait should succeed") {
            Some(status) => {
                assert!(status.success(), "daemon exited with {status}");
                break;
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("daemon did not shut down after SIGTERM");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    let _ = std::fs::remove_dir_all(&base);
}
