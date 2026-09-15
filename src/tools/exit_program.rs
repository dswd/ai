use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExitProgramArgs {}

/// Signals the interactive loop to end the program when the user asks to quit.
#[derive(Debug, Clone)]
pub struct ExitProgramTool {
    flag: Arc<AtomicBool>,
}

impl ExitProgramTool {
    pub fn new(flag: Arc<AtomicBool>) -> Self {
        Self { flag }
    }
}

impl PortableTool for ExitProgramTool {
    const NAME: &'static str = "exit_program";

    type Args = ExitProgramArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Exit the ai program. Call this only when the user asks to quit or exit.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ExitProgramArgs)).unwrap_or_default()
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("🚪 exit program");
        self.flag.store(true, Ordering::SeqCst);
        Ok("Exiting the program now.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_call_sets_exit_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let tool = ExitProgramTool::new(Arc::clone(&flag));
        assert!(!flag.load(Ordering::SeqCst));
        let out = tool.call(ExitProgramArgs {}).await.unwrap();
        assert!(flag.load(Ordering::SeqCst));
        assert!(out.to_lowercase().contains("exit"));
    }
}
