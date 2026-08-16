use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
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
    coordinator: Arc<TerminalOutputCoordinator>,
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

const UI_BATCH_INTERVAL_MS: u64 = 16;
const UI_PENDING_LIMIT_BYTES: usize = 131_072;
const UI_DELIVERY_ACK_TIMEOUT_MS: u64 = 5_000;
const UI_ACTIVATION_READY_TIMEOUT_MS: u64 = 5_000;
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
/// trim to a line boundary, then append, so the ring stays line aligned and its length
/// never exceeds the limit.
///
/// Every index is saturating on purpose. `VecDeque::drain(..k)` panics when `k > len`, and
/// here a panic is permanent rather than local: the caller flips the parser to `Unavailable`,
/// which leaves that console dead for the rest of the process.
fn append_history(history: &mut std::collections::VecDeque<u8>, data: &[u8]) {
    // A chunk larger than the whole ring keeps only its tail. Unreachable in production
    // (the local backend reads 4 KiB buffers, the container backend rejects frames over
    // 64 KiB) but it is where the trim arithmetic gets written wrong.
    let tail = &data[data.len().saturating_sub(UI_HISTORY_LIMIT_BYTES)..];
    let over = (history.len() + tail.len()).saturating_sub(UI_HISTORY_LIMIT_BYTES);
    if over > 0 {
        history.drain(..over.min(history.len()));
        if let Some(newline) = history
            .iter()
            .take(UI_HISTORY_LINE_SCAN_BYTES)
            .position(|byte| *byte == b'\n')
        {
            history.drain(..=newline);
        }
    }
    history.extend(tail);
}

/// The private result surface of the Tauri output effect. It deliberately carries no
/// underlying Tauri error because output bytes and event errors are both sensitive at this
/// boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PtyOutputEmitError {
    Emit,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind")]
enum TerminalOutputDelivery {
    #[serde(rename = "data")]
    Data {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "generation")]
        generation: String,
        #[serde(rename = "firstSequence")]
        first_sequence: String,
        #[serde(rename = "sequence")]
        sequence: String,
        #[serde(rename = "data")]
        data: Vec<u8>,
    },
    #[serde(rename = "resyncRequired")]
    ResyncRequired {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "generation")]
        generation: String,
        #[serde(rename = "sequence")]
        sequence: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
struct TerminalOutputActivationSnapshot {
    #[serde(rename = "data")]
    data: Vec<u8>,
    #[serde(rename = "rows")]
    rows: u16,
    #[serde(rename = "cols")]
    cols: u16,
    #[serde(rename = "sequence")]
    sequence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalOutputActivation {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "generation")]
    generation: String,
    #[serde(rename = "snapshot")]
    snapshot: TerminalOutputActivationSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) enum TerminalOutputActivationRecoveryCode {
    #[serde(rename = "parserUnavailable")]
    ParserUnavailable,
    #[serde(rename = "snapshotTooLarge")]
    SnapshotTooLarge,
    #[serde(rename = "snapshotMalformed")]
    SnapshotMalformed,
    #[serde(rename = "counterExhausted")]
    CounterExhausted,
    #[serde(rename = "outputTargetUnavailable")]
    OutputTargetUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind")]
pub(crate) enum TerminalOutputActivationResult {
    #[serde(rename = "activated")]
    Activated {
        #[serde(rename = "activation")]
        activation: TerminalOutputActivation,
    },
    #[serde(rename = "recoveryError")]
    RecoveryError {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "code")]
        code: TerminalOutputActivationRecoveryCode,
    },
}

impl TerminalOutputActivationResult {
    pub(crate) fn recovery(session_id: Uuid, code: TerminalOutputActivationRecoveryCode) -> Self {
        Self::RecoveryError {
            session_id: session_id.to_string(),
            code,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind")]
pub(crate) enum TerminalOutputControlState {
    #[serde(rename = "active")]
    Active {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "generation")]
        generation: String,
    },
    #[serde(rename = "awaitingFrontendReady")]
    AwaitingFrontendReady {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "generation")]
        generation: String,
        #[serde(rename = "snapshotSequence")]
        snapshot_sequence: String,
    },
    #[serde(rename = "resyncRequired")]
    ResyncRequired {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "generation")]
        generation: String,
        #[serde(rename = "sequence")]
        sequence: String,
    },
    #[serde(rename = "recoveryError")]
    RecoveryError {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "generation")]
        generation: String,
        #[serde(rename = "code")]
        code: &'static str,
    },
    #[serde(rename = "inactive")]
    Inactive {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "generation")]
        generation: String,
    },
    #[serde(rename = "stale")]
    Stale,
}

impl TerminalOutputControlState {
    pub(crate) fn stale() -> Self {
        Self::Stale
    }
}

/// A permissive Tauri boundary. Validation is intentionally delayed until the command has a
/// fully materialized JSON value, before it parses a session id or touches PtyManager.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct TerminalRendererMetricsWire(serde_json::Value);

/// Opaque, validated metrics. Its fields intentionally remain private to this module.
#[derive(Clone, Debug)]
pub(crate) struct TerminalRendererMetrics {
    values: [u32; 25],
}

impl TerminalRendererMetrics {
    fn values(&self) -> &[u32; 25] {
        &self.values
    }
}

impl TryFrom<TerminalRendererMetricsWire> for TerminalRendererMetrics {
    type Error = ();

    fn try_from(wire: TerminalRendererMetricsWire) -> Result<Self, Self::Error> {
        const KEYS: [&str; 25] = [
            "retainedTerminalCount",
            "visibleTerminalCount",
            "webglContextCount",
            "webglContextLossCount",
            "lruEvictionCount",
            "outputEventsReceived",
            "inactiveOrStaleEventsRejected",
            "bytesAccepted",
            "bytesWritten",
            "replayPendingBytes",
            "livePendingBytes",
            "writeInFlightBytes",
            "combinedAdmissionHighWaterBytes",
            "pendingHighWaterBytes",
            "resyncCount",
            "activationReadyAcknowledgements",
            "activationReadyRejections",
            "activationReadyTimeouts",
            "generationHealthPollsScheduled",
            "generationHealthPollsStarted",
            "generationHealthPollsCancelled",
            "replayPendingLivenessRecoveries",
            "snapshotReplayDurationMs",
            "retiredWriteCallbacksIgnoredAfterDisposal",
            "maxAnimationFrameLagMs",
        ];

        let object = wire.0.as_object().ok_or(())?;
        if object.len() != KEYS.len()
            || object.keys().map(String::as_str).collect::<HashSet<_>>()
                != KEYS.iter().copied().collect::<HashSet<_>>()
        {
            return Err(());
        }

        let mut values = [0_u32; 25];
        for (index, key) in KEYS.iter().enumerate() {
            let value = object
                .get(*key)
                .and_then(serde_json::Value::as_u64)
                .ok_or(())?;
            values[index] = u32::try_from(value).map_err(|_| ())?;
        }

        let bounded = |index: usize, maximum: u32| values[index] <= maximum;
        if !bounded(0, 4)
            || !bounded(1, 1)
            || !bounded(2, 4)
            || !bounded(9, UI_PENDING_LIMIT_BYTES as u32)
            || !bounded(10, UI_PENDING_LIMIT_BYTES as u32)
            || !bounded(11, UI_PENDING_LIMIT_BYTES as u32)
            || !bounded(12, UI_PENDING_LIMIT_BYTES as u32)
            || !bounded(13, UI_PENDING_LIMIT_BYTES as u32)
            || !bounded(22, 60_000)
            || !bounded(24, 60_000)
        {
            return Err(());
        }
        if values[1] > values[0]
            || values[2] > values[0]
            || (values[9] > 0 && (values[10] != 0 || values[11] != 0))
        {
            return Err(());
        }
        let live_and_in_flight = values[10].checked_add(values[11]).ok_or(())?;
        if live_and_in_flight > UI_PENDING_LIMIT_BYTES as u32
            || values[12] < values[9]
            || values[12] < live_and_in_flight
            || values[13] < values[9]
            || values[13] < values[10]
            || values[13] > values[12]
        {
            return Err(());
        }

        Ok(Self { values })
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

#[derive(Clone)]
pub(crate) struct PtyOutputTarget {
    emit_pty_output:
        Arc<dyn Fn(TerminalOutputDelivery) -> Result<(), PtyOutputEmitError> + Send + Sync>,
}

#[cfg(test)]
pub(crate) type PtyOutputTestEvent = (String, Vec<u8>, Option<u64>);

#[cfg(test)]
pub(crate) type PtyOutputTestSink = Arc<Mutex<Vec<PtyOutputTestEvent>>>;

impl PtyOutputTarget {
    pub(crate) fn from_app_handle<R: tauri::Runtime>(app_handle: AppHandle<R>) -> Self {
        Self {
            emit_pty_output: Arc::new(move |payload| {
                app_handle
                    .emit("pty_output", payload)
                    .map_err(|_| PtyOutputEmitError::Emit)
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn noop() -> Self {
        Self {
            emit_pty_output: Arc::new(|_| Ok(())),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_sink(sink: PtyOutputTestSink) -> Self {
        Self {
            emit_pty_output: Arc::new(move |payload| {
                let event = match payload {
                    TerminalOutputDelivery::Data {
                        session_id,
                        data,
                        sequence,
                        ..
                    } => (session_id, data, sequence.parse().ok()),
                    TerminalOutputDelivery::ResyncRequired {
                        session_id,
                        sequence,
                        ..
                    } => (session_id, Vec::new(), sequence.parse().ok()),
                };
                sink.lock().unwrap().push(event);
                Ok(())
            }),
        }
    }

    #[cfg(test)]
    fn failing_test_sink(sink: PtyOutputTestSink) -> Self {
        let target = Self::from_test_sink(sink);
        Self {
            emit_pty_output: Arc::new(move |payload| {
                let _ = target.emit_pty_output(payload);
                Err(PtyOutputEmitError::Emit)
            }),
        }
    }

    fn emit_pty_output(&self, payload: TerminalOutputDelivery) -> Result<(), PtyOutputEmitError> {
        (self.emit_pty_output)(payload)
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

#[derive(Default)]
struct PendingBatch {
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    data: Vec<u8>,
}

impl PendingBatch {
    fn append(
        &mut self,
        sequence: u64,
        mut data: Vec<u8>,
        in_flight_bytes: usize,
    ) -> Result<(), ()> {
        let used = in_flight_bytes
            .checked_add(self.data.len())
            .and_then(|value| value.checked_add(data.len()))
            .ok_or(())?;
        if used > UI_PENDING_LIMIT_BYTES {
            return Err(());
        }
        if let Some(last_sequence) = self.last_sequence {
            if last_sequence.checked_add(1) != Some(sequence) {
                return Err(());
            }
        } else {
            self.first_sequence = Some(sequence);
        }
        self.last_sequence = Some(sequence);
        self.data.append(&mut data);
        Ok(())
    }

    fn take(&mut self) -> Self {
        std::mem::take(self)
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

struct DeliveryCredit {
    first_sequence: u64,
    sequence: u64,
    bytes: usize,
    delivery_token: u64,
    deadline_token: u64,
}

enum DeliveryPhase {
    AwaitingFrontendReady {
        pending: PendingBatch,
        ready_deadline_token: u64,
    },
    Active {
        pending: PendingBatch,
        scheduled_flush_token: Option<u64>,
        in_flight: Option<DeliveryCredit>,
    },
    ResyncRequired {
        sequence: u64,
        activation_ready: bool,
        marker_reserved: bool,
    },
}

struct SelectedDeliveryRecord {
    registration: Arc<RegisteredPtyOutputTarget>,
    generation: u64,
    snapshot_sequence: u64,
    counter_exhausted: bool,
    phase: DeliveryPhase,
    effect_gate: Arc<GenerationEffectGate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputEffectKind {
    Data,
    Marker,
}

struct PendingEffect {
    target: Arc<RegisteredPtyOutputTarget>,
    delivery: TerminalOutputDelivery,
    kind: OutputEffectKind,
}

enum EffectPermit {
    Pending(PendingEffect),
    Executing,
}

struct GenerationEffectGateState {
    open: bool,
    next_permit: u64,
    executing: usize,
    permits: HashMap<u64, EffectPermit>,
}

struct GenerationEffectGate {
    state: Mutex<GenerationEffectGateState>,
    settled: Condvar,
}

impl GenerationEffectGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(GenerationEffectGateState {
                open: true,
                next_permit: 0,
                executing: 0,
                permits: HashMap::new(),
            }),
            settled: Condvar::new(),
        })
    }

    fn reserve(&self, effect: PendingEffect) -> Option<u64> {
        let mut state = self.state.lock().ok()?;
        if !state.open {
            return None;
        }
        let permit = state.next_permit.checked_add(1)?;
        state.next_permit = permit;
        state.permits.insert(permit, EffectPermit::Pending(effect));
        Some(permit)
    }

    fn begin(&self, permit: u64) -> Option<PendingEffect> {
        let mut state = self.state.lock().ok()?;
        if !state.open {
            return None;
        }
        let effect = match state.permits.remove(&permit)? {
            EffectPermit::Pending(effect) => effect,
            EffectPermit::Executing => return None,
        };
        state.executing = state.executing.checked_add(1)?;
        state.permits.insert(permit, EffectPermit::Executing);
        Some(effect)
    }

    fn finish(&self, permit: u64) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if matches!(state.permits.remove(&permit), Some(EffectPermit::Executing)) {
            state.executing = state.executing.saturating_sub(1);
        }
        if state.executing == 0 {
            self.settled.notify_all();
        }
    }

    fn cancel_pending(&self, permit: u64) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if matches!(state.permits.get(&permit), Some(EffectPermit::Pending(_))) {
            state.permits.remove(&permit);
        }
        if state.executing == 0 {
            self.settled.notify_all();
        }
    }

    fn close_and_wait(&self) {
        let cancelled = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.open = false;
            let pending_ids: Vec<u64> = state
                .permits
                .iter()
                .filter_map(|(id, permit)| {
                    matches!(permit, EffectPermit::Pending(_)).then_some(*id)
                })
                .collect();
            let mut cancelled = Vec::new();
            for id in pending_ids {
                if let Some(EffectPermit::Pending(effect)) = state.permits.remove(&id) {
                    cancelled.push(effect);
                }
            }
            cancelled
        };
        // Drop every pending payload and target before the lifecycle barrier waits.
        drop(cancelled);

        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while state.executing != 0 {
            state = self
                .settled
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

struct OutputEffectDescriptor {
    coordinator: Weak<TerminalOutputCoordinator>,
    gate: Weak<GenerationEffectGate>,
    registration_identity: Arc<RegistrationIdentity>,
    generation: u64,
    permit: u64,
}

impl OutputEffectDescriptor {
    fn run(self) {
        let Some(gate) = self.gate.upgrade() else {
            return;
        };
        let Some(effect) = gate.begin(self.permit) else {
            return;
        };
        let PendingEffect {
            target,
            delivery,
            kind,
        } = effect;
        let result = target.target.emit_pty_output(delivery);
        let next = self.coordinator.upgrade().and_then(|coordinator| {
            coordinator.consume_effect_result(
                &self.registration_identity,
                self.generation,
                self.permit,
                kind,
                result,
            )
        });
        gate.finish(self.permit);
        if let Some(next) = next {
            next.run();
        }
    }
}

impl Drop for OutputEffectDescriptor {
    fn drop(&mut self) {
        if let Some(gate) = self.gate.upgrade() {
            gate.cancel_pending(self.permit);
        }
    }
}

struct TerminalDeliveryState {
    next_generation: u64,
    selected: Option<SelectedDeliveryRecord>,
    renderer_metric_reports: u64,
    ui_emit_failures: u64,
    ui_marker_emit_failures: u64,
}

pub(crate) struct TerminalOutputCoordinator {
    state: Mutex<TerminalDeliveryState>,
    activation_gate: Mutex<()>,
    timer_sequence: AtomicU64,
}

impl TerminalOutputCoordinator {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TerminalDeliveryState {
                next_generation: 0,
                selected: None,
                renderer_metric_reports: 0,
                ui_emit_failures: 0,
                ui_marker_emit_failures: 0,
            }),
            activation_gate: Mutex::new(()),
            timer_sequence: AtomicU64::new(0),
        })
    }

    fn next_timer_token(&self) -> Option<u64> {
        self.timer_sequence
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_add(1)
            })
            .ok()
            .map(|current| current + 1)
    }

    fn matches_record(
        record: &SelectedDeliveryRecord,
        identity: &Arc<RegistrationIdentity>,
        generation: u64,
    ) -> bool {
        record.generation == generation && Arc::ptr_eq(&record.registration.identity, identity)
    }

    fn control_state(record: &SelectedDeliveryRecord) -> TerminalOutputControlState {
        let session_id = record.registration.session_id.to_string();
        let generation = record.generation.to_string();
        if record.counter_exhausted {
            return TerminalOutputControlState::RecoveryError {
                session_id,
                generation,
                code: "counterExhausted",
            };
        }
        match &record.phase {
            DeliveryPhase::AwaitingFrontendReady { .. } => {
                TerminalOutputControlState::AwaitingFrontendReady {
                    session_id,
                    generation,
                    snapshot_sequence: record.snapshot_sequence.to_string(),
                }
            }
            DeliveryPhase::Active { .. } => TerminalOutputControlState::Active {
                session_id,
                generation,
            },
            DeliveryPhase::ResyncRequired { sequence, .. } => {
                TerminalOutputControlState::ResyncRequired {
                    session_id,
                    generation,
                    sequence: sequence.to_string(),
                }
            }
        }
    }

    fn retire_after_counter_exhaustion(record: &mut SelectedDeliveryRecord) {
        record.counter_exhausted = true;
        record.phase = DeliveryPhase::ResyncRequired {
            sequence: record.snapshot_sequence,
            activation_ready: false,
            marker_reserved: true,
        };
    }

    fn reserve_effect(
        self: &Arc<Self>,
        record: &SelectedDeliveryRecord,
        delivery: TerminalOutputDelivery,
        kind: OutputEffectKind,
    ) -> Option<OutputEffectDescriptor> {
        let permit = record.effect_gate.reserve(PendingEffect {
            target: Arc::clone(&record.registration),
            delivery,
            kind,
        })?;
        Some(OutputEffectDescriptor {
            coordinator: Arc::downgrade(self),
            gate: Arc::downgrade(&record.effect_gate),
            registration_identity: Arc::clone(&record.registration.identity),
            generation: record.generation,
            permit,
        })
    }

    fn reserve_marker(
        self: &Arc<Self>,
        record: &mut SelectedDeliveryRecord,
    ) -> Option<OutputEffectDescriptor> {
        let (sequence, activation_ready, marker_reserved) = match &record.phase {
            DeliveryPhase::ResyncRequired {
                sequence,
                activation_ready,
                marker_reserved,
            } => (*sequence, *activation_ready, *marker_reserved),
            _ => return None,
        };
        if !activation_ready || marker_reserved {
            return None;
        }
        let delivery = TerminalOutputDelivery::ResyncRequired {
            session_id: record.registration.session_id.to_string(),
            generation: record.generation.to_string(),
            sequence: sequence.to_string(),
        };
        let descriptor = self.reserve_effect(record, delivery, OutputEffectKind::Marker);
        if descriptor.is_some() {
            if let DeliveryPhase::ResyncRequired {
                marker_reserved, ..
            } = &mut record.phase
            {
                *marker_reserved = true;
            }
        }
        descriptor
    }

    fn transition_to_resync(
        self: &Arc<Self>,
        record: &mut SelectedDeliveryRecord,
        sequence: u64,
        activation_ready: bool,
    ) -> Option<OutputEffectDescriptor> {
        if matches!(record.phase, DeliveryPhase::ResyncRequired { .. }) {
            return None;
        }
        record.phase = DeliveryPhase::ResyncRequired {
            sequence,
            activation_ready,
            marker_reserved: false,
        };
        self.reserve_marker(record)
    }

    fn schedule_ready_timeout(
        self: &Arc<Self>,
        identity: Arc<RegistrationIdentity>,
        generation: u64,
        timer_token: u64,
    ) {
        let coordinator = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(UI_ACTIVATION_READY_TIMEOUT_MS)).await;
            if let Some(coordinator) = coordinator.upgrade() {
                if let Some(descriptor) =
                    coordinator.handle_ready_timeout(&identity, generation, timer_token)
                {
                    descriptor.run();
                }
            }
        });
    }

    fn schedule_flush(
        self: &Arc<Self>,
        identity: Arc<RegistrationIdentity>,
        generation: u64,
        timer_token: u64,
    ) {
        let coordinator = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(UI_BATCH_INTERVAL_MS)).await;
            if let Some(coordinator) = coordinator.upgrade() {
                if let Some(descriptor) = coordinator.flush(&identity, generation, timer_token) {
                    descriptor.run();
                }
            }
        });
    }

    fn schedule_ack_timeout(
        self: &Arc<Self>,
        identity: Arc<RegistrationIdentity>,
        generation: u64,
        delivery_token: u64,
        deadline_token: u64,
    ) {
        let coordinator = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(UI_DELIVERY_ACK_TIMEOUT_MS)).await;
            if let Some(coordinator) = coordinator.upgrade() {
                if let Some(descriptor) = coordinator.handle_ack_timeout(
                    &identity,
                    generation,
                    delivery_token,
                    deadline_token,
                ) {
                    descriptor.run();
                }
            }
        });
    }

    fn activate(
        self: &Arc<Self>,
        registration: Arc<RegisteredPtyOutputTarget>,
        snapshot: PtyScreenSnapshot,
    ) -> TerminalOutputActivationResult {
        let _activation_guard = self
            .activation_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let retired = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.selected.take()
        };
        if let Some(retired) = retired {
            retired.effect_gate.close_and_wait();
        }

        let (generation, timer_token) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let Some(generation) = state.next_generation.checked_add(1) else {
                return TerminalOutputActivationResult::recovery(
                    registration.session_id,
                    TerminalOutputActivationRecoveryCode::CounterExhausted,
                );
            };
            let Some(timer_token) = self.next_timer_token() else {
                return TerminalOutputActivationResult::recovery(
                    registration.session_id,
                    TerminalOutputActivationRecoveryCode::CounterExhausted,
                );
            };
            state.next_generation = generation;
            state.selected = Some(SelectedDeliveryRecord {
                registration: Arc::clone(&registration),
                generation,
                snapshot_sequence: snapshot.sequence,
                counter_exhausted: false,
                phase: DeliveryPhase::AwaitingFrontendReady {
                    pending: PendingBatch::default(),
                    ready_deadline_token: timer_token,
                },
                effect_gate: GenerationEffectGate::new(),
            });
            (generation, timer_token)
        };
        self.schedule_ready_timeout(Arc::clone(&registration.identity), generation, timer_token);
        TerminalOutputActivationResult::Activated {
            activation: TerminalOutputActivation {
                session_id: registration.session_id.to_string(),
                generation: generation.to_string(),
                snapshot: TerminalOutputActivationSnapshot {
                    data: snapshot.data,
                    rows: snapshot.rows,
                    cols: snapshot.cols,
                    sequence: snapshot.sequence.to_string(),
                },
            },
        }
    }

    fn admit_output(
        self: &Arc<Self>,
        registration: &Arc<RegisteredPtyOutputTarget>,
        sequence: u64,
        data: Vec<u8>,
    ) -> Option<OutputEffectDescriptor> {
        let mut flush_schedule = None;
        let descriptor = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let record = state.selected.as_mut()?;
            if !Arc::ptr_eq(&record.registration.identity, &registration.identity) {
                return None;
            }
            match &mut record.phase {
                DeliveryPhase::AwaitingFrontendReady { pending, .. } => {
                    if pending.append(sequence, data, 0).is_err() {
                        self.transition_to_resync(record, sequence, false)
                    } else {
                        None
                    }
                }
                DeliveryPhase::Active {
                    pending,
                    scheduled_flush_token,
                    in_flight,
                } => {
                    let in_flight_bytes =
                        in_flight.as_ref().map(|credit| credit.bytes).unwrap_or(0);
                    if pending.append(sequence, data, in_flight_bytes).is_err() {
                        self.transition_to_resync(record, sequence, true)
                    } else if in_flight.is_none() && scheduled_flush_token.is_none() {
                        if let Some(timer_token) = self.next_timer_token() {
                            *scheduled_flush_token = Some(timer_token);
                            flush_schedule = Some((
                                Arc::clone(&record.registration.identity),
                                record.generation,
                                timer_token,
                            ));
                        } else {
                            Self::retire_after_counter_exhaustion(record);
                        }
                        None
                    } else {
                        None
                    }
                }
                DeliveryPhase::ResyncRequired { .. } => None,
            }
        };
        if let Some((identity, generation, timer_token)) = flush_schedule {
            self.schedule_flush(identity, generation, timer_token);
        }
        descriptor
    }

    fn flush(
        self: &Arc<Self>,
        identity: &Arc<RegistrationIdentity>,
        generation: u64,
        timer_token: u64,
    ) -> Option<OutputEffectDescriptor> {
        let (descriptor, (identity, generation, delivery_token, deadline_token)) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let record = state.selected.as_mut()?;
            if !Self::matches_record(record, identity, generation) {
                return None;
            }
            let (batch, first_sequence, sequence) = {
                let DeliveryPhase::Active {
                    pending,
                    scheduled_flush_token,
                    in_flight,
                } = &mut record.phase
                else {
                    return None;
                };
                if *scheduled_flush_token != Some(timer_token)
                    || in_flight.is_some()
                    || pending.is_empty()
                {
                    return None;
                }
                *scheduled_flush_token = None;
                let batch = pending.take();
                let first_sequence = batch.first_sequence?;
                let sequence = batch.last_sequence?;
                (batch, first_sequence, sequence)
            };
            let batch_bytes = batch.data.len();
            let Some(deadline_token) = self.next_timer_token() else {
                Self::retire_after_counter_exhaustion(record);
                return None;
            };
            let delivery = TerminalOutputDelivery::Data {
                session_id: record.registration.session_id.to_string(),
                generation: record.generation.to_string(),
                first_sequence: first_sequence.to_string(),
                sequence: sequence.to_string(),
                data: batch.data,
            };
            let descriptor = self.reserve_effect(record, delivery, OutputEffectKind::Data)?;
            if let DeliveryPhase::Active { in_flight, .. } = &mut record.phase {
                *in_flight = Some(DeliveryCredit {
                    first_sequence,
                    sequence,
                    bytes: batch_bytes,
                    delivery_token: descriptor.permit,
                    deadline_token,
                });
            } else {
                return None;
            }
            let delivery_token = descriptor.permit;
            (
                descriptor,
                (
                    Arc::clone(identity),
                    generation,
                    delivery_token,
                    deadline_token,
                ),
            )
        };
        self.schedule_ack_timeout(identity, generation, delivery_token, deadline_token);
        Some(descriptor)
    }

    fn handle_ready_timeout(
        self: &Arc<Self>,
        identity: &Arc<RegistrationIdentity>,
        generation: u64,
        timer_token: u64,
    ) -> Option<OutputEffectDescriptor> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let record = state.selected.as_mut()?;
        if !Self::matches_record(record, identity, generation) {
            return None;
        }
        let DeliveryPhase::AwaitingFrontendReady {
            ready_deadline_token,
            ..
        } = &record.phase
        else {
            return None;
        };
        if *ready_deadline_token != timer_token {
            return None;
        }
        // A ready timeout is deliberately unready: its one marker stays deferred until an
        // exact later readiness acknowledgement reaches this same generation.
        self.transition_to_resync(record, record.snapshot_sequence, false)
    }

    fn handle_ack_timeout(
        self: &Arc<Self>,
        identity: &Arc<RegistrationIdentity>,
        generation: u64,
        delivery_token: u64,
        deadline_token: u64,
    ) -> Option<OutputEffectDescriptor> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let record = state.selected.as_mut()?;
        if !Self::matches_record(record, identity, generation) {
            return None;
        }
        let anchor = match &record.phase {
            DeliveryPhase::Active {
                in_flight: Some(credit),
                ..
            } if credit.delivery_token == delivery_token
                && credit.deadline_token == deadline_token =>
            {
                credit.sequence
            }
            _ => return None,
        };
        self.transition_to_resync(record, anchor, true)
    }

    fn consume_effect_result(
        self: &Arc<Self>,
        identity: &Arc<RegistrationIdentity>,
        generation: u64,
        permit: u64,
        kind: OutputEffectKind,
        result: Result<(), PtyOutputEmitError>,
    ) -> Option<OutputEffectDescriptor> {
        let Err(PtyOutputEmitError::Emit) = result else {
            return None;
        };
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match kind {
            OutputEffectKind::Data => {
                let anchor = {
                    let record = state.selected.as_ref()?;
                    if !Self::matches_record(record, identity, generation) {
                        return None;
                    }
                    match &record.phase {
                        DeliveryPhase::Active {
                            in_flight: Some(credit),
                            ..
                        } if credit.delivery_token == permit => credit.sequence,
                        _ => return None,
                    }
                };
                state.ui_emit_failures = state.ui_emit_failures.saturating_add(1);
                let record = state.selected.as_mut()?;
                self.transition_to_resync(record, anchor, true)
            }
            OutputEffectKind::Marker => {
                let resync = {
                    let record = state.selected.as_ref()?;
                    if !Self::matches_record(record, identity, generation) {
                        return None;
                    }
                    matches!(record.phase, DeliveryPhase::ResyncRequired { .. })
                };
                if resync {
                    state.ui_marker_emit_failures = state.ui_marker_emit_failures.saturating_add(1);
                    log::warn!(
                        "[terminal-output] resync marker emit failed generation={generation}"
                    );
                }
                None
            }
        }
    }

    fn ready(
        self: &Arc<Self>,
        identity: &Arc<RegistrationIdentity>,
        generation: u64,
        snapshot_sequence: u64,
    ) -> (TerminalOutputControlState, Option<OutputEffectDescriptor>) {
        let mut flush_schedule = None;
        let (result, descriptor) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let Some(record) = state.selected.as_mut() else {
                return (TerminalOutputControlState::stale(), None);
            };
            if !Self::matches_record(record, identity, generation)
                || record.snapshot_sequence != snapshot_sequence
            {
                return (TerminalOutputControlState::stale(), None);
            }
            match &mut record.phase {
                DeliveryPhase::AwaitingFrontendReady { pending, .. } => {
                    let pending = pending.take();
                    record.phase = DeliveryPhase::Active {
                        pending,
                        scheduled_flush_token: None,
                        in_flight: None,
                    };
                    if let DeliveryPhase::Active {
                        pending,
                        scheduled_flush_token,
                        ..
                    } = &mut record.phase
                    {
                        if !pending.is_empty() {
                            if let Some(timer_token) = self.next_timer_token() {
                                *scheduled_flush_token = Some(timer_token);
                                flush_schedule = Some((
                                    Arc::clone(&record.registration.identity),
                                    record.generation,
                                    timer_token,
                                ));
                            }
                        }
                    }
                    (Self::control_state(record), None)
                }
                DeliveryPhase::Active { .. } => (Self::control_state(record), None),
                DeliveryPhase::ResyncRequired {
                    activation_ready, ..
                } => {
                    if !*activation_ready {
                        *activation_ready = true;
                    }
                    let descriptor = self.reserve_marker(record);
                    (Self::control_state(record), descriptor)
                }
            }
        };
        if let Some((identity, generation, timer_token)) = flush_schedule {
            self.schedule_flush(identity, generation, timer_token);
        }
        (result, descriptor)
    }

    fn acknowledge(
        self: &Arc<Self>,
        identity: &Arc<RegistrationIdentity>,
        generation: u64,
        first_sequence: u64,
        sequence: u64,
    ) -> TerminalOutputControlState {
        let mut flush_schedule = None;
        let result = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let Some(record) = state.selected.as_mut() else {
                return TerminalOutputControlState::stale();
            };
            if !Self::matches_record(record, identity, generation) {
                return TerminalOutputControlState::stale();
            }
            let DeliveryPhase::Active {
                pending,
                scheduled_flush_token,
                in_flight,
            } = &mut record.phase
            else {
                return TerminalOutputControlState::stale();
            };
            let matches_credit = in_flight.as_ref().is_some_and(|credit| {
                credit.first_sequence == first_sequence && credit.sequence == sequence
            });
            if !matches_credit {
                return TerminalOutputControlState::stale();
            }
            *in_flight = None;
            if !pending.is_empty() && scheduled_flush_token.is_none() {
                if let Some(timer_token) = self.next_timer_token() {
                    *scheduled_flush_token = Some(timer_token);
                    flush_schedule = Some((
                        Arc::clone(&record.registration.identity),
                        record.generation,
                        timer_token,
                    ));
                }
            }
            Self::control_state(record)
        };
        if let Some((identity, generation, timer_token)) = flush_schedule {
            self.schedule_flush(identity, generation, timer_token);
        }
        result
    }

    fn report_metrics(
        &self,
        identity: &Arc<RegistrationIdentity>,
        generation: u64,
        metrics: TerminalRendererMetrics,
    ) -> TerminalOutputControlState {
        // Keep the opaque value consumed inside the policy owner. No terminal payload or metric
        // field is logged from this control plane.
        let _metric_count = metrics.values().len();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let result = {
            let Some(record) = state.selected.as_ref() else {
                return TerminalOutputControlState::stale();
            };
            if !Self::matches_record(record, identity, generation) {
                return TerminalOutputControlState::stale();
            }
            Self::control_state(record)
        };
        state.renderer_metric_reports = state.renderer_metric_reports.saturating_add(1);
        result
    }

    fn parser_fault(
        self: &Arc<Self>,
        registration: &Arc<RegisteredPtyOutputTarget>,
        last_successful_sequence: u64,
    ) -> Option<OutputEffectDescriptor> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let record = state.selected.as_mut()?;
        if !Arc::ptr_eq(&record.registration.identity, &registration.identity) {
            return None;
        }
        let activation_ready = matches!(record.phase, DeliveryPhase::Active { .. });
        self.transition_to_resync(record, last_successful_sequence, activation_ready)
    }

    fn deactivate(
        self: &Arc<Self>,
        identity: &Arc<RegistrationIdentity>,
        generation: u64,
    ) -> TerminalOutputControlState {
        let retired = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let matches = state
                .selected
                .as_ref()
                .is_some_and(|record| Self::matches_record(record, identity, generation));
            if matches {
                state.selected.take()
            } else {
                None
            }
        };
        let Some(record) = retired else {
            return TerminalOutputControlState::stale();
        };
        record.effect_gate.close_and_wait();
        TerminalOutputControlState::Inactive {
            session_id: record.registration.session_id.to_string(),
            generation: record.generation.to_string(),
        }
    }

    fn retire_registration(&self, registration: &Arc<RegisteredPtyOutputTarget>) {
        let retired = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let matches = state.selected.as_ref().is_some_and(|record| {
                Arc::ptr_eq(&record.registration.identity, &registration.identity)
            });
            if matches {
                state.selected.take()
            } else {
                None
            }
        };
        if let Some(record) = retired {
            record.effect_gate.close_and_wait();
        }
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
        Self::with_coordinator(
            output_senders,
            idle_detector,
            ws_broadcaster,
            TerminalOutputCoordinator::new(),
        )
    }

    pub(crate) fn with_coordinator(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        coordinator: Arc<TerminalOutputCoordinator>,
    ) -> Self {
        Self {
            output_senders,
            idle_detector,
            response_watchers: Arc::new(Mutex::new(HashMap::new())),
            ws_broadcaster,
            screen_parsers: Arc::new(Mutex::new(HashMap::new())),
            coordinator,
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

    fn registration_for_control(&self, id: Uuid) -> Option<Arc<RegisteredPtyOutputTarget>> {
        let parsers = self.screen_parsers.lock().ok()?;
        parsers
            .get(&id)
            .map(|state| Arc::clone(&state.registration))
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

        let (registration, sequence, parser_fault, ui_open, last_successful_sequence) = {
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
            let ui_open = state.reader_gate.is_open();
            match state.parser_availability {
                ParserAvailability::Unavailable => {
                    (registration, None, false, false, state.output_sequence)
                }
                ParserAvailability::Available => {
                    let processed = crate::logging::catch_payload_unwind(|| {
                        state.parser.process(&data);
                        let sequence = state.output_sequence.checked_add(1).ok_or(())?;
                        state.output_sequence = sequence;
                        // Order matters, and these two lines must stay contiguous. The ring
                        // may only grow once `output_sequence` has advanced: on overflow the
                        // line above returns `Err` and the parser goes `Unavailable`, and a
                        // ring that grew anyway would make the activation payload carry bytes
                        // that `sequence` does not represent. The frontend acks by watermark
                        // and only skips what is at or below the snapshot sequence, so those
                        // bytes would be replayed and then written again when they arrive
                        // live: a duplicated block of history.
                        append_history(&mut state.history, &data);
                        Ok::<u64, ()>(sequence)
                    });
                    match processed {
                        Ok(Ok(sequence)) => {
                            #[cfg(test)]
                            self.trace(FanoutTraceEvent::ParserProcessed(sequence));
                            (registration, Some(sequence), false, ui_open, sequence)
                        }
                        Ok(Err(())) | Err(_) => {
                            state.parser_availability = ParserAvailability::Unavailable;
                            (registration, None, true, ui_open, state.output_sequence)
                        }
                    }
                }
            }
        };

        let (descriptor, ui_admitted) = if parser_fault {
            log::error!("[terminal-snapshot] stage=parser_fault session={id}");
            (
                self.coordinator
                    .parser_fault(&registration, last_successful_sequence),
                false,
            )
        } else if ui_open {
            (
                sequence.and_then(|sequence| {
                    self.coordinator
                        .admit_output(&registration, sequence, data.clone())
                }),
                sequence.is_some(),
            )
        } else {
            (None, false)
        };

        #[cfg(not(test))]
        let _ = ui_admitted;

        if let Some(ref broadcaster) = self.ws_broadcaster {
            #[cfg(test)]
            self.trace(FanoutTraceEvent::Websocket);
            broadcaster.broadcast_pty_output(session_id_str, &data);
        }

        #[cfg(test)]
        if ui_admitted {
            self.trace(FanoutTraceEvent::UiEmit);
        }

        if let Some(descriptor) = descriptor {
            descriptor.run();
        }
        lease.complete();
    }

    /// `include_history` asks for the retained ring instead of the mirrored viewport. The
    /// caller must only set it for a terminal with no rendered content: replaying the ring
    /// over a terminal that already has scrollback appends a duplicate block on every
    /// activation, and the frontend owns that discriminant (`!entry.hasRenderedOutput`).
    /// The failure is asymmetric on purpose: a wrongly `false` flag is today's behaviour.
    pub(crate) fn activate_terminal_output(
        &self,
        id: Uuid,
        include_history: bool,
    ) -> TerminalOutputActivationResult {
        let candidate = {
            let Ok(mut parsers) = self.screen_parsers.lock() else {
                return TerminalOutputActivationResult::recovery(
                    id,
                    TerminalOutputActivationRecoveryCode::ParserUnavailable,
                );
            };
            let Some(state) = parsers.get_mut(&id) else {
                return TerminalOutputActivationResult::recovery(
                    id,
                    TerminalOutputActivationRecoveryCode::ParserUnavailable,
                );
            };
            if state.parser_availability != ParserAvailability::Available {
                return TerminalOutputActivationResult::recovery(
                    id,
                    TerminalOutputActivationRecoveryCode::ParserUnavailable,
                );
            }
            if state.registration.session_id != id
                || !Arc::ptr_eq(&state.registration.fanout_identity, &self.fanout_identity)
            {
                return TerminalOutputActivationResult::recovery(
                    id,
                    TerminalOutputActivationRecoveryCode::OutputTargetUnavailable,
                );
            }
            let registration = Arc::clone(&state.registration);
            let copied = crate::logging::catch_payload_unwind(|| {
                let screen = state.parser.screen();
                let (rows, cols) = screen.size();
                let cells = usize::from(rows).checked_mul(usize::from(cols)).ok_or(())?;
                if rows > MAX_ROWS || cols > MAX_COLUMNS || cells > MAX_CELLS {
                    return Err(());
                }
                let data = if include_history && !state.history.is_empty() {
                    // `as_slices` keeps this a read: `make_contiguous` needs `&mut` and
                    // rotates the buffer during what is otherwise a copy out.
                    let (front, back) = state.history.as_slices();
                    let mut replay = Vec::with_capacity(
                        UI_HISTORY_REPLAY_PROLOGUE.len() + front.len() + back.len(),
                    );
                    replay.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
                    replay.extend_from_slice(front);
                    replay.extend_from_slice(back);
                    replay
                } else {
                    screen.contents_formatted()
                };
                Ok::<PtyScreenSnapshot, ()>(PtyScreenSnapshot {
                    data,
                    rows,
                    cols,
                    sequence: state.output_sequence,
                })
            });
            match copied {
                Ok(Ok(snapshot)) => Ok((registration, snapshot)),
                Ok(Err(())) => Err(TerminalOutputActivationRecoveryCode::SnapshotTooLarge),
                Err(_) => {
                    state.parser_availability = ParserAvailability::Unavailable;
                    Err(TerminalOutputActivationRecoveryCode::SnapshotMalformed)
                }
            }
        };
        match candidate {
            Ok((registration, snapshot)) => self.coordinator.activate(registration, snapshot),
            Err(code) => TerminalOutputActivationResult::recovery(id, code),
        }
    }

    pub(crate) fn ready_terminal_output(
        &self,
        id: Uuid,
        generation: u64,
        snapshot_sequence: u64,
    ) -> TerminalOutputControlState {
        let Some(registration) = self.registration_for_control(id) else {
            return TerminalOutputControlState::stale();
        };
        let (result, descriptor) =
            self.coordinator
                .ready(&registration.identity, generation, snapshot_sequence);
        if let Some(descriptor) = descriptor {
            descriptor.run();
        }
        result
    }

    pub(crate) fn deactivate_terminal_output(
        &self,
        id: Uuid,
        generation: u64,
    ) -> TerminalOutputControlState {
        let Some(registration) = self.registration_for_control(id) else {
            return TerminalOutputControlState::stale();
        };
        self.coordinator
            .deactivate(&registration.identity, generation)
    }

    pub(crate) fn ack_terminal_output_delivery(
        &self,
        id: Uuid,
        generation: u64,
        first_sequence: u64,
        sequence: u64,
    ) -> TerminalOutputControlState {
        let Some(registration) = self.registration_for_control(id) else {
            return TerminalOutputControlState::stale();
        };
        self.coordinator
            .acknowledge(&registration.identity, generation, first_sequence, sequence)
    }

    pub(crate) fn report_terminal_renderer_metrics(
        &self,
        id: Uuid,
        generation: u64,
        metrics: TerminalRendererMetrics,
    ) -> TerminalOutputControlState {
        let Some(registration) = self.registration_for_control(id) else {
            return TerminalOutputControlState::stale();
        };
        self.coordinator
            .report_metrics(&registration.identity, generation, metrics)
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
                return;
            };
            let Some(state) = parsers.get_mut(&id) else {
                return;
            };
            if state.parser_availability != ParserAvailability::Available {
                return;
            }
            let resized =
                crate::logging::catch_payload_unwind(|| state.parser.set_size(rows, cols));
            if resized.is_err() {
                state.parser_availability = ParserAvailability::Unavailable;
                Some((Arc::clone(&state.registration), state.output_sequence))
            } else {
                None
            }
        };
        if let Some((registration, sequence)) = parser_fault {
            log::error!("[terminal-snapshot] stage=parser_fault session={id}");
            if let Some(descriptor) = self.coordinator.parser_fault(&registration, sequence) {
                descriptor.run();
            }
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

        self.coordinator
            .retire_registration(&registration_and_gate.0);
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
    #[cfg(test)]
    pub(crate) fn poison_screen_parsers_for_test(&self) {
        let parsers = Arc::clone(&self.screen_parsers);
        let result = std::panic::catch_unwind(move || {
            let _guard = parsers.lock().unwrap();
            panic!("poison the screen parser map for deterministic test coverage");
        });
        assert!(result.is_err(), "screen-parser poison fixture must panic");
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

    #[test]
    fn fanout_characterization_preserves_raw_order_and_ui_is_last() {
        let id = Uuid::new_v4();
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
        output_senders.lock().unwrap().insert(id, sender);
        let broadcaster = crate::web::broadcast::WsBroadcaster::new();
        let mut websocket_receiver = broadcaster.subscribe();
        let sink: PtyOutputTestSink = Arc::new(Mutex::new(Vec::new()));
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

        let activation = match fanout.activate_terminal_output(id, false) {
            TerminalOutputActivationResult::Activated { activation } => activation,
            other => panic!("expected activation, got {other:?}"),
        };
        let generation = activation.generation.parse().expect("generation");
        let snapshot_sequence = activation
            .snapshot
            .sequence
            .parse()
            .expect("snapshot sequence");
        assert!(matches!(
            fanout.ready_terminal_output(id, generation, snapshot_sequence),
            TerminalOutputControlState::Active { .. }
        ));
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
        std::thread::sleep(Duration::from_millis(UI_BATCH_INTERVAL_MS + 30));
        let emitted = sink.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].1, bytes);
    }

    #[test]
    fn output_targets_have_explicit_success_and_failure_results() {
        let delivery = TerminalOutputDelivery::Data {
            session_id: Uuid::new_v4().to_string(),
            generation: "1".to_string(),
            first_sequence: "1".to_string(),
            sequence: "1".to_string(),
            data: b"target payload".to_vec(),
        };
        assert_eq!(
            PtyOutputTarget::noop().emit_pty_output(delivery.clone()),
            Ok(())
        );

        let sink: PtyOutputTestSink = Arc::new(Mutex::new(Vec::new()));
        let failing = PtyOutputTarget::failing_test_sink(Arc::clone(&sink));
        assert_eq!(
            failing.emit_pty_output(delivery),
            Err(PtyOutputEmitError::Emit)
        );
        assert_eq!(sink.lock().unwrap().len(), 1);
    }

    fn wait_for_target_events(sink: &PtyOutputTestSink, expected: usize) {
        for _ in 0..50 {
            if sink.lock().expect("target sink").len() >= expected {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("target did not receive {expected} event(s)");
    }

    #[test]
    fn ready_flushes_a_pre_ready_batch_through_the_registered_target() {
        let id = Uuid::new_v4();
        let sink: PtyOutputTestSink = Arc::new(Mutex::new(Vec::new()));
        let fanout = fanout();
        let token = fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                4,
                40,
                PtyOutputTarget::from_test_sink(Arc::clone(&sink)),
            )
            .expect("register session");
        let activation = match fanout.activate_terminal_output(id, false) {
            TerminalOutputActivationResult::Activated { activation } => activation,
            other => panic!("expected activation, got {other:?}"),
        };
        let generation = activation.generation.parse().expect("generation");
        let snapshot_sequence = activation
            .snapshot
            .sequence
            .parse()
            .expect("snapshot sequence");

        fanout.handle_output(&token, &id.to_string(), b"post-snapshot".to_vec());
        assert!(sink.lock().expect("target sink").is_empty());

        assert!(matches!(
            fanout.ready_terminal_output(id, generation, snapshot_sequence),
            TerminalOutputControlState::Active { .. }
        ));
        wait_for_target_events(&sink, 1);
        let events = sink.lock().expect("target sink");
        assert_eq!(events[0].0, id.to_string());
        assert_eq!(events[0].1, b"post-snapshot");
        assert_eq!(events[0].2, Some(1));
    }

    #[test]
    fn acknowledgement_timeout_emits_one_marker_without_a_later_chunk() {
        let id = Uuid::new_v4();
        let sink: PtyOutputTestSink = Arc::new(Mutex::new(Vec::new()));
        let fanout = fanout();
        let token = fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                4,
                40,
                PtyOutputTarget::from_test_sink(Arc::clone(&sink)),
            )
            .expect("register session");
        let activation = match fanout.activate_terminal_output(id, false) {
            TerminalOutputActivationResult::Activated { activation } => activation,
            other => panic!("expected activation, got {other:?}"),
        };
        let generation = activation.generation.parse().expect("generation");
        let snapshot_sequence = activation
            .snapshot
            .sequence
            .parse()
            .expect("snapshot sequence");
        assert!(matches!(
            fanout.ready_terminal_output(id, generation, snapshot_sequence),
            TerminalOutputControlState::Active { .. }
        ));
        fanout.handle_output(&token, &id.to_string(), b"credit".to_vec());
        wait_for_target_events(&sink, 1);

        let (identity, delivery_token, deadline_token) = {
            let state = fanout.coordinator.state.lock().expect("coordinator state");
            let record = state.selected.as_ref().expect("selected record");
            let DeliveryPhase::Active {
                in_flight: Some(credit),
                ..
            } = &record.phase
            else {
                panic!("expected in-flight credit");
            };
            (
                Arc::clone(&record.registration.identity),
                credit.delivery_token,
                credit.deadline_token,
            )
        };
        fanout
            .coordinator
            .handle_ack_timeout(&identity, generation, delivery_token, deadline_token)
            .expect("timeout marker")
            .run();

        let events = sink.lock().expect("target sink");
        assert_eq!(events.len(), 2);
        assert!(events[1].1.is_empty());
        assert_eq!(events[1].2, Some(1));
    }

    #[test]
    fn deferred_pre_ready_resync_uses_the_registered_target_after_ready() {
        let id = Uuid::new_v4();
        let sink: PtyOutputTestSink = Arc::new(Mutex::new(Vec::new()));
        let fanout = fanout();
        let token = fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                4,
                40,
                PtyOutputTarget::from_test_sink(Arc::clone(&sink)),
            )
            .expect("register session");
        let activation = match fanout.activate_terminal_output(id, false) {
            TerminalOutputActivationResult::Activated { activation } => activation,
            other => panic!("expected activation, got {other:?}"),
        };
        let generation = activation.generation.parse().expect("generation");
        let snapshot_sequence = activation
            .snapshot
            .sequence
            .parse()
            .expect("snapshot sequence");

        fanout.handle_output(
            &token,
            &id.to_string(),
            vec![b'x'; UI_PENDING_LIMIT_BYTES + 1],
        );
        assert!(sink.lock().expect("target sink").is_empty());

        assert!(matches!(
            fanout.ready_terminal_output(id, generation, snapshot_sequence),
            TerminalOutputControlState::ResyncRequired { .. }
        ));
        let events = sink.lock().expect("target sink");
        assert_eq!(events.len(), 1);
        assert!(events[0].1.is_empty());
        assert_eq!(events[0].2, Some(1));
    }

    #[test]
    fn retired_reader_tokens_cannot_target_a_same_uuid_replacement() {
        let id = Uuid::new_v4();
        let old_sink: PtyOutputTestSink = Arc::new(Mutex::new(Vec::new()));
        let new_sink: PtyOutputTestSink = Arc::new(Mutex::new(Vec::new()));
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
                PtyOutputTarget::from_test_sink(Arc::clone(&new_sink)),
            )
            .expect("register replacement session");

        fanout.handle_output(&old_token, &id.to_string(), b"old output".to_vec());
        let snapshot = fanout
            .get_screen_snapshot(id)
            .expect("replacement snapshot");
        assert_eq!(snapshot.sequence, 0);
        assert!(!String::from_utf8_lossy(&snapshot.data).contains("old output"));
        assert!(old_sink.lock().expect("old target sink").is_empty());

        let activation = match fanout.activate_terminal_output(id, false) {
            TerminalOutputActivationResult::Activated { activation } => activation,
            other => panic!("expected replacement activation, got {other:?}"),
        };
        let generation = activation.generation.parse().expect("generation");
        let snapshot_sequence = activation
            .snapshot
            .sequence
            .parse()
            .expect("snapshot sequence");
        assert!(matches!(
            fanout.ready_terminal_output(id, generation, snapshot_sequence),
            TerminalOutputControlState::Active { .. }
        ));
        fanout.handle_output(&new_token, &id.to_string(), b"replacement output".to_vec());
        wait_for_target_events(&new_sink, 1);
        assert_eq!(
            new_sink.lock().expect("new target sink")[0].1,
            b"replacement output"
        );
    }

    #[test]
    fn metrics_validation_requires_the_exact_object_and_relations() {
        let valid = serde_json::json!({
            "retainedTerminalCount": 1,
            "visibleTerminalCount": 1,
            "webglContextCount": 1,
            "webglContextLossCount": 0,
            "lruEvictionCount": 0,
            "outputEventsReceived": 0,
            "inactiveOrStaleEventsRejected": 0,
            "bytesAccepted": 0,
            "bytesWritten": 0,
            "replayPendingBytes": 0,
            "livePendingBytes": 0,
            "writeInFlightBytes": 0,
            "combinedAdmissionHighWaterBytes": 0,
            "pendingHighWaterBytes": 0,
            "resyncCount": 0,
            "activationReadyAcknowledgements": 0,
            "activationReadyRejections": 0,
            "activationReadyTimeouts": 0,
            "generationHealthPollsScheduled": 0,
            "generationHealthPollsStarted": 0,
            "generationHealthPollsCancelled": 0,
            "replayPendingLivenessRecoveries": 0,
            "snapshotReplayDurationMs": 0,
            "retiredWriteCallbacksIgnoredAfterDisposal": 0,
            "maxAnimationFrameLagMs": 0,
        });
        assert!(
            TerminalRendererMetrics::try_from(TerminalRendererMetricsWire(valid.clone())).is_ok()
        );

        let mut missing = valid.as_object().expect("valid metrics object").clone();
        missing.remove("retainedTerminalCount");
        assert!(
            TerminalRendererMetrics::try_from(TerminalRendererMetricsWire(
                serde_json::Value::Object(missing)
            ))
            .is_err()
        );

        let mut extra = valid.as_object().expect("valid metrics object").clone();
        extra.insert("unexpected".to_string(), serde_json::json!(0));
        assert!(
            TerminalRendererMetrics::try_from(TerminalRendererMetricsWire(
                serde_json::Value::Object(extra)
            ))
            .is_err()
        );

        let mut impossible = valid.as_object().expect("valid metrics object").clone();
        impossible.insert("visibleTerminalCount".to_string(), serde_json::json!(2));
        assert!(
            TerminalRendererMetrics::try_from(TerminalRendererMetricsWire(
                serde_json::Value::Object(impossible)
            ))
            .is_err()
        );
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
        match fanout.activate_terminal_output(id, include_history) {
            TerminalOutputActivationResult::Activated { activation } => activation.snapshot.data,
            other => panic!("expected activation, got {other:?}"),
        }
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

    /// Protects the most frequent path: a retained terminal already holds its scrollback, so
    /// replaying the ring over it would append a duplicate block on every activation.
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
