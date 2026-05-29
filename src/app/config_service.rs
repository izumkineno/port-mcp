use crate::model::{
    DomainError, HandleId, InstanceSummary, SerialConfig, TcpConfig, UdpConfig, VisaConfig,
};

use super::InstanceService;

impl InstanceService {
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

    pub fn configure_udp(
        &mut self,
        handle_id: &HandleId,
        config: UdpConfig,
    ) -> Result<InstanceSummary, DomainError> {
        self.registry.configure_udp(handle_id, config)
    }

    pub fn configure_visa(
        &mut self,
        handle_id: &HandleId,
        config: VisaConfig,
    ) -> Result<InstanceSummary, DomainError> {
        self.registry.configure_visa(handle_id, config)
    }
}
