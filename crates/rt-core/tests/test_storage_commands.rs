use std::sync::{Arc, RwLock};

use lt_common::Object;
use rt_core::commands::storage::register_storage_commands;
use rt_core::download_list::DownloadList;
use rt_rpc::{CommandMap, Target};

fn registry() -> (CommandMap, Arc<RwLock<DownloadList>>) {
    let mut map = CommandMap::new();
    let list = Arc::new(RwLock::new(DownloadList::new()));
    register_storage_commands(&mut map, Arc::clone(&list)).unwrap();
    (map, list)
}

fn call(map: &CommandMap, name: &str, arg: Object) -> Result<Object, rt_rpc::RpcError> {
    map.call_command(name, &arg, &Target::None, true)
}

#[test]
fn hash_concurrency_reads_and_writes_before_the_pool_exists() {
    let (map, list) = registry();

    let initial = call(&map, "storage.hash_concurrency", Object::create_empty())
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(initial, list.read().unwrap().hash_concurrency() as i64);

    call(&map, "storage.hash_concurrency.set", Object::from(3i64)).unwrap();
    assert_eq!(list.read().unwrap().hash_concurrency(), 3);
}

#[test]
fn hash_concurrency_rejects_out_of_range_values() {
    let (map, _list) = registry();

    assert!(call(&map, "storage.hash_concurrency.set", Object::from(0i64)).is_err());
    assert!(call(&map, "storage.hash_concurrency.set", Object::from(1025i64)).is_err());
}

#[test]
fn load_threads_reads_and_writes() {
    let (map, _list) = registry();

    call(&map, "storage.load_threads.set", Object::from(3i64)).unwrap();
    let value = call(&map, "storage.load_threads", Object::create_empty())
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(value, 3);

    assert!(call(&map, "storage.load_threads.set", Object::from(0i64)).is_err());
    assert!(call(&map, "storage.load_threads.set", Object::from(65i64)).is_err());
}

#[test]
fn hash_concurrency_cannot_change_after_the_pool_is_created() {
    let (map, list) = registry();
    // First recheck creates the pool.
    let expected = list.read().unwrap().hash_concurrency();
    assert_eq!(list.read().unwrap().hash_scheduler().workers(), expected);

    let err = call(&map, "storage.hash_concurrency.set", Object::from(4i64))
        .expect_err("pool size is fixed once the scheduler exists");
    assert!(err.to_string().contains("restart"), "{err}");
}
