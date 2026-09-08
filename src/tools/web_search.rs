use ansi_color_constants::*;
use log::{debug, info};
use rand::RngExt;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(feature = "browser")]
use super::browser_state::BrowserState;
use super::fmt_offset_limit;
use super::search_html::{check_quality, fetch, html_to_markdown, searxng_search_url};
use super::shared::ToolError;
use crate::config::SearchConfig;
use crate::policy::{Action, Policy};

/// Minimum time between two requests to the same engine.
const ENGINE_MIN_INTERVAL: Duration = Duration::from_secs(3);
/// Cooldown after a transient failure (network, timeout, rate limit).
const TRANSIENT_COOLDOWN: Duration = Duration::from_secs(20);
/// Cooldown after an engine returns a block page (CAPTCHA, Cloudflare, ...).
const BLOCK_COOLDOWN: Duration = Duration::from_secs(60);
/// Extra attempts per engine after the first try (backoff 2s per retry).
const ENGINE_RETRIES: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Structured failure from a search engine, used to decide whether to retry
/// and how long to cool the engine down.
#[derive(Debug)]
pub(super) enum EngineError {
    NotConfigured,
    Fetch(String),
    Quality(String),
}

impl EngineError {
    pub(super) fn detail(&self) -> String {
        match self {
            EngineError::NotConfigured => "not configured".to_string(),
            EngineError::Fetch(e) => format!("fetch: {e}"),
            EngineError::Quality(e) => format!("quality: {e}"),
        }
    }

    /// Errors worth one retry: anything that reached the network and failed
    /// (timeouts, rate limits, 403/503, browser failures). Configuration and
    /// quality failures are treated as permanent for this call.
    fn is_transient(&self) -> bool {
        matches!(self, EngineError::Fetch(_))
    }

    /// Errors that indicate the engine is blocking us (CAPTCHA, Cloudflare,
    /// challenge pages). These get a longer cooldown so the ladder skips the
    /// engine for a while instead of hammering it.
    fn is_block(&self) -> bool {
        matches!(self, EngineError::Quality(e)
            if e.contains("blocked marker") || e.contains("no results area found"))
    }
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
    proxy: Option<String>,
    last_request: Arc<Mutex<Option<Instant>>>,
    engine_last: Arc<Mutex<HashMap<SearchEngine, Instant>>>,
    engine_cooldown: Arc<Mutex<HashMap<SearchEngine, Instant>>>,
    #[cfg(feature = "browser")]
    pub(super) browser: Option<Arc<BrowserState>>,
}

impl WebSearchTool {
    #[cfg(not(feature = "browser"))]
    pub fn new(policy: Policy, search: SearchConfig, proxy: Option<String>) -> Self {
        Self {
            policy,
            search,
            proxy,
            last_request: Arc::new(Mutex::new(None)),
            engine_last: Arc::new(Mutex::new(HashMap::new())),
            engine_cooldown: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(feature = "browser")]
    pub fn with_browser(
        policy: Policy,
        search: SearchConfig,
        proxy: Option<String>,
        browser: Option<Arc<BrowserState>>,
    ) -> Self {
        Self {
            policy,
            search,
            proxy,
            last_request: Arc::new(Mutex::new(None)),
            engine_last: Arc::new(Mutex::new(HashMap::new())),
            engine_cooldown: Arc::new(Mutex::new(HashMap::new())),
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
            if self.engine_cooling_down(engine) {
                debug!("{DIM}  skipping {} (cooldown){RESET}", engine.name());
                continue;
            }
            match self.run_engine_with_retry(engine, &args.query).await {
                Ok(md) => {
                    let result = format!(
                        "Search results for \"{}\" via {}:\n\n{}",
                        args.query,
                        engine.name(),
                        md
                    );
                    return super::search_html::finalize(result, args.offset, args.limit);
                }
                Err(reason) => {
                    self.record_engine_failure(engine, &reason);
                    debug!(
                        "{DIM}  {} rejected: {}{RESET}",
                        engine.name(),
                        reason.detail()
                    );
                }
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

    /// Whether `engine` is on cooldown and should be skipped in the ladder.
    fn engine_cooling_down(&self, engine: SearchEngine) -> bool {
        self.engine_cooldown
            .lock()
            .unwrap()
            .get(&engine)
            .map(|t| *t > Instant::now())
            .unwrap_or(false)
    }

    /// Wait out the minimum interval between requests to the same engine.
    async fn wait_for_engine(&self, engine: SearchEngine) {
        let wait = {
            let map = self.engine_last.lock().unwrap();
            map.get(&engine).and_then(|t| {
                let elapsed = t.elapsed();
                if elapsed < ENGINE_MIN_INTERVAL {
                    Some(ENGINE_MIN_INTERVAL - elapsed)
                } else {
                    None
                }
            })
        };
        if let Some(wait) = wait {
            debug!(
                "{DIM}  rate limit: sleeping {:.1}s before {}{RESET}",
                wait.as_secs_f64(),
                engine.name()
            );
            tokio::time::sleep(wait).await;
        }
    }

    fn mark_engine_hit(&self, engine: SearchEngine) {
        self.engine_last
            .lock()
            .unwrap()
            .insert(engine, Instant::now());
    }

    /// Put an engine on cooldown after a failure. Transient errors get a short
    /// cooldown; block pages a long one; configuration errors none (they will
    /// never start working in this process).
    fn record_engine_failure(&self, engine: SearchEngine, err: &EngineError) {
        let cooldown = if err.is_block() {
            BLOCK_COOLDOWN
        } else if err.is_transient() {
            TRANSIENT_COOLDOWN
        } else {
            return;
        };
        debug!(
            "{DIM}  putting {} on cooldown for {}s{RESET}",
            engine.name(),
            cooldown.as_secs()
        );
        self.engine_cooldown
            .lock()
            .unwrap()
            .insert(engine, Instant::now() + cooldown);
    }

    /// Run one engine, retrying transient failures with a 2s backoff.
    /// Returns the final result text or the last error.
    async fn run_engine_with_retry(
        &self,
        engine: SearchEngine,
        query: &str,
    ) -> Result<String, EngineError> {
        let mut last_err = None;
        for attempt in 0..=ENGINE_RETRIES {
            self.wait_for_engine(engine).await;
            if attempt > 0 {
                let backoff = Duration::from_secs(2 * attempt as u64);
                debug!(
                    "{DIM}  retrying {} (attempt {}/{})...{RESET}",
                    engine.name(),
                    attempt + 1,
                    ENGINE_RETRIES + 1
                );
                tokio::time::sleep(backoff).await;
            }
            self.mark_engine_hit(engine);
            match self.run_engine(engine, query).await {
                Ok(md) => return Ok(md),
                Err(e) if e.is_transient() => last_err = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or(EngineError::Fetch("unknown error".to_string())))
    }

    /// Run a single engine and return the final result text (or a structured
    /// error). Shared between the normal ladder and `--probe-web`.
    pub(super) async fn run_engine(
        &self,
        engine: SearchEngine,
        query: &str,
    ) -> Result<String, EngineError> {
        match engine {
            SearchEngine::Searxng => {
                let Some(url) = self.search.searxng_url.as_ref().filter(|u| !u.is_empty()) else {
                    return Err(EngineError::NotConfigured);
                };
                let search_url = searxng_search_url(url, query);
                let body = fetch(&search_url, self.proxy.as_deref())
                    .await
                    .map_err(EngineError::Fetch)?;
                let md = html_to_markdown(&body);
                check_quality(&md).map_err(EngineError::Quality)?;
                Ok(md)
            }
            SearchEngine::DuckDuckGo => {
                let html = self.search_ddg(query).await.map_err(EngineError::Fetch)?;
                let md = html_to_markdown(&html);
                check_quality(&md).map_err(EngineError::Quality)?;
                Ok(md)
            }
            SearchEngine::Google => {
                let html = self.search_google(query).await.map_err(|e| {
                    if e.starts_with("blocked marker:") || e.starts_with("no results area found") {
                        EngineError::Quality(e)
                    } else {
                        EngineError::Fetch(e)
                    }
                })?;
                let md = html_to_markdown(&html);
                check_quality(&md).map_err(EngineError::Quality)?;
                Ok(md)
            }
            SearchEngine::Bing => {
                let html = self.search_bing(query).await.map_err(|e| {
                    if e.starts_with("blocked marker:") || e.starts_with("no results area found") {
                        EngineError::Quality(e)
                    } else {
                        EngineError::Fetch(e)
                    }
                })?;
                let md = html_to_markdown(&html);
                check_quality(&md).map_err(EngineError::Quality)?;
                Ok(md)
            }
        }
    }

    async fn search_ddg(&self, query: &str) -> Result<String, String> {
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );
        fetch(&url, self.proxy.as_deref())
            .await
            .map_err(|e| format!("DDG: {e}"))
    }

    #[cfg(feature = "browser")]
    async fn search_google(&self, query: &str) -> Result<String, String> {
        self.browser_search(
            "https://www.google.com",
            r#"textarea[name="q"], input[name="q"]"#,
            &["#rso", "#search", "#main"],
            query,
        )
        .await
    }

    #[cfg(not(feature = "browser"))]
    async fn search_google(&self, _query: &str) -> Result<String, String> {
        Err("Google search requires the browser feature".to_string())
    }

    #[cfg(feature = "browser")]
    async fn search_bing(&self, query: &str) -> Result<String, String> {
        self.browser_search(
            "https://www.bing.com",
            r#"#sb_form_q, input[name="q"]"#,
            &["#b_results", ".b_algo"],
            query,
        )
        .await
    }

    #[cfg(not(feature = "browser"))]
    async fn search_bing(&self, _query: &str) -> Result<String, String> {
        Err("Bing search requires the browser feature".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_names() {
        assert_eq!(SearchEngine::Searxng.name(), "SearXNG");
        assert_eq!(SearchEngine::DuckDuckGo.name(), "DuckDuckGo");
        assert_eq!(SearchEngine::Google.name(), "Google");
        assert_eq!(SearchEngine::Bing.name(), "Bing");
    }

    #[test]
    fn test_engine_error_classification() {
        assert!(
            EngineError::NotConfigured
                .detail()
                .contains("not configured")
        );
        assert!(!EngineError::NotConfigured.is_transient());
        assert!(!EngineError::NotConfigured.is_block());

        let fetch = EngineError::Fetch("request: timeout".to_string());
        assert!(fetch.is_transient());
        assert!(!fetch.is_block());
        assert!(fetch.detail().starts_with("fetch: "));

        let quality_block = EngineError::Quality("blocked marker: captcha".to_string());
        assert!(!quality_block.is_transient());
        assert!(quality_block.is_block());
        assert!(quality_block.detail().starts_with("quality: "));

        let quality_no_results = EngineError::Quality("no results area found".to_string());
        assert!(quality_no_results.is_block());

        let quality_no_results_rich = EngineError::Quality(
            "no results area found (url=https://google.com, title=\"x\", bytes=100, body=\"...\")"
                .to_string(),
        );
        assert!(quality_no_results_rich.is_block());
        assert!(quality_no_results_rich.detail().starts_with("quality: "));

        let quality_few_links = EngineError::Quality("too few links: 2".to_string());
        assert!(!quality_few_links.is_block());
    }
}
