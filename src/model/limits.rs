use serde::{Deserialize, Serialize};
use serde_json::json;

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
