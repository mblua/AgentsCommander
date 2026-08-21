use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    replay_budget: Arc<ReplayResourceBudget>,
    #[cfg(test)]
    trace: FanoutTraceRecorder,
    #[cfg(test)]
    activation_timings: Arc<Mutex<Vec<SemanticActivationTiming>>>,
    #[cfg(test)]
    capture_barrier: Arc<Mutex<Option<SemanticCaptureBarrier>>>,
}

#[derive(Clone)]
pub struct PtyScreenSnapshot {
    pub data: Vec<u8>,
    pub rows: u16,
    pub cols: u16,
    pub sequence: u64,
}

const SEMANTIC_SCROLLBACK_ROWS: usize = 1024;
const SEMANTIC_HISTORY_REPLAY_BYTES: usize = 64 * 1024;
const ALT_SEQUENCE_MAX_BYTES: usize = 64;
const MAX_JAVASCRIPT_SEQUENCE: u64 = 9_007_199_254_740_991;
const SEMANTIC_REPLAY_MAX_BYTES: usize = 512 * 1024;
const SUPPORTED_SEMANTIC_REPLAY_SESSIONS: usize = 32;
const SEMANTIC_STEADY_BUDGET_BYTES: usize = 128 * 1024 * 1024;
const SEMANTIC_CHECKPOINT_BUDGET_BYTES: usize = 256 * 1024 * 1024;
const SEMANTIC_ATTACH_BUDGET_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PtyTerminalActiveBuffer {
    Normal,
    Alternate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PtyTerminalAlternateEntryMode {
    Mode47,
    Mode1047,
    Mode1049,
}

impl PtyTerminalAlternateEntryMode {
    fn entry_bytes(self) -> &'static [u8] {
        match self {
            Self::Mode47 => b"\x1b[?47h",
            Self::Mode1047 => b"\x1b[?1047h",
            Self::Mode1049 => b"\x1b[?1049h",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PtyTerminalReplayStage {
    SemanticHistory,
    ScreenOnlyHistoryDisabled,
    ScreenOnlyCheckpointUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PtyTerminalHistoryTruncationReason {
    None,
    RowLimitReached,
    ByteLimitReached,
    RowAndByteLimitReached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
// The repeated prefix is the frozen Phase 1 IPC vocabulary, not redundant naming.
#[allow(clippy::enum_variant_names)]
pub(crate) enum PtyTerminalSeedlessReason {
    SeedlessParserUnavailable,
    SeedlessParserPoisoned,
    SeedlessContinuationUnsafe,
    SeedlessInvalidGrid,
    SeedlessResizeFailed,
    SeedlessResourceLimitExceeded,
    SeedlessReplayCapExceeded,
    SeedlessSequenceUnsafe,
    SeedlessCaptureFailed,
    SeedlessEncodeFailed,
}

impl PtyTerminalSeedlessReason {
    fn code(self) -> &'static str {
        match self {
            Self::SeedlessParserUnavailable => "seedlessParserUnavailable",
            Self::SeedlessParserPoisoned => "seedlessParserPoisoned",
            Self::SeedlessContinuationUnsafe => "seedlessContinuationUnsafe",
            Self::SeedlessInvalidGrid => "seedlessInvalidGrid",
            Self::SeedlessResizeFailed => "seedlessResizeFailed",
            Self::SeedlessResourceLimitExceeded => "seedlessResourceLimitExceeded",
            Self::SeedlessReplayCapExceeded => "seedlessReplayCapExceeded",
            Self::SeedlessSequenceUnsafe => "seedlessSequenceUnsafe",
            Self::SeedlessCaptureFailed => "seedlessCaptureFailed",
            Self::SeedlessEncodeFailed => "seedlessEncodeFailed",
        }
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyTerminalReplaySnapshot {
    pub(crate) replay_data: Vec<u8>,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    pub(crate) sequence: u64,
    pub(crate) active_buffer: PtyTerminalActiveBuffer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) alternate_entry_mode: Option<PtyTerminalAlternateEntryMode>,
    pub(crate) replay_stage: PtyTerminalReplayStage,
    pub(crate) history_included: bool,
    pub(crate) history_truncated: bool,
    pub(crate) history_truncation_reason: PtyTerminalHistoryTruncationReason,
    pub(crate) history_boundary_hardened: bool,
    pub(crate) normal_screen_included: bool,
    pub(crate) retained_history_rows: u32,
    pub(crate) included_history_rows: u32,
    pub(crate) semantic_history_bytes: u32,
    pub(crate) replay_bytes: u32,
    pub(crate) pending_parser_bytes: u32,
    pub(crate) active_screen_has_text: bool,
    pub(crate) active_bottom_line_has_text: bool,
    #[serde(skip)]
    _reservation: ReplayResourceReservation,
}

#[derive(Debug)]
pub(crate) struct PtyTerminalOutputActivation {
    pub(crate) snapshot: Option<PtyTerminalReplaySnapshot>,
    pub(crate) seedless_reason: Option<PtyTerminalSeedlessReason>,
    pub(crate) attach_generation: u32,
    pub(crate) document_epoch: u64,
}

impl PtyTerminalOutputActivation {
    fn snapshot(
        snapshot: PtyTerminalReplaySnapshot,
        document_epoch: u64,
        attach_generation: u32,
    ) -> Self {
        Self {
            snapshot: Some(snapshot),
            seedless_reason: None,
            attach_generation,
            document_epoch,
        }
    }

    fn seedless(
        reason: PtyTerminalSeedlessReason,
        document_epoch: u64,
        attach_generation: u32,
    ) -> Self {
        Self {
            snapshot: None,
            seedless_reason: Some(reason),
            attach_generation,
            document_epoch,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplayResourceKind {
    Sessions,
    Steady,
    Checkpoint,
    Attach,
}

#[derive(Debug)]
struct ReplayResourceBudget {
    admitted_sessions: AtomicUsize,
    steady_bytes: AtomicUsize,
    checkpoint_bytes: AtomicUsize,
    attach_bytes: AtomicUsize,
}

impl ReplayResourceBudget {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            admitted_sessions: AtomicUsize::new(0),
            steady_bytes: AtomicUsize::new(0),
            checkpoint_bytes: AtomicUsize::new(0),
            attach_bytes: AtomicUsize::new(0),
        })
    }

    fn counter(&self, kind: ReplayResourceKind) -> (&AtomicUsize, usize) {
        match kind {
            ReplayResourceKind::Sessions => {
                (&self.admitted_sessions, SUPPORTED_SEMANTIC_REPLAY_SESSIONS)
            }
            ReplayResourceKind::Steady => (&self.steady_bytes, SEMANTIC_STEADY_BUDGET_BYTES),
            ReplayResourceKind::Checkpoint => {
                (&self.checkpoint_bytes, SEMANTIC_CHECKPOINT_BUDGET_BYTES)
            }
            ReplayResourceKind::Attach => (&self.attach_bytes, SEMANTIC_ATTACH_BUDGET_BYTES),
        }
    }

    fn try_reserve(
        self: &Arc<Self>,
        kind: ReplayResourceKind,
        amount: usize,
    ) -> Option<ReplayResourceReservation> {
        let (counter, limit) = self.counter(kind);
        let mut current = counter.load(Ordering::Acquire);
        loop {
            let next = current.checked_add(amount)?;
            if next > limit {
                return None;
            }
            match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    return Some(ReplayResourceReservation {
                        budget: Arc::clone(self),
                        kind,
                        amount,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn try_admit(self: &Arc<Self>, steady_bytes: usize) -> Option<SemanticSessionReservation> {
        let session = self.try_reserve(ReplayResourceKind::Sessions, 1)?;
        let Some(steady) = self.try_reserve(ReplayResourceKind::Steady, steady_bytes) else {
            drop(session);
            return None;
        };
        Some(SemanticSessionReservation {
            _session: session,
            steady,
        })
    }

    fn snapshot(&self) -> ReplayResourceSnapshot {
        ReplayResourceSnapshot {
            sessions: self.admitted_sessions.load(Ordering::Acquire),
            steady_bytes: self.steady_bytes.load(Ordering::Acquire),
            checkpoint_bytes: self.checkpoint_bytes.load(Ordering::Acquire),
            attach_bytes: self.attach_bytes.load(Ordering::Acquire),
        }
    }
}

fn process_replay_budget() -> Arc<ReplayResourceBudget> {
    static BUDGET: OnceLock<Arc<ReplayResourceBudget>> = OnceLock::new();
    Arc::clone(BUDGET.get_or_init(ReplayResourceBudget::new))
}

#[derive(Clone, Copy, Debug)]
struct ReplayResourceSnapshot {
    sessions: usize,
    steady_bytes: usize,
    checkpoint_bytes: usize,
    attach_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct ReplayResourceReservation {
    budget: Arc<ReplayResourceBudget>,
    kind: ReplayResourceKind,
    amount: usize,
}

impl ReplayResourceReservation {
    fn try_resize(&mut self, new_amount: usize) -> bool {
        if new_amount == self.amount {
            return true;
        }
        let (counter, limit) = self.budget.counter(self.kind);
        if new_amount < self.amount {
            counter.fetch_sub(self.amount - new_amount, Ordering::AcqRel);
            self.amount = new_amount;
            return true;
        }
        let delta = new_amount - self.amount;
        let mut current = counter.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(delta) else {
                return false;
            };
            if next > limit {
                return false;
            }
            match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    self.amount = new_amount;
                    return true;
                }
                Err(observed) => current = observed,
            }
        }
    }
}

impl Drop for ReplayResourceReservation {
    fn drop(&mut self) {
        self.budget
            .counter(self.kind)
            .0
            .fetch_sub(self.amount, Ordering::AcqRel);
    }
}

#[derive(Debug)]
struct SemanticSessionReservation {
    _session: ReplayResourceReservation,
    steady: ReplayResourceReservation,
}

struct NormalScreenCheckpoint {
    screen: vt100::Screen,
    _reservation: ReplayResourceReservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContinuationUnsafeReason {
    Margins,
    Origin,
    Insert,
    Autowrap,
    SavedCursor,
    Charset,
    TabStops,
    UnknownMode,
    MalformedControl,
    OpenControlString,
}

impl ContinuationUnsafeReason {
    fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

#[derive(Clone, Copy)]
struct ControlBytes {
    bytes: [u8; ALT_SEQUENCE_MAX_BYTES],
    len: usize,
}

impl ControlBytes {
    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlStringKind {
    Osc,
    Opaque,
}

enum BoundaryEvent {
    Hold,
    Raw(u8),
    Control(ControlBytes),
    StartString(ControlBytes, ControlStringKind),
    Overlong(ControlBytes, bool),
}

struct ReplayBoundaryTracker {
    pending: [u8; ALT_SEQUENCE_MAX_BYTES],
    pending_len: usize,
    opaque_csi: bool,
    control_string: Option<ControlStringKind>,
    control_string_esc: bool,
    expected_alternate: bool,
    alternate_entry_mode: Option<PtyTerminalAlternateEntryMode>,
    checkpoint_reliable: bool,
    unsafe_reasons: u16,
    charset_designations: u8,
    charset_shifted: bool,
    charset_coding_non_default: bool,
}

impl ReplayBoundaryTracker {
    fn new() -> Self {
        Self {
            pending: [0; ALT_SEQUENCE_MAX_BYTES],
            pending_len: 0,
            opaque_csi: false,
            control_string: None,
            control_string_esc: false,
            expected_alternate: false,
            alternate_entry_mode: None,
            checkpoint_reliable: true,
            unsafe_reasons: 0,
            charset_designations: 0,
            charset_shifted: false,
            charset_coding_non_default: false,
        }
    }

    fn set_unsafe(&mut self, reason: ContinuationUnsafeReason, unsafe_now: bool) {
        if unsafe_now {
            self.unsafe_reasons |= reason.bit();
        } else {
            self.unsafe_reasons &= !reason.bit();
        }
    }

    fn first_unsafe_reason(&self) -> Option<ContinuationUnsafeReason> {
        [
            ContinuationUnsafeReason::Margins,
            ContinuationUnsafeReason::Origin,
            ContinuationUnsafeReason::Insert,
            ContinuationUnsafeReason::Autowrap,
            ContinuationUnsafeReason::SavedCursor,
            ContinuationUnsafeReason::Charset,
            ContinuationUnsafeReason::TabStops,
            ContinuationUnsafeReason::UnknownMode,
            ContinuationUnsafeReason::MalformedControl,
            ContinuationUnsafeReason::OpenControlString,
        ]
        .into_iter()
        .find(|reason| self.unsafe_reasons & reason.bit() != 0)
    }

    fn refresh_charset_safety(&mut self) {
        self.set_unsafe(
            ContinuationUnsafeReason::Charset,
            self.charset_designations != 0
                || self.charset_shifted
                || self.charset_coding_non_default,
        );
    }

    fn set_charset_designation(&mut self, register: u8, is_default: bool) {
        let bit = 1 << register;
        if is_default {
            self.charset_designations &= !bit;
        } else {
            self.charset_designations |= bit;
        }
        self.refresh_charset_safety();
    }

    fn set_charset_shifted(&mut self, shifted: bool) {
        self.charset_shifted = shifted;
        self.refresh_charset_safety();
    }

    fn set_charset_coding_non_default(&mut self, non_default: bool) {
        self.charset_coding_non_default = non_default;
        self.refresh_charset_safety();
    }

    fn reset(&mut self) {
        self.pending_len = 0;
        self.opaque_csi = false;
        self.control_string = None;
        self.control_string_esc = false;
        self.expected_alternate = false;
        self.alternate_entry_mode = None;
        self.checkpoint_reliable = true;
        self.unsafe_reasons = 0;
        self.charset_designations = 0;
        self.charset_shifted = false;
        self.charset_coding_non_default = false;
    }

    fn pending_original(&self) -> &[u8] {
        &self.pending[..self.pending_len]
    }

    fn pending_is_csi(&self) -> bool {
        self.pending.first() == Some(&0x9b)
            || (self.pending_len >= 2 && self.pending[0] == 0x1b && self.pending[1] == b'[')
    }

    fn take_pending(&mut self) -> ControlBytes {
        let control = ControlBytes {
            bytes: self.pending,
            len: self.pending_len,
        };
        self.pending_len = 0;
        control
    }

    fn consume(&mut self, byte: u8) -> BoundaryEvent {
        if let Some(kind) = self.control_string {
            let closes = byte == 0x9c
                || (kind == ControlStringKind::Osc && byte == 0x07)
                || (self.control_string_esc && byte == b'\\');
            self.control_string_esc = byte == 0x1b && !self.control_string_esc;
            if closes {
                self.control_string = None;
                self.control_string_esc = false;
                self.set_unsafe(ContinuationUnsafeReason::OpenControlString, false);
            }
            return BoundaryEvent::Raw(byte);
        }

        if self.opaque_csi {
            if (0x40..=0x7e).contains(&byte) {
                self.opaque_csi = false;
            }
            return BoundaryEvent::Raw(byte);
        }

        if self.pending_len == 0 {
            if byte == 0x1b || byte == 0x9b {
                self.pending[0] = byte;
                self.pending_len = 1;
                return BoundaryEvent::Hold;
            }
            if matches!(byte, 0x9d | 0x90 | 0x98 | 0x9e | 0x9f) {
                let kind = if byte == 0x9d {
                    ControlStringKind::Osc
                } else {
                    ControlStringKind::Opaque
                };
                self.control_string = Some(kind);
                self.set_unsafe(ContinuationUnsafeReason::OpenControlString, true);
                if kind == ControlStringKind::Opaque {
                    self.set_unsafe(ContinuationUnsafeReason::UnknownMode, true);
                }
            }
            return BoundaryEvent::Raw(byte);
        }

        self.pending[self.pending_len] = byte;
        self.pending_len += 1;

        if self.pending[0] == 0x1b && self.pending_len == 2 {
            match byte {
                b'[' => return BoundaryEvent::Hold,
                b']' | b'P' | b'X' | b'^' | b'_' => {
                    let kind = if byte == b']' {
                        ControlStringKind::Osc
                    } else {
                        ControlStringKind::Opaque
                    };
                    self.control_string = Some(kind);
                    self.set_unsafe(ContinuationUnsafeReason::OpenControlString, true);
                    if kind == ControlStringKind::Opaque {
                        self.set_unsafe(ContinuationUnsafeReason::UnknownMode, true);
                    }
                    return BoundaryEvent::StartString(self.take_pending(), kind);
                }
                0x20..=0x2f => return BoundaryEvent::Hold,
                0x30..=0x7e => return BoundaryEvent::Control(self.take_pending()),
                _ => {
                    self.set_unsafe(ContinuationUnsafeReason::MalformedControl, true);
                    self.checkpoint_reliable = false;
                    return BoundaryEvent::Control(self.take_pending());
                }
            }
        }

        let is_csi = self.pending_is_csi();
        if is_csi {
            if (0x40..=0x7e).contains(&byte) {
                return BoundaryEvent::Control(self.take_pending());
            }
            if !(0x20..=0x3f).contains(&byte) {
                self.set_unsafe(ContinuationUnsafeReason::MalformedControl, true);
                self.checkpoint_reliable = false;
                return BoundaryEvent::Control(self.take_pending());
            }
        } else if (0x30..=0x7e).contains(&byte) {
            return BoundaryEvent::Control(self.take_pending());
        } else if !(0x20..=0x2f).contains(&byte) {
            self.set_unsafe(ContinuationUnsafeReason::MalformedControl, true);
            self.checkpoint_reliable = false;
            return BoundaryEvent::Control(self.take_pending());
        }

        if self.pending_len == ALT_SEQUENCE_MAX_BYTES {
            let was_csi = is_csi;
            self.opaque_csi = was_csi;
            self.set_unsafe(ContinuationUnsafeReason::MalformedControl, true);
            self.checkpoint_reliable = false;
            return BoundaryEvent::Overlong(self.take_pending(), was_csi);
        }
        BoundaryEvent::Hold
    }
}

struct ScreenReplayState {
    parser: vt100::Parser,
    output_sequence: u64,
    registration: Arc<RegisteredPtyOutputTarget>,
    reader_gate: Arc<ReaderOperationGate>,
    parser_availability: ParserAvailability,
    semantic_unavailable: Option<PtyTerminalSeedlessReason>,
    tracker: ReplayBoundaryTracker,
    normal_checkpoint: Option<NormalScreenCheckpoint>,
    semantic_reservation: Option<SemanticSessionReservation>,
    poison_warned: bool,
    /// The last grid the ConPTY actually took (rows, cols): recorded by every
    /// follow call BEFORE the skippable steps, so a skipped or failed
    /// `set_size` leaves a visible divergence for the attach reconcile (#1439).
    /// On transport backends (container) the follow runs after merely queuing
    /// the resize frame, so there this records the size last REQUESTED of the
    /// remote, not necessarily taken; the local backend's `if sent` gate is
    /// what keeps the record honest where #1439 lives.
    conpty_size: (u16, u16),
}

struct SnapshotMaterial {
    live_screen: vt100::Screen,
    normal_checkpoint: Option<vt100::Screen>,
    pending: [u8; ALT_SEQUENCE_MAX_BYTES],
    pending_len: usize,
    sequence: u64,
    include_history: bool,
    alternate_entry_mode: Option<PtyTerminalAlternateEntryMode>,
    checkpoint_reliable: bool,
    clone_micros: u64,
    reservation: ReplayResourceReservation,
}

impl ScreenReplayState {
    fn process_parser_bytes(&mut self, bytes: &[u8], expected_transition: bool) {
        if bytes.is_empty() {
            return;
        }
        let before = self.parser.screen().alternate_screen();
        self.parser.process(bytes);
        let after = self.parser.screen().alternate_screen();
        if before != after && !expected_transition {
            self.normal_checkpoint = None;
            self.tracker.checkpoint_reliable = false;
            self.tracker.expected_alternate = after;
            self.tracker.alternate_entry_mode = None;
        }
    }

    fn invalidate_checkpoint(&mut self) {
        self.normal_checkpoint = None;
        self.tracker.checkpoint_reliable = false;
    }

    fn capture_normal_checkpoint(&mut self, budget: &Arc<ReplayResourceBudget>) {
        self.normal_checkpoint = None;
        self.tracker.checkpoint_reliable = false;
        if self.semantic_unavailable.is_some() || self.semantic_reservation.is_none() {
            return;
        }
        let (rows, cols) = self.parser.screen().size();
        let Some(bytes) = semantic_cell_storage_bytes(rows, cols) else {
            return;
        };
        let Some(reservation) = budget.try_reserve(ReplayResourceKind::Checkpoint, bytes) else {
            return;
        };
        let cloned = crate::logging::catch_payload_unwind(|| {
            let mut screen = self.parser.screen().clone();
            screen.set_scrollback(0);
            screen
        });
        if let Ok(screen) = cloned {
            self.normal_checkpoint = Some(NormalScreenCheckpoint {
                screen,
                _reservation: reservation,
            });
            self.tracker.checkpoint_reliable = true;
        }
    }

    fn apply_alternate_action(
        &mut self,
        mode: PtyTerminalAlternateEntryMode,
        enter: bool,
        budget: &Arc<ReplayResourceBudget>,
    ) {
        let was_alternate = self.parser.screen().alternate_screen();
        if enter && !was_alternate {
            self.capture_normal_checkpoint(budget);
        }

        match (mode, enter) {
            (PtyTerminalAlternateEntryMode::Mode47, true) => {
                self.process_parser_bytes(b"\x1b[?47h", true);
            }
            (PtyTerminalAlternateEntryMode::Mode47, false) => {
                self.process_parser_bytes(b"\x1b[?47l", true);
            }
            (PtyTerminalAlternateEntryMode::Mode1047, true) => {
                self.process_parser_bytes(b"\x1b[?47h", true);
            }
            (PtyTerminalAlternateEntryMode::Mode1047, false) => {
                self.process_parser_bytes(b"\x1b[2J", false);
                self.process_parser_bytes(b"\x1b[?47l", true);
            }
            (PtyTerminalAlternateEntryMode::Mode1049, true) => {
                self.process_parser_bytes(b"\x1b[?1049h", true);
            }
            (PtyTerminalAlternateEntryMode::Mode1049, false) => {
                self.process_parser_bytes(b"\x1b[?1049l", true);
            }
        }

        let expected = enter;
        self.tracker.expected_alternate = expected;
        self.tracker.alternate_entry_mode = enter.then_some(mode);
        if self.parser.screen().alternate_screen() != expected {
            self.invalidate_checkpoint();
        } else if enter && !was_alternate {
            self.tracker.checkpoint_reliable = self.normal_checkpoint.is_some();
        }
        if !enter {
            self.normal_checkpoint = None;
            self.tracker.checkpoint_reliable = true;
        }
    }

    fn update_mode_safety(&mut self, private: bool, parameter: u16, set: bool) {
        if private {
            match parameter {
                6 => self
                    .tracker
                    .set_unsafe(ContinuationUnsafeReason::Origin, set),
                7 => self
                    .tracker
                    .set_unsafe(ContinuationUnsafeReason::Autowrap, !set),
                1 | 9 | 25 | 47 | 1000 | 1002 | 1003 | 1005 | 1006 | 1047 | 1049 | 2004 => {}
                _ => self
                    .tracker
                    .set_unsafe(ContinuationUnsafeReason::UnknownMode, true),
            }
        } else {
            match parameter {
                4 => self
                    .tracker
                    .set_unsafe(ContinuationUnsafeReason::Insert, set),
                _ => self
                    .tracker
                    .set_unsafe(ContinuationUnsafeReason::UnknownMode, true),
            }
        }
    }

    fn process_csi_control(&mut self, bytes: &[u8], budget: &Arc<ReplayResourceBudget>) {
        let Some((private, parameters, final_byte)) = parse_csi(bytes) else {
            self.tracker
                .set_unsafe(ContinuationUnsafeReason::MalformedControl, true);
            self.invalidate_checkpoint();
            self.process_parser_bytes(bytes, false);
            return;
        };

        if matches!(final_byte, b'h' | b'l') {
            let set = final_byte == b'h';
            let has_alternate = private
                && parameters
                    .iter()
                    .any(|parameter| matches!(parameter, 47 | 1047 | 1049));
            for parameter in &parameters {
                self.update_mode_safety(private, *parameter, set);
            }
            if has_alternate {
                for parameter in parameters {
                    match parameter {
                        47 => self.apply_alternate_action(
                            PtyTerminalAlternateEntryMode::Mode47,
                            set,
                            budget,
                        ),
                        1047 => self.apply_alternate_action(
                            PtyTerminalAlternateEntryMode::Mode1047,
                            set,
                            budget,
                        ),
                        1049 => self.apply_alternate_action(
                            PtyTerminalAlternateEntryMode::Mode1049,
                            set,
                            budget,
                        ),
                        other => {
                            let normalized = canonical_mode_sequence(private, other, final_byte);
                            self.process_parser_bytes(&normalized, false);
                        }
                    }
                }
                return;
            }
        } else if !private && final_byte == b'r' {
            let (rows, _) = self.parser.screen().size();
            let default_margins = parameters.is_empty()
                || parameters.as_slice() == [0]
                || parameters.as_slice() == [1, rows];
            self.tracker
                .set_unsafe(ContinuationUnsafeReason::Margins, !default_margins);
        } else if !private && final_byte == b's' {
            self.tracker
                .set_unsafe(ContinuationUnsafeReason::SavedCursor, true);
        } else if !private && final_byte == b'g' {
            self.tracker
                .set_unsafe(ContinuationUnsafeReason::TabStops, true);
        }

        self.process_parser_bytes(bytes, false);
    }

    fn process_control(&mut self, control: ControlBytes, budget: &Arc<ReplayResourceBudget>) {
        let bytes = control.as_slice();
        if bytes == b"\x1bc" {
            self.process_parser_bytes(bytes, false);
            self.normal_checkpoint = None;
            self.tracker.reset();
            return;
        }
        if is_csi(bytes) {
            self.process_csi_control(bytes, budget);
            return;
        }
        match bytes {
            b"\x1b7" => self
                .tracker
                .set_unsafe(ContinuationUnsafeReason::SavedCursor, true),
            b"\x1bH" => self
                .tracker
                .set_unsafe(ContinuationUnsafeReason::TabStops, true),
            _ if bytes.len() >= 3
                && bytes[0] == 0x1b
                && matches!(bytes[1], b'(' | b')' | b'*' | b'+') =>
            {
                let register = match bytes[1] {
                    b'(' => 0,
                    b')' => 1,
                    b'*' => 2,
                    b'+' => 3,
                    _ => unreachable!(),
                };
                self.tracker
                    .set_charset_designation(register, bytes[2] == b'B');
            }
            _ if bytes.len() >= 3 && bytes[0] == 0x1b && bytes[1] == b'%' => self
                .tracker
                .set_charset_coding_non_default(!matches!(bytes[2], b'G' | b'8')),
            b"\x1bN" | b"\x1bO" | b"\x1bn" | b"\x1bo" | b"\x1b|" | b"\x1b}" | b"\x1b~" => {
                self.tracker.set_charset_shifted(true)
            }
            _ => {}
        }
        self.process_parser_bytes(bytes, false);
    }

    fn process_output(&mut self, data: &[u8], budget: &Arc<ReplayResourceBudget>) {
        let mut raw = Vec::with_capacity(data.len());
        for byte in data {
            match self.tracker.consume(*byte) {
                BoundaryEvent::Hold => {}
                BoundaryEvent::Raw(raw_byte) => {
                    match raw_byte {
                        0x0e | 0x8e | 0x8f => self.tracker.set_charset_shifted(true),
                        0x0f => self.tracker.set_charset_shifted(false),
                        _ => {}
                    }
                    raw.push(raw_byte);
                }
                BoundaryEvent::Control(control) => {
                    self.process_parser_bytes(&raw, false);
                    raw.clear();
                    self.process_control(control, budget);
                }
                BoundaryEvent::StartString(control, kind) => {
                    self.process_parser_bytes(&raw, false);
                    raw.clear();
                    if kind == ControlStringKind::Opaque {
                        self.invalidate_checkpoint();
                    }
                    self.process_parser_bytes(control.as_slice(), false);
                }
                BoundaryEvent::Overlong(control, was_csi) => {
                    self.process_parser_bytes(&raw, false);
                    raw.clear();
                    if was_csi {
                        self.tracker
                            .set_unsafe(ContinuationUnsafeReason::UnknownMode, true);
                    }
                    self.invalidate_checkpoint();
                    self.process_parser_bytes(control.as_slice(), false);
                }
            }
        }
        self.process_parser_bytes(&raw, false);
        if self.parser.screen().alternate_screen() != self.tracker.expected_alternate {
            self.invalidate_checkpoint();
            self.tracker.expected_alternate = self.parser.screen().alternate_screen();
            self.tracker.alternate_entry_mode = None;
        }
    }

    fn mark_semantic_unavailable(&mut self, reason: PtyTerminalSeedlessReason) -> bool {
        let transitioned = self.semantic_unavailable.is_none();
        self.semantic_unavailable = Some(reason);
        self.normal_checkpoint = None;
        transitioned
    }

    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), PtyTerminalSeedlessReason> {
        if !valid_semantic_grid(rows, cols) {
            self.mark_semantic_unavailable(PtyTerminalSeedlessReason::SeedlessInvalidGrid);
            return Err(PtyTerminalSeedlessReason::SeedlessInvalidGrid);
        }

        let bytes = semantic_cell_storage_bytes(rows, cols)
            .ok_or(PtyTerminalSeedlessReason::SeedlessInvalidGrid)?;
        if let Some(resources) = self.semantic_reservation.as_mut() {
            let old_steady = resources.steady.amount;
            if !resources.steady.try_resize(bytes) {
                self.mark_semantic_unavailable(
                    PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded,
                );
                return Err(PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded);
            }
            if let Some(checkpoint) = self.normal_checkpoint.as_mut() {
                if !checkpoint._reservation.try_resize(bytes) {
                    let _ = resources.steady.try_resize(old_steady);
                    self.mark_semantic_unavailable(
                        PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded,
                    );
                    return Err(PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded);
                }
            }
        }

        if !self.parser.screen().alternate_screen() {
            self.normal_checkpoint = None;
            self.tracker.checkpoint_reliable = true;
        }
        self.parser.screen_mut().set_size(rows, cols);
        if let Some(checkpoint) = self.normal_checkpoint.as_mut() {
            checkpoint.screen.set_size(rows, cols);
            checkpoint.screen.set_scrollback(0);
        }
        Ok(())
    }

    fn capture_snapshot_material(
        &self,
        include_history: bool,
        budget: &Arc<ReplayResourceBudget>,
    ) -> Result<SnapshotMaterial, PtyTerminalSeedlessReason> {
        if self.parser_availability != ParserAvailability::Available {
            return Err(PtyTerminalSeedlessReason::SeedlessParserUnavailable);
        }
        if let Some(reason) = self.semantic_unavailable {
            return Err(reason);
        }
        if self.output_sequence > MAX_JAVASCRIPT_SEQUENCE {
            return Err(PtyTerminalSeedlessReason::SeedlessSequenceUnsafe);
        }
        if self.tracker.first_unsafe_reason().is_some() {
            return Err(PtyTerminalSeedlessReason::SeedlessContinuationUnsafe);
        }
        let (rows, cols) = self.parser.screen().size();
        if !valid_semantic_grid(rows, cols) {
            return Err(PtyTerminalSeedlessReason::SeedlessInvalidGrid);
        }
        let screen_bytes = semantic_cell_storage_bytes(rows, cols)
            .ok_or(PtyTerminalSeedlessReason::SeedlessInvalidGrid)?;
        let use_checkpoint = self.parser.screen().alternate_screen()
            && self.tracker.checkpoint_reliable
            && self.normal_checkpoint.is_some()
            && self.tracker.alternate_entry_mode.is_some();
        let clone_count = 1usize + usize::from(use_checkpoint);
        let attach_bytes = screen_bytes
            .checked_mul(clone_count)
            .and_then(|bytes| bytes.checked_add(SEMANTIC_REPLAY_MAX_BYTES))
            .ok_or(PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded)?;
        let reservation = budget
            .try_reserve(ReplayResourceKind::Attach, attach_bytes)
            .ok_or(PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded)?;
        let clone_started = Instant::now();
        let live_screen = self.parser.screen().clone();
        let normal_checkpoint = use_checkpoint.then(|| {
            self.normal_checkpoint
                .as_ref()
                .expect("checkpoint presence validated")
                .screen
                .clone()
        });
        let clone_micros = elapsed_micros(clone_started);
        let mut pending = [0; ALT_SEQUENCE_MAX_BYTES];
        let pending_original = self.tracker.pending_original();
        pending[..pending_original.len()].copy_from_slice(pending_original);
        Ok(SnapshotMaterial {
            live_screen,
            normal_checkpoint,
            pending,
            pending_len: pending_original.len(),
            sequence: self.output_sequence,
            include_history,
            alternate_entry_mode: self.tracker.alternate_entry_mode,
            checkpoint_reliable: self.tracker.checkpoint_reliable,
            clone_micros,
            reservation,
        })
    }
}

fn elapsed_micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn valid_semantic_grid(rows: u16, cols: u16) -> bool {
    rows != 0
        && cols != 0
        && semantic_cell_storage_bytes(rows, cols)
            .is_some_and(|bytes| bytes <= SEMANTIC_STEADY_BUDGET_BYTES)
}

fn semantic_cell_storage_bytes(rows: u16, cols: u16) -> Option<usize> {
    SEMANTIC_SCROLLBACK_ROWS
        .checked_add(usize::from(rows).checked_mul(2)?)?
        .checked_mul(usize::from(cols))?
        .checked_mul(32)
}

fn is_csi(bytes: &[u8]) -> bool {
    bytes.first() == Some(&0x9b) || bytes.starts_with(b"\x1b[")
}

fn parse_csi(bytes: &[u8]) -> Option<(bool, Vec<u16>, u8)> {
    if !is_csi(bytes) || bytes.len() < 2 {
        return None;
    }
    let prefix = if bytes[0] == 0x9b { 1 } else { 2 };
    let final_byte = *bytes.last()?;
    if !(0x40..=0x7e).contains(&final_byte) || prefix >= bytes.len() {
        return None;
    }
    let mut body = &bytes[prefix..bytes.len() - 1];
    let private = body.first() == Some(&b'?');
    if private {
        body = &body[1..];
    }
    if body.is_empty() {
        return Some((private, Vec::new(), final_byte));
    }
    if !body
        .iter()
        .all(|byte| byte.is_ascii_digit() || *byte == b';')
    {
        return None;
    }
    let mut parameters = Vec::new();
    for parameter in body.split(|byte| *byte == b';') {
        if parameter.is_empty() {
            parameters.push(0);
            continue;
        }
        let mut value = 0u16;
        for digit in parameter {
            value = value
                .checked_mul(10)?
                .checked_add(u16::from(*digit - b'0'))?;
        }
        parameters.push(value);
    }
    Some((private, parameters, final_byte))
}

fn canonical_mode_sequence(private: bool, parameter: u16, final_byte: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x1b[");
    if private {
        bytes.push(b'?');
    }
    bytes.extend_from_slice(parameter.to_string().as_bytes());
    bytes.push(final_byte);
    bytes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReplayCellStyle {
    foreground: vt100::Color,
    background: vt100::Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl Default for ReplayCellStyle {
    fn default() -> Self {
        Self {
            foreground: vt100::Color::Default,
            background: vt100::Color::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

impl ReplayCellStyle {
    fn from_cell(cell: &vt100::Cell) -> Self {
        Self {
            foreground: cell.fgcolor(),
            background: cell.bgcolor(),
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }

    fn write_to(self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(b"\x1b[0");
        if self.bold {
            bytes.extend_from_slice(b";1");
        }
        if self.dim {
            bytes.extend_from_slice(b";2");
        }
        if self.italic {
            bytes.extend_from_slice(b";3");
        }
        if self.underline {
            bytes.extend_from_slice(b";4");
        }
        if self.inverse {
            bytes.extend_from_slice(b";7");
        }
        write_color(bytes, self.foreground, true);
        write_color(bytes, self.background, false);
        bytes.push(b'm');
    }
}

fn write_color(bytes: &mut Vec<u8>, color: vt100::Color, foreground: bool) {
    match color {
        vt100::Color::Default => {}
        vt100::Color::Idx(index) => {
            bytes.extend_from_slice(if foreground { b";38;5;" } else { b";48;5;" });
            bytes.extend_from_slice(index.to_string().as_bytes());
        }
        vt100::Color::Rgb(red, green, blue) => {
            bytes.extend_from_slice(if foreground { b";38;2;" } else { b";48;2;" });
            bytes.extend_from_slice(red.to_string().as_bytes());
            bytes.push(b';');
            bytes.extend_from_slice(green.to_string().as_bytes());
            bytes.push(b';');
            bytes.extend_from_slice(blue.to_string().as_bytes());
        }
    }
}

struct EncodedPhysicalRow {
    bytes: Vec<u8>,
    wrapped: bool,
}

impl EncodedPhysicalRow {
    fn followed_cost(&self) -> Option<usize> {
        self.bytes
            .len()
            .checked_add(if self.wrapped { 0 } else { 2 })
    }
}

struct EncodedScreenRows {
    history: VecDeque<EncodedPhysicalRow>,
    history_bytes: usize,
    current: Vec<EncodedPhysicalRow>,
    state: Vec<u8>,
    retained_history_rows: usize,
    row_limit_reached: bool,
    byte_limit_reached: bool,
    omitted_predecessor_wrapped: bool,
}

fn encode_physical_row(
    screen: &vt100::Screen,
    row: u16,
) -> Result<EncodedPhysicalRow, PtyTerminalSeedlessReason> {
    let (_, cols) = screen.size();
    let initial = usize::from(cols)
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(8))
        .ok_or(PtyTerminalSeedlessReason::SeedlessEncodeFailed)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(initial.min(SEMANTIC_REPLAY_MAX_BYTES))
        .map_err(|_| PtyTerminalSeedlessReason::SeedlessEncodeFailed)?;
    bytes.extend_from_slice(b"\x1b[0m");
    let mut style = ReplayCellStyle::default();
    for col in 0..cols {
        let cell = screen
            .cell(row, col)
            .ok_or(PtyTerminalSeedlessReason::SeedlessEncodeFailed)?;
        if cell.is_wide_continuation() {
            continue;
        }
        let next_style = ReplayCellStyle::from_cell(cell);
        if next_style != style {
            next_style.write_to(&mut bytes);
            style = next_style;
        }
        if cell.contents().is_empty() {
            bytes.push(b' ');
        } else {
            bytes.extend_from_slice(cell.contents().as_bytes());
        }
        if bytes.len() > SEMANTIC_REPLAY_MAX_BYTES {
            return Err(PtyTerminalSeedlessReason::SeedlessReplayCapExceeded);
        }
    }
    bytes.extend_from_slice(b"\x1b[0m");
    Ok(EncodedPhysicalRow {
        bytes,
        wrapped: screen.row_wrapped(row),
    })
}

fn push_history_row(
    history: &mut VecDeque<EncodedPhysicalRow>,
    history_bytes: &mut usize,
    row: EncodedPhysicalRow,
    byte_limit_reached: &mut bool,
    omitted_predecessor_wrapped: &mut bool,
) -> Result<(), PtyTerminalSeedlessReason> {
    let cost = row
        .followed_cost()
        .ok_or(PtyTerminalSeedlessReason::SeedlessEncodeFailed)?;
    *history_bytes = history_bytes
        .checked_add(cost)
        .ok_or(PtyTerminalSeedlessReason::SeedlessEncodeFailed)?;
    history.push_back(row);
    while *history_bytes > SEMANTIC_HISTORY_REPLAY_BYTES {
        let Some(omitted) = history.pop_front() else {
            return Err(PtyTerminalSeedlessReason::SeedlessEncodeFailed);
        };
        *history_bytes = history_bytes
            .checked_sub(
                omitted
                    .followed_cost()
                    .ok_or(PtyTerminalSeedlessReason::SeedlessEncodeFailed)?,
            )
            .ok_or(PtyTerminalSeedlessReason::SeedlessEncodeFailed)?;
        *byte_limit_reached = true;
        *omitted_predecessor_wrapped = omitted.wrapped;
    }
    Ok(())
}

fn encode_screen_rows(
    mut screen: vt100::Screen,
    include_history: bool,
) -> Result<EncodedScreenRows, PtyTerminalSeedlessReason> {
    screen.set_scrollback(usize::MAX);
    let retained_history_rows = screen.scrollback();
    let row_limit_reached = retained_history_rows == SEMANTIC_SCROLLBACK_ROWS;
    let mut history = VecDeque::new();
    let mut history_bytes = 0usize;
    let mut byte_limit_reached = false;
    let mut omitted_predecessor_wrapped = false;
    if include_history {
        for offset in (1..=retained_history_rows).rev() {
            screen.set_scrollback(offset);
            let row = encode_physical_row(&screen, 0)?;
            push_history_row(
                &mut history,
                &mut history_bytes,
                row,
                &mut byte_limit_reached,
                &mut omitted_predecessor_wrapped,
            )?;
        }
    }
    screen.set_scrollback(0);
    let (rows, _) = screen.size();
    let mut current = Vec::new();
    current
        .try_reserve_exact(usize::from(rows))
        .map_err(|_| PtyTerminalSeedlessReason::SeedlessEncodeFailed)?;
    for row in 0..rows {
        current.push(encode_physical_row(&screen, row)?);
    }
    let mut state = screen.input_mode_formatted();
    state.extend_from_slice(&screen.cursor_state_formatted());
    state.extend_from_slice(&screen.attributes_formatted());
    Ok(EncodedScreenRows {
        history,
        history_bytes,
        current,
        state,
        retained_history_rows,
        row_limit_reached,
        byte_limit_reached,
        omitted_predecessor_wrapped,
    })
}

fn physical_rows_cost(rows: &[EncodedPhysicalRow]) -> Option<usize> {
    let mut total = 0usize;
    for (index, row) in rows.iter().enumerate() {
        total = total.checked_add(row.bytes.len())?;
        if index + 1 < rows.len() && !row.wrapped {
            total = total.checked_add(2)?;
        }
    }
    Some(total)
}

fn append_physical_rows(
    output: &mut Vec<u8>,
    rows: impl Iterator<Item = EncodedPhysicalRow>,
    followed_by_row: bool,
) {
    let rows = rows.collect::<Vec<_>>();
    let count = rows.len();
    for (index, row) in rows.into_iter().enumerate() {
        output.extend_from_slice(&row.bytes);
        if (index + 1 < count || followed_by_row) && !row.wrapped {
            output.extend_from_slice(b"\r\n");
        }
    }
}

fn screen_has_text(screen: &vt100::Screen) -> bool {
    let (rows, cols) = screen.size();
    (0..rows).any(|row| {
        (0..cols).any(|col| {
            screen
                .cell(row, col)
                .is_some_and(|cell| cell.contents().chars().any(|value| !value.is_whitespace()))
        })
    })
}

fn bottom_line_has_text(screen: &vt100::Screen) -> bool {
    let (rows, cols) = screen.size();
    let Some(row) = rows.checked_sub(1) else {
        return false;
    };
    (0..cols).any(|col| {
        screen
            .cell(row, col)
            .is_some_and(|cell| cell.contents().chars().any(|value| !value.is_whitespace()))
    })
}

fn encode_snapshot_material(
    material: SnapshotMaterial,
) -> Result<PtyTerminalReplaySnapshot, PtyTerminalSeedlessReason> {
    let SnapshotMaterial {
        live_screen,
        normal_checkpoint,
        pending,
        pending_len,
        sequence,
        include_history,
        alternate_entry_mode,
        checkpoint_reliable,
        clone_micros: _,
        reservation,
    } = material;
    let (rows, cols) = live_screen.size();
    if !valid_semantic_grid(rows, cols) || sequence > MAX_JAVASCRIPT_SEQUENCE {
        return Err(PtyTerminalSeedlessReason::SeedlessInvalidGrid);
    }
    let active_buffer = if live_screen.alternate_screen() {
        PtyTerminalActiveBuffer::Alternate
    } else {
        PtyTerminalActiveBuffer::Normal
    };
    let active_screen_has_text = screen_has_text(&live_screen);
    let active_bottom_line_has_text = bottom_line_has_text(&live_screen);

    let (mut normal, alternate, entry_mode, replay_stage, normal_screen_included) =
        match active_buffer {
            PtyTerminalActiveBuffer::Normal => (
                encode_screen_rows(live_screen, include_history)?,
                None,
                None,
                if include_history {
                    PtyTerminalReplayStage::SemanticHistory
                } else {
                    PtyTerminalReplayStage::ScreenOnlyHistoryDisabled
                },
                true,
            ),
            PtyTerminalActiveBuffer::Alternate => {
                let alternate = encode_screen_rows(live_screen, false)?;
                if checkpoint_reliable {
                    if let (Some(checkpoint), Some(mode)) =
                        (normal_checkpoint, alternate_entry_mode)
                    {
                        (
                            encode_screen_rows(checkpoint, include_history)?,
                            Some(alternate),
                            Some(mode),
                            if include_history {
                                PtyTerminalReplayStage::SemanticHistory
                            } else {
                                PtyTerminalReplayStage::ScreenOnlyHistoryDisabled
                            },
                            true,
                        )
                    } else {
                        (
                            EncodedScreenRows {
                                history: VecDeque::new(),
                                history_bytes: 0,
                                current: Vec::new(),
                                state: Vec::new(),
                                retained_history_rows: 0,
                                row_limit_reached: false,
                                byte_limit_reached: false,
                                omitted_predecessor_wrapped: false,
                            },
                            Some(alternate),
                            Some(PtyTerminalAlternateEntryMode::Mode47),
                            PtyTerminalReplayStage::ScreenOnlyCheckpointUnavailable,
                            false,
                        )
                    }
                } else {
                    (
                        EncodedScreenRows {
                            history: VecDeque::new(),
                            history_bytes: 0,
                            current: Vec::new(),
                            state: Vec::new(),
                            retained_history_rows: 0,
                            row_limit_reached: false,
                            byte_limit_reached: false,
                            omitted_predecessor_wrapped: false,
                        },
                        Some(alternate),
                        Some(PtyTerminalAlternateEntryMode::Mode47),
                        PtyTerminalReplayStage::ScreenOnlyCheckpointUnavailable,
                        false,
                    )
                }
            }
        };

    if !include_history {
        normal.history.clear();
        normal.history_bytes = 0;
        normal.byte_limit_reached = false;
        normal.omitted_predecessor_wrapped = false;
    }

    let alternate_required = alternate.as_ref().map_or(Some(0), |screen| {
        physical_rows_cost(&screen.current)?.checked_add(screen.state.len())
    });
    let required_without_history = physical_rows_cost(&normal.current)
        .and_then(|value| value.checked_add(normal.state.len()))
        .and_then(|value| value.checked_add(entry_mode.map_or(0, |mode| mode.entry_bytes().len())))
        .and_then(|value| value.checked_add(alternate_required?))
        .and_then(|value| value.checked_add(pending_len))
        .ok_or(PtyTerminalSeedlessReason::SeedlessReplayCapExceeded)?;
    if required_without_history > SEMANTIC_REPLAY_MAX_BYTES {
        return Err(PtyTerminalSeedlessReason::SeedlessReplayCapExceeded);
    }
    let history_budget =
        SEMANTIC_HISTORY_REPLAY_BYTES.min(SEMANTIC_REPLAY_MAX_BYTES - required_without_history);
    while normal.history_bytes > history_budget {
        let Some(omitted) = normal.history.pop_front() else {
            return Err(PtyTerminalSeedlessReason::SeedlessEncodeFailed);
        };
        normal.history_bytes = normal
            .history_bytes
            .checked_sub(
                omitted
                    .followed_cost()
                    .ok_or(PtyTerminalSeedlessReason::SeedlessEncodeFailed)?,
            )
            .ok_or(PtyTerminalSeedlessReason::SeedlessEncodeFailed)?;
        normal.byte_limit_reached = true;
        normal.omitted_predecessor_wrapped = omitted.wrapped;
    }

    let replay_len = required_without_history
        .checked_add(normal.history_bytes)
        .ok_or(PtyTerminalSeedlessReason::SeedlessReplayCapExceeded)?;
    if replay_len > SEMANTIC_REPLAY_MAX_BYTES {
        return Err(PtyTerminalSeedlessReason::SeedlessReplayCapExceeded);
    }
    let mut replay_data = Vec::new();
    replay_data
        .try_reserve_exact(replay_len)
        .map_err(|_| PtyTerminalSeedlessReason::SeedlessEncodeFailed)?;
    let history_count = normal.history.len();
    let normal_has_current = !normal.current.is_empty();
    append_physical_rows(
        &mut replay_data,
        normal.history.into_iter(),
        normal_has_current,
    );
    append_physical_rows(&mut replay_data, normal.current.into_iter(), false);
    replay_data.extend_from_slice(&normal.state);
    if let Some(mode) = entry_mode {
        replay_data.extend_from_slice(mode.entry_bytes());
    }
    if let Some(alternate) = alternate {
        append_physical_rows(&mut replay_data, alternate.current.into_iter(), false);
        replay_data.extend_from_slice(&alternate.state);
    }
    replay_data.extend_from_slice(&pending[..pending_len]);
    if replay_data.len() != replay_len {
        return Err(PtyTerminalSeedlessReason::SeedlessEncodeFailed);
    }

    let row_limit_reached = include_history && normal.row_limit_reached;
    let byte_limit_reached = include_history && normal.byte_limit_reached;
    let history_truncation_reason = match (row_limit_reached, byte_limit_reached) {
        (false, false) => PtyTerminalHistoryTruncationReason::None,
        (true, false) => PtyTerminalHistoryTruncationReason::RowLimitReached,
        (false, true) => PtyTerminalHistoryTruncationReason::ByteLimitReached,
        (true, true) => PtyTerminalHistoryTruncationReason::RowAndByteLimitReached,
    };
    Ok(PtyTerminalReplaySnapshot {
        replay_data,
        rows,
        cols,
        sequence,
        active_buffer,
        alternate_entry_mode: entry_mode,
        replay_stage,
        history_included: history_count != 0,
        history_truncated: row_limit_reached || byte_limit_reached,
        history_truncation_reason,
        history_boundary_hardened: byte_limit_reached
            && history_count != 0
            && normal.omitted_predecessor_wrapped,
        normal_screen_included,
        retained_history_rows: u32::try_from(normal.retained_history_rows)
            .map_err(|_| PtyTerminalSeedlessReason::SeedlessEncodeFailed)?,
        included_history_rows: u32::try_from(history_count)
            .map_err(|_| PtyTerminalSeedlessReason::SeedlessEncodeFailed)?,
        semantic_history_bytes: u32::try_from(normal.history_bytes)
            .map_err(|_| PtyTerminalSeedlessReason::SeedlessEncodeFailed)?,
        replay_bytes: u32::try_from(replay_len)
            .map_err(|_| PtyTerminalSeedlessReason::SeedlessEncodeFailed)?,
        pending_parser_bytes: u32::try_from(pending_len)
            .map_err(|_| PtyTerminalSeedlessReason::SeedlessEncodeFailed)?,
        active_screen_has_text,
        active_bottom_line_has_text,
        _reservation: reservation,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivationDecisionLevel {
    Debug,
    Warn,
    Error,
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
struct CapturedActivationDecision {
    level: ActivationDecisionLevel,
    message: String,
}

#[cfg(test)]
thread_local! {
    static CAPTURED_ACTIVATION_DECISIONS: std::cell::RefCell<Option<Vec<CapturedActivationDecision>>> =
        const { std::cell::RefCell::new(None) };
}

#[allow(clippy::too_many_arguments)]
fn log_terminal_attach_decision(
    level: ActivationDecisionLevel,
    session_id: Uuid,
    label: &str,
    document_epoch: u64,
    attach_generation: u32,
    snapshot: Option<&PtyTerminalReplaySnapshot>,
    reason: Option<&str>,
    clone_micros: u64,
    lock_micros: u64,
    encode_micros: u64,
    activation_micros: u64,
    resources: ReplayResourceSnapshot,
) {
    let sequence = snapshot.map_or(0, |value| value.sequence);
    let rows = snapshot.map_or(0, |value| value.rows);
    let cols = snapshot.map_or(0, |value| value.cols);
    let replay_bytes = snapshot.map_or(0, |value| value.replay_bytes);
    let history_rows = snapshot.map_or(0, |value| value.included_history_rows);
    let pending_bytes = snapshot.map_or(0, |value| value.pending_parser_bytes);
    let stage = snapshot.map_or("none", |value| match value.replay_stage {
        PtyTerminalReplayStage::SemanticHistory => "semanticHistory",
        PtyTerminalReplayStage::ScreenOnlyHistoryDisabled => "screenOnlyHistoryDisabled",
        PtyTerminalReplayStage::ScreenOnlyCheckpointUnavailable => {
            "screenOnlyCheckpointUnavailable"
        }
    });
    let active = snapshot.map_or("none", |value| match value.active_buffer {
        PtyTerminalActiveBuffer::Normal => "normal",
        PtyTerminalActiveBuffer::Alternate => "alternate",
    });
    let reason = reason.unwrap_or("none");
    let message = format!(
        "[terminal-snapshot] event=terminal_attach_backend stage=activation_decision session={session_id} label={label:?} epoch={document_epoch} generation={attach_generation} reason={reason} sequence={sequence} rows={rows} cols={cols} active={active} replay_stage={stage} replay_bytes={replay_bytes} history_rows={history_rows} pending_bytes={pending_bytes} clone_us={clone_micros} lock_us={lock_micros} encode_us={encode_micros} activation_us={activation_micros} resource_sessions={} resource_steady_bytes={} resource_checkpoint_bytes={} resource_attach_bytes={}",
        resources.sessions,
        resources.steady_bytes,
        resources.checkpoint_bytes,
        resources.attach_bytes,
    );
    #[cfg(test)]
    CAPTURED_ACTIVATION_DECISIONS.with(|captured| {
        if let Some(records) = captured.borrow_mut().as_mut() {
            records.push(CapturedActivationDecision {
                level,
                message: message.clone(),
            });
        }
    });
    match level {
        ActivationDecisionLevel::Debug => log::debug!("{message}"),
        ActivationDecisionLevel::Warn => log::warn!("{message}"),
        ActivationDecisionLevel::Error => log::error!("{message}"),
    }
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
                let text = cell.contents().to_string();
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

struct AttachmentFlush {
    accumulator: Arc<Mutex<SessionAccumulator>>,
    labels: Vec<WindowLabel>,
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
    fn attach(&self, session_id: Uuid, label: &str) -> Option<AttachmentFlush> {
        let mut state = self.lock_state();
        let pending = state.accumulators.get(&session_id).map(Arc::clone);
        let labels = Self::labels_of(&state, session_id);
        state
            .attached
            .entry(session_id)
            .or_default()
            .insert(label.to_string());
        drop(state);
        pending.map(|accumulator| AttachmentFlush {
            accumulator,
            labels,
        })
    }

    fn flush_attachment(flush: Option<AttachmentFlush>) {
        if let Some(flush) = flush {
            Self::lock_accumulator(&flush.accumulator).flush(&flush.labels);
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

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct SemanticActivationTiming {
    clone_micros: u64,
    lock_micros: u64,
    encode_micros: u64,
    activation_micros: u64,
}

#[cfg(test)]
#[derive(Clone)]
struct SemanticCaptureBarrier {
    captured: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

impl SessionIoFanout {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    ) -> Self {
        #[cfg(not(test))]
        let replay_budget = process_replay_budget();
        // Unit backends run concurrently in one process and must not consume one
        // another's production admission allowance. Shared-budget behavior is
        // exercised explicitly below with an injected isolated budget.
        #[cfg(test)]
        let replay_budget = ReplayResourceBudget::new();
        Self::new_with_budget(output_senders, idle_detector, ws_broadcaster, replay_budget)
    }

    #[cfg(test)]
    pub(crate) fn new_with_isolated_replay_budget(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    ) -> Self {
        Self::new_with_budget(
            output_senders,
            idle_detector,
            ws_broadcaster,
            ReplayResourceBudget::new(),
        )
    }

    fn new_with_budget(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        replay_budget: Arc<ReplayResourceBudget>,
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
            replay_budget,
            #[cfg(test)]
            trace: Arc::new(Mutex::new(Vec::new())),
            #[cfg(test)]
            activation_timings: Arc::new(Mutex::new(Vec::new())),
            #[cfg(test)]
            capture_barrier: Arc::new(Mutex::new(None)),
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
        let grid_valid = valid_semantic_grid(rows, cols);
        let steady_bytes = grid_valid
            .then(|| semantic_cell_storage_bytes(rows, cols))
            .flatten();
        let mut semantic_reservation =
            steady_bytes.and_then(|bytes| self.replay_budget.try_admit(bytes));
        let mut semantic_unavailable = if !grid_valid {
            Some(PtyTerminalSeedlessReason::SeedlessInvalidGrid)
        } else if semantic_reservation.is_none() {
            Some(PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded)
        } else {
            None
        };
        let (parser_rows, parser_cols) = if grid_valid { (rows, cols) } else { (1, 1) };
        let scrollback = if semantic_reservation.is_some() {
            SEMANTIC_SCROLLBACK_ROWS
        } else {
            0
        };
        let parser = match crate::logging::catch_payload_unwind(|| {
            vt100::Parser::new(parser_rows, parser_cols, scrollback)
        }) {
            Ok(parser) => parser,
            Err(_) => {
                semantic_reservation = None;
                semantic_unavailable = Some(PtyTerminalSeedlessReason::SeedlessCaptureFailed);
                vt100::Parser::new(1, 1, 0)
            }
        };
        let replay = ScreenReplayState {
            parser,
            output_sequence: 0,
            registration,
            reader_gate: ReaderOperationGate::new(),
            parser_availability: ParserAvailability::Available,
            semantic_unavailable,
            tracker: ReplayBoundaryTracker::new(),
            normal_checkpoint: None,
            semantic_reservation,
            poison_warned: false,
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
        let parsers = self
            .screen_parsers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
    fn take_activation_timings_for_test(&self) -> Vec<SemanticActivationTiming> {
        std::mem::take(
            &mut *self
                .activation_timings
                .lock()
                .expect("activation timing recorder"),
        )
    }

    #[cfg(test)]
    fn install_capture_barrier_for_test(
        &self,
        captured: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
    ) {
        *self.capture_barrier.lock().expect("capture barrier") =
            Some(SemanticCaptureBarrier { captured, release });
    }

    #[cfg(test)]
    fn clear_capture_barrier_for_test(&self) {
        *self.capture_barrier.lock().expect("capture barrier") = None;
    }

    #[cfg(test)]
    fn wait_at_capture_barrier_for_test(&self) {
        let barrier = self
            .capture_barrier
            .lock()
            .expect("capture barrier")
            .clone();
        if let Some(barrier) = barrier {
            barrier.captured.wait();
            barrier.release.wait();
        }
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

        let (accumulated, poison_transition, semantic_transition) = {
            let (mut parsers, parser_lock_poisoned) = match self.screen_parsers.lock() {
                Ok(parsers) => (parsers, false),
                Err(error) => (error.into_inner(), true),
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
            let mut poison_transition = false;
            let mut semantic_transition = None;
            if parser_lock_poisoned {
                poison_transition = !state.poison_warned;
                state.poison_warned = true;
                state.parser_availability = ParserAvailability::Unavailable;
                if state
                    .mark_semantic_unavailable(PtyTerminalSeedlessReason::SeedlessParserPoisoned)
                {
                    semantic_transition = Some(PtyTerminalSeedlessReason::SeedlessParserPoisoned);
                }
            }
            let (sequence, parser_fault) = match state.parser_availability {
                ParserAvailability::Unavailable => (None, semantic_transition.is_some()),
                ParserAvailability::Available => {
                    let processed = crate::logging::catch_payload_unwind(|| {
                        state.process_output(&data, &self.replay_budget);
                    });
                    match processed {
                        Ok(()) if state.semantic_unavailable.is_some() => (None, false),
                        Ok(()) if state.output_sequence >= MAX_JAVASCRIPT_SEQUENCE => {
                            let transitioned = state.mark_semantic_unavailable(
                                PtyTerminalSeedlessReason::SeedlessSequenceUnsafe,
                            );
                            if transitioned {
                                semantic_transition =
                                    Some(PtyTerminalSeedlessReason::SeedlessSequenceUnsafe);
                            }
                            (None, transitioned)
                        }
                        Ok(()) => {
                            let sequence = state.output_sequence + 1;
                            state.output_sequence = sequence;
                            #[cfg(test)]
                            self.trace(FanoutTraceEvent::ParserProcessed(sequence));
                            (Some(sequence), false)
                        }
                        Err(_) => {
                            state.parser_availability = ParserAvailability::Unavailable;
                            let transitioned = state.mark_semantic_unavailable(
                                PtyTerminalSeedlessReason::SeedlessParserUnavailable,
                            );
                            if transitioned {
                                semantic_transition =
                                    Some(PtyTerminalSeedlessReason::SeedlessParserUnavailable);
                            }
                            (None, transitioned)
                        }
                    }
                }
            };
            // Appending under the parser lock is what keeps the batch boundary atomic with the
            // sequence assignment, so no coalesced batch can straddle an attach's snapshot.
            // The emit it may ask for is run below, after the lock is dropped.
            let accumulated = if ui_open {
                self.attachments
                    .accumulate(&registration, sequence, &data, parser_fault)
            } else {
                Accumulated::Unattached
            };
            (accumulated, poison_transition, semantic_transition)
        };

        if poison_transition {
            log::warn!(
                "[terminal-snapshot] event=semantic_unavailable reason=seedlessParserPoisoned session={id}"
            );
        } else if let Some(reason) = semantic_transition {
            log::warn!(
                "[terminal-snapshot] event=semantic_unavailable reason={} session={id}",
                reason.code()
            );
        }

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

    /// Registers one output attachment at the same sequence boundary as the semantic capture.
    /// Screen cloning is the only retained-cell work under the parser mutex. Row encoding,
    /// allocation, logging, and pending-batch flush all run after that mutex is released.
    pub(crate) fn activate_terminal_output(
        &self,
        id: Uuid,
        label: &str,
        include_history: bool,
        document_epoch: u64,
        attach_generation: u32,
    ) -> Result<PtyTerminalOutputActivation, TerminalOutputAttachError> {
        let activation_started = Instant::now();
        let lock_wait_finished;
        let mut clone_micros = 0;
        let mut reconciled = false;
        let mut transitioned_unsequenced = false;
        let (capture, attachment_flush, lock_micros) = {
            let (mut parsers, poisoned) = match self.screen_parsers.lock() {
                Ok(parsers) => (parsers, false),
                Err(error) => (error.into_inner(), true),
            };
            lock_wait_finished = Instant::now();
            let Some(state) = parsers.get_mut(&id) else {
                drop(parsers);
                log_terminal_attach_decision(
                    ActivationDecisionLevel::Error,
                    id,
                    label,
                    document_epoch,
                    attach_generation,
                    None,
                    Some("sessionUnavailable"),
                    0,
                    elapsed_micros(lock_wait_finished),
                    0,
                    elapsed_micros(activation_started),
                    self.replay_budget.snapshot(),
                );
                return Err(TerminalOutputAttachError::SessionUnavailable);
            };
            if state.registration.session_id != id
                || !Arc::ptr_eq(&state.registration.fanout_identity, &self.fanout_identity)
            {
                drop(parsers);
                log_terminal_attach_decision(
                    ActivationDecisionLevel::Error,
                    id,
                    label,
                    document_epoch,
                    attach_generation,
                    None,
                    Some("outputTargetUnavailable"),
                    0,
                    elapsed_micros(lock_wait_finished),
                    0,
                    elapsed_micros(activation_started),
                    self.replay_budget.snapshot(),
                );
                return Err(TerminalOutputAttachError::OutputTargetUnavailable);
            }

            let capture = if poisoned {
                if !state.poison_warned {
                    state.poison_warned = true;
                }
                state.parser_availability = ParserAvailability::Unavailable;
                transitioned_unsequenced = state
                    .mark_semantic_unavailable(PtyTerminalSeedlessReason::SeedlessParserPoisoned);
                Err(PtyTerminalSeedlessReason::SeedlessParserPoisoned)
            } else {
                let parser_grid = state.parser.screen().size();
                if parser_grid != state.conpty_size {
                    reconciled = true;
                    let (rows, cols) = state.conpty_size;
                    let before = state.semantic_unavailable;
                    let resized = crate::logging::catch_payload_unwind(|| state.resize(rows, cols));
                    match resized {
                        Ok(Ok(())) => {}
                        Ok(Err(reason)) => {
                            transitioned_unsequenced = before.is_none();
                            state.semantic_unavailable = Some(reason);
                        }
                        Err(_) => {
                            state.parser_availability = ParserAvailability::Unavailable;
                            transitioned_unsequenced = state.mark_semantic_unavailable(
                                PtyTerminalSeedlessReason::SeedlessResizeFailed,
                            );
                        }
                    }
                }
                let captured = crate::logging::catch_payload_unwind(|| {
                    state.capture_snapshot_material(include_history, &self.replay_budget)
                });
                match captured {
                    Ok(result) => result,
                    Err(_) => Err(PtyTerminalSeedlessReason::SeedlessCaptureFailed),
                }
            };
            if let Ok(material) = &capture {
                clone_micros = material.clone_micros;
            }
            let attachment_flush = self.attachments.attach(id, label);
            let lock_micros = elapsed_micros(lock_wait_finished);
            (capture, attachment_flush, lock_micros)
        };

        #[cfg(test)]
        self.wait_at_capture_barrier_for_test();

        TerminalOutputAttachments::flush_attachment(attachment_flush);
        if transitioned_unsequenced {
            self.attachments.flush(id, false);
        }

        let encode_started = Instant::now();
        let activation = match capture {
            Ok(material) => {
                let encoded =
                    crate::logging::catch_payload_unwind(|| encode_snapshot_material(material));
                match encoded {
                    Ok(Ok(snapshot)) => PtyTerminalOutputActivation::snapshot(
                        snapshot,
                        document_epoch,
                        attach_generation,
                    ),
                    Ok(Err(reason)) => PtyTerminalOutputActivation::seedless(
                        reason,
                        document_epoch,
                        attach_generation,
                    ),
                    Err(_) => PtyTerminalOutputActivation::seedless(
                        PtyTerminalSeedlessReason::SeedlessEncodeFailed,
                        document_epoch,
                        attach_generation,
                    ),
                }
            }
            Err(reason) => {
                PtyTerminalOutputActivation::seedless(reason, document_epoch, attach_generation)
            }
        };
        let encode_micros = elapsed_micros(encode_started);
        let reason = activation
            .seedless_reason
            .map(PtyTerminalSeedlessReason::code);
        let level = if reconciled
            || reason.is_some()
            || activation.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.replay_stage == PtyTerminalReplayStage::ScreenOnlyCheckpointUnavailable
            }) {
            ActivationDecisionLevel::Warn
        } else {
            ActivationDecisionLevel::Debug
        };
        log_terminal_attach_decision(
            level,
            id,
            label,
            document_epoch,
            attach_generation,
            activation.snapshot.as_ref(),
            reason,
            clone_micros,
            lock_micros,
            encode_micros,
            elapsed_micros(activation_started),
            self.replay_budget.snapshot(),
        );
        #[cfg(test)]
        self.activation_timings
            .lock()
            .expect("activation timing recorder")
            .push(SemanticActivationTiming {
                clone_micros,
                lock_micros,
                encode_micros,
                activation_micros: elapsed_micros(activation_started),
            });
        Ok(activation)
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

        let (resize_reason, transitioned_unsequenced) = {
            let (mut parsers, poisoned) = match self.screen_parsers.lock() {
                Ok(parsers) => (parsers, false),
                Err(error) => (error.into_inner(), true),
            };
            let Some(state) = parsers.get_mut(&id) else {
                drop(parsers);
                log::warn!("[terminal-snapshot] event=terminal_resize reason=noParserEntry session={id} cols={cols} rows={rows}");
                return;
            };
            // #1439 record-first: the ConPTY took this size whether or not the
            // parser can follow it; the attach reconcile compares against this.
            state.conpty_size = (rows, cols);
            if poisoned {
                state.parser_availability = ParserAvailability::Unavailable;
                let transitioned = state
                    .mark_semantic_unavailable(PtyTerminalSeedlessReason::SeedlessParserPoisoned);
                (
                    Some(PtyTerminalSeedlessReason::SeedlessParserPoisoned),
                    transitioned,
                )
            } else if state.parser_availability != ParserAvailability::Available {
                (
                    Some(PtyTerminalSeedlessReason::SeedlessParserUnavailable),
                    false,
                )
            } else {
                let was_available = state.semantic_unavailable.is_none();
                match crate::logging::catch_payload_unwind(|| state.resize(rows, cols)) {
                    Ok(Ok(())) => (None, false),
                    Ok(Err(reason)) => (Some(reason), was_available),
                    Err(_) => {
                        state.parser_availability = ParserAvailability::Unavailable;
                        let transitioned = state.mark_semantic_unavailable(
                            PtyTerminalSeedlessReason::SeedlessResizeFailed,
                        );
                        (
                            Some(PtyTerminalSeedlessReason::SeedlessResizeFailed),
                            transitioned,
                        )
                    }
                }
            }
        };
        if let Some(reason) = resize_reason {
            log::warn!(
                "[terminal-snapshot] event=terminal_resize reason={} session={id} cols={cols} rows={rows}",
                reason.code()
            );
        }
        if transitioned_unsequenced {
            self.attachments.flush(id, false);
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
            // vt100 0.16 removed the public unhandled-sequence counter. The fixed-cell copy
            // remains authoritative for the state the parser exposes, so its compatibility
            // field is now zero rather than inferred from unavailable internal state.
            let parser_errors = 0;
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
            .screen_mut()
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
        SessionIoFanout::new_with_budget(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
            ReplayResourceBudget::new(),
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

    fn attach(
        fanout: &SessionIoFanout,
        id: Uuid,
        label: &str,
    ) -> Option<PtyTerminalReplaySnapshot> {
        fanout
            .activate_terminal_output(id, label, true, 1, 1)
            .expect("attach")
            .snapshot
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

        let activation = fanout
            .activate_terminal_output(id, WINDOW, true, 1, 1)
            .expect("seedless attach");
        assert!(activation.snapshot.is_none());
        assert_eq!(
            activation.seedless_reason,
            Some(PtyTerminalSeedlessReason::SeedlessParserUnavailable)
        );
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
            fanout.activate_terminal_output(absent, WINDOW, true, 1, 1),
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
        assert!(String::from_utf8_lossy(&snapshot.replay_data).contains("before"));
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

    /// #1439. A parser grid that diverged from the grid the ConPTY last took is reconciled
    /// before the capture, so the same attach seeds at the recorded grid.
    ///
    /// Grid constraints are load-bearing: registration R0=30x120 (from `session_with_sink`),
    /// follow A=24x80, desync B=50x132 are pairwise distinct, each asymmetric, and no pair is
    /// a transposition of another. With A != R0, an implementation that never writes
    /// `conpty_size` converges onto R0 and the final grid assertion turns red; asymmetric,
    /// non-transposed grids make a rows/cols argument swap fail loudly (the #973 bug class).
    #[test]
    fn a_grid_divergence_is_reconciled_before_the_attach_seed() {
        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout.registration_token_for_test(id);

        // One follow the ConPTY took: parser and record both move to A = 24 rows x 80 cols.
        fanout.resize_screen_and_broadcast(id, 80, 24);
        fanout.handle_output(&token, &id.to_string(), b"seed me\r\n".to_vec());

        // The divergence class: the parser grid moves, the record does not.
        fanout.desync_screen_size_for_test(id, 50, 132);

        let seed = attach(&fanout, id, WINDOW).expect("reconciled attach seed");
        assert_eq!((seed.rows, seed.cols), (24, 80));
        assert_eq!(seed.sequence, 1);
        assert!(String::from_utf8_lossy(&seed.replay_data).contains("seed me"));
        // No batch entry exists yet: the only chunk so far arrived unattached.
        assert_eq!(fanout.pending_output_bytes_for_test(id), None);

        // The window is attached at the capture boundary, so later bytes keep flowing.
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

        let reseeded = attach(&fanout, id, SECOND_WINDOW).expect("second attach seed");
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
            .activate_terminal_output(id, WINDOW, include_history, 1, 1)
            .expect("attach")
            .snapshot
            .expect("snapshot on attach")
            .replay_data
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

    fn synthetic_row(bytes: usize, wrapped: bool) -> EncodedPhysicalRow {
        EncodedPhysicalRow {
            bytes: vec![b'x'; bytes],
            wrapped,
        }
    }

    #[test]
    fn semantic_history_selection_keeps_only_whole_newest_rows() {
        let mut history = VecDeque::new();
        let mut history_bytes = 0;
        let mut byte_limit = false;
        let mut hardened = false;

        push_history_row(
            &mut history,
            &mut history_bytes,
            synthetic_row(SEMANTIC_HISTORY_REPLAY_BYTES - 2, false),
            &mut byte_limit,
            &mut hardened,
        )
        .expect("exact-fit row");
        assert_eq!(history.len(), 1);
        assert_eq!(history_bytes, SEMANTIC_HISTORY_REPLAY_BYTES);
        assert!(!byte_limit);

        push_history_row(
            &mut history,
            &mut history_bytes,
            synthetic_row(SEMANTIC_HISTORY_REPLAY_BYTES - 1, false),
            &mut byte_limit,
            &mut hardened,
        )
        .expect("one-over row");
        assert!(history.is_empty());
        assert_eq!(history_bytes, 0);
        assert!(byte_limit);

        history.clear();
        history_bytes = 0;
        byte_limit = false;
        hardened = false;
        push_history_row(
            &mut history,
            &mut history_bytes,
            synthetic_row(40_000, true),
            &mut byte_limit,
            &mut hardened,
        )
        .expect("old row");
        push_history_row(
            &mut history,
            &mut history_bytes,
            synthetic_row(40_000, false),
            &mut byte_limit,
            &mut hardened,
        )
        .expect("new row");
        assert_eq!(history.len(), 1, "the oldest whole row is omitted");
        assert_eq!(history_bytes, 40_002);
        assert!(byte_limit);
        assert!(
            hardened,
            "a wrapped omitted predecessor hardens the boundary"
        );

        history.clear();
        history_bytes = 0;
        byte_limit = false;
        hardened = false;
        push_history_row(
            &mut history,
            &mut history_bytes,
            synthetic_row(SEMANTIC_HISTORY_REPLAY_BYTES + 1, false),
            &mut byte_limit,
            &mut hardened,
        )
        .expect("oversized row");
        assert!(history.is_empty());
        assert_eq!(history_bytes, 0);
        assert!(byte_limit);
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CellFingerprint {
        text: String,
        foreground: vt100::Color,
        background: vt100::Color,
        bold: bool,
        dim: bool,
        italic: bool,
        underline: bool,
        inverse: bool,
        wide: bool,
        continuation: bool,
    }

    fn replay_cell_text(cell: &vt100::Cell) -> String {
        if cell.contents().is_empty() {
            " ".to_string()
        } else {
            cell.contents().to_string()
        }
    }

    fn current_cells(screen: &vt100::Screen) -> Vec<CellFingerprint> {
        let (rows, cols) = screen.size();
        let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(cols));
        for row in 0..rows {
            for col in 0..cols {
                let cell = screen.cell(row, col).expect("cell in grid");
                cells.push(CellFingerprint {
                    text: replay_cell_text(cell),
                    foreground: cell.fgcolor(),
                    background: cell.bgcolor(),
                    bold: cell.bold(),
                    dim: cell.dim(),
                    italic: cell.italic(),
                    underline: cell.underline(),
                    inverse: cell.inverse(),
                    wide: cell.is_wide(),
                    continuation: cell.is_wide_continuation(),
                });
            }
        }
        cells
    }

    fn assert_current_screen_equivalent(expected: &vt100::Screen, actual: &vt100::Screen) {
        assert_eq!(actual.size(), expected.size());
        assert_eq!(actual.alternate_screen(), expected.alternate_screen());
        assert_eq!(actual.cursor_position(), expected.cursor_position());
        assert_eq!(actual.hide_cursor(), expected.hide_cursor());
        assert_eq!(
            actual.input_mode_formatted(),
            expected.input_mode_formatted()
        );
        assert_eq!(
            actual.attributes_formatted(),
            expected.attributes_formatted()
        );
        assert_eq!(current_cells(actual), current_cells(expected));
        let (rows, _) = expected.size();
        assert_eq!(
            (0..rows)
                .map(|row| actual.row_wrapped(row))
                .collect::<Vec<_>>(),
            (0..rows)
                .map(|row| expected.row_wrapped(row))
                .collect::<Vec<_>>()
        );
    }

    fn replay_parser(snapshot: &PtyTerminalReplaySnapshot) -> vt100::Parser {
        let mut parser = vt100::Parser::new(snapshot.rows, snapshot.cols, SEMANTIC_SCROLLBACK_ROWS);
        if snapshot.alternate_entry_mode == Some(PtyTerminalAlternateEntryMode::Mode1047) {
            // vt100 does not model DECSET 1047, while xterm does. Production preserves the
            // exact 1047 entry for xterm; this test parser needs the same 47 normalization
            // used by the production mirror.
            let entry = PtyTerminalAlternateEntryMode::Mode1047.entry_bytes();
            let at = snapshot
                .replay_data
                .windows(entry.len())
                .position(|window| window == entry)
                .expect("1047 replay entry");
            parser.process(&snapshot.replay_data[..at]);
            parser.process(b"\x1b[?47h");
            parser.process(&snapshot.replay_data[at + entry.len()..]);
        } else {
            parser.process(&snapshot.replay_data);
        }
        parser
    }

    fn physical_screen_fingerprint(
        mut screen: vt100::Screen,
    ) -> (usize, Vec<(Vec<CellFingerprint>, bool)>) {
        screen.set_scrollback(usize::MAX);
        let retained = screen.scrollback();
        let (_, cols) = screen.size();
        let row = |screen: &vt100::Screen, row: u16| {
            (0..cols)
                .map(|col| {
                    let cell = screen.cell(row, col).expect("cell in grid");
                    CellFingerprint {
                        text: replay_cell_text(cell),
                        foreground: cell.fgcolor(),
                        background: cell.bgcolor(),
                        bold: cell.bold(),
                        dim: cell.dim(),
                        italic: cell.italic(),
                        underline: cell.underline(),
                        inverse: cell.inverse(),
                        wide: cell.is_wide(),
                        continuation: cell.is_wide_continuation(),
                    }
                })
                .collect::<Vec<_>>()
        };
        let mut rows = Vec::new();
        for offset in (1..=retained).rev() {
            screen.set_scrollback(offset);
            rows.push((row(&screen, 0), screen.row_wrapped(0)));
        }
        screen.set_scrollback(0);
        for index in 0..screen.size().0 {
            rows.push((row(&screen, index), screen.row_wrapped(index)));
        }
        (retained, rows)
    }

    const SANITIZED_CODEX_CHUNKS: &[&[u8]] = &[
        b"\x1b[?25l\x1b[2J\x1b[H",
        b"\x1b[38;5;39mCodex\x1b[0m ready\r\n",
        b"\x1b]0;agent-session\x07",
        b"\x1b[?2004h\x1b[?25h",
    ];

    const SANITIZED_PI_CHUNKS: &[&[u8]] = &[
        b"\x1b[1;34mPi\x1b[0m workspace\r\n",
        b"tool result: sanitized\r\n",
        b"assistant> ",
    ];

    struct ActivationDecisionCaptureReset;

    impl Drop for ActivationDecisionCaptureReset {
        fn drop(&mut self) {
            CAPTURED_ACTIVATION_DECISIONS.with(|captured| {
                captured.borrow_mut().take();
            });
        }
    }

    fn capture_activation_decisions<T>(
        operation: impl FnOnce() -> T,
    ) -> (T, Vec<CapturedActivationDecision>) {
        let previous = CAPTURED_ACTIVATION_DECISIONS
            .with(|captured| captured.borrow_mut().replace(Vec::new()));
        assert!(
            previous.is_none(),
            "activation decision capture is not nested"
        );
        let _reset = ActivationDecisionCaptureReset;
        let result = operation();
        let records = CAPTURED_ACTIVATION_DECISIONS.with(|captured| {
            captured
                .borrow_mut()
                .take()
                .expect("active activation decision capture")
        });
        (result, records)
    }

    #[test]
    fn semantic_activation_diagnostics_are_cardinal_typed_and_content_free() {
        const CONTENT_CANARY: &[u8] =
            b"privacy-canary prompt=hidden cwd=hidden command=hidden secret=hidden";

        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[CONTENT_CANARY]);

        let (activation, records) = capture_activation_decisions(|| {
            fanout.activate_terminal_output(id, WINDOW, true, 41, 7)
        });
        let activation = activation.expect("successful diagnostic activation");
        assert_ne!(
            activation.snapshot.is_some(),
            activation.seedless_reason.is_some()
        );
        let snapshot = activation.snapshot.expect("diagnostic snapshot");
        assert!(snapshot.rows != 0 && snapshot.cols != 0);
        assert!(snapshot.sequence <= MAX_JAVASCRIPT_SEQUENCE);
        assert_eq!(snapshot.replay_bytes as usize, snapshot.replay_data.len());
        assert_eq!(records.len(), 1, "one success decision");
        assert_eq!(records[0].level, ActivationDecisionLevel::Debug);
        assert!(records[0]
            .message
            .contains("event=terminal_attach_backend stage=activation_decision"));
        assert!(records[0].message.contains(&format!("session={id}")));
        assert!(records[0].message.contains("epoch=41 generation=7"));
        for forbidden in [
            "privacy-canary",
            "data=",
            "contents=",
            "prompt=",
            "command=",
            "title=",
            "path=",
            "cwd=",
            "arguments=",
            "environment=",
            "secret=",
        ] {
            assert!(
                !records[0].message.contains(forbidden),
                "content-bearing diagnostic field {forbidden}"
            );
        }

        feed(&fanout, id, &[b"\x1b[?6h"]);
        let (seedless, records) = capture_activation_decisions(|| {
            fanout.activate_terminal_output(id, SECOND_WINDOW, true, 41, 8)
        });
        let seedless = seedless.expect("typed seedless diagnostic activation");
        assert!(seedless.snapshot.is_none());
        assert_eq!(
            seedless.seedless_reason,
            Some(PtyTerminalSeedlessReason::SeedlessContinuationUnsafe)
        );
        assert_eq!(records.len(), 1, "one seedless decision");
        assert_eq!(records[0].level, ActivationDecisionLevel::Warn);
        assert!(records[0]
            .message
            .contains("reason=seedlessContinuationUnsafe"));

        let absent = Uuid::new_v4();
        let (failure, records) = capture_activation_decisions(|| {
            fanout.activate_terminal_output(absent, WINDOW, true, 41, 9)
        });
        assert!(matches!(
            failure,
            Err(TerminalOutputAttachError::SessionUnavailable)
        ));
        assert_eq!(records.len(), 1, "one error decision");
        assert_eq!(records[0].level, ActivationDecisionLevel::Error);
        assert!(records[0].message.contains("reason=sessionUnavailable"));
    }

    #[test]
    fn semantic_normal_history_replays_into_a_fresh_parser() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 24, 81)
            .expect("register semantic session");
        feed(&fanout, id, SANITIZED_CODEX_CHUNKS);
        for index in 0..96 {
            let line = format!("semantic history {index:03}\r\n");
            feed(&fanout, id, &[line.as_bytes()]);
        }
        feed(&fanout, id, SANITIZED_PI_CHUNKS);

        let (expected, expected_sequence) = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            (state.parser.screen().clone(), state.output_sequence)
        };
        let activation = fanout
            .activate_terminal_output(id, WINDOW, true, 7, 9)
            .expect("semantic activation");
        assert_eq!(activation.document_epoch, 7);
        assert_eq!(activation.attach_generation, 9);
        assert!(activation.seedless_reason.is_none());
        let snapshot = activation.snapshot.expect("semantic snapshot");
        assert_eq!((snapshot.rows, snapshot.cols), (24, 81));
        assert_eq!(snapshot.sequence, expected_sequence);
        assert_eq!(snapshot.active_buffer, PtyTerminalActiveBuffer::Normal);
        assert_eq!(
            snapshot.replay_stage,
            PtyTerminalReplayStage::SemanticHistory
        );
        assert!(snapshot.history_included);
        assert!(snapshot.active_screen_has_text);
        assert!(snapshot.active_bottom_line_has_text);
        assert_eq!(snapshot.replay_bytes as usize, snapshot.replay_data.len());

        let replayed = replay_parser(&snapshot);
        assert_current_screen_equivalent(&expected, replayed.screen());
        assert_eq!(
            physical_screen_fingerprint(expected),
            physical_screen_fingerprint(replayed.screen().clone())
        );
    }

    #[test]
    fn semantic_row_encoding_preserves_styles_unicode_hard_lines_and_soft_wraps() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 5, 12)
            .expect("register styled session");
        feed(
            &fanout,
            id,
            &[
                b"\x1b[1;2;3;4;7;38;5;201;48;2;4;5;6m ",
                "e\u{301}界".as_bytes(),
                b"\x1b[0m hard\r\n",
                b"abcdefghijklmnopqrstuvwx",
                b"\r\nlast",
                b"\x1b[1;1H\x1b[1;2;3;4;7;38;5;201;48;2;4;5;6m ",
                b"\x1b[1;2H\x1b[1mB\x1b[0m",
            ],
        );

        let expected = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            parsers
                .get(&id)
                .expect("registered session")
                .parser
                .screen()
                .clone()
        };
        assert!(current_cells(&expected).iter().any(|cell| {
            cell.text.chars().all(char::is_whitespace)
                && cell.foreground == vt100::Color::Idx(201)
                && cell.background == vt100::Color::Rgb(4, 5, 6)
                && cell.dim
                && cell.italic
                && cell.underline
                && cell.inverse
        }));
        assert!(current_cells(&expected).iter().any(|cell| cell.bold));
        assert!(current_cells(&expected).iter().any(|cell| cell.wide));
        assert!(current_cells(&expected)
            .iter()
            .any(|cell| cell.continuation));
        assert!((0..expected.size().0).any(|row| expected.row_wrapped(row)));

        let snapshot = fanout
            .activate_terminal_output(id, WINDOW, true, 1, 1)
            .expect("activation")
            .snapshot
            .expect("snapshot");
        let replayed = replay_parser(&snapshot);
        assert_current_screen_equivalent(&expected, replayed.screen());
        assert!(snapshot.replay_data.starts_with(b"\x1b[0m"));
    }

    fn alternate_round_trip(mode: PtyTerminalAlternateEntryMode) {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 6, 20)
            .expect("register alternate session");
        for index in 0..12 {
            let line = format!("normal {index:02}\r\n");
            feed(&fanout, id, &[line.as_bytes()]);
        }
        feed(&fanout, id, &[b"\x1b[3;5H\x1b[33mN"]);
        feed(&fanout, id, &[mode.entry_bytes()]);
        feed(&fanout, id, SANITIZED_PI_CHUNKS);
        feed(&fanout, id, &[b"\x1b[2;4H\x1b[1;35mALT"]);

        let expected_alternate = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            assert!(state.normal_checkpoint.is_some());
            state.parser.screen().clone()
        };
        let snapshot = fanout
            .activate_terminal_output(id, WINDOW, true, 11, 12)
            .expect("alternate activation")
            .snapshot
            .expect("alternate snapshot");
        assert_eq!(snapshot.active_buffer, PtyTerminalActiveBuffer::Alternate);
        assert_eq!(snapshot.alternate_entry_mode, Some(mode));
        assert!(snapshot.normal_screen_included);
        assert_eq!(
            snapshot.replay_stage,
            PtyTerminalReplayStage::SemanticHistory
        );

        let mut replayed = replay_parser(&snapshot);
        assert_current_screen_equivalent(&expected_alternate, replayed.screen());
        let exit = match mode {
            PtyTerminalAlternateEntryMode::Mode47 => b"\x1b[?47l".as_slice(),
            PtyTerminalAlternateEntryMode::Mode1047 => b"\x1b[?1047l".as_slice(),
            PtyTerminalAlternateEntryMode::Mode1049 => b"\x1b[?1049l".as_slice(),
        };
        feed(&fanout, id, &[exit]);
        let expected_normal = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            parsers.get(&id).unwrap().parser.screen().clone()
        };
        if mode == PtyTerminalAlternateEntryMode::Mode1047 {
            // vt100 normalizes 1047 entry but does not model its exit like xterm. The
            // production tracker uses this same parser-only 47 normalization.
            replayed.process(b"\x1b[2J\x1b[?47l");
        } else {
            replayed.process(exit);
        }
        assert_current_screen_equivalent(&expected_normal, replayed.screen());
        assert_eq!(
            physical_screen_fingerprint(expected_normal),
            physical_screen_fingerprint(replayed.screen().clone())
        );
    }

    #[test]
    fn semantic_alternate_entry_modes_restore_normal_state() {
        for mode in [
            PtyTerminalAlternateEntryMode::Mode47,
            PtyTerminalAlternateEntryMode::Mode1047,
            PtyTerminalAlternateEntryMode::Mode1049,
        ] {
            alternate_round_trip(mode);
        }
    }

    fn seedless_reason(fanout: &SessionIoFanout, id: Uuid) -> PtyTerminalSeedlessReason {
        let activation = fanout
            .activate_terminal_output(id, WINDOW, true, 1, 1)
            .expect("seedless activation");
        assert!(activation.snapshot.is_none());
        activation.seedless_reason.expect("seedless reason")
    }

    #[test]
    fn semantic_continuation_safety_is_typed_and_exact_resets_recover() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"\x1b[2;20r", b"\x1b[r"),
            (b"\x1b[?6h", b"\x1b[?6l"),
            (b"\x1b[4h", b"\x1b[4l"),
            (b"\x1b[?7l", b"\x1b[?7h"),
            (b"\x1b(A", b"\x1b(B"),
            (b"\x0e", b"\x0f"),
            (b"\x1b%@", b"\x1b%G"),
        ];
        for (unsafe_sequence, reset) in cases {
            let fanout = fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[*unsafe_sequence]);
            assert_eq!(
                seedless_reason(&fanout, id),
                PtyTerminalSeedlessReason::SeedlessContinuationUnsafe
            );
            feed(&fanout, id, &[*reset]);
            assert!(fanout
                .activate_terminal_output(id, SECOND_WINDOW, true, 1, 2)
                .expect("recovered activation")
                .snapshot
                .is_some());
        }

        for unsafe_sequence in [
            b"\x1bH".as_slice(),
            b"\x1b[?7777h".as_slice(),
            b"\x1b[3g".as_slice(),
            b"\x1bPopaque".as_slice(),
            b"\x1b7\x1b8".as_slice(),
            b"\x1b[s\x1b[u".as_slice(),
        ] {
            let fanout = fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[unsafe_sequence]);
            assert_eq!(
                seedless_reason(&fanout, id),
                PtyTerminalSeedlessReason::SeedlessContinuationUnsafe
            );
            feed(&fanout, id, &[b"\x1b\\\x1bc"]);
            assert!(fanout
                .activate_terminal_output(id, SECOND_WINDOW, true, 1, 2)
                .expect("RIS recovery")
                .snapshot
                .is_some());
        }

        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b)0\x1b(B"]);
        assert_eq!(
            seedless_reason(&fanout, id),
            PtyTerminalSeedlessReason::SeedlessContinuationUnsafe
        );
        feed(&fanout, id, &[b"\x1b)B"]);
        assert!(fanout
            .activate_terminal_output(id, SECOND_WINDOW, true, 1, 2)
            .expect("all charset registers reset")
            .snapshot
            .is_some());
    }

    #[test]
    fn semantic_feed_differential_future_state_classes_match_or_are_seedless() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"\x1b[2;20r\x1b[?6h", b"\x1b[1;1Hmargin-origin"),
            (b"\x1b[4h\x1b[?7l", b"insert-autowrap"),
            (b"\x1b[1;3;4m", b"attributes"),
            (b"\x1b[?2004h", b"input-mode"),
            (b"\x1b7", b"saved-cursor"),
            (b"\x1b(0\x0e", b"charset"),
            (b"\x1b[3g", b"tabs"),
        ];
        for (prefix, suffix) in cases {
            let fanout = fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[prefix]);
            let activation = fanout.activate_terminal_output(id, WINDOW, true, 1, 1);
            if activation
                .as_ref()
                .expect("activation decision")
                .seedless_reason
                == Some(PtyTerminalSeedlessReason::SeedlessContinuationUnsafe)
            {
                feed(&fanout, id, &[suffix]);
                continue;
            }
            let snapshot = activation
                .expect("snapshot decision")
                .snapshot
                .expect("safe class snapshot");
            let mut replayed = replay_parser(&snapshot);
            feed(&fanout, id, &[suffix]);
            let expected = {
                let parsers = fanout.screen_parsers.lock().expect("parser state");
                parsers
                    .get(&id)
                    .expect("session parser")
                    .parser
                    .screen()
                    .clone()
            };
            replayed.process(suffix);
            assert_current_screen_equivalent(&expected, replayed.screen());
        }
    }

    /// The issue regression: a background session returns semantic history, not one viewport.
    #[test]
    fn semantic_activation_replays_background_history() {
        let fanout = fanout();
        let foreground = session(&fanout);
        let background = session(&fanout);
        activation_data(&fanout, foreground, false);

        for index in 0..200 {
            let line = format!("history line {index}\r\n");
            feed(&fanout, background, &[line.as_bytes()]);
        }

        let mirror = String::from_utf8_lossy(&mirrored_screen(&fanout, background)).into_owned();
        assert!(!mirror.contains("history line 0\r\n"));

        let activation = fanout
            .activate_terminal_output(background, WINDOW, true, 1, 1)
            .expect("background activation");
        let snapshot = activation.snapshot.expect("background snapshot");
        assert!(snapshot.history_included);
        assert!(snapshot.included_history_rows > 0);
        let replayed = String::from_utf8_lossy(&snapshot.replay_data);
        assert!(replayed.contains("history line 0"));
        assert!(replayed.contains("history line 199"));
    }

    #[test]
    fn semantic_activation_with_empty_history_replays_the_current_screen() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"current"]);
        let expected = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            parsers.get(&id).unwrap().parser.screen().clone()
        };

        let snapshot = fanout
            .activate_terminal_output(id, WINDOW, true, 1, 1)
            .expect("activation")
            .snapshot
            .expect("snapshot");
        assert!(!snapshot.history_included);
        assert_eq!(snapshot.included_history_rows, 0);
        assert_current_screen_equivalent(&expected, replay_parser(&snapshot).screen());
    }

    #[test]
    fn semantic_history_disabled_reports_screen_only_without_truncation() {
        let fanout = fanout();
        let id = session(&fanout);
        for index in 0..200 {
            let line = format!("history line {index}\r\n");
            feed(&fanout, id, &[line.as_bytes()]);
        }
        let expected = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            parsers.get(&id).unwrap().parser.screen().clone()
        };
        let snapshot = fanout
            .activate_terminal_output(id, WINDOW, false, 1, 1)
            .expect("history-disabled activation")
            .snapshot
            .expect("history-disabled snapshot");
        assert_eq!(
            snapshot.replay_stage,
            PtyTerminalReplayStage::ScreenOnlyHistoryDisabled
        );
        assert!(!snapshot.history_included);
        assert!(!snapshot.history_truncated);
        assert_eq!(
            snapshot.history_truncation_reason,
            PtyTerminalHistoryTruncationReason::None
        );
        assert_eq!(snapshot.included_history_rows, 0);
        assert_current_screen_equivalent(&expected, replay_parser(&snapshot).screen());
    }

    #[test]
    fn semantic_pending_prefix_handoff_completes_for_seven_bit_and_c1_csi() {
        let candidates = [
            b"\x1b[?1049h".to_vec(),
            vec![0x9b, b'?', b'1', b'0', b'4', b'9', b'h'],
        ];
        for candidate in candidates {
            for split in 1..candidate.len() {
                let fanout = fanout();
                let id = Uuid::new_v4();
                fanout
                    .register_session_for_test(id, IdleTuning::DEFAULT, 4, 12)
                    .expect("register prefix session");
                feed(&fanout, id, &[b"normal"]);
                for byte in &candidate[..split] {
                    feed(&fanout, id, &[std::slice::from_ref(byte)]);
                }
                let snapshot = fanout
                    .activate_terminal_output(id, WINDOW, true, 3, 4)
                    .expect("prefix activation")
                    .snapshot
                    .expect("prefix snapshot");
                assert_eq!(snapshot.pending_parser_bytes as usize, split);
                assert_eq!(
                    &snapshot.replay_data[snapshot.replay_data.len() - split..],
                    &candidate[..split]
                );

                let mut replayed = if candidate[0] == 0x9b {
                    // vt100 does not consume C1 CSI. Preserve and assert the original C1
                    // payload above, then normalize only this differential test parser.
                    let mut parser =
                        vt100::Parser::new(snapshot.rows, snapshot.cols, SEMANTIC_SCROLLBACK_ROWS);
                    parser.process(
                        &snapshot.replay_data
                            [..snapshot.replay_data.len() - snapshot.pending_parser_bytes as usize],
                    );
                    parser.process(b"\x1b[");
                    parser.process(&candidate[1..split]);
                    parser
                } else {
                    replay_parser(&snapshot)
                };
                replayed.process(&candidate[split..]);
                feed(&fanout, id, &[&candidate[split..]]);
                let expected = {
                    let parsers = fanout.screen_parsers.lock().expect("parser state");
                    parsers.get(&id).unwrap().parser.screen().clone()
                };
                assert_current_screen_equivalent(&expected, replayed.screen());
            }
        }
    }

    #[test]
    fn semantic_tracker_handles_mixed_modes_exit_entry_ris_and_disagreement() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 5, 16)
            .expect("register tracker session");
        feed(&fanout, id, &[b"normal\x1b[?25l"]);
        for byte in b"\x1b[?1049;2004h" {
            feed(&fanout, id, &[std::slice::from_ref(byte)]);
        }
        feed(&fanout, id, &[b"alternate"]);
        let first = fanout
            .activate_terminal_output(id, WINDOW, true, 1, 1)
            .expect("mixed activation")
            .snapshot
            .expect("mixed snapshot");
        assert_eq!(
            first.alternate_entry_mode,
            Some(PtyTerminalAlternateEntryMode::Mode1049)
        );

        feed(&fanout, id, &[b"\x1b[?1049l\x1b[2;2Hbetween\x1b[?47h"]);
        let second = fanout
            .activate_terminal_output(id, SECOND_WINDOW, true, 1, 2)
            .expect("exit-entry activation")
            .snapshot
            .expect("exit-entry snapshot");
        assert_eq!(
            second.alternate_entry_mode,
            Some(PtyTerminalAlternateEntryMode::Mode47)
        );
        assert!(second.normal_screen_included);

        feed(&fanout, id, &[b"\x1bc"]);
        let reset = fanout
            .activate_terminal_output(id, WINDOW, true, 1, 3)
            .expect("RIS activation")
            .snapshot
            .expect("RIS snapshot");
        assert_eq!(reset.active_buffer, PtyTerminalActiveBuffer::Normal);
        assert_eq!(reset.alternate_entry_mode, None);

        {
            let mut parsers = fanout.screen_parsers.lock().expect("parser state");
            parsers.get_mut(&id).unwrap().parser.process(b"\x1b[?1049h");
        }
        feed(&fanout, id, &[b"desync"]);
        let fallback = fanout
            .activate_terminal_output(id, SECOND_WINDOW, true, 1, 4)
            .expect("disagreement activation")
            .snapshot
            .expect("screen-only fallback");
        assert_eq!(fallback.active_buffer, PtyTerminalActiveBuffer::Alternate);
        assert_eq!(
            fallback.replay_stage,
            PtyTerminalReplayStage::ScreenOnlyCheckpointUnavailable
        );
        assert_eq!(
            fallback.alternate_entry_mode,
            Some(PtyTerminalAlternateEntryMode::Mode47)
        );
        assert!(!fallback.normal_screen_included);
    }

    #[test]
    fn semantic_tracker_rejects_malformed_and_overlong_controls_until_ris() {
        for hostile in [
            b"\x1b[?1049:h".to_vec(),
            [
                b"\x1b[?".as_slice(),
                vec![b'1'; ALT_SEQUENCE_MAX_BYTES].as_slice(),
            ]
            .concat(),
        ] {
            let fanout = fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[&hostile]);
            assert_eq!(
                seedless_reason(&fanout, id),
                PtyTerminalSeedlessReason::SeedlessContinuationUnsafe
            );
            feed(&fanout, id, &[b"m\x1bc"]);
            assert!(fanout
                .activate_terminal_output(id, SECOND_WINDOW, true, 1, 2)
                .expect("RIS recovery")
                .snapshot
                .is_some());
        }
    }

    #[test]
    fn semantic_missing_checkpoint_uses_the_typed_screen_only_fallback() {
        let fanout = fanout();
        let blocker = fanout
            .replay_budget
            .try_reserve(
                ReplayResourceKind::Checkpoint,
                SEMANTIC_CHECKPOINT_BUDGET_BYTES,
            )
            .expect("checkpoint budget blocker");
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 8, 24)
            .expect("register checkpoint session");
        feed(&fanout, id, &[b"normal\x1b[?1049halt"]);
        let snapshot = fanout
            .activate_terminal_output(id, WINDOW, true, 1, 1)
            .expect("fallback activation")
            .snapshot
            .expect("fallback snapshot");
        assert_eq!(
            snapshot.replay_stage,
            PtyTerminalReplayStage::ScreenOnlyCheckpointUnavailable
        );
        assert!(!snapshot.normal_screen_included);
        assert_eq!(
            snapshot.alternate_entry_mode,
            Some(PtyTerminalAlternateEntryMode::Mode47)
        );
        drop(blocker);
        assert_eq!(fanout.replay_budget.snapshot().checkpoint_bytes, 0);
    }

    #[test]
    fn semantic_resource_budget_admission_resize_and_raii_are_bounded() {
        assert!(Arc::ptr_eq(
            &process_replay_budget(),
            &process_replay_budget()
        ));
        let budget = ReplayResourceBudget::new();
        for (kind, limit) in [
            (
                ReplayResourceKind::Sessions,
                SUPPORTED_SEMANTIC_REPLAY_SESSIONS,
            ),
            (ReplayResourceKind::Steady, SEMANTIC_STEADY_BUDGET_BYTES),
            (
                ReplayResourceKind::Checkpoint,
                SEMANTIC_CHECKPOINT_BUDGET_BYTES,
            ),
            (ReplayResourceKind::Attach, SEMANTIC_ATTACH_BUDGET_BYTES),
        ] {
            let mut exact = budget.try_reserve(kind, limit).expect("exact reservation");
            assert!(budget.try_reserve(kind, 1).is_none());
            assert!(exact.try_resize(limit));
            assert!(exact.try_resize(limit - 1));
            assert!(exact.try_resize(limit));
            drop(exact);
        }
        let empty = budget.snapshot();
        assert_eq!(empty.sessions, 0);
        assert_eq!(empty.steady_bytes, 0);
        assert_eq!(empty.checkpoint_bytes, 0);
        assert_eq!(empty.attach_bytes, 0);

        let first = SessionIoFanout::new_with_budget(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
            Arc::clone(&budget),
        );
        let second = SessionIoFanout::new_with_budget(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
            Arc::clone(&budget),
        );
        let mut shared_ids = Vec::new();
        for index in 0..SUPPORTED_SEMANTIC_REPLAY_SESSIONS {
            let id = Uuid::new_v4();
            let target = if index % 2 == 0 { &first } else { &second };
            target
                .register_session_for_test(id, IdleTuning::DEFAULT, 27, 81)
                .expect("shared-budget admitted session");
            shared_ids.push((index % 2 == 0, id));
        }
        let shared_refused = Uuid::new_v4();
        second
            .register_session_for_test(shared_refused, IdleTuning::DEFAULT, 27, 81)
            .expect("shared-budget live-only session");
        assert_eq!(
            seedless_reason(&second, shared_refused),
            PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded
        );
        for (in_first, id) in shared_ids {
            if in_first {
                first.remove_session(id);
            } else {
                second.remove_session(id);
            }
        }
        second.remove_session(shared_refused);
        assert_eq!(budget.snapshot().sessions, 0);

        let fanout = SessionIoFanout::new_with_budget(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
            Arc::clone(&budget),
        );
        let mut ids = Vec::new();
        for _ in 0..SUPPORTED_SEMANTIC_REPLAY_SESSIONS {
            let id = Uuid::new_v4();
            fanout
                .register_session_for_test(id, IdleTuning::DEFAULT, 27, 81)
                .expect("admitted session");
            ids.push(id);
        }
        let refused = Uuid::new_v4();
        fanout
            .register_session_for_test(refused, IdleTuning::DEFAULT, 27, 81)
            .expect("live-only session");
        assert_eq!(
            seedless_reason(&fanout, refused),
            PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded
        );
        assert_eq!(
            budget.snapshot().sessions,
            SUPPORTED_SEMANTIC_REPLAY_SESSIONS
        );

        fanout.remove_session(ids.remove(0));
        let replacement = Uuid::new_v4();
        fanout
            .register_session_for_test(replacement, IdleTuning::DEFAULT, 80, 240)
            .expect("wide replacement session");
        let snapshot = fanout
            .activate_terminal_output(replacement, SECOND_WINDOW, true, 1, 2)
            .expect("replacement activation")
            .snapshot
            .expect("replacement snapshot");
        assert_eq!((snapshot.rows, snapshot.cols), (80, 240));
        assert!(budget.snapshot().attach_bytes > 0);
        drop(snapshot);
        assert_eq!(budget.snapshot().attach_bytes, 0);

        for id in ids.into_iter().chain([refused, replacement]) {
            fanout.remove_session(id);
        }
        let released = budget.snapshot();
        assert_eq!(released.sessions, 0);
        assert_eq!(released.steady_bytes, 0);
        assert_eq!(released.checkpoint_bytes, 0);
    }

    #[test]
    fn semantic_invalid_huge_and_resource_refused_grids_fail_closed() {
        for (rows, cols) in [(0, 80), (24, 0), (u16::MAX, u16::MAX)] {
            let fanout = fanout();
            let id = Uuid::new_v4();
            fanout
                .register_session_for_test(id, IdleTuning::DEFAULT, rows, cols)
                .expect("live-only invalid-grid session");
            assert_eq!(
                seedless_reason(&fanout, id),
                PtyTerminalSeedlessReason::SeedlessInvalidGrid
            );
        }

        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 8, 24)
            .expect("register resize session");
        let current = fanout.replay_budget.snapshot().steady_bytes;
        let blocker = fanout
            .replay_budget
            .try_reserve(
                ReplayResourceKind::Steady,
                SEMANTIC_STEADY_BUDGET_BYTES - current,
            )
            .expect("fill steady budget");
        fanout.resize_screen_and_broadcast(id, 24, 9);
        assert_eq!(
            seedless_reason(&fanout, id),
            PtyTerminalSeedlessReason::SeedlessResourceLimitExceeded
        );
        drop(blocker);
        fanout.remove_session(id);
        assert_eq!(fanout.replay_budget.snapshot().steady_bytes, 0);
    }

    #[test]
    fn semantic_resize_keeps_alternate_and_checkpoint_grids_in_sync() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 24, 81)
            .expect("register resize session");
        for index in 0..40 {
            let line = format!("resize history {index}\r\n");
            feed(&fanout, id, &[line.as_bytes()]);
        }
        feed(&fanout, id, &[b"\x1b[?1049halt"]);

        for rows in [27, 24] {
            fanout.resize_screen_and_broadcast(id, 81, rows);
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).unwrap();
            assert_eq!(state.parser.screen().size(), (rows, 81));
            assert_eq!(
                state.normal_checkpoint.as_ref().unwrap().screen.size(),
                (rows, 81)
            );
            drop(parsers);
            let snapshot = fanout
                .activate_terminal_output(id, WINDOW, true, 1, u32::from(rows))
                .expect("resized activation")
                .snapshot
                .expect("resized snapshot");
            assert_eq!((snapshot.rows, snapshot.cols), (rows, 81));
        }
    }

    #[test]
    fn semantic_sequence_and_parser_poison_preserve_original_live_bytes() {
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let broadcaster = crate::web::broadcast::WsBroadcaster::new();
        let mut websocket = broadcaster.subscribe();
        let sink = new_sink();
        let poisoned_fanout = SessionIoFanout::new_with_budget(
            Arc::clone(&output_senders),
            IdleDetector::new(|_| {}, |_| {}),
            Some(broadcaster),
            ReplayResourceBudget::new(),
        );
        let id = Uuid::new_v4();
        let token = poisoned_fanout
            .register_session(
                id,
                IdleTuning::DEFAULT,
                8,
                24,
                PtyOutputTarget::from_test_sink(Arc::clone(&sink)),
            )
            .expect("register poison session");
        output_senders.lock().unwrap().insert(id, sender);
        poisoned_fanout.poison_screen_parsers_for_test();
        let activation = poisoned_fanout
            .activate_terminal_output(id, WINDOW, true, 1, 1)
            .expect("poisoned activation");
        assert_eq!(
            activation.seedless_reason,
            Some(PtyTerminalSeedlessReason::SeedlessParserPoisoned)
        );
        let poison_bytes = b"poison-live-original".to_vec();
        poisoned_fanout.handle_output(&token, &id.to_string(), poison_bytes.clone());
        flush(&poisoned_fanout, id);
        assert_eq!(events(&sink)[0].1, poison_bytes);
        assert_eq!(events(&sink)[0].2, None);
        assert_eq!(receiver.try_recv().expect("raw output"), poison_bytes);
        match websocket.try_recv().expect("websocket output") {
            crate::web::broadcast::WsOutMsg::Binary(frame) => {
                assert_eq!(&frame[36..], poison_bytes.as_slice());
            }
            other => panic!("expected websocket output, got {other:?}"),
        }

        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let token = fanout.registration_token_for_test(id);
        attach(&fanout, id, WINDOW);
        fanout.exhaust_output_sequence_for_test(id);
        fanout.handle_output(&token, &id.to_string(), b"sequence-live".to_vec());
        flush(&fanout, id);
        assert_eq!(events(&sink)[0].2, None);
        assert_eq!(
            seedless_reason(&fanout, id),
            PtyTerminalSeedlessReason::SeedlessSequenceUnsafe
        );
    }

    #[test]
    fn semantic_replay_cap_never_truncates_the_authoritative_viewport() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 80, 240)
            .expect("register wide session");
        let mut styled = Vec::new();
        for index in 0..(80usize * 240) {
            if index % 2 == 0 {
                styled.extend_from_slice(b"\x1b[1;3;4;7;38;2;250;251;252;48;2;100;101;102mX");
            } else {
                styled.extend_from_slice(b"\x1b[2;38;2;1;2;3;48;2;240;241;242mY");
            }
        }
        feed(&fanout, id, &[&styled]);
        assert_eq!(
            seedless_reason(&fanout, id),
            PtyTerminalSeedlessReason::SeedlessReplayCapExceeded
        );
        assert_eq!(fanout.replay_budget.snapshot().attach_bytes, 0);
        assert_eq!(fanout.get_pty_size(id), Some((80, 240)));
    }

    #[test]
    fn semantic_snapshot_output_boundary_has_no_loss_or_duplication() {
        for iteration in 0..64 {
            let fanout = fanout();
            let sink = new_sink();
            let id = session_with_sink(&fanout, &sink);
            let token = fanout.registration_token_for_test(id);
            let marker = format!("boundary-{iteration:02}").into_bytes();
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let activation = std::thread::scope(|scope| {
                let activation_fanout = fanout.clone();
                let activation_barrier = Arc::clone(&barrier);
                let activation = scope.spawn(move || {
                    activation_barrier.wait();
                    activation_fanout
                        .activate_terminal_output(id, WINDOW, true, 1, 1)
                        .expect("racing activation")
                });
                let output_fanout = fanout.clone();
                let output_barrier = Arc::clone(&barrier);
                let output_marker = marker.clone();
                scope.spawn(move || {
                    output_barrier.wait();
                    output_fanout.handle_output(&token, &id.to_string(), output_marker);
                });
                barrier.wait();
                activation.join().expect("activation thread")
            });
            flush(&fanout, id);
            let snapshot = activation.snapshot.expect("racing snapshot");
            let in_snapshot = snapshot
                .replay_data
                .windows(marker.len())
                .any(|window| window == marker);
            let in_live = events(&sink)
                .iter()
                .any(|event| event.1.windows(marker.len()).any(|window| window == marker));
            assert_ne!(in_snapshot, in_live, "iteration {iteration}");
            assert_eq!(snapshot.sequence == 1, in_snapshot);
        }
    }

    #[test]
    fn semantic_history_metadata_reports_row_and_byte_clamping() {
        let fanout = fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 24, 81)
            .expect("register clamping session");
        for index in 0..1_100 {
            let line = format!("clamped {index:04} {}\r\n", "x".repeat(70));
            feed(&fanout, id, &[line.as_bytes()]);
        }
        let expected = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            parsers.get(&id).unwrap().parser.screen().clone()
        };
        let snapshot = fanout
            .activate_terminal_output(id, WINDOW, true, 1, 1)
            .expect("clamped activation")
            .snapshot
            .expect("clamped snapshot");
        assert_eq!(snapshot.retained_history_rows, 1024);
        assert!(snapshot.included_history_rows < snapshot.retained_history_rows);
        assert!(snapshot.history_truncated);
        assert_eq!(
            snapshot.history_truncation_reason,
            PtyTerminalHistoryTruncationReason::RowAndByteLimitReached
        );
        assert!(snapshot.semantic_history_bytes as usize <= SEMANTIC_HISTORY_REPLAY_BYTES);
        assert!(snapshot.replay_bytes as usize <= SEMANTIC_REPLAY_MAX_BYTES);
        assert_current_screen_equivalent(&expected, replay_parser(&snapshot).screen());
    }

    fn nearest_rank(values: &mut [u64], percentile: usize) -> u64 {
        values.sort_unstable();
        let rank = percentile
            .checked_mul(values.len())
            .and_then(|value| value.checked_add(99))
            .expect("percentile rank")
            / 100;
        values[rank.saturating_sub(1)]
    }

    fn assert_latency_percentiles(
        name: &str,
        mut values: Vec<u64>,
        p95_limit_micros: u64,
        p99_limit_micros: u64,
    ) {
        assert!(!values.is_empty(), "{name} samples");
        let p95 = nearest_rank(&mut values, 95);
        let p99 = nearest_rank(&mut values, 99);
        eprintln!("{name}: p95={p95}us p99={p99}us samples={}", values.len());
        assert!(p95 <= p95_limit_micros, "{name} p95 {p95}us");
        assert!(p99 <= p99_limit_micros, "{name} p99 {p99}us");
        assert!(
            values.iter().all(|value| *value < 5_000_000),
            "{name} contained a five-second sample"
        );
    }

    #[cfg(windows)]
    fn current_private_bytes() -> u64 {
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let mut counters = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let read = unsafe {
            K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                counters.cb,
            )
        };
        assert_ne!(read, 0, "read release-gate process private bytes");
        counters.PrivateUsage as u64
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    #[ignore = "release-only Windows x64 semantic replay resource and latency gate"]
    fn semantic_replay_resource_and_latency_gate() {
        const { assert!(cfg!(all(windows, target_arch = "x86_64"))) };
        assert_eq!(
            unsafe { windows_sys::Win32::System::Diagnostics::Debug::IsDebuggerPresent() },
            0,
            "release latency gate must not run under a debugger"
        );
        let private_baseline = current_private_bytes();
        let fanout = fanout();
        let mut sessions = Vec::new();
        for session_index in 0..SUPPORTED_SEMANTIC_REPLAY_SESSIONS {
            let id = Uuid::new_v4();
            let token = fanout
                .register_session_for_test(id, IdleTuning::DEFAULT, 27, 81)
                .expect("register latency session");
            let mut history = Vec::new();
            for row in 0..1_100 {
                history.extend_from_slice(
                    format!("s{session_index:02} row {row:04} {}\r\n", "h".repeat(61)).as_bytes(),
                );
            }
            fanout.handle_output(&token, &id.to_string(), history);
            {
                let parsers = fanout.screen_parsers.lock().expect("parser state");
                let state = parsers.get(&id).unwrap();
                let mut screen = state.parser.screen().clone();
                screen.set_scrollback(usize::MAX);
                assert_eq!(screen.scrollback(), SEMANTIC_SCROLLBACK_ROWS);
            }
            sessions.push((id, token));
        }
        let resources = fanout.replay_budget.snapshot();
        assert_eq!(resources.sessions, SUPPORTED_SEMANTIC_REPLAY_SESSIONS);
        assert!(resources.steady_bytes <= SEMANTIC_STEADY_BUDGET_BYTES);
        let steady_private_delta = current_private_bytes().saturating_sub(private_baseline);
        assert!(
            steady_private_delta <= SEMANTIC_STEADY_BUDGET_BYTES as u64,
            "steady private-byte delta {steady_private_delta}"
        );

        for (id, token) in &sessions {
            fanout.handle_output(token, &id.to_string(), b"\x1b[?1049halt".to_vec());
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            assert!(parsers.get(id).unwrap().normal_checkpoint.is_some());
        }
        let resources = fanout.replay_budget.snapshot();
        assert!(resources.checkpoint_bytes <= SEMANTIC_CHECKPOINT_BUDGET_BYTES);
        let checkpoint_private_delta = current_private_bytes().saturating_sub(private_baseline);
        assert!(
            checkpoint_private_delta <= SEMANTIC_CHECKPOINT_BUDGET_BYTES as u64,
            "checkpoint private-byte delta {checkpoint_private_delta}"
        );
        eprintln!(
            "private bytes: steady_delta={steady_private_delta} checkpoint_delta={checkpoint_private_delta}"
        );

        let stop = Arc::new(AtomicBool::new(false));
        let missed_cadence = Arc::new(AtomicUsize::new(0));
        let scheduled = sessions
            .iter()
            .map(|_| Arc::new(AtomicUsize::new(0)))
            .collect::<Vec<_>>();
        let produced = sessions
            .iter()
            .map(|_| Arc::new(AtomicUsize::new(0)))
            .collect::<Vec<_>>();
        let producer_start = Arc::new(std::sync::Barrier::new(sessions.len() + 1));
        let producer_count = sessions.len();
        let producers = sessions
            .iter()
            .enumerate()
            .map(|(index, (id, token))| {
                let (sender, receiver) = std::sync::mpsc::sync_channel(64);
                let reader_fanout = fanout.clone();
                let reader_produced = Arc::clone(&produced[index]);
                let reader_session_id = id.to_string();
                let reader_token = token.clone();
                let reader = std::thread::spawn(move || {
                    while let Ok(chunk) = receiver.recv() {
                        reader_fanout.handle_output(&reader_token, &reader_session_id, chunk);
                        reader_produced.fetch_add(1, Ordering::AcqRel);
                    }
                });

                let stop = Arc::clone(&stop);
                let missed_cadence = Arc::clone(&missed_cadence);
                let scheduled = Arc::clone(&scheduled[index]);
                let producer_start = Arc::clone(&producer_start);
                let producer = std::thread::spawn(move || {
                    let mut chunk = b"\x1b[H".to_vec();
                    chunk.extend(std::iter::repeat_n(b'x', 77));
                    let cadence = Duration::from_millis(10);
                    producer_start.wait();
                    let phase = Duration::from_nanos(
                        (cadence.as_nanos() * index as u128 / producer_count as u128) as u64,
                    );
                    let mut deadline = Instant::now() + phase;
                    while !stop.load(Ordering::Acquire) {
                        let now = Instant::now();
                        if now < deadline {
                            std::thread::sleep(deadline - now);
                        }
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let started = Instant::now();
                        if started > deadline + cadence {
                            missed_cadence.fetch_add(1, Ordering::AcqRel);
                        }
                        match sender.try_send(chunk.clone()) {
                            Ok(()) => {}
                            Err(std::sync::mpsc::TrySendError::Full(bytes)) => {
                                missed_cadence.fetch_add(1, Ordering::AcqRel);
                                sender
                                    .send(bytes)
                                    .expect("latency reader remains connected");
                            }
                            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                                panic!("latency reader disconnected")
                            }
                        }
                        scheduled.fetch_add(1, Ordering::AcqRel);
                        deadline += cadence;
                        if deadline < started {
                            deadline = started + cadence;
                        }
                    }
                });
                (producer, reader)
            })
            .collect::<Vec<_>>();
        producer_start.wait();

        let cadence_ready_deadline = Instant::now() + Duration::from_secs(5);
        while produced
            .iter()
            .any(|count| count.load(Ordering::Acquire) < 10)
        {
            assert!(
                Instant::now() < cadence_ready_deadline,
                "latency readers did not process ten cadence chunks: {:?}",
                produced
                    .iter()
                    .map(|count| count.load(Ordering::Acquire))
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            missed_cadence.load(Ordering::Acquire),
            0,
            "producer cadence failed before latency timing"
        );
        eprintln!("producer cadence precondition: all readers processed at least 10 chunks");

        for index in 0..100 {
            let id = sessions[index % sessions.len()].0;
            let activation = fanout
                .activate_terminal_output(id, "latency", true, 1, index as u32 + 1)
                .expect("warmup activation");
            assert!(activation.snapshot.is_some());
        }
        eprintln!(
            "producer cadence: after_warmup={}",
            missed_cadence.load(Ordering::Acquire)
        );
        fanout.take_activation_timings_for_test();

        for index in 0..1_000 {
            let id = sessions[index % sessions.len()].0;
            let activation = fanout
                .activate_terminal_output(id, "latency", true, 1, index as u32 + 101)
                .expect("measured activation");
            assert!(activation.snapshot.is_some());
        }
        let normal = fanout.take_activation_timings_for_test();
        eprintln!(
            "producer cadence: after_normal={}",
            missed_cadence.load(Ordering::Acquire)
        );
        assert_eq!(normal.len(), 1_000);
        assert_latency_percentiles(
            "normal clone",
            normal.iter().map(|sample| sample.clone_micros).collect(),
            5_000,
            12_000,
        );
        assert_latency_percentiles(
            "normal parser lock",
            normal.iter().map(|sample| sample.lock_micros).collect(),
            8_000,
            20_000,
        );
        assert_latency_percentiles(
            "normal encode",
            normal.iter().map(|sample| sample.encode_micros).collect(),
            25_000,
            75_000,
        );
        assert_latency_percentiles(
            "normal activation",
            normal
                .iter()
                .map(|sample| sample.activation_micros)
                .collect(),
            50_000,
            150_000,
        );

        let barrier = Arc::new(std::sync::Barrier::new(sessions.len() + 1));
        let captures_ready = Arc::new(std::sync::Barrier::new(sessions.len() + 1));
        let capture_release = Arc::new(std::sync::Barrier::new(sessions.len() + 1));
        fanout.install_capture_barrier_for_test(
            Arc::clone(&captures_ready),
            Arc::clone(&capture_release),
        );
        let burst = sessions
            .iter()
            .enumerate()
            .map(|(index, (id, _))| {
                let fanout = fanout.clone();
                let barrier = Arc::clone(&barrier);
                let id = *id;
                std::thread::spawn(move || {
                    barrier.wait();
                    let activation = fanout
                        .activate_terminal_output(
                            id,
                            &format!("burst-{index}"),
                            true,
                            1,
                            2_000 + index as u32,
                        )
                        .expect("burst activation");
                    assert!(activation.snapshot.is_some());
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        captures_ready.wait();
        let burst_resources = fanout.replay_budget.snapshot();
        assert_eq!(burst_resources.sessions, SUPPORTED_SEMANTIC_REPLAY_SESSIONS);
        assert!(burst_resources.attach_bytes > 0);
        assert!(burst_resources.attach_bytes <= SEMANTIC_ATTACH_BUDGET_BYTES);
        let burst_private_delta = current_private_bytes().saturating_sub(private_baseline);
        assert!(
            burst_private_delta <= SEMANTIC_ATTACH_BUDGET_BYTES as u64,
            "burst private-byte delta {burst_private_delta}"
        );
        eprintln!(
            "private bytes: simultaneous_capture_delta={burst_private_delta} attach_reserved={}",
            burst_resources.attach_bytes
        );
        fanout.clear_capture_barrier_for_test();
        capture_release.wait();
        for thread in burst {
            thread.join().expect("burst thread");
        }
        eprintln!(
            "producer cadence: after_burst={}",
            missed_cadence.load(Ordering::Acquire)
        );
        let burst = fanout.take_activation_timings_for_test();
        assert_eq!(burst.len(), SUPPORTED_SEMANTIC_REPLAY_SESSIONS);
        assert_latency_percentiles(
            "burst clone",
            burst.iter().map(|sample| sample.clone_micros).collect(),
            8_000,
            20_000,
        );
        assert_latency_percentiles(
            "burst parser lock",
            burst.iter().map(|sample| sample.lock_micros).collect(),
            10_000,
            25_000,
        );
        assert_latency_percentiles(
            "burst encode",
            burst.iter().map(|sample| sample.encode_micros).collect(),
            100_000,
            300_000,
        );
        assert_latency_percentiles(
            "burst activation",
            burst
                .iter()
                .map(|sample| sample.activation_micros)
                .collect(),
            1_000_000,
            2_000_000,
        );

        stop.store(true, Ordering::Release);
        for (producer, reader) in producers {
            producer.join().expect("producer thread");
            reader.join().expect("latency reader thread");
        }
        assert_eq!(missed_cadence.load(Ordering::Acquire), 0);

        let wide_private_baseline = current_private_bytes();
        let wide_fanout = self::fanout();
        let wide_id = Uuid::new_v4();
        let wide_token = wide_fanout
            .register_session_for_test(wide_id, IdleTuning::DEFAULT, 80, 240)
            .expect("register styled wide-glyph session");
        let mut styled_wide = Vec::new();
        for index in 0..(80usize * 120) {
            if index % 2 == 0 {
                styled_wide.extend_from_slice(
                    "\x1b[1;3;4;7;38;2;250;251;252;48;2;100;101;102m界".as_bytes(),
                );
            } else {
                styled_wide.extend_from_slice("\x1b[2;38;2;1;2;3;48;2;240;241;242m界".as_bytes());
            }
        }
        wide_fanout.handle_output(&wide_token, &wide_id.to_string(), styled_wide);
        {
            let parsers = wide_fanout
                .screen_parsers
                .lock()
                .expect("wide parser state");
            let screen = parsers.get(&wide_id).unwrap().parser.screen();
            assert_eq!(screen.size(), (80, 240));
            let lead = screen.cell(0, 0).expect("wide lead cell");
            let continuation = screen.cell(0, 1).expect("wide continuation cell");
            assert_eq!(lead.contents(), "界");
            assert!(lead.is_wide());
            assert!(continuation.is_wide_continuation());
        }
        let wide_capture_ready = Arc::new(std::sync::Barrier::new(2));
        let wide_capture_release = Arc::new(std::sync::Barrier::new(2));
        wide_fanout.install_capture_barrier_for_test(
            Arc::clone(&wide_capture_ready),
            Arc::clone(&wide_capture_release),
        );
        let activation_fanout = wide_fanout.clone();
        let wide_activation = std::thread::spawn(move || {
            activation_fanout
                .activate_terminal_output(wide_id, "wide-resource", true, 1, 1)
                .expect("styled wide-glyph activation")
        });
        wide_capture_ready.wait();
        let wide_resources = wide_fanout.replay_budget.snapshot();
        assert_eq!(wide_resources.sessions, 1);
        assert!(wide_resources.attach_bytes > 0);
        assert!(wide_resources.attach_bytes <= SEMANTIC_ATTACH_BUDGET_BYTES);
        let wide_private_delta = current_private_bytes().saturating_sub(wide_private_baseline);
        assert!(
            wide_private_delta <= SEMANTIC_STEADY_BUDGET_BYTES as u64,
            "styled wide-glyph private-byte delta {wide_private_delta}"
        );
        eprintln!(
            "private bytes: styled_wide_240x80_capture_delta={wide_private_delta} attach_reserved={}",
            wide_resources.attach_bytes
        );
        wide_fanout.clear_capture_barrier_for_test();
        wide_capture_release.wait();
        let wide_activation = wide_activation.join().expect("wide activation thread");
        assert!(wide_activation.snapshot.is_some());
        drop(wide_activation);
        assert_eq!(wide_fanout.replay_budget.snapshot().attach_bytes, 0);

        let parsers = fanout.screen_parsers.lock().expect("parser state");
        for (index, (id, _)) in sessions.iter().enumerate() {
            assert_eq!(
                scheduled[index].load(Ordering::Acquire),
                produced[index].load(Ordering::Acquire),
                "session {index} producer/reader count"
            );
            assert_eq!(
                parsers.get(id).unwrap().output_sequence,
                2 + produced[index].load(Ordering::Acquire) as u64
            );
        }
        drop(parsers);
        let final_resources = fanout.replay_budget.snapshot();
        assert!(final_resources.steady_bytes <= SEMANTIC_STEADY_BUDGET_BYTES);
        assert!(final_resources.checkpoint_bytes <= SEMANTIC_CHECKPOINT_BUDGET_BYTES);
        assert_eq!(final_resources.attach_bytes, 0);
        let final_private_delta = current_private_bytes().saturating_sub(private_baseline);
        assert!(
            final_private_delta <= SEMANTIC_ATTACH_BUDGET_BYTES as u64,
            "final private-byte delta {final_private_delta}"
        );
    }
}
