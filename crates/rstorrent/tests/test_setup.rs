use rstorrent::setup::{
    parse_config_file_comments, setup_default_schedule, setup_default_views, SetupError,
};
use rt_core::ViewManager;
use rt_rpc::CommandScheduler;
use std::fs::File;
use std::io::Write;

use std::sync::atomic::{AtomicUsize, Ordering};

static FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempConfigFile {
    path: String,
}

impl TempConfigFile {
    fn new(contents: &str) -> Self {
        let count = FILE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = format!("/tmp/rtorrent_test_setup_{}_{}", std::process::id(), count);
        let mut file = File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file.write_all(b"\n").unwrap();
        Self { path }
    }
}

impl Drop for TempConfigFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn test_config_comment_log_add_output() {
    let file = TempConfigFile::new("# do:log.add_output=debug_trace,test_output");
    assert_eq!(Ok(()), parse_config_file_comments(&file.path));
}

#[test]
fn test_config_comment_log_add_output_no_args() {
    let file = TempConfigFile::new("# do:log.add_output=");
    assert_eq!(
        Err(SetupError::InvalidArgCount),
        parse_config_file_comments(&file.path)
    );
}

#[test]
fn test_config_comment_log_add_output_one_arg() {
    let file = TempConfigFile::new("# do:log.add_output=debug_trace");
    assert_eq!(
        Err(SetupError::InvalidArgCount),
        parse_config_file_comments(&file.path)
    );
}

#[test]
fn test_config_comment_log_add_output_too_many_args() {
    let file = TempConfigFile::new("# do:log.add_output=debug_trace,test_output,extra");
    assert_eq!(
        Err(SetupError::InvalidArgCount),
        parse_config_file_comments(&file.path)
    );
}

#[test]
fn test_config_comment_invalid_format() {
    let file = TempConfigFile::new("# do:no_equal_sign");
    assert!(matches!(
        parse_config_file_comments(&file.path),
        Err(SetupError::InvalidCommand(_))
    ));
}

#[test]
fn test_config_comment_unknown_command() {
    let file = TempConfigFile::new("# do:unknown_cmd=123");
    assert!(matches!(
        parse_config_file_comments(&file.path),
        Err(SetupError::UnknownCommand(_))
    ));
}

#[test]
fn test_default_views_setup() {
    let mut vm = ViewManager::new();
    setup_default_views(&mut vm);

    assert!(vm.find("main").is_some());
    assert!(vm.find("started").is_some());
    assert!(vm.find("stopped").is_some());
    assert!(vm.find("complete").is_some());
    assert!(vm.find("incomplete").is_some());
    assert!(vm.find("hashing").is_some());
    assert!(vm.find("seeding").is_some());
    assert!(vm.find("leeching").is_some());
    assert!(vm.find("active").is_some());
}

#[test]
fn test_default_schedule_setup() {
    let mut sched = CommandScheduler::new();
    setup_default_schedule(&mut sched);

    assert!(sched.find("session_save").is_some());
    assert!(sched.find("low_diskspace").is_some());
}
