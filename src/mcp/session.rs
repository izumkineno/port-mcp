#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    RequestContextDebug,
}

impl SessionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RequestContextDebug => "request_context_debug",
        }
    }
}
