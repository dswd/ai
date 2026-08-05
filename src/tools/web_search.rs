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

#[cfg(feature = "browser")]
use super::browser::BrowserState;
#[cfg(feature = "browser")]
use super::shared::js_literal;
use super::shared::{ToolError, browser_headers, http_client};
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, fmt_offset_limit, process_output, truncate};
use crate::config::SearchConfig;
use crate::policy::{Action, Policy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEngine {
    Searxng,
    DuckDuckGo,
    Google,
    Bing,
}

impl SearchEngine {
    pub fn name(&self) -> &'static str {
        match self {
            SearchEngine::Searxng => "SearXNG",
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Google => "Google",
            SearchEngine::Bing => "Bing",
        }
    }

    pub const ALL: [SearchEngine; 4] = [
        SearchEngine::Searxng,
        SearchEngine::DuckDuckGo,
        SearchEngine::Google,
        SearchEngine::Bing,
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchArgs {
    #[schemars(description = "The search query")]
    pub query: String,
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
    #[cfg(feature = "browser")]
    browser: Option<Arc<BrowserState>>,
}

impl WebSearchTool {
    #[cfg(not(feature = "browser"))]
    pub fn new(policy: Policy, search: SearchConfig) -> Self {
        Self {
            policy,
            search,
            last_request: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(feature = "browser")]
    pub fn with_browser(
        policy: Policy,
        search: SearchConfig,
        browser: Option<Arc<BrowserState>>,
    ) -> Self {
        Self {
            policy,
            search,
            last_request: Arc::new(Mutex::new(None)),
            browser,
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

        self.rate_limit_wait().await;

        for engine in SearchEngine::ALL {
            debug!("{DIM}  trying {}{RESET}", engine.name());
            match self.run_engine(engine, &args.query).await {
                Ok(md) => {
                    let result = format!(
                        "Search results for \"{}\" via {}:\n\n{}",
                        args.query,
                        engine.name(),
                        md
                    );
                    return finalize(result, args.offset, args.limit);
                }
                Err(reason) => debug!("{DIM}  {} rejected: {reason}{RESET}", engine.name()),
            }
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
                    let jitter = rand::rng().random_range(1..=6);
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

    /// Run a single engine and return the final result text (or an error with
    /// the rejection reason). Shared between the normal ladder and `--probe-web`.
    async fn run_engine(&self, engine: SearchEngine, query: &str) -> Result<String, String> {
        match engine {
            SearchEngine::Searxng => {
                let Some(url) = self.search.searxng_url.as_ref().filter(|u| !u.is_empty()) else {
                    return Err("not configured".to_string());
                };
                let search_url = url.replacen("{query}", query, 1);
                let body = fetch(&search_url)
                    .await
                    .map_err(|e| format!("fetch: {e}"))?;
                let md = html_to_markdown(&body);
                check_quality(&md).map_err(|e| format!("quality: {e}"))?;
                Ok(md)
            }
            SearchEngine::DuckDuckGo => {
                let html = self
                    .search_ddg(query)
                    .await
                    .map_err(|e| format!("fetch: {e}"))?;
                let md = html_to_markdown(&html);
                check_quality(&md).map_err(|e| format!("quality: {e}"))?;
                Ok(md)
            }
            SearchEngine::Google => {
                let html = self
                    .search_google(query)
                    .await
                    .map_err(|e| format!("fetch: {e}"))?;
                let md = html_to_markdown(&html);
                check_quality(&md).map_err(|e| format!("quality: {e}"))?;
                Ok(md)
            }
            SearchEngine::Bing => {
                let html = self
                    .search_bing(query)
                    .await
                    .map_err(|e| format!("fetch: {e}"))?;
                let md = html_to_markdown(&html);
                check_quality(&md).map_err(|e| format!("quality: {e}"))?;
                Ok(md)
            }
        }
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
        let browser = match &self.browser {
            Some(bs) => bs.browser(),
            None => Arc::new(
                obscura::Browser::builder()
                    .stealth(true)
                    .build()
                    .map_err(|e| format!("browser: {e}"))?,
            ),
        };
        let query = query.to_string();
        let query_escaped = js_literal(&query);
        tokio::time::timeout(
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

                    page.goto("https://www.google.com")
                        .await
                        .map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(800)).await;

                    page.evaluate(
                        r#"(function(){var b=document.querySelectorAll('button,[role="button"]');for(var i=0;i<b.length;i++){var t=b[i].textContent.trim().toLowerCase();if(/^(accept all|accept|i agree|agree|ok|yes)$/i.test(t)){b[i].click();break;}}})()"#,
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    page.evaluate(&format!(
                        r#"(function(){{var i=document.querySelector('input[name="q"],textarea[name="q"],input[type="search"]');if(i){{i.focus();i.value={query_escaped};i.dispatchEvent(new Event('input',{{bubbles:true}}));i.dispatchEvent(new KeyboardEvent('keydown',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}}));i.dispatchEvent(new KeyboardEvent('keyup',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}}));}}}})()"#
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
        let browser = match &self.browser {
            Some(bs) => bs.browser(),
            None => Arc::new(
                obscura::Browser::builder()
                    .stealth(true)
                    .build()
                    .map_err(|e| format!("browser: {e}"))?,
            ),
        };
        let query = query.to_string();
        let query_escaped = js_literal(&query);
        tokio::time::timeout(
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

                    page.goto("https://www.bing.com")
                        .await
                        .map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(800)).await;

                    page.evaluate(
                        r#"(function(){var b=document.querySelectorAll('button,[role="button"]');for(var i=0;i<b.length;i++){var t=b[i].textContent.trim().toLowerCase();if(/^(accept all|accept|i agree|agree|ok|yes)$/i.test(t)){b[i].click();break;}}})()"#,
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    page.evaluate(&format!(
                        r#"(function(){{var i=document.querySelector('input[name="q"],textarea[name="q"],input[type="search"]');if(i){{i.focus();i.value={query_escaped};i.dispatchEvent(new Event('input',{{bubbles:true}}));i.dispatchEvent(new KeyboardEvent('keydown',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}}));i.dispatchEvent(new KeyboardEvent('keyup',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}}));}}}})()"#
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

/// Result of probing a single search engine via `--probe-web`.
pub struct ProbeResult {
    pub engine: &'static str,
    pub ok: bool,
    pub latency_ms: u64,
    pub bytes: usize,
    pub detail: String,
}

/// Probe every engine in the ladder and report per-engine diagnostics, without
/// taking the policy into account. Used by the hidden `--probe-web` flag.
#[allow(unused_variables)]
pub async fn probe_web_search(
    query: &str,
    search: &SearchConfig,
    #[cfg(feature = "browser")] browser: Option<Arc<BrowserState>>,
) -> Vec<ProbeResult> {
    #[cfg(feature = "browser")]
    let tool = WebSearchTool::with_browser(Policy::default(), search.clone(), browser);
    #[cfg(not(feature = "browser"))]
    let tool = WebSearchTool::new(Policy::default(), search.clone());

    let mut results = Vec::new();
    for engine in SearchEngine::ALL {
        let start = Instant::now();
        let outcome = tool.run_engine(engine, query).await;
        let latency_ms = start.elapsed().as_millis() as u64;
        match outcome {
            Ok(md) => results.push(ProbeResult {
                engine: engine.name(),
                ok: true,
                latency_ms,
                bytes: md.len(),
                detail: "ok".to_string(),
            }),
            Err(reason) => results.push(ProbeResult {
                engine: engine.name(),
                ok: false,
                latency_ms,
                bytes: 0,
                detail: reason,
            }),
        }
    }
    results
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

    let lower = md.to_lowercase();
    let bad = [
        "unusual traffic",
        "captcha",
        "blocked",
        "verify you are human",
        "enable javascript",
        "access denied",
        "cf-chl",
        "turning on",
        "g-recaptcha",
        "recaptcha",
        "just a moment",
    ];
    if let Some(word) = bad.iter().find(|w| lower.contains(**w)) {
        return Err(format!("blocked marker: {word}"));
    }

    let link_count = md.matches("](").count();
    if link_count < 5 {
        return Err(format!("too few links: {link_count}"));
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
    let resp = browser_headers(http_client().get(url))
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| format!("request: {e}"))?;

    if resp.status().as_u16() == 429 {
        return Err("HTTP 429 (rate limited)".to_string());
    }

    if resp.status().as_u16() == 403 || resp.status().as_u16() == 503 {
        return Err(format!("possibly blocked (HTTP {})", resp.status()));
    }

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    resp.text().await.map_err(|e| format!("read: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_quality_empty() {
        assert!(check_quality("").is_err());
        assert!(check_quality("   \n  ").is_err());
    }

    #[test]
    fn test_check_quality_too_few_links() {
        assert!(check_quality("just one [link](https://x.com)").is_err());
    }

    #[test]
    fn test_check_quality_blocked_markers() {
        for body in [
            "unusual traffic from your computer network",
            "captcha required",
            "access denied",
            "verify you are human",
            "please enable javascript",
            "cf-chl challenge",
            "g-recaptcha",
            "just a moment...",
        ] {
            let md = format!(
                "[a](https://a.com) [b](https://b.com) [c](https://c.com) [d](https://d.com) [e](https://e.com) {body}"
            );
            let err = check_quality(&md).unwrap_err();
            assert!(err.contains("blocked marker"), "unexpected err: {err}");
        }
    }

    #[test]
    fn test_check_quality_ok() {
        let md = "[a](https://a.com) [b](https://b.com) [c](https://c.com) [d](https://d.com) [e](https://e.com)";
        assert!(check_quality(md).is_ok());
    }

    #[test]
    fn test_engine_names() {
        assert_eq!(SearchEngine::Searxng.name(), "SearXNG");
        assert_eq!(SearchEngine::DuckDuckGo.name(), "DuckDuckGo");
        assert_eq!(SearchEngine::Google.name(), "Google");
        assert_eq!(SearchEngine::Bing.name(), "Bing");
    }
}
