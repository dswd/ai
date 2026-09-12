mod agent;
mod catalog;
mod cli;
mod clients;
mod commands;
mod config;
mod container;
mod context;
mod format;
mod interactive;
mod io;
mod logging;
mod memory;
mod output;
mod policy;
mod prompt;
mod providers;
mod sandbox;
mod session;
mod setup;
mod setup_cmd;
mod skills;
mod tool;
mod tools;
mod util;

use agent::{AgentContext, run_agent};
use clap::{CommandFactory, Parser};
use cli::Cli;
use clients::{anthropic_client, openai_client};
use commands::{cmd_delete_session, cmd_list_sessions, cmd_probe_web};
use config::Config;
use logging::setup_logging;
use policy::Policy;
use prompt::assemble_system_prompt;
use rig::client::CompletionClient;
use setup::{
    apply_cli_overrides, load_config, load_policy, resolve_prompt_text, resolve_provider,
    resolve_session, resolve_thinking,
};
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    setup_logging(cli.verbose, cli.quiet);
    output::set_no_color(cli.no_color);

    if let Some(shell) = cli.completions {
        print_completions(shell);
        return Ok(());
    }

    if let Some(ref setup_path) = cli.setup {
        let path = setup_path.clone();
        return tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(setup_cmd::run(path));
    }

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
    if let Some(ref query) = cli.probe_web {
        return tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(cmd_probe_web(query, &config));
    }

    let policy = load_policy(&cli, &config)?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli, config, session_dir, policy))
}

async fn run(cli: Cli, config: Config, session_dir: PathBuf, policy: Policy) -> anyhow::Result<()> {
    let container = setup::resolve_container(&cli, &config, &policy)?;
    let container_session = match &container {
        Some(rt) => {
            let session = Arc::new(
                container::ContainerSession::start((**rt).clone())
                    .map_err(|e| anyhow::anyhow!("{e}"))?,
            );
            spawn_signal_cleanup(Arc::clone(&session));
            Some(session)
        }
        None => None,
    };
    let skills = Arc::new(skills::discover(&cli.skill, &config.skills_dir_resolved()));
    let (system_prompt, memory) =
        assemble_system_prompt(&cli, &config, &policy, &skills, container_session.is_some())?;
    log::debug!("system prompt:\n{system_prompt}");

    let model_name = config.model.clone();
    let max_tokens = cli.max_tokens.or(config.max_tokens);
    let max_turns = cli.max_turns;

    let resolved = resolve_provider(&config)?;
    let mut session = resolve_session(
        &cli,
        &session_dir,
        &system_prompt,
        &model_name,
        &resolved.name,
    )?;
    if let Some(ref mem) = memory {
        mem.set_session_name(&session.name);
    }
    let prompt_text = resolve_prompt_text(&cli).await;
    let thinking = resolve_thinking(cli.thinking.or(config.thinking), &resolved);

    let tool_sets = if !cli.tool.is_empty() {
        tool::connect_tool_servers(&cli.tool).await?
    } else {
        Vec::new()
    };

    #[cfg(feature = "browser")]
    let browser_state: Option<Arc<tools::BrowserState>> =
        if policy.ask || policy.has_any_allow(&crate::policy::Action::WebFetch) {
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

    let session_id = session.name.clone();

    let ctx = AgentContext {
        system_prompt: &system_prompt,
        policy: &policy,
        max_tokens,
        max_turns,
        tool_sets,
        thinking,
        memory: memory.as_ref().map(Arc::clone),
        search: &config.search,
        proxy: config.proxy.as_deref(),
        skills: Arc::clone(&skills),
        #[cfg(feature = "browser")]
        browser_state,
        #[cfg(not(feature = "browser"))]
        _browser_state: browser_state,
        is_interactive: cli.is_interactive(),
        supports_tools: resolved.supports_tools,
        container_session,
        session: &mut session,
        session_dir: &session_dir,
        prompt_text,
        context_window: resolved.context_window,
        transient: false,
        setup_target: None,
    };

    match resolved.flavor {
        providers::Flavor::OpenAi => {
            run_agent(
                openai_client(&config, &resolved.base_url, &session_id, resolved.env_var)?
                    .completion_model(&model_name),
                ctx,
            )
            .await?
        }
        providers::Flavor::Anthropic => {
            run_agent(
                anthropic_client(&config, &resolved.base_url, &session_id, resolved.env_var)?
                    .completion_model(&model_name),
                ctx,
            )
            .await?
        }
    }

    Ok(())
}

fn print_completions(shell: clap_complete::Shell) {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
}

/// Remove the container if the process is interrupted (the normal path removes
/// it via `Drop`).
fn spawn_signal_cleanup(session: Arc<container::ContainerSession>) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut term = signal(SignalKind::terminate()).ok();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async {
                    match term.as_mut() {
                        Some(sig) => { sig.recv().await; }
                        None => std::future::pending::<()>().await,
                    }
                } => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        session.shutdown();
        std::process::exit(130);
    });
}
