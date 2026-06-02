#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendResult {
    pub queued: bool,
    pub sent_bytes: usize,
    pub target: Option<SendTargetSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendTargetSummary {
    pub mode: SendTargetMode,
    pub peer_id: Option<String>,
    pub peer_count: usize,
    pub successful_peer_ids: Vec<String>,
    pub failed_peer_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTargetMode {
    Peer,
    Broadcast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushResult {
    pub frames: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendItem {
    pub(crate) bytes: Vec<u8>,
}
