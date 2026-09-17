use anyhow::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Request shape the provider speaks. OpenAI-compatible endpoints (including
/// Gemini's compatibility path) use `openai`; Anthropic and its clones use
/// `anthropic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProviderFlavor {
    OpenAi,
    Anthropic,
}

/// Search backend name. Keyed APIs require an `api_key`; `searxng` requires a
/// `url`; `duckduckgo`, `google`, and `bing` are name-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchProviderName {
    Brave,
    Tavily,
    Exa,
    Serper,
    Searxng,
    #[serde(rename = "duckduckgo")]
    DuckDuckGo,
    Google,
    Bing,
}

impl SearchProviderName {
    /// Lowercase config name.
    pub fn as_str(self) -> &'static str {
        match self {
            SearchProviderName::Brave => "brave",
            SearchProviderName::Tavily => "tavily",
            SearchProviderName::Exa => "exa",
            SearchProviderName::Serper => "serper",
            SearchProviderName::Searxng => "searxng",
            SearchProviderName::DuckDuckGo => "duckduckgo",
            SearchProviderName::Google => "google",
            SearchProviderName::Bing => "bing",
        }
    }

    /// Conventional environment variable holding this provider's API key.
    pub fn env_var(self) -> Option<&'static str> {
        match self {
            SearchProviderName::Brave => Some("BRAVE_API_KEY"),
            SearchProviderName::Tavily => Some("TAVILY_API_KEY"),
            SearchProviderName::Exa => Some("EXA_API_KEY"),
            SearchProviderName::Serper => Some("SERPER_API_KEY"),
            _ => None,
        }
    }

    /// Whether this provider needs an API key to run.
    pub fn requires_key(self) -> bool {
        self.env_var().is_some()
    }
}

/// One configured search backend.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchProviderConfig {
    pub name: SearchProviderName,
    /// API key literal or `env:VAR`. When omitted, the provider's conventional
    /// environment variable is used (`BRAVE_API_KEY`, `TAVILY_API_KEY`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Base URL (SearXNG only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl SearchProviderConfig {
    /// Whether the entry has what it needs to run: a URL for SearXNG, a key
    /// for the keyed APIs, nothing for the scrapers.
    pub fn is_configured(&self) -> bool {
        let non_empty = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty());
        if self.name == SearchProviderName::Searxng {
            non_empty(&self.url)
        } else if self.name.requires_key() {
            non_empty(&self.api_key)
        } else {
            true
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SearchConfig {
    /// Ordered search backends; the first that succeeds wins. When unset, the
    /// default is DuckDuckGo, Google, then Bing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<Vec<SearchProviderConfig>>,
}

impl SearchConfig {
    /// The configured providers in order, or the default ladder when unset.
    /// `api_key` is resolved (literals kept, `env:VAR` expanded, and an omitted
    /// key falls back to the provider's conventional environment variable).
    /// Entries still missing a key or URL are skipped at call time.
    pub fn resolved_entries(&self) -> Vec<SearchProviderConfig> {
        match &self.providers {
            Some(list) => list
                .iter()
                .map(|p| SearchProviderConfig {
                    name: p.name,
                    api_key: p
                        .api_key
                        .as_deref()
                        .and_then(resolve_secret)
                        .or_else(|| p.name.env_var().and_then(|v| std::env::var(v).ok()))
                        .filter(|s| !s.is_empty()),
                    url: p.url.clone().filter(|s| !s.is_empty()),
                })
                .collect(),
            None => [
                SearchProviderName::DuckDuckGo,
                SearchProviderName::Google,
                SearchProviderName::Bing,
            ]
            .into_iter()
            .map(|name| SearchProviderConfig {
                name,
                api_key: None,
                url: None,
            })
            .collect(),
        }
    }
}

/// Resolve a config secret that may be a literal or `env:VAR`.
pub fn resolve_secret(value: &str) -> Option<String> {
    match value.strip_prefix("env:") {
        Some(env_var) => std::env::var(env_var).ok(),
        None => Some(value.to_string()),
    }
}

/// Container isolation for external commands. When `default_image` is set, all
/// external commands run inside that image; `--container`
/// overrides per run and `--no-container` forces host execution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub struct ContainerConfig {
    /// Image used for external commands, e.g. `debian:stable-slim`. Unset runs
    /// commands on the host.
    pub default_image: Option<String>,
    /// `auto` (default), `docker`, or `podman`.
    pub runtime: Option<String>,
    /// `policy` (none unless web is granted), `none`, or `host`.
    pub network: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    /// Provider id (e.g. `openai`, `anthropic`, `groq`, or any models.dev id).
    pub provider: String,
    /// API key literal, or `env:VAR` to read it from the environment.
    #[serde(alias = "api_key")]
    pub api_key: Option<String>,
    /// Override the provider's base URL. Required for custom endpoints.
    #[serde(alias = "api_base")]
    pub api_base: Option<String>,
    /// Model id sent to the provider.
    pub model: String,
    /// System prompt prepended to every conversation.
    pub system_prompt: Option<String>,
    /// Maximum tokens the model may generate per response.
    pub max_tokens: Option<usize>,
    /// Extended-thinking budget in tokens (Anthropic-flavored providers only).
    pub thinking: Option<usize>,
    /// Directory where sessions are stored.
    pub session_dir: Option<PathBuf>,
    /// Directory scanned for skills (SKILL.md files).
    pub skills_dir: Option<PathBuf>,
    /// Path to the policy file (allow/deny rules).
    pub policy: Option<PathBuf>,
    /// Path to the persistent memory SQLite database.
    pub memory: Option<PathBuf>,
    /// Number of parallel requests used by `ai dream` (default 4).
    pub dream_jobs: Option<usize>,
    /// Context window in tokens, used for the interactive usage indicator.
    pub context_window: Option<usize>,
    /// Optional proxy for web requests (HTTP, HTTPS, or SOCKS5 URL).
    /// Falls back to HTTP_PROXY/HTTPS_PROXY/ALL_PROXY environment variables.
    pub proxy: Option<String>,
    /// Request shape the provider speaks. Derived from the built-in provider
    /// table when omitted.
    pub flavor: Option<ProviderFlavor>,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub container: ContainerConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            api_key: None,
            api_base: None,
            model: "gpt-4o".to_string(),
            system_prompt: None,
            max_tokens: None,
            thinking: None,
            session_dir: None,
            skills_dir: None,
            policy: None,
            memory: None,
            dream_jobs: None,
            context_window: None,
            proxy: None,
            flavor: None,
            search: SearchConfig::default(),
            container: ContainerConfig::default(),
        }
    }
}

impl Config {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading config: {}", path.display()))?;
        let mut unknown = Vec::new();
        let deserializer = serde_yaml_ng::Deserializer::from_str(&content);
        let config: Config =
            serde_ignored::deserialize(deserializer, |key| unknown.push(key.to_string()))
                .with_context(|| format!("parsing config: {}", path.display()))?;
        for key in unknown {
            log::warn!("unknown config key '{key}' in {}", path.display());
        }
        Ok(config)
    }

    /// Parse a config, rejecting unknown keys instead of warning. Used by the
    /// setup session to catch the model inventing options.
    pub fn from_file_strict(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading config: {}", path.display()))?;
        Self::parse_strict(&content).with_context(|| format!("parsing config: {}", path.display()))
    }

    /// Strictly parse config YAML, erroring on unknown keys.
    pub fn parse_strict(content: &str) -> anyhow::Result<Self> {
        let mut unknown = Vec::new();
        let deserializer = serde_yaml_ng::Deserializer::from_str(content);
        let config: Config =
            serde_ignored::deserialize(deserializer, |key| unknown.push(key.to_string()))?;
        if !unknown.is_empty() {
            anyhow::bail!(
                "unknown config key(s): {}",
                unknown
                    .into_iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(config)
    }

    /// Copy the connection fields (provider, credentials, model, flavor) from
    /// `other`, leaving the rest of `self` untouched. Used by setup so the AI
    /// can never change or lose the provider connection or its key.
    pub fn copy_connection_from(&mut self, other: &Config) {
        self.provider = other.provider.clone();
        self.api_key = other.api_key.clone();
        self.api_base = other.api_base.clone();
        self.model = other.model.clone();
        self.flavor = other.flavor;
    }

    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("ai").join("config.yaml"))
    }

    /// Restore search API keys from `original` for providers whose key the setup
    /// AI omitted or blanked. New providers are left as-is (env fallback applies).
    pub fn preserve_search_secrets(&mut self, original: &Config) {
        let Some(providers) = self.search.providers.as_mut() else {
            return;
        };
        let originals: Vec<(SearchProviderName, Option<String>)> = original
            .search
            .providers
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|p| (p.name, p.api_key.clone()))
            .collect();
        for p in providers.iter_mut() {
            let missing = p
                .api_key
                .as_deref()
                .is_none_or(|k| k.is_empty() || k == "(redacted)");
            if missing && let Some((_, key)) = originals.iter().find(|(n, _)| *n == p.name) {
                p.api_key = key.clone();
            }
        }
    }

    /// Replace literal search API keys with a placeholder for display.
    pub fn redact_search_secrets(&mut self) {
        if let Some(providers) = self.search.providers.as_mut() {
            for p in providers.iter_mut() {
                if p.api_key
                    .as_deref()
                    .is_some_and(|k| !k.is_empty() && !k.starts_with("env:"))
                {
                    p.api_key = Some("(redacted)".to_string());
                }
            }
        }
    }

    pub fn resolve_api_key(&self) -> Option<String> {
        self.api_key.as_deref().and_then(resolve_secret)
    }

    pub fn session_dir_resolved(&self) -> PathBuf {
        self.session_dir
            .clone()
            .map(|p| crate::util::expand_tilde(&p.to_string_lossy()))
            .unwrap_or_else(|| {
                dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("ai")
                    .join("sessions")
            })
    }

    pub fn memory_path_resolved(&self) -> PathBuf {
        self.memory
            .clone()
            .map(|p| crate::util::expand_tilde(&p.to_string_lossy()))
            .unwrap_or_else(|| {
                dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("ai")
                    .join("memory.db")
            })
    }

    pub fn skills_dir_resolved(&self) -> PathBuf {
        self.skills_dir
            .clone()
            .map(|p| crate::util::expand_tilde(&p.to_string_lossy()))
            .unwrap_or_else(|| {
                dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("ai")
                    .join("skills")
            })
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml_ng::to_string(self)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let c = Config::default();
        assert_eq!(c.provider, "openai");
        assert_eq!(c.model, "gpt-4o");
        assert!(c.api_key.is_none());
        assert!(c.session_dir.is_none());
    }

    #[test]
    fn test_from_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ai-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let c = Config {
            provider: "anthropic".to_string(),
            api_key: Some("env:ANTHROPIC_API_KEY".to_string()),
            api_base: Some("https://example.com".to_string()),
            model: "claude-sonnet-4-20250514".to_string(),
            ..Config::default()
        };
        c.save(&path).unwrap();
        let loaded = Config::from_file(&path).unwrap();
        assert_eq!(loaded.provider, "anthropic");
        assert_eq!(loaded.model, "claude-sonnet-4-20250514");
        assert_eq!(loaded.api_key.as_deref(), Some("env:ANTHROPIC_API_KEY"));
        assert_eq!(loaded.api_base.as_deref(), Some("https://example.com"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_partial_config_uses_defaults() {
        let yaml = "provider: groq\nmodel: llama-3.3-70b-versatile\n";
        let c: Config = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(c.provider, "groq");
        assert_eq!(c.model, "llama-3.3-70b-versatile");
        assert!(c.api_key.is_none());
        assert!(c.system_prompt.is_none());
        assert!(c.search.providers.is_none());
    }

    #[test]
    fn test_search_default_providers() {
        let names: Vec<_> = Config::default()
            .search
            .resolved_entries()
            .iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(
            names,
            vec![
                SearchProviderName::DuckDuckGo,
                SearchProviderName::Google,
                SearchProviderName::Bing
            ]
        );
    }

    #[test]
    fn test_search_provider_list_and_env_fallback() {
        let yaml = "search:\n  providers:\n    - name: brave\n    - name: searxng\n      url: http://localhost:8080/search\n    - name: tavily\n      api_key: env:AI_TEST_TAVILY\n";
        let c: Config = serde_yaml_ng::from_str(yaml).unwrap();
        // SAFETY: env mutation is contained to this uniquely-named test.
        unsafe {
            std::env::set_var("BRAVE_API_KEY", "brave-key");
            std::env::set_var("AI_TEST_TAVILY", "tav-key");
        }
        let entries = c.search.resolved_entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, SearchProviderName::Brave);
        assert_eq!(entries[0].api_key.as_deref(), Some("brave-key"));
        assert!(entries[0].is_configured());
        assert_eq!(entries[1].name, SearchProviderName::Searxng);
        assert_eq!(
            entries[1].url.as_deref(),
            Some("http://localhost:8080/search")
        );
        assert_eq!(entries[2].name, SearchProviderName::Tavily);
        assert_eq!(entries[2].api_key.as_deref(), Some("tav-key"));
        unsafe {
            std::env::remove_var("BRAVE_API_KEY");
            std::env::remove_var("AI_TEST_TAVILY");
        }
    }

    #[test]
    fn test_search_unconfigured_keyed_provider() {
        let yaml = "search:\n  providers:\n    - name: exa\n";
        let c: Config = serde_yaml_ng::from_str(yaml).unwrap();
        // SAFETY: env mutation is contained to this uniquely-named test.
        unsafe {
            std::env::remove_var("EXA_API_KEY");
        }
        let entries = c.search.resolved_entries();
        assert!(!entries[0].is_configured());
    }

    #[test]
    fn test_search_unknown_provider_errors() {
        let yaml = "search:\n  providers:\n    - name: nope\n";
        assert!(serde_yaml_ng::from_str::<Config>(yaml).is_err());
    }

    #[test]
    fn test_resolve_api_key_env() {
        let c = Config {
            api_key: Some("env:AI_TEST_KEY".to_string()),
            ..Config::default()
        };
        // SAFETY: env mutation is contained to this uniquely-named test key.
        unsafe {
            std::env::set_var("AI_TEST_KEY", "secret-value");
        }
        assert_eq!(c.resolve_api_key().as_deref(), Some("secret-value"));
        unsafe {
            std::env::remove_var("AI_TEST_KEY");
        }
    }

    #[test]
    fn test_resolve_api_key_plain() {
        let c = Config {
            api_key: Some("sk-plain".to_string()),
            ..Config::default()
        };
        assert_eq!(c.resolve_api_key().as_deref(), Some("sk-plain"));
    }

    #[test]
    fn test_resolve_api_key_missing_env() {
        let c = Config {
            api_key: Some("env:AI_MISSING_KEY".to_string()),
            ..Config::default()
        };
        unsafe {
            std::env::remove_var("AI_MISSING_KEY");
        }
        assert_eq!(c.resolve_api_key(), None);
    }

    #[test]
    fn test_proxy_field_roundtrip_and_default() {
        assert!(Config::default().proxy.is_none());
        let c = Config {
            proxy: Some("socks5h://127.0.0.1:1080".to_string()),
            ..Config::default()
        };
        let dir = std::env::temp_dir().join(format!("ai-proxy-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        c.save(&path).unwrap();
        let loaded = Config::from_file(&path).unwrap();
        assert_eq!(loaded.proxy.as_deref(), Some("socks5h://127.0.0.1:1080"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolved_paths_override() {
        let c = Config {
            session_dir: Some(PathBuf::from("/custom/sessions")),
            skills_dir: Some(PathBuf::from("/custom/skills")),
            memory: Some(PathBuf::from("/custom/memory.json")),
            ..Config::default()
        };
        assert_eq!(c.session_dir_resolved(), PathBuf::from("/custom/sessions"));
        assert_eq!(c.skills_dir_resolved(), PathBuf::from("/custom/skills"));
        assert_eq!(
            c.memory_path_resolved(),
            PathBuf::from("/custom/memory.json")
        );
    }

    #[test]
    fn test_resolved_paths_defaults() {
        let c = Config::default();
        let base = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ai");
        assert_eq!(c.session_dir_resolved(), base.join("sessions"));
        assert_eq!(c.skills_dir_resolved(), base.join("skills"));
        assert_eq!(c.memory_path_resolved(), base.join("memory.db"));
    }

    #[test]
    fn test_flavor_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ai-flavor-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let c = Config {
            provider: "some-endpoint".to_string(),
            api_base: Some("https://example.com/v1".to_string()),
            flavor: Some(ProviderFlavor::Anthropic),
            ..Config::default()
        };
        c.save(&path).unwrap();
        let loaded = Config::from_file(&path).unwrap();
        assert_eq!(loaded.flavor, Some(ProviderFlavor::Anthropic));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_flavor_absent_by_default() {
        let c: Config = serde_yaml_ng::from_str("provider: openai\n").unwrap();
        assert!(c.flavor.is_none());
    }

    #[test]
    fn test_from_file_strict_rejects_unknown_keys() {
        let dir = std::env::temp_dir().join(format!("ai-strict-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "provider: openai\nnot_a_key: true\n").unwrap();
        assert!(Config::from_file_strict(&path).is_err());
        std::fs::write(&path, "provider: openai\nmodel: gpt-4o\n").unwrap();
        assert!(Config::from_file_strict(&path).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_strict() {
        let parsed = Config::parse_strict("provider: groq\nmodel: llama-3.3-70b-versatile\n");
        assert_eq!(parsed.unwrap().provider, "groq");
        assert!(Config::parse_strict("provider: groq\nnope: 1\n").is_err());
    }

    #[test]
    fn test_copy_connection_from_preserves_credentials() {
        let source = Config {
            provider: "anthropic".to_string(),
            api_key: Some("env:ANTHROPIC_API_KEY".to_string()),
            api_base: Some("https://api.anthropic.com".to_string()),
            model: "claude-sonnet-4-20250514".to_string(),
            flavor: Some(ProviderFlavor::Anthropic),
            ..Config::default()
        };
        let mut edited = Config {
            system_prompt: Some("be terse".to_string()),
            proxy: Some("socks5h://127.0.0.1:1080".to_string()),
            provider: "openai".to_string(),
            api_key: Some("stolen".to_string()),
            model: "gpt-4o".to_string(),
            ..Config::default()
        };
        edited.copy_connection_from(&source);
        assert_eq!(edited.provider, "anthropic");
        assert_eq!(edited.api_key.as_deref(), Some("env:ANTHROPIC_API_KEY"));
        assert_eq!(
            edited.api_base.as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(edited.model, "claude-sonnet-4-20250514");
        assert_eq!(edited.flavor, Some(ProviderFlavor::Anthropic));
        assert_eq!(edited.system_prompt.as_deref(), Some("be terse"));
        assert_eq!(edited.proxy.as_deref(), Some("socks5h://127.0.0.1:1080"));
    }
}
