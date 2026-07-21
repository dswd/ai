use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, info};

use crate::policy::{Action, Policy};

#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchArgs {
    #[schemars(description = "The URL to fetch content from")]
    pub url: String,
    #[schemars(description = "The format to return the content in (text, markdown, or html)")]
    pub format: Option<String>,
    #[schemars(description = "Optional timeout in seconds (max 120)")]
    pub timeout: Option<u64>,
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
    type Error = WebError;

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
        info!("tool_call: web_fetch {{ url: {:?}, format: {:?} }}", args.url, args.format);
        if args.url.is_empty() {
            return Err(WebError::Message("URL is required".to_string()));
        }

        if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
            return Err(WebError::Message(
                "URL must start with http:// or https://".to_string(),
            ));
        }

        if !self
            .policy
            .is_allowed(&Action::WebFetch, &args.url)
        {
            return Err(WebError::Message(format!(
                "web fetch access denied for: {}",
                args.url
            )));
        }

        let format = args.format.as_deref().unwrap_or("markdown").to_lowercase();
        if format != "text" && format != "markdown" && format != "html" {
            return Err(WebError::Message(
                "format must be one of: text, markdown, html".to_string(),
            ));
        }

        let timeout_secs = args.timeout.unwrap_or(30).min(120);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent("ai-cli/1.0")
            .build()
            .map_err(|e| WebError::Message(format!("failed to create HTTP client: {e}")))?;

        let resp = client
            .get(&args.url)
            .send()
            .await
            .map_err(|e| WebError::Message(format!("failed to fetch URL: {e}")))?;

        if !resp.status().is_success() {
            return Err(WebError::Message(format!(
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
            .map_err(|e| WebError::Message(format!("failed to read response body: {e}")))?;

        const MAX_SIZE: usize = 5 * 1024 * 1024;
        if body.len() > MAX_SIZE {
            return Err(WebError::Message(format!(
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
                    body
                }
            }
            "markdown" => {
                if content_type.contains("text/html") {
                    htmd::convert(&body)
                        .map_err(|e| WebError::Message(format!("failed to convert HTML to markdown: {e}")))?
                } else {
                    format!("```\n{body}\n```")
                }
            }
            "html" => body,
            _ => body,
        };
        debug!("tool_response: web_fetch: {} bytes", result.len());
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchArgs {
    #[schemars(description = "The search query")]
    pub query: String,
    #[schemars(description = "Number of results to return (default: 10, max: 20)")]
    pub num_results: Option<usize>,
}

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
    type Error = WebError;

    fn description(&self) -> String {
        "Search the internet using DuckDuckGo. Returns search results with titles, URLs, and snippets."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(WebSearchArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!("tool_call: web_search {{ query: {:?} }}", args.query);
        if args.query.is_empty() {
            return Err(WebError::Message("query is required".to_string()));
        }

        if !self
            .policy
            .is_allowed(&Action::WebSearch, &args.query)
        {
            return Err(WebError::Message(format!(
                "web search access denied for: {}",
                args.query
            )));
        }

        let num_results = args.num_results.unwrap_or(10).min(20);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")
            .build()
            .map_err(|e| WebError::Message(format!("failed to create HTTP client: {e}")))?;

        let search_url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&args.query)
        );

        let resp = client
            .get(&search_url)
            .send()
            .await
            .map_err(|e| WebError::Message(format!("failed to send search request: {e}")))?;

        if !resp.status().is_success() {
            return Err(WebError::Message(format!(
                "search request failed with status code: {}",
                resp.status()
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| WebError::Message(format!("failed to read search response: {e}")))?;

        let document = scraper::Html::parse_document(&body);

        let result_links = scraper::Selector::parse(".result__a")
            .map_err(|e| WebError::Message(format!("failed to parse selector: {e}")))?;
        let result_snippets = scraper::Selector::parse(".result__snippet")
            .map_err(|e| WebError::Message(format!("failed to parse selector: {e}")))?;

        let links: Vec<_> = document.select(&result_links).collect();
        let snippets: Vec<_> = document.select(&result_snippets).collect();

        let mut results = Vec::new();
        let count = links.len().min(snippets.len()).min(num_results);

        for i in 0..count {
            let title = links[i].text().collect::<String>().trim().to_string();
            let url = links[i]
                .value()
                .attr("href")
                .unwrap_or("")
                .to_string();
            let snippet = snippets[i].text().collect::<String>().trim().to_string();

            if !url.is_empty() && !title.is_empty() {
                results.push(format!("{}. {}\n   URL: {}\n   {}\n", i + 1, title, url, snippet));
            }
        }

        let result = if results.is_empty() {
            "No results found.".to_string()
        } else {
            let header = format!(
                "Search results for \"{}\" ({} found):\n\n",
                args.query,
                results.len()
            );
            header + &results.join("\n")
        };
        debug!("tool_response: web_search: {} results", results.len());
        Ok(result)
    }
}
