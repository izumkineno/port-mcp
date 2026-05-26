use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    app::{InstanceService, PortService},
    mcp::session::SessionMode,
    model::{
        DataBits, DomainError, ErrorCode, FlowControl, HandleId, IdGenerator, InstanceState,
        InstanceSummary, InstanceType, Parity, Payload, PayloadEncoding, SerialConfig, StopBits,
        TcpConfig, TcpMode, Timestamp, ToolResponse, UdpConfig,
    },
    runtime::ClearTarget,
};

#[derive(Clone)]
pub struct PortMcpServer {
    app: Arc<Mutex<InstanceService>>,
    ids: Arc<Mutex<IdGenerator>>,
}

impl PortMcpServer {
    pub fn new() -> Self {
        Self::new_for_tests("20260526")
    }

    pub fn new_for_tests(date: &str) -> Self {
        Self {
            app: Arc::new(Mutex::new(InstanceService::new_for_tests(date))),
            ids: Arc::new(Mutex::new(IdGenerator::new_for_tests(date))),
        }
    }

    fn next_request_id(&self) -> crate::model::RequestId {
        self.ids
            .lock()
            .expect("id generator mutex poisoned")
            .next_request_id()
    }

    fn ok(&self, tool: &str, data: Value) -> ToolResponse {
        ToolResponse::success(tool, self.next_request_id(), Timestamp::now_utc(), data)
    }

    fn err(&self, tool: &str, error: DomainError) -> ToolResponse {
        ToolResponse::failure(tool, self.next_request_id(), Timestamp::now_utc(), error)
    }

    fn result(&self, response: ToolResponse) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        log_tool_response(&response, started_at.elapsed().as_millis() as u64);
        let text = serde_json::to_string(&response)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    fn with_app<T>(&self, operation: impl FnOnce(&mut InstanceService) -> T) -> T {
        let mut app = self.app.lock().expect("app service mutex poisoned");
        operation(&mut app)
    }

    fn session_id(&self, context: &RequestContext<RoleServer>) -> String {
        format!("mcp-session-{:#?}", context.id)
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InstanceCreateParams {
    #[serde(rename = "type")]
    instance_type: InstanceTypeParam,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub enum InstanceTypeParam {
    #[serde(rename = "Serial", alias = "serial", alias = "SERIAL")]
    Serial,
    #[serde(rename = "TCP", alias = "tcp", alias = "Tcp")]
    Tcp,
    #[serde(rename = "UDP", alias = "udp", alias = "Udp")]
    Udp,
}

impl From<InstanceTypeParam> for InstanceType {
    fn from(value: InstanceTypeParam) -> Self {
        match value {
            InstanceTypeParam::Serial => Self::Serial,
            InstanceTypeParam::Tcp => Self::Tcp,
            InstanceTypeParam::Udp => Self::Udp,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HandleParams {
    handle_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InstanceQueryParams {
    handle_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InstanceReleaseParams {
    handle_id: String,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SerialConfigParams {
    handle_id: String,
    port: String,
    #[serde(default = "default_baudrate")]
    baudrate: u32,
    #[serde(default = "default_data_bits")]
    data_bits: DataBitsParam,
    #[serde(default = "default_stop_bits")]
    stop_bits: StopBitsParam,
    #[serde(default)]
    parity: ParityParam,
    #[serde(default)]
    flow_control: FlowControlParam,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default)]
    encoding: EncodingParam,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TcpUdpConfigParams {
    handle_id: String,
    #[serde(default)]
    mode: TcpModeParam,
    host: Option<String>,
    port: Option<u16>,
    bind_host: Option<String>,
    bind_port: Option<u16>,
    remote_host: Option<String>,
    remote_port: Option<u16>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TcpModeParam {
    Client,
    Listen,
}

impl Default for TcpModeParam {
    fn default() -> Self {
        Self::Client
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PortScanParams {
    host: String,
    start_port: u16,
    end_port: u16,
    #[serde(default = "default_scan_concurrency")]
    max_concurrency: usize,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PortSendParams {
    handle_id: String,
    data: String,
    #[serde(default)]
    encoding: EncodingParam,
    #[serde(default)]
    append_line_break: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PortPullParams {
    handle_id: String,
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PortClearParams {
    handle_id: String,
    #[serde(default)]
    target: ClearTargetParam,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SubscribeParams {
    handle_id: String,
    #[serde(default = "default_subscriber_payload_bytes")]
    max_payload_bytes: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ClearTargetParam {
    Tx,
    Rx,
    All,
}

impl Default for ClearTargetParam {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EncodingParam {
    Text,
    Hex,
}

impl Default for EncodingParam {
    fn default() -> Self {
        Self::Text
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub enum DataBitsParam {
    Seven,
    Eight,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub enum StopBitsParam {
    One,
    Two,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub enum ParityParam {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub enum FlowControlParam {
    #[default]
    None,
    Software,
    Hardware,
}

fn default_baudrate() -> u32 {
    115_200
}

fn default_data_bits() -> DataBitsParam {
    DataBitsParam::Eight
}

fn default_stop_bits() -> StopBitsParam {
    StopBitsParam::One
}

fn default_timeout_ms() -> u64 {
    1_000
}

fn default_scan_concurrency() -> usize {
    16
}

fn default_subscriber_payload_bytes() -> usize {
    16 * 1024
}

#[tool_router]
impl PortMcpServer {
    #[tool(description = "Create a Serial, TCP, or UDP port instance")]
    pub async fn instance_create(
        &self,
        Parameters(params): Parameters<InstanceCreateParams>,
    ) -> Result<CallToolResult, McpError> {
        let summary = self.with_app(|app| app.create(params.instance_type.into()));
        self.summary_response("instance_create", summary)
    }

    #[tool(description = "List active port instances")]
    pub async fn instance_list(&self) -> Result<CallToolResult, McpError> {
        let instances = self.with_app(|app| app.list());
        self.result(self.ok("instance_list", json!({ "instances": instances })))
    }

    #[tool(description = "Query an instance by handle id or current session default")]
    pub async fn instance_query(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<InstanceQueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = params.handle_id.as_deref().map(HandleId::from);
        let session_id = self.session_id(&context);
        let summary = self.with_app(|app| app.query(handle.as_ref(), Some(&session_id)));
        self.summary_response("instance_query", summary)
    }

    #[tool(description = "Bind an instance as the current session default")]
    pub async fn instance_use(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<HandleParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let session_id = self.session_id(&context);
        let previous = self.with_app(|app| app.use_instance(Some(&session_id), &handle));
        let response = match previous {
            Ok(previous_handle_id) => self
                .ok(
                    "instance_use",
                    json!({ "bound": true, "previous_handle_id": previous_handle_id }),
                )
                .with_handle(handle),
            Err(error) => self.err("instance_use", error).with_handle(handle),
        };
        self.result(response)
    }

    #[tool(description = "Release an instance")]
    pub async fn instance_release(
        &self,
        Parameters(params): Parameters<InstanceReleaseParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let summary = self.with_app(|app| app.release(&handle, params.force));
        self.summary_response("instance_release", summary)
    }

    #[tool(description = "Configure a Serial instance")]
    pub async fn serial_config(
        &self,
        Parameters(params): Parameters<SerialConfigParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let config = SerialConfig {
            port: params.port,
            baudrate: params.baudrate,
            data_bits: map_data_bits(params.data_bits),
            stop_bits: map_stop_bits(params.stop_bits),
            parity: map_parity(params.parity),
            flow_control: map_flow_control(params.flow_control),
            timeout_ms: params.timeout_ms,
            encoding: map_encoding(params.encoding),
        };
        let summary = self.with_app(|app| app.configure_serial(&handle, config));
        self.summary_response("serial_config", summary)
    }

    #[tool(description = "Configure a TCP or UDP instance")]
    pub async fn tcp_udp_config(
        &self,
        Parameters(params): Parameters<TcpUdpConfigParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let summary = self.with_app(|app| {
            match app
                .query(Some(&handle), None)
                .map(|summary| summary.instance_type)
            {
                Ok(InstanceType::Tcp) => {
                    let host = params
                        .host
                        .clone()
                        .or(params.bind_host.clone())
                        .unwrap_or_default();
                    let port = params.port.or(params.bind_port).unwrap_or_default();
                    app.configure_tcp(
                        &handle,
                        TcpConfig {
                            mode: map_tcp_mode(params.mode),
                            host,
                            port,
                            timeout_ms: params.timeout_ms,
                        },
                    )
                }
                Ok(InstanceType::Udp) => app.configure_udp(
                    &handle,
                    UdpConfig {
                        bind_host: params
                            .bind_host
                            .clone()
                            .or(params.host.clone())
                            .unwrap_or_default(),
                        bind_port: params.bind_port.or(params.port).unwrap_or_default(),
                        remote_host: params.remote_host.clone(),
                        remote_port: params.remote_port,
                        timeout_ms: params.timeout_ms,
                    },
                ),
                Ok(InstanceType::Serial) => Err(DomainError::invalid_argument(
                    ErrorCode::TypeMismatch,
                    "tcp_udp_config cannot configure a Serial instance.",
                    "Use serial_config for Serial instances.",
                )),
                Err(error) => Err(error),
            }
        });
        self.summary_response("tcp_udp_config", summary)
    }

    #[tool(description = "Scan allowed loopback TCP ports")]
    pub async fn port_scan(
        &self,
        Parameters(params): Parameters<PortScanParams>,
    ) -> Result<CallToolResult, McpError> {
        let service = PortService::new_for_tests("20260526");
        let response = match service
            .scan_loopback(
                &params.host,
                params.start_port,
                params.end_port,
                params.max_concurrency,
                params.timeout_ms,
            )
            .await
        {
            Ok(result) => self.ok("port_scan", json!({ "open_ports": result.open_ports })),
            Err(error) => self.err("port_scan", error),
        };
        self.result(response)
    }

    #[tool(description = "Connect a configured instance")]
    pub async fn port_connect(
        &self,
        Parameters(params): Parameters<HandleParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let summary = self.with_app(|app| app.connect(&handle));
        self.summary_response("port_connect", summary)
    }

    #[tool(description = "Disconnect a connected instance")]
    pub async fn port_disconnect(
        &self,
        Parameters(params): Parameters<HandleParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let summary = self.with_app(|app| app.disconnect(&handle));
        self.summary_response("port_disconnect", summary)
    }

    #[tool(description = "Queue bytes for sending")]
    pub async fn port_send(
        &self,
        Parameters(params): Parameters<PortSendParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let payload = match params.encoding {
            EncodingParam::Text => Payload::from_text(&params.data, params.append_line_break),
            EncodingParam::Hex => Payload::from_hex(&params.data, params.append_line_break),
        };
        let response =
            match payload.and_then(|payload| self.with_app(|app| app.send(&handle, &payload))) {
                Ok(result) => self
                    .ok(
                        "port_send",
                        json!({ "queued": result.queued, "sent_bytes": result.sent_bytes }),
                    )
                    .with_handle(handle)
                    .with_state(InstanceState::Connected),
                Err(error) => self.err("port_send", error).with_handle(handle),
            };
        self.result(response)
    }

    #[tool(description = "Pull received bytes")]
    pub async fn port_pull(
        &self,
        Parameters(params): Parameters<PortPullParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let response = match self.with_app(|app| app.pull(&handle, params.max_bytes)) {
            Ok(result) => self
                .ok(
                    "port_pull",
                    json!({
                        "payload": PortService::summarize_payload(&result.bytes, PayloadEncoding::Text),
                        "truncated": result.truncated,
                        "remaining_rx_buffer_bytes": result.remaining_rx_buffer_bytes
                    }),
                )
                .with_handle(handle)
                .with_state(InstanceState::Connected),
            Err(error) => self.err("port_pull", error).with_handle(handle),
        };
        self.result(response)
    }

    #[tool(description = "Clear tx/rx buffers")]
    pub async fn port_clear(
        &self,
        Parameters(params): Parameters<PortClearParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let target = match params.target {
            ClearTargetParam::Tx => ClearTarget::Tx,
            ClearTargetParam::Rx => ClearTarget::Rx,
            ClearTargetParam::All => ClearTarget::All,
        };
        let response = match self.with_app(|app| app.clear(&handle, target)) {
            Ok(result) => self
                .ok(
                    "port_clear",
                    json!({
                        "dropped_tx_items": result.dropped_tx_items,
                        "dropped_tx_bytes": result.dropped_tx_bytes,
                        "dropped_rx_bytes": result.dropped_rx_bytes
                    }),
                )
                .with_handle(handle)
                .with_state(InstanceState::Connected),
            Err(error) => self.err("port_clear", error).with_handle(handle),
        };
        self.result(response)
    }

    #[tool(description = "Subscribe current MCP session to instance stream notifications")]
    pub async fn port_subscribe_stream(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SubscribeParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let session_id = self.session_id(&context);
        let response = match self.with_app(|app| app.subscribe(&handle, &session_id, params.max_payload_bytes)) {
            Ok(result) => self
                .ok("port_subscribe_stream", json!({ "was_subscribed": result.was_subscribed, "session_mode": SessionMode::RequestContextDebug.as_str() }))
                .with_handle(handle)
                .with_state(InstanceState::Connected),
            Err(error) => self.err("port_subscribe_stream", error).with_handle(handle),
        };
        self.result(response)
    }

    #[tool(description = "Unsubscribe current MCP session from instance stream notifications")]
    pub async fn port_unsubscribe_stream(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<HandleParams>,
    ) -> Result<CallToolResult, McpError> {
        let handle = HandleId::from(params.handle_id.as_str());
        let session_id = self.session_id(&context);
        let response = match self.with_app(|app| app.unsubscribe(&handle, &session_id)) {
            Ok(result) => self
                .ok("port_unsubscribe_stream", json!({ "was_subscribed": result.was_subscribed, "session_mode": SessionMode::RequestContextDebug.as_str() }))
                .with_handle(handle),
            Err(error) => self.err("port_unsubscribe_stream", error).with_handle(handle),
        };
        self.result(response)
    }

    fn summary_response(
        &self,
        tool: &str,
        summary: Result<InstanceSummary, DomainError>,
    ) -> Result<CallToolResult, McpError> {
        let response = match summary {
            Ok(summary) => self
                .ok(
                    tool,
                    json!({ "type": summary.instance_type, "summary": summary }),
                )
                .with_handle(summary.handle_id)
                .with_state(summary.state),
            Err(error) => self.err(tool, error),
        };
        self.result(response)
    }
}

fn map_data_bits(value: DataBitsParam) -> DataBits {
    match value {
        DataBitsParam::Seven => DataBits::Seven,
        DataBitsParam::Eight => DataBits::Eight,
    }
}

fn map_stop_bits(value: StopBitsParam) -> StopBits {
    match value {
        StopBitsParam::One => StopBits::One,
        StopBitsParam::Two => StopBits::Two,
    }
}

fn map_parity(value: ParityParam) -> Parity {
    match value {
        ParityParam::None => Parity::None,
        ParityParam::Odd => Parity::Odd,
        ParityParam::Even => Parity::Even,
    }
}

fn map_flow_control(value: FlowControlParam) -> FlowControl {
    match value {
        FlowControlParam::None => FlowControl::None,
        FlowControlParam::Software => FlowControl::Software,
        FlowControlParam::Hardware => FlowControl::Hardware,
    }
}

fn map_encoding(value: EncodingParam) -> PayloadEncoding {
    match value {
        EncodingParam::Text => PayloadEncoding::Text,
        EncodingParam::Hex => PayloadEncoding::Hex,
    }
}

fn map_tcp_mode(value: TcpModeParam) -> TcpMode {
    match value {
        TcpModeParam::Client => TcpMode::Client,
        TcpModeParam::Listen => TcpMode::Listen,
    }
}

impl From<&str> for HandleId {
    fn from(value: &str) -> Self {
        Self::from_string(value)
    }
}

fn log_tool_response(response: &ToolResponse, duration_ms: u64) {
    let event = tool_log_event(response, None, duration_ms);
    tracing::info!(
        event = event["event"].as_str().unwrap_or("tool_call"),
        tool = event["tool"].as_str().unwrap_or_default(),
        request_id = event["request_id"].as_str().unwrap_or_default(),
        handle_id = event["handle_id"].as_str(),
        session = event["session"].as_str(),
        state_before = event["state_before"].as_str(),
        state_after = event["state_after"].as_str(),
        error_code = event["error_code"].as_str(),
        duration_ms,
        sensitive = false,
        "port-mcp tool call completed"
    );
}

fn tool_log_event(response: &ToolResponse, session: Option<&str>, duration_ms: u64) -> Value {
    match serde_json::to_value(response).expect("tool response should serialize") {
        Value::Object(fields) => tool_log_event_value(
            fields
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            fields
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            fields.get("handle_id").and_then(Value::as_str),
            session,
            fields.get("state").and_then(Value::as_str),
            fields.get("state").and_then(Value::as_str),
            fields
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str),
            duration_ms,
        ),
        _ => tool_log_event_value("", "", None, session, None, None, None, duration_ms),
    }
}

fn tool_log_event_value(
    tool: &str,
    request_id: &str,
    handle_id: Option<&str>,
    session: Option<&str>,
    state_before: Option<&str>,
    state_after: Option<&str>,
    error_code: Option<&str>,
    duration_ms: u64,
) -> Value {
    json!({
        "event": "tool_call",
        "tool": tool,
        "request_id": request_id,
        "handle_id": handle_id,
        "session": session,
        "state_before": state_before,
        "state_after": state_after,
        "error_code": error_code,
        "duration_ms": duration_ms,
        "sensitive": false
    })
}

#[cfg(test)]
fn tool_log_event_for_tests(
    tool: &str,
    request_id: &str,
    handle_id: Option<&str>,
    session: Option<&str>,
    state_before: Option<&str>,
    state_after: Option<&str>,
    error_code: Option<&str>,
    duration_ms: u64,
) -> Value {
    tool_log_event_value(
        tool,
        request_id,
        handle_id,
        session,
        state_before,
        state_after,
        error_code,
        duration_ms,
    )
}

#[tool_handler]
impl ServerHandler for PortMcpServer {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::{
        ClientHandler, ServiceExt,
        model::{CallToolRequestParams, CallToolResult},
        object,
    };
    use tokio::sync::Notify;

    use super::PortMcpServer;

    #[derive(Clone)]
    struct SmokeClient {
        _resource_updated: Arc<Notify>,
    }

    impl ClientHandler for SmokeClient {}

    #[tokio::test]
    async fn m7_tool_list_registers_initial_contract_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);

        let server_handle = tokio::spawn(async move {
            PortMcpServer::new_for_tests("20260526")
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            Ok::<(), rmcp::RmcpError>(())
        });

        let client = SmokeClient {
            _resource_updated: Arc::new(Notify::new()),
        }
        .serve(client_transport)
        .await?;

        let tools = client.list_tools(Default::default()).await?;
        let tool_names = tools
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<std::collections::BTreeSet<_>>();

        for expected in [
            "instance_create",
            "instance_list",
            "instance_query",
            "instance_use",
            "instance_release",
            "serial_config",
            "tcp_udp_config",
            "port_scan",
            "port_connect",
            "port_disconnect",
            "port_send",
            "port_pull",
            "port_clear",
            "port_subscribe_stream",
            "port_unsubscribe_stream",
        ] {
            assert!(tool_names.contains(expected), "missing tool {expected}");
        }
        assert!(!tool_names.contains("m0_smoke"));

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m7_instance_handler_returns_unified_response_with_request_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);

        let server_handle = tokio::spawn(async move {
            PortMcpServer::new_for_tests("20260526")
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            Ok::<(), rmcp::RmcpError>(())
        });

        let client = SmokeClient {
            _resource_updated: Arc::new(Notify::new()),
        }
        .serve(client_transport)
        .await?;

        let result = client
            .call_tool(
                CallToolRequestParams::new("instance_create")
                    .with_arguments(object!({ "type": "TCP" })),
            )
            .await?;
        let response = call_result_json(&result);

        assert_eq!(response["ok"], true);
        assert_eq!(response["tool"], "instance_create");
        assert_eq!(response["request_id"], "req_20260526_000001");
        assert_eq!(response["handle_id"], "h_tcp_001");
        assert_eq!(response["state"], "Created");
        assert_eq!(response["data"]["type"], "TCP");

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m7_e2e_smoke_covers_instance_config_port_and_release_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);

        let server_handle = tokio::spawn(async move {
            PortMcpServer::new_for_tests("20260526")
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            Ok::<(), rmcp::RmcpError>(())
        });

        let client = SmokeClient {
            _resource_updated: Arc::new(Notify::new()),
        }
        .serve(client_transport)
        .await?;

        let created =
            call_tool_json(&client, "instance_create", object!({ "type": "TCP" })).await?;
        let handle_id = created["handle_id"].as_str().unwrap();

        let configured = call_tool_json(
            &client,
            "tcp_udp_config",
            object!({
                "handle_id": handle_id,
                "mode": "client",
                "host": "127.0.0.1",
                "port": 9000,
                "timeout_ms": 1000
            }),
        )
        .await?;
        assert_eq!(configured["state"], "Configured");

        assert_eq!(
            call_tool_json(&client, "port_connect", object!({ "handle_id": handle_id })).await?["state"],
            "Connected"
        );
        assert_eq!(
            call_tool_json(
                &client,
                "port_send",
                object!({ "handle_id": handle_id, "data": "ping", "encoding": "text" }),
            )
            .await?["data"]["sent_bytes"],
            4
        );
        assert_eq!(
            call_tool_json(
                &client,
                "port_pull",
                object!({ "handle_id": handle_id, "max_bytes": 16 })
            )
            .await?["ok"],
            true
        );
        assert_eq!(
            call_tool_json(
                &client,
                "port_disconnect",
                object!({ "handle_id": handle_id })
            )
            .await?["state"],
            "Disconnected"
        );
        assert_eq!(
            call_tool_json(
                &client,
                "instance_release",
                object!({ "handle_id": handle_id })
            )
            .await?["state"],
            "Released"
        );

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m7_request_context_is_reflected_in_subscription_response()
    -> Result<(), Box<dyn std::error::Error>> {
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);

        let server_handle = tokio::spawn(async move {
            PortMcpServer::new_for_tests("20260526")
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            Ok::<(), rmcp::RmcpError>(())
        });

        let client = SmokeClient {
            _resource_updated: Arc::new(Notify::new()),
        }
        .serve(client_transport)
        .await?;

        let created =
            call_tool_json(&client, "instance_create", object!({ "type": "TCP" })).await?;
        let handle_id = created["handle_id"].as_str().unwrap();
        call_tool_json(
            &client,
            "tcp_udp_config",
            object!({ "handle_id": handle_id, "mode": "client", "host": "127.0.0.1", "port": 9000 }),
        )
        .await?;
        call_tool_json(&client, "port_connect", object!({ "handle_id": handle_id })).await?;

        let subscribed = call_tool_json(
            &client,
            "port_subscribe_stream",
            object!({ "handle_id": handle_id }),
        )
        .await?;

        assert_eq!(subscribed["request_id"], "req_20260526_000004");
        assert_eq!(subscribed["data"]["session_mode"], "request_context_debug");

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[test]
    fn m8_tool_log_event_contains_correlation_state_duration_and_sensitivity_fields() {
        let event = super::tool_log_event_for_tests(
            "port_send",
            "req_20260526_000123",
            Some("h_tcp_001"),
            Some("mcp-session-1"),
            Some("Connected"),
            Some("Connected"),
            Some("WRITE_IO_FAILED"),
            7,
        );

        assert_eq!(event["event"], "tool_call");
        assert_eq!(event["tool"], "port_send");
        assert_eq!(event["request_id"], "req_20260526_000123");
        assert_eq!(event["handle_id"], "h_tcp_001");
        assert_eq!(event["session"], "mcp-session-1");
        assert_eq!(event["state_before"], "Connected");
        assert_eq!(event["state_after"], "Connected");
        assert_eq!(event["error_code"], "WRITE_IO_FAILED");
        assert_eq!(event["duration_ms"], 7);
        assert_eq!(event["sensitive"], false);
    }

    async fn call_tool_json(
        client: &rmcp::service::RunningService<rmcp::RoleClient, SmokeClient>,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let result = client
            .call_tool(CallToolRequestParams::new(name.to_owned()).with_arguments(arguments))
            .await?;
        Ok(call_result_json(&result))
    }

    fn call_result_json(result: &CallToolResult) -> serde_json::Value {
        let text = result
            .content
            .first()
            .and_then(|content| content.raw.as_text())
            .map(|text| text.text.as_str())
            .expect("tool should return text json content");
        serde_json::from_str(text).expect("tool response should be valid json")
    }
}
