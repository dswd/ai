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

/// Apply line-based offset/limit to content, enforce hard output limits,
/// and append total line count. Returns an error for invalid ranges.
pub fn process_output(
    raw: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    let lines: Vec<&str> = raw.lines().collect();
    let total_lines = lines.len();

    let start = offset.unwrap_or(0);
    if start > total_lines {
        return Err(format!(
            "offset {start} is out of range: input has {total_lines} lines (valid: 0..{total_lines})"
        ));
    }

    let end = if let Some(n) = limit {
        if n == 0 {
            return Err("limit must be greater than 0".to_string());
        }
        (start + n).min(total_lines)
    } else {
        total_lines
    };

    let subset = &lines[start..end];
    let mut result = subset.join("\n");

    let ranged = start > 0 || end < total_lines;
    let shown_lines = subset.len();

    // apply hard caps
    let capped = shown_lines > MAX_OUTPUT_LINES || result.len() > MAX_OUTPUT_CHARS;

    if capped {
        let mut capped_out = String::with_capacity(MAX_OUTPUT_CHARS + 200);
        for line in subset.iter().take(MAX_OUTPUT_LINES) {
            capped_out.push_str(line);
            capped_out.push('\n');
        }
        if capped_out.len() > MAX_OUTPUT_CHARS {
            capped_out.truncate(MAX_OUTPUT_CHARS);
            if let Some(last_nl) = capped_out.rfind('\n') {
                capped_out.truncate(last_nl + 1);
            }
        }
        result = capped_out.trim_end_matches('\n').to_string();
        result.push_str(&format!(
            "\n\n[output capped at {MAX_OUTPUT_LINES} lines / {} kB — {total_lines} lines total; use offset/limit to read more]",
            MAX_OUTPUT_CHARS / 1024
        ));
    } else if ranged {
        result.push_str(&format!(
            "\n\n({total_lines} lines total, showing {}..{})",
            start + 1,
            start + shown_lines
        ));
    } else {
        result.push_str(&format!("\n\n({total_lines} lines total)"));
    }

    Ok(result)
}
