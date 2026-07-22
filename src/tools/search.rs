use ansi_color_constants::*;
use log::{info, debug};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::util::{bar_line, bar_title};

use super::truncate;
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchContentArgs {
    #[schemars(description = "Directory or file glob to search in")]
    pub path: String,
    #[schemars(description = "Regex pattern to search for")]
    pub pattern: String,
    #[schemars(description = "Optional comma-separated file extensions (e.g. '.rs,.toml')")]
    pub file_types: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("{0}")]
    Message(String),
}

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
    type Error = SearchError;

    fn description(&self) -> String {
        "Search file contents by regex pattern. Returns matching file paths with line numbers and content."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(SearchContentArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}🔎  search for {:?} in {}{RESET}",
            args.pattern, args.path
        );
        let root = PathBuf::from(&args.path);
        let canonical_root = root
            .canonicalize()
            .map_err(|e| SearchError::Message(format!("cannot resolve path: {e}")))?;

        if !self
            .policy
            .is_allowed(&Action::Read, &canonical_root.to_string_lossy())
        {
            return Err(SearchError::Message(format!(
                "read access denied for: {}",
                args.path
            )));
        }

        let pattern = regex::Regex::new(&args.pattern)
            .map_err(|e| SearchError::Message(format!("invalid regex pattern: {e}")))?;

        let exts: Vec<&str> = args
            .file_types
            .as_ref()
            .map(|ft| {
                ft.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
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
            "No matches found.".to_string()
        } else {
            results.join("\n")
        };
        let truncated = truncate(&result, 20, 500);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title("search results"),
            bar_line()
        );
        Ok(result)
    }
}

fn search_file(
    path: &std::path::Path,
    pattern: &regex::Regex,
    exts: &[&str],
    max_file_size: usize,
    results: &mut Vec<String>,
    count: &mut usize,
    max_matches: usize,
) -> Result<(), SearchError> {
    if !exts.is_empty() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let matches_ext = exts
            .iter()
            .any(|ext| name.to_lowercase().ends_with(&ext.to_lowercase()));
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

    if content.len() > max_file_size {
        return Ok(());
    }

    if content.contains('\0') {
        return Ok(());
    }

    for (line_num, line) in content.lines().enumerate() {
        if *count >= max_matches {
            results.push("[... results truncated ...]".to_string());
            return Ok(());
        }
        if pattern.is_match(line) {
            results.push(format!("{}:{}: {}", path.display(), line_num + 1, line));
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
) -> Result<(), SearchError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        if name.starts_with('.') {
            if name != "." && name != ".." && name != ".cargo" {
                continue;
            }
        }

        if name == "node_modules" || name == "target" || name == ".git" {
            continue;
        }

        if path.is_dir() {
            walk_dir(
                root,
                &path,
                pattern,
                exts,
                max_file_size,
                results,
                count,
                max_matches,
            )?;
        } else if path.is_file() {
            search_file(
                &path,
                pattern,
                exts,
                max_file_size,
                results,
                count,
                max_matches,
            )?;
        }
    }

    Ok(())
}

fn is_binary_filename(name: &str) -> bool {
    const BINARY_EXTS: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico", ".webp", ".pdf", ".doc", ".docx", ".xls",
        ".xlsx", ".ppt", ".pptx", ".zip", ".tar", ".gz", ".bz2", ".xz", ".7z", ".rar", ".exe",
        ".dll", ".so", ".dylib", ".o", ".a", ".lib", ".bin", ".dat", ".class", ".pyc", ".pyo",
        ".wasm", ".mp3", ".mp4", ".avi", ".mov", ".mkv", ".wav", ".flac", ".ttf", ".otf", ".woff",
        ".woff2", ".eot", ".db", ".sqlite", ".sqlite3", ".mdb",
    ];
    let lower = name.to_lowercase();
    BINARY_EXTS.iter().any(|ext| lower.ends_with(ext))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FindFilesArgs {
    #[schemars(description = "Root directory to search from")]
    pub path: String,
    #[schemars(description = "Glob pattern (e.g. '**/*.rs', 'src/**/*.ts')")]
    pub pattern: String,
}

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
    type Error = SearchError;

    fn description(&self) -> String {
        "Recursively find files matching a glob pattern under a directory. Returns sorted list of relative paths."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(FindFilesArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}🔎  find {:?} in {}{RESET}",
            args.pattern, args.path
        );
        let root = PathBuf::from(&args.path);
        let canonical_root = root
            .canonicalize()
            .map_err(|e| SearchError::Message(format!("cannot resolve path: {e}")))?;

        if !self
            .policy
            .is_allowed(&Action::Read, &canonical_root.to_string_lossy())
        {
            return Err(SearchError::Message(format!(
                "read access denied for: {}",
                args.path
            )));
        }

        let pattern = canonical_root.join(&args.pattern);
        let pattern_str = pattern.to_string_lossy();

        let mut results: Vec<String> = glob::glob(&pattern_str)
            .map_err(|e| SearchError::Message(format!("invalid glob pattern: {e}")))?
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
            let result = "No files found.".to_string();
            debug!("{DIM}  \u{2192} {}{RESET}", result);
            return Ok(result);
        }

        results.sort();
        results.truncate(500);

        if results.len() >= 500 {
            results.push("[... results truncated ...]".to_string());
        }

        let result = results.join("\n");
        let truncated = truncate(&result, 20, 500);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title("find files"),
            bar_line()
        );
        Ok(result)
    }
}
