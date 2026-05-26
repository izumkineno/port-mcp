use rmcp::{ServiceExt, transport::stdio};

use crate::mcp::tools::PortMcpServer;

pub async fn run_stdio_server() -> Result<(), rmcp::RmcpError> {
    let service = PortMcpServer::new()
        .serve(stdio())
        .await
        .inspect_err(|error| {
            tracing::error!(?error, "rmcp stdio server failed to initialize");
        })?;

    service.waiting().await?;
    Ok(())
}
