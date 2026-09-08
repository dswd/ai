use crate::agent::stream_response;
use crate::io;
use crate::logging::is_quiet;
use crate::memory;
use crate::output;
use crate::session::{Role, Session};
use ansi_color_constants::*;
use log::{error, info};
use rig_core::{
    agent::Agent,
    completion::{Chat, CompletionModel, Message, Usage},
    streaming::StreamingChat,
};
use std::sync::Arc;
use std::time::Instant;

pub(crate) async fn run_interactive<M: CompletionModel + 'static>(
    agent: Agent<M>,
    session: &mut Session,
    session_dir: &std::path::Path,
    initial_prompt: Option<String>,
    context_window: Option<usize>,
    memory: Option<Arc<memory::Memory>>,
) -> anyhow::Result<()> {
    let mut chat_history: Vec<Message> = session
        .messages
        .iter()
        .map(|m| match m.role {
            Role::User => Message::user(&m.content),
            Role::Assistant => Message::assistant(&m.content),
            Role::System => Message::system(&m.content),
        })
        .collect();

    if chat_history.len() >= 2 {
        let last_user_idx = session
            .messages
            .iter()
            .rposition(|m| m.role == Role::User)
            .unwrap_or(0);
        for msg in &session.messages[last_user_idx..] {
            match msg.role {
                Role::Assistant => {
                    output::stdout_push(&msg.content);
                    output::stdout_finish();
                }
                Role::User => output::stderr_line(&format!("> {}", msg.content)),
                Role::System => {}
            }
        }
    }

    let start = Instant::now();
    let mut total_usage = Usage::new();
    let mut last_input_tokens: u64 = 0;

    if let Some(text) = initial_prompt {
        session.add_message(Role::User, &text);
        let hist = chat_history.clone();
        let text = augment_prompt(&text, memory.as_deref()).unwrap_or(text);
        let result = async {
            let mut stream = agent.stream_chat(&text, hist).await;
            let response = stream_response(&mut stream).await?;
            Ok::<_, anyhow::Error>(response)
        }
        .await;
        match result {
            Ok(response) => {
                last_input_tokens = response.usage.input_tokens;
                session.add_message(Role::Assistant, &response.output);
                total_usage = accumulate(&total_usage, &response.usage);
                if let Some(messages) = response.messages {
                    chat_history.extend(messages);
                }
                session.save(session_dir)?;
            }
            Err(e) => {
                error!("Error: {e}");
                io::stderr_line(&format!("Error: {e}"));
            }
        }
    }

    loop {
        let prompt = format_interactive_prompt(last_input_tokens, context_window);
        let input = io::read_user_input(&prompt);
        match input {
            Some(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if trimmed == "/exit" || trimmed == "/quit" {
                    break;
                }

                if trimmed == "/clear" {
                    session.messages.clear();
                    chat_history.clear();
                    session.reconciled_until = 0;
                    last_input_tokens = 0;
                    io::stderr_line("[session cleared]");
                    continue;
                }

                if trimmed == "/compact" {
                    let old_count = chat_history.len();
                    if old_count < 2 {
                        io::stderr_line("[nothing to compact]");
                        continue;
                    }

                    let compact_result = async {
                        let mut hist = chat_history.clone();

                        agent
                            .chat("Summarize this conversation concisely, preserving all important decisions, code changes, and user preferences. Return only the summary, no commentary.", &mut hist)
                            .await
                            .map_err(|e| anyhow::anyhow!("compact failed: {e}"))
                    }.await;

                    match compact_result {
                        Ok(summary) => {
                            let est_tokens = summary.len() as u64 / 4;
                            let summary_msg = format!("[Conversation summary: {summary}]");
                            chat_history = vec![Message::system(summary_msg.clone())];
                            session.messages.clear();
                            session.add_message(Role::System, &summary_msg);
                            session.reconciled_until = session.messages.len();
                            last_input_tokens = est_tokens;
                            io::stderr_line(&format!(
                                "[context compacted: {old_count} messages -> ~{t} tokens]",
                                t = fmt_tok(est_tokens)
                            ));
                        }
                        Err(e) => {
                            error!("Compact error: {e}");
                            io::stderr_line(&format!("Compact failed: {e}"));
                        }
                    }
                    continue;
                }

                if trimmed == "/session" {
                    io::stderr_line(&format!("Current session: {}", session.name));
                    continue;
                }

                if trimmed == "/help" {
                    io::stderr_line("Commands: /exit, /quit, /clear, /compact, /session, /help");
                    continue;
                }

                session.add_message(Role::User, trimmed);

                let hist = chat_history.clone();
                let text = augment_prompt(trimmed, memory.as_deref())
                    .unwrap_or_else(|| trimmed.to_string());
                let result = async {
                    let mut stream = agent.stream_chat(&text, hist).await;
                    let response = stream_response(&mut stream).await?;
                    Ok::<_, anyhow::Error>(response)
                }
                .await;
                match result {
                    Ok(response) => {
                        last_input_tokens = response.usage.input_tokens;
                        session.add_message(Role::Assistant, &response.output);
                        total_usage = accumulate(&total_usage, &response.usage);
                        if let Some(messages) = response.messages {
                            chat_history.extend(messages);
                        }
                        session.save(session_dir)?;
                    }
                    Err(e) => {
                        error!("Error: {e}");
                        io::stderr_line(&format!("Error: {e}"));
                    }
                }
            }
            None => {
                break;
            }
        }
    }

    if let Some(ref mem) = memory {
        reconcile_memory(&agent, session, mem).await;
    }

    if !session.messages.is_empty() {
        session.save(session_dir)?;
        print_usage(&total_usage, start.elapsed());
        info!("Session saved: {}", session.name);
        output::stderr_line(&format!("  resume: ai -s {}", session.name));
    }

    Ok(())
}

/// Ask the model to review the unreconciled part of the conversation and store
/// durable facts in memory using the memory tools. Non-fatal: failures log and continue.
async fn reconcile_memory<M: CompletionModel + 'static>(
    agent: &Agent<M>,
    session: &mut Session,
    memory: &memory::Memory,
) {
    let start = session.reconciled_until.min(session.messages.len());
    if start >= session.messages.len() {
        return;
    }

    let conversation = session.messages[start..]
        .iter()
        .map(|m| match m.role {
            Role::User => format!("user: {}", m.content),
            Role::Assistant => format!("assistant: {}", m.content),
            Role::System => format!("system: {}", m.content),
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "The following is a portion of a user conversation. Review it and store any durable \
         facts a long-term memory should keep: user preferences, personal details, decisions, \
         and commitments explicitly stated in the conversation. Ignore transient requests, \
         greetings, and task-specific instructions.\n\n\
         Conversation:\n{conversation}\n\n\
         Use the memory_search tool to check whether a fact is already stored; if it is, do \
         not store it again. Use the memory_add tool to store each new fact, providing 2-5 \
         short keywords for retrieval. Reply with a short summary of what you stored."
    );

    let before: Vec<memory::MemoryEntry> = memory.list();
    let mut hist = Vec::<Message>::new();
    match agent.chat(&prompt, &mut hist).await {
        Ok(_) => {}
        Err(e) => {
            log::warn!("Memory reconciliation failed: {e}");
        }
    }

    let after: Vec<memory::MemoryEntry> = memory.list();
    let before_by_id: std::collections::HashMap<String, String> = before
        .iter()
        .map(|e| (e.id.clone(), e.updated.clone()))
        .collect();
    let mut added = 0;
    let mut updated = 0;
    for entry in &after {
        match before_by_id.get(&entry.id) {
            Some(prev_updated) if prev_updated != &entry.updated => updated += 1,
            None => added += 1,
            _ => {}
        }
    }

    session.reconciled_until = session.messages.len();
    if added + updated > 0 {
        output::stderr_line(&format!(
            "  🧠 memory reconciled: {added} new, {updated} updated"
        ));
    }
}

pub(crate) fn augment_prompt(prompt: &str, memory: Option<&memory::Memory>) -> Option<String> {
    let mem = memory?;
    let hits = mem.retrieve(prompt, memory::TOP_K);
    if hits.is_empty() {
        return None;
    }
    for entry in &hits {
        output::stderr_line(&format!("{GREY}🧠 from memory: {}{RESET}", entry.text));
    }
    let context = hits
        .iter()
        .map(|e| format!("- ({}) {}", e.id, e.text))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "## Relevant memory\n{context}\n\n## User message\n{prompt}"
    ))
}

fn format_interactive_prompt(last_input_tokens: u64, context_window: Option<usize>) -> String {
    match context_window {
        Some(window) if window > 0 && last_input_tokens > 0 => {
            let percent = (last_input_tokens as f64 / window as f64) * 100.0;
            let warning = if percent >= 75.0 {
                "\u{26A0}\u{FE0F} "
            } else {
                ""
            };
            format!(
                "{warning}[{inp}/{win}] > ",
                inp = fmt_tok(last_input_tokens),
                win = fmt_tok(window as u64),
            )
        }
        _ => "> ".to_string(),
    }
}

fn accumulate(total: &Usage, usage: &Usage) -> Usage {
    Usage {
        input_tokens: total.input_tokens + usage.input_tokens,
        output_tokens: total.output_tokens + usage.output_tokens,
        total_tokens: total.total_tokens + usage.total_tokens,
        cached_input_tokens: total.cached_input_tokens + usage.cached_input_tokens,
        cache_creation_input_tokens: total.cache_creation_input_tokens
            + usage.cache_creation_input_tokens,
        tool_use_prompt_tokens: total.tool_use_prompt_tokens + usage.tool_use_prompt_tokens,
        reasoning_tokens: total.reasoning_tokens + usage.reasoning_tokens,
    }
}

pub(crate) fn print_usage(usage: &Usage, elapsed: std::time::Duration) {
    if is_quiet() {
        return;
    }
    let secs = elapsed.as_secs_f64();
    let out_visible = usage.output_tokens.saturating_sub(usage.reasoning_tokens);
    output::stderr_line(&format!(
        "{BOLD}📊 {total} tokens in {dur}  ({inp} in, {reas} thinking, {out} out){RESET}",
        total = fmt_tok(usage.total_tokens),
        inp = fmt_tok(usage.input_tokens),
        out = fmt_tok(out_visible),
        reas = fmt_tok(usage.reasoning_tokens),
        dur = format_duration(secs),
    ));
}

fn fmt_tok(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}m", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_duration(secs: f64) -> String {
    if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let m = (secs / 60.0) as u64;
        let s = secs % 60.0;
        format!("{m}m {s:.0}s")
    }
}
