use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{DomainError, ErrorCode, HandleId, InstanceState, InstanceType, LastErrorSummary};

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
        Self::tcp_client(host, port)
    }

    pub fn tcp_client(host: &str, port: u16) -> Self {
        Self {
            kind: "tcp-client".to_owned(),
            display: format!("{host}:{port}"),
        }
    }

    pub fn tcp_listen(host: &str, port: u16) -> Self {
        Self {
            kind: "tcp-listen".to_owned(),
            display: format!("{host}:{port}"),
        }
    }

    pub fn udp(
        bind_host: &str,
        bind_port: u16,
        remote_host: Option<&str>,
        remote_port: Option<u16>,
    ) -> Self {
        let display = match (remote_host, remote_port) {
            (Some(remote_host), Some(remote_port)) => {
                format!("{bind_host}:{bind_port} -> {remote_host}:{remote_port}")
            }
            _ => format!("{bind_host}:{bind_port}"),
        };
        Self {
            kind: "udp".to_owned(),
            display,
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
        Ok(())
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
    pub last_activity_at: Option<super::Timestamp>,
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
