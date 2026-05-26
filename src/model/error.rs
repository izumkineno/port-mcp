use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ErrorId, Timestamp};

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
