//! The per-session owner of the `(Sender, CaptureSlot)` pair (#2232 phase 3).
//!
//! Round 1 left the sender unowned across all nine phases, so every phase could
//! merge green while production emitted nothing. The chain, named end to end:
//!
//! | Link | Phase |
//! | --- | --- |
//! | Watcher holds `Option<Sender>`, `None` everywhere | 1 |
//! | `CaptureRegistry` type and per-session `(tx, slot)` pair | **3** (here) |
//! | `lib.rs` creates the registry; the supervisor calls `open()` and passes `Some(tx)` into `spawn_watch_task` for both watchers | 4 |
//! | The supervisor subscribes to `slot()` and acts | 7 |
//!
//! In this phase **nothing constructs the registry in production**, because no
//! reader is spawned here; the consumer is still a test.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::capture::record::CapturedRecord;
use crate::capture::sink::CaptureSlot;

/// The capture endpoints of one session.
#[derive(Clone, Debug)]
pub struct SessionCapture {
    pub tx: UnboundedSender<Arc<CapturedRecord>>,
    pub slot: CaptureSlot,
}

/// A leaf registry of per-session capture endpoints.
#[derive(Debug, Default)]
pub struct CaptureRegistry {
    sessions: Mutex<HashMap<String, SessionCapture>>,
}

impl CaptureRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create the endpoints for `session_id`.
    ///
    /// Idempotent: two calls for one session id return the same slot and the
    /// same sender. The receiver is handed back **only** on the call that
    /// created the entry, because an unbounded channel has exactly one; a
    /// second `open` for a live session returns `None` for it.
    pub fn open(
        &self,
        session_id: &str,
    ) -> (
        SessionCapture,
        Option<UnboundedReceiver<Arc<CapturedRecord>>>,
    ) {
        let mut sessions = self.lock();
        if let Some(existing) = sessions.get(session_id) {
            return (existing.clone(), None);
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let capture = SessionCapture {
            tx,
            slot: CaptureSlot::new(),
        };
        sessions.insert(session_id.to_owned(), capture.clone());
        (capture, Some(rx))
    }

    pub fn slot(&self, session_id: &str) -> Option<CaptureSlot> {
        self.lock().get(session_id).map(|c| c.slot.clone())
    }

    pub fn sender(&self, session_id: &str) -> Option<UnboundedSender<Arc<CapturedRecord>>> {
        self.lock().get(session_id).map(|c| c.tx.clone())
    }

    /// Drop the entry on the last demand release.
    pub fn close(&self, session_id: &str) {
        self.lock().remove(session_id);
    }

    pub fn is_open(&self, session_id: &str) -> bool {
        self.lock().contains_key(session_id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SessionCapture>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test 23: `open` is idempotent — two calls for one session id return the
    // same slot — and one `close` drops it.
    #[test]
    fn open_is_idempotent_and_close_drops_the_entry() {
        let registry = CaptureRegistry::new();
        let (first, rx) = registry.open("session-a");
        assert!(rx.is_some(), "the creating call owns the receiver");
        let (second, rx_again) = registry.open("session-a");
        assert!(rx_again.is_none(), "a channel has exactly one receiver");

        // The same slot: a transition through one handle is visible on the other.
        first.slot.invalidate("probe");
        assert_eq!(first.slot.seq(), second.slot.seq());
        assert!(matches!(
            second.slot.snapshot().value,
            crate::capture::sink::SlotValue::Invalid(_)
        ));
        assert!(first.tx.same_channel(&second.tx));

        assert!(registry.slot("session-a").is_some());
        registry.close("session-a");
        assert!(!registry.is_open("session-a"));
        assert!(registry.slot("session-a").is_none());
    }

    #[test]
    fn an_unknown_session_has_no_slot_and_no_sender() {
        let registry = CaptureRegistry::new();
        assert!(registry.slot("nobody").is_none());
        assert!(registry.sender("nobody").is_none());
        // `close` on an unknown session is a no-op, not a panic.
        registry.close("nobody");
    }

    #[test]
    fn distinct_sessions_get_distinct_slots() {
        let registry = CaptureRegistry::new();
        let (a, _) = registry.open("a");
        let (b, _) = registry.open("b");
        a.slot.invalidate("only a");
        assert_eq!(b.slot.seq(), 0, "session b must be untouched");
    }
}
