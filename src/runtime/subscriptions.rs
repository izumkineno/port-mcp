use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionResult {
    pub was_subscribed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsubscribeResult {
    pub was_subscribed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub payload: Vec<u8>,
    pub truncated: bool,
    pub coalesced: bool,
    pub dropped_notifications: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct Subscriber {
    pub(crate) max_payload_bytes: usize,
    pub(crate) queue: VecDeque<Notification>,
    dropped_notifications: u64,
    dropped_bytes: u64,
    notification_tick: Option<u64>,
    notifications_this_tick: u32,
}

impl Subscriber {
    pub(crate) fn new(_session_id: &str, max_payload_bytes: usize) -> Self {
        Self {
            max_payload_bytes,
            queue: VecDeque::new(),
            dropped_notifications: 0,
            dropped_bytes: 0,
            notification_tick: None,
            notifications_this_tick: 0,
        }
    }

    pub(crate) fn queued_bytes(&self) -> usize {
        self.queue
            .iter()
            .map(|notification| notification.payload.len())
            .sum()
    }

    pub(crate) fn note_tick(&mut self, tick: u64, limit: u32) -> bool {
        if self.notification_tick != Some(tick) {
            self.notification_tick = Some(tick);
            self.notifications_this_tick = 0;
        }
        self.notifications_this_tick >= limit
    }

    pub(crate) fn enqueue(
        &mut self,
        bytes: &[u8],
        queue_limit: usize,
        rate_limited: bool,
    ) -> (usize, usize, u64) {
        let payload_len = bytes.len().min(self.max_payload_bytes);
        let payload = bytes[..payload_len].to_vec();
        let truncated = payload_len < bytes.len();
        if rate_limited {
            let released = self.queued_bytes();
            let dropped = self.queue.len().max(1) as u64;
            self.queue.clear();
            self.dropped_notifications += dropped;
            self.dropped_bytes += bytes.len().saturating_sub(payload_len) as u64;
            self.queue.push_back(Notification {
                payload,
                truncated,
                coalesced: true,
                dropped_notifications: self.dropped_notifications,
            });
            return (payload_len, released, dropped);
        }

        let mut released = 0;
        let mut dropped = 0;
        if self.queue.len() >= queue_limit {
            if let Some(old) = self.queue.pop_front() {
                released += old.payload.len();
                dropped += 1;
                self.dropped_notifications += 1;
            }
        }
        self.dropped_bytes += bytes.len().saturating_sub(payload_len) as u64;
        self.notifications_this_tick += 1;
        self.queue.push_back(Notification {
            payload,
            truncated,
            coalesced: false,
            dropped_notifications: self.dropped_notifications,
        });
        (payload_len, released, dropped)
    }
}
