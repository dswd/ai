use std::time::SystemTime;

/// Short horizontal rule for debug logs (40 chars).
pub fn bar_line() -> String {
    "─".repeat(40)
}

/// Title surrounded by short bars.
pub fn bar_title(title: &str) -> String {
    let avail = 40usize.saturating_sub(title.len() + 4);
    format!("── {title} ──{}", "─".repeat(avail))
}

/// Current UTC time as ISO string, e.g. "2026-07-22T14:26:13.123Z".
pub fn now_iso() -> String {
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let millis = dur.subsec_millis();
    let tm = secs_to_utc(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        tm.0, tm.1, tm.2, tm.3, tm.4, tm.5, millis
    )
}

/// Current UTC time as short string, e.g. "2026-07-22 14:26 UTC".
pub fn now_short() -> String {
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let tm = secs_to_utc(secs);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", tm.0, tm.1, tm.2, tm.3, tm.4)
}

type UtcTime = (i64, u32, u32, u32, u32, u32);

fn secs_to_utc(secs: u64) -> UtcTime {
    let days = secs / 86400;
    let time = secs % 86400;
    let hour = (time / 3600) as u32;
    let min = ((time % 3600) / 60) as u32;
    let sec = (time % 60) as u32;

    let mut y = 1970i64;
    let mut d = days as i64;
    loop {
        let diy = if is_leap(y) { 366 } else { 365 };
        if d < diy {
            break;
        }
        d -= diy;
        y += 1;
    }

    let md = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let feb = if is_leap(y) { 29 } else { 28 };

    let mut m = 1u32;
    for (i, &days_in_month) in md.iter().enumerate() {
        let limit = if i == 1 { feb } else { days_in_month };
        if d < limit as i64 {
            break;
        }
        d -= limit as i64;
        m = i as u32 + 2;
    }

    (y, m, (d + 1) as u32, hour, min, sec)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
