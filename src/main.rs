mod agent;
mod cli;
mod clients;
mod commands;
mod config;
mod context;
mod format;
mod init;
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
mod skills;
mod tool;
mod tools;
mod util;

use agent::{AgentContext, run_agent};
use clap::Parser;
use cli::Cli;
use clients::{anthropic_client, openai_client};
use commands::{cmd_delete_session, cmd_list_sessions, cmd_probe_web};
use logging::setup_logging;
use prompt::assemble_system_prompt;
use rig_core::client::CompletionClient;
use setup::{
    apply_cli_overrides, load_config, load_policy, resolve_prompt_text, resolve_provider,
    resolve_session, resolve_thinking,
};
use std::sync::Arc;

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
    if let Some(ref query) = cli.probe_web {
        return cmd_probe_web(query, &config).await;
    }

    let policy = load_policy(&cli, &config)?;
    let skills = Arc::new(skills::discover(&cli.skill, &config.skills_dir_resolved()));
    let (system_prompt, memory) = assemble_system_prompt(&cli, &config, &policy, &skills)?;
    log::debug!("system prompt:\n{system_prompt}");

    let model_name = config.model.clone();
    let max_tokens = cli.max_tokens.or(config.max_tokens);
    let max_turns = cli.max_turns;

    let (provider_spec, base_url) = resolve_provider(&config)?;
    let mut session = resolve_session(
        &cli,
        &session_dir,
        &system_prompt,
        &model_name,
        provider_spec.name,
    )?;
    if let Some(ref mem) = memory {
        mem.set_session_name(&session.name);
    }
    let prompt_text = resolve_prompt_text(&cli).await;
    let thinking = resolve_thinking(cli.thinking.or(config.thinking), provider_spec);

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
        supports_tools: provider_spec.supports_tools(),
        session: &mut session,
        session_dir: &session_dir,
        prompt_text,
        context_window: config.context_window,
    };

    match provider_spec.flavor {
        providers::Flavor::OpenAi => {
            run_agent(
                openai_client(&config, &base_url, &session_id)?.completion_model(&model_name),
                ctx,
            )
            .await?
        }
        providers::Flavor::Anthropic => {
            run_agent(
                anthropic_client(&config, &base_url, &session_id)?.completion_model(&model_name),
                ctx,
            )
            .await?
        }
    }

    Ok(())
}
