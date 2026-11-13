//! Error-path coverage for the `SysError` hierarchy (scheduler + watch).

use std::io;
use std::time::Duration;

use lt_sys::scheduler::{Scheduler, SchedulerEntry};
use lt_sys::watch::{flags, DirectoryEvents};
use lt_sys::SysError;

#[test]
fn scheduler_rejects_invalid_entry() {
    let mut scheduler = Scheduler::new();
    let mut entry = SchedulerEntry::new();

    let err = scheduler
        .wait_until(&mut entry, Duration::from_secs(1))
        .unwrap_err();
    assert!(matches!(err, SysError::InvalidEntry));
    assert!(!entry.is_scheduled());
}

#[test]
fn scheduler_rejects_double_schedule() {
    let mut scheduler = Scheduler::new();
    let mut entry = SchedulerEntry::with_slot(|| {});

    scheduler
        .wait_until(&mut entry, Duration::from_secs(1))
        .unwrap();
    assert!(entry.is_scheduled());

    let err = scheduler
        .wait_until(&mut entry, Duration::from_secs(2))
        .unwrap_err();
    assert!(matches!(err, SysError::AlreadyScheduled));
}

#[test]
fn scheduler_update_rejects_invalid_entry() {
    let mut scheduler = Scheduler::new();
    let mut entry = SchedulerEntry::new();

    let err = scheduler
        .update_wait_until(&mut entry, Duration::from_secs(1))
        .unwrap_err();
    assert!(matches!(err, SysError::InvalidEntry));
}

#[test]
fn scheduler_wait_for_schedules_valid_entry() {
    let mut scheduler = Scheduler::new();
    let mut entry = SchedulerEntry::with_slot(|| {});

    scheduler
        .wait_for(&mut entry, Duration::from_secs(1))
        .unwrap();
    assert!(entry.is_scheduled());
}

#[test]
fn watch_rejects_empty_path() {
    let mut events = DirectoryEvents::new();

    let err = events
        .notify_on("", flags::FLAG_ON_ADDED, |_| {})
        .unwrap_err();
    assert!(matches!(err, SysError::EmptyWatchPath));
}

#[test]
fn watch_reports_resolve_failure() {
    let mut events = DirectoryEvents::new();
    let missing = "/nonexistent-lt-sys-test-dir-xyz";

    let err = events
        .notify_on(missing, flags::FLAG_ON_ADDED, |_| {})
        .unwrap_err();
    match err {
        SysError::WatchResolve { path, source } => {
            assert_eq!(path.to_str(), Some(missing));
            assert_eq!(source.kind(), io::ErrorKind::NotFound);
        }
        other => panic!("expected WatchResolve, got {other:?}"),
    }
}

#[test]
fn watch_reports_flag_conflict() {
    let mut events = DirectoryEvents::new();
    let dir = std::env::temp_dir();

    events
        .notify_on(&dir, flags::FLAG_ON_READY, |_| {})
        .unwrap();

    let err = events
        .notify_on(&dir, flags::FLAG_ON_ADDED, |_| {})
        .unwrap_err();
    match err {
        SysError::WatchConflict {
            existing,
            requested,
        } => {
            assert_eq!(existing, requested);
            assert_eq!(existing, dir.canonicalize().unwrap());
        }
        other => panic!("expected WatchConflict, got {other:?}"),
    }
}

#[test]
fn watch_accepts_valid_directory() {
    let mut events = DirectoryEvents::new();

    events
        .notify_on(std::env::temp_dir(), flags::FLAG_ON_ADDED, |_| {})
        .unwrap();
}

#[test]
fn error_display_messages() {
    let resolve = SysError::WatchResolve {
        path: "/tmp/missing".into(),
        source: io::Error::new(io::ErrorKind::NotFound, "no such file"),
    };
    assert!(resolve
        .to_string()
        .contains("Could not resolve watch directory"));

    let conflict = SysError::WatchConflict {
        existing: "/tmp/a".into(),
        requested: "/tmp/b".into(),
    };
    assert!(conflict
        .to_string()
        .contains("Conflicting directory watch modes"));

    assert_eq!(
        SysError::EmptyWatchPath.to_string(),
        "Cannot add notification event for empty paths."
    );
    assert_eq!(
        SysError::AlreadyScheduled.to_string(),
        "Scheduler::wait_until called on already scheduled entry."
    );
    assert_eq!(
        SysError::InvalidEntry.to_string(),
        "Scheduler entry is invalid."
    );
}
