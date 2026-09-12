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
pub struct ReplaceInFileArgs {
    #[schemars(description = "Path to the file to modify")]
    pub path: String,
    #[schemars(description = "Exact string to replace")]
    pub old_str: String,
    #[schemars(description = "Replacement string")]
    pub new_str: String,
}

#[derive(Debug, Clone)]
pub struct ReplaceInFileTool {
    policy: Policy,
}

impl ReplaceInFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl PortableTool for ReplaceInFileTool {
    const NAME: &'static str = "replace_in_file";

    type Args = ReplaceInFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Replace a specific string in a file with a new string. \
         The old_str must match exactly once in the file. \
         Use this for surgical edits instead of overwriting the entire file."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ReplaceInFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📝 edit file {}{RESET}", args.path);
        let path = PathBuf::from(&args.path);
        let sandbox = Sandbox::new(self.policy.clone());
        let content = sandbox.read_to_string(&path)?;

        if args.old_str.is_empty() {
            return Err(ToolError::Message("old_str must not be empty".to_string()));
        }

        let count = content.matches(&args.old_str).count();
        if count == 0 {
            return Err(ToolError::Message(format!(
                "old_str not found in file: {}",
                args.path
            )));
        }
        if count > 1 {
            return Err(ToolError::Message(format!(
                "old_str found {} times in file (must be unique): {}",
                count, args.path
            )));
        }

        let new_content = content.replacen(&args.old_str, &args.new_str, 1);

        sandbox.write(&path, new_content.as_bytes())?;

        let result = format!("Successfully replaced in {} (1 occurrence)", args.path);
        debug!("{DIM}  \u{2192} {}{RESET}", result);
        Ok(result)
    }
}
