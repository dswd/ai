use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::shared::ToolError;
use crate::skills::SkillStore;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillDeleteArgs {
    #[schemars(description = "Name of the AI-created skill to delete")]
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct SkillDeleteTool {
    store: Arc<SkillStore>,
}

impl SkillDeleteTool {
    pub fn new(store: Arc<SkillStore>) -> Self {
        Self { store }
    }
}

impl PortableTool for SkillDeleteTool {
    const NAME: &'static str = "skill_delete";

    type Args = SkillDeleteArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Delete a skill created by the agent when its procedure is obsolete. Skills that were \
         not created by the agent cannot be deleted."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(SkillDeleteArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🧩 skill delete '{}'{RESET}", args.name);
        match self.store.delete(&args.name) {
            Ok(path) => Ok(format!(
                "Deleted skill '{}' ({})",
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
