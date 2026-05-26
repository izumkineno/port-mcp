#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use serde_json::json;

use crate::model::{
    ConfigSnapshot, DomainError, ErrorCategory, ErrorCode, HandleId, IdGenerator, InstanceState,
    InstanceStats, InstanceSummary, InstanceType, ResourceSummary, SerialConfig, TcpConfig,
    UdpConfig, validate_instance_type,
};

pub struct RuntimeRegistry {
    instances: HashMap<String, RuntimeInstance>,
    released_handles: HashSet<String>,
    session_bindings: HashMap<String, HandleId>,
    resource_locks: HashMap<ResourceKey, ResourceLockEntry>,
    ids: IdGenerator,
}

impl RuntimeRegistry {
    pub fn new_for_tests(date: &str) -> Self {
        Self {
            instances: HashMap::new(),
            released_handles: HashSet::new(),
            session_bindings: HashMap::new(),
            resource_locks: HashMap::new(),
            ids: IdGenerator::new_for_tests(date),
        }
    }

    pub fn create_instance(
        &mut self,
        instance_type: InstanceType,
    ) -> Result<InstanceSummary, DomainError> {
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
        config.validate_remote()?;
        self.configure(handle_id, InstanceType::Tcp, ConfigSnapshot::Tcp(config))
    }

    pub fn configure_udp(
        &mut self,
        handle_id: &HandleId,
        config: UdpConfig,
    ) -> Result<InstanceSummary, DomainError> {
        self.configure(handle_id, InstanceType::Udp, ConfigSnapshot::Udp(config))
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

        self.instances.remove(handle_id.as_str());
        self.released_handles.insert(handle_id.as_str().to_owned());
        self.session_bindings
            .retain(|_, bound_handle_id| bound_handle_id.as_str() != handle_id.as_str());
        self.release_or_close_owned_locks(handle_id, force);

        Ok(instance.released_summary())
    }

    pub fn acquire_resource_lock(
        &mut self,
        key: ResourceKey,
        owner_handle_id: &HandleId,
    ) -> Result<(), DomainError> {
        if let Some(entry) = self.resource_locks.get(&key) {
            return Err(resource_lock_error(&key, entry));
        }

        self.resource_locks.insert(
            key,
            ResourceLockEntry {
                owner_handle_id: owner_handle_id.clone(),
                state: ResourceLockState::Held,
                generation: 1,
                stale_close: false,
            },
        );
        Ok(())
    }

    pub fn resource_lock_state(&self, key: &ResourceKey) -> Option<ResourceLockState> {
        self.resource_locks.get(key).map(|entry| entry.state)
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
}

#[derive(Debug, Clone)]
pub struct SessionBindingResult {
    pub previous_handle_id: Option<HandleId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceKey(String);

impl ResourceKey {
    pub fn serial(port: &str) -> Self {
        Self(format!("serial:{}", port.trim().to_ascii_uppercase()))
    }

    pub fn tcp_listen(host: &str, port: u16) -> Self {
        Self(format!("tcp-listen:{}:{port}", normalize_host(host)))
    }

    pub fn udp_bind(host: &str, port: u16) -> Self {
        Self(format!("udp-bind:{}:{port}", normalize_host(host)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLockState {
    Held,
    Closing,
    Stale,
}

#[derive(Debug, Clone)]
struct ResourceLockEntry {
    owner_handle_id: HandleId,
    state: ResourceLockState,
    generation: u64,
    stale_close: bool,
}

#[derive(Debug, Clone)]
struct RuntimeInstance {
    handle_id: HandleId,
    instance_type: InstanceType,
    state: InstanceState,
    config: Option<ConfigSnapshot>,
    stats: InstanceStats,
}

impl RuntimeInstance {
    fn new(handle_id: HandleId, instance_type: InstanceType) -> Self {
        Self {
            handle_id,
            instance_type,
            state: InstanceState::Created,
            config: None,
            stats: InstanceStats::default(),
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
            last_error: None,
        }
    }

    fn released_summary(&self) -> InstanceSummary {
        let mut summary = self.to_summary();
        summary.state = InstanceState::Released;
        summary
    }
}

fn resource_summary(config: &ConfigSnapshot) -> ResourceSummary {
    match config {
        ConfigSnapshot::Serial(config) => ResourceSummary::serial(&config.port),
        ConfigSnapshot::Tcp(config) => ResourceSummary::tcp(&config.host, config.port),
        ConfigSnapshot::Udp(config) => ResourceSummary {
            kind: "udp".to_owned(),
            display: format!("{}:{}", config.bind_host, config.bind_port),
        },
    }
}

fn normalize_host(host: &str) -> String {
    let trimmed = host.trim().to_ascii_lowercase();
    if let Some(ipv4) = normalize_ipv4_with_leading_zeroes(&trimmed) {
        return ipv4;
    }
    if let Ok(address) = trimmed.parse::<std::net::IpAddr>() {
        address.to_string()
    } else {
        trimmed
    }
}

fn normalize_ipv4_with_leading_zeroes(host: &str) -> Option<String> {
    let parts = host.split('.').collect::<Vec<_>>();
    if parts.len() != 4 {
        return None;
    }

    let octets = parts
        .iter()
        .map(|part| part.parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some(format!(
        "{}.{}.{}.{}",
        octets[0], octets[1], octets[2], octets[3]
    ))
}

fn resource_lock_error(key: &ResourceKey, entry: &ResourceLockEntry) -> DomainError {
    let code = match entry.state {
        ResourceLockState::Held => resource_busy_code(key),
        ResourceLockState::Closing => ErrorCode::ResourceClosing,
        ResourceLockState::Stale => ErrorCode::ResourceLockStale,
    };
    let message = match entry.state {
        ResourceLockState::Held => "Resource is already owned by another instance.",
        ResourceLockState::Closing => "Resource is still closing from a released instance.",
        ResourceLockState::Stale => "Resource lock is stale and requires operator attention.",
    };
    DomainError::new(
        ErrorCategory::ResourceBusy,
        code,
        message,
        "Release the owning instance, wait for closing to complete, or choose another resource.",
        matches!(entry.state, ResourceLockState::Closing),
    )
    .with_detail(
        "resource",
        json!({
            "kind": resource_kind(key),
            "display": key.as_str()
        }),
    )
    .with_detail("owner_handle_id", json!(entry.owner_handle_id))
}

fn resource_busy_code(key: &ResourceKey) -> ErrorCode {
    if key.as_str().starts_with("serial:") {
        ErrorCode::SerialPortBusy
    } else if key.as_str().starts_with("tcp-listen:") {
        ErrorCode::TcpListenAddrBusy
    } else {
        ErrorCode::UdpBindAddrBusy
    }
}

fn resource_kind(key: &ResourceKey) -> &'static str {
    if key.as_str().starts_with("serial:") {
        "serial"
    } else if key.as_str().starts_with("tcp-listen:") {
        "tcp-listen"
    } else {
        "udp-bind"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ConfigSnapshot, ErrorCategory, ErrorCode, InstanceState, InstanceType, SerialConfig,
        TcpConfig, UdpConfig,
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
