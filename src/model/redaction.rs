use serde_json::{Value, json};

use super::{PayloadEncoding, PayloadSummary};

#[derive(Default)]
pub struct Redactor;

impl Redactor {
    pub fn local_path(&self, path: &str) -> Value {
        let normalized = path.replace('\\', "/");
        let file_name = normalized.rsplit('/').next().unwrap_or("<redacted-path>");
        json!(file_name)
    }

    pub fn env_value(&self, name: &str, _value: &str) -> Value {
        json!({ "name": name, "value": "<redacted>" })
    }

    pub fn payload_preview(&self, bytes: &[u8], max_preview_bytes: usize) -> Value {
        let summary =
            PayloadSummary::from_bytes(bytes, PayloadEncoding::Text, max_preview_bytes, false);
        serde_json::to_value(summary).expect("payload summary should serialize")
    }

    pub fn os_error(&self, message: &str) -> Value {
        let lowered = message.to_ascii_lowercase();
        let io_kind = if lowered.contains("permission") || lowered.contains("access") {
            "permission_denied"
        } else if lowered.contains("not found") {
            "not_found"
        } else {
            "other"
        };
        json!({ "io_kind": io_kind, "message": "<redacted>" })
    }

    pub fn stack_trace(&self, _stack: &str) -> Value {
        json!("<redacted>")
    }
}
