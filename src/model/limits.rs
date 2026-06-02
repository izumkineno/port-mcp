use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::IpAddr;

use super::{DomainError, ErrorCategory, ErrorCode};

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
    pub io_timeout_max_ms: u64,
    pub max_instances: usize,
    pub max_subscribers_per_instance: usize,
    pub max_total_buffer_bytes: usize,
    pub max_total_queued_bytes: usize,
    pub force_close_deadline_ms: u64,
    pub scan_allowed_hosts: Vec<String>,
    pub allow_non_loopback_network: bool,
    pub network_allowed_hosts: Vec<String>,
}

impl RuntimeLimits {
    pub const ABS_MAX_TOTAL_BUFFER_BYTES: usize = 512 * 1024 * 1024;
    pub const ABS_MAX_TOTAL_QUEUED_BYTES: usize = 256 * 1024 * 1024;
    pub const HELPER_MAX_INPUT_BYTES: usize = 64 * 1024;

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

    pub fn validate_io_timeout_ms(&self, field: &str, value: u64) -> Result<u64, DomainError> {
        if (1..=self.io_timeout_max_ms).contains(&value) {
            Ok(value)
        } else {
            Err(DomainError::invalid_argument(
                ErrorCode::InvalidRange,
                format!("{field} is outside the allowed range."),
                format!(
                    "Use a value between 1 and {} milliseconds.",
                    self.io_timeout_max_ms
                ),
            )
            .with_detail("field", json!(field))
            .with_detail("min", json!(1))
            .with_detail("max", json!(self.io_timeout_max_ms))
            .with_detail("actual", json!(value)))
        }
    }

    pub fn validate_tx_frame_len(&self, len: usize) -> Result<(), DomainError> {
        if len <= self.tx_frame_max_bytes {
            Ok(())
        } else {
            Err(DomainError::new(
                ErrorCategory::BufferLimitExceeded,
                ErrorCode::TxFrameTooLarge,
                "TX frame exceeds tx_frame_max_bytes.",
                "Reduce payload size and retry.",
                true,
            )
            .with_detail("limit", json!(self.tx_frame_max_bytes))
            .with_detail("requested", json!(len)))
        }
    }

    pub fn validate_network_host(&self, field: &str, host: &str) -> Result<(), DomainError> {
        let trimmed = host.trim();
        let ip: IpAddr = trimmed.parse().map_err(|_| {
            DomainError::invalid_argument(
                ErrorCode::InvalidAddress,
                format!("{field} must be an IP address allowed by the network boundary."),
                "Use a loopback IP address or configure an explicit allowlist entry.",
            )
            .with_detail("field", json!(field))
            .with_detail("host", json!(host))
        })?;

        if is_dangerous_network_address(ip) {
            return Err(network_boundary_error(field, host));
        }

        if ip.is_loopback() || self.allow_non_loopback_network || self.host_is_allowed(trimmed) {
            Ok(())
        } else {
            Err(network_boundary_error(field, host))
        }
    }

    fn host_is_allowed(&self, host: &str) -> bool {
        self.network_allowed_hosts
            .iter()
            .any(|allowed| allowed == host)
    }
}

fn is_dangerous_network_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(address) => {
            address.is_unspecified()
                || address.is_broadcast()
                || address.is_multicast()
                || address.is_link_local()
        }
        IpAddr::V6(address) => {
            address.is_unspecified() || address.is_multicast() || address.is_unicast_link_local()
        }
    }
}

fn network_boundary_error(field: &str, host: &str) -> DomainError {
    DomainError::invalid_argument(
        ErrorCode::ScanTargetNotAllowed,
        format!("{field} is outside the allowed network boundary."),
        "Use a loopback IP address or configure an explicit allowlist entry.",
    )
    .with_detail("field", json!(field))
    .with_detail("host", json!(host))
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
            io_timeout_max_ms: 30_000,
            max_instances: 64,
            max_subscribers_per_instance: 32,
            max_total_buffer_bytes: 128 * 1024 * 1024,
            max_total_queued_bytes: 64 * 1024 * 1024,
            force_close_deadline_ms: 5_000,
            scan_allowed_hosts: vec!["127.0.0.0/8".to_owned(), "::1".to_owned()],
            allow_non_loopback_network: false,
            network_allowed_hosts: Vec::new(),
        }
    }
}
