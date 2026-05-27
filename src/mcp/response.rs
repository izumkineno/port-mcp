use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};
use serde_json::{Value, json};

use crate::model::{PayloadEncoding, ToolResponse};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PortIoLogConfig {
    pub max_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PortIoDirection {
    Tx,
    Rx,
}

impl PortIoDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tx => "tx",
            Self::Rx => "rx",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PortIoLog {
    direction: PortIoDirection,
    bytes: Vec<u8>,
    preferred_encoding: PayloadEncoding,
}

impl PortIoLog {
    pub(crate) fn new(
        direction: PortIoDirection,
        bytes: Vec<u8>,
        preferred_encoding: PayloadEncoding,
    ) -> Self {
        Self {
            direction,
            bytes,
            preferred_encoding,
        }
    }
}

pub fn call_tool_result_with_duration(
    response: ToolResponse,
    duration_ms: u64,
) -> Result<CallToolResult, McpError> {
    call_tool_result_with_duration_and_io(response, duration_ms, PortIoLogConfig::default(), None)
}

pub(crate) fn call_tool_result_with_duration_and_io(
    response: ToolResponse,
    duration_ms: u64,
    port_io_log_config: PortIoLogConfig,
    port_io: Option<PortIoLog>,
) -> Result<CallToolResult, McpError> {
    log_tool_response(&response, duration_ms, port_io_log_config, port_io.as_ref());
    let text = serde_json::to_string(&response)
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

fn log_tool_response(
    response: &ToolResponse,
    duration_ms: u64,
    port_io_log_config: PortIoLogConfig,
    port_io: Option<&PortIoLog>,
) {
    let event = tool_log_event(response, None, duration_ms, port_io_log_config, port_io);
    let port_io = event.get("port_io");
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
        port_io_direction = port_io
            .and_then(|value| value.get("direction"))
            .and_then(serde_json::Value::as_str),
        port_io_bytes = port_io
            .and_then(|value| value.get("bytes"))
            .and_then(serde_json::Value::as_u64),
        port_io_preview_encoding = port_io
            .and_then(|value| value.get("preview_encoding"))
            .and_then(serde_json::Value::as_str),
        port_io_preview = port_io
            .and_then(|value| value.get("preview"))
            .and_then(serde_json::Value::as_str),
        port_io_hex = port_io
            .and_then(|value| value.get("hex"))
            .and_then(serde_json::Value::as_str),
        port_io_omitted_bytes = port_io
            .and_then(|value| value.get("omitted_bytes"))
            .and_then(serde_json::Value::as_u64),
        "port-mcp tool call completed"
    );
}

fn tool_log_event(
    response: &ToolResponse,
    session: Option<&str>,
    duration_ms: u64,
    port_io_log_config: PortIoLogConfig,
    port_io: Option<&PortIoLog>,
) -> Value {
    match serde_json::to_value(response).expect("tool response should serialize") {
        Value::Object(fields) => add_port_io_log(
            tool_log_event_value(
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
            port_io_log_config,
            port_io,
        ),
        _ => add_port_io_log(
            tool_log_event_value("", "", None, session, None, None, None, duration_ms),
            port_io_log_config,
            port_io,
        ),
    }
}

fn add_port_io_log(
    mut event: Value,
    config: PortIoLogConfig,
    port_io: Option<&PortIoLog>,
) -> Value {
    let Some(port_io) = port_io else {
        return event;
    };
    if config.max_bytes == 0 {
        return event;
    }

    let preview_len = port_io.bytes.len().min(config.max_bytes);
    let preview_bytes = &port_io.bytes[..preview_len];
    let preview_encoding = match port_io.preferred_encoding {
        PayloadEncoding::Text => "text",
        PayloadEncoding::Hex => "hex",
    };
    let preview = match port_io.preferred_encoding {
        PayloadEncoding::Text => String::from_utf8_lossy(preview_bytes).to_string(),
        PayloadEncoding::Hex => bytes_to_hex(preview_bytes),
    };
    let value = json!({
        "direction": port_io.direction.as_str(),
        "bytes": port_io.bytes.len(),
        "preview_encoding": preview_encoding,
        "preview": preview,
        "hex": bytes_to_hex(preview_bytes),
        "omitted_bytes": port_io.bytes.len().saturating_sub(preview_len),
    });
    if let Value::Object(fields) = &mut event {
        fields.insert("port_io".to_owned(), value);
    }
    event
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
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

#[cfg(test)]
pub(crate) fn tool_log_event_with_port_io_for_tests(
    response: &ToolResponse,
    config: PortIoLogConfig,
    port_io: Option<PortIoLog>,
) -> Value {
    tool_log_event(response, None, 7, config, port_io.as_ref())
}
