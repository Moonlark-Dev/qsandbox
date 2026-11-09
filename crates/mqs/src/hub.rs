//! In-process pub/sub used by the control WebSocket (GUI live view).

use std::sync::Mutex;

use tokio::sync::broadcast;

pub struct Hub {
    sender: broadcast::Sender<serde_json::Value>,
    subscribers: Mutex<usize>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(512);
        Self {
            sender,
            subscribers: Mutex::new(0),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<serde_json::Value> {
        *self.subscribers.lock().unwrap() += 1;
        self.sender.subscribe()
    }

    pub fn unsubscribe(&self) {
        let mut count = self.subscribers.lock().unwrap();
        *count = count.saturating_sub(1);
    }

    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    pub fn publish(&self, payload: &serde_json::Value) {
        let _ = self.sender.send(payload.clone());
    }
}
