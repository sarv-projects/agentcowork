//! Bounded in-process channel with backpressure (P0.5).
//!
//! The supervisor's notification pump (sidecar bytes → app) and the request
//! outbox both need a fixed-capacity queue: producers that outrun consumers
//! must **block** (backpressure), never buffer unboundedly. This wraps
//! `std::sync::mpsc::sync_channel` at the capacity the spec fixes (16).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{
    self, Receiver, RecvError, SendError, SyncSender, TryRecvError, TrySendError,
};

/// Fixed capacity for the IPC notification/request channel (spec P0.5).
pub const DEFAULT_CAPACITY: usize = 16;

/// A bounded, multi-producer single-consumer channel.
///
/// [`BoundedChannel::send`] blocks when the channel is full — that blocking
/// *is* the backpressure (a slow consumer throttles fast producers). The
/// queue length is tracked in an atomic counter (`mpsc::Receiver` does not
/// expose one).
pub struct BoundedChannel<T> {
    sender: SyncSender<T>,
    receiver: Receiver<T>,
    capacity: usize,
    len: Arc<AtomicUsize>,
}

impl<T> BoundedChannel<T> {
    /// Create a channel with the given capacity (clamped to ≥ 1).
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (sender, receiver) = mpsc::sync_channel(capacity);
        Self {
            sender,
            receiver,
            capacity,
            len: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The fixed capacity of this channel.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Send, **blocking** while the channel is full — the backpressure point.
    /// The counter is bumped only once the item is actually enqueued, so a
    /// blocked sender never skews the reported length.
    pub fn send(&self, item: T) -> Result<(), SendError<T>> {
        match self.sender.send(item) {
            Ok(()) => {
                self.len.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Non-blocking send; `Full` when at capacity (caller may drop or wait).
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        match self.sender.try_send(item) {
            Ok(()) => {
                self.len.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Receive, blocking until an item arrives or all senders are dropped.
    pub fn recv(&self) -> Result<T, RecvError> {
        match self.receiver.recv() {
            Ok(v) => {
                self.len.fetch_sub(1, Ordering::Relaxed);
                Ok(v)
            }
            Err(e) => Err(e),
        }
    }

    /// Non-blocking receive.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.receiver.try_recv() {
            Ok(v) => {
                self.len.fetch_sub(1, Ordering::Relaxed);
                Ok(v)
            }
            Err(e) => Err(e),
        }
    }

    /// Clone the sending half. `SyncSender` is clonable (unlike the
    /// receiver), so producers can be spawned onto threads. Note: sends via a
    /// clone do not update this struct's length counter (only sends through
    /// [`BoundedChannel::send`] / [`BoundedChannel::try_send`] do).
    pub fn sender(&self) -> SyncSender<T> {
        self.sender.clone()
    }

    /// Items currently queued (not yet consumed).
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True when the queue is at capacity (a `try_send` would return `Full`).
    pub fn is_full(&self) -> bool {
        self.len() == self.capacity
    }
}

impl<T> Default for BoundedChannel<T> {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn default_capacity_is_16() {
        let ch = BoundedChannel::<u8>::default();
        assert_eq!(ch.capacity(), DEFAULT_CAPACITY);
        assert_eq!(ch.capacity(), 16);
    }

    #[test]
    fn accepts_up_to_capacity_without_blocking() {
        let ch = BoundedChannel::new(16);
        for i in 0u8..16 {
            assert!(ch.try_send(i).is_ok(), "item {i} should fit");
        }
        // 17th try_send → Full (backpressure without blocking).
        match ch.try_send(99u8) {
            Err(TrySendError::Full(_)) => {}
            other => panic!("expected Full, got {other:?}"),
        }
        assert_eq!(ch.len(), 16);
        assert!(ch.is_full());
    }

    #[test]
    fn blocking_send_unblocks_after_drain() {
        let ch = BoundedChannel::<i32>::new(1);
        assert!(ch.try_send(1).is_ok());
        let tx = ch.sender();
        let handle = thread::spawn(move || {
            tx.send(2).expect("send should eventually succeed");
            tx.send(3).expect("send should eventually succeed");
        });
        // Give the producer time to block on `2` (channel full with `1`).
        thread::sleep(Duration::from_millis(50));
        assert_eq!(ch.recv().unwrap(), 1);
        assert_eq!(ch.recv().unwrap(), 2);
        assert_eq!(ch.recv().unwrap(), 3);
        handle.join().unwrap();
    }

    #[test]
    fn recv_blocks_until_item_arrives() {
        let ch = BoundedChannel::<i32>::new(4);
        let tx = ch.sender();
        // The receiver is not shareable, so the blocking consumer owns `ch`
        // while the main thread produces through a cloned sender.
        let handle = thread::spawn(move || ch.recv().unwrap());
        thread::sleep(Duration::from_millis(30));
        tx.send(42).unwrap();
        assert_eq!(handle.join().unwrap(), 42);
    }

    #[test]
    fn try_recv_empty_reports_empty() {
        let ch = BoundedChannel::<u8>::new(4);
        assert!(matches!(ch.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn capacity_zero_clamps_to_one() {
        let ch = BoundedChannel::<u8>::new(0);
        assert_eq!(ch.capacity(), 1);
    }
}
