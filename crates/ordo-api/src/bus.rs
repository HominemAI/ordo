//! In-memory invalidation fan-out over tokio broadcast channels — one
//! channel per op, created on demand. The command-log listener publishes
//! into the same shape later.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::broadcast;

use crate::spec::{InvalidationBus, InvalidationEvent};

const CHANNEL_CAPACITY: usize = 64;

#[derive(Default)]
pub struct MemoryBus {
    channels: Mutex<HashMap<String, broadcast::Sender<InvalidationEvent>>>,
}

impl MemoryBus {
    pub fn new() -> Self {
        Self::default()
    }

    fn channel(&self, op: &str) -> broadcast::Sender<InvalidationEvent> {
        self.channels
            .lock()
            .unwrap()
            .entry(op.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }

    /// Signal that `op`'s results may be stale. Returns the number of live
    /// subscribers reached (0 when nobody is listening — not an error).
    pub fn publish(&self, op: &str) -> usize {
        self.channel(op)
            .send(InvalidationEvent { op: op.to_string() })
            .unwrap_or(0)
    }
}

impl InvalidationBus for MemoryBus {
    fn subscribe(&self, op_name: &str) -> broadcast::Receiver<InvalidationEvent> {
        self.channel(op_name).subscribe()
    }
}
