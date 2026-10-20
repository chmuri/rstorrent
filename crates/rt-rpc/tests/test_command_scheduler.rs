use lt_common::Object;
use rt_rpc::exec_flags::*;
use rt_rpc::{CommandScheduler, ExecFile};
use std::time::Duration;

#[test]
fn test_parse_rearms_existing_key() {
    let mut scheduler = CommandScheduler::new();
    let test_command = Object::from("print=scheduled");

    scheduler.set_cached_time(Duration::ZERO);
    scheduler
        .parse("key", "3600", "3600", test_command.clone())
        .unwrap();

    let first = scheduler.find("key").unwrap().time_scheduled();

    scheduler.add_cached_time(Duration::from_secs(600));
    scheduler
        .parse("key", "3600", "3600", test_command)
        .unwrap();

    assert_eq!(scheduler.len(), 1);
    assert_eq!(
        scheduler.find("key").unwrap().time_scheduled(),
        first + Duration::from_secs(600)
    );
}

#[test]
fn test_find_locates_a_scheduled_key() {
    let mut scheduler = CommandScheduler::new();
    let test_command = Object::from("print=scheduled");

    assert!(scheduler.find("key").is_none());

    scheduler
        .parse("key", "3600", "3600", test_command)
        .unwrap();

    assert!(scheduler.find("key").is_some());
    assert!(scheduler.find("other").is_none());
}

#[test]
fn test_exec_file_capture() {
    let exec = ExecFile::new();

    let args = Object::create_list_with(vec![Object::from("echo"), Object::from("rtorrent_test")]);

    let res = exec.execute_object(&args, FLAG_CAPTURE).unwrap();
    if let Ok(s) = res.as_str() {
        assert_eq!(s.trim(), "rtorrent_test");
    } else {
        panic!("Expected string result from capture");
    }
}

#[test]
fn test_exec_file_throw_on_error() {
    let exec = ExecFile::new();

    let args = Object::create_list_with(vec![Object::from("false")]);

    assert!(exec.execute_object(&args, FLAG_THROW).is_err());
}
