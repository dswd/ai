use crate::agent::{AgentContext, run_agent};
use crate::catalog::{self, Catalog, CatalogProvider};
use crate::clients::{anthropic_client, openai_client};
use crate::config::{Config, ProviderFlavor};
use crate::output;
use crate::policy::Policy;
use crate::providers::{self, Flavor};
use crate::session::Session;
use crate::setup::resolve_provider;
use crate::tools::SetupTarget;
use dialoguer::{Confirm, FuzzySelect, Input, Password, Select, theme::ColorfulTheme};
use rig_core::client::CompletionClient;
use rig_core::completion::{CompletionModel, Message};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CURATED: &[&str] = &[
    "openai",
    "anthropic",
    "google",
    "groq",
    "mistral",
    "deepseek",
    "xai",
    "openrouter",
    "ollama",
];
const SEARCH_ALL: &str = "Search all providers…";
const CUSTOM_URL: &str = "Custom URL…";

/// How many times a failed connection test may send the user back to phase 1.
const MAX_CONNECTION_ATTEMPTS: usize = 5;

/// Phase-2 failure: either the phase-1 settings are wrong (fall back) or the
/// error is unrelated (propagate).
enum Phase2Error {
    Unavailable(String),
    Fatal(anyhow::Error),
}

pub(crate) async fn run(target: String) -> anyhow::Result<()> {
    let path = resolve_path(&target);
    let theme = ColorfulTheme::default();

    println!();
    println!("  AI Setup");
    println!();

    let mut config = pick_config(&theme, &path, true).await?;
    save_with_backup(&path, &config)?;
    println!("  Saved to {}", path.display());
    println!();

    for attempt in 1..=MAX_CONNECTION_ATTEMPTS {
        match run_ai_setup(&path, &config).await {
            Ok(()) => break,
            Err(Phase2Error::Fatal(e)) => return Err(e),
            Err(Phase2Error::Unavailable(message)) => {
                println!();
                println!("  Could not reach the model with the current settings:");
                println!("    {message}");
                println!("  Let's update the connection.");
                if attempt == MAX_CONNECTION_ATTEMPTS {
                    anyhow::bail!("could not establish a working model connection");
                }
                config = pick_config(&theme, &path, false).await?;
                config.save(&path)?;
                println!("  Saved to {}", path.display());
            }
        }
    }

    match Config::from_file_strict(&path) {
        Ok(_) => {
            println!();
            println!("  Setup complete: {}", path.display());
        }
        Err(e) => {
            println!();
            println!("  The saved config is not valid: {e}");
            offer_restore(&theme, &path)?;
        }
    }
    Ok(())
}

/// Phase 1: reuse the existing config when offered and confirmed, otherwise run
/// the provider/model wizard.
async fn pick_config(
    theme: &ColorfulTheme,
    path: &Path,
    allow_reuse: bool,
) -> anyhow::Result<Config> {
    if allow_reuse
        && let Some(existing) = Config::from_file(path).ok()
        && confirm_reuse(theme, path, &existing)?
    {
        return Ok(existing);
    }
    let catalog = Catalog::load().await;
    wizard(theme, &catalog).await
}

fn resolve_path(target: &str) -> PathBuf {
    if target.is_empty() {
        Config::default_path().unwrap_or_else(|| PathBuf::from("config.yaml"))
    } else {
        crate::util::expand_tilde(target)
    }
}

fn confirm_reuse(theme: &ColorfulTheme, path: &Path, config: &Config) -> anyhow::Result<bool> {
    println!("  Found existing config: {}", path.display());
    println!("    Provider: {}", config.provider);
    println!("    Model:    {}", config.model);
    println!("    API key:  {}", key_source(config));
    println!();
    Ok(Confirm::with_theme(theme)
        .with_prompt("Use this configuration?")
        .default(true)
        .interact()?)
}

fn key_source(config: &Config) -> String {
    match config.api_key.as_deref() {
        Some(key) if key.starts_with("env:") => {
            format!("from environment ({})", key.trim_start_matches("env:"))
        }
        Some(_) => "stored in config".to_string(),
        None => "not set (will fall back to the provider environment variable)".to_string(),
    }
}

fn save_with_backup(path: &Path, config: &Config) -> anyhow::Result<()> {
    if path.exists() {
        let backup = backup_path(path);
        std::fs::copy(path, &backup)?;
        println!("  Backed up previous config to {}", backup.display());
    }
    config.save(path)
}

fn backup_path(path: &Path) -> PathBuf {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => format!("{name}.bak"),
        None => "config.yaml.bak".to_string(),
    };
    path.with_file_name(name)
}

struct Selection<'a> {
    provider_id: String,
    flavor: Flavor,
    base_default: Option<String>,
    env_vars: Vec<String>,
    catalog: Option<&'a CatalogProvider>,
}

async fn wizard(theme: &ColorfulTheme, catalog: &Catalog) -> anyhow::Result<Config> {
    let (mut items, mut ids) = curated_items(catalog);
    items.push(SEARCH_ALL.to_string());
    ids.push(None);
    items.push(CUSTOM_URL.to_string());
    ids.push(None);

    let default_idx = detected_default(catalog, &ids);
    let mut select = Select::with_theme(theme)
        .with_prompt("Provider")
        .items(&items);
    if let Some(idx) = default_idx {
        select = select.default(idx);
    }
    let idx = select.interact()?;

    let selection = if items[idx] == SEARCH_ALL {
        search_provider(theme, catalog)?
    } else if items[idx] == CUSTOM_URL {
        custom_provider(theme)?
    } else {
        let id = ids[idx].clone().expect("curated items have ids");
        selection_for(catalog, &id)
    };

    let env_var = selection
        .env_vars
        .first()
        .cloned()
        .unwrap_or_else(|| default_env(selection.flavor).to_string());
    let api_key = prompt_api_key(theme, &env_var)?;
    let api_base = prompt_api_base(theme, &selection)?;
    let (model, context_window) =
        prompt_model(theme, &selection, api_base.as_deref(), &api_key).await?;

    let provider_id = selection.provider_id.clone();
    let flavor = selection.flavor;
    let config = Config {
        provider: provider_id,
        api_key,
        api_base,
        model,
        context_window,
        flavor: Some(to_config_flavor(flavor)),
        ..Config::default()
    };
    Ok(config)
}

fn curated_items(catalog: &Catalog) -> (Vec<String>, Vec<Option<String>>) {
    let mut items = Vec::new();
    let mut ids = Vec::new();
    for id in CURATED {
        let name = catalog
            .provider(id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| id.to_string());
        items.push(format!("{name} ({id})"));
        ids.push(Some(id.to_string()));
    }
    (items, ids)
}

fn detected_default(catalog: &Catalog, ids: &[Option<String>]) -> Option<usize> {
    ids.iter().position(|id| {
        let Some(id) = id else { return false };
        provider_env_vars(catalog, id)
            .iter()
            .any(|v| std::env::var(v).is_ok())
    })
}

fn provider_env_vars(catalog: &Catalog, id: &str) -> Vec<String> {
    if let Some(p) = catalog.provider(id) {
        p.env_vars().to_vec()
    } else if let Some(spec) = providers::resolve(id) {
        vec![spec.env_var.to_string()]
    } else {
        Vec::new()
    }
}

fn selection_for<'a>(catalog: &'a Catalog, id: &str) -> Selection<'a> {
    let catalog_provider = catalog.provider(id);
    let flavor = catalog_provider
        .and_then(catalog::flavor_for)
        .or_else(|| providers::resolve(id).map(|p| p.flavor))
        .unwrap_or(Flavor::OpenAi);
    let base_default = catalog_provider.and_then(|p| p.api.clone()).or_else(|| {
        providers::resolve(id)
            .and_then(|p| p.default_base_url)
            .map(str::to_string)
    });
    let env_vars = provider_env_vars(catalog, id);
    Selection {
        provider_id: id.to_string(),
        flavor,
        base_default,
        env_vars,
        catalog: catalog_provider,
    }
}

fn search_provider<'a>(
    theme: &ColorfulTheme,
    catalog: &'a Catalog,
) -> anyhow::Result<Selection<'a>> {
    let needle: String = Input::with_theme(theme)
        .with_prompt("Search providers")
        .allow_empty(true)
        .interact_text()?;
    let mut matches = catalog.search(&needle);
    if matches.is_empty() {
        println!("  No providers matched; entering a custom URL.");
        return custom_provider(theme);
    }
    matches.truncate(40);
    let names: Vec<String> = matches
        .iter()
        .map(|p| format!("{} ({})", p.name, p.id))
        .collect();
    let idx = FuzzySelect::with_theme(theme)
        .with_prompt("Provider")
        .items(&names)
        .interact()?;
    Ok(selection_for(catalog, &matches[idx].id))
}

fn custom_provider(theme: &ColorfulTheme) -> anyhow::Result<Selection<'static>> {
    let url: String = Input::with_theme(theme)
        .with_prompt("API base URL (e.g. https://example.com/v1)")
        .interact_text()?;
    let flavor_idx = Select::with_theme(theme)
        .with_prompt("API flavor")
        .items(["OpenAI-compatible", "Anthropic-compatible"])
        .default(0)
        .interact()?;
    let flavor = if flavor_idx == 1 {
        Flavor::Anthropic
    } else {
        Flavor::OpenAi
    };
    let provider_id = match flavor {
        Flavor::Anthropic => "anthropic-compatible",
        Flavor::OpenAi => "openai-compatible",
    };
    Ok(Selection {
        provider_id: provider_id.to_string(),
        flavor,
        base_default: Some(url.trim().trim_end_matches('/').to_string()),
        env_vars: vec![default_env(flavor).to_string()],
        catalog: None,
    })
}

fn default_env(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Anthropic => "ANTHROPIC_API_KEY",
        Flavor::OpenAi => "OPENAI_API_KEY",
    }
}

fn prompt_api_key(theme: &ColorfulTheme, env_var: &str) -> anyhow::Result<Option<String>> {
    if std::env::var(env_var).is_ok() {
        let use_env = Confirm::with_theme(theme)
            .with_prompt(format!("Use {env_var} from environment?"))
            .default(true)
            .interact()?;
        if use_env {
            return Ok(Some(format!("env:{env_var}")));
        }
    }
    let key = Password::with_theme(theme)
        .with_prompt(format!("API key ({env_var})"))
        .allow_empty_password(true)
        .interact()?;
    Ok(if key.is_empty() { None } else { Some(key) })
}

fn prompt_api_base(
    theme: &ColorfulTheme,
    selection: &Selection<'_>,
) -> anyhow::Result<Option<String>> {
    let default = selection.base_default.clone().unwrap_or_default();
    loop {
        let prompt = if default.is_empty() {
            "API base URL (required)".to_string()
        } else {
            format!("API base URL (default: {default})")
        };
        let value: String = Input::with_theme(theme)
            .with_prompt(prompt)
            .allow_empty(true)
            .default(default.clone())
            .interact_text()?;
        let trimmed = value.trim().trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed));
        }
        if selection.base_default.is_some() {
            return Ok(selection.base_default.clone());
        }
        println!("  A base URL is required.");
    }
}

async fn prompt_model(
    theme: &ColorfulTheme,
    selection: &Selection<'_>,
    api_base: Option<&str>,
    api_key: &Option<String>,
) -> anyhow::Result<(String, Option<usize>)> {
    if let Some(provider) = selection.catalog {
        let models = provider.models();
        if !models.is_empty() {
            let items: Vec<String> = models
                .iter()
                .map(|m| format!("{}{}", m.id, m.summary()))
                .collect();
            return pick_model(theme, &items, |idx| {
                let model = models[idx];
                (model.id.clone(), model.context().map(|c| c as usize))
            });
        }
    }

    let live = fetch_models(api_base, api_key, selection.flavor).await;
    if !live.is_empty() {
        return pick_model(theme, &live, |idx| (live[idx].clone(), None));
    }

    let static_models: Vec<String> = providers::resolve(&selection.provider_id)
        .map(|p| p.models.iter().map(|m| m.to_string()).collect())
        .unwrap_or_default();
    if !static_models.is_empty() {
        return pick_model(theme, &static_models, |idx| {
            (static_models[idx].clone(), None)
        });
    }

    let model: String = Input::with_theme(theme)
        .with_prompt("Model name")
        .interact_text()?;
    Ok((model, None))
}

fn pick_model(
    theme: &ColorfulTheme,
    items: &[String],
    resolve_item: impl Fn(usize) -> (String, Option<usize>),
) -> anyhow::Result<(String, Option<usize>)> {
    let mut choices: Vec<String> = items.to_vec();
    choices.push("Other (type manually)".to_string());
    let idx = Select::with_theme(theme)
        .with_prompt("Model")
        .items(&choices)
        .default(0)
        .interact()?;
    if idx == choices.len() - 1 {
        let model: String = Input::with_theme(theme)
            .with_prompt("Model name")
            .interact_text()?;
        Ok((model, None))
    } else {
        Ok(resolve_item(idx))
    }
}

async fn fetch_models(base: Option<&str>, api_key: &Option<String>, flavor: Flavor) -> Vec<String> {
    let Some(base) = base else {
        return Vec::new();
    };
    let url = format!("{}/models", base.trim_end_matches('/'));
    let key = api_key.as_ref().and_then(|k| match k.strip_prefix("env:") {
        Some(var) => std::env::var(var).ok(),
        None => Some(k.clone()),
    });

    let mut request = reqwest::Client::new().get(&url);
    if let Some(key) = key {
        request = match flavor {
            Flavor::OpenAi => request.bearer_auth(key),
            Flavor::Anthropic => request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
        };
    }

    #[derive(serde::Deserialize)]
    struct ModelEntry {
        id: String,
    }
    #[derive(serde::Deserialize)]
    struct ModelList {
        data: Vec<ModelEntry>,
    }

    match request.send().await {
        Ok(response) if response.status().is_success() => response
            .json::<ModelList>()
            .await
            .map(|list| list.data.into_iter().map(|m| m.id).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn to_config_flavor(flavor: Flavor) -> ProviderFlavor {
    match flavor {
        Flavor::OpenAi => ProviderFlavor::OpenAi,
        Flavor::Anthropic => ProviderFlavor::Anthropic,
    }
}

fn setup_policy() -> Policy {
    let mut policy = Policy::default();
    policy.ask = true;
    policy.approval = Some(Arc::new(crate::policy::ApprovalState::new()));
    policy
}

/// Serialize the config for the prompt with the API key omitted, so the secret
/// never reaches the model. The `write_config` tool restores it on save.
fn config_for_prompt(config: &Config) -> String {
    let mut shown = config.clone();
    shown.api_key = None;
    serde_yaml_ng::to_string(&shown).unwrap_or_default()
}

fn setup_system_prompt(config: &Config, policy: &Policy) -> String {
    let schema = serde_json::to_string_pretty(&schemars::schema_for!(Config))
        .unwrap_or_else(|_| "{}".to_string());
    format!(
        "You are configuring the `ai` CLI agent. The user has already chosen the LLM \
         connection; `provider`, `api_key`, `api_base`, `model`, and `flavor` are preserved \
         automatically, so never try to change or reveal them.\n\n\
         Current configuration (the API key is intentionally omitted):\n\
         ```yaml\n{current}\n```\n\n\
         You cannot read or write the config file directly. To save changes, call the \
         `write_config` tool with the complete updated configuration as YAML. It checks the \
         syntax, keeps the provider/credentials/model, and asks the user to approve the save. \
         Always pass the whole file, using only the option names in the schema. You do not \
         need to know the config path.\n\n\
         ## JSON schema\n```json\n{schema}\n```\n\n{guide}\n\n\
         {policy}\n\n\
         Start by briefly telling the user what can be configured, then ask what they would \
         like to change, one topic at a time. Explain trade-offs, confirm values, and save \
         with `write_config` when they are done. Do not invent options that are absent from \
         the schema.",
        current = config_for_prompt(config),
        schema = schema,
        guide = GUIDE,
        policy = policy.summary(),
    )
}

async fn run_ai_setup(path: &Path, config: &Config) -> Result<(), Phase2Error> {
    let resolved = resolve_provider(config).map_err(|e| Phase2Error::Unavailable(e.to_string()))?;
    let policy = setup_policy();
    let system_prompt = setup_system_prompt(config, &policy);
    let session_dir = config.session_dir_resolved();
    let target = Arc::new(SetupTarget {
        path: path.to_path_buf(),
        original: config.clone(),
    });
    let mut session = Session::new(
        "setup".to_string(),
        system_prompt.clone(),
        config.model.clone(),
        resolved.name.clone(),
    );

    #[cfg(feature = "browser")]
    let browser_state: Option<Arc<crate::tools::BrowserState>> = None;
    #[cfg(not(feature = "browser"))]
    let browser_state: Option<Arc<()>> = None;

    let ctx = AgentContext {
        system_prompt: &system_prompt,
        policy: &policy,
        max_tokens: config.max_tokens,
        max_turns: 50,
        tool_sets: Vec::new(),
        thinking: None,
        memory: None,
        search: &config.search,
        proxy: config.proxy.as_deref(),
        skills: Arc::new(Vec::new()),
        #[cfg(feature = "browser")]
        browser_state,
        #[cfg(not(feature = "browser"))]
        _browser_state: browser_state,
        is_interactive: true,
        supports_tools: resolved.supports_tools,
        container_session: None,
        session: &mut session,
        session_dir: &session_dir,
        prompt_text: Some(
            "Begin the setup. Briefly tell me what I can configure, then ask what I would \
             like to change."
                .to_string(),
        ),
        context_window: resolved.context_window,
        transient: true,
        setup_target: Some(target),
    };

    match resolved.flavor {
        Flavor::OpenAi => {
            let model = openai_client(config, &resolved.base_url, "setup", resolved.env_var)
                .map_err(|e| Phase2Error::Unavailable(e.to_string()))?
                .completion_model(&config.model);
            probe_model(&model).await?;
            run_agent(model, ctx).await.map_err(Phase2Error::Fatal)
        }
        Flavor::Anthropic => {
            let model = anthropic_client(config, &resolved.base_url, "setup", resolved.env_var)
                .map_err(|e| Phase2Error::Unavailable(e.to_string()))?
                .completion_model(&config.model);
            probe_model(&model).await?;
            run_agent(model, ctx).await.map_err(Phase2Error::Fatal)
        }
    }
}

/// Phase 2 begins by proving the phase-1 connection actually works. A failure
/// is reported to the user and sends them back to the connection wizard.
async fn probe_model<M: CompletionModel>(model: &M) -> Result<(), Phase2Error> {
    let request = model
        .completion_request(Message::user("Reply with the single word OK."))
        .build();
    let spinner = output::Spinner::start("testing the connection…");
    let result = model.completion(request).await;
    drop(spinner);
    result
        .map(|_| ())
        .map_err(|e| Phase2Error::Unavailable(e.to_string()))
}

fn offer_restore(theme: &ColorfulTheme, path: &Path) -> anyhow::Result<()> {
    let backup = backup_path(path);
    if !backup.exists() {
        return Ok(());
    }
    println!("  A backup is available at {}.", backup.display());
    if Confirm::with_theme(theme)
        .with_prompt("Restore the previous config?")
        .default(true)
        .interact()?
    {
        std::fs::copy(&backup, path)?;
        println!("  Restored {}", path.display());
    }
    Ok(())
}

const GUIDE: &str = "\
## Options

- `system_prompt`: text prepended to every conversation. Sets the assistant's role and
  defaults. Leave unset to use the built-in prompt.
- `max_tokens`: hard cap on generated tokens per response. Unset means provider default.
- `thinking`: extended-thinking budget in tokens. Only used by Anthropic-flavored
  providers; ignored (with a warning) elsewhere.
- `context_window`: context size in tokens, shown as a usage indicator in interactive
  sessions. Normally derived from the model; only override for unusual setups.
- `session_dir`: directory for saved sessions. Default: the platform data dir under `ai/sessions`.
- `skills_dir`: directory scanned for skills (`SKILL.md` files). Default: platform data dir under `ai/skills`.
- `memory`: path to the persistent memory JSON file. Enables the memory tools.
- `policy`: path to a policy file with allow/deny rules (one rule per line,
  `allow read PATH`, `deny write PATH`, etc.).
- `proxy`: proxy URL for web requests, e.g. `http://127.0.0.1:8080` or
  `socks5h://127.0.0.1:1080`. Falls back to HTTP_PROXY/HTTPS_PROXY/ALL_PROXY.
- `search.searxng_url`: SearXNG instance used for web search. A bare URL gets `?q=`
  appended; `{query}` may be used as the placeholder.
- `container.default_image`: run external commands inside this image (Docker/Podman).
  Unset runs commands on the host.
- `container.runtime`: `auto` (default), `docker`, or `podman`.
- `container.network`: `policy` (default; network only when web access is granted),
  `none`, or `host`.

Prefer relative or `~` paths where a path is expected. Ask the user before enabling
container execution or broad policies.";
