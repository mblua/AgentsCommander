//! PTY output fan-out: `SessionIoFanout` feeds session bytes to bridge senders, the idle
//! detector, response watchers, the web broadcaster and screen parsers, and batches UI output.
//! Owns UI history replay, `PtyScreenSnapshot`, and output target/registration types.
//! Submodules: `attachments`, `capture`, `delivery`, `markers`, `screen`.
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod attachments;
mod capture;
mod delivery;
mod markers;
mod screen;
mod target;

#[cfg(test)]
use crate::pty::backend::SessionBackendKind;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output_senders::OutputSenderMap;
use crate::session::profile::IdleTuning;
use attachments::Accumulated;
pub(crate) use attachments::TerminalOutputAttachments;
pub(crate) use capture::CapturedVtScreen;
use capture::{CaptureFailure, ScreenReplayState};
use markers::screen_shows_visible_content;
pub(crate) use markers::{output_has_printable_activity, scan_response_markers};
pub use markers::{ResponseWatcher, ResponseWatcherMap};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;
#[cfg(any(windows, test))]
use std::time::Instant;
#[cfg(test)]
use target::UI_BATCH_LIMIT_BYTES;
use target::{
    ParserAvailability, ReaderOperationGate, ReaderOperationLease, RegisteredPtyOutputTarget,
    RegistrationIdentity,
};
pub(crate) use target::{
    PtyOutputEmitError, PtyOutputRegistrationToken, PtyOutputTarget, PtyOutputTask,
};
#[cfg(test)]
pub(crate) use target::{PtyOutputPayload, PtyOutputTestEvent, PtyOutputTestSink};
#[cfg(test)]
use terminal_snapshot_renderer::{TerminalActiveBuffer, TerminalCellWidth, TerminalColor};
use uuid::Uuid;

#[derive(Clone)]
pub struct SessionIoFanout {
    output_senders: OutputSenderMap,
    idle_detector: Arc<IdleDetector>,
    response_watchers: ResponseWatcherMap,
    ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    screen_parsers: Arc<Mutex<HashMap<Uuid, ScreenReplayState>>>,
    attachments: Arc<TerminalOutputAttachments>,
    /// Whether `arm_output_flush` really spawns the 16 ms one-shot. Production is always on.
    /// Test builds start it off so no assertion races a live task, and the one test that
    /// covers the timer itself switches it on: gating the arming on `cfg(test)` instead
    /// compiled that path out of every test build, and left it with no coverage at all.
    output_timer_enabled: Arc<AtomicBool>,
    fanout_identity: Arc<()>,
    #[cfg(test)]
    trace: FanoutTraceRecorder,
}

#[derive(Clone)]
pub struct PtyScreenSnapshot {
    pub data: Vec<u8>,
    pub rows: u16,
    pub cols: u16,
    pub sequence: u64,
}

/// The UI coalescing window. Its only consumer is the one-shot timer armed per session, which
/// test builds hold so no assertion depends on a wall clock; the flush itself is driven
/// synchronously there, by every test but the timer's own.
const UI_BATCH_INTERVAL_MS: u64 = 16;

/// Raw output bytes retained per session so a freshly created terminal can be rehydrated
/// with history instead of a single viewport. Sized to keep the frontend replay peak close
/// to its current value rather than to double the admission ceiling.
const UI_HISTORY_LIMIT_BYTES: usize = 65_536;
/// How far into the ring the line-boundary trim looks for a newline. A stream without
/// newlines (progress bars redrawing with `\r`) would otherwise walk the whole ring on
/// every chunk, inside the parser mutex shared by every session of the backend.
const UI_HISTORY_LINE_SCAN_BYTES: usize = 4_096;
/// Normalization prologue for a history replay: normal buffer, full scroll region, autowrap
/// on, G0 back to ASCII, default attributes. Deliberately carries no erase sequence:
/// `\x1b[2J`, `\x1b[3J` and RIS each wipe part or all of the history this replay restores.
const UI_HISTORY_REPLAY_PROLOGUE: &[u8] = b"\x1b[?1049l\x1b[r\x1b[?7h\x1b(B\x1b[0m";

/// Appends a chunk to the bounded history ring. The order is mandatory: trim for space,
/// trim to a line boundary, then append. Only the length bound is guaranteed; the line
/// alignment is best effort, because the boundary scan is capped and can find nothing. Its
/// outcome is recorded in `aligned` and enforced at the attach seed instead (#1458).
///
/// Every index is saturating on purpose. `VecDeque::drain(..k)` panics when `k > len`, and
/// here a panic is permanent rather than local: the caller flips the parser to `Unavailable`,
/// which leaves that console dead for the rest of the process.
fn append_history(history: &mut std::collections::VecDeque<u8>, aligned: &mut bool, data: &[u8]) {
    // A chunk larger than the whole ring keeps only its tail. Unreachable in production
    // (the local backend reads 4 KiB buffers, the container backend rejects frames over
    // 64 KiB) but it is where the trim arithmetic gets written wrong.
    let tail = &data[data.len().saturating_sub(UI_HISTORY_LIMIT_BYTES)..];
    let over = (history.len() + tail.len()).saturating_sub(UI_HISTORY_LIMIT_BYTES);
    if over > 0 {
        history.drain(..over.min(history.len()));
        // The scan stays capped: this runs per chunk inside the parser mutex the whole
        // backend shares. What #1458 changes is only that its failure is now RECORDED
        // instead of assumed away, so the cold attach path knows it has work to do.
        match history
            .iter()
            .take(UI_HISTORY_LINE_SCAN_BYTES)
            .position(|byte| *byte == b'\n')
        {
            Some(newline) => {
                history.drain(..=newline);
                *aligned = true;
            }
            None => *aligned = false,
        }
    }
    // #1458: the one path on which the front changes without `over` ever being positive.
    // When `tail` becomes the WHOLE ring the front is `tail[0]`, and a chunk larger than the
    // ring was truncated at an arbitrary byte, so that front is not a line start and nothing
    // above recorded it. The `<` is load bearing: `tail.len() == data.len()` means the chunk
    // was NOT truncated, so `tail[0]` is a real stream boundary and `true` is correct.
    if history.is_empty() && tail.len() < data.len() {
        *aligned = false;
    }
    history.extend(tail);
}

/// The ring sliced from the byte after its first `\n`, or `None` when the ring holds no `\n`
/// at all or holds nothing after it.
///
/// Only the cold attach path calls this, so the scan is unbounded on purpose. `\n` is the one
/// resync point a replay can trust: a parser reading the ring from an arbitrary byte offset
/// renders the tail of whatever escape sequence that offset falls inside as literal text
/// (#1458), and no in-band cancel undoes it, because that parser is already in ground state.
fn history_from_first_line<'a>(front: &'a [u8], back: &'a [u8]) -> Option<(&'a [u8], &'a [u8])> {
    if let Some(newline) = front.iter().position(|byte| *byte == b'\n') {
        let aligned_front = &front[newline + 1..];
        if aligned_front.is_empty() && back.is_empty() {
            return None;
        }
        return Some((aligned_front, back));
    }
    let newline = back.iter().position(|byte| *byte == b'\n')?;
    let aligned_back = &back[newline + 1..];
    if aligned_back.is_empty() {
        return None;
    }
    Some((&[], aligned_back))
}

/// The only two conditions that fail an attach: there is nothing to attach to.
///
/// Every other condition attaches successfully without a snapshot, and that is not a
/// convenience. Under this design the attach IS the emission gate, and `parser_availability`
/// never returns to `Available` once it flips, so refusing to attach an unavailable parser or
/// a failed snapshot read would leave that terminal black for the life of the session with no
/// recovery lane left to repair it. That is #955 verbatim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalOutputAttachError {
    SessionUnavailable,
    OutputTargetUnavailable,
}

impl TerminalOutputAttachError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::SessionUnavailable => "sessionUnavailable",
            Self::OutputTargetUnavailable => "outputTargetUnavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionRegistrationFailure {
    ParserStatePoisoned,
    SessionAlreadyRegisteredOrClosing,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FanoutTraceEvent {
    TouchSilence,
    PrintableActivity,
    ResponseMarkers,
    OutputSender,
    ParserProcessed(u64),
    Websocket,
    UiEmit,
}

#[cfg(test)]
type FanoutTraceRecorder = Arc<Mutex<Vec<FanoutTraceEvent>>>;

impl SessionIoFanout {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    ) -> Self {
        Self {
            output_senders,
            idle_detector,
            response_watchers: Arc::new(Mutex::new(HashMap::new())),
            ws_broadcaster,
            screen_parsers: Arc::new(Mutex::new(HashMap::new())),
            attachments: TerminalOutputAttachments::new(),
            output_timer_enabled: Arc::new(AtomicBool::new(!cfg!(test))),
            fanout_identity: Arc::new(()),
            #[cfg(test)]
            trace: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn register_session(
        &self,
        id: Uuid,
        idle_tuning: IdleTuning,
        rows: u16,
        cols: u16,
        target: PtyOutputTarget,
    ) -> Result<PtyOutputRegistrationToken, SessionRegistrationFailure> {
        let identity = Arc::new(RegistrationIdentity {
            session_id: id,
            fanout_identity: Arc::clone(&self.fanout_identity),
        });
        let registration = Arc::new(RegisteredPtyOutputTarget {
            session_id: id,
            fanout_identity: Arc::clone(&self.fanout_identity),
            identity: Arc::clone(&identity),
            target,
        });
        let replay = ScreenReplayState {
            parser: vt100::Parser::new(rows, cols, 0),
            output_sequence: 0,
            registration,
            reader_gate: ReaderOperationGate::new(),
            parser_availability: ParserAvailability::Available,
            // Reserved up front, outside the parser mutex, so the ring never reallocates and
            // its ceiling is a property of construction. `VecDeque` growth is amortized, so
            // reserving later cannot pin the ceiling: the doubling already overshot by then.
            history: std::collections::VecDeque::with_capacity(UI_HISTORY_LIMIT_BYTES),
            history_aligned: true,
            conpty_size: (rows, cols),
        };
        let mut parsers = self
            .screen_parsers
            .lock()
            .map_err(|_| SessionRegistrationFailure::ParserStatePoisoned)?;
        if parsers.contains_key(&id) {
            return Err(SessionRegistrationFailure::SessionAlreadyRegisteredOrClosing);
        }
        parsers.insert(id, replay);
        drop(parsers);
        self.idle_detector.register_session(id, idle_tuning);
        Ok(PtyOutputRegistrationToken { identity })
    }

    #[cfg(test)]
    pub(crate) fn register_session_for_test(
        &self,
        id: Uuid,
        idle_tuning: IdleTuning,
        rows: u16,
        cols: u16,
    ) -> Result<PtyOutputRegistrationToken, SessionRegistrationFailure> {
        self.register_session(id, idle_tuning, rows, cols, PtyOutputTarget::noop())
    }

    fn acquire_reader_lease(
        &self,
        token: &PtyOutputRegistrationToken,
    ) -> Option<ReaderOperationLease> {
        if !Arc::ptr_eq(&token.identity.fanout_identity, &self.fanout_identity) {
            return None;
        }
        let parsers = self.screen_parsers.lock().ok()?;
        let state = parsers.get(&token.identity.session_id)?;
        if !state
            .registration
            .matches_token(token, &self.fanout_identity)
        {
            return None;
        }
        state.reader_gate.acquire()
    }

    #[cfg(any(windows, test))]
    pub(crate) fn observe_raw_read_for_reader(
        &self,
        token: &PtyOutputRegistrationToken,
        byte_count: usize,
        at: Instant,
    ) -> bool {
        if byte_count == 0 {
            return false;
        }
        let Some(lease) = self.acquire_reader_lease(token) else {
            return false;
        };
        self.idle_detector
            .notify_raw_output_at(token.identity.session_id, at);
        lease.complete();
        true
    }

    /// #3029: the reader's own parser is the only cursor authority. No locks escape.
    #[cfg(any(windows, test))]
    pub(crate) fn cursor_position_for_reader(
        &self,
        token: &PtyOutputRegistrationToken,
    ) -> Option<(u16, u16)> {
        let parsers = self.screen_parsers.lock().ok()?;
        let state = parsers.get(&token.identity.session_id)?;
        if !state
            .registration
            .matches_token(token, &self.fanout_identity)
            || !state.reader_gate.is_open()
            || state.parser_availability != ParserAvailability::Available
        {
            return None;
        }
        let (row, col) = state.parser.screen().cursor_position();
        Some((row + 1, col + 1))
    }

    pub(crate) fn registration_token_for_session(
        &self,
        id: Uuid,
    ) -> Option<PtyOutputRegistrationToken> {
        let parsers = self.screen_parsers.lock().ok()?;
        parsers.get(&id).map(|state| PtyOutputRegistrationToken {
            identity: Arc::clone(&state.registration.identity),
        })
    }

    #[cfg(test)]
    pub(crate) fn registration_token_for_test(&self, id: Uuid) -> PtyOutputRegistrationToken {
        self.registration_token_for_session(id)
            .expect("registered session")
    }

    #[cfg(test)]
    pub(crate) fn take_trace_for_test(&self) -> Vec<FanoutTraceEvent> {
        std::mem::take(&mut *self.trace.lock().expect("trace recorder"))
    }

    #[cfg(test)]
    fn trace(&self, event: FanoutTraceEvent) {
        self.trace.lock().expect("trace recorder").push(event);
    }
}

#[cfg(test)]
mod tests;
