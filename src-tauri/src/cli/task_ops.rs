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

/// Stale-lock window. After this elapsed since the lockfile's mtime, the next
/// caller treats the lock as abandoned and removes it (HIGH-2 in plan #137).
/// Production value 5 minutes; tests pass a smaller value to `LockGuard::acquire`.
const LOCK_STALE_AFTER_5M: Duration = Duration::from_secs(300);

// ── Public surface ──────────────────────────────────────────────────────────

/// Edit operation requested by a verb.
#[derive(Debug, Clone)]
pub enum TaskOp {
    /// Replace or insert the YAML-frontmatter `title:` field.
    SetTitle(String),
    /// Append a body paragraph (frontmatter untouched).
    AppendBody(String),
    /// Replace BOTH frontmatter title AND body with the canonical Clean form
    /// (title: 'Clean', body: "Ready to start a new topic\n"). Preserves the
    /// file's existing BOM and frontmatter line ending; body is always LF
    /// canonical. NoOp when the file is already in canonical Clean form.
    Clean,
}

/// Outcome of a successful [`perform`] call. The CLI translates this into the
/// verb-specific stdout line.
#[derive(Debug, Clone)]
pub enum EditOutcome {
    /// File was written. `backup` is `None` when the file did not exist before.
    Wrote { backup: Option<PathBuf> },
    /// Set-title found the existing value already matched; no write performed.
    NoOp,
}

/// Errors emitted by [`perform`]. `Display` impls match the §3 error matrix
/// of plan #137 verbatim.
#[derive(Debug, thiserror::Error)]
pub enum TaskOpError {
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
        TaskOp::AppendBody(text) => apply_append_body(parsed, text),
        TaskOp::Clean => apply_clean(parsed),
    }
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

    // Has frontmatter — find existing title-shaped line(s) (NIT-5).
    let title_count = parsed
        .frontmatter
        .iter()
        .filter(|line| line.trim_start().starts_with("title:"))
        .count();
    if title_count > 1 {
        log::warn!(
            "TASK.md frontmatter contains {} title: lines; replacing the first only — \
             downstream YAML parsers may pick a different one",
            title_count
        );
    }

    let mut new_fm: Vec<String> = parsed.frontmatter.clone();
    let title_idx = new_fm
        .iter()
        .position(|line| line.trim_start().starts_with("title:"));

    match title_idx {
        Some(idx) => {
            // Replace, preserving leading whitespace (so an indented `  title: x`
            // becomes `  title: 'NewTitle'`).
            let original = &new_fm[idx];
            let leading_len = original.len() - original.trim_start().len();
            let leading = &original[..leading_len];
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
/// the body is always LF canonical (`"Ready to start a new topic\n"`). For
/// an empty input (`parse_task("")`), `parsed.bom == false` and
/// `parsed.line_ending == "\n"`, so the output is the canonical LF/no-BOM
/// Clean form — no special case needed.
fn apply_clean(parsed: &ParsedTask) -> ParsedTask {
    ParsedTask {
        bom: parsed.bom,
        line_ending: parsed.line_ending,
        has_frontmatter: true,
        frontmatter: vec!["title: 'Clean'".to_string()],
        body: "Ready to start a new topic\n".to_string(),
    }
}

/// Extract the YAML-decoded value of the first `title:` line in the
/// frontmatter, for the semantic idempotence short-circuit (MED-3). Returns
/// `None` when the frontmatter has no title-shaped line.
pub(crate) fn title_value_of(parsed: &ParsedTask) -> Option<String> {
    parsed
        .frontmatter
        .iter()
        .find(|line| line.trim_start().starts_with("title:"))
        .map(|line| {
            let after_prefix = line
                .trim_start()
                .strip_prefix("title:")
                .unwrap_or("")
                .trim();
            extract_yaml_single_quoted(after_prefix)
        })
}

/// Decode a YAML scalar from the canonical single-quoted form `'value with '' escapes'`.
/// For non-canonical inputs (bare scalar, double-quoted, etc.) returns the raw
/// trimmed input. Sufficient for "did the user-visible title change?" — the
/// conservative direction (returning false → write a new backup) is harmless;
/// the unsafe direction (returning true → skip a real edit) is impossible because
/// the parsed-after-edit form is always canonical single-quoted.
fn extract_yaml_single_quoted(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        let inner = &s[1..s.len() - 1];
        return inner.replace("''", "'");
    }
    s.to_string()
}

// ── Lock guard ──────────────────────────────────────────────────────────────

/// Cooperative file-lock via `OpenOptions::create_new` (kernel-level mutex —
/// `O_CREAT | O_EXCL` on Unix, `CREATE_NEW` on Windows). On `Drop` removes the
/// lockfile best-effort. NOT mandatory — does not block external editors;
/// see the size+mtime sentinel in `perform_inner` for that surface.
pub(crate) struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    pub(crate) fn acquire(
        path: &Path,
        timeout: Duration,
        stale_after: Duration,
    ) -> Result<Self, TaskOpError> {
        let start = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    // Best-effort metadata write — never abort lock acquisition on this.
                    let _ = writeln!(
                        file,
                        "pid={} ts={}",
                        std::process::id(),
                        Utc::now().to_rfc3339()
                    );
                    return Ok(LockGuard {
                        path: path.to_path_buf(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Stale-lock recovery: kernel CREATE_NEW is the mutex —
                    // exactly one writer wins after the remove_file race.
                    if let Ok(meta) = std::fs::metadata(path) {
                        if meta
                            .modified()
                            .ok()
                            .and_then(|m| m.elapsed().ok())
                            .map(|d| d > stale_after)
                            .unwrap_or(false)
                        {
                            log::warn!("[task] removing stale lock at {}", path.display());
                            let _ = std::fs::remove_file(path);
                            continue;
                        }
                    }
                    if start.elapsed() >= timeout {
                        return Err(TaskOpError::LockTimeout);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(TaskOpError::LockIo(path.to_path_buf(), e)),
            }
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
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

    // ── 2. Read existing content ───────────────────────────────────────────
    let (existing, file_existed) = match std::fs::read_to_string(&task_path) {
        Ok(s) => (s, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => return Err(TaskOpError::ReadFailed(task_path, e)),
    };

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
    let new_parsed = apply_edit(&parsed, &op);

    // ── 5. Idempotence short-circuit ──────────────────────────────────────
    // SetTitle: semantic — short-circuit when YAML-decoded title value is
    //   unchanged (re-quoting/escaping never produces a NoOp).
    // Clean:    structural — short-circuit when post-edit frontmatter and
    //   body byte-match the pre-edit shape (covers repeated clean clicks).
    // AppendBody: never NoOp.
    let is_noop = match op {
        TaskOp::SetTitle(_) => title_value_of(&new_parsed) == title_value_of(&parsed),
        TaskOp::Clean => {
            new_parsed.frontmatter == parsed.frontmatter && new_parsed.body == parsed.body
        }
        TaskOp::AppendBody(_) => false,
    };
    if is_noop {
        return Ok(EditOutcome::NoOp);
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
    if let Err(e) = std::fs::write(&tmp_path, &new_content) {
        // MED-6 cleanup
        let _ = std::fs::remove_file(&tmp_path);
        return Err(TaskOpError::TmpWriteFailed(tmp_path, e));
    }

    // 7a. Sentinel check — see HIGH-4. Realistic editor-save case caught;
    // sub-millisecond TOCTOU at the read→metadata window remains theoretically open.
    // FAT32 mtime granularity is 2 s — for typical AC layouts (NTFS / EXT4 / APFS,
    // sub-second), this is not a concern.
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
            match std::fs::rename(&tmp_path, &task_path) {
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
            EditOutcome::NoOp => {}
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
    fn lock_guard_creates_and_removes_lockfile() {
        let fix = FixtureRoot::new("task-u19");
        let lock_path = fix.path().join("TASK.md.lock");
        {
            let _g = LockGuard::acquire(&lock_path, LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M).unwrap();
            assert!(lock_path.exists());
        }
        assert!(!lock_path.exists());
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
    fn lock_guard_recovers_stale_lockfile() {
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
        assert!(!lock_path.exists());
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
        // Lock file must also be gone.
        assert!(!wg.join("TASK.md.lock").exists());
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
            EditOutcome::Wrote { backup: Some(bp) } => bp,
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
        assert!(!wg.join("TASK.md.lock").exists());
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
        assert!(!wg.join("TASK.md.lock").exists());
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
        assert_eq!(
            out,
            "---\ntitle: 'Clean'\n---\nReady to start a new topic\n"
        );
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
        assert_eq!(p.body, "Ready to start a new topic\n");
    }

    #[test]
    fn apply_clean_preserves_crlf_and_bom() {
        // Round 2 (dev-rust R1.3): on a Notepad-saved Clean file with
        // body `"Ready to start a new topic\r\n"`, repeated Clean is NOT
        // idempotent — the CRLF→LF body conversion is treated as a
        // write-worthy diff. This matches `apply_append_body`'s pinned
        // trade-off (test U34).
        let input = "\u{FEFF}---\r\ntitle: old\r\nx: 1\r\n---\r\nbody\r\n";
        let parsed = parse_task(input);
        let p = apply_clean(&parsed);
        assert!(p.bom);
        assert_eq!(p.line_ending, "\r\n");
        let out = render(&p);
        // Frontmatter lines use CRLF; body uses LF (see §3.1.3 rationale).
        assert!(out.starts_with("\u{FEFF}---\r\ntitle: 'Clean'\r\n---\r\n"));
        assert!(out.ends_with("Ready to start a new topic\n"));
    }

    #[test]
    fn perform_clean_idempotent_on_canonical_clean() {
        let fix = FixtureRoot::new("task-u39");
        let wg = fix.path().join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        std::fs::write(
            wg.join("TASK.md"),
            "---\ntitle: 'Clean'\n---\nReady to start a new topic\n",
        )
        .unwrap();
        let now = || fixed_now_at(2026, 1, 1, 0, 0, 0);
        let r = perform_inner(&wg, TaskOp::Clean, now).unwrap();
        match r {
            EditOutcome::NoOp => {}
            other => panic!("expected NoOp, got {:?}", other),
        }
        // No backup file created.
        let entries: Vec<_> = std::fs::read_dir(&wg).unwrap().flatten().collect();
        let bak_count = entries
            .iter()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak.md"))
            .count();
        assert_eq!(bak_count, 0);
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
            EditOutcome::Wrote { backup: Some(bp) } => bp.clone(),
            other => panic!("expected Wrote with backup, got {:?}", other),
        };
        // HIGH-2 assertion: backup bytes must equal the pre-clean file.
        let backup_content = std::fs::read_to_string(&backup_path).unwrap();
        assert_eq!(backup_content, pre_clean);
        let final_content = std::fs::read_to_string(wg.join("TASK.md")).unwrap();
        assert_eq!(
            final_content,
            "---\ntitle: 'Clean'\n---\nReady to start a new topic\n"
        );
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
        assert!(matches!(r, EditOutcome::Wrote { backup: None }));
        assert_eq!(
            std::fs::read_to_string(wg.join("TASK.md")).unwrap(),
            "---\ntitle: 'Clean'\n---\nReady to start a new topic\n"
        );
        let bak_count = std::fs::read_dir(&wg)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak.md"))
            .count();
        assert_eq!(bak_count, 0);
    }
}
