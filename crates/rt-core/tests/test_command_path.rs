use lt_common::Object;
use rt_core::commands::system::{register_system_commands, resolve_path, resolve_path_or_throw};
use rt_rpc::{CommandMap, Target};

use std::sync::atomic::{AtomicU64, Ordering};
static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(1);

struct TempDirFixture {
    path: std::path::PathBuf,
}

impl TempDirFixture {
    fn new() -> Self {
        let id = FIXTURE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir =
            std::env::temp_dir().join(format!("rtorrent_test_path_{}_{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let data_dir = temp_dir.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let link_path = temp_dir.join("link");
        let _ = std::fs::remove_file(&link_path);
        std::os::unix::fs::symlink(&data_dir, &link_path).unwrap();

        Self { path: temp_dir }
    }
}

impl Drop for TempDirFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_resolves_symlink() {
    let fixture = TempDirFixture::new();
    let _temp_canon = resolve_path(fixture.path.to_str().unwrap());
    let link_path = fixture.path.join("link");
    let data_path = fixture.path.join("data");

    assert_eq!(
        resolve_path(data_path.to_str().unwrap()),
        resolve_path(link_path.to_str().unwrap())
    );

    // Below symlink
    let below = data_path.join("below");
    std::fs::create_dir_all(&below).unwrap();
    let link_below = link_path.join("below");
    assert_eq!(
        resolve_path(below.to_str().unwrap()),
        resolve_path(link_below.to_str().unwrap())
    );
}

#[test]
fn test_removes_relative_components() {
    let fixture = TempDirFixture::new();
    let temp_canon = resolve_path(fixture.path.to_str().unwrap());
    let data_path = fixture.path.join("data");
    let data_canon = resolve_path(data_path.to_str().unwrap());

    assert_eq!(
        temp_canon,
        resolve_path(&format!("{}/data/..", fixture.path.display()))
    );
    assert_eq!(
        data_canon,
        resolve_path(&format!("{}/./data", fixture.path.display()))
    );
    assert_eq!(
        data_canon,
        resolve_path(&format!("{}/data/", fixture.path.display()))
    );
    assert_eq!(
        data_canon,
        resolve_path(&format!("{}//data//", fixture.path.display()))
    );
}

#[test]
fn test_expands_tilde() {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            assert_eq!(resolve_path(&home), resolve_path("~"));
            assert!(resolve_path_or_throw("~root/somewhere").is_err());
        }
    }
}

#[test]
fn test_missing_path_throws() {
    let fixture = TempDirFixture::new();
    assert!(resolve_path_or_throw("").is_err());
    assert!(resolve_path_or_throw(&format!("{}/does_not_exist", fixture.path.display())).is_err());

    let link_path = fixture.path.join("link");
    let data_path = fixture.path.join("data");
    assert_eq!(
        resolve_path(data_path.to_str().unwrap()),
        resolve_path_or_throw(link_path.to_str().unwrap()).unwrap()
    );
}

#[test]
fn test_missing_path_is_empty() {
    let fixture = TempDirFixture::new();
    assert_eq!("", resolve_path(""));
    assert_eq!(
        "",
        resolve_path(&format!("{}/does_not_exist", fixture.path.display()))
    );
    assert_eq!(
        "",
        resolve_path(&format!("{}/does_not_exist/below", fixture.path.display()))
    );

    let dangling = fixture.path.join("dangling");
    std::os::unix::fs::symlink(fixture.path.join("gone"), &dangling).unwrap();
    assert_eq!("", resolve_path(dangling.to_str().unwrap()));
}

#[test]
fn test_commands() {
    let fixture = TempDirFixture::new();
    let mut map = CommandMap::new();
    register_system_commands(&mut map).unwrap();

    let link_path = fixture.path.join("link");
    let data_path = fixture.path.join("data");
    let data_canon = resolve_path(data_path.to_str().unwrap());

    map.call_command(
        "directory.default.set",
        &Object::from(link_path.to_str().unwrap()),
        &Target::None,
        true,
    )
    .unwrap();

    assert_eq!(
        link_path.to_str().unwrap(),
        map.call_command(
            "directory.default",
            &Object::create_empty(),
            &Target::None,
            true
        )
        .unwrap()
        .as_str()
        .unwrap()
    );

    assert_eq!(
        data_canon,
        map.call_command(
            "directory.default.realpath.or_empty",
            &Object::create_empty(),
            &Target::None,
            true
        )
        .unwrap()
        .as_str()
        .unwrap()
    );

    assert_eq!(
        data_canon,
        map.call_command(
            "directory.default.realpath.or_throw",
            &Object::create_empty(),
            &Target::None,
            true
        )
        .unwrap()
        .as_str()
        .unwrap()
    );

    // Missing path
    let missing_path = fixture.path.join("missing");
    map.call_command(
        "directory.default.set",
        &Object::from(missing_path.to_str().unwrap()),
        &Target::None,
        true,
    )
    .unwrap();

    assert_eq!(
        "",
        map.call_command(
            "directory.default.realpath.or_empty",
            &Object::create_empty(),
            &Target::None,
            true
        )
        .unwrap()
        .as_str()
        .unwrap()
    );

    assert!(map
        .call_command(
            "directory.default.realpath.or_throw",
            &Object::create_empty(),
            &Target::None,
            true
        )
        .is_err());
}
