use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeleteFileArgs {
    #[schemars(description = "Path to the file or directory to delete")]
    pub path: String,
    #[schemars(description = "If deleting a directory, remove recursively")]
    pub recursive: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct DeleteFileTool {
    policy: Policy,
}

impl DeleteFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for DeleteFileTool {
    const NAME: &'static str = "delete_file";

    type Args = DeleteFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Delete a file or directory at the given path".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(DeleteFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}✂️ delete file {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let canonical = path
            .canonicalize()
            .map_err(|e| ToolError::Message(format!("cannot resolve path: {e}")))?;
        let canonical_str = canonical.to_string_lossy();

        if !self.policy.is_allowed(&Action::Write, &canonical_str) {
            return Err(ToolError::Message(format!(
                "write access denied for: {}",
                args.path
            )));
        }

        let result = if canonical.is_dir() {
            if args.recursive.unwrap_or(false) {
                std::fs::remove_dir_all(&canonical)
                    .map_err(|e| ToolError::Message(format!("cannot delete directory: {e}")))?;
                format!("Deleted directory: {}", args.path)
            } else {
                return Err(ToolError::Message(format!(
                    "{} is a directory. Set recursive=true to delete it.",
                    args.path
                )));
            }
        } else {
            std::fs::remove_file(&canonical)
                .map_err(|e| ToolError::Message(format!("cannot delete file: {e}")))?;
            format!("Deleted file: {}", args.path)
        };
        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}
