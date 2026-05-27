use crate::{
    model::{
        ConfigSnapshot, DomainError, ErrorCategory, ErrorCode, HandleId, InstanceSummary,
        InstanceType, Payload, PayloadEncoding, PayloadSummary, RuntimeLimits,
    },
    runtime::{ClearResult, ClearTarget, PullResult, SendResult},
    transport::{
        ScanResult, SerialPortSummary, SerialWorker, TransportError, port_scan_loopback,
        scan_serial_ports,
    },
};

use super::InstanceService;

impl InstanceService {
    pub fn connect(&mut self, handle_id: &HandleId) -> Result<InstanceSummary, DomainError> {
        let summary = self.registry.query_instance(handle_id)?;
        if summary.instance_type != InstanceType::Serial {
            return self.registry.connect_mock(handle_id);
        }

        let config = match summary.config {
            Some(ConfigSnapshot::Serial(config)) => config,
            _ => return self.registry.connect_mock(handle_id),
        };
        if !self.serial_workers.contains_key(handle_id.as_str()) {
            let worker = SerialWorker::open(&config).map_err(transport_error_to_domain)?;
            if let Err(error) = self.registry.connect_mock(handle_id) {
                let _ = worker.close(config.timeout_ms);
                return Err(error);
            }
            self.serial_workers
                .insert(handle_id.as_str().to_owned(), worker);
            return self.registry.query_instance(handle_id);
        }
        self.registry.connect_mock(handle_id)
    }

    pub fn disconnect(&mut self, handle_id: &HandleId) -> Result<InstanceSummary, DomainError> {
        self.close_serial_worker(handle_id);
        self.registry.disconnect_mock(handle_id)
    }

    pub fn send(
        &mut self,
        handle_id: &HandleId,
        payload: &Payload,
    ) -> Result<SendResult, DomainError> {
        if let Some(worker) = self.serial_workers.get(handle_id.as_str()) {
            self.registry.ensure_connected(handle_id, "port_send")?;
            let written = worker
                .write(&payload.bytes, serial_timeout_ms(&self.registry, handle_id)?)
                .map_err(transport_error_to_domain)?;
            self.registry.record_direct_tx(handle_id, written)?;
            return Ok(SendResult {
                queued: false,
                sent_bytes: written,
            });
        }
        self.registry.port_send_mock(handle_id, &payload.bytes)
    }

    pub fn pull(
        &mut self,
        handle_id: &HandleId,
        max_bytes: Option<usize>,
    ) -> Result<PullResult, DomainError> {
        if let Some(worker) = self.serial_workers.get(handle_id.as_str()) {
            self.registry.ensure_connected(handle_id, "port_pull")?;
            let max_bytes = match max_bytes {
                Some(value) => self.registry.validate_pull_max_bytes(value)?,
                None => self.registry.default_pull_max_bytes(),
            };
            let bytes = worker
                .read(max_bytes, serial_timeout_ms(&self.registry, handle_id)?)
                .map_err(transport_error_to_domain)?;
            self.registry.record_direct_rx(handle_id, bytes.len())?;
            return Ok(PullResult {
                bytes,
                truncated: false,
                remaining_rx_buffer_bytes: 0,
            });
        }
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

fn serial_timeout_ms(
    registry: &crate::runtime::RuntimeRegistry,
    handle_id: &HandleId,
) -> Result<u64, DomainError> {
    let summary = registry.query_instance(handle_id)?;
    match summary.config {
        Some(ConfigSnapshot::Serial(config)) => Ok(config.timeout_ms),
        _ => Ok(1_000),
    }
}

fn transport_error_to_domain(error: TransportError) -> DomainError {
    DomainError::new(
        error.category,
        error.code,
        error.message,
        match error.category {
            ErrorCategory::ReadTimeout => {
                "Retry, increase timeout_ms, or check that the peer sent data."
            }
            ErrorCategory::WriteFailed => "Check the serial link and retry the write.",
            ErrorCategory::ResourceBusy => "Close other programs using this serial port, then retry.",
            ErrorCategory::ConnectTimeout => "Check the serial device and timeout_ms, then retry.",
            _ => "Check the serial device state, then retry.",
        },
        error.fatal,
    )
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
    use crate::{
        model::{InstanceType, SerialConfig},
        transport::{TcpListenTransport, serial_worker_for_tests},
    };

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

    #[test]
    fn unit_serial_send_and_pull_use_attached_worker() {
        let mut service = InstanceService::new_for_tests("20260526");
        let created = service.create(InstanceType::Serial).unwrap();
        service
            .registry
            .configure_serial(&created.handle_id, SerialConfig::new("COM_TEST"))
            .unwrap();
        service.attach_serial_worker_for_tests(
            &created.handle_id,
            serial_worker_for_tests(vec![b"<01004580000>".to_vec()]),
        );

        service.connect(&created.handle_id).unwrap();
        let sent = service
            .send(&created.handle_id, &Payload::from_text("<01004580000>", false).unwrap())
            .unwrap();
        assert_eq!(sent.sent_bytes, 13);
        assert!(!sent.queued);

        let pulled = service.pull(&created.handle_id, Some(64)).unwrap();
        assert_eq!(pulled.bytes, b"<01004580000>".to_vec());
        assert!(!pulled.truncated);

        let summary = service.query(Some(&created.handle_id), None).unwrap();
        assert_eq!(summary.stats.tx_bytes, 13);
        assert_eq!(summary.stats.rx_bytes, 13);
        assert_eq!(summary.stats.tx_queue_items, 0);
    }
}
