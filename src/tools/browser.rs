use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use regex::Regex;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::shared::ToolError;
use super::web_search::html_to_markdown;
use super::{process_output, truncate, MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES};
use crate::policy::{Action, Policy};

#[derive(Clone)]
pub struct BrowserState {
    browser: Arc<obscura::Browser>,
    last_url: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for BrowserState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserState").finish()
    }
}

impl BrowserState {
    pub async fn new() -> Result<Self, String> {
        let browser = obscura::Browser::builder()
            .stealth(true)
            .build()
            .map_err(|e| format!("obscura: {e}"))?;
        Ok(Self {
            browser: Arc::new(browser),
            last_url: Arc::new(Mutex::new(None)),
        })
    }
}

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
        info!(
            "{DIM}💻 browser navigate: {}{RESET}",
            args.url,
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
                "browse access denied for: {}",
                args.url
            )));
        }

        let url = args.url.clone();
        let url_label = args.url;
        let browser = Arc::clone(&self.browser);
        let last_url = Arc::clone(&self.last_url);
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("browser rt: {e}"))?;
                rt.block_on(async {
                    let mut page = browser
                        .new_page()
                        .await
                        .map_err(|e| format!("page: {e}"))?;
                    page.goto(&url)
                        .await
                        .map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let title = page
                        .evaluate("document.title")
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let size = page.content().len();
                    Ok((title, size))
                })
            }),
        )
        .await;

        let (title, size) = match result {
            Ok(Ok(Ok(v))) => v,
            Ok(Ok(Err(e))) => return Err(ToolError::Message(e)),
            Ok(Err(e)) => return Err(ToolError::Message(e.to_string())),
            Err(_) => return Err(ToolError::Message("browser timed out after 30s".to_string())),
        };

        *last_url.lock().unwrap() = Some(url_label.clone());
        Ok(format!(
            "Navigated to: {}\nTitle: {}\nContent: {} bytes",
            url_label, title, size
        ))
    }
}

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
        info!(
            "{DIM}💻 browser click: {}{RESET}",
            args.selector,
        );
        if args.selector.is_empty() {
            return Err(ToolError::Message("selector is required".to_string()));
        }

        let url = args.url.unwrap_or_else(|| {
            self.last_url
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_default()
        });
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
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("browser rt: {e}"))?;
                let sel = selector.replace('\\', "\\\\").replace('\'', "\\'");
                rt.block_on(async {
                    let mut page = browser
                        .new_page()
                        .await
                        .map_err(|e| format!("page: {e}"))?;

                    page.goto(&url)
                        .await
                        .map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(800)).await;

                    let click_js = format!(
                        r#"(function(){{var e=document.querySelector('{sel}');if(!e)return'not found';e.dispatchEvent(new MouseEvent('click',{{bubbles:true}}));return'clicked'}})()"#
                    );
                    let clicked = page.evaluate(&click_js);
                    if clicked.as_str() == Some("not found") {
                        return Err(format!("element not found: {}", sel_for_closure));
                    }
                    tokio::time::sleep(Duration::from_millis(1500)).await;

                    Ok(page.content())
                })
            }),
        )
        .await;

        let html = match result {
            Ok(Ok(Ok(h))) => h,
            Ok(Ok(Err(e))) => return Err(ToolError::Message(e)),
            Ok(Err(e)) => return Err(ToolError::Message(e.to_string())),
            Err(_) => return Err(ToolError::Message("browser timed out after 30s".to_string())),
        };

        let title = {
            let re =             Regex::new(r"<title>(.*?)</title>").unwrap();
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
        let url = self
            .last_url
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default();
        if url.is_empty() {
            return Err(ToolError::Message(
                "no page loaded — call browser_navigate first".to_string(),
            ));
        }
        let _ = &self.policy;
        let want_html = args.format.as_deref() == Some("html");
        let browser = Arc::clone(&self.browser);

        let result = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("browser rt: {e}"))?;
                rt.block_on(async {
                    let mut page = browser
                        .new_page()
                        .await
                        .map_err(|e| format!("page: {e}"))?;
                    page.goto(&url)
                        .await
                        .map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    Ok(page.content())
                })
            }),
        )
        .await;

        let html = match result {
            Ok(Ok(Ok(h))) => h,
            Ok(Ok(Err(e))) => return Err(ToolError::Message(e)),
            Ok(Err(e)) => return Err(ToolError::Message(e.to_string())),
            Err(_) => return Err(ToolError::Message("browser timed out after 30s".to_string())),
        };

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

impl Tool for BrowserGetElementTool {
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
        info!(
            "{DIM}💻 browser get element: {}{RESET}",
            args.selector,
        );
        if args.selector.is_empty() {
            return Err(ToolError::Message("selector is required".to_string()));
        }
        let url = self
            .last_url
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default();
        if url.is_empty() {
            return Err(ToolError::Message(
                "no page loaded — call browser_navigate first".to_string(),
            ));
        }
        let _ = &self.policy;
        let want_html = args.format.as_deref() == Some("html");
        let selector = args.selector.clone();
        let browser = Arc::clone(&self.browser);

        let result = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("browser rt: {e}"))?;
                let sel = selector.replace('\\', "\\\\").replace('\'', "\\'");
                rt.block_on(async {
                    let mut page = browser
                        .new_page()
                        .await
                        .map_err(|e| format!("page: {e}"))?;
                    page.goto(&url)
                        .await
                        .map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    let js = format!(
                        r#"(function(){{var e=document.querySelector('{sel}');return e?e.innerHTML:'__not_found__'}})()"#
                    );
                    let raw = page.evaluate(&js);
                    let inner = raw.as_str().unwrap_or("");
                    if inner == "__not_found__" {
                        return Err(format!("element not found: {}", selector));
                    }
                    Ok(inner.to_string())
                })
            }),
        )
        .await;

        let inner_html = match result {
            Ok(Ok(Ok(h))) => h,
            Ok(Ok(Err(e))) => return Err(ToolError::Message(e)),
            Ok(Err(e)) => return Err(ToolError::Message(e.to_string())),
            Err(_) => return Err(ToolError::Message("browser timed out after 30s".to_string())),
        };

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

impl Tool for BrowserEvaluateTool {
    const NAME: &'static str = "browser_evaluate";

    type Args = BrowserEvaluateArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Execute JavaScript on the current page and return the result as JSON."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(BrowserEvaluateArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}💻 browser evaluate{RESET}",
        );
        if args.expression.is_empty() {
            return Err(ToolError::Message("expression is required".to_string()));
        }

        let url = args.url.unwrap_or_else(|| {
            self.last_url
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_default()
        });
        if !url.is_empty() && !self.policy.is_allowed(&Action::WebFetch, &url) {
            return Err(ToolError::Message(format!(
                "browse access denied for: {}",
                url
            )));
        }
        let browser = Arc::clone(&self.browser);
        let expression = args.expression;

        let result = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("browser rt: {e}"))?;
                rt.block_on(async {
                    let mut page = browser
                        .new_page()
                        .await
                        .map_err(|e| format!("page: {e}"))?;

                    if !url.is_empty() {
                        page.goto(&url)
                            .await
                            .map_err(|e| format!("goto: {e}"))?;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }

                    let value = page.evaluate(&expression);
                    Ok(format!("{value}"))
                })}),
        ).await;

        match result {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(ToolError::Message(e)),
            Ok(Err(e)) => Err(ToolError::Message(e.to_string())),
            Err(_) => Err(ToolError::Message("browser timed out after 30s".to_string())),
        }
    }
}
