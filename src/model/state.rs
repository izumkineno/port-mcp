use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceType {
    Serial,
    #[serde(rename = "TCP")]
    Tcp,
    #[serde(rename = "UDP")]
    Udp,
}

impl InstanceType {
    pub(crate) fn handle_prefix(self) -> &'static str {
        match self {
            Self::Serial => "ser",
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceState {
    Created,
    Configured,
    Connected,
    Disconnecting,
    Disconnected,
    Error,
    Released,
}

impl InstanceState {
    pub const fn is_persistent(self) -> bool {
        !matches!(self, Self::Released)
    }
}
