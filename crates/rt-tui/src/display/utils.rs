pub fn format_hhmmss(t: u64) -> String {
    let hours = t / 3600;
    let mins = (t / 60) % 60;
    let secs = t % 60;
    format!("{:2}:{:02}:{:02}", hours, mins, secs)
}

pub fn format_ddhhmm(t: u64) -> String {
    let days = t / (24 * 3600);
    if days < 100 {
        let hours = (t / 3600) % 24;
        let mins = (t / 60) % 60;
        format!("{:2}d {:2}:{:02}", days, hours, mins)
    } else {
        "--d --:--".to_string()
    }
}

pub fn format_ddmmyyyy(timestamp: u64) -> String {
    // Break down timestamp (UTC)
    let secs = timestamp;
    let days = secs / 86400;
    let mut year = 1970;
    let mut d = days;

    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if leap { 366 } else { 365 };
        if d < days_in_year {
            break;
        }
        d -= days_in_year;
        year += 1;
    }

    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let days_in_months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];

    let mut month = 1;
    for &m_days in &days_in_months {
        if d < m_days {
            break;
        }
        d -= m_days;
        month += 1;
    }
    let day = d + 1;

    format!("{:02}/{:02}/{:04}", day, month, year)
}

pub fn format_kb(val: i64) -> String {
    format!("{:5.1}", (val as f64) / 1024.0)
}

pub fn format_mb(val: i64) -> String {
    format!("{:8.1}", (val as f64) / (1024.0 * 1024.0))
}

pub fn format_xb(val: i64) -> String {
    let fval = val as f64;
    if val < (1000i64 << 10) {
        format!("{:5.1} KB", fval / 1024.0)
    } else if val < (1000i64 << 20) {
        format!("{:5.1} MB", fval / (1024.0 * 1024.0))
    } else if val < (1000i64 << 30) {
        format!("{:5.1} GB", fval / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{:5.1} TB", fval / (1024.0 * 1024.0 * 1024.0 * 1024.0))
    }
}

pub fn format_timer(val: i64) -> String {
    if val <= 0 {
        "--:--:--".to_string()
    } else {
        format!("{:2}:{:02}:{:02}", val / 3600, (val / 60) % 60, val % 60)
    }
}

pub fn format_download_time_left(bytes_left: u64, rate: u64) -> String {
    if rate < 512 {
        "--d --:--".to_string()
    } else {
        let rate_masked = rate & !(512 - 1);
        if rate_masked == 0 {
            "--d --:--".to_string()
        } else {
            let remaining = bytes_left / rate_masked;
            format_ddhhmm(remaining)
        }
    }
}

pub fn format_percentage(done: u64, total: u64) -> String {
    if total == 0 {
        "[ 0%]".to_string()
    } else {
        format!("[{:2}%]", (done * 100) / total)
    }
}
