use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, handler::server::wrapper::Parameters,
    model::CallToolResult, schemars, service::RequestContext, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    app::{InstanceService, PortService},
    mcp::response::{self, PortIoDirection, PortIoLog, PortIoLogConfig},
    mcp::session::SessionMode,
    model::{
        DataBits, DomainError, ErrorCode, FlowControl, HandleId, IdGenerator, InstanceState,
        InstanceSummary, InstanceType, Parity, Payload, PayloadEncoding, RuntimeLimits,
        SerialConfig, StopBits, TcpConfig, TcpMode, Timestamp, ToolResponse, UdpConfig, VisaConfig,
    },
    runtime::ClearTarget,
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
}

impl PortMcpServer {
    pub fn new() -> Self {
        Self::new_for_tests("20260526")
    }

    pub fn new_for_tests(date: &str) -> Self {
        Self::new_for_tests_with_limits(date, RuntimeLimits::default())
    }

    pub fn new_for_tests_with_limits(date: &str, limits: RuntimeLimits) -> Self {
        Self {
            app: Arc::new(Mutex::new(InstanceService::new_for_tests_with_limits(
                date, limits,
            ))),
            ids: Arc::new(Mutex::new(IdGenerator::new_for_tests(date))),
            port_io_log_config: Arc::new(Mutex::new(PortIoLogConfig::default())),
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
        format!("mcp-session-{:#?}", context.id)
    }

    fn usage_guide_data() -> Value {
        json!({
            "purpose": "Help a new MCP agent use port-mcp correctly when only tool metadata is available.",
            "principles": [
                "Always pass handle_id explicitly after instance_create; do not rely on session defaults unless you intentionally called instance_use.",
                "Normal lifecycle is create -> configure -> connect -> send/pull or subscribe -> disconnect -> release.",
                "Use port_scan before serial_config or visa_config when the resource name is unknown.",
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
                    { "tool": "port_connect", "arguments": { "handle_id": "<handle_id>" } }
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
                "port_connect": "Open the configured Serial, TCP, UDP, or Visa resource. Requires state Configured or Disconnected.",
                "port_disconnect": "Close a Connected instance while keeping its config for later reconnect.",
                "port_send": "Send data on a Connected instance. encoding is text or hex; append_line_break defaults to false. Visa instances may append write_termination.",
                "port_pull": "Read received bytes from a Connected instance. max_bytes is optional and bounded by runtime limits. Visa instances may honor read_termination.",
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
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
pub struct DebugLogConfigParams {
    #[serde(default)]
    port_io_log_bytes: usize,
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
        let session_id = self.session_id(&context);
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
                let service = PortService::new_for_tests("20260526");
                let response = match service.scan_serial() {
                    Ok(resources) => self.ok("port_scan", json!({ "resources": resources })),
                    Err(error) => self.err("port_scan", error),
                };
                self.result(started_at, response)
            }
            InstanceTypeParam::Visa => {
                let service = PortService::new_for_tests("20260526");
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
        let service = PortService::new_for_tests("20260526");
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
        let payload = match params.encoding {
            EncodingParam::Text => Payload::from_text(&params.data, params.append_line_break),
            EncodingParam::Hex => Payload::from_hex(&params.data, params.append_line_break),
        };
        let response = match payload {
            Ok(payload) => {
                let port_io =
                    PortIoLog::new(PortIoDirection::Tx, payload.bytes.clone(), payload.encoding);
                let response = match self
                    .with_app_blocking(move |app| app.send(&handle_for_app, &payload))
                    .await
                {
                    Ok(result) => self
                        .ok(
                            "port_send",
                            json!({ "queued": result.queued, "sent_bytes": result.sent_bytes }),
                        )
                        .with_handle(handle)
                        .with_state(InstanceState::Connected),
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
        let response = match self
            .with_app_blocking(move |app| app.pull(&handle_for_app, params.max_bytes))
            .await
        {
            Ok(result) => {
                let port_io = PortIoLog::new(
                    PortIoDirection::Rx,
                    result.bytes.clone(),
                    PayloadEncoding::Text,
                );
                let response = self.ok(
                    "port_pull",
                    json!({
                        "payload": PortService::summarize_payload(&result.bytes, PayloadEncoding::Text),
                        "truncated": result.truncated,
                        "remaining_rx_buffer_bytes": result.remaining_rx_buffer_bytes
                    }),
                )
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

    use super::PortMcpServer;
    use crate::model::RuntimeLimits;

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
        assert_eq!(rejected["error"]["code"], "TX_FRAME_TOO_LARGE");

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
        assert_eq!(rejected["error"]["code"], "TX_FRAME_TOO_LARGE");

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
