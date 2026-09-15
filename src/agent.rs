use crate::config::SearchConfig;
use crate::context::ContextPruneHook;
use crate::interactive::run_interactive;
use crate::logging::is_quiet;
use crate::memory;
use crate::output;
use crate::policy::Policy;
use crate::prompt::permissions_available;
use crate::session::Session;
use crate::skills;
use crate::tool;
use crate::tools;
use ansi_color_constants::*;
use futures::StreamExt;
use rig::{
    agent::AgentBuilder,
    agent::{MultiTurnStreamItem, PromptResponse, StreamingResult},
    completion::CompletionModel,
    streaming::{StreamedAssistantContent, StreamingChat, StreamingPrompt},
    tool::server::ToolServer,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

pub(crate) struct AgentContext<'a> {
    pub(crate) system_prompt: &'a str,
    pub(crate) policy: &'a Policy,
    pub(crate) max_tokens: Option<usize>,
    pub(crate) max_turns: usize,
    pub(crate) tool_sets: Vec<tool::ToolSet>,
    pub(crate) thinking: Option<usize>,
    pub(crate) memory: Option<Arc<memory::Memory>>,
    pub(crate) search: &'a SearchConfig,
    pub(crate) proxy: Option<&'a str>,
    pub(crate) skills: Arc<Vec<skills::Skill>>,
    #[cfg(feature = "browser")]
    pub(crate) browser_state: Option<Arc<tools::BrowserState>>,
    #[cfg(not(feature = "browser"))]
    pub(crate) _browser_state: Option<Arc<()>>,
    pub(crate) is_interactive: bool,
    pub(crate) supports_tools: bool,
    pub(crate) container_session: Option<Arc<crate::container::ContainerSession>>,
    pub(crate) session: &'a mut Session,
    pub(crate) session_dir: &'a std::path::Path,
    pub(crate) prompt_text: Option<String>,
    pub(crate) context_window: Option<usize>,
    /// Do not persist a session: interactive setup, and `--no-session` one-offs.
    pub(crate) transient: bool,
    /// When set, the interactive `exit_program` tool is exposed and drives the loop.
    pub(crate) exit_flag: Option<Arc<AtomicBool>>,
    /// Setup-only config target; when set, the `write_config` tool is exposed.
    pub(crate) setup_target: Option<Arc<tools::SetupTarget>>,
}

pub(crate) async fn run_agent<M: CompletionModel + 'static>(
    model: M,
    ctx: AgentContext<'_>,
) -> anyhow::Result<()> {
    let agent = build_agent(model, &ctx);
    dispatch_agent(agent, ctx).await
}

async fn dispatch_agent(agent: rig::agent::Agent, ctx: AgentContext<'_>) -> anyhow::Result<()> {
    if ctx.is_interactive {
        run_interactive(
            agent,
            ctx.session,
            ctx.session_dir,
            ctx.prompt_text,
            ctx.context_window,
            ctx.memory.as_ref().map(Arc::clone),
            ctx.transient,
            ctx.exit_flag,
            ctx.container_session.clone(),
        )
        .await?;
    } else if let Some(text) = ctx.prompt_text {
        run_oneshot(
            agent,
            &text,
            ctx.memory.as_ref().map(Arc::clone),
            ctx.session,
            ctx.session_dir,
            ctx.transient,
        )
        .await?;
    } else {
        anyhow::bail!("No prompt provided. Pass a prompt argument or pipe text to stdin.");
    }
    Ok(())
}

fn build_agent<M: CompletionModel + 'static>(
    model: M,
    ctx: &AgentContext<'_>,
) -> rig::agent::Agent {
    let p = permissions_available(ctx.policy);
    let (can_read, can_write, can_web_fetch, can_web_search) =
        (p.read, p.write, p.web_fetch, p.web_search);

    let mut server = ToolServer::new();

    if ctx.supports_tools {
        if can_read {
            server = server
                .tool(tools::ReadFileTool::new(ctx.policy.clone()))
                .tool(tools::ListDirTool::new(ctx.policy.clone()))
                .tool(tools::SearchContentTool::new(ctx.policy.clone()))
                .tool(tools::FindFilesTool::new(ctx.policy.clone()))
                .tool(tools::FileInfoTool::new(ctx.policy.clone()))
                .tool(tools::FileViewTool::new(ctx.policy.clone()));
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

        if let Some(target) = &ctx.setup_target {
            server = server.tool(tools::WriteConfigTool::new(
                ctx.policy.clone(),
                Arc::clone(target),
            ));
        }

        server = server
            .tool(tools::ExecuteTool::new(
                ctx.policy.clone(),
                ctx.container_session.as_ref().map(Arc::clone),
            ))
            .tool(tools::GetCurrentTimeTool::new());

        if let Some(flag) = &ctx.exit_flag {
            server = server.tool(tools::ExitProgramTool::new(Arc::clone(flag)));
        }

        if can_web_fetch {
            #[cfg(feature = "browser")]
            let web_fetch_tool = tools::WebFetchTool::with_browser(
                ctx.policy.clone(),
                ctx.proxy.map(str::to_string),
                ctx.browser_state.as_ref().map(Arc::clone),
            );
            #[cfg(not(feature = "browser"))]
            let web_fetch_tool =
                tools::WebFetchTool::new(ctx.policy.clone(), ctx.proxy.map(str::to_string));
            server = server.tool(web_fetch_tool);
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
            server = server.tool(tools::DownloadFileTool::new(
                ctx.policy.clone(),
                ctx.proxy.map(str::to_string),
            ));
        }

        if can_web_search {
            #[cfg(feature = "browser")]
            let web_search_tool = tools::WebSearchTool::with_browser(
                ctx.policy.clone(),
                ctx.search.clone(),
                ctx.proxy.map(str::to_string),
                ctx.browser_state.as_ref().map(Arc::clone),
            );
            #[cfg(not(feature = "browser"))]
            let web_search_tool = tools::WebSearchTool::new(
                ctx.policy.clone(),
                ctx.search.clone(),
                ctx.proxy.map(str::to_string),
            );
            server = server.tool(web_search_tool);
        }

        if let Some(ref mem) = ctx.memory {
            server = server
                .tool(tools::MemoryAddTool::new(Arc::clone(mem)))
                .tool(tools::MemorySearchTool::new(Arc::clone(mem)))
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

pub(crate) async fn stream_response(
    stream: &mut StreamingResult,
) -> anyhow::Result<PromptResponse> {
    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text))) => {
                output::stdout_push(&text.text);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning {
                reasoning,
                ..
            })) if !is_quiet() => {
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

async fn run_oneshot(
    agent: rig::agent::Agent,
    prompt: &str,
    memory: Option<Arc<memory::Memory>>,
    session: &mut Session,
    session_dir: &std::path::Path,
    transient: bool,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let augmented = crate::interactive::augment_prompt(prompt, memory.as_deref());
    let sent = augmented.as_deref().unwrap_or(prompt);

    if transient {
        let spinner = output::Spinner::start("waiting for model…");
        let mut stream = agent
            .stream_prompt(sent)
            .add_hook(ContextPruneHook::default())
            .await;
        drop(spinner);
        let response = stream_response(&mut stream).await?;
        crate::interactive::print_usage(&response.usage, start.elapsed());
        return Ok(());
    }

    // Continue the session: replay its full history, then persist the turn.
    let mut chat_history = session.chat_history();
    session.add_user(prompt);
    let spinner = output::Spinner::start("waiting for model…");
    let mut stream = agent
        .stream_chat(sent, chat_history.clone())
        .add_hook(ContextPruneHook::default())
        .await;
    drop(spinner);
    let response = stream_response(&mut stream).await?;
    let usage = response.usage;
    crate::interactive::record_turn(session, &mut chat_history, response);
    session.save(session_dir)?;
    crate::interactive::print_usage(&usage, start.elapsed());
    Ok(())
}
