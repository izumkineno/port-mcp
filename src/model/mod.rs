#![allow(dead_code)]

mod config;
mod data;
mod error;
mod ids;
mod limits;
mod redaction;
mod response;
mod state;

#[allow(unused_imports)]
pub use config::{
    ConfigSnapshot, DataBits, FlowControl, InstanceStats, InstanceSummary, Parity, PayloadEncoding,
    PeerSummary, ResourceSummary, SerialConfig, StopBits, TcpConfig, TcpMode, UdpConfig,
    VisaConfig, validate_instance_type, validate_required_field, validate_tcp_port,
};
#[allow(unused_imports)]
pub use data::{Payload, PayloadSummary};
#[allow(unused_imports)]
pub use error::{DomainError, ErrorCategory, ErrorCode, ErrorDetails, LastErrorSummary};
#[allow(unused_imports)]
pub use ids::{ErrorId, HandleId, IdGenerator, RequestId, Timestamp};
#[allow(unused_imports)]
pub use limits::RuntimeLimits;
#[allow(unused_imports)]
pub use redaction::Redactor;
#[allow(unused_imports)]
pub use response::{FailureResponse, SuccessResponse, ToolResponse, Warning};
#[allow(unused_imports)]
pub use state::{InstanceState, InstanceType};

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn unit_state_model_serializes_documented_names_and_summary_shape() {
        assert_eq!(json!(InstanceType::Serial), "Serial");
        assert_eq!(json!(InstanceType::Tcp), "TCP");
        assert_eq!(json!(InstanceType::Udp), "UDP");
        assert_eq!(json!(InstanceType::Visa), "Visa");

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
            peers: None,
            last_error: None,
        };

        let value = serde_json::to_value(summary).unwrap();
        assert_eq!(value["handle_id"], "h_tcp_007");
        assert_eq!(value["type"], "TCP");
        assert_eq!(value["state"], "Configured");
        assert_eq!(
            value["resource"],
            json!({ "kind": "tcp-client", "display": "127.0.0.1:9000" })
        );
        assert_eq!(value["stats"]["rx_buffer_bytes"], 0);
        assert_eq!(
            serde_json::to_value(ResourceSummary::serial("COM3")).unwrap(),
            json!({
                "kind": "serial",
                "display": "COM3"
            })
        );
        assert_eq!(
            serde_json::to_value(ResourceSummary::visa(
                "TCPIP0::192.168.1.10::INSTR",
                Some("tcpip")
            ))
            .unwrap(),
            json!({
                "kind": "visa",
                "display": "TCPIP0::192.168.1.10::INSTR",
                "resource_class": "tcpip"
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
            HandleId::new_for_type(InstanceType::Visa, 1).as_str(),
            "h_visa_001"
        );
        assert_eq!(
            TcpConfig::client("0.0.0.0", 9000)
                .validate_remote(&RuntimeLimits::default())
                .unwrap_err()
                .code,
            ErrorCode::ScanTargetNotAllowed
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
            (
                ErrorCategory::InvalidArgument,
                ErrorCode::ProtocolFrameInvalid,
            ),
            (
                ErrorCategory::InvalidArgument,
                ErrorCode::ProtocolChecksumFailed,
            ),
            (ErrorCategory::ResourceBusy, ErrorCode::SerialPortBusy),
            (ErrorCategory::ResourceBusy, ErrorCode::VisaResourceBusy),
            (ErrorCategory::ResourceBusy, ErrorCode::TcpListenAddrBusy),
            (ErrorCategory::ResourceBusy, ErrorCode::UdpBindAddrBusy),
            (ErrorCategory::ResourceBusy, ErrorCode::ResourceClosing),
            (ErrorCategory::ResourceBusy, ErrorCode::ResourceLockStale),
            (ErrorCategory::ConnectTimeout, ErrorCode::ConnectTimeout),
            (ErrorCategory::ConnectTimeout, ErrorCode::SerialOpenTimeout),
            (ErrorCategory::ConnectTimeout, ErrorCode::VisaOpenTimeout),
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
            (ErrorCategory::InvalidState, ErrorCode::FeatureNotCompiled),
            (
                ErrorCategory::InvalidState,
                ErrorCode::VisaRuntimeUnavailable,
            ),
            (ErrorCategory::WriteFailed, ErrorCode::VisaEnumFailed),
            (ErrorCategory::WriteFailed, ErrorCode::VisaOpenFailed),
            (ErrorCategory::WriteFailed, ErrorCode::VisaWriteFailed),
            (ErrorCategory::WriteFailed, ErrorCode::VisaReadFailed),
            (ErrorCategory::WriteFailed, ErrorCode::VisaQueryIdnFailed),
            (
                ErrorCategory::InvalidArgument,
                ErrorCode::VisaResourceNotFound,
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
        let redactor = Redactor;
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
        assert_eq!(limits.io_timeout_max_ms, 30_000);
        assert_eq!(limits.max_instances, 64);
        assert_eq!(limits.max_subscribers_per_instance, 32);
        assert_eq!(limits.max_total_buffer_bytes, 128 * 1024 * 1024);
        assert_eq!(limits.max_total_queued_bytes, 64 * 1024 * 1024);
        assert_eq!(limits.force_close_deadline_ms, 5_000);
        assert!(!limits.allow_non_loopback_network);
        assert!(limits.network_allowed_hosts.is_empty());

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
        assert_eq!(limits.validate_io_timeout_ms("timeout_ms", 1).unwrap(), 1);
        assert_eq!(
            limits
                .validate_io_timeout_ms("timeout_ms", limits.io_timeout_max_ms + 1)
                .unwrap_err()
                .code,
            ErrorCode::InvalidRange
        );
        assert!(
            limits
                .validate_tx_frame_len(limits.tx_frame_max_bytes)
                .is_ok()
        );
        assert_eq!(
            limits
                .validate_tx_frame_len(limits.tx_frame_max_bytes + 1)
                .unwrap_err()
                .code,
            ErrorCode::TxFrameTooLarge
        );
        assert!(limits.validate_network_host("host", "127.0.0.1").is_ok());
        assert_eq!(
            limits
                .validate_network_host("host", "192.0.2.1")
                .unwrap_err()
                .code,
            ErrorCode::ScanTargetNotAllowed
        );

        let mut allowlisted = RuntimeLimits::default();
        allowlisted
            .network_allowed_hosts
            .push("192.0.2.1".to_owned());
        assert!(
            allowlisted
                .validate_network_host("host", "192.0.2.1")
                .is_ok()
        );

        assert_eq!(
            VisaConfig::new("TCPIP0::192.0.2.1::INSTR")
                .validate(&RuntimeLimits::default())
                .unwrap_err()
                .code,
            ErrorCode::ScanTargetNotAllowed
        );
        assert!(
            VisaConfig::new("TCPIP0::192.0.2.1::INSTR")
                .validate(&allowlisted)
                .is_ok()
        );
        assert!(VisaConfig::new("ASRL3::INSTR").validate(&limits).is_ok());

        let serialized: Value = serde_json::to_value(limits).unwrap();
        assert_eq!(
            serialized["scan_allowed_hosts"],
            json!(["127.0.0.0/8", "::1"])
        );
        assert_eq!(serialized["io_timeout_max_ms"], json!(30_000));
        assert_eq!(serialized["allow_non_loopback_network"], json!(false));
    }
}
