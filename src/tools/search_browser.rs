#![cfg(feature = "browser")]

use ansi_color_constants::*;
use log::debug;
use std::sync::Arc;
use std::time::Duration;

use super::browser_state::with_page;
use super::shared::{BLOCK_MARKERS, js_literal};
use super::web_search::WebSearchTool;

impl WebSearchTool {
    /// Navigate to a search-engine home page, type the query into the search box
    /// using obscura's trusted input primitives, press Enter, and extract the
    /// results container. Polls until the results appear (or a timeout elapses).
    pub(super) async fn browser_search(
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

        with_page(
            browser,
            "search timed out after 30s",
            move |mut page| async move {
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
            },
        )
        .await
    }
}

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
fn link_count(s: &str) -> usize {
    s.matches("](").count() + s.matches("href=\"http").count()
}

/// Collect result-like links (titles and URLs) regardless of the container
/// markup obscura's DOM exposes, excluding header/footer/nav chrome. Handles
/// Google's `/url?q=` redirect links and other relative hrefs by resolving
/// against the page URL. Returns up to 20 markdown link lines, or an empty
/// string when nothing qualifies.
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
fn detect_block_marker(body: &str, title: &str) -> Option<&'static str> {
    let lower_body = body.to_lowercase();
    if let Some(m) = BLOCK_MARKERS
        .iter()
        .copied()
        .find(|m| lower_body.contains(m))
    {
        return Some(m);
    }
    let lower_title = title.to_lowercase();
    const TITLE_MARKERS: &[&str] = &["robot", "just a moment", "captcha", "unusual traffic"];
    TITLE_MARKERS
        .iter()
        .copied()
        .find(|m| lower_title.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_type_js_uses_trusted_primitives() {
        let js = build_type_js(r#"input[name="q"]"#, "hello \"world\"");
        assert!(js.contains("__obscura_setFieldValue"));
        assert!(js.contains("__obscura_markTrusted"));
        assert!(js.contains("requestSubmit"));
        assert!(js.contains(r#"hello \"world\""#));
        assert!(js.contains(r#"input[name=\"q\"]"#));
    }

    #[test]
    fn test_link_extract_js_shape() {
        assert!(LINK_EXTRACT_JS.contains("a[href]"));
        assert!(LINK_EXTRACT_JS.contains("header,footer,nav"));
        assert!(LINK_EXTRACT_JS.contains("seen.add(h2)"));
        assert!(LINK_EXTRACT_JS.contains("slice(0,20)"));
        assert!(LINK_EXTRACT_JS.contains("/url?q="));
        assert!(LINK_EXTRACT_JS.contains("new URL(h,location.href)"));
    }

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

    #[test]
    fn test_search_url_from_google() {
        let url = search_url_from("https://www.google.com", "rust testing");
        assert_eq!(url, "https://www.google.com/search?q=rust%20testing");
    }

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

    #[test]
    fn test_search_url_from_bing() {
        let url = search_url_from("https://www.bing.com", "rust testing");
        assert_eq!(url, "https://www.bing.com/search?q=rust%20testing");
    }

    #[test]
    fn test_search_url_from_custom_home() {
        let url = search_url_from("https://www.other.com", "a b");
        assert_eq!(url, "https://www.other.com?q=a%20b");
    }
}
