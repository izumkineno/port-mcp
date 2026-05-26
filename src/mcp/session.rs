#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    UnverifiedM0Spike,
}

impl SessionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnverifiedM0Spike => "unverified_m0_spike",
        }
    }
}
