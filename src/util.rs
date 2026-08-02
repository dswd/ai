use std::cmp::max;

fn bar(len: usize) -> String {
    (0..max(len, 3)).map(|_| "=").collect::<String>()
}

pub fn bar_line() -> String {
    bar(80)
}

pub fn bar_title(title: &str) -> String {
    format!("{} {} {}", bar(10), title, bar(80 - 12 - title.len()))
}

/// Format a byte count as a human-readable size (B, KB, MB, GB).
pub fn fmt_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", n as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
