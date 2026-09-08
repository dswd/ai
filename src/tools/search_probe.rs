#[cfg(feature = "browser")]
use std::sync::Arc;
use std::time::Instant;

#[cfg(feature = "browser")]
use super::browser_state::BrowserState;
use super::web_search::{SearchEngine, WebSearchTool};
use crate::config::SearchConfig;
use crate::policy::Policy;

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
