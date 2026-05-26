use std::collections::VecDeque;

use crate::model::ErrorCode;

use super::TransportError;

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
