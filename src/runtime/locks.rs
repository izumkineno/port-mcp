use serde_json::json;

use crate::model::{DomainError, ErrorCategory, ErrorCode, HandleId};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceKey(String);

impl ResourceKey {
    pub fn serial(port: &str) -> Self {
        Self(format!("serial:{}", port.trim().to_ascii_uppercase()))
    }

    pub fn tcp_listen(host: &str, port: u16) -> Self {
        Self(format!("tcp-listen:{}:{port}", normalize_host(host)))
    }

    pub fn udp_bind(host: &str, port: u16) -> Self {
        Self(format!("udp-bind:{}:{port}", normalize_host(host)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLockState {
    Held,
    Closing,
    Stale,
}

#[derive(Debug, Clone)]
pub(crate) struct ResourceLockEntry {
    pub(crate) owner_handle_id: HandleId,
    pub(crate) state: ResourceLockState,
    pub(crate) generation: u64,
    pub(crate) stale_close: bool,
}

impl ResourceLockEntry {
    pub(crate) fn held(owner_handle_id: HandleId) -> Self {
        Self {
            owner_handle_id,
            state: ResourceLockState::Held,
            generation: 1,
            stale_close: false,
        }
    }
}

pub(crate) fn normalize_host(host: &str) -> String {
    let trimmed = host.trim().to_ascii_lowercase();
    if let Some(ipv4) = normalize_ipv4_with_leading_zeroes(&trimmed) {
        return ipv4;
    }
    if let Ok(address) = trimmed.parse::<std::net::IpAddr>() {
        address.to_string()
    } else {
        trimmed
    }
}

fn normalize_ipv4_with_leading_zeroes(host: &str) -> Option<String> {
    let parts = host.split('.').collect::<Vec<_>>();
    if parts.len() != 4 {
        return None;
    }

    let octets = parts
        .iter()
        .map(|part| part.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some(format!(
        "{}.{}.{}.{}",
        octets[0], octets[1], octets[2], octets[3]
    ))
}

pub(crate) fn resource_lock_error(key: &ResourceKey, entry: &ResourceLockEntry) -> DomainError {
    let code = match entry.state {
        ResourceLockState::Held => resource_busy_code(key),
        ResourceLockState::Closing => ErrorCode::ResourceClosing,
        ResourceLockState::Stale => ErrorCode::ResourceLockStale,
    };
    let message = match entry.state {
        ResourceLockState::Held => "Resource is already owned by another instance.",
        ResourceLockState::Closing => "Resource is still closing from a released instance.",
        ResourceLockState::Stale => "Resource lock is stale and requires operator attention.",
    };
    DomainError::new(
        ErrorCategory::ResourceBusy,
        code,
        message,
        "Release the owning instance, wait for closing to complete, or choose another resource.",
        matches!(entry.state, ResourceLockState::Closing),
    )
    .with_detail(
        "resource",
        json!({
            "kind": resource_kind(key),
            "display": key.as_str()
        }),
    )
    .with_detail("owner_handle_id", json!(entry.owner_handle_id))
}

fn resource_busy_code(key: &ResourceKey) -> ErrorCode {
    if key.as_str().starts_with("serial:") {
        ErrorCode::SerialPortBusy
    } else if key.as_str().starts_with("tcp-listen:") {
        ErrorCode::TcpListenAddrBusy
    } else {
        ErrorCode::UdpBindAddrBusy
    }
}

fn resource_kind(key: &ResourceKey) -> &'static str {
    if key.as_str().starts_with("serial:") {
        "serial"
    } else if key.as_str().starts_with("tcp-listen:") {
        "tcp-listen"
    } else {
        "udp-bind"
    }
}
