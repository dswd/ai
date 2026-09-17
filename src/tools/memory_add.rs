use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;
use crate::memory::Memory;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAddArgs {
    #[schemars(description = "The data to store in memory")]
    pub data: String,
    #[schemars(description = "Optional tags to improve retrieval (e.g. topics, entities, names)")]
    #[serde(default)]
    pub tags: Vec<String>,
    #[schemars(description = "Who stated the fact: \"user\" or \"agent\" (default: agent)")]
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryAddTool {
    memory: Memory,
}

impl MemoryAddTool {
    pub fn new(memory: Memory) -> Self {
        Self { memory }
    }
}

impl PortableTool for MemoryAddTool {
    const NAME: &'static str = "memory_add";

    type Args = MemoryAddArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Store a piece of data in persistent memory. Optionally provide tags to improve retrieval \
         and an origin (\"user\" or \"agent\") recording who stated the fact. Returns a unique key \
         that can be used to reference or delete the entry later."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(MemoryAddArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🧠 memory add '{}'{RESET}", args.data);
        let origin = args
            .origin
            .as_deref()
            .filter(|o| *o == "user" || *o == "agent");
        match self.memory.add(args.data, args.tags, origin) {
            Ok((key, updated)) => {
                info!("{DIM}  \u{2192} stored as {key}{RESET}");
                Ok(if updated {
                    format!("Updated existing entry {key}")
                } else {
                    format!("Stored as {key}")
                })
            }
            Err(e) => {
                info!("{DIM}  \u{2192} error: {e}{RESET}");
                Err(ToolError::Message(e))
            }
        }
    }
}
