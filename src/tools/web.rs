use ansi_color_constants::*;
use log::info;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{finalize_output, ToolError};
use crate::policy::{Action, Policy};

// ---------------------------------------------------------------------------
// Args structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchArgs {
    pub url: String,
    pub format: Option<String>,
    pub timeout: Option<u64>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchArgs {
    pub query: String,
    pub num_results: Option<usize>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// WebFetchTool
// ---------------------------------------------------------------------------

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
        "Fetch a URL (text/markdown/html, max 5MB)".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(WebFetchArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🌐 fetch {}{RESET}", args.url);
        if args.url.is_empty() {
            return Err(ToolError::Message("URL is required".to_string()));
        }
        if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
            return Err(ToolError::Message("URL must start with http:// or https://".to_string()));
        }
        if !self.policy.is_allowed(&Action::WebFetch, &args.url) {
            return Err(ToolError::Message(format!("web fetch denied for: {}", args.url)));
        }

        let format = args.format.as_deref().unwrap_or("markdown").to_lowercase();
        if !matches!(format.as_str(), "text" | "markdown" | "html") {
            return Err(ToolError::Message("format must be text, markdown, or html".to_string()));
        }

        let timeout_secs = args.timeout.unwrap_or(30).min(120);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent("ai-cli/1.0")
            .build()
            .map_err(|e| ToolError::Message(format!("HTTP client: {e}")))?;

        let resp = client
            .get(&args.url)
            .send()
            .await
            .map_err(|e| ToolError::Message(format!("fetch failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(ToolError::Message(format!(
                "HTTP {}",
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
            .map_err(|e| ToolError::Message(format!("read body failed: {e}")))?;

        const MAX_SIZE: usize = 5 * 1024 * 1024;
        if body.len() > MAX_SIZE {
            return Err(ToolError::Message(format!(
                "response too large: {} bytes (max {MAX_SIZE})",
                body.len()
            )));
        }

        let result = match format.as_str() {
            "text" => {
                if content_type.contains("text/html") {
                    html2text::from_read(body.as_bytes(), 80)
                } else {
                    body
                }
            }
            "markdown" => {
                if content_type.contains("text/html") {
                    htmd::convert(&body)
                        .map_err(|e| ToolError::Message(format!("html→md failed: {e}")))?
                } else {
                    format!("```\n{body}\n```")
                }
            }
            "html" => body,
            _ => body,
        };

        finalize_output(&result, args.offset, args.limit, &args.url)
    }
}

// ---------------------------------------------------------------------------
// WebSearchTool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WebSearchTool {
    policy: Policy,
}

impl WebSearchTool {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Tool for WebSearchTool {
    const NAME: &'static str = "web_search";

    type Args = WebSearchArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Search the web via DuckDuckGo".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(WebSearchArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("{DIM}🌐 search {:?}{RESET}", args.query);
        if args.query.is_empty() {
            return Err(ToolError::Message("query is required".to_string()));
        }
        if !self.policy.is_allowed(&Action::WebSearch, &args.query) {
            return Err(ToolError::Message(format!("web search denied for: {}", args.query)));
        }

        let num_results = args.num_results.unwrap_or(10).min(20);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")
            .build()
            .map_err(|e| ToolError::Message(format!("HTTP client: {e}")))?;

        let search_url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&args.query)
        );

        let resp = client
            .get(&search_url)
            .send()
            .await
            .map_err(|e| ToolError::Message(format!("search request failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(ToolError::Message(format!(
                "search HTTP {}",
                resp.status()
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| ToolError::Message(format!("read response failed: {e}")))?;

        let document = scraper::Html::parse_document(&body);
        let link_sel = scraper::Selector::parse(".result__a")
            .map_err(|e| ToolError::Message(format!("parse selector: {e}")))?;
        let snippet_sel = scraper::Selector::parse(".result__snippet")
            .map_err(|e| ToolError::Message(format!("parse selector: {e}")))?;

        let links: Vec<_> = document.select(&link_sel).collect();
        let snippets: Vec<_> = document.select(&snippet_sel).collect();

        let mut results = Vec::new();
        let count = links.len().min(snippets.len()).min(num_results);

        for i in 0..count {
            let title = links[i].text().collect::<String>().trim().to_string();
            let url = links[i].value().attr("href").unwrap_or("").to_string();
            let snippet = snippets[i].text().collect::<String>().trim().to_string();
            if !url.is_empty() && !title.is_empty() {
                results.push(format!("{}. {}\n   URL: {}\n   {}\n", i + 1, title, url, snippet));
            }
        }

        let result = if results.is_empty() {
            "No results.".to_string()
        } else {
            format!(
                "Results for {:?} ({} found):\n\n{}",
                args.query,
                results.len(),
                results.join("\n")
            )
        };

        finalize_output(&result, args.offset, args.limit, "web search")
    }
}
