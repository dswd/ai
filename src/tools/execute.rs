use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::shared::{ToolError, commands_in_string};
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::policy::{Action, Policy};
use tokio::time::timeout;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteArgs {
    #[schemars(description = "The shell command to execute")]
    pub command: String,
    #[schemars(description = "Working directory for the command")]
    pub cwd: Option<String>,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of lines to return")]
    pub limit: Option<usize>,
    #[schemars(description = "Optional timeout in seconds (max 300, default 30)")]
    pub timeout: Option<u64>,
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
    type Error = ToolError;

    fn description(&self) -> String {
        "Execute a shell command and return stdout and stderr".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ExecuteArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}🚀 execute {}{}{RESET}",
            args.command,
            fmt_offset_limit(args.offset, args.limit)
        );
        let commands = commands_in_string(&args.command);
        if commands.is_empty() {
            return Err(ToolError::Message(
                "no command found in execution string".to_string(),
            ));
        }
        for cmd in &commands {
            if !self.policy.is_allowed(&Action::Execute, cmd) {
                return Err(ToolError::Message(format!(
                    "execution denied for command: {cmd}"
                )));
            }
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

        let timeout_secs = args.timeout.unwrap_or(30).min(300);

        let child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::Message(format!("execution failed: {e}")))?;

        let child = Arc::new(Mutex::new(Some(child)));
        let child_for_task = child.clone();

        let result = timeout(
            Duration::from_secs(timeout_secs),
            tokio::task::spawn_blocking(move || {
                child_for_task
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .wait_with_output()
            }),
        )
        .await;

        let output = match result {
            Ok(Ok(Ok(out))) => out,
            Ok(Ok(Err(e))) => {
                return Err(ToolError::Message(format!("execution failed: {e}")));
            }
            Ok(Err(e)) => {
                return Err(ToolError::Message(format!("join error: {e}")));
            }
            Err(_elapsed) => {
                let mut guard = child.lock().unwrap();
                if let Some(mut c) = guard.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                return Err(ToolError::Message(format!(
                    "execution timed out after {timeout_secs}s"
                )));
            }
        };

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

        let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title(&args.command),
            bar_line()
        );
        process_output(&result, args.offset, args.limit).map_err(ToolError::Message)
    }
}
