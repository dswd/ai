use crate::config::Config;
use rig_core::providers as rig_providers;

pub(crate) fn openai_client(
    config: &Config,
    base_url: &str,
    session_id: &str,
) -> anyhow::Result<rig_providers::openai::CompletionsClient> {
    let api_key = config
        .resolve_api_key()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .ok_or_else(|| anyhow::anyhow!(
            "OpenAI API key not found. Set OPENAI_API_KEY environment variable or api_key in config."
        ))?;

    let mut builder = rig_providers::openai::CompletionsClient::builder()
        .api_key(api_key.as_str())
        .base_url(base_url);
    if let Some(headers) = opencode_session_header(base_url, session_id) {
        builder = builder.http_headers(headers);
    }
    Ok(builder.build()?)
}

pub(crate) fn anthropic_client(
    config: &Config,
    base_url: &str,
    session_id: &str,
) -> anyhow::Result<rig_providers::anthropic::Client> {
    let api_key = config
        .resolve_api_key()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
        .ok_or_else(|| anyhow::anyhow!(
            "Anthropic API key not found. Set ANTHROPIC_API_KEY environment variable or api_key in config."
        ))?;

    let mut builder = rig_providers::anthropic::Client::builder()
        .api_key(api_key.as_str())
        .base_url(base_url);
    if let Some(headers) = opencode_session_header(base_url, session_id) {
        builder = builder.http_headers(headers);
    }
    Ok(builder.build()?)
}

/// When talking to an opencode-compatible endpoint, tag requests with the
/// current session id so the proxy can correlate them.
fn opencode_session_header(base_url: &str, session_id: &str) -> Option<reqwest::header::HeaderMap> {
    if !base_url.contains("opencode") {
        return None;
    }
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::HeaderName::from_static("x-opencode-session"),
        reqwest::header::HeaderValue::from_str(session_id).ok()?,
    );
    Some(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opencode_session_header_present() {
        let headers = opencode_session_header("https://opencode.example.com/v1", "calm-hawk")
            .expect("header should be set for opencode url");
        let value = headers
            .get("x-opencode-session")
            .expect("x-opencode-session header present")
            .to_str()
            .unwrap();
        assert_eq!(value, "calm-hawk");
    }

    #[test]
    fn test_opencode_session_header_absent_for_other_urls() {
        assert!(opencode_session_header("https://api.openai.com/v1", "calm-hawk").is_none());
    }

    #[test]
    fn test_opencode_session_header_skips_invalid_value() {
        assert!(opencode_session_header("https://opencode.example.com/v1", "bad\nvalue").is_none());
    }
}
