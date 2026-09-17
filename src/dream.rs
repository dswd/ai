use std::collections::HashSet;
use std::sync::Arc;

use futures::StreamExt;
use rig::agent::AgentBuilder;
use rig::completion::{Chat, CompletionModel, Message};
use rig::tool::server::ToolServer;

use crate::memory::{self, Memory, MemoryEntry, TranscriptRow};
use crate::tools::{MemoryAddTool, MemoryDeleteTool, MemoryGetTool, MemorySearchTool};

const PREAMBLE: &str = "You are a maintenance agent for a personal AI assistant's long-term memory. \
     Follow the instructions exactly and use the provided memory tools.";

const EXTRACT_TASK: &str = "Below are completed exchanges from a past conversation session. Extract \
     durable facts that a long-term memory should keep: user preferences, personal details, \
     decisions, and commitments. Ignore transient requests, greetings, and task-specific \
     instructions. For each fact, call the memory_add tool with a concise statement, 2-5 short \
     tags, and origin set to \"user\" if the user stated it or \"agent\" if it came from the \
     assistant. Use memory_search first if you are unsure whether a fact is already stored. Reply \
     with a short summary of what you stored.";

const JUDGE_TASK: &str = "Below are memory entries that have not been used or reviewed for a while. \
     For each entry, decide whether it is still worth keeping. If an entry is superseded by a \
     newer similar entry, contradicted by it, or otherwise clearly no longer relevant, call the \
     memory_delete tool with its key. Only delete entries that are clearly obsolete; when in \
     doubt, keep them. Reply with a short summary of what you deleted.";

/// Run the three maintenance steps: extract, prune, judge.
pub async fn run<M: CompletionModel + Clone + 'static>(
    model: M,
    memory: Arc<Memory>,
    jobs: usize,
    max_turns: usize,
) -> anyhow::Result<()> {
    let jobs = jobs.max(1);

    let batches = memory.unprocessed_batches(memory::EXTRACT_BATCH);
    let before = memory.count_memory();
    let (done, total) = extract(&model, &memory, batches, jobs, max_turns).await;
    let after_extract = memory.count_memory();
    println!(
        "extract: {done}/{total} batch(es) processed, {} memories now ({} added/updated)",
        after_extract,
        after_extract as i64 - before as i64
    );

    let pruned = memory.prune_transcripts(memory::RETENTION_DAYS)?;
    println!("prune:   {pruned} processed transcript tuple(s) removed");

    let (judged, deleted) = judge(&model, &memory, jobs, max_turns).await;
    println!("judge:   {judged} entr(y/ies) reviewed, {deleted} deleted");

    Ok(())
}

async fn extract<M: CompletionModel + Clone + 'static>(
    model: &M,
    memory: &Arc<Memory>,
    batches: Vec<Vec<TranscriptRow>>,
    jobs: usize,
    max_turns: usize,
) -> (usize, usize) {
    let total = batches.len();
    let results = futures::stream::iter(batches.into_iter().map(|batch| {
        let model = model.clone();
        let memory = Arc::clone(memory);
        async move {
            let session = batch[0].session.clone();
            let ctx = (*memory).fork(Some(&session), "agent");
            let conversation = batch
                .iter()
                .map(|r| format!("--- exchange {} ---\n{}", r.seq, r.render()))
                .collect::<Vec<_>>()
                .join("\n\n");
            let prompt = format!("{EXTRACT_TASK}\n\nSession: {session}\n\n{conversation}");
            let agent = build_agent(model, ctx, None, max_turns);
            let mut history = Vec::<Message>::new();
            match agent.chat(&prompt, &mut history).await {
                Ok(_) => {
                    let rowids: Vec<i64> = batch.iter().map(|r| r.rowid).collect();
                    if let Err(e) = memory.mark_processed(&rowids) {
                        log::warn!("failed to mark transcripts processed: {e}");
                    }
                    1
                }
                Err(e) => {
                    log::warn!("extract batch failed for session {session}: {e}");
                    0
                }
            }
        }
    }))
    .buffer_unordered(jobs)
    .collect::<Vec<usize>>()
    .await;
    (results.iter().sum(), total)
}

async fn judge<M: CompletionModel + Clone + 'static>(
    model: &M,
    memory: &Arc<Memory>,
    jobs: usize,
    max_turns: usize,
) -> (usize, usize) {
    let pool = memory.judge_candidates(jobs * memory::JUDGE_BATCH * 4, memory::JUDGE_DAYS);
    if pool.is_empty() {
        return (0, 0);
    }
    let batches: Vec<Vec<MemoryEntry>> = pool
        .chunks(memory::JUDGE_BATCH)
        .map(|chunk| chunk.to_vec())
        .collect();
    let before = memory.count_memory();

    let results = futures::stream::iter(batches.into_iter().map(|batch| {
        let model = model.clone();
        let memory = Arc::clone(memory);
        async move {
            let ids: Vec<String> = batch.iter().map(|e| e.id.clone()).collect();
            let rendered = batch
                .iter()
                .map(|e| render_candidate(&memory, e))
                .collect::<Vec<_>>()
                .join("\n");
            let prompt = format!("{JUDGE_TASK}\n\n{rendered}");
            let allowed = Arc::new(ids.iter().cloned().collect::<HashSet<_>>());
            let ctx = (*memory).fork(None, "agent");
            let agent = build_agent(model, ctx, Some(allowed), max_turns);
            let mut history = Vec::<Message>::new();
            match agent.chat(&prompt, &mut history).await {
                Ok(_) => {
                    if let Err(e) = memory.mark_judged(&ids) {
                        log::warn!("failed to mark entries judged: {e}");
                    }
                    batch.len()
                }
                Err(e) => {
                    log::warn!("judge batch failed: {e}");
                    0
                }
            }
        }
    }))
    .buffer_unordered(jobs)
    .collect::<Vec<usize>>()
    .await;

    let judged = results.iter().sum();
    let after = memory.count_memory();
    (judged, before.saturating_sub(after))
}

fn render_candidate(memory: &Memory, entry: &MemoryEntry) -> String {
    let newer = memory.similar_newer(entry, memory::SIMILAR_LIMIT);
    let newer = if newer.is_empty() {
        "  (no newer similar entries)".to_string()
    } else {
        newer
            .iter()
            .map(|n| format!("    - ({}) {} {}", n.id, n.updated, n.text))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "- key {} [{}] {} (last used {})\n  newer similar:\n{}",
        entry.id,
        entry.tags.join(", "),
        entry.text,
        entry.last_used,
        newer
    )
}

/// A minimal agent exposing only the memory tools. `allowed`, when set, restricts
/// deletion to those keys (the judge's candidate batch). `max_turns` bounds the
/// tool-calling loop; rig's default of 1 is far too tight for a batch of facts.
fn build_agent<M: CompletionModel + Clone + 'static>(
    model: M,
    memory: Memory,
    allowed: Option<Arc<HashSet<String>>>,
    max_turns: usize,
) -> rig::agent::Agent {
    let mut server = ToolServer::new()
        .tool(MemoryAddTool::new(memory.clone()))
        .tool(MemorySearchTool::new(memory.clone()))
        .tool(MemoryGetTool::new(memory.clone()));
    if let Some(allowed) = allowed {
        server = server.tool(MemoryDeleteTool::scoped(memory, allowed));
    }
    let handle = server.run();
    AgentBuilder::new(model)
        .preamble(PREAMBLE)
        .default_max_turns(max_turns)
        .tool_server_handle(handle)
        .build()
}
