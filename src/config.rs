use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchConfig {
    #[serde(default)]
    pub searxng_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub provider: String,
    #[serde(alias = "api_key")]
    pub api_key: Option<String>,
    #[serde(alias = "api_base")]
    pub api_base: Option<String>,
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_tokens: Option<usize>,
    pub thinking: Option<usize>,
    pub session_dir: Option<PathBuf>,
    pub skills_dir: Option<PathBuf>,
    pub policy: Option<PathBuf>,
    pub memory: Option<PathBuf>,
    pub context_window: Option<usize>,
    #[serde(default)]
    pub search: SearchConfig,
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
            search: SearchConfig::default(),
        }
    }
}

impl Config {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading config: {}", path.display()))?;
        let config: Config = serde_yaml_ng::from_str(&content)
            .with_context(|| format!("parsing config: {}", path.display()))?;
        Ok(config)
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
        self.session_dir.clone().unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("ai")
                .join("sessions")
        })
    }

    pub fn memory_path_resolved(&self) -> PathBuf {
        self.memory.clone().unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("ai")
                .join("memory.json")
        })
    }

    pub fn skills_dir_resolved(&self) -> PathBuf {
        self.skills_dir.clone().unwrap_or_else(|| {
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
}
