#![cfg(feature = "browser")]

use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::browser_state::{BrowserState, with_page};
use super::search_html::html_to_markdown;
use super::shared::ToolError;
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, process_output, truncate};
use crate::policy::Policy;

// ----- BrowserGetContentTool -----

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserGetContentArgs {
    #[schemars(description = "Output format: \"markdown\" (default) or \"html\"")]
    pub format: Option<String>,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of lines to return")]
    pub limit: Option<usize>,
}

#[derive(Clone)]
pub struct BrowserGetContentTool {
    policy: Policy,
    browser: Arc<obscura::Browser>,
    last_url: Arc<Mutex<Option<String>>>,
}

impl BrowserGetContentTool {
    pub fn new(policy: Policy, browser: Arc<BrowserState>) -> Self {
        Self {
            policy,
            browser: Arc::clone(&browser.browser),
            last_url: Arc::clone(&browser.last_url),
        }
    }
}

impl std::fmt::Debug for BrowserGetContentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserGetContentTool").finish()
    }
}

impl Tool for BrowserGetContentTool {
    const NAME: &'static str = "browser_get_content";

    type Args = BrowserGetContentArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Get the full page content of the current page in markdown or raw HTML.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(BrowserGetContentArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}💻 browser get content{RESET}");
        let url = self.last_url.lock().unwrap().clone().unwrap_or_default();
        if url.is_empty() {
            return Err(ToolError::Message(
                "no page loaded — call browser_navigate first".to_string(),
            ));
        }
        let _ = &self.policy;
        let want_html = args.format.as_deref() == Some("html");
        let browser = Arc::clone(&self.browser);

        let html = with_page(
            browser,
            "browser timed out after 30s",
            move |mut page| async move {
                page.goto(&url).await.map_err(|e| format!("goto: {e}"))?;
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(page.content())
            },
        )
        .await
        .map_err(ToolError::Message)?;

        let output = if want_html {
            html
        } else {
            html_to_markdown(&html)
        };
        let truncated = truncate(&output, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title("page content"),
            bar_line()
        );
        process_output(&output, args.offset, args.limit).map_err(ToolError::Message)
    }
}
