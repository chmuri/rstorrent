use std::process::Command;

#[test]
fn test_cli_config_commands() {
    let dir = std::env::temp_dir().join(format!("rstorrent_cli_config_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("rstorrent.toml");

    let initial_toml = r#"
[network]
port = 6881
download_throttle_kb = 1000

[storage]
directory = "/tmp/downloads"
"#;
    std::fs::write(&config_path, initial_toml).unwrap();

    // 1. config get
    let output = Command::new(env!("CARGO_BIN_EXE_rstorrent"))
        .arg("--config")
        .arg(&config_path)
        .args(["config", "get", "network.port"])
        .output()
        .expect("failed to run config get");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "6881");

    // 2. config set (key value)
    let output = Command::new(env!("CARGO_BIN_EXE_rstorrent"))
        .arg("--config")
        .arg(&config_path)
        .args(["config", "set", "network.port", "6899"])
        .output()
        .expect("failed to run config set");

    assert!(output.status.success());

    // verify get again
    let output = Command::new(env!("CARGO_BIN_EXE_rstorrent"))
        .arg("--config")
        .arg(&config_path)
        .args(["config", "get", "network.port"])
        .output()
        .expect("failed to run config get");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "6899");

    // 3. config set (key=value)
    let output = Command::new(env!("CARGO_BIN_EXE_rstorrent"))
        .arg("--config")
        .arg(&config_path)
        .args(["config", "set", "dht.mode=auto"])
        .output()
        .expect("failed to run config set");

    assert!(output.status.success());

    let output = Command::new(env!("CARGO_BIN_EXE_rstorrent"))
        .arg("--config")
        .arg(&config_path)
        .args(["config", "get", "dht.mode"])
        .output()
        .expect("failed to run config get");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "auto");

    // 4. config validate
    let output = Command::new(env!("CARGO_BIN_EXE_rstorrent"))
        .arg("--config")
        .arg(&config_path)
        .args(["config", "validate"])
        .output()
        .expect("failed to run config validate");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Configuration is valid"));

    // 5. config show
    let output = Command::new(env!("CARGO_BIN_EXE_rstorrent"))
        .arg("--config")
        .arg(&config_path)
        .args(["config", "show"])
        .output()
        .expect("failed to run config show");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("network.port = 6899") || stdout.contains("port = 6899"));

    let _ = std::fs::remove_dir_all(&dir);
}
