#[cfg(feature = "visa")]
mod compiled {
    use std::time::Duration;

    use serde::Serialize;
    use visa_rs::{
        enums::attribute::{
            AttrSendEndEn, AttrTermchar, AttrTermcharEn, AttrTmoValue, HasAttribute,
        },
        flags::{AccessMode, FlushMode},
        prelude::{AsResourceManager, DefaultRM, Instrument},
    };

    use crate::model::{DomainError, ErrorCategory, ErrorCode, VisaConfig};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct VisaResourceSummary {
        pub resource_address: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub resource_class: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct VisaScanResult {
        pub resources: Vec<VisaResourceSummary>,
    }

    pub fn scan_visa_resources(
        resource_filter: &str,
        max_results: usize,
    ) -> std::result::Result<VisaScanResult, DomainError> {
        let rm = DefaultRM::new().map_err(map_runtime_unavailable)?.leak();
        let expr =
            visa_rs::VisaString::from_string(resource_filter.to_owned()).ok_or_else(|| {
                DomainError::invalid_argument(
                    ErrorCode::InvalidAddress,
                    "VISA resource filter contains a null byte.",
                    "Use a valid VISA resource filter without embedded null bytes.",
                )
                .with_detail("field", serde_json::json!("resource_filter"))
            })?;
        let mut list = rm.find_res_list(&expr).map_err(map_enum_error)?;
        let mut resources = Vec::new();
        while let Some(item) = list.find_next().map_err(map_enum_error)? {
            let resource_address = item.to_string_lossy().to_string();
            let resource_class = parse_resource_class(&resource_address);
            resources.push(VisaResourceSummary {
                resource_address,
                resource_class,
            });
            if resources.len() >= max_results {
                break;
            }
        }
        Ok(VisaScanResult { resources })
    }

    pub struct VisaWorker {
        instrument: Instrument,
        config: VisaConfig,
    }

    impl VisaWorker {
        pub fn open(config: &VisaConfig) -> std::result::Result<Self, DomainError> {
            let rm = DefaultRM::new().map_err(map_runtime_unavailable)?.leak();
            let resource = visa_rs::VisaString::from_string(config.resource_address.clone())
                .ok_or_else(|| {
                    DomainError::invalid_argument(
                        ErrorCode::VisaResourceNotFound,
                        "VISA resource address contains a null byte.",
                        "Use a valid VISA resource address without embedded null bytes.",
                    )
                    .with_detail("field", serde_json::json!("resource_address"))
                })?;
            let instrument = rm
                .open(
                    &resource,
                    AccessMode::NO_LOCK,
                    Duration::from_millis(config.open_timeout_ms),
                )
                .map_err(map_open_error)?;
            apply_io_config(&instrument, config)?;
            Ok(Self {
                instrument,
                config: config.clone(),
            })
        }

        pub fn write(&self, bytes: &[u8]) -> std::result::Result<usize, DomainError> {
            let mut payload = bytes.to_vec();
            if let Some(termination) = &self.config.write_termination {
                payload.extend_from_slice(termination.as_bytes());
            }
            std::io::Write::write_all(&mut &self.instrument, &payload)
                .map_err(map_io_write_error)?;
            Ok(payload.len())
        }

        pub fn read(&self, max_bytes: usize) -> std::result::Result<Vec<u8>, DomainError> {
            let mut buffer = vec![0; max_bytes];
            let read = std::io::Read::read(&mut &self.instrument, &mut buffer)
                .map_err(map_io_read_error)?;
            buffer.truncate(read);
            Ok(buffer)
        }

        pub fn clear(&self) -> std::result::Result<(), DomainError> {
            self.instrument.clear().map_err(map_write_error)
        }

        pub fn close(self) -> std::result::Result<(), DomainError> {
            let _ = self.instrument.visa_flush(FlushMode::IO_OUT_BUF);
            Ok(())
        }
    }

    fn parse_resource_class(resource_address: &str) -> Option<String> {
        let head = resource_address.split("::").next()?.trim();
        if head.is_empty() {
            return None;
        }
        let prefix = head
            .chars()
            .take_while(|ch| ch.is_ascii_alphabetic())
            .collect::<String>()
            .to_ascii_lowercase();
        if prefix.is_empty() {
            None
        } else {
            Some(prefix)
        }
    }

    fn apply_io_config(
        instr: &Instrument,
        config: &VisaConfig,
    ) -> std::result::Result<(), DomainError> {
        instr
            .set_attr(
                AttrTmoValue::new_checked(config.io_timeout_ms as _).ok_or_else(|| {
                    DomainError::invalid_argument(
                        ErrorCode::InvalidRange,
                        "VISA io_timeout_ms is outside the allowed range.",
                        "Use a positive timeout within the driver limits.",
                    )
                    .with_detail("field", serde_json::json!("io_timeout_ms"))
                    .with_detail("actual", serde_json::json!(config.io_timeout_ms))
                })?,
            )
            .map_err(map_runtime_unavailable)?;
        if let Some(term) = &config.read_termination {
            let term_char = term.as_bytes().first().copied().ok_or_else(|| {
                DomainError::invalid_argument(
                    ErrorCode::InvalidRange,
                    "VISA read_termination is empty.",
                    "Provide a single-character termination string or omit the field.",
                )
                .with_detail("field", serde_json::json!("read_termination"))
            })?;
            instr
                .set_attr(AttrTermchar::new_checked(term_char).ok_or_else(|| {
                    DomainError::invalid_argument(
                        ErrorCode::InvalidRange,
                        "VISA read_termination is invalid.",
                        "Provide a single-character termination string or omit the field.",
                    )
                    .with_detail("field", serde_json::json!("read_termination"))
                })?)
                .map_err(map_runtime_unavailable)?;
            instr
                .set_attr(AttrTermcharEn::VI_TRUE)
                .map_err(map_runtime_unavailable)?;
        }
        if config.write_termination.is_some() {
            instr
                .set_attr(AttrSendEndEn::VI_TRUE)
                .map_err(map_runtime_unavailable)?;
        }
        Ok(())
    }

    fn map_runtime_unavailable(error: visa_rs::Error) -> DomainError {
        map_visa_error(
            error,
            ErrorCode::VisaRuntimeUnavailable,
            ErrorCategory::InvalidState,
            "VISA runtime is unavailable.",
            "Install and validate the VISA runtime and driver stack, then retry.",
        )
    }

    fn map_enum_error(error: visa_rs::Error) -> DomainError {
        map_visa_error(
            error,
            ErrorCode::VisaEnumFailed,
            ErrorCategory::WriteFailed,
            "VISA resource enumeration failed.",
            "Check the VISA runtime and resource filter, then retry.",
        )
    }

    fn map_open_error(error: visa_rs::Error) -> DomainError {
        map_visa_error(
            error,
            ErrorCode::VisaOpenFailed,
            ErrorCategory::WriteFailed,
            "VISA open failed.",
            "Check the resource address, resource occupancy, and VISA runtime state.",
        )
    }

    fn map_write_error(error: visa_rs::Error) -> DomainError {
        map_visa_error(
            error,
            ErrorCode::VisaWriteFailed,
            ErrorCategory::WriteFailed,
            "VISA write failed.",
            "Check termination settings, timeout, and resource state, then retry.",
        )
    }

    fn map_read_error(error: visa_rs::Error) -> DomainError {
        map_visa_error(
            error,
            ErrorCode::VisaReadFailed,
            ErrorCategory::WriteFailed,
            "VISA read failed.",
            "Check read timeout, termination settings, and device output, then retry.",
        )
    }

    fn map_io_write_error(error: std::io::Error) -> DomainError {
        DomainError::new(
            ErrorCategory::WriteFailed,
            ErrorCode::VisaWriteFailed,
            "VISA write failed.",
            "Check termination settings, timeout, and resource state, then retry.",
            false,
        )
        .with_detail("backend", serde_json::json!("visa-rs"))
        .with_detail("raw_error", serde_json::json!(error.to_string()))
    }

    fn map_io_read_error(error: std::io::Error) -> DomainError {
        let raw = error.to_string();
        let lowered = raw.to_ascii_lowercase();
        if matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ) || lowered.contains("timeout")
            || lowered.contains("timed out")
            || lowered.contains("tmo")
        {
            return DomainError::new(
                ErrorCategory::ReadTimeout,
                ErrorCode::ReadTimeout,
                "No data was available before the VISA read timeout elapsed.",
                "Retry, increase io_timeout_ms, or check that the device sent data.",
                true,
            )
            .with_detail("backend", serde_json::json!("visa-rs"))
            .with_detail("raw_error", serde_json::json!(raw));
        }
        DomainError::new(
            ErrorCategory::WriteFailed,
            ErrorCode::VisaReadFailed,
            "VISA read failed.",
            "Check read timeout, termination settings, and device output, then retry.",
            false,
        )
        .with_detail("backend", serde_json::json!("visa-rs"))
        .with_detail("raw_error", serde_json::json!(raw))
    }

    fn map_visa_error(
        error: visa_rs::Error,
        code: ErrorCode,
        category: ErrorCategory,
        message: impl Into<String>,
        recovery_hint: impl Into<String>,
    ) -> DomainError {
        let raw = error.to_string();
        let mut mapped = DomainError::new(category, code, message, recovery_hint, false)
            .with_detail("backend", serde_json::json!("visa-rs"))
            .with_detail("raw_error", serde_json::json!(raw.clone()));
        let lowered = raw.to_ascii_lowercase();
        if lowered.contains("lock") || lowered.contains("busy") {
            mapped = DomainError::new(
                ErrorCategory::ResourceBusy,
                ErrorCode::VisaResourceBusy,
                "VISA resource is busy.",
                "Release the owning instance or close the external instrument session, then retry.",
                false,
            )
            .with_detail("backend", serde_json::json!("visa-rs"))
            .with_detail("raw_error", serde_json::json!(raw));
        } else if matches!(code, ErrorCode::VisaOpenFailed)
            && (lowered.contains("timeout") || lowered.contains("tmo"))
        {
            mapped.category = ErrorCategory::ConnectTimeout;
            mapped.code = ErrorCode::VisaOpenTimeout;
            mapped.message = "VISA operation timed out.".to_owned();
            mapped.recovery_hint =
                "Check the resource address, device power, and timeout settings, then retry."
                    .to_owned();
        } else if matches!(code, ErrorCode::VisaOpenFailed | ErrorCode::VisaEnumFailed)
            && (lowered.contains("not found") || lowered.contains("invalid"))
        {
            mapped.code = ErrorCode::VisaResourceNotFound;
            mapped.category = ErrorCategory::InvalidArgument;
            mapped.message = "VISA resource was not found.".to_owned();
            mapped.recovery_hint =
                "Use port_scan(type=Visa) to confirm the address or correct the resource_address."
                    .to_owned();
        }
        mapped
    }

    pub fn feature_not_compiled(tool: &str) -> DomainError {
        DomainError::feature_not_compiled("visa", tool)
    }

    #[cfg(test)]
    mod tests {
        use super::map_io_read_error;
        use crate::model::{ErrorCategory, ErrorCode};

        #[test]
        fn unit_visa_io_read_timeout_maps_to_read_timeout() {
            let error = map_io_read_error(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "operation timed out",
            ));

            assert_eq!(error.category, ErrorCategory::ReadTimeout);
            assert_eq!(error.code, ErrorCode::ReadTimeout);
            assert!(error.retryable);
        }
    }
}

#[cfg(not(feature = "visa"))]
mod compiled {
    use serde::Serialize;

    use crate::model::{DomainError, VisaConfig};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct VisaResourceSummary {
        pub resource_address: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub resource_class: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct VisaScanResult {
        pub resources: Vec<VisaResourceSummary>,
    }

    pub struct VisaWorker;

    impl VisaWorker {
        pub fn open(_config: &VisaConfig) -> std::result::Result<Self, DomainError> {
            Err(DomainError::feature_not_compiled("visa", "port_connect"))
        }

        pub fn write(&self, _bytes: &[u8]) -> std::result::Result<usize, DomainError> {
            Err(DomainError::feature_not_compiled("visa", "port_send"))
        }

        pub fn read(&self, _max_bytes: usize) -> std::result::Result<Vec<u8>, DomainError> {
            Err(DomainError::feature_not_compiled("visa", "port_pull"))
        }

        pub fn clear(&self) -> std::result::Result<(), DomainError> {
            Err(DomainError::feature_not_compiled("visa", "port_clear"))
        }

        pub fn close(self) -> std::result::Result<(), DomainError> {
            let _ = self;
            Err(DomainError::feature_not_compiled("visa", "port_disconnect"))
        }
    }

    pub fn scan_visa_resources(
        _resource_filter: &str,
        _max_results: usize,
    ) -> std::result::Result<VisaScanResult, DomainError> {
        Err(DomainError::feature_not_compiled("visa", "port_scan"))
    }

    pub fn feature_not_compiled(tool: &str) -> DomainError {
        DomainError::feature_not_compiled("visa", tool)
    }
}

pub use compiled::{VisaResourceSummary, VisaScanResult, VisaWorker, scan_visa_resources};
