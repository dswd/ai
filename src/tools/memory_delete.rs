use std::collections::HashSet;
use std::sync::Arc;

use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;
use crate::memory::Memory;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDeleteArgs {
    #[schemars(description = "The unique key of the memory entry to delete")]
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct MemoryDeleteTool {
    memory: Memory,
    /// When set, only these keys may be deleted. Used by the dreaming judge so a
    /// confused or manipulated model cannot delete entries outside its batch.
    allowed: Option<Arc<HashSet<String>>>,
}

impl MemoryDeleteTool {
    pub fn new(memory: Memory) -> Self {
        Self {
            memory,
            allowed: None,
        }
    }

    pub fn scoped(memory: Memory, allowed: Arc<HashSet<String>>) -> Self {
        Self {
            memory,
            allowed: Some(allowed),
        }
    }
}

impl PortableTool for MemoryDeleteTool {
    const NAME: &'static str = "memory_delete";

    type Args = MemoryDeleteArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Delete a memory entry by its unique key.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(MemoryDeleteArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🗑️ memory delete {}{RESET}", args.key);
        if let Some(allowed) = &self.allowed
            && !allowed.contains(&args.key)
        {
            return Err(ToolError::Message(format!(
                "Key '{}' is not one of the entries under review.",
                args.key
            )));
        }
        match self.memory.delete(&args.key) {
            Ok(msg) => {
                info!("{DIM}  \u{2192} {msg}{RESET}");
                Ok(msg)
            }
            Err(e) => {
                info!("{DIM}  \u{2192} error: {e}{RESET}");
                Err(ToolError::Message(e))
            }
        }
    }
}
