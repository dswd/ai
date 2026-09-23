use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;
use crate::memory::{HitKind, Memory};

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
    memory: Memory,
    /// Session whose own transcripts are excluded to avoid duplication. `None`
    /// searches every session (maintenance).
    session: Option<String>,
}

impl MemorySearchTool {
    /// Unscoped search: transcript excerpts from every session are returned.
    /// Used by maintenance (`ai dream`), which reviews historical sessions and
    /// must not exclude the session it is processing.
    pub fn new(memory: Memory) -> Self {
        Self {
            memory,
            session: None,
        }
    }

    /// Session-scoped search: excludes the session in progress so it never
    /// re-injects its own conversation. Used by the interactive agent.
    pub fn scoped(memory: Memory, session: String) -> Self {
        Self {
            memory,
            session: Some(session),
        }
    }
}

impl PortableTool for MemorySearchTool {
    const NAME: &'static str = "memory_search";

    type Args = MemorySearchArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        let scope = if self.session.is_some() {
            " The current session's own exchanges are excluded."
        } else {
            ""
        };
        format!(
            "Search persistent memory and past conversation excerpts for entries relevant to a \
             query.{scope} Returns the best-matching items with their unique keys so they can be \
             referenced or deleted."
        )
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(MemorySearchArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🔍 memory search {:?}{RESET}", args.query);
        let top_k = args.limit.unwrap_or(5).min(50);
        let hits = self
            .memory
            .retrieve_excluding(&args.query, top_k, self.session.as_deref());
        if hits.is_empty() {
            info!("{DIM}  \u{2192} no matches{RESET}");
            return Ok("No matching memory entries.".to_string());
        }
        let out = hits
            .iter()
            .map(|h| match h.kind {
                HitKind::Memory => format!(
                    "({}) {} (score {:.2})",
                    h.key,
                    crate::memory::fragment(&h.text, &args.query),
                    h.score
                ),
                HitKind::Transcript => {
                    let session = h.session.as_deref().unwrap_or("?");
                    format!(
                        "(transcript {session}) {} (score {:.2})",
                        crate::memory::fragment(&h.text, &args.query),
                        h.score
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        info!("{DIM}  \u{2192} {} results{RESET}", hits.len());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbedder;
    use crate::session::TranscriptTuple;
    use std::sync::Arc;

    fn temp_memory(tag: &str) -> (std::path::PathBuf, Memory) {
        let dir = std::env::temp_dir().join(format!("ai-memsearch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mem = Memory::open(&dir.join("memory.db"), Arc::new(HashEmbedder::new(256))).unwrap();
        (dir, mem)
    }

    fn tuple(seq: usize, user: &str, agent: &str) -> TranscriptTuple {
        TranscriptTuple {
            seq,
            user: user.to_string(),
            agent: agent.to_string(),
        }
    }

    #[tokio::test]
    async fn test_scoped_search_excludes_only_the_current_session() {
        let (dir, mem) = temp_memory("scope");
        mem.index_session(
            "current",
            &[tuple(1, "tell me about berlin", "berlin is nice")],
        )
        .unwrap();
        mem.index_session(
            "past",
            &[tuple(1, "berlin trip plans", "visit berlin in may")],
        )
        .unwrap();
        let args = MemorySearchArgs {
            query: "berlin".to_string(),
            limit: Some(10),
        };

        let scoped = MemorySearchTool::scoped(mem.clone(), "current".to_string());
        let out = scoped.call(args.clone()).await.unwrap();
        assert!(
            !out.contains("transcript current"),
            "the current session must be excluded: {out}"
        );
        assert!(
            out.contains("transcript past"),
            "other sessions stay searchable: {out}"
        );

        // Maintenance (`ai dream`) uses the unscoped constructor: nothing excluded.
        let unscoped = MemorySearchTool::new(mem.clone());
        let out = unscoped.call(args).await.unwrap();
        assert!(
            out.contains("transcript current"),
            "unscoped search must see every session: {out}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
