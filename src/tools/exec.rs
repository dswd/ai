use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, level_enabled, Level};

use super::truncate;
use crate::policy::{Action, Policy};
use std::io::Write;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteArgs {
    #[schemars(description = "The shell command to execute")]
    pub command: String,
    #[schemars(description = "Working directory for the command")]
    pub cwd: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("{0}")]
    Message(String),
}

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
    type Error = ExecError;

    fn description(&self) -> String {
        "Execute a shell command and return stdout and stderr".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ExecuteArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("\u{2699}  execute {}", args.command);
        let first_word = args
            .command
            .split_whitespace()
            .next()
            .unwrap_or(&args.command);

        if !self.policy.is_allowed(&Action::Execute, first_word) {
            return Err(ExecError::Message(format!(
                "execution denied for command: {}",
                first_word
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
            .map_err(|e| ExecError::Message(format!("execution failed: {e}")))?;

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
            result = format!("(exit code: {})", output.status.code().unwrap_or(-1));
        }

        let truncated = truncate(&result, 20, 500);
        if level_enabled!(Level::DEBUG) {
            let _ = writeln!(std::io::stderr(), "\x1b[34m  execute \u{2192}\n{truncated}\x1b[0m");
        }
        debug!("  execute \u{2192}\n{truncated}");
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffArgs {
    #[schemars(description = "Show staged changes (--staged)")]
    pub staged: Option<bool>,
    #[schemars(description = "Limit diff to a specific file path")]
    pub path: Option<String>,
}

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
    type Error = ExecError;

    fn description(&self) -> String {
        "Show git diff of working tree changes. Set staged=true to show staged changes."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(GitDiffArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("diff");
        cmd.arg("--no-color");

        if args.staged.unwrap_or(false) {
            cmd.arg("--staged");
        }

        if let Some(ref path) = args.path {
            cmd.arg("--").arg(path);
        }
        
        info!("\u{2699}  git diff {}", cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "));
        if !self.policy.is_allowed(&Action::Execute, "git") {
            return Err(ExecError::Message("execution denied for command: git".to_string()));
        }

        let output = cmd
            .output()
            .map_err(|e| ExecError::Message(format!("git diff failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result = if stdout.is_empty() {
            "No changes.".to_string()
        } else {
            stdout.to_string()
        };
        let truncated = truncate(&result, 20, 500);
        if level_enabled!(Level::DEBUG) {
            let _ = writeln!(std::io::stderr(), "\x1b[34m  git diff \u{2192}\n{truncated}\x1b[0m");
        }
        debug!("  git diff \u{2192}\n{truncated}");
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GitLogArgs {
    #[schemars(description = "Number of commits to show (default: 20)")]
    pub n: Option<usize>,
    #[schemars(description = "Limit log to a specific file path")]
    pub path: Option<String>,
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
    type Error = ExecError;

    fn description(&self) -> String {
        "Show git commit log (--oneline) with optional limit and path filter".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(GitLogArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("log");
        cmd.arg("--oneline");
        cmd.arg(format!("-n{}", args.n.unwrap_or(20)));

        if let Some(ref path) = args.path {
            cmd.arg("--").arg(path);
        }

        info!("\u{2699}  git log {}", cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "));
        if !self.policy.is_allowed(&Action::Execute, "git") {
            return Err(ExecError::Message("execution denied for command: git".to_string()));
        }

        let output = cmd
            .output()
            .map_err(|e| ExecError::Message(format!("git log failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result = if stdout.is_empty() {
            "No commits.".to_string()
        } else {
            stdout.to_string()
        };
        let truncated = truncate(&result, 20, 500);
        if level_enabled!(Level::DEBUG) {
            let _ = writeln!(std::io::stderr(), "\x1b[34m  git log \u{2192}\n{truncated}\x1b[0m");
        }
        debug!("  git log \u{2192}\n{truncated}");
        Ok(result)
    }
}
