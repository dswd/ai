use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rand::RngExt;
use regex::Regex;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::shared::ToolError;
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::config::SearchConfig;
use crate::policy::{Action, Policy};

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0";

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
    search: SearchConfig,
    last_request: Arc<Mutex<Option<Instant>>>,
}

impl WebSearchTool {
    pub fn new(policy: Policy, search: SearchConfig) -> Self {
        Self {
            policy,
            search,
            last_request: Arc::new(Mutex::new(None)),
        }
    }
}

impl Tool for WebSearchTool {
    const NAME: &'static str = "web_search";

    type Args = WebSearchArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Search the internet and return results with titles, URLs, and snippets.".to_string()
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

        let _num_results = args.num_results.unwrap_or(10).min(20);
        self.rate_limit_wait().await;

        // 1. Try custom SearXNG (if configured)
        if let Some(url) = &self.search.searxng_url.as_ref().filter(|u| !u.is_empty()) {
            let search_url = url.replacen("{query}", &args.query, 1);
            debug!("{DIM}  trying SearXNG (custom){RESET}");
            match fetch(&search_url).await {
                Ok(body) => {
                    let md = html_to_markdown(&body);
                    match check_quality(&md) {
                        Ok(()) => {
                            let result = format!(
                                "Search results for \"{}\" via SearXNG:\n\n{}",
                                args.query, md
                            );
                            return finalize(result, args.offset, args.limit);
                        }
                        Err(reason) => debug!("{DIM}  SearXNG rejected: {reason}{RESET}"),
                    }
                }
                Err(e) => debug!("{DIM}  SearXNG: {e}{RESET}"),
            }
        }

        // 2. Try DuckDuckGo
        debug!("{DIM}  trying DuckDuckGo{RESET}");
        match self.search_ddg(&args.query).await {
            Ok(html) => {
                let md = html_to_markdown(&html);
                match check_quality(&md) {
                    Ok(()) => {
                        let result = format!(
                            "Search results for \"{}\" via DuckDuckGo:\n\n{}",
                            args.query, md
                        );
                        return finalize(result, args.offset, args.limit);
                    }
                    Err(reason) => debug!("{DIM}  DuckDuckGo rejected: {reason}{RESET}"),
                }
            }
            Err(e) => debug!("{DIM}  DuckDuckGo: {e}{RESET}"),
        }

        // 3. Try Google
        debug!("{DIM}  trying Google{RESET}");
        match self.search_google(&args.query).await {
            Ok(html) => {
                let md = html_to_markdown(&html);
                match check_quality(&md) {
                    Ok(()) => {
                        let result = format!(
                            "Search results for \"{}\" via Google:\n\n{}",
                            args.query, md
                        );
                        return finalize(result, args.offset, args.limit);
                    }
                    Err(reason) => debug!("{DIM}  Google rejected: {reason}{RESET}"),
                }
            }
            Err(e) => debug!("{DIM}  Google: {e}{RESET}"),
        }

        // 4. Try Bing
        debug!("{DIM}  trying Bing{RESET}");
        match self.search_bing(&args.query).await {
            Ok(html) => {
                let md = html_to_markdown(&html);
                match check_quality(&md) {
                    Ok(()) => {
                        let result =
                            format!("Search results for \"{}\" via Bing:\n\n{}", args.query, md);
                        return finalize(result, args.offset, args.limit);
                    }
                    Err(reason) => debug!("{DIM}  Bing rejected: {reason}{RESET}"),
                }
            }
            Err(e) => debug!("{DIM}  Bing: {e}{RESET}"),
        }

        Err(ToolError::Message(
            "Search failed. All engines returned no results. Try rephrasing your query."
                .to_string(),
        ))
    }
}

impl WebSearchTool {
    async fn rate_limit_wait(&self) {
        let wait = {
            let last = self.last_request.lock().unwrap();
            if let Some(t) = *last {
                let elapsed = t.elapsed();
                if elapsed < Duration::from_secs(2) {
                    let jitter = rand::rng().random_range(1..=5);
                    Some(Duration::from_secs(jitter))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(wait) = wait {
            debug!(
                "{DIM}  rate limit: sleeping {:.1}s{RESET}",
                wait.as_secs_f64()
            );
            tokio::time::sleep(wait).await;
        }
        *self.last_request.lock().unwrap() = Some(Instant::now());
    }

    async fn search_ddg(&self, query: &str) -> Result<String, String> {
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );
        fetch(&url).await.map_err(|e| format!("DDG: {e}"))
    }

    #[cfg(feature = "browser")]
    async fn search_google(&self, query: &str) -> Result<String, String> {
        let query = query.to_string();
        let query_escaped = query.replace('\\', "\\\\").replace('\'', "\\'");
        tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("browser rt: {e}"))?;
                rt.block_on(async {
                    let browser = obscura::Browser::builder()
                        .stealth(true)
                        .build()
                        .map_err(|e| format!("browser: {e}"))?;
                    let mut page = browser
                        .new_page()
                        .await
                        .map_err(|e| format!("page: {e}"))?;

                    page.goto("https://www.google.com")
                        .await
                        .map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(800)).await;

                    page.evaluate(
                        r#"(function(){var b=document.querySelectorAll('button,[role="button"]');for(var i=0;i<b.length;i++){var t=b[i].textContent.trim().toLowerCase();if(/^(accept all|accept|i agree|agree|ok|yes)$/i.test(t)){b[i].click();break;}}})()"#,
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    page.evaluate(&format!(
                        r#"(function(){{var i=document.querySelector('input[name="q"],textarea[name="q"],input[type="search"]');if(i){{i.focus();i.value='{query_escaped}';i.dispatchEvent(new Event('input',{{bubbles:true}}));i.dispatchEvent(new KeyboardEvent('keydown',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}}));i.dispatchEvent(new KeyboardEvent('keyup',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}}));}}}})()"#
                    ));

                    tokio::time::sleep(Duration::from_millis(2500)).await;

                    let raw = page.evaluate(
                        r#"(function(){var l=document.querySelectorAll('a[href^="http"]');if(!l.length)return'';var c=new Map();for(var i=0;i<l.length;i++){var e=l[i];for(var j=0;j<4;j++){e=e.parentElement;if(!e)break}if(e)c.set(e,(c.get(e)||0)+1)}var b=null,n=0;c.forEach(function(v,k){if(v>n){b=k;n=v}});return b&&n>3?b.innerHTML:''})()"#,
                    );
                    let html = raw.as_str().unwrap_or("").to_string();
                    if html.is_empty() {
                        return Err("no results area found".to_string());
                    }
                    Ok(html)
                })
            }),
        )
        .await
        .map_err(|_| "search timed out after 30s".to_string())?
        .map_err(|e| e.to_string())?
    }

    #[cfg(not(feature = "browser"))]
    async fn search_google(&self, _query: &str) -> Result<String, String> {
        Err("Google search requires the browser feature".to_string())
    }

    #[cfg(feature = "browser")]
    async fn search_bing(&self, query: &str) -> Result<String, String> {
        let query = query.to_string();
        let query_escaped = query.replace('\\', "\\\\").replace('\'', "\\'");
        debug!("{DIM}  Obscura Bing: spawning browser...{RESET}");
        tokio::time::timeout(
            Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("browser rt: {e}"))?;
                rt.block_on(async {
                    let browser = obscura::Browser::builder()
                        .stealth(true)
                        .build()
                        .map_err(|e| format!("browser: {e}"))?;
                    let mut page = browser
                        .new_page()
                        .await
                        .map_err(|e| format!("page: {e}"))?;

                    page.goto("https://www.bing.com")
                        .await
                        .map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(800)).await;

                    page.evaluate(
                        r#"(function(){var b=document.querySelectorAll('button,[role="button"]');for(var i=0;i<b.length;i++){var t=b[i].textContent.trim().toLowerCase();if(/^(accept all|accept|i agree|agree|ok|yes)$/i.test(t)){b[i].click();break;}}})()"#,
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    page.evaluate(&format!(
                        r#"(function(){{var i=document.querySelector('input[name="q"],textarea[name="q"],input[type="search"]');if(i){{i.focus();i.value='{query_escaped}';i.dispatchEvent(new Event('input',{{bubbles:true}}));i.dispatchEvent(new KeyboardEvent('keydown',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}}));i.dispatchEvent(new KeyboardEvent('keyup',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}}));}}}})()"#
                    ));

                    tokio::time::sleep(Duration::from_millis(2500)).await;

                    let raw = page.evaluate(
                        r#"(function(){var l=document.querySelectorAll('a[href^="http"]');if(!l.length)return'';var c=new Map();for(var i=0;i<l.length;i++){var e=l[i];for(var j=0;j<4;j++){e=e.parentElement;if(!e)break}if(e)c.set(e,(c.get(e)||0)+1)}var b=null,n=0;c.forEach(function(v,k){if(v>n){b=k;n=v}});return b&&n>3?b.innerHTML:''})()"#,
                    );
                    let html = raw.as_str().unwrap_or("").to_string();
                    if html.is_empty() {
                        return Err("no results area found".to_string());
                    }
                    Ok(html)
                })
            }),
        )
        .await
        .map_err(|_| "search timed out after 30s".to_string())?
        .map_err(|e| e.to_string())?
    }

    #[cfg(not(feature = "browser"))]
    async fn search_bing(&self, _query: &str) -> Result<String, String> {
        Err("Bing search requires the browser feature".to_string())
    }
}

// ----- HTML to markdown conversion -----

pub(crate) fn html_to_markdown(html: &str) -> String {
    let re_block = Regex::new(r"(?s)<style[^>]*>.*?</style>|<script[^>]*>.*?</script>").unwrap();
    let cleaned = re_block.replace_all(html, "").to_string();
    let md = html2md::parse_html(&cleaned);
    let re_tag = Regex::new(r"<[^>]+>").unwrap();
    re_tag.replace_all(&md, "").to_string()
}

fn check_quality(md: &str) -> Result<(), String> {
    if md.trim().is_empty() {
        return Err("empty output".to_string());
    }

    let link_count = md.matches("](").count();
    if link_count < 5 {
        return Err(format!("too few links: {link_count}"));
    }

    let lower = md.to_lowercase();
    let bad = ["unusual traffic", "captcha", "blocked"];
    let total_bad: usize = bad.iter().map(|w| lower.matches(w).count()).sum();
    if total_bad > 2 {
        return Err(format!("bad words: {total_bad}"));
    }

    Ok(())
}

fn finalize(
    result: String,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, ToolError> {
    let truncated = truncate(&result, MAX_OUTPUT_LINES, MAX_OUTPUT_CHARS);
    debug!(
        "{DIM} {} \n{truncated}\n {} {RESET}",
        bar_title("search results"),
        bar_line()
    );
    process_output(&result, offset, limit).map_err(ToolError::Message)
}

// ----- HTTP fetch -----

async fn fetch(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(UA)
        .build()
        .map_err(|e| format!("client: {e}"))?;

    let resp = client
        .get(url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.5")
        .header("Accept-Encoding", "gzip, deflate")
        .header("DNT", "1")
        .header("Upgrade-Insecure-Requests", "1")
        .send()
        .await
        .map_err(|e| format!("request: {e}"))?;

    if resp.status().as_u16() == 429 {
        return Err("HTTP 429 (rate limited)".to_string());
    }

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    resp.text().await.map_err(|e| format!("read: {e}"))
}
