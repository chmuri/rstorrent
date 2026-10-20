use lt_common::Object;
use rstorrent::cli::Cli;
use rstorrent::control::Control;
use std::fs;

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rstorrent_test_session_default_{}_{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn create_sample_torrent_file(path: &std::path::Path) -> Vec<u8> {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from("test_persisted_torrent"))
        .unwrap();
    info.insert_key("length", Object::from(1024i64)).unwrap();
    info.insert_key("piece length", Object::from(262144i64))
        .unwrap();
    info.insert_key("pieces", Object::from(vec![0xAAu8; 20]))
        .unwrap();

    let mut object = Object::create_map();
    object.insert_key("info", info).unwrap();
    let bytes = lt_common::object::bencode::encode(&object).unwrap();
    fs::write(path, &bytes).unwrap();
    bytes
}

#[tokio::test]
async fn test_default_session_persists_and_restores_torrents() {
    let home_dir = TempDir::new("home");
    let xdg_data_dir = home_dir.path.join(".local/share");
    fs::create_dir_all(&xdg_data_dir).unwrap();

    std::env::set_var("HOME", &home_dir.path);
    std::env::set_var("XDG_DATA_HOME", &xdg_data_dir);

    let torrent_file = home_dir.path.join("sample.torrent");
    create_sample_torrent_file(&torrent_file);

    let cli = Cli {
        bind_address: None,
        directory: None,
        local_address: None,
        port_range: None,
        session: None, // No session path passed -> uses XDG default
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
    };

    // First session: load torrent and shutdown
    {
        let mut control = Control::new();
        assert!(control.initialize(&cli).is_ok());

        let session_path = {
            let sm = control.session_manager.lock().unwrap();
            assert!(sm.is_active(), "Session manager should be active by default");
            sm.path().to_string()
        };

        let expected_session = xdg_data_dir.join("rstorrent/session");
        assert!(
            session_path.contains("rstorrent/session"),
            "Session path should point to default XDG path, got: {}",
            session_path
        );
        assert!(expected_session.exists(), "Default session directory should exist");

        let load_res = control.try_load_torrent(torrent_file.to_str().unwrap(), true);
        assert!(load_res.is_ok(), "Load failed: {:?}", load_res.err());

        assert_eq!(control.download_list.read().unwrap().all().len(), 1);

        control.shutdown().await;
        assert!(control.is_shutdown_completed());
    }

    // Second session: startup without arguments and verify torrent is restored
    {
        let mut control = Control::new();
        assert!(control.initialize(&cli).is_ok());

        let downloads = control.download_list.read().unwrap().all();
        assert_eq!(
            downloads.len(),
            1,
            "Torrent should have been restored from default session directory"
        );

        {
            let dl = downloads[0].read().unwrap();
            assert_eq!(dl.name(), "test_persisted_torrent");
        }

        control.shutdown().await;
    }
}
