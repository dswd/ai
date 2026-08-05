use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[cfg(feature = "browser")]
use super::browser::BrowserState;
use super::shared::{ToolError, browser_headers, http_client};
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
    #[cfg(feature = "browser")]
    browser: Option<Arc<BrowserState>>,
}

#[cfg(feature = "browser")]
use std::sync::Arc;

impl WebFetchTool {
    #[cfg(not(feature = "browser"))]
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }

    #[cfg(feature = "browser")]
    pub fn with_browser(policy: Policy, browser: Option<Arc<BrowserState>>) -> Self {
        Self { policy, browser }
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

        let mut last_err = String::new();
        #[cfg(feature = "browser")]
        let mut block_signaled = false;
        for attempt in 0..2 {
            if attempt > 0 {
                debug!("{DIM}  retrying fetch (attempt 2)...{RESET}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            match fetch_url(&args.url, timeout_secs).await {
                Ok(body) => {
                    let result = convert_body(&body, &format)?;
                    let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
                    debug!(
                        "{DIM} {} \n{truncated}\n {} {RESET}",
                        bar_title(&args.url),
                        bar_line()
                    );
                    return process_output(&result, args.offset, args.limit)
                        .map_err(ToolError::Message);
                }
                Err(e) => {
                    #[cfg(feature = "browser")]
                    {
                        // Keep the flag if ANY attempt signaled a block; the
                        // last attempt may fail for an unrelated reason.
                        block_signaled |= is_block_error(&e);
                    }
                    last_err = e;
                }
            }
        }

        #[cfg(feature = "browser")]
        if block_signaled && let Some(ref bs) = self.browser {
            debug!("{DIM}  escalating to stealth browser...{RESET}");
            match bs.fetch_html(&args.url).await {
                Ok(html) => {
                    let result = convert_body(&html, &format)?;
                    let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
                    debug!(
                        "{DIM} {} \n{truncated}\n {} {RESET}",
                        bar_title(&args.url),
                        bar_line()
                    );
                    return process_output(&result, args.offset, args.limit)
                        .map_err(ToolError::Message);
                }
                Err(e) => {
                    last_err = format!("{last_err}; browser fallback failed: {e}");
                }
            }
        }

        Err(ToolError::Message(format!(
            "failed to fetch URL: {last_err}"
        )))
    }
}

#[cfg(feature = "browser")]
fn is_block_error(e: &str) -> bool {
    e.contains("blocked")
        || e.contains("Cloudflare")
        || e.contains("CAPTCHA")
        || e.contains("rate limited")
        || e.contains("HTTP 403")
        || e.contains("HTTP 503")
        || e.contains("empty body")
}

async fn fetch_url(url: &str, timeout_secs: u64) -> Result<String, String> {
    let resp = browser_headers(http_client().get(url))
        .timeout(Duration::from_secs(timeout_secs))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if resp.status().as_u16() == 429 {
        return Err("rate limited (HTTP 429). Try again in a few seconds.".to_string());
    }

    if resp.status().as_u16() == 403 || resp.status().as_u16() == 503 {
        return Err(format!("possibly blocked (HTTP {})", resp.status()));
    }

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| format!("failed to read response body: {e}"))?;

    if body.trim().is_empty() {
        return Err("empty body (possibly blocked)".to_string());
    }

    if let Some(marker) = block_marker(&body) {
        return Err(marker.to_string());
    }

    const MAX_SIZE: usize = 5 * 1024 * 1024;
    if body.len() > MAX_SIZE {
        return Err(format!(
            "response too large: {} bytes (max {})",
            body.len(),
            MAX_SIZE
        ));
    }

    Ok(body)
}

/// Detect anti-bot challenge markers in a response body. Returns a description
/// of the block type, or `None` if the body looks like a normal page.
fn block_marker(body: &str) -> Option<&'static str> {
    if body.contains("cf-browser-verification")
        || body.contains("Just a moment")
        || body.contains("Checking your browser")
        || body.contains("cf-chl")
    {
        return Some("URL is protected by Cloudflare and cannot be fetched automatically.");
    }

    if body.contains("g-recaptcha") || body.contains("recaptcha") {
        return Some("URL requires a CAPTCHA and cannot be fetched.");
    }

    if body.contains("Access denied") || body.contains("access denied") {
        return Some("URL returned 'Access denied'.");
    }

    None
}

fn convert_body(body: &str, format: &str) -> Result<String, ToolError> {
    let body_lower = body.trim_start().to_lowercase();
    let is_html = body_lower.starts_with("<!doctype html")
        || body_lower.starts_with("<html")
        || body_lower.contains("<head>")
        || body_lower.contains("<body")
        || body_lower.contains("<div")
        || body_lower.contains("<p>");

    let result = match format {
        "text" => {
            if is_html {
                html2text::from_read(body.as_bytes(), 80)
            } else {
                Ok(body.to_string())
            }
        }
        "markdown" => {
            if is_html {
                html2text::from_read(body.as_bytes(), 80)
            } else {
                Ok(format!("```\n{body}\n```"))
            }
        }
        "html" => Ok(body.to_string()),
        _ => Ok(body.to_string()),
    };

    result.map_err(|e| ToolError::Message(format!("failed to convert: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_marker_cloudflare() {
        for body in [
            "<html>Just a moment...</html>",
            "cf-browser-verification",
            "Checking your browser before accessing",
            "challenge platform cf-chl",
        ] {
            let marker = block_marker(body);
            assert!(marker.is_some(), "expected marker for: {body}");
            assert!(marker.unwrap().contains("Cloudflare"));
        }
    }

    #[test]
    fn test_block_marker_captcha() {
        for body in ["g-recaptcha", "recaptcha v2"] {
            let marker = block_marker(body);
            assert!(marker.is_some(), "expected marker for: {body}");
            assert!(marker.unwrap().contains("CAPTCHA"));
        }
    }

    #[test]
    fn test_block_marker_access_denied() {
        let marker = block_marker("Access denied. Please try again later.");
        assert!(marker.is_some());
    }

    #[test]
    fn test_block_marker_none() {
        for body in [
            "<html><head><title>ok</title></head><body><p>hello</p></body></html>",
            "plain text page",
        ] {
            assert!(
                block_marker(body).is_none(),
                "unexpected marker for: {body}"
            );
        }
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_is_block_error() {
        assert!(is_block_error("possibly blocked (HTTP 403)"));
        assert!(is_block_error("rate limited (HTTP 429)."));
        assert!(is_block_error("URL is protected by Cloudflare"));
        assert!(is_block_error("empty body (possibly blocked)"));
        assert!(is_block_error("URL requires a CAPTCHA"));
        assert!(!is_block_error("HTTP 404"));
        assert!(!is_block_error("request failed: connection reset"));
    }
}
