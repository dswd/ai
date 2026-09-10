use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use crate::policy::Policy;
use crate::sandbox::Sandbox;

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
        let src = PathBuf::from(&args.source);
        let dst = PathBuf::from(&args.destination);
        let sandbox = Sandbox::new(self.policy.clone());
        sandbox.rename(&src, &dst)?;

        let result = format!("Moved {} to {}", args.source, args.destination);
        Ok(result)
    }
}
