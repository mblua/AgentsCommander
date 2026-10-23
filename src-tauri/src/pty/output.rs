use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
#[cfg(any(windows, test))]
use std::time::Instant;

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Emitter};
use terminal_snapshot_renderer::{
    canonical_timestamp, TerminalActiveBuffer, TerminalBackendKind, TerminalCell,
    TerminalCellStyle, TerminalCellWidth, TerminalColor, TerminalCursor, TerminalDimensions,
    TerminalLine, TerminalScreen, TerminalScreenModel, TerminalSnapshotFidelity,
    TerminalSnapshotSession, MAX_CELLS, MAX_COLUMNS, MAX_ROWS,
};
use uuid::Uuid;

use crate::pty::backend::SessionBackendKind;
use crate::pty::idle_detector::IdleDetector;
use crate::session::profile::IdleTuning;
use crate::telegram::manager::OutputSenderMap;

/// Tracks active response marker watchers per session.
/// Key: (session_id, request_id) -> accumulated output buffer.
/// The read loop scans for %%AC_RESPONSE::<rid>::START/END%% markers.
pub type ResponseWatcherMap = Arc<Mutex<HashMap<(Uuid, String), ResponseWatcher>>>;

pub struct ResponseWatcher {
    pub response_dir: std::path::PathBuf,
    pub buffer: Option<String>,
    pub capturing: bool,
}

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

struct ScreenReplayState {
    parser: vt100::Parser,
    output_sequence: u64,
    registration: Arc<RegisteredPtyOutputTarget>,
    reader_gate: Arc<ReaderOperationGate>,
    parser_availability: ParserAvailability,
    /// Bounded suffix of the raw output stream. Must be a `VecDeque`: trimming a `Vec` from
    /// the front would memmove the whole ring on every chunk of the hot path.
    history: std::collections::VecDeque<u8>,
    /// Whether the ring's front byte is known to sit at the start of a line, that is, at a
    /// point a parser in ground state can start reading from. Starts true, which is correct
    /// for a ring that is still growing from the first byte the session emitted, and is
    /// corrected by `append_history` both when a trim moves the front and when an oversized
    /// chunk installs a truncated one. Conservative in one direction only: `false` never
    /// means the front is definitely unsafe, it means nothing proved it safe (#1458).
    history_aligned: bool,
    /// The last grid the ConPTY actually took (rows, cols): recorded by every
    /// follow call BEFORE the skippable steps, so a skipped or failed
    /// `set_size` leaves a visible divergence for the attach reconcile (#1439).
    /// On transport backends (container) the follow runs after merely queuing
    /// the resize frame, so there this records the size last REQUESTED of the
    /// remote, not necessarily taken; the local backend's `if sent` gate is
    /// what keeps the record honest where #1439 lives.
    conpty_size: (u16, u16),
}

enum CaptureFailure {
    TooLarge,
    Unavailable,
}

pub(crate) struct CapturedVtScreen {
    rows: u16,
    columns: u16,
    output_sequence: u64,
    captured_at_millis: i64,
    active_buffer: TerminalActiveBuffer,
    cursor_row: u16,
    cursor_column: u16,
    cursor_visible: bool,
    parser_errors: u64,
    wraps: Vec<bool>,
    cells: Vec<vt100::Cell>,
}

impl std::fmt::Debug for CapturedVtScreen {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapturedVtScreen")
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .field("output_sequence", &self.output_sequence)
            .field("active_buffer", &self.active_buffer)
            .field("cursor_row", &self.cursor_row)
            .field("cursor_column", &self.cursor_column)
            .field("cursor_visible", &self.cursor_visible)
            .field("parser_errors", &self.parser_errors)
            .field("wraps", &self.wraps.len())
            .field("cells", &self.cells.len())
            .finish_non_exhaustive()
    }
}

impl CapturedVtScreen {
    pub(crate) fn into_model(
        self,
        session_id: Uuid,
        backend_kind: SessionBackendKind,
    ) -> Result<Arc<TerminalScreenModel>, ()> {
        let captured_at = DateTime::<Utc>::from_timestamp_millis(self.captured_at_millis)
            .map(canonical_timestamp)
            .ok_or(())?;
        if self.wraps.len() != usize::from(self.rows)
            || self.cells.len()
                != usize::from(self.rows)
                    .checked_mul(usize::from(self.columns))
                    .ok_or(())?
        {
            return Err(());
        }
        let mut lines = Vec::new();
        lines
            .try_reserve_exact(usize::from(self.rows))
            .map_err(|_| ())?;
        let mut cells = self.cells.into_iter();
        for row in 0..self.rows {
            let mut line_cells = Vec::new();
            line_cells
                .try_reserve_exact(usize::from(self.columns))
                .map_err(|_| ())?;
            for _column in 0..self.columns {
                let cell = cells.next().ok_or(())?;
                let is_wide = cell.is_wide();
                let is_continuation = cell.is_wide_continuation();
                if is_wide && is_continuation {
                    return Err(());
                }
                let width = if is_wide {
                    TerminalCellWidth::WideLead
                } else if is_continuation {
                    TerminalCellWidth::WideContinuation
                } else {
                    TerminalCellWidth::Narrow
                };
                let text = cell.contents();
                line_cells.push(TerminalCell {
                    text,
                    width,
                    foreground: convert_color(cell.fgcolor()),
                    background: convert_color(cell.bgcolor()),
                    style: TerminalCellStyle {
                        bold: cell.bold(),
                        italic: cell.italic(),
                        underline: cell.underline(),
                        inverse: cell.inverse(),
                    },
                });
            }
            lines.push(TerminalLine {
                wrapped: *self.wraps.get(usize::from(row)).ok_or(())?,
                cells: line_cells,
            });
        }
        if cells.next().is_some() {
            return Err(());
        }
        let backend = match backend_kind {
            SessionBackendKind::LocalProcess => TerminalBackendKind::LocalProcess,
            SessionBackendKind::ContainerTransport => TerminalBackendKind::ContainerTransport,
        };
        let dimensions = TerminalDimensions {
            rows: self.rows,
            columns: self.columns,
        };
        let cursor = TerminalCursor {
            row: self.cursor_row,
            column: self.cursor_column,
            visible: self.cursor_visible,
            in_bounds: self.cursor_row < self.rows && self.cursor_column < self.columns,
        };
        let model = TerminalScreenModel {
            captured_at,
            session: TerminalSnapshotSession {
                id: session_id.to_string(),
                backend,
            },
            screen: TerminalScreen {
                dimensions,
                sequence: self.output_sequence,
                active_buffer: self.active_buffer,
                cursor,
                parser_errors: self.parser_errors,
                lines,
            },
            fidelity: TerminalSnapshotFidelity::version_one(self.parser_errors != 0),
        };
        model.validate().map_err(|_| ())?;
        Ok(Arc::new(model))
    }
}

fn convert_color(color: vt100::Color) -> TerminalColor {
    match color {
        vt100::Color::Default => TerminalColor::Default,
        vt100::Color::Idx(index) => TerminalColor::Indexed { index },
        vt100::Color::Rgb(red, green, blue) => TerminalColor::Rgb { red, green, blue },
    }
}

/// The UI coalescing window. Its only consumer is the one-shot timer armed per session, which
/// test builds hold so no assertion depends on a wall clock; the flush itself is driven
/// synchronously there, by every test but the timer's own.
const UI_BATCH_INTERVAL_MS: u64 = 16;

/// Ingest-thread flush threshold for the per-session coalescing accumulator. Reaching it
/// inside one 16 ms window flushes on the reader thread, and that is the only backpressure
/// left on this path: a slow emit there blocks that session's reader, never the parser mutex
/// the whole backend shares.
const UI_BATCH_LIMIT_BYTES: usize = 65_536;
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

/// The private result surface of the Tauri output effect. It deliberately carries no
/// underlying Tauri error because output bytes and event errors are both sensitive at this
/// boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PtyOutputEmitError {
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
/// live with no reconcile: live PTY bytes are never gated (PR #961). Emitting nothing for a
/// faulted parser is what made the terminal permanently black in #955.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyOutputPayload {
    session_id: String,
    data: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sequence: Option<u64>,
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

#[derive(Clone)]
pub(crate) struct PtyOutputRegistrationToken {
    identity: Arc<RegistrationIdentity>,
}

struct RegistrationIdentity {
    session_id: Uuid,
    fanout_identity: Arc<()>,
}

struct RegisteredPtyOutputTarget {
    session_id: Uuid,
    fanout_identity: Arc<()>,
    identity: Arc<RegistrationIdentity>,
    target: PtyOutputTarget,
}

impl RegisteredPtyOutputTarget {
    fn matches_token(&self, token: &PtyOutputRegistrationToken, fanout_identity: &Arc<()>) -> bool {
        Arc::ptr_eq(&self.identity, &token.identity)
            && Arc::ptr_eq(&self.fanout_identity, fanout_identity)
    }
}

/// One emit per attached window label, fallible as a whole.
type PtyOutputEmitFn =
    dyn Fn(&[WindowLabel], PtyOutputPayload) -> Result<(), PtyOutputEmitError> + Send + Sync;

#[derive(Clone)]
pub(crate) struct PtyOutputTarget {
    emit_pty_output: Arc<PtyOutputEmitFn>,
}

#[cfg(test)]
pub(crate) type PtyOutputTestEvent = (String, Vec<u8>, Option<u64>);

#[cfg(test)]
pub(crate) type PtyOutputTestSink = Arc<Mutex<Vec<PtyOutputTestEvent>>>;

impl PtyOutputTarget {
    /// Emits once per attached window label.
    ///
    /// `emit` would deliver to EVERY open webview, listener or not: the sidebar and every
    /// detached terminal window each pay a receive-side deserialization before their event
    /// router discards the payload. With `emit_to` the bridge multiplier is the number of
    /// (session, attached window) pairs instead, which is what bounds the webview axis at all.
    /// `emit_to` returns `Ok` for a label with no listener and for a label whose window is
    /// already gone, so a window that died before the destroy reap ran is benign.
    pub(crate) fn from_app_handle<R: tauri::Runtime>(app_handle: AppHandle<R>) -> Self {
        Self {
            emit_pty_output: Arc::new(move |labels, payload| {
                let mut result = Ok(());
                for label in labels {
                    if app_handle
                        .emit_to(label.as_str(), "pty_output", payload.clone())
                        .is_err()
                    {
                        result = Err(PtyOutputEmitError::Emit);
                    }
                }
                result
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn noop() -> Self {
        Self {
            emit_pty_output: Arc::new(|_, _| Ok(())),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_sink(sink: PtyOutputTestSink) -> Self {
        Self {
            emit_pty_output: Arc::new(move |_labels, payload| {
                sink.lock()
                    .unwrap()
                    .push((payload.session_id, payload.data, payload.sequence));
                Ok(())
            }),
        }
    }

    #[cfg(test)]
    fn failing_test_sink(sink: PtyOutputTestSink) -> Self {
        let target = Self::from_test_sink(sink);
        Self {
            emit_pty_output: Arc::new(move |labels, payload| {
                let _ = target.emit_pty_output(labels, payload);
                Err(PtyOutputEmitError::Emit)
            }),
        }
    }

    fn emit_pty_output(
        &self,
        labels: &[WindowLabel],
        payload: PtyOutputPayload,
    ) -> Result<(), PtyOutputEmitError> {
        (self.emit_pty_output)(labels, payload)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParserAvailability {
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionRegistrationFailure {
    ParserStatePoisoned,
    SessionAlreadyRegisteredOrClosing,
}

#[derive(Default)]
struct ReaderOperationGateState {
    lifecycle_closing: bool,
    admitted_count: usize,
}

struct ReaderOperationGate {
    state: Mutex<ReaderOperationGateState>,
    drained: Condvar,
}

impl ReaderOperationGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ReaderOperationGateState::default()),
            drained: Condvar::new(),
        })
    }

    fn acquire(self: &Arc<Self>) -> Option<ReaderOperationLease> {
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

    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.lifecycle_closing = true;
        if state.admitted_count == 0 {
            self.drained.notify_all();
        }
    }

    fn is_open(&self) -> bool {
        self.state
            .lock()
            .map(|state| !state.lifecycle_closing)
            .unwrap_or(false)
    }

    fn wait_for_drain(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while state.admitted_count != 0 {
            state = self
                .drained
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    fn complete(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
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

struct ReaderOperationLease {
    gate: Arc<ReaderOperationGate>,
    completed: bool,
}

impl ReaderOperationLease {
    fn complete(mut self) {
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

/// A window label, exactly as Tauri reports it for the calling webview. It is never a value
/// JavaScript chooses, so a frontend cannot forge one or attach on another window's behalf.
type WindowLabel = String;

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
    state: Mutex<AttachmentState>,
}

#[derive(Default)]
struct AttachmentState {
    attached: HashMap<Uuid, HashSet<WindowLabel>>,
    /// Per-session coalescing buffers, present only while a session has an attachment. The
    /// outer map is locked for lookup only: holding it across an emit would let one flooding
    /// session throttle every other session's ingest.
    accumulators: HashMap<Uuid, Arc<Mutex<SessionAccumulator>>>,
}

/// One session's pending batch.
///
/// Two emitters drain it, the 64 KiB threshold flush on the ingest thread and the 16 ms timer
/// flush on a Tokio task, and both do the drain AND the emit while holding this mutex. A drain
/// that released the mutex before emitting would let the two interleave a session's bytes,
/// which is a silently corrupted terminal.
struct SessionAccumulator {
    registration: Arc<RegisteredPtyOutputTarget>,
    data: Vec<u8>,
    /// The batch's LAST sequence, or `None` once the parser is unavailable. A batch never
    /// mixes the two: the accumulator is flushed at the `Available -> Unavailable` transition,
    /// so one scalar always describes the whole batch. Labelled with the last real sequence, a
    /// mixed batch would make the client watermark-drop bytes that were never seeded; labelled
    /// `None`, its sequenced prefix would escape reconciliation and duplicate against the seed.
    sequence: Option<u64>,
    /// A one-shot timer task is pending for this session. Only the timer path clears it, which
    /// is what keeps exactly one task in flight: a threshold flush leaves the flag set, so the
    /// chunk that follows it cannot arm a second timer behind the one still sleeping. Clearing
    /// it on every flush let a flooding session emit its ceiling flushes AND a timer flush
    /// behind each of them, up to twice the event rate criterion E' bounds.
    timer_armed: bool,
    /// Whether the last emit failed. A webview torn down mid-flood makes every flush fail, and
    /// logging one line per flush would be ~62 lines/s per session, which would itself read as
    /// the bug. Only the transition into the failing state and the recovery are logged.
    emit_failing: bool,
}

impl SessionAccumulator {
    fn new(registration: Arc<RegisteredPtyOutputTarget>) -> Self {
        Self {
            registration,
            data: Vec::new(),
            sequence: None,
            timer_armed: false,
            emit_failing: false,
        }
    }

    fn discard(&mut self) {
        self.data.clear();
        self.sequence = None;
    }

    /// Drains the batch and emits it to `labels`, under the caller's lock on this accumulator.
    /// With no label left the bytes are dropped rather than deferred: they must not surface on
    /// a later re-attach, out of order and after that attach's reset.
    fn flush(&mut self, labels: &[WindowLabel]) {
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
enum Accumulated {
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
    fn lock_state(&self) -> std::sync::MutexGuard<'_, AttachmentState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn lock_accumulator(
        accumulator: &Arc<Mutex<SessionAccumulator>>,
    ) -> std::sync::MutexGuard<'_, SessionAccumulator> {
        accumulator
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn labels_of(state: &AttachmentState, session_id: Uuid) -> Vec<WindowLabel> {
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
    fn attach(&self, session_id: Uuid, label: &str) {
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
    fn detach(&self, session_id: Uuid, label: &str) {
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
    fn release_window(&self, label: &str) {
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
    fn remove_session(&self, session_id: Uuid) {
        let orphaned = {
            let mut state = self.lock_state();
            state.attached.remove(&session_id);
            state.accumulators.remove(&session_id)
        };
        Self::discard_orphaned(orphaned);
    }

    /// Always outside the outer lock: an in-flight emit holds the accumulator mutex, and
    /// waiting for it with the map locked would stall every other session's ingest.
    fn discard_orphaned(orphaned: Option<Arc<Mutex<SessionAccumulator>>>) {
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
    fn accumulate(
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
    fn flush(&self, session_id: Uuid, from_timer: bool) {
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
    fn labels_for_test(&self, session_id: Uuid) -> Vec<WindowLabel> {
        let mut labels = Self::labels_of(&self.lock_state(), session_id);
        labels.sort();
        labels
    }

    #[cfg(test)]
    fn pending_bytes_for_test(&self, session_id: Uuid) -> Option<usize> {
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

    pub(crate) fn handle_output(
        &self,
        token: &PtyOutputRegistrationToken,
        session_id_str: &str,
        data: Vec<u8>,
    ) {
        let Some(lease) = self.acquire_reader_lease(token) else {
            return;
        };
        let id = token.identity.session_id;
        let n = data.len();

        let text = String::from_utf8_lossy(&data);
        if text.contains('\u{FFFD}') {
            log::debug!(
                "[PTY] session {} chunk had invalid UTF-8 at buffer boundary ({} bytes, {} replacement chars)",
                id,
                n,
                text.matches('\u{FFFD}').count()
            );
        }

        // #2124 - the detector decides whether this chunk counts, resets the
        // silence clock or is absorbed into a pending short burst. The trace
        // order stays TouchSilence then PrintableActivity for the burst-off
        // sessions the trace tests use.
        let printable = output_has_printable_activity(&text);
        #[cfg(test)]
        self.trace(FanoutTraceEvent::TouchSilence);
        #[cfg(test)]
        if printable {
            self.trace(FanoutTraceEvent::PrintableActivity);
        }
        self.idle_detector.record_output(id, n, printable);
        if !printable {
            log::trace!(
                "[idle] SKIPPED activity for {} ({} bytes, escape-only output)",
                &id.to_string()[..8],
                n
            );
        }

        #[cfg(test)]
        self.trace(FanoutTraceEvent::ResponseMarkers);
        scan_response_markers(id, &text, &self.response_watchers);

        let output_sender = self
            .output_senders
            .lock()
            .ok()
            .and_then(|senders| senders.get(&id).cloned());
        if let Some(sender) = output_sender {
            #[cfg(test)]
            self.trace(FanoutTraceEvent::OutputSender);
            let _ = sender.try_send(data.clone());
        }

        let accumulated = {
            let Ok(mut parsers) = self.screen_parsers.lock() else {
                return;
            };
            let Some(state) = parsers.get_mut(&id) else {
                return;
            };
            if !state
                .registration
                .matches_token(token, &self.fanout_identity)
            {
                return;
            }
            let registration = Arc::clone(&state.registration);
            // The emission predicate is `reader_gate.is_open() && attached`, and it is an AND:
            // the gate is the teardown drain gate, the attached set is delivery, and folding
            // one into the other would let output emit into a session being torn down.
            //
            // It is evaluated independently of `parser_availability` on purpose. A faulted
            // parser must keep emitting, unsequenced, or the terminal goes black for the life
            // of the process, so the `Unavailable` arm below carries the real `ui_open` rather
            // than the `false` the delivery contract used to hardcode there.
            let ui_open = state.reader_gate.is_open();
            let (sequence, parser_fault) = match state.parser_availability {
                ParserAvailability::Unavailable => (None, false),
                ParserAvailability::Available => {
                    let processed = crate::logging::catch_payload_unwind(|| {
                        state.parser.process(&data);
                        let sequence = state.output_sequence.checked_add(1).ok_or(())?;
                        state.output_sequence = sequence;
                        // Order matters, and these two lines must stay contiguous. The ring
                        // may only grow once `output_sequence` has advanced: on overflow the
                        // line above returns `Err` and the parser goes `Unavailable`, and a
                        // ring that grew anyway would make the attach snapshot carry bytes
                        // that `sequence` does not represent. The frontend reconciles by
                        // watermark and only skips what is at or below the snapshot sequence,
                        // so those bytes would be seeded and then written again when they
                        // arrive live: a duplicated block of history.
                        append_history(&mut state.history, &mut state.history_aligned, &data);
                        Ok::<u64, ()>(sequence)
                    });
                    match processed {
                        Ok(Ok(sequence)) => {
                            #[cfg(test)]
                            self.trace(FanoutTraceEvent::ParserProcessed(sequence));
                            (Some(sequence), false)
                        }
                        Ok(Err(())) | Err(_) => {
                            state.parser_availability = ParserAvailability::Unavailable;
                            (None, true)
                        }
                    }
                }
            };
            if parser_fault {
                log::error!("[terminal-snapshot] stage=parser_fault session={id}");
            }
            // Appending under the parser lock is what keeps the batch boundary atomic with the
            // sequence assignment, so no coalesced batch can straddle an attach's snapshot.
            // The emit it may ask for is run below, after the lock is dropped.
            if ui_open {
                self.attachments
                    .accumulate(&registration, sequence, &data, parser_fault)
            } else {
                Accumulated::Unattached
            }
        };

        if let Some(ref broadcaster) = self.ws_broadcaster {
            #[cfg(test)]
            self.trace(FanoutTraceEvent::Websocket);
            broadcaster.broadcast_pty_output(session_id_str, &data);
        }

        // The UI EMIT is still the last thing this function does, which is what keeps a slow
        // one from delaying the idle detector, the response-marker scan, the raw sender or the
        // websocket broadcaster. Only the append above runs earlier, and it copies bytes.
        #[cfg(test)]
        if accumulated != Accumulated::Unattached {
            self.trace(FanoutTraceEvent::UiEmit);
        }
        match accumulated {
            Accumulated::Unattached | Accumulated::Pending => {}
            Accumulated::ArmTimer => self.arm_output_flush(id),
            Accumulated::FlushNow => self.attachments.flush(id, false),
        }
        lease.complete();
    }

    /// Attaches `label`'s window to this session's output and returns the seed for it.
    ///
    /// The whole body runs inside one `screen_parsers` hold and does three things, in this
    /// order: drain the session's pending batch, read the snapshot at `state.output_sequence`,
    /// insert the label. Because no chunk can be processed while that lock is held, the three
    /// are atomic with respect to the ingest, and that removes three hazards at once. No
    /// coalesced batch can straddle the snapshot, so a scalar `sequence` carrying the batch's
    /// last sequence is sufficient and no first-sequence field is needed. There is no window
    /// between reading the snapshot and becoming attached in which chunks are neither seeded
    /// nor emitted. And an attach racing `remove_session` cannot leave a label behind for a
    /// session that no longer exists, because the insert only happens with the session present.
    ///
    /// The insert is the last step, on a path that has already decided to succeed, so the
    /// attach is transactional by construction: no guard, no compensating release.
    ///
    /// Only two conditions fail, and both mean there is nothing to attach to: the session is
    /// absent, or the registration identity does not match. Every other condition attaches
    /// without a snapshot and lets the client write live (see `TerminalOutputAttachError`).
    ///
    /// `include_history` asks for the retained ring instead of the mirrored viewport. It is
    /// safe on every attach because the client applies the snapshot through a reset first:
    /// replaying the ring over a terminal that still holds scrollback would append a duplicate
    /// block, and the reset is what makes that impossible. The ring also keeps filling while a
    /// session is detached, which is what makes a re-attach gap free, so the frontend always
    /// asks for it; the parameter stays so the mirrored viewport remains addressable.
    pub(crate) fn activate_terminal_output(
        &self,
        id: Uuid,
        label: &str,
        include_history: bool,
    ) -> Result<Option<PtyScreenSnapshot>, TerminalOutputAttachError> {
        let Ok(mut parsers) = self.screen_parsers.lock() else {
            // Nothing can be read through a poisoned parser lock, but that is not "nothing to
            // attach to": the window attaches and writes live, exactly as it would for a
            // faulted parser.
            self.attachments.attach(id, label);
            return Ok(None);
        };
        let Some(state) = parsers.get_mut(&id) else {
            return Err(TerminalOutputAttachError::SessionUnavailable);
        };
        if state.registration.session_id != id
            || !Arc::ptr_eq(&state.registration.fanout_identity, &self.fanout_identity)
        {
            return Err(TerminalOutputAttachError::OutputTargetUnavailable);
        }
        let mut reconcile_fault = false;
        // #1458: carried out of the parser lock exactly like `reconcile_fault`. `Some` only
        // when the ring was flagged unaligned; the pair is (ring length, bytes kept).
        let mut history_unaligned: Option<(usize, usize)> = None;
        let snapshot = if state.parser_availability == ParserAvailability::Available {
            let (parser_rows, parser_cols) = state.parser.screen().size();
            if (parser_rows, parser_cols) != state.conpty_size {
                // #1439: the parser grid diverged from the grid the ConPTY took
                // (a skipped follow, or any path that resized one without the
                // other). A seed rendered off this parser adopts the stale grid
                // in the attaching window and replays other-grid bytes into it:
                // the garbled re-attach. Converge the grid now, seed nothing;
                // the frontend attaches live (the no-snapshot path), and the
                // next attach after the child's next repaint seeds cleanly.
                let (conpty_rows, conpty_cols) = state.conpty_size;
                log::warn!(
                    "[terminal-snapshot] stage=attach_grid_mismatch session={id} parser={parser_cols}x{parser_rows} conpty={conpty_cols}x{conpty_rows} (#1439)"
                );
                let resized = crate::logging::catch_payload_unwind(|| {
                    state.parser.set_size(conpty_rows, conpty_cols)
                });
                if resized.is_err() {
                    state.parser_availability = ParserAvailability::Unavailable;
                    reconcile_fault = true;
                }
                None
            } else {
                let copied = {
                    let uses_history = include_history && !state.history.is_empty();
                    // `as_slices` keeps this a read: `make_contiguous` needs `&mut` and
                    // rotates the buffer during what is otherwise a copy out.
                    let replay = if uses_history {
                        let (front, back) = state.history.as_slices();
                        // #1458: the hot-path realignment is capped at 4 KiB and stays failed
                        // for as long as the front sits in a newline-free region, so it can
                        // leave the front inside an escape sequence. Attach is cold: pay the
                        // full scan here rather than seed a literal sequence tail.
                        if state.history_aligned {
                            Some((front, back))
                        } else {
                            history_from_first_line(front, back)
                        }
                    } else {
                        None
                    };
                    if uses_history && !state.history_aligned {
                        history_unaligned = Some((
                            state.history.len(),
                            replay.map_or(0, |(front, back)| front.len() + back.len()),
                        ));
                    }
                    crate::logging::catch_payload_unwind(|| {
                        let screen = state.parser.screen();
                        let (rows, cols) = screen.size();
                        let cells = usize::from(rows).checked_mul(usize::from(cols)).ok_or(())?;
                        if rows > MAX_ROWS || cols > MAX_COLUMNS || cells > MAX_CELLS {
                            return Err(());
                        }
                        let data = match replay {
                            Some((front, back)) => {
                                let mut bytes = Vec::with_capacity(
                                    UI_HISTORY_REPLAY_PROLOGUE.len() + front.len() + back.len(),
                                );
                                bytes.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
                                bytes.extend_from_slice(front);
                                bytes.extend_from_slice(back);
                                bytes
                            }
                            // No line start with content behind it. The ring cannot be
                            // replayed from any offset, and in the observed incident it holds
                            // nothing but spinner frames anyway. The mirror is a consistent
                            // full repaint on a grid the #1439 branch above already validated.
                            None => screen.contents_formatted(),
                        };
                        Ok::<PtyScreenSnapshot, ()>(PtyScreenSnapshot {
                            data,
                            rows,
                            cols,
                            sequence: state.output_sequence,
                        })
                    })
                };
                match copied {
                    Ok(Ok(snapshot)) => Some(snapshot),
                    Ok(Err(())) => None,
                    Err(_) => {
                        log::warn!("[terminal-snapshot] stage=render_panic reason=payload_panic session={id} (#1452)");
                        state.parser_availability = ParserAvailability::Unavailable;
                        None
                    }
                }
            }
        } else {
            None
        };
        self.attachments.attach(id, label);
        drop(parsers);
        if let Some((ring, kept)) = history_unaligned {
            log::warn!(
                "[terminal-snapshot] stage=attach_history_unaligned session={id} ring={ring} kept={kept} (#1458)"
            );
        }
        if reconcile_fault {
            log::error!("[terminal-snapshot] stage=parser_fault session={id}");
            // #1439 R2: flush at the transition, OUTSIDE the parser lock. An
            // emit under that lock stalls the PTY reader on its next chunk;
            // both existing fault sites flush only after releasing the lock.
            self.attachments.flush(id, false);
        }
        Ok(snapshot)
    }

    /// Releases `label`'s attachment. Detaching a session this window never attached, and
    /// detaching twice, are window-local no-ops that cannot disturb another window.
    pub(crate) fn detach_terminal_output(&self, id: Uuid, label: &str) {
        self.attachments.detach(id, label);
    }

    /// A window was destroyed: release every attachment it held, with no frontend cooperation.
    /// This is what makes the frontend detach a bandwidth optimization rather than a
    /// correctness dependency, and what lets the close hook avoid blocking the close.
    pub(crate) fn release_window_attachments(&self, label: &str) {
        self.attachments.release_window(label);
    }

    /// One one-shot task per session, armed when that session's accumulator goes from empty to
    /// non-empty.
    ///
    /// Deliberately not a global 16 ms ticker. A ticker runs forever, including with zero
    /// attached sessions, which defeats timer coalescing and costs idle power on a
    /// long-running tray app, and it serializes every attached session's emit onto one task,
    /// so a flooding session's 64 KiB delays the keystroke echo flush of the interactive
    /// session queued behind it. Arming on demand preserves today's idle cost, which is zero,
    /// and today's latency shape, where the first byte after idle is emitted 16 ms later.
    /// Arming is switched at runtime rather than by `cfg(test)`: test builds start it off so
    /// every emit a test observes comes from an explicit `flush_terminal_output_for_test` and
    /// no assertion races a 16 ms task, and the one test that covers the timer turns it on.
    fn arm_output_flush(&self, id: Uuid) {
        if !self.output_timer_enabled.load(Ordering::Relaxed) {
            return;
        }
        let attachments = Arc::clone(&self.attachments);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(UI_BATCH_INTERVAL_MS)).await;
            attachments.flush(id, true);
        });
    }

    /// The synchronous flush seam. Every backend test drives it instead of sleeping, so the
    /// timer is the only spawned thing on this path and no assertion depends on a wall clock.
    /// It stands in for the timer, so it clears `timer_armed` exactly as the timer does.
    #[cfg(test)]
    pub(crate) fn flush_terminal_output_for_test(&self, id: Uuid) {
        self.attachments.flush(id, true);
    }

    /// Lets the timer's own test run the real arming path. Nothing else switches this on.
    #[cfg(test)]
    pub(crate) fn enable_output_timer_for_test(&self) {
        self.output_timer_enabled.store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn attached_labels_for_test(&self, id: Uuid) -> Vec<String> {
        self.attachments.labels_for_test(id)
    }

    #[cfg(test)]
    pub(crate) fn pending_output_bytes_for_test(&self, id: Uuid) -> Option<usize> {
        self.attachments.pending_bytes_for_test(id)
    }

    pub fn record_resize(&self, id: Uuid) {
        self.idle_detector.record_resize(id);
    }

    /// #973 - a degenerate size must never reach the vt100 parser, and this is the boundary
    /// where that invariant actually breaks, so this is where it is enforced.
    ///
    /// `vt100::grid::set_size` computes `size.rows - 1` on a `u16` (grid.rs:73). With
    /// `rows == 0` that UNDERFLOWS: it panics in a debug build and wraps to 65535 in a release
    /// one, leaving a zero-row grid with a scroll region of 65535. And the panic fires while
    /// this function is HOLDING `screen_parsers`, which poisons the mutex - after which
    /// `handle_output`, `get_screen_snapshot` and `get_pty_size` all take the `if let Ok` /
    /// `.ok()?` branch and silently do nothing, for every session, for the life of the process.
    /// One `cols: 0` would take #955's screen snapshot down app-wide, without a single log line.
    ///
    /// The callers guard too (`local_backend` only moves the screen for a size the ConPTY
    /// actually took), but they cannot be the only guard: `container_backend:1099` calls this
    /// with whatever `pty_resize` was given, and there is no gate on that path at all.
    ///
    /// A refused resize did not happen, so the broadcast is skipped with it: clients must not
    /// be told the terminal is 0 columns wide.
    pub fn resize_screen_and_broadcast(&self, id: Uuid, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            log::warn!("[pty] refusing to resize the screen of {id} to {cols}x{rows} (#973)");
            return;
        }

        let parser_fault = {
            let Ok(mut parsers) = self.screen_parsers.lock() else {
                log::warn!("[terminal-snapshot] stage=resize_skipped reason=parsers_lock_poisoned session={id} cols={cols} rows={rows} (#1439)");
                return;
            };
            let Some(state) = parsers.get_mut(&id) else {
                log::warn!("[terminal-snapshot] stage=resize_skipped reason=no_parser_entry session={id} cols={cols} rows={rows} (#1439)");
                return;
            };
            // #1439 record-first: the ConPTY took this size whether or not the
            // parser can follow it; the attach reconcile compares against this.
            state.conpty_size = (rows, cols);
            if state.parser_availability != ParserAvailability::Available {
                log::warn!("[terminal-snapshot] stage=resize_skipped reason=parser_unavailable session={id} cols={cols} rows={rows} (#1439)");
                return;
            }
            let resized =
                crate::logging::catch_payload_unwind(|| state.parser.set_size(rows, cols));
            if resized.is_err() {
                state.parser_availability = ParserAvailability::Unavailable;
                true
            } else {
                false
            }
        };
        if parser_fault {
            log::error!("[terminal-snapshot] stage=parser_fault session={id}");
            // Flush at the transition: from here every batch carries no sequence, and one
            // batch cannot describe both with a single scalar.
            self.attachments.flush(id, false);
            return;
        }

        if let Some(ref bc) = self.ws_broadcaster {
            bc.broadcast_event(
                "pty_resized",
                &serde_json::json!({
                    "sessionId": id.to_string(),
                    "cols": cols,
                    "rows": rows,
                }),
            );
        }
    }

    pub fn remove_session(&self, id: Uuid) {
        let registration_and_gate = {
            let mut parsers = self
                .screen_parsers
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(state) = parsers.get_mut(&id) else {
                return;
            };
            state.reader_gate.close();
            (
                Arc::clone(&state.registration),
                Arc::clone(&state.reader_gate),
            )
        };

        registration_and_gate.1.wait_for_drain();

        let mut parsers = self
            .screen_parsers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if parsers.get(&id).is_some_and(|state| {
            Arc::ptr_eq(
                &state.registration.identity,
                &registration_and_gate.0.identity,
            )
        }) {
            parsers.remove(&id);
            // Unconditional in the backend, whatever the frontend did: the session is gone, so
            // its attachments and its pending bytes go with it. The release runs under the same
            // `screen_parsers` hold that removed the session, and that order is what makes the
            // no-zombie invariant true. An attach inserts its label while holding this map with
            // the session present, so with the entry already gone no insert can be in flight and
            // none can start. Releasing before the drain instead left the whole drain window
            // open for an attach to insert a label for a session about to disappear, and that
            // label then survived for the life of the process. It is skipped when the identity
            // does not match, because the entry then belongs to a same-uuid replacement.
            self.attachments.remove_session(id);
        }
        drop(parsers);

        self.idle_detector.remove_session(id);
        if let Ok(mut watchers) = self.response_watchers.lock() {
            watchers.retain(|(sid, _), _| *sid != id);
        }
    }

    pub(crate) fn shutdown_terminal_output(&self) {
        let ids = self
            .screen_parsers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for id in ids {
            self.remove_session(id);
        }
    }

    /// #973 (B) - has the child painted anything a human could see?
    ///
    /// This is the startup gate's trigger, and it asks the REAL, STATEFUL vt100 parser that
    /// `handle_output` has already fed every byte of every chunk. Call it after `handle_output`,
    /// so the chunk that just arrived is in the screen.
    ///
    /// It replaced a per-chunk text predicate (`output_has_printable_activity`, which the idle
    /// detector still uses), and it had to, because that predicate answers a different question
    /// and got this one wrong two ways:
    ///
    /// - **Chunk boundaries.** `strip_ansi_csi` is stateless and carries no residue between
    ///   calls, but the read loop hands it raw `read()` chunks. A chunk that ends mid-CSI or
    ///   mid-OSC delivers the tail in the next chunk with NO leading `ESC`, and `1049h`, `2J` and
    ///   `cmd.exe` are all printable. conhost really does split its output across writes.
    /// - **Three-byte escapes.** It consumed `ESC` plus exactly ONE char. A charset designator is
    ///   three (`ESC ( B`, which ncurses and half the TUI world emit on the way up, plus
    ///   `ESC ) 0` and `ESC % G`), so the third byte survived and read as printable.
    ///
    /// Either one opens the gate on a child that has painted nothing, which is the bug this gate
    /// exists to prevent. A real parser carries state across reads and knows what an escape is,
    /// so both are closed by construction rather than by another special case.
    ///
    /// Returns false when the parser cannot be reached: an unfed parser can never show content
    /// anyway (`handle_output` skips the same poisoned lock), so the gate simply stays shut,
    /// which is the safe direction. It is never resized, and a child showing nothing does not
    /// care what size it is.
    pub fn has_rendered_visible_content(&self, id: Uuid) -> bool {
        let Ok(parsers) = self.screen_parsers.lock() else {
            return false;
        };
        let Some(state) = parsers.get(&id) else {
            return false;
        };
        if state.parser_availability != ParserAvailability::Available {
            return false;
        }
        screen_shows_visible_content(state.parser.screen())
    }

    pub fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        let parsers = self.screen_parsers.lock().ok()?;
        let state = parsers.get(&id)?;
        if state.parser_availability != ParserAvailability::Available {
            return None;
        }
        let screen = state.parser.screen();
        let (rows, cols) = screen.size();
        Some(PtyScreenSnapshot {
            data: screen.contents_formatted(),
            rows,
            cols,
            sequence: state.output_sequence,
        })
    }

    pub(crate) fn copy_terminal_screen(
        &self,
        id: Uuid,
    ) -> crate::pty::backend::TerminalScreenCopyRead {
        use crate::pty::backend::TerminalScreenCopyRead;

        let Ok(mut parsers) = self.screen_parsers.lock() else {
            return TerminalScreenCopyRead::Unavailable;
        };
        let Some(state) = parsers.get(&id) else {
            return TerminalScreenCopyRead::Unavailable;
        };
        if state.parser_availability != ParserAvailability::Available {
            return TerminalScreenCopyRead::Unavailable;
        }
        let copied = crate::logging::catch_payload_unwind(|| {
            let screen = state.parser.screen();
            let (rows, columns) = screen.size();
            let cell_count = usize::from(rows)
                .checked_mul(usize::from(columns))
                .ok_or(CaptureFailure::TooLarge)?;
            if rows == 0
                || columns == 0
                || rows > MAX_ROWS
                || columns > MAX_COLUMNS
                || cell_count > MAX_CELLS
            {
                return Err(CaptureFailure::TooLarge);
            }
            let mut wraps = Vec::new();
            wraps
                .try_reserve_exact(usize::from(rows))
                .map_err(|_| CaptureFailure::Unavailable)?;
            let mut cells = Vec::new();
            cells
                .try_reserve_exact(cell_count)
                .map_err(|_| CaptureFailure::Unavailable)?;
            let captured_at_millis = Utc::now().timestamp_millis();
            let (cursor_row, cursor_column) = screen.cursor_position();
            let parser_errors =
                u64::try_from(screen.errors()).map_err(|_| CaptureFailure::Unavailable)?;
            for row in 0..rows {
                wraps.push(screen.row_wrapped(row));
                for column in 0..columns {
                    cells.push(
                        screen
                            .cell(row, column)
                            .ok_or(CaptureFailure::Unavailable)?
                            .clone(),
                    );
                }
            }
            Ok(CapturedVtScreen {
                rows,
                columns,
                output_sequence: state.output_sequence,
                captured_at_millis,
                active_buffer: if screen.alternate_screen() {
                    TerminalActiveBuffer::Alternate
                } else {
                    TerminalActiveBuffer::Normal
                },
                cursor_row,
                cursor_column,
                cursor_visible: !screen.hide_cursor(),
                parser_errors,
                wraps,
                cells,
            })
        });
        match copied {
            Ok(Ok(captured)) => TerminalScreenCopyRead::Copied(captured),
            Ok(Err(CaptureFailure::TooLarge)) => TerminalScreenCopyRead::TooLarge,
            Ok(Err(CaptureFailure::Unavailable)) => TerminalScreenCopyRead::Unavailable,
            Err(_) => {
                if let Some(state) = parsers.get_mut(&id) {
                    state.parser_availability = ParserAvailability::Unavailable;
                }
                log::error!("[terminal-snapshot] stage=parser_fault session={id}");
                TerminalScreenCopyRead::Unavailable
            }
        }
    }

    /// #1032 - the live grid's rows, plain text, for the context scrape.
    ///
    /// TWO-state on purpose. The fanout knows nothing about children, so `None` here means
    /// "no parser for this id" and says NOTHING about whether the session is over: only a
    /// backend holds the liveness oracle, so only a backend can make that call.
    ///
    /// Sync, and the rows are cloned out: the guard is a local and is released at the
    /// return, so no caller can hold it across an await.
    pub fn get_screen_rows(&self, id: Uuid) -> Option<Vec<String>> {
        let parsers = self.screen_parsers.lock().ok()?;
        let state = parsers.get(&id)?;
        if state.parser_availability != ParserAvailability::Available {
            return None;
        }
        let screen = state.parser.screen();
        let (_rows, cols) = screen.size();
        Some(screen.rows(0, cols).collect())
    }

    /// #1171 - the same live grid `get_screen_rows` clones, but only when it CHANGED since
    /// the caller last looked, plus the two O(1) facts the watcher engine needs from the same
    /// guard: the per-row wrap flags and the cursor row.
    ///
    /// The unchanged path clones nothing and allocates nothing: `ScreenRowsSince::Unchanged`
    /// carries no rows, so that property is enforced by the type rather than by a comment.
    /// This is what lets a 200 ms sampler cost less than the 5 s one it sits beside.
    ///
    /// `seen` is compared against the WHOLE stamp, sequence and size, because
    /// `resize_screen_and_broadcast` reflows the grid without bumping `output_sequence`
    /// (`:202-212`) - a sequence-only comparison would report `Unchanged` over a screen that
    /// was just re-laid at a new width. `output_sequence` is only READ here; its semantics
    /// are #955's and are untouched.
    ///
    /// TWO-state on the negative side for the same reason `get_screen_rows` is: `Missing`
    /// means "no parser for this id" and says NOTHING about whether the session is over. Only
    /// a backend holds the liveness oracle, so only a backend can turn this into `Gone`.
    ///
    /// Sync, and everything is cloned out: the guard is a local and is released at the return,
    /// so no caller can hold it across an await.
    pub fn get_screen_rows_since(
        &self,
        id: Uuid,
        seen: Option<crate::pty::watchers::FrameStamp>,
    ) -> crate::pty::watchers::ScreenRowsSince {
        use crate::pty::watchers::{FrameStamp, ScreenFrame, ScreenRowsSince};

        let Ok(parsers) = self.screen_parsers.lock() else {
            return ScreenRowsSince::Missing;
        };
        let Some(state) = parsers.get(&id) else {
            return ScreenRowsSince::Missing;
        };
        if state.parser_availability != ParserAvailability::Available {
            return ScreenRowsSince::Missing;
        }
        let screen = state.parser.screen();
        let (grid_rows, cols) = screen.size();
        let stamp = FrameStamp {
            sequence: state.output_sequence,
            rows: grid_rows,
            cols,
        };
        if seen == Some(stamp) {
            return ScreenRowsSince::Unchanged;
        }

        let rows: Vec<String> = screen.rows(0, cols).collect();
        // Indexed off the cloned rows and not off `size().0`, so `wrapped.len() == rows.len()`
        // holds by construction: the frame diff indexes the two together.
        let wrapped: Vec<bool> = (0..rows.len() as u16)
            .map(|row| screen.row_wrapped(row))
            .collect();
        ScreenRowsSince::Frame(ScreenFrame {
            rows,
            wrapped,
            cursor_row: screen.cursor_position().0,
            stamp: Some(stamp),
        })
    }

    pub fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        let parsers = self.screen_parsers.lock().ok()?;
        let state = parsers.get(&id)?;
        if state.parser_availability != ParserAvailability::Available {
            return None;
        }
        Some(state.parser.screen().size())
    }

    /// #1171 - poison `screen_parsers` deterministically, so the "a lock we could not take
    /// makes no claim about the session" arm is covered by a test rather than by reasoning.
    /// Mould: `PtyManager::poison_route_registry_for_test` (`manager.rs:346-355`).
    /// Test-only: parks the sequence one step from overflow, which is the reachable way to make
    /// the next chunk take the parser-fault path of `handle_output`.
    #[cfg(test)]
    pub(crate) fn exhaust_output_sequence_for_test(&self, id: Uuid) {
        let mut parsers = self.screen_parsers.lock().expect("parser state");
        parsers
            .get_mut(&id)
            .expect("registered session")
            .output_sequence = u64::MAX;
    }

    #[cfg(test)]
    pub(crate) fn poison_screen_parsers_for_test(&self) {
        let parsers = Arc::clone(&self.screen_parsers);
        let result = std::panic::catch_unwind(move || {
            let _guard = parsers.lock().unwrap();
            panic!("poison the screen parser map for deterministic test coverage");
        });
        assert!(result.is_err(), "screen-parser poison fixture must panic");
    }

    /// #1439 test-only: force the parser grid WITHOUT recording a ConPTY grid,
    /// simulating any historical silent-skip divergence.
    #[cfg(test)]
    pub(crate) fn desync_screen_size_for_test(&self, id: Uuid, rows: u16, cols: u16) {
        let mut parsers = self.screen_parsers.lock().expect("parser state");
        parsers
            .get_mut(&id)
            .expect("registered session")
            .parser
            .set_size(rows, cols);
    }

    pub fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: std::path::PathBuf,
    ) {
        if let Ok(mut watchers) = self.response_watchers.lock() {
            watchers.insert(
                (session_id, request_id),
                ResponseWatcher {
                    response_dir,
                    buffer: None,
                    capturing: false,
                },
            );
        }
    }
}

/// #973 (B) - is there a cell on this screen a human could see?
///
/// "Visible" is deliberately not "the child wrote a byte", and not even "a cell has contents":
///
/// - A **space is not content.** `vt100::Cell::has_contents` is true for one (it only asks
///   whether the cell was written), and a TUI painting its still-empty viewport writes plenty.
///   That is the exact moment the gate must stay shut, so the glyph test ignores whitespace.
/// - A **coloured space IS content.** TUIs draw status bars, boxes and selections with nothing
///   but a background colour, and `Cell::clear` KEEPS the attributes, so `ESC[41m ESC[2J` is a
///   red screen holding no contents at all. A human plainly sees it.
/// - **Reverse video** does the same thing with the default colours: a space rendered inverse is
///   a solid block.
///
/// Cost: the attribute scan is allocation-free and exits at the first visible cell.
/// `Screen::contents` is one allocation for the whole grid, where `Cell::contents` would be one
/// per cell. It runs only until the gate opens, and after that the caller's relaxed bool skips
/// it entirely.
fn screen_shows_visible_content(screen: &vt100::Screen) -> bool {
    let (rows, cols) = screen.size();
    for row in 0..rows {
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                if cell.inverse() || cell.bgcolor() != vt100::Color::Default {
                    return true;
                }
            }
        }
    }

    screen.contents().chars().any(|c| !c.is_whitespace())
}

/// Strip ANSI escape sequences so marker detection ignores color, cursor,
/// title, hyperlink, shell-integration, and device-control noise.
///
/// #973 - this is the IDLE DETECTOR's predicate, and the startup gate no longer uses it. It is
/// stateless and consumes `ESC` plus exactly one char, which is fine for deciding whether a chunk
/// looked busy and wrong for deciding whether a child has painted. See
/// `SessionIoFanout::has_rendered_visible_content`.
pub(crate) fn strip_ansi_csi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some(&'[') => {
                    chars.next();
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(&']') => {
                    chars.next();
                    while let Some(&ch) = chars.peek() {
                        if ch == '\x07' {
                            chars.next();
                            break;
                        }
                        if ch == '\x1b' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                            continue;
                        }
                        chars.next();
                    }
                }
                Some(&'P') => {
                    chars.next();
                    while let Some(&ch) = chars.peek() {
                        if ch == '\x1b' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                            continue;
                        }
                        chars.next();
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn output_has_printable_activity(text: &str) -> bool {
    let is_printable = |c: char| c > ' ' && c != '\u{FFFD}';
    if text.contains('\x1b') {
        strip_ansi_csi(text).chars().any(is_printable)
    } else {
        text.chars().any(is_printable)
    }
}

/// Scan PTY output text for %%AC_RESPONSE::<rid>::START/END%% markers.
pub(crate) fn scan_response_markers(session_id: Uuid, text: &str, watchers: &ResponseWatcherMap) {
    let completed = {
        let Ok(mut watchers) = watchers.lock() else {
            return;
        };

        let keys: Vec<(Uuid, String)> = watchers
            .keys()
            .filter(|(sid, _)| *sid == session_id)
            .cloned()
            .collect();

        let mut completed = None;
        for key in keys {
            let (_, rid) = &key;
            let start_marker = format!("%%AC_RESPONSE::{}::START%%", rid);
            let end_marker = format!("%%AC_RESPONSE::{}::END%%", rid);

            let completion = {
                let Some(watcher) = watchers.get_mut(&key) else {
                    continue;
                };

                if watcher.capturing {
                    if let Some(end_pos) = text.find(&end_marker) {
                        if let Some(buffer) = &mut watcher.buffer {
                            buffer.push_str(&text[..end_pos]);
                        }
                        Some((
                            watcher.response_dir.clone(),
                            watcher.buffer.take().unwrap_or_default().trim().to_string(),
                        ))
                    } else {
                        if let Some(buffer) = &mut watcher.buffer {
                            buffer.push_str(text);
                        }
                        None
                    }
                } else if let Some(start_pos) = text.find(&start_marker) {
                    watcher.capturing = true;
                    let after_start = &text[start_pos + start_marker.len()..];
                    if let Some(end_pos) = after_start.find(&end_marker) {
                        Some((
                            watcher.response_dir.clone(),
                            after_start[..end_pos].trim().to_string(),
                        ))
                    } else {
                        watcher.buffer = Some(after_start.to_string());
                        None
                    }
                } else {
                    None
                }
            };

            if let Some((response_dir, content)) = completion {
                watchers.remove(&key);
                completed = Some((response_dir, rid.clone(), content));
                break;
            }
        }
        completed
    };

    if let Some((response_dir, request_id, content)) = completed {
        write_response_file(&response_dir, session_id, &request_id, content);
    }
}

fn write_response_file(
    response_dir: &std::path::Path,
    session_id: Uuid,
    request_id: &str,
    content: String,
) {
    let response_path = response_dir.join(format!("{}.json", request_id));
    if let Err(e) = std::fs::create_dir_all(response_dir) {
        log::warn!("Failed to create responses dir: {}", e);
    }

    let response_json = serde_json::json!({
        "requestId": request_id,
        "content": content,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    match serde_json::to_string_pretty(&response_json) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&response_path, json) {
                log::warn!("Failed to write response file: {}", e);
            } else {
                log::info!(
                    "Captured response for request {} from session {}",
                    request_id,
                    session_id
                );
            }
        }
        Err(e) => log::warn!("Failed to serialize response: {}", e),
    }
}

#[cfg(test)]
mod tests;
