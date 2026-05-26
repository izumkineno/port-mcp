#![allow(dead_code)]

use crate::{
    model::{DomainError, HandleId, InstanceSummary, InstanceType, SerialConfig, TcpConfig},
    runtime::RuntimeRegistry,
    transport::{ScanResult, port_scan_loopback},
};

pub struct InstanceService {
    registry: RuntimeRegistry,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::ErrorCode, transport::TcpListenTransport};

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

impl InstanceService {
    pub fn new_for_tests(date: &str) -> Self {
        Self {
            registry: RuntimeRegistry::new_for_tests(date),
        }
    }

    pub fn create(&mut self, instance_type: InstanceType) -> Result<InstanceSummary, DomainError> {
        self.registry.create_instance(instance_type)
    }

    pub fn list(&self) -> Vec<InstanceSummary> {
        self.registry.list_instances()
    }

    pub fn query(
        &self,
        handle_id: Option<&HandleId>,
        session_id: Option<&str>,
    ) -> Result<InstanceSummary, DomainError> {
        let handle_id = self.registry.resolve_handle(handle_id, session_id)?;
        self.registry.query_instance(&handle_id)
    }

    pub fn use_instance(
        &mut self,
        session_id: Option<&str>,
        handle_id: &HandleId,
    ) -> Result<Option<HandleId>, DomainError> {
        self.registry
            .use_instance(session_id, handle_id)
            .map(|binding| binding.previous_handle_id)
    }

    pub fn configure_serial(
        &mut self,
        handle_id: &HandleId,
        config: SerialConfig,
    ) -> Result<InstanceSummary, DomainError> {
        self.registry.configure_serial(handle_id, config)
    }

    pub fn configure_tcp(
        &mut self,
        handle_id: &HandleId,
        config: TcpConfig,
    ) -> Result<InstanceSummary, DomainError> {
        self.registry.configure_tcp(handle_id, config)
    }

    pub fn release(
        &mut self,
        handle_id: &HandleId,
        force: bool,
    ) -> Result<InstanceSummary, DomainError> {
        self.registry.release_instance(handle_id, force)
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
}
