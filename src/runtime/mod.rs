#![allow(dead_code)]

mod buffers;
mod locks;
mod queues;
mod subscriptions;
mod tasks;

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::json;

use crate::model::{
    ConfigSnapshot, DomainError, ErrorCategory, ErrorCode, ErrorDetails, HandleId, IdGenerator,
    InstanceState, InstanceStats, InstanceSummary, InstanceType, LastErrorSummary, ResourceSummary,
    RuntimeLimits, SerialConfig, TcpConfig, Timestamp, UdpConfig, VisaConfig,
    validate_instance_type,
};
#[allow(unused_imports)]
pub use buffers::{ClearResult, ClearTarget, PullResult, PullSource};
#[allow(unused_imports)]
pub use locks::{ResourceKey, ResourceLockState};
#[allow(unused_imports)]
pub use queues::{FlushResult, SendResult, SendTargetMode, SendTargetSummary};
#[allow(unused_imports)]
pub use subscriptions::{Notification, SubscriptionResult, UnsubscribeResult};
#[allow(unused_imports)]
pub use tasks::{TaskExit, TaskGroup, TaskGroupState};

use locks::{ResourceLockEntry, resource_lock_error};
use queues::SendItem;
use subscriptions::Subscriber;

pub struct RuntimeRegistry {
    instances: HashMap<String, RuntimeInstance>,
    released_handles: HashSet<String>,
    session_bindings: HashMap<String, HandleId>,
    resource_locks: HashMap<ResourceKey, ResourceLockEntry>,
    ids: IdGenerator,
    limits: RuntimeLimits,
    buffer_bytes_budget: usize,
    queued_bytes_budget: usize,
    global_notification_tick: Option<u64>,
    global_notifications_this_tick: u32,
}

impl RuntimeRegistry {
    pub fn new_for_tests(date: &str) -> Self {
        Self::new_for_tests_with_limits(date, RuntimeLimits::default())
    }

    pub fn new_for_tests_with_limits(date: &str, limits: RuntimeLimits) -> Self {
        Self {
            instances: HashMap::new(),
            released_handles: HashSet::new(),
            session_bindings: HashMap::new(),
            resource_locks: HashMap::new(),
            ids: IdGenerator::new_for_tests(date),
            limits,
            buffer_bytes_budget: 0,
            queued_bytes_budget: 0,
            global_notification_tick: None,
            global_notifications_this_tick: 0,
        }
    }

    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
            released_handles: HashSet::new(),
            session_bindings: HashMap::new(),
            resource_locks: HashMap::new(),
            ids: IdGenerator::new(),
            limits: RuntimeLimits::default(),
            buffer_bytes_budget: 0,
            queued_bytes_budget: 0,
            global_notification_tick: None,
            global_notifications_this_tick: 0,
        }
    }

    pub fn create_instance(
        &mut self,
        instance_type: InstanceType,
    ) -> Result<InstanceSummary, DomainError> {
        if self.instances.len() >= self.limits.max_instances {
            return Err(buffer_limit_error(
                ErrorCode::InvalidRange,
                "Instance limit exceeded.",
                self.limits.max_instances,
                self.instances.len() + 1,
            ));
        }
        let handle_id = self.ids.next_handle_id(instance_type);
        let instance = RuntimeInstance::new(handle_id.clone(), instance_type);
        let summary = instance.to_summary();
        self.instances
            .insert(handle_id.as_str().to_owned(), instance);
        Ok(summary)
    }

    pub fn list_instances(&self) -> Vec<InstanceSummary> {
        let mut instances = self
            .instances
            .values()
            .map(RuntimeInstance::to_summary)
            .collect::<Vec<_>>();
        instances.sort_by(|left, right| left.handle_id.as_str().cmp(right.handle_id.as_str()));
        instances
    }

    pub fn query_instance(&self, handle_id: &HandleId) -> Result<InstanceSummary, DomainError> {
        let instance = self.instance(handle_id)?;
        Ok(instance.to_summary())
    }

    pub fn use_instance(
        &mut self,
        session_id: Option<&str>,
        handle_id: &HandleId,
    ) -> Result<SessionBindingResult, DomainError> {
        let session_id = session_id.ok_or_else(DomainError::session_id_unavailable)?;
        let instance = self.instance(handle_id)?;
        if matches!(instance.state, InstanceState::Disconnecting) {
            return Err(state_not_allowed(
                "instance_use",
                instance.state,
                &[
                    "Created",
                    "Configured",
                    "Connected",
                    "Disconnected",
                    "Error",
                ],
            ));
        }

        let previous_handle_id = self
            .session_bindings
            .insert(session_id.to_owned(), handle_id.clone());
        Ok(SessionBindingResult { previous_handle_id })
    }

    pub fn resolve_handle(
        &self,
        explicit_handle_id: Option<&HandleId>,
        session_id: Option<&str>,
    ) -> Result<HandleId, DomainError> {
        if let Some(handle_id) = explicit_handle_id {
            self.instance(handle_id)?;
            return Ok(handle_id.clone());
        }

        let session_id = session_id.ok_or_else(DomainError::session_id_unavailable)?;
        let handle_id = self.session_bindings.get(session_id).ok_or_else(|| {
            DomainError::new(
                ErrorCategory::HandleNotFound,
                ErrorCode::SessionBindingMissing,
                "Current session has no default instance binding.",
                "Pass handle_id explicitly or call instance_use first.",
                false,
            )
        })?;
        self.instance(handle_id)?;
        Ok(handle_id.clone())
    }

    pub fn configure_serial(
        &mut self,
        handle_id: &HandleId,
        config: SerialConfig,
    ) -> Result<InstanceSummary, DomainError> {
        self.limits
            .validate_io_timeout_ms("timeout_ms", config.timeout_ms)?;
        self.configure(
            handle_id,
            InstanceType::Serial,
            ConfigSnapshot::Serial(config),
        )
    }

    pub fn configure_tcp(
        &mut self,
        handle_id: &HandleId,
        config: TcpConfig,
    ) -> Result<InstanceSummary, DomainError> {
        config.validate_remote(&self.limits)?;
        self.configure(handle_id, InstanceType::Tcp, ConfigSnapshot::Tcp(config))
    }

    pub fn configure_udp(
        &mut self,
        handle_id: &HandleId,
        config: UdpConfig,
    ) -> Result<InstanceSummary, DomainError> {
        config.validate_remote(&self.limits)?;
        self.configure(handle_id, InstanceType::Udp, ConfigSnapshot::Udp(config))
    }

    pub fn configure_visa(
        &mut self,
        handle_id: &HandleId,
        config: VisaConfig,
    ) -> Result<InstanceSummary, DomainError> {
        self.validate_visa_config(&config)?;
        self.configure(handle_id, InstanceType::Visa, ConfigSnapshot::Visa(config))
    }

    pub fn validate_visa_config(&self, config: &VisaConfig) -> Result<(), DomainError> {
        config.validate(&self.limits)
    }

    pub fn release_instance(
        &mut self,
        handle_id: &HandleId,
        force: bool,
    ) -> Result<InstanceSummary, DomainError> {
        let instance = self.instance(handle_id)?.clone();
        if matches!(instance.state, InstanceState::Connected) && !force {
            return Err(DomainError::new(
                ErrorCategory::InvalidState,
                ErrorCode::ConnectedReleaseRequiresForce,
                "Connected instances require force=true before release.",
                "Call port_disconnect first, or retry instance_release with force=true.",
                false,
            )
            .with_detail("current_state", json!(instance.state))
            .with_detail(
                "allowed_states",
                json!(["Created", "Configured", "Disconnected", "Error"]),
            ));
        }
        if matches!(instance.state, InstanceState::Disconnecting) && !force {
            return Err(state_not_allowed(
                "instance_release",
                instance.state,
                &["Created", "Configured", "Disconnected", "Error"],
            ));
        }

        let removed = self.instances.remove(handle_id.as_str());
        if let Some(removed) = &removed {
            let (queued_bytes, buffer_bytes) = removed.buffered_bytes();
            self.release_queued_bytes(queued_bytes);
            self.release_buffer_bytes(buffer_bytes);
        }
        self.released_handles.insert(handle_id.as_str().to_owned());
        self.session_bindings
            .retain(|_, bound_handle_id| bound_handle_id.as_str() != handle_id.as_str());
        self.release_or_close_owned_locks(handle_id, force);

        Ok(instance.released_summary())
    }

    pub fn connect_mock(&mut self, handle_id: &HandleId) -> Result<InstanceSummary, DomainError> {
        let instance = self.instance(handle_id)?;
        if !matches!(
            instance.state,
            InstanceState::Configured | InstanceState::Disconnected
        ) {
            return Err(state_not_allowed(
                "connect_mock",
                instance.state,
                &["Configured", "Disconnected"],
            ));
        }
        if let Some(key) = mock_resource_key(instance)? {
            self.acquire_resource_lock(key, handle_id)?;
        }

        let instance = self.instance_mut(handle_id)?;
        instance.task_group = Some(TaskGroup::new_for_tests());
        instance.state = InstanceState::Connected;
        Ok(instance.to_summary())
    }

    pub fn disconnect_mock(
        &mut self,
        handle_id: &HandleId,
    ) -> Result<InstanceSummary, DomainError> {
        let state = self.instance(handle_id)?.state;
        if matches!(state, InstanceState::Disconnected) {
            return Ok(self.instance(handle_id)?.to_summary());
        }
        if !matches!(state, InstanceState::Connected | InstanceState::Error) {
            return Err(state_not_allowed(
                "disconnect_mock",
                state,
                &["Connected", "Error", "Disconnected"],
            ));
        }

        let owned_locks = self.owned_resource_keys(handle_id);
        for key in owned_locks {
            self.resource_locks.remove(&key);
        }

        let (summary, released_queued_bytes) = {
            let instance = self.instance_mut(handle_id)?;
            if let Some(task_group) = &mut instance.task_group {
                task_group.cancel();
                task_group.finish(TaskExit::Clean);
            }
            let released_queued_bytes = instance.queued_bytes();
            instance.tx_queue.clear();
            instance.subscribers.clear();
            instance.stats.tx_queue_items = 0;
            instance.stats.subscriber_count = 0;
            instance.state = InstanceState::Disconnected;
            (instance.to_summary(), released_queued_bytes)
        };
        self.release_queued_bytes(released_queued_bytes);
        Ok(summary)
    }

    pub fn record_mock_task_failure(
        &mut self,
        handle_id: &HandleId,
        code: ErrorCode,
        message: &str,
    ) -> Result<(), DomainError> {
        let error_id = self.ids.next_error_id();
        let instance = self.instance_mut(handle_id)?;
        if let Some(task_group) = &mut instance.task_group {
            task_group.finish(TaskExit::Failed(code));
        }
        instance.state = InstanceState::Error;
        instance.last_error = Some(LastErrorSummary {
            error_id,
            at: Timestamp::now_utc(),
            tool: "mock_task".to_owned(),
            category: ErrorCategory::WriteFailed,
            code,
            message: message.to_owned(),
            recovery_hint: "Disconnect or release the mock instance before retrying.".to_owned(),
            details: ErrorDetails::new(),
        });
        Ok(())
    }

    pub fn complete_mock_background_close(&mut self, key: &ResourceKey) -> Result<(), DomainError> {
        self.complete_resource_close(key)
    }

    pub fn acquire_resource_lock(
        &mut self,
        key: ResourceKey,
        owner_handle_id: &HandleId,
    ) -> Result<(), DomainError> {
        if let Some(entry) = self.resource_locks.get(&key) {
            return Err(resource_lock_error(&key, entry));
        }

        self.resource_locks
            .insert(key, ResourceLockEntry::held(owner_handle_id.clone()));
        Ok(())
    }

    pub fn resource_lock_state(&self, key: &ResourceKey) -> Option<ResourceLockState> {
        self.resource_locks.get(key).map(|entry| entry.state)
    }

    fn owned_resource_keys(&self, handle_id: &HandleId) -> Vec<ResourceKey> {
        self.resource_locks
            .iter()
            .filter_map(|(key, entry)| {
                if entry.owner_handle_id.as_str() == handle_id.as_str() {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn configure(
        &mut self,
        handle_id: &HandleId,
        expected_type: InstanceType,
        config: ConfigSnapshot,
    ) -> Result<InstanceSummary, DomainError> {
        let instance = self.instance(handle_id)?;
        validate_instance_type(instance.instance_type, expected_type)?;
        ensure_configurable(instance.state)?;

        let instance = self.instance_mut(handle_id)?;
        instance.config = Some(config);
        instance.state = InstanceState::Configured;
        Ok(instance.to_summary())
    }

    #[cfg(test)]
    pub fn set_state_for_tests(
        &mut self,
        handle_id: &HandleId,
        state: InstanceState,
    ) -> Result<(), DomainError> {
        self.instance_mut(handle_id)?.state = state;
        Ok(())
    }

    #[cfg(test)]
    pub fn move_resource_lock_to_closing_for_tests(
        &mut self,
        key: &ResourceKey,
        owner_handle_id: &HandleId,
    ) -> Result<(), DomainError> {
        let entry = self.resource_locks.get_mut(key).ok_or_else(|| {
            DomainError::new(
                ErrorCategory::HandleNotFound,
                ErrorCode::HandleNotFound,
                "Resource lock does not exist.",
                "Acquire the resource lock before moving it to closing.",
                false,
            )
        })?;
        if entry.owner_handle_id.as_str() != owner_handle_id.as_str() {
            return Err(resource_lock_error(key, entry));
        }
        entry.state = ResourceLockState::Closing;
        entry.generation += 1;
        Ok(())
    }

    #[cfg(test)]
    pub fn mark_resource_lock_stale_for_tests(
        &mut self,
        key: &ResourceKey,
    ) -> Result<(), DomainError> {
        let entry = self.resource_locks.get_mut(key).ok_or_else(|| {
            DomainError::new(
                ErrorCategory::HandleNotFound,
                ErrorCode::HandleNotFound,
                "Resource lock does not exist.",
                "Acquire the resource lock before marking it stale.",
                false,
            )
        })?;
        entry.state = ResourceLockState::Stale;
        entry.stale_close = true;
        Ok(())
    }

    #[cfg(test)]
    pub fn complete_resource_close_for_tests(
        &mut self,
        key: &ResourceKey,
    ) -> Result<(), DomainError> {
        self.complete_resource_close(key)
    }

    fn complete_resource_close(&mut self, key: &ResourceKey) -> Result<(), DomainError> {
        match self.resource_locks.get(key) {
            Some(entry) if entry.state == ResourceLockState::Closing => {
                self.resource_locks.remove(key);
                Ok(())
            }
            Some(entry) => Err(resource_lock_error(key, entry)),
            None => Ok(()),
        }
    }

    fn instance(&self, handle_id: &HandleId) -> Result<&RuntimeInstance, DomainError> {
        self.instances
            .get(handle_id.as_str())
            .ok_or_else(|| self.missing_handle_error(handle_id))
    }

    fn instance_mut(&mut self, handle_id: &HandleId) -> Result<&mut RuntimeInstance, DomainError> {
        if !self.instances.contains_key(handle_id.as_str()) {
            return Err(self.missing_handle_error(handle_id));
        }
        Ok(self
            .instances
            .get_mut(handle_id.as_str())
            .expect("checked instance presence before mutable lookup"))
    }

    fn missing_handle_error(&self, handle_id: &HandleId) -> DomainError {
        let code = if self.released_handles.contains(handle_id.as_str()) {
            ErrorCode::HandleReleased
        } else {
            ErrorCode::HandleNotFound
        };
        DomainError::new(
            ErrorCategory::HandleNotFound,
            code,
            "Instance handle does not refer to an active instance.",
            "Call instance_list to inspect active instances, or create a new instance.",
            false,
        )
        .with_detail("handle_id", json!(handle_id))
    }

    fn release_or_close_owned_locks(&mut self, handle_id: &HandleId, force: bool) {
        if force {
            for entry in self.resource_locks.values_mut() {
                if entry.owner_handle_id.as_str() == handle_id.as_str() {
                    entry.state = ResourceLockState::Closing;
                    entry.generation += 1;
                }
            }
        } else {
            self.resource_locks
                .retain(|_, entry| entry.owner_handle_id.as_str() != handle_id.as_str());
        }
    }

    pub fn port_send_mock(
        &mut self,
        handle_id: &HandleId,
        bytes: &[u8],
    ) -> Result<SendResult, DomainError> {
        self.ensure_connected(handle_id, "port_send")?;
        if bytes.len() > self.limits.tx_frame_max_bytes {
            return Err(buffer_limit_error(
                ErrorCode::TxFrameTooLarge,
                "TX frame exceeds tx_frame_max_bytes.",
                self.limits.tx_frame_max_bytes,
                bytes.len(),
            ));
        }

        let instance = self.instance(handle_id)?;
        if instance.tx_queue.len() >= self.limits.tx_queue_max_items {
            return Err(buffer_limit_error(
                ErrorCode::TxQueueFull,
                "TX queue is full.",
                self.limits.tx_queue_max_items,
                instance.tx_queue.len() + 1,
            ));
        }
        self.reserve_queued_bytes(bytes.len())?;

        let instance = self.instance_mut(handle_id)?;
        instance.tx_queue.push_back(SendItem {
            bytes: bytes.to_vec(),
        });
        instance.stats.tx_queue_items = instance.tx_queue.len();
        Ok(SendResult {
            queued: true,
            sent_bytes: bytes.len(),
            target: None,
        })
    }

    pub fn flush_mock_tx(&mut self, handle_id: &HandleId) -> Result<FlushResult, DomainError> {
        self.ensure_connected(handle_id, "flush_mock_tx")?;
        let instance = self.instance_mut(handle_id)?;
        let frames = instance
            .tx_queue
            .drain(..)
            .map(|item| item.bytes)
            .collect::<Vec<_>>();
        let released_bytes = frames.iter().map(Vec::len).sum::<usize>();
        instance.stats.tx_queue_items = 0;
        instance.stats.tx_bytes += released_bytes as u64;
        self.release_queued_bytes(released_bytes);
        Ok(FlushResult { frames })
    }

    pub fn receive_mock_chunk(
        &mut self,
        handle_id: &HandleId,
        bytes: &[u8],
        tick: u64,
    ) -> Result<(), DomainError> {
        self.ensure_connected(handle_id, "receive_mock_chunk")?;
        self.append_rx_buffer(handle_id, bytes)?;
        self.broadcast_mock(handle_id, bytes, tick)
    }

    pub fn port_pull_mock(
        &mut self,
        handle_id: &HandleId,
        max_bytes: Option<usize>,
    ) -> Result<PullResult, DomainError> {
        self.ensure_connected(handle_id, "port_pull")?;
        let max_bytes = match max_bytes {
            Some(value) => self.limits.validate_pull_max_bytes(value)?,
            None => self.limits.pull_default_max_bytes,
        };
        let instance = self.instance_mut(handle_id)?;
        if instance.rx_buffer.is_empty() {
            return Err(DomainError::read_timeout());
        }
        let take = max_bytes.min(instance.rx_buffer.len());
        let bytes = instance.rx_buffer.drain(..take).collect::<Vec<_>>();
        let remaining = instance.rx_buffer.len();
        instance.stats.rx_buffer_bytes = remaining;
        self.release_buffer_bytes(bytes.len());
        Ok(PullResult {
            truncated: remaining > 0,
            remaining_rx_buffer_bytes: remaining,
            bytes,
            source: None,
        })
    }

    pub fn port_clear_mock(
        &mut self,
        handle_id: &HandleId,
        target: ClearTarget,
    ) -> Result<ClearResult, DomainError> {
        self.ensure_connected_or_error(handle_id, "port_clear")?;
        let instance = self.instance_mut(handle_id)?;
        let mut result = ClearResult::default();
        if matches!(target, ClearTarget::Tx | ClearTarget::All) {
            result.dropped_tx_items = instance.tx_queue.len();
            result.dropped_tx_bytes = instance.tx_queue.iter().map(|item| item.bytes.len()).sum();
            instance.tx_queue.clear();
            instance.stats.tx_queue_items = 0;
        }
        if matches!(target, ClearTarget::Rx | ClearTarget::All) {
            result.dropped_rx_bytes = instance.rx_buffer.len();
            instance.rx_buffer.clear();
            instance.stats.rx_buffer_bytes = 0;
        }
        self.release_queued_bytes(result.dropped_tx_bytes);
        self.release_buffer_bytes(result.dropped_rx_bytes);
        Ok(result)
    }

    pub fn subscribe_mock(
        &mut self,
        handle_id: &HandleId,
        session_id: &str,
        max_payload_bytes: usize,
    ) -> Result<SubscriptionResult, DomainError> {
        self.ensure_connected(handle_id, "port_subscribe_stream")?;
        let max_payload_bytes = max_payload_bytes.min(self.limits.subscriber_payload_max_bytes);
        let limit = self.limits.max_subscribers_per_instance;
        let instance = self.instance_mut(handle_id)?;
        let was_subscribed = instance.subscribers.contains_key(session_id);
        if !was_subscribed && instance.subscribers.len() >= limit {
            return Err(buffer_limit_error(
                ErrorCode::SubscriberLimitExceeded,
                "Subscriber limit exceeded for this instance.",
                limit,
                instance.subscribers.len() + 1,
            ));
        }
        instance
            .subscribers
            .entry(session_id.to_owned())
            .and_modify(|subscriber| subscriber.max_payload_bytes = max_payload_bytes)
            .or_insert_with(|| Subscriber::new(session_id, max_payload_bytes));
        instance.stats.subscriber_count = instance.subscribers.len();
        Ok(SubscriptionResult { was_subscribed })
    }

    pub fn unsubscribe_mock(
        &mut self,
        handle_id: &HandleId,
        session_id: &str,
    ) -> Result<UnsubscribeResult, DomainError> {
        self.ensure_connected_or_error(handle_id, "port_unsubscribe_stream")?;
        let instance = self.instance_mut(handle_id)?;
        let removed = instance.subscribers.remove(session_id);
        let released_bytes = removed
            .as_ref()
            .map(|subscriber| subscriber.queued_bytes())
            .unwrap_or(0);
        instance.stats.subscriber_count = instance.subscribers.len();
        self.release_queued_bytes(released_bytes);
        Ok(UnsubscribeResult {
            was_subscribed: removed.is_some(),
        })
    }

    pub fn pull_subscription_mock(
        &mut self,
        handle_id: &HandleId,
        session_id: &str,
    ) -> Result<Vec<Notification>, DomainError> {
        let instance = self.instance_mut(handle_id)?;
        let subscriber = instance.subscribers.get_mut(session_id).ok_or_else(|| {
            DomainError::new(
                ErrorCategory::HandleNotFound,
                ErrorCode::HandleNotFound,
                "Subscriber does not exist for this instance.",
                "Subscribe before pulling mock notifications.",
                false,
            )
        })?;
        let notifications = subscriber.queue.drain(..).collect::<Vec<_>>();
        let released_bytes = notifications
            .iter()
            .map(|notification| notification.payload.len())
            .sum();
        self.release_queued_bytes(released_bytes);
        Ok(notifications)
    }

    pub fn buffer_bytes_for_tests(&self) -> usize {
        self.buffer_bytes_budget
    }

    pub fn queued_bytes_for_tests(&self) -> usize {
        self.queued_bytes_budget
    }

    fn append_rx_buffer(&mut self, handle_id: &HandleId, bytes: &[u8]) -> Result<(), DomainError> {
        let keep_limit = self
            .limits
            .rx_buffer_max_bytes
            .min(self.limits.max_total_buffer_bytes);
        let instance = self.instance_mut(handle_id)?;
        let before = instance.rx_buffer.len();
        instance.rx_buffer.extend(bytes.iter().copied());
        let mut dropped = 0usize;
        while instance.rx_buffer.len() > keep_limit {
            instance.rx_buffer.pop_front();
            dropped += 1;
        }
        let after = instance.rx_buffer.len();
        instance.stats.rx_bytes += bytes.len() as u64;
        instance.stats.rx_dropped_bytes += dropped as u64;
        instance.stats.rx_buffer_bytes = after;
        let delta = after.saturating_sub(before);
        let release = before.saturating_add(bytes.len()).saturating_sub(after);
        self.reserve_buffer_bytes_unchecked(delta);
        self.release_buffer_bytes(release.saturating_sub(dropped));
        Ok(())
    }

    fn broadcast_mock(
        &mut self,
        handle_id: &HandleId,
        bytes: &[u8],
        tick: u64,
    ) -> Result<(), DomainError> {
        self.reset_global_notification_window(tick);
        let subscriber_limit = self.limits.subscriber_notifications_per_sec;
        let instance_limit = self.limits.instance_notifications_per_sec;
        let global_limit = self.limits.global_notifications_per_sec;
        let queue_limit = self.limits.subscriber_queue_max_items;
        let mut global_notifications_this_tick = self.global_notifications_this_tick;
        let instance = self.instance_mut(handle_id)?;
        if instance.notification_tick != Some(tick) {
            instance.notification_tick = Some(tick);
            instance.notifications_this_tick = 0;
        }

        let mut queued_delta = 0usize;
        let mut released_delta = 0usize;
        let mut dropped_total = 0u64;
        for subscriber in instance.subscribers.values_mut() {
            let rate_limited = subscriber.note_tick(tick, subscriber_limit)
                || instance.notifications_this_tick >= instance_limit
                || global_notifications_this_tick >= global_limit;
            let (queued, released, dropped) = subscriber.enqueue(bytes, queue_limit, rate_limited);
            queued_delta += queued;
            released_delta += released;
            dropped_total += dropped;
            if !rate_limited {
                instance.notifications_this_tick += 1;
                global_notifications_this_tick += 1;
            }
        }
        instance.stats.dropped_notifications += dropped_total;
        self.global_notifications_this_tick = global_notifications_this_tick;
        self.release_queued_bytes(released_delta);
        if queued_delta > 0 {
            self.reserve_queued_bytes(queued_delta)?;
        }
        Ok(())
    }

    pub fn ensure_connected(&self, handle_id: &HandleId, tool: &str) -> Result<(), DomainError> {
        let state = self.instance(handle_id)?.state;
        if state == InstanceState::Connected {
            Ok(())
        } else {
            Err(state_not_allowed(tool, state, &["Connected"]))
        }
    }

    pub fn validate_pull_max_bytes(&self, value: usize) -> Result<usize, DomainError> {
        self.limits.validate_pull_max_bytes(value)
    }

    pub fn validate_tx_frame_len(&self, len: usize) -> Result<(), DomainError> {
        self.limits.validate_tx_frame_len(len)
    }

    pub fn tx_frame_max_bytes(&self) -> usize {
        self.limits.tx_frame_max_bytes
    }

    pub fn default_pull_max_bytes(&self) -> usize {
        self.limits.pull_default_max_bytes
    }

    pub fn record_direct_tx(
        &mut self,
        handle_id: &HandleId,
        bytes: usize,
    ) -> Result<(), DomainError> {
        let instance = self.instance_mut(handle_id)?;
        instance.stats.tx_bytes += bytes as u64;
        instance.stats.tx_queue_items = instance.tx_queue.len();
        instance.stats.last_activity_at = Some(Timestamp::now_utc());
        Ok(())
    }

    pub fn record_direct_rx(
        &mut self,
        handle_id: &HandleId,
        bytes: usize,
    ) -> Result<(), DomainError> {
        let instance = self.instance_mut(handle_id)?;
        instance.stats.rx_bytes += bytes as u64;
        instance.stats.last_activity_at = Some(Timestamp::now_utc());
        Ok(())
    }

    fn ensure_connected_or_error(
        &self,
        handle_id: &HandleId,
        tool: &str,
    ) -> Result<(), DomainError> {
        let state = self.instance(handle_id)?.state;
        if matches!(state, InstanceState::Connected | InstanceState::Error) {
            Ok(())
        } else {
            Err(state_not_allowed(tool, state, &["Connected", "Error"]))
        }
    }

    fn reserve_queued_bytes(&mut self, bytes: usize) -> Result<(), DomainError> {
        let requested = self.queued_bytes_budget.saturating_add(bytes);
        if requested > self.limits.max_total_queued_bytes {
            return Err(buffer_limit_error(
                ErrorCode::TxQueueFull,
                "Queued bytes budget would be exceeded.",
                self.limits.max_total_queued_bytes,
                requested,
            ));
        }
        self.queued_bytes_budget = requested;
        Ok(())
    }

    fn release_queued_bytes(&mut self, bytes: usize) {
        self.queued_bytes_budget = self.queued_bytes_budget.saturating_sub(bytes);
    }

    fn reserve_buffer_bytes_unchecked(&mut self, bytes: usize) {
        self.buffer_bytes_budget = self.buffer_bytes_budget.saturating_add(bytes);
    }

    fn release_buffer_bytes(&mut self, bytes: usize) {
        self.buffer_bytes_budget = self.buffer_bytes_budget.saturating_sub(bytes);
    }

    fn reset_global_notification_window(&mut self, tick: u64) {
        if self.global_notification_tick != Some(tick) {
            self.global_notification_tick = Some(tick);
            self.global_notifications_this_tick = 0;
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionBindingResult {
    pub previous_handle_id: Option<HandleId>,
}

#[derive(Debug, Clone)]
struct RuntimeInstance {
    handle_id: HandleId,
    instance_type: InstanceType,
    state: InstanceState,
    config: Option<ConfigSnapshot>,
    stats: InstanceStats,
    task_group: Option<TaskGroup>,
    tx_queue: VecDeque<SendItem>,
    rx_buffer: VecDeque<u8>,
    subscribers: HashMap<String, Subscriber>,
    notification_tick: Option<u64>,
    notifications_this_tick: u32,
    last_error: Option<LastErrorSummary>,
}

impl RuntimeInstance {
    fn new(handle_id: HandleId, instance_type: InstanceType) -> Self {
        Self {
            handle_id,
            instance_type,
            state: InstanceState::Created,
            config: None,
            stats: InstanceStats::default(),
            task_group: None,
            tx_queue: VecDeque::new(),
            rx_buffer: VecDeque::new(),
            subscribers: HashMap::new(),
            notification_tick: None,
            notifications_this_tick: 0,
            last_error: None,
        }
    }

    fn to_summary(&self) -> InstanceSummary {
        InstanceSummary {
            handle_id: self.handle_id.clone(),
            instance_type: self.instance_type,
            state: self.state,
            resource: self.config.as_ref().map(resource_summary),
            config: self.config.clone(),
            stats: self.stats.clone(),
            peers: None,
            last_error: self.last_error.clone(),
        }
    }

    fn released_summary(&self) -> InstanceSummary {
        let mut summary = self.to_summary();
        summary.state = InstanceState::Released;
        summary
    }

    fn queued_bytes(&self) -> usize {
        let tx_bytes = self
            .tx_queue
            .iter()
            .map(|item| item.bytes.len())
            .sum::<usize>();
        let subscriber_bytes = self
            .subscribers
            .values()
            .map(Subscriber::queued_bytes)
            .sum::<usize>();
        tx_bytes + subscriber_bytes
    }

    fn buffered_bytes(&self) -> (usize, usize) {
        (self.queued_bytes(), self.rx_buffer.len())
    }
}

fn resource_summary(config: &ConfigSnapshot) -> ResourceSummary {
    match config {
        ConfigSnapshot::Serial(config) => ResourceSummary::serial(&config.port),
        ConfigSnapshot::Tcp(config) => match config.mode {
            crate::model::TcpMode::Client => ResourceSummary::tcp_client(&config.host, config.port),
            crate::model::TcpMode::Listen => ResourceSummary::tcp_listen(&config.host, config.port),
        },
        ConfigSnapshot::Udp(config) => ResourceSummary::udp(
            &config.bind_host,
            config.bind_port,
            config.remote_host.as_deref(),
            config.remote_port,
        ),
        ConfigSnapshot::Visa(config) => ResourceSummary::visa(&config.resource_address, None),
    }
}

fn mock_resource_key(instance: &RuntimeInstance) -> Result<Option<ResourceKey>, DomainError> {
    match &instance.config {
        Some(ConfigSnapshot::Serial(config)) => Ok(Some(ResourceKey::serial(&config.port))),
        Some(ConfigSnapshot::Tcp(config)) => match config.mode {
            crate::model::TcpMode::Client => Ok(None),
            crate::model::TcpMode::Listen => {
                Ok(Some(ResourceKey::tcp_listen(&config.host, config.port)))
            }
        },
        Some(ConfigSnapshot::Udp(config)) => Ok(Some(ResourceKey::udp_bind(
            &config.bind_host,
            config.bind_port,
        ))),
        Some(ConfigSnapshot::Visa(config)) => Ok(Some(ResourceKey::visa(&config.resource_address))),
        None => Err(DomainError::new(
            ErrorCategory::InvalidState,
            ErrorCode::ConfigRequired,
            "Mock connect requires an instance configuration.",
            "Configure the instance before connecting mock transport.",
            false,
        )),
    }
}

fn ensure_configurable(state: InstanceState) -> Result<(), DomainError> {
    if matches!(
        state,
        InstanceState::Created | InstanceState::Configured | InstanceState::Disconnected
    ) {
        Ok(())
    } else {
        Err(state_not_allowed(
            "configure",
            state,
            &["Created", "Configured", "Disconnected"],
        ))
    }
}

fn state_not_allowed(tool: &str, state: InstanceState, allowed_states: &[&str]) -> DomainError {
    DomainError::new(
        ErrorCategory::InvalidState,
        ErrorCode::StateNotAllowed,
        format!("State {state:?} is not allowed for {tool}."),
        "Call an allowed lifecycle tool first, then retry.",
        false,
    )
    .with_detail("current_state", json!(state))
    .with_detail("allowed_states", json!(allowed_states))
}

fn buffer_limit_error(
    code: ErrorCode,
    message: impl Into<String>,
    limit: usize,
    requested: usize,
) -> DomainError {
    DomainError::new(
        ErrorCategory::BufferLimitExceeded,
        code,
        message,
        "Reduce payload size, pull or clear buffered data, then retry.",
        true,
    )
    .with_detail("limit", json!(limit))
    .with_detail("requested", json!(requested))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ConfigSnapshot, ErrorCategory, ErrorCode, InstanceState, InstanceType, RuntimeLimits,
        SerialConfig, TcpConfig, UdpConfig,
    };

    #[test]
    fn unit_registry_creates_lists_queries_and_releases_without_io() {
        let mut registry = RuntimeRegistry::new_for_tests("20260526");

        let serial = registry.create_instance(InstanceType::Serial).unwrap();
        let tcp = registry.create_instance(InstanceType::Tcp).unwrap();

        assert_eq!(serial.handle_id.as_str(), "h_ser_001");
        assert_eq!(serial.state, InstanceState::Created);
        assert_eq!(tcp.handle_id.as_str(), "h_tcp_001");

        let listed = registry.list_instances();
        assert_eq!(listed.len(), 2);
        assert!(
            listed
                .iter()
                .all(|summary| summary.state == InstanceState::Created)
        );

        let queried = registry.query_instance(&serial.handle_id).unwrap();
        assert_eq!(queried.instance_type, InstanceType::Serial);
        assert!(queried.config.is_none());

        let released = registry.release_instance(&serial.handle_id, false).unwrap();
        assert_eq!(released.state, InstanceState::Released);

        let error = registry.query_instance(&serial.handle_id).unwrap_err();
        assert_eq!(error.category, ErrorCategory::HandleNotFound);
        assert_eq!(error.code, ErrorCode::HandleReleased);
        assert_eq!(registry.list_instances().len(), 1);
    }

    #[test]
    fn unit_registry_enforces_max_instances_and_releases_capacity() {
        let mut limits = RuntimeLimits::default();
        limits.max_instances = 1;
        let mut registry = RuntimeRegistry::new_for_tests_with_limits("20260526", limits);

        let serial = registry.create_instance(InstanceType::Serial).unwrap();
        let error = registry.create_instance(InstanceType::Tcp).unwrap_err();
        assert_eq!(error.category, ErrorCategory::BufferLimitExceeded);
        assert_eq!(error.code, ErrorCode::InvalidRange);

        registry.release_instance(&serial.handle_id, false).unwrap();
        let tcp = registry.create_instance(InstanceType::Tcp).unwrap();
        assert_eq!(tcp.handle_id.as_str(), "h_tcp_001");
    }

    #[test]
    fn unit_session_binding_resolves_default_handles_without_global_fallback() {
        let mut registry = RuntimeRegistry::new_for_tests("20260526");
        let instance = registry.create_instance(InstanceType::Udp).unwrap();

        let unavailable = registry
            .use_instance(None, &instance.handle_id)
            .unwrap_err();
        assert_eq!(unavailable.code, ErrorCode::SessionIdUnavailable);

        let missing = registry
            .resolve_handle(None, Some("session-a"))
            .unwrap_err();
        assert_eq!(missing.category, ErrorCategory::HandleNotFound);
        assert_eq!(missing.code, ErrorCode::SessionBindingMissing);

        let bound = registry
            .use_instance(Some("session-a"), &instance.handle_id)
            .unwrap();
        assert!(bound.previous_handle_id.is_none());

        let rebound = registry
            .use_instance(Some("session-a"), &instance.handle_id)
            .unwrap();
        assert_eq!(
            rebound.previous_handle_id.unwrap().as_str(),
            instance.handle_id.as_str()
        );

        let resolved = registry.resolve_handle(None, Some("session-a")).unwrap();
        assert_eq!(resolved.as_str(), instance.handle_id.as_str());

        let explicit = registry
            .resolve_handle(Some(&instance.handle_id), None)
            .unwrap();
        assert_eq!(explicit.as_str(), instance.handle_id.as_str());
    }

    #[test]
    fn unit_config_state_writes_atomically_and_keeps_old_config_on_error() {
        let mut registry = RuntimeRegistry::new_for_tests("20260526");
        let serial = registry.create_instance(InstanceType::Serial).unwrap();

        let configured = registry
            .configure_serial(&serial.handle_id, SerialConfig::new("COM3"))
            .unwrap();
        assert_eq!(configured.state, InstanceState::Configured);
        assert!(matches!(configured.config, Some(ConfigSnapshot::Serial(_))));

        let wrong_type = registry
            .configure_tcp(&serial.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap_err();
        assert_eq!(wrong_type.code, ErrorCode::TypeMismatch);

        let after_error = registry.query_instance(&serial.handle_id).unwrap();
        assert_eq!(after_error.state, InstanceState::Configured);
        assert!(matches!(
            after_error.config,
            Some(ConfigSnapshot::Serial(_))
        ));

        let udp = registry.create_instance(InstanceType::Udp).unwrap();
        let udp_summary = registry
            .configure_udp(
                &udp.handle_id,
                UdpConfig {
                    bind_host: "127.0.0.1".to_owned(),
                    bind_port: 9001,
                    remote_host: None,
                    remote_port: None,
                    timeout_ms: 1_000,
                },
            )
            .unwrap();
        assert!(matches!(udp_summary.config, Some(ConfigSnapshot::Udp(_))));
    }

    #[test]
    fn unit_config_rejects_timeout_and_network_boundary_violations() {
        let mut limits = RuntimeLimits::default();
        limits.io_timeout_max_ms = 2_000;
        let mut registry = RuntimeRegistry::new_for_tests_with_limits("20260526", limits);

        let serial = registry.create_instance(InstanceType::Serial).unwrap();
        let mut serial_config = SerialConfig::new("COM3");
        serial_config.timeout_ms = 2_001;
        assert_eq!(
            registry
                .configure_serial(&serial.handle_id, serial_config)
                .unwrap_err()
                .code,
            ErrorCode::InvalidRange
        );

        let tcp = registry.create_instance(InstanceType::Tcp).unwrap();
        assert_eq!(
            registry
                .configure_tcp(&tcp.handle_id, TcpConfig::client("192.0.2.1", 9000))
                .unwrap_err()
                .code,
            ErrorCode::ScanTargetNotAllowed
        );

        let udp = registry.create_instance(InstanceType::Udp).unwrap();
        assert_eq!(
            registry
                .configure_udp(
                    &udp.handle_id,
                    UdpConfig {
                        bind_host: "0.0.0.0".to_owned(),
                        bind_port: 9001,
                        remote_host: None,
                        remote_port: None,
                        timeout_ms: 1_000,
                    },
                )
                .unwrap_err()
                .code,
            ErrorCode::ScanTargetNotAllowed
        );

        let mut allow_limits = RuntimeLimits::default();
        allow_limits
            .network_allowed_hosts
            .push("192.0.2.1".to_owned());
        let mut allow_registry =
            RuntimeRegistry::new_for_tests_with_limits("20260526", allow_limits);
        let tcp = allow_registry.create_instance(InstanceType::Tcp).unwrap();
        assert!(
            allow_registry
                .configure_tcp(&tcp.handle_id, TcpConfig::client("192.0.2.1", 9000))
                .is_ok()
        );
        let visa = allow_registry.create_instance(InstanceType::Visa).unwrap();
        assert!(
            allow_registry
                .configure_visa(&visa.handle_id, VisaConfig::new("TCPIP0::192.0.2.1::INSTR"))
                .is_ok()
        );

        let visa = registry.create_instance(InstanceType::Visa).unwrap();
        assert_eq!(
            registry
                .configure_visa(&visa.handle_id, VisaConfig::new("TCPIP0::192.0.2.1::INSTR"))
                .unwrap_err()
                .code,
            ErrorCode::ScanTargetNotAllowed
        );
    }

    #[test]
    fn unit_state_matrix_rejects_configure_and_release_in_disallowed_states() {
        let mut registry = RuntimeRegistry::new_for_tests("20260526");
        let tcp = registry.create_instance(InstanceType::Tcp).unwrap();

        registry
            .set_state_for_tests(&tcp.handle_id, InstanceState::Connected)
            .unwrap();

        let config_error = registry
            .configure_tcp(&tcp.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap_err();
        assert_eq!(config_error.category, ErrorCategory::InvalidState);
        assert_eq!(config_error.code, ErrorCode::StateNotAllowed);

        let release_error = registry
            .release_instance(&tcp.handle_id, false)
            .unwrap_err();
        assert_eq!(release_error.code, ErrorCode::ConnectedReleaseRequiresForce);

        let released = registry.release_instance(&tcp.handle_id, true).unwrap();
        assert_eq!(released.state, InstanceState::Released);
    }

    #[test]
    fn unit_resource_locks_normalize_keys_and_report_held_closing_stale_states() {
        let mut registry = RuntimeRegistry::new_for_tests("20260526");
        let owner = registry.create_instance(InstanceType::Serial).unwrap();
        let contender = registry.create_instance(InstanceType::Serial).unwrap();

        let serial_key = ResourceKey::serial(" com3 ");
        assert_eq!(serial_key.as_str(), "serial:COM3");
        assert_eq!(
            ResourceKey::tcp_listen("127.000.000.001", 9000).as_str(),
            "tcp-listen:127.0.0.1:9000"
        );
        assert_eq!(
            ResourceKey::udp_bind("LOCALHOST", 9001).as_str(),
            "udp-bind:localhost:9001"
        );

        registry
            .acquire_resource_lock(serial_key.clone(), &owner.handle_id)
            .unwrap();
        assert_eq!(
            registry.resource_lock_state(&serial_key).unwrap(),
            ResourceLockState::Held
        );

        let busy = registry
            .acquire_resource_lock(serial_key.clone(), &contender.handle_id)
            .unwrap_err();
        assert_eq!(busy.category, ErrorCategory::ResourceBusy);
        assert_eq!(busy.code, ErrorCode::SerialPortBusy);

        registry
            .move_resource_lock_to_closing_for_tests(&serial_key, &owner.handle_id)
            .unwrap();
        assert_eq!(
            registry.resource_lock_state(&serial_key).unwrap(),
            ResourceLockState::Closing
        );
        let closing = registry
            .acquire_resource_lock(serial_key.clone(), &contender.handle_id)
            .unwrap_err();
        assert_eq!(closing.code, ErrorCode::ResourceClosing);

        registry
            .mark_resource_lock_stale_for_tests(&serial_key)
            .unwrap();
        assert_eq!(
            registry.resource_lock_state(&serial_key).unwrap(),
            ResourceLockState::Stale
        );
        let stale = registry
            .acquire_resource_lock(serial_key, &contender.handle_id)
            .unwrap_err();
        assert_eq!(stale.code, ErrorCode::ResourceLockStale);
    }

    #[test]
    fn unit_release_lifecycle_moves_forced_connected_resources_to_closing_tombstone() {
        let mut registry = RuntimeRegistry::new_for_tests("20260526");
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        let key = ResourceKey::tcp_listen("127.0.0.1", 9000);

        registry
            .acquire_resource_lock(key.clone(), &instance.handle_id)
            .unwrap();
        registry
            .set_state_for_tests(&instance.handle_id, InstanceState::Connected)
            .unwrap();
        registry
            .use_instance(Some("session-a"), &instance.handle_id)
            .unwrap();

        let released = registry
            .release_instance(&instance.handle_id, true)
            .unwrap();
        assert_eq!(released.state, InstanceState::Released);
        assert_eq!(
            registry.resource_lock_state(&key).unwrap(),
            ResourceLockState::Closing
        );
        assert_eq!(
            registry
                .query_instance(&instance.handle_id)
                .unwrap_err()
                .code,
            ErrorCode::HandleReleased
        );
        assert_eq!(
            registry
                .resolve_handle(None, Some("session-a"))
                .unwrap_err()
                .code,
            ErrorCode::SessionBindingMissing
        );

        registry.complete_resource_close_for_tests(&key).unwrap();
        assert!(registry.resource_lock_state(&key).is_none());
    }

    #[test]
    fn unit_tasks_create_cancel_and_report_mock_task_state() {
        let mut group = TaskGroup::new_for_tests();
        assert_eq!(group.state(), TaskGroupState::Running);

        group.cancel();
        assert_eq!(group.state(), TaskGroupState::Cancelling);

        group.finish(TaskExit::Clean);
        assert_eq!(group.state(), TaskGroupState::Finished);
        assert_eq!(group.exit(), Some(TaskExit::Clean));
    }

    #[test]
    fn integration_mock_lifecycle_connects_disconnects_and_releases_without_real_io() {
        let mut registry = RuntimeRegistry::new_for_tests("20260526");
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        registry
            .configure_tcp(&instance.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap();

        let connected = registry.connect_mock(&instance.handle_id).unwrap();
        assert_eq!(connected.state, InstanceState::Connected);

        let disconnected = registry.disconnect_mock(&instance.handle_id).unwrap();
        assert_eq!(disconnected.state, InstanceState::Disconnected);

        let released = registry
            .release_instance(&instance.handle_id, false)
            .unwrap();
        assert_eq!(released.state, InstanceState::Released);
    }

    #[test]
    fn integration_mock_task_error_records_last_error_and_enters_error_state() {
        let mut registry = RuntimeRegistry::new_for_tests("20260526");
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        registry
            .configure_tcp(&instance.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap();
        registry.connect_mock(&instance.handle_id).unwrap();

        registry
            .record_mock_task_failure(
                &instance.handle_id,
                ErrorCode::ReadIoFailed,
                "mock read failed",
            )
            .unwrap();

        let errored = registry.query_instance(&instance.handle_id).unwrap();
        assert_eq!(errored.state, InstanceState::Error);
        let last_error = errored.last_error.unwrap();
        assert_eq!(last_error.code, ErrorCode::ReadIoFailed);
        assert_eq!(last_error.tool, "mock_task");
    }

    #[test]
    fn integration_force_release_keeps_closing_lock_until_mock_close_completes() {
        let mut registry = RuntimeRegistry::new_for_tests("20260526");
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        registry
            .configure_tcp(
                &instance.handle_id,
                TcpConfig {
                    mode: crate::model::TcpMode::Listen,
                    host: "127.0.0.1".to_owned(),
                    port: 9000,
                    timeout_ms: 1_000,
                },
            )
            .unwrap();
        registry.connect_mock(&instance.handle_id).unwrap();

        let key = ResourceKey::tcp_listen("127.0.0.1", 9000);
        registry
            .release_instance(&instance.handle_id, true)
            .unwrap();
        assert_eq!(
            registry.resource_lock_state(&key),
            Some(ResourceLockState::Closing)
        );

        registry.complete_mock_background_close(&key).unwrap();
        assert_eq!(registry.resource_lock_state(&key), None);
    }

    #[test]
    fn unit_queues_accept_fifo_reject_full_and_restore_budget_on_flush() {
        let mut limits = RuntimeLimits::default();
        limits.tx_queue_max_items = 2;
        limits.tx_frame_max_bytes = 4;
        limits.max_total_queued_bytes = 8;
        let mut registry = RuntimeRegistry::new_for_tests_with_limits("20260526", limits);
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        registry
            .configure_tcp(&instance.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap();
        registry.connect_mock(&instance.handle_id).unwrap();

        assert_eq!(
            registry
                .port_send_mock(&instance.handle_id, b"aa")
                .unwrap()
                .queued,
            true
        );
        assert_eq!(
            registry
                .port_send_mock(&instance.handle_id, b"bb")
                .unwrap()
                .sent_bytes,
            2
        );
        assert_eq!(registry.queued_bytes_for_tests(), 4);

        let full = registry
            .port_send_mock(&instance.handle_id, b"cc")
            .unwrap_err();
        assert_eq!(full.code, ErrorCode::TxQueueFull);
        let too_large = registry
            .port_send_mock(&instance.handle_id, b"hello")
            .unwrap_err();
        assert_eq!(too_large.code, ErrorCode::TxFrameTooLarge);

        let flushed = registry.flush_mock_tx(&instance.handle_id).unwrap();
        assert_eq!(flushed.frames, vec![b"aa".to_vec(), b"bb".to_vec()]);
        assert_eq!(registry.queued_bytes_for_tests(), 0);
        let summary = registry.query_instance(&instance.handle_id).unwrap();
        assert_eq!(summary.stats.tx_bytes, 4);
        assert_eq!(summary.stats.tx_queue_items, 0);
    }

    #[test]
    fn unit_buffers_drop_old_pull_truncate_and_clear_without_touching_subscribers() {
        let mut limits = RuntimeLimits::default();
        limits.rx_buffer_max_bytes = 5;
        limits.pull_max_bytes = 4;
        limits.max_total_buffer_bytes = 5;
        let mut registry = RuntimeRegistry::new_for_tests_with_limits("20260526", limits);
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        registry
            .configure_tcp(&instance.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap();
        registry.connect_mock(&instance.handle_id).unwrap();
        registry
            .subscribe_mock(&instance.handle_id, "session-a", 16)
            .unwrap();

        registry
            .receive_mock_chunk(&instance.handle_id, b"abcdef", 1)
            .unwrap();
        let summary = registry.query_instance(&instance.handle_id).unwrap();
        assert_eq!(summary.stats.rx_buffer_bytes, 5);
        assert_eq!(summary.stats.rx_dropped_bytes, 1);
        assert_eq!(registry.buffer_bytes_for_tests(), 5);

        let pulled = registry
            .port_pull_mock(&instance.handle_id, Some(3))
            .unwrap();
        assert_eq!(pulled.bytes, b"bcd".to_vec());
        assert!(pulled.truncated);
        assert_eq!(pulled.remaining_rx_buffer_bytes, 2);
        assert_eq!(registry.buffer_bytes_for_tests(), 2);

        let cleared = registry
            .port_clear_mock(&instance.handle_id, ClearTarget::Rx)
            .unwrap();
        assert_eq!(cleared.dropped_rx_bytes, 2);
        assert_eq!(registry.buffer_bytes_for_tests(), 0);
        assert_eq!(
            registry
                .pull_subscription_mock(&instance.handle_id, "session-a")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn unit_subscription_isolates_slow_subscribers_and_unsubscribe_is_idempotent() {
        let mut limits = RuntimeLimits::default();
        limits.subscriber_queue_max_items = 1;
        limits.subscriber_payload_max_bytes = 4;
        limits.max_subscribers_per_instance = 2;
        let mut registry = RuntimeRegistry::new_for_tests_with_limits("20260526", limits);
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        registry
            .configure_tcp(&instance.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap();
        registry.connect_mock(&instance.handle_id).unwrap();

        registry
            .subscribe_mock(&instance.handle_id, "session-a", 3)
            .unwrap();
        registry
            .subscribe_mock(&instance.handle_id, "session-b", 4)
            .unwrap();

        let too_many = registry
            .subscribe_mock(&instance.handle_id, "session-c", 4)
            .unwrap_err();
        assert_eq!(too_many.code, ErrorCode::SubscriberLimitExceeded);

        registry
            .receive_mock_chunk(&instance.handle_id, b"first", 1)
            .unwrap();
        registry
            .receive_mock_chunk(&instance.handle_id, b"second", 2)
            .unwrap();

        let a = registry
            .pull_subscription_mock(&instance.handle_id, "session-a")
            .unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].payload, b"sec".to_vec());
        assert!(a[0].truncated);
        assert_eq!(a[0].dropped_notifications, 1);

        let b = registry
            .pull_subscription_mock(&instance.handle_id, "session-b")
            .unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].payload, b"seco".to_vec());

        assert!(
            registry
                .unsubscribe_mock(&instance.handle_id, "session-a")
                .unwrap()
                .was_subscribed
        );
        assert!(
            !registry
                .unsubscribe_mock(&instance.handle_id, "session-a")
                .unwrap()
                .was_subscribed
        );
    }

    #[test]
    fn unit_runtime_budget_rejects_tx_and_rx_restores_after_clear_and_release() {
        let mut limits = RuntimeLimits::default();
        limits.tx_queue_max_items = 4;
        limits.rx_buffer_max_bytes = 10;
        limits.max_total_queued_bytes = 3;
        limits.max_total_buffer_bytes = 4;
        let mut registry = RuntimeRegistry::new_for_tests_with_limits("20260526", limits);
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        registry
            .configure_tcp(&instance.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap();
        registry.connect_mock(&instance.handle_id).unwrap();

        registry
            .port_send_mock(&instance.handle_id, b"abc")
            .unwrap();
        let budget = registry
            .port_send_mock(&instance.handle_id, b"d")
            .unwrap_err();
        assert_eq!(budget.category, ErrorCategory::BufferLimitExceeded);

        registry
            .receive_mock_chunk(&instance.handle_id, b"abcdef", 1)
            .unwrap();
        assert_eq!(registry.buffer_bytes_for_tests(), 4);
        assert_eq!(
            registry
                .query_instance(&instance.handle_id)
                .unwrap()
                .stats
                .rx_dropped_bytes,
            2
        );

        registry
            .port_clear_mock(&instance.handle_id, ClearTarget::All)
            .unwrap();
        assert_eq!(registry.buffer_bytes_for_tests(), 0);
        assert_eq!(registry.queued_bytes_for_tests(), 0);

        registry
            .release_instance(&instance.handle_id, true)
            .unwrap();
        assert_eq!(registry.buffer_bytes_for_tests(), 0);
        assert_eq!(registry.queued_bytes_for_tests(), 0);
    }

    #[test]
    fn unit_notification_rate_coalesces_without_affecting_rx_buffer() {
        let mut limits = RuntimeLimits::default();
        limits.subscriber_queue_max_items = 4;
        limits.subscriber_notifications_per_sec = 1;
        limits.instance_notifications_per_sec = 1;
        limits.global_notifications_per_sec = 1;
        limits.notification_burst = 1;
        let mut registry = RuntimeRegistry::new_for_tests_with_limits("20260526", limits);
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        registry
            .configure_tcp(&instance.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap();
        registry.connect_mock(&instance.handle_id).unwrap();
        registry
            .subscribe_mock(&instance.handle_id, "session-a", 16)
            .unwrap();

        registry
            .receive_mock_chunk(&instance.handle_id, b"one", 1)
            .unwrap();
        registry
            .receive_mock_chunk(&instance.handle_id, b"two", 1)
            .unwrap();

        let notifications = registry
            .pull_subscription_mock(&instance.handle_id, "session-a")
            .unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].payload, b"two".to_vec());
        assert!(notifications[0].coalesced);
        assert_eq!(notifications[0].dropped_notifications, 1);

        let pulled = registry
            .port_pull_mock(&instance.handle_id, Some(6))
            .unwrap();
        assert_eq!(pulled.bytes, b"onetwo".to_vec());
    }

    #[test]
    fn integration_mock_full_covers_send_pull_clear_subscribe_error_and_release() {
        let mut limits = RuntimeLimits::default();
        limits.tx_queue_max_items = 4;
        limits.rx_buffer_max_bytes = 16;
        let mut registry = RuntimeRegistry::new_for_tests_with_limits("20260526", limits);
        let instance = registry.create_instance(InstanceType::Tcp).unwrap();
        registry
            .configure_tcp(&instance.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap();
        registry.connect_mock(&instance.handle_id).unwrap();
        registry
            .subscribe_mock(&instance.handle_id, "session-a", 16)
            .unwrap();

        registry
            .port_send_mock(&instance.handle_id, b"ping")
            .unwrap();
        assert_eq!(
            registry.flush_mock_tx(&instance.handle_id).unwrap().frames,
            vec![b"ping".to_vec()]
        );
        registry
            .receive_mock_chunk(&instance.handle_id, b"pong", 1)
            .unwrap();
        assert_eq!(
            registry
                .port_pull_mock(&instance.handle_id, None)
                .unwrap()
                .bytes,
            b"pong".to_vec()
        );
        assert_eq!(
            registry
                .pull_subscription_mock(&instance.handle_id, "session-a")
                .unwrap()[0]
                .payload,
            b"pong".to_vec()
        );
        registry
            .port_clear_mock(&instance.handle_id, ClearTarget::All)
            .unwrap();

        registry
            .record_mock_task_failure(
                &instance.handle_id,
                ErrorCode::TaskFailed,
                "mock task failed",
            )
            .unwrap();
        assert_eq!(
            registry.query_instance(&instance.handle_id).unwrap().state,
            InstanceState::Error
        );

        let released = registry
            .release_instance(&instance.handle_id, true)
            .unwrap();
        assert_eq!(released.state, InstanceState::Released);
        assert_eq!(registry.buffer_bytes_for_tests(), 0);
        assert_eq!(registry.queued_bytes_for_tests(), 0);
    }

    #[test]
    fn unit_app_instances_delegates_to_registry_and_maps_errors() {
        let mut service = crate::app::InstanceService::new_for_tests("20260526");

        let created = service.create(InstanceType::Tcp).unwrap();
        assert_eq!(service.list().len(), 1);
        assert!(
            service
                .use_instance(Some("session-a"), &created.handle_id)
                .unwrap()
                .is_none()
        );
        service
            .configure_tcp(&created.handle_id, TcpConfig::client("127.0.0.1", 9000))
            .unwrap();

        let queried = service.query(Some(&created.handle_id), None).unwrap();
        assert_eq!(queried.state, InstanceState::Configured);

        let missing_session = service.query(None, None).unwrap_err();
        assert_eq!(missing_session.code, ErrorCode::SessionIdUnavailable);

        let queried_by_session = service.query(None, Some("session-a")).unwrap();
        assert_eq!(
            queried_by_session.handle_id.as_str(),
            created.handle_id.as_str()
        );

        let serial = service.create(InstanceType::Serial).unwrap();
        service
            .configure_serial(&serial.handle_id, SerialConfig::new("COM7"))
            .unwrap();

        service.release(&created.handle_id, false).unwrap();
        let released = service.query(Some(&created.handle_id), None).unwrap_err();
        assert_eq!(released.code, ErrorCode::HandleReleased);
    }
}
