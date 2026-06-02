use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    DomainError, ErrorCode, HandleId, InstanceState, InstanceType, LastErrorSummary, RuntimeLimits,
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers: Option<Vec<PeerSummary>>,
    pub last_error: Option<LastErrorSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerSummary {
    pub peer_id: String,
    pub remote_addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSummary {
    pub kind: String,
    pub display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_class: Option<String>,
}

impl ResourceSummary {
    pub fn serial(port: &str) -> Self {
        Self {
            kind: "serial".to_owned(),
            display: port.to_owned(),
            resource_class: None,
        }
    }

    pub fn tcp(host: &str, port: u16) -> Self {
        Self::tcp_client(host, port)
    }

    pub fn tcp_client(host: &str, port: u16) -> Self {
        Self {
            kind: "tcp-client".to_owned(),
            display: format!("{host}:{port}"),
            resource_class: None,
        }
    }

    pub fn tcp_listen(host: &str, port: u16) -> Self {
        Self {
            kind: "tcp-listen".to_owned(),
            display: format!("{host}:{port}"),
            resource_class: None,
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
            resource_class: None,
        }
    }

    pub fn visa(resource_address: &str, resource_class: Option<&str>) -> Self {
        Self {
            kind: "visa".to_owned(),
            display: resource_address.to_owned(),
            resource_class: resource_class.map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "config", rename_all = "snake_case")]
pub enum ConfigSnapshot {
    Serial(SerialConfig),
    Tcp(TcpConfig),
    Udp(UdpConfig),
    Visa(VisaConfig),
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

    pub fn validate_remote(&self, limits: &RuntimeLimits) -> Result<(), DomainError> {
        let field = match self.mode {
            TcpMode::Client => "host",
            TcpMode::Listen => "bind_host",
        };
        limits.validate_network_host(field, &self.host)?;
        limits.validate_io_timeout_ms("timeout_ms", self.timeout_ms)?;
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

impl UdpConfig {
    pub fn validate_remote(&self, limits: &RuntimeLimits) -> Result<(), DomainError> {
        limits.validate_network_host("bind_host", &self.bind_host)?;
        if let Some(remote_host) = &self.remote_host {
            limits.validate_network_host("remote_host", remote_host)?;
        }
        limits.validate_io_timeout_ms("timeout_ms", self.timeout_ms)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisaConfig {
    pub resource_address: String,
    pub open_timeout_ms: u64,
    pub io_timeout_ms: u64,
    pub read_termination: Option<String>,
    pub write_termination: Option<String>,
    pub encoding: PayloadEncoding,
    pub query_idn_on_connect: bool,
}

impl VisaConfig {
    pub fn new(resource_address: &str) -> Self {
        Self {
            resource_address: resource_address.to_owned(),
            open_timeout_ms: 1_000,
            io_timeout_ms: 1_000,
            read_termination: None,
            write_termination: None,
            encoding: PayloadEncoding::Text,
            query_idn_on_connect: false,
        }
    }

    pub fn validate(&self, limits: &RuntimeLimits) -> Result<(), DomainError> {
        limits.validate_io_timeout_ms("open_timeout_ms", self.open_timeout_ms)?;
        limits.validate_io_timeout_ms("io_timeout_ms", self.io_timeout_ms)?;
        if let Some(host) = visa_tcpip_host(&self.resource_address)? {
            limits.validate_network_host("resource_address.host", host)?;
        }
        Ok(())
    }
}

fn visa_tcpip_host(resource_address: &str) -> Result<Option<&str>, DomainError> {
    let mut parts = resource_address.split("::");
    let class = parts.next().unwrap_or_default().trim();
    if !class.to_ascii_uppercase().starts_with("TCPIP") {
        return Ok(None);
    }

    let host = parts.next().map(str::trim).ok_or_else(|| {
        DomainError::invalid_argument(
            ErrorCode::InvalidAddress,
            "VISA TCPIP resource address is missing a host.",
            "Use a valid TCPIP resource address such as TCPIP0::127.0.0.1::INSTR.",
        )
        .with_detail("field", serde_json::json!("resource_address.host"))
        .with_detail("resource_address", serde_json::json!(resource_address))
    })?;

    if host.is_empty() {
        return Err(DomainError::invalid_argument(
            ErrorCode::InvalidAddress,
            "VISA TCPIP resource address is missing a host.",
            "Use a valid TCPIP resource address such as TCPIP0::127.0.0.1::INSTR.",
        )
        .with_detail("field", serde_json::json!("resource_address.host"))
        .with_detail("resource_address", serde_json::json!(resource_address)));
    }

    Ok(Some(host))
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
