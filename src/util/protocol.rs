#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::model::{DomainError, ErrorCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolKind {
    Scpi,
    At,
    Slip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolSummary {
    pub kind: ProtocolKind,
    pub normalized: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_hex: Option<String>,
}

pub fn normalize_scpi(
    command: &str,
    arguments: Option<&str>,
    expect_response: Option<&str>,
) -> Result<ProtocolSummary, DomainError> {
    validate_length(command, 512, "command")?;
    if let Some(expect_response) = expect_response {
        validate_length(expect_response, 512, "expect_response")?;
    }
    let mut normalized = command.trim().to_owned();
    if let Some(arguments) = arguments {
        validate_length(arguments, 512, "arguments")?;
        if !arguments.trim().is_empty() {
            normalized.push(' ');
            normalized.push_str(arguments.trim());
        }
    }
    Ok(ProtocolSummary {
        kind: ProtocolKind::Scpi,
        normalized,
        response_class: expect_response
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        payload_hex: None,
    })
}

pub fn classify_at(command: &str) -> Result<ProtocolSummary, DomainError> {
    validate_length(command, 256, "command")?;
    let trimmed = command.trim();
    let response_class = if trimmed.starts_with("AT+") {
        Some("extended".to_owned())
    } else if trimmed == "AT" {
        Some("basic".to_owned())
    } else {
        Some("custom".to_owned())
    };
    Ok(ProtocolSummary {
        kind: ProtocolKind::At,
        normalized: trimmed.to_owned(),
        response_class,
        payload_hex: None,
    })
}

pub fn encode_slip_payload(payload_hex: &str) -> Result<ProtocolSummary, DomainError> {
    encode_slip_payload_with_limit(
        payload_hex,
        crate::model::RuntimeLimits::ABS_MAX_TOTAL_BUFFER_BYTES,
    )
}

pub fn encode_slip_payload_with_limit(
    payload_hex: &str,
    max_payload_bytes: usize,
) -> Result<ProtocolSummary, DomainError> {
    let payload = crate::util::encoding::hex_to_bytes_with_limit(
        payload_hex,
        "payload_hex",
        max_payload_bytes,
    )?;
    let mut escaped = Vec::with_capacity(payload.len() + 2);
    escaped.push(0xC0);
    for byte in payload {
        match byte {
            0xC0 => {
                escaped.extend_from_slice(&[0xDB, 0xDC]);
            }
            0xDB => {
                escaped.extend_from_slice(&[0xDB, 0xDD]);
            }
            other => escaped.push(other),
        }
    }
    escaped.push(0xC0);
    if escaped.len() > max_payload_bytes {
        return Err(DomainError::invalid_argument(
            ErrorCode::InvalidRange,
            "SLIP encoded frame exceeds the helper output limit.",
            "Use a smaller payload and retry.",
        )
        .with_detail("field", serde_json::json!("payload_hex"))
        .with_detail("max", serde_json::json!(max_payload_bytes))
        .with_detail("actual", serde_json::json!(escaped.len())));
    }
    Ok(ProtocolSummary {
        kind: ProtocolKind::Slip,
        normalized: crate::util::encoding::bytes_to_hex(&escaped),
        response_class: None,
        payload_hex: Some(payload_hex.to_owned()),
    })
}

pub fn decode_slip_frame(frame_hex: &str) -> Result<ProtocolSummary, DomainError> {
    decode_slip_frame_with_limit(
        frame_hex,
        crate::model::RuntimeLimits::ABS_MAX_TOTAL_BUFFER_BYTES,
    )
}

pub fn decode_slip_frame_with_limit(
    frame_hex: &str,
    max_frame_bytes: usize,
) -> Result<ProtocolSummary, DomainError> {
    let frame =
        crate::util::encoding::hex_to_bytes_with_limit(frame_hex, "payload_hex", max_frame_bytes)?;
    if frame.len() < 2 || frame.first() != Some(&0xC0) || frame.last() != Some(&0xC0) {
        return Err(DomainError::protocol_frame_invalid(
            "SLIP frame must start and end with 0xC0.",
            "Provide a framed SLIP packet and retry.",
        ));
    }
    let mut payload = Vec::new();
    let mut index = 1usize;
    while index < frame.len() - 1 {
        match frame[index] {
            0xDB => {
                if index + 1 >= frame.len() - 1 {
                    return Err(DomainError::protocol_frame_invalid(
                        "SLIP frame contains an invalid escape sequence.",
                        "Use DB DC for C0 and DB DD for DB.",
                    ));
                }
                match frame[index + 1] {
                    0xDC => payload.push(0xC0),
                    0xDD => payload.push(0xDB),
                    _ => {
                        return Err(DomainError::protocol_frame_invalid(
                            "SLIP frame contains an invalid escape sequence.",
                            "Use DB DC for C0 and DB DD for DB.",
                        ));
                    }
                }
                index += 2;
            }
            byte => {
                payload.push(byte);
                index += 1;
            }
        }
    }
    Ok(ProtocolSummary {
        kind: ProtocolKind::Slip,
        normalized: crate::util::encoding::bytes_to_hex(&payload),
        response_class: Some("decoded".to_owned()),
        payload_hex: Some(crate::util::encoding::bytes_to_hex(&payload)),
    })
}

pub fn normalize_slip_payload(payload_hex: &str) -> Result<ProtocolSummary, DomainError> {
    encode_slip_payload(payload_hex)
}

fn validate_length(input: &str, max: usize, field: &'static str) -> Result<(), DomainError> {
    if input.len() <= max {
        Ok(())
    } else {
        Err(DomainError::invalid_argument(
            ErrorCode::InvalidRange,
            format!("{field} exceeds the allowed length."),
            format!("Shorten `{field}` to at most {max} characters."),
        )
        .with_detail("field", serde_json::json!(field))
        .with_detail("max", serde_json::json!(max))
        .with_detail("actual", serde_json::json!(input.len())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_scpi_normalization_and_at_classification_work() {
        let scpi = normalize_scpi("  *IDN?  ", Some(" "), Some("line")).unwrap();
        assert_eq!(scpi.normalized, "*IDN?");
        assert_eq!(scpi.response_class.as_deref(), Some("line"));
        let at = classify_at("AT+CGMI").unwrap();
        assert_eq!(at.response_class.as_deref(), Some("extended"));
    }

    #[test]
    fn unit_scpi_rejects_oversized_expect_response() {
        let error = normalize_scpi("*IDN?", None, Some(&"x".repeat(513))).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRange);
    }

    #[test]
    fn unit_slip_normalization_escapes_control_bytes() {
        let summary = normalize_slip_payload("c0db01").unwrap();
        assert!(summary.normalized.starts_with("c0"));
        assert_eq!(summary.kind, ProtocolKind::Slip);
    }

    #[test]
    fn unit_slip_decode_recovers_payload_hex() {
        let decoded = decode_slip_frame("c0dbdc01c0").unwrap();
        assert_eq!(decoded.normalized, "c001");
        assert_eq!(decoded.payload_hex.as_deref(), Some("c001"));
        assert_eq!(decoded.response_class.as_deref(), Some("decoded"));

        let decoded = decode_slip_frame("c0dbddc0").unwrap();
        assert_eq!(decoded.normalized, "db");
        assert_eq!(decoded.payload_hex.as_deref(), Some("db"));
    }

    #[test]
    fn unit_slip_decode_rejects_invalid_escape_sequences() {
        assert_eq!(
            decode_slip_frame("c0dbc0").unwrap_err().code,
            ErrorCode::ProtocolFrameInvalid
        );
        assert_eq!(
            decode_slip_frame("c0db00c0").unwrap_err().code,
            ErrorCode::ProtocolFrameInvalid
        );
    }

    #[test]
    fn unit_slip_helpers_reject_oversized_payloads() {
        assert_eq!(
            encode_slip_payload_with_limit("0000", 1).unwrap_err().code,
            ErrorCode::InvalidRange
        );
        assert_eq!(
            encode_slip_payload_with_limit("c0", 3).unwrap_err().code,
            ErrorCode::InvalidRange
        );
        assert_eq!(
            decode_slip_frame_with_limit("c000c0", 2).unwrap_err().code,
            ErrorCode::InvalidRange
        );
    }
}
