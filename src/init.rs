use std::path::PathBuf;

use crate::config::Config;
use dialoguer::{Input, Password, Select, theme::ColorfulTheme};

const PROVIDERS: &[&str] = &[
    "openai",
    "anthropic",
    "ollama",
    "groq",
    "deepseek",
    "google",
    "mistral",
    "openrouter",
    "xai",
];

fn default_models(provider: &str) -> Vec<String> {
    match provider {
        "openai" => vec![
            "gpt-4o".into(),
            "gpt-4.1".into(),
            "gpt-4.1-mini".into(),
            "gpt-4o-mini".into(),
            "gpt-4.1-nano".into(),
        ],
        "anthropic" => vec![
            "claude-sonnet-4-20250514".into(),
            "claude-3-5-sonnet-20241022".into(),
            "claude-3-5-haiku-20241022".into(),
        ],
        "ollama" => vec![
            "llama3.2".into(),
            "qwen3".into(),
            "deepseek-r1".into(),
        ],
        "groq" => vec![
            "llama-3.3-70b-versatile".into(),
            "mixtral-8x7b-32768".into(),
        ],
        "deepseek" => vec![
            "deepseek-chat".into(),
            "deepseek-reasoner".into(),
        ],
        "google" => vec![
            "gemini-2.5-flash".into(),
            "gemini-2.5-pro".into(),
        ],
        "mistral" => vec![
            "mistral-large-latest".into(),
            "mistral-small-latest".into(),
        ],
        "openrouter" => vec![
            "openai/gpt-4o".into(),
            "openai/gpt-4.1".into(),
            "anthropic/claude-sonnet-4".into(),
        ],
        "xai" => vec![
            "grok-3".into(),
        ],
        _ => vec![],
    }
}

fn default_context_window(provider: &str) -> usize {
    match provider {
        "openai" => 128_000,
        "anthropic" => 200_000,
        "deepseek" => 128_000,
        "google" => 1_000_000,
        "xai" => 128_000,
        _ => 128_000,
    }
}

fn env_var_for(provider: &str) -> &str {
    match provider {
        "openai" => "OPENAI_API_KEY",
        "anthropic" => "ANTHROPIC_API_KEY",
        "ollama" => "OLLAMA_API_KEY",
        "groq" => "GROQ_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "google" => "GEMINI_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "xai" => "XAI_API_KEY",
        _ => "",
    }
}

pub fn run(target_path: Option<String>) -> anyhow::Result<()> {
    let theme = ColorfulTheme::default();

    println!();
    println!("  AI Config Wizard");
    println!();

    // Step 1: Provider
    let provider_idx = Select::with_theme(&theme)
        .with_prompt("Provider")
        .items(PROVIDERS)
        .default(0)
        .interact()?;
    let provider = PROVIDERS[provider_idx].to_string();

    // Step 2: API key
    let env_var = env_var_for(&provider);
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

    // Step 3: API base URL
    let api_base: String = Input::with_theme(&theme)
        .with_prompt("API base URL (optional)")
        .allow_empty(true)
        .default(String::new())
        .interact_text()?;
    let api_base = if api_base.is_empty() { None } else { Some(api_base) };

    // Step 4: Model
    let mut models = default_models(&provider);
    models.push("Other (type manually)".into());
    let model_idx = Select::with_theme(&theme)
        .with_prompt("Model")
        .items(&models)
        .default(0)
        .interact()?;
    let model = if model_idx == models.len() - 1 {
        Input::with_theme(&theme)
            .with_prompt("Model name")
            .interact_text()?
    } else {
        models[model_idx].clone()
    };

    // Step 5: Context window
    let default_cw = default_context_window(&provider);
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
        let display = if key.starts_with("env:") { key.to_string() } else { "****".to_string() };
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
        policy: None,
        memory: None,
        context_window,
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
