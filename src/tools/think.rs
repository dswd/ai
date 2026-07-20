use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ThinkArgs {
    #[schemars(description = "A thought to think about.")]
    pub thought: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ThinkError {
    #[error("{0}")]
    Message(String),
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
    type Error = ThinkError;

    fn description(&self) -> String {
        "Use the tool to think about something. It will not obtain new information or make changes, \
         but just append the thought to the log. Use it when complex reasoning or some cache memory \
         is needed.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ThinkArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(args.thought)
    }
}
