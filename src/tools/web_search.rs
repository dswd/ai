use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::{debug, info};
use rand::RngExt;
use regex::Regex;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
enum EngineError {
    NotConfigured,
    Fetch(String),
    Quality(String),
}

impl EngineError {
    fn detail(&self) -> String {
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
    browser: Option<Arc<BrowserState>>,
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
                    return finalize(result, args.offset, args.limit);
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
    async fn run_engine(&self, engine: SearchEngine, query: &str) -> Result<String, EngineError> {
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

    /// Navigate to a search-engine home page, type the query into the search box
    /// using obscura's trusted input primitives, press Enter, and extract the
    /// results container. Polls until the results appear (or a timeout elapses).
    #[cfg(feature = "browser")]
    async fn browser_search(
        &self,
        home_url: &str,
        input_selector: &str,
        selectors: &[&str],
        query: &str,
    ) -> Result<String, String> {
        let browser = match &self.browser {
            Some(bs) => bs.browser(),
            None => Arc::new(
                obscura::Browser::builder()
                    .stealth(true)
                    .build()
                    .map_err(|e| format!("browser: {e}"))?,
            ),
        };
        let browser_state = self.browser.clone();
        let home_url = home_url.to_string();
        let input_selector = input_selector.to_string();
        let query = query.to_string();
        let selectors_owned: Vec<String> = selectors.iter().map(|s| s.to_string()).collect();
        let selectors_json = serde_json::to_string(&selectors_owned).unwrap_or_default();
        let fallback_url = search_url_from(&home_url, &query);

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

                    page.goto(&home_url)
                        .await
                        .map_err(|e| format!("goto: {e}"))?;
                    tokio::time::sleep(Duration::from_millis(800)).await;

                    // Accept consent: try clicking in-page banners first, then, if
                    // the engine redirected us to a dedicated consent host, accept
                    // via the consent form's save URL (a plain GET that stores the
                    // consent cookie server-side and redirects back).
                    if let Some(bs) = &browser_state {
                        let _ = bs.accept_consent(&mut page).await;
                    } else {
                        page.evaluate(
                            r#"(function(){var b=document.querySelectorAll('button,[role="button"]');for(var i=0;i<b.length;i++){var t=b[i].textContent.trim().toLowerCase();if(/^(accept all|accept|i agree|agree|ok|yes)$/i.test(t)){b[i].click();break;}}})()"#,
                        );
                    }
                    page.settle(1000).await;

                    // Some engines redirect to a consent/captcha host; accept via
                    // the consent form and re-navigate.
                    let mut host = page
                        .evaluate("location.hostname")
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    if host.contains("consent") || host.contains("sorry") {
                        if submit_consent_form(&mut page) {
                            debug!("{DIM}  accepted consent via form POST{RESET}");
                            page.settle(1500).await;
                        }
                        page.goto(&home_url)
                            .await
                            .map_err(|e| format!("goto: {e}"))?;
                        tokio::time::sleep(Duration::from_millis(800)).await;
                        host = page
                            .evaluate("location.hostname")
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                    }
                    if host.contains("consent") || host.contains("sorry") {
                        let title = page
                            .evaluate("document.title")
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        let body = page.content();
                        let snippet = body.chars().take(200).collect::<String>();
                        return Err(format!(
                            "blocked marker: consent (url={}, title={title:?}, bytes={}, body={snippet:?})",
                            page.url(),
                            body.len(),
                        ));
                    }

                    // Type the query with trusted events and submit the form.
                    let type_js = build_type_js(&input_selector, &query);
                    let mut typed = false;
                    for _ in 0..6 {
                        let raw = page.evaluate(&type_js);
                        let status = raw.as_str().unwrap_or("").to_string();
                        if status == "submitted" {
                            typed = true;
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    if !typed {
                        return Err("search input not found on home page".to_string());
                    }
                    page.settle(800).await;

                    // The public obscura API cannot process JS-initiated form
                    // submission, so the form submit may not have navigated.
                    // If we're still on the home page, reach the results URL via
                    // an explicit goto (keeps the human-like warm-up + typing).
                    let after_url = page.url();
                    let navigated = after_url.contains("/search")
                        || after_url.contains("q=")
                        || after_url.contains("&q=");
                    if !navigated {
                        let target = form_search_url(&mut page).unwrap_or(fallback_url.clone());
                        debug!("{DIM}  navigating to results URL: {target}{RESET}");
                        page.goto(&target)
                            .await
                            .map_err(|e| format!("goto results: {e}"))?;
                        page.settle(800).await;
                    }

                    // The results URL may itself redirect to a consent/sorry page.
                    // If so, accept consent by POSTing the consent form from within
                    // the page (stores the cookie), then re-navigate.
                    let mut results_url = page.url();
                    if is_consent_url(&results_url) && submit_consent_form(&mut page) {
                        debug!("{DIM}  accepted consent via form POST{RESET}");
                        page.settle(1500).await;
                        page.goto(&fallback_url)
                            .await
                            .map_err(|e| format!("goto results: {e}"))?;
                        page.settle(800).await;
                        results_url = page.url();
                    }
                    if is_consent_url(&results_url) {
                        let title = page
                            .evaluate("document.title")
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        let body = page.content();
                        let snippet = body.chars().take(200).collect::<String>();
                        return Err(format!(
                            "blocked marker: consent (url={results_url}, title={title:?}, bytes={}, body={snippet:?})",
                            body.len(),
                        ));
                    }

                    // Poll until the results appear. Two extractors are tried:
                    // the container-based one (engine result selectors + ancestor
                    // heuristic) and the link-based one (markdown `[title](url)`
                    // lines). obscura's DOM may render Google results differently,
                    // so whichever yields the most links wins.
                    let mut html = String::new();
                    let mut best_links = 0usize;
                    let mut best = String::new();
                    for _ in 0..12 {
                        let raw_container =
                            page.evaluate(&extract_results_js(selectors_json.as_str()));
                        let container = raw_container.as_str().unwrap_or("").to_string();
                        let raw_links = page.evaluate(LINK_EXTRACT_JS);
                        let links = raw_links.as_str().unwrap_or("").to_string();

                        for cand in [container, links] {
                            let n = link_count(&cand);
                            if n > best_links {
                                best_links = n;
                                best = cand.clone();
                            }
                        }
                        if best_links >= 5 {
                            html = best.clone();
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    if html.trim().is_empty() {
                        html = best.clone();
                    }
                    if html.trim().is_empty() {
                        let content = page.content();
                        let title = page
                            .evaluate("document.title")
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        if let Some(marker) = detect_block_marker(&content, &title) {
                            return Err(format!("blocked marker: {marker}"));
                        }
                        let len = content.len();
                        let snippet = content.chars().take(200).collect::<String>();
                        return Err(format!(
                            "no results area found (url={results_url}, title={title:?}, bytes={len}, body={snippet:?})"
                        ));
                    }
                    Ok(html)
                })
            }),
        )
        .await
        .map_err(|_| "search timed out after 30s".to_string())?
        .map_err(|e| e.to_string())?
    }
}

#[cfg(feature = "browser")]
fn extract_results_js(selectors_json: &str) -> String {
    format!(
        r#"(function(){{var sels={selectors_json};for(var i=0;i<sels.length;i++){{var el=document.querySelector(sels[i]);if(el&&el.innerText.trim().length>0)return el.innerHTML;}}
var l=document.querySelectorAll('a[href]');if(!l.length)return'';
var c=new Map();for(var i=0;i<l.length;i++){{var e=l[i];var h=e.getAttribute('href')||'';if(h.indexOf('http')===0||h.indexOf('/url?q=')===0){{for(var j=0;j<4;j++){{e=e.parentElement;if(!e)break}}if(e)c.set(e,(c.get(e)||0)+1)}}}}
var b=null,n=0;c.forEach(function(v,k){{if(v>n){{b=k;n=v}}}});
if(!b||n<5)return'';
var tag=b.tagName?b.tagName.toLowerCase():'';
if(tag==='header'||tag==='footer'||tag==='nav')return'';
if(b.closest&&b.closest('header,footer,nav'))return'';
return b&&n>3?b.innerHTML:''}})()"#
    )
}

/// Count result-like links in extracted text: markdown links (`[x](url)`) plus
/// HTML anchors (`href="http...`). Used to pick the richer extractor output.
#[cfg(feature = "browser")]
fn link_count(s: &str) -> usize {
    s.matches("](").count() + s.matches("href=\"http").count()
}

/// Collect result-like links (titles and URLs) regardless of the container
/// markup obscura's DOM exposes, excluding header/footer/nav chrome. Handles
/// Google's `/url?q=` redirect links and other relative hrefs by resolving
/// against the page URL. Returns up to 20 markdown link lines, or an empty
/// string when nothing qualifies.
#[cfg(feature = "browser")]
const LINK_EXTRACT_JS: &str = r#"(function(){
var out=[];var seen=new Set();
var anchors=document.querySelectorAll('a[href]');
for(var i=0;i<anchors.length;i++){
  var a=anchors[i];
  if(a.closest&&a.closest('header,footer,nav'))continue;
  var t=(a.textContent||'').trim();
  if(!t||t.length>200)continue;
  var h=a.getAttribute('href');
  if(!h)continue;
  var h2=h;
  if(h.indexOf('/url?q=')===0){var m=h.match(/[?&]q=([^&]+)/);if(m){try{h2=decodeURIComponent(m[1]);}catch(e){}else continue;}
  }else if(h.indexOf('http')===0){h2=h;}
  else if(h.indexOf('//')===0){h2='https:'+h;}
  else {try{h2=new URL(h,location.href).href;}catch(e){continue;}}
  if(h2.indexOf('http')!==0)continue;
  if(h2.indexOf('google.com')>=0&&h2.indexOf('url?q=')<0)continue;
  if(seen.has(h2))continue;
  seen.add(h2);
  out.push('['+t+']('+h2+')');
}
return out.slice(0,20).join('\n');
})()"#;

#[cfg(feature = "browser")]
fn search_url_from(home_url: &str, query: &str) -> String {
    let q = urlencoding::encode(query);
    let base = if home_url.contains("google.com") {
        "https://www.google.com/search"
    } else if home_url.contains("bing.com") {
        "https://www.bing.com/search"
    } else {
        home_url
    };
    format!("{base}?q={q}")
}

/// Whether a URL is a consent/captcha interstitial host (e.g.
/// `consent.google.com` or Google's `/sorry/` endpoint). Host-based, so it
/// cannot false-positive on a results page that merely contains "consent" text.
#[cfg(feature = "browser")]
fn is_consent_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("").to_lowercase();
    host.contains("consent") || host.contains("sorry") || url.contains("/sorry/")
}

/// Read the search URL that the on-page form would navigate to (form action +
/// the typed query). Returns `None` if no form/query can be found.
#[cfg(feature = "browser")]
fn form_search_url(page: &mut obscura::Page) -> Option<String> {
    let js = r#"(function(){
var f=document.querySelector('form[action*="search"]')||document.querySelector('form');
if(!f)return'';
var i=f.querySelector('input[name="q"],textarea[name="q"]');
if(!i||!i.value)return'';
var action=f.getAttribute('action')||'';
var sep=action.indexOf('?')>=0?'&':'?';
var url=action+sep+'q='+encodeURIComponent(i.value);
try{url=new URL(url,location.href).href;}catch(e){}
return url;
})()"#;
    let raw = page.evaluate(js);
    let url = raw.as_str().unwrap_or("").to_string();
    if url.is_empty() { None } else { Some(url) }
}

/// Submit the consent form from within the page via `fetch()`, so the
/// consent cookie (Set-Cookie on the redirect) is stored in the shared jar —
/// obscura's public API cannot process the JS form-submit *navigation*, but its
/// `fetch()` shim does run POSTs, follow redirects, and persist Set-Cookie.
/// Returns `true` if a consent form was found and submitted.
#[cfg(feature = "browser")]
fn submit_consent_form(page: &mut obscura::Page) -> bool {
    let js = r#"(function(){
var f=document.querySelector('form[action*="save"]')||document.querySelector('form');
if(!f)return false;
var action=f.getAttribute('action')||'';
if(!action)return false;
try{action=new URL(action,location.href).href;}catch(e){}
var data=new URLSearchParams();
var inputs=f.querySelectorAll('input,select,textarea');
for(var i=0;i<inputs.length;i++){
  var el=inputs[i];
  var name=el.getAttribute('name');
  if(!name)continue;
  var t=(el.getAttribute('type')||'').toLowerCase();
  if(t==='submit'||t==='button')continue;
  var val=el.value||'';
  data.append(name,val);
}
fetch(action,{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:data.toString(),redirect:'follow'}).then(function(r){});
return true;
})()"#;
    page.evaluate(js).as_bool().unwrap_or(false)
}

/// Build the JS that types `query` into the search box (matched by
/// `input_selector`) using obscura's trusted input primitives, then submits the
/// surrounding form with Enter.
#[cfg(feature = "browser")]
fn build_type_js(input_selector: &str, query: &str) -> String {
    let sel = js_literal(input_selector);
    let q = js_literal(query);
    format!(
        r#"(function(){{
var i=document.querySelector({sel});
if(!i)return'no-input';
i.focus();
globalThis.__obscura_setFieldValue(i,'value',{q});
i.dispatchEvent(globalThis.__obscura_markTrusted(new InputEvent('input',{{bubbles:true,data:{q}}})));
i.dispatchEvent(globalThis.__obscura_markTrusted(new KeyboardEvent('keydown',{{key:'Enter',code:'Enter',keyCode:13,which:13,bubbles:true}})));
i.dispatchEvent(globalThis.__obscura_markTrusted(new KeyboardEvent('keypress',{{key:'Enter',code:'Enter',keyCode:13,which:13,bubbles:true}})));
i.dispatchEvent(globalThis.__obscura_markTrusted(new KeyboardEvent('keyup',{{key:'Enter',code:'Enter',keyCode:13,which:13,bubbles:true}})));
var form=i.form||i.closest('form');
if(form){{try{{if(form.requestSubmit){{form.requestSubmit();return'submitted';}}}}catch(e){{}}form.submit();return'submitted';}}
return'no-form';
}})()"#
    )
}

/// Detect anti-bot challenge markers in a page body and/or title (used for
/// search-engine browser fallbacks). Returns the matched marker or `None`.
///
/// NOTE: consent is *not* detected here — the word "consent" appears in normal
/// results pages (privacy links, scripts), which would false-positive. Consent is
/// detected precisely by URL host (consent.google / sorry) in `browser_search`.
#[cfg(feature = "browser")]
fn detect_block_marker(body: &str, title: &str) -> Option<&'static str> {
    const MARKERS: &[&str] = &[
        "unusual traffic",
        "captcha",
        "verify you are human",
        "enable javascript",
        "enable js",
        "please enable",
        "access denied",
        "cf-chl",
        "g-recaptcha",
        "recaptcha",
        "just a moment",
        "our systems have detected",
        "please show you're not a robot",
    ];
    let lower_body = body.to_lowercase();
    if let Some(m) = MARKERS.iter().copied().find(|m| lower_body.contains(m)) {
        return Some(m);
    }
    let lower_title = title.to_lowercase();
    const TITLE_MARKERS: &[&str] = &["robot", "just a moment", "captcha", "unusual traffic"];
    TITLE_MARKERS
        .iter()
        .copied()
        .find(|m| lower_title.contains(m))
}

/// Result of probing a single search engine via `--probe-web`.
pub struct ProbeResult {
    pub engine: &'static str,
    pub ok: bool,
    pub latency_ms: u64,
    pub bytes: usize,
    pub detail: String,
    /// The exact result text that would be handed to the AI on success.
    pub output: String,
}

/// Probe every engine in the ladder and report per-engine diagnostics, without
/// taking the policy into account. Used by the hidden `--probe-web` flag.
#[allow(unused_variables)]
pub async fn probe_web_search(
    query: &str,
    search: &SearchConfig,
    proxy: Option<String>,
    #[cfg(feature = "browser")] browser: Option<Arc<BrowserState>>,
) -> Vec<ProbeResult> {
    #[cfg(feature = "browser")]
    let tool = WebSearchTool::with_browser(Policy::default(), search.clone(), proxy, browser);
    #[cfg(not(feature = "browser"))]
    let tool = WebSearchTool::new(Policy::default(), search.clone(), proxy);

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
                output: md,
            }),
            Err(reason) => results.push(ProbeResult {
                engine: engine.name(),
                ok: false,
                latency_ms,
                bytes: 0,
                detail: reason.detail(),
                output: String::new(),
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
        "verify you are human",
        "enable javascript",
        "access denied",
        "cf-chl",
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

/// Build a SearXNG search URL from a configured base. The base may contain a
/// `{query}` placeholder; if it doesn't, `?q=` / `&q=` is appended so a bare
/// instance URL like `http://localhost:8080/search` still works.
fn searxng_search_url(base: &str, query: &str) -> String {
    if base.contains("{query}") {
        base.replacen("{query}", &urlencoding::encode(query), 1)
    } else {
        let sep = if base.contains('?') { '&' } else { '?' };
        format!("{base}{sep}q={}", urlencoding::encode(query))
    }
}

async fn fetch(url: &str, proxy: Option<&str>) -> Result<String, String> {
    let resp = browser_headers(http_client(proxy).get(url))
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

    #[test]
    fn test_searxng_search_url_placeholder() {
        let url = searxng_search_url("http://localhost:8080/search?q={query}", "hello world");
        assert_eq!(url, "http://localhost:8080/search?q=hello%20world");
    }

    #[test]
    fn test_searxng_search_url_appends_query() {
        let url = searxng_search_url("http://localhost:8080/search", "a b");
        assert_eq!(url, "http://localhost:8080/search?q=a%20b");
        let url = searxng_search_url("http://localhost:8080/search?lang=en", "a b");
        assert_eq!(url, "http://localhost:8080/search?lang=en&q=a%20b");
    }

    #[test]
    fn test_searxng_search_url_placeholder_replaced_once() {
        let url = searxng_search_url("http://x/{query}?q={query}", "hi");
        assert_eq!(url, "http://x/hi?q={query}");
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

    #[cfg(feature = "browser")]
    #[test]
    fn test_build_type_js_uses_trusted_primitives() {
        let js = build_type_js(r#"input[name="q"]"#, "hello \"world\"");
        assert!(js.contains("__obscura_setFieldValue"));
        assert!(js.contains("__obscura_markTrusted"));
        assert!(js.contains("requestSubmit"));
        assert!(js.contains(r#"hello \"world\""#));
        assert!(js.contains(r#"input[name=\"q\"]"#));
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_link_extract_js_shape() {
        assert!(LINK_EXTRACT_JS.contains("a[href]"));
        assert!(LINK_EXTRACT_JS.contains("header,footer,nav"));
        assert!(LINK_EXTRACT_JS.contains("seen.add(h2)"));
        assert!(LINK_EXTRACT_JS.contains("slice(0,20)"));
        assert!(LINK_EXTRACT_JS.contains("/url?q="));
        assert!(LINK_EXTRACT_JS.contains("new URL(h,location.href)"));
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_link_count() {
        assert_eq!(link_count(""), 0);
        assert_eq!(link_count("[a](https://x.com)"), 1);
        assert_eq!(link_count("<a href=\"https://x.com\">a</a>"), 1);
        assert_eq!(
            link_count("[a](https://x.com) <a href=\"https://y.com\">b</a>"),
            2
        );
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_detect_block_marker() {
        assert_eq!(
            detect_block_marker("unusual traffic from your computer network", ""),
            Some("unusual traffic")
        );
        assert_eq!(
            detect_block_marker("<html>just a moment...</html>", ""),
            Some("just a moment")
        );
        assert_eq!(detect_block_marker("g-recaptcha", ""), Some("captcha"));
        assert_eq!(detect_block_marker("normal page with results", ""), None);
        assert_eq!(detect_block_marker("", ""), None);
        // "consent" in body/title must NOT be flagged — it appears in normal
        // results pages; consent is detected by URL host instead.
        assert_eq!(
            detect_block_marker("consent.google.com privacy settings", ""),
            None
        );
        assert_eq!(
            detect_block_marker("before you continue to google", ""),
            None
        );
        assert_eq!(detect_block_marker("enable cookies to continue", ""), None);
        assert_eq!(
            detect_block_marker("enable js and cookies", ""),
            Some("enable js")
        );
        assert_eq!(
            detect_block_marker("our systems have detected unusual activity", ""),
            Some("our systems have detected")
        );
        assert_eq!(detect_block_marker("", "Robot Check"), Some("robot"));
        assert_eq!(detect_block_marker("", "Google - Consent"), None);
        assert_eq!(
            detect_block_marker("", "Just a moment..."),
            Some("just a moment")
        );
        assert_eq!(detect_block_marker("plain body", "Search results"), None);
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_search_url_from_google() {
        let url = search_url_from("https://www.google.com", "rust testing");
        assert_eq!(url, "https://www.google.com/search?q=rust%20testing");
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_is_consent_url() {
        assert!(is_consent_url("https://consent.google.com/m?continue=..."));
        assert!(is_consent_url("https://consent.google.de/save"));
        assert!(is_consent_url(
            "https://www.google.com/sorry/index?continue=..."
        ));
        assert!(!is_consent_url("https://www.google.com/search?q=consent"));
        assert!(!is_consent_url("https://www.bing.com/search?q=consent"));
        assert!(!is_consent_url("https://www.google.com/search?q=rust"));
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_search_url_from_bing() {
        let url = search_url_from("https://www.bing.com", "rust testing");
        assert_eq!(url, "https://www.bing.com/search?q=rust%20testing");
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_search_url_from_custom_home() {
        let url = search_url_from("https://www.other.com", "a b");
        assert_eq!(url, "https://www.other.com?q=a%20b");
    }
}
