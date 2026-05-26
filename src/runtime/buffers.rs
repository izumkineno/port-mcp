#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullResult {
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub remaining_rx_buffer_bytes: usize,
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
