#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullResult {
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub remaining_rx_buffer_bytes: usize,
    pub source: Option<PullSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullSource {
    pub transport: String,
    pub peer_id: String,
    pub remote_addr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearTarget {
    Tx,
    Rx,
    All,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClearResult {
    pub dropped_tx_items: usize,
    pub dropped_tx_bytes: usize,
    pub dropped_rx_bytes: usize,
}
