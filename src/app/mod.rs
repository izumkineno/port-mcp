#![allow(dead_code)]

use crate::{
    model::{DomainError, HandleId, InstanceSummary, InstanceType, SerialConfig, TcpConfig},
    runtime::RuntimeRegistry,
};

pub struct InstanceService {
    registry: RuntimeRegistry,
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
