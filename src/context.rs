//! Deterministic context editing (Q6/C).
//!
//! Old tool-result payloads dominate a long context and lose value once acted
//! on. This replaces them with a short stub for the history sent to the model,
//! while the persisted session log keeps the full originals (archive-first).
//! User and assistant text is never touched, and summarization is left as the
//! manual `/compact` fallback.

use rig_core::OneOrMany;
use rig_core::agent::{AgentHook, Flow, HookContext, RequestPatch, StepEvent, StepEventKind};
use rig_core::completion::CompletionModel;
use rig_core::completion::Message;
use rig_core::completion::message::{ToolResult, ToolResultContent, UserContent};

/// Only prune once the history is at least this long.
pub(crate) const PRUNE_THRESHOLD: usize = 24;
/// Always keep this many of the most recent messages verbatim.
pub(crate) const DEFAULT_KEEP_RECENT: usize = 12;
const STUB: &str = "[tool result elided to save context; re-run the tool if needed]";

/// Build the pruned history. Returns `None` when nothing would change, so the
/// caller can skip patching the request.
pub(crate) fn prune_history(history: &[Message], keep_recent: usize) -> Option<Vec<Message>> {
    if history.len() <= PRUNE_THRESHOLD {
        return None;
    }
    let len = history.len();
    let mut changed = false;
    let out: Vec<Message> = history
        .iter()
        .enumerate()
        .map(|(i, msg)| {
            if i + keep_recent >= len {
                msg.clone()
            } else {
                let stubbed = stub_tool_results(msg);
                changed |= stubbed != *msg;
                stubbed
            }
        })
        .collect();
    changed.then_some(out)
}

fn stub_tool_results(msg: &Message) -> Message {
    let Message::User { content } = msg else {
        return msg.clone();
    };
    let mut items: Vec<UserContent> = Vec::new();
    let mut changed = false;
    for item in content.iter() {
        match item {
            UserContent::ToolResult(tr) => {
                changed = true;
                items.push(UserContent::ToolResult(ToolResult {
                    id: tr.id.clone(),
                    call_id: tr.call_id.clone(),
                    content: OneOrMany::one(ToolResultContent::text(STUB)),
                }));
            }
            other => items.push(other.clone()),
        }
    }
    if !changed {
        return msg.clone();
    }
    match OneOrMany::many(items) {
        Ok(content) => Message::User { content },
        Err(_) => msg.clone(),
    }
}

/// A rig hook that prunes stale tool outputs from the history sent each turn.
pub(crate) struct ContextPruneHook {
    keep_recent: usize,
}

impl ContextPruneHook {
    pub(crate) fn new(keep_recent: usize) -> Self {
        Self { keep_recent }
    }
}

impl Default for ContextPruneHook {
    fn default() -> Self {
        Self::new(DEFAULT_KEEP_RECENT)
    }
}

impl<M: CompletionModel> AgentHook<M> for ContextPruneHook {
    fn observes(&self, kind: StepEventKind) -> bool {
        kind == StepEventKind::CompletionCall
    }

    async fn on_event(&self, _ctx: &HookContext, event: StepEvent<'_, M>) -> Flow {
        if let StepEvent::CompletionCall { history, .. } = event
            && let Some(pruned) = prune_history(history, self.keep_recent)
        {
            return Flow::patch_request(RequestPatch::new().history(pruned));
        }
        Flow::cont()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_result(id: &str, text: &str) -> Message {
        Message::tool_result(id, text)
    }

    #[test]
    fn test_no_prune_below_threshold() {
        let h: Vec<Message> = (0..5)
            .map(|i| tool_result("id", &format!("r{i}")))
            .collect();
        assert!(prune_history(&h, DEFAULT_KEEP_RECENT).is_none());
    }

    #[test]
    fn test_old_tool_results_stubbed_recent_kept() {
        let mut h: Vec<Message> = (0..PRUNE_THRESHOLD + 5)
            .map(|i| tool_result("id", &format!("payload-{i}")))
            .collect();
        h.push(Message::user("do the thing"));
        let pruned = prune_history(&h, DEFAULT_KEEP_RECENT).expect("should prune");
        assert_eq!(pruned.len(), h.len());
        // The most recent message is untouched.
        assert_eq!(pruned.last().unwrap(), h.last().unwrap());
        // An old tool result is stubbed.
        let old = pruned.first().unwrap();
        let text = format!("{old:?}");
        assert!(text.contains("elided"));
        assert!(!text.contains("payload-0"));
    }

    #[test]
    fn test_user_text_never_stubbed() {
        let mut h: Vec<Message> = Vec::new();
        for i in 0..PRUNE_THRESHOLD + 5 {
            h.push(tool_result("id", &format!("payload-{i}")));
        }
        h.push(Message::user("important user text"));
        let pruned = prune_history(&h, 1).expect("should prune");
        let last = pruned.last().unwrap();
        assert_eq!(last, &Message::user("important user text"));
    }
}
