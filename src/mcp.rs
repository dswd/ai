use rmcp::{
    model::{ClientCapabilities, ClientInfo, Implementation},
    service::{ServerSink, ServiceExt},
    transport::streamable_http_client::StreamableHttpClientTransport,
};
use log::{info, error};

pub struct McpToolSet {
    #[allow(dead_code)]
    pub url: String,
    pub tools: Vec<rmcp::model::Tool>,
    pub sink: ServerSink,
}

pub async fn connect_mcp_servers(urls: &[String]) -> anyhow::Result<Vec<McpToolSet>> {
    let mut sets = Vec::new();
    for url in urls {
        let set = connect_one(url).await?;
        sets.push(set);
    }
    Ok(sets)
}

async fn connect_one(url: &str) -> anyhow::Result<McpToolSet> {
    info!("Connecting to MCP server: {url}");

    let transport = StreamableHttpClientTransport::from_uri(url);
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
    );

    let service = client_info
        .serve(transport)
        .await
        .inspect_err(|e| error!("MCP connection error: {:?}", e))?;

    let tools = service
        .peer()
        .list_all_tools()
        .await
        .inspect_err(|e| error!("MCP list_tools error: {:?}", e))?;

    info!("Found {} tools on MCP server: {url}", tools.len());

    Ok(McpToolSet {
        url: url.to_string(),
        tools,
        sink: service.peer().clone(),
    })
}
