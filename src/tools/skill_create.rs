use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::shared::ToolError;
use crate::skills::SkillStore;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillCreateArgs {
    #[schemars(
        description = "Skill name: lowercase letters, digits and hyphens (max 64 characters)"
    )]
    pub name: String,
    #[schemars(description = "One-line description of when to use the skill (max 200 characters)")]
    pub description: String,
    #[schemars(description = "Markdown instructions and steps for the skill")]
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct SkillCreateTool {
    store: Arc<SkillStore>,
}

impl SkillCreateTool {
    pub fn new(store: Arc<SkillStore>) -> Self {
        Self { store }
    }
}

impl PortableTool for SkillCreateTool {
    const NAME: &'static str = "skill_create";

    type Args = SkillCreateArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Create a new skill from a repeatable procedure. Fails if a skill with that name \
         already exists; use skill_update for AI-created skills."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(SkillCreateArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🧩 skill create '{}'{RESET}", args.name);
        match self.store.create(&args.name, &args.description, &args.body) {
            Ok(path) => Ok(format!(
                "Created skill '{}' at {}",
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
