use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ResourceUpdatedNotificationParam},
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::mcp::session::SessionMode;

#[derive(Debug, Clone)]
pub struct M0SmokeServer {
    session_mode: SessionMode,
}

impl M0SmokeServer {
    pub const fn new() -> Self {
        Self {
            session_mode: SessionMode::UnverifiedM0Spike,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SmokeParams {
    message: String,
}

#[tool_router]
impl M0SmokeServer {
    #[tool(description = "M0 temporary smoke tool; not part of the initial port MCP contract")]
    pub async fn m0_smoke(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(SmokeParams { message }): Parameters<SmokeParams>,
    ) -> Result<CallToolResult, McpError> {
        let request_id = format!("{:?}", context.id);
        let peer_info_available = context.peer.peer_info().is_some();

        context
            .peer
            .notify_resource_updated(ResourceUpdatedNotificationParam::new("port-mcp://m0/smoke"))
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;

        let payload = serde_json::json!({
            "ok": true,
            "tool": "m0_smoke",
            "echo": message,
            "request_context_observed": true,
            "request_id_debug": request_id,
            "peer_info_available": peer_info_available,
            "session_mode": self.session_mode.as_str(),
            "resource_notification": "sent"
        });

        Ok(CallToolResult::success(vec![Content::text(
            payload.to_string(),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for M0SmokeServer {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::{
        ClientHandler, ServiceExt,
        model::{CallToolRequestParams, ResourceUpdatedNotificationParam},
        object,
        service::NotificationContext,
    };
    use tokio::sync::Notify;

    use super::M0SmokeServer;

    #[derive(Clone)]
    struct SmokeClient {
        resource_updated: Arc<Notify>,
    }

    impl ClientHandler for SmokeClient {
        async fn on_resource_updated(
            &self,
            params: ResourceUpdatedNotificationParam,
            _context: NotificationContext<rmcp::RoleClient>,
        ) {
            if params.uri == "port-mcp://m0/smoke" {
                self.resource_updated.notify_one();
            }
        }
    }

    #[tokio::test]
    async fn m0_smoke_tool_observes_context_and_sends_resource_notification()
    -> Result<(), Box<dyn std::error::Error>> {
        let (server_transport, client_transport) = tokio::io::duplex(4096);

        let server_handle = tokio::spawn(async move {
            M0SmokeServer::new()
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            Ok::<(), rmcp::RmcpError>(())
        });

        let resource_updated = Arc::new(Notify::new());
        let client = SmokeClient {
            resource_updated: resource_updated.clone(),
        }
        .serve(client_transport)
        .await?;

        let result = client
            .call_tool(
                CallToolRequestParams::new("m0_smoke")
                    .with_arguments(object!({ "message": "hello-m0" })),
            )
            .await?;

        let text = result
            .content
            .first()
            .and_then(|content| content.raw.as_text())
            .map(|text| text.text.as_str())
            .expect("m0 smoke should return text content");

        assert!(text.contains("\"request_context_observed\":true"));
        assert!(text.contains("\"resource_notification\":\"sent\""));
        assert!(text.contains("\"session_mode\":\"unverified_m0_spike\""));

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            resource_updated.notified(),
        )
        .await?;

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }
}
