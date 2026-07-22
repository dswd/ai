use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{finalize_output, resolve_path, ToolError};
use crate::policy::{Action, Policy};

// ---------------------------------------------------------------------------
// Args structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchContentArgs {
    pub path: String,
    pub pattern: String,
    pub file_types: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FindFilesArgs {
    pub path: String,
    pub pattern: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// SearchContentTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SearchContentTool {
    policy: Policy,
}

impl SearchContentTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for SearchContentTool {
    const NAME: &'static str = "search_content";

    type Args = SearchContentArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Search file contents by regex".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(SearchContentArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🔎 search {:?} in {}{RESET}", args.pattern, args.path);
        let canonical_root = resolve_path(&args.path, &self.policy, &Action::Read)?;

        let pattern = regex::Regex::new(&args.pattern)
            .map_err(|e| ToolError::Message(format!("invalid regex: {e}")))?;

        let exts: Vec<&str> = args
            .file_types
            .as_ref()
            .map(|ft| ft.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();

        let mut results = Vec::new();
        let max_matches = 500;
        let max_file_size = 1_000_000;

        if canonical_root.is_file() {
            search_file(
                &canonical_root,
                &pattern,
                &exts,
                max_file_size,
                &mut results,
                &mut 0,
                max_matches,
            )?;
        } else if canonical_root.is_dir() {
            walk_dir(
                &canonical_root,
                &canonical_root,
                &pattern,
                &exts,
                max_file_size,
                &mut results,
                &mut 0,
                max_matches,
            )?;
        }

        let result = if results.is_empty() {
            "No matches.".to_string()
        } else {
            results.join("\n")
        };
        finalize_output(&result, args.offset, args.limit, "search results")
    }
}

// ---------------------------------------------------------------------------
// FindFilesTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FindFilesTool {
    policy: Policy,
}

impl FindFilesTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for FindFilesTool {
    const NAME: &'static str = "find_files";

    type Args = FindFilesArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Find files by glob pattern".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(FindFilesArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🔎 find {:?} in {}{RESET}", args.pattern, args.path);
        let canonical_root = resolve_path(&args.path, &self.policy, &Action::Read)?;

        let full_pattern = canonical_root.join(&args.pattern);
        let mut results: Vec<String> = glob::glob(&full_pattern.to_string_lossy())
            .map_err(|e| ToolError::Message(format!("invalid glob: {e}")))?
            .filter_map(|entry| {
                entry.ok().map(|p| {
                    p.strip_prefix(&canonical_root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .to_string()
                })
            })
            .collect();

        if results.is_empty() {
            return finalize_output("No files found.", args.offset, args.limit, "find files");
        }

        results.sort();
        results.truncate(500);
        if results.len() >= 500 {
            results.push("[... truncated ...]".to_string());
        }

        let result = results.join("\n");
        finalize_output(&result, args.offset, args.limit, "find files")
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn search_file(
    path: &std::path::Path,
    pattern: &regex::Regex,
    exts: &[&str],
    max_file_size: usize,
    results: &mut Vec<String>,
    count: &mut usize,
    max_matches: usize,
) -> Result<(), ToolError> {
    if !exts.is_empty() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let matches_ext = exts.iter().any(|ext| name.to_lowercase().ends_with(&ext.to_lowercase()));
        if !matches_ext {
            return Ok(());
        }
    }

    if is_binary_filename(&path.to_string_lossy()) {
        return Ok(());
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    if content.len() > max_file_size || content.contains('\0') {
        return Ok(());
    }

    for (line_num, line) in content.lines().enumerate() {
        if *count >= max_matches {
            results.push("[... results truncated ...]".to_string());
            return Ok(());
        }
        if pattern.is_match(line) {
            results.push(format!("{}:{}:{}", path.display(), line_num + 1, line));
            *count += 1;
        }
    }
    Ok(())
}

fn walk_dir(
    root: &std::path::Path,
    dir: &std::path::Path,
    pattern: &regex::Regex,
    exts: &[&str],
    max_file_size: usize,
    results: &mut Vec<String>,
    count: &mut usize,
    max_matches: usize,
) -> Result<(), ToolError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        if name.starts_with('.') && name != "." && name != ".." && name != ".cargo" {
            continue;
        }
        if name == "node_modules" || name == "target" || name == ".git" {
            continue;
        }

        if path.is_dir() {
            walk_dir(root, &path, pattern, exts, max_file_size, results, count, max_matches)?;
        } else if path.is_file() {
            search_file(&path, pattern, exts, max_file_size, results, count, max_matches)?;
        }
    }
    Ok(())
}

fn is_binary_filename(name: &str) -> bool {
    const BINARY_EXTS: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico", ".webp", ".pdf",
        ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".zip", ".tar",
        ".gz", ".bz2", ".xz", ".7z", ".rar", ".exe", ".dll", ".so", ".dylib",
        ".o", ".a", ".lib", ".bin", ".dat", ".class", ".pyc", ".pyo", ".wasm",
        ".mp3", ".mp4", ".avi", ".mov", ".mkv", ".wav", ".flac", ".ttf", ".otf",
        ".woff", ".woff2", ".eot", ".db", ".sqlite", ".sqlite3", ".mdb",
    ];
    let lower = name.to_lowercase();
    BINARY_EXTS.iter().any(|ext| lower.ends_with(ext))
}
