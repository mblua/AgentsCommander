use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

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

        #[cfg(test)]
        self.trace(FanoutTraceEvent::TouchSilence);
        self.idle_detector.touch_silence(id);

        let text = String::from_utf8_lossy(&data);
        if text.contains('\u{FFFD}') {
            log::debug!(
                "[PTY] session {} chunk had invalid UTF-8 at buffer boundary ({} bytes, {} replacement chars)",
                id,
                n,
                text.matches('\u{FFFD}').count()
            );
        }

        if output_has_printable_activity(&text) {
            #[cfg(test)]
            self.trace(FanoutTraceEvent::PrintableActivity);
            self.idle_detector.record_activity_with_bytes(id, n);
        } else {
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
mod tests {
    use super::*;

    fn fanout() -> SessionIoFanout {
        SessionIoFanout::new(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
        )
    }

    /// A coding agent's TUI on the way up: hide the cursor, clear, switch to the alternate
    /// screen, home, set the title, turn on mouse reporting, reset the attributes. Every byte of
    /// it is an escape sequence. It paints NOTHING a human can see, and the gate must stay shut
    /// through all of it.
    ///
    /// The charset designator a real TUI also emits here (`ESC ( B`) is deliberately NOT in this
    /// constant: it fools the old predicate even UNSPLIT, so putting it here would let the
    /// chunk-boundary test below pass for the wrong reason. It gets its own test.
    const TUI_PROLOGUE: &[u8] =
        b"\x1b[?25l\x1b[2J\x1b[?1049h\x1b[H\x1b]0;cmd.exe\x07\x1b[?1000h\x1b[?1006h\x1b[m";

    fn feed(fanout: &SessionIoFanout, id: Uuid, chunks: &[&[u8]]) {
        let token = fanout.registration_token_for_test(id);
        for chunk in chunks {
            fanout.handle_output(&token, &id.to_string(), chunk.to_vec());
        }
    }

    fn session(fanout: &SessionIoFanout) -> Uuid {
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
            .expect("register test session");
        id
    }

    /// #973 (B), (a) - CHUNK BOUNDARIES. The trigger must survive the prologue arriving in two
    /// reads, split anywhere.
    ///
    /// The old predicate could not. `strip_ansi_csi` is stateless and keeps no residue between
    /// calls, but the read loop feeds it raw `read()` chunks, and conhost really does split its
    /// output across writes. A chunk ending mid-CSI or mid-OSC hands the tail to the next call
    /// with NO leading `ESC`, and `1049h`, `2J` and `cmd.exe` are all printable - so the gate
    /// opened on a child that had painted nothing, which is the bug the gate exists to prevent.
    ///
    /// It is not an edge case. **43 of this prologue's 51 interior split points** fool the old
    /// predicate: a boundary inside a CSI leaves the tail with no `ESC`, and the tail of nearly
    /// every sequence here is printable - `l`, `2J`, `1049h`, `m`, and `cmd.exe` out of the OSC
    /// title. Only a boundary that happens to land ON an `ESC` is safe.
    ///
    /// This asserts the hazard is REAL first (some split does fool the old predicate), or the
    /// rest of the test would prove nothing, and then walks every interior split point through
    /// the new trigger.
    #[test]
    fn the_gate_holds_a_prologue_split_at_any_chunk_boundary() {
        let fanout = fanout();

        // whole, in one chunk, the old predicate gets right
        assert!(
            !output_has_printable_activity(&String::from_utf8_lossy(TUI_PROLOGUE)),
            "the prologue paints nothing, and unsplit even the old predicate saw that"
        );

        // split, it does not. These are the boundaries that would have opened the gate.
        let fooled: Vec<usize> = (1..TUI_PROLOGUE.len())
            .filter(|&at| {
                let (head, tail) = TUI_PROLOGUE.split_at(at);
                output_has_printable_activity(&String::from_utf8_lossy(head))
                    || output_has_printable_activity(&String::from_utf8_lossy(tail))
            })
            .collect();
        assert!(
            !fooled.is_empty(),
            "if no split fools the old predicate, this test is not testing the hazard"
        );

        // the new trigger, at every one of them
        for at in 1..TUI_PROLOGUE.len() {
            let id = session(&fanout);
            let (head, tail) = TUI_PROLOGUE.split_at(at);
            feed(&fanout, id, &[head, tail]);
            assert!(
                !fanout.has_rendered_visible_content(id),
                "split at {at}: the child has painted nothing, so the gate must stay shut"
            );
        }

        // ...and it is a gate, not a wall: the moment the child paints, it opens
        let id = session(&fanout);
        feed(&fanout, id, &[TUI_PROLOGUE]);
        assert!(
            !fanout.has_rendered_visible_content(id),
            "still nothing to see"
        );
        feed(&fanout, id, &[b"> "]);
        assert!(
            fanout.has_rendered_visible_content(id),
            "a glyph the user can see must open the gate"
        );
    }

    /// #973 (B), (b) - THREE-BYTE ESCAPES. `ESC ( B` is what ncurses and half the TUI world emit
    /// on the way up. The old stripper consumed `ESC` plus exactly ONE char, so the `B` survived
    /// and read as printable, and the gate opened on a blank screen. Same for `ESC ) 0` (leaks
    /// `0`) and `ESC % G` (leaks `G`).
    ///
    /// `ESC # 8` (DECALN) is deliberately not asserted here. It is the fourth three-byte escape
    /// and it fools the old stripper too (leaks `8`), but it is the one that genuinely PAINTS: a
    /// real terminal fills the screen with `E`. vt100 does not implement it, so our parser shows
    /// nothing and this gate would stay shut - which is a limitation of the crate, not a property
    /// worth pinning as correct. Pinning it would freeze the wrong answer.
    #[test]
    fn a_three_byte_charset_designator_does_not_open_the_gate() {
        assert!(
            output_has_printable_activity("\x1b(B"),
            "the old predicate really is fooled by this - that is the whole defect"
        );

        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b(B", b"\x1b)0", b"\x1b%G"]);

        assert!(
            !fanout.has_rendered_visible_content(id),
            "a charset designator paints nothing: the gate must stay shut"
        );
    }

    /// What "a human can see" means, pinned. A space is not content, even though the cell was
    /// written and `Cell::has_contents` says true. A COLOURED space is - TUIs draw status bars
    /// and boxes with nothing else, and `Cell::clear` keeps the attributes, so a cleared screen
    /// can be solid red while holding no contents at all.
    #[test]
    fn a_blank_viewport_is_not_content_but_a_coloured_one_is() {
        let fanout = fanout();

        // the TUI paints its still-empty viewport: spaces, and plenty of them. This is the exact
        // moment the gate must stay shut - it is inside the danger window.
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b[2J\x1b[H", &[b' '; 240]]);
        assert!(
            !fanout.has_rendered_visible_content(id),
            "a viewport of spaces is still a blank viewport"
        );
        feed(&fanout, id, &[b"ready"]);
        assert!(
            fanout.has_rendered_visible_content(id),
            "a glyph is content"
        );

        // a red status bar: no glyph anywhere, and a human plainly sees it
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b[41m", &[b' '; 20]]);
        assert!(
            fanout.has_rendered_visible_content(id),
            "a coloured space is content: it is how a TUI draws a status bar"
        );

        // reverse video does it with the default colours
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b[7m", &[b' '; 20]]);
        assert!(
            fanout.has_rendered_visible_content(id),
            "an inverse space renders as a solid block"
        );
    }

    /// #973 / #955 - a degenerate size must never reach the vt100 parser.
    ///
    /// Red before the guard, and not with an assertion - with a **panic**:
    ///
    /// ```text
    /// panicked at vt100-0.15.2/src/grid.rs:74:34: attempt to subtract with overflow
    /// ```
    ///
    /// `vt100::grid::set_size` computes `size.rows - 1` on a `u16`, so `rows == 0` underflows.
    /// Debug panics; release wraps to 65535. Worse, the panic fires while
    /// `resize_screen_and_broadcast` holds `screen_parsers`, poisoning it - and every reader of
    /// that mutex swallows the poison silently (`if let Ok` / `.ok()?`), so #955's snapshot,
    /// the output sequence numbering and `get_pty_size` would go dead for EVERY session, for
    /// the life of the process, without a log line. A black tile on re-attach is what the user
    /// would see: exactly the bug #955 shipped to kill.
    #[test]
    fn a_degenerate_resize_never_reaches_the_vt100_parser() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
            .expect("register session");
        let token = fanout.registration_token_for_test(id);
        fanout.handle_output(&token, &id.to_string(), b"hello".to_vec());

        fanout.resize_screen_and_broadcast(id, 0, 0);

        let after = fanout.get_screen_snapshot(id).expect("snapshot");
        assert_eq!(
            (after.rows, after.cols),
            (30, 120),
            "the screen must be untouched by a size the child was never given"
        );
        assert!(
            String::from_utf8_lossy(&after.data).contains("hello"),
            "and the child's output must still be in it: an empty snapshot is #955's black tile"
        );

        // a real size still moves the screen - the guard refuses the degenerate, not the resize
        fanout.resize_screen_and_broadcast(id, 80, 24);
        let resized = fanout.get_screen_snapshot(id).expect("snapshot");
        assert_eq!((resized.rows, resized.cols), (24, 80));
    }

    #[test]
    fn printable_activity_ignores_ansi_only_chunks() {
        assert!(!output_has_printable_activity("\x1b[31m\x1b[0m"));
        assert!(!output_has_printable_activity("\x1b]0;title\x07"));
        assert!(output_has_printable_activity("\x1b[31mready\x1b[0m"));
    }

    const WINDOW: &str = "main";
    const SECOND_WINDOW: &str = "terminal-2";

    fn new_sink() -> PtyOutputTestSink {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn session_with_sink(fanout: &SessionIoFanout, sink: &PtyOutputTestSink) -> Uuid {
        let id = Uuid::new_v4();
        fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                30,
                120,
                PtyOutputTarget::from_test_sink(Arc::clone(sink)),
            )
            .expect("register session with sink");
        id
    }

    fn attach(fanout: &SessionIoFanout, id: Uuid, label: &str) -> Option<PtyScreenSnapshot> {
        fanout
            .activate_terminal_output(id, label, true)
            .expect("attach")
    }

    /// Drives the flush by hand. Nothing on this path emits synchronously except the 64 KiB
    /// threshold, and test builds hold the 16 ms timer, so every emit a test observes is the one
    /// it asked for and no assertion races a task.
    fn flush(fanout: &SessionIoFanout, id: Uuid) {
        fanout.flush_terminal_output_for_test(id);
    }

    fn events(sink: &PtyOutputTestSink) -> Vec<PtyOutputTestEvent> {
        sink.lock().expect("target sink").clone()
    }

    /// The non-UI consumers keep their order and their behaviour, and the UI is still served
    /// last of all, which is what keeps a slow emit from delaying any of them. What changed is
    /// that the UI step no longer EMITS inside the fanout: it appends to the session's 16 ms
    /// batch, and the flush is what emits.
    #[test]
    fn fanout_characterization_preserves_raw_order_and_ui_is_last() {
        let id = Uuid::new_v4();
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
        output_senders.lock().unwrap().insert(id, sender);
        let broadcaster = crate::web::broadcast::WsBroadcaster::new();
        let mut websocket_receiver = broadcaster.subscribe();
        let sink = new_sink();
        let fanout = SessionIoFanout::new(
            output_senders,
            IdleDetector::new(|_| {}, |_| {}),
            Some(broadcaster),
        );
        let token = fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                30,
                120,
                PtyOutputTarget::from_test_sink(Arc::clone(&sink)),
            )
            .expect("register fanout");

        assert!(attach(&fanout, id, WINDOW).is_some());
        assert!(fanout.take_trace_for_test().is_empty());

        let bytes = b"characterized raw bytes".to_vec();
        fanout.handle_output(&token, &id.to_string(), bytes.clone());

        assert_eq!(
            fanout.take_trace_for_test(),
            vec![
                FanoutTraceEvent::TouchSilence,
                FanoutTraceEvent::PrintableActivity,
                FanoutTraceEvent::ResponseMarkers,
                FanoutTraceEvent::OutputSender,
                FanoutTraceEvent::ParserProcessed(1),
                FanoutTraceEvent::Websocket,
                FanoutTraceEvent::UiEmit,
            ]
        );
        assert_eq!(receiver.try_recv().expect("raw sender payload"), bytes);
        match websocket_receiver.try_recv().expect("websocket payload") {
            crate::web::broadcast::WsOutMsg::Binary(frame) => {
                assert_eq!(&frame[36..], bytes.as_slice());
            }
            other => panic!("expected websocket binary payload, got {other:?}"),
        }
        assert!(
            events(&sink).is_empty(),
            "the ingest coalesces; the flush is what emits"
        );

        flush(&fanout, id);
        let emitted = events(&sink);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].1, bytes);
        assert_eq!(emitted[0].2, Some(1));
    }

    #[test]
    fn output_targets_have_explicit_success_and_failure_results() {
        let payload = PtyOutputPayload {
            session_id: Uuid::new_v4().to_string(),
            data: b"target payload".to_vec(),
            sequence: Some(1),
        };
        let labels = vec![WINDOW.to_string()];
        assert_eq!(
            PtyOutputTarget::noop().emit_pty_output(&labels, payload.clone()),
            Ok(())
        );

        let sink = new_sink();
        let failing = PtyOutputTarget::failing_test_sink(Arc::clone(&sink));
        assert_eq!(
            failing.emit_pty_output(&labels, payload),
            Err(PtyOutputEmitError::Emit)
        );
        assert_eq!(events(&sink).len(), 1);
    }

    /// Section 5's review gate, executable.
    ///
    /// Delivery gating is a map from session id to the SET of window labels watching it, so two
    /// windows on one session hold two attachments and one window's detach cannot mute the
    /// other. A single slot fails at the second attach, a single key or an uncounted set fails
    /// at the first detach, and per-window bookkeeping without counts fails the same way.
    #[test]
    fn attachments_are_counted_by_window_label() {
        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout.registration_token_for_test(id);

        attach(&fanout, id, WINDOW);
        attach(&fanout, id, SECOND_WINDOW);
        assert_eq!(
            fanout.attached_labels_for_test(id),
            vec![WINDOW.to_string(), SECOND_WINDOW.to_string()]
        );

        fanout.detach_terminal_output(id, SECOND_WINDOW);
        fanout.handle_output(&token, &id.to_string(), b"still watched".to_vec());
        flush(&fanout, id);
        let emitted = events(&sink);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].1, b"still watched");

        fanout.detach_terminal_output(id, WINDOW);
        assert!(fanout.attached_labels_for_test(id).is_empty());
        fanout.handle_output(&token, &id.to_string(), b"unwatched".to_vec());
        flush(&fanout, id);
        assert_eq!(events(&sink).len(), 1);
    }

    /// Criterion M, and the case that IS #1363: two windows on two sessions, both delivering at
    /// the same time with no reactivation. A single delivery slot passes every other test in
    /// this family and fails this one, which is why it is here.
    #[test]
    fn two_windows_on_two_sessions_both_emit() {
        let fanout = fanout();
        let first = new_sink();
        let second = new_sink();
        let a = session_with_sink(&fanout, &first);
        let b = session_with_sink(&fanout, &second);
        let token_a = fanout.registration_token_for_test(a);
        let token_b = fanout.registration_token_for_test(b);

        attach(&fanout, a, WINDOW);
        attach(&fanout, b, SECOND_WINDOW);
        fanout.handle_output(&token_a, &a.to_string(), b"from a".to_vec());
        fanout.handle_output(&token_b, &b.to_string(), b"from b".to_vec());
        flush(&fanout, a);
        flush(&fanout, b);

        let from_a = events(&first);
        let from_b = events(&second);
        assert_eq!(from_a.len(), 1);
        assert_eq!(from_a[0].1, b"from a");
        assert_eq!(from_b.len(), 1);
        assert_eq!(from_b[0].1, b"from b");
    }

    /// The ingest emit, and its complement: an attached session's chunks produce payloads with
    /// the right bytes and a monotonic sequence, and a session nobody attached produces nothing
    /// and retains nothing.
    #[test]
    fn only_attached_sessions_emit_and_sequences_stay_monotonic() {
        let fanout = fanout();
        let watched_sink = new_sink();
        let hidden_sink = new_sink();
        let watched = session_with_sink(&fanout, &watched_sink);
        let hidden = session_with_sink(&fanout, &hidden_sink);
        let watched_token = fanout.registration_token_for_test(watched);
        let hidden_token = fanout.registration_token_for_test(hidden);

        attach(&fanout, watched, WINDOW);
        for chunk in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
            fanout.handle_output(&watched_token, &watched.to_string(), chunk.to_vec());
            flush(&fanout, watched);
        }
        let emitted = events(&watched_sink);
        assert_eq!(
            emitted
                .iter()
                .map(|event| (event.1.clone(), event.2))
                .collect::<Vec<_>>(),
            vec![
                (b"one".to_vec(), Some(1)),
                (b"two".to_vec(), Some(2)),
                (b"three".to_vec(), Some(3)),
            ]
        );

        fanout.handle_output(&hidden_token, &hidden.to_string(), b"unseen".to_vec());
        flush(&fanout, hidden);
        assert!(events(&hidden_sink).is_empty());
        assert_eq!(fanout.pending_output_bytes_for_test(hidden), None);
    }

    /// Criterion N. Detach is total and window local: an unattached session, a second detach
    /// past zero and a destroyed session all succeed, and no window's detach can reach another
    /// window's attachment.
    #[test]
    fn detach_is_idempotent_and_never_crosses_windows() {
        let fanout = fanout();
        let sink = new_sink();
        let a = session_with_sink(&fanout, &sink);
        let b = session_with_sink(&fanout, &sink);
        let token_a = fanout.registration_token_for_test(a);

        fanout.detach_terminal_output(a, WINDOW);
        attach(&fanout, a, WINDOW);
        attach(&fanout, b, SECOND_WINDOW);
        fanout.detach_terminal_output(b, SECOND_WINDOW);
        fanout.detach_terminal_output(b, SECOND_WINDOW);
        fanout.remove_session(b);
        fanout.detach_terminal_output(b, SECOND_WINDOW);

        assert_eq!(fanout.attached_labels_for_test(a), vec![WINDOW.to_string()]);
        fanout.handle_output(&token_a, &a.to_string(), b"a keeps delivering".to_vec());
        flush(&fanout, a);
        assert_eq!(events(&sink).len(), 1);
    }

    /// Criterion L. A session whose parser is gone still attaches and still emits, with no
    /// sequence, and the client writes those bytes live: emitting nothing there is #955, the
    /// permanently black terminal, and there is no recovery lane left to repair it. A rejected
    /// attach, by contrast, changes nothing at all.
    #[test]
    fn an_unavailable_parser_still_attaches_and_emits_unsequenced() {
        let fanout = fanout();
        let sink = new_sink();
        let id = Uuid::new_v4();
        let token = fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                1,
                1,
                PtyOutputTarget::from_test_sink(Arc::clone(&sink)),
            )
            .expect("register one-cell session");
        // A wide grapheme in a one-column grid removes this session's parser for good.
        fanout.handle_output(&token, &id.to_string(), "界".as_bytes().to_vec());

        assert!(matches!(
            fanout.activate_terminal_output(id, WINDOW, true),
            Ok(None)
        ));
        assert_eq!(
            fanout.attached_labels_for_test(id),
            vec![WINDOW.to_string()]
        );

        fanout.handle_output(&token, &id.to_string(), b"after the fault".to_vec());
        flush(&fanout, id);
        let emitted = events(&sink);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].1, b"after the fault");
        assert_eq!(emitted[0].2, None);

        let absent = Uuid::new_v4();
        assert!(matches!(
            fanout.activate_terminal_output(absent, WINDOW, true),
            Err(TerminalOutputAttachError::SessionUnavailable)
        ));
        assert!(fanout.attached_labels_for_test(absent).is_empty());
    }

    /// Criterion O. Destroying the session releases its attachments and drops its pending
    /// bytes in the backend, whatever the frontend did, and the ingest stops even with an
    /// attachment outstanding.
    #[test]
    fn destroying_a_session_releases_its_attachments_and_pending_bytes() {
        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout.registration_token_for_test(id);

        attach(&fanout, id, WINDOW);
        fanout.handle_output(&token, &id.to_string(), b"pending".to_vec());
        assert_eq!(
            fanout.pending_output_bytes_for_test(id),
            Some(b"pending".len())
        );

        fanout.remove_session(id);
        assert!(fanout.attached_labels_for_test(id).is_empty());
        assert_eq!(fanout.pending_output_bytes_for_test(id), None);
        flush(&fanout, id);
        fanout.handle_output(&token, &id.to_string(), b"after destroy".to_vec());
        assert!(events(&sink).is_empty());
    }

    /// Criterion O, the other release site. A destroyed window's attachments go with it, in the
    /// backend, with no frontend call - and only that window's: this is what closes the leak
    /// class rather than mitigating it, and why the frontend close hook need not block.
    #[test]
    fn destroying_a_window_releases_only_its_own_attachments() {
        let fanout = fanout();
        let shared_sink = new_sink();
        let solo_sink = new_sink();
        let shared = session_with_sink(&fanout, &shared_sink);
        let solo = session_with_sink(&fanout, &solo_sink);
        let shared_token = fanout.registration_token_for_test(shared);
        let solo_token = fanout.registration_token_for_test(solo);

        attach(&fanout, shared, WINDOW);
        attach(&fanout, shared, SECOND_WINDOW);
        attach(&fanout, solo, SECOND_WINDOW);

        fanout.release_window_attachments(SECOND_WINDOW);
        assert_eq!(
            fanout.attached_labels_for_test(shared),
            vec![WINDOW.to_string()]
        );
        assert!(fanout.attached_labels_for_test(solo).is_empty());

        fanout.handle_output(&shared_token, &shared.to_string(), b"survives".to_vec());
        fanout.handle_output(&solo_token, &solo.to_string(), b"silenced".to_vec());
        flush(&fanout, shared);
        flush(&fanout, solo);
        assert_eq!(events(&shared_sink).len(), 1);
        assert!(events(&solo_sink).is_empty());
    }

    /// Section 3.4.3 rule 4. A detach DRAINS rather than skipping: a flush that fires after it
    /// emits nothing, and the bytes it would have carried are gone rather than waiting to
    /// surface on a later re-attach, out of order and after that attach's reset.
    #[test]
    fn a_flush_after_detach_emits_nothing_and_drops_the_bytes() {
        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout.registration_token_for_test(id);

        attach(&fanout, id, WINDOW);
        fanout.handle_output(&token, &id.to_string(), b"never delivered".to_vec());
        fanout.detach_terminal_output(id, WINDOW);
        assert_eq!(fanout.pending_output_bytes_for_test(id), None);

        flush(&fanout, id);
        attach(&fanout, id, WINDOW);
        flush(&fanout, id);
        assert!(events(&sink).is_empty());
    }

    /// Section 3.4.1. The attach cuts the batch exactly at the snapshot: what was pending goes
    /// to the windows already watching, the snapshot carries every byte up to its own sequence,
    /// and the ingest continues from there. No batch straddles the boundary, which is what makes
    /// one scalar `sequence` sufficient to describe a coalesced batch.
    #[test]
    fn attach_cuts_the_batch_exactly_at_the_snapshot_boundary() {
        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout.registration_token_for_test(id);

        attach(&fanout, id, WINDOW);
        fanout.handle_output(&token, &id.to_string(), b"before\r\n".to_vec());

        let snapshot = attach(&fanout, id, SECOND_WINDOW).expect("snapshot on attach");
        assert_eq!(snapshot.sequence, 1);
        assert!(String::from_utf8_lossy(&snapshot.data).contains("before"));
        assert_eq!(fanout.pending_output_bytes_for_test(id), Some(0));
        let seeded = events(&sink);
        assert_eq!(
            seeded.len(),
            1,
            "the window already watching keeps its bytes"
        );
        assert_eq!(seeded[0].1, b"before\r\n");
        assert_eq!(seeded[0].2, Some(1));

        fanout.handle_output(&token, &id.to_string(), b"after".to_vec());
        flush(&fanout, id);
        let emitted = events(&sink);
        assert_eq!(emitted.len(), 2);
        assert_eq!(emitted[1].1, b"after");
        assert_eq!(emitted[1].2, Some(2));
    }

    /// #1439. A parser grid that diverged from the grid the ConPTY last took must never seed
    /// an attach: the attach returns no snapshot, converges the parser onto the RECORDED
    /// grid, and the next attach seeds cleanly at that grid with the current sequence.
    ///
    /// Grid constraints are load-bearing: registration R0=30x120 (from `session_with_sink`),
    /// follow A=24x80, desync B=50x132 are pairwise distinct, each asymmetric, and no pair is
    /// a transposition of another. With A != R0, an implementation that never writes
    /// `conpty_size` converges onto R0 and the final grid assertion turns red; asymmetric,
    /// non-transposed grids make a rows/cols argument swap fail loudly (the #973 bug class).
    #[test]
    fn a_grid_divergence_yields_no_seed_and_the_next_attach_seeds_clean() {
        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout.registration_token_for_test(id);

        // One follow the ConPTY took: parser and record both move to A = 24 rows x 80 cols.
        fanout.resize_screen_and_broadcast(id, 80, 24);
        fanout.handle_output(&token, &id.to_string(), b"seed me\r\n".to_vec());

        // The divergence class: the parser grid moves, the record does not.
        fanout.desync_screen_size_for_test(id, 50, 132);

        let seed = attach(&fanout, id, WINDOW);
        assert!(seed.is_none(), "a diverged grid must not seed the attach");
        // No batch entry exists yet: the only chunk so far arrived unattached, and a
        // seedless attach creates none (same `None` the detach test observes).
        assert_eq!(fanout.pending_output_bytes_for_test(id), None);

        // The seedless window still attached: live bytes keep flowing to it, sequenced.
        fanout.handle_output(&token, &id.to_string(), b"after divergence".to_vec());
        flush(&fanout, id);
        let emitted = events(&sink);
        assert_eq!(
            emitted.len(),
            1,
            "the attached window keeps receiving its bytes"
        );
        assert_eq!(emitted[0].1, b"after divergence");
        assert_eq!(emitted[0].2, Some(2));

        let reseeded =
            attach(&fanout, id, SECOND_WINDOW).expect("the attach after convergence seeds");
        assert_eq!(
            (reseeded.rows, reseeded.cols),
            (24, 80),
            "the parser converged onto the recorded ConPTY grid"
        );
        assert_eq!(reseeded.sequence, 2);
        assert_eq!(fanout.pending_output_bytes_for_test(id), Some(0));
    }

    /// Section 3.4.3 rule 5. The batch is flushed at the `Available -> Unavailable` transition,
    /// so none of them mixes sequenced and unsequenced bytes. Labelled with the last real
    /// sequence a mixed batch would make the client watermark-drop bytes that were never
    /// seeded; labelled with none, its sequenced prefix would escape reconciliation and
    /// duplicate against the seed.
    #[test]
    fn a_parser_fault_flushes_at_the_transition_and_unsequences_everything_after() {
        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout.registration_token_for_test(id);

        attach(&fanout, id, WINDOW);
        fanout.handle_output(&token, &id.to_string(), b"ok".to_vec());
        // One step from overflow, so the next chunk cannot be sequenced and the parser goes
        // `Unavailable` for good.
        fanout.exhaust_output_sequence_for_test(id);
        fanout.handle_output(&token, &id.to_string(), b"faulting".to_vec());
        flush(&fanout, id);
        fanout.handle_output(&token, &id.to_string(), b"later".to_vec());
        flush(&fanout, id);

        let emitted = events(&sink);
        assert_eq!(emitted.len(), 3);
        assert_eq!(
            (emitted[0].1.clone(), emitted[0].2),
            (b"ok".to_vec(), Some(1)),
            "the sequenced prefix is flushed at the transition"
        );
        assert_eq!(
            (emitted[1].1.clone(), emitted[1].2),
            (b"faulting".to_vec(), None)
        );
        assert_eq!(
            (emitted[2].1.clone(), emitted[2].2),
            (b"later".to_vec(), None)
        );
    }

    /// Criterion E'. The flush fires when the batch REACHES its ceiling, which happens after
    /// appending the chunk that crossed it, so accumulation peaks at 64 KiB plus one ingest
    /// chunk. It runs on the reader thread, and that is the only backpressure left on this path.
    #[test]
    fn the_batch_flushes_on_the_reader_thread_at_its_ceiling() {
        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout.registration_token_for_test(id);
        attach(&fanout, id, WINDOW);

        let chunk = vec![b'.'; 4_096];
        for _ in 0..(UI_BATCH_LIMIT_BYTES / chunk.len()) {
            fanout.handle_output(&token, &id.to_string(), chunk.clone());
        }

        let emitted = events(&sink);
        assert_eq!(
            emitted.len(),
            1,
            "no timer, no explicit flush: the ceiling did it"
        );
        assert_eq!(emitted[0].1.len(), UI_BATCH_LIMIT_BYTES);
        assert_eq!(fanout.pending_output_bytes_for_test(id), Some(0));
    }

    /// Criterion P, at the Tauri event layer. `emit` delivers to EVERY open webview, listener
    /// or not, so an unattached terminal window and the sidebar would each pay a receive-side
    /// deserialization before discarding the payload. `emit_to` per attached label is what makes
    /// the bridge multiplier the number of (session, attached window) pairs.
    #[test]
    fn pty_output_reaches_only_the_attached_webview() {
        use tauri::Listener;

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build attachment app");
        let attached_webview = tauri::WebviewWindowBuilder::new(&app, WINDOW, Default::default())
            .build()
            .expect("attached webview");
        let unattached_webview =
            tauri::WebviewWindowBuilder::new(&app, SECOND_WINDOW, Default::default())
                .build()
                .expect("unattached webview");
        let attached_events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let unattached_events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let any_target_events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Label-shaped registrations, which is what the frontend makes: its `listen` passes
        // `{ target: <this window's label> }`, and Tauri matches an `AnyLabel` candidate
        // against an `emit_to` on the same label exactly as it matches this one.
        let attached_counter = Arc::clone(&attached_events);
        attached_webview.listen("pty_output", move |_| {
            attached_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let unattached_counter = Arc::clone(&unattached_events);
        unattached_webview.listen("pty_output", move |_| {
            unattached_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        // The guard this test exists for is only worth anything while that stays true. An
        // `EventTarget::Any` registration - which is what a bare JS `listen(event, handler)`
        // with no options sends - SHORT-CIRCUITS the label filter: `match_any_or_filter`
        // returns true for `Any` before the filter is ever consulted, so `emit_to` delivers to
        // it anyway. That is asserted below rather than described, so that dropping the
        // target option on the frontend cannot quietly turn criterion P back into a lie.
        let any_target_counter = Arc::clone(&any_target_events);
        unattached_webview.listen_any("pty_output", move |_| {
            any_target_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        let fanout = SessionIoFanout::new(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
        );
        let id = Uuid::new_v4();
        let token = fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                30,
                120,
                PtyOutputTarget::from_app_handle(app.handle().clone()),
            )
            .expect("register app-handle session");

        attach(&fanout, id, WINDOW);
        fanout.handle_output(
            &token,
            &id.to_string(),
            b"only the attached window".to_vec(),
        );
        flush(&fanout, id);

        assert_eq!(attached_events.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            unattached_events.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            any_target_events.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    /// 3.4.1's no-zombie invariant, at the one interleaving that broke it: an attach that
    /// lands while `remove_session` is draining must not leave a label behind for a session
    /// that is about to disappear. The reader lease is the synchronisation point and no sleep
    /// is needed - `remove_session` parks in `wait_for_drain` until it is released, and that
    /// is precisely the window the attach used to slip through.
    #[test]
    fn an_attach_racing_remove_session_leaves_no_label_behind() {
        let sink = new_sink();
        let fanout = fanout();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout
            .registration_token_for_session(id)
            .expect("registration token");
        let lease = fanout.acquire_reader_lease(&token).expect("reader lease");

        std::thread::scope(|scope| {
            let destroyer = scope.spawn(|| fanout.remove_session(id));
            // The gate refusing a new lease IS `remove_session` past its first phase.
            while fanout.acquire_reader_lease(&token).is_some() {
                std::thread::yield_now();
            }
            attach(&fanout, id, WINDOW);
            drop(lease);
            destroyer.join().expect("remove_session thread");
        });

        assert!(fanout.attached_labels_for_test(id).is_empty());
        assert_eq!(fanout.pending_output_bytes_for_test(id), None);
    }

    /// The 16 ms timer, armed and fired for real. Every other backend test holds it and drives
    /// the synchronous seam instead, which is exactly why the spawned task itself had no
    /// coverage at all: it is the only emitter that needs no caller, and if it ever stopped
    /// firing an attached session would emit only on the 64 KiB ceiling, so an interactive
    /// terminal would look frozen until it produced 64 KiB.
    #[test]
    fn the_armed_timer_emits_with_no_explicit_flush() {
        let sink = new_sink();
        let fanout = fanout();
        fanout.enable_output_timer_for_test();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout
            .registration_token_for_session(id)
            .expect("registration token");
        attach(&fanout, id, WINDOW);

        fanout.handle_output(&token, &id.to_string(), b"armed by the ingest".to_vec());

        // The task sleeps on Tauri's own runtime, so this wait is real time. The assertion
        // hangs on the poll, never on the sleep length: the deadline only bounds a failure.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while events(&sink).is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            events(&sink),
            vec![(id.to_string(), b"armed by the ingest".to_vec(), Some(1))]
        );
    }

    #[test]
    fn retired_reader_tokens_cannot_target_a_same_uuid_replacement() {
        let id = Uuid::new_v4();
        let old_sink = new_sink();
        let new_sink_events = new_sink();
        let fanout = fanout();
        let old_token = fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                4,
                40,
                PtyOutputTarget::from_test_sink(Arc::clone(&old_sink)),
            )
            .expect("register old session");
        fanout.remove_session(id);
        let new_token = fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                4,
                40,
                PtyOutputTarget::from_test_sink(Arc::clone(&new_sink_events)),
            )
            .expect("register replacement session");

        fanout.handle_output(&old_token, &id.to_string(), b"old output".to_vec());
        let snapshot = fanout
            .get_screen_snapshot(id)
            .expect("replacement snapshot");
        assert_eq!(snapshot.sequence, 0);
        assert!(!String::from_utf8_lossy(&snapshot.data).contains("old output"));
        assert!(events(&old_sink).is_empty());

        attach(&fanout, id, WINDOW);
        fanout.handle_output(&new_token, &id.to_string(), b"replacement output".to_vec());
        flush(&fanout, id);
        let emitted = events(&new_sink_events);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].1, b"replacement output");
    }

    #[test]
    fn response_marker_capture_writes_trimmed_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = Uuid::new_v4();
        let watchers: ResponseWatcherMap = Arc::new(Mutex::new(HashMap::new()));
        watchers.lock().unwrap().insert(
            (session_id, "r1".to_string()),
            ResponseWatcher {
                response_dir: dir.path().to_path_buf(),
                buffer: None,
                capturing: false,
            },
        );

        scan_response_markers(
            session_id,
            "before %%AC_RESPONSE::r1::START%% {\"ok\": true} %%AC_RESPONSE::r1::END%% after",
            &watchers,
        );

        let json = std::fs::read_to_string(dir.path().join("r1.json")).expect("response json");
        assert!(json.contains("\"requestId\": \"r1\""));
        assert!(json.contains("\"content\": \"{\\\"ok\\\": true}\""));
        assert!(watchers.lock().unwrap().is_empty());
    }

    // ---- #1032: the rows accessor, against the real parser -------------------------

    /// The whole transfer claim of the round-1 capture rests on this: the capture read its
    /// bytes with `contents_between`, and the engine reads them with `rows`. If those two
    /// ever disagree, every measured fact about the statusline is about a screen this
    /// accessor does not return.
    ///
    /// Valid rows only: the `Equal` branch of `contents_between` ends in
    /// `.unwrap_or_default()`, so an out-of-range row answers `""` where `rows()` simply
    /// has no index - comparing those would pin the crate's error handling, not the claim.
    #[test]
    fn get_screen_rows_matches_contents_between() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(
            &fanout,
            id,
            &[
                "  Context \u{2591}\u{2591}\u{2588} 42% \u{2502} Usage\r\nprose above it\r\n"
                    .as_bytes(),
            ],
        );

        let rows = fanout
            .get_screen_rows(id)
            .expect("rows for a registered session");

        let parsers = fanout.screen_parsers.lock().unwrap();
        let screen = parsers.get(&id).expect("parser").parser.screen();
        let (row_count, cols) = screen.size();
        assert_eq!(rows.len(), row_count as usize);
        for r in 0..row_count {
            assert_eq!(
                rows[r as usize],
                screen.contents_between(r, 0, r, cols),
                "row {r} disagrees with the accessor the capture measured"
            );
        }
    }

    /// The column-2 anchor is the engine's only defence against single-row input-box prose,
    /// and it is worth exactly as much as this: that the two leading spaces of a row painted
    /// at column 2 survive the round trip out of the grid.
    #[test]
    fn leading_spaces_survive_so_the_column_two_anchor_works() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"  Context 7% and a tail"]);

        let rows = fanout.get_screen_rows(id).expect("rows");
        assert_eq!(rows[0], "  Context 7% and a tail");
        assert!(
            rows[0].starts_with("  Context"),
            "the anchor rests on these two spaces: {:?}",
            rows[0]
        );
    }

    /// Criterion 1. The mirror is keyed by session id and so is the scraper's map; two
    /// concurrent agents must never read each other's number.
    #[test]
    fn two_sessions_never_cross_rows() {
        let fanout = fanout();
        let a = session(&fanout);
        let b = session(&fanout);

        feed(&fanout, a, &[b"  Context 11%"]);
        feed(&fanout, b, &[b"  Context 88%"]);

        assert_eq!(
            fanout.get_screen_rows(a).expect("rows a")[0],
            "  Context 11%"
        );
        assert_eq!(
            fanout.get_screen_rows(b).expect("rows b")[0],
            "  Context 88%"
        );
    }

    /// The fanout's two-state contract: absent parser is `None`, and that is all it means.
    /// Whether the session is OVER is a question this type cannot answer and does not try to.
    #[test]
    fn get_screen_rows_is_none_for_an_unknown_session() {
        let fanout = fanout();
        assert!(fanout.get_screen_rows(Uuid::new_v4()).is_none());
    }

    /// Criterion 2. The reading comes off the mirror, which AC feeds from the PTY read loop
    /// whether or not a terminal was ever mounted: `PtyOutputTarget::noop()`, never resized,
    /// still reads back.
    #[test]
    fn rows_are_readable_for_a_session_with_no_terminal() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"  Context 5%"]);

        let rows = fanout
            .get_screen_rows(id)
            .expect("a never-mounted session still reads");
        assert_eq!(rows[0], "  Context 5%");
    }

    #[test]
    fn terminal_screen_copy_preserves_cells_styles_colors_and_wide_pairs() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 2, 4)
            .expect("register test session");
        feed(
            &fanout,
            id,
            &["\x1b[?1049h\x1b[31;44;1;3;4;7mA界".as_bytes()],
        );

        let copied = match fanout.copy_terminal_screen(id) {
            crate::pty::backend::TerminalScreenCopyRead::Copied(copied) => copied,
            _ => panic!("expected compact viewport copy"),
        };
        let model = copied
            .into_model(id, SessionBackendKind::LocalProcess)
            .expect("valid owned model");
        assert_eq!(model.screen.sequence, 1);
        assert_eq!(model.screen.active_buffer, TerminalActiveBuffer::Alternate);
        assert_eq!(model.screen.lines.len(), 2);
        assert_eq!(model.screen.lines[0].cells.len(), 4);
        let first = &model.screen.lines[0].cells[0];
        assert_eq!(first.text, "A");
        assert_eq!(first.foreground, TerminalColor::Indexed { index: 1 });
        assert_eq!(first.background, TerminalColor::Indexed { index: 4 });
        assert!(first.style.bold && first.style.italic && first.style.underline);
        assert!(first.style.inverse);
        assert_eq!(
            model.screen.lines[0].cells[1].width,
            TerminalCellWidth::WideLead
        );
        assert_eq!(
            model.screen.lines[0].cells[2].width,
            TerminalCellWidth::WideContinuation
        );
        assert!(model.screen.lines[0].cells[2].text.is_empty());
    }

    #[test]
    fn capture_and_model_result_debug_omit_cell_osc_and_session_canaries() {
        const CELL_CANARY: &str = "CELL_1173_VT_Q8L5";
        const OSC_CANARY: &str = "OSC_1173_VT_Q8L5";
        let fanout = fanout();
        let id = Uuid::parse_str("11730000-0000-4000-8000-00000000a815").unwrap();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 2, 40)
            .expect("register test session");
        let output = format!("{CELL_CANARY}\x1b]0;{OSC_CANARY}\x07");
        feed(&fanout, id, &[output.as_bytes()]);

        let copied = fanout.copy_terminal_screen(id);
        let copied_diagnostic = format!("{copied:?}");
        let captured = match copied {
            crate::pty::backend::TerminalScreenCopyRead::Copied(captured) => captured,
            _ => panic!("expected compact viewport copy"),
        };
        let model = captured
            .into_model(id, SessionBackendKind::LocalProcess)
            .expect("valid owned model");
        let read = crate::pty::backend::TerminalScreenRead::Captured(model);
        let diagnostic = format!(
            "{copied_diagnostic}\n{read:?}\n{:?}\n{:?}",
            crate::pty::backend::TerminalScreenCopyRead::Unavailable,
            crate::pty::backend::TerminalScreenRead::TooLarge,
        );
        let id_text = id.to_string();
        for forbidden in [CELL_CANARY, OSC_CANARY, id_text.as_str()] {
            assert!(!diagnostic.contains(forbidden));
        }
        for structural in [
            "rows: 2",
            "columns: 40",
            "cells: 80",
            "TerminalScreenRead::Captured",
            "TerminalScreenCopyRead::Unavailable",
            "TerminalScreenRead::TooLarge",
        ] {
            assert!(diagnostic.contains(structural));
        }
    }

    #[test]
    fn output_and_capture_race_is_wholly_before_or_after_one_chunk() {
        let fanout = fanout();
        for _ in 0..64 {
            let id = Uuid::new_v4();
            fanout
                .register_session_for_test(id, IdleTuning::DEFAULT, 1, 2)
                .expect("register test session");
            let writer = fanout.clone();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let writer_barrier = Arc::clone(&barrier);
            let thread = std::thread::spawn(move || {
                writer_barrier.wait();
                feed(&writer, id, &[b"A"]);
            });
            barrier.wait();
            let copied = fanout.copy_terminal_screen(id);
            thread.join().unwrap();
            let model = match copied {
                crate::pty::backend::TerminalScreenCopyRead::Copied(copied) => copied
                    .into_model(id, SessionBackendKind::LocalProcess)
                    .unwrap(),
                _ => panic!("expected coherent capture"),
            };
            let first = &model.screen.lines[0].cells[0].text;
            assert!(
                (model.screen.sequence == 0 && first.is_empty())
                    || (model.screen.sequence == 1 && first == "A")
            );
            fanout.remove_session(id);
        }
    }

    #[test]
    fn resize_and_capture_race_returns_one_complete_dimension_set() {
        let fanout = fanout();
        for _ in 0..64 {
            let id = Uuid::new_v4();
            fanout
                .register_session_for_test(id, IdleTuning::DEFAULT, 2, 2)
                .expect("register test session");
            let writer = fanout.clone();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let writer_barrier = Arc::clone(&barrier);
            let thread = std::thread::spawn(move || {
                writer_barrier.wait();
                writer.resize_screen_and_broadcast(id, 4, 3);
            });
            barrier.wait();
            let copied = fanout.copy_terminal_screen(id);
            thread.join().unwrap();
            let model = match copied {
                crate::pty::backend::TerminalScreenCopyRead::Copied(copied) => copied
                    .into_model(id, SessionBackendKind::LocalProcess)
                    .unwrap(),
                _ => panic!("expected coherent capture"),
            };
            let dimensions = model.screen.dimensions;
            assert!(
                (dimensions.rows == 2 && dimensions.columns == 2)
                    || (dimensions.rows == 3 && dimensions.columns == 4)
            );
            assert_eq!(model.screen.lines.len(), usize::from(dimensions.rows));
            assert!(model
                .screen
                .lines
                .iter()
                .all(|line| line.cells.len() == usize::from(dimensions.columns)));
            assert_eq!(model.screen.sequence, 0);
            fanout.remove_session(id);
        }
    }

    #[test]
    fn large_and_partial_osc_state_is_excluded_from_the_viewport_copy() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 1, 4)
            .expect("register test session");
        let sentinel = "snapshot-osc-sentinel".repeat(8_192);
        let first = format!("\x1b]0;{sentinel}");
        feed(&fanout, id, &[first.as_bytes()]);
        let copied = match fanout.copy_terminal_screen(id) {
            crate::pty::backend::TerminalScreenCopyRead::Copied(copied) => copied,
            _ => panic!("partial OSC must not break capture"),
        };
        let model = copied
            .into_model(id, SessionBackendKind::LocalProcess)
            .unwrap();
        assert!(model
            .screen
            .lines
            .iter()
            .flat_map(|line| &line.cells)
            .all(|cell| !cell.text.contains("snapshot-osc-sentinel")));
    }

    #[test]
    fn hostile_one_column_wide_input_removes_only_its_parser() {
        let fanout = fanout();
        let faulty = Uuid::new_v4();
        let healthy = Uuid::new_v4();
        fanout
            .register_session_for_test(faulty, IdleTuning::DEFAULT, 1, 1)
            .expect("register faulty test session");
        fanout
            .register_session_for_test(healthy, IdleTuning::DEFAULT, 2, 4)
            .expect("register healthy test session");

        feed(&fanout, faulty, &["界".as_bytes()]);
        feed(&fanout, healthy, &[b"ok"]);

        assert!(matches!(
            fanout.copy_terminal_screen(faulty),
            crate::pty::backend::TerminalScreenCopyRead::Unavailable
        ));
        assert!(matches!(
            fanout.copy_terminal_screen(healthy),
            crate::pty::backend::TerminalScreenCopyRead::Copied(_)
        ));
    }

    fn activation_data(fanout: &SessionIoFanout, id: Uuid, include_history: bool) -> Vec<u8> {
        fanout
            .activate_terminal_output(id, WINDOW, include_history)
            .expect("attach")
            .expect("snapshot on attach")
            .data
    }

    fn mirrored_screen(fanout: &SessionIoFanout, id: Uuid) -> Vec<u8> {
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        parsers
            .get(&id)
            .expect("registered session")
            .parser
            .screen()
            .contents_formatted()
    }

    /// The ring stays under its limit, keeps the reserved capacity it was built with, and
    /// starts on a line boundary. The capacity assert is the point: `len` is bounded by the
    /// trim under every reserve variant, so without it this test passes just as happily with
    /// a ring that quietly grew to twice its ceiling.
    ///
    /// The oversized chunk covers the one case where the trim arithmetic gets written as a
    /// plain subtraction. That panic would not be local: the caller flips the parser to
    /// `Unavailable`, which leaves the console dead for the rest of the process.
    ///
    /// The line length is load bearing and must stay a non divisor of the limit. With lines
    /// of 128 bytes against a 65 536 byte ceiling the space trim lands on a line boundary by
    /// arithmetic alone, so the front assert passes just as happily with the line-boundary
    /// trim deleted: verified by mutation, both variants passed.
    #[test]
    fn history_ring_is_bounded_and_line_aligned() {
        let fanout = fanout();
        let id = session(&fanout);
        let mut line = [b'-'; 100];
        line[0] = b'>';
        line[99] = b'\n';
        let chunk: Vec<u8> = line.repeat(3);
        for _ in 0..512 {
            feed(&fanout, id, &[&chunk]);
        }
        {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            assert!(state.history.len() <= UI_HISTORY_LIMIT_BYTES);
            assert_eq!(state.history.capacity(), UI_HISTORY_LIMIT_BYTES);
            assert_eq!(state.history.front().copied(), Some(b'>'));
        }

        let oversized = vec![b'x'; UI_HISTORY_LIMIT_BYTES + 4_096];
        feed(&fanout, id, &[&oversized]);
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        assert_eq!(state.history.len(), UI_HISTORY_LIMIT_BYTES);
        assert_eq!(state.history.capacity(), UI_HISTORY_LIMIT_BYTES);
        assert_eq!(state.parser_availability, ParserAvailability::Available);
    }

    /// #1458. A ring saturated by a newline-free stream (a coding agent's spinner rewriting one
    /// line with `\r`) leaves the ring's front at an arbitrary byte offset, which lands inside an
    /// escape sequence most of the time. The seed must never emit that sequence's literal tail;
    /// with no `\n` anywhere in the ring there is nothing to realign to, so the attach takes the
    /// parser mirror.
    #[test]
    fn a_newline_free_ring_seeds_the_mirror_instead_of_a_partial_sequence() {
        let fanout = fanout();
        let id = session(&fanout);
        // A realistic spinner frame: truecolor SGR, label, carriage return. No `\n`. 33 bytes, so
        // the steady-state front lands 2 bytes into the SGR, exactly where the incident cut.
        let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r";
        for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
            feed(&fanout, id, &[frame]);
        }
        {
            // The precondition of the defect, asserted rather than assumed.
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            assert_eq!(state.history.len(), UI_HISTORY_LIMIT_BYTES);
            assert!(!state.history_aligned);
        }

        let expected = mirrored_screen(&fanout, id);
        let data = activation_data(&fanout, id, true);

        assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
        assert_eq!(data, expected);
    }

    /// #1458. The healthy path must stay byte identical: when the capped trim did realign the
    /// ring, the seed is the ring verbatim, from its very first byte. Asserting the whole body
    /// against the ring is the point. An alignment scan applied unconditionally would silently
    /// drop the ring's first line here, and a `starts_with` on the line's prefix would still pass,
    /// because every line of such a replay begins with the same SGR bytes.
    #[test]
    fn a_line_aligned_ring_still_seeds_the_whole_ring() {
        let fanout = fanout();
        let id = session(&fanout);
        // 102 bytes per line, not a divisor of the 65 536 byte ring, so the space trim lands off a
        // line boundary and the realignment has to do real work: 50 bytes drained for space and 52
        // more to realign, on every overflow.
        for index in 0..2_000 {
            feed(
                &fanout,
                id,
                &[format!("\x1b[38;2;153;153;153m>{index:081}\n").as_bytes()],
            );
        }
        let expected = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            assert!(state.history_aligned);
            let (front, back) = state.history.as_slices();
            [front, back].concat()
        };

        let data = activation_data(&fanout, id, true);

        assert!(data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
        assert_eq!(
            &data[UI_HISTORY_REPLAY_PROLOGUE.len()..],
            expected.as_slice()
        );
    }

    /// #1458 edge case. A ring whose only `\n` is its last byte does have a line start, but has
    /// nothing after it: aligning to it would seed the prologue and zero bytes of content, which
    /// blanks the attaching terminal. That case must take the mirror, exactly like a ring with no
    /// `\n` at all.
    #[test]
    fn a_ring_whose_only_newline_is_its_last_byte_seeds_the_mirror() {
        let fanout = fanout();
        let id = session(&fanout);
        let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r";
        for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
            feed(&fanout, id, &[frame]);
        }
        // One frame that ends in the ring's only newline. The capped trim scan still sees no `\n`
        // in the first 4 KiB, so the ring stays flagged unaligned.
        feed(&fanout, id, &[b"\x1b[38;2;153;153;153m* Drizzling..\n"]);
        {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            assert!(!state.history_aligned);
            assert_eq!(state.history.back().copied(), Some(b'\n'));
            assert_eq!(
                state.history.iter().filter(|byte| **byte == b'\n').count(),
                1
            );
        }

        let expected = mirrored_screen(&fanout, id);
        let data = activation_data(&fanout, id, true);

        assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
        assert_eq!(data, expected);
    }

    /// #1458. The recovering case, and the only one that exercises `history_from_first_line`'s
    /// `Some` arm: an unaligned ring that still holds lines must seed from the byte after its
    /// first `\n`, not fall back to the mirror. Asserting the whole body is the point; a stub
    /// helper that always returns `None` passes every other test in this file.
    #[test]
    fn an_unaligned_ring_with_a_later_newline_seeds_from_that_line() {
        let fanout = fanout();
        let id = session(&fanout);
        let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r"; // 33 bytes, no `\n`
        for _ in 0..(UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
            feed(&fanout, id, &[frame]);
        }
        for index in 0..40 {
            feed(
                &fanout,
                id,
                &[format!("\x1b[38;2;153;153;153mrecovered line {index:03}\n").as_bytes()],
            );
        }
        let expected = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            // The 4 KiB hot scan still sees only spinner at the front, so the flag stays false
            // even though the ring now holds 40 newlines further in.
            assert!(!state.history_aligned);
            let (front, back) = state.history.as_slices();
            let ring = [front, back].concat();
            let newline = ring
                .iter()
                .position(|byte| *byte == b'\n')
                .expect("a newline");
            ring[newline + 1..].to_vec()
        };

        let data = activation_data(&fanout, id, true);

        assert!(data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
        assert_eq!(
            &data[UI_HISTORY_REPLAY_PROLOGUE.len()..],
            expected.as_slice()
        );
    }

    /// #1458. The helper's four decided branches, pinned without a fixture because which half of
    /// the ring holds the first `\n` is a `VecDeque` layout detail no fanout test can choose.
    /// Covers 7.2 rows 8, 9 and 10 and the both-halves emptiness check.
    #[test]
    fn history_from_first_line_decides_every_branch() {
        // A newline in `front`, content behind it: seed from the byte after it, keep all of `back`.
        assert_eq!(
            history_from_first_line(b"ab\ncd", b""),
            Some((&b"cd"[..], &b""[..]))
        );
        // The newline is `front`'s last byte but `back` is not empty: still a normal seed. The
        // emptiness check is on BOTH halves for exactly this row.
        assert_eq!(
            history_from_first_line(b"ab\n", b"cd"),
            Some((&b""[..], &b"cd"[..]))
        );
        // No newline in `front`, one in `back` with content behind it: the whole of `front` is
        // unreplayable and is dropped. Deleting the second scan makes this row dead.
        assert_eq!(
            history_from_first_line(b"ab", b"cd\nef"),
            Some((&b""[..], &b"ef"[..]))
        );
        // The ring's only newline is its last byte: nothing survives it, so the caller must take
        // the mirror rather than seed a prologue and zero bytes.
        assert_eq!(history_from_first_line(b"ab", b"cd\n"), None);
        // No newline anywhere: the reported incident.
        assert_eq!(history_from_first_line(b"ab", b"cd"), None);
    }

    /// The regression the issue reports: a session that produced output while another one was
    /// selected must come back with history, not with a single viewport.
    #[test]
    fn activation_payload_replays_history_for_background_session() {
        let fanout = fanout();
        let foreground = session(&fanout);
        let background = session(&fanout);
        activation_data(&fanout, foreground, false);

        for index in 0..200 {
            let line = format!("history line {index}\r\n");
            feed(&fanout, background, &[line.as_bytes()]);
        }

        // The first line scrolled out of the 30 row mirror, so it is only reachable through
        // the ring. Without this the payload assert could pass on the viewport alone.
        let mirror = String::from_utf8_lossy(&mirrored_screen(&fanout, background)).into_owned();
        assert!(!mirror.contains("history line 0\r\n"));

        let data = activation_data(&fanout, background, true);
        // Asserting the prologue keeps a silent fallback to `contents_formatted()` from
        // passing this test for the wrong reason.
        assert!(data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
        let replayed = String::from_utf8_lossy(&data);
        assert!(replayed.contains("history line 0\r\n"));
        assert!(replayed.contains("history line 199"));
    }

    #[test]
    fn activation_payload_falls_back_to_screen_when_history_empty() {
        let fanout = fanout();
        let id = session(&fanout);
        let expected = mirrored_screen(&fanout, id);

        let data = activation_data(&fanout, id, true);
        assert_eq!(data, expected);
        assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    }

    /// Pins the parameter's contract: `false` is the mirrored viewport, never the ring. The
    /// frontend asks for the ring on every attach and applies it after a reset, which is what
    /// makes the replay safe there, but the viewport-only read stays addressable.
    #[test]
    fn activation_payload_ignores_history_when_not_requested() {
        let fanout = fanout();
        let id = session(&fanout);
        for index in 0..200 {
            let line = format!("history line {index}\r\n");
            feed(&fanout, id, &[line.as_bytes()]);
        }
        let expected = mirrored_screen(&fanout, id);

        let data = activation_data(&fanout, id, false);
        assert_eq!(data, expected);
        assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    }
}
