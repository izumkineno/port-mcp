use std::{cell::Cell, fmt};

use serde::{Deserialize, Serialize};

use super::InstanceType;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandleId(String);

impl HandleId {
    pub fn new_for_type(instance_type: InstanceType, sequence: u64) -> Self {
        Self(format!("h_{}_{sequence:03}", instance_type.handle_prefix()))
    }

    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HandleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    pub fn new(sequence: u64) -> Self {
        Self(format!("req_20260526_{sequence:06}"))
    }

    pub fn from_parts(date: &str, sequence: u64) -> Self {
        Self(format!("req_{date}_{sequence:06}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ErrorId(String);

impl ErrorId {
    pub fn from_parts(date: &str, sequence: u64) -> Self {
        Self(format!("err_{date}_{sequence:06}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(String);

impl Timestamp {
    pub fn now_utc() -> Self {
        let now = time::OffsetDateTime::now_utc();
        let timestamp = now
            .format(&time::format_description::well_known::Rfc3339)
            .expect("UTC timestamp should format as RFC3339");
        Self(normalize_rfc3339(timestamp))
    }

    pub fn from_rfc3339(value: &str) -> Result<Self, time::error::Parse> {
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)?;
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn normalize_rfc3339(value: String) -> String {
    if let Some(stripped) = value.strip_suffix("+00:00") {
        format!("{stripped}Z")
    } else {
        value
    }
}

pub struct IdGenerator {
    date: String,
    request_counter: Cell<u64>,
    error_counter: Cell<u64>,
    serial_counter: Cell<u64>,
    tcp_counter: Cell<u64>,
    udp_counter: Cell<u64>,
}

impl IdGenerator {
    pub fn new_for_tests(date: &str) -> Self {
        Self {
            date: date.to_owned(),
            request_counter: Cell::new(0),
            error_counter: Cell::new(0),
            serial_counter: Cell::new(0),
            tcp_counter: Cell::new(0),
            udp_counter: Cell::new(0),
        }
    }

    pub fn next_request_id(&self) -> RequestId {
        let sequence = self.next(&self.request_counter);
        RequestId::from_parts(&self.date, sequence)
    }

    pub fn next_error_id(&self) -> ErrorId {
        let sequence = self.next(&self.error_counter);
        ErrorId::from_parts(&self.date, sequence)
    }

    pub fn next_handle_id(&self, instance_type: InstanceType) -> HandleId {
        let counter = match instance_type {
            InstanceType::Serial => &self.serial_counter,
            InstanceType::Tcp => &self.tcp_counter,
            InstanceType::Udp => &self.udp_counter,
        };
        HandleId::new_for_type(instance_type, self.next(counter))
    }

    fn next(&self, counter: &Cell<u64>) -> u64 {
        let sequence = counter.get() + 1;
        counter.set(sequence);
        sequence
    }
}
