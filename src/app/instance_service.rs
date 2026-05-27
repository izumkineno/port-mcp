use std::collections::HashMap;

use crate::{
    model::{DomainError, HandleId, InstanceSummary, InstanceType},
    runtime::RuntimeRegistry,
    transport::SerialWorker,
};

pub struct InstanceService {
    pub(crate) registry: RuntimeRegistry,
    pub(crate) serial_workers: HashMap<String, SerialWorker>,
}

impl InstanceService {
    pub fn new_for_tests(date: &str) -> Self {
        Self {
            registry: RuntimeRegistry::new_for_tests(date),
            serial_workers: HashMap::new(),
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
        Ok(summary)
    }

    pub(crate) fn close_serial_worker(&mut self, handle_id: &HandleId) {
        if let Some(worker) = self.serial_workers.remove(handle_id.as_str()) {
            let _ = worker.close(1_000);
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
}
