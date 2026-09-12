use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::policy::Policy;
use crate::sandbox::Sandbox;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    #[schemars(description = "Path to the file to read")]
    pub path: String,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of lines to return")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ReadFileTool {
    policy: Policy,
}

impl ReadFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl PortableTool for ReadFileTool {
    const NAME: &'static str = "read_file";

    type Args = ReadFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Read the contents of a file at the given path".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ReadFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}📄 read file {}{}{RESET}",
            args.path,
            fmt_offset_limit(args.offset, args.limit)
        );
        let path = PathBuf::from(&args.path);
        let sandbox = Sandbox::new(self.policy.clone());
        let content = sandbox.read_to_string(&path)?;
        let truncated = truncate(&content, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title(&args.path),
            bar_line()
        );
        process_output(&content, args.offset, args.limit).map_err(ToolError::Message)
    }
}
