use std::sync::Arc;

use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::Memory;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAddArgs {
    #[schemars(description = "The data to store in memory")]
    pub data: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("{0}")]
    Message(String),
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
    type Error = MemoryError;

    fn description(&self) -> String {
        "Store a piece of data in persistent memory. Returns a unique key that can be used to reference or delete the entry later.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(MemoryAddArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🧠 memory add '{}'{RESET}", args.data);
        match self.memory.add(args.data) {
            Ok(key) => {
                info!("{DIM}  \u{2192} stored as {key}{RESET}");
                Ok(format!("Stored as {key}"))
            }
            Err(e) => {
                info!("{DIM}  \u{2192} error: {e}{RESET}");
                Err(MemoryError::Message(e))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDeleteArgs {
    #[schemars(description = "The unique key of the memory entry to delete")]
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct MemoryDeleteTool {
    memory: Arc<Memory>,
}

impl MemoryDeleteTool {
    pub fn new(memory: Arc<Memory>) -> Self {
        Self { memory }
    }
}

impl Tool for MemoryDeleteTool {
    const NAME: &'static str = "memory_delete";

    type Args = MemoryDeleteArgs;
    type Output = String;
    type Error = MemoryError;

    fn description(&self) -> String {
        "Delete a memory entry by its unique key.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(MemoryDeleteArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🗑️ memory delete {}{RESET}", args.key);
        match self.memory.delete(&args.key) {
            Ok(msg) => {
                info!("{DIM}  \u{2192} {msg}{RESET}");
                Ok(msg)
            }
            Err(e) => {
                info!("{DIM}  \u{2192} error: {e}{RESET}");
                Err(MemoryError::Message(e))
            }
        }
    }
}
