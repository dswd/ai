//! Container-based isolation for external commands.
//!
//! When a container image is configured, a single container is started at agent
//! startup and every `execute` command runs inside it via `exec`. The host
//! filesystem is invisible except for the policy-granted
//! read/write roots, which are bind-mounted read-only / read-write respectively.
//! Network is disabled unless the policy grants web access.

use crate::policy::{Action, Policy};
use ansi_color_constants::*;
use log::info;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

const KEEPALIVE: &str = "while :; do sleep 86400; done";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Runtime {
    Docker,
    Podman,
}

impl Runtime {
    pub(crate) fn binary(self) -> &'static str {
        match self {
            Runtime::Docker => "docker",
            Runtime::Podman => "podman",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Network {
    /// `--network none`.
    None,
    /// Use the runtime default (bridge).
    Default,
    /// `--network host`.
    Host,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Mounts {
    pub read: Vec<PathBuf>,
    pub write: Vec<PathBuf>,
    pub deny: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct ContainerRuntime {
    pub image: String,
    pub runtime: Runtime,
    pub network: Network,
    pub mounts: Mounts,
}

/// Detect a usable container runtime. `preferred` may be `"docker"`, `"podman"`,
/// or `None`/`"auto"`.
pub(crate) fn detect_runtime(preferred: Option<&str>) -> Option<Runtime> {
    let usable = |bin: &str| {
        Command::new(bin)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    match preferred.map(str::to_lowercase).as_deref() {
        Some("docker") => usable("docker").then_some(Runtime::Docker),
        Some("podman") => usable("podman").then_some(Runtime::Podman),
        Some(_) => None,
        None => {
            if usable("docker") {
                Some(Runtime::Docker)
            } else if usable("podman") {
                Some(Runtime::Podman)
            } else {
                None
            }
        }
    }
}

/// Derive bind mounts (and deny masks) from the policy allow/deny patterns.
pub(crate) fn mounts_from_policy(policy: &Policy) -> (Mounts, Vec<String>) {
    let mut warnings = Vec::new();
    let mut mounts = Mounts::default();

    for pattern in policy.allow_patterns(&Action::Read) {
        add_root(&pattern, &mut mounts.read, "read", &mut warnings);
    }
    for pattern in policy.allow_patterns(&Action::Write) {
        add_root(&pattern, &mut mounts.read, "read", &mut warnings);
        add_root(&pattern, &mut mounts.write, "write", &mut warnings);
    }

    for pattern in policy
        .deny_patterns(&Action::Read)
        .into_iter()
        .chain(policy.deny_patterns(&Action::Write))
    {
        match static_prefix(&pattern) {
            Some(p) if p.is_dir() => mounts.deny.push(p),
            Some(p) => warnings.push(format!(
                "container: cannot mask denied file '{}' (only directories can be masked); it is not mounted unless a broader allow covers it",
                p.display()
            )),
            None => warnings.push(format!(
                "container: cannot represent deny rule '{pattern}' (a whole-filesystem pattern)"
            )),
        }
    }

    dedup(&mut mounts.read);
    dedup(&mut mounts.write);
    dedup(&mut mounts.deny);
    (mounts, warnings)
}

fn add_root(pattern: &str, out: &mut Vec<PathBuf>, kind: &str, warnings: &mut Vec<String>) {
    match static_prefix(pattern) {
        Some(path) => {
            if path.exists() {
                out.push(path);
            } else {
                warnings.push(format!(
                    "container mount: granted {kind} path '{pattern}' does not exist and was skipped"
                ));
            }
        }
        None => warnings.push(format!(
            "container mount: a {kind} grant of '{pattern}' spans the whole filesystem and is not \
             mounted (bind-mounting / would shadow the container rootfs); grant specific paths \
             with -r/-w for the container to see host files"
        )),
    }
}

/// The static path prefix of a policy pattern, or `None` for `*`/`**`/`/`.
fn static_prefix(pattern: &str) -> Option<PathBuf> {
    if pattern == "*" || pattern == "**" || pattern == "/" {
        return None;
    }
    let prefix = match pattern.find('*') {
        Some(idx) => pattern[..idx].trim_end_matches('/'),
        None => pattern,
    };
    if prefix.is_empty() {
        None
    } else {
        Some(PathBuf::from(prefix))
    }
}

fn dedup(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}

fn covers_mounts(mounts: &Mounts, path: &Path) -> bool {
    mounts
        .read
        .iter()
        .chain(mounts.write.iter())
        .any(|root| path == root || path.starts_with(root))
}

fn uid_gid() -> Option<(u32, u32)> {
    #[cfg(unix)]
    unsafe {
        Some((libc::getuid(), libc::getgid()))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn apply_network(cmd: &mut Command, network: Network) {
    match network {
        Network::None => {
            cmd.arg("--network").arg("none");
        }
        Network::Host => {
            cmd.arg("--network").arg("host");
        }
        Network::Default => {}
    }
}

fn apply_user(cmd: &mut Command, runtime: Runtime) {
    match runtime {
        Runtime::Docker => {
            if let Some((uid, gid)) = uid_gid() {
                cmd.arg("--user").arg(format!("{uid}:{gid}"));
            }
        }
        Runtime::Podman => {
            cmd.arg("--userns").arg("keep-id");
        }
    }
}

fn append_mounts(cmd: &mut Command, mounts: &Mounts) {
    for root in &mounts.read {
        cmd.arg("-v")
            .arg(format!("{}:{}:ro", root.display(), root.display()));
    }
    for root in &mounts.write {
        cmd.arg("-v")
            .arg(format!("{}:{}", root.display(), root.display()));
    }
    for denied in &mounts.deny {
        cmd.arg("--tmpfs").arg(denied);
    }
}

/// Build the `run -d` invocation that creates the session container.
fn build_run_command(rt: &ContainerRuntime, name: &str) -> Command {
    let mut cmd = Command::new(rt.runtime.binary());
    cmd.arg("run").arg("-d").arg("--rm");
    cmd.arg("--name").arg(name);
    cmd.arg("--label").arg(format!("ai.session={name}"));
    apply_network(&mut cmd, rt.network);
    cmd.arg("--security-opt").arg("no-new-privileges");
    apply_user(&mut cmd, rt.runtime);
    append_mounts(&mut cmd, &rt.mounts);
    cmd.arg("--entrypoint")
        .arg("sh")
        .arg(&rt.image)
        .arg("-c")
        .arg(KEEPALIVE);
    cmd
}

/// Build a `exec` invocation running `program` with `args`.
fn build_exec_program(
    rt: &ContainerRuntime,
    name: &str,
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(rt.runtime.binary());
    cmd.arg("exec").arg("-i");
    if let Some(dir) = cwd
        && covers_mounts(&rt.mounts, dir)
    {
        cmd.arg("-w").arg(dir);
    }
    cmd.arg(name).arg(program).args(args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Build a `exec` invocation running a whole shell command via `sh -c`.
fn build_exec_shell(
    rt: &ContainerRuntime,
    name: &str,
    has_timeout: bool,
    command: &str,
    cwd: Option<&Path>,
    timeout_secs: u64,
) -> Command {
    let script = if has_timeout {
        let quoted = format!("'{}'", command.replace('\'', "'\\''"));
        format!("timeout {timeout_secs} sh -c {quoted}")
    } else {
        command.to_string()
    };
    build_exec_program(rt, name, "sh", &["-c".to_string(), script], cwd)
}

#[derive(Debug)]
struct Inner {
    name: String,
    has_timeout: bool,
    restarted: bool,
}

/// A live, session-scoped container. Started eager, `exec`'d into for every
/// command, and removed on drop.
#[derive(Debug)]
pub(crate) struct ContainerSession {
    rt: ContainerRuntime,
    inner: Mutex<Inner>,
}

impl ContainerSession {
    /// Start the container (detached) and probe for `timeout`.
    pub(crate) fn start(rt: ContainerRuntime) -> Result<Self, String> {
        let name = session_name();
        run_detached(&rt, &name)?;
        info!(
            "{DIM}📦 container started '{}' ({} via {}){RESET}",
            name,
            rt.image,
            rt.runtime.binary()
        );
        let has_timeout = probe_timeout(&rt, &name);
        Ok(Self {
            rt,
            inner: Mutex::new(Inner {
                name,
                has_timeout,
                restarted: false,
            }),
        })
    }

    /// Ensure the container is running, restarting it once if it died.
    fn ensure_running(&self) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "container session lock poisoned".to_string())?;
        if is_running(&self.rt, &inner.name) {
            return Ok(());
        }
        if inner.restarted {
            return Err(format!("container '{}' is not running", inner.name));
        }
        let name = session_name();
        run_detached(&self.rt, &name)?;
        info!(
            "{DIM}📦 container restarted '{}' ({} via {}){RESET}",
            name,
            self.rt.image,
            self.rt.runtime.binary()
        );
        inner.name = name;
        inner.has_timeout = probe_timeout(&self.rt, &inner.name);
        inner.restarted = true;
        crate::output::stderr_line("warning: exec container exited; restarted it");
        Ok(())
    }

    /// A command that runs the whole `command` string via `sh -c` inside the
    /// container. `cwd` is applied only when it is under a mounted root.
    pub(crate) fn exec_shell(
        &self,
        command: &str,
        cwd: Option<&Path>,
        timeout_secs: u64,
    ) -> Result<Command, String> {
        self.ensure_running()?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| "container session lock poisoned".to_string())?;
        Ok(build_exec_shell(
            &self.rt,
            &inner.name,
            inner.has_timeout,
            command,
            cwd,
            timeout_secs,
        ))
    }

    /// Remove the container (best-effort).
    pub(crate) fn shutdown(&self) {
        if let Ok(inner) = self.inner.lock() {
            info!("{DIM}📦 removing container '{}'{RESET}", inner.name);
            let _ = Command::new(self.rt.runtime.binary())
                .arg("rm")
                .arg("-f")
                .arg(&inner.name)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

impl Drop for ContainerSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn session_name() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("ai-{}-{:04x}", std::process::id(), nanos & 0xffff)
}

fn run_detached(rt: &ContainerRuntime, name: &str) -> Result<(), String> {
    let out = build_run_command(rt, name)
        .output()
        .map_err(|e| format!("failed to start container: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "container start failed ({}): {}",
            rt.image,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn is_running(rt: &ContainerRuntime, name: &str) -> bool {
    let out = Command::new(rt.runtime.binary())
        .arg("inspect")
        .arg("-f")
        .arg("{{.State.Running}}")
        .arg(name)
        .output();
    match out {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim() == "true",
        _ => false,
    }
}

fn probe_timeout(rt: &ContainerRuntime, name: &str) -> bool {
    Command::new(rt.runtime.binary())
        .arg("exec")
        .arg(name)
        .arg("sh")
        .arg("-c")
        .arg("command -v timeout")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyRule;

    fn allow(policy: &mut Policy, action: Action, pattern: &str) {
        policy.add_cli_rule(PolicyRule::Allow(action, pattern.to_string()));
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn test_mounts_read_ro_write_rw() {
        let dir = std::env::temp_dir().join(format!("ai-container-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut policy = Policy::default();
        allow(&mut policy, Action::Read, &format!("{}/**", dir.display()));
        allow(&mut policy, Action::Write, &dir.to_string_lossy());
        let (mounts, _) = mounts_from_policy(&policy);
        assert!(mounts.read.contains(&dir));
        assert!(mounts.write.contains(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_whole_fs_grant_not_mounted() {
        let mut policy = Policy::default();
        allow(&mut policy, Action::Read, "**");
        let (mounts, warnings) = mounts_from_policy(&policy);
        assert!(mounts.read.is_empty());
        assert!(warnings.iter().any(|w| w.contains("whole filesystem")));
    }

    #[test]
    fn test_yolo_style_policy_mounts_explicit_roots_only() {
        let dir = std::env::temp_dir().join(format!("ai-yolo-mount-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut policy = Policy::default();
        allow(&mut policy, Action::Read, "**");
        allow(&mut policy, Action::Write, "**");
        allow(&mut policy, Action::Write, &dir.to_string_lossy());
        let (mounts, _) = mounts_from_policy(&policy);
        assert_eq!(mounts.read, vec![dir.clone()]);
        assert_eq!(mounts.write, vec![dir.clone()]);
        assert!(!mounts.read.contains(&PathBuf::from("/")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_deny_dir_becomes_mask_file_warns() {
        let base = std::env::temp_dir().join(format!("ai-container-deny-{}", std::process::id()));
        let secret = base.join("proj").join("secret");
        let key = base.join("proj").join("key.pem");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(&key, "k").unwrap();

        let mut policy = Policy::default();
        allow(&mut policy, Action::Read, &format!("{}/**", base.display()));
        policy.add_cli_rule(PolicyRule::Deny(
            Action::Read,
            format!("{}/**", secret.display()),
        ));
        policy.add_cli_rule(PolicyRule::Deny(
            Action::Read,
            key.to_string_lossy().into_owned(),
        ));
        let (mounts, warnings) = mounts_from_policy(&policy);
        assert!(mounts.deny.contains(&secret));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("cannot mask denied file"))
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_detect_runtime_rejects_unknown() {
        assert!(detect_runtime(Some("nonesuch-runtime")).is_none());
        let _ = detect_runtime(None);
    }

    fn spec(network: Network) -> ContainerRuntime {
        let dir = std::env::temp_dir();
        ContainerRuntime {
            image: "alpine".to_string(),
            runtime: Runtime::Podman,
            network,
            mounts: Mounts {
                read: vec![dir.clone()],
                write: vec![dir.join("w")],
                deny: vec![],
            },
        }
    }

    #[test]
    fn test_run_command_shape() {
        let cmd = build_run_command(&spec(Network::None), "ai-test");
        let args = args_of(&cmd);
        assert_eq!(cmd.get_program().to_string_lossy(), "podman");
        assert!(args.windows(2).any(|w| w == ["--name", "ai-test"]));
        assert!(args.windows(2).any(|w| w == ["--network", "none"]));
        assert!(args.windows(2).any(|w| w == ["--entrypoint", "sh"]));
        assert!(args.iter().any(|a| a == "alpine"));
        assert!(args.iter().any(|a| a.ends_with(":ro")));
    }

    #[test]
    fn test_exec_shell_timeout_wrapping() {
        let rt = spec(Network::None);
        let cmd = build_exec_shell(&rt, "ai-test", true, "echo hi", None, 42);
        let args = args_of(&cmd);
        assert!(args.windows(2).any(|w| w == ["exec", "-i"]));
        assert!(args.iter().any(|a| a.contains("timeout 42 sh -c")));
        assert!(args.windows(2).any(|w| w == ["ai-test", "sh"]));
    }

    #[test]
    fn test_exec_shell_without_timeout_is_plain() {
        let rt = spec(Network::None);
        let cmd = build_exec_shell(&rt, "ai-test", false, "echo hi", None, 42);
        let args = args_of(&cmd);
        assert!(args.windows(2).any(|w| w == ["-c", "echo hi"]));
    }

    #[test]
    fn test_workdir_only_when_covered() {
        let rt = spec(Network::None);
        let inside = std::env::temp_dir();
        let args = args_of(&build_exec_program(
            &rt,
            "ai-test",
            "pwd",
            &[],
            Some(&inside),
        ));
        assert!(
            args.windows(2)
                .any(|w| w == ["-w", &inside.to_string_lossy()])
        );

        let outside = Path::new("/nonexistent-xyz");
        let args = args_of(&build_exec_program(
            &rt,
            "ai-test",
            "pwd",
            &[],
            Some(outside),
        ));
        assert!(!args.iter().any(|a| a == "-w"));
    }
}
