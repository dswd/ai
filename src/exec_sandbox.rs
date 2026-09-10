//! OS-enforced sandbox for external commands.
//!
//! External commands spawned by the `execute` tool are launched through a
//! self-launcher (`ai --sandbox-exec -- <program> ...`) which applies the
//! restrictions derived from the active policy and then `execvp`s the target.
//! On Linux the sandbox is Landlock (filesystem allow-list + TCP bind/connect)
//! plus resource limits. On other platforms it is a documented no-op and the
//! caller disables it unless explicitly required.

use crate::config::SandboxConfig;
use crate::policy::{Action, Policy};
use std::path::PathBuf;

pub(crate) const LAUNCHER_FLAG: &str = "--sandbox-exec";
const ENV_READ: &str = "AI_SANDBOX_READ";
const ENV_WRITE: &str = "AI_SANDBOX_WRITE";
const ENV_NET: &str = "AI_SANDBOX_NET";
const ENV_MEM: &str = "AI_SANDBOX_MEM";
const ENV_CPU: &str = "AI_SANDBOX_CPU";
const ENV_NPROC: &str = "AI_SANDBOX_NPROC";
const ENV_FSIZE: &str = "AI_SANDBOX_FSIZE";

/// Base paths a sandboxed command needs to run at all (dynamic loader,
/// libraries, NSS/TLS config, devices). Overridable via config.
const SYSTEM_READ: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/etc/ld.so.cache",
    "/etc/ld.so.conf",
    "/etc/ld.so.conf.d",
    "/etc/nsswitch.conf",
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/localtime",
    "/etc/ssl",
    "/etc/ca-certificates",
    "/etc/pki",
    "/dev/null",
    "/dev/zero",
    "/dev/urandom",
    "/dev/random",
    "/dev/tty",
    "/proc",
    "/sys",
];

/// Device nodes commands routinely open read-write.
const SYSTEM_WRITE: &[&str] = &["/dev/null", "/dev/zero", "/dev/full", "/dev/tty"];

#[derive(Debug, Clone, Default)]
pub(crate) struct SandboxSpec {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub allow_network: bool,
    pub memory_bytes: Option<u64>,
    pub cpu_secs: Option<u64>,
    pub nproc: Option<u64>,
    pub file_size_bytes: Option<u64>,
}

impl SandboxSpec {
    pub(crate) fn env_vars(&self) -> Vec<(String, String)> {
        fn join(paths: &[PathBuf]) -> String {
            paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\n")
        }
        let mut vars = vec![
            (ENV_READ.to_string(), join(&self.read_roots)),
            (ENV_WRITE.to_string(), join(&self.write_roots)),
            (
                ENV_NET.to_string(),
                if self.allow_network { "1" } else { "0" }.to_string(),
            ),
        ];
        if let Some(v) = self.memory_bytes {
            vars.push((ENV_MEM.to_string(), v.to_string()));
        }
        if let Some(v) = self.cpu_secs {
            vars.push((ENV_CPU.to_string(), v.to_string()));
        }
        if let Some(v) = self.nproc {
            vars.push((ENV_NPROC.to_string(), v.to_string()));
        }
        if let Some(v) = self.file_size_bytes {
            vars.push((ENV_FSIZE.to_string(), v.to_string()));
        }
        vars
    }

    fn from_env() -> Self {
        fn paths(key: &str) -> Vec<PathBuf> {
            std::env::var(key)
                .ok()
                .map(|v| {
                    v.split('\n')
                        .filter(|s| !s.is_empty())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default()
        }
        fn num(key: &str) -> Option<u64> {
            std::env::var(key).ok().and_then(|v| v.parse().ok())
        }
        Self {
            read_roots: paths(ENV_READ),
            write_roots: paths(ENV_WRITE),
            allow_network: std::env::var(ENV_NET).ok().as_deref() == Some("1"),
            memory_bytes: num(ENV_MEM),
            cpu_secs: num(ENV_CPU),
            nproc: num(ENV_NPROC),
            file_size_bytes: num(ENV_FSIZE),
        }
    }
}

/// Translate the policy into a sandbox spec. Landlock is allow-list only, so
/// deny rules and globs cannot be represented exactly; returns human-readable
/// warnings for the caller to surface.
pub(crate) fn spec_from_policy(policy: &Policy, cfg: &SandboxConfig) -> (SandboxSpec, Vec<String>) {
    let mut warnings = Vec::new();
    let mut read = Vec::new();
    let mut write: Vec<PathBuf> = SYSTEM_WRITE
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect();

    for pattern in policy.allow_patterns(&Action::Read) {
        add_root(&pattern, &mut read, "read", &mut warnings);
    }
    for pattern in policy.allow_patterns(&Action::Write) {
        add_root(&pattern, &mut read, "read", &mut warnings);
        add_root(&pattern, &mut write, "write", &mut warnings);
    }

    for pattern in policy
        .deny_patterns(&Action::Read)
        .into_iter()
        .chain(policy.deny_patterns(&Action::Write))
    {
        warnings.push(format!(
            "exec sandbox cannot enforce deny rule '{pattern}' (Landlock is allow-list only)"
        ));
    }

    // Base system access so ordinary binaries can run.
    let mut read_roots: Vec<PathBuf> = match &cfg.system_read {
        Some(list) => list.iter().map(PathBuf::from).collect(),
        None => SYSTEM_READ.iter().map(PathBuf::from).collect(),
    };
    read_roots.retain(|p| p.exists());
    read_roots.extend(read);
    if let Ok(cwd) = std::env::current_dir() {
        read_roots.push(cwd);
    }
    if let Some(list) = &cfg.system_write {
        write.extend(list.iter().map(PathBuf::from).filter(|p| p.exists()));
    }
    dedup(&mut read_roots);
    dedup(&mut write);

    let allow_network =
        policy.has_any_allow(&Action::WebFetch) || policy.has_any_allow(&Action::WebSearch);

    let spec = SandboxSpec {
        read_roots,
        write_roots: write,
        allow_network,
        memory_bytes: cfg.memory_mb.map(|mb| mb * 1024 * 1024),
        cpu_secs: cfg.cpu_secs,
        nproc: cfg.nproc,
        file_size_bytes: cfg.file_size_mb.map(|mb| mb * 1024 * 1024),
    };
    (spec, warnings)
}

fn add_root(pattern: &str, out: &mut Vec<PathBuf>, kind: &str, warnings: &mut Vec<String>) {
    if pattern == "*" || pattern == "**" || pattern == "/" {
        warnings.push(format!(
            "exec sandbox: a {kind} grant of '{pattern}' grants the whole filesystem"
        ));
        out.push(PathBuf::from("/"));
        return;
    }
    let prefix = match pattern.find('*') {
        Some(idx) => pattern[..idx].trim_end_matches('/').to_string(),
        None => pattern.to_string(),
    };
    if prefix.is_empty() {
        return;
    }
    let path = PathBuf::from(&prefix);
    if path.exists() {
        out.push(path);
    } else {
        warnings.push(format!(
            "exec sandbox: granted {kind} path '{prefix}' does not exist and was skipped"
        ));
    }
}

fn dedup(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}

/// Whether the platform can enforce the exec sandbox.
pub(crate) fn available() -> bool {
    #[cfg(target_os = "linux")]
    {
        use landlock::{ABI, Access, AccessFs, Ruleset, RulesetAttr};
        // Creating a ruleset does not restrict anything; it only proves the
        // kernel implements the Landlock syscalls.
        Ruleset::default()
            .handle_access(AccessFs::from_all(ABI::V1))
            .and_then(|r| r.create())
            .is_ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// If this process was started as the sandbox launcher, return the program and
/// its arguments (everything after `--`).
pub(crate) fn launcher_request() -> Option<(String, Vec<String>)> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(LAUNCHER_FLAG) {
        return None;
    }
    let sep = args.iter().position(|a| a == "--")?;
    let program = args.get(sep + 1)?.clone();
    let rest = args[sep + 2..].to_vec();
    Some((program, rest))
}

/// Path to this binary, used as the sandbox launcher.
pub(crate) fn launcher_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// Build a blocking command for `program`, wrapped in the sandbox launcher when
/// a spec is present.
pub(crate) fn command(program: &str, spec: Option<&SandboxSpec>) -> std::process::Command {
    match spec {
        Some(spec) => match launcher_exe() {
            Some(exe) => {
                let mut cmd = std::process::Command::new(exe);
                cmd.arg(LAUNCHER_FLAG).arg("--").arg(program);
                for (key, value) in spec.env_vars() {
                    cmd.env(key, value);
                }
                cmd
            }
            None => std::process::Command::new(program),
        },
        None => std::process::Command::new(program),
    }
}

/// Apply the sandbox described by the environment, then replace this process
/// with `program`.
pub(crate) fn run_launcher(program: String, args: Vec<String>) -> ! {
    let mut spec = SandboxSpec::from_env();

    let tmp = make_temp_dir();
    spec.write_roots.push(tmp.clone());
    dedup(&mut spec.write_roots);

    #[cfg(target_os = "linux")]
    if let Err(e) = apply_landlock(&spec) {
        eprintln!("ai: exec sandbox failed: {e}");
        std::process::exit(126);
    }

    apply_rlimits(&spec);

    let mut command = std::process::Command::new(&program);
    command
        .args(&args)
        .env("TMPDIR", &tmp)
        .env("TMP", &tmp)
        .env("TEMP", &tmp)
        .env_remove(ENV_READ)
        .env_remove(ENV_WRITE)
        .env_remove(ENV_NET)
        .env_remove(ENV_MEM)
        .env_remove(ENV_CPU)
        .env_remove(ENV_NPROC)
        .env_remove(ENV_FSIZE);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = command.exec();
        eprintln!("ai: sandbox launcher failed to execute {program}: {err}");
        std::process::exit(127);
    }

    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(err) => {
                eprintln!("ai: sandbox launcher failed to execute {program}: {err}");
                std::process::exit(127);
            }
        }
    }
}

fn make_temp_dir() -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // ponytail: private temp dir is left behind for the OS to reap; clean up if
    // sandbox runs become frequent.
    let dir = std::env::temp_dir().join(format!("ai-sandbox-{}-{nanos}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir
}

#[cfg(target_os = "linux")]
fn apply_landlock(spec: &SandboxSpec) -> Result<(), String> {
    use landlock::{
        ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus, path_beneath_rules,
    };

    let abi = ABI::V9;
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| format!("landlock handle_access: {e}"))?;
    if !spec.allow_network {
        ruleset = ruleset
            .handle_access(AccessNet::BindTcp)
            .map_err(|e| format!("landlock net bind: {e}"))?
            .handle_access(AccessNet::ConnectTcp)
            .map_err(|e| format!("landlock net connect: {e}"))?;
    }

    let status = ruleset
        .create()
        .map_err(|e| format!("landlock create: {e}"))?
        .add_rules(path_beneath_rules(
            &spec.read_roots,
            AccessFs::from_read(abi),
        ))
        .map_err(|e| format!("landlock read rules: {e}"))?
        .add_rules(path_beneath_rules(
            &spec.write_roots,
            AccessFs::from_all(abi),
        ))
        .map_err(|e| format!("landlock write rules: {e}"))?
        .set_compatibility(CompatLevel::BestEffort)
        .restrict_self()
        .map_err(|e| format!("landlock restrict_self: {e}"))?;

    if status.ruleset == RulesetStatus::NotEnforced {
        return Err("Landlock ruleset not enforced by this kernel".to_string());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_rlimits(spec: &SandboxSpec) {
    use rlimit::{Resource, setrlimit};
    let _ = setrlimit(Resource::CORE, 0, 0);
    if let Some(b) = spec.memory_bytes {
        let _ = setrlimit(Resource::AS, b, b);
    }
    if let Some(s) = spec.cpu_secs {
        let _ = setrlimit(Resource::CPU, s, s);
    }
    if let Some(n) = spec.nproc {
        let _ = setrlimit(Resource::NPROC, n, n);
    }
    if let Some(f) = spec.file_size_bytes {
        let _ = setrlimit(Resource::FSIZE, f, f);
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_rlimits(_spec: &SandboxSpec) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{Policy, PolicyRule};

    fn allow(policy: &mut Policy, action: Action, pattern: &str) {
        policy.add_cli_rule(PolicyRule::Allow(action, pattern.to_string()));
    }

    #[test]
    fn test_spec_globs_reduce_to_roots() {
        let dir = std::env::temp_dir().join(format!("ai-exec-sandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut policy = Policy::default();
        allow(
            &mut policy,
            Action::Read,
            &format!("{}/**/*.rs", dir.display()),
        );
        let (spec, _) = spec_from_policy(&policy, &SandboxConfig::default());
        assert!(spec.read_roots.contains(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_spec_wildcard_grants_root_and_warns() {
        let mut policy = Policy::default();
        allow(&mut policy, Action::Write, "**");
        let (spec, warnings) = spec_from_policy(&policy, &SandboxConfig::default());
        assert!(spec.write_roots.contains(&PathBuf::from("/")));
        assert!(warnings.iter().any(|w| w.contains("whole filesystem")));
    }

    #[test]
    fn test_spec_deny_warns() {
        let mut policy = Policy::default();
        allow(&mut policy, Action::Read, "/tmp");
        policy.add_cli_rule(PolicyRule::Deny(Action::Read, "/tmp/secret".to_string()));
        let (_, warnings) = spec_from_policy(&policy, &SandboxConfig::default());
        assert!(warnings.iter().any(|w| w.contains("cannot enforce deny")));
    }

    #[test]
    fn test_env_roundtrip() {
        let spec = SandboxSpec {
            read_roots: vec![PathBuf::from("/usr"), PathBuf::from("/etc")],
            write_roots: vec![PathBuf::from("/tmp/x")],
            allow_network: true,
            memory_bytes: Some(1024),
            ..Default::default()
        };
        let vars = spec.env_vars();
        for (k, v) in vars {
            // SAFETY: single-threaded test with uniquely-named keys.
            unsafe { std::env::set_var(k, v) };
        }
        let back = SandboxSpec::from_env();
        assert_eq!(back.read_roots, spec.read_roots);
        assert_eq!(back.write_roots, spec.write_roots);
        assert!(back.allow_network);
        assert_eq!(back.memory_bytes, Some(1024));
        for k in [ENV_READ, ENV_WRITE, ENV_NET, ENV_MEM] {
            unsafe { std::env::remove_var(k) };
        }
    }
}
