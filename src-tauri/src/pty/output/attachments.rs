use super::{
    PtyOutputEmitError, PtyOutputPayload, RegisteredPtyOutputTarget, UI_BATCH_LIMIT_BYTES,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// A window label, exactly as Tauri reports it for the calling webview. It is never a value
/// JavaScript chooses, so a frontend cannot forge one or attach on another window's behalf.
pub(super) type WindowLabel = String;

/// The emission gate: a map from session id to the set of window labels watching it, where a
/// session's attach count is `set.len()`.
///
/// The count is what stops a second window from stealing or releasing the first window's
/// delivery, which is the whole of #1363. The label is what makes the count count OWNERS
/// rather than CALLS: a bare counter cannot tell a decrement owed by window 1 from one issued
/// by window 2, so a single over-detach out of the frontend's several disposal paths would
/// silently mute a session another window is still watching. With labels every one of those
/// paths degrades into a window-local no-op, and two further things become possible that a
/// counter cannot express - reaping a dead window's attachments in the backend, and scoping
/// the emit with `emit_to`.
pub(crate) struct TerminalOutputAttachments {
    pub(super) state: Mutex<AttachmentState>,
}

#[derive(Default)]
pub(super) struct AttachmentState {
    pub(super) attached: HashMap<Uuid, HashSet<WindowLabel>>,
    /// Per-session coalescing buffers, present only while a session has an attachment. The
    /// outer map is locked for lookup only: holding it across an emit would let one flooding
    /// session throttle every other session's ingest.
    pub(super) accumulators: HashMap<Uuid, Arc<Mutex<SessionAccumulator>>>,
}

/// One session's pending batch.
///
/// Two emitters drain it, the 64 KiB threshold flush on the ingest thread and the 16 ms timer
/// flush on a Tokio task, and both do the drain AND the emit while holding this mutex. A drain
/// that released the mutex before emitting would let the two interleave a session's bytes,
/// which is a silently corrupted terminal.
pub(super) struct SessionAccumulator {
    pub(super) registration: Arc<RegisteredPtyOutputTarget>,
    pub(super) data: Vec<u8>,
    /// The batch's LAST sequence, or `None` once the parser is unavailable. A batch never
    /// mixes the two: the accumulator is flushed at the `Available -> Unavailable` transition,
    /// so one scalar always describes the whole batch. Labelled with the last real sequence, a
    /// mixed batch would make the client watermark-drop bytes that were never seeded; labelled
    /// `None`, its sequenced prefix would escape reconciliation and duplicate against the seed.
    pub(super) sequence: Option<u64>,
    /// A one-shot timer task is pending for this session. Only the timer path clears it, which
    /// is what keeps exactly one task in flight: a threshold flush leaves the flag set, so the
    /// chunk that follows it cannot arm a second timer behind the one still sleeping. Clearing
    /// it on every flush let a flooding session emit its ceiling flushes AND a timer flush
    /// behind each of them, up to twice the event rate criterion E' bounds.
    pub(super) timer_armed: bool,
    /// Whether the last emit failed. A webview torn down mid-flood makes every flush fail, and
    /// logging one line per flush would be ~62 lines/s per session, which would itself read as
    /// the bug. Only the transition into the failing state and the recovery are logged.
    pub(super) emit_failing: bool,
}

impl SessionAccumulator {
    pub(super) fn new(registration: Arc<RegisteredPtyOutputTarget>) -> Self {
        Self {
            registration,
            data: Vec::new(),
            sequence: None,
            timer_armed: false,
            emit_failing: false,
        }
    }

    pub(super) fn discard(&mut self) {
        self.data.clear();
        self.sequence = None;
    }

    /// Drains the batch and emits it to `labels`, under the caller's lock on this accumulator.
    /// With no label left the bytes are dropped rather than deferred: they must not surface on
    /// a later re-attach, out of order and after that attach's reset.
    pub(super) fn flush(&mut self, labels: &[WindowLabel]) {
        if self.data.is_empty() || labels.is_empty() {
            self.discard();
            return;
        }
        let session_id = self.registration.session_id;
        let payload = PtyOutputPayload {
            session_id: session_id.to_string(),
            data: std::mem::take(&mut self.data),
            sequence: self.sequence.take(),
        };
        match self.registration.target.emit_pty_output(labels, payload) {
            Ok(()) => {
                if self.emit_failing {
                    self.emit_failing = false;
                    log::info!("[terminal-output] session {session_id} delivery recovered");
                }
            }
            Err(PtyOutputEmitError::Emit) => {
                if !self.emit_failing {
                    self.emit_failing = true;
                    log::warn!("[terminal-output] session {session_id} delivery is failing");
                }
            }
        }
    }
}

/// What the ingest must still do once it has dropped the `screen_parsers` lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Accumulated {
    /// No window is attached to this session, so nothing was retained. At 20-30 sessions the
    /// alternative would hold up to 30 x 64 KiB of bytes nobody will ever receive.
    Unattached,
    /// Appended, and a flush is already scheduled.
    Pending,
    /// Appended, and this session's one-shot 16 ms timer must be armed.
    ArmTimer,
    /// Appended, and the batch reached its ceiling: flush now, on the reader thread.
    FlushNow,
}

impl TerminalOutputAttachments {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(AttachmentState::default()),
        })
    }

    /// The gate must never stop gating because a thread panicked while holding it: a poisoned
    /// map that refused every read would leave every terminal black.
    pub(super) fn lock_state(&self) -> std::sync::MutexGuard<'_, AttachmentState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn lock_accumulator(
        accumulator: &Arc<Mutex<SessionAccumulator>>,
    ) -> std::sync::MutexGuard<'_, SessionAccumulator> {
        accumulator
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn labels_of(state: &AttachmentState, session_id: Uuid) -> Vec<WindowLabel> {
        state
            .attached
            .get(&session_id)
            .map(|labels| labels.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Records that `label`'s window is watching this session. The caller holds
    /// `screen_parsers` and has already decided to succeed, which is what makes the attach
    /// transactional: no error path can leak an attachment, so no guard and no compensating
    /// release exist.
    ///
    /// Whatever the session had pending is flushed first, to the windows attached BEFORE this
    /// one. Every byte of it carries a sequence at or below the snapshot this attach is about
    /// to read, so the arriving window loses nothing by not receiving it - but a window
    /// already watching this session has not received it yet, and discarding it would punch a
    /// silent hole in that window's output. The accumulator only ever holds bytes for a session
    /// that already has an attachment, so on a first attach there is nothing here at all and
    /// this costs nothing.
    pub(super) fn attach(&self, session_id: Uuid, label: &str) {
        let mut state = self.lock_state();
        let pending = state.accumulators.get(&session_id).map(Arc::clone);
        let labels = Self::labels_of(&state, session_id);
        state
            .attached
            .entry(session_id)
            .or_default()
            .insert(label.to_string());
        drop(state);
        if let Some(pending) = pending {
            Self::lock_accumulator(&pending).flush(&labels);
        }
    }

    /// Releases `label`'s attachment. Detaching a session this window never attached, and
    /// detaching twice, are window-local no-ops, and neither can disturb another window.
    pub(super) fn detach(&self, session_id: Uuid, label: &str) {
        let orphaned = {
            let mut state = self.lock_state();
            let Some(labels) = state.attached.get_mut(&session_id) else {
                return;
            };
            if !labels.remove(label) || !labels.is_empty() {
                return;
            }
            state.attached.remove(&session_id);
            state.accumulators.remove(&session_id)
        };
        Self::discard_orphaned(orphaned);
    }

    /// A window was destroyed: release every attachment it held. This runs with no frontend
    /// cooperation, which is what makes the frontend detach a bandwidth optimization rather
    /// than a correctness dependency, and what lets the close hook avoid blocking the close.
    pub(super) fn release_window(&self, label: &str) {
        let orphaned = {
            let mut state = self.lock_state();
            let emptied = state
                .attached
                .iter_mut()
                .filter_map(|(session_id, labels)| {
                    (labels.remove(label) && labels.is_empty()).then_some(*session_id)
                })
                .collect::<Vec<_>>();
            let mut orphaned = Vec::new();
            for session_id in emptied {
                state.attached.remove(&session_id);
                if let Some(accumulator) = state.accumulators.remove(&session_id) {
                    orphaned.push(accumulator);
                }
            }
            orphaned
        };
        for accumulator in orphaned {
            Self::lock_accumulator(&accumulator).discard();
        }
    }

    /// The session is gone: drop its attachments and its pending bytes unconditionally. This
    /// is the release that makes "session destroy releases attachments" true in the backend
    /// however the frontend behaves.
    pub(super) fn remove_session(&self, session_id: Uuid) {
        let orphaned = {
            let mut state = self.lock_state();
            state.attached.remove(&session_id);
            state.accumulators.remove(&session_id)
        };
        Self::discard_orphaned(orphaned);
    }

    /// Always outside the outer lock: an in-flight emit holds the accumulator mutex, and
    /// waiting for it with the map locked would stall every other session's ingest.
    pub(super) fn discard_orphaned(orphaned: Option<Arc<Mutex<SessionAccumulator>>>) {
        if let Some(accumulator) = orphaned {
            Self::lock_accumulator(&accumulator).discard();
        }
    }

    /// Ingest side of the gate: appends one chunk to the session's batch.
    ///
    /// The caller holds `screen_parsers`, and that is what makes the batch boundary atomic
    /// with the sequence assignment. No chunk can be appended between the attach's snapshot
    /// read and the attach's drain, so no batch can straddle the snapshot and the scalar
    /// `sequence` is sufficient to describe one. The emit is deliberately NOT done here: it is
    /// reported back so the caller can run it after dropping that lock, because a `serde_json`
    /// emit under the parser mutex would stall every session's ingest.
    ///
    /// `parser_fault` marks the `Available -> Unavailable` transition and flushes what is
    /// already accumulated, so the unsequenced chunk that follows cannot share a batch with
    /// sequenced bytes.
    #[must_use]
    pub(super) fn accumulate(
        &self,
        registration: &Arc<RegisteredPtyOutputTarget>,
        sequence: Option<u64>,
        data: &[u8],
        parser_fault: bool,
    ) -> Accumulated {
        let session_id = registration.session_id;
        let (accumulator, labels) = {
            let mut state = self.lock_state();
            let labels = Self::labels_of(&state, session_id);
            if labels.is_empty() {
                return Accumulated::Unattached;
            }
            let accumulator =
                Arc::clone(state.accumulators.entry(session_id).or_insert_with(|| {
                    Arc::new(Mutex::new(SessionAccumulator::new(Arc::clone(
                        registration,
                    ))))
                }));
            (accumulator, labels)
        };

        let mut pending = Self::lock_accumulator(&accumulator);
        if parser_fault {
            pending.flush(&labels);
        }
        pending.data.extend_from_slice(data);
        pending.sequence = sequence;
        if pending.data.len() >= UI_BATCH_LIMIT_BYTES {
            return Accumulated::FlushNow;
        }
        if pending.timer_armed {
            return Accumulated::Pending;
        }
        pending.timer_armed = true;
        Accumulated::ArmTimer
    }

    /// The flush, and the synchronous seam every backend test drives instead of sleeping.
    ///
    /// It emits to the windows attached right now, and does nothing once the session is
    /// detached or destroyed, because those release sites drain the accumulator rather than
    /// leaving bytes for a later timer to deliver.
    ///
    /// `from_timer` marks the one-shot task firing, and only that path clears `timer_armed`.
    pub(super) fn flush(&self, session_id: Uuid, from_timer: bool) {
        let (accumulator, labels) = {
            let state = self.lock_state();
            let Some(accumulator) = state.accumulators.get(&session_id).map(Arc::clone) else {
                return;
            };
            (accumulator, Self::labels_of(&state, session_id))
        };
        let mut pending = Self::lock_accumulator(&accumulator);
        if from_timer {
            pending.timer_armed = false;
        }
        pending.flush(&labels);
    }

    #[cfg(test)]
    pub(super) fn labels_for_test(&self, session_id: Uuid) -> Vec<WindowLabel> {
        let mut labels = Self::labels_of(&self.lock_state(), session_id);
        labels.sort();
        labels
    }

    #[cfg(test)]
    pub(super) fn pending_bytes_for_test(&self, session_id: Uuid) -> Option<usize> {
        let accumulator = self
            .lock_state()
            .accumulators
            .get(&session_id)
            .map(Arc::clone)?;
        let pending = Self::lock_accumulator(&accumulator);
        let bytes = pending.data.len();
        Some(bytes)
    }
}
