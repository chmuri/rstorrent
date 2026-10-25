use lt_common::Object;
use rstorrent::cli::Cli;
use rstorrent::control::Control;
use rt_rpc::Target;
use std::fs::File;
use std::io::Write;

struct TempDir {
    path: String,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = format!("/tmp/rtorrent_test_e2e_{}_{}", name, std::process::id());
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

#[tokio::test]
async fn test_full_lifecycle_and_rpc() {
    let session_dir = TempDir::new("session");
    let download_dir = TempDir::new("download");

    // Create custom rc file
    let rc_path = format!("{}/test.rc", session_dir.path);
    {
        let mut file = File::create(&rc_path).unwrap();
        writeln!(file, "# do:log.add_output=debug_trace,test_output").unwrap();
        writeln!(file, "throttle.global_up.max_rate.set_kb = 250").unwrap();
        writeln!(file, "throttle.global_down.max_rate.set_kb = 1000").unwrap();
    }

    let cli = Cli {
        bind_address: Some("127.0.0.1".to_string()),
        directory: Some(download_dir.path.clone()),
        watch_dir: None,
        local_address: None,
        port_range: Some("6881-6889".to_string()),
        session: Some(session_dir.path.clone()),
        no_rc: false,
        config_file: None,
        daemon: true,
        grpc_port: None,
        grpc_socket: None,
        options: Vec::new(),
        command_options: vec!["choke_group.up.max.set = 50".to_string()],
        deprecated: false,
        disable_intermediate: false,
        intermediate_no_xmlrpc: false,
        files: Vec::new(),
    };

    // Override HOME to point to session_dir containing .rtorrent.rc
    std::env::set_var("HOME", &session_dir.path);
    std::fs::copy(&rc_path, format!("{}/.rtorrent.rc", session_dir.path)).unwrap();

    let mut control = Control::new();
    assert!(control.initialize(&cli).is_ok());

    // Verify initialized options via CommandMap
    {
        let map = control.command_map.read().unwrap();

        // Check throttle rates configured via CLI / RC
        let up_rate = map
            .call_command(
                "throttle.global_up.max_rate",
                &Object::create_empty(),
                &Target::None,
                false,
            )
            .unwrap();
        assert_eq!(250 * 1024, up_rate.as_value().unwrap());

        let down_rate = map
            .call_command(
                "throttle.global_down.max_rate",
                &Object::create_empty(),
                &Target::None,
                false,
            )
            .unwrap();
        assert_eq!(1000 * 1024, down_rate.as_value().unwrap());

        let max_uploads = map
            .call_command(
                "choke_group.up.max",
                &Object::create_empty(),
                &Target::None,
                false,
            )
            .unwrap();
        assert_eq!(50, max_uploads.as_value().unwrap());
    }

    // Verify views are present
    {
        let vm = control.view_manager.read().unwrap();
        assert!(vm.find("main").is_some());
        assert!(vm.find("complete").is_some());
        assert!(vm.find("seeding").is_some());
    }

    // Verify session directory lockfile exists
    assert!(std::path::Path::new(&format!("{}/rtorrent.lock", session_dir.path)).exists());

    // Clean graceful shutdown, including draining the supervised network
    // runtime and committing/releasing the session.
    control.signals.request_normal_shutdown();
    assert!(control.signals.is_shutdown());

    control.shutdown().await;
    assert!(control.is_shutdown_completed());

    // A second shutdown must be an idempotent no-op.
    control.shutdown().await;
    assert!(control.is_shutdown_completed());

    // Lockfile should be unlocked / removed
    assert!(!control.session_manager.lock().unwrap().is_active());
}
