use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use super::shared::ToolError;
use crate::config::Config;
use crate::policy::Policy;
use crate::sandbox::Sandbox;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteConfigArgs {
    #[schemars(
        description = "The complete configuration as YAML. Unknown keys are rejected, and \
                       provider, api_key, api_base, model, and flavor are restored from the \
                       existing config."
    )]
    pub content: String,
}

/// The setup session's target: where to save and the connection fields that
/// must survive the AI's edit.
#[derive(Debug)]
pub struct SetupTarget {
    pub path: PathBuf,
    pub original: Config,
}

/// Setup-only tool that validates YAML, restores the real credentials, and
/// saves the config with the user's approval. It deliberately takes no path
/// argument and reports no path, so the AI never learns the config location.
#[derive(Debug, Clone)]
pub struct WriteConfigTool {
    policy: Policy,
    target: Arc<SetupTarget>,
}

impl WriteConfigTool {
    pub fn new(policy: Policy, target: Arc<SetupTarget>) -> Self {
        Self { policy, target }
    }
}

impl PortableTool for WriteConfigTool {
    const NAME: &'static str = "write_config";

    type Args = WriteConfigArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Validate and save the agent configuration. Pass the complete configuration as YAML. \
         The syntax is checked, the existing provider/credentials/model/flavor are kept, and \
         the user is asked to approve the save."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(WriteConfigArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}⚙️  write config{RESET}");
        let mut config = Config::parse_strict(&args.content)
            .map_err(|e| ToolError::Message(format!("invalid config: {e}")))?;
        config.copy_connection_from(&self.target.original);
        let yaml = serde_yaml_ng::to_string(&config)
            .map_err(|e| ToolError::Message(format!("cannot serialize config: {e}")))?;

        let sandbox = Sandbox::new(self.policy.clone());
        if let Err(e) = sandbox.write(&self.target.path, yaml.as_bytes()) {
            log::debug!("write_config failed: {e}");
            return Err(ToolError::Message(
                "could not save the configuration (access denied or write failed)".to_string(),
            ));
        }
        Ok("Configuration saved and validated.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderFlavor;
    use crate::policy::{Action, PolicyRule};

    fn temp_setup(name: &str) -> (PathBuf, std::path::PathBuf, Config) {
        let dir =
            std::env::temp_dir().join(format!("ai-write-config-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let path = dir.join("config.yaml");
        let original = Config {
            provider: "anthropic".to_string(),
            api_key: Some("env:ANTHROPIC_API_KEY".to_string()),
            api_base: Some("https://api.anthropic.com".to_string()),
            model: "claude-sonnet-4-20250514".to_string(),
            flavor: Some(ProviderFlavor::Anthropic),
            ..Config::default()
        };
        std::fs::write(&path, serde_yaml_ng::to_string(&original).unwrap()).unwrap();
        (dir, path, original)
    }

    fn allow_write(path: &std::path::Path) -> Policy {
        let mut policy = Policy::default();
        policy.add_cli_rule(PolicyRule::Allow(
            Action::Write,
            path.to_string_lossy().to_string(),
        ));
        policy
    }

    #[tokio::test]
    async fn writes_valid_config_and_preserves_credentials() {
        let (dir, path, original) = temp_setup("valid");
        let tool = WriteConfigTool::new(
            allow_write(&path),
            Arc::new(SetupTarget {
                path: path.clone(),
                original,
            }),
        );
        let out = tool
            .call(WriteConfigArgs {
                content: "system_prompt: be terse\nproxy: http://127.0.0.1:8080\n".to_string(),
            })
            .await
            .unwrap();
        assert!(out.contains("saved"));

        let written = Config::from_file(&path).unwrap();
        assert_eq!(written.provider, "anthropic");
        assert_eq!(written.api_key.as_deref(), Some("env:ANTHROPIC_API_KEY"));
        assert_eq!(written.model, "claude-sonnet-4-20250514");
        assert_eq!(written.system_prompt.as_deref(), Some("be terse"));
        assert_eq!(written.proxy.as_deref(), Some("http://127.0.0.1:8080"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rejects_unknown_keys() {
        let (dir, path, original) = temp_setup("unknown");
        let tool = WriteConfigTool::new(
            allow_write(&path),
            Arc::new(SetupTarget {
                path: path.clone(),
                original,
            }),
        );
        let err = tool
            .call(WriteConfigArgs {
                content: "nope: 1\n".to_string(),
            })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("unknown config key"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn denial_does_not_leak_the_path() {
        let (dir, path, original) = temp_setup("denial");
        let tool = WriteConfigTool::new(
            Policy::default(),
            Arc::new(SetupTarget {
                path: path.clone(),
                original,
            }),
        );
        let err = tool
            .call(WriteConfigArgs {
                content: "system_prompt: x\n".to_string(),
            })
            .await
            .unwrap_err();
        assert!(!format!("{err}").contains(&path.to_string_lossy().to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
