//! Per-record capture payload produced by the JSONL readers (#2232 phase 1).
//!
//! Nothing consumes these records yet: the sink arrives in phase 3. The
//! watchers build one record per accepted assistant record and deliver it to
//! an optional unbounded channel that every caller leaves absent in this
//! phase, so the emit is a no-op in production.
//!
//! `provider` is a local enum, deliberately not `session::CodingAgentKind`:
//! this module must stay a leaf whose dependency set is auditable by eye, and
//! `CodingAgentKind` names providers (`Pi`, `Muse`, `Antigravity`) that this
//! module has no capture semantics for. Mapping to the session type happens at
//! the edge, in `lib.rs`, in phase 7.

use std::path::PathBuf;

/// The provider whose session file produced a [`CapturedRecord`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureProvider {
    Claude,
    Codex,
}

/// How a record entered the reader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordOrigin {
    /// Read incrementally from the watched file's current offset.
    Live,
    /// Emitted by the §J first-attach preamble scan, whose lines are not
    /// tracked by the kernel; `record_start` is `None` for these.
    Preamble,
    /// First sweep of a rotated Claude transcript (phase 3 semantics).
    RotationBackfill,
}

/// One accepted assistant record, ready for the capture sink.
///
/// `provider_final` and `turn_identified` are two independent bits and nothing
/// later may collapse them: Codex always carries its provider's own
/// `final_answer` marker, while only a record with a `turn_id` can be grouped.
/// No phase raises Claude's bits, because the `Stop` hook is out of scope
/// (`epic.md` 3.3); the Claude row is the permanent shape and by the user's
/// decision (`epic.md` 3.2) it still derives.
#[derive(Clone, Debug)]
pub struct CapturedRecord {
    pub session_id: String,
    pub text: String,
    pub file: PathBuf,
    pub epoch: u64,
    /// Absolute file byte offset of the raw line, before trimming. `None` only
    /// for a preamble line, whose position the kernel does not track.
    pub record_start: Option<u64>,
    pub reader_seq: u64,
    /// SHA-256 over `text.as_bytes()` (UTF-8 bytes, not characters).
    pub text_sha256: [u8; 32],
    pub turn_id: Option<String>,
    pub provider: CaptureProvider,
    pub provider_final: bool,
    pub turn_identified: bool,
    pub origin: RecordOrigin,
}
