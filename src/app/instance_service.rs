use std::collections::HashMap;

use crate::{
    model::{
        ConfigSnapshot, DomainError, HandleId, InstanceSummary, InstanceType, PeerSummary,
        RuntimeLimits, TcpMode,
    },
    runtime::{
        PullResult, PullSource, ResourceKey, RuntimeRegistry, SendResult, SendTargetMode,
        SendTargetSummary,
    },
    transport::{
        SerialWorker, TcpClientWorker, TcpListenWorker, TcpListenWriteMode, TransportError,
        UdpWorker, VisaWorker, resolve_udp_peer,
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
        peer_id: Option<&str>,
    ) -> Result<SendResult, TransportError> {
        match (self, config.mode) {
            (Self::TcpClient(worker), TcpMode::Client) => {
                if peer_id.is_some() {
                    return Err(TransportError::transport_closed(
                        "peer_id is only supported for tcp listen instances",
                    ));
                }
                worker.write(bytes).map(|written| SendResult {
                    queued: false,
                    sent_bytes: written,
                    target: None,
                })
            }
            (Self::TcpListen(worker), TcpMode::Listen) => {
                worker.write(peer_id, bytes).map(|result| {
                    let mode = match result.mode {
                        TcpListenWriteMode::Peer => SendTargetMode::Peer,
                        TcpListenWriteMode::Broadcast => SendTargetMode::Broadcast,
                    };
                    SendResult {
                        queued: false,
                        sent_bytes: result.sent_bytes,
                        target: Some(SendTargetSummary {
                            mode,
                            peer_id: result.peer_id,
                            peer_count: result.successful_peer_ids.len(),
                            successful_peer_ids: result.successful_peer_ids,
                            failed_peer_count: result.failed_peer_count,
                        }),
                    }
                })
            }
            _ => Err(TransportError::transport_closed(
                "tcp worker does not match the configured mode",
            )),
        }
    }

    pub(crate) fn tcp_read(
        &self,
        max_bytes: usize,
        config: &crate::model::TcpConfig,
        peer_id: Option<&str>,
    ) -> Result<PullResult, TransportError> {
        match (self, config.mode) {
            (Self::TcpClient(worker), TcpMode::Client) => {
                if peer_id.is_some() {
                    return Err(TransportError::transport_closed(
                        "peer_id is only supported for tcp listen instances",
                    ));
                }
                worker.read(max_bytes).map(|bytes| PullResult {
                    bytes,
                    truncated: false,
                    remaining_rx_buffer_bytes: 0,
                    source: None,
                })
            }
            (Self::TcpListen(worker), TcpMode::Listen) => {
                worker.read(peer_id, max_bytes).map(|result| PullResult {
                    bytes: result.bytes,
                    truncated: false,
                    remaining_rx_buffer_bytes: result.remaining_frames,
                    source: Some(PullSource {
                        transport: "tcp-listen".to_owned(),
                        peer_id: result.peer_id,
                        remote_addr: result.remote_addr,
                    }),
                })
            }
            _ => Err(TransportError::transport_closed(
                "tcp worker does not match the configured mode",
            )),
        }
    }

    pub(crate) fn tcp_listen_peers(&self) -> Result<Vec<PeerSummary>, TransportError> {
        match self {
            Self::TcpListen(worker) => worker.list_peers().map(|peers| {
                peers
                    .into_iter()
                    .map(|peer| PeerSummary {
                        peer_id: peer.peer_id,
                        remote_addr: peer.remote_addr,
                    })
                    .collect()
            }),
            _ => Ok(Vec::new()),
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
        Self::new_for_tests_with_limits(date, RuntimeLimits::default())
    }

    pub fn new_for_tests_with_limits(date: &str, limits: RuntimeLimits) -> Self {
        Self {
            registry: RuntimeRegistry::new_for_tests_with_limits(date, limits),
            serial_workers: HashMap::new(),
            network_workers: HashMap::new(),
            visa_workers: HashMap::new(),
        }
    }

    pub fn create(&mut self, instance_type: InstanceType) -> Result<InstanceSummary, DomainError> {
        self.registry.create_instance(instance_type)
    }

    pub fn list(&self) -> Vec<InstanceSummary> {
        self.registry
            .list_instances()
            .into_iter()
            .map(|summary| self.enrich_summary(summary))
            .collect()
    }

    pub fn query(
        &self,
        handle_id: Option<&HandleId>,
        session_id: Option<&str>,
    ) -> Result<InstanceSummary, DomainError> {
        let handle_id = self.registry.resolve_handle(handle_id, session_id)?;
        self.registry
            .query_instance(&handle_id)
            .map(|summary| self.enrich_summary(summary))
    }

    fn enrich_summary(&self, mut summary: InstanceSummary) -> InstanceSummary {
        if !matches!(summary.config, Some(ConfigSnapshot::Tcp(ref config)) if config.mode == TcpMode::Listen)
        {
            return summary;
        }
        if let Some(worker) = self.network_workers.get(summary.handle_id.as_str()) {
            if let Ok(peers) = worker.tcp_listen_peers() {
                summary.peers = Some(peers);
            }
        }
        summary
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
    use crate::model::{ErrorCode, RuntimeLimits, TcpConfig, UdpConfig};

    #[test]
    fn unit_udp_config_default_boundary_rejects_non_loopback_remote() {
        let limits = RuntimeLimits::default();
        let config = UdpConfig {
            bind_host: "127.0.0.1".to_owned(),
            bind_port: 9001,
            remote_host: Some("192.0.2.1".to_owned()),
            remote_port: Some(9002),
            timeout_ms: 1_000,
        };

        assert_eq!(
            config.validate_remote(&limits).unwrap_err().code,
            ErrorCode::ScanTargetNotAllowed
        );
    }

    #[test]
    fn unit_udp_remote_addr_resolves_explicitly_allowlisted_non_loopback_host_strings() {
        let service = InstanceService::new_for_tests("20260526");
        let mut limits = RuntimeLimits::default();
        limits.network_allowed_hosts.push("192.0.2.1".to_owned());
        let config = UdpConfig {
            bind_host: "127.0.0.1".to_owned(),
            bind_port: 9001,
            remote_host: Some("192.0.2.1".to_owned()),
            remote_port: Some(9002),
            timeout_ms: 1_000,
        };

        config.validate_remote(&limits).unwrap();
        let result = service.udp_remote_addr_for_tests(&UdpConfig {
            bind_host: "127.0.0.1".to_owned(),
            bind_port: 9001,
            remote_host: Some("192.0.2.1".to_owned()),
            remote_port: Some(9002),
            timeout_ms: 1_000,
        });

        assert!(result.is_ok());
    }

    #[test]
    fn unit_tcp_config_default_boundary_rejects_wildcard_hosts() {
        let limits = RuntimeLimits::default();
        assert_eq!(
            TcpConfig::client("0.0.0.0", 9000)
                .validate_remote(&limits)
                .unwrap_err()
                .code,
            ErrorCode::ScanTargetNotAllowed
        );
        assert_eq!(
            TcpConfig::client("::", 9000)
                .validate_remote(&limits)
                .unwrap_err()
                .code,
            ErrorCode::ScanTargetNotAllowed
        );
        assert_eq!(
            TcpConfig::client("", 9000)
                .validate_remote(&limits)
                .unwrap_err()
                .code,
            ErrorCode::InvalidAddress
        );
    }
}
