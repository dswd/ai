use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GitLogArgs {
    #[schemars(description = "Number of commits to show (default: 20)")]
    pub n: Option<usize>,
    #[schemars(description = "Limit log to a specific file path")]
    pub path: Option<String>,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of lines to return")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct GitLogTool {
    policy: Policy,
}

impl GitLogTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for GitLogTool {
    const NAME: &'static str = "git_log";

    type Args = GitLogArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Show git commit log (--oneline).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(GitLogArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if !self.policy.is_allowed(&Action::Execute, "git") {
            return Err(ToolError::Message(
                "execution denied for command: git".to_string(),
            ));
        }

        info!(
            "{DIM}📜 git log -n{}{}{RESET}",
            args.n.unwrap_or(20),
            fmt_offset_limit(args.offset, args.limit)
        );

        let mut cmd = std::process::Command::new("git");
        cmd.arg("log");
        cmd.arg("--oneline");
        cmd.arg(format!("-n{}", args.n.unwrap_or(20)));

        if let Some(ref path) = args.path {
            cmd.arg("--").arg(path);
        }

        let output = cmd
            .output()
            .map_err(|e| ToolError::Message(format!("git log failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result = if stdout.trim().is_empty() {
            "No commits.".to_string()
        } else {
            stdout.to_string()
        };

        let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title("git log"),
            bar_line()
        );
        process_output(&result, args.offset, args.limit).map_err(ToolError::Message)
    }
}
