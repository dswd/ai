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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SearchConfig {
    /// Full URL of a SearXNG instance used for web search. `{query}` may be
    /// used as the query placeholder; a bare URL gets `?q=` appended.
    #[serde(default)]
    pub searxng_url: Option<String>,
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
    /// Path to the persistent memory JSON file.
    pub memory: Option<PathBuf>,
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

    pub fn resolve_api_key(&self) -> Option<String> {
        self.api_key.as_ref().and_then(|key| {
            if let Some(env_var) = key.strip_prefix("env:") {
                std::env::var(env_var).ok()
            } else {
                Some(key.clone())
            }
        })
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
                    .join("memory.json")
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
        assert!(c.search.searxng_url.is_none());
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
        assert_eq!(c.memory_path_resolved(), base.join("memory.json"));
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
