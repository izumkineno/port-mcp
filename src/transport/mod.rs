#![allow(dead_code)]

use std::collections::VecDeque;

use crate::model::{ErrorCategory, ErrorCode};

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
}

#[derive(Debug, Default)]
pub struct MockTransport {
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
    next_write_error: Option<TransportError>,
    closed: bool,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inject_read(&mut self, bytes: &[u8]) {
        self.reads.push_back(bytes.to_vec());
    }

    pub fn read_chunk(&mut self, _max_bytes: usize) -> Result<Vec<u8>, TransportError> {
        if self.closed {
            return Err(TransportError::transport_closed("mock transport is closed"));
        }
        self.reads
            .pop_front()
            .ok_or_else(|| TransportError::read_timeout("mock transport has no injected reads"))
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<usize, TransportError> {
        if self.closed {
            return Err(TransportError::transport_closed("mock transport is closed"));
        }
        if let Some(error) = self.next_write_error.take() {
            return Err(error);
        }
        self.writes.push(bytes.to_vec());
        Ok(bytes.len())
    }

    pub fn writes(&self) -> &[Vec<u8>] {
        &self.writes
    }

    pub fn fail_next_write(&mut self, code: ErrorCode) {
        self.next_write_error = Some(TransportError::write_failed(code, "mock write failed"));
    }

    pub fn close(&mut self) -> Result<(), TransportError> {
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ErrorCode;

    #[test]
    fn unit_transport_common_maps_mock_errors_without_deciding_response_shape() {
        let timeout = TransportError::read_timeout("mock read timeout");
        assert_eq!(timeout.code, ErrorCode::ReadTimeout);
        assert!(!timeout.fatal);

        let closed = TransportError::transport_closed("mock closed");
        assert_eq!(closed.code, ErrorCode::TransportClosed);
        assert!(closed.fatal);
    }

    #[test]
    fn integration_mock_transport_injects_reads_observes_writes_and_failures() {
        let mut transport = MockTransport::new();
        transport.inject_read(b"pong");

        let read = transport.read_chunk(8).unwrap();
        assert_eq!(read, b"pong".to_vec());

        let written = transport.write_all(b"ping").unwrap();
        assert_eq!(written, 4);
        assert_eq!(transport.writes(), &[b"ping".to_vec()]);

        transport.fail_next_write(ErrorCode::WriteIoFailed);
        let failed = transport.write_all(b"boom").unwrap_err();
        assert_eq!(failed.code, ErrorCode::WriteIoFailed);

        transport.close().unwrap();
        let closed = transport.read_chunk(1).unwrap_err();
        assert_eq!(closed.code, ErrorCode::TransportClosed);
    }
}
