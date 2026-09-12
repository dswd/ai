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
                if !session::is_safe_name(name) {
                    anyhow::bail!("invalid session name: {name:?}");
                }
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
            Err(e) => {
                let path = session_dir.join(format!("{name}.json"));
                if path.exists() {
                    output::stderr_line(&format!(
                        "warning: failed to load session '{name}': {e}; starting a new session"
                    ));
                }
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

/// Provider resolved for a run: a static built-in when the name is known, or a
/// config-described endpoint (flavor + api_base) for any models.dev provider.
pub(crate) struct ResolvedProvider {
    pub name: String,
    pub flavor: providers::Flavor,
    pub base_url: String,
    /// Environment variable the API key may fall back to; only known for
    /// built-in providers (custom configs store the key or `env:VAR` instead).
    pub env_var: Option<&'static str>,
    pub supports_thinking: bool,
    pub supports_tools: bool,
    pub context_window: Option<usize>,
}

pub(crate) fn resolve_provider(config: &Config) -> anyhow::Result<ResolvedProvider> {
    let provider = config.provider.to_lowercase();
    if let Some(spec) = providers::resolve(&provider) {
        let base_url = config
            .api_base
            .clone()
            .or_else(|| spec.default_base_url.map(str::to_string))
            .ok_or_else(|| {
                anyhow::anyhow!("provider '{provider}' requires an api_base in config")
            })?;
        let flavor = config.flavor.map(flavor_from_config).unwrap_or(spec.flavor);
        return Ok(ResolvedProvider {
            name: spec.name.to_string(),
            flavor,
            base_url,
            env_var: Some(spec.env_var),
            supports_thinking: flavor == providers::Flavor::Anthropic,
            supports_tools: spec.supports_tools(),
            context_window: config.context_window.or(Some(spec.context_window)),
        });
    }

    let flavor = config.flavor.map(flavor_from_config).ok_or_else(|| {
        anyhow::anyhow!(
            "provider '{provider}' is not built in and has no `flavor` in config \
             (expected `openai` or `anthropic`)"
        )
    })?;
    let base_url = config.api_base.clone().ok_or_else(|| {
        anyhow::anyhow!("provider '{provider}' is not built in and requires an api_base in config")
    })?;
    Ok(ResolvedProvider {
        name: provider,
        flavor,
        base_url,
        env_var: None,
        supports_thinking: flavor == providers::Flavor::Anthropic,
        supports_tools: true,
        context_window: config.context_window,
    })
}

fn flavor_from_config(flavor: crate::config::ProviderFlavor) -> providers::Flavor {
    match flavor {
        crate::config::ProviderFlavor::OpenAi => providers::Flavor::OpenAi,
        crate::config::ProviderFlavor::Anthropic => providers::Flavor::Anthropic,
    }
}

/// Drop a requested thinking budget for providers that do not accept it, so we
/// never inject an unsupported parameter into the request.
pub(crate) fn resolve_thinking(
    requested: Option<usize>,
    provider: &ResolvedProvider,
) -> Option<usize> {
    if requested.is_some() && !provider.supports_thinking {
        log::warn!(
            "--thinking is not supported by provider '{}'; ignoring",
            provider.name
        );
        return None;
    }
    requested
}

/// Resolve the container isolation for external commands: image from
/// `--container` or `container.default_image`, runtime auto-detected, and bind
/// mounts derived from the policy. Returns `None` for host execution.
pub(crate) fn resolve_container(
    cli: &Cli,
    config: &Config,
    policy: &Policy,
) -> anyhow::Result<Option<std::sync::Arc<crate::container::ContainerRuntime>>> {
    if cli.no_container {
        return Ok(None);
    }
    let image = cli
        .container
        .clone()
        .or_else(|| config.container.default_image.clone());
    let Some(image) = image else {
        return Ok(None);
    };

    let preferred = cli
        .container_runtime
        .clone()
        .or_else(|| config.container.runtime.clone());
    let runtime = crate::container::detect_runtime(preferred.as_deref()).ok_or_else(|| {
        let requested = preferred.unwrap_or_else(|| "auto".to_string());
        anyhow::anyhow!(
            "no container runtime found (requested: {requested}); install docker or podman, \
             or pass --no-container to run on the host"
        )
    })?;

    let (mounts, warnings) = crate::container::mounts_from_policy(policy);
    for warning in warnings {
        crate::output::stderr_line(&format!("warning: {warning}"));
    }

    let network = match config
        .container
        .network
        .as_deref()
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("none") => crate::container::Network::None,
        Some("host") => crate::container::Network::Host,
        Some("policy") | None | Some("") => {
            let web =
                policy.has_any_allow(&Action::WebFetch) || policy.has_any_allow(&Action::WebSearch);
            if web {
                crate::container::Network::Default
            } else {
                crate::container::Network::None
            }
        }
        Some(other) => {
            anyhow::bail!("unknown container.network '{other}' (expected policy, none, or host)")
        }
    };

    Ok(Some(std::sync::Arc::new(
        crate::container::ContainerRuntime {
            image,
            runtime,
            network,
            mounts,
        },
    )))
}

pub(crate) fn load_config(cli: &Cli, vanilla: bool) -> anyhow::Result<Config> {
    if let Some(path) = &cli.config {
        let path = crate::util::expand_tilde(&path.to_string_lossy());
        Config::from_file(&path)
    } else if let Some(default_path) = Config::default_path() {
        if default_path.exists() {
            Config::from_file(&default_path)
        } else {
            if vanilla {
                output::stderr_line("No config found. Run `ai --setup` to create one.");
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
    if let Some(ref image) = cli.container {
        config.container.default_image = Some(image.clone());
    }
    if let Some(ref runtime) = cli.container_runtime {
        config.container.runtime = Some(runtime.clone());
    }
}

pub(crate) fn load_policy(cli: &Cli, config: &Config) -> anyhow::Result<Policy> {
    let mut policy = if let Some(path) = &cli.policy {
        let path = crate::util::expand_tilde(&path.to_string_lossy());
        Policy::from_file(&path)?
    } else if let Some(path) = &config.policy {
        let path = crate::util::expand_tilde(&path.to_string_lossy());
        if path.exists() {
            Policy::from_file(&path)?
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

    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderFlavor;

    #[test]
    fn test_resolve_known_provider_uses_static_defaults() {
        let config = Config {
            provider: "openai".to_string(),
            ..Config::default()
        };
        let resolved = resolve_provider(&config).unwrap();
        assert_eq!(resolved.flavor, providers::Flavor::OpenAi);
        assert_eq!(resolved.base_url, "https://api.openai.com/v1");
        assert_eq!(resolved.env_var, Some("OPENAI_API_KEY"));
        assert_eq!(resolved.context_window, Some(128_000));
    }

    #[test]
    fn test_resolve_known_provider_flavor_override() {
        let config = Config {
            provider: "openai-compatible".to_string(),
            api_base: Some("https://example.com/v1".to_string()),
            flavor: Some(ProviderFlavor::Anthropic),
            ..Config::default()
        };
        let resolved = resolve_provider(&config).unwrap();
        assert_eq!(resolved.flavor, providers::Flavor::Anthropic);
        assert!(resolved.supports_thinking);
    }

    #[test]
    fn test_resolve_dynamic_provider_requires_flavor_and_base() {
        let missing = Config {
            provider: "some-new-provider".to_string(),
            ..Config::default()
        };
        assert!(resolve_provider(&missing).is_err());

        let config = Config {
            provider: "some-new-provider".to_string(),
            api_base: Some("https://api.example.com/v1".to_string()),
            flavor: Some(ProviderFlavor::OpenAi),
            context_window: Some(64_000),
            ..Config::default()
        };
        let resolved = resolve_provider(&config).unwrap();
        assert_eq!(resolved.flavor, providers::Flavor::OpenAi);
        assert_eq!(resolved.base_url, "https://api.example.com/v1");
        assert_eq!(resolved.env_var, None);
        assert_eq!(resolved.context_window, Some(64_000));
    }
}
