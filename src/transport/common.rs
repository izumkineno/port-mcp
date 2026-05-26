use std::{io, time::Duration};

use tokio::{net::TcpStream, time::timeout};

use crate::model::{DomainError, ErrorCategory, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    pub category: ErrorCategory,
    pub code: ErrorCode,
    pub message: String,
    pub fatal: bool,
}

impl TransportError {
    pub fn read_timeout(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::ReadTimeout,
            code: ErrorCode::ReadTimeout,
            message: message.into(),
            fatal: false,
        }
    }

    pub fn transport_closed(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::WriteFailed,
            code: ErrorCode::TransportClosed,
            message: message.into(),
            fatal: true,
        }
    }

    pub fn write_failed(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::WriteFailed,
            code,
            message: message.into(),
            fatal: true,
        }
    }

    pub fn connect_timeout(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::ConnectTimeout,
            code: ErrorCode::ConnectTimeout,
            message: message.into(),
            fatal: true,
        }
    }

    pub fn tcp_listen_busy(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::ResourceBusy,
            code: ErrorCode::TcpListenAddrBusy,
            message: message.into(),
            fatal: false,
        }
    }

    pub fn udp_bind_busy(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::ResourceBusy,
            code: ErrorCode::UdpBindAddrBusy,
            message: message.into(),
            fatal: false,
        }
    }

    pub fn invalid_address(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::InvalidArgument,
            code: ErrorCode::InvalidAddress,
            message: message.into(),
            fatal: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    pub open_ports: Vec<u16>,
}

pub async fn port_scan_loopback(
    host: &str,
    start_port: u16,
    end_port: u16,
    max_concurrency: usize,
    timeout_ms: u64,
) -> Result<ScanResult, DomainError> {
    if !is_loopback_single_host(host) {
        return Err(DomainError::invalid_argument(
            ErrorCode::ScanTargetNotAllowed,
            "port_scan target must be a loopback single host.",
            "Use 127.0.0.1 or ::1 for initial loopback scanning.",
        )
        .with_detail("field", serde_json::json!("host")));
    }
    if end_port < start_port {
        return Err(DomainError::invalid_argument(
            ErrorCode::InvalidRange,
            "port_scan end_port must be greater than or equal to start_port.",
            "Use a valid inclusive port range.",
        )
        .with_detail("field", serde_json::json!("end_port")));
    }
    let port_count = usize::from(end_port - start_port) + 1;
    if port_count > 256 || max_concurrency == 0 || max_concurrency > 64 {
        return Err(DomainError::new(
            ErrorCategory::BufferLimitExceeded,
            ErrorCode::ScanRangeTooLarge,
            "port_scan range or concurrency exceeds the M5 loopback limit.",
            "Reduce port range to at most 256 ports and concurrency to 1..64.",
            false,
        )
        .with_detail("limit", serde_json::json!(256))
        .with_detail("requested", serde_json::json!(port_count)));
    }

    let mut open_ports = Vec::new();
    for port in start_port..=end_port {
        let address = format!("{host}:{port}");
        if let Ok(Ok(stream)) = timeout(
            Duration::from_millis(timeout_ms),
            TcpStream::connect(address),
        )
        .await
        {
            drop(stream);
            open_ports.push(port);
        }
    }
    Ok(ScanResult { open_ports })
}

pub(crate) fn ensure_loopback_host(host: &str) -> Result<(), TransportError> {
    if is_loopback_single_host(host) {
        Ok(())
    } else {
        Err(TransportError::invalid_address(
            "network transport is restricted to loopback during M5",
        ))
    }
}

fn is_loopback_single_host(host: &str) -> bool {
    let trimmed = host.trim();
    trimmed
        .parse::<std::net::IpAddr>()
        .map(|address| address.is_loopback() && !matches!(trimmed, "0.0.0.0" | "::"))
        .unwrap_or(false)
}

pub(crate) fn map_tcp_connect_error(error: io::Error) -> TransportError {
    if matches!(error.kind(), io::ErrorKind::TimedOut) {
        TransportError::connect_timeout("tcp connect timed out")
    } else {
        TransportError::transport_closed("tcp connect failed")
    }
}

pub(crate) fn map_tcp_bind_error(error: io::Error) -> TransportError {
    if matches!(error.kind(), io::ErrorKind::AddrInUse) {
        TransportError::tcp_listen_busy("tcp listen address is already in use")
    } else {
        TransportError::write_failed(ErrorCode::WriteIoFailed, "tcp listen bind failed")
    }
}

pub(crate) fn map_udp_bind_error(error: io::Error) -> TransportError {
    if matches!(error.kind(), io::ErrorKind::AddrInUse) {
        TransportError::udp_bind_busy("udp bind address is already in use")
    } else {
        TransportError::write_failed(ErrorCode::WriteIoFailed, "udp bind failed")
    }
}

pub(crate) fn map_read_error(error: io::Error) -> TransportError {
    if matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        TransportError::read_timeout("transport read timed out")
    } else {
        TransportError::write_failed(ErrorCode::ReadIoFailed, "transport read failed")
    }
}

pub(crate) fn map_write_error(error: io::Error) -> TransportError {
    if matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
    ) {
        TransportError::transport_closed("transport peer closed")
    } else {
        TransportError::write_failed(ErrorCode::WriteIoFailed, "transport write failed")
    }
}
