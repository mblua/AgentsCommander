//! The lock-free per-session armed flag (#2232 phase 3).
//!
//! `is_armed` is called from the `IdleDetector` callback, which runs on a
//! native watcher or PTY-read thread and emits `session_idle` **synchronously**.
//! It must therefore never block, never touch disk and never take an async
//! lock: it does a `try_read` on the map and an `Acquire` load on the flag, and
//! returns `false` if the map is momentarily contended. Failing closed shows
//! the normal waiting dot for one edge, which is the safe direction.
//!
//! Phase 7 raises the flag when the session is Co-managed-`Ready` **and** the
//! slot holds an unconsumed candidate, and clears it after consumption,
//! invalidation or removal. This phase ships the type and its tests only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Debug, Default)]
pub struct ArmedFlags {
    flags: RwLock<HashMap<String, Arc<AtomicBool>>>,
}

impl ArmedFlags {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create the flag handle for `session_id`.
    ///
    /// This is the writer side and it may block; it is never called from the
    /// idle callback.
    pub fn handle(&self, session_id: &str) -> Arc<AtomicBool> {
        if let Ok(flags) = self.flags.read() {
            if let Some(flag) = flags.get(session_id) {
                return Arc::clone(flag);
            }
        }
        let mut flags = self.flags.write().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            flags
                .entry(session_id.to_owned())
                .or_insert_with(|| Arc::new(AtomicBool::new(false))),
        )
    }

    /// The idle-callback read. Never blocks, never panics, never allocates a
    /// map entry: an unknown session and a contended map are both `false`.
    pub fn is_armed(&self, session_id: &str) -> bool {
        match self.flags.try_read() {
            Ok(flags) => flags
                .get(session_id)
                .is_some_and(|flag| flag.load(Ordering::Acquire)),
            Err(_) => false,
        }
    }

    pub fn arm(&self, session_id: &str) {
        self.handle(session_id).store(true, Ordering::Release);
    }

    pub fn disarm(&self, session_id: &str) {
        if let Ok(flags) = self.flags.read() {
            if let Some(flag) = flags.get(session_id) {
                flag.store(false, Ordering::Release);
            }
        }
    }

    pub fn remove(&self, session_id: &str) {
        self.disarm(session_id);
        let mut flags = self.flags.write().unwrap_or_else(|e| e.into_inner());
        flags.remove(session_id);
    }
}

#[cfg(test)]
mod tests;
