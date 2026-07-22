pub mod exec;
pub mod fs;
pub mod search;
pub mod think;
pub mod web;

use crate::policy::{Action, Policy};
use log::debug;
use std::path::PathBuf;

/// Maximum output limits enforced for all tools.
pub const MAX_OUTPUT_LINES: usize = 200;
pub const MAX_OUTPUT_CHARS: usize = 100_000;

/// Unified error type for all tools.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Message(String),
}

/// Resolve a path, check it against policy, return the canonical path.
pub fn resolve_path(path: &str, policy: &Policy, action: &Action) -> Result<PathBuf, ToolError> {
    let p = PathBuf::from(path);
    let canonical = p
        .canonicalize()
        .map_err(|e| ToolError::Message(format!("cannot resolve path: {e}")))?;
    if !policy.is_allowed(action, &canonical.to_string_lossy()) {
        return Err(ToolError::Message(format!("access denied for: {path}")));
    }
    Ok(canonical)
}

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

/// Apply line-based offset/limit to content, enforce hard output limits,
/// and append a compact total line count. Returns an error for invalid ranges.
pub fn process_output(
    raw: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, ToolError> {
    let lines: Vec<&str> = raw.lines().collect();
    let total = lines.len();

    let start = offset.unwrap_or(0);
    if start > total {
        return Err(ToolError::Message(format!(
            "offset {start} out of range: input has {total} lines (valid 0..{total})"
        )));
    }

    let end = if let Some(n) = limit {
        if n == 0 {
            return Err(ToolError::Message("limit must be > 0".to_string()));
        }
        (start + n).min(total)
    } else {
        total
    };

    let subset = &lines[start..end];
    let mut result = subset.join("\n");
    let shown = subset.len();

    // Apply hard caps silently — no banner for first-time hits
    let capped = shown > MAX_OUTPUT_LINES || result.len() > MAX_OUTPUT_CHARS;

    if capped {
        let mut buf = String::with_capacity(MAX_OUTPUT_CHARS + 100);
        for line in subset.iter().take(MAX_OUTPUT_LINES) {
            buf.push_str(line);
            buf.push('\n');
        }
        if buf.len() > MAX_OUTPUT_CHARS {
            buf.truncate(MAX_OUTPUT_CHARS);
            if let Some(nl) = buf.rfind('\n') {
                buf.truncate(nl + 1);
            }
        }
        result = buf.trim_end_matches('\n').to_string();
        result.push_str(&format!(
            "\n[capped: {}/{} lines {}K/{}K]",
            shown.min(MAX_OUTPUT_LINES),
            total,
            result.len() / 1024,
            MAX_OUTPUT_CHARS / 1024
        ));
    } else if start > 0 || end < total {
        result.push_str(&format!("\n({total} lines, {}..{})", start + 1, start + shown));
    } else {
        // Only add line count if there's more than one line, keep it compact
        if total > 1 {
            result.push_str(&format!("\n({total} lines)"));
        }
    }

    Ok(result)
}

/// Log debug info and return processed output (common tail of every tool).
pub fn finalize_output(
    raw: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    label: &str,
) -> Result<String, ToolError> {
    let truncated = truncate(raw, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
    debug!("  {label} \n{truncated}");
    process_output(raw, offset, limit)
}
