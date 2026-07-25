use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MoveFileArgs {
    #[schemars(description = "Source path")]
    pub source: String,
    #[schemars(description = "Destination path")]
    pub destination: String,
}

#[derive(Debug, Clone)]
pub struct MoveFileTool {
    policy: Policy,
}

impl MoveFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for MoveFileTool {
    const NAME: &'static str = "move_file";

    type Args = MoveFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Move or rename a file or directory.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(MoveFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}➡️ move file {} -> {}{RESET}",
            args.source, args.destination
        );
        let src_path = PathBuf::from(&args.source);
        let src = src_path
            .canonicalize()
            .map_err(|e| ToolError::Message(format!("cannot resolve path: {e}")))?;
        let src_str = src.to_string_lossy().to_string();
        if !self.policy.is_allowed(&Action::Write, &src_str) {
            return Err(ToolError::Message(format!(
                "write access denied for: {src_str}"
            )));
        }

        let dst_parent = std::path::Path::new(&args.destination)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        let dst_canonical = if dst_parent.exists() {
            dst_parent.canonicalize().unwrap_or(dst_parent).join(
                std::path::Path::new(&args.destination)
                    .file_name()
                    .unwrap_or_default(),
            )
        } else {
            std::path::PathBuf::from(&args.destination)
        };

        let dst_str = dst_canonical.to_string_lossy().to_string();

        if !self.policy.is_allowed(&Action::Write, &dst_str) {
            return Err(ToolError::Message(format!(
                "write access denied for: {dst_str}"
            )));
        }

        std::fs::rename(&src, &dst_canonical)
            .map_err(|e| ToolError::Message(format!("cannot move file: {e}")))?;

        let result = format!("Moved {} to {}", args.source, args.destination);
        Ok(result)
    }
}
