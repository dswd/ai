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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_bytes() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(1023), "1023 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(fmt_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn test_bar_functions() {
        assert!(bar_line().contains('='));
        assert!(bar_title("hello").contains("hello"));
        assert!(bar_title("x").len() >= 12);
    }
}
