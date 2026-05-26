use rmcp::{ServiceExt, transport::stdio};

use crate::mcp::tools::M0SmokeServer;

pub async fn run_stdio_server() -> Result<(), rmcp::RmcpError> {
    let service = M0SmokeServer::new()
        .serve(stdio())
        .await
        .inspect_err(|error| {
            tracing::error!(?error, "rmcp stdio server failed to initialize");
        })?;

    service.waiting().await?;
    Ok(())
}
