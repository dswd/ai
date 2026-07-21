pub mod exec;
pub mod fs;
pub mod search;
pub mod think;
pub mod web;

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
