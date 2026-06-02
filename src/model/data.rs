use serde::{Deserialize, Serialize};

use super::{DomainError, PayloadEncoding};
use crate::util::encoding::{bytes_to_hex, hex_to_bytes, text_to_bytes};

#[derive(Debug)]
pub struct Payload {
    pub bytes: Vec<u8>,
    pub encoding: PayloadEncoding,
}

impl Payload {
    pub fn from_text(input: &str, append_line_break: bool) -> Result<Self, DomainError> {
        let mut bytes = text_to_bytes(input)?;
        if append_line_break {
            bytes.push(b'\n');
        }
        Ok(Self {
            bytes,
            encoding: PayloadEncoding::Text,
        })
    }

    pub fn from_hex(input: &str, append_line_break: bool) -> Result<Self, DomainError> {
        let mut bytes = hex_to_bytes(input)?;
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
            PayloadEncoding::Hex => bytes_to_hex(&bytes[..preview_bytes]),
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
