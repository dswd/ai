mod cli;
mod config;
mod io;
mod mcp;
mod policy;
mod session;
mod tools;

use clap::Parser;
use cli::Cli;
use config::Config;
use policy::{Action, Policy, PolicyRule};
use rig_core::{
    agent::AgentBuilder,
    client::CompletionClient,
    completion::{Chat, CompletionModel, Message, Prompt},
    providers,
    tool::server::ToolServer,
};
use session::Session;
use std::io::Write;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;

struct IconWriter {
    inner: std::io::Stderr,
    buffer: String,
}

impl IconWriter {
    fn new() -> Self {
        IconWriter {
            inner: std::io::stderr(),
            buffer: String::new(),
        }
    }

    fn format_line(line: &str) -> String {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return "\n".to_string();
        }

        if let Some(rest) = trimmed.strip_prefix("tool_response:") {
            return format!("  tool_response:{}\n", rest);
        }
        if let Some(rest) = trimmed.strip_prefix("tool_call:") {
            if rest.starts_with(" think") || rest.starts_with(" \"think") {
                return format!("\u{1F9E0} tool_call:{}\n", rest);
            }
            return format!("\u{2699}  tool_call:{}\n", rest);
        }
        if let Some(rest) = trimmed.strip_prefix("think: ") {
            return format!("\u{1F9E0}  \x1b[3m{}\x1b[0m\n", rest);
        }
        if trimmed.starts_with("accepted:") {
            return format!("\u{2705}  {}\n", trimmed);
        }
        if trimmed.starts_with("denied:") {
            return format!("\u{274C}  {}\n", trimmed);
        }

        format!("{line}")
    }
}

impl Write for IconWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        self.buffer.push_str(&s);

        while let Some(pos) = self.buffer.find('\n') {
            let line: String = self.buffer[..=pos].to_string();
            self.buffer = self.buffer[pos + 1..].to_string();
            let formatted = Self::format_line(&line);
            self.inner.write_all(formatted.as_bytes())?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buffer.is_empty() {
            let formatted = Self::format_line(&self.buffer);
            self.inner.write_all(formatted.as_bytes())?;
            self.buffer.clear();
        }
        self.inner.flush()
    }
}

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

    let model_name = config.model.clone();
    let max_tokens = cli.max_tokens.or(config.max_tokens);
    let max_turns = cli.max_turns;

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

    let prompt_text = if let Some(text) = cli.prompt_text() {
        Some(text)
    } else {
        io::read_stdin()
    };

    if let Some(text) = prompt_text.as_ref() {
        debug!("Prompt: {text}");
    }

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
            let agent = build_agent(model, &system_prompt, &policy, max_tokens, max_turns, mcp_tool_sets);

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
            let agent = build_agent(model, &system_prompt, &policy, max_tokens, max_turns, mcp_tool_sets);

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
    let filter = if verbose {
        EnvFilter::new("ai=debug,info")
    } else if quiet {
        EnvFilter::new("ai=warn,error")
    } else {
        EnvFilter::new("ai=info,warn")
    };

    tracing_subscriber::fmt()
        .with_writer(|| IconWriter::new())
        .with_target(false)
        .with_level(false)
        .with_line_number(false)
        .with_file(false)
        .without_time()
        .with_ansi(true)
        .with_env_filter(filter)
        .init();
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

fn build_agent<M: CompletionModel + 'static>(
    model: M,
    system_prompt: &str,
    policy: &Policy,
    max_tokens: Option<usize>,
    max_turns: usize,
    mcp_tool_sets: Vec<mcp::McpToolSet>,
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

    let builder = AgentBuilder::new(model)
        .preamble(system_prompt)
        .default_max_turns(max_turns)
        .tool_server_handle(handle);

    match max_tokens {
        Some(tokens) => builder.max_tokens(tokens as u64).build(),
        None => builder.build(),
    }
}

async fn run_oneshot<M: CompletionModel + 'static>(
    agent: rig_core::agent::Agent<M>,
    prompt: &str,
) -> anyhow::Result<()> {
    debug!("Sending prompt...");
    let response = agent.prompt(prompt).await?;
    debug!("Response: {response}");
    io::stdout_line(&response);
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
        match agent.chat(&text, &mut chat_history).await {
            Ok(response) => {
                session.add_message("user", &text);
                session.add_message("assistant", &response);
                debug!("Response: {response}");
                io::stdout_line(&response);
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

                match agent.chat(trimmed, &mut chat_history).await {
                    Ok(response) => {
                        session.add_message("assistant", &response);
                        debug!("Response: {response}");
                        io::stdout_line(&response);
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
