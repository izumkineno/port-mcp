use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};
use serde_json::{Value, json};

use crate::model::ToolResponse;

pub fn call_tool_result_with_duration(
    response: ToolResponse,
    duration_ms: u64,
) -> Result<CallToolResult, McpError> {
    log_tool_response(&response, duration_ms);
    let text = serde_json::to_string(&response)
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

fn log_tool_response(response: &ToolResponse, duration_ms: u64) {
    let event = tool_log_event(response, None, duration_ms);
    tracing::info!(
        event = event["event"].as_str().unwrap_or("tool_call"),
        tool = event["tool"].as_str().unwrap_or_default(),
        request_id = event["request_id"].as_str().unwrap_or_default(),
        handle_id = event["handle_id"].as_str(),
        session = event["session"].as_str(),
        state_before = event["state_before"].as_str(),
        state_after = event["state_after"].as_str(),
        error_code = event["error_code"].as_str(),
        duration_ms,
        sensitive = false,
        "port-mcp tool call completed"
    );
}

fn tool_log_event(response: &ToolResponse, session: Option<&str>, duration_ms: u64) -> Value {
    match serde_json::to_value(response).expect("tool response should serialize") {
        Value::Object(fields) => tool_log_event_value(
            fields
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            fields
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            fields.get("handle_id").and_then(Value::as_str),
            session,
            fields.get("state").and_then(Value::as_str),
            fields.get("state").and_then(Value::as_str),
            fields
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str),
            duration_ms,
        ),
        _ => tool_log_event_value("", "", None, session, None, None, None, duration_ms),
    }
}

fn tool_log_event_value(
    tool: &str,
    request_id: &str,
    handle_id: Option<&str>,
    session: Option<&str>,
    state_before: Option<&str>,
    state_after: Option<&str>,
    error_code: Option<&str>,
    duration_ms: u64,
) -> Value {
    json!({
        "event": "tool_call",
        "tool": tool,
        "request_id": request_id,
        "handle_id": handle_id,
        "session": session,
        "state_before": state_before,
        "state_after": state_after,
        "error_code": error_code,
        "duration_ms": duration_ms,
        "sensitive": false
    })
}

#[cfg(test)]
pub(crate) fn tool_log_event_for_tests(
    tool: &str,
    request_id: &str,
    handle_id: Option<&str>,
    session: Option<&str>,
    state_before: Option<&str>,
    state_after: Option<&str>,
    error_code: Option<&str>,
    duration_ms: u64,
) -> Value {
    tool_log_event_value(
        tool,
        request_id,
        handle_id,
        session,
        state_before,
        state_after,
        error_code,
        duration_ms,
    )
}
