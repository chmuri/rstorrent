//! `convert.*` and legacy `to_*` formatting helpers.
//!
//! Output strings intentionally mirror the C++ `snprintf` formats, including
//! the fixed widths used by the TUI columns.

use std::time::{SystemTime, UNIX_EPOCH};

use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_value, CommandMap, RpcError};

fn value(obj: &Object) -> Result<i64, RpcError> {
    convert_to_value(obj, 10, 1)
}

fn format_time(timestamp: i64, local: bool, date: bool) -> String {
    let time = timestamp as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let result = unsafe {
        if local {
            libc::localtime_r(&time, &mut tm)
        } else {
            libc::gmtime_r(&time, &mut tm)
        }
    };
    if result.is_null() {
        return String::new();
    }

    if date {
        format!(
            "{:02}/{:02}/{:04}",
            tm.tm_mday,
            tm.tm_mon + 1,
            tm.tm_year + 1900
        )
    } else {
        format!("{:2}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

fn format_elapsed(timestamp: i64, now: i64) -> String {
    let elapsed = if timestamp >= 0 && timestamp <= now {
        (now - timestamp) as u64
    } else {
        0
    };
    format!(
        "{:2}:{:02}:{:02}",
        elapsed / 3600,
        (elapsed / 60) % 60,
        elapsed % 60
    )
}

fn format_kb(value: i64) -> String {
    format!("{:5.1}", value as f64 / (1 << 10) as f64)
}

fn format_mb(value: i64) -> String {
    format!("{:8.1}", value as f64 / (1 << 20) as f64)
}

fn format_xb(value: i64) -> String {
    const KB: i64 = 1 << 10;
    const MB: i64 = 1 << 20;
    const GB: i64 = 1 << 30;
    const TB: i64 = 1 << 40;

    if value < 1000 * KB {
        format!("{:5.1} KB", value as f64 / KB as f64)
    } else if value < 1000 * MB {
        format!("{:5.1} MB", value as f64 / MB as f64)
    } else if value < 1000 * GB {
        format!("{:5.1} GB", value as f64 / GB as f64)
    } else {
        format!("{:5.1} TB", value as f64 / TB as f64)
    }
}

fn format_throttle(value: i64) -> String {
    if value < 0 {
        "---".to_string()
    } else if value == 0 {
        "off".to_string()
    } else {
        format!("{:3}", value / (1 << 10))
    }
}

type ConvertCommand = (&'static str, fn(i64) -> String);

pub fn register_convert_commands(map: &mut CommandMap) -> Result<(), RpcError> {
    let commands: [ConvertCommand; 18] = [
        ("convert.gm_time", |v| format_time(v, false, false)),
        ("convert.gm_date", |v| format_time(v, false, true)),
        ("convert.time", |v| format_time(v, true, false)),
        ("convert.date", |v| format_time(v, true, true)),
        ("convert.elapsed_time", |v| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            format_elapsed(v, now)
        }),
        ("convert.kb", format_kb),
        ("convert.mb", format_mb),
        ("convert.xb", format_xb),
        ("convert.throttle", format_throttle),
        ("to_gm_time", |v| format_time(v, false, false)),
        ("to_gm_date", |v| format_time(v, false, true)),
        ("to_time", |v| format_time(v, true, false)),
        ("to_date", |v| format_time(v, true, true)),
        ("to_elapsed_time", |v| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            format_elapsed(v, now)
        }),
        ("to_kb", format_kb),
        ("to_mb", format_mb),
        ("to_xb", format_xb),
        ("to_throttle", format_throttle),
    ];

    for (name, formatter) in commands {
        map.insert_func(
            name,
            move |_target, obj| Ok(Object::from(formatter(value(obj)?))),
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "<value>",
            name,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gm_formats_match_legacy_widths() {
        assert_eq!(format_time(0, false, false), " 0:00:00");
        assert_eq!(format_time(0, false, true), "01/01/1970");
    }

    #[test]
    fn byte_and_throttle_formats_match_legacy() {
        assert_eq!(format_kb(1536), "  1.5");
        assert_eq!(format_mb(1536 * 1024), "     1.5");
        assert_eq!(format_xb(1536), "  1.5 KB");
        assert_eq!(format_xb(2 * 1024 * 1024), "  2.0 MB");
        assert_eq!(format_throttle(-1), "---");
        assert_eq!(format_throttle(0), "off");
        assert_eq!(format_throttle(1536), "  1");
    }

    #[test]
    fn elapsed_time_clamps_future_timestamps() {
        assert_eq!(format_elapsed(90, 100), " 0:00:10");
        assert_eq!(format_elapsed(101, 100), " 0:00:00");
        assert_eq!(format_elapsed(-5, 100), " 0:00:00");
    }
}
