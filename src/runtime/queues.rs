#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendResult {
    pub queued: bool,
    pub sent_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushResult {
    pub frames: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendItem {
    pub(crate) bytes: Vec<u8>,
}
