use crate::util::fmt_bytes;
use ansi_color_constants::*;
use log::{debug, info};
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use crate::policy::Policy;
use crate::sandbox::Sandbox;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileInfoArgs {
    #[schemars(description = "Path to the file or directory")]
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct FileInfoTool {
    policy: Policy,
}

impl FileInfoTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl PortableTool for FileInfoTool {
    const NAME: &'static str = "file_info";

    type Args = FileInfoArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Get metadata about a file or directory: size, permissions, modification time, type."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(FileInfoArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}ℹ️  file info {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let sandbox = Sandbox::new(self.policy.clone());
        let meta = sandbox.metadata(&path)?;

        let size = meta.len();
        let size_str = fmt_bytes(size);

        let modified = meta
            .modified()
            .ok()
            .map(|t| {
                let dur = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                let secs = dur.as_secs();
                format!("{secs}")
            })
            .unwrap_or_else(|| "unknown".to_string());

        let permissions = if meta.permissions().readonly() {
            "r--"
        } else {
            "rw-"
        };

        let resolved = sandbox.resolve(&path);
        let entry_type = if resolved.is_dir() {
            "directory"
        } else if resolved.is_file() {
            "file"
        } else {
            "unknown"
        };

        let result = format!(
            "Type: {entry_type}\nSize: {size_str}\nPermission: {permissions}\nModified: {modified}"
        );

        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}
