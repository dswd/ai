use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::{ToolError, find_git_dir};
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::exec_sandbox::{self, SandboxSpec};
use crate::policy::{Action, Policy};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffArgs {
    #[schemars(description = "Show staged changes (--staged)")]
    pub staged: Option<bool>,
    #[schemars(description = "Limit diff to a specific file path")]
    pub path: Option<String>,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of lines to return")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct GitDiffTool {
    policy: Policy,
    sandbox: Option<Arc<SandboxSpec>>,
}

impl GitDiffTool {
    pub fn new(policy: Policy, sandbox: Option<Arc<SandboxSpec>>) -> Self {
        Self { policy, sandbox }
    }
}

impl Tool for GitDiffTool {
    const NAME: &'static str = "git_diff";

    type Args = GitDiffArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Show git diff of working tree changes.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(GitDiffArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}🔀 git diff{}{}{RESET}",
            args.staged
                .map_or(String::new(), |_| " --staged".to_string()),
            fmt_offset_limit(args.offset, args.limit)
        );

        let _git_dir = find_git_dir()?;
        // `git` is an external process: gate it on Execute rather than a read of
        // `.git`, since a repo can run code via hooks / external diff drivers.
        if !self.policy.is_allowed(&Action::Execute, "git") {
            return Err(ToolError::Message(
                "execute access denied for: git (re-run with -x=git)".to_string(),
            ));
        }

        let mut cmd = exec_sandbox::command("git", self.sandbox.as_deref());
        if self.sandbox.is_some() {
            // The sandbox denies the user's dotfiles; don't let git trip over an
            // unreadable global config.
            cmd.env("GIT_CONFIG_NOSYSTEM", "1");
            cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
        }
        cmd.arg("diff");
        cmd.arg("--no-color");

        if args.staged.unwrap_or(false) {
            cmd.arg("--staged");
        }
        if let Some(ref path) = args.path {
            cmd.arg("--").arg(path);
        }

        let output = cmd
            .output()
            .map_err(|e| ToolError::Message(format!("git diff failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result = if stdout.trim().is_empty() {
            "No changes.".to_string()
        } else {
            stdout.to_string()
        };

        let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title("git diff"),
            bar_line()
        );
        process_output(&result, args.offset, args.limit).map_err(ToolError::Message)
    }
}
