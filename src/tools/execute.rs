use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use bashkit::{
    Bash, Builtin, BuiltinContext, ExecResult, ExecutionLimits, FileSystem, PosixFs, RealFs,
    RealFsMode, async_trait,
};
use log::{debug, info};
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command as TokioCommand;

use super::policy_fs::PolicyFsBackend;
use super::shared::{ToolError, commands_in_string, is_bashkit_builtin};
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::container::ContainerSession;
use crate::policy::{Action, Policy};

/// A minimal environment for the virtual shell, seeded from the real process so
/// builtins and external commands agree on `HOME`, `USER`, `PATH`, and `PWD`.
/// Deliberately an allowlist: the shell's `env`/`printenv` output is visible to
/// the model, and the process environment holds provider API keys.
fn shell_env(cwd: &std::path::Path) -> Vec<(String, String)> {
    let mut env = Vec::new();
    let home = dirs::home_dir().or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from));
    if let Some(home) = home {
        env.push(("HOME".to_string(), home.to_string_lossy().into_owned()));
    }
    if let Some(user) = std::env::var_os("USER") {
        env.push(("USER".to_string(), user.to_string_lossy().into_owned()));
    }
    if let Some(path) = std::env::var_os("PATH") {
        env.push(("PATH".to_string(), path.to_string_lossy().into_owned()));
    }
    env.push(("PWD".to_string(), cwd.to_string_lossy().into_owned()));
    for key in ["LANG", "LC_ALL", "LC_CTYPE", "TERM", "SHELL"] {
        if let Some(value) = std::env::var_os(key) {
            env.push((key.to_string(), value.to_string_lossy().into_owned()));
        }
    }
    env
}

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
    session: Option<Arc<ContainerSession>>,
}

impl ExecuteTool {
    pub fn new(policy: Policy, session: Option<Arc<ContainerSession>>) -> Self {
        Self { policy, session }
    }
}

struct ExtBuiltin {
    name: String,
    policy: Policy,
    cwd: Option<std::path::PathBuf>,
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

        let mut command = {
            let mut c = TokioCommand::new(&self.name);
            c.args(ctx.args);
            // Keep the child in the working directory the shell was told to use,
            // so builtin and external path resolution agree.
            if let Some(dir) = &self.cwd {
                c.current_dir(dir);
            }
            c
        };

        match command
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
                result.stderr = stderr.into();
                result.exit_code = output.status.code().unwrap_or(-1);
                Ok(result)
            }
            Err(e) => Ok(ExecResult::err(format!("{}: command not found", e), 127)),
        }
    }
}

impl PortableTool for ExecuteTool {
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

        // Resolve the working directory once, against the real filesystem, and
        // reject a nonexistent one instead of silently running elsewhere. When
        // the caller does not pass one, use the agent's own cwd. bashkit
        // otherwise defaults to a fabricated `/home/user`, which makes builtins
        // and external commands resolve relative paths differently.
        let effective_cwd = match &args.cwd {
            Some(c) => {
                let p = std::path::PathBuf::from(c);
                if p.is_absolute() {
                    p
                } else {
                    std::env::current_dir().unwrap_or_default().join(p)
                }
            }
            None => std::env::current_dir().map_err(|e| {
                ToolError::Message(format!("cannot determine working directory: {e}"))
            })?,
        };
        if !effective_cwd.is_dir() {
            return Err(ToolError::Message(format!(
                "working directory does not exist: {}",
                effective_cwd.display()
            )));
        }

        // When a container session is active, run the whole command inside it:
        // bashkit and the in-process policy layer are bypassed; the container's
        // mounts and network are the boundary.
        if let Some(session) = &self.session {
            return call_in_container(session, &args, &effective_cwd, timeout_secs).await;
        }

        let full_command = args.command.clone();

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

        let mut builder = Bash::builder()
            .fs(fs)
            .limits(limits)
            .cwd(effective_cwd.clone());
        for (key, value) in shell_env(&effective_cwd) {
            builder = builder.env(key, value);
        }

        for name in &external_names {
            builder = builder.builtin(
                name.clone(),
                Box::new(ExtBuiltin {
                    name: name.clone(),
                    policy: policy.clone(),
                    cwd: Some(effective_cwd.clone()),
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

/// Run the whole command inside the session container via `sh -c`.
async fn call_in_container(
    session: &ContainerSession,
    args: &ExecuteArgs,
    cwd: &std::path::Path,
    timeout_secs: u64,
) -> Result<String, ToolError> {
    let cmd = session
        .exec_shell(&args.command, Some(cwd), timeout_secs)
        .map_err(ToolError::Message)?;
    let mut command = tokio::process::Command::from(cmd);
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ToolError::Message(format!("container exec failed: {e}")))?;
    let output =
        match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
            .await
        {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err(ToolError::Message(format!("container exec failed: {e}"))),
            Err(_) => {
                return Err(ToolError::Message(format!(
                    "execution timed out after {timeout_secs}s"
                )));
            }
        };

    let mut result = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("--- stderr ---\n");
        result.push_str(&stderr);
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
                    cwd: None,
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
                    cwd: None,
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
    async fn test_policy_fs_symlink_cannot_escape_read_policy() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("ai-symlink-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let allowed = base.join("allowed");
        let denied = base.join("denied");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&denied).unwrap();
        std::fs::write(denied.join("secret.txt"), "TOP-SECRET").unwrap();
        symlink(denied.join("secret.txt"), allowed.join("link.txt")).unwrap();

        let mut policy = Policy::default();
        policy.add_cli_rule(PolicyRule::Allow(
            Action::Read,
            format!("{}/**", allowed.display()),
        ));
        policy.add_cli_rule(PolicyRule::Deny(
            Action::Read,
            format!("{}/**", denied.display()),
        ));

        let fs_backend = RealFs::open("/", RealFsMode::ReadWrite).await.unwrap();
        let policy_backend = PolicyFsBackend::new(fs_backend, policy);
        let fs: Arc<dyn FileSystem> = Arc::new(PosixFs::new(policy_backend));
        let limits = ExecutionLimits {
            timeout: Duration::from_secs(10),
            ..Default::default()
        };
        let mut bash = Bash::builder().fs(fs).limits(limits).build();

        let result = bash
            .exec(&format!("cat {}", allowed.join("link.txt").display()))
            .await
            .unwrap();
        assert!(
            !result.stdout.contains("TOP-SECRET"),
            "symlink bypassed read policy: {:?}",
            result.stdout
        );
        let _ = std::fs::remove_dir_all(&base);
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
                    cwd: None,
                }),
            )
            .build();
        let result = bash.exec("sh -c 'exit 7'").await.unwrap();
        assert_eq!(result.exit_code, 7);
    }

    async fn run_tool(policy: Policy, command: &str, cwd: Option<String>) -> String {
        let tool = ExecuteTool::new(policy, None);
        tool.call(ExecuteArgs {
            command: command.to_string(),
            cwd,
            offset: None,
            limit: None,
            timeout: Some(10),
        })
        .await
        .expect("execute tool failed")
    }

    #[tokio::test]
    async fn test_execute_uses_process_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let out = run_tool(Policy::default(), "pwd", None).await;
        assert_eq!(
            out.lines().next().unwrap().trim(),
            cwd.to_string_lossy(),
            "bashkit must not fabricate /home/user"
        );
    }

    #[tokio::test]
    async fn test_execute_uses_requested_cwd() {
        let dir = std::env::temp_dir().join(format!("ai-exec-cwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = run_tool(
            Policy::default(),
            "pwd",
            Some(dir.to_string_lossy().into_owned()),
        )
        .await;
        assert_eq!(out.lines().next().unwrap().trim(), dir.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_execute_relative_read_uses_cwd() {
        let dir = std::env::temp_dir().join(format!("ai-exec-rel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rel.txt"), "RELATIVE-OK").unwrap();

        let mut policy = Policy::default();
        policy.add_cli_rule(PolicyRule::Allow(
            Action::Read,
            dir.to_string_lossy().into_owned(),
        ));
        let out = run_tool(
            policy,
            "cat rel.txt",
            Some(dir.to_string_lossy().into_owned()),
        )
        .await;
        assert!(out.contains("RELATIVE-OK"), "unexpected output: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_shell_env_seeds_home_and_pwd() {
        let cwd = std::path::Path::new("/tmp");
        let env = shell_env(cwd);
        assert!(env.iter().any(|(k, _)| k == "PWD"));
        assert!(env.iter().any(|(k, _)| k == "HOME"));
        assert_eq!(
            env.iter().find(|(k, _)| k == "PWD").unwrap().1,
            "/tmp".to_string()
        );
    }
}
