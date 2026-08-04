mod cli;
mod config;
mod init;
mod io;
mod memory;
mod output;
mod policy;
mod providers;
mod session;
mod skills;
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
    providers as rig_providers,
    streaming::{StreamedAssistantContent, StreamingChat, StreamingPrompt},
    tool::server::ToolServer,
};
use session::{Role, Session};

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

    run(cli).await
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let vanilla = cli.is_vanilla();
    let mut config = load_config(&cli, vanilla)?;
    apply_cli_overrides(&cli, &mut config);

    let session_dir = config.session_dir_resolved();
    if cli.list {
        return cmd_list_sessions(&session_dir);
    }
    if let Some(ref name) = cli.delete {
        return cmd_delete_session(name, &session_dir);
    }

    let policy = load_policy(&cli, &config)?;
    let skills = Arc::new(skills::discover(&cli.skill, &config.skills_dir_resolved()));
    let (system_prompt, memory) = assemble_system_prompt(&cli, &config, &policy, &skills)?;
    log::debug!("system prompt:\n{system_prompt}");

    let model_name = config.model.clone();
    let max_tokens = cli.max_tokens.or(config.max_tokens);
    let max_turns = cli.max_turns;
    let thinking = cli.thinking.or(config.thinking);

    let mut session = resolve_session(&cli, &session_dir, &system_prompt, &model_name)?;
    let prompt_text = resolve_prompt_text(&cli).await;
    let (provider_spec, base_url) = resolve_provider(&config, thinking)?;

    let tool_sets = if !cli.tool.is_empty() {
        tool::connect_tool_servers(&cli.tool).await?
    } else {
        Vec::new()
    };

    #[cfg(feature = "browser")]
    let browser_state: Option<Arc<tools::BrowserState>> =
        if policy.ask || policy.has_any_allow(&Action::WebFetch) {
            match tools::BrowserState::new().await {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    log::warn!("Failed to initialize browser: {e}");
                    None
                }
            }
        } else {
            None
        };

    #[cfg(not(feature = "browser"))]
    let browser_state: Option<Arc<()>> = None;

    let ctx = AgentContext {
        system_prompt: &system_prompt,
        policy: &policy,
        max_tokens,
        max_turns,
        tool_sets,
        thinking,
        memory: memory.as_ref().map(Arc::clone),
        search: &config.search,
        skills: Arc::clone(&skills),
        #[cfg(feature = "browser")]
        browser_state,
        #[cfg(not(feature = "browser"))]
        _browser_state: browser_state,
        is_interactive: cli.is_interactive(),
        session: &mut session,
        session_dir: &session_dir,
        prompt_text,
        context_window: config.context_window,
    };

    match provider_spec.flavor {
        providers::Flavor::OpenAi => {
            run_agent(
                openai_client(&config, &base_url)?.completion_model(&model_name),
                ctx,
            )
            .await?
        }
        providers::Flavor::Anthropic => {
            run_agent(
                anthropic_client(&config, &base_url)?.completion_model(&model_name),
                ctx,
            )
            .await?
        }
    }

    Ok(())
}

struct AgentContext<'a> {
    system_prompt: &'a str,
    policy: &'a Policy,
    max_tokens: Option<usize>,
    max_turns: usize,
    tool_sets: Vec<tool::ToolSet>,
    thinking: Option<usize>,
    memory: Option<Arc<memory::Memory>>,
    search: &'a config::SearchConfig,
    skills: Arc<Vec<skills::Skill>>,
    #[cfg(feature = "browser")]
    browser_state: Option<Arc<tools::BrowserState>>,
    #[cfg(not(feature = "browser"))]
    _browser_state: Option<Arc<()>>,
    is_interactive: bool,
    session: &'a mut Session,
    session_dir: &'a std::path::Path,
    prompt_text: Option<String>,
    context_window: Option<usize>,
}

const DEFAULT_SYSTEM_PROMPT: &str = "You are a CLI assistant. Keep responses concise. \
     For multi-step tasks, work methodically and report progress.";

fn assemble_system_prompt(
    cli: &Cli,
    config: &Config,
    policy: &Policy,
    skills: &[skills::Skill],
) -> anyhow::Result<(String, Option<Arc<memory::Memory>>)> {
    let mut system_prompt = cli
        .system
        .clone()
        .or_else(|| config.system_prompt.clone())
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

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

    system_prompt = format!("{system_prompt}\n\n{}", policy.summary());

    if !skills.is_empty() {
        system_prompt = format!("{system_prompt}\n\n{}", skills::summary(skills));
    }

    Ok((system_prompt, memory))
}

fn resolve_session(
    cli: &Cli,
    session_dir: &std::path::Path,
    system_prompt: &str,
    model_name: &str,
) -> anyhow::Result<Session> {
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

    let session = if let Some(ref name) = session_name {
        match Session::load(name, session_dir) {
            Ok(s) => {
                info!("Continuing session: {name}");
                s
            }
            Err(_) => {
                let s = Session::new(
                    name.clone(),
                    system_prompt.to_string(),
                    model_name.to_string(),
                );
                info!("Started new session: {name}");
                s
            }
        }
    } else {
        Session::new(
            session::generate_session_name(),
            system_prompt.to_string(),
            model_name.to_string(),
        )
    };

    if cli.is_interactive() {
        let user_lines: Vec<String> = session
            .messages
            .iter()
            .filter(|m| m.role == Role::User)
            .map(|m| m.content.clone())
            .collect();
        io::load_session_history(&user_lines);
    }

    Ok(session)
}

async fn resolve_prompt_text(cli: &Cli) -> Option<String> {
    let cli_prompt = cli.prompt_text();
    let stdin_prompt = io::read_stdin_async().await;
    match (cli_prompt, stdin_prompt) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn resolve_provider(
    config: &Config,
    thinking: Option<usize>,
) -> anyhow::Result<(&'static providers::Provider, String)> {
    let provider = config.provider.to_lowercase();
    let provider_spec = providers::resolve(&provider).ok_or_else(|| {
        let supported = providers::all_names().collect::<Vec<_>>().join(", ");
        anyhow::anyhow!("Unsupported provider: {provider}. Supported: {supported}")
    })?;

    let base_url = config
        .api_base
        .clone()
        .or_else(|| provider_spec.default_base_url.map(str::to_string))
        .ok_or_else(|| anyhow::anyhow!("provider '{provider}' requires an api_base in config"))?;

    if thinking.is_some() && provider_spec.flavor != providers::Flavor::Anthropic {
        log::warn!(
            "--thinking is only supported by the anthropic flavor; it has no effect with provider '{provider}'"
        );
    }

    Ok((provider_spec, base_url))
}

async fn run_agent<M: CompletionModel + 'static>(
    model: M,
    ctx: AgentContext<'_>,
) -> anyhow::Result<()> {
    let agent = build_agent(model, &ctx);
    dispatch_agent(agent, ctx).await
}

async fn dispatch_agent<M: CompletionModel + 'static>(
    agent: rig_core::agent::Agent<M>,
    ctx: AgentContext<'_>,
) -> anyhow::Result<()> {
    if ctx.is_interactive {
        run_interactive(
            agent,
            ctx.session,
            ctx.session_dir,
            ctx.prompt_text,
            ctx.context_window,
        )
        .await?;
    } else if let Some(text) = ctx.prompt_text {
        run_oneshot(agent, &text).await?;
    } else {
        anyhow::bail!("No prompt provided. Pass a prompt argument or pipe text to stdin.");
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
        if metadata.target().starts_with("ai::") || metadata.target() == "ai" {
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
    if let Some(ref model) = cli.model {
        config.model = model.clone();
    }
    if let Some(ref provider) = cli.provider {
        config.provider = provider.clone();
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

    policy.ask = cli.ask || cli.is_interactive();

    Ok(policy)
}

fn openai_client(
    config: &Config,
    base_url: &str,
) -> anyhow::Result<rig_providers::openai::CompletionsClient> {
    let api_key = config
        .resolve_api_key()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .ok_or_else(|| anyhow::anyhow!(
            "OpenAI API key not found. Set OPENAI_API_KEY environment variable or api_key in config."
        ))?;

    let builder = rig_providers::openai::CompletionsClient::builder()
        .api_key(api_key.as_str())
        .base_url(base_url);
    Ok(builder.build()?)
}

fn anthropic_client(
    config: &Config,
    base_url: &str,
) -> anyhow::Result<rig_providers::anthropic::Client> {
    let api_key = config
        .resolve_api_key()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
        .ok_or_else(|| anyhow::anyhow!(
            "Anthropic API key not found. Set ANTHROPIC_API_KEY environment variable or api_key in config."
        ))?;

    let builder = rig_providers::anthropic::Client::builder()
        .api_key(api_key.as_str())
        .base_url(base_url);
    Ok(builder.build()?)
}

fn build_agent<M: CompletionModel + 'static>(
    model: M,
    ctx: &AgentContext<'_>,
) -> rig_core::agent::Agent<M> {
    let can_read = ctx.policy.ask || ctx.policy.has_any_allow(&Action::Read);
    let can_write = ctx.policy.ask || ctx.policy.has_any_allow(&Action::Write);
    let can_web_fetch = ctx.policy.ask || ctx.policy.has_any_allow(&Action::WebFetch);
    let can_web_search = ctx.policy.ask || ctx.policy.has_any_allow(&Action::WebSearch);

    let mut server = ToolServer::new();

    if can_read {
        server = server
            .tool(tools::ReadFileTool::new(ctx.policy.clone()))
            .tool(tools::ListDirTool::new(ctx.policy.clone()))
            .tool(tools::SearchContentTool::new(ctx.policy.clone()))
            .tool(tools::FindFilesTool::new(ctx.policy.clone()))
            .tool(tools::FileInfoTool::new(ctx.policy.clone()))
            .tool(tools::FileViewTool::new(ctx.policy.clone()))
            .tool(tools::GitDiffTool::new(ctx.policy.clone()))
            .tool(tools::GitLogTool::new(ctx.policy.clone()));
    }

    if can_write {
        server = server
            .tool(tools::WriteFileTool::new(ctx.policy.clone()))
            .tool(tools::ReplaceInFileTool::new(ctx.policy.clone()))
            .tool(tools::DeleteFileTool::new(ctx.policy.clone()))
            .tool(tools::CreateDirectoryTool::new(ctx.policy.clone()))
            .tool(tools::MoveFileTool::new(ctx.policy.clone()))
            .tool(tools::CopyFileTool::new(ctx.policy.clone()));
    }

    server = server
        .tool(tools::ExecuteTool::new(ctx.policy.clone()))
        .tool(tools::GetCurrentTimeTool::new());

    if can_web_fetch {
        server = server.tool(tools::WebFetchTool::new(ctx.policy.clone()));
        #[cfg(feature = "browser")]
        if let Some(ref bs) = ctx.browser_state {
            server = server
                .tool(tools::BrowserNavigateTool::new(
                    ctx.policy.clone(),
                    Arc::clone(bs),
                ))
                .tool(tools::BrowserClickTool::new(
                    ctx.policy.clone(),
                    Arc::clone(bs),
                ))
                .tool(tools::BrowserEvaluateTool::new(
                    ctx.policy.clone(),
                    Arc::clone(bs),
                ))
                .tool(tools::BrowserGetContentTool::new(
                    ctx.policy.clone(),
                    Arc::clone(bs),
                ))
                .tool(tools::BrowserGetElementTool::new(
                    ctx.policy.clone(),
                    Arc::clone(bs),
                ));
        }
    }

    if can_web_fetch && can_write {
        server = server.tool(tools::DownloadFileTool::new(ctx.policy.clone()));
    }

    if can_web_search {
        server = server.tool(tools::WebSearchTool::new(
            ctx.policy.clone(),
            ctx.search.clone(),
        ));
    }

    if let Some(ref mem) = ctx.memory {
        server = server
            .tool(tools::MemoryAddTool::new(Arc::clone(mem)))
            .tool(tools::MemoryDeleteTool::new(Arc::clone(mem)));
    }

    if !ctx.skills.is_empty() {
        server = server.tool(tools::LoadSkillTool::new(Arc::clone(&ctx.skills)));
    }

    for set in &ctx.tool_sets {
        for tool in &set.tools {
            server = server.rmcp_tool(tool.clone(), set.sink.clone());
        }
    }

    let handle = server.run();

    let mut builder = AgentBuilder::new(model)
        .preamble(ctx.system_prompt)
        .default_max_turns(ctx.max_turns)
        .tool_server_handle(handle);

    if let Some(budget) = ctx.thinking {
        builder = builder.additional_params(serde_json::json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": budget
            }
        }));
    }

    match ctx.max_tokens {
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
                let result = async {
                    let mut stream = agent.stream_chat(trimmed, hist).await;
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
