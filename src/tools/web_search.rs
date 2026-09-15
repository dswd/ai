use ansi_color_constants::*;
use log::{debug, info};
use rand::RngExt;
use rig::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(feature = "browser")]
use super::browser_state::BrowserState;
use super::fmt_offset_limit;
use super::search_api::{
    EngineError, SearchResult, brave, exa, fetch_json, parse_brave, parse_exa, parse_searxng,
    parse_serper, parse_tavily, parse_tavily_answer, render_results, searxng, serper, tavily,
};
use super::search_html::{check_quality, fetch, html_to_markdown, searxng_search_url};
use super::shared::ToolError;
use crate::config::{SearchConfig, SearchProviderConfig, SearchProviderName};
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
    Brave,
    Tavily,
    Exa,
    Serper,
    Searxng,
    DuckDuckGo,
    Google,
    Bing,
}

impl SearchEngine {
    pub fn name(&self) -> &'static str {
        match self {
            SearchEngine::Brave => "Brave",
            SearchEngine::Tavily => "Tavily",
            SearchEngine::Exa => "Exa",
            SearchEngine::Serper => "Serper",
            SearchEngine::Searxng => "SearXNG",
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Google => "Google",
            SearchEngine::Bing => "Bing",
        }
    }

    fn from_name(name: SearchProviderName) -> Self {
        match name {
            SearchProviderName::Brave => SearchEngine::Brave,
            SearchProviderName::Tavily => SearchEngine::Tavily,
            SearchProviderName::Exa => SearchEngine::Exa,
            SearchProviderName::Serper => SearchEngine::Serper,
            SearchProviderName::Searxng => SearchEngine::Searxng,
            SearchProviderName::DuckDuckGo => SearchEngine::DuckDuckGo,
            SearchProviderName::Google => SearchEngine::Google,
            SearchProviderName::Bing => SearchEngine::Bing,
        }
    }
}

/// A configured backend with its resolved credentials.
#[derive(Debug, Clone)]
pub(super) struct ResolvedProvider {
    engine: SearchEngine,
    config: SearchProviderConfig,
}

impl ResolvedProvider {
    fn configured(&self) -> bool {
        self.config.is_configured()
    }

    fn key(&self) -> Option<&str> {
        self.config.api_key.as_deref()
    }

    fn url(&self) -> Option<&str> {
        self.config.url.as_deref()
    }
}

impl From<&SearchProviderConfig> for ResolvedProvider {
    fn from(p: &SearchProviderConfig) -> Self {
        ResolvedProvider {
            engine: SearchEngine::from_name(p.name),
            config: p.clone(),
        }
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
    providers: Vec<ResolvedProvider>,
    proxy: Option<String>,
    last_request: Arc<Mutex<Option<Instant>>>,
    engine_last: Arc<Mutex<HashMap<SearchEngine, Instant>>>,
    engine_cooldown: Arc<Mutex<HashMap<SearchEngine, Instant>>>,
    #[cfg(feature = "browser")]
    pub(super) browser: Option<Arc<BrowserState>>,
}

impl WebSearchTool {
    /// Resolve configured providers and warn about listed-but-unconfigured ones.
    fn resolve_providers(search: &SearchConfig) -> Vec<ResolvedProvider> {
        let providers: Vec<ResolvedProvider> = search
            .resolved_entries()
            .iter()
            .map(ResolvedProvider::from)
            .collect();
        for p in &providers {
            if !p.configured() {
                let missing = if p.engine == SearchEngine::Searxng {
                    "url"
                } else {
                    "api_key"
                };
                log::warn!(
                    "search provider {} is configured but missing its {}; it will be skipped",
                    p.engine.name(),
                    missing
                );
            }
        }
        providers
    }

    #[cfg(not(feature = "browser"))]
    pub fn new(policy: Policy, search: SearchConfig, proxy: Option<String>) -> Self {
        Self {
            policy,
            providers: Self::resolve_providers(&search),
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
            providers: Self::resolve_providers(&search),
            proxy,
            last_request: Arc::new(Mutex::new(None)),
            engine_last: Arc::new(Mutex::new(HashMap::new())),
            engine_cooldown: Arc::new(Mutex::new(HashMap::new())),
            browser,
        }
    }

    /// The configured backends in order, for `--probe-web`.
    pub(super) fn providers(&self) -> Vec<SearchEngine> {
        self.providers.iter().map(|p| p.engine).collect()
    }
}

impl PortableTool for WebSearchTool {
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

        if self.providers.is_empty() {
            return Err(ToolError::Message(
                "no search providers configured; add search.providers to the config".to_string(),
            ));
        }

        self.rate_limit_wait().await;

        for provider in &self.providers {
            let engine = provider.engine;
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
                    if matches!(reason, EngineError::Denied(_)) {
                        log::warn!(
                            "search via {} was rejected: {}",
                            engine.name(),
                            reason.detail()
                        );
                    }
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
            "Search failed. All providers returned no results. Try rephrasing your query."
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
        let provider = self
            .providers
            .iter()
            .find(|p| p.engine == engine)
            .ok_or(EngineError::NotConfigured)?;
        match engine {
            SearchEngine::Brave => {
                let key = provider.key().ok_or(EngineError::NotConfigured)?;
                let body = fetch_json(brave(query, key), self.proxy.as_deref()).await?;
                self.finish_api(&parse_brave(&body), None)
            }
            SearchEngine::Serper => {
                let key = provider.key().ok_or(EngineError::NotConfigured)?;
                let body = fetch_json(serper(query, key), self.proxy.as_deref()).await?;
                self.finish_api(&parse_serper(&body), None)
            }
            SearchEngine::Tavily => {
                let key = provider.key().ok_or(EngineError::NotConfigured)?;
                let body = fetch_json(tavily(query, key), self.proxy.as_deref()).await?;
                let answer = parse_tavily_answer(&body);
                self.finish_api(&parse_tavily(&body), answer.as_deref())
            }
            SearchEngine::Exa => {
                let key = provider.key().ok_or(EngineError::NotConfigured)?;
                let body = fetch_json(exa(query, key), self.proxy.as_deref()).await?;
                self.finish_api(&parse_exa(&body), None)
            }
            SearchEngine::Searxng => {
                let url = provider.url().ok_or(EngineError::NotConfigured)?;
                match fetch_json(searxng(query, url), self.proxy.as_deref()).await {
                    Ok(body) => self.finish_api(&parse_searxng(&body), None),
                    Err(e) => {
                        debug!(
                            "{DIM}  SearXNG JSON unavailable ({}); using HTML{RESET}",
                            e.detail()
                        );
                        let search_url = searxng_search_url(url, query);
                        let html = fetch(&search_url, self.proxy.as_deref())
                            .await
                            .map_err(EngineError::Fetch)?;
                        let md = html_to_markdown(&html);
                        check_quality(&md).map_err(EngineError::Quality)?;
                        Ok(md)
                    }
                }
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

    /// Normalize an API response into result text, rejecting empty results.
    fn finish_api(
        &self,
        results: &[SearchResult],
        answer: Option<&str>,
    ) -> Result<String, EngineError> {
        if results.is_empty() {
            return Err(EngineError::Quality("no results".to_string()));
        }
        Ok(render_results(results, answer))
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
        assert_eq!(SearchEngine::Brave.name(), "Brave");
        assert_eq!(SearchEngine::Tavily.name(), "Tavily");
        assert_eq!(SearchEngine::Exa.name(), "Exa");
        assert_eq!(SearchEngine::Serper.name(), "Serper");
        assert_eq!(SearchEngine::Searxng.name(), "SearXNG");
        assert_eq!(SearchEngine::DuckDuckGo.name(), "DuckDuckGo");
        assert_eq!(SearchEngine::Google.name(), "Google");
        assert_eq!(SearchEngine::Bing.name(), "Bing");
    }

    #[test]
    fn test_engine_from_name_and_requires_key() {
        assert_eq!(
            SearchEngine::from_name(SearchProviderName::Brave),
            SearchEngine::Brave
        );
        assert_eq!(
            SearchEngine::from_name(SearchProviderName::Searxng),
            SearchEngine::Searxng
        );
        assert!(SearchProviderName::Brave.requires_key());
        assert!(!SearchProviderName::Searxng.requires_key());
        assert!(!SearchProviderName::DuckDuckGo.requires_key());
    }

    fn tool(search: SearchConfig) -> WebSearchTool {
        #[cfg(feature = "browser")]
        {
            WebSearchTool::with_browser(Policy::default(), search, None, None)
        }
        #[cfg(not(feature = "browser"))]
        {
            WebSearchTool::new(Policy::default(), search, None)
        }
    }

    #[test]
    fn test_providers_follow_config_order() {
        let yaml = "providers:\n  - name: exa\n    api_key: k\n  - name: duckduckgo\n  - name: brave\n    api_key: k\n";
        let search: SearchConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(
            tool(search).providers(),
            vec![
                SearchEngine::Exa,
                SearchEngine::DuckDuckGo,
                SearchEngine::Brave
            ]
        );
    }

    #[test]
    fn test_default_providers_when_unset() {
        assert_eq!(
            tool(SearchConfig::default()).providers(),
            vec![
                SearchEngine::DuckDuckGo,
                SearchEngine::Google,
                SearchEngine::Bing
            ]
        );
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
