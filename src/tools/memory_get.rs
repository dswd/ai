use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;
use crate::memory::Memory;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryGetArgs {
    #[schemars(description = "The unique key of the memory entry to retrieve")]
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct MemoryGetTool {
    memory: Memory,
}

impl MemoryGetTool {
    pub fn new(memory: Memory) -> Self {
        Self { memory }
    }
}

impl PortableTool for MemoryGetTool {
    const NAME: &'static str = "memory_get";

    type Args = MemoryGetArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Retrieve a memory entry by its unique key, returning its full text, tags, origin, and \
         source. Use this to expand an entry referenced by key in search results or injected \
         context."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(MemoryGetArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📖 memory get {}{RESET}", args.key);
        let Some(entry) = self.memory.get(&args.key) else {
            let message = format!("No memory entry with key '{}'.", args.key);
            info!("{DIM}  \u{2192} {message}{RESET}");
            return Err(ToolError::Message(message));
        };
        info!("{DIM}  \u{2192} {}{RESET}", entry.text);
        let mut meta = vec![format!("origin: {}", entry.origin)];
        if !entry.tags.is_empty() {
            meta.push(format!("tags: {}", entry.tags.join(", ")));
        }
        meta.push(format!(
            "created {}",
            entry.created.get(..10).unwrap_or(&entry.created)
        ));
        if let Some(session) = &entry.source_session {
            meta.push(format!("source: {session}"));
        }
        Ok(format!(
            "({}) {}\n  {}",
            entry.id,
            entry.text,
            meta.join(" · ")
        ))
    }
}
