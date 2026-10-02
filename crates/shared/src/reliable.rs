//! Minimal reliable, ordered message channel on top of the unreliable packet stream.
//!
//! Sender: every message gets a sequence number and is resent until acknowledged.
//! Receiver: delivers messages exactly once, in sequence order, buffering early arrivals, and
//! acknowledges cumulatively (`ack_value` = highest seq such that everything up to it arrived).
//! Both ends start at seq 1 for a new session (a new join).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub struct ReliableOut<T> {
    next_seq: u32,
    pending: BTreeMap<u32, Pending<T>>,
    pub sent: u64,
    pub resends: u64,
}

struct Pending<T> {
    msg: T,
    last_sent: Option<Instant>,
}

impl<T: Clone> Default for ReliableOut<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> ReliableOut<T> {
    pub const fn new() -> Self {
        Self { next_seq: 1, pending: BTreeMap::new(), sent: 0, resends: 0 }
    }

    pub fn push(&mut self, msg: T) -> u32 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.pending.insert(seq, Pending { msg, last_sent: None });
        seq
    }

    /// Messages to put on the wire now: never sent, or unacknowledged for `resend_after`.
    pub fn due(&mut self, now: Instant, resend_after: Duration) -> Vec<(u32, T)> {
        let mut out = Vec::new();
        for (seq, p) in self.pending.iter_mut() {
            let send = match p.last_sent {
                None => true,
                Some(t) => now.duration_since(t) >= resend_after,
            };
            if send {
                if p.last_sent.is_some() {
                    self.resends += 1;
                }
                self.sent += 1;
                p.last_sent = Some(now);
                out.push((*seq, p.msg.clone()));
            }
        }
        out
    }

    /// Cumulative acknowledgement: everything up to and including `upto` arrived.
    pub fn ack(&mut self, upto: u32) {
        self.pending.retain(|seq, _| *seq > upto);
    }

    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }
}

pub struct ReliableIn<T> {
    next_expected: u32,
    buffer: BTreeMap<u32, T>,
    pub delivered: u64,
    pub duplicates: u64,
}

impl<T> Default for ReliableIn<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ReliableIn<T> {
    pub const fn new() -> Self {
        Self { next_expected: 1, buffer: BTreeMap::new(), delivered: 0, duplicates: 0 }
    }

    /// Accepts one message; returns the messages that are now deliverable, in order.
    pub fn receive(&mut self, seq: u32, msg: T) -> Vec<T> {
        if seq < self.next_expected || self.buffer.contains_key(&seq) {
            self.duplicates += 1;
            return Vec::new();
        }
        self.buffer.insert(seq, msg);
        let mut out = Vec::new();
        while let Some(m) = self.buffer.remove(&self.next_expected) {
            out.push(m);
            self.next_expected += 1;
        }
        self.delivered += out.len() as u64;
        out
    }

    /// Value to send in an Ack packet.
    pub fn ack_value(&self) -> u32 {
        self.next_expected - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_order_exactly_once_with_loss_reorder_and_duplicates() {
        let mut out = ReliableOut::new();
        let mut inp = ReliableIn::new();
        for i in 0..5 {
            out.push(i);
        }
        let t0 = Instant::now();
        let wire = out.due(t0, Duration::from_millis(100));
        assert_eq!(wire.len(), 5);
        // Lose #2 (index 1), deliver the rest out of order, with a duplicate.
        let mut delivered = Vec::new();
        for (seq, m) in [wire[3], wire[0], wire[4], wire[2], wire[0]] {
            delivered.extend(inp.receive(seq, m));
        }
        assert_eq!(delivered, vec![0]);
        assert_eq!(inp.ack_value(), 1);
        assert_eq!(inp.duplicates, 1);
        out.ack(inp.ack_value());
        assert_eq!(out.in_flight(), 4);
        // Nothing is due again before the resend timeout...
        assert!(out.due(t0 + Duration::from_millis(50), Duration::from_millis(100)).is_empty());
        // ...then the unacked ones are resent and the gap fills.
        let resent = out.due(t0 + Duration::from_millis(150), Duration::from_millis(100));
        assert_eq!(resent.len(), 4);
        for (seq, m) in resent {
            delivered.extend(inp.receive(seq, m));
        }
        assert_eq!(delivered, vec![0, 1, 2, 3, 4]);
        out.ack(inp.ack_value());
        assert_eq!(out.in_flight(), 0);
    }
}
