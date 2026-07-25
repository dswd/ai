use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateDirectoryArgs {
    #[schemars(description = "Directory path to create")]
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct CreateDirectoryTool {
    policy: Policy,
}

impl CreateDirectoryTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for CreateDirectoryTool {
    const NAME: &'static str = "create_directory";

    type Args = CreateDirectoryArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Create a directory and any missing parent directories (like mkdir -p)".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(CreateDirectoryArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📁 create dir {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);

        let canonical = if path.exists() {
            path.canonicalize()
                .map_err(|e| ToolError::Message(format!("cannot resolve path: {e}")))?
        } else {
            if let Some(parent) = path.parent() {
                if parent.exists() {
                    parent
                        .canonicalize()
                        .map_err(|e| ToolError::Message(format!("cannot resolve parent: {e}")))?
                        .join(path.file_name().unwrap_or_default())
                } else {
                    path.clone()
                }
            } else {
                path.clone()
            }
        };
        let canonical_str = canonical.to_string_lossy();

        if !self.policy.is_allowed(&Action::Write, &canonical_str) {
            return Err(ToolError::Message(format!(
                "write access denied for: {}",
                args.path
            )));
        }

        std::fs::create_dir_all(&canonical)
            .map_err(|e| ToolError::Message(format!("cannot create directory: {e}")))?;

        let result = format!("Created directory: {}", args.path);
        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}
