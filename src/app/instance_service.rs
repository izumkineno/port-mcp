use std::collections::HashMap;

use crate::{
    model::{ConfigSnapshot, DomainError, HandleId, InstanceSummary, InstanceType, TcpMode},
    runtime::{ResourceKey, RuntimeRegistry},
    transport::{
        SerialWorker, TcpClientWorker, TcpListenWorker, TransportError, UdpWorker, VisaWorker,
        resolve_udp_peer,
    },
};

pub struct InstanceService {
    pub(crate) registry: RuntimeRegistry,
    pub(crate) serial_workers: HashMap<String, SerialWorker>,
    pub(crate) network_workers: HashMap<String, NetworkWorker>,
    pub(crate) visa_workers: HashMap<String, VisaWorker>,
}

#[derive(Debug)]
pub(crate) enum NetworkWorker {
    TcpClient(TcpClientWorker),
    TcpListen(TcpListenWorker),
    Udp(UdpWorker),
}

impl NetworkWorker {
    pub(crate) fn close(&self) {
        let _ = match self {
            Self::TcpClient(worker) => worker.close(),
            Self::TcpListen(worker) => worker.close(),
            Self::Udp(worker) => worker.close(),
        };
    }

    pub(crate) fn tcp_write(
        &self,
        bytes: &[u8],
        config: &crate::model::TcpConfig,
    ) -> Result<usize, TransportError> {
        match (self, config.mode) {
            (Self::TcpClient(worker), TcpMode::Client) => worker.write(bytes),
            (Self::TcpListen(worker), TcpMode::Listen) => worker.write(bytes),
            _ => Err(TransportError::transport_closed(
                "tcp worker does not match the configured mode",
            )),
        }
    }

    pub(crate) fn tcp_read(
        &self,
        max_bytes: usize,
        config: &crate::model::TcpConfig,
    ) -> Result<Vec<u8>, TransportError> {
        match (self, config.mode) {
            (Self::TcpClient(worker), TcpMode::Client) => worker.read(max_bytes),
            (Self::TcpListen(worker), TcpMode::Listen) => worker.read(max_bytes),
            _ => Err(TransportError::transport_closed(
                "tcp worker does not match the configured mode",
            )),
        }
    }

    pub(crate) fn udp_send(
        &self,
        bytes: &[u8],
        config: &crate::model::UdpConfig,
    ) -> Result<usize, TransportError> {
        match self {
            Self::Udp(worker) => {
                let peer = udp_remote_addr(config)?;
                worker.send_to(bytes, peer)
            }
            _ => Err(TransportError::transport_closed(
                "udp worker does not match the configured instance",
            )),
        }
    }

    pub(crate) fn udp_recv(
        &self,
        max_bytes: usize,
        _config: &crate::model::UdpConfig,
    ) -> Result<Vec<u8>, TransportError> {
        match self {
            Self::Udp(worker) => worker.recv(max_bytes),
            _ => Err(TransportError::transport_closed(
                "udp worker does not match the configured instance",
            )),
        }
    }
}

fn udp_remote_addr(
    config: &crate::model::UdpConfig,
) -> Result<std::net::SocketAddr, TransportError> {
    let remote_host = config.remote_host.as_deref().ok_or_else(|| {
        TransportError::invalid_address("udp remote_host is required for sending")
    })?;
    let remote_port = config.remote_port.ok_or_else(|| {
        TransportError::invalid_address("udp remote_port is required for sending")
    })?;
    resolve_udp_peer(remote_host, remote_port)
}

fn resource_key_for_summary(summary: &InstanceSummary) -> Option<ResourceKey> {
    match summary.config.as_ref()? {
        ConfigSnapshot::Tcp(config) => match config.mode {
            TcpMode::Client => None,
            TcpMode::Listen => Some(ResourceKey::tcp_listen(&config.host, config.port)),
        },
        ConfigSnapshot::Udp(config) => {
            Some(ResourceKey::udp_bind(&config.bind_host, config.bind_port))
        }
        ConfigSnapshot::Serial(_) => None,
        ConfigSnapshot::Visa(config) => Some(ResourceKey::visa(&config.resource_address)),
    }
}

impl InstanceService {
    pub fn new_for_tests(date: &str) -> Self {
        Self {
            registry: RuntimeRegistry::new_for_tests(date),
            serial_workers: HashMap::new(),
            network_workers: HashMap::new(),
            visa_workers: HashMap::new(),
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

    pub fn release(
        &mut self,
        handle_id: &HandleId,
        force: bool,
    ) -> Result<InstanceSummary, DomainError> {
        let summary = self.registry.release_instance(handle_id, force)?;
        self.close_serial_worker(handle_id);
        self.close_network_worker(handle_id);
        self.close_visa_worker(handle_id);
        if force {
            if let Some(resource_key) = resource_key_for_summary(&summary) {
                let _ = self.registry.complete_mock_background_close(&resource_key);
            }
        }
        Ok(summary)
    }

    pub(crate) fn close_serial_worker(&mut self, handle_id: &HandleId) {
        if let Some(worker) = self.serial_workers.remove(handle_id.as_str()) {
            let _ = worker.close(1_000);
        }
    }

    pub(crate) fn close_network_worker(&mut self, handle_id: &HandleId) {
        if let Some(worker) = self.network_workers.remove(handle_id.as_str()) {
            worker.close();
        }
    }

    pub(crate) fn close_visa_worker(&mut self, handle_id: &HandleId) {
        if let Some(worker) = self.visa_workers.remove(handle_id.as_str()) {
            let _ = worker.close();
        }
    }

    #[cfg(test)]
    pub(crate) fn attach_serial_worker_for_tests(
        &mut self,
        handle_id: &HandleId,
        worker: SerialWorker,
    ) {
        self.serial_workers
            .insert(handle_id.as_str().to_owned(), worker);
    }

    #[cfg(test)]
    pub(crate) fn udp_remote_addr_for_tests(
        &self,
        config: &crate::model::UdpConfig,
    ) -> Result<std::net::SocketAddr, TransportError> {
        udp_remote_addr(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TcpConfig, UdpConfig};

    #[test]
    fn unit_udp_remote_addr_accepts_non_loopback_host_strings() {
        let service = InstanceService::new_for_tests("20260526");
        let result = service.udp_remote_addr_for_tests(&UdpConfig {
            bind_host: "192.0.2.10".to_owned(),
            bind_port: 9001,
            remote_host: Some("192.0.2.1".to_owned()),
            remote_port: Some(9002),
            timeout_ms: 1_000,
        });

        assert!(result.is_ok());
    }

    #[test]
    fn unit_tcp_config_validation_no_longer_rejects_wildcard_hosts() {
        assert!(TcpConfig::client("0.0.0.0", 9000).validate_remote().is_ok());
        assert!(TcpConfig::client("::", 9000).validate_remote().is_ok());
        assert!(TcpConfig::client("", 9000).validate_remote().is_ok());
    }
}
