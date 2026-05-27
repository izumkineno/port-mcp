use crate::{
    model::{
        DomainError, ErrorCode, HandleId, InstanceSummary, Payload, PayloadEncoding,
        PayloadSummary, RuntimeLimits,
    },
    runtime::{ClearResult, ClearTarget, PullResult, SendResult},
    transport::{ScanResult, SerialPortSummary, port_scan_loopback, scan_serial_ports},
};

use super::InstanceService;

impl InstanceService {
    pub fn connect(&mut self, handle_id: &HandleId) -> Result<InstanceSummary, DomainError> {
        self.registry.connect_mock(handle_id)
    }

    pub fn disconnect(&mut self, handle_id: &HandleId) -> Result<InstanceSummary, DomainError> {
        self.registry.disconnect_mock(handle_id)
    }

    pub fn send(
        &mut self,
        handle_id: &HandleId,
        payload: &Payload,
    ) -> Result<SendResult, DomainError> {
        self.registry.port_send_mock(handle_id, &payload.bytes)
    }

    pub fn pull(
        &mut self,
        handle_id: &HandleId,
        max_bytes: Option<usize>,
    ) -> Result<PullResult, DomainError> {
        match self.registry.port_pull_mock(handle_id, max_bytes) {
            Ok(result) => Ok(result),
            Err(error) if error.code == ErrorCode::ReadTimeout => Ok(PullResult {
                truncated: false,
                remaining_rx_buffer_bytes: 0,
                bytes: Vec::new(),
            }),
            Err(error) => Err(error),
        }
    }

    pub fn clear(
        &mut self,
        handle_id: &HandleId,
        target: ClearTarget,
    ) -> Result<ClearResult, DomainError> {
        self.registry.port_clear_mock(handle_id, target)
    }
}

pub struct PortService;

impl PortService {
    pub fn new_for_tests(_date: &str) -> Self {
        Self
    }

    pub async fn scan_loopback(
        &self,
        host: &str,
        start_port: u16,
        end_port: u16,
        max_concurrency: usize,
        timeout_ms: u64,
    ) -> Result<ScanResult, DomainError> {
        port_scan_loopback(host, start_port, end_port, max_concurrency, timeout_ms).await
    }

    pub fn scan_serial(&self) -> Result<Vec<SerialPortSummary>, DomainError> {
        scan_serial_ports().map_err(|error| {
            DomainError::new(
                error.category,
                error.code,
                error.message,
                "Check serial device permissions and driver state, then retry port_scan.",
                false,
            )
        })
    }

    pub fn summarize_payload(bytes: &[u8], encoding: PayloadEncoding) -> PayloadSummary {
        PayloadSummary::from_bytes(
            bytes,
            encoding,
            RuntimeLimits::default().pull_default_max_bytes,
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TcpListenTransport;

    #[tokio::test]
    async fn unit_port_service_scans_loopback_and_preserves_scan_errors() {
        let listener = TcpListenTransport::bind("127.0.0.1", 0).await.unwrap();
        let open_port = listener.local_addr().port();
        let service = PortService::new_for_tests("20260526");

        let result = service
            .scan_loopback("127.0.0.1", open_port, open_port, 4, 1_000)
            .await
            .unwrap();
        assert_eq!(result.open_ports, vec![open_port]);

        let error = service
            .scan_loopback("0.0.0.0", open_port, open_port, 4, 1_000)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ScanTargetNotAllowed);
        listener.close().await.unwrap();
    }
}
