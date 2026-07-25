mod cli;
mod config;
mod init;
mod io;
mod memory;
mod output;
mod policy;
mod session;
mod tool;
mod tools;
mod util;

use ansi_color_constants::*;
use clap::Parser;
use cli::Cli;
use config::Config;
use log::{Level, LevelFilter};
use log::{error, info, set_max_level};
use policy::{Action, Policy, PolicyRule};
use rig_core::{
    agent::AgentBuilder,
    agent::{MultiTurnStreamItem, PromptResponse, StreamingResult},
    client::CompletionClient,
    completion::{Chat, CompletionModel, Message, Usage},
    providers,
    streaming::{StreamedAssistantContent, StreamingChat, StreamingPrompt},
    tool::server::ToolServer,
};
use session::Session;

use futures::StreamExt;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    setup_logging(cli.verbose, cli.quiet);

    if let Some(ref init_path) = cli.init {
        init::run(Some(init_path.clone()))?;
        return Ok(());
    }

    let vanilla = cli.is_vanilla();
    let mut config = load_config(&cli, vanilla)?;
    apply_cli_overrides(&cli, &mut config);

    let system_prompt = cli
        .system
        .clone()
        .or_else(|| config.system_prompt.clone())
        .unwrap_or_else(|| {
            "You are a CLI assistant. Keep responses concise. \
             For multi-step tasks, work methodically and report progress."
                .to_string()
        });

    let now = current_time();
    let mut system_prompt = format!("{system_prompt}\n\nCurrent time: {now}");

    let memory = if let Some(memory_path) = &cli.memory {
        let path = if memory_path.is_empty() {
            config.memory_path_resolved()
        } else {
            std::path::PathBuf::from(memory_path)
        };
        let mem = Arc::new(memory::Memory::load(&path)?);
        let md = mem.to_markdown();
        system_prompt = format!("{system_prompt}\n\n{md}");
        Some(mem)
    } else {
        None
    };

    let model_name = config.model.clone();
    let max_tokens = cli.max_tokens.or(config.max_tokens);
    let max_turns = cli.max_turns;
    let thinking = cli.thinking.or(config.thinking);

    let policy = load_policy(&cli, &config)?;
    system_prompt = format!("{system_prompt}\n\n{}", policy.summary());
    let session_dir = config.session_dir_resolved();

    if cli.list {
        return cmd_list_sessions(&session_dir);
    }

    if let Some(ref name) = cli.delete {
        return cmd_delete_session(name, &session_dir);
    }

    let is_interactive = cli.is_interactive();

    let session_name = match &cli.session {
        Some(name) => {
            if name.is_empty() {
                Some(session::generate_session_name())
            } else {
                Some(name.clone())
            }
        }
        None => None,
    };

    let mut session = if let Some(ref name) = session_name {
        match Session::load(name, &session_dir) {
            Ok(s) => {
                info!("Continuing session: {name}");
                s
            }
            Err(_) => {
                let s = Session::new(name.clone(), system_prompt.clone(), model_name.clone());
                info!("Started new session: {name}");
                s
            }
        }
    } else {
        Session::new(
            session::generate_session_name(),
            system_prompt.clone(),
            model_name.clone(),
        )
    };

    if is_interactive {
        let user_lines: Vec<String> = session
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.clone())
            .collect();
        io::load_session_history(&user_lines);
    }

    let cli_prompt = cli.prompt_text();
    let stdin_prompt = io::read_stdin_async().await;
    let prompt_text = match (cli_prompt, stdin_prompt) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    let provider = config.provider.to_lowercase();

    let tool_sets = if !cli.tool.is_empty() {
        tool::connect_tool_servers(&cli.tool).await?
    } else {
        Vec::new()
    };

    match provider.as_str() {
        "openai" => {
            let client = openai_client(&config)?;
            let model = client.completion_model(&model_name);
            let agent = build_agent(
                model,
                &system_prompt,
                &policy,
                max_tokens,
                max_turns,
                tool_sets,
                thinking,
                memory.as_ref().map(Arc::clone),
            );

            if is_interactive {
                run_interactive(
                    agent,
                    &mut session,
                    &session_dir,
                    prompt_text,
                    config.context_window,
                )
                .await?;
            } else if let Some(text) = prompt_text {
                run_oneshot(agent, &text).await?;
            } else {
                anyhow::bail!("No prompt provided. Pass a prompt argument or pipe text to stdin.");
            }
        }
        "anthropic" => {
            let client = anthropic_client(&config)?;
            let model = client.completion_model(&model_name);
            let agent = build_agent(
                model,
                &system_prompt,
                &policy,
                max_tokens,
                max_turns,
                tool_sets,
                thinking,
                memory.as_ref().map(Arc::clone),
            );

            if is_interactive {
                run_interactive(
                    agent,
                    &mut session,
                    &session_dir,
                    prompt_text,
                    config.context_window,
                )
                .await?;
            } else if let Some(text) = prompt_text {
                run_oneshot(agent, &text).await?;
            } else {
                anyhow::bail!("No prompt provided. Pass a prompt argument or pipe text to stdin.");
            }
        }
        _ => {
            anyhow::bail!("Unsupported provider: {provider}. Supported: openai, anthropic");
        }
    }

    Ok(())
}

fn cmd_list_sessions(dir: &std::path::Path) -> anyhow::Result<()> {
    let names = Session::list(dir)?;
    if names.is_empty() {
        println!("No saved sessions.");
        return Ok(());
    }
    for name in &names {
        if let Ok(s) = Session::load(name, dir) {
            println!(
                "{}  — {} messages, model {}, created {}",
                name,
                s.messages.len(),
                s.model,
                s.created
            );
        } else {
            println!("{name}");
        }
    }

    Ok(())
}

fn cmd_delete_session(name: &str, dir: &std::path::Path) -> anyhow::Result<()> {
    let path = dir.join(format!("{name}.json"));
    if path.exists() {
        std::fs::remove_file(&path)?;
        output::stderr_line(&format!("Deleted session: {name}"));
    } else {
        anyhow::bail!("Session not found: {name}");
    }
    Ok(())
}

fn setup_logging(verbose: bool, quiet: bool) {
    let level = if verbose {
        LevelFilter::Debug
    } else if quiet {
        LevelFilter::Warn
    } else {
        LevelFilter::Info
    };
    set_max_level(level);
    log::set_logger(&ConsoleLogger).expect("logger already set");
}

struct ConsoleLogger;

impl log::Log for ConsoleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        if metadata.target().starts_with("ai::") {
            metadata.level() <= log::max_level()
        } else {
            metadata.level() <= Level::Warn
        }
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            output::stderr_line(&format!("{}", record.args()));
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

fn is_quiet() -> bool {
    log::max_level() <= LevelFilter::Warn
}

fn load_config(cli: &Cli, vanilla: bool) -> anyhow::Result<Config> {
    if let Some(path) = &cli.config {
        Config::from_file(path)
    } else if let Some(default_path) = Config::default_path() {
        if default_path.exists() {
            Config::from_file(&default_path)
        } else {
            if vanilla {
                output::stderr_line("No config found. Run `ai --init` to create one.");
            }
            Ok(Config::default())
        }
    } else {
        Ok(Config::default())
    }
}

fn apply_cli_overrides(cli: &Cli, config: &mut Config) {
    if let Some(tokens) = cli.max_tokens {
        config.max_tokens = Some(tokens);
    }
    if let Some(ref system) = cli.system {
        config.system_prompt = Some(system.clone());
    }
}

fn load_policy(cli: &Cli, config: &Config) -> anyhow::Result<Policy> {
    let mut policy = if let Some(path) = &cli.policy {
        Policy::from_file(path)?
    } else if let Some(path) = &config.policy {
        if path.exists() {
            Policy::from_file(path)?
        } else {
            Policy::default()
        }
    } else {
        Policy::default()
    };

    for path in &cli.read {
        let resolved =
            policy::resolve_policy_pattern(path, &std::env::current_dir().unwrap_or_default());
        policy.add_cli_rule(PolicyRule::Allow(Action::Read, resolved));
    }
    for path in &cli.write {
        let resolved =
            policy::resolve_policy_pattern(path, &std::env::current_dir().unwrap_or_default());
        policy.add_cli_rule(PolicyRule::Allow(Action::Read, resolved.clone()));
        policy.add_cli_rule(PolicyRule::Allow(Action::Write, resolved));
    }
    for pat in &cli.execute {
        policy.add_cli_rule(PolicyRule::Allow(Action::Execute, pat.clone()));
    }

    for pat in &cli.web_fetch {
        policy.add_cli_rule(PolicyRule::Allow(Action::WebFetch, pat.clone()));
    }

    for pat in &cli.web_search {
        policy.add_cli_rule(PolicyRule::Allow(Action::WebSearch, pat.clone()));
    }

    if cli.web {
        policy.add_cli_rule(PolicyRule::Allow(Action::WebFetch, "**".to_string()));
        policy.add_cli_rule(PolicyRule::Allow(Action::WebSearch, "**".to_string()));
    }

    if cli.yolo {
        policy.add_cli_rule(PolicyRule::Allow(Action::Read, "**".to_string()));
        policy.add_cli_rule(PolicyRule::Allow(Action::Write, "**".to_string()));
        policy.add_cli_rule(PolicyRule::Allow(Action::Execute, "*".to_string()));
        policy.add_cli_rule(PolicyRule::Allow(Action::WebFetch, "**".to_string()));
        policy.add_cli_rule(PolicyRule::Allow(Action::WebSearch, "**".to_string()));
    }

    policy.ask = cli.interactive;

    Ok(policy)
}

fn openai_client(config: &Config) -> anyhow::Result<providers::openai::CompletionsClient> {
    let api_key = config
        .resolve_api_key()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .ok_or_else(|| anyhow::anyhow!(
            "OpenAI API key not found. Set OPENAI_API_KEY environment variable or api_key in config."
        ))?;

    let mut builder = providers::openai::CompletionsClient::builder().api_key(api_key.as_str());
    if let Some(ref base) = config.api_base {
        builder = builder.base_url(base);
    }
    Ok(builder.build()?)
}

fn anthropic_client(config: &Config) -> anyhow::Result<providers::anthropic::Client> {
    let api_key = config
        .resolve_api_key()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
        .ok_or_else(|| anyhow::anyhow!(
            "Anthropic API key not found. Set ANTHROPIC_API_KEY environment variable or api_key in config."
        ))?;

    let mut builder = providers::anthropic::Client::builder().api_key(api_key.as_str());
    if let Some(ref base) = config.api_base {
        builder = builder.base_url(base);
    }
    Ok(builder.build()?)
}

#[allow(clippy::too_many_arguments)]
fn build_agent<M: CompletionModel + 'static>(
    model: M,
    system_prompt: &str,
    policy: &Policy,
    max_tokens: Option<usize>,
    max_turns: usize,
    tool_sets: Vec<tool::ToolSet>,
    thinking: Option<usize>,
    memory: Option<Arc<memory::Memory>>,
) -> rig_core::agent::Agent<M> {
    let can_read = policy.ask || policy.has_any_allow(&Action::Read);
    let can_write = policy.ask || policy.has_any_allow(&Action::Write);
    let can_exec = policy.ask || policy.has_any_allow(&Action::Execute);
    let can_web_fetch = policy.ask || policy.has_any_allow(&Action::WebFetch);
    let can_web_search = policy.ask || policy.has_any_allow(&Action::WebSearch);

    let mut server = ToolServer::new();

    if can_read {
        server = server
            .tool(tools::fs::ReadFileTool::new(policy.clone()))
            .tool(tools::fs::ListDirTool::new(policy.clone()))
            .tool(tools::search::SearchContentTool::new(policy.clone()))
            .tool(tools::search::FindFilesTool::new(policy.clone()))
            .tool(tools::fs::FileInfoTool::new(policy.clone()))
            .tool(tools::fs::FileViewTool::new(policy.clone()));
    }

    if can_write {
        server = server
            .tool(tools::fs::WriteFileTool::new(policy.clone()))
            .tool(tools::fs::ReplaceInFileTool::new(policy.clone()))
            .tool(tools::fs::DeleteFileTool::new(policy.clone()))
            .tool(tools::fs::CreateDirectoryTool::new(policy.clone()))
            .tool(tools::fs::MoveFileTool::new(policy.clone()))
            .tool(tools::fs::CopyFileTool::new(policy.clone()));
    }

    if can_exec {
        server = server
            .tool(tools::exec::ExecuteTool::new(policy.clone()))
            .tool(tools::exec::GitDiffTool::new(policy.clone()))
            .tool(tools::exec::GitLogTool::new(policy.clone()));
    }

    if can_web_fetch {
        server = server
            .tool(tools::web::WebFetchTool::new(policy.clone()))
            .tool(tools::web::DownloadFileTool::new(policy.clone()));
    }

    if can_web_search {
        server = server.tool(tools::web::WebSearchTool::new(policy.clone()));
    }

    server = server.tool(tools::think::ThinkTool::new());

    if let Some(ref mem) = memory {
        server = server
            .tool(tools::memory::MemoryAddTool::new(Arc::clone(mem)))
            .tool(tools::memory::MemoryDeleteTool::new(Arc::clone(mem)));
    }

    for set in tool_sets {
        for tool in set.tools {
            server = server.rmcp_tool(tool, set.sink.clone());
        }
    }

    let handle = server.run();

    let mut builder = AgentBuilder::new(model)
        .preamble(system_prompt)
        .default_max_turns(max_turns)
        .tool_server_handle(handle);

    if let Some(budget) = thinking {
        builder = builder.additional_params(serde_json::json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": budget
            }
        }));
    }

    match max_tokens {
        Some(tokens) => builder.max_tokens(tokens as u64).build(),
        None => builder.build(),
    }
}

async fn stream_response<R>(stream: &mut StreamingResult<R>) -> anyhow::Result<PromptResponse> {
    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text))) => {
                output::stdout_push(&text.text);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(
                reasoning,
            ))) if !is_quiet() => {
                output::stderr_line(&format!(
                    "{ITALICS}{BLUE}{}{RESET}",
                    reasoning.display_text()
                ));
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::ReasoningDelta { reasoning, .. },
            )) if !is_quiet() => {
                output::stderr_push(&format!("{ITALICS}{BLUE}{reasoning}{RESET}"));
            }
            Ok(MultiTurnStreamItem::FinalResponse(resp)) => {
                output::stdout_finish();
                return Ok(resp);
            }
            Err(e) => {
                output::stderr_line(&format!("Error: {e}"));
            }
            _ => {}
        }
    }
    Err(anyhow::anyhow!("no final response"))
}

async fn run_oneshot<M: CompletionModel + 'static>(
    agent: rig_core::agent::Agent<M>,
    prompt: &str,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let mut stream = agent.stream_prompt(prompt).await;
    let response = stream_response(&mut stream).await?;
    print_usage(&response.usage, start.elapsed());
    Ok(())
}

async fn run_interactive<M: CompletionModel + 'static>(
    agent: rig_core::agent::Agent<M>,
    session: &mut Session,
    session_dir: &std::path::Path,
    initial_prompt: Option<String>,
    context_window: Option<usize>,
) -> anyhow::Result<()> {
    let mut chat_history: Vec<Message> = session
        .messages
        .iter()
        .map(|m| match m.role.as_str() {
            "user" => Message::user(&m.content),
            "assistant" => Message::assistant(&m.content),
            _ => Message::user(&m.content),
        })
        .collect();

    if chat_history.len() >= 2 {
        let last_user = session.messages.iter().rev().find(|m| m.role == "user");
        let last_assistant = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant");
        if let Some(msg) = last_user {
            output::stderr_line(&format!("> {}", msg.content));
        }
        if let Some(msg) = last_assistant {
            output::stdout_push(&msg.content);
            output::stdout_finish();
        }
    }

    let start = Instant::now();
    let mut total_usage = Usage::new();
    let mut last_input_tokens: u64 = 0;

    if let Some(text) = initial_prompt {
        session.add_message("user", &text);
        let hist = chat_history.clone();
        let result = async {
            let mut stream = agent.stream_chat(&text, hist).await;
            let response = stream_response(&mut stream).await?;
            Ok::<_, anyhow::Error>(response)
        }
        .await;
        match result {
            Ok(response) => {
                last_input_tokens = response.usage.input_tokens;
                session.add_message("assistant", &response.output);
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
                            chat_history =
                                vec![Message::user(format!("[Conversation summary: {summary}]"))];
                            session.messages.clear();
                            session.add_message("system", &summary);
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

                if trimmed == "/tools" {
                    io::stderr_line(
                        "Available tools: read_file, write_file, list_dir, replace_in_file, delete_file, create_directory, file_info, move_file, copy_file, search_content, find_files, execute, git_diff, git_log, web_fetch, web_search, download_file, think, memory_add, memory_delete",
                    );
                    continue;
                }

                if trimmed == "/help" {
                    io::stderr_line(
                        "Commands: /exit, /quit, /clear, /compact, /session, /tools, /help",
                    );
                    continue;
                }

                session.add_message("user", trimmed);

                let hist = chat_history.clone();
                let result = async {
                    let mut stream = agent.stream_chat(trimmed, hist).await;
                    let response = stream_response(&mut stream).await?;
                    Ok::<_, anyhow::Error>(response)
                }
                .await;
                match result {
                    Ok(response) => {
                        last_input_tokens = response.usage.input_tokens;
                        session.add_message("assistant", &response.output);
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

    if !session.messages.is_empty() {
        session.save(session_dir)?;
        print_usage(&total_usage, start.elapsed());
        info!("Session saved: {}", session.name);
        output::stderr_line(&format!("  resume: ai -s {}", session.name));
    }

    Ok(())
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

fn current_time() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let day_secs = total_secs % 86400;
    let h = day_secs / 3600;
    let mi = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    let (y, mo, d) = days_to_date(total_secs / 86400);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

fn days_to_date(days: u64) -> (u64, u64, u64) {
    let mut d = days as i64;
    let mut y = 1970i64;
    loop {
        let diy: i64 = if leap(y) { 366 } else { 365 };
        if d < diy {
            break;
        }
        d -= diy;
        y += 1;
    }
    let md = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let feb = if leap(y) { 29 } else { 28 };
    let mut m = 0u64;
    for (i, &days_in_month) in md.iter().enumerate() {
        let limit = if i == 1 { feb } else { days_in_month };
        if d < limit as i64 {
            break;
        }
        d -= limit as i64;
        m = i as u64 + 1;
    }
    (y as u64, m + 1, (d + 1) as u64)
}

fn leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
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

fn print_usage(usage: &Usage, elapsed: std::time::Duration) {
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
