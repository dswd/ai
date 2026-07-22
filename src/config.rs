use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    pub policy: Option<PathBuf>,
    pub memory: Option<PathBuf>,
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
            policy: None,
            memory: None,
        }
    }
}

impl Config {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading config: {}", path.display()))?;
        let config: Config =
            serde_yaml::from_str(&content).with_context(|| format!("parsing config: {}", path.display()))?;
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
        self.session_dir
            .clone()
            .unwrap_or_else(|| dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("ai").join("sessions"))
    }

    pub fn memory_path_resolved(&self) -> PathBuf {
        self.memory
            .clone()
            .unwrap_or_else(|| dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("ai").join("memory.json"))
    }
}
