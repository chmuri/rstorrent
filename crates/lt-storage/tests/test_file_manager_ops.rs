use lt_storage::file::File;
use lt_storage::file_manager::FileManager;
use lt_storage::mmap_safe::Prot;
use lt_storage::socket_file::OpenFlags;

#[test]
fn opens_with_fadvise_and_closes_idle_files() {
    let path =
        std::env::temp_dir().join(format!("rtorrent_file_manager_{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let mut file = File::new();
    file.set_path(&path);
    file.set_size_bytes(4096);

    let mut manager = FileManager::new();
    manager.set_advise_random(true);
    manager.set_close_idle_secs(10);

    assert!(manager.open(
        &mut file,
        false,
        Prot::READ | Prot::WRITE,
        OpenFlags::CREATE,
    ));
    assert!(file.is_open());
    assert_eq!(manager.open_files(), 1);
    assert_eq!(manager.files_opened_counter(), 1);
    assert_eq!(manager.files_advised_counter(), 1);

    assert_eq!(manager.close_idle(0), 0);
    assert_eq!(manager.close_idle(u64::MAX), 1);
    assert!(!file.is_open());
    assert_eq!(manager.open_files(), 0);
    assert_eq!(manager.files_closed_counter(), 1);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn hashing_advice_is_gated_by_its_own_toggle() {
    let path = std::env::temp_dir().join(format!(
        "rtorrent_file_manager_hashing_{}.bin",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    let mut manager = FileManager::new();
    manager.set_advise_random(false);
    manager.set_advise_random_hashing(false);

    let mut file = File::new();
    file.set_path(&path);

    assert!(manager.open(&mut file, true, Prot::READ | Prot::WRITE, OpenFlags::CREATE,));
    assert_eq!(manager.files_advised_counter(), 0);
    manager.close(&mut file);

    manager.set_advise_random_hashing(true);
    assert!(manager.open(&mut file, true, Prot::READ | Prot::WRITE, OpenFlags::CREATE,));
    assert_eq!(manager.files_advised_counter(), 1);
    manager.close(&mut file);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn max_open_files_is_validated() {
    let mut manager = FileManager::new();
    assert_eq!(manager.max_open_files(), 500);
    manager.set_max_open_files(64);
    assert_eq!(manager.max_open_files(), 64);
}
