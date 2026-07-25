use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use regex::Regex;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::shared::ToolError;
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchArgs {
    #[schemars(description = "The search query")]
    pub query: String,
    #[schemars(description = "Number of results to return (default: 10, max: 20)")]
    pub num_results: Option<usize>,
    #[schemars(description = "Line number to start reading from (0-based)")]
    pub offset: Option<usize>,
    #[schemars(description = "Maximum number of lines to return")]
    pub limit: Option<usize>,
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
    type Error = ToolError;

    fn description(&self) -> String {
        "Search the internet using DuckDuckGo. Returns search results with titles, URLs, and snippets."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(WebSearchArgs)).unwrap_or_default()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        info!(
            "{DIM}🌐 search web for {:?}{}{RESET}",
            args.query,
            fmt_offset_limit(args.offset, args.limit)
        );
        if args.query.is_empty() {
            return Err(ToolError::Message("query is required".to_string()));
        }

        if !self.policy.is_allowed(&Action::WebSearch, &args.query) {
            return Err(ToolError::Message(format!(
                "web search access denied for: {}",
                args.query
            )));
        }

        let num_results = args.num_results.unwrap_or(10).min(20);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")
            .build()
            .map_err(|e| ToolError::Message(format!("failed to create HTTP client: {e}")))?;

        let search_url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&args.query)
        );

        let resp = client
            .get(&search_url)
            .send()
            .await
            .map_err(|e| ToolError::Message(format!("failed to send search request: {e}")))?;

        if !resp.status().is_success() {
            return Err(ToolError::Message(format!(
                "search request failed with status code: {}",
                resp.status()
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| ToolError::Message(format!("failed to read search response: {e}")))?;

        let re_link = Regex::new(r#"class="result__a"\s+href="([^"]+)"[^>]*>([^<]+)"#)
            .map_err(|e| ToolError::Message(format!("regex error: {e}")))?;
        let re_snippet = Regex::new(r#"class="result__snippet"[^>]*>(.+?)</a>"#)
            .map_err(|e| ToolError::Message(format!("regex error: {e}")))?;

        let links: Vec<(&str, &str)> = re_link
            .captures_iter(&body)
            .map(|c| (c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str()))
            .collect();
        let snippets: Vec<&str> = re_snippet
            .captures_iter(&body)
            .map(|c| c.get(1).unwrap().as_str())
            .collect();

        let mut results = Vec::new();
        let count = links.len().min(snippets.len()).min(num_results);

        for i in 0..count {
            let url = links[i].0;
            let title = links[i].1.trim();
            let snippet = snippets[i].trim();

            if !url.is_empty() && !title.is_empty() {
                results.push(format!(
                    "{}. {}\n   URL: {}\n   {}\n",
                    i + 1,
                    title,
                    url,
                    snippet
                ));
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
        let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
        debug!(
            "{DIM} {} \n{truncated}\n {} {RESET}",
            bar_title("search results"),
            bar_line()
        );
        process_output(&result, args.offset, args.limit).map_err(ToolError::Message)
    }
}
