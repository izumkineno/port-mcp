#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::model::{DomainError, ErrorCode, PayloadEncoding};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodingSummary {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub truncated: bool,
    pub preview: String,
    pub encoding: PayloadEncoding,
}

pub fn text_to_bytes(input: &str) -> Result<Vec<u8>, DomainError> {
    text_to_bytes_with_limit(
        input,
        "input_string",
        crate::model::RuntimeLimits::ABS_MAX_TOTAL_BUFFER_BYTES,
    )
}

pub fn text_to_bytes_with_limit(
    input: &str,
    field: &'static str,
    max_input_bytes: usize,
) -> Result<Vec<u8>, DomainError> {
    if input.len() > max_input_bytes {
        return Err(DomainError::invalid_argument(
            ErrorCode::InvalidRange,
            "Text payload exceeds the hard limit.",
            "Shorten the text payload and retry.",
        )
        .with_detail("field", serde_json::json!(field))
        .with_detail("max", serde_json::json!(max_input_bytes))
        .with_detail("actual", serde_json::json!(input.len())));
    }

    Ok(input.as_bytes().to_vec())
}

pub fn hex_to_bytes(input: &str) -> Result<Vec<u8>, DomainError> {
    hex_to_bytes_with_limit(
        input,
        "hex",
        crate::model::RuntimeLimits::ABS_MAX_TOTAL_BUFFER_BYTES,
    )
}

pub fn hex_to_bytes_with_limit(
    input: &str,
    field: &'static str,
    max_output_bytes: usize,
) -> Result<Vec<u8>, DomainError> {
    let output_len = validate_hex_input_with_limit(input, field, max_output_bytes)?;

    let mut bytes = Vec::with_capacity(output_len);
    for index in (0..input.len()).step_by(2) {
        let byte = u8::from_str_radix(&input[index..index + 2], 16).map_err(|_| {
            DomainError::invalid_argument(
                ErrorCode::InvalidHex,
                "Hex payload contains invalid characters.",
                "Use only valid hexadecimal characters.",
            )
        })?;
        bytes.push(byte);
    }

    Ok(bytes)
}

fn validate_hex_input(input: &str, field: &'static str) -> Result<usize, DomainError> {
    validate_hex_input_with_limit(
        input,
        field,
        crate::model::RuntimeLimits::ABS_MAX_TOTAL_BUFFER_BYTES,
    )
}

fn validate_hex_input_with_limit(
    input: &str,
    field: &'static str,
    max_output_bytes: usize,
) -> Result<usize, DomainError> {
    if !input.len().is_multiple_of(2)
        || !input.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(DomainError::invalid_argument(
            ErrorCode::InvalidHex,
            "Hex payload must contain an even number of hexadecimal characters.",
            "Use only 0-9, a-f, A-F and provide an even character count.",
        ));
    }

    let output_len = input.len() / 2;
    if output_len > max_output_bytes {
        return Err(DomainError::invalid_argument(
            ErrorCode::InvalidRange,
            "Hex payload exceeds the hard limit.",
            "Shorten the hex payload and retry.",
        )
        .with_detail("field", serde_json::json!(field))
        .with_detail("max", serde_json::json!(max_output_bytes))
        .with_detail("actual", serde_json::json!(output_len)));
    }

    Ok(output_len)
}

pub fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn str_to_hex(input: &str) -> Result<String, DomainError> {
    let bytes = text_to_bytes(input)?;
    Ok(bytes_to_hex(&bytes))
}

pub fn str_to_hex_with_limit(
    input: &str,
    field: &'static str,
    max_input_bytes: usize,
) -> Result<String, DomainError> {
    let bytes = text_to_bytes_with_limit(input, field, max_input_bytes)?;
    Ok(bytes_to_hex(&bytes))
}

pub fn hex_to_str(input: &str) -> Result<String, DomainError> {
    let bytes = hex_to_bytes(input)?;
    bytes_to_utf8_string(bytes)
}

pub fn hex_to_str_with_limit(
    input: &str,
    field: &'static str,
    max_output_bytes: usize,
) -> Result<String, DomainError> {
    let bytes = hex_to_bytes_with_limit(input, field, max_output_bytes)?;
    bytes_to_utf8_string(bytes)
}

fn bytes_to_utf8_string(bytes: Vec<u8>) -> Result<String, DomainError> {
    String::from_utf8(bytes).map_err(|_| {
        DomainError::invalid_argument(
            ErrorCode::TextEncodingFailed,
            "Hex payload cannot be decoded as UTF-8 text.",
            "Use UTF-8 text or `encoding=hex` instead.",
        )
    })
}

pub fn summarize_text(input: &str, max_preview_bytes: usize) -> EncodingSummary {
    let bytes = input.as_bytes();
    let preview_bytes = bytes.len().min(max_preview_bytes);
    EncodingSummary {
        input_bytes: bytes.len(),
        output_bytes: bytes.len(),
        truncated: bytes.len() > preview_bytes,
        preview: String::from_utf8_lossy(&bytes[..preview_bytes]).to_string(),
        encoding: PayloadEncoding::Text,
    }
}

pub fn summarize_hex(
    input: &str,
    max_preview_bytes: usize,
) -> Result<EncodingSummary, DomainError> {
    let bytes = hex_to_bytes(input)?;
    let preview_bytes = bytes.len().min(max_preview_bytes);
    Ok(EncodingSummary {
        input_bytes: input.len(),
        output_bytes: bytes.len(),
        truncated: bytes.len() > preview_bytes,
        preview: bytes_to_hex(&bytes[..preview_bytes]),
        encoding: PayloadEncoding::Hex,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_text_and_hex_conversions_round_trip() {
        let hex = str_to_hex("ping").unwrap();
        assert_eq!(hex, "70696e67");
        assert_eq!(hex_to_str(&hex).unwrap(), "ping");
        assert_eq!(text_to_bytes("ping").unwrap(), b"ping");
        assert_eq!(hex_to_bytes("70696e67").unwrap(), b"ping");
    }

    #[test]
    fn unit_hex_validation_rejects_invalid_input() {
        assert_eq!(
            hex_to_bytes("abc").unwrap_err().code,
            crate::model::ErrorCode::InvalidHex
        );
        assert_eq!(
            hex_to_str("zz").unwrap_err().code,
            crate::model::ErrorCode::InvalidHex
        );
    }

    #[test]
    fn unit_hex_validation_rejects_oversized_output_bytes() {
        let max = 4usize;
        assert_eq!(
            validate_hex_input_with_limit(&"00".repeat(max), "hex", max).unwrap(),
            max
        );

        let error = validate_hex_input_with_limit(&"00".repeat(max + 1), "hex", max).unwrap_err();
        assert_eq!(error.code, crate::model::ErrorCode::InvalidRange);
        let details = serde_json::to_value(&error.details).unwrap();
        assert_eq!(details["field"], "hex");
        assert_eq!(details["max"], max);
        assert_eq!(details["actual"], max + 1);
    }

    #[test]
    fn unit_limited_text_conversion_rejects_oversized_input() {
        let error = str_to_hex_with_limit("abcde", "input_string", 4).unwrap_err();
        assert_eq!(error.code, crate::model::ErrorCode::InvalidRange);
        let details = serde_json::to_value(&error.details).unwrap();
        assert_eq!(details["field"], "input_string");
        assert_eq!(details["max"], 4);
        assert_eq!(details["actual"], 5);
    }

    #[test]
    fn unit_hex_to_str_rejects_non_utf8_payload() {
        let error = hex_to_str("ff").unwrap_err();
        assert_eq!(error.code, crate::model::ErrorCode::TextEncodingFailed);
        assert!(error.message.contains("UTF-8"));
    }

    #[test]
    fn unit_summaries_truncate_and_preserve_encoding() {
        let text = summarize_text("0123456789", 4);
        assert_eq!(text.preview, "0123");
        assert!(text.truncated);
        assert_eq!(text.encoding, PayloadEncoding::Text);

        let hex = summarize_hex("70696e67", 2).unwrap();
        assert_eq!(hex.preview, "7069");
        assert!(hex.truncated);
        assert_eq!(hex.encoding, PayloadEncoding::Hex);
    }
}
