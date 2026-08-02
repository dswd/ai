use std::path::PathBuf;

use crate::config::{Config, SearchConfig};
use crate::providers::{PROVIDERS, resolve};
use dialoguer::{Input, Password, Select, theme::ColorfulTheme};

pub fn run(target_path: Option<String>) -> anyhow::Result<()> {
    let theme = ColorfulTheme::default();

    println!();
    println!("  AI Config Wizard");
    println!();

    // Step 1: Provider
    let names: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
    let provider_idx = Select::with_theme(&theme)
        .with_prompt("Provider")
        .items(&names)
        .default(0)
        .interact()?;
    let provider_spec = resolve(names[provider_idx]).expect("provider list derived from table");
    let provider = provider_spec.name.to_string();

    // Step 2: API key
    let env_var = provider_spec.env_var;
    let detected = std::env::var(env_var).ok();
    let api_key = Password::with_theme(&theme)
        .with_prompt(format!("API key ({env_var})"))
        .allow_empty_password(true)
        .with_confirmation("Confirm API key", "Keys do not match")
        .interact()?;
    let api_key = if api_key.is_empty() {
        detected.map(|_| format!("env:{env_var}"))
    } else {
        Some(api_key)
    };

    // Step 3: API base URL (optional for named providers, required for generic)
    let api_base = match provider_spec.default_base_url {
        Some(default) => {
            let default_str = default.to_string();
            let base: String = Input::with_theme(&theme)
                .with_prompt(format!("API base URL (default: {default_str})"))
                .allow_empty(true)
                .default(default_str.clone())
                .interact_text()?;
            Some(base)
        }
        None => {
            let base: String = loop {
                let input: String = Input::with_theme(&theme)
                    .with_prompt(format!(
                        "API base URL (required for {provider}, e.g. https://example.com/v1)"
                    ))
                    .allow_empty(true)
                    .interact_text()?;
                let trimmed = input.trim();
                if trimmed.is_empty() {
                    println!("  API base URL is required for provider '{provider}'.");
                    continue;
                }
                break trimmed.to_string();
            };
            Some(base)
        }
    };

    // Step 4: Model
    let model = if provider_spec.models.is_empty() {
        Input::with_theme(&theme)
            .with_prompt("Model name")
            .interact_text()?
    } else {
        let mut models: Vec<String> = provider_spec.models.iter().map(|m| m.to_string()).collect();
        models.push("Other (type manually)".into());
        let model_idx = Select::with_theme(&theme)
            .with_prompt("Model")
            .items(&models)
            .default(0)
            .interact()?;
        if model_idx == models.len() - 1 {
            Input::with_theme(&theme)
                .with_prompt("Model name")
                .interact_text()?
        } else {
            models[model_idx].clone()
        }
    };

    // Step 5: Context window
    let default_cw = provider_spec.context_window;
    let cw_input: String = Input::with_theme(&theme)
        .with_prompt("Context window (tokens, 0 to skip)")
        .default(default_cw.to_string())
        .interact_text()?;
    let context_window = cw_input.parse::<usize>().ok().filter(|&n| n > 0);

    // Summary
    println!();
    println!("  ─────────────────────────────");
    println!("  Provider:      {provider}");
    if let Some(ref key) = api_key {
        let display = if key.starts_with("env:") {
            key.to_string()
        } else {
            "****".to_string()
        };
        println!("  API key:       {display}");
    } else {
        println!("  API key:       (none)");
    }
    if let Some(ref url) = api_base {
        println!("  API base URL:  {url}");
    }
    println!("  Model:         {model}");
    if let Some(cw) = context_window {
        println!("  Context window: {cw}");
    }
    println!("  ─────────────────────────────");
    println!();

    let confirm = dialoguer::Confirm::with_theme(&theme)
        .with_prompt("Save this config?")
        .default(true)
        .interact()?;

    if !confirm {
        println!("Aborted.");
        return Ok(());
    }

    // Build config
    let config = Config {
        provider,
        api_key,
        api_base,
        model,
        system_prompt: None,
        max_tokens: None,
        thinking: None,
        session_dir: None,
        skills_dir: None,
        policy: None,
        memory: None,
        context_window,
        search: SearchConfig::default(),
    };

    let path = if let Some(ref p) = target_path {
        if p.is_empty() {
            Config::default_path().unwrap_or_else(|| PathBuf::from("config.yaml"))
        } else {
            PathBuf::from(p)
        }
    } else {
        Config::default_path().unwrap_or_else(|| PathBuf::from("config.yaml"))
    };

    config.save(&path)?;
    println!("  Saved to {}", path.display());
    Ok(())
}
