use lt_common::hash_string::HashString;
use lt_common::Object;
use rt_core::download_factory::DownloadFactory;
use rt_core::download_list::DownloadList;
use rt_core::view_manager::ViewManager;
use std::collections::HashSet;
use std::time::Instant;

fn create_test_torrent(id: usize) -> Object {
    let mut info = Object::create_map();
    let _ = info.insert_key("name", Object::from(format!("scale_test_{id}")));
    let _ = info.insert_key("length", Object::from(1024i64));
    let _ = info.insert_key("piece length", Object::from(262144i64));

    let mut piece_hash = [0u8; 20];
    piece_hash[..8].copy_from_slice(&id.to_be_bytes());
    let _ = info.insert_key("pieces", Object::from(piece_hash.to_vec()));

    let mut object = Object::create_map();
    let _ = object.insert_key("info", info);
    object
}

#[test]
fn test_bulk_erase_scale_parallel() {
    let mut list = DownloadList::new();
    let mut vm = ViewManager::new();

    const TOTAL_TORRENTS: usize = 5000;
    const ERASE_COUNT: usize = 2500;

    let mut all_hashes = Vec::with_capacity(TOTAL_TORRENTS);

    for i in 0..TOTAL_TORRENTS {
        let obj = create_test_torrent(i);
        let mut factory = DownloadFactory::new();
        factory.load_object(obj);
        let dl = factory.commit(&mut list).expect("commit download");
        vm.insert_download_to_all(std::sync::Arc::clone(&dl));
        let hash = *dl.read().unwrap().info_hash();
        all_hashes.push(hash);
    }

    assert_eq!(list.len(), TOTAL_TORRENTS);
    for view in vm.all() {
        assert_eq!(view.total_size(), TOTAL_TORRENTS);
    }

    let to_erase: HashSet<HashString> = all_hashes[..ERASE_COUNT].iter().copied().collect();

    let start = Instant::now();
    let erased = list.erase_batch(&to_erase);
    let dl_list_elapsed = start.elapsed();

    assert_eq!(erased.len(), ERASE_COUNT);
    assert_eq!(list.len(), TOTAL_TORRENTS - ERASE_COUNT);

    let start_vm = Instant::now();
    vm.erase_batch_from_all(&to_erase, None);
    let vm_elapsed = start_vm.elapsed();

    for view in vm.all() {
        assert_eq!(view.total_size(), TOTAL_TORRENTS - ERASE_COUNT);
    }

    eprintln!(
        "Batch erase of {}/{} downloads: DownloadList={:?}, ViewManager (11 views)={:?}",
        ERASE_COUNT, TOTAL_TORRENTS, dl_list_elapsed, vm_elapsed
    );

    // Erasing 2500 torrents across 11 views must be sub-second
    assert!(
        dl_list_elapsed.as_millis() < 1000,
        "DownloadList::erase_batch took too long: {:?}",
        dl_list_elapsed
    );
    assert!(
        vm_elapsed.as_millis() < 1000,
        "ViewManager::erase_batch_from_all took too long: {:?}",
        vm_elapsed
    );

    // Erase the remaining torrents
    let remaining: HashSet<HashString> = all_hashes[ERASE_COUNT..].iter().copied().collect();
    list.erase_batch(&remaining);
    vm.erase_batch_from_all(&remaining, None);

    assert_eq!(list.len(), 0);
    for view in vm.all() {
        assert_eq!(view.total_size(), 0);
    }
}
