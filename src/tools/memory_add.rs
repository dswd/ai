use std::sync::Arc;

use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;
use crate::memory::Memory;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAddArgs {
    #[schemars(description = "The data to store in memory")]
    pub data: String,
    #[schemars(
        description = "Optional keywords to improve retrieval (e.g. topics, entities, names)"
    )]
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryAddTool {
    memory: Arc<Memory>,
}

impl MemoryAddTool {
    pub fn new(memory: Arc<Memory>) -> Self {
        Self { memory }
    }
}

impl Tool for MemoryAddTool {
    const NAME: &'static str = "memory_add";

    type Args = MemoryAddArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Store a piece of data in persistent memory. Optionally provide keywords to improve retrieval later. Returns a unique key that can be used to reference or delete the entry later.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(MemoryAddArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🧠 memory add '{}'{RESET}", args.data);
        match self.memory.add(args.data, args.keywords) {
            Ok(key) => {
                info!("{DIM}  \u{2192} stored as {key}{RESET}");
                Ok(format!("Stored as {key}"))
            }
            Err(e) => {
                info!("{DIM}  \u{2192} error: {e}{RESET}");
                Err(ToolError::Message(e))
            }
        }
    }
}
