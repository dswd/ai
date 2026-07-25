use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::shared::ToolError;
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchArgs {
    #[schemars(description = "The URL to fetch content from")]
    pub url: String,
    #[schemars(description = "The format to return the content in (text, markdown, or html)")]
    pub format: Option<String>,
    #[schemars(description = "Optional timeout in seconds (max 120)")]
    pub timeout: Option<u64>,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of lines to return")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct WebFetchTool {
    policy: Policy,
}

impl WebFetchTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for WebFetchTool {
    const NAME: &'static str = "web_fetch";

    type Args = WebFetchArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Fetches content from a URL and returns it in the specified format. \
         Supports text, markdown, and html output formats. \
         Maximum response size is 5MB."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(WebFetchArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}🌐 fetch {}{}{RESET}",
            args.url,
            fmt_offset_limit(args.offset, args.limit)
        );
        if args.url.is_empty() {
            return Err(ToolError::Message("URL is required".to_string()));
        }

        if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
            return Err(ToolError::Message(
                "URL must start with http:// or https://".to_string(),
            ));
        }

        if !self.policy.is_allowed(&Action::WebFetch, &args.url) {
            return Err(ToolError::Message(format!(
                "web fetch access denied for: {}",
                args.url
            )));
        }

        let format = args.format.as_deref().unwrap_or("markdown").to_lowercase();
        if format != "text" && format != "markdown" && format != "html" {
            return Err(ToolError::Message(
                "format must be one of: text, markdown, html".to_string(),
            ));
        }

        let timeout_secs = args.timeout.unwrap_or(30).min(120);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent("ai-cli/1.0")
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

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = resp
            .text()
            .await
            .map_err(|e| ToolError::Message(format!("failed to read response body: {e}")))?;

        const MAX_SIZE: usize = 5 * 1024 * 1024;
        if body.len() > MAX_SIZE {
            return Err(ToolError::Message(format!(
                "response too large: {} bytes (max {})",
                body.len(),
                MAX_SIZE
            )));
        }

        let result = match format.as_str() {
            "text" => {
                if content_type.contains("text/html") {
                    html2text::from_read(body.as_bytes(), 80)
                } else {
                    Ok(body)
                }
            }
            "markdown" => {
                if content_type.contains("text/html") {
                    html2text::from_read(body.as_bytes(), 80)
                } else {
                    Ok(format!("```\n{body}\n```"))
                }
            }
            "html" => Ok(body),
            _ => Ok(body),
        };
        let result = result.map_err(|e| ToolError::Message(format!("failed to convert: {e}")))?;
        let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title(&args.url),
            bar_line()
        );
        process_output(&result, args.offset, args.limit).map_err(ToolError::Message)
    }
}
