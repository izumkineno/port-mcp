use crate::{
    model::{DomainError, HandleId},
    runtime::{SubscriptionResult, UnsubscribeResult},
};

use super::InstanceService;

impl InstanceService {
    pub fn subscribe(
        &mut self,
        handle_id: &HandleId,
        session_id: &str,
        max_payload_bytes: usize,
    ) -> Result<SubscriptionResult, DomainError> {
        self.registry
            .subscribe_mock(handle_id, session_id, max_payload_bytes)
    }

    pub fn unsubscribe(
        &mut self,
        handle_id: &HandleId,
        session_id: &str,
    ) -> Result<UnsubscribeResult, DomainError> {
        self.registry.unsubscribe_mock(handle_id, session_id)
    }
}
