use crate::util::{bar_line, bar_title};
use ansi_color_constants::*;
use log::debug;
use regex::Regex;
use std::time::Duration;

use super::shared::{BLOCK_MARKERS, ToolError, browser_headers, http_client};
use super::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES, process_output, truncate};

pub(crate) fn html_to_markdown(html: &str) -> String {
    let re_block = Regex::new(r"(?s)<style[^>]*>.*?</style>|<script[^>]*>.*?</script>").unwrap();
    let cleaned = re_block.replace_all(html, "").to_string();
    let md = html2md::parse_html(&cleaned);
    let re_tag = Regex::new(r"<[^>]+>").unwrap();
    re_tag.replace_all(&md, "").to_string()
}

pub(super) fn check_quality(md: &str) -> Result<(), String> {
    if md.trim().is_empty() {
        return Err("empty output".to_string());
    }

    let lower = md.to_lowercase();
    if let Some(word) = BLOCK_MARKERS.iter().copied().find(|w| lower.contains(w)) {
        return Err(format!("blocked marker: {word}"));
    }

    let link_count = md.matches("](").count();
    if link_count < 5 {
        return Err(format!("too few links: {link_count}"));
    }

    Ok(())
}

pub(super) fn finalize(
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

/// Build a SearXNG search URL from a configured base. The base may contain a
/// `{query}` placeholder; if it doesn't, `?q=` / `&q=` is appended so a bare
/// instance URL like `http://localhost:8080/search` still works.
pub(super) fn searxng_search_url(base: &str, query: &str) -> String {
    if base.contains("{query}") {
        base.replacen("{query}", &urlencoding::encode(query), 1)
    } else {
        let sep = if base.contains('?') { '&' } else { '?' };
        format!("{base}{sep}q={}", urlencoding::encode(query))
    }
}

pub(super) async fn fetch(url: &str, proxy: Option<&str>) -> Result<String, String> {
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
}
