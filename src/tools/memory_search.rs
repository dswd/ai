use std::sync::Arc;

use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;
use crate::memory::Memory;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemorySearchArgs {
    #[schemars(description = "The query to search memory with")]
    pub query: String,
    #[schemars(description = "Maximum number of results to return (default: 5)")]
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct MemorySearchTool {
    memory: Arc<Memory>,
}

impl MemorySearchTool {
    pub fn new(memory: Arc<Memory>) -> Self {
        Self { memory }
    }
}

impl Tool for MemorySearchTool {
    const NAME: &'static str = "memory_search";

    type Args = MemorySearchArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Search persistent memory for entries relevant to a query. Returns the best-matching memory entries (with their unique keys) so they can be referenced or deleted.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(MemorySearchArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🔍 memory search {:?}{RESET}", args.query);
        let top_k = args.limit.unwrap_or(5).min(50);
        let hits = self.memory.retrieve(&args.query, top_k);
        if hits.is_empty() {
            info!("{DIM}  \u{2192} no matches{RESET}");
            return Ok("No matching memory entries.".to_string());
        }
        let out = hits
            .iter()
            .map(|e| format!("({}) {}", e.id, e.text))
            .collect::<Vec<_>>()
            .join("\n");
        info!("{DIM}  \u{2192} {} results{RESET}", hits.len());
        Ok(out)
    }
}
