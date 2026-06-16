use std::{collections::BTreeMap, collections::VecDeque, sync::Arc, time::Instant};

use regex::Regex;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    model::{
        DataBits, DomainError, ErrorCategory, ErrorCode, FlowControl, Payload, PayloadEncoding,
        PayloadSummary, RuntimeLimits, SerialConfig, StopBits, VisaConfig,
    },
    transport::{SerialWorker, VisaWorker, scan_serial_ports, scan_visa_resources},
    util::encoding::hex_to_bytes_with_limit,
};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeviceProbeParams {
    pub targets: Vec<ProbeTarget>,
    #[serde(default)]
    pub serial: Option<SerialProbeParams>,
    #[serde(default)]
    pub visa: Option<VisaProbeParams>,
    pub payload: ProbePayloadParams,
    pub matcher: ProbeMatcherParams,
    #[serde(default)]
    pub limits: ProbeLimitsParams,
    #[serde(default)]
    pub failure_output: FailureOutputParam,
    #[serde(default)]
    pub include_failure_samples: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub enum ProbeTarget {
    #[serde(rename = "Serial", alias = "serial", alias = "SERIAL")]
    Serial,
    #[serde(rename = "Visa", alias = "visa", alias = "VISA")]
    Visa,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SerialProbeParams {
    #[serde(default)]
    pub ports: Option<Vec<String>>,
    pub baudrates: Vec<u32>,
    #[serde(default = "default_data_bits")]
    pub data_bits: ProbeDataBits,
    #[serde(default = "default_stop_bits")]
    pub stop_bits: ProbeStopBits,
    #[serde(default)]
    pub parity: ProbeParity,
    #[serde(default)]
    pub flow_control: ProbeFlowControl,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub encoding: ProbeEncoding,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct VisaProbeParams {
    #[serde(default)]
    pub resources: Option<Vec<String>>,
    #[serde(default)]
    pub resource_filter: Option<String>,
    #[serde(default = "default_visa_max_results")]
    pub max_results: usize,
    #[serde(default = "default_timeout_ms")]
    pub open_timeout_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub io_timeout_ms: u64,
    #[serde(default)]
    pub read_termination: Option<String>,
    #[serde(default)]
    pub write_termination: Option<String>,
    #[serde(default)]
    pub encoding: ProbeEncoding,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ProbePayloadParams {
    pub data: String,
    #[serde(default)]
    pub encoding: ProbeEncoding,
    #[serde(default)]
    pub append_line_break: bool,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProbeMatcherParams {
    Contains { value: String },
    HexContains { value: String },
    Regex { value: String },
    HexRegex { value: String },
    AnyResponse,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ProbeLimitsParams {
    #[serde(default = "default_max_resources")]
    pub max_resources: usize,
    #[serde(default = "default_max_candidates_per_resource")]
    pub max_candidates_per_resource: usize,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_payload_max_bytes")]
    pub payload_max_bytes: usize,
    #[serde(default = "default_response_max_bytes")]
    pub response_max_bytes: usize,
    #[serde(default = "default_regex_pattern_max_bytes")]
    pub regex_pattern_max_bytes: usize,
    #[serde(default = "default_total_timeout_ms")]
    pub total_timeout_ms: u64,
    #[serde(default = "default_failure_sample_max")]
    pub failure_sample_max: usize,
    #[serde(default = "default_stop_after_first_success_per_resource")]
    pub stop_after_first_success_per_resource: bool,
    #[serde(default)]
    pub stop_after_first_success_total: bool,
}

impl Default for ProbeLimitsParams {
    fn default() -> Self {
        Self {
            max_resources: default_max_resources(),
            max_candidates_per_resource: default_max_candidates_per_resource(),
            max_concurrency: default_max_concurrency(),
            payload_max_bytes: default_payload_max_bytes(),
            response_max_bytes: default_response_max_bytes(),
            regex_pattern_max_bytes: default_regex_pattern_max_bytes(),
            total_timeout_ms: default_total_timeout_ms(),
            failure_sample_max: default_failure_sample_max(),
            stop_after_first_success_per_resource: default_stop_after_first_success_per_resource(),
            stop_after_first_success_total: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProbeEncoding {
    Text,
    Hex,
}
impl Default for ProbeEncoding {
    fn default() -> Self {
        Self::Text
    }
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
pub enum ProbeDataBits {
    Seven,
    Eight,
}
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
pub enum ProbeStopBits {
    One,
    Two,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
pub enum ProbeParity {
    #[default]
    None,
    Odd,
    Even,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
pub enum ProbeFlowControl {
    #[default]
    None,
    Software,
    Hardware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureOutputParam {
    #[default]
    Counts,
    Samples,
}

#[derive(Debug, Serialize)]
pub struct DeviceProbeResponse {
    pub successes: Vec<ProbeSuccess>,
    pub summary: ProbeSummary,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failure_samples: Vec<ProbeFailureSample>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeSuccess {
    #[serde(rename = "type")]
    pub target_type: ProbeTarget,
    pub resource: String,
    pub config: Value,
    pub matched_by: String,
    pub sent_bytes: usize,
    pub recv_bytes: usize,
    pub response_preview: String,
    pub response_hex: String,
    pub truncated: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ProbeSummary {
    pub resources_discovered: usize,
    pub resources_attempted: usize,
    pub candidates_attempted: usize,
    pub matched_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
    pub timed_out_count: usize,
    pub failure_status_counts: BTreeMap<String, usize>,
    pub failure_error_counts: BTreeMap<String, usize>,
    pub family_errors: Value,
    pub duration_ms: u64,
    pub stopped_after_first_total_success: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeFailureSample {
    #[serde(rename = "type")]
    pub target_type: ProbeTarget,
    pub resource: String,
    pub config: Value,
    pub status: ProbeAttemptStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recv_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_hex: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeAttemptStatus {
    Timeout,
    MatcherMiss,
    OpenError,
    WriteError,
    ReadError,
    Unsupported,
    LimitSkipped,
}

impl ProbeAttemptStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::MatcherMiss => "matcher_miss",
            Self::OpenError => "open_error",
            Self::WriteError => "write_error",
            Self::ReadError => "read_error",
            Self::Unsupported => "unsupported",
            Self::LimitSkipped => "limit_skipped",
        }
    }
}

#[derive(Debug, Clone)]
enum Matcher {
    Contains(String),
    HexContains(Vec<u8>),
    Regex(Regex),
    HexRegex(Regex),
    AnyResponse,
}
impl Matcher {
    fn name(&self) -> &'static str {
        match self {
            Self::Contains(_) => "contains",
            Self::HexContains(_) => "hex_contains",
            Self::Regex(_) => "regex",
            Self::HexRegex(_) => "hex_regex",
            Self::AnyResponse => "any_response",
        }
    }
}

#[derive(Debug, Clone)]
struct ProbeRequest {
    targets: Vec<ProbeTarget>,
    serial: Option<SerialProbeParams>,
    visa: Option<VisaProbeParams>,
    payload: Arc<Vec<u8>>,
    matcher: Arc<Matcher>,
    limits: ProbeLimitsParams,
    failure_output: FailureOutputParam,
}

enum ResourcePlan {
    Serial {
        resource: String,
        candidates: Vec<SerialConfig>,
    },
    Visa {
        resource: String,
        candidates: Vec<VisaConfig>,
    },
}

#[derive(Debug, Default)]
struct ResourceProbeResult {
    successes: Vec<ProbeSuccess>,
    failures: Vec<ProbeFailureSample>,
    attempted: usize,
    timed_out: usize,
    skipped: usize,
}

pub async fn run_device_probe(
    params: DeviceProbeParams,
    runtime_limits: RuntimeLimits,
) -> Result<DeviceProbeResponse, DomainError> {
    let request = validate_request(params, &runtime_limits)?;
    let mut family_errors = serde_json::Map::new();
    let mut resources = Vec::new();

    if request.targets.contains(&ProbeTarget::Serial) {
        match build_serial_plans(&request, &runtime_limits) {
            Ok(mut plans) => resources.append(&mut plans),
            Err(error) => {
                family_errors.insert("Serial".to_owned(), error_to_value(&error));
            }
        }
    }
    if request.targets.contains(&ProbeTarget::Visa) {
        match build_visa_plans(&request, &runtime_limits) {
            Ok(mut plans) => resources.append(&mut plans),
            Err(error) => {
                family_errors.insert("Visa".to_owned(), error_to_value(&error));
            }
        }
    }
    if resources.is_empty() {
        return Err(DomainError::new(
            ErrorCategory::InvalidState,
            ErrorCode::NoDataAvailable,
            "device_probe found no executable resources.",
            "Provide explicit resources or verify resource scanning and feature availability.",
            false,
        )
        .with_detail("family_errors", json!(family_errors)));
    }

    run_resource_plans(resources, request, json!(family_errors), |plan, request| {
        tokio::task::spawn_blocking(move || run_resource_plan(plan, &request))
    })
    .await
}

async fn run_resource_plans(
    resources: Vec<ResourcePlan>,
    request: ProbeRequest,
    family_errors: Value,
    mut spawn_plan: impl FnMut(
        ResourcePlan,
        ProbeRequest,
    ) -> tokio::task::JoinHandle<ResourceProbeResult>,
) -> Result<DeviceProbeResponse, DomainError> {
    let started_at = Instant::now();
    let resources_discovered = resources.len();
    let max_concurrency = request.limits.max_concurrency.min(resources.len()).max(1);
    let mut resources = resources.into_iter();
    let mut handles = VecDeque::new();
    let mut response = DeviceProbeResponse {
        successes: Vec::new(),
        summary: ProbeSummary {
            resources_discovered,
            resources_attempted: 0,
            candidates_attempted: 0,
            matched_count: 0,
            failed_count: 0,
            skipped_count: 0,
            timed_out_count: 0,
            failure_status_counts: BTreeMap::new(),
            failure_error_counts: BTreeMap::new(),
            family_errors,
            duration_ms: 0,
            stopped_after_first_total_success: false,
        },
        failure_samples: Vec::new(),
    };

    loop {
        while handles.len() < max_concurrency {
            let Some(plan) = resources.next() else { break };
            handles.push_back(spawn_plan(plan, request.clone()));
        }
        let Some(handle) = handles.pop_front() else {
            break;
        };
        let result = handle.await.map_err(|error| {
            DomainError::new(
                ErrorCategory::InvalidState,
                ErrorCode::TaskFailed,
                "device_probe resource task failed.",
                "Retry the probe with fewer resources or lower concurrency.",
                true,
            )
            .with_detail("join_error", json!(error.to_string()))
        })?;
        response.summary.resources_attempted += 1;
        response.summary.candidates_attempted += result.attempted;
        response.summary.timed_out_count += result.timed_out;
        response.summary.skipped_count += result.skipped;
        response.summary.failed_count += result.failures.len();
        record_failure_counts(&mut response.summary, &result.failures);
        response.summary.matched_count += result.successes.len();
        response.successes.extend(result.successes);
        if request.failure_output == FailureOutputParam::Samples {
            let remaining = request
                .limits
                .failure_sample_max
                .saturating_sub(response.failure_samples.len());
            response
                .failure_samples
                .extend(result.failures.into_iter().take(remaining));
        }
        if request.limits.stop_after_first_success_total && !response.successes.is_empty() {
            response.summary.stopped_after_first_total_success = true;
            break;
        }
    }
    for handle in handles {
        let _ = handle.await;
    }
    response.summary.duration_ms = elapsed_ms(started_at);
    Ok(response)
}

fn validate_request(
    params: DeviceProbeParams,
    runtime_limits: &RuntimeLimits,
) -> Result<ProbeRequest, DomainError> {
    if params.targets.is_empty() {
        return Err(invalid_field(
            "targets",
            "device_probe targets must not be empty.",
        ));
    }
    validate_limits(&params.limits, runtime_limits)?;
    let payload = match params.payload.encoding {
        ProbeEncoding::Text => {
            Payload::from_text(&params.payload.data, params.payload.append_line_break)?
        }
        ProbeEncoding::Hex => {
            Payload::from_hex(&params.payload.data, params.payload.append_line_break)?
        }
    };
    if payload.bytes.is_empty() {
        return Err(invalid_field(
            "payload.data",
            "device_probe payload must not be empty.",
        ));
    }
    if payload.bytes.len() > params.limits.payload_max_bytes {
        return Err(DomainError::new(
            ErrorCategory::BufferLimitExceeded,
            ErrorCode::TxFrameTooLarge,
            "device_probe payload exceeds payload_max_bytes.",
            "Reduce payload size or raise payload_max_bytes within the allowed hard limit.",
            false,
        )
        .with_detail("field", json!("payload.data"))
        .with_detail("limit", json!(params.limits.payload_max_bytes))
        .with_detail("actual", json!(payload.bytes.len())));
    }
    runtime_limits.validate_tx_frame_len(payload.bytes.len())?;
    let matcher = validate_matcher(
        params.matcher,
        runtime_limits,
        params.limits.regex_pattern_max_bytes,
    )?;
    if params.targets.contains(&ProbeTarget::Serial) {
        let serial = params.serial.as_ref().ok_or_else(|| {
            DomainError::invalid_argument(
                ErrorCode::MissingRequiredField,
                "device_probe serial config is required when targets contains Serial.",
                "Pass serial with at least a baudrates list.",
            )
            .with_detail("field", json!("serial"))
        })?;
        if serial.baudrates.is_empty() {
            return Err(invalid_field(
                "serial.baudrates",
                "serial baudrates must not be empty.",
            ));
        }
        runtime_limits.validate_io_timeout_ms("serial.timeout_ms", serial.timeout_ms)?;
    }
    if params.targets.contains(&ProbeTarget::Visa) {
        if let Some(visa) = &params.visa {
            runtime_limits.validate_io_timeout_ms("visa.open_timeout_ms", visa.open_timeout_ms)?;
            runtime_limits.validate_io_timeout_ms("visa.io_timeout_ms", visa.io_timeout_ms)?;
        }
    }
    Ok(ProbeRequest {
        targets: params.targets,
        serial: params.serial,
        visa: params.visa,
        payload: Arc::new(payload.bytes),
        matcher: Arc::new(matcher),
        limits: params.limits,
        failure_output: if params.include_failure_samples {
            FailureOutputParam::Samples
        } else {
            params.failure_output
        },
    })
}

fn record_failure_counts(summary: &mut ProbeSummary, failures: &[ProbeFailureSample]) {
    for failure in failures {
        *summary
            .failure_status_counts
            .entry(failure.status.as_str().to_owned())
            .or_insert(0) += 1;
        if let Some(error_code) = failure.error_code {
            *summary
                .failure_error_counts
                .entry(error_code_name(error_code))
                .or_insert(0) += 1;
        }
    }
}

fn validate_limits(
    limits: &ProbeLimitsParams,
    runtime_limits: &RuntimeLimits,
) -> Result<(), DomainError> {
    validate_limit_range("limits.max_resources", limits.max_resources, 1, 128)?;
    validate_limit_range(
        "limits.max_candidates_per_resource",
        limits.max_candidates_per_resource,
        1,
        64,
    )?;
    validate_limit_range("limits.max_concurrency", limits.max_concurrency, 1, 32)?;
    validate_limit_range(
        "limits.payload_max_bytes",
        limits.payload_max_bytes,
        1,
        runtime_limits.tx_frame_max_bytes,
    )?;
    validate_limit_range(
        "limits.response_max_bytes",
        limits.response_max_bytes,
        1,
        runtime_limits.pull_max_bytes,
    )?;
    validate_limit_range(
        "limits.regex_pattern_max_bytes",
        limits.regex_pattern_max_bytes,
        1,
        4 * 1_024,
    )?;
    validate_limit_range(
        "limits.total_timeout_ms",
        limits.total_timeout_ms as usize,
        1,
        runtime_limits.scan_total_timeout_ms as usize,
    )?;
    validate_limit_range(
        "limits.failure_sample_max",
        limits.failure_sample_max,
        0,
        64,
    )
}

fn validate_limit_range(
    field: &'static str,
    actual: usize,
    min: usize,
    max: usize,
) -> Result<(), DomainError> {
    if (min..=max).contains(&actual) {
        Ok(())
    } else {
        Err(DomainError::invalid_argument(
            ErrorCode::InvalidRange,
            format!("{field} is outside the allowed range."),
            format!("Use a value between {min} and {max}."),
        )
        .with_detail("field", json!(field))
        .with_detail("min", json!(min))
        .with_detail("max", json!(max))
        .with_detail("actual", json!(actual)))
    }
}

fn validate_matcher(
    matcher: ProbeMatcherParams,
    runtime_limits: &RuntimeLimits,
    regex_pattern_max_bytes: usize,
) -> Result<Matcher, DomainError> {
    match matcher {
        ProbeMatcherParams::Contains { value } => {
            if value.is_empty() {
                Err(invalid_field(
                    "matcher.value",
                    "contains matcher value must not be empty.",
                ))
            } else {
                Ok(Matcher::Contains(value))
            }
        }
        ProbeMatcherParams::HexContains { value } => {
            if value.is_empty() {
                Err(invalid_field(
                    "matcher.value",
                    "hex_contains matcher value must not be empty.",
                ))
            } else {
                Ok(Matcher::HexContains(hex_to_bytes_with_limit(
                    &value,
                    "matcher.value",
                    runtime_limits.pull_max_bytes,
                )?))
            }
        }
        ProbeMatcherParams::Regex { value } => {
            compile_probe_regex(value, regex_pattern_max_bytes, "regex").map(Matcher::Regex)
        }
        ProbeMatcherParams::HexRegex { value } => {
            compile_probe_regex(value, regex_pattern_max_bytes, "hex_regex").map(Matcher::HexRegex)
        }
        ProbeMatcherParams::AnyResponse => Ok(Matcher::AnyResponse),
    }
}

fn compile_probe_regex(
    value: String,
    regex_pattern_max_bytes: usize,
    matcher_name: &str,
) -> Result<Regex, DomainError> {
    if value.is_empty() {
        return Err(invalid_field(
            "matcher.value",
            "regex matcher value must not be empty.",
        ));
    }
    if value.len() > regex_pattern_max_bytes {
        return Err(DomainError::invalid_argument(
            ErrorCode::InvalidRange,
            format!("{matcher_name} matcher value exceeds regex_pattern_max_bytes."),
            "Shorten the regex pattern or raise regex_pattern_max_bytes within the allowed hard limit.",
        )
        .with_detail("field", json!("matcher.value"))
        .with_detail("limit", json!(regex_pattern_max_bytes))
        .with_detail("actual", json!(value.len())));
    }
    Regex::new(&value).map_err(|error| {
        DomainError::invalid_argument(
            ErrorCode::ProtocolFrameInvalid,
            format!("{matcher_name} matcher value is not a valid Rust regex pattern."),
            "Fix the regex pattern and retry. Rust regex does not support look-around or backreferences.",
        )
        .with_detail("field", json!("matcher.value"))
        .with_detail("raw_error", json!(error.to_string()))
    })
}

fn build_serial_plans(
    request: &ProbeRequest,
    runtime_limits: &RuntimeLimits,
) -> Result<Vec<ResourcePlan>, DomainError> {
    let serial = request.serial.as_ref().expect("serial params validated");
    let ports: Vec<String> = match &serial.ports {
        Some(ports) => ports.clone(),
        None => scan_serial_ports()
            .map_err(transport_error_to_domain)?
            .into_iter()
            .map(|summary| summary.name)
            .collect(),
    };
    if ports.len() > request.limits.max_resources || ports.len() > runtime_limits.scan_max_ports {
        return Err(DomainError::new(
            ErrorCategory::BufferLimitExceeded,
            ErrorCode::ScanRangeTooLarge,
            "device_probe serial resources exceed max_resources.",
            "Reduce serial.ports or max_results before probing.",
            false,
        )
        .with_detail("field", json!("serial.ports"))
        .with_detail(
            "limit",
            json!(
                request
                    .limits
                    .max_resources
                    .min(runtime_limits.scan_max_ports)
            ),
        )
        .with_detail("actual", json!(ports.len())));
    }
    Ok(ports
        .into_iter()
        .map(|port| {
            let candidates = serial
                .baudrates
                .iter()
                .take(request.limits.max_candidates_per_resource)
                .map(|baudrate| {
                    let mut config = SerialConfig::new(&port);
                    config.baudrate = *baudrate;
                    config.data_bits = map_data_bits(serial.data_bits);
                    config.stop_bits = map_stop_bits(serial.stop_bits);
                    config.parity = map_parity(serial.parity);
                    config.flow_control = map_flow_control(serial.flow_control);
                    config.timeout_ms = serial.timeout_ms;
                    config.encoding = map_encoding(serial.encoding);
                    config
                })
                .collect();
            ResourcePlan::Serial {
                resource: port,
                candidates,
            }
        })
        .collect())
}

fn build_visa_plans(
    request: &ProbeRequest,
    runtime_limits: &RuntimeLimits,
) -> Result<Vec<ResourcePlan>, DomainError> {
    let visa = request.visa.clone().unwrap_or_else(|| VisaProbeParams {
        resources: None,
        resource_filter: None,
        max_results: default_visa_max_results(),
        open_timeout_ms: default_timeout_ms(),
        io_timeout_ms: default_timeout_ms(),
        read_termination: None,
        write_termination: None,
        encoding: ProbeEncoding::Text,
    });
    let resources: Vec<String> = match &visa.resources {
        Some(resources) => resources.clone(),
        None => scan_visa_resources(
            visa.resource_filter.as_deref().unwrap_or("?*INSTR"),
            visa.max_results.min(request.limits.max_resources),
        )?
        .resources
        .into_iter()
        .map(|summary| summary.resource_address)
        .collect(),
    };
    if resources.len() > request.limits.max_resources {
        return Err(DomainError::new(
            ErrorCategory::BufferLimitExceeded,
            ErrorCode::ScanRangeTooLarge,
            "device_probe VISA resources exceed max_resources.",
            "Reduce visa.resources or visa.max_results before probing.",
            false,
        )
        .with_detail("field", json!("visa.resources"))
        .with_detail("limit", json!(request.limits.max_resources))
        .with_detail("actual", json!(resources.len())));
    }
    resources
        .into_iter()
        .map(|resource| {
            let mut config = VisaConfig::new(&resource);
            config.open_timeout_ms = visa.open_timeout_ms;
            config.io_timeout_ms = visa.io_timeout_ms;
            config.read_termination = visa.read_termination.clone();
            config.write_termination = visa.write_termination.clone();
            config.encoding = map_encoding(visa.encoding);
            config.validate(runtime_limits)?;
            Ok(ResourcePlan::Visa {
                resource,
                candidates: vec![config],
            })
        })
        .collect()
}

fn run_resource_plan(plan: ResourcePlan, request: &ProbeRequest) -> ResourceProbeResult {
    match plan {
        ResourcePlan::Serial {
            resource,
            candidates,
        } => run_serial_resource(&resource, &candidates, request),
        ResourcePlan::Visa {
            resource,
            candidates,
        } => run_visa_resource(&resource, &candidates, request),
    }
}

fn run_serial_resource(
    resource: &str,
    candidates: &[SerialConfig],
    request: &ProbeRequest,
) -> ResourceProbeResult {
    run_serial_resource_with_attempt(resource, candidates, request, probe_serial_candidate)
}

fn run_serial_resource_with_attempt(
    resource: &str,
    candidates: &[SerialConfig],
    request: &ProbeRequest,
    mut attempt: impl FnMut(&SerialConfig, &[u8], usize) -> CandidateAttempt,
) -> ResourceProbeResult {
    let mut result = ResourceProbeResult::default();
    for config in candidates {
        result.attempted += 1;
        let attempt_started = Instant::now();
        match attempt(config, &request.payload, request.limits.response_max_bytes) {
            CandidateAttempt::Response {
                sent_bytes,
                response,
            } if matches_response(&request.matcher, &response) => {
                result.successes.push(success(
                    ProbeTarget::Serial,
                    resource,
                    json!(config),
                    request.matcher.name(),
                    sent_bytes,
                    &response,
                    request.limits.response_max_bytes,
                    elapsed_ms(attempt_started),
                ));
                if request.limits.stop_after_first_success_per_resource {
                    break;
                }
            }
            CandidateAttempt::Response { response, .. } => result.failures.push(failure_sample(
                ProbeTarget::Serial,
                resource,
                json!(config),
                ProbeAttemptStatus::MatcherMiss,
                None,
                Some("response did not match probe matcher".to_owned()),
                Some("Use response_hex/response_preview to refine the matcher, or use hex_regex for binary protocols.".to_owned()),
                Some(false),
                Some(&response),
                request.limits.response_max_bytes,
            )),
            CandidateAttempt::Error { status, error } => {
                if error.code == ErrorCode::ReadTimeout {
                    result.timed_out += 1;
                }
                result.failures.push(error_failure(
                    ProbeTarget::Serial,
                    resource,
                    json!(config),
                    status,
                    error,
                ));
            }
        }
    }
    result
}

enum CandidateAttempt {
    Response {
        sent_bytes: usize,
        response: Vec<u8>,
    },
    Error {
        status: ProbeAttemptStatus,
        error: DomainError,
    },
}

fn probe_serial_candidate(
    config: &SerialConfig,
    payload: &[u8],
    response_max_bytes: usize,
) -> CandidateAttempt {
    match SerialWorker::open(config) {
        Ok(worker) => {
            probe_serial_candidate_with_worker(worker, config, payload, response_max_bytes)
        }
        Err(error) => CandidateAttempt::Error {
            status: ProbeAttemptStatus::OpenError,
            error: transport_error_to_domain(error),
        },
    }
}

fn probe_serial_candidate_with_worker(
    worker: SerialWorker,
    config: &SerialConfig,
    payload: &[u8],
    response_max_bytes: usize,
) -> CandidateAttempt {
    match worker.write(payload, config.timeout_ms) {
        Ok(sent_bytes) => {
            match read_serial_response(&worker, response_max_bytes, config.timeout_ms) {
                Ok(response) => {
                    let _ = worker.close(config.timeout_ms);
                    CandidateAttempt::Response {
                        sent_bytes,
                        response,
                    }
                }
                Err(error) => {
                    let _ = worker.close(config.timeout_ms);
                    CandidateAttempt::Error {
                        status: ProbeAttemptStatus::ReadError,
                        error: transport_error_to_domain(error),
                    }
                }
            }
        }
        Err(error) => {
            let _ = worker.close(config.timeout_ms);
            CandidateAttempt::Error {
                status: ProbeAttemptStatus::WriteError,
                error: transport_error_to_domain(error),
            }
        }
    }
}

fn read_serial_response(
    worker: &SerialWorker,
    response_max_bytes: usize,
    timeout_ms: u64,
) -> Result<Vec<u8>, crate::transport::TransportError> {
    let mut response = Vec::new();
    loop {
        let remaining = response_max_bytes.saturating_sub(response.len());
        if remaining == 0 {
            return Ok(response);
        }
        match worker.read(remaining, timeout_ms) {
            Ok(chunk) => response.extend_from_slice(&chunk),
            Err(error) if error.code == ErrorCode::ReadTimeout && !response.is_empty() => {
                return Ok(response);
            }
            Err(error) => return Err(error),
        }
    }
}

fn read_visa_response(
    mut read: impl FnMut() -> Result<Vec<u8>, DomainError>,
    response_max_bytes: usize,
) -> Result<Vec<u8>, DomainError> {
    let mut response = Vec::new();
    loop {
        if response.len() >= response_max_bytes {
            response.truncate(response_max_bytes);
            return Ok(response);
        }
        match read() {
            Ok(chunk) => {
                if chunk.is_empty() {
                    return Ok(response);
                }
                response.extend_from_slice(&chunk);
            }
            Err(error) if error.code == ErrorCode::ReadTimeout && !response.is_empty() => {
                return Ok(response);
            }
            Err(error) => return Err(error),
        }
    }
}

fn run_visa_resource(
    resource: &str,
    candidates: &[VisaConfig],
    request: &ProbeRequest,
) -> ResourceProbeResult {
    let mut result = ResourceProbeResult::default();
    for config in candidates {
        result.attempted += 1;
        let attempt_started = Instant::now();
        match VisaWorker::open(config) {
            Ok(worker) => {
                let _ = worker.clear();
                match worker.write(&request.payload) {
                    Ok(sent_bytes) => match read_visa_response(
                        || worker.read(request.limits.response_max_bytes),
                        request.limits.response_max_bytes,
                    ) {
                        Ok(response) if matches_response(&request.matcher, &response) => {
                            result.successes.push(success(
                                ProbeTarget::Visa,
                                resource,
                                json!(config),
                                request.matcher.name(),
                                sent_bytes,
                                &response,
                                request.limits.response_max_bytes,
                                elapsed_ms(attempt_started),
                            ));
                            let _ = worker.close();
                            if request.limits.stop_after_first_success_per_resource {
                                break;
                            }
                        }
                        Ok(response) => {
                            result.failures.push(failure_sample(
                                ProbeTarget::Visa,
                                resource,
                                json!(config),
                                ProbeAttemptStatus::MatcherMiss,
                                None,
                                Some("response did not match probe matcher".to_owned()),
                                Some("Use response_hex/response_preview to refine the matcher, or use hex_regex for binary protocols.".to_owned()),
                                Some(false),
                                Some(&response),
                                request.limits.response_max_bytes,
                            ));
                            let _ = worker.close();
                        }
                        Err(error) => {
                            let status = if error.code == ErrorCode::ReadTimeout {
                                result.timed_out += 1;
                                ProbeAttemptStatus::Timeout
                            } else {
                                ProbeAttemptStatus::ReadError
                            };
                            result.failures.push(error_failure(
                                ProbeTarget::Visa,
                                resource,
                                json!(config),
                                status,
                                error,
                            ));
                            let _ = worker.close();
                        }
                    },
                    Err(error) => {
                        result.failures.push(error_failure(
                            ProbeTarget::Visa,
                            resource,
                            json!(config),
                            ProbeAttemptStatus::WriteError,
                            error,
                        ));
                        let _ = worker.close();
                    }
                }
            }
            Err(error) => {
                let status = if error.code == ErrorCode::FeatureNotCompiled {
                    result.skipped += 1;
                    ProbeAttemptStatus::Unsupported
                } else {
                    ProbeAttemptStatus::OpenError
                };
                result.failures.push(error_failure(
                    ProbeTarget::Visa,
                    resource,
                    json!(config),
                    status,
                    error,
                ));
            }
        }
    }
    result
}

fn matches_response(matcher: &Matcher, response: &[u8]) -> bool {
    match matcher {
        Matcher::Contains(value) => String::from_utf8_lossy(response).contains(value),
        Matcher::Regex(regex) => regex.is_match(&String::from_utf8_lossy(response)),
        Matcher::HexRegex(regex) => {
            let hex = hex_preview(response, response.len());
            regex.is_match(&hex) || regex.is_match(&hex.to_ascii_uppercase())
        }
        Matcher::HexContains(needle) => response
            .windows(needle.len())
            .any(|window| window == needle),
        Matcher::AnyResponse => !response.is_empty(),
    }
}

fn hex_preview(response: &[u8], max_preview_bytes: usize) -> String {
    PayloadSummary::from_bytes(response, PayloadEncoding::Hex, max_preview_bytes, false).preview
}

fn success(
    target_type: ProbeTarget,
    resource: &str,
    config: Value,
    matched_by: &str,
    sent_bytes: usize,
    response: &[u8],
    max_preview_bytes: usize,
    duration_ms: u64,
) -> ProbeSuccess {
    let text =
        PayloadSummary::from_bytes(response, PayloadEncoding::Text, max_preview_bytes, false);
    let hex = PayloadSummary::from_bytes(response, PayloadEncoding::Hex, max_preview_bytes, false);
    ProbeSuccess {
        target_type,
        resource: resource.to_owned(),
        config,
        matched_by: matched_by.to_owned(),
        sent_bytes,
        recv_bytes: response.len(),
        response_preview: text.preview,
        response_hex: hex.preview,
        truncated: text.truncated || hex.truncated,
        duration_ms,
    }
}

fn failure_sample(
    target_type: ProbeTarget,
    resource: &str,
    config: Value,
    status: ProbeAttemptStatus,
    error_code: Option<ErrorCode>,
    message: Option<String>,
    recovery_hint: Option<String>,
    retryable: Option<bool>,
    response: Option<&[u8]>,
    max_preview_bytes: usize,
) -> ProbeFailureSample {
    let text_summary = response.map(|bytes| {
        PayloadSummary::from_bytes(bytes, PayloadEncoding::Text, max_preview_bytes, false)
    });
    let hex_summary = response.map(|bytes| {
        PayloadSummary::from_bytes(bytes, PayloadEncoding::Hex, max_preview_bytes, false)
    });
    ProbeFailureSample {
        target_type,
        resource: resource.to_owned(),
        config,
        status,
        error_code,
        message,
        recovery_hint,
        retryable,
        recv_bytes: response.map(<[u8]>::len),
        response_preview: text_summary.as_ref().map(|item| item.preview.clone()),
        response_hex: hex_summary.as_ref().map(|item| item.preview.clone()),
        truncated: text_summary.is_some_and(|item| item.truncated)
            || hex_summary.is_some_and(|item| item.truncated),
    }
}

fn error_failure(
    target_type: ProbeTarget,
    resource: &str,
    config: Value,
    status: ProbeAttemptStatus,
    error: DomainError,
) -> ProbeFailureSample {
    failure_sample(
        target_type,
        resource,
        config,
        status,
        Some(error.code),
        Some(error.message),
        Some(error.recovery_hint),
        Some(error.retryable),
        None,
        0,
    )
}

fn transport_error_to_domain(error: crate::transport::TransportError) -> DomainError {
    let recovery_hint = if error.code == ErrorCode::InvalidAddress
        && error.message.contains("serial port was not found")
    {
        "For serial probe this can be a transient open/enumeration result under concurrent probing; rerun port_scan, retry the specific port, or lower max_concurrency."
    } else {
        "Check the resource and retry with adjusted probe settings."
    };
    DomainError::new(
        error.category,
        error.code,
        error.message,
        recovery_hint,
        !error.fatal,
    )
}

fn error_to_value(error: &DomainError) -> Value {
    json!({ "category": error.category, "code": error.code, "message": error.message, "recovery_hint": error.recovery_hint, "details": error.details })
}

fn invalid_field(field: &'static str, message: &'static str) -> DomainError {
    DomainError::invalid_argument(
        ErrorCode::InvalidRange,
        message,
        "Correct the field and retry.",
    )
    .with_detail("field", json!(field))
}

fn error_code_name(error_code: ErrorCode) -> String {
    serde_json::to_value(error_code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{error_code:?}"))
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn map_encoding(value: ProbeEncoding) -> PayloadEncoding {
    match value {
        ProbeEncoding::Text => PayloadEncoding::Text,
        ProbeEncoding::Hex => PayloadEncoding::Hex,
    }
}
fn map_data_bits(value: ProbeDataBits) -> DataBits {
    match value {
        ProbeDataBits::Seven => DataBits::Seven,
        ProbeDataBits::Eight => DataBits::Eight,
    }
}
fn map_stop_bits(value: ProbeStopBits) -> StopBits {
    match value {
        ProbeStopBits::One => StopBits::One,
        ProbeStopBits::Two => StopBits::Two,
    }
}
fn map_parity(value: ProbeParity) -> crate::model::Parity {
    match value {
        ProbeParity::None => crate::model::Parity::None,
        ProbeParity::Odd => crate::model::Parity::Odd,
        ProbeParity::Even => crate::model::Parity::Even,
    }
}
fn map_flow_control(value: ProbeFlowControl) -> FlowControl {
    match value {
        ProbeFlowControl::None => FlowControl::None,
        ProbeFlowControl::Software => FlowControl::Software,
        ProbeFlowControl::Hardware => FlowControl::Hardware,
    }
}

fn default_data_bits() -> ProbeDataBits {
    ProbeDataBits::Eight
}
fn default_stop_bits() -> ProbeStopBits {
    ProbeStopBits::One
}
fn default_timeout_ms() -> u64 {
    1_000
}
fn default_visa_max_results() -> usize {
    128
}
fn default_max_resources() -> usize {
    32
}
fn default_max_candidates_per_resource() -> usize {
    16
}
fn default_max_concurrency() -> usize {
    8
}
fn default_payload_max_bytes() -> usize {
    1_024
}
fn default_response_max_bytes() -> usize {
    4 * 1_024
}
fn default_regex_pattern_max_bytes() -> usize {
    1_024
}
fn default_total_timeout_ms() -> u64 {
    10_000
}
fn default_failure_sample_max() -> usize {
    8
}
fn default_stop_after_first_success_per_resource() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_probe_matcher_covers_text_hex_and_any_response() {
        assert!(matches_response(
            &Matcher::Contains("OK".to_owned()),
            b":RESIOK\n"
        ));
        assert!(!matches_response(
            &Matcher::Contains("ok".to_owned()),
            b":RESIOK\n"
        ));
        assert!(matches_response(
            &Matcher::HexContains(vec![0x52, 0x45]),
            b":RESIOK\n"
        ));
        let regex = Regex::new(r"RESI(OK|READY)").unwrap();
        assert!(matches_response(&Matcher::Regex(regex), b":RESIOK\n"));
        let hex_regex = Regex::new(r"^010302[0-9A-F]+$").unwrap();
        assert!(matches_response(
            &Matcher::HexRegex(hex_regex),
            &[0x01, 0x03, 0x02, 0x01, 0x0B, 0xE5, 0xEC]
        ));
        assert!(matches_response(&Matcher::AnyResponse, b"x"));
        assert!(!matches_response(&Matcher::AnyResponse, b""));
    }

    #[test]
    fn unit_probe_validation_accepts_regex_and_rejects_invalid_regex() {
        let matcher = validate_matcher(
            ProbeMatcherParams::Regex {
                value: r"RESI(OK|READY)".to_owned(),
            },
            &RuntimeLimits::default(),
            default_regex_pattern_max_bytes(),
        )
        .unwrap();
        assert_eq!(matcher.name(), "regex");

        let matcher = validate_matcher(
            ProbeMatcherParams::HexRegex {
                value: r"^010302[0-9A-F]+$".to_owned(),
            },
            &RuntimeLimits::default(),
            default_regex_pattern_max_bytes(),
        )
        .unwrap();
        assert_eq!(matcher.name(), "hex_regex");

        let invalid = validate_matcher(
            ProbeMatcherParams::Regex {
                value: "(".to_owned(),
            },
            &RuntimeLimits::default(),
            default_regex_pattern_max_bytes(),
        )
        .unwrap_err();
        assert_eq!(invalid.code, ErrorCode::ProtocolFrameInvalid);
    }

    #[test]
    fn unit_probe_validation_rejects_empty_targets_payload_and_matcher() {
        let invalid = DeviceProbeParams {
            targets: Vec::new(),
            serial: None,
            visa: None,
            payload: ProbePayloadParams {
                data: "ping".to_owned(),
                encoding: ProbeEncoding::Text,
                append_line_break: false,
            },
            matcher: ProbeMatcherParams::AnyResponse,
            limits: ProbeLimitsParams::default(),
            failure_output: FailureOutputParam::Counts,
            include_failure_samples: false,
        };
        assert_eq!(
            validate_request(invalid, &RuntimeLimits::default())
                .unwrap_err()
                .code,
            ErrorCode::InvalidRange
        );

        let invalid = DeviceProbeParams {
            targets: vec![ProbeTarget::Serial],
            serial: Some(SerialProbeParams {
                ports: Some(vec!["COM3".to_owned()]),
                baudrates: vec![9_600],
                data_bits: ProbeDataBits::Eight,
                stop_bits: ProbeStopBits::One,
                parity: ProbeParity::None,
                flow_control: ProbeFlowControl::None,
                timeout_ms: 1_000,
                encoding: ProbeEncoding::Text,
            }),
            visa: None,
            payload: ProbePayloadParams {
                data: String::new(),
                encoding: ProbeEncoding::Text,
                append_line_break: false,
            },
            matcher: ProbeMatcherParams::Contains {
                value: String::new(),
            },
            limits: ProbeLimitsParams::default(),
            failure_output: FailureOutputParam::Counts,
            include_failure_samples: false,
        };
        assert_eq!(
            validate_request(invalid, &RuntimeLimits::default())
                .unwrap_err()
                .code,
            ErrorCode::InvalidRange
        );
    }

    #[test]
    fn unit_probe_serial_executor_stops_after_first_matching_candidate() {
        let request = ProbeRequest {
            targets: vec![ProbeTarget::Serial],
            serial: None,
            visa: None,
            payload: Arc::new(b"ping".to_vec()),
            matcher: Arc::new(Matcher::Contains("pong".to_owned())),
            limits: ProbeLimitsParams::default(),
            failure_output: FailureOutputParam::Samples,
        };
        let mut first = SerialConfig::new("COM9");
        first.baudrate = 9_600;
        let mut second = SerialConfig::new("COM9");
        second.baudrate = 115_200;
        let candidates = vec![first, second];
        let mut attempts = 0;
        let result = run_serial_resource_with_attempt(
            "COM9",
            &candidates,
            &request,
            |_config, payload, _response_max_bytes| {
                attempts += 1;
                assert_eq!(payload, b"ping");
                CandidateAttempt::Response {
                    sent_bytes: payload.len(),
                    response: b"pong".to_vec(),
                }
            },
        );
        assert_eq!(attempts, 1);
        assert_eq!(result.attempted, 1);
        assert_eq!(result.successes.len(), 1);
        assert_eq!(result.successes[0].resource, "COM9");
        assert_eq!(result.successes[0].matched_by, "contains");
        assert_eq!(result.successes[0].config["baudrate"], 9_600);
    }

    #[tokio::test]
    async fn unit_probe_dispatch_stops_scheduling_after_first_total_success() {
        let request = ProbeRequest {
            targets: vec![ProbeTarget::Serial],
            serial: None,
            visa: None,
            payload: Arc::new(b"ping".to_vec()),
            matcher: Arc::new(Matcher::AnyResponse),
            limits: ProbeLimitsParams {
                max_concurrency: 1,
                stop_after_first_success_total: true,
                ..ProbeLimitsParams::default()
            },
            failure_output: FailureOutputParam::Counts,
        };
        let plans = vec![
            ResourcePlan::Serial {
                resource: "COM1".to_owned(),
                candidates: Vec::new(),
            },
            ResourcePlan::Serial {
                resource: "COM2".to_owned(),
                candidates: Vec::new(),
            },
        ];
        let dispatched = Arc::new(std::sync::Mutex::new(Vec::new()));

        let response = run_resource_plans(plans, request, json!({}), |plan, _request| {
            let dispatched = Arc::clone(&dispatched);
            let resource = match plan {
                ResourcePlan::Serial { resource, .. } => resource,
                ResourcePlan::Visa { resource, .. } => resource,
            };
            tokio::spawn(async move {
                dispatched.lock().unwrap().push(resource.clone());
                ResourceProbeResult {
                    successes: vec![success(
                        ProbeTarget::Serial,
                        &resource,
                        json!({}),
                        "any_response",
                        4,
                        b"pong",
                        16,
                        0,
                    )],
                    ..ResourceProbeResult::default()
                }
            })
        })
        .await
        .unwrap();

        assert_eq!(*dispatched.lock().unwrap(), vec!["COM1"]);
        assert_eq!(response.summary.resources_attempted, 1);
        assert!(response.summary.stopped_after_first_total_success);
    }

    #[test]
    fn unit_probe_failure_sample_includes_hex_response_preview() {
        let sample = failure_sample(
            ProbeTarget::Serial,
            "COM9",
            json!({ "baudrate": 9600 }),
            ProbeAttemptStatus::MatcherMiss,
            None,
            Some("response did not match probe matcher".to_owned()),
            Some("Use response_hex/response_preview to refine the matcher, or use hex_regex for binary protocols.".to_owned()),
            Some(false),
            Some(&[0x01, 0x03, 0x02, 0x01, 0x0B, 0xE5, 0xEC]),
            32,
        );

        assert_eq!(sample.recv_bytes, Some(7));
        assert_eq!(sample.response_hex.as_deref(), Some("010302010be5ec"));
        assert_eq!(sample.retryable, Some(false));
    }

    #[test]
    fn unit_probe_serial_not_found_open_error_is_reported_as_retryable() {
        let error = transport_error_to_domain(crate::transport::TransportError::invalid_address(
            "serial port was not found",
        ));
        let sample = error_failure(
            ProbeTarget::Serial,
            "COM9",
            json!({ "baudrate": 9600 }),
            ProbeAttemptStatus::OpenError,
            error,
        );

        assert_eq!(sample.error_code, Some(ErrorCode::InvalidAddress));
        assert_eq!(sample.retryable, Some(true));
        assert!(
            sample
                .recovery_hint
                .as_deref()
                .unwrap()
                .contains("transient open/enumeration")
        );
    }

    #[test]
    fn unit_probe_serial_candidate_aggregates_chunked_response_until_match() {
        let mut config = SerialConfig::new("COM9");
        config.timeout_ms = 1_000;
        let worker =
            crate::transport::serial_worker_for_tests(vec![b"P".to_vec(), b"ONG".to_vec()]);

        let attempt = probe_serial_candidate_with_worker(worker, &config, b"ping", 16);

        match attempt {
            CandidateAttempt::Response { response, .. } => assert_eq!(response, b"PONG"),
            CandidateAttempt::Error { error, .. } => panic!("unexpected probe error: {error:?}"),
        }
    }

    #[test]
    fn unit_probe_visa_response_aggregates_chunked_response() {
        let mut chunks = vec![b"ID".to_vec(), b"N".to_vec(), Vec::new()].into_iter();

        let response = read_visa_response(|| Ok(chunks.next().unwrap_or_default()), 16).unwrap();

        assert_eq!(response, b"IDN");
    }

    #[tokio::test]
    async fn unit_probe_failure_output_defaults_to_counts_without_samples() {
        let response = run_device_probe(
            DeviceProbeParams {
                targets: vec![ProbeTarget::Visa],
                serial: None,
                visa: Some(VisaProbeParams {
                    resources: Some(vec!["USB0::0x0000::0x0000::SN::INSTR".to_owned()]),
                    resource_filter: None,
                    max_results: 1,
                    open_timeout_ms: 1_000,
                    io_timeout_ms: 1_000,
                    read_termination: None,
                    write_termination: None,
                    encoding: ProbeEncoding::Text,
                }),
                payload: ProbePayloadParams {
                    data: "*IDN?".to_owned(),
                    encoding: ProbeEncoding::Text,
                    append_line_break: true,
                },
                matcher: ProbeMatcherParams::AnyResponse,
                limits: ProbeLimitsParams::default(),
                failure_output: FailureOutputParam::Counts,
                include_failure_samples: false,
            },
            RuntimeLimits::default(),
        )
        .await
        .unwrap();

        assert!(response.failure_samples.is_empty());
        assert_eq!(response.summary.skipped_count, 1);
        assert_eq!(response.summary.failure_status_counts["unsupported"], 1);
        assert_eq!(
            response.summary.failure_error_counts["FEATURE_NOT_COMPILED"],
            1
        );
    }
}
