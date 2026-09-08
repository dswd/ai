#![cfg(feature = "browser")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Run `f` with a fresh page in the shared browser, on a dedicated
/// single-threaded runtime, with a 30s timeout. The closure receives an owned
/// page and may use blocking-style obscura APIs.
pub(crate) async fn with_page<T, F, Fut>(
    browser: Arc<obscura::Browser>,
    timeout_msg: &str,
    f: F,
) -> Result<T, String>
where
    F: FnOnce(obscura::Page) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>>,
    T: Send + 'static,
{
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("browser rt: {e}"))?;
            let page = rt
                .block_on(browser.new_page())
                .map_err(|e| format!("page: {e}"))?;
            rt.block_on(f(page))
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok(v))) => Ok(v),
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(timeout_msg.to_string()),
    }
}

#[derive(Clone)]
pub struct BrowserState {
    pub(super) browser: Arc<obscura::Browser>,
    pub(super) last_url: Arc<Mutex<Option<String>>>,
}

impl BrowserState {
    /// Shared reference to the underlying stealth browser.
    pub fn browser(&self) -> Arc<obscura::Browser> {
        Arc::clone(&self.browser)
    }
}

impl std::fmt::Debug for BrowserState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserState").finish()
    }
}

impl BrowserState {
    pub async fn new() -> Result<Self, String> {
        let storage_dir = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("ai")
            .join("browser");
        let browser = obscura::Browser::builder()
            .stealth(true)
            .storage_dir(storage_dir)
            .build()
            .map_err(|e| format!("obscura: {e}"))?;
        Ok(Self {
            browser: Arc::new(browser),
            last_url: Arc::new(Mutex::new(None)),
        })
    }

    /// Try to dismiss a consent / cookie banner on the current page. Waits
    /// briefly for known consent buttons and clicks the first one found.
    /// Returns `true` if a button was clicked.
    #[allow(clippy::unused_self)]
    pub async fn accept_consent(&self, page: &mut obscura::Page) -> Result<bool, String> {
        let selectors = [
            "#L2AGLb",
            "#bnp_btn_accept",
            "#bnp_hfly_cta",
            "button[aria-label='Accept all']",
            "button[aria-label='I agree']",
        ];
        for sel in selectors {
            match page
                .wait_for_selector(sel, Duration::from_millis(600))
                .await
            {
                Ok(el) => {
                    el.click().map_err(|e| format!("consent click: {e}"))?;
                    page.settle(500).await;
                    return Ok(true);
                }
                Err(_) => continue,
            }
        }

        // Generic fallback: any button whose text matches accept/agree.
        let js = r#"(function(){var b=document.querySelectorAll('button,[role="button"]');for(var i=0;i<b.length;i++){var t=b[i].textContent.trim().toLowerCase();if(/^(accept all|accept|i agree|agree|ok|yes)$/i.test(t)){b[i].click();return true;}}return false;})()"#;
        let clicked = page.evaluate(js);
        if clicked.as_bool().unwrap_or(false) {
            page.settle(500).await;
            return Ok(true);
        }
        Ok(false)
    }

    /// Navigate to `url` in the shared stealth browser and return the page HTML.
    /// Used as a fallback when plain HTTP fetch is blocked.
    pub async fn fetch_html(&self, url: &str) -> Result<String, String> {
        let browser = Arc::clone(&self.browser);
        let url = url.to_string();
        with_page(
            browser,
            "browser fetch timed out after 30s",
            move |mut page| async move {
                page.goto(&url).await.map_err(|e| format!("goto: {e}"))?;
                tokio::time::sleep(Duration::from_millis(500)).await;
                let html = page.content();
                if html.trim().is_empty() {
                    return Err("empty page content".to_string());
                }
                Ok(html)
            },
        )
        .await
    }
}
