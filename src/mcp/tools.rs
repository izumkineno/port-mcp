use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, handler::server::wrapper::Parameters,
    model::CallToolResult, schemars, service::RequestContext, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    app::{DeviceProbeParams, InstanceService, PortService, run_device_probe},
    mcp::response::{self, PortIoDirection, PortIoLog, PortIoLogConfig},
    mcp::session::SessionMode,
    model::{
        DataBits, DomainError, ErrorCode, FlowControl, HandleId, IdGenerator, InstanceState,
        InstanceSummary, InstanceType, Parity, Payload, PayloadEncoding, RuntimeLimits,
        SerialConfig, StopBits, TcpConfig, TcpMode, Timestamp, ToolResponse, UdpConfig, VisaConfig,
    },
    runtime::{ClearTarget, PullResult},
    util::{
        ModbusMode, ModbusPackRequest, ModbusUnpackRequest, classify_at,
        decode_slip_frame_with_limit, encode_slip_payload_with_limit, hex_to_str_with_limit,
        normalize_scpi, pack_rtu_with_hex_limit, str_to_hex_with_limit, unpack_rtu_with_hex_limit,
    },
};

#[derive(Clone)]
pub struct PortMcpServer {
    app: Arc<Mutex<InstanceService>>,
    ids: Arc<Mutex<IdGenerator>>,
    port_io_log_config: Arc<Mutex<PortIoLogConfig>>,
    debug_profiles: Arc<Mutex<HashMap<String, DebugProfile>>>,
}

impl PortMcpServer {
    pub fn new() -> Self {
        Self {
            app: Arc::new(Mutex::new(InstanceService::new())),
            ids: Arc::new(Mutex::new(IdGenerator::new())),
            port_io_log_config: Arc::new(Mutex::new(PortIoLogConfig::default())),
            debug_profiles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[allow(dead_code)]
    pub fn new_for_tests(date: &str) -> Self {
        Self::new_for_tests_with_limits(date, RuntimeLimits::default())
    }

    #[allow(dead_code)]
    pub fn new_for_tests_with_limits(date: &str, limits: RuntimeLimits) -> Self {
        Self {
            app: Arc::new(Mutex::new(InstanceService::new_for_tests_with_limits(
                date, limits,
            ))),
            ids: Arc::new(Mutex::new(IdGenerator::new_for_tests(date))),
            port_io_log_config: Arc::new(Mutex::new(PortIoLogConfig::default())),
            debug_profiles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn tx_frame_max_bytes(&self) -> usize {
        self.app
            .lock()
            .expect("app service mutex poisoned")
            .tx_frame_max_bytes()
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

    fn result(
        &self,
        started_at: Instant,
        response: ToolResponse,
    ) -> Result<CallToolResult, McpError> {
        let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        response::call_tool_result_with_duration(response, duration_ms)
    }

    fn result_with_port_io(
        &self,
        started_at: Instant,
        response: ToolResponse,
        port_io: Option<PortIoLog>,
    ) -> Result<CallToolResult, McpError> {
        let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        let config = *self
            .port_io_log_config
            .lock()
            .expect("port io log config mutex poisoned");
        response::call_tool_result_with_duration_and_io(response, duration_ms, config, port_io)
    }

    fn with_app<T>(&self, operation: impl FnOnce(&mut InstanceService) -> T) -> T {
        let mut app = self.app.lock().expect("app service mutex poisoned");
        operation(&mut app)
    }

    async fn with_app_blocking<T>(
        &self,
        operation: impl FnOnce(&mut InstanceService) -> T + Send + 'static,
    ) -> T
    where
        T: Send + 'static,
    {
        let app = self.app.clone();
        tokio::task::spawn_blocking(move || {
            let mut app = app.lock().expect("app service mutex poisoned");
            operation(&mut app)
        })
        .await
        .expect("blocking app task should complete")
    }

    fn session_id(&self, context: &RequestContext<RoleServer>) -> String {
        format!("mcp-session-{:#?}", context.peer)
    }

    fn debug_profile_key(&self, context: &RequestContext<RoleServer>) -> String {
        self.session_id(context)
    }

    fn usage_guide_data() -> Value {
        json!({
            "purpose": "Help a new MCP agent use port-mcp correctly when only tool metadata is available.",
            "principles": [
                "Always pass handle_id explicitly after instance_create; do not rely on session defaults unless you intentionally called instance_use.",
                "Normal lifecycle is create -> configure -> connect -> send/pull or subscribe -> disconnect -> release.",
                "Use port_scan before serial_config or visa_config when the resource name is unknown.",
                "Use debug_profile_set, debug_connect, debug_exchange, and debug_close for fast interactive debugging; shortcut tools still return explicit handle_id and phase results.",
                "Use debug_log_config only when troubleshooting raw I/O logs; port_io_log_bytes=0 disables raw I/O logging."
            ],
            "common_sequences": {
                "tcp_client": [
                    { "tool": "instance_create", "arguments": { "type": "TCP" } },
                    { "tool": "tcp_udp_config", "arguments": { "handle_id": "<handle_id>", "mode": "client", "host": "127.0.0.1", "port": 9000, "timeout_ms": 1000 } },
                    { "tool": "port_connect", "arguments": { "handle_id": "<handle_id>" } },
                    { "tool": "port_send", "arguments": { "handle_id": "<handle_id>", "data": "ping", "encoding": "text" } },
                    { "tool": "port_pull", "arguments": { "handle_id": "<handle_id>", "max_bytes": 64 } },
                    { "tool": "port_disconnect", "arguments": { "handle_id": "<handle_id>" } },
                    { "tool": "instance_release", "arguments": { "handle_id": "<handle_id>" } }
                ],
                "tcp_listen": [
                    { "tool": "instance_create", "arguments": { "type": "TCP" } },
                    { "tool": "tcp_udp_config", "arguments": { "handle_id": "<handle_id>", "mode": "listen", "bind_host": "127.0.0.1", "bind_port": 9000, "timeout_ms": 1000 } },
                    { "tool": "port_connect", "arguments": { "handle_id": "<handle_id>" } },
                    { "tool": "instance_query", "arguments": { "handle_id": "<handle_id>" } },
                    { "tool": "port_send", "arguments": { "handle_id": "<handle_id>", "peer_id": "<peer_id>", "data": "ping", "encoding": "text" } },
                    { "tool": "port_send", "arguments": { "handle_id": "<handle_id>", "data": "broadcast", "encoding": "text" } },
                    { "tool": "port_pull", "arguments": { "handle_id": "<handle_id>", "peer_id": "<peer_id>", "max_bytes": 64 } }
                ],
                "udp": [
                    { "tool": "instance_create", "arguments": { "type": "UDP" } },
                    { "tool": "tcp_udp_config", "arguments": { "handle_id": "<handle_id>", "bind_host": "127.0.0.1", "bind_port": 9001, "remote_host": "127.0.0.1", "remote_port": 9002, "timeout_ms": 1000 } },
                    { "tool": "port_connect", "arguments": { "handle_id": "<handle_id>" } }
                ],
                "serial": [
                    { "tool": "port_scan", "arguments": { "type": "Serial", "config": {} } },
                    { "tool": "instance_create", "arguments": { "type": "Serial" } },
                    { "tool": "serial_config", "arguments": { "handle_id": "<handle_id>", "port": "COM3", "baudrate": 115200, "data_bits": "Eight", "stop_bits": "One", "parity": "None", "flow_control": "None", "timeout_ms": 1000 } },
                    { "tool": "port_connect", "arguments": { "handle_id": "<handle_id>" } }
                ],
                "helpers": [
                    { "tool": "str_to_hex", "arguments": { "input_string": "ping" } },
                    { "tool": "hex_to_str", "arguments": { "hex": "70696e67" } },
                    { "tool": "modbus_helper", "arguments": { "action": "pack", "mode": "rtu", "slave_id": 1, "function_code": 3, "address": 16, "data_or_hex": "0002" } },
                    { "tool": "modbus_helper", "arguments": { "action": "unpack", "mode": "rtu", "frame_hex": "010300100002c5ce", "crc_check": true } }
                ],
                "visa": [
                    { "tool": "port_scan", "arguments": { "type": "Visa", "config": { "resource_filter": "?*INSTR", "max_results": 128 } } },
                    { "tool": "instance_create", "arguments": { "type": "Visa" } },
                    { "tool": "visa_config", "arguments": { "handle_id": "<handle_id>", "resource_address": "TCPIP0::192.168.1.10::INSTR", "open_timeout_ms": 1000, "io_timeout_ms": 1000, "encoding": "text" } },
                    { "tool": "port_connect", "arguments": { "handle_id": "<handle_id>" } },
                    { "tool": "port_send", "arguments": { "handle_id": "<handle_id>", "data": "*IDN?", "encoding": "text", "append_line_break": true } },
                    { "tool": "port_pull", "arguments": { "handle_id": "<handle_id>", "max_bytes": 256 } },
                    { "tool": "port_disconnect", "arguments": { "handle_id": "<handle_id>" } }
                ],
                "device_probe": [
                    { "tool": "device_probe", "arguments": { "targets": ["Serial"], "serial": { "ports": ["COM3"], "baudrates": [9600, 115200] }, "payload": { "data": "*IDN?", "encoding": "text", "append_line_break": true }, "matcher": { "kind": "any_response" }, "failure_output": "counts" } }
                ],
                "debug_tcp_client": [
                    { "tool": "debug_profile_set", "arguments": { "transport": "TCP", "tcp": { "mode": "client", "host": "127.0.0.1", "port": 9000, "timeout_ms": 1000 }, "payload": { "encoding": "text", "append_line_break": false }, "pull": { "max_bytes": 64 } } },
                    { "tool": "debug_connect", "arguments": {} },
                    { "tool": "debug_exchange", "arguments": { "data": "ping" } },
                    { "tool": "debug_close", "arguments": { "release": true } }
                ]
            },
            "tool_notes": {
                "instance_create": "Create a Serial, TCP, UDP, or Visa instance. Save data.summary.handle_id or handle_id from the response.",
                "instance_list": "List unreleased instances and their states; useful before choosing a handle_id.",
                "instance_query": "Inspect one instance. Prefer passing handle_id explicitly.",
                "instance_use": "Optional session convenience. Avoid for portable automation unless the client has stable session identity.",
                "instance_release": "Release an instance after disconnect. If state is Connected, pass force=true only when cleanup is intended.",
                "serial_config": "Configure only Serial instances. Required fields: handle_id and port. Common port examples: COM3, /dev/ttyUSB0.",
                "tcp_udp_config": "Configure TCP or UDP instances. TCP client uses host/port; TCP listen uses mode=listen plus bind_host/bind_port; UDP uses bind_host/bind_port and optional remote_host/remote_port.",
                "visa_config": "Configure only Visa instances. Required field: handle_id and resource_address. Optional fields: open_timeout_ms, io_timeout_ms, read_termination, write_termination, encoding. query_idn_on_connect is reserved for later non-blocking identification and is not part of the basic flow.",
                "port_scan": "Serial scan accepts type=Serial and empty config. Visa scan accepts type=Visa and optional resource_filter/max_results. TCP/UDP scans require loopback host, start_port, and end_port in config.",
                "device_probe": "Actively probe Serial and/or Visa resources with a caller-provided payload and bounded matcher. It is temporary probing: it does not create handles, bind debug profiles, or write report files. failure_output defaults to counts; pass samples only when bounded failure details are needed.",
                "debug_profile_set": "Store temporary MCP-layer debug defaults for the current request-context debug session. It does not create, configure, connect, send, pull, clear, disconnect, release, or change debug_log_config.",
                "debug_profile_get": "Return the current debug profile, scope marker, suggested next tools, and whether any bound handle is still valid.",
                "debug_connect": "Create/configure/connect through the normal atomic service path using profile defaults plus overrides. Always keep the returned handle_id, phase results, and cleanup hint.",
                "debug_exchange": "Run optional explicit clear, then send and pull on an already Connected handle. It does not implicitly connect and returns per-phase clear/send/pull results; Serial pull_after performs one extra bounded pull to aggregate split replies.",
                "debug_close": "Disconnect a debug handle by default. Pass release=true only when the instance should also be released; releasing a profile-bound handle invalidates that binding.",
                "port_connect": "Open the configured Serial, TCP, UDP, or Visa resource. Requires state Configured or Disconnected.",
                "port_disconnect": "Close a Connected instance while keeping its config for later reconnect.",
                "port_send": "Send data on a Connected instance. encoding is text or hex; append_line_break defaults to false. For TCP listen, pass peer_id to send to one client; omit peer_id to broadcast to all active clients. Visa instances may append write_termination.",
                "port_pull": "Read received bytes from a Connected instance. max_bytes is optional and bounded by runtime limits. For TCP listen, pass peer_id to filter one client; responses include source metadata. Visa instances may honor read_termination.",
                "port_clear": "Clear tx, rx, or all buffers. target defaults to all. Visa clear maps to the backend instrument clear call.",
                "port_subscribe_stream": "Subscribe current MCP session to receive stream notifications for a Connected instance.",
                "port_unsubscribe_stream": "Cancel a prior stream subscription for the current MCP session.",
                "debug_log_config": "Set raw I/O preview bytes in logs. Use 0 to disable; maximum is 65536.",
                "str_to_hex": "Convert text to hex for protocol framing and audit-safe transport input. encoding defaults to text/UTF-8 in the first slice.",
                "hex_to_str": "Convert hex back to UTF-8 text when the payload is expected to be textual.",
                "modbus_helper": "Pack or unpack Modbus RTU frames. pack accepts optional data_or_hex payload bytes; unpack requires frame_hex. crc_check defaults to true; pass false only for lenient diagnostics that report checksum_valid.",
                "scpi_helper": "Normalize a SCPI command and its optional arguments into a concise audit-safe summary.",
                "at_helper": "Classify a basic AT command into a normalized text summary and response class.",
                "slip_helper": "Encode or decode SLIP payloads using framed hex payloads."
            }
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InstanceCreateParams {
    #[serde(rename = "type")]
    instance_type: InstanceTypeParam,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
pub enum InstanceTypeParam {
    #[serde(rename = "Serial", alias = "serial", alias = "SERIAL")]
    Serial,
    #[serde(rename = "TCP", alias = "tcp", alias = "Tcp")]
    Tcp,
    #[serde(rename = "UDP", alias = "udp", alias = "Udp")]
    Udp,
    #[serde(rename = "Visa", alias = "visa", alias = "VISA")]
    Visa,
}

impl From<InstanceTypeParam> for InstanceType {
    fn from(value: InstanceTypeParam) -> Self {
        match value {
            InstanceTypeParam::Serial => Self::Serial,
            InstanceTypeParam::Tcp => Self::Tcp,
            InstanceTypeParam::Udp => Self::Udp,
            InstanceTypeParam::Visa => Self::Visa,
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
pub struct VisaConfigParams {
    handle_id: String,
    resource_address: String,
    #[serde(default = "default_timeout_ms")]
    open_timeout_ms: u64,
    #[serde(default = "default_timeout_ms")]
    io_timeout_ms: u64,
    read_termination: Option<String>,
    write_termination: Option<String>,
    #[serde(default)]
    encoding: EncodingParam,
    #[serde(default)]
    query_idn_on_connect: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
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
    #[serde(rename = "type")]
    scan_type: InstanceTypeParam,
    #[serde(default)]
    config: PortScanConfigParams,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct PortScanConfigParams {
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    start_port: Option<u16>,
    #[serde(default)]
    end_port: Option<u16>,
    #[serde(default)]
    resource_filter: Option<String>,
    #[serde(default = "default_scan_concurrency")]
    max_concurrency: usize,
    #[serde(default = "default_scan_results")]
    max_results: usize,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PortSendParams {
    handle_id: String,
    data: String,
    peer_id: Option<String>,
    #[serde(default)]
    encoding: EncodingParam,
    #[serde(default)]
    append_line_break: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PortPullParams {
    handle_id: String,
    max_bytes: Option<usize>,
    peer_id: Option<String>,
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
pub struct DebugLogConfigParams {
    #[serde(default)]
    port_io_log_bytes: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DebugProfileParams {
    #[serde(default)]
    transport: Option<InstanceTypeParam>,
    #[serde(default)]
    serial: Option<DebugSerialProfileParams>,
    #[serde(default)]
    tcp: Option<DebugTcpProfileParams>,
    #[serde(default)]
    udp: Option<DebugUdpProfileParams>,
    #[serde(default)]
    visa: Option<DebugVisaProfileParams>,
    #[serde(default)]
    payload: Option<DebugPayloadDefaultsParams>,
    #[serde(default)]
    pull: Option<DebugPullDefaultsParams>,
    #[serde(default)]
    bound_handle_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DebugSerialProfileParams {
    #[serde(default)]
    port: Option<String>,
    #[serde(default)]
    baudrate: Option<u32>,
    #[serde(default)]
    data_bits: Option<DataBitsParam>,
    #[serde(default)]
    stop_bits: Option<StopBitsParam>,
    #[serde(default)]
    parity: Option<ParityParam>,
    #[serde(default)]
    flow_control: Option<FlowControlParam>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    encoding: Option<EncodingParam>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DebugTcpProfileParams {
    #[serde(default)]
    mode: Option<TcpModeParam>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    bind_host: Option<String>,
    #[serde(default)]
    bind_port: Option<u16>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DebugUdpProfileParams {
    #[serde(default)]
    bind_host: Option<String>,
    #[serde(default)]
    bind_port: Option<u16>,
    #[serde(default)]
    remote_host: Option<String>,
    #[serde(default)]
    remote_port: Option<u16>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DebugVisaProfileParams {
    #[serde(default)]
    resource_address: Option<String>,
    #[serde(default)]
    open_timeout_ms: Option<u64>,
    #[serde(default)]
    io_timeout_ms: Option<u64>,
    #[serde(default)]
    read_termination: Option<String>,
    #[serde(default)]
    write_termination: Option<String>,
    #[serde(default)]
    encoding: Option<EncodingParam>,
    #[serde(default)]
    query_idn_on_connect: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DebugPayloadDefaultsParams {
    #[serde(default)]
    encoding: Option<EncodingParam>,
    #[serde(default)]
    append_line_break: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DebugPullDefaultsParams {
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    peer_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct DebugConnectParams {
    #[serde(default)]
    transport: Option<InstanceTypeParam>,
    #[serde(default)]
    serial: Option<DebugSerialProfileParams>,
    #[serde(default)]
    tcp: Option<DebugTcpProfileParams>,
    #[serde(default)]
    udp: Option<DebugUdpProfileParams>,
    #[serde(default)]
    visa: Option<DebugVisaProfileParams>,
    #[serde(default)]
    reuse_bound_handle: bool,
    #[serde(default = "default_true")]
    bind_profile_handle: bool,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DebugClearTargetParam {
    Tx,
    Rx,
    All,
}

#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct DebugExchangeParams {
    #[serde(default)]
    handle_id: Option<String>,
    #[serde(default)]
    clear_before: Option<DebugClearTargetParam>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    encoding: Option<EncodingParam>,
    #[serde(default)]
    append_line_break: Option<bool>,
    #[serde(default = "default_true")]
    pull_after: bool,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    peer_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct DebugCloseParams {
    #[serde(default)]
    handle_id: Option<String>,
    #[serde(default)]
    release: bool,
    #[serde(default)]
    force_release: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
struct DebugProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<InstanceTypeParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    serial: Option<DebugSerialProfileParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tcp: Option<DebugTcpProfileParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    udp: Option<DebugUdpProfileParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    visa: Option<DebugVisaProfileParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<DebugPayloadDefaultsParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pull: Option<DebugPullDefaultsParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bound_handle_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StrToHexParams {
    input_string: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HexToStrParams {
    hex: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ModbusHelperParams {
    action: ModbusActionParam,
    #[serde(default)]
    mode: ModbusModeParam,
    #[serde(default)]
    slave_id: Option<u8>,
    #[serde(default)]
    function_code: Option<u8>,
    #[serde(default)]
    address: Option<u16>,
    #[serde(default)]
    data_or_hex: Option<String>,
    #[serde(default)]
    frame_hex: Option<String>,
    #[serde(default = "default_crc_check")]
    crc_check: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScpiHelperParams {
    action: ScpiActionParam,
    command: String,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default)]
    expect_response: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AtHelperParams {
    command: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SlipHelperParams {
    action: SlipActionParam,
    payload_hex: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScpiActionParam {
    Normalize,
}

impl Default for ScpiActionParam {
    fn default() -> Self {
        Self::Normalize
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SlipActionParam {
    Encode,
    Decode,
}

impl Default for SlipActionParam {
    fn default() -> Self {
        Self::Encode
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ModbusActionParam {
    Pack,
    Unpack,
}

impl Default for ModbusActionParam {
    fn default() -> Self {
        Self::Pack
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ModbusModeParam {
    Rtu,
    Ascii,
}

impl Default for ModbusModeParam {
    fn default() -> Self {
        Self::Rtu
    }
}

fn default_crc_check() -> bool {
    true
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
pub enum DataBitsParam {
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
pub enum StopBitsParam {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub enum ParityParam {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, schemars::JsonSchema)]
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

fn default_scan_results() -> usize {
    128
}

fn default_scan_concurrency() -> usize {
    16
}

fn default_subscriber_payload_bytes() -> usize {
    16 * 1024
}

#[tool_router]
impl PortMcpServer {
    #[tool(
        description = "Return a machine-readable quickstart for new agents: lifecycle, common call sequences, required handle_id usage, and per-tool notes. Call this first when no external documentation is available."
    )]
    pub async fn usage_guide(&self) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        self.result(started_at, self.ok("usage_guide", Self::usage_guide_data()))
    }

    #[tool(
        description = "Create a Serial, TCP, UDP, or Visa instance without opening the resource. First step of every workflow; save the returned handle_id for all later calls."
    )]
    pub async fn instance_create(
        &self,
        Parameters(params): Parameters<InstanceCreateParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let summary = self.with_app(|app| app.create(params.instance_type.into()));
        self.summary_response(started_at, "instance_create", summary)
    }

    #[tool(
        description = "List all unreleased instances with handle_id, type, state, resource summary, and counters. Use when choosing or recovering a handle_id."
    )]
    pub async fn instance_list(&self) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let instances = self.with_app(|app| app.list());
        self.result(
            started_at,
            self.ok("instance_list", json!({ "instances": instances })),
        )
    }

    #[tool(
        description = "Query one instance state, config, counters, buffers, subscribers, and last error. Prefer passing handle_id explicitly; omitted handle_id uses the current session default if available."
    )]
    pub async fn instance_query(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<InstanceQueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = params.handle_id.as_deref().map(HandleId::from);
        let session_id = self.debug_profile_key(&context);
        let summary = self.with_app(|app| app.query(handle.as_ref(), Some(&session_id)));
        self.summary_response(started_at, "instance_query", summary)
    }

    #[tool(
        description = "Optionally bind handle_id as this MCP session's default instance. For reliable automation, still prefer explicit handle_id on later tools."
    )]
    pub async fn instance_use(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<HandleParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = HandleId::from(params.handle_id.as_str());
        let session_id = self.debug_profile_key(&context);
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
        self.result(started_at, response)
    }

    #[tool(
        description = "Release an instance and remove it from instance_list. Disconnect first; use force=true only when intentionally releasing a Connected instance."
    )]
    pub async fn instance_release(
        &self,
        Parameters(params): Parameters<InstanceReleaseParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = HandleId::from(params.handle_id.as_str());
        let summary = self.with_app(|app| app.release(&handle, params.force));
        self.summary_response(started_at, "instance_release", summary)
    }

    #[tool(
        description = "Configure a Serial instance before port_connect. Required: handle_id and port such as COM3 or /dev/ttyUSB0. Defaults: baudrate=115200, data_bits=Eight, stop_bits=One, parity=None, flow_control=None, encoding=text."
    )]
    pub async fn serial_config(
        &self,
        Parameters(params): Parameters<SerialConfigParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
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
        self.summary_response(started_at, "serial_config", summary)
    }

    #[tool(
        description = "Configure a Visa instance before port_connect. Required: handle_id and resource_address. Optional: open_timeout_ms, io_timeout_ms, read_termination, write_termination, encoding, query_idn_on_connect."
    )]
    pub async fn visa_config(
        &self,
        Parameters(params): Parameters<VisaConfigParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = HandleId::from(params.handle_id.as_str());
        let config = VisaConfig {
            resource_address: params.resource_address,
            open_timeout_ms: params.open_timeout_ms,
            io_timeout_ms: params.io_timeout_ms,
            read_termination: params.read_termination,
            write_termination: params.write_termination,
            encoding: map_encoding(params.encoding),
            query_idn_on_connect: params.query_idn_on_connect,
        };
        let summary = self.with_app(|app| app.configure_visa(&handle, config));
        self.summary_response(started_at, "visa_config", summary)
    }

    #[tool(
        description = "Configure a TCP or UDP instance before port_connect. TCP client uses mode=client, host, port. TCP listen uses mode=listen, bind_host, bind_port. UDP uses bind_host, bind_port, and optional remote_host/remote_port."
    )]
    pub async fn tcp_udp_config(
        &self,
        Parameters(params): Parameters<TcpUdpConfigParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
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
                Ok(InstanceType::Visa) => Err(DomainError::invalid_argument(
                    ErrorCode::TypeMismatch,
                    "tcp_udp_config cannot configure a Visa instance.",
                    "Use visa_config for Visa instances.",
                )),
                Err(error) => Err(error),
            }
        });
        self.summary_response(started_at, "tcp_udp_config", summary)
    }

    #[tool(
        description = "Scan available resources. For Serial, pass type=Serial and config={}. For Visa, pass type=Visa and optional resource_filter/max_results. For TCP/UDP, pass loopback config with host, start_port, end_port, optional max_concurrency and timeout_ms."
    )]
    pub async fn port_scan(
        &self,
        Parameters(params): Parameters<PortScanParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        match params.scan_type {
            InstanceTypeParam::Serial => {
                let service = PortService::new();
                let response = match service.scan_serial() {
                    Ok(resources) => self.ok("port_scan", json!({ "resources": resources })),
                    Err(error) => self.err("port_scan", error),
                };
                self.result(started_at, response)
            }
            InstanceTypeParam::Visa => {
                let service = PortService::new();
                let response = match service.scan_visa(
                    params
                        .config
                        .resource_filter
                        .as_deref()
                        .unwrap_or("?*INSTR"),
                    params.config.max_results,
                ) {
                    Ok(resources) => {
                        self.ok("port_scan", json!({ "resources": resources.resources }))
                    }
                    Err(error) => self.err("port_scan", error),
                };
                self.result(started_at, response)
            }
            InstanceTypeParam::Tcp | InstanceTypeParam::Udp => {
                let response = self.port_scan_network(started_at, params.config).await?;
                Ok(response)
            }
        }
    }

    #[tool(
        description = "Actively probe Serial and/or Visa resources with a caller-provided payload and bounded matcher. Uses resource-level concurrency, does not create persistent handles, and returns successful configuration summaries plus compact counts."
    )]
    pub async fn device_probe(
        &self,
        Parameters(params): Parameters<DeviceProbeParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let response = match run_device_probe(params, RuntimeLimits::default()).await {
            Ok(result) => self.ok("device_probe", json!(result)),
            Err(error) => self.err("device_probe", error),
        };
        self.result(started_at, response)
    }

    #[tool(
        description = "Store temporary MCP-layer debug defaults for this request-context debug session. It does not create, configure, connect, send, pull, clear, disconnect, release, or change debug_log_config."
    )]
    pub async fn debug_profile_set(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<DebugProfileParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let session_id = self.debug_profile_key(&context);
        let profile = DebugProfile::from_params(params);
        self.debug_profiles
            .lock()
            .expect("debug profiles mutex poisoned")
            .insert(session_id, profile.clone());
        let data = self.debug_profile_data(&profile);
        self.result(started_at, self.ok("debug_profile_set", data))
    }

    #[tool(
        description = "Return the temporary MCP-layer debug profile, scope marker, suggested next tools, and whether the bound handle is still valid."
    )]
    pub async fn debug_profile_get(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let session_id = self.debug_profile_key(&context);
        let profile = self
            .debug_profiles
            .lock()
            .expect("debug profiles mutex poisoned")
            .get(&session_id)
            .cloned();
        let data = match profile {
            Some(profile) => self.debug_profile_data(&profile),
            None => json!({
                "scope": self.debug_scope(),
                "profile": null,
                "derived_defaults": null,
                "bound_handle": null,
                "suggested_next_tools": ["debug_profile_set"]
            }),
        };
        self.result(started_at, self.ok("debug_profile_get", data))
    }

    #[tool(
        description = "Create, configure, and connect through the normal atomic service path using debug profile defaults plus overrides. Always returns handle_id, phase results, and cleanup guidance."
    )]
    pub async fn debug_connect(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<DebugConnectParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let session_id = self.debug_profile_key(&context);
        let profile = self.debug_profile_for_session(&session_id);
        let transport = params
            .transport
            .or(profile.as_ref().and_then(|profile| profile.transport))
            .ok_or_else(|| {
                DomainError::invalid_argument(
                    ErrorCode::MissingRequiredField,
                    "debug_connect requires a transport after merging debug profile defaults.",
                    "Pass transport or call debug_profile_set with a transport.",
                )
                .with_detail("field", json!("transport"))
            });
        let transport = match transport {
            Ok(transport) => transport,
            Err(error) => return self.result(started_at, self.err("debug_connect", error)),
        };
        let config = match self.debug_config_for_transport(transport, &profile, &params) {
            Ok(config) => config,
            Err(error) => return self.result(started_at, self.err("debug_connect", error)),
        };
        let reuse_requested = params.reuse_bound_handle;
        let bind_profile_handle = params.bind_profile_handle;
        let response = self.with_app(|app| {
            let mut phases = Vec::new();
            let create_summary = match app.create(InstanceType::from(transport)) {
                Ok(summary) => summary,
                Err(error) => return self.debug_phase_failure("debug_connect", phases, "create", None, error),
            };
            let handle = create_summary.handle_id.clone();
            phases.push(debug_phase("create", &create_summary, json!({ "reused": false })));

            let configured = match configure_debug_instance(app, &handle, config) {
                Ok(summary) => summary,
                Err(error) => return self.debug_phase_failure("debug_connect", phases, "configure", Some(handle), error),
            };
            phases.push(debug_phase("configure", &configured, json!({ "summary": configured })));

            let connected = match app.connect(&handle) {
                Ok(summary) => summary,
                Err(error) => return self.debug_phase_failure("debug_connect", phases, "connect", Some(handle), error),
            };
            phases.push(debug_phase("connect", &connected, json!({ "summary": connected })));

            let mut bound_to_profile = false;
            if bind_profile_handle {
                if let Ok(previous_handle_id) = app.use_instance(Some(&session_id), &handle) {
                    phases.push(json!({
                        "phase": "profile_bind",
                        "ok": true,
                        "handle_id": handle,
                        "previous_handle_id": previous_handle_id
                    }));
                    bound_to_profile = true;
                }
            }
            if bound_to_profile {
                self.bind_profile_handle(&session_id, &handle);
            }
            self.ok(
                "debug_connect",
                json!({
                    "scope": self.debug_scope(),
                    "handle_id": handle,
                    "type": connected.instance_type,
                    "state": connected.state,
                    "reused": false,
                    "reuse_requested": reuse_requested,
                    "bound_to_profile": bound_to_profile,
                    "summary": connected,
                    "phases": phases,
                    "cleanup_hint": "Call debug_close with this handle_id; pass release=true when the instance should be removed."
                }),
            )
            .with_handle(handle)
            .with_state(InstanceState::Connected)
        });
        self.result(started_at, response)
    }

    #[tool(
        description = "Run optional explicit clear, then send and pull on an already Connected handle. It does not connect implicitly and returns clear/send/pull phase results. For Serial, pull_after performs one extra bounded pull to aggregate split replies."
    )]
    pub async fn debug_exchange(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<DebugExchangeParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let session_id = self.debug_profile_key(&context);
        let profile = self.debug_profile_for_session(&session_id);
        let used_profile_handle = params.handle_id.is_none();
        let handle = match params.handle_id.clone().or_else(|| {
            profile
                .as_ref()
                .and_then(|profile| profile.bound_handle_id.clone())
        }) {
            Some(handle_id) => HandleId::from(handle_id.as_str()),
            None => {
                let error = DomainError::invalid_argument(
                    ErrorCode::MissingRequiredField,
                    "debug_exchange requires handle_id or a valid profile-bound handle.",
                    "Pass handle_id, call debug_connect, or bind a handle with debug_profile_set.",
                )
                .with_detail("field", json!("handle_id"));
                return self.result(started_at, self.err("debug_exchange", error));
            }
        };
        let payload_defaults = profile
            .as_ref()
            .and_then(|profile| profile.payload.as_ref());
        let pull_defaults = profile.as_ref().and_then(|profile| profile.pull.as_ref());
        let encoding = params
            .encoding
            .or_else(|| payload_defaults.and_then(|payload| payload.encoding))
            .unwrap_or_default();
        let append_line_break = params
            .append_line_break
            .or_else(|| payload_defaults.and_then(|payload| payload.append_line_break))
            .unwrap_or(false);
        let max_bytes = params
            .max_bytes
            .or_else(|| pull_defaults.and_then(|pull| pull.max_bytes));
        let peer_id = params
            .peer_id
            .clone()
            .or_else(|| pull_defaults.and_then(|pull| pull.peer_id.clone()));
        let tx_limit = self.tx_frame_max_bytes();
        let response = self.with_app(|app| {
            let mut phases = Vec::new();
            let instance_type = match app.query(Some(&handle), None) {
                Ok(summary) if summary.state == InstanceState::Connected => summary.instance_type,
                Ok(summary) => {
                    let error = DomainError::new(
                        crate::model::ErrorCategory::InvalidState,
                        ErrorCode::StateNotAllowed,
                        "debug_exchange requires a Connected instance.",
                        "Call port_connect or debug_connect first; debug_exchange never connects implicitly.",
                        false,
                    )
                    .with_detail("current_state", json!(summary.state));
                    return self.debug_phase_failure("debug_exchange", phases, "preflight", Some(handle), error);
                }
                Err(error) => return self.debug_phase_failure("debug_exchange", phases, "preflight", Some(handle), error),
            };
            if let Some(target) = params.clear_before {
                let target = map_debug_clear_target(target);
                match app.clear(&handle, target) {
                    Ok(result) => phases.push(json!({
                        "phase": "clear",
                        "ok": true,
                        "target": debug_clear_target_name(target),
                        "dropped_tx_items": result.dropped_tx_items,
                        "dropped_tx_bytes": result.dropped_tx_bytes,
                        "dropped_rx_bytes": result.dropped_rx_bytes
                    })),
                    Err(error) => return self.debug_phase_failure("debug_exchange", phases, "clear", Some(handle), error),
                }
            }
            if let Some(data) = params.data {
                let payload = match encoding {
                    EncodingParam::Text => Payload::from_text_with_limit(&data, append_line_break, tx_limit),
                    EncodingParam::Hex => Payload::from_hex_with_limit(&data, append_line_break, tx_limit),
                };
                let payload = match payload {
                    Ok(payload) => payload,
                    Err(error) => return self.debug_phase_failure("debug_exchange", phases, "send", Some(handle), error),
                };
                match app.send(&handle, &payload, peer_id.as_deref()) {
                    Ok(result) => phases.push(json!({
                        "phase": "send",
                        "ok": true,
                        "queued": result.queued,
                        "sent_bytes": result.sent_bytes,
                        "target": debug_send_target(result.target)
                    })),
                    Err(error) => return self.debug_phase_failure("debug_exchange", phases, "send", Some(handle), error),
                }
            }
            if params.pull_after {
                match debug_exchange_pull(app, &handle, instance_type, max_bytes, peer_id.as_deref()) {
                    Ok((result, pull_attempts, completed_after_timeout)) => phases.push(json!({
                        "phase": "pull",
                        "ok": true,
                        "payload": PortService::summarize_payload(&result.bytes, PayloadEncoding::Text),
                        "truncated": result.truncated,
                        "remaining_rx_buffer_bytes": result.remaining_rx_buffer_bytes,
                        "pull_attempts": pull_attempts,
                        "completed_after_timeout": completed_after_timeout,
                        "source": debug_pull_source(result.source)
                    })),
                    Err(error) => return self.debug_phase_failure("debug_exchange", phases, "pull", Some(handle), error),
                }
            }
            self.ok(
                "debug_exchange",
                json!({
                    "scope": self.debug_scope(),
                    "handle_id": handle,
                    "state": InstanceState::Connected,
                    "used_profile_handle": used_profile_handle,
                    "phases": phases,
                    "cleanup_hint": "Call debug_close with this handle_id when the debug session is done."
                }),
            )
            .with_handle(handle)
            .with_state(InstanceState::Connected)
        });
        self.result(started_at, response)
    }

    #[tool(
        description = "Disconnect a debug handle by default and optionally release it with release=true. Releasing a profile-bound handle invalidates that binding."
    )]
    pub async fn debug_close(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<DebugCloseParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let session_id = self.debug_profile_key(&context);
        let profile = self.debug_profile_for_session(&session_id);
        let handle = match params.handle_id.clone().or_else(|| {
            profile
                .as_ref()
                .and_then(|profile| profile.bound_handle_id.clone())
        }) {
            Some(handle_id) => HandleId::from(handle_id.as_str()),
            None => {
                let error = DomainError::invalid_argument(
                    ErrorCode::MissingRequiredField,
                    "debug_close requires handle_id or a profile-bound handle.",
                    "Pass handle_id or call debug_profile_get to inspect the current debug profile.",
                )
                .with_detail("field", json!("handle_id"));
                return self.result(started_at, self.err("debug_close", error));
            }
        };
        let response = self.with_app(|app| {
            let mut phases = Vec::new();
            let disconnected = match app.disconnect(&handle) {
                Ok(summary) => summary,
                Err(error) => {
                    return self.debug_phase_failure(
                        "debug_close",
                        phases,
                        "disconnect",
                        Some(handle),
                        error,
                    );
                }
            };
            phases.push(debug_phase(
                "disconnect",
                &disconnected,
                json!({ "summary": disconnected }),
            ));
            let mut final_summary = disconnected;
            if params.release {
                final_summary = match app.release(&handle, params.force_release) {
                    Ok(summary) => summary,
                    Err(error) => {
                        return self.debug_phase_failure(
                            "debug_close",
                            phases,
                            "release",
                            Some(handle),
                            error,
                        );
                    }
                };
                phases.push(debug_phase(
                    "release",
                    &final_summary,
                    json!({ "summary": final_summary }),
                ));
                self.unbind_profile_handle(&session_id, &handle);
            }
            self.ok(
                "debug_close",
                json!({
                    "scope": self.debug_scope(),
                    "handle_id": handle,
                    "state": final_summary.state,
                    "released": params.release,
                    "phases": phases,
                    "summary": final_summary
                }),
            )
            .with_handle(handle)
            .with_state(final_summary.state)
        });
        self.result(started_at, response)
    }

    async fn port_scan_network(
        &self,
        started_at: Instant,
        config: PortScanConfigParams,
    ) -> Result<CallToolResult, McpError> {
        let limits = RuntimeLimits::default();
        if config.timeout_ms == 0 || config.timeout_ms > limits.scan_total_timeout_ms {
            return self.result(
                started_at,
                self.err(
                    "port_scan",
                    DomainError::invalid_argument(
                        ErrorCode::InvalidRange,
                        "port_scan timeout_ms is outside the allowed range.",
                        "Use a timeout between 1 and the configured scan_total_timeout_ms.",
                    )
                    .with_detail("field", json!("timeout_ms"))
                    .with_detail("min", json!(1))
                    .with_detail("max", json!(limits.scan_total_timeout_ms))
                    .with_detail("actual", json!(config.timeout_ms)),
                ),
            );
        }
        let Some(host) = config.host else {
            return self.result(started_at, self.missing_scan_config_field("host"));
        };
        let Some(start_port) = config.start_port else {
            return self.result(started_at, self.missing_scan_config_field("start_port"));
        };
        let Some(end_port) = config.end_port else {
            return self.result(started_at, self.missing_scan_config_field("end_port"));
        };
        let service = PortService::new();
        let response = match service
            .scan_loopback(
                &host,
                start_port,
                end_port,
                config.max_concurrency,
                config.timeout_ms,
            )
            .await
        {
            Ok(result) => self.ok("port_scan", json!({ "open_ports": result.open_ports })),
            Err(error) => self.err("port_scan", error),
        };
        self.result(started_at, response)
    }

    fn missing_scan_config_field(&self, field: &str) -> ToolResponse {
        self.err(
            "port_scan",
            DomainError::invalid_argument(
                ErrorCode::MissingRequiredField,
                format!("port_scan config.{field} is required for network scans."),
                "Pass host, start_port, and end_port inside config for TCP or UDP scans.",
            )
            .with_detail("field", json!(field)),
        )
    }

    #[tool(
        description = "Open a Configured or Disconnected instance so it can send, pull, or subscribe. Requires explicit handle_id."
    )]
    pub async fn port_connect(
        &self,
        Parameters(params): Parameters<HandleParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = HandleId::from(params.handle_id.as_str());
        let summary = self
            .with_app_blocking(move |app| app.connect(&handle))
            .await;
        self.summary_response(started_at, "port_connect", summary)
    }

    #[tool(
        description = "Close a Connected instance while keeping its configuration for reconnect or release. Requires explicit handle_id."
    )]
    pub async fn port_disconnect(
        &self,
        Parameters(params): Parameters<HandleParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = HandleId::from(params.handle_id.as_str());
        let summary = self
            .with_app_blocking(move |app| app.disconnect(&handle))
            .await;
        self.summary_response(started_at, "port_disconnect", summary)
    }

    #[tool(
        description = "Send payload on a Connected instance. Required: handle_id and data. encoding defaults to text; use encoding=hex for hexadecimal bytes. append_line_break defaults to false. Visa instances may append write_termination."
    )]
    pub async fn port_send(
        &self,
        Parameters(params): Parameters<PortSendParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = HandleId::from(params.handle_id.as_str());
        let handle_for_app = handle.clone();
        let peer_id = params.peer_id.clone();
        let tx_limit = self.tx_frame_max_bytes();
        let payload = match params.encoding {
            EncodingParam::Text => {
                Payload::from_text_with_limit(&params.data, params.append_line_break, tx_limit)
            }
            EncodingParam::Hex => {
                Payload::from_hex_with_limit(&params.data, params.append_line_break, tx_limit)
            }
        };
        let response = match payload {
            Ok(payload) => {
                let port_io =
                    PortIoLog::new(PortIoDirection::Tx, payload.bytes.clone(), payload.encoding);
                let response = match self
                    .with_app_blocking(move |app| {
                        app.send(&handle_for_app, &payload, peer_id.as_deref())
                    })
                    .await
                {
                    Ok(result) => {
                        let mut data = json!({
                            "queued": result.queued,
                            "sent_bytes": result.sent_bytes
                        });
                        if let Some(target) = result.target {
                            data["target"] = json!({
                                "mode": match target.mode {
                                    crate::runtime::SendTargetMode::Peer => "peer",
                                    crate::runtime::SendTargetMode::Broadcast => "broadcast",
                                },
                                "peer_id": target.peer_id,
                                "peer_count": target.peer_count,
                                "successful_peer_ids": target.successful_peer_ids,
                                "failed_peer_count": target.failed_peer_count,
                            });
                        }
                        self.ok("port_send", data)
                            .with_handle(handle)
                            .with_state(InstanceState::Connected)
                    }
                    Err(error) => self.err("port_send", error).with_handle(handle),
                };
                return self.result_with_port_io(started_at, response, Some(port_io));
            }
            Err(error) => self.err("port_send", error).with_handle(handle),
        };
        self.result(started_at, response)
    }

    #[tool(
        description = "Pull received bytes from a Connected instance rx buffer. Required: handle_id. Optional max_bytes controls returned payload summary size within runtime limits. Visa instances may honor read_termination."
    )]
    pub async fn port_pull(
        &self,
        Parameters(params): Parameters<PortPullParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = HandleId::from(params.handle_id.as_str());
        let handle_for_app = handle.clone();
        let peer_id = params.peer_id.clone();
        let response = match self
            .with_app_blocking(move |app| {
                app.pull(&handle_for_app, params.max_bytes, peer_id.as_deref())
            })
            .await
        {
            Ok(result) => {
                let port_io = PortIoLog::new(
                    PortIoDirection::Rx,
                    result.bytes.clone(),
                    PayloadEncoding::Text,
                );
                let mut data = json!({
                    "payload": PortService::summarize_payload(&result.bytes, PayloadEncoding::Text),
                    "truncated": result.truncated,
                    "remaining_rx_buffer_bytes": result.remaining_rx_buffer_bytes
                });
                if let Some(source) = result.source {
                    data["source"] = json!({
                        "transport": source.transport,
                        "peer_id": source.peer_id,
                        "remote_addr": source.remote_addr,
                    });
                }
                let response = self
                    .ok("port_pull", data)
                    .with_handle(handle)
                    .with_state(InstanceState::Connected);
                return self.result_with_port_io(started_at, response, Some(port_io));
            }
            Err(error) => self.err("port_pull", error).with_handle(handle),
        };
        self.result(started_at, response)
    }

    #[tool(
        description = "Configure raw port I/O preview in server logs for troubleshooting port_send and port_pull. port_io_log_bytes=0 disables raw I/O logging; maximum is 65536."
    )]
    pub async fn debug_log_config(
        &self,
        Parameters(params): Parameters<DebugLogConfigParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let max_allowed = 65_536usize;
        if params.port_io_log_bytes > max_allowed {
            return self.result(
                started_at,
                self.err(
                    "debug_log_config",
                    DomainError::invalid_argument(
                        ErrorCode::InvalidRange,
                        "debug_log_config port_io_log_bytes is outside the allowed range.",
                        "Use a value between 0 and 65536 bytes.",
                    )
                    .with_detail("field", json!("port_io_log_bytes"))
                    .with_detail("max", json!(max_allowed))
                    .with_detail("actual", json!(params.port_io_log_bytes)),
                ),
            );
        }
        *self
            .port_io_log_config
            .lock()
            .expect("port io log config mutex poisoned") = PortIoLogConfig {
            max_bytes: params.port_io_log_bytes,
        };
        self.result(
            started_at,
            self.ok(
                "debug_log_config",
                json!({ "port_io_log_bytes": params.port_io_log_bytes }),
            ),
        )
    }

    #[tool(
        description = "Convert text to hex for protocol framing or audit-safe transport input. input_string is required. The first slice supports UTF-8 text only."
    )]
    pub async fn str_to_hex(
        &self,
        Parameters(params): Parameters<StrToHexParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let helper_limit = RuntimeLimits::HELPER_MAX_INPUT_BYTES;
        let response =
            match str_to_hex_with_limit(&params.input_string, "input_string", helper_limit) {
                Ok(hex) => self.ok(
                    "str_to_hex",
                    json!({
                        "hex": hex,
                        "input_bytes": params.input_string.len(),
                    }),
                ),
                Err(error) => self.err("str_to_hex", error),
            };
        self.result(started_at, response)
    }

    #[tool(
        description = "Convert hex back to UTF-8 text when the payload is expected to be textual. hex is required."
    )]
    pub async fn hex_to_str(
        &self,
        Parameters(params): Parameters<HexToStrParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let helper_limit = RuntimeLimits::HELPER_MAX_INPUT_BYTES;
        let response = match hex_to_str_with_limit(&params.hex, "hex", helper_limit) {
            Ok(text) => self.ok(
                "hex_to_str",
                json!({
                    "text": text,
                    "input_bytes": params.hex.len() / 2,
                }),
            ),
            Err(error) => self.err("hex_to_str", error),
        };
        self.result(started_at, response)
    }

    #[tool(
        description = "Pack or unpack Modbus RTU frames. First slice supports action=pack or unpack and mode=rtu only. pack requires slave_id, function_code, address, and optional data_or_hex; unpack requires frame_hex."
    )]
    pub async fn modbus_helper(
        &self,
        Parameters(params): Parameters<ModbusHelperParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let response = match params.action {
            ModbusActionParam::Pack => {
                let slave_id = match required_modbus_u8("slave_id", params.slave_id) {
                    Ok(value) => value,
                    Err(error) => return self.result(started_at, self.err("modbus_helper", error)),
                };
                let function_code = match required_modbus_u8("function_code", params.function_code)
                {
                    Ok(value) => value,
                    Err(error) => return self.result(started_at, self.err("modbus_helper", error)),
                };
                let address = match required_modbus_u16("address", params.address) {
                    Ok(value) => value,
                    Err(error) => return self.result(started_at, self.err("modbus_helper", error)),
                };
                let request = ModbusPackRequest {
                    mode: map_modbus_mode(params.mode),
                    slave_id,
                    function_code,
                    address,
                    data_or_hex: params.data_or_hex,
                    crc_check: params.crc_check,
                };
                match pack_rtu_with_hex_limit(request, RuntimeLimits::HELPER_MAX_INPUT_BYTES) {
                    Ok(result) => self.ok(
                        "modbus_helper",
                        json!({
                            "action": "pack",
                            "mode": "rtu",
                            "frame_hex": result.frame_hex,
                            "frame_bytes": result.frame_bytes,
                            "crc_hex": result.crc_hex,
                        }),
                    ),
                    Err(error) => self.err("modbus_helper", error),
                }
            }
            ModbusActionParam::Unpack => {
                let frame_hex = match required_modbus_string("frame_hex", params.frame_hex) {
                    Ok(value) => value,
                    Err(error) => return self.result(started_at, self.err("modbus_helper", error)),
                };
                let request = ModbusUnpackRequest {
                    mode: map_modbus_mode(params.mode),
                    frame_hex,
                    crc_check: params.crc_check,
                };
                match unpack_rtu_with_hex_limit(request, RuntimeLimits::HELPER_MAX_INPUT_BYTES) {
                    Ok(result) => self.ok(
                        "modbus_helper",
                        json!({
                            "action": "unpack",
                            "mode": "rtu",
                            "slave_id": result.slave_id,
                            "function_code": result.function_code,
                            "address": result.address,
                            "data_hex": result.data_hex,
                            "crc_hex": result.crc_hex,
                            "checksum_valid": result.checksum_valid,
                        }),
                    ),
                    Err(error) => self.err("modbus_helper", error),
                }
            }
        };
        self.result(started_at, response)
    }

    #[tool(
        description = "Normalize a SCPI command and optional arguments into a compact summary. action currently supports normalize only."
    )]
    pub async fn scpi_helper(
        &self,
        Parameters(params): Parameters<ScpiHelperParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let response = match params.action {
            ScpiActionParam::Normalize => match normalize_scpi(
                &params.command,
                params.arguments.as_deref(),
                params.expect_response.as_deref(),
            ) {
                Ok(summary) => self.ok(
                    "scpi_helper",
                    json!({
                        "kind": "scpi",
                        "normalized": summary.normalized,
                        "response_class": summary.response_class,
                    }),
                ),
                Err(error) => self.err("scpi_helper", error),
            },
        };
        self.result(started_at, response)
    }

    #[tool(
        description = "Classify a basic AT command into a normalized text summary and response class."
    )]
    pub async fn at_helper(
        &self,
        Parameters(params): Parameters<AtHelperParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let response = match classify_at(&params.command) {
            Ok(summary) => self.ok(
                "at_helper",
                json!({
                    "kind": "at",
                    "normalized": summary.normalized,
                    "response_class": summary.response_class,
                }),
            ),
            Err(error) => self.err("at_helper", error),
        };
        self.result(started_at, response)
    }

    #[tool(
        description = "Encode or decode SLIP payloads using framed hex payloads. action supports encode and decode."
    )]
    pub async fn slip_helper(
        &self,
        Parameters(params): Parameters<SlipHelperParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let response = match params.action {
            SlipActionParam::Encode => match encode_slip_payload_with_limit(
                &params.payload_hex,
                RuntimeLimits::HELPER_MAX_INPUT_BYTES,
            ) {
                Ok(summary) => self.ok(
                    "slip_helper",
                    json!({
                        "kind": "slip",
                        "normalized": summary.normalized,
                        "payload_hex": summary.payload_hex,
                    }),
                ),
                Err(error) => self.err("slip_helper", error),
            },
            SlipActionParam::Decode => match decode_slip_frame_with_limit(
                &params.payload_hex,
                RuntimeLimits::HELPER_MAX_INPUT_BYTES,
            ) {
                Ok(summary) => self.ok(
                    "slip_helper",
                    json!({
                        "kind": "slip",
                        "normalized": summary.normalized,
                        "payload_hex": summary.payload_hex,
                        "response_class": summary.response_class,
                    }),
                ),
                Err(error) => self.err("slip_helper", error),
            },
        };
        self.result(started_at, response)
    }

    #[tool(
        description = "Clear buffered data for an instance. Required: handle_id. target defaults to all; valid targets are tx, rx, and all. Visa clear maps to the backend instrument clear call."
    )]
    pub async fn port_clear(
        &self,
        Parameters(params): Parameters<PortClearParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
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
        self.result(started_at, response)
    }

    #[tool(
        description = "Subscribe the current MCP session to receive stream notifications for a Connected instance. Required: handle_id. Optional max_payload_bytes defaults to 16384."
    )]
    pub async fn port_subscribe_stream(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SubscribeParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = HandleId::from(params.handle_id.as_str());
        let session_id = self.session_id(&context);
        let response = match self.with_app(|app| app.subscribe(&handle, &session_id, params.max_payload_bytes)) {
            Ok(result) => self
                .ok("port_subscribe_stream", json!({ "was_subscribed": result.was_subscribed, "session_mode": SessionMode::RequestContextDebug.as_str() }))
                .with_handle(handle)
                .with_state(InstanceState::Connected),
            Err(error) => self.err("port_subscribe_stream", error).with_handle(handle),
        };
        self.result(started_at, response)
    }

    #[tool(
        description = "Unsubscribe the current MCP session from stream notifications for an instance. Required: handle_id. Repeated unsubscribe returns was_subscribed=false."
    )]
    pub async fn port_unsubscribe_stream(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<HandleParams>,
    ) -> Result<CallToolResult, McpError> {
        let started_at = Instant::now();
        let handle = HandleId::from(params.handle_id.as_str());
        let session_id = self.session_id(&context);
        let response = match self.with_app(|app| app.unsubscribe(&handle, &session_id)) {
            Ok(result) => self
                .ok("port_unsubscribe_stream", json!({ "was_subscribed": result.was_subscribed, "session_mode": SessionMode::RequestContextDebug.as_str() }))
                .with_handle(handle),
            Err(error) => self.err("port_unsubscribe_stream", error).with_handle(handle),
        };
        self.result(started_at, response)
    }

    fn summary_response(
        &self,
        started_at: Instant,
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
        self.result(started_at, response)
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

fn map_debug_clear_target(value: DebugClearTargetParam) -> ClearTarget {
    match value {
        DebugClearTargetParam::Tx => ClearTarget::Tx,
        DebugClearTargetParam::Rx => ClearTarget::Rx,
        DebugClearTargetParam::All => ClearTarget::All,
    }
}

fn debug_clear_target_name(value: ClearTarget) -> &'static str {
    match value {
        ClearTarget::Tx => "tx",
        ClearTarget::Rx => "rx",
        ClearTarget::All => "all",
    }
}

enum DebugConfig {
    Serial(SerialConfig),
    Tcp(TcpConfig),
    Udp(UdpConfig),
    Visa(VisaConfig),
}

fn configure_debug_instance(
    app: &mut InstanceService,
    handle: &HandleId,
    config: DebugConfig,
) -> Result<InstanceSummary, DomainError> {
    match config {
        DebugConfig::Serial(config) => app.configure_serial(handle, config),
        DebugConfig::Tcp(config) => app.configure_tcp(handle, config),
        DebugConfig::Udp(config) => app.configure_udp(handle, config),
        DebugConfig::Visa(config) => app.configure_visa(handle, config),
    }
}

fn debug_phase(phase: &str, summary: &InstanceSummary, data: Value) -> Value {
    json!({
        "phase": phase,
        "ok": true,
        "handle_id": summary.handle_id,
        "state": summary.state,
        "data": data
    })
}

fn debug_exchange_pull(
    app: &mut InstanceService,
    handle: &HandleId,
    instance_type: InstanceType,
    max_bytes: Option<usize>,
    peer_id: Option<&str>,
) -> Result<(PullResult, usize, bool), DomainError> {
    let mut result = app.pull(handle, max_bytes, peer_id)?;
    if instance_type != InstanceType::Serial || result.bytes.is_empty() || result.truncated {
        return Ok((result, 1, false));
    }

    let remaining_max_bytes = match max_bytes {
        Some(limit) => {
            let remaining = limit.saturating_sub(result.bytes.len());
            if remaining == 0 {
                return Ok((result, 1, false));
            }
            Some(remaining)
        }
        None => None,
    };

    match app.pull(handle, remaining_max_bytes, peer_id) {
        Ok(next) => {
            result.bytes.extend_from_slice(&next.bytes);
            result.truncated = next.truncated;
            result.remaining_rx_buffer_bytes = next.remaining_rx_buffer_bytes;
            Ok((result, 2, false))
        }
        Err(error) if error.code == ErrorCode::ReadTimeout => Ok((result, 2, true)),
        Err(error) => Err(error),
    }
}

fn debug_send_target(target: Option<crate::runtime::SendTargetSummary>) -> Value {
    match target {
        Some(target) => json!({
            "mode": match target.mode {
                crate::runtime::SendTargetMode::Peer => "peer",
                crate::runtime::SendTargetMode::Broadcast => "broadcast",
            },
            "peer_id": target.peer_id,
            "peer_count": target.peer_count,
            "successful_peer_ids": target.successful_peer_ids,
            "failed_peer_count": target.failed_peer_count
        }),
        None => Value::Null,
    }
}

fn debug_pull_source(source: Option<crate::runtime::PullSource>) -> Value {
    match source {
        Some(source) => json!({
            "transport": source.transport,
            "peer_id": source.peer_id,
            "remote_addr": source.remote_addr
        }),
        None => Value::Null,
    }
}

impl DebugProfile {
    fn from_params(params: DebugProfileParams) -> Self {
        Self {
            transport: params.transport,
            serial: params.serial,
            tcp: params.tcp,
            udp: params.udp,
            visa: params.visa,
            payload: params.payload,
            pull: params.pull,
            bound_handle_id: params.bound_handle_id.flatten(),
        }
    }
}

impl PortMcpServer {
    fn debug_scope(&self) -> Value {
        json!({
            "scope": SessionMode::RequestContextDebug.as_str(),
            "stable_session": false,
            "profile_state": "mcp_server_memory"
        })
    }

    fn debug_profile_for_session(&self, session_id: &str) -> Option<DebugProfile> {
        self.debug_profiles
            .lock()
            .expect("debug profiles mutex poisoned")
            .get(session_id)
            .cloned()
    }

    fn bind_profile_handle(&self, session_id: &str, handle: &HandleId) {
        if let Some(profile) = self
            .debug_profiles
            .lock()
            .expect("debug profiles mutex poisoned")
            .get_mut(session_id)
        {
            profile.bound_handle_id = Some(handle.as_str().to_owned());
        }
    }

    fn unbind_profile_handle(&self, session_id: &str, handle: &HandleId) {
        if let Some(profile) = self
            .debug_profiles
            .lock()
            .expect("debug profiles mutex poisoned")
            .get_mut(session_id)
        {
            if profile.bound_handle_id.as_deref() == Some(handle.as_str()) {
                profile.bound_handle_id = None;
            }
        }
    }

    fn debug_profile_data(&self, profile: &DebugProfile) -> Value {
        let bound_handle = profile.bound_handle_id.as_ref().map(|handle_id| {
            let handle = HandleId::from(handle_id.as_str());
            match self.with_app(|app| app.query(Some(&handle), None)) {
                Ok(summary) => json!({
                    "handle_id": handle,
                    "valid": true,
                    "state": summary.state,
                    "type": summary.instance_type,
                    "summary": summary,
                    "reason": "ok"
                }),
                Err(error) => json!({
                    "handle_id": handle,
                    "valid": false,
                    "reason": error.code,
                    "message": error.message
                }),
            }
        });
        json!({
            "scope": self.debug_scope(),
            "profile": profile,
            "derived_defaults": self.debug_derived_defaults(profile),
            "bound_handle": bound_handle,
            "suggested_next_tools": if profile.bound_handle_id.is_some() {
                json!(["debug_exchange", "debug_close", "debug_connect"])
            } else {
                json!(["debug_connect"])
            }
        })
    }

    fn debug_derived_defaults(&self, profile: &DebugProfile) -> Value {
        json!({
            "transport": profile.transport,
            "payload": {
                "encoding": profile.payload.as_ref().and_then(|payload| payload.encoding).unwrap_or_default(),
                "append_line_break": profile.payload.as_ref().and_then(|payload| payload.append_line_break).unwrap_or(false)
            },
            "pull": {
                "max_bytes": profile.pull.as_ref().and_then(|pull| pull.max_bytes).unwrap_or(4096),
                "peer_id": profile.pull.as_ref().and_then(|pull| pull.peer_id.clone())
            }
        })
    }

    fn debug_config_for_transport(
        &self,
        transport: InstanceTypeParam,
        profile: &Option<DebugProfile>,
        params: &DebugConnectParams,
    ) -> Result<DebugConfig, DomainError> {
        match transport {
            InstanceTypeParam::Serial => {
                let merged = merge_serial_profile(
                    profile.as_ref().and_then(|profile| profile.serial.as_ref()),
                    params.serial.as_ref(),
                );
                let port = merged.port.ok_or_else(|| {
                    DomainError::invalid_argument(
                        ErrorCode::MissingRequiredField,
                        "debug_connect Serial requires serial.port after merging debug profile defaults.",
                        "Pass serial.port or set it with debug_profile_set.",
                    )
                    .with_detail("field", json!("serial.port"))
                })?;
                Ok(DebugConfig::Serial(SerialConfig {
                    port,
                    baudrate: merged.baudrate.unwrap_or_else(default_baudrate),
                    data_bits: map_data_bits(merged.data_bits.unwrap_or_else(default_data_bits)),
                    stop_bits: map_stop_bits(merged.stop_bits.unwrap_or_else(default_stop_bits)),
                    parity: map_parity(merged.parity.unwrap_or_default()),
                    flow_control: map_flow_control(merged.flow_control.unwrap_or_default()),
                    timeout_ms: merged.timeout_ms.unwrap_or_else(default_timeout_ms),
                    encoding: map_encoding(merged.encoding.unwrap_or_default()),
                }))
            }
            InstanceTypeParam::Tcp => {
                let merged = merge_tcp_profile(
                    profile.as_ref().and_then(|profile| profile.tcp.as_ref()),
                    params.tcp.as_ref(),
                );
                let mode = merged.mode.unwrap_or_default();
                let host = merged.host.or(merged.bind_host).unwrap_or_default();
                let port = merged.port.or(merged.bind_port).unwrap_or_default();
                Ok(DebugConfig::Tcp(TcpConfig {
                    mode: map_tcp_mode(mode),
                    host,
                    port,
                    timeout_ms: merged.timeout_ms.unwrap_or_else(default_timeout_ms),
                }))
            }
            InstanceTypeParam::Udp => {
                let merged = merge_udp_profile(
                    profile.as_ref().and_then(|profile| profile.udp.as_ref()),
                    params.udp.as_ref(),
                );
                Ok(DebugConfig::Udp(UdpConfig {
                    bind_host: merged.bind_host.unwrap_or_default(),
                    bind_port: merged.bind_port.unwrap_or_default(),
                    remote_host: merged.remote_host,
                    remote_port: merged.remote_port,
                    timeout_ms: merged.timeout_ms.unwrap_or_else(default_timeout_ms),
                }))
            }
            InstanceTypeParam::Visa => {
                let merged = merge_visa_profile(
                    profile.as_ref().and_then(|profile| profile.visa.as_ref()),
                    params.visa.as_ref(),
                );
                let resource_address = merged.resource_address.ok_or_else(|| {
                    DomainError::invalid_argument(
                        ErrorCode::MissingRequiredField,
                        "debug_connect Visa requires visa.resource_address after merging debug profile defaults.",
                        "Pass visa.resource_address or set it with debug_profile_set.",
                    )
                    .with_detail("field", json!("visa.resource_address"))
                })?;
                Ok(DebugConfig::Visa(VisaConfig {
                    resource_address,
                    open_timeout_ms: merged.open_timeout_ms.unwrap_or_else(default_timeout_ms),
                    io_timeout_ms: merged.io_timeout_ms.unwrap_or_else(default_timeout_ms),
                    read_termination: merged.read_termination,
                    write_termination: merged.write_termination,
                    encoding: map_encoding(merged.encoding.unwrap_or_default()),
                    query_idn_on_connect: merged.query_idn_on_connect.unwrap_or(false),
                }))
            }
        }
    }

    fn debug_phase_failure(
        &self,
        tool: &str,
        phases: Vec<Value>,
        failed_phase: &str,
        handle: Option<HandleId>,
        error: DomainError,
    ) -> ToolResponse {
        let data = json!({
            "scope": self.debug_scope(),
            "phases": phases,
            "failed_phase": failed_phase,
            "cleanup_hint": handle.as_ref().map(|handle| format!("Inspect or clean up handle {} with instance_query, debug_close, port_disconnect, or instance_release.", handle.as_str()))
        });
        let response = self
            .err(tool, error)
            .with_warning(crate::model::Warning::new(
                "DEBUG_PHASE_FAILURE",
                &data.to_string(),
            ));
        match handle {
            Some(handle) => response.with_handle(handle),
            None => response,
        }
    }
}

fn merge_serial_profile(
    base: Option<&DebugSerialProfileParams>,
    override_value: Option<&DebugSerialProfileParams>,
) -> DebugSerialProfileParams {
    let mut merged = base.cloned().unwrap_or_default();
    if let Some(value) = override_value {
        if value.port.is_some() {
            merged.port = value.port.clone();
        }
        if value.baudrate.is_some() {
            merged.baudrate = value.baudrate;
        }
        if value.data_bits.is_some() {
            merged.data_bits = value.data_bits;
        }
        if value.stop_bits.is_some() {
            merged.stop_bits = value.stop_bits;
        }
        if value.parity.is_some() {
            merged.parity = value.parity;
        }
        if value.flow_control.is_some() {
            merged.flow_control = value.flow_control;
        }
        if value.timeout_ms.is_some() {
            merged.timeout_ms = value.timeout_ms;
        }
        if value.encoding.is_some() {
            merged.encoding = value.encoding;
        }
    }
    merged
}

fn merge_tcp_profile(
    base: Option<&DebugTcpProfileParams>,
    override_value: Option<&DebugTcpProfileParams>,
) -> DebugTcpProfileParams {
    let mut merged = base.cloned().unwrap_or_default();
    if let Some(value) = override_value {
        if value.mode.is_some() {
            merged.mode = value.mode;
        }
        if value.host.is_some() {
            merged.host = value.host.clone();
        }
        if value.port.is_some() {
            merged.port = value.port;
        }
        if value.bind_host.is_some() {
            merged.bind_host = value.bind_host.clone();
        }
        if value.bind_port.is_some() {
            merged.bind_port = value.bind_port;
        }
        if value.timeout_ms.is_some() {
            merged.timeout_ms = value.timeout_ms;
        }
    }
    merged
}

fn merge_udp_profile(
    base: Option<&DebugUdpProfileParams>,
    override_value: Option<&DebugUdpProfileParams>,
) -> DebugUdpProfileParams {
    let mut merged = base.cloned().unwrap_or_default();
    if let Some(value) = override_value {
        if value.bind_host.is_some() {
            merged.bind_host = value.bind_host.clone();
        }
        if value.bind_port.is_some() {
            merged.bind_port = value.bind_port;
        }
        if value.remote_host.is_some() {
            merged.remote_host = value.remote_host.clone();
        }
        if value.remote_port.is_some() {
            merged.remote_port = value.remote_port;
        }
        if value.timeout_ms.is_some() {
            merged.timeout_ms = value.timeout_ms;
        }
    }
    merged
}

fn merge_visa_profile(
    base: Option<&DebugVisaProfileParams>,
    override_value: Option<&DebugVisaProfileParams>,
) -> DebugVisaProfileParams {
    let mut merged = base.cloned().unwrap_or_default();
    if let Some(value) = override_value {
        if value.resource_address.is_some() {
            merged.resource_address = value.resource_address.clone();
        }
        if value.open_timeout_ms.is_some() {
            merged.open_timeout_ms = value.open_timeout_ms;
        }
        if value.io_timeout_ms.is_some() {
            merged.io_timeout_ms = value.io_timeout_ms;
        }
        if value.read_termination.is_some() {
            merged.read_termination = value.read_termination.clone();
        }
        if value.write_termination.is_some() {
            merged.write_termination = value.write_termination.clone();
        }
        if value.encoding.is_some() {
            merged.encoding = value.encoding;
        }
        if value.query_idn_on_connect.is_some() {
            merged.query_idn_on_connect = value.query_idn_on_connect;
        }
    }
    merged
}

fn map_modbus_mode(value: ModbusModeParam) -> ModbusMode {
    match value {
        ModbusModeParam::Rtu => ModbusMode::Rtu,
        ModbusModeParam::Ascii => ModbusMode::Ascii,
    }
}

fn required_modbus_u8(field: &'static str, value: Option<u8>) -> Result<u8, DomainError> {
    value.ok_or_else(|| {
        DomainError::invalid_argument(
            ErrorCode::MissingRequiredField,
            format!("Missing required field `{field}`."),
            format!("Provide `{field}` and retry."),
        )
        .with_detail("field", json!(field))
    })
}

fn required_modbus_u16(field: &'static str, value: Option<u16>) -> Result<u16, DomainError> {
    value.ok_or_else(|| {
        DomainError::invalid_argument(
            ErrorCode::MissingRequiredField,
            format!("Missing required field `{field}`."),
            format!("Provide `{field}` and retry."),
        )
        .with_detail("field", json!(field))
    })
}

fn required_modbus_string(
    field: &'static str,
    value: Option<String>,
) -> Result<String, DomainError> {
    value.ok_or_else(|| {
        DomainError::invalid_argument(
            ErrorCode::MissingRequiredField,
            format!("Missing required field `{field}`."),
            format!("Provide `{field}` and retry."),
        )
        .with_detail("field", json!(field))
    })
}

impl From<&str> for HandleId {
    fn from(value: &str) -> Self {
        Self::from_string(value)
    }
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
    use tokio::{
        io::AsyncReadExt,
        io::AsyncWriteExt,
        net::{TcpListener, UdpSocket},
        sync::Notify,
        time::{Duration, timeout},
    };

    use super::{PortMcpServer, debug_exchange_pull};
    use crate::{
        app::InstanceService,
        model::{InstanceType, RuntimeLimits, SerialConfig},
    };

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
            "usage_guide",
            "str_to_hex",
            "hex_to_str",
            "modbus_helper",
            "scpi_helper",
            "at_helper",
            "slip_helper",
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
            "debug_profile_set",
            "debug_profile_get",
            "debug_connect",
            "debug_exchange",
            "debug_close",
            "debug_log_config",
        ] {
            assert!(tool_names.contains(expected), "missing tool {expected}");
        }
        assert!(!tool_names.contains("m0_smoke"));

        let modbus_tool = tools
            .tools
            .iter()
            .find(|tool| tool.name.as_ref() == "modbus_helper")
            .expect("modbus_helper tool is registered");
        let description = modbus_tool.description.as_deref().unwrap_or_default();
        assert!(description.contains("pack requires"));
        assert!(description.contains("optional data_or_hex"));
        assert!(description.contains("unpack requires frame_hex"));

        let debug_connect_tool = tools
            .tools
            .iter()
            .find(|tool| tool.name.as_ref() == "debug_connect")
            .expect("debug_connect tool is registered");
        let description = debug_connect_tool
            .description
            .as_deref()
            .unwrap_or_default();
        assert!(description.contains("handle_id"));
        assert!(description.contains("phase"));
        assert!(description.contains("cleanup"));

        let debug_exchange_tool = tools
            .tools
            .iter()
            .find(|tool| tool.name.as_ref() == "debug_exchange")
            .expect("debug_exchange tool is registered");
        let description = debug_exchange_tool
            .description
            .as_deref()
            .unwrap_or_default();
        assert!(description.contains("Connected"));
        assert!(description.contains("does not connect"));

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m9_usage_guide_returns_agent_onboarding_sequences()
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

        let response = call_tool_json(&client, "usage_guide", object!({})).await?;

        assert_eq!(response["ok"], true);
        assert_eq!(response["tool"], "usage_guide");
        assert_eq!(response["request_id"], "req_20260526_000001");
        assert!(
            response["data"]["principles"][0]
                .as_str()
                .unwrap()
                .contains("handle_id explicitly")
        );
        assert_eq!(
            response["data"]["common_sequences"]["tcp_client"][0]["tool"],
            "instance_create"
        );
        assert_eq!(
            response["data"]["common_sequences"]["serial"][0]["tool"],
            "port_scan"
        );
        assert_eq!(
            response["data"]["common_sequences"]["helpers"][0]["tool"],
            "str_to_hex"
        );
        assert_eq!(
            response["data"]["common_sequences"]["debug_tcp_client"][0]["tool"],
            "debug_profile_set"
        );
        assert_eq!(
            response["data"]["common_sequences"]["debug_tcp_client"][1]["tool"],
            "debug_connect"
        );
        assert_eq!(
            response["data"]["common_sequences"]["debug_tcp_client"][2]["tool"],
            "debug_exchange"
        );
        assert_eq!(
            response["data"]["common_sequences"]["debug_tcp_client"][3]["tool"],
            "debug_close"
        );
        assert_eq!(
            response["data"]["common_sequences"]["helpers"][2]["arguments"]["data_or_hex"],
            "0002"
        );
        assert_eq!(
            response["data"]["common_sequences"]["helpers"][3]["arguments"]["frame_hex"],
            "010300100002c5ce"
        );
        assert!(
            response["data"]["tool_notes"]["tcp_udp_config"]
                .as_str()
                .unwrap()
                .contains("TCP client uses host/port")
        );
        assert!(
            response["data"]["tool_notes"]["debug_exchange"]
                .as_str()
                .unwrap()
                .contains("does not implicitly connect")
        );
        assert!(
            response["data"]["tool_notes"]["device_probe"]
                .as_str()
                .unwrap()
                .contains("does not create handles")
        );

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn phase1_debug_profile_set_get_stores_defaults_without_creating_instances()
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

        let set = call_tool_json(
            &client,
            "debug_profile_set",
            object!({
                "transport": "TCP",
                "tcp": { "mode": "client", "host": "127.0.0.1", "port": 9000, "timeout_ms": 1000 },
                "payload": { "encoding": "text", "append_line_break": true },
                "pull": { "max_bytes": 32 }
            }),
        )
        .await?;
        assert_eq!(set["ok"], true);
        assert_eq!(set["tool"], "debug_profile_set");
        assert_eq!(set["data"]["scope"]["scope"], "request_context_debug");
        assert_eq!(set["data"]["derived_defaults"]["transport"], "TCP");
        assert_eq!(set["data"]["derived_defaults"]["pull"]["max_bytes"], 32);

        let list = call_tool_json(&client, "instance_list", object!({})).await?;
        assert_eq!(list["data"]["instances"].as_array().unwrap().len(), 0);

        let get = call_tool_json(&client, "debug_profile_get", object!({})).await?;
        assert_eq!(get["ok"], true);
        assert_eq!(get["data"]["derived_defaults"]["transport"], "TCP");
        assert!(
            get["data"]["suggested_next_tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool == "debug_connect")
        );

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn phase1_debug_exchange_rejects_non_connected_handle_without_connecting()
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
        let rejected = call_tool_json(
            &client,
            "debug_exchange",
            object!({ "handle_id": handle_id, "data": "ping", "encoding": "text", "max_bytes": 16 }),
        )
        .await?;
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["error"]["code"], "STATE_NOT_ALLOWED");

        let query = call_tool_json(
            &client,
            "instance_query",
            object!({ "handle_id": handle_id }),
        )
        .await?;
        assert_eq!(query["state"], "Created");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[test]
    fn phase1_debug_exchange_pull_aggregates_split_serial_response() {
        let mut app = InstanceService::new_for_tests("20260526");
        let created = app.create(InstanceType::Serial).unwrap();
        let handle = created.handle_id.clone();
        app.configure_serial(&handle, SerialConfig::new("COM9"))
            .unwrap();
        app.registry.connect_mock(&handle).unwrap();
        app.attach_serial_worker_for_tests(
            &handle,
            crate::transport::serial_worker_for_tests(vec![
                vec![0x01],
                vec![0x08, 0x00, 0x00, 0x12, 0x34, 0xED, 0x7C],
            ]),
        );

        let (result, attempts, completed_after_timeout) =
            debug_exchange_pull(&mut app, &handle, InstanceType::Serial, Some(16), None).unwrap();

        assert_eq!(attempts, 2);
        assert!(!completed_after_timeout);
        assert_eq!(
            result.bytes,
            vec![0x01, 0x08, 0x00, 0x00, 0x12, 0x34, 0xED, 0x7C]
        );
    }

    #[tokio::test]
    async fn phase1_debug_shortcuts_use_profile_bound_handle_when_omitted()
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

        let port_probe = UdpSocket::bind("127.0.0.1:0").await?;
        let bind_port = port_probe.local_addr()?.port();
        drop(port_probe);

        let set = call_tool_json(
            &client,
            "debug_profile_set",
            object!({
                "transport": "UDP",
                "udp": {
                    "bind_host": "127.0.0.1",
                    "bind_port": bind_port,
                    "remote_host": "127.0.0.1",
                    "remote_port": bind_port,
                    "timeout_ms": 1000
                },
                "payload": { "encoding": "text", "append_line_break": false },
                "pull": { "max_bytes": 64 }
            }),
        )
        .await?;
        assert_eq!(set["ok"], true);

        let connected = call_tool_json(&client, "debug_connect", object!({})).await?;
        assert_eq!(connected["ok"], true);
        let handle_id = connected["handle_id"].as_str().unwrap().to_owned();
        assert!(connected["data"]["handle_id"].as_str().is_some());

        let exchanged = call_tool_json(
            &client,
            "debug_exchange",
            object!({ "data": "ping", "pull_after": true }),
        )
        .await?;
        assert_eq!(exchanged["ok"], true);
        assert_eq!(exchanged["handle_id"], handle_id);
        assert_eq!(exchanged["data"]["used_profile_handle"], true);
        assert_eq!(exchanged["data"]["phases"][1]["payload"]["preview"], "ping");

        let closed = call_tool_json(&client, "debug_close", object!({ "release": true })).await?;
        assert_eq!(closed["ok"], true);
        assert_eq!(closed["handle_id"], handle_id);
        assert_eq!(closed["data"]["released"], true);

        let profile = call_tool_json(&client, "debug_profile_get", object!({})).await?;
        assert!(profile["data"]["profile"].get("bound_handle_id").is_none());

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_str_to_hex_returns_hex_string() -> Result<(), Box<dyn std::error::Error>> {
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

        let response =
            call_tool_json(&client, "str_to_hex", object!({ "input_string": "ping" })).await?;
        assert_eq!(response["ok"], true);
        assert_eq!(response["tool"], "str_to_hex");
        assert_eq!(response["data"]["hex"], "70696e67");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_hex_to_str_returns_text() -> Result<(), Box<dyn std::error::Error>> {
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

        let response =
            call_tool_json(&client, "hex_to_str", object!({ "hex": "70696e67" })).await?;
        assert_eq!(response["ok"], true);
        assert_eq!(response["tool"], "hex_to_str");
        assert_eq!(response["data"]["text"], "ping");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_hex_to_str_rejects_non_utf8() -> Result<(), Box<dyn std::error::Error>> {
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

        let response = call_tool_json(&client, "hex_to_str", object!({ "hex": "ff" })).await?;
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "TEXT_ENCODING_FAILED");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_str_to_hex_rejects_oversized_input() -> Result<(), Box<dyn std::error::Error>>
    {
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

        let response = call_tool_json(
            &client,
            "str_to_hex",
            object!({ "input_string": "x".repeat(crate::model::RuntimeLimits::HELPER_MAX_INPUT_BYTES + 1) }),
        )
        .await?;
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "INVALID_RANGE");
        assert_eq!(response["error"]["details"]["field"], "input_string");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_modbus_pack_and_unpack_round_trip() -> Result<(), Box<dyn std::error::Error>>
    {
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

        let pack = call_tool_json(
            &client,
            "modbus_helper",
            object!({
                "action": "pack",
                "mode": "rtu",
                "slave_id": 1,
                "function_code": 3,
                "address": 16,
                "data_or_hex": "0002",
                "crc_check": true
            }),
        )
        .await?;
        assert_eq!(pack["ok"], true);
        let frame_hex = pack["data"]["frame_hex"].as_str().unwrap().to_owned();

        let unpack = call_tool_json(
            &client,
            "modbus_helper",
            object!({
                "action": "unpack",
                "mode": "rtu",
                "frame_hex": frame_hex,
                "crc_check": true
            }),
        )
        .await?;
        assert_eq!(unpack["ok"], true);
        assert_eq!(unpack["data"]["slave_id"], 1);
        assert_eq!(unpack["data"]["function_code"], 3);

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_modbus_unpack_requires_frame_hex_without_old_fallback()
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

        let response = call_tool_json(
            &client,
            "modbus_helper",
            object!({
                "action": "unpack",
                "mode": "rtu",
                "data_or_hex": "010300100002c5ce"
            }),
        )
        .await?;
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "MISSING_REQUIRED_FIELD");
        assert_eq!(response["error"]["details"]["field"], "frame_hex");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_modbus_crc_check_defaults_to_strict()
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

        let response = call_tool_json(
            &client,
            "modbus_helper",
            object!({
                "action": "unpack",
                "mode": "rtu",
                "frame_hex": "0103001000020000"
            }),
        )
        .await?;
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "PROTOCOL_CHECKSUM_FAILED");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_scpi_normalize_returns_summary() -> Result<(), Box<dyn std::error::Error>> {
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

        let response = call_tool_json(
            &client,
            "scpi_helper",
            object!({
                "action": "normalize",
                "command": "  *IDN?  ",
                "arguments": " "
            }),
        )
        .await?;
        assert_eq!(response["ok"], true);
        assert_eq!(response["tool"], "scpi_helper");
        assert_eq!(response["data"]["normalized"], "*IDN?");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_scpi_rejects_oversized_expect_response()
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

        let response = call_tool_json(
            &client,
            "scpi_helper",
            object!({
                "action": "normalize",
                "command": "*IDN?",
                "expect_response": "x".repeat(513)
            }),
        )
        .await?;
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "INVALID_RANGE");
        assert_eq!(response["error"]["details"]["field"], "expect_response");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_at_classify_returns_summary() -> Result<(), Box<dyn std::error::Error>> {
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

        let response = call_tool_json(
            &client,
            "at_helper",
            object!({
                "command": "AT+CGMI"
            }),
        )
        .await?;
        assert_eq!(response["ok"], true);
        assert_eq!(response["tool"], "at_helper");
        assert_eq!(response["data"]["normalized"], "AT+CGMI");
        assert_eq!(response["data"]["response_class"], "extended");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_slip_encode_and_decode_round_trip() -> Result<(), Box<dyn std::error::Error>>
    {
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

        let encoded = call_tool_json(
            &client,
            "slip_helper",
            object!({
                "action": "encode",
                "payload_hex": "c0db01"
            }),
        )
        .await?;
        assert_eq!(encoded["ok"], true);
        let slip_hex = encoded["data"]["normalized"].as_str().unwrap().to_owned();

        let decoded = call_tool_json(
            &client,
            "slip_helper",
            object!({
                "action": "decode",
                "payload_hex": slip_hex
            }),
        )
        .await?;
        assert_eq!(decoded["ok"], true);
        assert_eq!(decoded["data"]["response_class"], "decoded");
        assert_eq!(decoded["data"]["normalized"], "c0db01");
        assert_eq!(decoded["data"]["payload_hex"], "c0db01");

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn m9_helper_slip_reports_decode_and_limit_errors()
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

        let invalid_escape = call_tool_json(
            &client,
            "slip_helper",
            object!({
                "action": "decode",
                "payload_hex": "c0db00c0"
            }),
        )
        .await?;
        assert_eq!(invalid_escape["ok"], false);
        assert_eq!(invalid_escape["error"]["code"], "PROTOCOL_FRAME_INVALID");

        let oversized = call_tool_json(
            &client,
            "slip_helper",
            object!({
                "action": "encode",
                "payload_hex": "c0".repeat(crate::model::RuntimeLimits::HELPER_MAX_INPUT_BYTES)
            }),
        )
        .await?;
        assert_eq!(oversized["ok"], false);
        assert_eq!(oversized["error"]["code"], "INVALID_RANGE");
        assert_eq!(oversized["error"]["details"]["field"], "payload_hex");

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
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let listen_port = listener.local_addr()?.port();
        let receive_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut buffer = vec![0; 4];
            stream.read_exact(&mut buffer).await?;
            stream.write_all(b"pong").await?;
            Ok::<(), std::io::Error>(())
        });

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
                "port": listen_port,
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
        let pulled = call_tool_json(
            &client,
            "port_pull",
            object!({ "handle_id": handle_id, "max_bytes": 16 }),
        )
        .await?;
        assert_eq!(pulled["data"]["payload"]["preview"], "pong");
        timeout(Duration::from_secs(5), receive_task)
            .await
            .expect("listener should receive ping")??;
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
    async fn n1_mcp_tcp_listen_supports_multiple_clients_and_peer_routing()
    -> Result<(), Box<dyn std::error::Error>> {
        let (server_transport, client_transport) = tokio::io::duplex(128 * 1024);

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

        let port_probe = TcpListener::bind("127.0.0.1:0").await?;
        let listen_port = port_probe.local_addr()?.port();
        drop(port_probe);

        let server = call_tool_json(&client, "instance_create", object!({ "type": "TCP" })).await?;
        let server_handle_id = server["handle_id"].as_str().unwrap();
        call_tool_json(
            &client,
            "tcp_udp_config",
            object!({
                "handle_id": server_handle_id,
                "mode": "listen",
                "bind_host": "127.0.0.1",
                "bind_port": listen_port,
                "timeout_ms": 1000,
            }),
        )
        .await?;
        call_tool_json(
            &client,
            "port_connect",
            object!({ "handle_id": server_handle_id }),
        )
        .await?;

        let client_a = create_connected_tcp_client(&client, listen_port).await?;
        let client_b = create_connected_tcp_client(&client, listen_port).await?;
        let peers = wait_for_mcp_peers(&client, server_handle_id, 2).await?;
        let peer_a = peers[0]["peer_id"].as_str().unwrap().to_owned();
        let peer_b = peers[1]["peer_id"].as_str().unwrap().to_owned();
        assert!(peer_a.starts_with("h_tcp_001:peer-"));
        assert!(
            peers[0]["remote_addr"]
                .as_str()
                .unwrap()
                .starts_with("127.0.0.1:")
        );

        let targeted = call_tool_json(
            &client,
            "port_send",
            object!({
                "handle_id": server_handle_id,
                "peer_id": peer_a,
                "data": "one",
                "encoding": "text"
            }),
        )
        .await?;
        assert_eq!(targeted["data"]["target"]["mode"], "peer");
        assert_eq!(targeted["data"]["target"]["successful_peer_ids"][0], peer_a);
        let received_a = call_tool_json(
            &client,
            "port_pull",
            object!({ "handle_id": client_a, "max_bytes": 8 }),
        )
        .await?;
        assert_eq!(received_a["data"]["payload"]["preview"], "one");

        let broadcast = call_tool_json(
            &client,
            "port_send",
            object!({
                "handle_id": server_handle_id,
                "data": "all",
                "encoding": "text"
            }),
        )
        .await?;
        assert_eq!(broadcast["data"]["target"]["mode"], "broadcast");
        assert_eq!(broadcast["data"]["target"]["peer_count"], 2);
        assert_eq!(
            call_tool_json(
                &client,
                "port_pull",
                object!({ "handle_id": client_a, "max_bytes": 8 })
            )
            .await?["data"]["payload"]["preview"],
            "all"
        );
        assert_eq!(
            call_tool_json(
                &client,
                "port_pull",
                object!({ "handle_id": client_b, "max_bytes": 8 })
            )
            .await?["data"]["payload"]["preview"],
            "all"
        );

        call_tool_json(
            &client,
            "port_send",
            object!({ "handle_id": client_b, "data": "from-b", "encoding": "text" }),
        )
        .await?;
        let pulled_b = call_tool_json(
            &client,
            "port_pull",
            object!({ "handle_id": server_handle_id, "peer_id": peer_b, "max_bytes": 16 }),
        )
        .await?;
        assert_eq!(pulled_b["data"]["payload"]["preview"], "from-b");
        assert_eq!(pulled_b["data"]["source"]["peer_id"], peer_b);

        call_tool_json(
            &client,
            "port_disconnect",
            object!({ "handle_id": client_a }),
        )
        .await?;
        let remaining = wait_for_mcp_peers(&client, server_handle_id, 1).await?;
        assert_eq!(remaining[0]["peer_id"], peer_b);

        for handle_id in [client_b, server_handle_id.to_owned()] {
            call_tool_json(
                &client,
                "port_disconnect",
                object!({ "handle_id": handle_id }),
            )
            .await?;
            call_tool_json(
                &client,
                "instance_release",
                object!({ "handle_id": handle_id }),
            )
            .await?;
        }
        call_tool_json(
            &client,
            "instance_release",
            object!({ "handle_id": client_a }),
        )
        .await?;

        client.cancel().await?;
        server_handle.await??;
        Ok(())
    }

    async fn create_connected_tcp_client(
        client: &rmcp::service::RunningService<rmcp::RoleClient, SmokeClient>,
        port: u16,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let created = call_tool_json(client, "instance_create", object!({ "type": "TCP" })).await?;
        let handle_id = created["handle_id"].as_str().unwrap().to_owned();
        call_tool_json(
            client,
            "tcp_udp_config",
            object!({
                "handle_id": handle_id,
                "mode": "client",
                "host": "127.0.0.1",
                "port": port,
                "timeout_ms": 1000,
            }),
        )
        .await?;
        call_tool_json(client, "port_connect", object!({ "handle_id": handle_id })).await?;
        Ok(handle_id)
    }

    async fn wait_for_mcp_peers(
        client: &rmcp::service::RunningService<rmcp::RoleClient, SmokeClient>,
        handle_id: &str,
        expected: usize,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let query = call_tool_json(
                client,
                "instance_query",
                object!({ "handle_id": handle_id }),
            )
            .await?;
            let peers = query["data"]["summary"]["peers"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if peers.len() == expected || std::time::Instant::now() >= deadline {
                return Ok(peers);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn r1_mcp_tcp_client_send_reaches_real_loopback_listener()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let listen_port = listener.local_addr()?.port();
        let receive_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut buffer = vec![0; 4];
            stream.read_exact(&mut buffer).await?;
            Ok::<Vec<u8>, std::io::Error>(buffer)
        });

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

        tokio::time::timeout(Duration::from_secs(5), async {
            call_tool_json(
                &client,
                "tcp_udp_config",
                object!({
                    "handle_id": handle_id,
                    "mode": "client",
                    "host": "127.0.0.1",
                    "port": listen_port,
                    "timeout_ms": 1000
                }),
            )
            .await
        })
        .await
        .expect("tcp_udp_config should complete")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            call_tool_json(&client, "port_connect", object!({ "handle_id": handle_id })).await
        })
        .await
        .expect("port_connect should complete")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            call_tool_json(
                &client,
                "port_send",
                object!({ "handle_id": handle_id, "data": "ping", "encoding": "text" }),
            )
            .await
        })
        .await
        .expect("port_send should complete")?;

        let received = timeout(Duration::from_millis(500), receive_task)
            .await
            .expect("real TCP listener should receive bytes from MCP port_send")??;
        assert_eq!(received, b"ping");

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
        tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("MCP server should stop after client cancellation")??;

        Ok(())
    }

    #[tokio::test]
    async fn r1_mcp_tcp_real_loopback_rejects_oversized_send_before_write()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let listen_port = listener.local_addr()?.port();
        let accept_task = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await?;
            Ok::<(), std::io::Error>(())
        });

        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let mut limits = RuntimeLimits::default();
        limits.tx_frame_max_bytes = 4;

        let server_handle = tokio::spawn(async move {
            PortMcpServer::new_for_tests_with_limits("20260526", limits)
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
            object!({
                "handle_id": handle_id,
                "mode": "client",
                "host": "127.0.0.1",
                "port": listen_port,
                "timeout_ms": 1000
            }),
        )
        .await?;
        call_tool_json(&client, "port_connect", object!({ "handle_id": handle_id })).await?;
        accept_task.await??;

        let rejected = call_tool_json(
            &client,
            "port_send",
            object!({ "handle_id": handle_id, "data": "12345", "encoding": "text" }),
        )
        .await?;
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["error"]["code"], "INVALID_RANGE");

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn r1_mcp_udp_send_reaches_real_loopback_listener()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = UdpSocket::bind("127.0.0.1:0").await?;
        let listen_port = listener.local_addr()?.port();
        let receive_task = tokio::spawn(async move {
            let mut buffer = vec![0; 16];
            let (read, _) = listener.recv_from(&mut buffer).await?;
            buffer.truncate(read);
            Ok::<Vec<u8>, std::io::Error>(buffer)
        });

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
            call_tool_json(&client, "instance_create", object!({ "type": "UDP" })).await?;
        let handle_id = created["handle_id"].as_str().unwrap();

        call_tool_json(
            &client,
            "tcp_udp_config",
            object!({
                "handle_id": handle_id,
                "bind_host": "127.0.0.1",
                "bind_port": 0,
                "remote_host": "127.0.0.1",
                "remote_port": listen_port,
                "timeout_ms": 1000
            }),
        )
        .await?;
        call_tool_json(&client, "port_connect", object!({ "handle_id": handle_id })).await?;
        assert_eq!(
            call_tool_json(
                &client,
                "port_send",
                object!({ "handle_id": handle_id, "data": "ping", "encoding": "text" }),
            )
            .await?["data"]["sent_bytes"],
            4
        );

        let received = timeout(Duration::from_millis(500), receive_task)
            .await
            .expect("real UDP listener should receive bytes from MCP port_send")??;
        assert_eq!(received, b"ping");

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn r1_mcp_udp_real_loopback_rejects_oversized_send_before_datagram()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = UdpSocket::bind("127.0.0.1:0").await?;
        let listen_port = listener.local_addr()?.port();

        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let mut limits = RuntimeLimits::default();
        limits.tx_frame_max_bytes = 4;

        let server_handle = tokio::spawn(async move {
            PortMcpServer::new_for_tests_with_limits("20260526", limits)
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
            call_tool_json(&client, "instance_create", object!({ "type": "UDP" })).await?;
        let handle_id = created["handle_id"].as_str().unwrap();
        call_tool_json(
            &client,
            "tcp_udp_config",
            object!({
                "handle_id": handle_id,
                "bind_host": "127.0.0.1",
                "bind_port": 0,
                "remote_host": "127.0.0.1",
                "remote_port": listen_port,
                "timeout_ms": 1000
            }),
        )
        .await?;
        call_tool_json(&client, "port_connect", object!({ "handle_id": handle_id })).await?;

        let rejected = call_tool_json(
            &client,
            "port_send",
            object!({ "handle_id": handle_id, "data": "12345", "encoding": "text" }),
        )
        .await?;
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["error"]["code"], "INVALID_RANGE");

        let mut buffer = [0_u8; 8];
        assert!(
            timeout(Duration::from_millis(100), listener.recv_from(&mut buffer))
                .await
                .is_err()
        );

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn r1_mcp_udp_listener_receives_real_loopback_datagram()
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
            call_tool_json(&client, "instance_create", object!({ "type": "UDP" })).await?;
        let handle_id = created["handle_id"].as_str().unwrap();
        call_tool_json(
            &client,
            "tcp_udp_config",
            object!({
                "handle_id": handle_id,
                "bind_host": "127.0.0.1",
                "bind_port": 18091,
                "timeout_ms": 1000
            }),
        )
        .await?;
        call_tool_json(&client, "port_connect", object!({ "handle_id": handle_id })).await?;

        let sender = UdpSocket::bind("127.0.0.1:0").await?;
        sender.send_to(b"pong", "127.0.0.1:18091").await?;

        let pulled = call_tool_json(
            &client,
            "port_pull",
            object!({ "handle_id": handle_id, "max_bytes": 16 }),
        )
        .await?;
        assert_eq!(pulled["data"]["payload"]["preview"], "pong");

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn r1_mcp_debug_exchange_honors_configured_tx_frame_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let port_probe = UdpSocket::bind("127.0.0.1:0").await?;
        let bind_port = port_probe.local_addr()?.port();
        drop(port_probe);

        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let mut limits = RuntimeLimits::default();
        limits.tx_frame_max_bytes = 4;

        let server_handle = tokio::spawn(async move {
            PortMcpServer::new_for_tests_with_limits("20260526", limits)
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

        let set = call_tool_json(
            &client,
            "debug_profile_set",
            object!({
                "transport": "UDP",
                "udp": {
                    "bind_host": "127.0.0.1",
                    "bind_port": bind_port,
                    "remote_host": "127.0.0.1",
                    "remote_port": bind_port,
                    "timeout_ms": 1000
                },
                "payload": { "encoding": "text", "append_line_break": false },
                "pull": { "max_bytes": 64 }
            }),
        )
        .await?;
        assert_eq!(set["ok"], true);

        let connected = call_tool_json(&client, "debug_connect", object!({})).await?;
        assert_eq!(connected["ok"], true);
        let handle_id = connected["handle_id"].as_str().unwrap().to_owned();

        let rejected = call_tool_json(
            &client,
            "debug_exchange",
            object!({ "data": "12345", "handle_id": handle_id }),
        )
        .await?;
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["error"]["code"], "INVALID_RANGE");

        let closed = call_tool_json(&client, "debug_close", object!({ "release": true })).await?;
        assert_eq!(closed["ok"], true);
        assert_eq!(closed["data"]["released"], true);

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m7_request_context_is_reflected_in_subscription_response()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let listen_port = listener.local_addr()?.port();
        let receive_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut buffer = vec![0; 4];
            stream.read_exact(&mut buffer).await?;
            Ok::<(), std::io::Error>(())
        });

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
            object!({ "handle_id": handle_id, "mode": "client", "host": "127.0.0.1", "port": listen_port }),
        )
        .await?;
        call_tool_json(&client, "port_connect", object!({ "handle_id": handle_id })).await?;
        call_tool_json(
            &client,
            "port_send",
            object!({ "handle_id": handle_id, "data": "ping", "encoding": "text" }),
        )
        .await?;
        timeout(Duration::from_secs(5), receive_task)
            .await
            .expect("listener should receive ping")??;

        let subscribed = call_tool_json(
            &client,
            "port_subscribe_stream",
            object!({ "handle_id": handle_id }),
        )
        .await?;

        assert_eq!(subscribed["request_id"], "req_20260526_000005");
        assert_eq!(subscribed["data"]["session_mode"], "request_context_debug");

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[test]
    fn m8_tool_log_event_contains_correlation_state_duration_and_sensitivity_fields() {
        let event = crate::mcp::response::tool_log_event_for_tests(
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

    #[test]
    fn m8_tool_log_event_respects_port_io_display_scope() {
        let response = crate::model::ToolResponse::success(
            "port_send",
            crate::model::RequestId::from_parts("20260526", 123),
            crate::model::Timestamp::now_utc(),
            serde_json::json!({ "sent_bytes": 5 }),
        );
        let hidden = crate::mcp::response::tool_log_event_with_port_io_for_tests(
            &response,
            crate::mcp::response::PortIoLogConfig { max_bytes: 0 },
            Some(crate::mcp::response::PortIoLog::new(
                crate::mcp::response::PortIoDirection::Tx,
                b"hello".to_vec(),
                crate::model::PayloadEncoding::Text,
            )),
        );
        assert!(hidden.get("port_io").is_none());

        let shown = crate::mcp::response::tool_log_event_with_port_io_for_tests(
            &response,
            crate::mcp::response::PortIoLogConfig { max_bytes: 3 },
            Some(crate::mcp::response::PortIoLog::new(
                crate::mcp::response::PortIoDirection::Tx,
                b"hello".to_vec(),
                crate::model::PayloadEncoding::Text,
            )),
        );
        assert_eq!(shown["port_io"]["direction"], "tx");
        assert_eq!(shown["port_io"]["bytes"], 5);
        assert_eq!(shown["port_io"]["preview_encoding"], "text");
        assert_eq!(shown["port_io"]["preview"], "hel");
        assert_eq!(shown["port_io"]["hex"], "68656c");
        assert_eq!(shown["port_io"]["omitted_bytes"], 2);
    }

    #[tokio::test]
    async fn m9_port_scan_rejects_timeout_above_runtime_limit()
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

        let response = call_tool_json(
            &client,
            "port_scan",
            object!({
                "type": "TCP",
                "config": {
                    "host": "127.0.0.1",
                    "start_port": 9000,
                    "end_port": 9000,
                    "timeout_ms": 10_001
                }
            }),
        )
        .await?;

        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "INVALID_RANGE");
        assert_eq!(response["error"]["details"]["field"], "timeout_ms");
        assert_eq!(response["error"]["details"]["max"], 10_000);

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m9_instance_create_enforces_runtime_max_instances()
    -> Result<(), Box<dyn std::error::Error>> {
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
        let mut limits = RuntimeLimits::default();
        limits.max_instances = 1;

        let server_handle = tokio::spawn(async move {
            PortMcpServer::new_for_tests_with_limits("20260526", limits)
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

        let first =
            call_tool_json(&client, "instance_create", object!({ "type": "Serial" })).await?;
        assert_eq!(first["ok"], true);

        let second = call_tool_json(&client, "instance_create", object!({ "type": "TCP" })).await?;
        assert_eq!(second["ok"], false);
        assert_eq!(second["error"]["category"], "BufferLimitExceeded");
        assert_eq!(second["error"]["code"], "INVALID_RANGE");

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m9_config_rejects_runtime_timeout_and_network_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
        let mut limits = RuntimeLimits::default();
        limits.io_timeout_max_ms = 2_000;

        let server_handle = tokio::spawn(async move {
            PortMcpServer::new_for_tests_with_limits("20260526", limits)
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

        let serial =
            call_tool_json(&client, "instance_create", object!({ "type": "Serial" })).await?;
        let serial_handle = serial["handle_id"].as_str().unwrap();
        let serial_config = call_tool_json(
            &client,
            "serial_config",
            object!({ "handle_id": serial_handle, "port": "COM3", "timeout_ms": 2_001 }),
        )
        .await?;
        assert_eq!(serial_config["ok"], false);
        assert_eq!(serial_config["error"]["code"], "INVALID_RANGE");
        assert_eq!(serial_config["error"]["details"]["field"], "timeout_ms");

        let tcp = call_tool_json(&client, "instance_create", object!({ "type": "TCP" })).await?;
        let tcp_handle = tcp["handle_id"].as_str().unwrap();
        let tcp_config = call_tool_json(
            &client,
            "tcp_udp_config",
            object!({ "handle_id": tcp_handle, "mode": "client", "host": "192.0.2.1", "port": 9000 }),
        )
        .await?;
        assert_eq!(tcp_config["ok"], false);
        assert_eq!(tcp_config["error"]["code"], "SCAN_TARGET_NOT_ALLOWED");

        let udp = call_tool_json(&client, "instance_create", object!({ "type": "UDP" })).await?;
        let udp_handle = udp["handle_id"].as_str().unwrap();
        let udp_config = call_tool_json(
            &client,
            "tcp_udp_config",
            object!({
                "handle_id": udp_handle,
                "bind_host": "127.0.0.1",
                "bind_port": 0,
                "timeout_ms": 2_001
            }),
        )
        .await?;
        assert_eq!(udp_config["ok"], false);
        assert_eq!(udp_config["error"]["code"], "INVALID_RANGE");

        let visa = call_tool_json(&client, "instance_create", object!({ "type": "Visa" })).await?;
        let visa_handle = visa["handle_id"].as_str().unwrap();
        let visa_config = call_tool_json(
            &client,
            "visa_config",
            object!({
                "handle_id": visa_handle,
                "resource_address": "TCPIP0::127.0.0.1::INSTR",
                "open_timeout_ms": 2_001,
                "io_timeout_ms": 1000
            }),
        )
        .await?;
        assert_eq!(visa_config["ok"], false);
        assert_eq!(visa_config["error"]["code"], "INVALID_RANGE");
        assert_eq!(visa_config["error"]["details"]["field"], "open_timeout_ms");

        let visa_boundary =
            call_tool_json(&client, "instance_create", object!({ "type": "Visa" })).await?;
        let visa_boundary_handle = visa_boundary["handle_id"].as_str().unwrap();
        let visa_boundary_config = call_tool_json(
            &client,
            "visa_config",
            object!({
                "handle_id": visa_boundary_handle,
                "resource_address": "TCPIP0::192.0.2.1::INSTR",
                "open_timeout_ms": 1000,
                "io_timeout_ms": 1000
            }),
        )
        .await?;
        assert_eq!(visa_boundary_config["ok"], false);
        assert_eq!(
            visa_boundary_config["error"]["code"],
            "SCAN_TARGET_NOT_ALLOWED"
        );
        assert_eq!(
            visa_boundary_config["error"]["details"]["field"],
            "resource_address.host"
        );

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m9_port_scan_routes_by_type_config() -> Result<(), Box<dyn std::error::Error>> {
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

        let serial = call_tool_json(
            &client,
            "port_scan",
            object!({ "type": "Serial", "config": {} }),
        )
        .await?;
        assert_eq!(serial["ok"], true);
        assert!(serial["data"]["resources"].is_array());

        let tcp = call_tool_json(
            &client,
            "port_scan",
            object!({
                "type": "TCP",
                "config": {
                    "host": "127.0.0.1",
                    "start_port": 9000,
                    "end_port": 9000,
                    "timeout_ms": 1000
                }
            }),
        )
        .await?;
        assert_eq!(tcp["ok"], true);
        assert!(tcp["data"]["open_ports"].is_array());

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m10_device_probe_validates_input_and_reports_visa_fallback()
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

        let invalid = call_tool_json(
            &client,
            "device_probe",
            object!({
                "targets": [],
                "payload": { "data": "ping", "encoding": "text" },
                "matcher": { "kind": "any_response" }
            }),
        )
        .await?;
        assert_eq!(invalid["ok"], false);
        assert_eq!(invalid["error"]["code"], "INVALID_RANGE");
        assert_eq!(invalid["error"]["details"]["field"], "targets");

        let visa = call_tool_json(
            &client,
            "device_probe",
            object!({
                "targets": ["Visa"],
                "visa": {
                    "resources": ["USB0::0x0000::0x0000::SN::INSTR"],
                    "open_timeout_ms": 1000,
                    "io_timeout_ms": 1000
                },
                "payload": { "data": "*IDN?", "encoding": "text", "append_line_break": true },
                "matcher": { "kind": "any_response" },
                "failure_output": "samples"
            }),
        )
        .await?;
        assert_eq!(visa["ok"], true);
        assert_eq!(visa["data"]["summary"]["resources_attempted"], 1);
        assert_eq!(visa["data"]["summary"]["skipped_count"], 1);
        assert_eq!(visa["data"]["failure_samples"][0]["status"], "unsupported");
        assert_eq!(
            visa["data"]["failure_samples"][0]["error_code"],
            "FEATURE_NOT_COMPILED"
        );

        let counts = call_tool_json(
            &client,
            "device_probe",
            object!({
                "targets": ["Visa"],
                "visa": {
                    "resources": ["USB0::0x0000::0x0000::SN::INSTR"],
                    "open_timeout_ms": 1000,
                    "io_timeout_ms": 1000
                },
                "payload": { "data": "*IDN?", "encoding": "text", "append_line_break": true },
                "matcher": { "kind": "regex", "value": "(?i).*idn.*|.*" },
                "failure_output": "counts"
            }),
        )
        .await?;
        assert_eq!(counts["ok"], true);
        assert!(counts["data"].get("failure_samples").is_none());
        assert_eq!(
            counts["data"]["summary"]["failure_status_counts"]["unsupported"],
            1
        );
        assert_eq!(
            counts["data"]["summary"]["failure_error_counts"]["FEATURE_NOT_COMPILED"],
            1
        );

        client.cancel().await?;
        server_handle.await??;

        Ok(())
    }

    #[tokio::test]
    async fn m9_debug_log_config_sets_port_io_display_scope()
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

        let configured = call_tool_json(
            &client,
            "debug_log_config",
            object!({ "port_io_log_bytes": 64 }),
        )
        .await?;
        assert_eq!(configured["ok"], true);
        assert_eq!(configured["data"]["port_io_log_bytes"], 64);

        let rejected = call_tool_json(
            &client,
            "debug_log_config",
            object!({ "port_io_log_bytes": 65_537 }),
        )
        .await?;
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["error"]["code"], "INVALID_RANGE");

        client.cancel().await?;
        server_handle.await??;

        Ok(())
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
