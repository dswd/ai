use std::time::Duration;

use serde_json::Value;

use super::shared::http_client;

/// Results requested from every API provider.
const RESULT_COUNT: usize = 10;
/// Per-request timeout for search API calls.
const API_TIMEOUT: Duration = Duration::from_secs(15);
/// Longest snippet kept per result (Exa/Tavily can return full page text).
const MAX_SNIPPET_CHARS: usize = 500;

/// One normalized search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Structured failure from a search backend, used to decide whether to retry
/// and how long to cool the backend down.
#[derive(Debug)]
pub(super) enum EngineError {
    NotConfigured,
    Fetch(String),
    Quality(String),
    Denied(String),
}

impl EngineError {
    pub(super) fn detail(&self) -> String {
        match self {
            EngineError::NotConfigured => "not configured".to_string(),
            EngineError::Fetch(e) => format!("fetch: {e}"),
            EngineError::Quality(e) => format!("quality: {e}"),
            EngineError::Denied(e) => format!("denied: {e}"),
        }
    }

    /// Errors worth one retry: anything that reached the network and failed
    /// (timeouts, rate limits, 5xx, browser failures). Configuration, auth, and
    /// quality failures are treated as permanent for this call.
    pub(super) fn is_transient(&self) -> bool {
        matches!(self, EngineError::Fetch(_))
    }

    /// Errors that indicate the backend is blocking us (CAPTCHA, Cloudflare,
    /// challenge pages). These get a longer cooldown so the ladder skips the
    /// backend for a while instead of hammering it.
    pub(super) fn is_block(&self) -> bool {
        matches!(self, EngineError::Quality(e)
            if e.contains("blocked marker") || e.contains("no results area found"))
    }
}

/// A prepared JSON API request.
pub(super) struct JsonRequest {
    pub method: reqwest::Method,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<Value>,
}

/// Send an API request, mapping HTTP status to a structured error:
/// `401/403/402` are permanent (auth/quota), everything else non-2xx
/// (`429`, `5xx`, ...) is transient and worth one retry.
pub(super) async fn fetch_json(
    req: JsonRequest,
    proxy: Option<&str>,
) -> Result<Value, EngineError> {
    let mut request = http_client(proxy)
        .request(req.method, &req.url)
        .timeout(API_TIMEOUT)
        .header(reqwest::header::ACCEPT, "application/json");
    for (name, value) in &req.headers {
        request = request.header(*name, value);
    }
    if let Some(body) = req.body {
        request = request.json(&body);
    }

    let resp = request
        .send()
        .await
        .map_err(|e| EngineError::Fetch(format!("request: {e}")))?;
    let status = resp.status().as_u16();
    if let Some(err) = status_error(status) {
        return Err(err);
    }

    let body = resp
        .text()
        .await
        .map_err(|e| EngineError::Fetch(format!("read: {e}")))?;
    serde_json::from_str(&body).map_err(|e| EngineError::Quality(format!("invalid JSON: {e}")))
}

/// Map a non-2xx status to a structured error. Auth/quota and other 4xx are
/// permanent (skip to the next provider, with a warning); `429` and `5xx` are
/// transient and retried once.
fn status_error(status: u16) -> Option<EngineError> {
    if (200..300).contains(&status) {
        return None;
    }
    Some(match status {
        401 | 403 => EngineError::Denied(format!("authentication failed (HTTP {status})")),
        402 => EngineError::Denied("quota/credits exhausted (HTTP 402)".to_string()),
        429 => EngineError::Fetch("HTTP 429 (rate limited)".to_string()),
        s if s < 500 => EngineError::Denied(format!("request rejected (HTTP {s})")),
        s => EngineError::Fetch(format!("HTTP {s}")),
    })
}

/// Render normalized results as markdown, optionally prefixed by a provider
/// answer/summary. Snippets are clipped to keep output bounded.
pub(super) fn render_results(results: &[SearchResult], answer: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(answer) = answer.map(str::trim).filter(|a| !a.is_empty()) {
        out.push_str(answer);
        out.push_str("\n\n");
    }
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}]({})\n",
            i + 1,
            r.title.trim(),
            r.url.trim()
        ));
        let snippet = clip(r.snippet.trim(), MAX_SNIPPET_CHARS).replace('\n', " ");
        if !snippet.is_empty() {
            out.push_str(&format!("   {snippet}\n"));
        }
    }
    out
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn collect(arr: Option<&Value>, f: impl Fn(&Value) -> SearchResult) -> Vec<SearchResult> {
    arr.and_then(Value::as_array)
        .map(|items| items.iter().map(f).filter(|r| !r.url.is_empty()).collect())
        .unwrap_or_default()
}

pub(super) fn brave(query: &str, key: &str) -> JsonRequest {
    JsonRequest {
        method: reqwest::Method::GET,
        url: format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={RESULT_COUNT}",
            urlencoding::encode(query)
        ),
        headers: vec![
            ("X-Subscription-Token", key.to_string()),
            ("Accept-Encoding", "gzip".to_string()),
        ],
        body: None,
    }
}

pub(super) fn parse_brave(v: &Value) -> Vec<SearchResult> {
    collect(v.pointer("/web/results"), |r| SearchResult {
        title: str_field(r, "title"),
        url: str_field(r, "url"),
        snippet: str_field(r, "description"),
    })
}

pub(super) fn serper(query: &str, key: &str) -> JsonRequest {
    JsonRequest {
        method: reqwest::Method::POST,
        url: "https://google.serper.dev/search".to_string(),
        headers: vec![("X-API-KEY", key.to_string())],
        body: Some(serde_json::json!({ "q": query, "num": RESULT_COUNT })),
    }
}

pub(super) fn parse_serper(v: &Value) -> Vec<SearchResult> {
    collect(v.get("organic"), |r| SearchResult {
        title: str_field(r, "title"),
        url: str_field(r, "link"),
        snippet: str_field(r, "snippet"),
    })
}

pub(super) fn tavily(query: &str, key: &str) -> JsonRequest {
    JsonRequest {
        method: reqwest::Method::POST,
        url: "https://api.tavily.com/search".to_string(),
        headers: vec![("Authorization", format!("Bearer {key}"))],
        body: Some(serde_json::json!({ "query": query, "max_results": RESULT_COUNT })),
    }
}

pub(super) fn parse_tavily(v: &Value) -> Vec<SearchResult> {
    collect(v.get("results"), |r| SearchResult {
        title: str_field(r, "title"),
        url: str_field(r, "url"),
        snippet: str_field(r, "content"),
    })
}

pub(super) fn parse_tavily_answer(v: &Value) -> Option<String> {
    v.get("answer").and_then(Value::as_str).map(str::to_string)
}

pub(super) fn exa(query: &str, key: &str) -> JsonRequest {
    JsonRequest {
        method: reqwest::Method::POST,
        url: "https://api.exa.ai/search".to_string(),
        headers: vec![("x-api-key", key.to_string())],
        body: Some(serde_json::json!({ "query": query, "numResults": RESULT_COUNT })),
    }
}

pub(super) fn parse_exa(v: &Value) -> Vec<SearchResult> {
    collect(v.get("results"), |r| SearchResult {
        title: str_field(r, "title"),
        url: str_field(r, "url"),
        snippet: exa_snippet(r),
    })
}

fn exa_snippet(r: &Value) -> String {
    if let Some(text) = r
        .get("text")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        return text.to_string();
    }
    r.get("highlights")
        .and_then(Value::as_array)
        .and_then(|h| h.first())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn searxng(query: &str, base: &str) -> JsonRequest {
    let url = super::search_html::searxng_search_url(base, query);
    let sep = if url.contains('?') { '&' } else { '?' };
    JsonRequest {
        method: reqwest::Method::GET,
        url: format!("{url}{sep}format=json"),
        headers: Vec::new(),
        body: None,
    }
}

pub(super) fn parse_searxng(v: &Value) -> Vec<SearchResult> {
    collect(v.get("results"), |r| SearchResult {
        title: str_field(r, "title"),
        url: str_field(r, "url"),
        snippet: str_field(r, "content"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_brave() {
        let v = json!({"web": {"results": [
            {"title": "Rust", "url": "https://rust-lang.org", "description": "A language"}
        ]}});
        let results = parse_brave(&v);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].url, "https://rust-lang.org");
        assert_eq!(results[0].snippet, "A language");
    }

    #[test]
    fn test_parse_serper() {
        let v = json!({"organic": [
            {"title": "T", "link": "https://e.com", "snippet": "S"}
        ]});
        let results = parse_serper(&v);
        assert_eq!(results[0].url, "https://e.com");
        assert_eq!(results[0].snippet, "S");
    }

    #[test]
    fn test_parse_tavily_includes_answer() {
        let v = json!({
            "answer": "42",
            "results": [{"title": "T", "url": "https://e.com", "content": "C"}]
        });
        assert_eq!(parse_tavily(&v).len(), 1);
        assert_eq!(parse_tavily_answer(&v).as_deref(), Some("42"));
    }

    #[test]
    fn test_parse_exa_prefers_text_over_highlights() {
        let v = json!({"results": [
            {"title": "T", "url": "https://e.com", "text": "full", "highlights": ["h"]},
            {"title": "U", "url": "https://f.com", "highlights": ["h2"]}
        ]});
        let results = parse_exa(&v);
        assert_eq!(results[0].snippet, "full");
        assert_eq!(results[1].snippet, "h2");
    }

    #[test]
    fn test_parse_searxng() {
        let v = json!({"results": [{"title": "T", "url": "https://e.com", "content": "C"}]});
        assert_eq!(parse_searxng(&v)[0].snippet, "C");
    }

    #[test]
    fn test_parse_missing_or_malformed_is_empty() {
        assert!(parse_brave(&json!({})).is_empty());
        assert!(parse_serper(&json!({"organic": "nope"})).is_empty());
        let v = json!({"results": [{"title": "no url"}]});
        assert!(parse_searxng(&v).is_empty());
    }

    #[test]
    fn test_render_results_with_answer_and_clipping() {
        let results = vec![SearchResult {
            title: "T".to_string(),
            url: "https://e.com".to_string(),
            snippet: "x".repeat(600),
        }];
        let out = render_results(&results, Some("summary"));
        assert!(out.starts_with("summary\n\n"));
        assert!(out.contains("1. [T](https://e.com)"));
        assert!(out.contains('…'));
        assert!(!out.contains(&"x".repeat(600)));
    }

    #[test]
    fn test_searxng_json_url() {
        let req = searxng("a b", "http://localhost:8080/search");
        assert_eq!(req.url, "http://localhost:8080/search?q=a%20b&format=json");
        assert_eq!(req.method, reqwest::Method::GET);
    }

    #[test]
    fn test_api_request_shapes() {
        let brave = brave("q", "k");
        assert_eq!(brave.method, reqwest::Method::GET);
        assert!(brave.url.contains("count=10"));
        assert_eq!(brave.headers[0].0, "X-Subscription-Token");

        let serper = serper("q", "k");
        assert_eq!(serper.method, reqwest::Method::POST);
        assert_eq!(serper.headers[0], ("X-API-KEY", "k".to_string()));
        assert_eq!(serper.body.unwrap()["num"], 10);

        let tavily = tavily("q", "k");
        assert_eq!(tavily.headers[0].1, "Bearer k");

        let exa = exa("q", "k");
        assert_eq!(exa.headers[0].0, "x-api-key");
        assert_eq!(exa.body.unwrap()["numResults"], 10);
    }

    #[test]
    fn test_error_classification() {
        assert!(!EngineError::NotConfigured.is_transient());
        assert!(
            EngineError::Denied("401".to_string())
                .detail()
                .starts_with("denied: ")
        );
        assert!(!EngineError::Denied("401".to_string()).is_transient());
        assert!(EngineError::Fetch("timeout".to_string()).is_transient());
        assert!(EngineError::Quality("blocked marker: captcha".to_string()).is_block());
        assert!(!EngineError::Quality("too few links: 2".to_string()).is_block());
    }

    #[test]
    fn test_status_error_mapping() {
        assert!(status_error(200).is_none());
        assert!(!status_error(401).unwrap().is_transient());
        assert!(!status_error(402).unwrap().is_transient());
        assert!(!status_error(422).unwrap().is_transient());
        assert!(status_error(422).unwrap().detail().contains("422"));
        assert!(status_error(429).unwrap().is_transient());
        assert!(status_error(503).unwrap().is_transient());
    }
}
