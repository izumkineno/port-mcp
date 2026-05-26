use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{DomainError, HandleId, InstanceState, RequestId, Timestamp};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResponse {
    Success(SuccessResponse),
    Failure(FailureResponse),
}

impl ToolResponse {
    pub fn success(tool: &str, request_id: RequestId, timestamp: Timestamp, data: Value) -> Self {
        Self::Success(SuccessResponse {
            ok: true,
            tool: tool.to_owned(),
            request_id,
            timestamp,
            handle_id: None,
            state: None,
            data,
            warnings: Vec::new(),
        })
    }

    pub fn failure(
        tool: &str,
        request_id: RequestId,
        timestamp: Timestamp,
        error: DomainError,
    ) -> Self {
        Self::Failure(FailureResponse {
            ok: false,
            tool: tool.to_owned(),
            request_id,
            timestamp,
            handle_id: None,
            state: None,
            error,
        })
    }

    pub fn with_handle(mut self, handle_id: HandleId) -> Self {
        match &mut self {
            Self::Success(response) => response.handle_id = Some(handle_id),
            Self::Failure(response) => response.handle_id = Some(handle_id),
        }
        self
    }

    pub fn with_state(mut self, state: InstanceState) -> Self {
        match &mut self {
            Self::Success(response) => response.state = Some(state),
            Self::Failure(response) => response.state = Some(state),
        }
        self
    }

    pub fn with_warning(mut self, warning: Warning) -> Self {
        if let Self::Success(response) = &mut self {
            response.warnings.push(warning);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessResponse {
    pub ok: bool,
    pub tool: String,
    pub request_id: RequestId,
    pub timestamp: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle_id: Option<HandleId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<InstanceState>,
    pub data: Value,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureResponse {
    pub ok: bool,
    pub tool: String,
    pub request_id: RequestId,
    pub timestamp: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle_id: Option<HandleId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<InstanceState>,
    pub error: DomainError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

impl Warning {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
        }
    }
}
