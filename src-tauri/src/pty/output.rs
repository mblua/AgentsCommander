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
    /// Lexical tail + persistent modes from session start (Route A): survives ring
    /// rotation; feeds the fast-path eligibility and the open-suffix trim/restore.
    tail: LexicalTailTracker,
    /// Effective history ring size for this session (bytes), from the project
    /// settings at attach (default when absent/invalid). The ring and the tracker's
    /// front-states window are built at this size.
    history_limit_bytes: usize,
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
/// DEFAULT raw output bytes retained per session so a freshly created terminal can be
/// rehydrated with history instead of a single viewport. Sized to keep the frontend
/// replay peak close to its current value rather than to double the admission ceiling.
/// Each session's effective limit comes from the project settings at attach
/// (`terminalHistoryLimitBytes`, clamped to [4 KiB, 4 MiB]); this default applies when
/// the settings are absent, invalid, or the project path cannot be derived.
const DEFAULT_UI_HISTORY_LIMIT_BYTES: usize = 65_536;
/// How far into the ring the line-boundary trim looks for a newline. A stream without
/// newlines (progress bars redrawing with `\r`) would otherwise walk the whole ring on
/// every chunk, inside the parser mutex shared by every session of the backend.
const UI_HISTORY_LINE_SCAN_BYTES: usize = 4_096;
/// Normalization prologue for a history replay: normal buffer, full scroll region, autowrap
/// on, G0 back to ASCII, default attributes. Deliberately carries no erase sequence:
/// `\x1b[2J`, `\x1b[3J` and RIS each wipe part or all of the history this replay restores.
const UI_HISTORY_REPLAY_PROLOGUE: &[u8] = b"\x1b[?1049l\x1b[r\x1b[?7h\x1b(B\x1b[0m";

/// Normalization seam between the replayed ring and the parser mirror, emitted on the
/// fast path only (Route A). Grounds any pending sequence (`\x18` CAN — a no-op in ground,
/// defensive), restores the full scroll region, G0 ASCII and origin-off. Deliberately does
/// NOT touch DECAWM, IRM or the active buffer: forcing wrap/buffer semantics is what the
/// pivot removed (counterexample pairs and the saved-cursor effect of `?1049l` re-entry),
/// and eligibility guarantees those states are already default on this path.
const UI_HISTORY_REPLAY_SEAM: &[u8] = b"\x18\x1b[r\x1b(B\x1b[?6l";

/// Ceiling for the tracked open-sequence buffer. Covers the largest sequence the parsers
/// admit (vte caps OSC payloads at 1024 bytes); a longer open sequence marks the tail
/// indeterminate and the attach falls back to HEAD's bytes.
const UI_TAIL_TRACKER_BOUND: usize = 4_096;

/// Appends a chunk to the bounded history ring. The order is mandatory: trim for space,
/// trim to a line boundary, then append. Only the length bound is guaranteed; the line
/// alignment is best effort, because the boundary scan is capped and can find nothing. Its
/// outcome is recorded in `aligned` and enforced at the attach seed instead (#1458).
///
/// Every index is saturating on purpose. `VecDeque::drain(..k)` panics when `k > len`, and
/// here a panic is permanent rather than local: the caller flips the parser to `Unavailable`,
/// which leaves that console dead for the rest of the process.
fn append_history(
    history: &mut std::collections::VecDeque<u8>,
    aligned: &mut bool,
    tail: &mut LexicalTailTracker,
    data: &[u8],
    history_limit_bytes: usize,
) {
    // The tail tracker is stream-global: it must see every byte from session start,
    // whether or not the ring kept them, so a pending sequence spanning the ring front
    // stays recoverable. O(1) per byte beside the parser's own per-byte work. The
    // pre-state of every byte is recorded so the ring's front-states mirror stays in
    // lockstep with the ring's window (drains and extensions below use the same amounts).
    let mut pre_states: Vec<u8> = Vec::with_capacity(data.len());
    for &byte in data {
        pre_states.push(tail.state as u8);
        tail.push_byte(byte);
    }
    // A chunk larger than the whole ring keeps only its tail. Unreachable in production
    // (the local backend reads 4 KiB buffers, the container backend rejects frames over
    // 64 KiB) but it is where the trim arithmetic gets written wrong.
    let chunk_tail = &data[data.len().saturating_sub(history_limit_bytes)..];
    let over = (history.len() + chunk_tail.len()).saturating_sub(history_limit_bytes);
    if over > 0 {
        history.drain(..over.min(history.len()));
        tail.front_states.drain(..over.min(tail.front_states.len()));
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
                tail.front_states.drain(..=newline.min(tail.front_states.len().saturating_sub(1)));
                *aligned = true;
            }
            None => *aligned = false,
        }
    }
    // #1458: the one path on which the front changes without `over` ever being positive.
    // When `chunk_tail` becomes the WHOLE ring the front is `chunk_tail[0]`, and a chunk larger than the
    // ring was truncated at an arbitrary byte, so that front is not a line start and nothing
    // above recorded it. The `<` is load bearing: `chunk_tail.len() == data.len()` means the chunk
    // was NOT truncated, so `chunk_tail[0]` is a real stream boundary and `true` is correct.
    if history.is_empty() && chunk_tail.len() < data.len() {
        *aligned = false;
    }
    // The front-states for the appended bytes are the pre-states of the last
    // `chunk_tail.len()` bytes of the chunk (the oversized prefix is discarded with its
    // states — its first byte's pre-state becomes the new front state).
    let states_start = pre_states.len() - chunk_tail.len();
    tail.front_states.extend(pre_states.drain(states_start..));
    history.extend(chunk_tail);
    debug_assert_eq!(
        tail.front_states.len(),
        history.len(),
        "front-states must stay in byte-for-byte lockstep with the ring (Root RIS gate)"
    );
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

/// Lexical position of the stream tail, mirroring the sequence states the real client
/// (xterm.js 6.x `EscapeSequenceParser`) and the backend parser (vte 0.11.1) share: the
/// abort rules (ESC/CAN/SUB ground from any state) are identical in both parsers, so one
/// state machine is faithful for the source and for the receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TailState {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    Dcs,
    Sos,
    Utf8,
}

/// Persistente per-session lexical tail and mode tracker, fed every chunk from session
/// start so that neither the open-sequence state nor the terminal modes rotate with the
/// 64 KiB history ring (Route A).
///
/// Two jobs, both on the ingest hot path inside `append_history` (same parser mutex,
/// O(1) per byte, no allocations while in ground):
///
/// 1. **Tail state**: the current lexical position (Ground / ESC[-intermediate] / CSI /
///    OSC / DCS / SOS-PM-APC / partial UTF-8) so the attach can trim the open suffix from
///    the replay and re-emit it exactly once after the mirror, ending the seed in the same
///    partial state as the source at sequence N. C1 forms (0x9b/0x9d/0x90/0x98/0x9e/0x9f)
///    are recognized per the pipeline's raw-byte reality; C1 bytes inside string payloads
///    are absorbed as data (xterm.js OscPut semantics).
///
/// 2. **Persistent modes**: DECAWM (`?7`), IRM (`4`), G0 charset (`ESC ( <final>`),
///    DECOM (`?6`), scroll region (`CSI r`), active buffer (`?47`/`?1047`/`?1049`),
///    SO/SI shift and LRMM (`?69`), applied only when a sequence completes — a pending
///    toggle is part of the open suffix and must not affect eligibility at N. The sticky
///    historical flag is set by any representation-affecting CHANGE of the modes vt100
///    0.15.2 does not model (DECAWM, IRM, charset, shift, LRMM, alternate): cells formed
///    under those semantics cannot be represented by the parser grid, so the attach stays
///    fail-closed even when the mode returns to default. Only a proven checkpoint clears
///    it: session start, or RIS (`ESC c`), which resets the screen and every mode in both
///    parsers. DECSTBM and DECOM ARE modeled by vt100 and are not sticky: a
///    used-and-restored region/origin leaves a faithful grid, admitted when the scratch
///    certificate (cells, wrapped rows, cursor) agrees.
///
/// A pending sequence longer than `UI_TAIL_TRACKER_BOUND` marks the tail indeterminate
/// (`lost`): the open suffix cannot be reconstructed, and because the lost bytes could
/// carry a mode toggle the tracker also sets `sticky`, keeping the session fail-closed
/// until a proven checkpoint.
#[derive(Clone, Debug)]
struct LexicalTailTracker {
    state: TailState,
    pending: Vec<u8>,
    utf8_need: u8,
    utf8_seen: u8,
    /// Lead byte of the in-flight UTF-8 char (0xc2..=0xf4), so the completion can
    /// detect C1-as-UTF-8 (`C2 9B` decodes to U+009B, which the real client EXECUTES
    /// as an 8-bit CSI while a bare 0x9B is data on its Uint8Array path — A/B matrix F1).
    utf8_lead: u8,
    /// The source's REP preceding glyph at N (A/B matrix F2 final design + PGC exact
    /// gate): the DECODED last graphic char (ASCII or the full multi-byte grapheme),
    /// set by the last processed printable, reset to None by ANY valid intermediate
    /// sequence and any C0 EXECUTE (xterm `precedingJoinState`). The certified seed
    /// reparse compares this exactly against the candidate seed's own parsed tail.
    last_graphic: Option<Vec<u8>>,
    /// Whether the CURRENT open sequence was entered by an ESC that aborted a control
    /// string (A/B matrix F4): the real client's continuation of such a sequence renders
    /// literally, so the fast path must be excluded while this holds.
    entered_via_string_abort: bool,
    /// Lexical pre-state of every byte currently in the history ring, in ring order
    /// (drained and extended in lockstep with the ring inside `append_history`). This is
    /// what lets the attach PROVE the replay starts at a genuine Ground boundary instead
    /// of trusting a raw `\n` that may sit inside a control string (Root blocking
    /// review): a LF inside an OSC/DCS/APC/SOS/PM payload is not a lexical boundary.
    /// Memory: one byte per ring byte (≤ `DEFAULT_UI_HISTORY_LIMIT_BYTES` per session).
    front_states: std::collections::VecDeque<u8>,
    decawm: bool,
    irm: bool,
    g0: u8,
    decom: bool,
    region: (Option<u16>, Option<u16>),
    buffer_alternate: bool,
    shift_out: bool,
    lrmm: bool,
    sticky: bool,
    lost: bool,
}

impl LexicalTailTracker {
    fn new(history_limit_bytes: usize) -> Self {
        Self {
            state: TailState::Ground,
            pending: Vec::new(),
            utf8_need: 0,
            utf8_seen: 0,
            utf8_lead: 0,
            last_graphic: None,
            entered_via_string_abort: false,
            front_states: std::collections::VecDeque::with_capacity(history_limit_bytes),
            decawm: true,
            irm: false,
            g0: b'B',
            decom: false,
            region: (None, None),
            buffer_alternate: false,
            shift_out: false,
            lrmm: false,
            sticky: false,
            lost: false,
        }
    }

    fn push_byte(&mut self, byte: u8) {
        // Raw C1 fail-closed in ANY state (Root gap 3): xterm's parser applies global
        // C1 rules everywhere (the payload comment was false) — the backend parser and
        // the client disagree, so any raw 0x80..0x9F marks the session sticky, EXCEPT
        // inside a partial UTF-8 char where those bytes are continuation data.
        if (0x80..=0x9f).contains(&byte) && self.state != TailState::Utf8 {
            self.sticky = true;
        }
        match self.state {
            TailState::Ground => match byte {
                0x1b => self.begin(byte),
                // Bare C1 (0x80..0x9F) is NOT mirror-equivalent (Root correction): the
                // client's Uint8Array path treats it as data, but the backend parser
                // (vt100/vte) EXECUTES 8-bit C1 — e.g. raw `9B 31mX` renders the mirror
                // with `X` in red while the client shows `31mX` literally. Fail-closed
                // from ingest: any raw C1 marks the session sticky.
                0x80..=0x9f => self.sticky = true,
                0xc2..=0xdf => {
                    self.state = TailState::Utf8;
                    self.utf8_need = 1;
                    self.utf8_seen = 0;
                    self.utf8_lead = byte;
                    self.begin(byte);
                }
                0xe0..=0xef => {
                    self.state = TailState::Utf8;
                    self.utf8_need = 2;
                    self.utf8_seen = 0;
                    self.utf8_lead = byte;
                    self.begin(byte);
                }
                0xf0..=0xf4 => {
                    self.state = TailState::Utf8;
                    self.utf8_need = 3;
                    self.utf8_seen = 0;
                    self.utf8_lead = byte;
                    self.begin(byte);
                }
                0x0e => {
                    if !self.shift_out {
                        self.shift_out = true;
                        self.sticky = true;
                    }
                    self.last_graphic = None;
                }
                0x0f => {
                    if self.shift_out {
                        self.shift_out = false;
                        self.sticky = true;
                    }
                    self.last_graphic = None;
                }
                // ALL Ground C0 executables (0x00..0x1f except ESC — incl. NUL, BEL,
                // CAN/SUB, CR/LF/BS/TAB…) reset the PGC (Root gap 1)
                0x00..=0x1f => {
                    self.last_graphic = None;
                }
                // ASCII graphic char: the REP preceding glyph is recorded (decoded)
                0x20..=0x7e => {
                    self.last_graphic = Some(vec![byte]);
                }
                // Invalid UTF-8 leads: F5..FF (client replaces them), C0/C1 (impossible
                // overlong leads), and lone continuations A0..BF — all fail closed
                0xf5..=0xff | 0xc0 | 0xc1 | 0xa0..=0xbf => self.sticky = true,
                _ => {}
            },
            TailState::Escape => match byte {
                0x5b => {
                    self.state = TailState::Csi;
                    self.pending.push(byte);
                }
                0x5d => {
                    self.state = TailState::Osc;
                    self.pending.push(byte);
                }
                0x50 => {
                    self.state = TailState::Dcs;
                    self.pending.push(byte);
                }
                0x58 | 0x5e | 0x5f => {
                    self.state = TailState::Sos;
                    self.pending.push(byte);
                }
                0x20..=0x2f => {
                    self.state = TailState::EscapeIntermediate;
                    self.pending.push(byte);
                }
                // final byte: completed 2-byte escape (incl. ST `ESC \`, RIS `ESC c`,
                // charset designators `ESC ( <final>`)
                0x30..=0x7e => self.complete_with(byte),
                0x18 | 0x1a | 0x9c => self.abort(),
                // C0 EXECUTE (C0 action matrix): resets the PGC, the escape continues
                0x00..=0x17 | 0x19 | 0x1c..=0x1f => {
                    self.last_graphic = None;
                }
                _ => self.abort(),
            },
            TailState::EscapeIntermediate => {
                if (0x30..=0x7e).contains(&byte) {
                    self.complete_with(byte);
                } else if (0x20..=0x2f).contains(&byte) {
                    self.pending.push(byte);
                    self.check_bound();
                } else if byte == 0x1b {
                    self.begin(byte);
                } else if byte == 0x18 || byte == 0x1a || byte == 0x9c {
                    self.abort();
                } else if (0x00..=0x1f).contains(&byte) {
                    // C0 EXECUTE: resets the PGC, the escape continues
                    self.last_graphic = None;
                } else {
                    // CAN/SUB/ST (0x18/0x1a/0x9c) and any other byte abort the string
                    self.abort();
                }
            }
            TailState::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.complete_with(byte);
                } else if byte == 0x1b {
                    self.begin(byte);
                } else if byte == 0x18 || byte == 0x1a || byte == 0x9c {
                    self.abort();
                } else if (0x00..=0x1f).contains(&byte) {
                    // C0 EXECUTE (C0 action matrix): resets the PGC, the CSI continues
                    self.last_graphic = None;
                } else {
                    self.pending.push(byte);
                    self.check_bound();
                }
            }
            TailState::Osc => {
                // BEL/ST terminate an OSC (reset); the ordinary C0s are IGNORED as
                // payload (C0 action matrix: OSC preserves the PGC across them).
                if byte == 0x07 || byte == 0x9c {
                    self.complete_with(byte);
                } else if byte == 0x18 || byte == 0x1a {
                    self.abort();
                } else if byte == 0x1b {
                    // A/B matrix F4 + inner-ESC PGC families (measured xterm): OSC_END
                    // resets the PGC at the ESC; the fast path is excluded while this
                    // sequence is open.
                    self.entered_via_string_abort = true;
                    self.last_graphic = None;
                    self.begin(byte);
                } else {
                    // C1 bytes inside string payloads are data (xterm.js OscPut)
                    self.pending.push(byte);
                    self.check_bound();
                }
            }
            TailState::Dcs => {
                // ST (0x9c / ESC \) unhoods DCS: reset. BEL and the ordinary C0s are
                // DCS_PUT payload (C0 action matrix: they PRESERVE the PGC).
                if byte == 0x9c {
                    self.complete_with(byte);
                } else if byte == 0x18 || byte == 0x1a {
                    self.abort();
                } else if byte == 0x1b {
                    // F4 + inner-ESC families: DCS_UNHOOK resets the PGC at the ESC.
                    self.entered_via_string_abort = true;
                    self.last_graphic = None;
                    self.begin(byte);
                } else {
                    self.pending.push(byte);
                    self.check_bound();
                }
            }
            TailState::Sos => {
                // SOS/PM/APC are IGNORED strings: the ST (0x9c) ends them via
                // IGNORE -> Ground with the PGC PRESERVED (Root gap 2 — unlike DCS).
                if byte == 0x9c {
                    self.reset_after_sequence();
                } else if byte == 0x18 || byte == 0x1a {
                    self.abort();
                } else if byte == 0x1b {
                    // F4 + inner-ESC families: SOS/PM/APC preserve the PGC until the
                    // following sequence dispatches.
                    self.entered_via_string_abort = true;
                    self.begin(byte);
                } else {
                    self.pending.push(byte);
                    self.check_bound();
                }
            }
            TailState::Utf8 => {
                if (0x80..=0xbf).contains(&byte) {
                    // Scalar validation per the lead (Root gap 4): the second byte's
                    // permitted range excludes overlong (E0/F0), surrogates (ED) and
                    // > U+10FFFF (F4); anything else is fail-closed sticky.
                    let valid = match (self.utf8_lead, self.utf8_seen) {
                        (0xe0, 0) => byte >= 0xa0,
                        (0xed, 0) => byte <= 0x9f,
                        (0xf0, 0) => byte >= 0x90,
                        (0xf4, 0) => byte <= 0x8f,
                        _ => true,
                    };
                    if !valid {
                        self.sticky = true;
                        self.last_graphic = None;
                        self.reset_after_sequence();
                        return;
                    }
                    self.utf8_seen += 1;
                    self.pending.push(byte);
                    if self.utf8_seen >= self.utf8_need {
                        self.finish_utf8(byte);
                    }
                } else {
                    // ESC or any non-continuation byte truncating a partial scalar:
                    // fail closed, reset the scalar, and redispatch the byte exactly
                    // once (the ESC lands in Escape so its subsequent bytes — e.g.
                    // `ESC [` — keep their CSI semantics; a printable lands in Ground).
                    self.sticky = true;
                    self.last_graphic = None;
                    self.reset_after_sequence();
                    self.push_byte(byte);
                }
            }
        }
    }

    /// Resets the terminal state, modes and checkpoint (RIS `ESC c` semantics) while
    /// PRESERVING the `front_states` window, which must stay in byte-for-byte lockstep
    /// with the history ring (Root RIS gate): the pre-states of the already-ring bytes
    /// are historical and the RIS does not rewrite them.
    fn reset_terminal_state(&mut self) {
        self.state = TailState::Ground;
        self.pending.clear();
        self.utf8_need = 0;
        self.utf8_seen = 0;
        self.utf8_lead = 0;
        self.last_graphic = None;
        self.entered_via_string_abort = false;
        self.decawm = true;
        self.irm = false;
        self.g0 = b'B';
        self.decom = false;
        self.region = (None, None);
        self.buffer_alternate = false;
        self.shift_out = false;
        self.lrmm = false;
        self.sticky = false;
        self.lost = false;
        // front_states intentionally untouched: it mirrors the ring window, not the
        // terminal state.
    }

    /// Starts a fresh sequence from ground: any previous pending bytes are discarded
    /// (ESC/CAN always abort the in-flight sequence in both parsers).
    fn begin(&mut self, byte: u8) {
        self.state = if (0xc2..=0xf4).contains(&byte) {
            TailState::Utf8
        } else {
            TailState::Escape
        };
        self.pending.clear();
        self.pending.push(byte);
    }

    /// A completed graphic character (ASCII handled in Ground; multi-byte here): record
    /// it for the REP re-poke, advance the cursor model, and return to ground. C1-as-
    /// UTF-8 (`C2 9B` etc.) decodes to U+0080..U+009F: the real client executes it but
    /// the backend parser does not (mirror divergence — the SAME class as raw C1), so
    /// fail-closed from ingest (Root C1 correction).
    fn finish_utf8(&mut self, continuation: u8) {
        if self.utf8_need == 1 && self.utf8_lead == 0xc2 && (0x80..=0x9f).contains(&continuation) {
            self.sticky = true;
            self.last_graphic = None;
            self.reset_after_sequence();
            return;
        }
        // the decoded grapheme's raw bytes (the exact glyph for the PGC certification)
        self.last_graphic = Some(self.pending.clone());
        self.reset_after_sequence();
    }

    /// Completes the in-flight sequence WITH its final byte: the final byte is part of
    /// the interpreted sequence, so `apply_modes` sees the true terminator (the Root
    /// review fix: `ESC[?7l` must reach the mode logic as `...l`, not `...7`). Any valid
    /// intermediate sequence also resets the client's REP preceding state (A/B F2).
    fn complete_with(&mut self, byte: u8) {
        self.pending.push(byte);
        self.apply_modes();
        self.last_graphic = None;
        self.entered_via_string_abort = false;
        self.reset_after_sequence();
    }

    /// Aborts the in-flight sequence (CAN/SUB/ESC/ST interrupt): an incomplete sequence
    /// must never apply mode side effects as if it had completed. The abort also
    /// resets the REP preceding state (a C0 EXECUTE in xterm).
    fn abort(&mut self) {
        self.entered_via_string_abort = false;
        self.last_graphic = None;
        self.reset_after_sequence();
    }

    fn reset_after_sequence(&mut self) {
        self.state = TailState::Ground;
        self.pending.clear();
        self.utf8_need = 0;
        self.utf8_seen = 0;
        self.lost = false;
    }

    fn check_bound(&mut self) {
        if self.pending.len() > UI_TAIL_TRACKER_BOUND {
            // The open suffix is indeterminate AND its lost bytes could carry a mode
            // toggle: fail-closed until a proven checkpoint.
            self.lost = true;
            self.sticky = true;
            self.state = TailState::Ground;
            self.pending.clear();
        }
    }

    /// The open suffix at the current stream position, or `None` when the tail is in
    /// ground (or indeterminate). Called once per attach, cold path.
    fn pending_suffix(&self) -> Option<Vec<u8>> {
        if self.state != TailState::Ground && !self.pending.is_empty() && !self.lost {
            Some(self.pending.clone())
        } else {
            None
        }
    }

    /// The first ring-relative offset at or after `start` whose lexical pre-state is
    /// Ground — a position a parser in ground state can safely begin reading from. The
    /// ring front may sit inside a control string (its opening was evicted): every byte
    /// of the payload has a non-Ground pre-state, and the first valid boundary is the
    /// byte after the terminator. Offsets directly after an ESC-aborted string (A/B F4:
    /// string → ESC → the following byte) are excluded. Returns `None` when no Ground
    /// position exists in the remaining window — the attach must fall back to HEAD's
    /// bytes. Cold path (one linear scan per attach, ≤ 64 KiB).
    fn first_ground_from(&self, start: usize) -> Option<usize> {
        let states: Vec<u8> = self.front_states.iter().copied().collect();
        (start..states.len()).find(|offset| {
            states[*offset] == TailState::Ground as u8 && !esc_aborted_string_at(*offset, &states)
        })
    }

    /// Why the common fast-path preconditions fail, or `None` when they hold: tail
    /// determinable, no unmodeled history since the last proven checkpoint, final modes
    /// in the supported subset, active buffer normal (both tracker and parser), and no
    /// ESC-aborted string pending (A/B F4).
    fn common_eligibility(&self, parser_alternate: bool) -> Option<&'static str> {
        if self.lost {
            return Some("tail_indeterminate");
        }
        if self.sticky {
            return Some("sticky_historical");
        }
        // A/B matrix F4: a string aborted by an ESC leaves a sequence whose client
        // continuation renders literally — exclude while it is pending.
        if self.entered_via_string_abort {
            return Some("esc_aborted_string");
        }
        // Frontend-validated (2026-08-23): a partial UTF-8 lead at the boundary is NOT
        // restorable through the CAN seam — xterm.js's decoder does not resync on the
        // re-emitted lead and the live continuation renders mojibake. HEAD's ring-only
        // seed preserves the split character (the live chunk completes it), so this tail
        // class falls back byte-identically.
        if self.state == TailState::Utf8 {
            return Some("utf8_partial_tail");
        }
        if !self.decawm {
            return Some("decawm=off");
        }
        if self.irm {
            return Some("irm=on");
        }
        if self.g0 != b'B' {
            return Some("g0!=ascii");
        }
        if self.shift_out {
            return Some("shift=out");
        }
        if self.lrmm {
            return Some("lrmm=on");
        }
        if self.decom {
            return Some("decom=on");
        }
        if self.region != (None, None) {
            return Some("region=trimmed");
        }
        if self.buffer_alternate || parser_alternate {
            return Some("buffer=alternate");
        }
        None
    }

    /// Applies the mode side effects of the completed sequence `self.pending`. The final
    /// byte gates every arm, so aborted sequences and non-mode sequences are no-ops.
    fn apply_modes(&mut self) {
        let seq = &self.pending;
        if seq.len() < 2 {
            return;
        }
        if seq[0] == 0x1b && seq[1] != b'[' {
            match seq[1] {
                // RIS: full reset — the proven checkpoint (screen and modes rebuilt).
                // Must NOT touch `front_states`: they stay in byte-for-byte lockstep
                // with the history ring (Root RIS gate).
                b'c' => self.reset_terminal_state(),
                // G0 designation: `ESC ( <final>`
                b'(' => {
                    let g0 = *seq.last().unwrap_or(&b'B');
                    if self.g0 != g0 {
                        self.g0 = g0;
                        self.sticky = true;
                    }
                }
                _ => {}
            }
            return;
        }
        // CSI introducer: 7-bit `ESC [` or the C1-via-UTF-8 form `C2 9B` (A/B F1 — the
        // real client decodes it and executes it as an 8-bit CSI).
        let csi = (seq[0] == 0x1b && seq[1] == b'[')
            || (seq[0] == 0xc2 && seq[1] == 0x9b);
        if !csi || seq.len() < 3 {
            return;
        }
        let final_byte = *seq.last().unwrap();
        let body = &seq[2..seq.len() - 1];
        let (private, params) = parse_csi_params(body);
        match final_byte {
            b'h' | b'l' => {
                let on = final_byte == b'h';
                if private {
                    for param in params {
                        match param {
                            7 if self.decawm != on => {
                                self.decawm = on;
                                self.sticky = true;
                            }
                            6 => self.decom = on,
                            69 if self.lrmm != on => {
                                self.lrmm = on;
                                self.sticky = true;
                            }
                            47 | 1047 | 1049 if self.buffer_alternate != on => {
                                self.buffer_alternate = on;
                                self.sticky = true;
                            }
                            _ => {}
                        }
                    }
                } else {
                    for param in params {
                        if param == 4 && self.irm != on {
                            self.irm = on;
                            self.sticky = true;
                        }
                    }
                }
            }
            b'r' => {
                if !private {
                    self.region = match params.as_slice() {
                        // bare `CSI r` / `CSI ;r` = full margins
                        [0] | [0, 0] => (None, None),
                        [top, bottom] => (Some(*top), Some(*bottom)),
                        [top] => (Some(*top), None),
                        _ => (None, None),
                    };
                }
            }
            b'b' if !private => {
                // Cross-validation p3: vt100 0.15.2 does NOT implement `CSI Ps b`
                // (REP) — the authoritative screen loses the cells a historical REP
                // created, so the mirror diverges from the real client regardless of
                // the PGC state at N. Fail-closed from the first completed REP until a
                // proven cell checkpoint (RIS).
                self.sticky = true;
            }
            b'p' if !private && body.contains(&b'!') => {
                // DECSTR: modes to defaults, buffer kept, sticky KEPT (cells persist).
                self.decawm = true;
                self.irm = false;
                self.g0 = b'B';
                self.decom = false;
                self.region = (None, None);
                self.shift_out = false;
                self.lrmm = false;
            }
            _ => {}
        }
    }
}

/// Per-attach Route A instrumentation carried out of the parser lock (the file's own
/// #1439 R2 rule: no logging under the mutex). Sizes and states only — never content.
/// `behavior` is "fast" | "ring-only" | "screen-only"; `reason` names the failing gate
/// on fallbacks ("fast" on the fast path).
type AttachInstrumentation = (
    &'static str,
    &'static str,
    usize,
    usize,
    usize,
    usize,
    u128,
    &'static str,
);

/// Whether `states[offset]` is directly after a string state that an ESC aborted
/// (A/B matrix F4): the real client ends the string at the ESC and parses the
/// following bytes from Escape, so a boundary there would replay an ESC-tail the client
/// renders literally.
fn esc_aborted_string_at(offset: usize, states: &[u8]) -> bool {
    if offset < 2 {
        return false;
    }
    let before = states[offset - 2];
    let string_state = before == TailState::Osc as u8
        || before == TailState::Dcs as u8
        || before == TailState::Sos as u8;
    string_state && states[offset - 1] == TailState::Escape as u8
}

/// Splits the body of a completed CSI (bytes between `ESC [` / C1 and the final byte)
/// into `(private, params)`. An empty body yields `[0]`; omitted parameters yield 0
/// (the region arm maps `[0]`/`[0, 0]` back to the full-margins default).
fn parse_csi_params(body: &[u8]) -> (bool, Vec<u16>) {
    let mut private = false;
    let mut params = Vec::new();
    let mut current: Option<u16> = None;
    for &byte in body {
        match byte {
            b'?' => private = true,
            b'0'..=b'9' => current = Some(current.unwrap_or(0) * 10 + u16::from(byte - b'0')),
            b';' => {
                params.push(current.unwrap_or(0));
                current = None;
            }
            _ => {}
        }
    }
    params.push(current.unwrap_or(0));
    (private, params)
}

/// Fail-closed certificate that the replay the client will parse (prologue + ring minus
/// its open suffix) is a SUBSET-consistent rendering of the live parser's grid: every
/// cell the replay paints must match the source cell-for-cell (contents, colors, style),
/// while cells the replay leaves empty are allowed to differ — the mirror repaints the
/// whole viewport from the live grid, so scrolled-out paint the ring cannot reproduce is
/// exactly what the fast path fixes. What the certificate still rejects: a ring front
/// landing inside a string sequence (the source has no literal text where the replay
/// renders it), grid fragmentation (sequential `\n`-only lines render differently after
/// the ring front), and DECSTBM/DECOM history whose cells the replay cannot reproduce.
fn scratch_certificate(rows: u16, cols: u16, body: &[u8], suffix: Option<&[u8]>, live: &vt100::Screen) -> bool {
    let cut = suffix.map_or(body.len(), |s| body.len() - s.len());
    let mut scratch = vt100::Parser::new(rows, cols, 0);
    scratch.process(UI_HISTORY_REPLAY_PROLOGUE);
    scratch.process(&body[..cut]);
    if scratch.screen().alternate_screen() {
        return false;
    }
    for row in 0..rows {
        let scratch_row_has_content = (0..cols)
            .any(|col| cell_non_default(scratch.screen().cell(row, col)));
        if scratch_row_has_content
            && scratch.screen().row_wrapped(row) != live.row_wrapped(row)
        {
            return false;
        }
        for col in 0..cols {
            let cell = scratch.screen().cell(row, col);
            if !cell_non_default(cell) {
                continue;
            }
            let actual = cell.map(|x| {
                (
                    x.contents().to_string(),
                    x.fgcolor(),
                    x.bgcolor(),
                    x.bold(),
                    x.italic(),
                    x.underline(),
                    x.inverse(),
                )
            });
            let wanted = live.cell(row, col).map(|x| {
                (
                    x.contents().to_string(),
                    x.fgcolor(),
                    x.bgcolor(),
                    x.bold(),
                    x.italic(),
                    x.underline(),
                    x.inverse(),
                )
            });
            if actual != wanted {
                return false;
            }
        }
    }
    true
}

/// PGC restoration bytes for the seed's tail (Root PGC gate): the source's preceding
/// glyph from the tracker (`Some` — the decoded grapheme) drives the re-poke; `None`
/// gets a deterministic final `\x18` (CAN — visually inert in Ground, resets the PGC
/// by C0 EXECUTE). The re-poke's TARGET comes from the authoritative vt100 Screen
/// (cursor + glyph cell, wide-lead adjusted, exact cell attributes). The FINAL proof is
/// the certified seed reparse (`seed_certificate`), which compares the candidate
/// seed's parsed tail against the source exactly — any mismatch falls back.
fn pgc_restoration_bytes(tail: &LexicalTailTracker, screen: &vt100::Screen) -> Option<Vec<u8>> {
    if tail.last_graphic.is_none() {
        // deterministic None via CAN: visually inert in Ground, resets the PGC by C0
        // EXECUTE (certified by the seed reparse)
        return Some(b"\x18".to_vec());
    }
    let (rows, cols) = screen.size();
    let (row, col) = screen.cursor_position();
    if row >= rows {
        return None;
    }
    let mut target_col = if col > 0 {
        col - 1
    } else if col >= cols {
        cols - 1
    } else {
        return None;
    };
    let cell = screen.cell(row, target_col)?;
    if cell.contents().is_empty() && target_col > 0 {
        // A wide glyph's continuation cell is empty: the lead is one column earlier.
        let lead = screen.cell(row, target_col - 1)?;
        if lead.is_wide() && !lead.contents().is_empty() {
            target_col -= 1;
        } else {
            return None;
        }
    }
    let cell = screen.cell(row, target_col)?;
    let glyph = cell.contents();
    if glyph.is_empty() {
        return None;
    }
    // Screen-neutral requirement: the glyph renders under the mirror's final SGR (the
    // source's current attributes) — the cell must carry exactly those.
    if cell.fgcolor() != screen.fgcolor()
        || cell.bgcolor() != screen.bgcolor()
        || cell.bold() != screen.bold()
        || cell.italic() != screen.italic()
        || cell.underline() != screen.underline()
        || cell.inverse() != screen.inverse()
    {
        return None;
    }
    let mut bytes = format!("\x1b[{};{}H", row + 1, target_col + 1).into_bytes();
    bytes.extend_from_slice(glyph.as_bytes());
    Some(bytes)
}

/// Certified seed reparse (Root PGC gate): parse the FINAL seed — prologue + replay −
/// suffix + seam + mirror + PGC restoration + suffix — in a fresh parser AND a fresh
/// tracker, and compare the resulting state exactly against the source at N:
///
/// * the PGC (the fresh tracker's last preceding glyph vs the source's — the only
///   direct proof; covers the CAN-for-None and the exact-glyph-for-Some cases),
/// * the cursor position,
/// * the wraps and EVERY cell (contents and attributes) of the repainted viewport.
///
/// Any mismatch → byte-identical HEAD fallback (indeterminate never enters the fast
/// path). One extra bounded parse per attach (cold path).
fn seed_certificate(
    seed: &[u8],
    rows: u16,
    cols: u16,
    source_tail: &LexicalTailTracker,
    source: &vt100::Screen,
) -> bool {
    let mut fresh = vt100::Parser::new(rows, cols, 0);
    fresh.process(seed);
    let mut fresh_tail = LexicalTailTracker::new(DEFAULT_UI_HISTORY_LIMIT_BYTES);
    for &byte in seed {
        fresh_tail.push_byte(byte);
    }
    if fresh_tail.last_graphic != source_tail.last_graphic {
        return false;
    }
    if fresh.screen().cursor_position() != source.cursor_position() {
        return false;
    }
    for row in 0..rows {
        if fresh.screen().row_wrapped(row) != source.row_wrapped(row) {
            return false;
        }
        for col in 0..cols {
            let a = fresh.screen().cell(row, col);
            let b = source.cell(row, col);
            let ae = a.map(|x| {
                (
                    x.contents().to_string(),
                    x.fgcolor(),
                    x.bgcolor(),
                    x.bold(),
                    x.italic(),
                    x.underline(),
                    x.inverse(),
                )
            });
            let be = b.map(|x| {
                (
                    x.contents().to_string(),
                    x.fgcolor(),
                    x.bgcolor(),
                    x.bold(),
                    x.italic(),
                    x.underline(),
                    x.inverse(),
                )
            });
            if ae != be {
                return false;
            }
        }
    }
    true
}

/// A cell the replay must reproduce faithfully: it has visible content or carries style.
fn cell_non_default(cell: Option<&vt100::Cell>) -> bool {
    cell.is_some_and(|x| {
        !x.contents().is_empty()
            || x.fgcolor() != vt100::Color::Default
            || x.bgcolor() != vt100::Color::Default
            || x.bold()
            || x.italic()
            || x.underline()
            || x.inverse()
    })
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
            let accumulator = Arc::clone(
                state
                    .accumulators
                    .entry(session_id)
                    .or_insert_with(|| {
                        Arc::new(Mutex::new(SessionAccumulator::new(Arc::clone(registration))))
                    }),
            );
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
        history_limit_bytes: usize,
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
            history: std::collections::VecDeque::with_capacity(history_limit_bytes),
            history_aligned: true,
            conpty_size: (rows, cols),
            tail: LexicalTailTracker::new(history_limit_bytes),
            history_limit_bytes,
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
        self.register_session(
            id,
            idle_tuning,
            rows,
            cols,
            DEFAULT_UI_HISTORY_LIMIT_BYTES,
            PtyOutputTarget::noop(),
        )
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
                        append_history(
                            &mut state.history,
                            &mut state.history_aligned,
                            &mut state.tail,
                            &data,
                            state.history_limit_bytes,
                        );
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
        // Route A instrumentation carrier, emitted after the parser lock is dropped (the
        // file's own #1439 R2 rule: no logging under the mutex).
        // (behavior, reason, ring_bytes, kept_bytes, mirror_bytes, tail_bytes, elapsed_us, buffer)
        let mut attach_info: Option<AttachInstrumentation> = None;
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
                    state.parser.set_size(conpty_rows, conpty_cols);
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
                    let ring_len = replay.map_or(0, |(front, back)| front.len() + back.len());
                    // Root blocking review: the replay must start at a PROVEN Ground
                    // boundary. The ring front (and the first-line offset) may sit inside a
                    // control string whose opening was evicted; a raw `\n` there is not a
                    // lexical boundary. The front-states mirror gives the exact pre-state
                    // of every ring byte: advance the replay start to the first Ground
                    // position (the byte after the terminator), or reject the fast path
                    // when no Ground position exists in the window. Payload bytes before
                    // that position are dropped — the client never misreads them from
                    // Ground (false-positive cells, mode side effects, scrollback debris).
                    let fast_replay: Option<(&[u8], &[u8])> = match replay {
                        Some((front, back)) => {
                            let start = state.history.len() - (front.len() + back.len());
                            match state.tail.first_ground_from(start) {
                                Some(offset) if offset == start => Some((front, back)),
                                Some(offset) => {
                                    let advance = offset - start;
                                    if advance < front.len() {
                                        Some((&front[advance..], back))
                                    } else if advance - front.len() < back.len() {
                                        Some((&[], &back[advance - front.len()..]))
                                    } else {
                                        None
                                    }
                                }
                                None => None,
                            }
                        }
                        None => None,
                    };
                    let buffer_state = if state.parser.screen().alternate_screen() {
                        "alternate"
                    } else {
                        "normal"
                    };
                    // Route A instrumentation carrier: set inside the closure, emitted
                    // after the parser lock is dropped (the file's own #1439 R2 rule: no
                    // logging under the mutex).
                    let started = std::time::Instant::now();                    crate::logging::catch_payload_unwind(|| {
                        let screen = state.parser.screen();
                        let (rows, cols) = screen.size();
                        let cells = usize::from(rows).checked_mul(usize::from(cols)).ok_or(())?;
                        if rows > MAX_ROWS || cols > MAX_COLUMNS || cells > MAX_CELLS {
                            return Err(());
                        }
                        let suffix = state.tail.pending_suffix();
                        let common_reason =
                            state.tail.common_eligibility(screen.alternate_screen());
                        let formatted = screen.contents_formatted();
                        let (data, behavior, reason) = match replay {
                            Some((orig_front, orig_back)) => {
                                // The fast attempt uses the boundary-ADVANCED replay when
                                // the boundary proof advanced it; the fallback ALWAYS uses
                                // the ORIGINAL replay (byte-identical to HEAD).
                                let (fast_front, fast_back) =
                                    fast_replay.unwrap_or((orig_front, orig_back));
                                let boundary_proven = fast_replay.is_some();
                                // One bounded contiguous copy of the fast replay (the open
                                // suffix may straddle the VecDeque front/back halves — the
                                // trim must never do half arithmetic on the two slices).
                                let mut body = Vec::with_capacity(fast_front.len() + fast_back.len());
                                body.extend_from_slice(fast_front);
                                body.extend_from_slice(fast_back);
                                let suffix_inside = suffix
                                    .as_ref()
                                    .is_none_or(|s| s.len() <= body.len());
                                let pgc = pgc_restoration_bytes(&state.tail, screen);
                                if boundary_proven
                                    && common_reason.is_none()
                                    && suffix_inside
                                    && pgc.is_some()
                                    && scratch_certificate(rows, cols, &body, suffix.as_deref(), screen)
                                {
                                    let cut = suffix.as_ref().map_or(body.len(), |s| body.len() - s.len());
                                    let mut bytes = Vec::with_capacity(
                                        UI_HISTORY_REPLAY_PROLOGUE.len()
                                            + cut
                                            + UI_HISTORY_REPLAY_SEAM.len()
                                            + formatted.len()
                                            + suffix.as_ref().map_or(0, |s| s.len()),
                                    );
                                    bytes.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
                                    bytes.extend_from_slice(&body[..cut]);
                                    bytes.extend_from_slice(UI_HISTORY_REPLAY_SEAM);
                                    bytes.extend_from_slice(&formatted);
                                    // PGC exact restoration (Root gate): the CAN for a
                                    // source-PGC-None (deterministic, visually inert) or
                                    // the exact-glyph re-poke for a source-PGC-Some —
                                    // before the open suffix.
                                    bytes.extend_from_slice(pgc.as_deref().unwrap_or(b""));
                                    if let Some(s) = &suffix {
                                        bytes.extend_from_slice(s);
                                    }
                                    // Certified seed reparse: the final state (PGC,
                                    // cursor, wraps, per-cell) must equal the source.
                                    if seed_certificate(&bytes, rows, cols, &state.tail, screen) {
                                        (bytes, "fast", "fast")
                                    } else {
                                        let mut fallback =
                                            Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + orig_front.len() + orig_back.len());
                                        fallback.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
                                        fallback.extend_from_slice(orig_front);
                                        fallback.extend_from_slice(orig_back);
                                        (fallback, "ring-only", "seed_certificate_mismatch")
                                    }
                                } else {
                                    // Fallback: byte-identical to HEAD (prologue + the
                                    // ORIGINAL replay verbatim; the client stays
                                    // mid-sequence and the live chunk completes the tail,
                                    // #1458 §7.4). The boundary advance is DISCARDED on
                                    // any gate failure, so an evicted string opening never
                                    // leaks into the fallback bytes.
                                    let mut bytes = Vec::with_capacity(
                                        UI_HISTORY_REPLAY_PROLOGUE.len() + orig_front.len() + orig_back.len(),
                                    );
                                    bytes.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
                                    bytes.extend_from_slice(orig_front);
                                    bytes.extend_from_slice(orig_back);
                                    let reason = if !boundary_proven {
                                        "no_ground_boundary"
                                    } else {
                                        common_reason.unwrap_or("suffix_untrimable_or_certificate")
                                    };
                                    (bytes, "ring-only", reason)
                                }
                            }
                            // No line start with content behind it. The ring cannot be
                            // replayed from any offset, and in the observed incident it holds
                            // nothing but spinner frames anyway. The mirror is a consistent
                            // full repaint on a grid the #1439 branch above already validated.
                            // Fast path appends the open suffix so the live chunk completes
                            // it (fixing the pre-existing orphaned-continuation wart);
                            // fallback is HEAD's mirror byte for byte.
                            None => {
                                let pgc = pgc_restoration_bytes(&state.tail, screen);
                                if common_reason.is_none() && pgc.is_some() {
                                    let mut bytes = Vec::with_capacity(
                                        formatted.len()
                                            + pgc.as_ref().map_or(0, |p| p.len())
                                            + suffix.as_ref().map_or(0, |s| s.len()),
                                    );
                                    bytes.extend_from_slice(&formatted);
                                    bytes.extend_from_slice(pgc.as_deref().unwrap_or(b""));
                                    if let Some(s) = &suffix {
                                        bytes.extend_from_slice(s);
                                    }
                                    if seed_certificate(&bytes, rows, cols, &state.tail, screen) {
                                        (bytes, "fast", "fast")
                                    } else {
                                        (formatted.clone(), "screen-only", "seed_certificate_mismatch")
                                    }
                                } else {
                                    (
                                        formatted.clone(),
                                        "screen-only",
                                        common_reason.unwrap_or("fast_gate"),
                                    )
                                }
                            }
                        };

                        attach_info = Some((
                            behavior,
                            reason,
                            ring_len,
                            ring_len,
                            formatted.len(),
                            suffix.as_ref().map_or(0, |s| s.len()),
                            started.elapsed().as_micros(),
                            buffer_state,
                        ));
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
        if let Some((behavior, reason, ring, kept, mirror, tail, elapsed_us, buffer)) = attach_info {
            if behavior == "fast" {
                log::debug!(
                    "[terminal-snapshot] stage=attach_replay_repaint session={id} behavior={behavior} reason={reason} ring={ring} kept={kept} mirror={mirror} tail={tail} elapsed_us={elapsed_us}"
                );
            } else {
                log::warn!(
                    "[terminal-snapshot] stage=attach_replay_fallback session={id} reason={reason} ring={ring} kept={kept} tail={tail} buffer={buffer} behavior={behavior}"
                );
            }
        }
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

    #[cfg(test)]
    pub(crate) fn history_limit_bytes_for_test(&self, id: Uuid) -> Option<usize> {
        let parsers = self.screen_parsers.lock().ok()?;
        parsers
            .get(&id)
            .map(|state| state.history_limit_bytes)
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
            let resized = crate::logging::catch_payload_unwind(|| {
                state.parser.set_size(rows, cols);
            });
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
                DEFAULT_UI_HISTORY_LIMIT_BYTES,
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
                DEFAULT_UI_HISTORY_LIMIT_BYTES,
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
                DEFAULT_UI_HISTORY_LIMIT_BYTES,
                PtyOutputTarget::from_test_sink(Arc::clone(&sink)),
            )
            .expect("register one-cell session");
        // A wide grapheme in a one-column grid removes this session's parser for good.
        fanout.handle_output(&token, &id.to_string(), "界".as_bytes().to_vec());

        assert!(matches!(
            fanout.activate_terminal_output(id, WINDOW, true),
            Ok(None)
        ));
        assert_eq!(fanout.attached_labels_for_test(id), vec![WINDOW.to_string()]);

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
        assert_eq!(seeded.len(), 1, "the window already watching keeps its bytes");
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
        assert_eq!(emitted.len(), 1, "the attached window keeps receiving its bytes");
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
        assert_eq!(emitted.len(), 1, "no timer, no explicit flush: the ceiling did it");
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
                DEFAULT_UI_HISTORY_LIMIT_BYTES,
                PtyOutputTarget::from_app_handle(app.handle().clone()),
            )
            .expect("register app-handle session");

        attach(&fanout, id, WINDOW);
        fanout.handle_output(&token, &id.to_string(), b"only the attached window".to_vec());
        flush(&fanout, id);

        assert_eq!(attached_events.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(unattached_events.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(any_target_events.load(std::sync::atomic::Ordering::SeqCst), 1);
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
                DEFAULT_UI_HISTORY_LIMIT_BYTES,
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
                DEFAULT_UI_HISTORY_LIMIT_BYTES,
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

        let oversized = vec![b'x'; DEFAULT_UI_HISTORY_LIMIT_BYTES + 4_096];
        feed(&fanout, id, &[&oversized]);
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let state = parsers.get(&id).expect("registered session");
        assert_eq!(state.history.len(), DEFAULT_UI_HISTORY_LIMIT_BYTES);
        assert_eq!(state.history.capacity(), DEFAULT_UI_HISTORY_LIMIT_BYTES);
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
        for _ in 0..(DEFAULT_UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
            feed(&fanout, id, &[frame]);
        }
        {
            // The precondition of the defect, asserted rather than assumed.
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            assert_eq!(state.history.len(), DEFAULT_UI_HISTORY_LIMIT_BYTES);
            assert!(!state.history_aligned);
        }

        let expected = mirrored_screen(&fanout, id);
        let data = activation_data(&fanout, id, true);

        assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
        // the fast mirror path ends with the deterministic PGC-None CAN ()
        let mut with_pgc = expected.clone();
        with_pgc.push(0x18);
        assert_eq!(data, with_pgc);
    }

    /// #1458. The healthy path: when the capped trim did realign the ring, the seed is the
    /// prologue followed by the ring verbatim from its very first byte. This fixture's
    /// sequential `\n`-only lines (no `\r`) fragment differently after the ring front than
    /// in the source, so the Route A scratch certificate correctly sends it to the
    /// byte-identical HEAD fallback — the ring-verbatim property below pins exactly that.
    #[test]
    fn a_line_aligned_ring_still_seeds_the_whole_ring() {
        let fanout = fanout();
        let id = session(&fanout);
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
        for _ in 0..(DEFAULT_UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
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
        let mut with_pgc = expected.clone();
        with_pgc.push(0x18);
        assert_eq!(data, with_pgc);
    }

    /// #1458. The recovering case, and the only one that exercises `history_from_first_line`'s
    /// `Some` arm: an unaligned ring that still holds lines must seed from the byte after its
    /// first `\n`, not fall back to the mirror. Asserting the whole body is the point; a stub
    /// helper that always returns `None` passes every other test in this file. Like the
    /// aligned fixture, this one lands in the HEAD fallback (sequential `\n`-only lines are
    /// not grid-reproducible after the ring front), so the body must equal HEAD's bytes.
    #[test]
    fn an_unaligned_ring_with_a_later_newline_seeds_from_that_line() {
        let fanout = fanout();
        let id = session(&fanout);
        let frame = b"\x1b[38;2;153;153;153m* Drizzling..\r"; // 33 bytes, no `\n`
        for _ in 0..(DEFAULT_UI_HISTORY_LIMIT_BYTES / frame.len() + 64) {
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
        // the fast mirror path ends with the deterministic PGC-None CAN (\x18)
        let mut with_pgc = expected.clone();
        with_pgc.push(0x18);
        assert_eq!(data, with_pgc);
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
        let mut with_pgc = expected.clone();
        with_pgc.push(0x18);
        assert_eq!(data, with_pgc);
        assert!(!data.starts_with(UI_HISTORY_REPLAY_PROLOGUE));
    }

    // ── Route A (fail-closed fast path) ────────────────────────────────────────


    /// Codex-like fixture: a normal-buffer TUI with cursor addressing, > 64 KiB, whose
    /// initial full paint scrolled out of the ring. HEAD's ring-only seed leaves the rows
    /// the ring never repaints hollow (the incident's black band); the fast path must
    /// engage (seam + mirror present) and the fresh client must equal the no-attach
    /// reference after the live continuation.
    #[test]
    fn codex_like_fixture_engages_fast_path_and_is_exact_after_live() {
        fn codex_stream(frames: usize) -> Vec<u8> {
            let mut v = Vec::new();
            // full paint of all 30 rows (this is what scrolls out of the ring)
            for r in 1..=30u16 {
                v.extend_from_slice(format!("\x1b[{r};1H\x1b[38;2;{};60;90mrow-{r:02} {:0<85}\x1b[K", (r * 7) % 255, "").as_bytes());
            }
            for i in 0..frames {
                let r = 3 + (i % 25) as u16;
                let c = 5 + (i % 60) as u16;
                v.extend_from_slice(format!("\x1b[{r};{c}H\x1b[38;2;{};200;50mcell-{i:04}\x1b[K", i % 255).as_bytes());
                v.extend_from_slice(format!("\x1b[30;1H\x1b[38;2;153;153;153m* working {}\r", "*".repeat(i % 5)).as_bytes());
                v.extend_from_slice(format!("\x1b[1;1Hlog {i:04} ok\n").as_bytes());
            }
            v
        }
        let continuation = format!("\x1b[1;1Hlog 9999 done\n\x1b[15;1Hfinal").into_bytes();
        let fanout = fanout();
        let id = session(&fanout);
        let mut stream = codex_stream(760); // ~93 KiB with the paint
        assert!(stream.len() > DEFAULT_UI_HISTORY_LIMIT_BYTES);
        let cut = stream.len() - continuation.len() - 1;
        let after: Vec<u8> = stream.split_off(cut);
        // feed everything but the last chunk, attach, then the live continuation
        feed(&fanout, id, &[&stream]);
        let snapshot = attach(&fanout, id, WINDOW).expect("snapshot");
        let seed = snapshot.data.clone();
        feed(&fanout, id, &[&after]);

        // fast path engaged: seam present, mirror present
        assert!(
            seed.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "fast path must engage for the eligible Codex-like fixture (reason={:?})",
            {
                let parsers = fanout.screen_parsers.lock().expect("parser state");
                let st = parsers.get(&id).expect("registered session");
                st.tail.common_eligibility(st.parser.screen().alternate_screen())
            }
        );
        // HEAD's ring-only seed leaves hollow rows (the incident)
        let ring = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            let (front, back) = state.history.as_slices();
            [front, back].concat()
        };
        let mut head_fresh = vt100::Parser::new(30, 120, 0);
        head_fresh.process(UI_HISTORY_REPLAY_PROLOGUE);
        head_fresh.process(&ring);
        let mut live = vt100::Parser::new(30, 120, 0);
        live.process(&stream);
        live.process(&after);
        assert!(
            head_fresh.screen().contents() != live.screen().contents(),
            "HEAD ring-only replay must leave hollow rows for this fixture"
        );
        // fast path: fresh == no-attach reference after the live bytes
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&seed);
        fresh.process(&after);
        let mut reference = vt100::Parser::new(30, 120, 0);
        reference.process(&stream);
        reference.process(&after);
        assert_eq!(fresh.screen().contents_formatted(), reference.screen().contents_formatted());
        assert_eq!(fresh.screen().contents(), reference.screen().contents());
        assert_eq!(fresh.screen().cursor_position(), reference.screen().cursor_position());
        assert_eq!(fresh.screen().title(), reference.screen().title());
    }

    /// A `\n` inside a control string is not a lexical boundary: a ring whose aligned front
    /// lands inside an OSC must fall back to HEAD's exact bytes (the pre-existing E.5
    /// assumption), never the fast path — and the open-suffix guard must not panic on a
    /// suffix longer than the aligned slice.
    #[test]
    fn ring_front_inside_a_string_falls_back_byte_identical() {
        let fanout = fanout();
        let id = session(&fanout);
        // 65 528 'x' bytes without newlines, then an OSC whose newline is the ring's first
        // `\n`; the ring front lands mid-OSC.
        feed(&fanout, id, &[&vec![b'x'; DEFAULT_UI_HISTORY_LIMIT_BYTES - 8]]);
        feed(&fanout, id, &[b"\x1b]0;abc\ndef"]); // OSC still open at N
        let (slice, formatted) = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            assert!(!state.history_aligned);
            let (front, back) = state.history.as_slices();
            let ring = [front, back].concat();
            let newline = ring.iter().position(|byte| *byte == b'\n').expect("a newline");
            (
                ring[newline + 1..].to_vec(),
                state.parser.screen().contents_formatted(),
            )
        };
        let data = activation_data(&fanout, id, true);
        // HEAD would seed prologue + first-line slice; Route A must fall back byte-identical
        assert!(
            !data.windows(UI_HISTORY_REPLAY_SEAM.len()).any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "mid-string front must not engage the fast path"
        );
        let mut head = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + slice.len());
        head.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
        head.extend_from_slice(&slice);
        assert_eq!(data, head);
        let _ = formatted;
    }

    /// Every determinable open-tail class: fast path trims the suffix and re-emits it
    /// EXACTLY once after the mirror, and the fresh client equals the no-attach reference
    /// after the live continuation. The partial UTF-8 tail is the one class that must fall
    /// back (frontend-validated: the CAN seam breaks xterm.js's decoder resync).
    #[test]
    fn open_tails_are_trimmed_reemitted_once_and_exact_after_live() {
        let cases: &[(&str, &[u8], &[u8])] = &[
            ("osc-bel", b"\x1b]0;partial", b" title\x07"),
            ("osc-st", b"\x1b]0;partial", b" title\x1b\\"),
            ("csi", b"\x1b[38;2;15", b"5;153;153mX"),
            ("dcs", b"\x1bP1;2|payload", b" more\x1b\\"),
            ("apc", b"\x1b_payload", b" more\x1b\\"),
            ("sos", b"\x1bXpayload", b" more\x1b\\"),
            ("pm", b"\x1b^payload", b" more\x1b\\"),
            ("charset-intermediate", b"\x1b(", b"0"), // ESC + intermediate, still open at N
            ("loose-esc", b"\x1b", b"[1;1HZ"),
        ];
        for (name, tail, continuation) in cases {
            let fanout = fanout();
            let id = session(&fanout);
            let mut base = Vec::new();
            for i in 0..30 {
                base.extend_from_slice(format!("\x1b[{i};1Hline {i:03}\x1b[K\n").as_bytes());
            }
            feed(&fanout, id, &[&base]);
            feed(&fanout, id, &[tail]);
            let data = activation_data(&fanout, id, true);
            assert!(
                data.windows(UI_HISTORY_REPLAY_SEAM.len())
                    .any(|w| w == UI_HISTORY_REPLAY_SEAM),
                "{name}: fast path must engage"
            );
            // the open suffix appears exactly once, at the end: its bytes add EXACTLY one
            // occurrence beyond the same bytes inside prologue/ring/seam/mirror (a 1-byte
            // tail like a loose ESC also matches every ESC in the content)
            let count = data.windows(tail.len()).filter(|w| *w == *tail).count();
            let rest_count = data[..data.len() - tail.len()]
                .windows(tail.len())
                .filter(|w| *w == *tail)
                .count();
            assert_eq!(count, rest_count + 1, "{name}: suffix must appear exactly once, got {count}");
            assert!(data.ends_with(tail), "{name}: suffix must be at the end");
            // equality after the live continuation vs the no-attach reference
            let mut stream = base.clone();
            stream.extend_from_slice(tail);
            let mut fresh = vt100::Parser::new(30, 120, 0);
            fresh.process(&data);
            fresh.process(continuation);
            let mut reference = vt100::Parser::new(30, 120, 0);
            reference.process(&stream);
            reference.process(continuation);
            assert_eq!(fresh.screen().contents_formatted(), reference.screen().contents_formatted(), "{name}");
            assert_eq!(fresh.screen().contents(), reference.screen().contents(), "{name}");
            assert_eq!(fresh.screen().cursor_position(), reference.screen().cursor_position(), "{name}");
            assert_eq!(fresh.screen().title(), reference.screen().title(), "{name}");
        }

        // partial UTF-8: MUST fall back (frontend-validated mojibake on the CAN seam)
        let fanout = fanout();
        let id = session(&fanout);
        let mut base = Vec::new();
        for i in 0..30 {
            base.extend_from_slice(format!("\x1b[{i};1Hline {i:03}\x1b[K\n").as_bytes());
        }
        feed(&fanout, id, &[&base]);
        feed(&fanout, id, &[b"\xe2\x82"]); // two bytes of a 3-byte char
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.windows(UI_HISTORY_REPLAY_SEAM.len()).any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "partial UTF-8 tail must fall back"
        );
        // HEAD byte-identical: prologue + full ring (the ring holds the whole stream here)
        let ring = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            let (front, back) = state.history.as_slices();
            [front, back].concat()
        };
        let mut head = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + ring.len());
        head.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
        head.extend_from_slice(&ring);
        assert_eq!(data, head);
        // and HEAD's behavior preserves the split char after live
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&data);
        fresh.process(b"\xac");
        let mut reference = vt100::Parser::new(30, 120, 0);
        let mut stream = base.clone();
        stream.extend_from_slice(b"\xe2\x82");
        reference.process(&stream);
        reference.process(b"\xac");
        assert_eq!(fresh.screen().contents(), reference.screen().contents());
        assert_eq!(fresh.screen().cursor_position(), reference.screen().cursor_position());
    }

    /// Unmodeled mode history (DECAWM, IRM, charset, LRMM, alternate, SO) is sticky:
    /// even a used-and-restored mode must fall back byte-identical to HEAD.
    #[test]
    fn unmodeled_history_falls_back_byte_identical() {
        let cases: &[(&str, &[u8])] = &[
            ("decawm-off", b"\x1b[?7l"),
            ("decawm-then-on", b"\x1b[?7l\x1b[1;1Hx\x1b[?7h"),
            ("irm", b"\x1b[4h"),
            ("irm-restored", b"\x1b[4h\x1b[4l"),
            ("charset", b"\x1b(0"),
            ("charset-restored", b"\x1b(0\x1b(B"),
            ("lrmm", b"\x1b[?69h"),
            ("alternate-active", b"\x1b[?1049h"),
            ("alternate-entered-exited", b"\x1b[?1049h\x1b[?1049l"),
            ("shift-out", b"\x0e"),
        ];
        for (name, toggle) in cases {
            let fanout = fanout();
            let id = session(&fanout);
            let mut base = Vec::new();
            for i in 0..30 {
                base.extend_from_slice(format!("line {i:03}\n").as_bytes());
            }
            feed(&fanout, id, &[&base]);
            feed(&fanout, id, &[toggle]);
            let data = activation_data(&fanout, id, true);
            assert!(
                !data.windows(UI_HISTORY_REPLAY_SEAM.len()).any(|w| w == UI_HISTORY_REPLAY_SEAM),
                "{name}: unmodeled history must fall back"
            );
            let ring = {
                let parsers = fanout.screen_parsers.lock().expect("parser state");
                let state = parsers.get(&id).expect("registered session");
                let (front, back) = state.history.as_slices();
                [front, back].concat()
            };
            let mut head = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + ring.len());
            head.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
            head.extend_from_slice(&ring);
            assert_eq!(data, head, "{name}: fallback must be byte-identical to HEAD");
        }

        // IRM activated BEFORE the ring front, still ON at N: the stream-global tracker
        // sees it (the ring never rotated it away) -> fallback; the first live byte that
        // would distinguish insert/overwrite stays HEAD-identical.
        let fanout = fanout();
        let id = session(&fanout);
        let mut base = Vec::new();
        for i in 0..(2 * 1024 / 24) {
            base.extend_from_slice(format!("line {i:05} xxxxxxxxxx\r\n").as_bytes());
        }
        feed(&fanout, id, &[&base]);
        feed(&fanout, id, &[b"\x1b[4h"]); // IRM on, before the ring window
        let mut tail = Vec::new();
        for i in 0..(DEFAULT_UI_HISTORY_LIMIT_BYTES / 24 + 64) {
            tail.extend_from_slice(format!("tail {i:05} xxxxxxxxxx\r\n").as_bytes());
        }
        feed(&fanout, id, &[&tail]);
        // sanity: the 4h bytes rotated out of the ring
        {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            let (front, back) = state.history.as_slices();
            let ring = [front, back].concat();
            assert!(!ring.windows(3).any(|w| w == b"\x1b[4h"), "4h must have rotated out");
            assert!(
                state.history.len() >= DEFAULT_UI_HISTORY_LIMIT_BYTES - 64,
                "ring must be saturated, got {}",
                state.history.len()
            );
        }
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.windows(UI_HISTORY_REPLAY_SEAM.len()).any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "IRM-on rotated outside the ring must fall back (tracker is stream-global)"
        );
        let ring = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            let (front, back) = state.history.as_slices();
            [front, back].concat()
        };
        // HEAD's fallback = prologue + the aligned replay (full ring when aligned, the
        // first-line slice otherwise) — mirror the production decision exactly
        let replay: &[u8] = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            if state.history_aligned {
                &ring
            } else {
                let newline = ring.iter().position(|byte| *byte == b'\n').expect("a newline");
                &ring[newline + 1..]
            }
        };
        let mut head = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + replay.len());
        head.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
        head.extend_from_slice(replay);
        assert_eq!(data, head, "IRM-rotated fallback must be byte-identical to HEAD");
        // live byte stream that would distinguish insert (IRM on) from overwrite (off):
        // fresh after live == no-attach reference after live (HEAD behavior preserved)
        let mut stream = base.clone();
        stream.extend_from_slice(b"\x1b[4h");
        stream.extend_from_slice(&tail);
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&data);
        fresh.process(b"\x1b[5;5HABC\x1b[5;5HD");
        let mut reference = vt100::Parser::new(30, 120, 0);
        reference.process(&stream);
        reference.process(b"\x1b[5;5HABC\x1b[5;5HD");
        assert_eq!(fresh.screen().contents(), reference.screen().contents());
        assert_eq!(fresh.screen().cursor_position(), reference.screen().cursor_position());
    }

    /// DECSTBM/DECOM are modeled by vt100: used-and-restored is admitted (final default +
    /// scratch certificate) and the fast path is exact after live; still-active region or
    /// origin at N falls back.
    #[test]
    fn decom_region_used_and_restored_is_admitted() {
        // restored: admitted
        let make_fanout = fanout;
        let fanout = make_fanout();
        let id = session(&fanout);
        let mut stream = Vec::new();
        for i in 0..30 {
            stream.extend_from_slice(format!("line {i:03}\n").as_bytes());
        }
        stream.extend_from_slice(b"\x1b[?6h\x1b[5;25r\x1b[1;1Horigin-content\x1b[r\x1b[?6l");
        stream.extend_from_slice(b"\x1b[1;1Hafter-restore\n");
        feed(&fanout, id, &[&stream]);
        let data = activation_data(&fanout, id, true);
        assert!(
            data.windows(UI_HISTORY_REPLAY_SEAM.len()).any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "used-and-restored DECSTBM/DECOM with default final must be admitted"
        );
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&data);
        fresh.process(b"\x1b[1;1Hlive\n");
        let mut reference = vt100::Parser::new(30, 120, 0);
        reference.process(&stream);
        reference.process(b"\x1b[1;1Hlive\n");
        assert_eq!(fresh.screen().contents_formatted(), reference.screen().contents_formatted());
        assert_eq!(fresh.screen().cursor_position(), reference.screen().cursor_position());

        // still trimmed at N: fallback
        let fanout2 = make_fanout();
        let id2 = session(&fanout2);
        let mut stream2 = Vec::new();
        for i in 0..30 {
            stream2.extend_from_slice(format!("line {i:03}\n").as_bytes());
        }
        stream2.extend_from_slice(b"\x1b[5;25r");
        feed(&fanout2, id2, &[&stream2]);
        let data2 = activation_data(&fanout2, id2, true);
        assert!(
            !data2
                .windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "region still trimmed at N must fall back"
        );
        let ring2 = {
            let parsers = fanout2.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id2).expect("registered session");
            let (front, back) = state.history.as_slices();
            [front, back].concat()
        };
        let mut head2 = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + ring2.len());
        head2.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
        head2.extend_from_slice(&ring2);
        assert_eq!(data2, head2);
    }

    /// Wrap-pending (exact-width row) and an active SGR at N survive the fast path after
    /// live bytes: the mirror's built-in re-poke and final attrs diff are pinned here
    /// (frontend-validated h2/h3).
    #[test]
    fn wrap_pending_and_active_sgr_survive_after_live() {
        // exact-width wrap-pending at cols=10: the live "X" must wrap like the source
        let make_fanout = fanout;
        let fanout = make_fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 3, 10)
            .expect("register narrow session");
        feed(&fanout, id, &[b"0123456789"]); // exactly 10 chars: cursor past-end at N
        let data = activation_data(&fanout, id, true);
        assert!(
            data.windows(UI_HISTORY_REPLAY_SEAM.len()).any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "narrow exact-width fixture must be eligible"
        );
        let mut fresh = vt100::Parser::new(3, 10, 0);
        fresh.process(&data);
        fresh.process(b"X");
        let mut reference = vt100::Parser::new(3, 10, 0);
        reference.process(b"0123456789X");
        assert_eq!(fresh.screen().contents(), reference.screen().contents());

        // active SGR at N: the mirror restores it; the live byte renders bold+red
        let fanout2 = make_fanout();
        let id2 = session(&fanout2);
        feed(&fanout2, id2, &[b"ab\x1b[31;1m"]);
        let data2 = activation_data(&fanout2, id2, true);
        assert!(
            data2
                .windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "SGR-active fixture must be eligible"
        );
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&data2);
        let (row, col) = fresh.screen().cursor_position();
        assert!(fresh.screen().bold(), "mirror must restore the active SGR state");
        assert_eq!(fresh.screen().fgcolor(), vt100::Color::Idx(1));
        fresh.process(b"X");
        let cell = fresh.screen().cell(row, col).expect("cell at the cursor");
        assert_eq!(cell.contents(), "X");
        assert!(cell.bold(), "live byte must render with the restored SGR");
        assert_eq!(cell.fgcolor(), vt100::Color::Idx(1));
    }

    /// The tail tracker applies modes with the TRUE final byte (split across chunk
    /// boundaries), and aborted sequences never apply modes or mark sticky. Pins the
    /// Root review fix: `ESC[?7l` must reach the mode logic as `...l`, not `...7`.
    #[test]
    fn tail_tracker_applies_modes_per_byte_and_aborts_do_not() {
        fn feed_bytewise(fanout: &SessionIoFanout, id: Uuid, seq: &[u8]) {
            for byte in seq {
                feed(fanout, id, &[&[*byte]]);
            }
        }
        // (state, decawm, irm, g0, decom, region, buffer_alt, shift, lrmm, sticky, lost)
        let t = |fanout: &SessionIoFanout, id: Uuid| {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            (
                state.tail.state,
                state.tail.decawm,
                state.tail.irm,
                state.tail.g0,
                state.tail.decom,
                state.tail.region,
                state.tail.buffer_alternate,
                state.tail.shift_out,
                state.tail.lrmm,
                state.tail.sticky,
                state.tail.lost,
            )
        };

        let make_fanout = fanout;
        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x1b[?7l");
        let s = t(&fanout, id);
        assert!(!s.1 && s.9, "?7l: DECAWM off + sticky (final byte l applied)");
        feed_bytewise(&fanout, id, b"\x1b[?7h");
        let s = t(&fanout, id);
        assert!(s.1 && s.9, "?7h: DECAWM on, sticky stays");

        feed_bytewise(&fanout, id, b"\x1b[4h");
        let s = t(&fanout, id);
        assert!(s.2 && s.9, "4h: IRM on + sticky");
        // CAN aborts an incomplete IRM set: prior state must stay
        feed_bytewise(&fanout, id, b"\x1b[4");
        feed_bytewise(&fanout, id, b"\x18");
        let s = t(&fanout, id);
        assert!(s.2 && s.9, "aborted incomplete 4: prior IRM+sticky must stay");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x1b[4\x18"); // CAN aborts the incomplete 4h
        let s = t(&fanout, id);
        assert!(!s.2 && !s.9, "aborted 4h: IRM NOT applied, NO sticky");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x1b[?6h");
        let s = t(&fanout, id);
        assert!(s.4 && !s.9, "?6h: DECOM on, NOT sticky (modeled)");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x1b[?69h");
        let s = t(&fanout, id);
        assert!(s.8 && s.9, "?69h: LRMM on + sticky");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x1b[?1049h");
        let s = t(&fanout, id);
        assert!(s.6 && s.9, "?1049h: alternate + sticky");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x1b(0");
        let s = t(&fanout, id);
        assert!(s.3 == b'0' && s.9, "ESC(0: G0 special + sticky");
        feed_bytewise(&fanout, id, b"\x1b(B");
        let s = t(&fanout, id);
        assert!(s.3 == b'B' && s.9, "ESC(B: G0 ASCII, sticky stays");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x1b[4h");
        feed_bytewise(&fanout, id, b"\x1bc"); // completed RIS: proven checkpoint
        let s = t(&fanout, id);
        assert!(s.1 && !s.2 && s.3 == b'B' && !s.9, "completed RIS resets everything incl. sticky");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x1b[4h");
        feed_bytewise(&fanout, id, b"\x1b");
        feed_bytewise(&fanout, id, b"\x18"); // RIS aborted by CAN
        let s = t(&fanout, id);
        assert!(s.2 && s.9, "aborted RIS must NOT clear the checkpoint (IRM+sticky stay)");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x1b[5;25r");
        let s = t(&fanout, id);
        assert!(s.5 == (Some(5), Some(25)) && !s.9, "DECSTBM applied, NOT sticky");
        feed_bytewise(&fanout, id, b"\x1b[r");
        let s = t(&fanout, id);
        assert!(s.5 == (None, None), "bare CSI r restores full margins");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x0e");
        let s = t(&fanout, id);
        assert!(s.7 && s.9, "SO: shift out + sticky");
        feed_bytewise(&fanout, id, b"\x0f");
        let s = t(&fanout, id);
        assert!(!s.7 && s.9, "SI: shift in, sticky stays (historical)");

        let fanout = make_fanout();
        let id = session(&fanout);
        feed_bytewise(&fanout, id, b"\x9b"); // raw C1: mirror divergence -> fail closed (Root correction)
        let s = t(&fanout, id);
        assert!(
            s.0 == TailState::Ground && s.9 && !s.10,
            "raw C1: ground + sticky, NOT lost (client and vt100 disagree on it)"
        );
    }

    /// Root blocking review, test 1: the ring front lands inside a TERMINATED control
    /// string whose opening was evicted; the first `\n` in the window is the string's
    /// payload LF, so only the position AFTER the terminator is a valid boundary. The
    /// replay must start there (the payload is dropped), the fast path engages, and the
    /// fresh client equals the no-attach reference after the live bytes.
    #[test]
    fn ring_front_inside_a_terminated_string_advances_to_the_post_terminator_boundary() {
        let fanout = fanout();
        let id = session(&fanout);
        // 1000 'x' bytes rotate out so the ring front lands inside the OSC; the payload's
        // LF is the first newline in the window (a raw-LF boundary that must NOT be
        // trusted); the BEL terminates the OSC inside the ring.
        feed(&fanout, id, &[&vec![b'x'; 1_000]]);
        feed(&fanout, id, &[b"\x1b]0;ab\ncd\x07"]);
        let mut lines = Vec::new();
        for i in 0..2_849 {
            lines.extend_from_slice(format!("tail {i:05} xxxxxxxxxx\r\n").as_bytes()); // 23 B
        }
        feed(&fanout, id, &[&lines]); // total ≈ 65.5 KiB + OSC; front lands inside the OSC
        let data = activation_data(&fanout, id, true);
        assert!(
            data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "fast path must engage with the advanced boundary"
        );
        // the replay portion starts with the first line, NOT with the OSC payload
        let body = &data[UI_HISTORY_REPLAY_PROLOGUE.len()..];
        assert!(
            body.starts_with(b"tail 00000"),
            "replay must start after the string terminator, got {:?}",
            &body[..body.len().min(20)]
        );
        // equality after live vs the no-attach reference
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&data);
        fresh.process(b"\x1b[1;1Hlive\n");
        let mut reference = vt100::Parser::new(30, 120, 0);
        reference.process(&[b'x'; 1_000]);
        reference.process(b"\x1b]0;ab\ncd\x07");
        reference.process(&lines);
        reference.process(b"\x1b[1;1Hlive\n");
        assert_eq!(fresh.screen().contents_formatted(), reference.screen().contents_formatted());
        // title deliberately not compared: the string opening was evicted, so the replay
        // cannot restore it (pre-existing, cosmetic window chrome — HEAD loses it too).
        assert_eq!(fresh.screen().cursor_position(), reference.screen().cursor_position());
    }

    /// Root blocking review, test 2: payload that, misread from Ground, would paint
    /// EXACTLY cells the live grid already holds. The scratch certificate ALONE would
    /// pass (false positive — demonstrated); the boundary proof is what rejects the
    /// misparse: the payload is dropped from the replay, never painted.
    #[test]
    fn mid_string_payload_that_would_false_positive_the_scratch_is_dropped() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[&vec![b'x'; 1_000]]);
        // payload, read from Ground, paints `line 0000` — identical to a real live line
        feed(&fanout, id, &[b"\x1b]0;ab\nline 0000\x07"]);
        let mut lines = Vec::new();
        for i in 0..5_957 {
            lines.extend_from_slice(format!("line {i:04}\r\n").as_bytes()); // 11 B; total > 64 KiB, front inside the OSC
        }
        feed(&fanout, id, &[&lines]);
        // the misparse WOULD pass the subset certificate (false positive)
        let ring = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            let (front, back) = state.history.as_slices();
            [front, back].concat()
        };
        let mut live = vt100::Parser::new(30, 120, 0);
        live.process(&[b'x'; 1_000]);
        live.process(b"\x1b]0;ab\nline 0000\x07");
        live.process(&lines);
        let misparsed_passes = scratch_certificate(30, 120, &ring, None, live.screen());
        assert!(
            misparsed_passes,
            "the fixture must demonstrate the scratch false positive"
        );
        let data = activation_data(&fanout, id, true);
        // the boundary proof rejects the misparse: the payload is NOT replayed
        let body = &data[UI_HISTORY_REPLAY_PROLOGUE.len()..];
        assert!(
            body.starts_with(b"line "),
            "replay must start after the BEL (payload dropped), got {:?}",
            &body[..body.len().min(20)]
        );
    }

    /// Root blocking review, test 3: bytes that would activate an unmodeled mode when
    /// misread from Ground must never reach the fast path. Note the parser semantics: an
    /// ESC inside a control string TERMINATES the string (both vte and xterm.js), so the
    /// `?7l` after it is a REAL sequence — the tracker applies it (DECAWM off + sticky),
    /// the eligibility rejects, and the fallback is byte-identical to HEAD.
    #[test]
    fn mid_string_payload_with_a_mode_toggle_is_never_replayed() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[&vec![b'x'; 1_000]]);
        feed(&fanout, id, &[b"\x1b]0;ab\n\x1b[?7lcd\x07"]); // the LF is inside the OSC; the ESC ends it
        let mut lines = Vec::new();
        for i in 0..5_957 {
            lines.extend_from_slice(format!("line {i:04}\r\n").as_bytes()); // 11 B; total > 64 KiB, front inside the OSC
        }
        feed(&fanout, id, &[&lines]);
        let data = activation_data(&fanout, id, true);
        // the tracker applied the real `?7l` (the ESC terminated the string) -> sticky ->
        // the fast path is rejected, never a misparse of the payload
        assert!(
            !data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "mode-activating bytes after a string LF must reject the fast path"
        );
        // HEAD byte-identical: prologue + the aligned replay (the whole ring here)
        let ring = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            let (front, back) = state.history.as_slices();
            [front, back].concat()
        };
        let mut head = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + ring.len());
        head.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
        head.extend_from_slice(&ring);
        if data != head {
            let first = data.iter().zip(head.iter()).position(|(a, b)| a != b);
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let st = parsers.get(&id).expect("registered session");
            eprintln!("T3DBG aligned={} first_div={:?} data_head={:?} head_data={:?} ring_front={:?}", st.history_aligned, first, first.map(|i| &data[i..(i + 10).min(data.len())]), first.map(|i| &head[i..(i + 10).min(head.len())]), st.history.iter().take(8).copied().collect::<Vec<u8>>());
        }
        assert_eq!(data, head, "fallback must be byte-identical to HEAD; data={} head={}", data.len(), head.len());
    }

    /// Root blocking review, test 4: an OSC whose ST (ESC \) is split across chunk
    /// boundaries and across the VecDeque front/back halves must keep the front-states
    /// mirror in lockstep: the only valid boundary is after the completed ST.
    #[test]
    fn split_st_keeps_the_front_states_in_lockstep() {
        let fanout = fanout();
        let id = session(&fanout);
        // a long filler so the ring rotates; the OSC opening at the front boundary
        feed(&fanout, id, &[&vec![b'y'; 61_000]]);
        feed(&fanout, id, &[b"\x1b]0;ab"]);
        feed(&fanout, id, &[b"cd\x1b\\"]); // ST split across chunks
        let mut lines = Vec::new();
        for i in 0..600 {
            lines.extend_from_slice(format!("line {i:04}\r\n").as_bytes());
        }
        feed(&fanout, id, &[&lines]);
        let data = activation_data(&fanout, id, true);
        // the fast path must engage and the split-ST OSC must parse correctly through the
        // replay (the front-states mirror stays in lockstep across chunk splits)
        assert!(
            data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "split-ST stream must stay eligible"
        );
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&data);
        fresh.process(b"\x1b[1;1Hlive\n");
        let mut reference = vt100::Parser::new(30, 120, 0);
        reference.process(&[b'y'; 61_000]);
        reference.process(b"\x1b]0;ab");
        reference.process(b"cd\x1b\\"); // ESC + backslash = ST
        reference.process(&lines);
        reference.process(b"\x1b[1;1Hlive\n");
        assert_eq!(fresh.screen().contents_formatted(), reference.screen().contents_formatted());
        // title deliberately not compared: the OSC sits before the replay start (the ring
        // cannot restore it — pre-existing, cosmetic; HEAD loses it identically).
        assert_eq!(fresh.screen().cursor_position(), reference.screen().cursor_position());
    }

    /// Root blocking review, test 5: an oversized chunk discards a string opening AND its
    /// payload in the same call; the ring starts at a genuine Ground position (the lines)
    /// and the fast path engages with the full ring.
    #[test]
    fn oversized_chunk_discarding_an_open_string_keeps_the_ground_boundary() {
        let fanout = fanout();
        let id = session(&fanout);
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"\x1b]0;ab\x07"); // opening + payload (BEL-terminated) in the discarded prefix
        for i in 0..(DEFAULT_UI_HISTORY_LIMIT_BYTES / 11 + 64) {
            chunk.extend_from_slice(format!("line {i:04}\r\n").as_bytes());
        }
        assert!(chunk.len() > DEFAULT_UI_HISTORY_LIMIT_BYTES);
        feed(&fanout, id, &[&chunk]); // ONE chunk, oversized
        let data = activation_data(&fanout, id, true);
        assert!(
            data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "the discarded-prefix ring must stay eligible (front at Ground)"
        );
        let body = &data[UI_HISTORY_REPLAY_PROLOGUE.len()..];
        assert!(
            body.starts_with(b"line "),
            "replay must start with the lines, got {:?}",
            &body[..body.len().min(20)]
        );
        assert!(
            !body.windows(9).any(|w| w == b"\x1b]0;ab\x07"),
            "the discarded OSC bytes must not appear in the replay"
        );
        // equality after live vs the no-attach reference
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&data);
        fresh.process(b"\x1b[1;1Hlive\n");
        let mut reference = vt100::Parser::new(30, 120, 0);
        reference.process(&chunk);
        reference.process(b"\x1b[1;1Hlive\n");
        assert_eq!(fresh.screen().contents_formatted(), reference.screen().contents_formatted());
        assert_eq!(fresh.screen().cursor_position(), reference.screen().cursor_position());
    }

    /// Root blocking review, test 6: an OPEN string at N (never terminated) leaves NO
    /// Ground position after the front — the fast path must fall back byte-identical to
    /// HEAD (the existing `ring_front_inside_a_string` fixture now exercises the
    /// `no_ground_boundary` reason), and the Codex fixture must still engage.
    #[test]
    fn open_string_at_n_has_no_ground_boundary_and_falls_back() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[&vec![b'x'; DEFAULT_UI_HISTORY_LIMIT_BYTES - 8]]);
        feed(&fanout, id, &[b"\x1b]0;abc\ndef"]); // OSC never terminated
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "an open string with no Ground in the window must fall back"
        );
        // HEAD byte-identical: prologue + the first-line slice (the only replay HEAD has)
        let ring = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            let (front, back) = state.history.as_slices();
            [front, back].concat()
        };
        let newline = ring.iter().position(|byte| *byte == b'\n').expect("a newline");
        let mut head = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + ring.len() - newline - 1);
        head.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
        head.extend_from_slice(&ring[newline + 1..]);
        assert_eq!(data, head, "fallback must be byte-identical to HEAD");
    }

    /// RIS must reset the terminal state WITHOUT breaking the front_states lockstep
    /// (Root RIS gate): the invariant `front_states.len() == history.len()` holds across
    /// RIS at the start/middle/end, trims and oversized chunks, and the fast path still
    /// works after a mid-stream RIS.
    #[test]
    fn ris_preserves_front_states_lockstep() {
        fn assert_lockstep(fanout: &SessionIoFanout, id: Uuid) {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let st = parsers.get(&id).expect("registered session");
            assert_eq!(
                st.tail.front_states.len(),
                st.history.len(),
                "front-states must mirror the ring byte-for-byte"
            );
        }
        let fanout = fanout();
        let id = session(&fanout);
        // RIS at the START (empty ring)
        feed(&fanout, id, &[b"\x1bc"]);
        assert_lockstep(&fanout, id);
        // RIS in the MIDDLE of a partial ring, pre-RIS bytes still in it
        feed(&fanout, id, &[b"line one\r\n"]);
        feed(&fanout, id, &[b"\x1bc"]);
        feed(&fanout, id, &[b"line two\r\n"]);
        assert_lockstep(&fanout, id);
        // saturated ring + a RIS + more (subsequent trim exercises the drains)
        let mut filler = Vec::new();
        for i in 0..(DEFAULT_UI_HISTORY_LIMIT_BYTES / 11) {
            filler.extend_from_slice(format!("fill {i:05} x\r\n").as_bytes());
        }
        feed(&fanout, id, &[&filler]);
        feed(&fanout, id, &[b"\x1bc"]);
        for i in 0..100 {
            feed(&fanout, id, &[format!("post {i:03}\r\n").as_bytes()]);
        }
        assert_lockstep(&fanout, id);
        // oversized chunk after the RIS
        let mut big = Vec::new();
        for i in 0..(DEFAULT_UI_HISTORY_LIMIT_BYTES / 11 + 64) {
            big.extend_from_slice(format!("big {i:05} x\r\n").as_bytes());
        }
        feed(&fanout, id, &[&big]);
        assert_lockstep(&fanout, id);
        // the stream after the RIS is fresh: eligible, fast path engages
        let data = activation_data(&fanout, id, true);
        assert!(
            data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "a post-RIS stream must stay eligible (RIS is the proven checkpoint)"
        );
    }

    /// A/B F2 (final design): the re-poke is CONDITIONAL on the source's preceding
    /// state at N — active (last byte a printable) → the seed ends with
    /// `CUP(glyph cell) + glyph`; reset (last byte a CSI/ESC/C0 dispatch) → NO re-poke
    /// (the seam + the mirror's own sequences already leave the client reset, matching
    /// the source). xterm repeats the glyph empirically for non-ASCII too, so the
    /// tracker records the raw multi-byte glyph.
    #[test]
    fn rep_preceding_state_drives_the_conditional_repoke() {
        fn seed_ends_with_repoke(data: &[u8], glyph: &[u8]) -> bool {
            data.windows(glyph.len()).any(|w| w == glyph) && data.ends_with(glyph)
        }
        let make_fanout = fanout;
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"A"]); // preceding ACTIVE at N
        let data = activation_data(&fanout, id, true);
        assert!(
            seed_ends_with_repoke(&data, b"A"),
            "active preceding: the seed must end with the glyph re-poke"
        );
        // the re-poke's CUP targets the glyph's cell: `\x1b[1;1H` + `A` at the end
        assert!(data.ends_with(b"\x1b[1;1HA"));

        // reset case: ASCII then a CUP dispatch — the seed must NOT re-poke
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"A\x1b[1;1H"]); // CUP resets the preceding
        let data = activation_data(&fanout, id, true);
        assert!(
            !seed_ends_with_repoke(&data, b"A"),
            "reset preceding: the seed must NOT re-poke (the mirror's sequences already reset it)"
        );

        // C0 reset case: ASCII then CR
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"AB\r"]);
        let data = activation_data(&fanout, id, true);
        assert!(!seed_ends_with_repoke(&data, b"B"));
        assert!(!seed_ends_with_repoke(&data, b"A"));

        // non-ASCII last graphic: the re-poke carries the REAL multi-byte glyph
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &["Aé".as_bytes()]); // ends with U+00E9 (2 bytes)
        let data = activation_data(&fanout, id, true);
        assert!(
            seed_ends_with_repoke(&data, "é".as_bytes()),
            "non-ASCII preceding: the re-poke must carry the decoded glyph"
        );

        // wrap-pending P5: glyph at the last column -> re-poke at (row, cols-1)
        let fanout = make_fanout();
        let id = Uuid::new_v4();
        fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 3, 10)
            .expect("register narrow session");
        feed(&fanout, id, &[b"0123456789"]); // glyph `9` at the last column
        let data = activation_data(&fanout, id, true);
        assert!(
            data.ends_with(b"\x1b[1;10H9"),
            "wrap-pending re-poke: CUP(row, cols-1) + glyph restores preceding AND pending wrap"
        );
    }

    /// A/B F4: a string aborted by an ESC leaves a pending sequence the real client
    /// continues literally — the fast path is excluded while it is open (fallback
    /// byte-identical to HEAD), including the ESC/`[` split across chunks.
    #[test]
    fn f4_esc_aborted_string_excludes_the_fast_path() {
        for stream in [
            &b"\x1b]0;title\x1b[31".to_vec(), // OSC aborted by ESC, CSI PENDING at N
            &b"\x1bP1;2|payload\x1b[".to_vec(), // DCS aborted, ESC[ split pending
            &b"\x1b_title\x1b[1;1".to_vec(),   // APC aborted, CUP pending
        ] {
            let fanout = fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[b"line one\r\n"]);
            feed(&fanout, id, &[stream]);
            let data = activation_data(&fanout, id, true);
            assert!(
                !data.windows(UI_HISTORY_REPLAY_SEAM.len())
                    .any(|w| w == UI_HISTORY_REPLAY_SEAM),
                "an ESC-aborted string pending at N must exclude the fast path"
            );
            // HEAD byte-identical (the ring holds the whole stream here)
            let ring = {
                let parsers = fanout.screen_parsers.lock().expect("parser state");
                let state = parsers.get(&id).expect("registered session");
                let (front, back) = state.history.as_slices();
                [front, back].concat()
            };
            let mut head = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + ring.len());
            head.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
            head.extend_from_slice(&ring);
            assert_eq!(data, head, "F4 fallback must be byte-identical to HEAD");
        }
    }

    /// A/B F1 + Root correction: C1 via UTF-8 (`C2 9B` etc.) and bare C1 both fail
    /// closed — the client and the backend parser disagree on their semantics, so the
    /// mirror cannot be trusted; the session falls back byte-identically.
    #[test]
    fn f1_c1_via_utf8_and_raw_c1_fail_closed() {
        let make_fanout = fanout;
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"\xc2\x9b?7l"]); // C2 9B = U+009B: executed by the client, not by vt100
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let st = parsers.get(&id).expect("registered session");
        assert!(st.tail.sticky, "C2 9B must fail closed (mirror divergence)");
        drop(parsers);
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "C1-via-UTF-8 stream must fall back"
        );
        // raw C1 0x80..0x9F from ingest: same fail-close
        for raw in [b"\x9b31mX".as_slice(), b"\x9dtitle", b"\x90payload", b"\x9c"] {
            let fanout = make_fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[b"line one\r\n"]);
            feed(&fanout, id, &[raw]);
            let data = activation_data(&fanout, id, true);
            assert!(
                !data.windows(UI_HISTORY_REPLAY_SEAM.len())
                    .any(|w| w == UI_HISTORY_REPLAY_SEAM),
                "raw C1 {raw:?} must fail closed"
            );
            let ring = {
                let parsers = fanout.screen_parsers.lock().expect("parser state");
                let state = parsers.get(&id).expect("registered session");
                let (front, back) = state.history.as_slices();
                [front, back].concat()
            };
            let mut head = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + ring.len());
            head.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
            head.extend_from_slice(&ring);
            assert_eq!(data, head, "raw C1 fallback must be byte-identical to HEAD");
        }
    }

    /// ST/PGC and REP-dispatch gates (measured xterm): a completed string termination
    /// (OSC_END/DCS_UNHOOK by BEL/ST/CAN/SUB) and a completed REP (`CSI Ps b` — the
    /// CSI_DISPATCH itself) both reset the preceding-graphic state. The snapshot just
    /// BEFORE the REP's final `b` keeps the PGC active (the re-poke applies); just
    /// AFTER the completion it is reset (no re-poke).
    #[test]
    fn pgc_resets_on_string_termination_and_rep_dispatch() {
        let make_fanout = fanout;
        // X + OSC + ST: the ST termination resets the PGC
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b]0;t\x1b\\"]);
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.ends_with(b"\x1b[1;1HX"),
            "ST termination must reset the PGC (no re-poke)"
        );
        // X + OSC + BEL
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b]0;t\x07"]);
        let data = activation_data(&fanout, id, true);
        assert!(!data.ends_with(b"\x1b[1;1HX"), "BEL termination must reset the PGC");
        // X + DCS + ST, X + APC + CAN
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1bP1;2|t\x1b\\"]);
        let data = activation_data(&fanout, id, true);
        assert!(!data.ends_with(b"\x1b[1;1HX"), "DCS ST must reset the PGC");
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b_t\x18"]);
        let data = activation_data(&fanout, id, true);
        assert!(!data.ends_with(b"\x1b[1;1HX"), "APC CAN must reset the PGC");
        // inner-ESC families (Root correction): OSC/DCS reset the PGC AT the ESC;
        // SOS/PM/APC (ignored strings) keep it ACTIVE until the next dispatch
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b]0;t\x1b"]); // OSC + inner ESC: OSC_END already reset
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.ends_with(b"\x1b[1;1HX"),
            "OSC inner ESC must reset the PGC (OSC_END)"
        );
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1bP1;2|t\x1b"]); // DCS + inner ESC: DCS_UNHOOK reset
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.ends_with(b"\x1b[1;1HX"),
            "DCS inner ESC must reset the PGC (DCS_UNHOOK)"
        );
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b_t\x1b"]); // APC + inner ESC
        let data = activation_data(&fanout, id, true);
        // The F4 exclusion subsumes the PGC nuance: ANY ESC-aborted string excludes the
        // fast path, so the seed is the byte-identical fallback (the PGC-active case for
        // the ignored strings never reaches a re-poke decision).
        assert!(
            !data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "APC inner ESC must exclude the fast path (F4)"
        );
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b^t\x1b"]); // PM: same as APC
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "PM inner ESC must exclude the fast path (F4)"
        );
        // completed REP resets the PGC: X + CSI 2b
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b[2b"]);
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.ends_with(b"\x1b[1;1HX"),
            "a completed REP resets the PGC (CSI_DISPATCH)"
        );
        // snapshot just BEFORE the REP's final `b`: PGC still active, the re-poke applies
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b[2"]);
        let data = activation_data(&fanout, id, true);
        assert!(
            data.ends_with(b"\x1b[1;1HX\x1b[2"),
            "before the REP dispatch the PGC is active: re-poke + deferred suffix"
        );
    }


    /// Cross-validation p3: a completed `CSI Ps b` (REP) in the ring makes the
    /// authoritative screen diverge (vt100 0.15.2 has no REP handler — the REP cells
    /// are lost) — the session must fall back byte-identically.
    #[test]
    fn rep_in_ring_is_unmodelled_and_falls_back() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b[2b"]); // historical REP completes
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.windows(UI_HISTORY_REPLAY_SEAM.len())
                .any(|w| w == UI_HISTORY_REPLAY_SEAM),
            "a completed REP must fail closed (unmodelled cells)"
        );
        let ring = {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let state = parsers.get(&id).expect("registered session");
            let (front, back) = state.history.as_slices();
            [front, back].concat()
        };
        let mut head = Vec::with_capacity(UI_HISTORY_REPLAY_PROLOGUE.len() + ring.len());
        head.extend_from_slice(UI_HISTORY_REPLAY_PROLOGUE);
        head.extend_from_slice(&ring);
        assert_eq!(data, head, "REP-in-ring fallback must be byte-identical to HEAD");
    }

    /// Cross-validation n6 (frontend repro `sem2.html`): the ring ends with a PENDING
    /// CSI (`\x1b[`) after an SGR on an empty screen; the suffix is trimmed and
    /// re-emitted, the PGC-None CAN is appended, and the live `5b` completes the REP
    /// with the PGC already reset — no-op, equal to the no-attach reference.
    #[test]
    fn pending_csi_tail_after_sgr_dispatch_stays_gt() {
        let fanout = fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"\x1b[H\x1b[2J\x1b[31m\x1b["]); // ring from the frontend repro
        let data = activation_data(&fanout, id, true);
        // the seed must trim the pending CSI and re-emit it after the PGC restoration
        assert!(
            data.ends_with(b"\x18\x1b["),
            "seed must end CAN + re-emitted suffix, got {:?}",
            &data[data.len().saturating_sub(8)..]
        );
        // the fresh client + the live continuation == the no-attach reference
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&data);
        fresh.process(b"5b");
        let mut reference = vt100::Parser::new(30, 120, 0);
        reference.process(b"\x1b[H\x1b[2J\x1b[31m\x1b[5b");
        assert_eq!(fresh.screen().contents_formatted(), reference.screen().contents_formatted());
        assert_eq!(fresh.screen().cursor_position(), reference.screen().cursor_position());
    }

    /// C0 action matrix (measured xterm): the ordinary C0s EXECUTE (reset the PGC)
    /// only in Ground/Escape/CSI; inside OSC they are IGNORED payload, inside
    /// DCS/SOS-PM-APC they are DCS_PUT/IGNORE — both PRESERVE the PGC. The counter-
    /// example `X + OSC-open + NUL` must keep the preceding glyph.
    #[test]
    fn c0_action_matrix_is_state_specific() {
        let make_fanout = fanout;
        // X + OSC-open + NUL: the PGC must survive the NUL (OSC IGNOREs it)
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b]0;t\x00"]);
        let data = activation_data(&fanout, id, true);
        assert!(
            data.ends_with(b"\x1b[1;1HX\x1b]0;t\x00"),
            "OSC-open + NUL must preserve the PGC (re-poke + deferred suffix)"
        );
        // X + DCS-open + BEL: the BEL is DCS_PUT payload — the PGC survives
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1bP1;2|t\x07"]);
        let data = activation_data(&fanout, id, true);
        assert!(
            data.ends_with(b"\x1b[1;1HX\x1bP1;2|t\x07"),
            "DCS-open + BEL must preserve the PGC"
        );
        // X + SOS-open + NUL: IGNORE payload, preserved
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1bX t\x00"]);
        let data = activation_data(&fanout, id, true);
        assert!(
            data.ends_with(b"\x1b[1;1HX\x1bX t\x00"),
            "SOS-open + NUL must preserve the PGC"
        );
        // X + CSI-open + NUL: the CSI state EXECUTEs the NUL — the PGC resets
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b[1\x00"]);
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.ends_with(b"\x1b[1;1HX"),
            "CSI-open + NUL must reset the PGC (C0 EXECUTE)"
        );
        // X + ESC-open + NUL: same EXECUTE reset, the escape continues
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b\x00"]);
        let data = activation_data(&fanout, id, true);
        assert!(
            !data.ends_with(b"\x1b[1;1HX"),
            "ESC-open + NUL must reset the PGC (C0 EXECUTE)"
        );
    }

    /// Root tracker gaps (083010): (1) ALL Ground C0 executables reset the PGC;
    /// (2) SO/SI reset it while keeping the shift side effect; (3) raw C1 is sticky in
    /// EVERY state; (4) UTF-8 scalar validation (overlong/surrogate/>max) + the
    /// interrupted byte is redispatched exactly once.
    #[test]
    fn tracker_c0_sweep_raw_c1_and_utf8_scalars() {
        let make_fanout = fanout;
        let t = |fanout: &SessionIoFanout, id: Uuid| {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let st = parsers.get(&id).expect("registered session");
            (
                st.tail.last_graphic.clone(),
                st.tail.shift_out,
                st.tail.sticky,
                st.tail.lost,
            )
        };
        // (1) every Ground C0 (0x00..=0x1f except ESC) after a print resets the PGC
        for c0 in 0x00u8..=0x1f {
            if c0 == 0x1b {
                continue;
            }
            let fanout = make_fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[b"X"]);
            feed(&fanout, id, &[&[c0]]);
            let s = t(&fanout, id);
            assert!(
                s.0.is_none(),
                "Ground C0 0x{c0:02x} must reset the PGC"
            );
        }
        // (2) SO/SI reset the PGC AND keep the shift side effect
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x0e"]);
        let s = t(&fanout, id);
        assert!(s.0.is_none() && s.1, "SO must reset the PGC and set shift_out");
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x0f"]);
        let s = t(&fanout, id);
        assert!(s.0.is_none() && !s.1, "SI must reset the PGC and clear shift_out");
        // (3) SOS with the C1-ST (0x9c) PRESERVES the PGC; DCS with 0x9c resets it
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1b_t\x9c"]);
        let s = t(&fanout, id);
        assert!(s.0.is_some(), "SOS + 0x9c must preserve the PGC (IGNORE -> Ground)");
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"X\x1bP1;2|t\x9c"]);
        let s = t(&fanout, id);
        assert!(s.0.is_none(), "DCS + 0x9c must reset the PGC (UNHOOK)");
        // raw C1 is sticky in EVERY state (Ground/OSC/DCS/SOS/CSI)
        for (tag, prefix) in [
            ("ground", b"".as_slice()),
            ("osc", b"\x1b]0;t".as_slice()),
            ("dcs", b"\x1bP1;2|t".as_slice()),
            ("sos", b"\x1b_t".as_slice()),
            ("csi", b"\x1b[1;1".as_slice()),
        ] {
            let fanout = make_fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[b"X"]);
            feed(&fanout, id, &[prefix]);
            feed(&fanout, id, &[b"\x9b"]);
            let s = t(&fanout, id);
            assert!(s.2, "raw C1 in {tag} must fail closed");
        }
        // (4) UTF-8 scalar validation: overlong/surrogate/>max/invalid-lead -> sticky
        for (tag, bytes) in [
            ("overlong", b"\xe0\x80\x80".as_slice()),
            ("surrogate", b"\xed\xa0\x80".as_slice()),
            ("too-big", b"\xf4\x90\x80\x80".as_slice()),
            ("invalid-lead", b"\xf5".as_slice()),
        ] {
            let fanout = make_fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[bytes]);
            let s = t(&fanout, id);
            assert!(s.2, "{tag} UTF-8 must fail closed");
        }
        // interrupted char: the non-continuation byte is redispatched exactly once
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"\xc3"]); // partial é
        feed(&fanout, id, &[b"Z"]); // not a continuation: sticky + Z processed as a graphic
        let s = t(&fanout, id);
        assert!(s.2, "malformed UTF-8 must fail closed");
        assert_eq!(s.0.as_deref(), Some(b"Z".as_slice()), "the interruptor must be redispatched");
        // Root three-forms fix: lone C0/C1 leads and lone A0..BF continuations in
        // Ground fail closed
        for (tag, byte) in [("c0-lead", 0xc0u8), ("c1-lead", 0xc1), ("a0-cont", 0xa0), ("bf-cont", 0xbf)] {
            let fanout = make_fanout();
            let id = session(&fanout);
            feed(&fanout, id, &[&[byte]]);
            let s = t(&fanout, id);
            assert!(s.2, "{tag} must fail closed");
        }
        // ESC truncating a partial scalar: sticky + the ESC redispatched into Escape
        // so `ESC [ 31 m` still applies the SGR in the tracker
        let fanout = make_fanout();
        let id = session(&fanout);
        feed(&fanout, id, &[b"\xc3\x1b[31m"]);
        let parsers = fanout.screen_parsers.lock().expect("parser state");
        let st = parsers.get(&id).expect("registered session");
        assert!(st.tail.sticky, "ESC truncating a scalar must fail closed");
        assert_eq!(
            st.tail.state,
            TailState::Ground,
            "the redispatched ESC must land in Escape and the CSI must complete"
        );
        assert!(
            st.tail.pending.is_empty(),
            "the CSI after the redispatch must be consumed"
        );
        drop(parsers);
    }


    /// Configurable history limit (project settings): a session registered with a
    /// custom limit trims its ring at that limit, keeps the front-states lockstep,
    /// and seeds within the custom limit; a second session keeps its own default.
    #[test]
    fn custom_history_limit_trims_and_seeds_per_session() {
        let make_fanout = fanout;
        let fanout = make_fanout();
        let token = fanout
            .register_session(
                Uuid::new_v4(),
                IdleTuning::DEFAULT,
                30,
                120,
                8_192,
                PtyOutputTarget::noop(),
            )
            .expect("register custom-limit session");
        let id = token.identity.session_id;
        let mut filler = Vec::new();
        for i in 0..2_000 {
            filler.extend_from_slice(format!("line {i:05} x\r\n").as_bytes()); // 13 B
        }
        feed(&fanout, id, &[&filler]); // 26 KiB >> 8 KiB
        {
            let parsers = fanout.screen_parsers.lock().expect("parser state");
            let st = parsers.get(&id).expect("registered session");
            assert_eq!(st.history_limit_bytes, 8_192);
            assert!(
                st.history.len() <= 8_192,
                "ring must trim at the session limit, got {}",
                st.history.len()
            );
            assert_eq!(
                st.tail.front_states.len(),
                st.history.len(),
                "front-states lockstep with the custom limit"
            );
        }
        let data = activation_data(&fanout, id, true);
        assert!(
            data.len() < 8_192 + 2_048,
            "the seed stays within the custom limit, got {}",
            data.len()
        );
        // isolation: a default-limit session trims at 64 KiB, not at the custom one
        let fanout2 = make_fanout();
        let id2 = session(&fanout2);
        let mut filler = Vec::new();
        for i in 0..2_000 {
            filler.extend_from_slice(format!("line {i:05} x\r\n").as_bytes());
        }
        feed(&fanout2, id2, &[&filler]);
        let parsers = fanout2.screen_parsers.lock().expect("parser state");
        let st = parsers.get(&id2).expect("registered session");
        assert_eq!(st.history_limit_bytes, DEFAULT_UI_HISTORY_LIMIT_BYTES);
        assert!(
            st.history.len() > 8_192 && st.history.len() <= DEFAULT_UI_HISTORY_LIMIT_BYTES,
            "default session keeps the default ceiling, got {}",
            st.history.len()
        );
    }

    /// Atomic frontier: the attach holds the parser mutex, the open suffix appears in the
    /// seed exactly once, nothing at or below N is replayed as live, and the live
    /// continuation is byte-identical to N+1.
    #[test]
    fn concurrent_output_keeps_the_atomic_frontier() {
        let fanout = fanout();
        let sink = new_sink();
        let id = session_with_sink(&fanout, &sink);
        let mut base = Vec::new();
        for i in 0..30 {
            base.extend_from_slice(format!("line {i:03}\n").as_bytes());
        }
        feed(&fanout, id, &[&base]);
        feed(&fanout, id, &[b"\x1b]0;partial"]); // ends mid-OSC at N
        let snapshot = attach(&fanout, id, WINDOW).expect("snapshot");
        let data = snapshot.data.clone();
        let n = snapshot.sequence;
        let tail = b"\x1b]0;partial";
        let count = data.windows(tail.len()).filter(|w| *w == *tail).count();
        assert_eq!(count, 1, "open suffix must appear exactly once in the seed");
        feed(&fanout, id, &[b" title\x07\x1b[1;1Hlive\n"]);
        flush(&fanout, id);
        let emitted = events(&sink);
        let live_seqs: Vec<u64> = emitted.iter().filter_map(|e| e.2).collect();
        assert!(
            live_seqs.iter().all(|s| *s > n),
            "nothing at or below N may be delivered as live: {live_seqs:?} vs N={n}"
        );
        // fresh == no-attach reference after the live continuation
        let mut fresh = vt100::Parser::new(30, 120, 0);
        fresh.process(&data);
        fresh.process(b" title\x07\x1b[1;1Hlive\n");
        let mut reference = vt100::Parser::new(30, 120, 0);
        reference.process(&base);
        reference.process(b"\x1b]0;partial title\x07\x1b[1;1Hlive\n");
        assert_eq!(fresh.screen().contents_formatted(), reference.screen().contents_formatted());
        assert_eq!(fresh.screen().title(), reference.screen().title());
    }
}
