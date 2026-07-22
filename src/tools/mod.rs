pub mod exec;
pub mod fs;
pub mod search;
pub mod think;
pub mod web;

/// Maximum output limits enforced for all tools.
pub const MAX_OUTPUT_LINES: usize = 200;
pub const MAX_OUTPUT_CHARS: usize = 100_000; // ~100 KB

/// Truncate a string for debug/log display.
pub fn truncate(s: &str, max_lines: usize, max_chars: usize) -> String {
    let line_count = s.lines().count();
    let char_count = s.len();
    if line_count <= max_lines && char_count <= max_chars {
        return s.to_string();
    }
    let mut out = String::with_capacity(max_chars + 4);
    for line in s.lines().take(max_lines) {
        out.push_str(line);
        out.push('\n');
    }
    if out.len() > max_chars {
        out.truncate(max_chars);
    }
    let trimmed = out.trim_end_matches('\n');
    format!("{trimmed}\n...")
}

/// Enforce hard limits on tool output: at most `MAX_OUTPUT_LINES` lines
/// and `MAX_OUTPUT_CHARS` bytes. Appends a notice with totals if truncated.
/// Also applies an optional per-call `max_lines` / `max_chars` if they are
/// stricter than the global limits.
pub fn enforce_output_limits(
    s: &str,
    call_max_lines: Option<usize>,
    call_max_chars: Option<usize>,
) -> String {
    let max_lines = call_max_lines
        .map(|m| m.min(MAX_OUTPUT_LINES))
        .unwrap_or(MAX_OUTPUT_LINES);
    let max_chars = call_max_chars
        .map(|m| m.min(MAX_OUTPUT_CHARS))
        .unwrap_or(MAX_OUTPUT_CHARS);

    let line_count = s.lines().count();
    let char_count = s.len();

    if line_count <= max_lines && char_count <= max_chars {
        return s.to_string();
    }

    let mut out = String::with_capacity(max_chars + 120);
    for line in s.lines().take(max_lines) {
        out.push_str(line);
        out.push('\n');
    }
    if out.len() > max_chars {
        out.truncate(max_chars);
        // re-truncate to whole lines if we chopped mid-line
        if let Some(last_newline) = out[..max_chars].rfind('\n') {
            out.truncate(last_newline + 1);
        } else {
            out.truncate(max_chars);
        }
    }
    let trimmed = out.trim_end_matches('\n');

    format!(
        "{trimmed}\n\n[truncated: {}/{} lines, {}/{} bytes shown]",
        trimmed.lines().count(),
        line_count,
        trimmed.len(),
        char_count,
    )
}

/// Truncate a single line to `max_len` chars for display.
pub fn truncate_line(line: &str, max_len: usize) -> String {
    if line.len() <= max_len {
        line.to_string()
    } else {
        format!("{}... ({} total chars)", &line[..max_len], line.len())
    }
}
