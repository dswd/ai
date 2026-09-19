use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::shared::ToolError;
use crate::skills::SkillStore;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillUpdateArgs {
    #[schemars(description = "Name of the AI-created skill to update")]
    pub name: String,
    #[schemars(description = "Optional new one-line description; omitted keeps the current one")]
    #[serde(default)]
    pub description: Option<String>,
    #[schemars(description = "New markdown instructions and steps for the skill")]
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct SkillUpdateTool {
    store: Arc<SkillStore>,
}

impl SkillUpdateTool {
    pub fn new(store: Arc<SkillStore>) -> Self {
        Self { store }
    }
}

impl PortableTool for SkillUpdateTool {
    const NAME: &'static str = "skill_update";

    type Args = SkillUpdateArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Replace the body (and optionally the description) of a skill created by the agent. \
         Skills that were not created by the agent cannot be modified."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(SkillUpdateArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🧩 skill update '{}'{RESET}", args.name);
        match self
            .store
            .update(&args.name, args.description.as_deref(), &args.body)
        {
            Ok(path) => Ok(format!(
                "Updated skill '{}' at {}",
                args.name,
                path.display()
            )),
            Err(e) => {
                info!("{DIM}  \u{2192} error: {e}{RESET}");
                Err(ToolError::Message(e))
            }
        }
    }
}
