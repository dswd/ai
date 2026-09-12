use std::cmp::max;
use std::path::PathBuf;

fn bar(len: usize) -> String {
    (0..max(len, 3)).map(|_| "=").collect::<String>()
}

pub fn bar_line() -> String {
    bar(80)
}

pub fn bar_title(title: &str) -> String {
    format!(
        "{} {} {}",
        bar(10),
        title,
        bar(80usize.saturating_sub(title.len().saturating_add(12)))
    )
}

/// Expand a leading `~` (or `~/…`) to the user's home directory. Any other
/// path is returned unchanged. `~user` shorthand is not supported.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// Current UTC time as an ISO-8601 string.
pub fn now_iso() -> String {
    use time::OffsetDateTime;
    use time::format_description::FormatItem;
    use time::macros::format_description;

    let now = OffsetDateTime::now_utc();
    let fmt: &[FormatItem] = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    now.format(fmt).unwrap_or_default()
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

    #[test]
    fn test_bar_title_long_does_not_panic() {
        let title = "x".repeat(500);
        assert!(bar_title(&title).contains(&title));
    }

    #[test]
    fn test_expand_tilde() {
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expand_tilde("~"), home);
            assert_eq!(expand_tilde("~/x"), home.join("x"));
        }
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("rel/path"), PathBuf::from("rel/path"));
        assert_eq!(expand_tilde("~user/x"), PathBuf::from("~user/x"));
    }
}
