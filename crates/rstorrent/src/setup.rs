use lt_common::Object;
use rt_core::ViewManager;
use rt_rpc::CommandScheduler;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

/// Default periodic session commit interval, matching the legacy
/// `schedule = session_save,1200,1200,((session.save))` entry.
pub const DEFAULT_SESSION_COMMIT_INTERVAL_SECS: u64 = 1200;

/// Default resume-only checkpoint interval for active downloads.
///
/// Bounds the progress lost on a crash to roughly this window without paying
/// the full session commit cost (torrent files included) every time.
pub const DEFAULT_SESSION_CHECKPOINT_INTERVAL_SECS: u64 = 300;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum SetupError {
    #[error("Invalid number of arguments.")]
    InvalidArgCount,
    #[error("Invalid command in config file comment: {0}")]
    InvalidCommand(String),
    #[error("Unknown command in config file comment: {0}")]
    UnknownCommand(String),
    #[error("Unknown log command: {0}")]
    UnknownLogCommand(String),
    #[error("IO error: {0}")]
    Io(String),
}

/// Parses special pre-config comments in the config file (# do:<command>=<args>)
pub fn parse_config_file_comments(path: &str) -> Result<(), SetupError> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(()), // Non-existing file is silently skipped
    };

    let reader = BufReader::new(file);

    for line_res in reader.lines() {
        let line = line_res.map_err(|e| SetupError::Io(e.to_string()))?;
        let trimmed = line.trim();

        if !trimmed.starts_with("# do:") {
            continue;
        }

        let content = &trimmed[5..]; // skip "# do:"
        let equal_pos = match content.find('=') {
            Some(p) => p,
            None => return Err(SetupError::InvalidCommand(line.clone())),
        };

        let command = &content[..equal_pos];
        let raw_args = &content[equal_pos + 1..];

        if command.is_empty() {
            return Err(SetupError::InvalidCommand(line.clone()));
        }

        let args: Vec<String> = if raw_args.is_empty() {
            Vec::new()
        } else {
            raw_args.split(',').map(|s| s.to_string()).collect()
        };

        if command.starts_with("log.") {
            match command {
                "log.add_output" => {
                    if args.len() != 2 {
                        return Err(SetupError::InvalidArgCount);
                    }
                }
                "log.open_file"
                | "log.open_file.flush"
                | "log.open_gz_file"
                | "log.open_file_pid"
                | "log.open_gz_file_pid"
                | "log.append_file"
                | "log.append_file.flush"
                | "log.append_gz_file" => {
                    if args.len() < 2 {
                        return Err(SetupError::InvalidArgCount);
                    }
                }
                _ => return Err(SetupError::UnknownLogCommand(command.to_string())),
            }
        } else {
            return Err(SetupError::UnknownCommand(line.clone()));
        }
    }

    Ok(())
}

/// Discovers the path to the configuration file (.rstorrent.rc or fallback .rtorrent.rc)
pub fn find_config_file(no_rc: bool) -> Option<PathBuf> {
    if no_rc {
        return None;
    }

    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            let p_rst = Path::new(&xdg).join("rstorrent/rstorrent.rc");
            if p_rst.exists() {
                return Some(p_rst);
            }
            let p_rt = Path::new(&xdg).join("rtorrent/rtorrent.rc");
            if p_rt.exists() {
                return Some(p_rt);
            }
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            let candidates = [
                Path::new(&home).join(".config/rstorrent/rstorrent.rc"),
                Path::new(&home).join(".config/rtorrent/rtorrent.rc"),
                Path::new(&home).join(".rstorrent.rc"),
                Path::new(&home).join(".rtorrent.rc"),
            ];
            for p in candidates {
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }

    // System and container paths (/config and /etc)
    let system_candidates = [
        PathBuf::from("/config/.rstorrent.rc"),
        PathBuf::from("/config/.rtorrent.rc"),
        PathBuf::from("/config/rstorrent.rc"),
        PathBuf::from("/config/rtorrent.rc"),
        PathBuf::from("/etc/rstorrent/rstorrent.rc"),
        PathBuf::from("/etc/rtorrent/rtorrent.rc"),
    ];
    for p in system_candidates {
        if p.exists() {
            return Some(p);
        }
    }

    None
}

/// Sets up default views according to rtorrent main.cc
pub fn setup_default_views(vm: &mut ViewManager) {
    let views = [
        "main",
        "default",
        "name",
        "active",
        "started",
        "stopped",
        "complete",
        "incomplete",
        "hashing",
        "seeding",
        "leeching",
    ];

    for name in views {
        if vm.find(name).is_none() {
            vm.insert(name);
        }
    }

    // Filters
    if let Some(v) = vm.find_mut("complete") {
        v.set_filter(Object::from("d.complete"));
    }
}

/// Registers (or re-arms) the periodic session commit task.
///
/// The interval is expressed in seconds. A zero interval is clamped to one
/// second so the scheduler can never re-arm itself into a zero-time loop.
pub fn setup_session_commit_schedule(sched: &mut CommandScheduler, interval: Duration) {
    let seconds = interval.as_secs().max(1);
    let seconds = seconds.to_string();
    let _ = sched.schedule(
        "session_save",
        &seconds,
        &seconds,
        Object::from("session.save="),
    );
}

/// Registers the resume-only checkpoint task.
///
/// Runs `session.checkpoint`, which rewrites `<hash>.rtorrent` and the resume
/// file for every active download but does not re-encode torrent files.
pub fn setup_session_checkpoint_schedule(sched: &mut CommandScheduler, interval: Duration) {
    let seconds = interval.as_secs().max(1);
    let seconds = seconds.to_string();
    let _ = sched.schedule(
        "session_checkpoint",
        &seconds,
        &seconds,
        Object::from("session.checkpoint="),
    );
}

/// Sets up default recurring schedule tasks.
pub fn setup_default_schedule(sched: &mut CommandScheduler) {
    setup_default_schedule_with_interval(
        sched,
        Duration::from_secs(DEFAULT_SESSION_COMMIT_INTERVAL_SECS),
    );
}

/// Sets up the default schedule with a caller-selected session commit interval.
///
/// This is the setup hook used by rtorrent when the session interval is made
/// configurable; keeping it in `setup.rs` avoids the UI/control path owning
/// scheduler details.
pub fn setup_default_schedule_with_interval(sched: &mut CommandScheduler, interval: Duration) {
    setup_session_commit_schedule(sched, interval);
    setup_session_checkpoint_schedule(
        sched,
        Duration::from_secs(DEFAULT_SESSION_CHECKPOINT_INTERVAL_SECS),
    );
    let _ = sched.schedule(
        "low_diskspace",
        "5",
        "60",
        Object::from("close_low_diskspace=500M"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(label: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rstorrent_setup_{}_{}.rc",
            label,
            std::process::id()
        ));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn config_comments_accept_known_commands_and_ignore_plain_comments() {
        let path = write_config(
            "valid",
            "# a plain comment\n# do:log.add_output=stdout,net\n# do:log.open_file=log,file\n",
        );
        assert!(parse_config_file_comments(path.to_str().unwrap()).is_ok());

        // Missing files are silently skipped.
        assert!(parse_config_file_comments("/nonexistent/rstorrent.rc").is_ok());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn config_comments_reject_malformed_entries() {
        let path = write_config("invalid_eq", "# do:log.add_output\n");
        assert_eq!(
            parse_config_file_comments(path.to_str().unwrap()),
            Err(SetupError::InvalidCommand(
                "# do:log.add_output".to_string()
            ))
        );

        let path = write_config("arg_count", "# do:log.add_output=one\n");
        assert_eq!(
            parse_config_file_comments(path.to_str().unwrap()),
            Err(SetupError::InvalidArgCount)
        );

        let path = write_config("open_file_args", "# do:log.open_file=one\n");
        assert_eq!(
            parse_config_file_comments(path.to_str().unwrap()),
            Err(SetupError::InvalidArgCount)
        );

        let path = write_config("unknown_log", "# do:log.unknown=x\n");
        assert_eq!(
            parse_config_file_comments(path.to_str().unwrap()),
            Err(SetupError::UnknownLogCommand("log.unknown".to_string()))
        );

        let path = write_config("unknown_cmd", "# do:session.save=x\n");
        assert_eq!(
            parse_config_file_comments(path.to_str().unwrap()),
            Err(SetupError::UnknownCommand(
                "# do:session.save=x".to_string()
            ))
        );
    }

    #[test]
    fn find_config_file_honours_no_rc() {
        assert!(find_config_file(true).is_none());
    }

    #[test]
    fn default_schedule_uses_legacy_session_interval() {
        let mut scheduler = CommandScheduler::new();
        setup_default_schedule(&mut scheduler);

        let item = scheduler.find("session_save").expect("session_save");
        assert_eq!(
            item.interval(),
            Duration::from_secs(DEFAULT_SESSION_COMMIT_INTERVAL_SECS)
        );
        assert_eq!(
            item.time_scheduled(),
            Duration::from_secs(DEFAULT_SESSION_COMMIT_INTERVAL_SECS)
        );
        assert!(item.command().as_str().unwrap().contains("session.save"));
    }

    #[test]
    fn default_schedule_adds_resume_checkpoint() {
        let mut scheduler = CommandScheduler::new();
        setup_default_schedule(&mut scheduler);

        let item = scheduler
            .find("session_checkpoint")
            .expect("session_checkpoint");
        assert_eq!(
            item.interval(),
            Duration::from_secs(DEFAULT_SESSION_CHECKPOINT_INTERVAL_SECS)
        );
        assert!(item
            .command()
            .as_str()
            .unwrap()
            .contains("session.checkpoint"));
    }

    #[test]
    fn custom_session_interval_is_honoured() {
        let mut scheduler = CommandScheduler::new();
        setup_default_schedule_with_interval(&mut scheduler, Duration::from_secs(90));

        let item = scheduler.find("session_save").expect("session_save");
        assert_eq!(item.interval(), Duration::from_secs(90));
        assert_eq!(item.time_scheduled(), Duration::from_secs(90));
    }
}
