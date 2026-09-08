#![cfg(feature = "browser")]

use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::browser_state::{BrowserState, with_page};
use super::shared::ToolError;
use crate::policy::{Action, Policy};

// ----- BrowserNavigateTool -----

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserNavigateArgs {
    #[schemars(description = "The URL to navigate to")]
    pub url: String,
}

#[derive(Clone)]
pub struct BrowserNavigateTool {
    policy: Policy,
    browser: Arc<obscura::Browser>,
    last_url: Arc<Mutex<Option<String>>>,
}

impl BrowserNavigateTool {
    pub fn new(policy: Policy, browser: Arc<BrowserState>) -> Self {
        Self {
            policy,
            browser: Arc::clone(&browser.browser),
            last_url: Arc::clone(&browser.last_url),
        }
    }
}

impl Tool for BrowserNavigateTool {
    const NAME: &'static str = "browser_navigate";

    type Args = BrowserNavigateArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Navigate the browser to a URL. Returns the page title and content size.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(BrowserNavigateArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}💻 browser navigate: {}{RESET}", args.url,);
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
                "browse access denied for: {}",
                args.url
            )));
        }

        let url = args.url.clone();
        let url_label = args.url;
        let browser = Arc::clone(&self.browser);
        let last_url = Arc::clone(&self.last_url);
        let (title, size) = with_page(
            browser,
            "browser timed out after 30s",
            move |mut page| async move {
                page.goto(&url).await.map_err(|e| format!("goto: {e}"))?;
                tokio::time::sleep(Duration::from_millis(500)).await;
                let title = page
                    .evaluate("document.title")
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let size = page.content().len();
                Ok((title, size))
            },
        )
        .await
        .map_err(ToolError::Message)?;

        *last_url.lock().unwrap() = Some(url_label.clone());
        Ok(format!(
            "Navigated to: {}\nTitle: {}\nContent: {} bytes",
            url_label, title, size
        ))
    }
}
