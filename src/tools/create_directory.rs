use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use crate::policy::Policy;
use crate::sandbox::Sandbox;

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
        let sandbox = Sandbox::new(self.policy.clone());
        sandbox.create_dir_all(&path)?;

        let result = format!("Created directory: {}", args.path);
        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}
