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
pub struct WriteFileArgs {
    #[schemars(description = "Path to the file to write")]
    pub path: String,
    #[schemars(description = "Content to write to the file")]
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct WriteFileTool {
    policy: Policy,
}

impl WriteFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl PortableTool for WriteFileTool {
    const NAME: &'static str = "write_file";

    type Args = WriteFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Write content to a file at the given path. Creates or overwrites the file.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(WriteFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}✏️  write file {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let sandbox = Sandbox::new(self.policy.clone());
        sandbox.write(&path, args.content.as_bytes())?;

        let result = format!("Successfully wrote to {}", args.path);
        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}
