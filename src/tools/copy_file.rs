use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::shared::ToolError;
use crate::policy::{Action, Policy};
use crate::sandbox::Sandbox;

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

impl PortableTool for CopyFileTool {
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
        let src = PathBuf::from(&args.source);
        let dst = PathBuf::from(&args.destination);
        let sandbox = Sandbox::new(self.policy.clone());

        let src_resolved = sandbox.authorize(Action::Read, &src)?;
        if !src_resolved.is_file() {
            return Err(ToolError::Message(format!(
                "cannot copy: {} is not a file",
                args.source
            )));
        }

        sandbox.copy(&src, &dst)?;

        let result = format!("Copied {} to {}", args.source, args.destination);
        Ok(result)
    }
}
