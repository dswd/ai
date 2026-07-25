use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CopyFileArgs {
    #[schemars(description = "Source path")]
    pub source: String,
    #[schemars(description = "Destination path")]
    pub destination: String,
}

#[derive(Debug, Clone)]
pub struct CopyFileTool {
    policy: Policy,
}

impl CopyFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for CopyFileTool {
    const NAME: &'static str = "copy_file";

    type Args = CopyFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Copy a file to a new location.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(CopyFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}🗐 copy file {} -> {}{RESET}",
            args.source, args.destination
        );
        let src_path = PathBuf::from(&args.source);
        let src = src_path
            .canonicalize()
            .map_err(|e| ToolError::Message(format!("cannot resolve path: {e}")))?;
        let src_str = src.to_string_lossy().to_string();
        if !self.policy.is_allowed(&Action::Read, &src_str) {
            return Err(ToolError::Message(format!(
                "read access denied for: {src_str}"
            )));
        }

        if !src.is_file() {
            return Err(ToolError::Message(format!(
                "cannot copy: {src_str} is not a file"
            )));
        }

        let dst_parent = std::path::Path::new(&args.destination)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        if !dst_parent.exists() {
            std::fs::create_dir_all(&dst_parent)
                .map_err(|e| ToolError::Message(format!("cannot create parent dirs: {e}")))?;
        }

        let dst_canonical = dst_parent.canonicalize().unwrap_or(dst_parent).join(
            std::path::Path::new(&args.destination)
                .file_name()
                .unwrap_or_default(),
        );

        let dst_str = dst_canonical.to_string_lossy().to_string();

        if !self.policy.is_allowed(&Action::Write, &dst_str) {
            return Err(ToolError::Message(format!(
                "write access denied for: {dst_str}"
            )));
        }

        std::fs::copy(&src, &dst_canonical)
            .map_err(|e| ToolError::Message(format!("cannot copy file: {e}")))?;

        let result = format!("Copied {} to {}", args.source, args.destination);
        Ok(result)
    }
}
