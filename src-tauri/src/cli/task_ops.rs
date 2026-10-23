//! Pure logic for the `task-set-title` and `task-append-body` CLI verbs.
//!
//! This module owns the TASK.md parser/renderer, edit application, advisory
//! filesystem lock, atomic publish, and timestamped backup. It contains NO
//! clap surface and NO authorization — the per-verb modules
//! (`task_set_title`, `task_append_body`) handle those concerns and call
//! into [`perform`].
//!
//! Trust model: caller honestly reports their own `--root` and `--token`.
//! The same model is inherited from `send`/`close-session` and has a known
//! weakness (any well-formed UUID is accepted as a token, and `--root` is
//! unverified). See plan #137 §3a for the escalation analysis. A follow-up
//! issue is recommended to bind tokens to issued sessions, closing the hole
//! for all CLI verbs simultaneously.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};

/// Timeout for the cooperative lock. Mirrors the issue acceptance criterion
/// "concurrent writes from two coordinators don't corrupt the file" — first
/// wins, second polls every 50 ms for up to this window, else `LockTimeout`.
const LOCK_TIMEOUT_5S: Duration = Duration::from_secs(5);

/// Compatibility acquisition argument; age no longer controls ownership.
const LOCK_STALE_AFTER_5M: Duration = Duration::from_secs(300);

// ── Public surface ──────────────────────────────────────────────────────────

/// Edit operation requested by a verb.
#[derive(Debug, Clone)]
pub enum TaskOp {
    /// Coordinator/agent title write. Writes the value verbatim (no `USER:`
    /// marker) and is subject to the user-lock + reserved-prefix guards.
    SetTitle(String),
    /// Human title write (GUI editor + workgroup creation). Prefixes the value
    /// with `USER:` (unless already present) and bypasses the coordinator
    /// user-lock guard so a human can always edit a prior human title.
    SetUserTitle(String),
    /// Append a body paragraph (frontmatter untouched).
    AppendBody(String),
    /// Replace the complete body literally, preserving title and status.
    SetBody(String),
    /// Replace BOTH frontmatter title AND body with the canonical Clean form
    /// (title: 'Clean', empty body). Preserves the
    /// file's existing BOM and frontmatter line ending; body has no bytes. NoOp when the file is already in canonical Clean form.
    Clean,
}

/// Outcome of a successful [`perform`] call. The CLI translates this into the
/// verb-specific stdout line.
#[derive(Debug, Clone)]
pub enum EditOutcome {
    /// File was written. `backup` is `None` when the file did not exist before.
    Wrote {
        backup: Option<PathBuf>,
        content: String,
        title: Option<String>,
    },
    /// Set-title found the existing value already matched; no write performed.
    NoOp {
        content: String,
        title: Option<String>,
    },
    /// Coordinator set-title rejected because the current title is user-owned
    /// (`USER:`). NOT an error: exit code 0, no write, no backup. The GUI/human
    /// path (`SetUserTitle`) never produces this.
    RejectedUserTitle {
        content: String,
        title: Option<String>,
    },
}

/// Reserved marker that flags a task title as human-set (#738).
pub(crate) const USER_TITLE_PREFIX: &str = "USER:";

#[derive(Debug, Clone, Copy)]
struct TitleLine<'a> {
    leading: &'a str,
    value: &'a str,
}

/// Recognize a frontmatter `title:` line the same way the UI/backend readers do:
/// split on the first `:`, match the key case-insensitively, and keep the
/// leading whitespace so replacement can preserve indentation. `value` is the
/// trimmed raw scalar (still quoted, if it was).
fn title_line(line: &str) -> Option<TitleLine<'_>> {
    let leading_len = line.len() - line.trim_start().len();
    let leading = &line[..leading_len];
    let rest = &line[leading_len..];
    let (key, value) = rest.split_once(':')?;
    if key.trim().eq_ignore_ascii_case("title") {
        Some(TitleLine {
            leading,
            value: value.trim(),
        })
    } else {
        None
    }
}

/// Decode a YAML scalar as broadly as the app's title readers: single-quoted
/// (with `''` unescape), double-quoted (stripped), or bare (trimmed).
fn decode_title_scalar(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("''", "'");
    }
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

/// True when a title value carries the reserved human-owned marker.
pub(crate) fn is_user_owned_title(title: &str) -> bool {
    title.trim_start().starts_with(USER_TITLE_PREFIX)
}

/// Normalize a human-set title to the `USER: <value>` form, without double
/// prefixing an already-marked value.
pub(crate) fn user_owned_title(input: &str) -> String {
    let trimmed = input.trim();
    if is_user_owned_title(trimmed) {
        trimmed.to_string()
    } else {
        format!("{} {}", USER_TITLE_PREFIX, trimmed)
    }
}

/// Errors emitted by [`perform`]. `Display` impls match the §3 error matrix
/// of plan #137 verbatim.
#[derive(Debug, thiserror::Error)]
pub enum TaskOpError {
    /// Coordinator `--title` input itself started with the reserved `USER:`
    /// prefix while the current title was not already user-owned (#738). Invalid
    /// input: exit 1, no stdout, TASK.md unchanged, no backup.
    #[error("--title cannot start with reserved USER: prefix")]
    ReservedUserTitlePrefix,
    #[error("invalid_status: {0}")]
    InvalidStatus(String),
    #[error("snapshot_too_large: TASK.md exceeds 256 KiB")]
    SnapshotTooLarge,
    #[error("status_too_large: encoded record exceeds 65536 bytes")]
    StatusTooLarge,
    #[error("sequence_overflow")]
    SequenceOverflow,
    #[error("request_id_conflict")]
    RequestIdConflict,
    #[error("revision_conflict: currentRevision={current_revision}")]
    RevisionConflict { current_revision: String },
    #[error("write_failed: {0}")]
    WriteFailed(std::io::Error),
    #[error("write_failed: status source changed; partial evidence preserved")]
    StatusSourceChanged,
    #[error("clean_recovery_pending: {0}")]
    CleanRecoveryPending(String),
    #[error("clean_recovery_conflict: {0}")]
    CleanRecoveryConflict(String),
    #[error("TASK.md is locked by another writer (5s timeout). Try again.")]
    LockTimeout,
    #[error("failed to acquire TASK.md lock at {}: {}. Aborting; TASK.md left unchanged.", .0.display(), .1)]
    LockIo(PathBuf, std::io::Error),
    #[error("failed to read TASK.md at {}: {}", .0.display(), .1)]
    ReadFailed(PathBuf, std::io::Error),
    #[error("failed to write backup at {}: {}. Aborting; TASK.md left unchanged.", .0.display(), .1)]
    BackupFailed(PathBuf, std::io::Error),
    #[error("failed to write backup at {}: 100 collision retries exhausted in the same second. Aborting; TASK.md left unchanged.", .0.display())]
    BackupExhausted(PathBuf),
    #[error("failed to write {}: {}. Aborting; TASK.md left unchanged.", .0.display(), .1)]
    TmpWriteFailed(PathBuf, std::io::Error),
    #[error("TASK.md was modified externally between read and write; aborting. Backup at {} retains the externally-modified state.", .0.display())]
    ExternalWrite(PathBuf),
    /// Custom Display below: `Some(p)` → "Backup at <p> retains the prior state.";
    /// `None` → "No backup (TASK.md did not exist before)." (§H.4 / NIT-2 in plan).
    #[error("{}", format_rename_failed(.0, .1))]
    RenameFailed(std::io::Error, Option<PathBuf>),
}

fn format_rename_failed(io_err: &std::io::Error, backup: &Option<PathBuf>) -> String {
    match backup {
        Some(p) => format!(
            "failed to publish TASK.md (rename): {}. Backup at {} retains the prior state.",
            io_err,
            p.display()
        ),
        None => format!(
            "failed to publish TASK.md (rename): {}. No backup (TASK.md did not exist before).",
            io_err
        ),
    }
}

/// Production entry point. Captures `chrono::Utc::now` for backup-name
/// timestamping and delegates to [`perform_inner`].
pub fn perform(wg_root: &Path, op: TaskOp) -> Result<EditOutcome, TaskOpError> {
    perform_inner(wg_root, op, Utc::now)
}

// ── Parsed-frontmatter shape ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedTask {
    /// Input started with U+FEFF (HIGH-3): preserve+re-emit at render time.
    pub bom: bool,
    /// Dominant line ending in the input (LOW-3). Used for frontmatter delimiter +
    /// inter-line separators in `render`. The body slice is preserved byte-for-byte regardless.
    pub line_ending: &'static str,
    pub has_frontmatter: bool,
    /// Raw frontmatter lines (eol-stripped, trim_end_matches(['\r','\n'])).
    pub frontmatter: Vec<String>,
    /// Everything after the closing `---<eol>` (or whole post-BOM input when `has_frontmatter == false`).
    pub body: String,
}

pub(crate) fn parse_task(s_in: &str) -> ParsedTask {
    // ── BOM peel (HIGH-3) ──────────────────────────────────────────────────
    let (bom, s) = match s_in.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, s_in),
    };

    // ── Line-ending detection (LOW-3) ──────────────────────────────────────
    let line_ending: &'static str = match s.find('\n') {
        Some(i) if i > 0 && s.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    };

    // ── Pull the opening line (CRIT-1 Form B fix) ──────────────────────────
    // The opening's actual byte length is whatever split_inclusive yields —
    // 4 bytes for "---\n", 5 for "---\r\n", 7 for "--- \r\n", etc.
    let mut iter = s.split_inclusive('\n');
    let opening = match iter.next() {
        Some(line) if line.trim() == "---" => line,
        _ => {
            return ParsedTask {
                bom,
                line_ending,
                has_frontmatter: false,
                frontmatter: Vec::new(),
                body: s.to_string(),
            };
        }
    };
    let mut consumed = opening.len();

    // ── Walk to the closing `---` (D.1: tolerate trailing whitespace) ──────
    let mut fm_lines: Vec<String> = Vec::new();
    let mut closed = false;
    for line in iter {
        consumed += line.len();
        let stripped = line.trim_end_matches(['\r', '\n']);
        if stripped.trim() == "---" {
            closed = true;
            break;
        }
        fm_lines.push(stripped.to_string());
    }

    if !closed {
        // Malformed frontmatter — preserve whole post-BOM input as body.
        return ParsedTask {
            bom,
            line_ending,
            has_frontmatter: false,
            frontmatter: Vec::new(),
            body: s.to_string(),
        };
    }

    let body = s[consumed..].to_string();
    ParsedTask {
        bom,
        line_ending,
        has_frontmatter: true,
        frontmatter: fm_lines,
        body,
    }
}

pub(crate) fn render(parsed: &ParsedTask) -> String {
    let eol = parsed.line_ending;
    let mut out = String::with_capacity(parsed.body.len() + 64);
    if parsed.bom {
        out.push('\u{FEFF}');
    }
    if !parsed.has_frontmatter {
        out.push_str(&parsed.body);
        return out;
    }
    out.push_str("---");
    out.push_str(eol);
    for line in &parsed.frontmatter {
        out.push_str(line);
        out.push_str(eol);
    }
    out.push_str("---");
    out.push_str(eol);
    out.push_str(&parsed.body);
    out
}

pub(crate) fn apply_edit(parsed: &ParsedTask, op: &TaskOp) -> ParsedTask {
    match op {
        TaskOp::SetTitle(title) => apply_set_title(parsed, title),
        TaskOp::SetUserTitle(title) => apply_set_title(parsed, &user_owned_title(title)),
        TaskOp::AppendBody(text) => apply_append_body(parsed, text),
        TaskOp::SetBody(text) => apply_set_body(parsed, text),
        TaskOp::Clean => apply_clean(parsed),
    }
}

fn apply_set_body(parsed: &ParsedTask, text: &str) -> ParsedTask {
    let mut updated = parsed.clone();
    // A closed empty block prevents body metadata/BOM from becoming a title
    // or file BOM when this legacy representation is read again.
    updated.has_frontmatter = true;
    updated.body = text.to_owned();
    updated
}

fn apply_set_title(parsed: &ParsedTask, title: &str) -> ParsedTask {
    let escaped = title.replace('\'', "''");
    let new_title_line = format!("title: '{}'", escaped);

    // Brand-new file: parsed has empty body and no frontmatter. The set-title
    // matrix says born-LF and BOM-less per entity_creation.rs convention.
    // Also require !parsed.bom so a BOM-only existing file (post-BOM-peel
    // body is "") falls through to the "preserve bom/eol" branch instead of
    // tripping this brand-new shortcut and stripping the BOM (LOW-1, plan §5
    // row 2 — HIGH-3 byte-exact round-trip).
    if !parsed.has_frontmatter && parsed.body.is_empty() && !parsed.bom {
        return ParsedTask {
            bom: false,
            line_ending: "\n",
            has_frontmatter: true,
            frontmatter: vec![new_title_line],
            body: String::new(),
        };
    }

    // No frontmatter, body has content: prepend a fresh frontmatter block.
    if !parsed.has_frontmatter {
        return ParsedTask {
            bom: parsed.bom,
            line_ending: parsed.line_ending,
            has_frontmatter: true,
            frontmatter: vec![new_title_line],
            body: parsed.body.clone(),
        };
    }

    // Has frontmatter — find existing title-shaped line(s) (NIT-5). Match the
    // key case-insensitively (via `title_line`) so `Title:`/`TITLE:` lines are
    // replaced in place instead of leaving a stranded key + a new duplicate.
    let title_count = parsed
        .frontmatter
        .iter()
        .filter(|line| title_line(line).is_some())
        .count();
    if title_count > 1 {
        log::warn!(
            "TASK.md frontmatter contains {} title: lines; replacing the first only — \
             downstream YAML parsers may pick a different one",
            title_count
        );
    }

    let mut new_fm: Vec<String> = parsed.frontmatter.clone();
    let title_idx = new_fm.iter().position(|line| title_line(line).is_some());

    match title_idx {
        Some(idx) => {
            // Replace, preserving the existing leading whitespace (so an indented
            // `  title: x` becomes `  title: 'NewTitle'`).
            let leading = title_line(&new_fm[idx])
                .map(|title| title.leading)
                .unwrap_or("");
            new_fm[idx] = format!("{}{}", leading, new_title_line);
        }
        None => {
            new_fm.insert(0, new_title_line);
        }
    }

    ParsedTask {
        bom: parsed.bom,
        line_ending: parsed.line_ending,
        has_frontmatter: true,
        frontmatter: new_fm,
        body: parsed.body.clone(),
    }
}

fn apply_append_body(parsed: &ParsedTask, text: &str) -> ParsedTask {
    let trimmed_text = text.trim_end();
    let new_body = if parsed.body.trim().is_empty() {
        format!("{}\n", trimmed_text)
    } else {
        // trim_end on the existing body collapses any number of trailing newlines/spaces
        // to zero; the literal "\n\n" inserts exactly one blank-line separator;
        // the appended chunk ends with a single "\n".
        format!("{}\n\n{}\n", parsed.body.trim_end(), trimmed_text)
    };
    ParsedTask {
        bom: parsed.bom,
        line_ending: parsed.line_ending,
        has_frontmatter: parsed.has_frontmatter,
        frontmatter: parsed.frontmatter.clone(),
        body: new_body,
    }
}

/// Replace frontmatter title and body with the canonical Clean form.
/// Preserves the file's BOM and dominant line ending for the frontmatter;
/// the body contains no bytes. For
/// an empty input (`parse_task("")`), `parsed.bom == false` and
/// `parsed.line_ending == "\n"`, so the output is the canonical LF/no-BOM
/// Clean form — no special case needed.
fn apply_clean(parsed: &ParsedTask) -> ParsedTask {
    ParsedTask {
        bom: parsed.bom,
        line_ending: parsed.line_ending,
        has_frontmatter: true,
        frontmatter: vec!["title: 'Clean'".to_string()],
        body: String::new(),
    }
}

/// Extract the decoded value of the first `title:` line in the frontmatter, for
/// the semantic idempotence short-circuit (MED-3) and the #738 user-lock guard.
/// Matches the key case-insensitively and decodes single/double-quoted and bare
/// scalars via [`decode_title_scalar`], mirroring the app's other title readers.
/// Returns `None` when the frontmatter has no title-shaped line.
pub(crate) fn title_value_of(parsed: &ParsedTask) -> Option<String> {
    parsed
        .frontmatter
        .iter()
        .find_map(|line| title_line(line).map(|title| decode_title_scalar(title.value)))
}

// ── Lock guard ──────────────────────────────────────────────────────────────

/// Stable kernel lock. Each acquisition opens a separate handle; the kernel
/// releases ownership on process death. Never unlink or replace this file.
pub(crate) struct LockGuard {
    _file: std::fs::File,
}
impl LockGuard {
    pub(crate) fn acquire(
        path: &Path,
        timeout: Duration,
        _stale_after: Duration,
    ) -> Result<Self, TaskOpError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| TaskOpError::LockIo(path.to_path_buf(), e))?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if start.elapsed() >= timeout {
                        return Err(TaskOpError::LockTimeout);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(std::fs::TryLockError::Error(e)) => {
                    return Err(TaskOpError::LockIo(path.to_path_buf(), e))
                }
            }
        }
    }
}

// Status persistence shares the TASK lock; callers own authorization.
const STATUS_LIMIT: usize = 65_536;
const STATUS_WINDOW: u64 = 131_073;
const TASK_LIMIT: u64 = 256 * 1024;
const MAX_SEQUENCE: u64 = (1u64 << 53) - 1;
const STATUS_NAME: &str = "TASK-status.jsonl";
const JOURNAL_NAME: &str = "TASK-clean.pending.json";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusRecord {
    pub schema_version: u32,
    pub kind: String,
    pub topic_id: String,
    pub sequence: u64,
    pub request_id: Option<String>,
    pub base_revision: Option<String>,
    pub recorded_at: String,
    pub author: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub workgroup_root: String,
    pub task: Option<String>,
    pub task_title: Option<String>,
    pub description: String,
    pub status: Option<String>,
    pub revision: String,
    pub status_record: Option<StatusRecord>,
    pub tail_incomplete: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReceipt {
    pub revision: String,
    pub record: StatusRecord,
    pub replayed: bool,
}

struct StatusTail {
    record: Option<StatusRecord>,
    partial: Vec<u8>,
    complete_len: u64,
    len: u64,
    modified: Option<SystemTime>,
    bytes_read: usize,
}

impl StatusTail {
    fn revision(&self) -> String {
        self.record.as_ref().map_or_else(
            || "legacy:0".into(),
            |r| format!("{}:{}", r.topic_id, r.sequence),
        )
    }
}

fn status_invalid(reason: impl Into<String>) -> TaskOpError {
    TaskOpError::InvalidStatus(reason.into())
}

fn validate_status_text(text: &str) -> Result<(), TaskOpError> {
    if text.trim().is_empty()
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(status_invalid(
            "empty status or forbidden control character",
        ));
    }
    Ok(())
}

fn valid_author(author: &str) -> bool {
    !author.chars().any(|c| c.is_whitespace() || c.is_control())
        && author.split_once('/').is_some_and(|(project, agent)| {
            !project.is_empty() && !agent.is_empty() && !agent.contains('/')
        })
}

fn valid_revision(revision: &str) -> bool {
    revision == "legacy:0"
        || revision.rsplit_once(':').is_some_and(|(topic, sequence)| {
            uuid::Uuid::parse_str(topic).is_ok()
                && sequence
                    .parse::<u64>()
                    .is_ok_and(|n| n <= MAX_SEQUENCE && n.to_string() == sequence)
        })
}

fn validate_record(bytes: &[u8]) -> Result<StatusRecord, TaskOpError> {
    if bytes.len() + 1 > STATUS_LIMIT {
        return Err(status_invalid("completed row exceeds 65536 bytes"));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| status_invalid(format!("invalid completed JSON: {e}")))?;
    for field in [
        "schemaVersion",
        "kind",
        "topicId",
        "sequence",
        "requestId",
        "baseRevision",
        "recordedAt",
        "author",
        "status",
    ] {
        if value.get(field).is_none() {
            return Err(status_invalid(format!("missing {field}")));
        }
    }
    let r: StatusRecord = serde_json::from_value(value)
        .map_err(|e| status_invalid(format!("invalid record shape: {e}")))?;
    if r.schema_version != 1
        || uuid::Uuid::parse_str(&r.topic_id).is_err()
        || r.sequence > MAX_SEQUENCE
        || DateTime::parse_from_rfc3339(&r.recorded_at)
            .map_or(true, |t| t.offset().local_minus_utc() != 0)
    {
        return Err(status_invalid(
            "unsupported schema, topic, sequence or UTC time",
        ));
    }
    match r.kind.as_str() {
        "topic_started"
            if r.sequence == 0
                && r.request_id.is_none()
                && r.base_revision.is_none()
                && r.author.is_none()
                && r.status.is_none() => {}
        "status"
            if r.sequence > 0
                && r.request_id
                    .as_deref()
                    .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
                && r.base_revision.as_deref().is_some_and(valid_revision)
                && r.author.as_deref().is_some_and(valid_author) =>
        {
            validate_status_text(
                r.status
                    .as_deref()
                    .ok_or_else(|| status_invalid("missing status"))?,
            )?;
            let base = r.base_revision.as_deref().unwrap();
            let expected = if r.sequence == 1 && base == "legacy:0" {
                "legacy:0".to_string()
            } else {
                format!("{}:{}", r.topic_id, r.sequence - 1)
            };
            if base != expected {
                return Err(status_invalid("invalid base sequence"));
            }
        }
        _ => return Err(status_invalid("invalid kind or record semantics")),
    }
    Ok(r)
}

fn read_latest_status_locked(root: &Path) -> Result<StatusTail, TaskOpError> {
    use std::io::{Read, Seek, SeekFrom};
    let path = root.join(STATUS_NAME);
    let mut file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StatusTail {
                record: None,
                partial: Vec::new(),
                complete_len: 0,
                len: 0,
                modified: None,
                bytes_read: 0,
            })
        }
        Err(e) => return Err(TaskOpError::ReadFailed(path, e)),
    };
    let meta = file
        .metadata()
        .map_err(|e| TaskOpError::ReadFailed(path.clone(), e))?;
    let len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| TaskOpError::ReadFailed(path.clone(), e))?;
    let start = len.saturating_sub(STATUS_WINDOW);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| TaskOpError::ReadFailed(path.clone(), e))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.take(STATUS_WINDOW)
        .read_to_end(&mut bytes)
        .map_err(|e| TaskOpError::ReadFailed(path, e))?;
    let last_lf = bytes.iter().rposition(|b| *b == b'\n');
    let end = last_lf.map_or(0, |i| i + 1);
    let partial = bytes[end..].to_vec();
    if partial.len() >= STATUS_LIMIT || (last_lf.is_none() && start != 0) {
        return Err(status_invalid("partial suffix exceeds 65535 bytes"));
    }
    let record = if let Some(lf) = last_lf {
        let row_start = bytes[..lf]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        if row_start == 0 && start != 0 {
            return Err(status_invalid("oversized completed row"));
        }
        Some(validate_record(&bytes[row_start..lf])?)
    } else {
        None
    };
    Ok(StatusTail {
        record,
        partial,
        complete_len: start + end as u64,
        len,
        modified: meta.modified().ok(),
        bytes_read: bytes.len(),
    })
}

pub fn read_latest_status(root: &Path) -> Result<TaskSnapshot, TaskOpError> {
    read_snapshot(root)
}

pub fn read_snapshot(root: &Path) -> Result<TaskSnapshot, TaskOpError> {
    use std::io::Read;
    let _lock = LockGuard::acquire(
        &root.join("TASK.md.lock"),
        LOCK_TIMEOUT_5S,
        LOCK_STALE_AFTER_5M,
    )?;
    recover_clean_pair_locked(root)?;
    let path = root.join("TASK.md");
    let task = match std::fs::File::open(&path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(TASK_LIMIT + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| TaskOpError::ReadFailed(path.clone(), e))?;
            if bytes.len() as u64 > TASK_LIMIT {
                return Err(TaskOpError::SnapshotTooLarge);
            }
            Some(String::from_utf8(bytes).map_err(|e| {
                TaskOpError::ReadFailed(
                    path.clone(),
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                )
            })?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(TaskOpError::ReadFailed(path, e)),
    };
    let parsed = parse_task(task.as_deref().unwrap_or(""));
    let tail = read_latest_status_locked(root)?;
    Ok(TaskSnapshot {
        workgroup_root: root.to_string_lossy().into_owned(),
        task_title: title_value_of(&parsed),
        description: parsed.body,
        task,
        status: tail.record.as_ref().and_then(|r| r.status.clone()),
        revision: tail.revision(),
        tail_incomplete: !tail.partial.is_empty(),
        status_record: tail.record,
    })
}

fn write_failed(e: std::io::Error) -> TaskOpError {
    TaskOpError::WriteFailed(e)
}

// Failures are injected only in fixtures, in this thread and this module.
#[cfg(test)]
thread_local! {
    static IO_MUTATION: std::cell::RefCell<Option<(String, PathBuf, Vec<u8>)>> = const { std::cell::RefCell::new(None) };
    static IO_FAULT: std::cell::RefCell<(Option<String>, Vec<String>)> = const { std::cell::RefCell::new((None, Vec::new())) };
}
fn io_boundary(name: &str) -> std::io::Result<()> {
    #[cfg(test)]
    {
        if std::env::var("AC_TASK_OPS_CHILD_BOUNDARY").as_deref() == Ok(name)
            && std::env::var_os("AC_TASK_OPS_CHILD_FIXTURE").is_some()
        {
            std::process::exit(71);
        }
        IO_MUTATION.with(|v| {
            let mut v = v.borrow_mut();
            if v.as_ref().is_some_and(|(point, _, _)| point == name) {
                let (_, path, bytes) = v.take().unwrap();
                std::fs::write(path, bytes).unwrap();
            }
        });
        IO_FAULT.with(|f| {
            let mut f = f.borrow_mut();
            f.1.push(name.into());
            if f.0.as_deref() == Some(name) {
                Err(std::io::Error::other(format!("fixture: {name}")))
            } else {
                Ok(())
            }
        })
    }
    #[cfg(not(test))]
    {
        let _ = name;
        Ok(())
    }
}

fn sync_file(file: &std::fs::File, boundary: &str) -> std::io::Result<()> {
    io_boundary(boundary)?;
    file.sync_all()
}

fn repair_partial_locked(root: &Path, tail: &StatusTail) -> Result<(), TaskOpError> {
    if tail.partial.is_empty() {
        return Ok(());
    }
    let path = root.join(STATUS_NAME);
    let unchanged = |current: &StatusTail| {
        current.len == tail.len
            && current.modified == tail.modified
            && current.partial == tail.partial
            && current.complete_len == tail.complete_len
            && current.record == tail.record
    };
    if !unchanged(&read_latest_status_locked(root)?) {
        return Err(TaskOpError::StatusSourceChanged);
    }
    let backup = root.join(format!(
        "TASK-status.partial.{}.{}.bak",
        Utc::now().format("%Y%m%d-%H%M%S"),
        uuid::Uuid::new_v4()
    ));
    io_boundary("partial_backup_create").map_err(write_failed)?;
    let mut saved = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(write_failed)?;
    io_boundary("partial_backup_write").map_err(write_failed)?;
    saved.write_all(&tail.partial).map_err(write_failed)?;
    sync_file(&saved, "partial_backup_sync").map_err(write_failed)?;
    if !unchanged(&read_latest_status_locked(root)?) {
        return Err(TaskOpError::StatusSourceChanged);
    }
    io_boundary("partial_truncate").map_err(write_failed)?;
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(write_failed)?;
    file.set_len(tail.complete_len).map_err(write_failed)?;
    sync_file(&file, "partial_truncate_sync").map_err(write_failed)
}

pub fn append_status(
    root: &Path,
    expected_revision: &str,
    request_id: &str,
    text: &str,
    author: &str,
) -> Result<StatusReceipt, TaskOpError> {
    let _lock = LockGuard::acquire(
        &root.join("TASK.md.lock"),
        LOCK_TIMEOUT_5S,
        LOCK_STALE_AFTER_5M,
    )?;
    recover_clean_pair_locked(root)?;
    let tail = read_latest_status_locked(root)?;
    if let Some(last) = &tail.record {
        if last.request_id.as_deref() == Some(request_id) {
            if last.base_revision.as_deref() != Some(expected_revision)
                || last.author.as_deref() != Some(author)
                || last.status.as_deref() != Some(text)
            {
                return Err(TaskOpError::RequestIdConflict);
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(root.join(STATUS_NAME))
                .map_err(write_failed)?;
            sync_file(&file, "append_sync").map_err(write_failed)?;
            return Ok(StatusReceipt {
                revision: tail.revision(),
                record: last.clone(),
                replayed: true,
            });
        }
    }
    validate_status_text(text)?;
    if uuid::Uuid::parse_str(request_id).is_err()
        || !valid_revision(expected_revision)
        || !valid_author(author)
    {
        return Err(status_invalid("invalid request, revision or author"));
    }
    let revision = tail.revision();
    if expected_revision != revision {
        return Err(TaskOpError::RevisionConflict {
            current_revision: revision,
        });
    }
    let (topic, sequence) = match &tail.record {
        Some(r) if r.sequence == MAX_SEQUENCE => return Err(TaskOpError::SequenceOverflow),
        Some(r) => (r.topic_id.clone(), r.sequence + 1),
        None => (uuid::Uuid::new_v4().to_string(), 1),
    };
    let record = StatusRecord {
        schema_version: 1,
        kind: "status".into(),
        topic_id: topic,
        sequence,
        request_id: Some(request_id.into()),
        base_revision: Some(expected_revision.into()),
        recorded_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        author: Some(author.into()),
        status: Some(text.into()),
    };
    let mut bytes = serde_json::to_vec(&record).map_err(|e| status_invalid(e.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > STATUS_LIMIT {
        return Err(TaskOpError::StatusTooLarge);
    }
    repair_partial_locked(root, &tail)?;
    io_boundary("append_open").map_err(write_failed)?;
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(root.join(STATUS_NAME))
        .map_err(write_failed)?;
    #[cfg(test)]
    if IO_FAULT.with(|f| f.borrow().0.as_deref() == Some("append_partial")) {
        file.write_all(&bytes[..20]).map_err(write_failed)?;
        return Err(write_failed(std::io::Error::other("fixture partial")));
    }
    io_boundary("append_write").map_err(write_failed)?;
    file.write_all(&bytes).map_err(write_failed)?;
    sync_file(&file, "append_sync").map_err(write_failed)?;
    Ok(StatusReceipt {
        revision: format!("{}:{}", record.topic_id, record.sequence),
        record,
        replayed: false,
    })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanSide {
    existed: bool,
    original_hash: Option<String>,
    target_hash: String,
    backup: Option<String>,
    stage: String,
}
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanJournal {
    schema_version: u32,
    tx_id: String,
    stamp: String,
    suffix: u32,
    task: CleanSide,
    status: CleanSide,
}
#[derive(PartialEq)]
struct SourceState {
    hash: Option<String>,
    len: u64,
    modified: Option<SystemTime>,
}

fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}
fn source_state(path: &Path) -> std::io::Result<SourceState> {
    use sha2::Digest;
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SourceState {
                hash: None,
                len: 0,
                modified: None,
            })
        }
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    let mut hash = sha2::Sha256::new();
    let mut buf = [0u8; 65536];
    let mut len = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        len += n as u64;
    }
    if len != meta.len() {
        return Err(std::io::Error::other("source changed while hashing"));
    }
    Ok(SourceState {
        hash: Some(format!("{:x}", hash.finalize())),
        len,
        modified: meta.modified().ok(),
    })
}

fn rename_retry(source: &Path, target: &Path) -> std::io::Result<()> {
    for attempt in 0..3 {
        match std::fs::rename(source, target) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt < 2
                    && (e.kind() == std::io::ErrorKind::PermissionDenied
                        || matches!(e.raw_os_error(), Some(5) | Some(32))) =>
            {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

fn clean_conflict(reason: impl Into<String>) -> TaskOpError {
    TaskOpError::CleanRecoveryConflict(reason.into())
}
fn clean_pending(e: std::io::Error) -> TaskOpError {
    TaskOpError::CleanRecoveryPending(e.to_string())
}

fn backup_names(stamp: &str, suffix: u32) -> (String, String) {
    let tag = if suffix == 0 {
        stamp.into()
    } else {
        format!("{stamp}.{suffix}")
    };
    (
        format!("TASK.{tag}.bak.md"),
        format!("TASK-status.{tag}.bak.jsonl"),
    )
}

fn validate_journal(j: &CleanJournal) -> Result<(), TaskOpError> {
    let hash_ok = |h: &str| {
        h.len() == 64
            && h.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    };
    let stamp_ok = j.stamp.len() == 15
        && j.stamp.as_bytes()[8] == b'-'
        && j.stamp
            .bytes()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit())
        && chrono::NaiveDateTime::parse_from_str(&j.stamp, "%Y%m%d-%H%M%S").is_ok();
    if j.schema_version != 1
        || uuid::Uuid::parse_str(&j.tx_id).is_err()
        || !stamp_ok
        || j.suffix > 99
    {
        return Err(clean_conflict("invalid journal header"));
    }
    let names = backup_names(&j.stamp, j.suffix);
    let any = j.task.existed || j.status.existed;
    for (side, target, backup) in [
        (&j.task, "TASK.md", names.0),
        (&j.status, STATUS_NAME, names.1),
    ] {
        if side.stage != format!("{target}.tmp.{}", j.tx_id)
            || side.backup != any.then_some(backup)
            || side.existed != side.original_hash.is_some()
            || side.original_hash.as_deref().is_some_and(|h| !hash_ok(h))
            || !hash_ok(&side.target_hash)
        {
            return Err(clean_conflict("invalid journal side or basename"));
        }
    }
    Ok(())
}

fn cleanup_owned(path: &Path, hash: &str) {
    if source_state(path).ok().and_then(|s| s.hash).as_deref() == Some(hash) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn recover_clean_pair(root: &Path) -> Result<(), TaskOpError> {
    let _lock = LockGuard::acquire(
        &root.join("TASK.md.lock"),
        LOCK_TIMEOUT_5S,
        LOCK_STALE_AFTER_5M,
    )?;
    recover_clean_pair_locked(root)
}

fn recover_clean_pair_locked(root: &Path) -> Result<(), TaskOpError> {
    use std::io::Read;
    let journal_path = root.join(JOURNAL_NAME);
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(&journal_path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(clean_pending(e)),
    };
    let mut bytes = Vec::new();
    file.take(16_385)
        .read_to_end(&mut bytes)
        .map_err(clean_pending)?;
    if bytes.len() > 16_384 {
        return Err(clean_conflict("oversized journal"));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| clean_conflict(e.to_string()))?;
    for field in ["schemaVersion", "txId", "stamp", "suffix", "task", "status"] {
        if value.get(field).is_none() {
            return Err(clean_conflict("missing journal field"));
        }
    }
    for side in ["task", "status"] {
        for field in ["existed", "originalHash", "targetHash", "backup", "stage"] {
            if value[side].get(field).is_none() {
                return Err(clean_conflict("missing journal side field"));
            }
        }
    }
    let journal: CleanJournal =
        serde_json::from_value(value).map_err(|e| clean_conflict(e.to_string()))?;
    validate_journal(&journal)?;
    let journal_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&journal_path)
        .map_err(clean_pending)?;
    sync_file(&journal_file, "journal_sync").map_err(clean_pending)?;
    drop(journal_file);
    finish_clean_locked(root, &journal, &bytes)
}

fn finish_clean_locked(
    root: &Path,
    journal: &CleanJournal,
    journal_bytes: &[u8],
) -> Result<(), TaskOpError> {
    // Validate the entire pair before replacing either target.
    for (side, target) in [(&journal.task, "TASK.md"), (&journal.status, STATUS_NAME)] {
        let current = source_state(&root.join(target)).map_err(clean_pending)?;
        if current.hash.as_deref() != Some(&side.target_hash) && current.hash != side.original_hash
        {
            return Err(clean_conflict(format!("external edit: {target}")));
        }
        if let Some(backup) = &side.backup {
            let expected = side
                .original_hash
                .clone()
                .unwrap_or_else(|| hash_bytes(&[]));
            if source_state(&root.join(backup))
                .map_err(|e| clean_conflict(e.to_string()))?
                .hash
                .as_deref()
                != Some(&expected)
            {
                return Err(clean_conflict("missing or changed backup"));
            }
        }
        if current.hash.as_deref() != Some(&side.target_hash)
            && source_state(&root.join(&side.stage))
                .map_err(|e| clean_conflict(e.to_string()))?
                .hash
                .as_deref()
                != Some(&side.target_hash)
        {
            return Err(clean_conflict("missing or changed needed stage"));
        }
    }
    for (side, target, boundary) in [
        (&journal.task, "TASK.md", "task_rename"),
        (&journal.status, STATUS_NAME, "status_rename"),
    ] {
        let path = root.join(target);
        let current = source_state(&path).map_err(clean_pending)?;
        if current.hash.as_deref() == Some(&side.target_hash) {
            continue;
        }
        if current.hash != side.original_hash {
            return Err(clean_conflict(format!(
                "external edit before replacement: {target}"
            )));
        }
        io_boundary(boundary).map_err(clean_pending)?;
        if source_state(&path).map_err(clean_pending)?.hash != side.original_hash {
            return Err(clean_conflict(
                "external edit immediately before replacement",
            ));
        }
        if source_state(&root.join(&side.stage))
            .map_err(|e| clean_conflict(e.to_string()))?
            .hash
            .as_deref()
            != Some(&side.target_hash)
        {
            return Err(clean_conflict(
                "stage changed immediately before replacement",
            ));
        }
        rename_retry(&root.join(&side.stage), &path).map_err(clean_pending)?;
        io_boundary(&format!("after_{boundary}")).map_err(clean_pending)?;
    }
    // Even an already-new pair requires fresh sync attempts for BOTH targets.
    // Keep the first failure while still attempting the other target.
    let mut sync_error = None;
    for (side, target, boundary) in [
        (&journal.task, "TASK.md", "task_target_sync"),
        (&journal.status, STATUS_NAME, "status_target_sync"),
    ] {
        let path = root.join(target);
        if source_state(&path).map_err(clean_pending)?.hash.as_deref() != Some(&side.target_hash) {
            return Err(clean_conflict("target hash changed"));
        }
        let attempt = io_boundary(&format!("{boundary}_open")).and_then(|()| {
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            sync_file(&file, boundary)
        });
        if let Err(e) = attempt {
            if sync_error.is_none() {
                sync_error = Some(e);
            }
        }
    }
    if let Some(e) = sync_error {
        return Err(clean_pending(e));
    }
    for (side, target) in [(&journal.task, "TASK.md"), (&journal.status, STATUS_NAME)] {
        if source_state(&root.join(target))
            .map_err(clean_pending)?
            .hash
            .as_deref()
            != Some(&side.target_hash)
        {
            return Err(clean_conflict("target changed after sync"));
        }
    }
    if std::fs::read(root.join(JOURNAL_NAME)).map_err(clean_pending)? != journal_bytes {
        return Err(clean_conflict("journal changed"));
    }
    io_boundary("journal_remove").map_err(clean_pending)?;
    std::fs::remove_file(root.join(JOURNAL_NAME)).map_err(clean_pending)?;
    for side in [&journal.task, &journal.status] {
        cleanup_owned(&root.join(&side.stage), &side.target_hash);
    }
    Ok(())
}

fn copy_backup(
    source: &Path,
    dest: &mut std::fs::File,
    exists: bool,
    boundary: &str,
    owned_hash: &mut String,
) -> std::io::Result<()> {
    use sha2::Digest;
    use std::io::Read;
    let mut hash = sha2::Sha256::new();
    io_boundary(&format!("{boundary}_copy"))?;
    if exists {
        let mut source = std::fs::File::open(source)?;
        let mut buffer = [0u8; 65536];
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            let mut offset = 0;
            while offset < count {
                io_boundary(&format!("{boundary}_write"))?;
                let written = dest.write(&buffer[offset..count])?;
                if written == 0 {
                    return Err(std::io::ErrorKind::WriteZero.into());
                }
                hash.update(&buffer[offset..offset + written]);
                *owned_hash = format!("{:x}", hash.clone().finalize());
                offset += written;
                io_boundary(&format!("{boundary}_after_write"))?;
            }
        }
    }
    sync_file(dest, &format!("{boundary}_sync"))
}

pub fn clean_pair(root: &Path) -> Result<EditOutcome, TaskOpError> {
    perform(root, TaskOp::Clean)
}

fn clean_pair_locked(
    root: &Path,
    existing: &str,
    now: DateTime<Utc>,
) -> Result<EditOutcome, TaskOpError> {
    let parsed = parse_task(existing);
    let cleaned = apply_clean(&parsed);
    // Corrupt logs are intentionally archivable; only inspect for NoOp when
    // the description already matches the canonical reset.
    if cleaned.frontmatter == parsed.frontmatter && cleaned.body == parsed.body {
        if let Ok(tail) = read_latest_status_locked(root) {
            if tail.partial.is_empty()
                && (tail.len == 0
                    || tail
                        .record
                        .as_ref()
                        .is_some_and(|r| r.kind == "topic_started")
                        && tail.complete_len == tail.bytes_read as u64
                        && tail.bytes_read <= STATUS_LIMIT
                        && {
                            let bytes =
                                std::fs::read(root.join(STATUS_NAME)).map_err(write_failed)?;
                            bytes.iter().filter(|b| **b == b'\n').count() == 1
                        })
            {
                return Ok(EditOutcome::NoOp {
                    content: existing.into(),
                    title: title_value_of(&parsed),
                });
            }
        }
    }
    let task_path = root.join("TASK.md");
    let status_path = root.join(STATUS_NAME);
    let original_task = source_state(&task_path).map_err(write_failed)?;
    let original_status = source_state(&status_path).map_err(write_failed)?;
    // The caller's description must still be the description being archived.
    if original_task
        .hash
        .as_deref()
        .is_some_and(|h| h != hash_bytes(existing.as_bytes()))
    {
        return Err(clean_conflict("description changed before archive"));
    }
    let new_content = render(&cleaned);
    let seed = StatusRecord {
        schema_version: 1,
        kind: "topic_started".into(),
        topic_id: uuid::Uuid::new_v4().to_string(),
        sequence: 0,
        request_id: None,
        base_revision: None,
        recorded_at: now.to_rfc3339(),
        author: None,
        status: None,
    };
    let mut seed_bytes = serde_json::to_vec(&seed).map_err(|e| status_invalid(e.to_string()))?;
    seed_bytes.push(b'\n');
    let tx = uuid::Uuid::new_v4().to_string();
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let mut journal = CleanJournal {
        schema_version: 1,
        tx_id: tx.clone(),
        stamp: stamp.clone(),
        suffix: 0,
        task: CleanSide {
            existed: original_task.hash.is_some(),
            original_hash: original_task.hash.clone(),
            target_hash: hash_bytes(new_content.as_bytes()),
            backup: None,
            stage: format!("TASK.md.tmp.{tx}"),
        },
        status: CleanSide {
            existed: original_status.hash.is_some(),
            original_hash: original_status.hash.clone(),
            target_hash: hash_bytes(&seed_bytes),
            backup: None,
            stage: format!("TASK-status.jsonl.tmp.{tx}"),
        },
    };
    if journal.task.existed || journal.status.existed {
        let mut reserved = None;
        for n in 0..=99 {
            let (task_name, status_name) = backup_names(&stamp, n);
            io_boundary("task_backup_create").map_err(write_failed)?;
            let task_file = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join(&task_name))
            {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(write_failed(e)),
            };
            let status_file = io_boundary("status_backup_create").and_then(|()| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(root.join(&status_name))
            });
            match status_file {
                Ok(f) => {
                    reserved = Some((n, task_name, status_name, task_file, f));
                    break;
                }
                Err(e) => {
                    drop(task_file);
                    cleanup_owned(&root.join(&task_name), &hash_bytes(&[]));
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        continue;
                    }
                    return Err(write_failed(e));
                }
            }
        }
        let (n, task_name, status_name, mut task_file, mut status_file) =
            reserved.ok_or_else(|| {
                TaskOpError::BackupExhausted(root.join(format!("TASK.{stamp}.bak.md")))
            })?;
        journal.suffix = n;
        journal.task.backup = Some(task_name.clone());
        journal.status.backup = Some(status_name.clone());
        // Track bytes actually written, so failure cleanup cannot delete a
        // foreign replacement. Successfully synced archives stay immutable.
        let mut task_owned_hash = hash_bytes(&[]);
        let mut status_owned_hash = hash_bytes(&[]);
        let task_result = copy_backup(
            &task_path,
            &mut task_file,
            journal.task.existed,
            "task_backup",
            &mut task_owned_hash,
        );
        let task_finished = task_result.is_ok();
        let copied = task_result.and_then(|()| {
            copy_backup(
                &status_path,
                &mut status_file,
                journal.status.existed,
                "status_backup",
                &mut status_owned_hash,
            )
        });
        drop(task_file);
        drop(status_file);
        if let Err(e) = copied {
            if !task_finished {
                cleanup_owned(&root.join(&task_name), &task_owned_hash);
            }
            cleanup_owned(&root.join(&status_name), &status_owned_hash);
            return Err(write_failed(e));
        }
        for (side, expected) in [
            (&journal.task, &original_task),
            (&journal.status, &original_status),
        ] {
            let backup_hash = source_state(&root.join(side.backup.as_ref().unwrap()))
                .map_err(write_failed)?
                .hash;
            if backup_hash != Some(expected.hash.clone().unwrap_or_else(|| hash_bytes(&[]))) {
                return Err(clean_conflict("archive source changed"));
            }
        }
    }
    for (side, bytes, boundary) in [
        (&journal.task, new_content.as_bytes(), "task_stage"),
        (&journal.status, seed_bytes.as_slice(), "status_stage"),
    ] {
        io_boundary(&format!("{boundary}_create")).map_err(write_failed)?;
        let mut stage = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(&side.stage))
            .map_err(write_failed)?;
        io_boundary(&format!("{boundary}_write")).map_err(write_failed)?;
        stage.write_all(bytes).map_err(write_failed)?;
        sync_file(&stage, &format!("{boundary}_sync")).map_err(write_failed)?;
    }
    io_boundary("before_source_recheck").map_err(write_failed)?;
    if source_state(&task_path).map_err(write_failed)? != original_task
        || source_state(&status_path).map_err(write_failed)? != original_status
    {
        return Err(clean_conflict("source changed before journal"));
    }
    let journal_bytes = serde_json::to_vec(&journal).map_err(|e| clean_conflict(e.to_string()))?;
    let tmp = root.join(format!("{JOURNAL_NAME}.tmp.{tx}"));
    io_boundary("journal_create").map_err(write_failed)?;
    let mut journal_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(write_failed)?;
    io_boundary("journal_write").map_err(write_failed)?;
    journal_file
        .write_all(&journal_bytes)
        .map_err(write_failed)?;
    sync_file(&journal_file, "journal_stage_sync").map_err(write_failed)?;
    drop(journal_file);
    if root.join(JOURNAL_NAME).exists() {
        return Err(clean_conflict("foreign journal appeared"));
    }
    io_boundary("journal_publish").map_err(write_failed)?;
    rename_retry(&tmp, &root.join(JOURNAL_NAME)).map_err(write_failed)?;
    io_boundary("after_journal_publish").map_err(clean_pending)?;
    let published = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(JOURNAL_NAME))
        .map_err(clean_pending)?;
    sync_file(&published, "journal_sync").map_err(clean_pending)?;
    drop(published);
    finish_clean_locked(root, &journal, &journal_bytes)?;
    Ok(EditOutcome::Wrote {
        backup: journal.task.backup.map(|p| root.join(p)),
        content: new_content,
        title: title_value_of(&cleaned),
    })
}

// ── Core flow (clock-injection seam, §G.1) ─────────────────────────────────

pub(crate) fn perform_inner<F>(
    wg_root: &Path,
    op: TaskOp,
    now: F,
) -> Result<EditOutcome, TaskOpError>
where
    F: FnOnce() -> DateTime<Utc>,
{
    let task_path = wg_root.join("TASK.md");
    let lock_path = wg_root.join("TASK.md.lock");
    // Per-PID tmp suffix eliminates the tmp-collision race during stale-lock
    // recovery (HIGH-2).
    let tmp_path = wg_root.join(format!("TASK.md.tmp.{}", std::process::id()));

    let _lock = LockGuard::acquire(&lock_path, LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M)?;

    recover_clean_pair_locked(wg_root)?;

    // ── 2. Read existing content ───────────────────────────────────────────
    let (existing, file_existed) = match std::fs::read_to_string(&task_path) {
        Ok(s) => (s, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => return Err(TaskOpError::ReadFailed(task_path, e)),
    };

    if matches!(op, TaskOp::Clean) {
        return clean_pair_locked(wg_root, &existing, now());
    }

    // ── 2a. Capture pre-edit sentinel (HIGH-4) ────────────────────────────
    // Snapshot is taken AFTER the read, so an external write that lands in the
    // read→metadata window (~µs) is reflected in the captured snapshot rather
    // than detected at step 7a. Do NOT "tighten" by moving this BEFORE the
    // read: that would introduce an unbounded write-between-snapshot-and-read
    // window. The sentinel covers the realistic editor-save case (seconds apart).
    let pre_sentinel: Option<(u64, Option<SystemTime>)> = if file_existed {
        match std::fs::metadata(&task_path) {
            Ok(m) => Some((m.len(), m.modified().ok())),
            Err(_) => None,
        }
    } else {
        None
    };

    // ── 3-4. Parse + apply edit ───────────────────────────────────────────
    let parsed = parse_task(&existing);
    let current_title = title_value_of(&parsed);
    let current_is_user_owned = current_title.as_deref().is_some_and(is_user_owned_title);

    // #738 user-lock: a coordinator SetTitle must never overwrite a human-owned
    // (`USER:`) title. This is a successful outcome (exit 0): no write, no backup.
    // Ordered before the reserved-prefix check so a coordinator re-sending the
    // same `USER: X` is rejected here (not treated as invalid input).
    if matches!(op, TaskOp::SetTitle(_)) && current_is_user_owned {
        return Ok(EditOutcome::RejectedUserTitle {
            content: existing.clone(),
            title: current_title.clone(),
        });
    }

    // #738 reserved-prefix: a coordinator SetTitle whose own input starts with
    // `USER:` is invalid input (it would forge a human lock) unless an existing
    // user-owned title already blocked the write above. Hard error, exit 1, no
    // write, no backup. SetUserTitle/creation are unaffected (human-owned paths).
    if let TaskOp::SetTitle(title) = &op {
        if is_user_owned_title(title) {
            return Err(TaskOpError::ReservedUserTitlePrefix);
        }
    }

    let new_parsed = apply_edit(&parsed, &op);

    // ── 5. Idempotence short-circuit ──────────────────────────────────────
    // SetTitle: semantic — short-circuit when YAML-decoded title value is
    //   unchanged (re-quoting/escaping never produces a NoOp).
    // Clean:    structural — short-circuit when post-edit frontmatter and
    //   body byte-match the pre-edit shape (covers repeated clean clicks).
    // AppendBody: never NoOp.
    let is_noop = match op {
        TaskOp::SetTitle(_) | TaskOp::SetUserTitle(_) => {
            title_value_of(&new_parsed) == current_title
        }
        TaskOp::Clean => {
            new_parsed.frontmatter == parsed.frontmatter && new_parsed.body == parsed.body
        }
        TaskOp::AppendBody(_) => false,
        TaskOp::SetBody(ref text) => *text == parsed.body,
    };
    if is_noop {
        return Ok(EditOutcome::NoOp {
            content: existing.clone(),
            title: title_value_of(&parsed),
        });
    }

    // ── 5b. Render to bytes for the upcoming write ────────────────────────
    let new_content = render(&new_parsed);

    // ── 6. Backup with collision-suffix loop (only if file existed) ───────
    let backup_path: Option<PathBuf> = if file_existed {
        // NOTE: backup filenames sort by wall-clock; an NTP backward correction
        // can break chronological ordering. Acceptable per spec; see plan #137 LOW-2.
        let ts = now().format("%Y%m%d-%H%M%S").to_string();
        let mut chosen: Option<PathBuf> = None;
        for n in 0..=99u32 {
            let candidate = if n == 0 {
                wg_root.join(format!("TASK.{}.bak.md", ts))
            } else {
                wg_root.join(format!("TASK.{}.{}.bak.md", ts, n))
            };
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => {
                    drop(file);
                    chosen = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(TaskOpError::BackupFailed(candidate, e)),
            }
        }
        let bp = chosen.ok_or_else(|| {
            TaskOpError::BackupExhausted(wg_root.join(format!("TASK.{}.bak.md", ts)))
        })?;
        match std::fs::copy(&task_path, &bp) {
            Ok(_) => Some(bp),
            Err(copy_err) => {
                // §C.1: fs::copy makes NO guarantee of partial-file cleanup.
                let _ = std::fs::remove_file(&bp);
                return Err(TaskOpError::BackupFailed(bp, copy_err));
            }
        }
    } else {
        None
    };

    // ── 7. Atomic write: tmp + sentinel-check + rename ────────────────────
    let write_tmp = || {
        #[cfg(test)]
        io_boundary("nonclean_tmp_write")?;
        std::fs::write(&tmp_path, &new_content)
    };
    if let Err(e) = write_tmp() {
        // MED-6 cleanup
        let _ = std::fs::remove_file(&tmp_path);
        return Err(TaskOpError::TmpWriteFailed(tmp_path, e));
    }

    // 7a. Sentinel check — see HIGH-4. Realistic editor-save case caught;
    // sub-millisecond TOCTOU at the read→metadata window remains theoretically open.
    // FAT32 mtime granularity is 2 s — for typical AC layouts (NTFS / EXT4 / APFS,
    // sub-second), this is not a concern.
    #[cfg(test)]
    io_boundary("nonclean_before_recheck").expect("mutation-only fixture boundary");
    if let Some((pre_len, pre_mtime)) = pre_sentinel {
        match std::fs::metadata(&task_path) {
            Ok(now_meta) => {
                let now_mtime = now_meta.modified().ok();
                let len_changed = now_meta.len() != pre_len;
                let mtime_changed = match (pre_mtime, now_mtime) {
                    (Some(a), Some(b)) => a != b,
                    _ => false,
                };
                if len_changed || mtime_changed {
                    let _ = std::fs::remove_file(&tmp_path);
                    let bp = backup_path
                        .clone()
                        .expect("file_existed ⇒ backup_path is Some");
                    return Err(TaskOpError::ExternalWrite(bp));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // External delete between read and rename. Without this branch,
                // rename to a vanished destination silently re-creates the file
                // (normal create on both Windows MoveFileExW(MOVEFILE_REPLACE_EXISTING)
                // and Unix), undoing the external delete. Treat as ExternalWrite.
                let _ = std::fs::remove_file(&tmp_path);
                let bp = backup_path
                    .clone()
                    .expect("file_existed ⇒ backup_path is Some");
                return Err(TaskOpError::ExternalWrite(bp));
            }
            Err(_) => { /* other transient FS error — let rename surface the real error */ }
        }
    }

    // 7b. Rename with retry on Windows AV/Explorer transient holds (MED-4).
    let do_rename = || -> Result<(), std::io::Error> {
        for attempt in 0..=2u32 {
            let rename = || {
                #[cfg(test)]
                io_boundary("nonclean_rename")?;
                std::fs::rename(&tmp_path, &task_path)
            };
            match rename() {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let retry = e.kind() == std::io::ErrorKind::PermissionDenied
                        || e.raw_os_error() == Some(32) // ERROR_SHARING_VIOLATION
                        || e.raw_os_error() == Some(5); // ERROR_ACCESS_DENIED
                    if attempt < 2 && retry {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        unreachable!("loop body always returns")
    };

    if let Err(e) = do_rename() {
        // MED-1 cleanup — keeps I20's "no TASK.md.tmp.* litter" assertion holding.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(TaskOpError::RenameFailed(e, backup_path));
    }

    Ok(EditOutcome::Wrote {
        backup: backup_path,
        content: new_content.clone(),
        title: title_value_of(&new_parsed),
    })
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
