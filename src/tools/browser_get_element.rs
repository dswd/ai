#![cfg(feature = "browser")]

use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::browser_state::{BrowserState, with_page};
use super::search_html::html_to_markdown;
use super::shared::{ToolError, js_literal};
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, process_output, truncate};
use crate::policy::Policy;

// ----- BrowserGetElementTool -----

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserGetElementArgs {
    #[schemars(description = "CSS selector of the element to extract")]
    pub selector: String,
    #[schemars(description = "Output format: \"markdown\" (default) or \"html\"")]
    pub format: Option<String>,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of lines to return")]
    pub limit: Option<usize>,
}

#[derive(Clone)]
pub struct BrowserGetElementTool {
    policy: Policy,
    browser: Arc<obscura::Browser>,
    last_url: Arc<Mutex<Option<String>>>,
}

impl BrowserGetElementTool {
    pub fn new(policy: Policy, browser: Arc<BrowserState>) -> Self {
        Self {
            policy,
            browser: Arc::clone(&browser.browser),
            last_url: Arc::clone(&browser.last_url),
        }
    }
}

impl std::fmt::Debug for BrowserGetElementTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserGetElementTool").finish()
    }
}

impl PortableTool for BrowserGetElementTool {
    const NAME: &'static str = "browser_get_element";

    type Args = BrowserGetElementArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Extract the content of a specific DOM element by CSS selector from the current page."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(BrowserGetElementArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}💻 browser get element: {}{RESET}", args.selector,);
        if args.selector.is_empty() {
            return Err(ToolError::Message("selector is required".to_string()));
        }
        let url = self.last_url.lock().unwrap().clone().unwrap_or_default();
        if url.is_empty() {
            return Err(ToolError::Message(
                "no page loaded — call browser_navigate first".to_string(),
            ));
        }
        let _ = &self.policy;
        let want_html = args.format.as_deref() == Some("html");
        let selector = args.selector.clone();
        let browser = Arc::clone(&self.browser);

        let inner_html = with_page(
            browser,
            "browser timed out after 30s",
            move |mut page| async move {
                let sel = js_literal(&selector);
                page.goto(&url).await.map_err(|e| format!("goto: {e}"))?;
                tokio::time::sleep(Duration::from_millis(500)).await;

                let js = format!(
                    r#"(function(){{var e=document.querySelector({sel});return e?e.innerHTML:'__not_found__'}})()"#
                );
                let raw = page.evaluate(&js);
                let inner = raw.as_str().unwrap_or("");
                if inner == "__not_found__" {
                    return Err(format!("element not found: {}", selector));
                }
                Ok(inner.to_string())
            },
        )
        .await
        .map_err(ToolError::Message)?;

        let output = if want_html {
            inner_html
        } else {
            html_to_markdown(&inner_html)
        };
        let truncated = truncate(&output, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title(&args.selector),
            bar_line()
        );
        process_output(&output, args.offset, args.limit).map_err(ToolError::Message)
    }
}
