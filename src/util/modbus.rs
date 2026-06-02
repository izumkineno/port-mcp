#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::model::DomainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModbusMode {
    Rtu,
    Ascii,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModbusAction {
    Pack,
    Unpack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModbusPackRequest {
    pub mode: ModbusMode,
    pub slave_id: u8,
    pub function_code: u8,
    pub address: u16,
    pub data_or_hex: Option<String>,
    #[serde(default = "default_crc_check")]
    pub crc_check: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModbusUnpackRequest {
    pub mode: ModbusMode,
    pub frame_hex: String,
    #[serde(default = "default_crc_check")]
    pub crc_check: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModbusPackResult {
    pub frame_hex: String,
    pub frame_bytes: usize,
    pub crc_hex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModbusUnpackResult {
    pub slave_id: u8,
    pub function_code: u8,
    pub address: Option<u16>,
    pub data_hex: String,
    pub crc_hex: Option<String>,
    pub checksum_valid: bool,
}

pub fn pack_rtu(request: ModbusPackRequest) -> Result<ModbusPackResult, DomainError> {
    pack_rtu_with_hex_limit(
        request,
        crate::model::RuntimeLimits::ABS_MAX_TOTAL_BUFFER_BYTES,
    )
}

pub fn pack_rtu_with_hex_limit(
    request: ModbusPackRequest,
    max_hex_output_bytes: usize,
) -> Result<ModbusPackResult, DomainError> {
    if request.mode != ModbusMode::Rtu {
        return Err(DomainError::protocol_frame_invalid(
            "Only Modbus RTU is supported in the first helper slice.",
            "Use mode=rtu and retry.",
        )
        .with_detail("field", serde_json::json!("mode")));
    }
    let mut frame = vec![request.slave_id, request.function_code];
    frame.extend_from_slice(&request.address.to_be_bytes());
    if let Some(data) = request.data_or_hex {
        frame.extend_from_slice(&crate::util::encoding::hex_to_bytes_with_limit(
            &data,
            "data_or_hex",
            max_hex_output_bytes,
        )?);
    }
    let crc = crc16_modbus(&frame);
    let crc_bytes = crc.to_le_bytes();
    frame.extend_from_slice(&crc_bytes);
    Ok(ModbusPackResult {
        frame_hex: crate::util::encoding::bytes_to_hex(&frame),
        frame_bytes: frame.len(),
        crc_hex: Some(crate::util::encoding::bytes_to_hex(&crc_bytes)),
    })
}

pub fn unpack_rtu(request: ModbusUnpackRequest) -> Result<ModbusUnpackResult, DomainError> {
    unpack_rtu_with_hex_limit(
        request,
        crate::model::RuntimeLimits::ABS_MAX_TOTAL_BUFFER_BYTES,
    )
}

pub fn unpack_rtu_with_hex_limit(
    request: ModbusUnpackRequest,
    max_hex_output_bytes: usize,
) -> Result<ModbusUnpackResult, DomainError> {
    if request.mode != ModbusMode::Rtu {
        return Err(DomainError::protocol_frame_invalid(
            "Only Modbus RTU is supported in the first helper slice.",
            "Use mode=rtu and retry.",
        )
        .with_detail("field", serde_json::json!("mode")));
    }
    let frame = crate::util::encoding::hex_to_bytes_with_limit(
        &request.frame_hex,
        "frame_hex",
        max_hex_output_bytes,
    )?;
    if frame.len() < 4 {
        return Err(DomainError::protocol_frame_invalid(
            "Modbus RTU frame is too short.",
            "Provide at least slave_id, function_code, payload, and CRC bytes.",
        ));
    }
    let (payload, crc_bytes) = frame.split_at(frame.len() - 2);
    let expected_crc = crc16_modbus(payload);
    let expected_crc_bytes = expected_crc.to_le_bytes();
    let checksum_valid = crc_bytes == expected_crc_bytes.as_slice();
    if request.crc_check && !checksum_valid {
        return Err(DomainError::protocol_checksum_failed(
            "Modbus RTU CRC check failed.",
            "Repack the frame or verify the payload and CRC bytes.",
        )
        .with_detail(
            "expected_crc",
            serde_json::json!(crate::util::encoding::bytes_to_hex(&expected_crc_bytes)),
        )
        .with_detail(
            "actual_crc",
            serde_json::json!(crate::util::encoding::bytes_to_hex(crc_bytes)),
        ));
    }
    if payload.len() < 4 {
        return Err(DomainError::protocol_frame_invalid(
            "Modbus RTU payload is too short.",
            "Provide a frame containing slave_id, function_code, address, and CRC bytes.",
        ));
    }
    Ok(ModbusUnpackResult {
        slave_id: payload[0],
        function_code: payload[1],
        address: Some(u16::from_be_bytes([payload[2], payload[3]])),
        data_hex: crate::util::encoding::bytes_to_hex(&payload[4..]),
        crc_hex: Some(crate::util::encoding::bytes_to_hex(crc_bytes)),
        checksum_valid,
    })
}

pub fn crc16_modbus(bytes: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for byte in bytes {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            if crc & 0x0001 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

fn default_crc_check() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_modbus_rtu_pack_and_unpack_round_trip() {
        let packed = pack_rtu(ModbusPackRequest {
            mode: ModbusMode::Rtu,
            slave_id: 1,
            function_code: 3,
            address: 0x0010,
            data_or_hex: Some("0002".to_owned()),
            crc_check: true,
        })
        .unwrap();

        let unpacked = unpack_rtu(ModbusUnpackRequest {
            mode: ModbusMode::Rtu,
            frame_hex: packed.frame_hex.clone(),
            crc_check: true,
        })
        .unwrap();

        assert_eq!(unpacked.slave_id, 1);
        assert_eq!(unpacked.function_code, 3);
        assert_eq!(unpacked.address, Some(0x0010));
        assert_eq!(unpacked.data_hex, "0002");
        assert!(unpacked.checksum_valid);
    }

    #[test]
    fn unit_modbus_rtu_unpack_rejects_bad_crc_when_requested() {
        let error = unpack_rtu(ModbusUnpackRequest {
            mode: ModbusMode::Rtu,
            frame_hex: "0103001000020000".to_owned(),
            crc_check: true,
        })
        .unwrap_err();
        assert_eq!(error.code, crate::model::ErrorCode::ProtocolChecksumFailed);
    }

    #[test]
    fn unit_modbus_rtu_unpack_reports_bad_crc_when_not_strict() {
        let unpacked = unpack_rtu(ModbusUnpackRequest {
            mode: ModbusMode::Rtu,
            frame_hex: "0103001000020000".to_owned(),
            crc_check: false,
        })
        .unwrap();

        assert!(!unpacked.checksum_valid);
        assert_eq!(unpacked.slave_id, 1);
        assert_eq!(unpacked.function_code, 3);
    }

    #[test]
    fn unit_modbus_rtu_unpack_rejects_short_payload_without_panic() {
        for frame_hex in ["01030000", "0103000000"] {
            let error = unpack_rtu(ModbusUnpackRequest {
                mode: ModbusMode::Rtu,
                frame_hex: frame_hex.to_owned(),
                crc_check: false,
            })
            .unwrap_err();
            assert_eq!(error.code, crate::model::ErrorCode::ProtocolFrameInvalid);
        }
    }

    #[test]
    fn unit_modbus_rtu_pack_rejects_helper_oversized_payload() {
        let error = pack_rtu_with_hex_limit(
            ModbusPackRequest {
                mode: ModbusMode::Rtu,
                slave_id: 1,
                function_code: 3,
                address: 0x0010,
                data_or_hex: Some("0000".to_owned()),
                crc_check: true,
            },
            1,
        )
        .unwrap_err();
        assert_eq!(error.code, crate::model::ErrorCode::InvalidRange);
    }
}
