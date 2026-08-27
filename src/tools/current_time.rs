use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetCurrentTimeArgs {}

#[derive(Debug, Clone)]
pub struct GetCurrentTimeTool;

impl GetCurrentTimeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for GetCurrentTimeTool {
    const NAME: &'static str = "get_current_time";

    type Args = GetCurrentTimeArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Get the current date and time in UTC (ISO-8601).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(GetCurrentTimeArgs)).unwrap_or_default()
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("🕒 get current time");
        Ok(crate::util::now_iso())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now_iso_shape() {
        let out = crate::util::now_iso();
        let re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$").unwrap();
        assert!(re.is_match(&out), "unexpected shape: {out}");
    }

    #[tokio::test]
    async fn test_get_current_time_call() {
        let tool = GetCurrentTimeTool::new();
        let out = tool.call(GetCurrentTimeArgs {}).await.unwrap();
        assert!(out.contains('T'));
        assert!(out.ends_with('Z'));
    }
}
