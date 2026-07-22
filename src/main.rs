mod cli;
mod config;
mod io;
mod mcp;
mod policy;
mod session;
mod tools;
mod util;

use clap::Parser;
use cli::Cli;
use config::Config;
use policy::{Action, Policy, PolicyRule};
use rig_core::{
    agent::AgentBuilder,
    agent::{MultiTurnStreamItem, PromptResponse, StreamingResult},
    client::CompletionClient,
    completion::{CompletionModel, Message},
    providers,
    streaming::{StreamedAssistantContent, StreamingChat, StreamingPrompt},
    tool::server::ToolServer,
};
use session::Session;
use log::{set_max_level, LevelFilter, Level, info, error};

use std::io::Write;
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    setup_logging(cli.verbose, cli.quiet);

    let mut config = load_config(&cli)?;
    apply_cli_overrides(&cli, &mut config);

    let system_prompt = cli
        .system
        .clone()
        .or_else(|| config.system_prompt.clone())
        .unwrap_or_else(|| {
            "You are a CLI assistant with access to tools for reading, writing, and searching files, \
             as well as running shell commands. Use tools when helpful. Keep responses concise. \
             For multi-step tasks, work methodically and report progress."
                .to_string()
        });

    let now = util::now_short();
    let system_prompt = format!("{system_prompt}\n\nCurrent time: {now}");

    let model_name = config.model.clone();
    let max_tokens = cli.max_tokens.or(config.max_tokens);
    let max_turns = cli.max_turns;
    let thinking = cli.thinking.or(config.thinking);

    let policy = load_policy(&cli, &config)?;
    let session_dir = config.session_dir_resolved();
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
        if !user_lines.is_empty() {
            io::set_history(&user_lines);
        }
    }

    let prompt_text = if let Some(text) = cli.prompt_text() {
        Some(text)
    } else {
        io::read_stdin()
    };

    let provider = config.provider.to_lowercase();

    let mcp_tool_sets = if !cli.mcp.is_empty() {
        mcp::connect_mcp_servers(&cli.mcp).await?
    } else {
        Vec::new()
    };

    match provider.as_str() {
        "openai" => {
            let client = openai_client(&config)?;
            let model = client.completion_model(&model_name);
            let agent = build_agent(model, &system_prompt, &policy, max_tokens, max_turns, mcp_tool_sets, thinking);

            if is_interactive {
                run_interactive(agent, &mut session, &session_dir, prompt_text).await?;
            } else if let Some(text) = prompt_text {
                run_oneshot(agent, &text).await?;
            } else {
                anyhow::bail!(
                    "No prompt provided. Pass a prompt argument or pipe text to stdin."
                );
            }
        }
        "anthropic" => {
            let client = anthropic_client(&config)?;
            let model = client.completion_model(&model_name);
            let agent = build_agent(model, &system_prompt, &policy, max_tokens, max_turns, mcp_tool_sets, thinking);

            if is_interactive {
                run_interactive(agent, &mut session, &session_dir, prompt_text).await?;
            } else if let Some(text) = prompt_text {
                run_oneshot(agent, &text).await?;
            } else {
                anyhow::bail!(
                    "No prompt provided. Pass a prompt argument or pipe text to stdin."
                );
            }
        }
        _ => {
            anyhow::bail!(
                "Unsupported provider: {provider}. Supported: openai, anthropic"
            );
        }
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
            eprintln!("{}", record.args());
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

fn load_config(cli: &Cli) -> anyhow::Result<Config> {
    if let Some(path) = &cli.config {
        Config::from_file(path)
    } else if let Some(default_path) = Config::default_path() {
        if default_path.exists() {
            Config::from_file(&default_path)
        } else {
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
        policy.add_cli_rule(PolicyRule::Allow(Action::Read, path.clone()));
    }
    for path in &cli.write {
        policy.add_cli_rule(PolicyRule::Allow(Action::Read, path.clone()));
        policy.add_cli_rule(PolicyRule::Allow(Action::Write, path.clone()));
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
            "OpenAI API key not found. Set OPENAI_API_KEY env var or api_key in config."
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
            "Anthropic API key not found. Set ANTHROPIC_API_KEY env var or api_key in config."
        ))?;

    let mut builder = providers::anthropic::Client::builder().api_key(api_key.as_str());
    if let Some(ref base) = config.api_base {
        builder = builder.base_url(base);
    }
    Ok(builder.build()?)
}

fn build_agent<M: CompletionModel + 'static>(
    model: M,
    system_prompt: &str,
    policy: &Policy,
    max_tokens: Option<usize>,
    max_turns: usize,
    mcp_tool_sets: Vec<mcp::McpToolSet>,
    thinking: Option<usize>,
) -> rig_core::agent::Agent<M> {
    let mut server = ToolServer::new()
        .tool(tools::fs::ReadFileTool::new(policy.clone()))
        .tool(tools::fs::WriteFileTool::new(policy.clone()))
        .tool(tools::fs::ListDirTool::new(policy.clone()))
        .tool(tools::fs::ReplaceInFileTool::new(policy.clone()))
        .tool(tools::fs::DeleteFileTool::new(policy.clone()))
        .tool(tools::fs::CreateDirectoryTool::new(policy.clone()))
        .tool(tools::exec::ExecuteTool::new(policy.clone()))
        .tool(tools::exec::GitDiffTool::new(policy.clone()))
        .tool(tools::exec::GitLogTool::new(policy.clone()))
        .tool(tools::search::SearchContentTool::new(policy.clone()))
        .tool(tools::search::FindFilesTool::new(policy.clone()))
        .tool(tools::think::ThinkTool::new())
        .tool(tools::web::WebFetchTool::new(policy.clone()))
        .tool(tools::web::WebSearchTool::new(policy.clone()));

    for set in mcp_tool_sets {
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

async fn stream_response<R>(
    stream: &mut StreamingResult<R>,
) -> anyhow::Result<PromptResponse> {
    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::Text(text),
            )) => {
                print!("{}", text.text);
                let _ = std::io::stdout().flush();
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::Reasoning(reasoning),
            )) => {
                let reasoning = reasoning.display_text();
                eprintln!("\x1b[90;3m{reasoning}\x1b[0m");
            }
            Ok(MultiTurnStreamItem::FinalResponse(resp)) => {
                println!();
                return Ok(resp);
            }
            Err(e) => {
                eprintln!("\nError: {e}");
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
    let mut stream = agent.stream_prompt(prompt).await;
    stream_response(&mut stream).await?;
    Ok(())
}

async fn run_interactive<M: CompletionModel + 'static>(
    agent: rig_core::agent::Agent<M>,
    session: &mut Session,
    session_dir: &std::path::Path,
    initial_prompt: Option<String>,
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

    if let Some(text) = initial_prompt {
        session.add_message("user", &text);
        let hist = chat_history.clone();
        let result = async {
            let mut stream = agent.stream_chat(&text, hist).await;
            let response = stream_response(&mut stream).await?;
            Ok::<_, anyhow::Error>(response)
        }.await;
        match result {
            Ok(response) => {
                session.add_message("assistant", &response.output);
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
        let input = io::read_user_input("> ");
        match input {
            Some(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if trimmed == "/exit" || trimmed == "/quit" {
                    session.save(session_dir)?;
                    info!("Session saved: {}", session.name);
                    break;
                }

                if trimmed == "/clear" {
                    session.messages.clear();
                    chat_history.clear();
                    io::stderr_line("[session cleared]");
                    continue;
                }

                if trimmed == "/session" {
                    io::stderr_line(&format!("Current session: {}", session.name));
                    continue;
                }

                if trimmed == "/tools" {
                    io::stderr_line("Available tools: read_file, write_file, list_dir, execute");
                    continue;
                }

                if trimmed == "/help" {
                    io::stderr_line(
                        "Commands: /exit, /quit, /clear, /session, /tools, /help",
                    );
                    continue;
                }

                session.add_message("user", trimmed);

                let hist = chat_history.clone();
                let result = async {
                    let mut stream = agent.stream_chat(trimmed, hist).await;
                    let response = stream_response(&mut stream).await?;
                    Ok::<_, anyhow::Error>(response)
                }.await;
                match result {
                    Ok(response) => {
                        session.add_message("assistant", &response.output);
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
                session.save(session_dir)?;
                info!("Session saved: {}", session.name);
                break;
            }
        }
    }

    Ok(())
}
