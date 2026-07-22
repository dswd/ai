use ansi_color_constants::*;
use log::{info, debug};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ToolError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ThinkArgs {
    pub thought: String,
}

#[derive(Debug, Clone, Default)]
pub struct ThinkTool;

impl ThinkTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for ThinkTool {
    const NAME: &'static str = "think";

    type Args = ThinkArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Think (logs a thought without side effects)".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ThinkArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}{BLUE}🧠 {}{RESET}", args.thought);
        debug!("  → {} chars", args.thought.len());
        Ok(args.thought)
    }
}
