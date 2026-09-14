use bashkit::{DirEntry, FsBackend, FsLimits, FsUsage, Metadata, async_trait};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::policy::{Action, Policy};
use crate::sandbox::Sandbox;

pub(super) struct PolicyFsBackend<B: FsBackend> {
    inner: B,
    sandbox: Sandbox,
}

impl<B: FsBackend> PolicyFsBackend<B> {
    pub fn new(inner: B, policy: Policy) -> Self {
        Self {
            inner,
            sandbox: Sandbox::new(policy),
        }
    }

    fn check(&self, action: Action, path: &Path) -> bashkit::Result<()> {
        if self.sandbox.is_allowed(&action, path) {
            Ok(())
        } else {
            let resolved = self.sandbox.resolve(path);
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "{} access denied by policy: {}",
                    action,
                    resolved.to_string_lossy()
                ),
            )
            .into())
        }
    }

    fn check_pair(
        &self,
        ra: Action,
        rpath: &Path,
        wa: Action,
        wpath: &Path,
    ) -> bashkit::Result<()> {
        self.check(ra, rpath)?;
        self.check(wa, wpath)
    }
}

#[async_trait]
impl<B: FsBackend + Send + Sync> FsBackend for PolicyFsBackend<B> {
    async fn read(&self, path: &Path) -> bashkit::Result<Vec<u8>> {
        self.check(Action::Read, path)?;
        self.inner.read(path).await
    }

    async fn write(&self, path: &Path, content: &[u8]) -> bashkit::Result<()> {
        self.check(Action::Write, path)?;
        self.inner.write(path, content).await
    }

    async fn append(&self, path: &Path, content: &[u8]) -> bashkit::Result<()> {
        self.check(Action::Write, path)?;
        self.inner.append(path, content).await
    }

    async fn mkdir(&self, path: &Path, recursive: bool) -> bashkit::Result<()> {
        self.check(Action::Write, path)?;
        self.inner.mkdir(path, recursive).await
    }

    async fn remove(&self, path: &Path, recursive: bool) -> bashkit::Result<()> {
        self.check(Action::Write, path)?;
        self.inner.remove(path, recursive).await
    }

    /// Metadata is deliberately not policy-gated: bashkit resolves commands
    /// (`command`/`type`/`which`/`hash`) and searches `PATH` with `stat`, so
    /// gating it would prompt for a Read grant on every `PATH` entry. Contents
    /// and directory listings stay behind the Read policy.
    async fn stat(&self, path: &Path) -> bashkit::Result<Metadata> {
        self.inner.stat(path).await
    }

    async fn read_dir(&self, path: &Path) -> bashkit::Result<Vec<DirEntry>> {
        self.check(Action::Read, path)?;
        self.inner.read_dir(path).await
    }

    async fn exists(&self, path: &Path) -> bashkit::Result<bool> {
        self.inner.exists(path).await
    }

    async fn rename(&self, from: &Path, to: &Path) -> bashkit::Result<()> {
        self.check_pair(Action::Read, from, Action::Write, to)?;
        self.inner.rename(from, to).await
    }

    async fn copy(&self, from: &Path, to: &Path) -> bashkit::Result<()> {
        self.check_pair(Action::Read, from, Action::Write, to)?;
        self.inner.copy(from, to).await
    }

    async fn symlink(&self, target: &Path, link: &Path) -> bashkit::Result<()> {
        self.check(Action::Write, link)?;
        self.inner.symlink(target, link).await
    }

    async fn read_link(&self, path: &Path) -> bashkit::Result<PathBuf> {
        self.check(Action::Read, path)?;
        self.inner.read_link(path).await
    }

    async fn chmod(&self, path: &Path, mode: u32) -> bashkit::Result<()> {
        self.check(Action::Write, path)?;
        self.inner.chmod(path, mode).await
    }

    async fn set_modified_time(&self, path: &Path, time: SystemTime) -> bashkit::Result<()> {
        self.check(Action::Write, path)?;
        self.inner.set_modified_time(path, time).await
    }

    fn usage(&self) -> FsUsage {
        self.inner.usage()
    }

    fn limits(&self) -> FsLimits {
        self.inner.limits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bashkit::{RealFs, RealFsMode};

    #[tokio::test]
    async fn metadata_is_not_policy_gated_but_contents_are() {
        let inner = RealFs::open("/", RealFsMode::ReadOnly).await.unwrap();
        let backend = PolicyFsBackend::new(inner, Policy::default());
        assert!(backend.stat(Path::new("/")).await.is_ok());
        assert!(
            !backend
                .exists(Path::new("/definitely-not-here-xyz"))
                .await
                .unwrap()
        );
        let err = backend
            .read(Path::new("/definitely-not-here-xyz"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("access denied"));
    }
}
