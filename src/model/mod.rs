#![allow(dead_code)]

use std::{cell::Cell, collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceType {
    Serial,
    #[serde(rename = "TCP")]
    Tcp,
    #[serde(rename = "UDP")]
    Udp,
}

impl InstanceType {
    fn handle_prefix(self) -> &'static str {
        match self {
            Self::Serial => "ser",
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceState {
    Created,
    Configured,
    Connected,
    Disconnecting,
    Disconnected,
    Error,
    Released,
}

impl InstanceState {
    pub const fn is_persistent(self) -> bool {
        !matches!(self, Self::Released)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandleId(String);

impl HandleId {
    pub fn new_for_type(instance_type: InstanceType, sequence: u64) -> Self {
        Self(format!("h_{}_{sequence:03}", instance_type.handle_prefix()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HandleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    pub fn new(sequence: u64) -> Self {
        Self(format!("req_20260526_{sequence:06}"))
    }

    pub fn from_parts(date: &str, sequence: u64) -> Self {
        Self(format!("req_{date}_{sequence:06}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ErrorId(String);

impl ErrorId {
    pub fn from_parts(date: &str, sequence: u64) -> Self {
        Self(format!("err_{date}_{sequence:06}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(String);

impl Timestamp {
    pub fn now_utc() -> Self {
        let now = time::OffsetDateTime::now_utc();
        let timestamp = now
            .format(&time::format_description::well_known::Rfc3339)
            .expect("UTC timestamp should format as RFC3339");
        Self(normalize_rfc3339(timestamp))
    }

    pub fn from_rfc3339(value: &str) -> Result<Self, time::error::Parse> {
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)?;
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn normalize_rfc3339(value: String) -> String {
    if let Some(stripped) = value.strip_suffix("+00:00") {
        format!("{stripped}Z")
    } else {
        value
    }
}

pub struct IdGenerator {
    date: String,
    request_counter: Cell<u64>,
    error_counter: Cell<u64>,
    serial_counter: Cell<u64>,
    tcp_counter: Cell<u64>,
    udp_counter: Cell<u64>,
}

impl IdGenerator {
    pub fn new_for_tests(date: &str) -> Self {
        Self {
            date: date.to_owned(),
            request_counter: Cell::new(0),
            error_counter: Cell::new(0),
            serial_counter: Cell::new(0),
            tcp_counter: Cell::new(0),
            udp_counter: Cell::new(0),
        }
    }

    pub fn next_request_id(&self) -> RequestId {
        let sequence = self.next(&self.request_counter);
        RequestId::from_parts(&self.date, sequence)
    }

    pub fn next_error_id(&self) -> ErrorId {
        let sequence = self.next(&self.error_counter);
        ErrorId::from_parts(&self.date, sequence)
    }

    pub fn next_handle_id(&self, instance_type: InstanceType) -> HandleId {
        let counter = match instance_type {
            InstanceType::Serial => &self.serial_counter,
            InstanceType::Tcp => &self.tcp_counter,
            InstanceType::Udp => &self.udp_counter,
        };
        HandleId::new_for_type(instance_type, self.next(counter))
    }

    fn next(&self, counter: &Cell<u64>) -> u64 {
        let sequence = counter.get() + 1;
        counter.set(sequence);
        sequence
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceSummary {
    pub handle_id: HandleId,
    #[serde(rename = "type")]
    pub instance_type: InstanceType,
    pub state: InstanceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigSnapshot>,
    pub stats: InstanceStats,
    pub last_error: Option<LastErrorSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSummary {
    pub kind: String,
    pub display: String,
}

impl ResourceSummary {
    pub fn serial(port: &str) -> Self {
        Self {
            kind: "serial".to_owned(),
            display: port.to_owned(),
        }
    }

    pub fn tcp(host: &str, port: u16) -> Self {
        Self {
            kind: "tcp".to_owned(),
            display: format!("{host}:{port}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "config", rename_all = "snake_case")]
pub enum ConfigSnapshot {
    Serial(SerialConfig),
    Tcp(TcpConfig),
    Udp(UdpConfig),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataBits {
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopBits {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Parity {
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowControl {
    None,
    Software,
    Hardware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayloadEncoding {
    Text,
    Hex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerialConfig {
    pub port: String,
    pub baudrate: u32,
    pub data_bits: DataBits,
    pub stop_bits: StopBits,
    pub parity: Parity,
    pub flow_control: FlowControl,
    pub timeout_ms: u64,
    pub encoding: PayloadEncoding,
}

impl SerialConfig {
    pub fn new(port: &str) -> Self {
        Self {
            port: port.to_owned(),
            baudrate: 115_200,
            data_bits: DataBits::Eight,
            stop_bits: StopBits::One,
            parity: Parity::None,
            flow_control: FlowControl::None,
            timeout_ms: 1_000,
            encoding: PayloadEncoding::Text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpConfig {
    pub mode: TcpMode,
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
}

impl TcpConfig {
    pub fn client(host: &str, port: u16) -> Self {
        Self {
            mode: TcpMode::Client,
            host: host.to_owned(),
            port,
            timeout_ms: 1_000,
        }
    }

    pub fn validate_remote(&self) -> Result<(), DomainError> {
        if matches!(self.host.as_str(), "0.0.0.0" | "::" | "") {
            Err(DomainError::invalid_argument(
                ErrorCode::InvalidAddress,
                "Address is not valid for a remote endpoint.",
                "Use a concrete loopback or remote host address.",
            )
            .with_detail("field", json!("host")))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TcpMode {
    Client,
    Listen,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpConfig {
    pub bind_host: String,
    pub bind_port: u16,
    pub remote_host: Option<String>,
    pub remote_port: Option<u16>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstanceStats {
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub rx_buffer_bytes: usize,
    pub rx_dropped_bytes: u64,
    pub tx_queue_items: usize,
    pub subscriber_count: usize,
    pub dropped_notifications: u64,
    pub last_activity_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastErrorSummary {
    pub error_id: ErrorId,
    pub at: Timestamp,
    pub tool: String,
    pub category: ErrorCategory,
    pub code: ErrorCode,
    pub message: String,
    pub recovery_hint: String,
    pub details: ErrorDetails,
}

pub fn validate_required_field(field: &str, value: Option<&str>) -> Result<String, DomainError> {
    value.map(str::to_owned).ok_or_else(|| {
        DomainError::invalid_argument(
            ErrorCode::MissingRequiredField,
            format!("Missing required field `{field}`."),
            format!("Provide `{field}` and retry."),
        )
        .with_detail("field", json!(field))
    })
}

pub fn validate_tcp_port(port: u32) -> Result<u16, DomainError> {
    u16::try_from(port).map_err(|_| {
        DomainError::invalid_argument(
            ErrorCode::InvalidRange,
            "TCP/UDP port is outside 0..65535.",
            "Use a valid TCP/UDP port.",
        )
        .with_detail("field", json!("port"))
        .with_detail("min", json!(0))
        .with_detail("max", json!(65535))
        .with_detail("actual", json!(port))
    })
}

pub fn validate_instance_type(
    actual: InstanceType,
    expected: InstanceType,
) -> Result<(), DomainError> {
    if actual == expected {
        Ok(())
    } else {
        Err(DomainError::invalid_argument(
            ErrorCode::TypeMismatch,
            "Instance type does not match this tool.",
            "Use the configuration tool that matches the instance type.",
        )
        .with_detail("expected_type", json!(expected))
        .with_detail("actual_type", json!(actual)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCategory {
    HandleNotFound,
    InvalidState,
    InvalidArgument,
    ResourceBusy,
    ConnectTimeout,
    ReadTimeout,
    WriteFailed,
    BufferLimitExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    HandleNotFound,
    HandleReleased,
    SessionBindingMissing,
    StateNotAllowed,
    ConfigRequired,
    ConnectedReleaseRequiresForce,
    ErrorRequiresDisconnect,
    DisconnectingInProgress,
    SessionIdUnavailable,
    MissingRequiredField,
    InvalidFieldType,
    InvalidEnumValue,
    InvalidRange,
    TypeMismatch,
    InvalidAddress,
    ScanTargetNotAllowed,
    InvalidHex,
    TextEncodingFailed,
    SerialPortBusy,
    TcpListenAddrBusy,
    UdpBindAddrBusy,
    ResourceClosing,
    ResourceLockStale,
    ConnectTimeout,
    SerialOpenTimeout,
    ScanTimeout,
    ReadTimeout,
    NoDataAvailable,
    WriteIoFailed,
    ReadIoFailed,
    TransportClosed,
    TaskFailed,
    DisconnectFailed,
    ReleaseFailed,
    TxQueueClosed,
    TxQueueFull,
    TxFrameTooLarge,
    PullMaxBytesExceeded,
    SubscriberPayloadTooLarge,
    SubscriberLimitExceeded,
    ScanRangeTooLarge,
    ResultTooLarge,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainError {
    pub category: ErrorCategory,
    pub code: ErrorCode,
    pub message: String,
    pub recovery_hint: String,
    pub retryable: bool,
    pub details: ErrorDetails,
}

impl DomainError {
    pub fn new(
        category: ErrorCategory,
        code: ErrorCode,
        message: impl Into<String>,
        recovery_hint: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            category,
            code,
            message: message.into(),
            recovery_hint: recovery_hint.into(),
            retryable,
            details: ErrorDetails::new(),
        }
    }

    pub fn invalid_argument(
        code: ErrorCode,
        message: impl Into<String>,
        recovery_hint: impl Into<String>,
    ) -> Self {
        Self::new(
            ErrorCategory::InvalidArgument,
            code,
            message,
            recovery_hint,
            false,
        )
    }

    pub fn session_id_unavailable() -> Self {
        Self::new(
            ErrorCategory::InvalidState,
            ErrorCode::SessionIdUnavailable,
            "Stable MCP session id is unavailable in the current runtime mode.",
            "Pass an explicit handle_id, or enable a runtime mode with stable session identity.",
            false,
        )
    }

    pub fn read_timeout() -> Self {
        Self::new(
            ErrorCategory::ReadTimeout,
            ErrorCode::ReadTimeout,
            "No data was available before the read timeout elapsed.",
            "Retry, increase timeout_ms, or subscribe to receive notifications.",
            true,
        )
    }

    pub fn with_detail(mut self, key: &str, value: Value) -> Self {
        self.details = self.details.with(key, value);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct ErrorDetails(BTreeMap<String, Value>);

impl ErrorDetails {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.0.insert(key.to_owned(), value);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResponse {
    Success(SuccessResponse),
    Failure(FailureResponse),
}

impl ToolResponse {
    pub fn success(tool: &str, request_id: RequestId, timestamp: Timestamp, data: Value) -> Self {
        Self::Success(SuccessResponse {
            ok: true,
            tool: tool.to_owned(),
            request_id,
            timestamp,
            handle_id: None,
            state: None,
            data,
            warnings: Vec::new(),
        })
    }

    pub fn failure(
        tool: &str,
        request_id: RequestId,
        timestamp: Timestamp,
        error: DomainError,
    ) -> Self {
        Self::Failure(FailureResponse {
            ok: false,
            tool: tool.to_owned(),
            request_id,
            timestamp,
            handle_id: None,
            state: None,
            error,
        })
    }

    pub fn with_handle(mut self, handle_id: HandleId) -> Self {
        match &mut self {
            Self::Success(response) => response.handle_id = Some(handle_id),
            Self::Failure(response) => response.handle_id = Some(handle_id),
        }
        self
    }

    pub fn with_state(mut self, state: InstanceState) -> Self {
        match &mut self {
            Self::Success(response) => response.state = Some(state),
            Self::Failure(response) => response.state = Some(state),
        }
        self
    }

    pub fn with_warning(mut self, warning: Warning) -> Self {
        if let Self::Success(response) = &mut self {
            response.warnings.push(warning);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessResponse {
    pub ok: bool,
    pub tool: String,
    pub request_id: RequestId,
    pub timestamp: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle_id: Option<HandleId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<InstanceState>,
    pub data: Value,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureResponse {
    pub ok: bool,
    pub tool: String,
    pub request_id: RequestId,
    pub timestamp: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle_id: Option<HandleId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<InstanceState>,
    pub error: DomainError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

impl Warning {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
        }
    }
}

#[derive(Default)]
pub struct Redactor;

impl Redactor {
    pub fn local_path(&self, path: &str) -> Value {
        let normalized = path.replace('\\', "/");
        let file_name = normalized.rsplit('/').next().unwrap_or("<redacted-path>");
        json!(file_name)
    }

    pub fn env_value(&self, name: &str, _value: &str) -> Value {
        json!({ "name": name, "value": "<redacted>" })
    }

    pub fn payload_preview(&self, bytes: &[u8], max_preview_bytes: usize) -> Value {
        let summary =
            PayloadSummary::from_bytes(bytes, PayloadEncoding::Text, max_preview_bytes, false);
        serde_json::to_value(summary).expect("payload summary should serialize")
    }

    pub fn os_error(&self, message: &str) -> Value {
        let lowered = message.to_ascii_lowercase();
        let io_kind = if lowered.contains("permission") || lowered.contains("access") {
            "permission_denied"
        } else if lowered.contains("not found") {
            "not_found"
        } else {
            "other"
        };
        json!({ "io_kind": io_kind, "message": "<redacted>" })
    }

    pub fn stack_trace(&self, _stack: &str) -> Value {
        json!("<redacted>")
    }
}

#[derive(Debug)]
pub struct Payload {
    pub bytes: Vec<u8>,
    pub encoding: PayloadEncoding,
}

impl Payload {
    pub fn from_text(input: &str, append_line_break: bool) -> Result<Self, DomainError> {
        let mut bytes = input.as_bytes().to_vec();
        if append_line_break {
            bytes.push(b'\n');
        }
        Ok(Self {
            bytes,
            encoding: PayloadEncoding::Text,
        })
    }

    pub fn from_hex(input: &str, append_line_break: bool) -> Result<Self, DomainError> {
        if !input.len().is_multiple_of(2)
            || !input.chars().all(|character| character.is_ascii_hexdigit())
        {
            return Err(DomainError::invalid_argument(
                ErrorCode::InvalidHex,
                "Hex payload must contain an even number of hexadecimal characters.",
                "Use only 0-9, a-f, A-F and provide an even character count.",
            ));
        }

        let mut bytes = Vec::with_capacity(input.len() / 2 + usize::from(append_line_break));
        for index in (0..input.len()).step_by(2) {
            let byte = u8::from_str_radix(&input[index..index + 2], 16).map_err(|_| {
                DomainError::invalid_argument(
                    ErrorCode::InvalidHex,
                    "Hex payload contains invalid characters.",
                    "Use only valid hexadecimal characters.",
                )
            })?;
            bytes.push(byte);
        }
        if append_line_break {
            bytes.push(b'\n');
        }

        Ok(Self {
            bytes,
            encoding: PayloadEncoding::Hex,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadSummary {
    pub preview: String,
    pub preview_encoding: PayloadEncoding,
    pub payload_bytes: usize,
    pub omitted_bytes: usize,
    pub truncated: bool,
    pub datagram: bool,
}

impl PayloadSummary {
    pub fn from_bytes(
        bytes: &[u8],
        encoding: PayloadEncoding,
        max_preview_bytes: usize,
        datagram: bool,
    ) -> Self {
        let preview_bytes = bytes.len().min(max_preview_bytes);
        let preview = match encoding {
            PayloadEncoding::Text => String::from_utf8_lossy(&bytes[..preview_bytes]).to_string(),
            PayloadEncoding::Hex => bytes[..preview_bytes]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        };
        Self {
            preview,
            preview_encoding: encoding,
            payload_bytes: bytes.len(),
            omitted_bytes: bytes.len().saturating_sub(preview_bytes),
            truncated: bytes.len() > preview_bytes,
            datagram,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLimits {
    pub tx_queue_max_items: usize,
    pub tx_frame_max_bytes: usize,
    pub rx_buffer_max_bytes: usize,
    pub pull_default_max_bytes: usize,
    pub pull_max_bytes: usize,
    pub subscriber_queue_max_items: usize,
    pub subscriber_payload_max_bytes: usize,
    pub subscriber_notifications_per_sec: u32,
    pub instance_notifications_per_sec: u32,
    pub global_notifications_per_sec: u32,
    pub notification_burst: u32,
    pub scan_max_concurrency: usize,
    pub scan_max_ports: usize,
    pub scan_total_timeout_ms: u64,
    pub max_instances: usize,
    pub max_subscribers_per_instance: usize,
    pub max_total_buffer_bytes: usize,
    pub max_total_queued_bytes: usize,
    pub force_close_deadline_ms: u64,
    pub scan_allowed_hosts: Vec<String>,
}

impl RuntimeLimits {
    pub const ABS_MAX_TOTAL_BUFFER_BYTES: usize = 512 * 1024 * 1024;
    pub const ABS_MAX_TOTAL_QUEUED_BYTES: usize = 256 * 1024 * 1024;

    pub fn validate_force_close_deadline(&self, value: u64) -> Result<u64, DomainError> {
        if (1_000..=30_000).contains(&value) {
            Ok(value)
        } else {
            Err(DomainError::invalid_argument(
                ErrorCode::InvalidRange,
                "force_close_deadline_ms is outside the allowed range.",
                "Use a value between 1000 and 30000 milliseconds.",
            )
            .with_detail("field", json!("force_close_deadline_ms"))
            .with_detail("min", json!(1000))
            .with_detail("max", json!(30000))
            .with_detail("actual", json!(value)))
        }
    }

    pub fn validate_pull_max_bytes(&self, value: usize) -> Result<usize, DomainError> {
        if value <= self.pull_max_bytes {
            Ok(value)
        } else {
            Err(DomainError::new(
                ErrorCategory::BufferLimitExceeded,
                ErrorCode::PullMaxBytesExceeded,
                "Requested pull max_bytes exceeds the configured limit.",
                "Lower max_bytes and retry.",
                false,
            )
            .with_detail("limit", json!(self.pull_max_bytes))
            .with_detail("requested", json!(value)))
        }
    }
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            tx_queue_max_items: 256,
            tx_frame_max_bytes: 64 * 1024,
            rx_buffer_max_bytes: 1024 * 1024,
            pull_default_max_bytes: 4 * 1024,
            pull_max_bytes: 64 * 1024,
            subscriber_queue_max_items: 128,
            subscriber_payload_max_bytes: 16 * 1024,
            subscriber_notifications_per_sec: 64,
            instance_notifications_per_sec: 256,
            global_notifications_per_sec: 1024,
            notification_burst: 128,
            scan_max_concurrency: 64,
            scan_max_ports: 256,
            scan_total_timeout_ms: 10_000,
            max_instances: 64,
            max_subscribers_per_instance: 32,
            max_total_buffer_bytes: 128 * 1024 * 1024,
            max_total_queued_bytes: 64 * 1024 * 1024,
            force_close_deadline_ms: 5_000,
            scan_allowed_hosts: vec!["127.0.0.0/8".to_owned(), "::1".to_owned()],
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn unit_state_model_serializes_documented_names_and_summary_shape() {
        assert_eq!(json!(InstanceType::Serial), "Serial");
        assert_eq!(json!(InstanceType::Tcp), "TCP");
        assert_eq!(json!(InstanceType::Udp), "UDP");

        let states = vec![
            InstanceState::Created,
            InstanceState::Configured,
            InstanceState::Connected,
            InstanceState::Disconnecting,
            InstanceState::Disconnected,
            InstanceState::Error,
            InstanceState::Released,
        ];
        assert_eq!(
            serde_json::to_value(&states).unwrap(),
            json!([
                "Created",
                "Configured",
                "Connected",
                "Disconnecting",
                "Disconnected",
                "Error",
                "Released"
            ])
        );
        assert!(!InstanceState::Released.is_persistent());

        let summary = InstanceSummary {
            handle_id: HandleId::new_for_type(InstanceType::Tcp, 7),
            instance_type: InstanceType::Tcp,
            state: InstanceState::Configured,
            resource: Some(ResourceSummary::tcp("127.0.0.1", 9000)),
            config: Some(ConfigSnapshot::Tcp(TcpConfig::client("127.0.0.1", 9000))),
            stats: InstanceStats::default(),
            last_error: None,
        };

        let value = serde_json::to_value(summary).unwrap();
        assert_eq!(value["handle_id"], "h_tcp_007");
        assert_eq!(value["type"], "TCP");
        assert_eq!(value["state"], "Configured");
        assert_eq!(
            value["resource"],
            json!({ "kind": "tcp", "display": "127.0.0.1:9000" })
        );
        assert_eq!(value["stats"]["rx_buffer_bytes"], 0);
        assert_eq!(
            serde_json::to_value(ResourceSummary::serial("COM3")).unwrap(),
            json!({
                "kind": "serial",
                "display": "COM3"
            })
        );
    }

    #[test]
    fn unit_argument_validation_covers_config_defaults_and_errors() {
        let serial = SerialConfig::new("COM3");
        assert_eq!(serial.baudrate, 115_200);
        assert_eq!(serial.data_bits, DataBits::Eight);
        assert_eq!(serial.stop_bits, StopBits::One);
        assert_eq!(serial.parity, Parity::None);
        assert_eq!(serial.flow_control, FlowControl::None);
        assert_eq!(serial.timeout_ms, 1_000);
        assert_eq!(serial.encoding, PayloadEncoding::Text);

        assert_eq!(
            validate_required_field("port", Some("COM3")).unwrap(),
            "COM3"
        );
        assert_eq!(
            validate_required_field("port", None).unwrap_err().code,
            ErrorCode::MissingRequiredField
        );
        assert_eq!(
            validate_tcp_port(70_000).unwrap_err().code,
            ErrorCode::InvalidRange
        );
        assert_eq!(
            validate_instance_type(InstanceType::Serial, InstanceType::Tcp)
                .unwrap_err()
                .code,
            ErrorCode::TypeMismatch
        );
        assert_eq!(
            TcpConfig::client("0.0.0.0", 9000)
                .validate_remote()
                .unwrap_err()
                .code,
            ErrorCode::InvalidAddress
        );
    }

    #[test]
    fn unit_response_shape_serializes_success_failure_and_warnings() {
        let request_id = RequestId::new(1);
        let success = ToolResponse::success(
            "port_send",
            request_id.clone(),
            Timestamp::from_rfc3339("2026-05-26T00:00:00.000Z").unwrap(),
            json!({ "queued": true, "sent_bytes": 6 }),
        )
        .with_handle(HandleId::new_for_type(InstanceType::Tcp, 1))
        .with_state(InstanceState::Connected)
        .with_warning(Warning::new(
            "rx_buffer_truncated",
            "Older bytes were dropped.",
        ));

        let success_value = serde_json::to_value(success).unwrap();
        assert_eq!(success_value["ok"], true);
        assert_eq!(success_value["tool"], "port_send");
        assert_eq!(success_value["request_id"], "req_20260526_000001");
        assert_eq!(success_value["timestamp"], "2026-05-26T00:00:00.000Z");
        assert_eq!(success_value["handle_id"], "h_tcp_001");
        assert_eq!(success_value["state"], "Connected");
        assert_eq!(success_value["warnings"][0]["code"], "rx_buffer_truncated");

        let failure = ToolResponse::failure(
            "instance_use",
            RequestId::new(2),
            Timestamp::from_rfc3339("2026-05-26T00:00:01.000Z").unwrap(),
            DomainError::session_id_unavailable(),
        )
        .with_handle(HandleId::new_for_type(InstanceType::Tcp, 1));

        let failure_value = serde_json::to_value(failure).unwrap();
        assert_eq!(failure_value["ok"], false);
        assert_eq!(failure_value["error"]["category"], "InvalidState");
        assert_eq!(failure_value["error"]["code"], "SESSION_ID_UNAVAILABLE");
        assert_eq!(failure_value["error"]["retryable"], false);
        assert!(failure_value.get("warnings").is_none());
    }

    #[test]
    fn unit_error_codes_cover_m1_required_categories_and_details() {
        let cases = [
            (ErrorCategory::HandleNotFound, ErrorCode::HandleNotFound),
            (ErrorCategory::HandleNotFound, ErrorCode::HandleReleased),
            (
                ErrorCategory::HandleNotFound,
                ErrorCode::SessionBindingMissing,
            ),
            (ErrorCategory::InvalidState, ErrorCode::StateNotAllowed),
            (ErrorCategory::InvalidState, ErrorCode::ConfigRequired),
            (
                ErrorCategory::InvalidState,
                ErrorCode::ConnectedReleaseRequiresForce,
            ),
            (ErrorCategory::InvalidState, ErrorCode::SessionIdUnavailable),
            (
                ErrorCategory::InvalidArgument,
                ErrorCode::MissingRequiredField,
            ),
            (ErrorCategory::InvalidArgument, ErrorCode::InvalidFieldType),
            (ErrorCategory::InvalidArgument, ErrorCode::InvalidEnumValue),
            (ErrorCategory::InvalidArgument, ErrorCode::InvalidRange),
            (ErrorCategory::InvalidArgument, ErrorCode::TypeMismatch),
            (ErrorCategory::InvalidArgument, ErrorCode::InvalidAddress),
            (ErrorCategory::InvalidArgument, ErrorCode::InvalidHex),
            (
                ErrorCategory::InvalidArgument,
                ErrorCode::TextEncodingFailed,
            ),
            (ErrorCategory::ResourceBusy, ErrorCode::SerialPortBusy),
            (ErrorCategory::ResourceBusy, ErrorCode::TcpListenAddrBusy),
            (ErrorCategory::ResourceBusy, ErrorCode::UdpBindAddrBusy),
            (ErrorCategory::ResourceBusy, ErrorCode::ResourceClosing),
            (ErrorCategory::ResourceBusy, ErrorCode::ResourceLockStale),
            (ErrorCategory::ConnectTimeout, ErrorCode::ConnectTimeout),
            (ErrorCategory::ConnectTimeout, ErrorCode::SerialOpenTimeout),
            (ErrorCategory::ConnectTimeout, ErrorCode::ScanTimeout),
            (ErrorCategory::ReadTimeout, ErrorCode::ReadTimeout),
            (ErrorCategory::ReadTimeout, ErrorCode::NoDataAvailable),
            (ErrorCategory::WriteFailed, ErrorCode::WriteIoFailed),
            (ErrorCategory::WriteFailed, ErrorCode::ReadIoFailed),
            (ErrorCategory::WriteFailed, ErrorCode::TransportClosed),
            (ErrorCategory::WriteFailed, ErrorCode::TaskFailed),
            (ErrorCategory::WriteFailed, ErrorCode::DisconnectFailed),
            (ErrorCategory::WriteFailed, ErrorCode::TxQueueClosed),
            (ErrorCategory::BufferLimitExceeded, ErrorCode::TxQueueFull),
            (
                ErrorCategory::BufferLimitExceeded,
                ErrorCode::TxFrameTooLarge,
            ),
            (
                ErrorCategory::BufferLimitExceeded,
                ErrorCode::PullMaxBytesExceeded,
            ),
            (
                ErrorCategory::BufferLimitExceeded,
                ErrorCode::SubscriberPayloadTooLarge,
            ),
            (
                ErrorCategory::BufferLimitExceeded,
                ErrorCode::SubscriberLimitExceeded,
            ),
            (
                ErrorCategory::BufferLimitExceeded,
                ErrorCode::ScanRangeTooLarge,
            ),
            (
                ErrorCategory::BufferLimitExceeded,
                ErrorCode::ResultTooLarge,
            ),
        ];

        for (category, code) in cases {
            let error = DomainError::new(category, code, "message", "hint", false)
                .with_detail("field", json!("timeout_ms"));
            let value = serde_json::to_value(error).unwrap();
            assert_eq!(value["category"], json!(category));
            assert_eq!(value["code"], json!(code));
            assert_eq!(value["details"]["field"], "timeout_ms");
        }

        let retryable = DomainError::read_timeout();
        assert!(retryable.retryable);
        assert_eq!(retryable.category, ErrorCategory::ReadTimeout);
        assert_eq!(retryable.code, ErrorCode::ReadTimeout);
    }

    #[test]
    fn unit_ids_time_generates_readable_unique_ids_and_rfc3339_timestamp() {
        let generator = IdGenerator::new_for_tests("20260526");
        assert_eq!(generator.next_request_id().as_str(), "req_20260526_000001");
        assert_eq!(generator.next_request_id().as_str(), "req_20260526_000002");
        assert_eq!(generator.next_error_id().as_str(), "err_20260526_000001");
        assert_eq!(
            generator.next_handle_id(InstanceType::Serial).as_str(),
            "h_ser_001"
        );
        assert_eq!(
            generator.next_handle_id(InstanceType::Udp).as_str(),
            "h_udp_001"
        );

        let timestamp = Timestamp::now_utc();
        assert!(timestamp.as_str().ends_with('Z'));
        assert!(Timestamp::from_rfc3339(timestamp.as_str()).is_ok());
    }

    #[test]
    fn unit_redaction_removes_sensitive_paths_users_env_payload_and_os_text() {
        let redactor = Redactor::default();
        let details = ErrorDetails::new()
            .with(
                "path",
                redactor.local_path("C:\\Users\\alice\\secret\\device.log"),
            )
            .with("env", redactor.env_value("PORT_MCP_TOKEN", "secret-token"))
            .with(
                "payload",
                redactor.payload_preview(b"abcdefghijklmnopqrstuvwxyz", 8),
            )
            .with(
                "os_error",
                redactor.os_error("permission denied at C:\\Users\\alice\\secret"),
            )
            .with(
                "stack",
                redactor.stack_trace("panic at C:\\Users\\alice\\main.rs:1"),
            );

        let value = serde_json::to_value(details).unwrap();
        let rendered = serde_json::to_string(&value).unwrap();
        assert!(!rendered.contains("alice"));
        assert!(!rendered.contains("secret-token"));
        assert!(!rendered.contains("abcdefghijklmnopqrstuvwxyz"));
        assert_eq!(value["path"], "device.log");
        assert_eq!(
            value["env"],
            json!({ "name": "PORT_MCP_TOKEN", "value": "<redacted>" })
        );
        assert_eq!(value["payload"]["omitted_bytes"], 18);
        assert_eq!(value["os_error"]["io_kind"], "permission_denied");
        assert_eq!(value["stack"], "<redacted>");
    }

    #[test]
    fn unit_payload_summary_handles_text_hex_line_break_truncation_and_datagrams() {
        let text = Payload::from_text("ping", true).unwrap();
        assert_eq!(text.bytes, b"ping\n");
        assert_eq!(text.encoding, PayloadEncoding::Text);

        let hex = Payload::from_hex("70696e67", false).unwrap();
        assert_eq!(hex.bytes, b"ping");
        assert_eq!(hex.encoding, PayloadEncoding::Hex);
        assert_eq!(
            Payload::from_hex("abc", false).unwrap_err().code,
            ErrorCode::InvalidHex
        );

        let summary = PayloadSummary::from_bytes(b"0123456789", PayloadEncoding::Text, 4, true);
        assert_eq!(summary.preview, "0123");
        assert_eq!(summary.payload_bytes, 10);
        assert_eq!(summary.omitted_bytes, 6);
        assert!(summary.truncated);
        assert!(summary.datagram);
    }

    #[test]
    fn unit_limits_defines_defaults_hard_limits_and_range_errors() {
        let limits = RuntimeLimits::default();
        assert_eq!(limits.tx_queue_max_items, 256);
        assert_eq!(limits.tx_frame_max_bytes, 64 * 1024);
        assert_eq!(limits.rx_buffer_max_bytes, 1024 * 1024);
        assert_eq!(limits.pull_default_max_bytes, 4 * 1024);
        assert_eq!(limits.pull_max_bytes, 64 * 1024);
        assert_eq!(limits.subscriber_queue_max_items, 128);
        assert_eq!(limits.subscriber_payload_max_bytes, 16 * 1024);
        assert_eq!(limits.subscriber_notifications_per_sec, 64);
        assert_eq!(limits.instance_notifications_per_sec, 256);
        assert_eq!(limits.global_notifications_per_sec, 1024);
        assert_eq!(limits.notification_burst, 128);
        assert_eq!(limits.scan_max_concurrency, 64);
        assert_eq!(limits.scan_max_ports, 256);
        assert_eq!(limits.scan_total_timeout_ms, 10_000);
        assert_eq!(limits.max_instances, 64);
        assert_eq!(limits.max_subscribers_per_instance, 32);
        assert_eq!(limits.max_total_buffer_bytes, 128 * 1024 * 1024);
        assert_eq!(limits.max_total_queued_bytes, 64 * 1024 * 1024);
        assert_eq!(limits.force_close_deadline_ms, 5_000);

        assert!(RuntimeLimits::ABS_MAX_TOTAL_BUFFER_BYTES >= limits.max_total_buffer_bytes);
        assert!(RuntimeLimits::ABS_MAX_TOTAL_QUEUED_BYTES >= limits.max_total_queued_bytes);
        assert_eq!(
            limits.validate_force_close_deadline(0).unwrap_err().code,
            ErrorCode::InvalidRange
        );
        assert_eq!(
            limits.validate_pull_max_bytes(128 * 1024).unwrap_err().code,
            ErrorCode::PullMaxBytesExceeded
        );

        let serialized: Value = serde_json::to_value(limits).unwrap();
        assert_eq!(
            serialized["scan_allowed_hosts"],
            json!(["127.0.0.0/8", "::1"])
        );
    }
}
