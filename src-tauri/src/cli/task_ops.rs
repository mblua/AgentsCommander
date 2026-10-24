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
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(test)]
use chrono::DateTime;
use chrono::Utc;
use std::io::Write as _;

mod clean;
mod flow;
mod status;

#[cfg(test)]
use clean::recover_clean_pair;
use clean::recover_clean_pair_locked;
#[cfg(test)]
use clean::{backup_names, CleanJournal};
use flow::perform_inner;
pub use status::StatusReceipt;
pub use status::StatusRecord;
pub use status::TaskSnapshot;
use status::{
    io_boundary, read_latest_status_locked, repair_partial_locked, status_invalid, sync_file,
    valid_author, valid_revision, validate_status_text, write_failed, MAX_SEQUENCE, STATUS_LIMIT,
    STATUS_NAME, TASK_LIMIT,
};
#[cfg(test)]
use status::{validate_record, IO_FAULT, IO_MUTATION, JOURNAL_NAME, STATUS_WINDOW};

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
    /// file's existing BOM and frontmatter line ending; body has no bytes. NoOp when the
    /// file is already in canonical Clean form.
    Clean,
}

/// Outcome of a successful [`perform`] call. The CLI translates this into the
/// verb-specific stdout line.
#[derive(Debug, Clone)]
pub enum EditOutcome {
    /// File was written. `backup` is `None` when the file did not exist before.
    Wrote {
        backup: Option<PathBuf>,
        #[cfg(test)]
        content: String,
    },
    /// Set-title found the existing value already matched; no write performed.
    NoOp {},
    /// Coordinator set-title rejected because the current title is user-owned
    /// (`USER:`). NOT an error: exit code 0, no write, no backup. The GUI/human
    /// path (`SetUserTitle`) never produces this.
    RejectedUserTitle {},
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
    #[error(
        "failed to acquire TASK.md lock at {}: {}. Aborting; TASK.md left unchanged.",
        .0.display(),
        .1
    )]
    LockIo(PathBuf, std::io::Error),
    #[error("failed to read TASK.md at {}: {}", .0.display(), .1)]
    ReadFailed(PathBuf, std::io::Error),
    #[error(
        "failed to write backup at {}: {}. Aborting; TASK.md left unchanged.",
        .0.display(),
        .1
    )]
    BackupFailed(PathBuf, std::io::Error),
    #[error(
        "failed to write backup at {}: 100 collision retries exhausted in the same second. \
         Aborting; TASK.md left unchanged.",
        .0.display()
    )]
    BackupExhausted(PathBuf),
    #[error("failed to write {}: {}. Aborting; TASK.md left unchanged.", .0.display(), .1)]
    TmpWriteFailed(PathBuf, std::io::Error),
    #[error(
        "TASK.md was modified externally between read and write; aborting. \
         Backup at {} retains the externally-modified state.",
        .0.display()
    )]
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
    /// Everything after the closing `---<eol>` (or whole post-BOM input when
    /// `has_frontmatter == false`).
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

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
