use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::ToolError;
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DownloadFileArgs {
    #[schemars(description = "URL to download from")]
    pub url: String,
    #[schemars(description = "Local file path to save to")]
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct DownloadFileTool {
    policy: Policy,
}

impl DownloadFileTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for DownloadFileTool {
    const NAME: &'static str = "download_file";

    type Args = DownloadFileArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Download a file from a URL and save it to disk (max 5 MB).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(DownloadFileArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}📥 download file {}{RESET}", args.url);
        if args.url.is_empty() {
            return Err(ToolError::Message("URL is required".to_string()));
        }
        if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
            return Err(ToolError::Message(
                "URL must start with http:// or https://".to_string(),
            ));
        }
        if args.path.is_empty() {
            return Err(ToolError::Message("path is required".to_string()));
        }

        if !self.policy.is_allowed(&Action::WebFetch, &args.url) {
            return Err(ToolError::Message(format!(
                "web fetch access denied for: {}",
                args.url
            )));
        }

        let client = reqwest::Client::builder()
            .user_agent("ai-cli/1.0")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ToolError::Message(format!("failed to create HTTP client: {e}")))?;

        let resp = client
            .get(&args.url)
            .send()
            .await
            .map_err(|e| ToolError::Message(format!("failed to fetch URL: {e}")))?;

        if !resp.status().is_success() {
            return Err(ToolError::Message(format!(
                "request failed with status code: {}",
                resp.status()
            )));
        }

        let body = resp
            .bytes()
            .await
            .map_err(|e| ToolError::Message(format!("failed to read response body: {e}")))?;

        let max_size: usize = 5 * 1024 * 1024;
        if body.len() > max_size {
            return Err(ToolError::Message(format!(
                "response too large: {} bytes (max {})",
                body.len(),
                max_size
            )));
        }

        let path = std::path::Path::new(&args.path);
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Message(format!("cannot create parent dirs: {e}")))?;
        }

        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let path_str = canonical.to_string_lossy().to_string();

        if !self.policy.is_allowed(&Action::Write, &path_str) {
            return Err(ToolError::Message(format!(
                "write access denied for: {path_str}"
            )));
        }

        std::fs::write(path, &body)
            .map_err(|e| ToolError::Message(format!("cannot write file: {e}")))?;

        let size_str = if body.len() < 1024 {
            format!("{} B", body.len())
        } else if body.len() < 1024 * 1024 {
            format!("{:.1} KB", body.len() as f64 / 1024.0)
        } else {
            format!("{:.1} MB", body.len() as f64 / (1024.0 * 1024.0))
        };

        let result = format!("Downloaded {} ({size_str})", args.url);
        Ok(result)
    }
}
