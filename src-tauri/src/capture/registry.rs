//! The per-session owner of the `(Sender, CaptureSlot)` pair (#2232 phase 3).
//!
//! Round 1 left the sender unowned across all nine phases, so every phase could
//! merge green while production emitted nothing. The chain, named end to end:
//!
//! | Link | Phase |
//! | --- | --- |
//! | Watcher holds `Option<Sender>`, `None` everywhere | 1 |
//! | `CaptureRegistry` type and per-session `(tx, slot)` pair | **3** (here) |
//! | `lib.rs` creates the registry; the supervisor calls `open()` | 4 |
//! | and passes `Some(tx)` into `spawn_watch_task` for both watchers | 4 |
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
mod tests;
