use crate::util::fmt_bytes;
use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::shared::{ToolError, http_client};
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

        let resp = http_client()
            .get(&args.url)
            .timeout(std::time::Duration::from_secs(30))
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

        // Resolve the destination to an absolute path before checking policy:
        // `canonicalize` fails for a not-yet-existing file, so resolve the
        // parent and re-append the file name (same approach as write_file).
        // Without this, relative destinations never match absolute allow
        // patterns and every download to a new file is denied.
        let canonical = if path.exists() {
            path.canonicalize()
                .map_err(|e| ToolError::Message(format!("cannot resolve path: {e}")))?
        } else if let Some(parent) = path.parent() {
            parent
                .canonicalize()
                .map_err(|e| ToolError::Message(format!("cannot resolve parent: {e}")))?
                .join(path.file_name().unwrap_or_default())
        } else {
            path.to_path_buf()
        };
        let path_str = canonical.to_string_lossy().to_string();

        // Check policy BEFORE creating anything so a denied write has no
        // side effects (the old code created parent dirs first).
        if !self.policy.is_allowed(&Action::Write, &path_str) {
            return Err(ToolError::Message(format!(
                "write access denied for: {path_str}"
            )));
        }

        if let Some(parent) = canonical.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::Message(format!("cannot create parent dirs: {e}")))?;
        }

        std::fs::write(&canonical, &body)
            .map_err(|e| ToolError::Message(format!("cannot write file: {e}")))?;

        let size_str = fmt_bytes(body.len() as u64);

        let result = format!("Downloaded {} ({size_str})", args.url);
        Ok(result)
    }
}
