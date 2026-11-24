//! Output target and registration leaf for `pty::output`: the `pty_output` payload and emit
//! error, the registered output target and its token, parser availability, and the reader
//! operation gate. Imports nothing from `super`, so the children of `pty::output` can share
//! these types without a module cycle.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use uuid::Uuid;

/// A window label, exactly as Tauri reports it for the calling webview. It is never a value
/// JavaScript chooses, so a frontend cannot forge one or attach on another window's behalf.
pub(super) type WindowLabel = String;

/// Ingest-thread flush threshold for the per-session coalescing accumulator. Reaching it
/// inside one 16 ms window flushes on the reader thread, and that is the only backpressure
/// left on this path: a slow emit there blocks that session's reader, never the parser mutex
/// the whole backend shares.
pub(super) const UI_BATCH_LIMIT_BYTES: usize = 65_536;

/// Output emission failure without transport details or sensitive output bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PtyOutputEmitError {
    Emit,
}

/// The `pty_output` wire payload.
///
/// `sequence` is a number, not a canonical decimal string: the client reconciles a live event
/// against its seed with `sequence <= snapshot.sequence`, and on strings that comparison is
/// lexicographic, so `"9" <= "10"` is false and the terminal corrupts silently past sequence
/// 9. u64 precision is not a concern below 2^53 output events.
///
/// The field is absent when the parser is unavailable, and the client then writes those bytes
/// live with no reconcile: live PTY bytes are never gated. Emitting nothing for a
/// faulted parser is what made the terminal permanently black.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyOutputPayload {
    pub(crate) session_id: String,
    pub(crate) data: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sequence: Option<u64>,
}

#[derive(Clone)]
pub(crate) struct PtyOutputRegistrationToken {
    pub(super) identity: Arc<RegistrationIdentity>,
}

pub(super) struct RegistrationIdentity {
    pub(super) session_id: Uuid,
    pub(super) fanout_identity: Arc<()>,
}

pub(super) struct RegisteredPtyOutputTarget {
    pub(super) session_id: Uuid,
    pub(super) fanout_identity: Arc<()>,
    pub(super) identity: Arc<RegistrationIdentity>,
    pub(super) target: PtyOutputTarget,
}

impl RegisteredPtyOutputTarget {
    pub(super) fn matches_token(
        &self,
        token: &PtyOutputRegistrationToken,
        fanout_identity: &Arc<()>,
    ) -> bool {
        Arc::ptr_eq(&self.identity, &token.identity)
            && Arc::ptr_eq(&self.fanout_identity, fanout_identity)
    }
}

/// One emit per attached window label, fallible as a whole.
pub(super) type PtyOutputEmitFn =
    dyn Fn(&[String], PtyOutputPayload) -> Result<(), PtyOutputEmitError> + Send + Sync;

pub(crate) type PtyOutputTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

#[derive(Clone)]
pub(crate) struct PtyOutputTarget {
    pub(super) emit_pty_output: Arc<PtyOutputEmitFn>,
    pub(super) spawn_task: Arc<dyn Fn(PtyOutputTask) + Send + Sync + 'static>,
}

#[cfg(test)]
pub(crate) type PtyOutputTestEvent = (String, Vec<u8>, Option<u64>);

#[cfg(test)]
pub(crate) type PtyOutputTestSink = Arc<Mutex<Vec<PtyOutputTestEvent>>>;

impl PtyOutputTarget {
    pub(crate) fn new<E, S>(emit: E, spawn: S) -> Self
    where
        E: Fn(&[String], PtyOutputPayload) -> Result<(), PtyOutputEmitError>
            + Send
            + Sync
            + 'static,
        S: Fn(PtyOutputTask) + Send + Sync + 'static,
    {
        Self {
            emit_pty_output: Arc::new(emit),
            spawn_task: Arc::new(spawn),
        }
    }

    pub(super) fn spawn_flush(&self, task: PtyOutputTask) {
        (self.spawn_task)(task);
    }

    #[cfg(test)]
    pub(crate) fn noop() -> Self {
        Self::new(|_, _| Ok(()), drop)
    }

    #[cfg(test)]
    pub(crate) fn from_test_sink(sink: PtyOutputTestSink) -> Self {
        Self::new(
            move |_labels, payload| {
                sink.lock()
                    .unwrap()
                    .push((payload.session_id, payload.data, payload.sequence));
                Ok(())
            },
            drop,
        )
    }

    #[cfg(test)]
    pub(super) fn failing_test_sink(sink: PtyOutputTestSink) -> Self {
        let target = Self::from_test_sink(sink);
        Self::new(
            move |labels, payload| {
                let _ = target.emit_pty_output(labels, payload);
                Err(PtyOutputEmitError::Emit)
            },
            drop,
        )
    }

    pub(super) fn emit_pty_output(
        &self,
        labels: &[WindowLabel],
        payload: PtyOutputPayload,
    ) -> Result<(), PtyOutputEmitError> {
        (self.emit_pty_output)(labels, payload)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ParserAvailability {
    Available,
    Unavailable,
}

#[derive(Default)]
pub(super) struct ReaderOperationGateState {
    pub(super) lifecycle_closing: bool,
    pub(super) admitted_count: usize,
}

pub(super) struct ReaderOperationGate {
    pub(super) state: Mutex<ReaderOperationGateState>,
    pub(super) drained: Condvar,
}

impl ReaderOperationGate {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ReaderOperationGateState::default()),
            drained: Condvar::new(),
        })
    }

    pub(super) fn acquire(self: &Arc<Self>) -> Option<ReaderOperationLease> {
        let mut state = self.state.lock().ok()?;
        if state.lifecycle_closing {
            return None;
        }
        state.admitted_count = state.admitted_count.checked_add(1)?;
        Some(ReaderOperationLease {
            gate: Arc::clone(self),
            completed: false,
        })
    }

    pub(super) fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.lifecycle_closing = true;
        if state.admitted_count == 0 {
            self.drained.notify_all();
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| !state.lifecycle_closing)
    }

    pub(super) fn wait_for_drain(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.admitted_count != 0 {
            state = self
                .drained
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub(super) fn complete(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.admitted_count == 0 {
            log::error!("[terminal-output] reader lease underflow");
            return;
        }
        state.admitted_count -= 1;
        if state.admitted_count == 0 {
            self.drained.notify_all();
        }
    }
}

pub(super) struct ReaderOperationLease {
    pub(super) gate: Arc<ReaderOperationGate>,
    pub(super) completed: bool,
}

impl ReaderOperationLease {
    pub(super) fn complete(mut self) {
        if !self.completed {
            self.gate.complete();
            self.completed = true;
        }
    }
}

impl Drop for ReaderOperationLease {
    fn drop(&mut self) {
        if !self.completed {
            self.gate.complete();
            self.completed = true;
        }
    }
}
