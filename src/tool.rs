use log::{debug, info, warn};
use rmcp::{
    model::{ClientCapabilities, ClientInfo, Implementation},
    service::{ServerSink, ServiceExt},
    transport::streamable_http_client::StreamableHttpClientTransport,
};
use std::collections::HashSet;

use crate::config::McpServerConfig;

pub struct ToolSet {
    pub tools: Vec<rmcp::model::Tool>,
    pub sink: ServerSink,
}

/// One tool server to connect. `required` servers abort startup on failure
/// (`--tool`); optional ones (from config) are skipped with a warning.
pub struct ToolServerSpec {
    pub name: Option<String>,
    pub url: String,
    pub required: bool,
}

/// Merge config-declared servers with `--tool` URLs. CLI entries come first so
/// they keep their required (fatal) behavior; duplicates by URL are dropped.
pub fn merge_specs(config_servers: &[McpServerConfig], cli_urls: &[String]) -> Vec<ToolServerSpec> {
    let mut seen = HashSet::new();
    let mut specs = Vec::new();
    for url in cli_urls {
        if seen.insert(url.clone()) {
            specs.push(ToolServerSpec {
                name: None,
                url: url.clone(),
                required: true,
            });
        }
    }
    for server in config_servers {
        let url = server.url.trim();
        if url.is_empty() {
            warn!("mcp: skipping server with an empty url");
            continue;
        }
        if seen.insert(url.to_string()) {
            specs.push(ToolServerSpec {
                name: server.name.clone(),
                url: url.to_string(),
                required: false,
            });
        }
    }
    specs
}

pub async fn connect_tool_servers(specs: &[ToolServerSpec]) -> anyhow::Result<Vec<ToolSet>> {
    let mut sets = Vec::new();
    for spec in specs {
        match connect_one(spec.name.as_deref(), &spec.url).await {
            Ok(set) => sets.push(set),
            Err(e) if spec.required => return Err(e),
            Err(e) => {
                warn!("mcp: skipping tool server '{}': {e}", spec.label());
            }
        }
    }
    Ok(sets)
}

impl ToolServerSpec {
    fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.url)
    }
}

async fn connect_one(name: Option<&str>, url: &str) -> anyhow::Result<ToolSet> {
    match name {
        Some(name) => info!("Connecting to tool server '{name}': {url}"),
        None => info!("Connecting to tool server: {url}"),
    }

    let transport = StreamableHttpClientTransport::from_uri(url);
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
    );

    let service = client_info
        .serve(transport)
        .await
        .inspect_err(|e| debug!("tool server connection error: {e:?}"))?;

    let tools = service
        .peer()
        .list_all_tools()
        .await
        .inspect_err(|e| debug!("tool server list_tools error: {e:?}"))?;

    info!("Found {} tools on tool server: {url}", tools.len());

    Ok(ToolSet {
        tools,
        sink: service.peer().clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str, name: Option<&str>) -> McpServerConfig {
        McpServerConfig {
            url: url.to_string(),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn test_merge_cli_first_and_dedup() {
        let config = vec![cfg("https://b/mcp", Some("b")), cfg("https://a/mcp", None)];
        let cli = vec!["https://a/mcp".to_string(), "https://c/mcp".to_string()];
        let specs = merge_specs(&config, &cli);
        let urls: Vec<&str> = specs.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://a/mcp", "https://c/mcp", "https://b/mcp"]
        );
        assert!(specs[0].required && specs[1].required);
        assert!(!specs[2].required);
        assert_eq!(specs[2].name.as_deref(), Some("b"));
    }

    #[test]
    fn test_merge_skips_empty_urls() {
        let config = vec![cfg("   ", Some("blank")), cfg("https://a/mcp", None)];
        let specs = merge_specs(&config, &[]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].url, "https://a/mcp");
    }
}
