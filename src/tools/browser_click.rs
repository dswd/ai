#![cfg(feature = "browser")]

use ansi_color_constants::*;
use log::info;
use regex::Regex;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::browser_state::{BrowserState, with_page};
use super::shared::{ToolError, js_literal};
use crate::policy::{Action, Policy};

// ----- BrowserClickTool -----
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserClickArgs {
    #[schemars(description = "CSS selector of the element to click")]
    pub selector: String,
    #[schemars(description = "Optional URL to navigate to before clicking")]
    pub url: Option<String>,
}

#[derive(Clone)]
pub struct BrowserClickTool {
    policy: Policy,
    browser: Arc<obscura::Browser>,
    last_url: Arc<Mutex<Option<String>>>,
}

impl BrowserClickTool {
    pub fn new(policy: Policy, browser: Arc<BrowserState>) -> Self {
        Self {
            policy,
            browser: Arc::clone(&browser.browser),
            last_url: Arc::clone(&browser.last_url),
        }
    }
}

impl std::fmt::Debug for BrowserClickTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserClickTool").finish()
    }
}

impl Tool for BrowserClickTool {
    const NAME: &'static str = "browser_click";

    type Args = BrowserClickArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Click an element on the page by CSS selector. Returns the new page title and content size."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(BrowserClickArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}💻 browser click: {}{RESET}", args.selector,);
        if args.selector.is_empty() {
            return Err(ToolError::Message("selector is required".to_string()));
        }

        let url = args
            .url
            .unwrap_or_else(|| self.last_url.lock().unwrap().clone().unwrap_or_default());
        if url.is_empty() {
            return Err(ToolError::Message(
                "no URL — call browser_navigate first or provide a url parameter".to_string(),
            ));
        }
        if !self.policy.is_allowed(&Action::WebFetch, &url) {
            return Err(ToolError::Message(format!(
                "browse access denied for: {}",
                url
            )));
        }
        let selector = args.selector.clone();
        let selector_label = args.selector.clone();
        let browser = Arc::clone(&self.browser);

        let sel_for_closure = selector_label.clone();
        let html = with_page(
            browser,
            "browser timed out after 30s",
            move |mut page| async move {
                let sel = js_literal(&selector);
                page.goto(&url).await.map_err(|e| format!("goto: {e}"))?;
                tokio::time::sleep(Duration::from_millis(800)).await;

                let click_js = format!(
                    r#"(function(){{var e=document.querySelector({sel});if(!e)return'not found';e.dispatchEvent(new MouseEvent('click',{{bubbles:true}}));return'clicked'}})()"#
                );
                let clicked = page.evaluate(&click_js);
                if clicked.as_str() == Some("not found") {
                    return Err(format!("element not found: {}", sel_for_closure));
                }
                tokio::time::sleep(Duration::from_millis(1500)).await;

                Ok(page.content())
            },
        )
        .await
        .map_err(ToolError::Message)?;

        let title = {
            let re = Regex::new(r"<title>(.*?)</title>").unwrap();
            re.captures(&html)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        };
        Ok(format!(
            "Clicked {}\nTitle: {}\nContent: {} bytes",
            selector_label,
            title,
            html.len()
        ))
    }
}
