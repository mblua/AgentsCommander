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
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::{Arc, Barrier};
    use std::thread;

    /// Auto-cleaned temp dir for fixture roots. Mirrors `config/teams.rs::FixtureRoot`
    /// — copied locally so we don't have to make it `pub(crate)` cross-module.
    struct FixtureRoot(PathBuf);
    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl FixtureRoot {
        fn new(prefix: &str) -> Self {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::process::id().hash(&mut h);
            std::thread::current().id().hash(&mut h);
            let path = std::env::temp_dir().join(format!(
                "{}-{}-{}",
                prefix,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                h.finish()
            ));
            std::fs::create_dir_all(&path).expect("fixture root");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    fn fixed_now_at(year: i32, month: u32, day: u32, h: u32, m: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, h, m, s).unwrap()
    }

    // ── U1-U6: parse_task ──────────────────────────────────────────────

    #[test]
    fn parse_task_no_frontmatter() {
        let p = parse_task("# Body");
        assert!(!p.has_frontmatter);
        assert_eq!(p.body, "# Body");
    }

    #[test]
    fn parse_task_empty_string() {
        let p = parse_task("");
        assert!(!p.has_frontmatter);
        assert_eq!(p.body, "");
    }

    #[test]
    fn parse_task_well_formed_frontmatter() {
        let p = parse_task("---\ntitle: x\n---\nbody");
        assert!(p.has_frontmatter);
        assert_eq!(p.frontmatter, vec!["title: x".to_string()]);
        assert_eq!(p.body, "body");
    }

    #[test]
    fn parse_task_frontmatter_no_title_field() {
        let p = parse_task("---\nfoo: bar\n---\nbody");
        assert!(p.has_frontmatter);
        assert_eq!(p.frontmatter, vec!["foo: bar".to_string()]);
    }

    #[test]
    fn parse_task_unclosed_frontmatter_treated_as_body() {
        let input = "---\ntitle: x\n(no closer)\n";
        let p = parse_task(input);
        assert!(!p.has_frontmatter);
        assert_eq!(p.body, input);
    }

    #[test]
    fn parse_task_tolerates_crlf() {
        // CRIT-1 strict pin: body must be exactly "body" with no leading "\n".
        let p = parse_task("---\r\ntitle: x\r\n---\r\nbody");
        assert!(p.has_frontmatter);
        assert_eq!(p.body, "body");
        assert_eq!(p.line_ending, "\r\n");
    }

    // ── U7-U13: apply_set_title ─────────────────────────────────────────

    #[test]
    fn apply_set_title_creates_frontmatter_when_absent() {
        let parsed = parse_task("");
        let p = apply_set_title(&parsed, "X");
        let out = render(&p);
        assert_eq!(out, "---\ntitle: 'X'\n---\n");
    }

    #[test]
    fn apply_set_title_replaces_existing_title_value() {
        let parsed = parse_task("---\ntitle: old\n---\nbody\n");
        let p = apply_set_title(&parsed, "new");
        assert_eq!(p.frontmatter, vec!["title: 'new'".to_string()]);
        assert_eq!(p.body, "body\n");
        let legacy = parse_task("Ready to start a new topic\n");
        assert_eq!(apply_set_title(&legacy, "new").body, legacy.body);
        assert_eq!(
            apply_edit(&legacy, &TaskOp::SetUserTitle("new".into())).body,
            legacy.body
        );
    }

    #[test]
    fn apply_set_title_inserts_into_existing_frontmatter() {
        let parsed = parse_task("---\nfoo: bar\n---\nbody\n");
        let p = apply_set_title(&parsed, "x");
        assert_eq!(
            p.frontmatter,
            vec!["title: 'x'".to_string(), "foo: bar".to_string()]
        );
    }

    #[test]
    fn apply_set_title_preserves_other_frontmatter_fields() {
        let parsed = parse_task("---\nfoo: 1\ntitle: old\nbar: 2\n---\nbody");
        let p = apply_set_title(&parsed, "new");
        assert_eq!(
            p.frontmatter,
            vec![
                "foo: 1".to_string(),
                "title: 'new'".to_string(),
                "bar: 2".to_string(),
            ]
        );
        assert_eq!(p.body, "body");
    }

    #[test]
    fn apply_set_title_yaml_escapes_single_quote() {
        let parsed = parse_task("");
        let p = apply_set_title(&parsed, "won't");
        let out = render(&p);
        assert!(out.contains("title: 'won''t'"));
    }

    #[test]
    fn apply_set_title_yaml_safe_with_colon_and_hash() {
        let title = "v1.0: stable #release";
        let parsed = parse_task("");
        let p = apply_set_title(&parsed, title);
        let out = render(&p);
        // Round-trip via parser
        let re = parse_task(&out);
        assert_eq!(title_value_of(&re).as_deref(), Some(title));
    }

    #[test]
    fn apply_set_title_idempotent_when_value_matches() {
        let fix = FixtureRoot::new("task-u13");
        let wg = fix.path().join("wg-1-test");
        std::fs::create_dir_all(&wg).unwrap();
        // Seed file
        std::fs::write(wg.join("TASK.md"), "---\ntitle: 'X'\n---\nbody\n").unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let r = perform_inner(&wg, TaskOp::SetTitle("X".into()), now).unwrap();
        match r {
            EditOutcome::NoOp { .. } => {}
            other => panic!("expected NoOp, got {:?}", other),
        }
    }

    // ── U14-U18: apply_append_body ──────────────────────────────────────

    #[test]
    fn apply_append_body_to_empty_file() {
        let parsed = parse_task("");
        let p = apply_append_body(&parsed, "hello");
        assert_eq!(p.body, "hello\n");
    }

    #[test]
    fn apply_append_body_preserves_prior_content() {
        let parsed = parse_task("---\ntitle: x\n---\nold\n");
        let p = apply_append_body(&parsed, "new");
        let out = render(&p);
        assert_eq!(out, "---\ntitle: x\n---\nold\n\nnew\n");
        let legacy = parse_task("Ready to start a new topic\n");
        assert_eq!(
            apply_append_body(&legacy, "summary").body,
            "Ready to start a new topic\n\nsummary\n"
        );
    }

    #[test]
    fn apply_append_body_normalizes_blank_line_separator() {
        // Body with multiple trailing newlines: collapses to exactly one blank line.
        let parsed = ParsedTask {
            bom: false,
            line_ending: "\n",
            has_frontmatter: false,
            frontmatter: Vec::new(),
            body: "old\n\n\n\n".to_string(),
        };
        let p = apply_append_body(&parsed, "new");
        assert_eq!(p.body, "old\n\nnew\n");
    }

    #[test]
    fn apply_append_body_does_not_touch_frontmatter() {
        let parsed = parse_task("---\ntitle: x\n---\nold\n");
        let p = apply_append_body(&parsed, "new");
        assert_eq!(p.frontmatter, parsed.frontmatter);
    }

    #[test]
    fn apply_append_body_strips_trailing_whitespace_from_text() {
        let parsed = parse_task("");
        let p = apply_append_body(&parsed, "hello   \n\n");
        assert_eq!(p.body, "hello\n");
    }

    // ── U19-U22: LockGuard + atomic publish ─────────────────────────────

    #[test]
    fn lock_guard_creates_and_preserves_lockfile() {
        let fix = FixtureRoot::new("task-u19");
        let lock_path = fix.path().join("TASK.md.lock");
        {
            let _g = LockGuard::acquire(&lock_path, LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M).unwrap();
            assert!(lock_path.exists());
        }
        assert!(lock_path.exists());
    }

    #[test]
    fn lock_guard_blocks_concurrent_acquisition() {
        let fix = FixtureRoot::new("task-u20");
        let lock_path = fix.path().join("TASK.md.lock");
        let _held = LockGuard::acquire(&lock_path, LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M).unwrap();
        let res = LockGuard::acquire(&lock_path, Duration::from_millis(100), LOCK_STALE_AFTER_5M);
        assert!(matches!(res, Err(TaskOpError::LockTimeout)));
    }

    #[test]
    fn lock_guard_ignores_stale_unowned_contents() {
        // Test approach (std-only — no `filetime`, no FFI):
        // pre-create the lockfile via OpenOptions::create_new, drop the handle,
        // sleep ~30 ms, then call acquire with a small `stale_after` (e.g. 10 ms).
        // The production constant is LOCK_STALE_AFTER_5M (300 s); the test uses
        // a smaller value because std-only Rust cannot fake file mtimes.
        let fix = FixtureRoot::new("task-u21");
        let lock_path = fix.path().join("TASK.md.lock");
        {
            let f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
                .unwrap();
            drop(f);
        }
        std::thread::sleep(Duration::from_millis(30));
        let g = LockGuard::acquire(
            &lock_path,
            Duration::from_secs(2),
            Duration::from_millis(10),
        )
        .expect("stale lock should be recovered");
        assert!(lock_path.exists());
        drop(g);
        assert!(lock_path.exists());
    }

    #[test]
    fn atomic_publish_via_rename_round_trip() {
        let fix = FixtureRoot::new("task-u22");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let _r = perform_inner(&wg, TaskOp::SetTitle("X".into()), now).unwrap();
        // After success, no per-PID tmp file remains.
        let pid_tmp = wg.join(format!("TASK.md.tmp.{}", std::process::id()));
        assert!(!pid_tmp.exists());
        // No stray TASK.md.tmp.* either.
        for entry in std::fs::read_dir(&wg).unwrap().flatten() {
            let n = entry.file_name();
            let name = n.to_string_lossy();
            assert!(
                !name.starts_with("TASK.md.tmp."),
                "leftover tmp file: {}",
                name
            );
        }
        // Stable lock file remains.
        assert!(wg.join("TASK.md.lock").exists());
    }

    // ── U23: backup filename format ─────────────────────────────────────

    #[test]
    fn backup_filename_uses_utc_timestamp_format() {
        let fix = FixtureRoot::new("task-u23");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        std::fs::write(wg.join("TASK.md"), "old\n").unwrap();
        let now = || fixed_now_at(2026, 1, 1, 12, 34, 56);
        let r = perform_inner(&wg, TaskOp::SetTitle("X".into()), now).unwrap();
        let bp = match r {
            EditOutcome::Wrote {
                backup: Some(bp), ..
            } => bp,
            other => panic!("expected Wrote with backup, got {:?}", other),
        };
        let name = bp.file_name().unwrap().to_string_lossy().into_owned();
        // Pattern: TASK.YYYYMMDD-HHMMSS(.N)?.bak.md
        assert!(
            name == "TASK.20260101-123456.bak.md"
                || name.starts_with("TASK.20260101-123456.") && name.ends_with(".bak.md"),
            "unexpected backup filename: {}",
            name
        );
    }

    // ── U24, U30: backup-failure path ───────────────────────────────────

    #[test]
    fn backup_failure_aborts_write_and_preserves_task() {
        // Per plan §9 U24, the test pins "backup failure aborts cleanly" — either
        // BackupExhausted (loop exhausts 100 collisions) or BackupFailed (a
        // create_new error other than AlreadyExists). On Windows, attempting to
        // OpenOptions::create_new a path where a directory already exists returns
        // PermissionDenied (not AlreadyExists), so we get BackupFailed; on Unix,
        // the same returns IsADirectory (also not AlreadyExists). Both are
        // graceful failures; the assertion accepts either variant.
        let fix = FixtureRoot::new("task-u24");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let task = wg.join("TASK.md");
        std::fs::write(&task, "old\n").unwrap();
        let original = std::fs::read(&task).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        // Pre-create directories at every candidate path so create_new fails
        // for all of them.
        for n in 0..=99u32 {
            let candidate = if n == 0 {
                wg.join("TASK.20260101-000000.bak.md")
            } else {
                wg.join(format!("TASK.20260101-000000.{}.bak.md", n))
            };
            std::fs::create_dir(&candidate).unwrap();
        }
        let result = perform_inner(&wg, TaskOp::SetTitle("x".into()), now);
        assert!(
            matches!(
                result,
                Err(TaskOpError::BackupExhausted(_)) | Err(TaskOpError::BackupFailed(_, _))
            ),
            "expected backup-class failure, got {:?}",
            result
        );
        // TASK.md unchanged.
        assert_eq!(std::fs::read(&task).unwrap(), original);
        // U30: lock cleaned up.
        assert!(wg.join("TASK.md.lock").exists());
        // No tmp file written (we abort before the tmp-write).
        let pid_tmp = wg.join(format!("TASK.md.tmp.{}", std::process::id()));
        assert!(!pid_tmp.exists());
    }

    #[test]
    fn backup_failure_releases_lockfile() {
        // Companion to U24: assert lock file cleaned even on backup failure.
        let fix = FixtureRoot::new("task-u30");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        std::fs::write(wg.join("TASK.md"), "old\n").unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        for n in 0..=99u32 {
            let candidate = if n == 0 {
                wg.join("TASK.20260101-000000.bak.md")
            } else {
                wg.join(format!("TASK.20260101-000000.{}.bak.md", n))
            };
            std::fs::create_dir(&candidate).unwrap();
        }
        let _ = perform_inner(&wg, TaskOp::SetTitle("x".into()), now);
        assert!(wg.join("TASK.md.lock").exists());
    }

    // ── U25: concurrent set-title + append-body ─────────────────────────

    #[test]
    fn concurrent_set_title_and_append_body_both_apply() {
        // MED-2: synchronize via Barrier so threads contend at the same instant.
        // Without the barrier the test would pass for the wrong reason.
        for _iter in 0..10 {
            let fix = FixtureRoot::new("task-u25");
            let wg = fix.path().join("wg-1");
            std::fs::create_dir_all(&wg).unwrap();
            let barrier = Arc::new(Barrier::new(2));
            let wg_clone1 = wg.clone();
            let wg_clone2 = wg.clone();
            let b1 = barrier.clone();
            let b2 = barrier.clone();

            let h1 = thread::spawn(move || {
                b1.wait();
                perform(&wg_clone1, TaskOp::SetTitle("X".into()))
            });
            let h2 = thread::spawn(move || {
                b2.wait();
                perform(&wg_clone2, TaskOp::AppendBody("appended body line".into()))
            });
            let r1 = h1.join().unwrap();
            let r2 = h2.join().unwrap();
            // At least one must succeed; whichever lost the lock may LockTimeout
            // (unlikely with the 5 s window), but we don't strictly require both.
            assert!(r1.is_ok() || matches!(r1, Err(TaskOpError::LockTimeout)));
            assert!(r2.is_ok() || matches!(r2, Err(TaskOpError::LockTimeout)));
            if r1.is_ok() && r2.is_ok() {
                let final_content = std::fs::read_to_string(wg.join("TASK.md")).unwrap();
                assert!(final_content.contains("title: 'X'"));
                assert!(final_content.contains("appended body line"));
            }
        }
    }

    // ── U26-U28: parser/applier edge cases ──────────────────────────────

    #[test]
    fn parse_task_tolerates_trailing_space_on_markers() {
        let p = parse_task("--- \ntitle: x\n--- \nbody");
        assert!(p.has_frontmatter);
        assert_eq!(p.frontmatter, vec!["title: x".to_string()]);
        assert_eq!(p.body, "body");
    }

    #[test]
    fn parse_task_unicode_in_body_preserved_byte_for_byte() {
        let body = "café\n🎉\n";
        let input = format!("---\ntitle: x\n---\n{}", body);
        let p = parse_task(&input);
        assert!(p.has_frontmatter);
        assert_eq!(p.body, body);
    }

    #[test]
    fn apply_set_title_preserves_indentation_of_existing_title_line() {
        let parsed = parse_task("---\n  title: old\n---\n");
        let p = apply_set_title(&parsed, "new");
        assert_eq!(p.frontmatter, vec!["  title: 'new'".to_string()]);
    }

    // ── U29: backup collision suffix loop ───────────────────────────────

    #[test]
    fn backup_collision_within_same_second_does_not_clobber_prior_backup() {
        let fix = FixtureRoot::new("task-u29");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        std::fs::write(wg.join("TASK.md"), "first\n").unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        // First call → TASK.20260101-000000.bak.md
        let _ = perform_inner(&wg, TaskOp::AppendBody("a".into()), now).unwrap();
        // Second call same second → TASK.20260101-000000.1.bak.md
        let _ = perform_inner(&wg, TaskOp::AppendBody("b".into()), now).unwrap();
        let bk0 = wg.join("TASK.20260101-000000.bak.md");
        let bk1 = wg.join("TASK.20260101-000000.1.bak.md");
        assert!(bk0.exists(), "first backup should exist");
        assert!(
            bk1.exists(),
            "collision-suffixed second backup should exist"
        );
        // First backup contains "first\n" (the pre-edit state of the first call).
        assert_eq!(std::fs::read_to_string(&bk0).unwrap(), "first\n");
    }

    // ── U31: BOM round-trip ─────────────────────────────────────────────

    #[test]
    fn parse_task_strips_and_re_emits_leading_bom() {
        let input = "\u{FEFF}---\ntitle: x\n---\nbody";
        let p = parse_task(input);
        assert!(p.bom);
        assert!(p.has_frontmatter);
        assert_eq!(p.frontmatter, vec!["title: x".to_string()]);
        assert_eq!(p.body, "body");
        let rendered = render(&p);
        assert_eq!(rendered, input);
    }

    // ── U32: CRLF round-trip byte-exact ─────────────────────────────────

    #[test]
    fn set_title_round_trip_preserves_crlf_no_extra_blank_line() {
        let input = "---\r\ntitle: old\r\n---\r\nbody\r\n";
        let parsed = parse_task(input);
        assert_eq!(parsed.line_ending, "\r\n");
        let edited = apply_set_title(&parsed, "new");
        let out = render(&edited);
        // Closing "---\r\n" is followed immediately by "body\r\n" (no blank line).
        assert!(
            out.contains("---\r\nbody\r\n"),
            "expected '---\\r\\nbody\\r\\n' in output, got: {:?}",
            out
        );
        // No extra leading "\r\n" before "body".
        assert!(!out.contains("---\r\n\r\nbody"));
    }

    // ── U33: line-ending preservation ───────────────────────────────────

    #[test]
    fn parse_task_preserves_dominant_line_ending() {
        let crlf = parse_task("---\r\ntitle: x\r\n---\r\n");
        assert_eq!(crlf.line_ending, "\r\n");
        let lf = parse_task("---\ntitle: x\n---\n");
        assert_eq!(lf.line_ending, "\n");
        let edited = apply_set_title(&crlf, "y");
        let out = render(&edited);
        assert!(out.contains("---\r\ntitle: 'y'\r\n---\r\n"));
    }

    // ── U34: append-body line-ending trade-off pin (NIT-E) ──────────────

    #[test]
    fn apply_append_body_preserves_internal_body_line_endings_and_documents_trailing_loss() {
        // Pins the §5 row-510 trade-off: existing body's internal CRLF is preserved
        // byte-for-byte, but the body's trailing CRLF gets trim_end'd and replaced
        // by an LF separator + LF terminator.
        let parsed = ParsedTask {
            bom: false,
            line_ending: "\r\n",
            has_frontmatter: false,
            frontmatter: Vec::new(),
            body: "Line1\r\nLine2\r\n".to_string(),
        };
        let p = apply_append_body(&parsed, "NewLine");
        // Line1's CRLF preserved; Line2's trailing CRLF replaced; NewLine ends with LF.
        assert_eq!(p.body, "Line1\r\nLine2\n\nNewLine\n");
    }

    // ── U35: BOM-only existing file preserves BOM through set-title (LOW-1)

    #[test]
    fn apply_set_title_preserves_bom_on_bom_only_existing_file() {
        // BOM-only file (e.g. coordinator opened TASK.md in Notepad on
        // Windows, which writes \xEF\xBB\xBF, then saved). The brand-new
        // branch must NOT fire — that would strip the BOM and violate the
        // HIGH-3 byte-exact round-trip guarantee. The fix gates the
        // brand-new branch on !parsed.bom so this case falls through to the
        // "no frontmatter, preserve bom/eol" branch.
        let parsed = parse_task("\u{FEFF}");
        let p = apply_set_title(&parsed, "X");
        assert_eq!(render(&p), "\u{FEFF}---\ntitle: 'X'\n---\n");
    }

    // ── extra: title_value_of helper ────────────────────────────────────

    #[test]
    fn title_value_of_canonical_single_quoted() {
        let p = parse_task("---\ntitle: 'won''t'\n---\n");
        assert_eq!(title_value_of(&p).as_deref(), Some("won't"));
    }

    #[test]
    fn title_value_of_bare_scalar() {
        let p = parse_task("---\ntitle: bare\n---\n");
        assert_eq!(title_value_of(&p).as_deref(), Some("bare"));
    }

    #[test]
    fn title_value_of_absent() {
        let p = parse_task("---\nfoo: bar\n---\n");
        assert_eq!(title_value_of(&p), None);
    }

    // ── U36-U41: TaskOp::Clean ─────────────────────────────────────────

    #[test]
    fn apply_clean_creates_canonical_clean_for_empty_file() {
        let parsed = parse_task("");
        let p = apply_clean(&parsed);
        let out = render(&p);
        assert_eq!(out, "---\ntitle: 'Clean'\n---\n");
    }

    #[test]
    fn apply_clean_replaces_existing_frontmatter_and_body() {
        // Round 2 (dev-rust R1.3): hard-reset semantics also normalize
        // indentation — a coordinator-edited `  title: 'X'` (two-space
        // indent) becomes unindented `"title: 'Clean'"`. Idempotence
        // check in §3.1.4 will treat this as write-worthy.
        let parsed = parse_task("---\ntitle: 'Old'\nfoo: bar\n---\nold body\n");
        let p = apply_clean(&parsed);
        // Frontmatter is REPLACED entirely (foo: bar is dropped — Clean
        // is a hard reset, not a merge).
        assert_eq!(p.frontmatter, vec!["title: 'Clean'".to_string()]);
        assert_eq!(p.body, "");
    }

    #[test]
    fn apply_clean_preserves_crlf_and_bom() {
        // Clean preserves BOM/CRLF frontmatter; a second Clean is NoOp.
        let input = "\u{FEFF}---\r\ntitle: old\r\nx: 1\r\n---\r\nbody\r\n";
        let parsed = parse_task(input);
        let p = apply_clean(&parsed);
        assert!(p.bom);
        assert_eq!(p.line_ending, "\r\n");
        let out = render(&p);
        assert_eq!(out, "\u{FEFF}---\r\ntitle: 'Clean'\r\n---\r\n");
        assert!(p.body.is_empty());
        let fix = FixtureRoot::new("task-clean-crlf");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        std::fs::write(wg.join("TASK.md"), input).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        assert!(matches!(
            perform_inner(&wg, TaskOp::Clean, now).unwrap(),
            EditOutcome::Wrote { .. }
        ));
        let log = std::fs::read(wg.join("TASK-status.jsonl")).unwrap();
        let entries = std::fs::read_dir(&wg).unwrap().count();
        assert!(matches!(
            perform_inner(&wg, TaskOp::Clean, now).unwrap(),
            EditOutcome::NoOp { .. }
        ));
        assert_eq!(std::fs::read(wg.join("TASK.md")).unwrap(), out.as_bytes());
        assert_eq!(std::fs::read(wg.join("TASK-status.jsonl")).unwrap(), log);
        assert_eq!(std::fs::read_dir(&wg).unwrap().count(), entries);
    }

    #[test]
    fn perform_clean_idempotent_on_canonical_clean() {
        let fix = FixtureRoot::new("task-u39");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        std::fs::write(wg.join("TASK.md"), "---\ntitle: 'Clean'\n---\n").unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let r = perform_inner(&wg, TaskOp::Clean, now).unwrap();
        match r {
            EditOutcome::NoOp { .. } => {}
            other => panic!("expected NoOp, got {:?}", other),
        }
        // No backup file created.
        let entries: Vec<_> = std::fs::read_dir(&wg).unwrap().flatten().collect();
        let bak_count = entries
            .iter()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak.md"))
            .count();
        assert_eq!(bak_count, 0);
        perform_inner(&wg, TaskOp::AppendBody("human summary".into()), now).unwrap();
        assert_eq!(read_snapshot(&wg).unwrap().description, "human summary\n");
    }

    #[test]
    fn perform_clean_writes_backup_when_file_existed() {
        // Round 2 (Grinch HIGH-2): assert the backup CONTENTS match the
        // pre-clean bytes. The whole point of the backup is recovery; a
        // regression where the backup file gets the post-clean (Clean)
        // bytes instead of the prior state would be silently shipped
        // without this assertion.
        let fix = FixtureRoot::new("task-u40");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let pre_clean = "---\ntitle: stale\n---\nstale body\n";
        std::fs::write(wg.join("TASK.md"), pre_clean).unwrap();
        let now = || fixed_now_at(2026, 5, 7, 12, 0, 0);
        let r = perform_inner(&wg, TaskOp::Clean, now).unwrap();
        let backup_path = match &r {
            EditOutcome::Wrote {
                backup: Some(bp), ..
            } => bp.clone(),
            other => panic!("expected Wrote with backup, got {:?}", other),
        };
        // HIGH-2 assertion: backup bytes must equal the pre-clean file.
        let backup_content = std::fs::read_to_string(&backup_path).unwrap();
        assert_eq!(backup_content, pre_clean);
        let final_content = std::fs::read_to_string(wg.join("TASK.md")).unwrap();
        assert_eq!(final_content, "---\ntitle: 'Clean'\n---\n");
    }

    #[test]
    fn perform_clean_creates_task_when_file_missing() {
        // Round 2 (Grinch LOW-3): brand-new workgroup, no TASK.md.
        // Implementation handles this via the `if file_existed` gate at
        // task_ops.rs:455 — Clean writes the canonical form with no
        // backup. Pin the behavior here so a future refactor that
        // reorders the gate doesn't regress silently.
        let fix = FixtureRoot::new("task-u41");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let r = perform_inner(&wg, TaskOp::Clean, now).unwrap();
        assert!(matches!(r, EditOutcome::Wrote { backup: None, .. }));
        assert_eq!(
            std::fs::read_to_string(wg.join("TASK.md")).unwrap(),
            "---\ntitle: 'Clean'\n---\n"
        );
        let bak_count = std::fs::read_dir(&wg)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak.md"))
            .count();
        assert_eq!(bak_count, 0);
    }

    #[test]
    fn concurrent_writes_return_correct_post_edit_content() {
        for _iter in 0..10 {
            let fix = FixtureRoot::new("task-u42");
            let wg = fix.path().join("wg-1");
            std::fs::create_dir_all(&wg).unwrap();
            let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);

            // First call sets up the file
            perform_inner(&wg, TaskOp::SetTitle("Initial".into()), now).unwrap();

            let barrier = Arc::new(Barrier::new(2));
            let wg_clone1 = wg.clone();
            let wg_clone2 = wg.clone();
            let b1 = barrier.clone();
            let b2 = barrier.clone();

            let h1 = thread::spawn(move || {
                b1.wait();
                perform(&wg_clone1, TaskOp::AppendBody("first append".into()))
            });
            let h2 = thread::spawn(move || {
                b2.wait();
                perform(&wg_clone2, TaskOp::AppendBody("second append".into()))
            });

            let r1 = h1.join().unwrap();
            let r2 = h2.join().unwrap();

            let content1 = match r1 {
                Ok(EditOutcome::Wrote { content, .. }) => content,
                Err(TaskOpError::LockTimeout) => continue,
                other => panic!("h1 unexpected outcome: {:?}", other),
            };
            let content2 = match r2 {
                Ok(EditOutcome::Wrote { content, .. }) => content,
                Err(TaskOpError::LockTimeout) => continue,
                other => panic!("h2 unexpected outcome: {:?}", other),
            };

            let final_disk_content = std::fs::read_to_string(wg.join("TASK.md")).unwrap();

            if final_disk_content.ends_with("first append\n") {
                assert_eq!(content1, final_disk_content);
            } else if final_disk_content.ends_with("second append\n") {
                assert_eq!(content2, final_disk_content);
            } else {
                panic!("unexpected final disk content");
            }
        }
    }

    // ── #738: user-owned (USER:) title ownership ────────────────────────

    fn bak_count(wg: &Path) -> usize {
        std::fs::read_dir(wg)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak.md"))
            .count()
    }

    #[test]
    fn user_owned_title_prefixes_plain_title() {
        assert_eq!(user_owned_title("Build login"), "USER: Build login");
    }

    #[test]
    fn user_owned_title_does_not_double_prefix() {
        assert_eq!(user_owned_title("USER: Build login"), "USER: Build login");
    }

    #[test]
    fn set_title_rejects_when_existing_title_is_user_owned() {
        let fix = FixtureRoot::new("task-738-reject");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let task = wg.join("TASK.md");
        std::fs::write(&task, "---\ntitle: 'USER: Keep me'\n---\nbody\n").unwrap();
        let before = std::fs::read(&task).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
        assert!(matches!(r, EditOutcome::RejectedUserTitle { .. }));
        assert_eq!(
            std::fs::read(&task).unwrap(),
            before,
            "TASK.md must be byte-unchanged on reject"
        );
        assert_eq!(bak_count(&wg), 0, "no backup on reject");
    }

    #[test]
    fn set_title_rejects_case_variant_user_owned_titles() {
        // A UI-visible user-owned title with a case-variant key must still block
        // the coordinator overwrite (Grinch MUST-FIX #1).
        for key in ["Title", "TITLE", "tItLe"] {
            let fix = FixtureRoot::new("task-738-case");
            let wg = fix.path().join("wg-1");
            std::fs::create_dir_all(&wg).unwrap();
            let task = wg.join("TASK.md");
            std::fs::write(&task, format!("---\n{key}: 'USER: Manual'\n---\n")).unwrap();
            let before = std::fs::read(&task).unwrap();
            let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
            let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
            assert!(
                matches!(r, EditOutcome::RejectedUserTitle { .. }),
                "key {key} must reject"
            );
            assert_eq!(std::fs::read(&task).unwrap(), before, "key {key} unchanged");
            assert_eq!(bak_count(&wg), 0, "key {key} no backup");
        }
    }

    #[test]
    fn set_title_rejects_double_quoted_user_owned_title() {
        // Double-quoted scalar must decode to the same USER: marker (Grinch #1).
        let fix = FixtureRoot::new("task-738-dq");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let task = wg.join("TASK.md");
        std::fs::write(&task, "---\ntitle: \"USER: Manual\"\n---\n").unwrap();
        let before = std::fs::read(&task).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
        assert!(matches!(r, EditOutcome::RejectedUserTitle { .. }));
        assert_eq!(std::fs::read(&task).unwrap(), before);
        assert_eq!(bak_count(&wg), 0);
    }

    #[test]
    fn apply_set_title_replaces_case_variant_title_key() {
        // A coordinator SetTitle over a PLAIN case-variant `Title: Old` (not
        // user-owned) must replace that line in place, not leave it plus a new
        // lowercase duplicate (Grinch MUST-FIX #1 second half).
        let parsed = parse_task("---\nTitle: Old\n---\nbody\n");
        let p = apply_set_title(&parsed, "Auto");
        let title_lines = p
            .frontmatter
            .iter()
            .filter(|l| title_line(l).is_some())
            .count();
        assert_eq!(title_lines, 1, "exactly one title line after replacement");
        assert_eq!(p.frontmatter, vec!["title: 'Auto'".to_string()]);
    }

    #[test]
    fn set_title_rejects_reserved_user_prefix_input() {
        // Coordinator input that itself starts with USER: is invalid input when
        // the current title is not already user-owned (Grinch MUST-FIX #2).
        let fix = FixtureRoot::new("task-738-reserved");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let task = wg.join("TASK.md");
        std::fs::write(&task, "---\ntitle: 'Auto'\n---\n").unwrap();
        let before = std::fs::read(&task).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let r = perform_inner(&wg, TaskOp::SetTitle("USER: forged".into()), now);
        assert!(matches!(r, Err(TaskOpError::ReservedUserTitlePrefix)));
        assert_eq!(
            std::fs::read(&task).unwrap(),
            before,
            "unchanged on invalid input"
        );
        assert_eq!(bak_count(&wg), 0, "no backup on invalid input");
    }

    #[test]
    fn set_title_allowed_after_clean_clears_user_lock() {
        let fix = FixtureRoot::new("task-738-clean");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        std::fs::write(
            wg.join("TASK.md"),
            "---\ntitle: 'USER: Keep me'\n---\nbody\n",
        )
        .unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        // Clean resets the user-owned lock (writes canonical title 'Clean').
        perform_inner(&wg, TaskOp::Clean, now).unwrap();
        // Coordinator SetTitle is now allowed and writes a plain title.
        let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
        assert!(matches!(r, EditOutcome::Wrote { .. }));
        let parsed = parse_task(&std::fs::read_to_string(wg.join("TASK.md")).unwrap());
        assert_eq!(title_value_of(&parsed).as_deref(), Some("Auto"));
    }

    #[test]
    fn set_user_title_prefixes_plain_title() {
        let fix = FixtureRoot::new("task-738-suser");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        perform_inner(&wg, TaskOp::SetUserTitle("Manual".into()), now).unwrap();
        let parsed = parse_task(&std::fs::read_to_string(wg.join("TASK.md")).unwrap());
        assert_eq!(title_value_of(&parsed).as_deref(), Some("USER: Manual"));
    }

    #[test]
    fn set_user_title_overwrites_existing_user_title() {
        let fix = FixtureRoot::new("task-738-suser2");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        std::fs::write(wg.join("TASK.md"), "---\ntitle: 'USER: Old'\n---\n").unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        perform_inner(&wg, TaskOp::SetUserTitle("New".into()), now).unwrap();
        let parsed = parse_task(&std::fs::read_to_string(wg.join("TASK.md")).unwrap());
        assert_eq!(title_value_of(&parsed).as_deref(), Some("USER: New"));
    }

    #[test]
    fn set_title_noop_on_plain_existing_title_still_updates_contract() {
        let fix = FixtureRoot::new("task-738-noop");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        std::fs::write(wg.join("TASK.md"), "---\ntitle: 'Auto'\n---\nbody\n").unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let r = perform_inner(&wg, TaskOp::SetTitle("Auto".into()), now).unwrap();
        assert!(matches!(r, EditOutcome::NoOp { .. }));
    }

    struct SetBodyIoGuard;
    impl SetBodyIoGuard {
        fn new(fault: Option<&str>) -> Self {
            IO_MUTATION.with(|v| *v.borrow_mut() = None);
            issue_2837_fault(fault);
            Self
        }
    }
    impl Drop for SetBodyIoGuard {
        fn drop(&mut self) {
            IO_MUTATION.with(|v| *v.borrow_mut() = None);
            issue_2837_fault(None);
        }
    }
    fn set_body(root: &Path, text: &str) -> Result<EditOutcome, TaskOpError> {
        perform_inner(root, TaskOp::SetBody(text.into()), || {
            fixed_now_at(2026, 10, 7, 15, 0, 0)
        })
    }
    fn set_body_backups(root: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with(".bak.md")
            })
            .collect()
    }
    fn set_body_no_litter(root: &Path) {
        assert!(!root.join(JOURNAL_NAME).exists());
        assert!(!std::fs::read_dir(root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("TASK.md.tmp.")));
    }
    fn assert_set_body_write(
        root: &Path,
        previous: Option<&str>,
        text: &str,
        before: &TaskSnapshot,
    ) {
        let task_path = root.join("TASK.md");
        let parsed = parse_task(previous.unwrap_or(""));
        let noop = parsed.body == text;
        let result = set_body(root, text).unwrap();
        assert_eq!(matches!(result, EditOutcome::NoOp { .. }), noop);
        let backups = set_body_backups(root);
        assert_eq!(backups.len(), usize::from(!noop && previous.is_some()));
        if let Some(backup) = backups.first() {
            assert_eq!(std::fs::read(backup).unwrap(), previous.unwrap().as_bytes());
        }
        if noop {
            assert_eq!(
                std::fs::read(&task_path).ok(),
                previous.map(|p| p.as_bytes().to_vec())
            );
        } else {
            let written = std::fs::read_to_string(&task_path).unwrap();
            let after = parse_task(&written);
            assert!(after.has_frontmatter);
            assert_eq!(after.frontmatter, parsed.frontmatter);
            assert_eq!(after.bom, parsed.bom);
            assert_eq!(after.line_ending, parsed.line_ending);
            assert_eq!(after.body, text);
            assert_eq!(
                crate::commands::entity_creation::parse_task_title(&written),
                before.task_title
            );
            if previous.is_none() {
                assert_eq!(written, format!("---\n---\n{text}"));
                assert!(matches!(result, EditOutcome::Wrote { backup: None, .. }));
            }
        }
    }

    fn assert_set_body_case(previous: Option<&str>, text: &str, with_status: bool) {
        let fixture = FixtureRoot::new("task-set-body-matrix");
        let root = fixture.path();
        let task_path = root.join("TASK.md");
        if let Some(previous) = previous {
            std::fs::write(&task_path, previous).unwrap();
        }
        if with_status {
            issue_2837_append(root, "legacy:0", "Tickets/FUP/continuation 🦀");
        }
        let status_bytes = std::fs::read(root.join(STATUS_NAME)).ok();
        let before = read_snapshot(root).unwrap();
        assert_set_body_write(root, previous, text, &before);
        let after = read_snapshot(root).unwrap();
        assert_eq!(after.description, text);
        assert_eq!(after.task_title, before.task_title);
        assert_eq!(after.status, before.status);
        assert_eq!(after.revision, before.revision);
        assert_eq!(
            serde_json::to_value(after.status_record).unwrap(),
            serde_json::to_value(before.status_record).unwrap()
        );
        assert_eq!(std::fs::read(root.join(STATUS_NAME)).ok(), status_bytes);
        if !with_status {
            assert_eq!(after.revision, "legacy:0");
        }
        let current_bytes = std::fs::read(&task_path).ok();
        let count = set_body_backups(root).len();
        assert!(matches!(
            set_body(root, text).unwrap(),
            EditOutcome::NoOp { .. }
        ));
        assert_eq!(std::fs::read(&task_path).ok(), current_bytes);
        assert_eq!(set_body_backups(root).len(), count);
        set_body(root, "").unwrap();
        let cleared = read_snapshot(root).unwrap();
        assert_eq!(cleared.description, "");
        assert_eq!(cleared.task_title, after.task_title);
        assert_eq!(cleared.revision, after.revision);
        assert_eq!(std::fs::read(root.join(STATUS_NAME)).ok(), status_bytes);
        let clear_bytes = std::fs::read(&task_path).ok();
        let count = set_body_backups(root).len();
        assert!(matches!(
            set_body(root, "").unwrap(),
            EditOutcome::NoOp { .. }
        ));
        assert_eq!(std::fs::read(&task_path).ok(), clear_bytes);
        assert_eq!(set_body_backups(root).len(), count);
        set_body_no_litter(root);
    }

    #[test]
    fn set_body_literal_representation_title_status_matrix() {
        let _guard = SetBodyIoGuard::new(None);
        let states = [
            None,
            Some(""),
            Some("\u{feff}"),
            Some("# Legacy heading\r\nold body\r\n"),
            Some("---\r\ntitle: 'unclosed'\r\nold"),
            Some("---\nextra: kept\n---\nold"),
            Some("---\ntitle: 'Real'\nextra: kept\n---\nold"),
            Some("\u{feff}---\r\ntitle: 'USER: Real'\r\nextra: kept\r\n---\r\nold"),
        ];
        let payloads = [
            "",
            "---\ntitle: 'USER: forged'\n---\nliteral",
            "\u{feff}literal",
            "\u{feff}---\ntitle: 'USER: forged'\n---\nliteral",
            " \t🦀\nline\r\nend\r\t\n\n",
        ];
        for with_status in [false, true] {
            for previous in states {
                for text in payloads {
                    assert_set_body_case(previous, text, with_status);
                }
            }
        }
    }

    #[test]
    fn set_body_noop_preserves_legacy_bom_crlf_bytes() {
        let _guard = SetBodyIoGuard::new(Some("nonclean_tmp_write"));
        for original in [
            "# Heading\r\nbody\r\n",
            "\u{feff}old\r\n",
            "--- \r\ntitle: \"Real\"\r\n--- \r\nold\r\n",
            "---\ntitle: unclosed\n",
        ] {
            let fixture = FixtureRoot::new("task-set-body-noop");
            let root = fixture.path();
            std::fs::write(root.join("TASK.md"), original).unwrap();
            let body = parse_task(original).body;
            issue_2837_fault(Some("nonclean_tmp_write"));
            assert!(matches!(
                set_body(root, &body).unwrap(),
                EditOutcome::NoOp { .. }
            ));
            assert!(issue_2837_calls().is_empty());
            assert_eq!(
                std::fs::read(root.join("TASK.md")).unwrap(),
                original.as_bytes()
            );
            assert!(set_body_backups(root).is_empty());
            assert!(!root.join(STATUS_NAME).exists());
            set_body_no_litter(root);
        }
    }
    fn set_body_failure_fixture() -> FixtureRoot {
        let fixture = FixtureRoot::new("task-set-body-failure");
        std::fs::write(
            fixture.path().join("TASK.md"),
            "---\ntitle: 'USER: Keep'\n---\nold body",
        )
        .unwrap();
        issue_2837_append(fixture.path(), "legacy:0", "keep status");
        fixture
    }
    #[test]
    fn set_body_tmp_write_failure_reaches_nonclean_cleanup() {
        let fixture = set_body_failure_fixture();
        let root = fixture.path();
        let original = std::fs::read(root.join("TASK.md")).unwrap();
        let status = issue_2837_log(root);
        let _guard = SetBodyIoGuard::new(Some("nonclean_tmp_write"));
        match set_body(root, "replacement").unwrap_err() {
            TaskOpError::TmpWriteFailed(path, _) => assert_eq!(
                path,
                root.join(format!("TASK.md.tmp.{}", std::process::id()))
            ),
            error => panic!("unexpected error: {error:?}"),
        }
        assert_eq!(issue_2837_calls(), ["nonclean_tmp_write"]);
        assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), original);
        assert_eq!(issue_2837_log(root), status);
        let backups = set_body_backups(root);
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), original);
        set_body_no_litter(root);
    }
    #[test]
    fn set_body_external_edit_reaches_real_nonclean_sentinel() {
        let fixture = set_body_failure_fixture();
        let root = fixture.path();
        let original = std::fs::read(root.join("TASK.md")).unwrap();
        let status = issue_2837_log(root);
        let external = b"External editor data of a deliberately different length";
        assert_ne!(external.len(), original.len());
        let _guard = SetBodyIoGuard::new(None);
        IO_MUTATION.with(|v| {
            *v.borrow_mut() = Some((
                "nonclean_before_recheck".into(),
                root.join("TASK.md"),
                external.to_vec(),
            ))
        });
        let backup = match set_body(root, "replacement").unwrap_err() {
            TaskOpError::ExternalWrite(path) => path,
            error => panic!("unexpected error: {error:?}"),
        };
        assert_eq!(
            issue_2837_calls(),
            ["nonclean_tmp_write", "nonclean_before_recheck"]
        );
        assert!(IO_MUTATION.with(|v| v.borrow().is_none()));
        assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), external);
        assert_eq!(issue_2837_log(root), status);
        assert_eq!(
            set_body_backups(root).as_slice(),
            std::slice::from_ref(&backup)
        );
        assert_eq!(std::fs::read(backup).unwrap(), original);
        set_body_no_litter(root);
    }
    #[test]
    fn set_body_rename_failure_reaches_nonclean_cleanup() {
        let fixture = set_body_failure_fixture();
        let root = fixture.path();
        let original = std::fs::read(root.join("TASK.md")).unwrap();
        let status = issue_2837_log(root);
        let _guard = SetBodyIoGuard::new(Some("nonclean_rename"));
        let backup = match set_body(root, "replacement").unwrap_err() {
            TaskOpError::RenameFailed(_, Some(path)) => path,
            error => panic!("unexpected error: {error:?}"),
        };
        assert_eq!(
            issue_2837_calls(),
            [
                "nonclean_tmp_write",
                "nonclean_before_recheck",
                "nonclean_rename"
            ]
        );
        assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), original);
        assert_eq!(issue_2837_log(root), status);
        assert_eq!(
            set_body_backups(root).as_slice(),
            std::slice::from_ref(&backup)
        );
        assert_eq!(std::fs::read(backup).unwrap(), original);
        set_body_no_litter(root);
    }
    #[test]
    fn set_body_positive_control_visits_all_nonclean_boundaries() {
        let fixture = set_body_failure_fixture();
        let root = fixture.path();
        let original = std::fs::read(root.join("TASK.md")).unwrap();
        let status = issue_2837_log(root);
        let _guard = SetBodyIoGuard::new(None);
        let text = "\u{feff}---\ntitle: 'USER: forged'\n---\n 🦀\r\t\n";
        let backup = match set_body(root, text).unwrap() {
            EditOutcome::Wrote {
                backup: Some(path), ..
            } => path,
            result => panic!("unexpected outcome: {result:?}"),
        };
        assert_eq!(
            issue_2837_calls(),
            [
                "nonclean_tmp_write",
                "nonclean_before_recheck",
                "nonclean_rename"
            ]
        );
        assert_eq!(std::fs::read(backup).unwrap(), original);
        assert_eq!(issue_2837_log(root), status);
        assert_eq!(
            parse_task(&std::fs::read_to_string(root.join("TASK.md")).unwrap()).body,
            text
        );
        set_body_no_litter(root);
    }

    #[test]
    fn set_body_real_lock_timeout_then_success() {
        let fixture = set_body_failure_fixture();
        let root = fixture.path();
        let original = std::fs::read(root.join("TASK.md")).unwrap();
        let status = issue_2837_log(root);
        let held = LockGuard::acquire(
            &root.join("TASK.md.lock"),
            LOCK_TIMEOUT_5S,
            LOCK_STALE_AFTER_5M,
        )
        .unwrap();
        let contender_root = root.to_path_buf();
        let contender = thread::spawn(move || {
            let _guard = SetBodyIoGuard::new(None);
            let result = set_body(&contender_root, "replacement");
            assert!(issue_2837_calls().is_empty());
            result
        });
        // Keep the kernel lock alive through join: no release/sleep race.
        assert!(matches!(
            contender.join().unwrap(),
            Err(TaskOpError::LockTimeout)
        ));
        assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), original);
        assert_eq!(issue_2837_log(root), status);
        assert!(set_body_backups(root).is_empty());
        set_body_no_litter(root);
        drop(held);
        let _guard = SetBodyIoGuard::new(None);
        assert!(matches!(
            set_body(root, "replacement").unwrap(),
            EditOutcome::Wrote { .. }
        ));
        assert_eq!(read_snapshot(root).unwrap().description, "replacement");
        assert_eq!(issue_2837_log(root), status);
    }
    #[test]
    fn set_body_recovers_pending_clean_before_preserving_baseline() {
        let fixture = set_body_failure_fixture();
        let root = fixture.path();
        let _guard = SetBodyIoGuard::new(Some("after_journal_publish"));
        assert!(issue_2837_clean(root).is_err());
        assert!(root.join(JOURNAL_NAME).exists());
        issue_2837_fault(None);
        let journal: CleanJournal =
            serde_json::from_slice(&std::fs::read(root.join(JOURNAL_NAME)).unwrap()).unwrap();
        // Read staged targets before SetBody performs actual pending recovery.
        let clean_task = std::fs::read(root.join(&journal.task.stage)).unwrap();
        let clean_status = std::fs::read(root.join(&journal.status.stage)).unwrap();
        let backup = match set_body(root, "after recovery 🦀\n").unwrap() {
            EditOutcome::Wrote {
                backup: Some(path), ..
            } => path,
            result => panic!("unexpected outcome: {result:?}"),
        };
        assert_eq!(std::fs::read(backup).unwrap(), clean_task);
        assert_eq!(issue_2837_log(root), clean_status);
        let snapshot = read_snapshot(root).unwrap();
        assert_eq!(snapshot.task_title.as_deref(), Some("Clean"));
        assert_eq!(snapshot.description, "after recovery 🦀\n");
        assert!(snapshot.status.is_none());
        assert!(!snapshot.revision.starts_with("legacy:"));
        set_body_no_litter(root);
    }

    fn issue_2837_fixture() -> FixtureRoot {
        let f = FixtureRoot::new("issue-2837");
        std::fs::create_dir_all(f.path()).unwrap();
        f
    }
    fn issue_2837_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn issue_2837_fault(point: Option<&str>) {
        IO_FAULT.with(|f| *f.borrow_mut() = (point.map(str::to_string), Vec::new()));
    }
    fn issue_2837_calls() -> Vec<String> {
        IO_FAULT.with(|f| f.borrow().1.clone())
    }
    fn issue_2837_append(root: &Path, revision: &str, text: &str) -> StatusReceipt {
        append_status(
            root,
            revision,
            &issue_2837_id(),
            text,
            "project:room-10/agent",
        )
        .unwrap()
    }
    fn issue_2837_log(root: &Path) -> Vec<u8> {
        std::fs::read(root.join(STATUS_NAME)).unwrap()
    }
    fn issue_2837_lines(root: &Path) -> usize {
        issue_2837_log(root).iter().filter(|c| **c == b'\n').count()
    }
    fn issue_2837_clean(root: &Path) -> Result<EditOutcome, TaskOpError> {
        perform_inner(root, TaskOp::Clean, || fixed_now_at(2026, 10, 2, 21, 0, 0))
    }

    #[test]
    fn issue_2837_legacy_missing_bom_crlf_user_description() {
        let f = issue_2837_fixture();
        let root = f.path();
        let s = read_snapshot(root).unwrap();
        assert_eq!(s.revision, "legacy:0");
        assert!(s.task.is_none() && s.status_record.is_none() && !s.tail_incomplete);
        let task = "\u{feff}---\r\ntitle: 'USER: Hold'\r\n---\r\nOne\r\nTwo\r\n";
        std::fs::write(root.join("TASK.md"), task).unwrap();
        let s = read_snapshot(root).unwrap();
        assert_eq!(s.task.as_deref(), Some(task));
        assert_eq!(s.task_title.as_deref(), Some("USER: Hold"));
        assert_eq!(s.description, "One\r\nTwo\r\n");
        assert!(s.status.is_none());
        issue_2837_append(
            root,
            "legacy:0",
            "Tickets: uno\nFUP: ninguno\t🦀\r\nContinue: sí",
        );
        assert_eq!(std::fs::read_to_string(root.join("TASK.md")).unwrap(), task);
        let s = read_snapshot(root).unwrap();
        issue_2837_append(root, &s.revision, "Replacement complete");
        assert_eq!(
            read_snapshot(root).unwrap().status.as_deref(),
            Some("Replacement complete")
        );
        assert_eq!(issue_2837_lines(root), 2);
        let wire = serde_json::to_value(read_snapshot(root).unwrap()).unwrap();
        assert!(wire.get("workgroupRoot").is_some() && wire.get("statusRecord").is_some());
        std::fs::write(root.join("TASK.md"), vec![b'x'; TASK_LIMIT as usize + 1]).unwrap();
        assert!(matches!(
            read_snapshot(root),
            Err(TaskOpError::SnapshotTooLarge)
        ));
    }

    #[test]
    fn issue_2837_retry_mismatch_overtaken_and_cas() {
        let f = issue_2837_fixture();
        let root = f.path();
        let id = issue_2837_id();
        let first = append_status(root, "legacy:0", &id, "one", "p/a").unwrap();
        assert!(!first.replayed);
        let bytes = issue_2837_log(root);
        issue_2837_fault(None);
        assert!(
            append_status(root, "legacy:0", &id, "one", "p/a")
                .unwrap()
                .replayed
        );
        assert_eq!(issue_2837_calls(), vec!["append_sync"]);
        for (base, author, text) in [
            ("legacy:0", "p/b", "one"),
            ("legacy:0", "p/a", "two"),
            (first.revision.as_str(), "p/a", "one"),
        ] {
            assert!(matches!(
                append_status(root, base, &id, text, author),
                Err(TaskOpError::RequestIdConflict)
            ));
        }
        assert_eq!(issue_2837_log(root), bytes);
        let barrier = Arc::new(Barrier::new(2));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let p = root.to_path_buf();
            let b = barrier.clone();
            let revision = first.revision.clone();
            threads.push(thread::spawn(move || {
                b.wait();
                append_status(&p, &revision, &issue_2837_id(), "next", "p/a")
            }));
        }
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(TaskOpError::RevisionConflict { .. })))
                .count(),
            1
        );
        assert!(matches!(
            append_status(root, "legacy:0", &id, "one", "p/a"),
            Err(TaskOpError::RevisionConflict { .. })
        ));
        assert_eq!(issue_2837_lines(root), 2);
        issue_2837_fault(None);
    }

    #[test]
    fn issue_2837_complete_append_failed_sync_retry_requires_fresh_sync() {
        let f = issue_2837_fixture();
        let root = f.path();
        let id = issue_2837_id();
        issue_2837_fault(Some("append_sync"));
        assert!(matches!(
            append_status(root, "legacy:0", &id, "text", "p/a"),
            Err(TaskOpError::WriteFailed(_))
        ));
        assert_eq!(issue_2837_lines(root), 1);
        let bytes = issue_2837_log(root);
        assert_eq!(read_snapshot(root).unwrap().status.as_deref(), Some("text"));
        issue_2837_fault(Some("append_sync"));
        assert!(matches!(
            append_status(root, "legacy:0", &id, "text", "p/a"),
            Err(TaskOpError::WriteFailed(_))
        ));
        assert_eq!(issue_2837_calls(), vec!["append_sync"]);
        assert_eq!(issue_2837_log(root), bytes);
        issue_2837_fault(None);
        assert!(
            append_status(root, "legacy:0", &id, "text", "p/a")
                .unwrap()
                .replayed
        );
        assert_eq!(issue_2837_calls(), vec!["append_sync"]);
        assert_eq!(issue_2837_log(root), bytes);
    }

    #[test]
    fn issue_2837_first_partial_twenty_bytes_and_split_utf8_repair() {
        for split_utf8 in [false, true] {
            let f = issue_2837_fixture();
            let root = f.path();
            let id = issue_2837_id();
            if split_utf8 {
                let mut bytes = vec![b'x'; 18];
                bytes.extend_from_slice(&[0xf0, 0x9f]);
                std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
            } else {
                issue_2837_fault(Some("append_partial"));
                assert!(matches!(
                    append_status(root, "legacy:0", &id, "🦀", "p/a"),
                    Err(TaskOpError::WriteFailed(_))
                ));
            }
            let original = issue_2837_log(root);
            assert_eq!(original.len(), 20);
            let s = read_snapshot(root).unwrap();
            assert_eq!(s.revision, "legacy:0");
            assert!(s.status_record.is_none() && s.tail_incomplete);
            for point in [
                "partial_backup_create",
                "partial_backup_write",
                "partial_backup_sync",
            ] {
                issue_2837_fault(Some(point));
                assert!(matches!(
                    append_status(root, "legacy:0", &id, "🦀", "p/a"),
                    Err(TaskOpError::WriteFailed(_))
                ));
                assert_eq!(issue_2837_log(root), original);
            }
            issue_2837_fault(None);
            let receipt = append_status(root, "legacy:0", &id, "🦀", "p/a").unwrap();
            assert_eq!(receipt.record.sequence, 1);
            assert_eq!(issue_2837_lines(root), 1);
            assert!(!read_snapshot(root).unwrap().tail_incomplete);
            assert!(std::fs::read_dir(root).unwrap().flatten().any(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with("TASK-status.partial.")
                && std::fs::read(e.path()).unwrap() == original));
        }
    }

    #[test]
    fn issue_2837_partial_bounds_and_validation_before_repair() {
        let f = issue_2837_fixture();
        let root = f.path();
        let first = issue_2837_append(root, "legacy:0", "valid");
        let complete = issue_2837_log(root);
        let mut bytes = complete.clone();
        bytes.extend(vec![b'x'; 65_535]);
        std::fs::write(root.join(STATUS_NAME), &bytes).unwrap();
        let s = read_snapshot(root).unwrap();
        assert!(s.tail_incomplete);
        assert_eq!(s.revision, first.revision);
        assert!(matches!(
            append_status(root, "legacy:0", &issue_2837_id(), "wrong base", "p/a"),
            Err(TaskOpError::RevisionConflict { .. })
        ));
        assert!(matches!(
            append_status(root, &first.revision, &issue_2837_id(), " \t", "p/a"),
            Err(TaskOpError::InvalidStatus(_))
        ));
        assert_eq!(issue_2837_log(root), bytes);
        issue_2837_append(root, &first.revision, "next");
        assert!(issue_2837_log(root).starts_with(&complete));
        let mut bytes = issue_2837_log(root);
        bytes.extend(vec![b'x'; 65_536]);
        std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
        assert!(matches!(
            read_snapshot(root),
            Err(TaskOpError::InvalidStatus(_))
        ));
        std::fs::write(root.join(STATUS_NAME), vec![0xf0; 65_535]).unwrap();
        assert!(read_snapshot(root).unwrap().tail_incomplete);
        std::fs::write(root.join(STATUS_NAME), vec![0xf0; 65_536]).unwrap();
        assert!(matches!(
            read_snapshot(root),
            Err(TaskOpError::InvalidStatus(_))
        ));
    }

    #[test]
    fn issue_2837_corrupt_complete_schema_shape_and_sequence() {
        let f = issue_2837_fixture();
        let root = f.path();
        let first = issue_2837_append(root, "legacy:0", "original");
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
            let mut value = serde_json::to_value(&first.record).unwrap();
            value.as_object_mut().unwrap().remove(field);
            let mut bytes = serde_json::to_vec(&value).unwrap();
            bytes.push(b'\n');
            std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
            assert!(
                matches!(read_snapshot(root), Err(TaskOpError::InvalidStatus(_))),
                "missing {field}"
            );
        }
        for (field, value) in [
            ("schemaVersion", serde_json::json!(2)),
            ("kind", serde_json::json!("clear")),
            ("topicId", serde_json::json!("bad")),
            ("sequence", serde_json::json!(0)),
            ("sequence", serde_json::json!(MAX_SEQUENCE + 1)),
            ("status", serde_json::json!("\u{7}")),
            ("recordedAt", serde_json::json!("2026-01-01T00:00:00+03:00")),
        ] {
            let mut record = serde_json::to_value(&first.record).unwrap();
            record[field] = value;
            let mut bytes = serde_json::to_vec(&record).unwrap();
            bytes.push(b'\n');
            std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
            assert!(
                matches!(read_snapshot(root), Err(TaskOpError::InvalidStatus(_))),
                "invalid {field}"
            );
        }
        for bytes in [
            b"{broken}\n".to_vec(),
            [vec![b'x'; 65_536], vec![b'\n']].concat(),
        ] {
            std::fs::write(root.join(STATUS_NAME), bytes).unwrap();
            assert!(matches!(
                read_snapshot(root),
                Err(TaskOpError::InvalidStatus(_))
            ));
        }
        let mut record = first.record;
        record.sequence = MAX_SEQUENCE;
        record.base_revision = Some(format!("{}:{}", record.topic_id, MAX_SEQUENCE - 1));
        let mut bytes = serde_json::to_vec(&record).unwrap();
        bytes.push(b'\n');
        std::fs::write(root.join(STATUS_NAME), &bytes).unwrap();
        assert!(matches!(
            append_status(
                root,
                &format!("{}:{}", record.topic_id, MAX_SEQUENCE),
                &issue_2837_id(),
                "overflow",
                "p/a"
            ),
            Err(TaskOpError::SequenceOverflow)
        ));
        assert_eq!(issue_2837_log(root), bytes);
    }

    #[test]
    fn issue_2837_encoded_size_boundary_and_bounded_history_read() {
        let f = issue_2837_fixture();
        let root = f.path();
        let id = issue_2837_id();
        let receipt = append_status(root, "legacy:0", &id, "x", "p/a").unwrap();
        let overhead = issue_2837_log(root).len() - 1;
        // Timestamp formatting is stable for Utc::now(); allow for nanosecond
        // formatter width by explicitly checking encoded rows, not characters.
        let mut r = receipt.record.clone();
        r.status = Some("x".repeat(STATUS_LIMIT - overhead));
        let mut row = serde_json::to_vec(&r).unwrap();
        row.push(b'\n');
        assert_eq!(row.len(), STATUS_LIMIT);
        assert!(validate_record(&row[..row.len() - 1]).is_ok());
        row.insert(row.len() - 2, b'x');
        assert!(matches!(
            validate_record(&row[..row.len() - 1]),
            Err(TaskOpError::InvalidStatus(_))
        ));
        let original = issue_2837_log(root);
        assert!(matches!(
            append_status(
                root,
                &receipt.revision,
                &issue_2837_id(),
                &"\n".repeat(STATUS_LIMIT),
                "p/a"
            ),
            Err(TaskOpError::InvalidStatus(_))
        ));
        assert!(matches!(
            append_status(
                root,
                &receipt.revision,
                &issue_2837_id(),
                &format!("x{}", "\n".repeat(STATUS_LIMIT)),
                "p/a"
            ),
            Err(TaskOpError::StatusTooLarge)
        ));
        assert_eq!(issue_2837_log(root), original);
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(root.join(STATUS_NAME))
            .unwrap();
        for _ in 0..50_000 {
            file.write_all(&original).unwrap();
        }
        drop(file);
        let tail = read_latest_status_locked(root).unwrap();
        assert_eq!(tail.bytes_read as u64, STATUS_WINDOW);
        assert_eq!(tail.record.unwrap(), receipt.record);
    }

    #[test]
    fn issue_2837_clean_pair_matrix_exact_bytes_noop_and_aba() {
        for (task_exists, log_exists) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let f = issue_2837_fixture();
            let root = f.path();
            let task = b"\xef\xbb\xbf---\r\ntitle: 'USER: Original'\r\n---\r\nbody\r\n";
            let log = b"{corrupt complete}\n\xf0\x9f";
            if task_exists {
                std::fs::write(root.join("TASK.md"), task).unwrap();
            }
            if log_exists {
                std::fs::write(root.join(STATUS_NAME), log).unwrap();
            }
            let result = issue_2837_clean(root).unwrap();
            assert!(matches!(result, EditOutcome::Wrote { .. }));
            let (t, s) = backup_names("20261002-210000", 0);
            if task_exists || log_exists {
                assert_eq!(
                    std::fs::read(root.join(t)).unwrap(),
                    if task_exists { task.as_slice() } else { &[] }
                );
                assert_eq!(
                    std::fs::read(root.join(s)).unwrap(),
                    if log_exists { log.as_slice() } else { &[] }
                );
            } else {
                assert!(!root.join(t).exists() && !root.join(s).exists());
            }
            let snapshot = read_snapshot(root).unwrap();
            assert_eq!(snapshot.task_title.as_deref(), Some("Clean"));
            assert_eq!(
                snapshot.status_record.as_ref().unwrap().kind,
                "topic_started"
            );
            assert!(snapshot.revision.ends_with(":0"));
            assert_ne!(snapshot.revision, "legacy:0");
            let bytes = issue_2837_log(root);
            assert!(matches!(
                issue_2837_clean(root),
                Ok(EditOutcome::NoOp { .. })
            ));
            assert_eq!(issue_2837_log(root), bytes);
            assert!(matches!(
                append_status(root, "legacy:0", &issue_2837_id(), "stale", "p/a"),
                Err(TaskOpError::RevisionConflict { .. })
            ));
            issue_2837_append(root, &snapshot.revision, "history after Clean");
            assert!(matches!(
                issue_2837_clean(root),
                Ok(EditOutcome::Wrote { .. })
            ));
            assert_ne!(read_snapshot(root).unwrap().revision, snapshot.revision);
        }
    }

    #[test]
    fn issue_2837_clean_backup_collision_on_either_side() {
        for collision_on_task in [true, false] {
            let f = issue_2837_fixture();
            let root = f.path();
            std::fs::write(root.join("TASK.md"), "old").unwrap();
            let (t, s) = backup_names("20261002-210000", 0);
            let foreign = if collision_on_task {
                t.clone()
            } else {
                s.clone()
            };
            std::fs::write(root.join(&foreign), "foreign").unwrap();
            issue_2837_clean(root).unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join(foreign)).unwrap(),
                "foreign"
            );
            let (t1, s1) = backup_names("20261002-210000", 1);
            assert!(root.join(t1).exists() && root.join(s1).exists());
            if !collision_on_task {
                assert!(!root.join(t).exists());
            }
        }
    }

    #[test]
    fn issue_2837_clean_prejournal_failure_matrix_preserves_originals() {
        for point in [
            "task_backup_create",
            "status_backup_create",
            "task_backup_copy",
            "task_backup_sync",
            "status_backup_copy",
            "status_backup_sync",
            "task_stage_create",
            "task_stage_write",
            "task_stage_sync",
            "status_stage_create",
            "status_stage_write",
            "status_stage_sync",
            "journal_create",
            "journal_write",
            "journal_stage_sync",
            "journal_publish",
        ] {
            let f = issue_2837_fixture();
            let root = f.path();
            std::fs::write(root.join("TASK.md"), "old task").unwrap();
            std::fs::write(root.join(STATUS_NAME), b"old\npartial\xf0").unwrap();
            let old_task = std::fs::read(root.join("TASK.md")).unwrap();
            let old_log = issue_2837_log(root);
            issue_2837_fault(Some(point));
            assert!(issue_2837_clean(root).is_err(), "{point}");
            assert_eq!(
                std::fs::read(root.join("TASK.md")).unwrap(),
                old_task,
                "{point}"
            );
            assert_eq!(issue_2837_log(root), old_log, "{point}");
            assert!(!root.join(JOURNAL_NAME).exists(), "{point}");
            issue_2837_fault(None);
            issue_2837_clean(root).unwrap();
            assert!(read_snapshot(root).unwrap().status.is_none());
        }
    }

    #[test]
    fn issue_2837_clean_postjournal_failure_matrix_and_repeat_recovery() {
        for point in [
            "after_journal_publish",
            "journal_sync",
            "task_rename",
            "after_task_rename",
            "status_rename",
            "after_status_rename",
            "task_target_sync_open",
            "task_target_sync",
            "status_target_sync_open",
            "status_target_sync",
            "journal_remove",
        ] {
            let f = issue_2837_fixture();
            let root = f.path();
            std::fs::write(root.join("TASK.md"), "old task").unwrap();
            std::fs::write(root.join(STATUS_NAME), "old log").unwrap();
            issue_2837_fault(Some(point));
            assert!(
                matches!(
                    issue_2837_clean(root),
                    Err(TaskOpError::CleanRecoveryPending(_))
                ),
                "{point}"
            );
            assert!(root.join(JOURNAL_NAME).exists(), "{point}");
            issue_2837_fault(None);
            recover_clean_pair(root).unwrap();
            let new_task = std::fs::read(root.join("TASK.md")).unwrap();
            let new_log = issue_2837_log(root);
            assert!(!root.join(JOURNAL_NAME).exists());
            let calls = issue_2837_calls();
            let task_sync = calls.iter().position(|c| c == "task_target_sync").unwrap();
            let status_sync = calls
                .iter()
                .position(|c| c == "status_target_sync")
                .unwrap();
            let remove = calls.iter().position(|c| c == "journal_remove").unwrap();
            assert!(task_sync < status_sync && status_sync < remove);
            recover_clean_pair(root).unwrap();
            assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), new_task);
            assert_eq!(issue_2837_log(root), new_log);
        }
    }

    #[test]
    fn issue_2837_already_new_pair_retries_both_syncs_before_cleanup() {
        for fail_sync in ["task_target_sync", "status_target_sync"] {
            let f = issue_2837_fixture();
            let root = f.path();
            std::fs::write(root.join("TASK.md"), "old").unwrap();
            issue_2837_fault(Some("after_status_rename"));
            assert!(matches!(
                issue_2837_clean(root),
                Err(TaskOpError::CleanRecoveryPending(_))
            ));
            let task = std::fs::read(root.join("TASK.md")).unwrap();
            let log = issue_2837_log(root);
            let journal = std::fs::read(root.join(JOURNAL_NAME)).unwrap();
            let backup = std::fs::read(root.join("TASK.20261002-210000.bak.md")).unwrap();
            for _ in 0..2 {
                issue_2837_fault(Some(fail_sync));
                assert!(matches!(
                    read_snapshot(root),
                    Err(TaskOpError::CleanRecoveryPending(_))
                ));
                assert_eq!(std::fs::read(root.join(JOURNAL_NAME)).unwrap(), journal);
                assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), task);
                assert_eq!(issue_2837_log(root), log);
                let calls = issue_2837_calls();
                assert!(!calls
                    .iter()
                    .any(|c| c.contains("rename") || c == "journal_remove"));
                assert_eq!(
                    calls
                        .iter()
                        .filter(|c| c.as_str() == "task_target_sync")
                        .count(),
                    1
                );
                assert_eq!(
                    calls
                        .iter()
                        .filter(|c| c.as_str() == "status_target_sync")
                        .count(),
                    1
                );
            }
            issue_2837_fault(None);
            let s = read_snapshot(root).unwrap();
            assert_eq!(s.status_record.unwrap().sequence, 0);
            assert_eq!(
                issue_2837_calls(),
                vec![
                    "journal_sync",
                    "task_target_sync_open",
                    "task_target_sync",
                    "status_target_sync_open",
                    "status_target_sync",
                    "journal_remove"
                ]
            );
            assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), task);
            assert_eq!(issue_2837_log(root), log);
            assert_eq!(
                std::fs::read(root.join("TASK.20261002-210000.bak.md")).unwrap(),
                backup
            );
            assert!(!root.join(JOURNAL_NAME).exists());
        }
    }

    #[test]
    fn issue_2837_recovery_external_edits_missing_evidence_invalid_journal() {
        for mode in ["task", "status", "backup", "stage", "journal"] {
            let f = issue_2837_fixture();
            let root = f.path();
            std::fs::write(root.join("TASK.md"), "old").unwrap();
            issue_2837_fault(Some("task_rename"));
            assert!(issue_2837_clean(root).is_err());
            issue_2837_fault(None);
            let journal: CleanJournal =
                serde_json::from_slice(&std::fs::read(root.join(JOURNAL_NAME)).unwrap()).unwrap();
            match mode {
                "task" => std::fs::write(root.join("TASK.md"), "external").unwrap(),
                "status" => std::fs::write(root.join(STATUS_NAME), "external").unwrap(),
                "backup" => std::fs::remove_file(root.join(journal.task.backup.unwrap())).unwrap(),
                "stage" => std::fs::remove_file(root.join(journal.task.stage)).unwrap(),
                "journal" => {
                    let mut value = serde_json::to_value(journal).unwrap();
                    value["task"]["stage"] = serde_json::json!("../foreign");
                    std::fs::write(root.join(JOURNAL_NAME), serde_json::to_vec(&value).unwrap())
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let task = std::fs::read(root.join("TASK.md")).unwrap();
            let log = std::fs::read(root.join(STATUS_NAME)).ok();
            assert!(
                matches!(
                    read_snapshot(root),
                    Err(TaskOpError::CleanRecoveryConflict(_))
                ),
                "{mode}"
            );
            assert!(root.join(JOURNAL_NAME).exists());
            assert_eq!(std::fs::read(root.join("TASK.md")).unwrap(), task);
            assert_eq!(std::fs::read(root.join(STATUS_NAME)).ok(), log);
        }
    }

    #[test]
    fn issue_2837_append_title_body_clean_concurrency() {
        let f = issue_2837_fixture();
        let root = f.path();
        let first = issue_2837_append(root, "legacy:0", "status");
        let original = issue_2837_log(root);
        perform(root, TaskOp::SetTitle("title".into())).unwrap();
        perform(root, TaskOp::AppendBody("description".into())).unwrap();
        assert_eq!(issue_2837_log(root), original);
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for op in 0..3 {
            let p = root.to_path_buf();
            let b = barrier.clone();
            let revision = first.revision.clone();
            handles.push(thread::spawn(move || {
                b.wait();
                match op {
                    0 => append_status(&p, &revision, &issue_2837_id(), "new status", "p/a")
                        .map(|_| ()),
                    1 => perform(&p, TaskOp::AppendBody("new description".into())).map(|_| ()),
                    _ => perform(&p, TaskOp::Clean).map(|_| ()),
                }
            }));
        }
        for (i, h) in handles.into_iter().enumerate() {
            let r = h.join().unwrap();
            assert!(r.is_ok() || i == 0 && matches!(r, Err(TaskOpError::RevisionConflict { .. })));
        }
        let s = read_snapshot(root).unwrap();
        assert_eq!(s.task_title.as_deref(), Some("Clean"));
        assert_eq!(s.status_record.unwrap().kind, "topic_started");
    }

    #[test]
    fn issue_2837_child_fixture() {
        let Some(root) = std::env::var_os("AC_TASK_OPS_CHILD_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root);
        if std::env::var_os("AC_TASK_OPS_CHILD_LOCK").is_some() {
            let _held = LockGuard::acquire(
                &root.join("TASK.md.lock"),
                LOCK_TIMEOUT_5S,
                LOCK_STALE_AFTER_5M,
            )
            .unwrap();
            std::fs::write(root.join("child-ready"), "ready").unwrap();
            std::thread::sleep(Duration::from_secs(30));
        } else {
            let _ = issue_2837_clean(&root);
        }
    }

    fn issue_2837_child(root: &Path) -> std::process::Command {
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "cli::task_ops::tests::issue_2837_child_fixture",
            "--nocapture",
        ])
        .env("AC_TASK_OPS_CHILD_FIXTURE", root);
        cmd
    }

    #[test]
    fn issue_2837_kernel_lock_child_exit_and_stable_file() {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md.lock"), "old stale contents").unwrap();
        let mut child = issue_2837_child(root)
            .env("AC_TASK_OPS_CHILD_LOCK", "1")
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.join("child-ready").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        if !root.join("child-ready").exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child failed to acquire lock");
        }
        assert!(matches!(
            LockGuard::acquire(
                &root.join("TASK.md.lock"),
                Duration::from_millis(100),
                Duration::ZERO
            ),
            Err(TaskOpError::LockTimeout)
        ));
        child.kill().unwrap();
        child.wait().unwrap();
        let held = LockGuard::acquire(
            &root.join("TASK.md.lock"),
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .unwrap();
        drop(held);
        assert_eq!(
            std::fs::read_to_string(root.join("TASK.md.lock")).unwrap(),
            "old stale contents"
        );
        assert!(root.join("TASK.md.lock").exists());
        std::fs::create_dir(root.join("directory-lock")).unwrap();
        assert!(matches!(
            LockGuard::acquire(&root.join("directory-lock"), Duration::ZERO, Duration::ZERO),
            Err(TaskOpError::LockIo(..))
        ));
    }

    #[test]
    fn issue_2837_process_exit_publication_boundaries_recover_pair() {
        for point in [
            "after_journal_publish",
            "after_task_rename",
            "after_status_rename",
            "task_target_sync",
            "status_target_sync",
            "journal_remove",
        ] {
            let f = issue_2837_fixture();
            let root = f.path();
            std::fs::write(root.join("TASK.md"), "prior task").unwrap();
            std::fs::write(root.join(STATUS_NAME), "prior log\npartial").unwrap();
            let status = issue_2837_child(root)
                .env("AC_TASK_OPS_CHILD_BOUNDARY", point)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(71), "{point}");
            assert!(root.join(JOURNAL_NAME).exists());
            let s = read_snapshot(root).unwrap();
            assert_eq!(s.task_title.as_deref(), Some("Clean"));
            assert_eq!(s.status_record.unwrap().kind, "topic_started");
            assert!(!root.join(JOURNAL_NAME).exists());
            assert_eq!(
                std::fs::read_to_string(root.join("TASK.20261002-210000.bak.md")).unwrap(),
                "prior task"
            );
            assert_eq!(
                std::fs::read_to_string(root.join("TASK-status.20261002-210000.bak.jsonl"))
                    .unwrap(),
                "prior log\npartial"
            );
        }
    }

    #[test]
    fn issue_2837_unknown_fields_and_preceding_corrupt_row_partial() {
        let f = issue_2837_fixture();
        let root = f.path();
        let r = issue_2837_append(root, "legacy:0", "text");
        let mut value = serde_json::to_value(&r.record).unwrap();
        value["futureV1Extension"] = serde_json::json!({"ok":true});
        let mut row = serde_json::to_vec(&value).unwrap();
        row.push(b'\n');
        std::fs::write(root.join(STATUS_NAME), &row).unwrap();
        assert_eq!(read_snapshot(root).unwrap().status.as_deref(), Some("text"));
        let mut bytes = row;
        bytes.extend_from_slice(b"{broken}\n\xf0\x9f");
        std::fs::write(root.join(STATUS_NAME), &bytes).unwrap();
        assert!(matches!(
            read_snapshot(root),
            Err(TaskOpError::InvalidStatus(_))
        ));
        assert!(append_status(root, &r.revision, &issue_2837_id(), "next", "p/a").is_err());
        assert_eq!(issue_2837_log(root), bytes);
    }

    #[test]
    fn issue_2837_recovery_preserves_foreign_unused_stage() {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "old").unwrap();
        issue_2837_fault(Some("after_status_rename"));
        assert!(issue_2837_clean(root).is_err());
        issue_2837_fault(None);
        let journal: CleanJournal =
            serde_json::from_slice(&std::fs::read(root.join(JOURNAL_NAME)).unwrap()).unwrap();
        let foreign = root.join(&journal.task.stage);
        std::fs::write(&foreign, "foreign replacement of unused stage").unwrap();
        recover_clean_pair(root).unwrap();
        assert_eq!(
            std::fs::read_to_string(foreign).unwrap(),
            "foreign replacement of unused stage"
        );
        assert!(!root.join(JOURNAL_NAME).exists());
    }

    #[test]
    fn issue_2837_clean_external_change_before_journal_keeps_pair() {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "old").unwrap();
        std::fs::write(root.join(STATUS_NAME), "old log").unwrap();
        IO_MUTATION.with(|v| {
            *v.borrow_mut() = Some((
                "before_source_recheck".into(),
                root.join(STATUS_NAME),
                b"external history".to_vec(),
            ))
        });
        assert!(matches!(
            issue_2837_clean(root),
            Err(TaskOpError::CleanRecoveryConflict(_))
        ));
        assert_eq!(
            std::fs::read_to_string(root.join("TASK.md")).unwrap(),
            "old"
        );
        assert_eq!(issue_2837_log(root), b"external history");
        assert!(!root.join(JOURNAL_NAME).exists());
    }

    #[test]
    fn issue_2837_clean_external_change_immediately_before_rename() {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "old").unwrap();
        IO_MUTATION.with(|v| {
            *v.borrow_mut() = Some((
                "task_rename".into(),
                root.join("TASK.md"),
                b"external task".to_vec(),
            ))
        });
        assert!(matches!(
            issue_2837_clean(root),
            Err(TaskOpError::CleanRecoveryConflict(_))
        ));
        assert_eq!(
            std::fs::read_to_string(root.join("TASK.md")).unwrap(),
            "external task"
        );
        assert!(root.join(JOURNAL_NAME).exists());
        assert!(!root.join(STATUS_NAME).exists());
    }
    #[test]
    fn issue_2837_append_exact_encoded_limit_and_one_byte_over() {
        let first = issue_2837_fixture();
        let root = first.path();
        issue_2837_append(root, "legacy:0", "x");
        let overhead = issue_2837_log(root).len() - 1;
        let exact = issue_2837_fixture();
        let root = exact.path();
        let text = "x".repeat(STATUS_LIMIT - overhead);
        issue_2837_append(root, "legacy:0", &text);
        assert_eq!(issue_2837_log(root).len(), STATUS_LIMIT);
        let over = issue_2837_fixture();
        let root = over.path();
        assert!(matches!(
            append_status(
                root,
                "legacy:0",
                &issue_2837_id(),
                &format!("{text}x"),
                "project:room-10/agent"
            ),
            Err(TaskOpError::StatusTooLarge)
        ));
        assert!(!root.join(STATUS_NAME).exists());
    }

    #[test]
    fn issue_2837_backup_partial_failure_cleanup_and_finished_empty_side() {
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join("TASK.md"), "prior description").unwrap();
        let original = vec![b'x'; 200_000];
        std::fs::write(root.join(STATUS_NAME), &original).unwrap();
        issue_2837_fault(Some("status_backup_after_write"));
        assert!(matches!(
            issue_2837_clean(root),
            Err(TaskOpError::WriteFailed(_))
        ));
        issue_2837_fault(None);
        assert_eq!(issue_2837_log(root), original);
        assert_eq!(
            std::fs::read_to_string(root.join("TASK.20261002-210000.bak.md")).unwrap(),
            "prior description"
        );
        assert!(!root.join("TASK-status.20261002-210000.bak.jsonl").exists());
        let f = issue_2837_fixture();
        let root = f.path();
        std::fs::write(root.join(STATUS_NAME), "prior log").unwrap();
        issue_2837_fault(Some("status_backup_sync"));
        assert!(matches!(
            issue_2837_clean(root),
            Err(TaskOpError::WriteFailed(_))
        ));
        issue_2837_fault(None);
        assert_eq!(
            std::fs::read(root.join("TASK.20261002-210000.bak.md")).unwrap(),
            Vec::<u8>::new()
        );
        assert!(!root.join("TASK-status.20261002-210000.bak.jsonl").exists());
        assert!(!root.join("TASK.md").exists());
        assert_eq!(issue_2837_log(root), b"prior log");
    }
    fn issue_2837_assert_backup_fault_calls(point: &str) {
        let calls = issue_2837_calls();
        assert_eq!(
            calls.iter().filter(|c| c.as_str() == point).count(),
            1,
            "must execute requested boundary"
        );
        if point == "task_backup_after_write" {
            assert_eq!(
                calls
                    .iter()
                    .filter(|c| c.as_str() == "task_backup_write")
                    .count(),
                1,
                "fail immediately after the first block"
            );
            assert!(!calls.iter().any(|c| c == "task_backup_sync"));
        }
        if point == "task_backup_write" {
            assert!(!calls.iter().any(|c| c == "task_backup_after_write"));
        }
        if point == "status_backup_write" {
            assert!(calls.iter().any(|c| c == "task_backup_sync"));
            assert!(!calls.iter().any(|c| c == "status_backup_after_write"));
        }
    }

    fn issue_2837_assert_failed_backup_pair(
        root: &Path,
        original_task: &[u8],
        original_log: &[u8],
        task_backup_completed: bool,
    ) {
        assert_eq!(
            std::fs::read(root.join("TASK.md")).unwrap(),
            original_task,
            "original TASK unchanged"
        );
        assert_eq!(issue_2837_log(root), original_log, "original log unchanged");
        assert!(
            !root.join(JOURNAL_NAME).exists(),
            "no reset transaction published"
        );
        let (task_backup, status_backup) = backup_names("20261002-210000", 0);
        assert!(
            !root.join(&status_backup).exists(),
            "own empty log reservation removed"
        );
        if task_backup_completed {
            assert_eq!(
                std::fs::read(root.join(&task_backup)).unwrap(),
                original_task,
                "completed TASK backup preserved"
            );
        } else {
            assert!(
                !root.join(&task_backup).exists(),
                "own empty/partial TASK reservation removed"
            );
        }
        let backups = std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".bak."))
            .count();
        assert_eq!(backups, usize::from(task_backup_completed));
        assert!(!std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains(".tmp.")));
    }

    fn issue_2837_backup_write_failure_case(point: &str, task_backup_completed: bool) {
        let fixture = issue_2837_fixture();
        let root = fixture.path();
        let original_task = format!(
            "\u{feff}---\r\ntitle: 'USER: Keep original'\r\n---\r\n{}\r\n",
            "original description 🦀\r\n".repeat(6_000)
        )
        .into_bytes();
        assert!(
            original_task.len() > 65_536,
            "TASK must span multiple copy blocks"
        );
        let original_log = b"{old completed row}\npartial\xf0\x9f";
        std::fs::write(root.join("TASK.md"), &original_task).unwrap();
        std::fs::write(root.join(STATUS_NAME), original_log).unwrap();
        issue_2837_fault(Some(point));
        assert!(
            matches!(issue_2837_clean(root), Err(TaskOpError::WriteFailed(_))),
            "{point}"
        );
        issue_2837_assert_backup_fault_calls(point);
        issue_2837_fault(None);
        issue_2837_assert_failed_backup_pair(
            root,
            &original_task,
            original_log,
            task_backup_completed,
        );
        let (task_backup, _) = backup_names("20261002-210000", 0);
        assert!(
            matches!(issue_2837_clean(root), Ok(EditOutcome::Wrote { .. })),
            "retry succeeds"
        );
        let suffix = u32::from(task_backup_completed);
        let (retry_task_backup, retry_status_backup) = backup_names("20261002-210000", suffix);
        assert_eq!(
            std::fs::read(root.join(retry_task_backup)).unwrap(),
            original_task,
            "retry archives exact TASK"
        );
        assert_eq!(
            std::fs::read(root.join(retry_status_backup)).unwrap(),
            original_log,
            "retry archives exact log including partial UTF-8"
        );
        if task_backup_completed {
            assert_eq!(
                std::fs::read(root.join(task_backup)).unwrap(),
                original_task,
                "retry cannot overwrite finished archive"
            );
        }
        let snapshot = read_snapshot(root).unwrap();
        assert_eq!(snapshot.task_title.as_deref(), Some("Clean"));
        assert_eq!(snapshot.description, "");
        let seed = snapshot.status_record.unwrap();
        assert_eq!(seed.kind, "topic_started");
        assert_eq!(seed.sequence, 0);
        assert_eq!(issue_2837_lines(root), 1);
        assert!(!root.join(JOURNAL_NAME).exists());
    }

    #[test]
    fn issue_2837_task_backup_write_failure_preserves_originals_and_retries() {
        issue_2837_backup_write_failure_case("task_backup_write", false);
    }

    #[test]
    fn issue_2837_status_backup_write_failure_preserves_finished_task_and_retries() {
        issue_2837_backup_write_failure_case("status_backup_write", true);
    }

    #[test]
    fn issue_2837_task_backup_after_write_cleans_first_partial_block_and_retries() {
        issue_2837_backup_write_failure_case("task_backup_after_write", false);
    }
}
