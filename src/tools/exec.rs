use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{finalize_output, ToolError};
use crate::policy::{Action, Policy};

// ---------------------------------------------------------------------------
// Args structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteArgs {
    pub command: String,
    pub cwd: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffArgs {
    pub staged: Option<bool>,
    pub path: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GitLogArgs {
    pub n: Option<usize>,
    pub path: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// ExecuteTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ExecuteTool {
    policy: Policy,
}

impl ExecuteTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for ExecuteTool {
    const NAME: &'static str = "execute";

    type Args = ExecuteArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Run a shell command".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ExecuteArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🚀 exec {}{RESET}", args.command);
        let first_word = args
            .command
            .split_whitespace()
            .next()
            .unwrap_or(&args.command);

        if !self.policy.is_allowed(&Action::Execute, first_word) {
            return Err(ToolError::Message(format!(
                "execution denied for: {first_word}"
            )));
        }

        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = std::process::Command::new("cmd");
            c.arg("/C").arg(&args.command);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(&args.command);
            c
        };

        if let Some(cwd) = &args.cwd {
            cmd.current_dir(cwd);
        }

        let output = cmd
            .output()
            .map_err(|e| ToolError::Message(format!("exec failed: {e}")))?;

        let mut result = String::new();
        if !output.stdout.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("--- stderr ---\n");
            result.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        if result.is_empty() {
            result = format!("(exit: {})", output.status.code().unwrap_or(-1));
        }

        finalize_output(&result, args.offset, args.limit, &args.command)
    }
}

// ---------------------------------------------------------------------------
// GitDiffTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GitDiffTool {
    policy: Policy,
}

impl GitDiffTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for GitDiffTool {
    const NAME: &'static str = "git_diff";

    type Args = GitDiffArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Show git diff (working tree or staged)".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(GitDiffArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if !self.policy.is_allowed(&Action::Execute, "git") {
            return Err(ToolError::Message("execution denied for: git".to_string()));
        }

        let mut cmd = std::process::Command::new("git");
        cmd.arg("diff").arg("--no-color");
        if args.staged.unwrap_or(false) {
            cmd.arg("--staged");
        }
        if let Some(ref p) = args.path {
            cmd.arg("--").arg(p);
        }

        info!(
            "{DIM}⚙  git diff {}{RESET}",
            cmd.get_args()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        );

        let output = cmd
            .output()
            .map_err(|e| ToolError::Message(format!("git diff failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result = if stdout.is_empty() {
            "No changes.".to_string()
        } else {
            stdout.to_string()
        };

        finalize_output(&result, args.offset, args.limit, "git diff")
    }
}

// ---------------------------------------------------------------------------
// GitLogTool
// ---------------------------------------------------------------------------

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
        "Show git log (--oneline)".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(GitLogArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if !self.policy.is_allowed(&Action::Execute, "git") {
            return Err(ToolError::Message("execution denied for: git".to_string()));
        }

        let mut cmd = std::process::Command::new("git");
        cmd.arg("log").arg("--oneline");
        cmd.arg(format!("-n{}", args.n.unwrap_or(20)));
        if let Some(ref p) = args.path {
            cmd.arg("--").arg(p);
        }

        info!(
            "{DIM}⚙  git log {}{RESET}",
            cmd.get_args()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        );

        let output = cmd
            .output()
            .map_err(|e| ToolError::Message(format!("git log failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result = if stdout.is_empty() {
            "No commits.".to_string()
        } else {
            stdout.to_string()
        };

        finalize_output(&result, args.offset, args.limit, "git log")
    }
}
