use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::{ToolError, search_file, walk_dir};
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchContentArgs {
    #[schemars(description = "Directory or file glob to search in")]
    pub path: String,
    #[schemars(description = "Regex pattern to search for")]
    pub pattern: String,
    #[schemars(description = "Optional comma-separated file extensions (e.g. '.rs,.toml')")]
    pub file_types: Option<String>,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of matches to return")]
    pub limit: Option<usize>,
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
    type Error = ToolError;

    fn description(&self) -> String {
        "Search file contents by regex pattern. Returns matching file paths with line numbers and content."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(SearchContentArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}🔎 search for {:?} in {}{}{RESET}",
            args.pattern,
            args.path,
            fmt_offset_limit(args.offset, args.limit)
        );
        let root = PathBuf::from(&args.path);
        let canonical_root = root
            .canonicalize()
            .map_err(|e| ToolError::Message(format!("cannot resolve path: {e}")))?;

        if !self
            .policy
            .is_allowed(&Action::Read, &canonical_root.to_string_lossy())
        {
            return Err(ToolError::Message(format!(
                "read access denied for: {}",
                args.path
            )));
        }

        let pattern = regex::Regex::new(&args.pattern)
            .map_err(|e| ToolError::Message(format!("invalid regex pattern: {e}")))?;

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
        let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title("search results"),
            bar_line()
        );
        process_output(&result, args.offset, args.limit).map_err(ToolError::Message)
    }
}
