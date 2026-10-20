use lt_common::hash_string::HashString;
use rstorrent::cli::Cli;
use rstorrent::control::Control;

const SAMPLE_MAGNET: &str = "magnet:?xt=urn:btih:E156CE70FEE6464549FEF653C8468AECB3E7E9F1&dn=Black+Panther+%282018%29+%5BBluRay%5D+%5B1080p%5D+%5BYTS%5D+%5BYIFY%5D&tr=udp%3A%2F%2Ftracker.coppersurfer.tk%3A6969%2Fannounce&tr=udp%3A%2F%2F9.rarbg.com%3A2710%2Fannounce&tr=udp%3A%2F%2Fp4p.arenabg.com%3A1337&tr=udp%3A%2F%2Ftracker.leechers-paradise.org%3A6969&tr=udp%3A%2F%2Ftracker.internetwarriors.net%3A1337&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce&tr=http%3A%2F%2Ftracker.openbittorrent.com%3A80%2Fannounce&tr=udp%3A%2F%2Fopentracker.i2p.rocks%3A6969%2Fannounce&tr=udp%3A%2F%2Ftracker.internetwarriors.net%3A1337%2Fannounce&tr=udp%3A%2F%2Ftracker.leechers-paradise.org%3A6969%2Fannounce&tr=udp%3A%2F%2Fcoppersurfer.tk%3A6969%2Fannounce&tr=udp%3A%2F%2Ftracker.zer0day.to%3A1337%2Fannounce";

#[test]
fn test_try_load_torrent_magnet() {
    let control = Control::new();
    control.view_manager.write().unwrap().insert("main");

    let result = control.try_load_torrent(SAMPLE_MAGNET, true);
    assert!(result.is_ok(), "Failed to load magnet: {:?}", result.err());

    let hash = HashString::from_hex("E156CE70FEE6464549FEF653C8468AECB3E7E9F1").unwrap();
    let dl_opt = control.download_list.read().unwrap().find(&hash);
    assert!(dl_opt.is_some(), "Torrent not found in DownloadList");

    let dl_arc = dl_opt.unwrap();
    let dl = dl_arc.read().unwrap();
    assert_eq!(
        dl.metadata().name,
        "Black Panther (2018) [BluRay] [1080p] [YTS] [YIFY]"
    );
    assert!(dl.is_metadata_download());
    assert!(dl.is_active());

    // Verify it was added to the main view
    let vm = control.view_manager.read().unwrap();
    let view = vm.find("main").unwrap();
    assert_eq!(view.size(), 1);

    // Duplicate loading should succeed without error
    drop(dl);
    let dup_result = control.try_load_torrent(SAMPLE_MAGNET, true);
    assert!(
        dup_result.is_ok(),
        "Duplicate magnet load failed: {:?}",
        dup_result.err()
    );
}

#[test]
fn test_try_load_torrent_magnet_with_quotes_and_whitespace() {
    let control = Control::new();
    control.view_manager.write().unwrap().insert("main");

    let quoted_magnet = format!("  \"{}\"  ", SAMPLE_MAGNET);
    let result = control.try_load_torrent(&quoted_magnet, false);
    assert!(
        result.is_ok(),
        "Failed to load quoted magnet: {:?}",
        result.err()
    );

    let hash = HashString::from_hex("E156CE70FEE6464549FEF653C8468AECB3E7E9F1").unwrap();
    let dl_opt = control.download_list.read().unwrap().find(&hash);
    assert!(dl_opt.is_some());
    let dl_arc = dl_opt.unwrap();
    let dl = dl_arc.read().unwrap();
    assert!(!dl.is_active(), "Should not be active when start=false");
}

#[tokio::test]
async fn test_cli_positional_magnet_loading() {
    let mut control = Control::new();
    let cli = Cli {
        files: vec![SAMPLE_MAGNET.to_string()],
        ..Default::default()
    };

    let init_result = control.initialize(&cli);
    assert!(
        init_result.is_ok(),
        "Control initialize failed: {:?}",
        init_result.err()
    );

    let hash = HashString::from_hex("E156CE70FEE6464549FEF653C8468AECB3E7E9F1").unwrap();
    let dl_opt = control.download_list.read().unwrap().find(&hash);
    assert!(
        dl_opt.is_some(),
        "Torrent from CLI positional args not found in DownloadList"
    );

    let dl_arc = dl_opt.unwrap();
    let dl = dl_arc.read().unwrap();
    assert_eq!(
        dl.metadata().name,
        "Black Panther (2018) [BluRay] [1080p] [YTS] [YIFY]"
    );
    assert!(dl.is_active());

    drop(dl);
    drop(dl_arc);
    let _ = control.shutdown().await;
}

#[test]
fn test_invalid_magnet_returns_error() {
    let control = Control::new();
    let result = control.try_load_torrent("magnet:?invalid", false);
    assert!(result.is_err());
}
