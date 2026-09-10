//! The single checked filesystem layer.
//!
//! Every tool that touches a user-supplied path goes through [`Sandbox`], so
//! authorization and the operation happen on the same resolved path and there
//! is exactly one place that can be audited. Paths are canonicalized before the
//! policy is consulted, so a symlink inside a granted directory cannot redirect
//! a read or write outside the granted roots.
//!
//! Limitation: this resolves-then-operates, so a symlink swapped between the
//! check and the operation (TOCTOU) is not fully closed without `openat` +
//! `O_NOFOLLOW`. The common cases (static symlinks, per-entry traversal checks)
//! are handled; a kernel-enforced sandbox would be required to remove the race.

use crate::policy::{Action, Policy};
use crate::tools::shared::ToolError;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct Sandbox {
    policy: Policy,
}

impl Sandbox {
    pub(crate) fn new(policy: Policy) -> Self {
        Self { policy }
    }

    /// Resolve `path` to its real location before consulting policy.
    ///
    /// For paths that do not exist yet (writes, creates), canonicalize the
    /// deepest existing ancestor and re-append the missing components, so a
    /// symlinked parent directory cannot redirect file creation either.
    pub(crate) fn resolve(&self, path: &Path) -> PathBuf {
        if let Ok(canon) = std::fs::canonicalize(path) {
            return canon;
        }
        let mut ancestor = path;
        let mut missing: Vec<std::ffi::OsString> = Vec::new();
        while !ancestor.exists() {
            match (ancestor.parent(), ancestor.file_name()) {
                (Some(parent), Some(name)) => {
                    missing.push(name.to_os_string());
                    ancestor = parent;
                }
                _ => break,
            }
        }
        let mut resolved =
            std::fs::canonicalize(ancestor).unwrap_or_else(|_| ancestor.to_path_buf());
        for name in missing.iter().rev() {
            resolved.push(name);
        }
        resolved
    }

    pub(crate) fn is_allowed(&self, action: &Action, path: &Path) -> bool {
        let resolved = self.resolve(path);
        self.policy.is_allowed(action, &resolved.to_string_lossy())
    }

    /// Authorize an operation, returning the resolved path on success.
    pub(crate) fn authorize(&self, action: Action, path: &Path) -> Result<PathBuf, ToolError> {
        let resolved = self.resolve(path);
        if self.policy.is_allowed(&action, &resolved.to_string_lossy()) {
            Ok(resolved)
        } else {
            Err(ToolError::Message(format!(
                "{action} access denied for: {}",
                path.display()
            )))
        }
    }

    pub(crate) fn read_to_string(&self, path: &Path) -> Result<String, ToolError> {
        let resolved = self.authorize(Action::Read, path)?;
        std::fs::read_to_string(&resolved)
            .map_err(|e| ToolError::Message(format!("cannot read {}: {e}", path.display())))
    }

    pub(crate) fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        let resolved = self.authorize(Action::Read, path)?;
        std::fs::read(&resolved)
            .map_err(|e| ToolError::Message(format!("cannot read {}: {e}", path.display())))
    }

    pub(crate) fn write(&self, path: &Path, content: &[u8]) -> Result<(), ToolError> {
        let resolved = self.authorize(Action::Write, path)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Message(format!("cannot create parent dirs: {e}")))?;
        }
        std::fs::write(&resolved, content)
            .map_err(|e| ToolError::Message(format!("cannot write {}: {e}", path.display())))
    }

    pub(crate) fn create_dir_all(&self, path: &Path) -> Result<(), ToolError> {
        let resolved = self.authorize(Action::Write, path)?;
        std::fs::create_dir_all(&resolved)
            .map_err(|e| ToolError::Message(format!("cannot create directory: {e}")))
    }

    pub(crate) fn remove_file(&self, path: &Path) -> Result<(), ToolError> {
        let resolved = self.authorize(Action::Write, path)?;
        std::fs::remove_file(&resolved)
            .map_err(|e| ToolError::Message(format!("cannot delete {}: {e}", path.display())))
    }

    pub(crate) fn remove_dir_all(&self, path: &Path) -> Result<(), ToolError> {
        let resolved = self.authorize(Action::Write, path)?;
        std::fs::remove_dir_all(&resolved)
            .map_err(|e| ToolError::Message(format!("cannot delete {}: {e}", path.display())))
    }

    pub(crate) fn rename(&self, from: &Path, to: &Path) -> Result<(), ToolError> {
        let from = self.authorize(Action::Read, from)?;
        let to = self.authorize(Action::Write, to)?;
        std::fs::rename(&from, &to)
            .map_err(|e| ToolError::Message(format!("cannot move file: {e}")))
    }

    pub(crate) fn copy(&self, from: &Path, to: &Path) -> Result<(), ToolError> {
        let from = self.authorize(Action::Read, from)?;
        let to = self.authorize(Action::Write, to)?;
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Message(format!("cannot create parent dirs: {e}")))?;
        }
        std::fs::copy(&from, &to)
            .map_err(|e| ToolError::Message(format!("cannot copy file: {e}")))?;
        Ok(())
    }

    pub(crate) fn metadata(&self, path: &Path) -> Result<std::fs::Metadata, ToolError> {
        let resolved = self.authorize(Action::Read, path)?;
        std::fs::symlink_metadata(&resolved)
            .map_err(|e| ToolError::Message(format!("cannot read metadata: {e}")))
    }

    pub(crate) fn read_dir(
        &self,
        path: &Path,
    ) -> Result<Vec<(PathBuf, std::fs::FileType)>, ToolError> {
        let resolved = self.authorize(Action::Read, path)?;
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&resolved)
            .map_err(|e| ToolError::Message(format!("cannot read dir: {e}")))?
            .flatten()
        {
            if let Ok(ft) = entry.file_type() {
                out.push((entry.path(), ft));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyRule;

    fn allow_read(policy: &Policy, path: &str) -> Policy {
        let mut p = policy.clone();
        p.add_cli_rule(PolicyRule::Allow(Action::Read, path.to_string()));
        p
    }

    #[test]
    fn test_symlink_cannot_escape_read_policy() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let base = std::env::temp_dir().join(format!("ai-sandbox-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let allowed = base.join("allowed");
            let denied = base.join("denied");
            std::fs::create_dir_all(&allowed).unwrap();
            std::fs::create_dir_all(&denied).unwrap();
            std::fs::write(denied.join("secret.txt"), "TOP-SECRET").unwrap();
            symlink(denied.join("secret.txt"), allowed.join("link.txt")).unwrap();

            let policy = allow_read(&Policy::default(), &format!("{}/**", allowed.display()));
            let sandbox = Sandbox::new(policy);
            let err = sandbox
                .read_to_string(&allowed.join("link.txt"))
                .unwrap_err();
            assert!(format!("{err}").contains("denied"));
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    #[test]
    fn test_write_through_symlinked_parent_denied() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let base = std::env::temp_dir().join(format!("ai-sandbox-w-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let allowed = base.join("allowed");
            let denied = base.join("denied");
            std::fs::create_dir_all(&allowed).unwrap();
            std::fs::create_dir_all(&denied).unwrap();
            symlink(&denied, allowed.join("out")).unwrap();

            let mut policy = Policy::default();
            policy.add_cli_rule(PolicyRule::Allow(
                Action::Read,
                format!("{}/**", allowed.display()),
            ));
            policy.add_cli_rule(PolicyRule::Allow(
                Action::Write,
                format!("{}/**", allowed.display()),
            ));
            let sandbox = Sandbox::new(policy);
            let err = sandbox
                .write(&allowed.join("out/created.txt"), b"x")
                .unwrap_err();
            assert!(format!("{err}").contains("denied"));
            assert!(!denied.join("created.txt").exists());
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}
