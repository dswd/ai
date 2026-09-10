use crate::cli::Cli;
use crate::config::Config;
use crate::io;
use crate::output;
use crate::policy::{self, Action, Policy, PolicyRule};
use crate::providers;
use crate::session::{self, Role, Session};
use log::info;

pub(crate) fn resolve_session(
    cli: &Cli,
    session_dir: &std::path::Path,
    system_prompt: &str,
    model_name: &str,
    provider_name: &str,
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
                if s.provider != provider_name || s.model != model_name {
                    // A conversation is only meaningful under the model that
                    // produced it; alias the old log aside and start fresh.
                    let forked = session::generate_session_name();
                    let prev_provider = if s.provider.is_empty() {
                        "unknown"
                    } else {
                        &s.provider
                    };
                    output::stderr_line(&format!(
                        "Session '{name}' used {prev_provider}/{}; forking to '{forked}' for {provider_name}/{model_name}.",
                        s.model
                    ));
                    let mut new = Session::new(
                        forked,
                        system_prompt.to_string(),
                        model_name.to_string(),
                        provider_name.to_string(),
                    );
                    new.forked_from = Some(s.name);
                    new
                } else {
                    if s.partial {
                        output::stderr_line(&format!(
                            "Continuing legacy session '{name}': tool history is unavailable."
                        ));
                    }
                    if s.system_prompt != system_prompt {
                        output::stderr_line(
                            "[note] the system prompt has changed since this session was saved",
                        );
                    }
                    info!("Continuing session: {name}");
                    s
                }
            }
            Err(_) => {
                let s = Session::new(
                    name.clone(),
                    system_prompt.to_string(),
                    model_name.to_string(),
                    provider_name.to_string(),
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
            provider_name.to_string(),
        )
    };

    if cli.is_interactive() {
        let user_lines: Vec<String> = session
            .transcript()
            .into_iter()
            .filter(|(role, _)| *role == Role::User)
            .map(|(_, content)| content)
            .collect();
        io::load_session_history(&user_lines);
    }

    Ok(session)
}

pub(crate) async fn resolve_prompt_text(cli: &Cli) -> Option<String> {
    let cli_prompt = cli.prompt_text();
    let stdin_prompt = io::read_stdin_async().await;
    match (cli_prompt, stdin_prompt) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

pub(crate) fn resolve_provider(
    config: &Config,
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

    Ok((provider_spec, base_url))
}

/// Drop a requested thinking budget for providers that do not accept it, so we
/// never inject an unsupported parameter into the request.
pub(crate) fn resolve_thinking(
    requested: Option<usize>,
    provider: &providers::Provider,
) -> Option<usize> {
    if requested.is_some() && !provider.supports_thinking() {
        log::warn!(
            "--thinking is not supported by provider '{}'; ignoring",
            provider.name
        );
        return None;
    }
    requested
}

/// Resolve the exec-sandbox spec from the policy, honoring `--sandbox`/config and
/// platform support. Returns `None` when the sandbox is disabled.
pub(crate) fn resolve_sandbox(
    cli: &Cli,
    config: &Config,
    policy: &Policy,
) -> anyhow::Result<Option<std::sync::Arc<crate::exec_sandbox::SandboxSpec>>> {
    if cli.yolo {
        // `--yolo` is documented as full access; keep it unsandboxed.
        return Ok(None);
    }
    let mode = cli
        .sandbox
        .clone()
        .unwrap_or_else(|| config.sandbox.mode.clone());
    let enabled = match mode.to_lowercase().as_str() {
        "off" | "never" => false,
        "on" | "always" => {
            if crate::exec_sandbox::available() {
                true
            } else {
                anyhow::bail!(
                    "--sandbox=on was requested but Landlock is not available on this system"
                );
            }
        }
        "auto" | "" => {
            if crate::exec_sandbox::available() {
                true
            } else {
                crate::output::stderr_line(
                    "warning: exec sandbox is not available on this platform; external commands run unsandboxed",
                );
                false
            }
        }
        other => anyhow::bail!("unknown sandbox mode '{other}' (expected auto, on, or off)"),
    };

    if !enabled {
        return Ok(None);
    }
    let (spec, warnings) = crate::exec_sandbox::spec_from_policy(policy, &config.sandbox);
    for warning in warnings {
        crate::output::stderr_line(&format!("warning: {warning}"));
    }
    Ok(Some(std::sync::Arc::new(spec)))
}

pub(crate) fn load_config(cli: &Cli, vanilla: bool) -> anyhow::Result<Config> {
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

pub(crate) fn apply_cli_overrides(cli: &Cli, config: &mut Config) {
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
    if let Some(ref proxy) = cli.proxy {
        config.proxy = Some(proxy.clone());
    }
    if let Some(ref mode) = cli.sandbox {
        config.sandbox.mode = mode.clone();
    }
}

pub(crate) fn load_policy(cli: &Cli, config: &Config) -> anyhow::Result<Policy> {
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

    if policy.ask {
        policy.approval = Some(std::sync::Arc::new(policy::ApprovalState::new()));
    }

    for warning in policy.broad_grant_warnings(cli.yolo) {
        crate::output::stderr_line(&format!("warning: {warning}"));
    }

    Ok(policy)
}
