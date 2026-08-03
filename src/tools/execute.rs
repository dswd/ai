use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use bashkit::{
    Bash, Builtin, BuiltinContext, ExecResult, ExecutionLimits, FileSystem, PosixFs, RealFs,
    RealFsMode, async_trait,
};
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command as TokioCommand;

use super::policy_fs::PolicyFsBackend;
use super::shared::{ToolError, commands_in_string, is_bashkit_builtin};
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::policy::{Action, Policy};

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

struct ExtBuiltin {
    name: String,
    policy: Policy,
}

#[async_trait]
impl Builtin for ExtBuiltin {
    async fn execute(&self, ctx: BuiltinContext<'_>) -> bashkit::Result<ExecResult> {
        if !self.policy.is_allowed(&Action::Execute, &self.name) {
            return Ok(ExecResult::err(
                format!("command denied by policy: {}", self.name),
                1,
            ));
        }
        match TokioCommand::new(&self.name)
            .args(ctx.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => {
                let output = match child.wait_with_output().await {
                    Ok(o) => o,
                    Err(e) => {
                        return Ok(ExecResult::err(format!("failed to collect output: {e}"), 1));
                    }
                };
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let mut result = ExecResult::ok(stdout);
                result.stderr = stderr;
                result.exit_code = output.status.code().unwrap_or(-1);
                Ok(result)
            }
            Err(e) => Ok(ExecResult::err(format!("{}: command not found", e), 127)),
        }
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

        let timeout_secs = args.timeout.unwrap_or(30).min(300);

        let full_command = if let Some(ref cwd) = args.cwd {
            format!("cd {} && {}", cwd, args.command)
        } else {
            args.command.clone()
        };

        let commands = commands_in_string(&full_command);
        if commands.is_empty() {
            return Err(ToolError::Message(
                "no command found in execution string".to_string(),
            ));
        }

        let policy = self.policy.clone();

        let external_names: Vec<String> = commands
            .iter()
            .filter(|c| !is_bashkit_builtin(c))
            .map(|c| c.to_string())
            .collect();

        let fs_backend = RealFs::open("/", RealFsMode::ReadWrite)
            .await
            .map_err(|e| ToolError::Message(format!("filesystem backend init failed: {e}")))?;
        let policy_backend = PolicyFsBackend::new(fs_backend, policy.clone());
        let fs: Arc<dyn FileSystem> = Arc::new(PosixFs::new(policy_backend));

        let limits = ExecutionLimits {
            timeout: Duration::from_secs(timeout_secs),
            ..Default::default()
        };

        let mut builder = Bash::builder().fs(fs).limits(limits);

        for name in &external_names {
            builder = builder.builtin(
                name.clone(),
                Box::new(ExtBuiltin {
                    name: name.clone(),
                    policy: policy.clone(),
                }),
            );
        }

        let mut bash = builder.build();
        let result = bash.exec(&full_command).await;

        let output = match result {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("timed out") || msg.contains("timeout") {
                    return Err(ToolError::Message(format!(
                        "execution timed out after {timeout_secs}s"
                    )));
                }
                return Err(ToolError::Message(format!("bashkit error: {msg}")));
            }
        };

        let mut result = String::new();
        if !output.stdout.is_empty() {
            result.push_str(&output.stdout);
        }
        if !output.stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("--- stderr ---\n");
            result.push_str(&output.stderr);
        }
        if result.is_empty() {
            result = format!("(exit code: {})", output.exit_code);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyRule;
    use bashkit::{Bash, ExecutionLimits, FileSystem, PosixFs, RealFs, RealFsMode};
    use std::sync::Arc;
    use std::time::Duration;

    async fn test_bash() -> Bash {
        let fs_backend = RealFs::open("/", RealFsMode::ReadWrite).await.unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(PosixFs::new(fs_backend));
        let limits = ExecutionLimits {
            timeout: Duration::from_secs(10),
            ..Default::default()
        };
        Bash::builder().fs(fs).limits(limits).build()
    }

    #[tokio::test]
    async fn test_bashkit_echo() {
        let mut bash = test_bash().await;
        let result = bash.exec("echo 'hello bashkit'").await.unwrap();
        assert_eq!(result.stdout.trim(), "hello bashkit");
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_bashkit_pipeline() {
        let mut bash = test_bash().await;
        let result = bash
            .exec("echo -e 'apple\nbanana\ncherry' | grep a")
            .await
            .unwrap();
        assert!(result.stdout.contains("apple"));
        assert!(result.stdout.contains("banana"));
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_bashkit_cat() {
        let mut bash = test_bash().await;
        let result = bash.exec("cat /etc/hostname").await.unwrap();
        assert!(!result.stdout.trim().is_empty());
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_bashkit_exit_code() {
        let mut bash = test_bash().await;
        let result = bash.exec("false").await.unwrap();
        assert_eq!(result.exit_code, 1);
    }

    fn test_bash_with_limits(timeout: Duration) -> bashkit::BashBuilder {
        let fs_backend = RealFs::open("/", RealFsMode::ReadWrite);
        let fs_backend = futures::executor::block_on(fs_backend).unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(PosixFs::new(fs_backend));
        let limits = ExecutionLimits {
            timeout,
            ..Default::default()
        };
        Bash::builder().fs(fs).limits(limits)
    }

    fn allow_execute(policy: &Policy, name: &str) -> Policy {
        let mut p = policy.clone();
        p.add_cli_rule(PolicyRule::Allow(Action::Execute, name.to_string()));
        p
    }

    #[tokio::test]
    async fn test_ext_builtin_fast_command() {
        let policy = allow_execute(&Policy::default(), "echo");
        let mut bash = test_bash_with_limits(Duration::from_secs(10))
            .builtin(
                "echo",
                Box::new(ExtBuiltin {
                    name: "echo".to_string(),
                    policy,
                }),
            )
            .build();
        let result = bash.exec("echo hello").await.unwrap();
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.exit_code, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_ext_builtin_timeout_kills_process() {
        // External commands must be killed by the bashkit timeout, not run to
        // completion. Before the tokio::process rewrite, the blocking
        // wait_with_output() never yielded, so `sleep 10` would run the full
        // 10s and only then report a timeout.
        let policy = allow_execute(&Policy::default(), "sleep");
        let mut bash = test_bash_with_limits(Duration::from_millis(500))
            .builtin(
                "sleep",
                Box::new(ExtBuiltin {
                    name: "sleep".to_string(),
                    policy,
                }),
            )
            .build();

        let start = std::time::Instant::now();
        let result = bash.exec("sleep 10").await;
        let elapsed = start.elapsed();
        assert!(result.is_err(), "expected timeout, got {result:?}");
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout did not interrupt the process promptly: {elapsed:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_ext_builtin_exit_code() {
        let policy = allow_execute(&Policy::default(), "sh");
        let mut bash = test_bash_with_limits(Duration::from_secs(10))
            .builtin(
                "sh",
                Box::new(ExtBuiltin {
                    name: "sh".to_string(),
                    policy,
                }),
            )
            .build();
        let result = bash.exec("sh -c 'exit 7'").await.unwrap();
        assert_eq!(result.exit_code, 7);
    }
}
