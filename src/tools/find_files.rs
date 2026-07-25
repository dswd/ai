use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FindFilesArgs {
    #[schemars(description = "Root directory to search from")]
    pub path: String,
    #[schemars(description = "Glob pattern (e.g. '**/*.rs', 'src/**/*.ts')")]
    pub pattern: String,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of files to return")]
    pub limit: Option<usize>,
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
    type Error = ToolError;

    fn description(&self) -> String {
        "Recursively find files matching a glob pattern under a directory. Returns sorted list of relative paths."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(FindFilesArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}🔎 find {:?} in {}{}{RESET}",
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

        let pattern = canonical_root.join(&args.pattern);
        let pattern_str = pattern.to_string_lossy();

        let mut results: Vec<String> = glob::glob(&pattern_str)
            .map_err(|e| ToolError::Message(format!("invalid glob pattern: {e}")))?
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
            return process_output(&result, args.offset, args.limit).map_err(ToolError::Message);
        }

        results.sort();
        results.truncate(500);

        if results.len() >= 500 {
            results.push("[... results truncated ...]".to_string());
        }

        let result = results.join("\n");
        let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title("find files"),
            bar_line()
        );
        process_output(&result, args.offset, args.limit).map_err(ToolError::Message)
    }
}
