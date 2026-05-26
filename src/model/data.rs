use serde::{Deserialize, Serialize};

use super::{DomainError, ErrorCode, PayloadEncoding};

#[derive(Debug)]
pub struct Payload {
    pub bytes: Vec<u8>,
    pub encoding: PayloadEncoding,
}

impl Payload {
    pub fn from_text(input: &str, append_line_break: bool) -> Result<Self, DomainError> {
        let mut bytes = input.as_bytes().to_vec();
        if append_line_break {
            bytes.push(b'\n');
        }
        Ok(Self {
            bytes,
            encoding: PayloadEncoding::Text,
        })
    }

    pub fn from_hex(input: &str, append_line_break: bool) -> Result<Self, DomainError> {
        if !input.len().is_multiple_of(2)
            || !input.chars().all(|character| character.is_ascii_hexdigit())
        {
            return Err(DomainError::invalid_argument(
                ErrorCode::InvalidHex,
                "Hex payload must contain an even number of hexadecimal characters.",
                "Use only 0-9, a-f, A-F and provide an even character count.",
            ));
        }

        let mut bytes = Vec::with_capacity(input.len() / 2 + usize::from(append_line_break));
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
        if append_line_break {
            bytes.push(b'\n');
        }

        Ok(Self {
            bytes,
            encoding: PayloadEncoding::Hex,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadSummary {
    pub preview: String,
    pub preview_encoding: PayloadEncoding,
    pub payload_bytes: usize,
    pub omitted_bytes: usize,
    pub truncated: bool,
    pub datagram: bool,
}

impl PayloadSummary {
    pub fn from_bytes(
        bytes: &[u8],
        encoding: PayloadEncoding,
        max_preview_bytes: usize,
        datagram: bool,
    ) -> Self {
        let preview_bytes = bytes.len().min(max_preview_bytes);
        let preview = match encoding {
            PayloadEncoding::Text => String::from_utf8_lossy(&bytes[..preview_bytes]).to_string(),
            PayloadEncoding::Hex => bytes[..preview_bytes]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        };
        Self {
            preview,
            preview_encoding: encoding,
            payload_bytes: bytes.len(),
            omitted_bytes: bytes.len().saturating_sub(preview_bytes),
            truncated: bytes.len() > preview_bytes,
            datagram,
        }
    }
}
