#![cfg(feature = "browser")]

use ansi_color_constants::*;
use log::info;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::browser_state::{BrowserState, with_page};
use super::shared::ToolError;
use crate::policy::{Action, Policy};

// ----- BrowserEvaluateTool -----

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserEvaluateArgs {
    #[schemars(description = "JavaScript expression to evaluate on the page")]
    pub expression: String,
    #[schemars(description = "Optional URL to navigate to before evaluating")]
    pub url: Option<String>,
}

#[derive(Clone)]
pub struct BrowserEvaluateTool {
    policy: Policy,
    browser: Arc<obscura::Browser>,
    last_url: Arc<Mutex<Option<String>>>,
}

impl BrowserEvaluateTool {
    pub fn new(policy: Policy, browser: Arc<BrowserState>) -> Self {
        Self {
            policy,
            browser: Arc::clone(&browser.browser),
            last_url: Arc::clone(&browser.last_url),
        }
    }
}

impl std::fmt::Debug for BrowserEvaluateTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserEvaluateTool").finish()
    }
}

impl PortableTool for BrowserEvaluateTool {
    const NAME: &'static str = "browser_evaluate";

    type Args = BrowserEvaluateArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Execute JavaScript on the current page and return the result as JSON.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(BrowserEvaluateArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}💻 browser evaluate{RESET}",);
        if args.expression.is_empty() {
            return Err(ToolError::Message("expression is required".to_string()));
        }

        let url = args
            .url
            .unwrap_or_else(|| self.last_url.lock().unwrap().clone().unwrap_or_default());
        if !url.is_empty() && !self.policy.is_allowed(&Action::WebFetch, &url) {
            return Err(ToolError::Message(format!(
                "browse access denied for: {}",
                url
            )));
        }
        let browser = Arc::clone(&self.browser);
        let expression = args.expression;

        with_page(
            browser,
            "browser timed out after 30s",
            move |mut page| async move {
                if !url.is_empty() {
                    page.goto(&url).await.map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }

                let value = page.evaluate(&expression);
                Ok(format!("{value}"))
            },
        )
        .await
        .map_err(ToolError::Message)
    }
}
