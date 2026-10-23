//! #2713 (phase B1a of #2703) - the file-naming migration engine: the journal,
//! the locking, the disk-driven decision table and the create-only rename
//! primitive that every later phase-B migration calls.
//!
//! A leaf on purpose: it names only `super::instance_artifacts`, `std`, `serde`,
//! `serde_json`, `chrono` and, per platform, the OS bindings. The crate root is a
//! member of the crate's one cyclic SCC, and a module called from it that named
//! any SCC member would join that SCC. The guard
//! `naming_migration_names_nothing_that_reaches_the_knot` in
//! `tests/instance_gitignore_layering.rs` holds that line.
//!
//! No step deletes or overwrites. When both the old and the new name exist, the
//! new file wins and the old one is moved, create-only, to
//! `<from>.deprecated-<n>.no-git`.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::instance_artifacts::{
    CONFIG_STATE_TARGET_NAME, LOOP_STATE_TARGET_NAME, LOOP_STATE_TMP_TARGET_GLOB,
    NAMING_MIGRATION_LOCK_NAME, NAMING_MIGRATION_STATE_NAME, PROJECT_SETTINGS_TARGET_NAME,
    SET_ASIDE_GLOB,
};

mod ignore_rows;

pub(crate) use ignore_rows::{
    config_state_ignore_rows, loop_state_ignore_rows, missing_ignore_rows,
    project_settings_ignore_rows, retire_ignore_pairs,
};

/// The only journal format this build reads or writes.
const JOURNAL_VERSION: u32 = 1;

/// A thousand demoted copies in one directory is a pathology, not a state.
const SET_ASIDE_CEILING: u32 = 999;

const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The five-second lock budget: the value of the private `CONFIG_LOCK_TIMEOUT`
/// (`local_config_io.rs`), restated because this leaf may not name that module.
pub(crate) const MIGRATION_LOCK_BUDGET: Duration = Duration::from_secs(5);

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rename {
    pub from: &'static str,
    pub to: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum StepState {
    Renaming,
    Renamed,
    SourceAbsent,
    SetAside,
    Refused,
}

impl StepState {
    /// A recorded state the table skips when `from` is absent.
    fn is_settled(self) -> bool {
        matches!(
            self,
            StepState::Renamed | StepState::SourceAbsent | StepState::SetAside
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Renamed,
    AlreadyDone,
    SourceAbsent,
    SetAside(String),
    Refused(Refusal),
}

/// Both names present is resolved by a set-aside, never refused, so there is no
/// destination-exists variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    LockUnavailable,
    Io(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ScopeStatus {
    InProgress,
    Complete,
    Deferred,
    Unreachable,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StepRecord {
    pub dir: String,
    pub from: String,
    pub to: String,
    pub state: StepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_aside: Option<String>,
    pub at: String,
}

impl StepRecord {
    /// A record stamped now, with no reason and no set-aside name.
    pub(crate) fn new(dir: &Path, from: &str, to: &str, state: StepState) -> Self {
        StepRecord {
            dir: dir.to_string_lossy().into_owned(),
            from: from.to_string(),
            to: to.to_string(),
            state,
            reason: None,
            set_aside: None,
            at: now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopeRecord {
    pub status: ScopeStatus,
    pub updated_at: String,
    #[serde(default)]
    pub steps: Vec<StepRecord>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A read-only snapshot of the journal document, stale the moment it is
/// returned. Every mutation goes through `update_journal`, which re-reads under
/// the journal lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Journal {
    #[serde(skip)]
    path: PathBuf,
    version: u32,
    #[serde(default)]
    scopes: BTreeMap<String, ScopeRecord>,
}

fn now() -> String {
    // Fixed width, so a record rewritten in place keeps its byte length.
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

impl Journal {
    fn empty(path: PathBuf) -> Self {
        Journal {
            path,
            version: JOURNAL_VERSION,
            scopes: BTreeMap::new(),
        }
    }

    #[allow(dead_code)] // B1a API; no production caller yet
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn scope(&self, scope: &str) -> Option<&ScopeRecord> {
        self.scopes.get(scope)
    }

    fn scope_mut(&mut self, scope: &str) -> &mut ScopeRecord {
        self.scopes
            .entry(scope.to_string())
            .or_insert_with(|| ScopeRecord {
                status: ScopeStatus::InProgress,
                updated_at: now(),
                steps: Vec::new(),
                notes: Vec::new(),
            })
    }

    pub(crate) fn set_status(&mut self, scope: &str, status: ScopeStatus) {
        let record = self.scope_mut(scope);
        record.status = status;
        record.updated_at = now();
    }

    /// Records one step, keyed by its `(dir, from, to)` triple and replaced in
    /// place, so a scope holds exactly as many records as it has steps.
    pub(crate) fn record_step(&mut self, scope: &str, record: StepRecord) {
        let scope_record = self.scope_mut(scope);
        scope_record.updated_at = record.at.clone();
        match scope_record
            .steps
            .iter_mut()
            .find(|s| s.dir == record.dir && s.from == record.from && s.to == record.to)
        {
            Some(existing) => *existing = record,
            None => scope_record.steps.push(record),
        }
    }

    /// Adds an observation once: `notes` is bounded by what is observed, not by
    /// how often.
    pub(crate) fn note(&mut self, scope: &str, text: &str) {
        let record = self.scope_mut(scope);
        if !record.notes.iter().any(|n| n == text) {
            record.notes.push(text.to_string());
        }
    }

    fn step_state(&self, scope: &str, dir: &Path, from: &str, to: &str) -> Option<StepState> {
        let dir = dir.to_string_lossy();
        self.scopes
            .get(scope)?
            .steps
            .iter()
            .find_map(|s| (s.dir == dir && s.from == from && s.to == to).then_some(s.state))
    }
}

fn journal_path(dir: &Path) -> PathBuf {
    dir.join(NAMING_MIGRATION_STATE_NAME)
}

/// `Ok(None)` when there is no configuration directory or no journal file yet.
/// An unreadable or version-unknown journal is an error, never a reset.
fn read_journal_at(path: &Path) -> Result<Option<Journal>, Refusal> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Refusal::Io(format!(
                "failed to read naming-migration journal {}: {e}",
                path.display()
            )))
        }
    };
    let version = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| {
            Refusal::Io(format!(
                "unreadable naming-migration journal {}: {e}",
                path.display()
            ))
        })?
        .get("version")
        .and_then(serde_json::Value::as_u64);
    if version != Some(u64::from(JOURNAL_VERSION)) {
        return Err(Refusal::Io(format!(
            "naming-migration journal {} has unknown version {version:?}",
            path.display()
        )));
    }
    let mut journal: Journal = serde_json::from_slice(&bytes).map_err(|e| {
        Refusal::Io(format!(
            "unreadable naming-migration journal {}: {e}",
            path.display()
        ))
    })?;
    journal.path = path.to_path_buf();
    Ok(Some(journal))
}

/// `dir: None` = no configuration directory: no journal to read.
pub(crate) fn read_journal(dir: Option<&Path>) -> Result<Option<Journal>, Refusal> {
    match dir {
        None => Ok(None),
        Some(dir) => read_journal_at(&journal_path(dir)),
    }
}

#[allow(dead_code)] // first production caller: the B2 scope summary
pub(crate) fn status(j: &Journal, scope: &str) -> Option<ScopeStatus> {
    j.scopes.get(scope).map(|s| s.status)
}

/// For the journal summary only. Skip decisions call `scope_is_settled`.
#[allow(dead_code)] // first production caller: the B2 scope summary
pub(crate) fn is_complete(j: &Journal, scope: &str) -> bool {
    status(j, scope) == Some(ScopeStatus::Complete)
}

/// `Complete` is trusted only when the disk agrees: every recorded step must
/// have its `from` absent. A present `from` (a lost Windows directory entry, a
/// pre-migration binary, a restored backup) demotes the whole scope. With no
/// journal this is `false`.
pub(crate) fn scope_is_settled(dir_of: Option<&Journal>, scope: &str) -> bool {
    let Some(journal) = dir_of else {
        return false;
    };
    let Some(record) = journal.scopes.get(scope) else {
        return false;
    };
    record.status == ScopeStatus::Complete
        && record
            .steps
            .iter()
            .all(|s| matches!(present(&Path::new(&s.dir).join(&s.from)), Ok(false)))
}

fn present(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Locks
// ---------------------------------------------------------------------------

fn open_sidecar(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn is_lock_contention_error(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
        error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Opens (creating once, never deleting) and exclusively locks a sidecar,
/// bounded-blocking on `timeout`. The returned `File` is the lock.
fn acquire_sidecar(path: &Path, timeout: Duration) -> Result<File, Refusal> {
    let file = open_sidecar(path)
        .map_err(|e| Refusal::Io(format!("failed to open lock {}: {e}", path.display())))?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(e)) if is_lock_contention_error(&e) => {}
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(Refusal::Io(format!(
                    "failed to lock {}: {e}",
                    path.display()
                )))
            }
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(Refusal::LockUnavailable);
        }
        std::thread::sleep(LOCK_POLL_INTERVAL.min(timeout - elapsed));
    }
}

/// Proof that this scope's data lock is held. Dropping it releases every lock
/// this module opened; the sidecar files stay on disk.
#[derive(Debug)]
pub(crate) struct ScopeLock {
    _files: Vec<File>,
}

/// Acquire the NEW sidecar, then the OLD one when there is one: fixed order over
/// fixed names, bounded-blocking on `timeout`.
pub(crate) fn lock_scope(
    dir: &Path,
    new_lock: &str,
    old_lock: Option<&str>,
    timeout: Duration,
) -> Result<ScopeLock, Refusal> {
    let started = Instant::now();
    let mut files = vec![acquire_sidecar(&dir.join(new_lock), timeout)?];
    if let Some(old_lock) = old_lock {
        let remaining = timeout.saturating_sub(started.elapsed());
        files.push(acquire_sidecar(&dir.join(old_lock), remaining)?);
    }
    Ok(ScopeLock { _files: files })
}

/// For a caller whose own handle already covers the NEW name: takes only the
/// OLD sidecar. It never re-opens the caller's lock, which would block against
/// that handle.
#[allow(dead_code)] // first production caller: B2, under the catalog writer lock
pub(crate) fn lock_scope_under_held(
    dir: &Path,
    old_lock: Option<&str>,
    timeout: Duration,
) -> Result<ScopeLock, Refusal> {
    let mut files = Vec::new();
    if let Some(old_lock) = old_lock {
        files.push(acquire_sidecar(&dir.join(old_lock), timeout)?);
    }
    Ok(ScopeLock { _files: files })
}

// ---------------------------------------------------------------------------
// The journal writer
// ---------------------------------------------------------------------------

/// The one journal writer: take the instance-wide journal lock, RE-READ the
/// document under it, apply `edit`, commit atomically, release. Always the
/// innermost lock, never held across a rename. `dir == None`: `Ok(None)`.
pub(crate) fn update_journal<T>(
    dir: Option<&Path>,
    edit: impl FnOnce(&mut Journal) -> T,
) -> Result<Option<T>, Refusal> {
    let Some(dir) = dir else {
        return Ok(None);
    };
    let _lock = acquire_sidecar(&dir.join(NAMING_MIGRATION_LOCK_NAME), MIGRATION_LOCK_BUDGET)?;
    let path = journal_path(dir);
    let mut journal = read_journal_at(&path)?.unwrap_or_else(|| Journal::empty(path.clone()));
    let value = edit(&mut journal);
    commit_journal(dir, &journal)?;
    Ok(Some(value))
}

fn commit_journal(dir: &Path, journal: &Journal) -> Result<(), Refusal> {
    let destination = journal_path(dir);
    let bytes = serde_json::to_vec_pretty(journal)
        .map_err(|e| Refusal::Io(format!("failed to serialize naming-migration journal: {e}")))?;
    let temp = dir.join(format!(
        "{NAMING_MIGRATION_STATE_NAME}.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file_with_retry(&temp, &destination)?;
        sync_directory(dir)
    })();
    written.map_err(|e| {
        // This run's own temporary, never a stale one found on disk.
        let _ = std::fs::remove_file(&temp);
        Refusal::Io(format!(
            "failed to commit naming-migration journal {}: {e}",
            destination.display()
        ))
    })
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Windows has no directory `fsync`; a lost last commit re-runs an idempotent
/// step, because no decision depends on a record existing.
#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Sleeps between replace attempts: three attempts. Copied from
/// `config/loops.rs`, which sits inside the SCC this leaf must not name. Serves
/// the journal commit only; a set-aside never replaces.
const REPLACE_RETRY_DELAYS_MS: [u64; 2] = [20, 80];

fn replace_file_with_retry(src: &Path, dst: &Path) -> io::Result<()> {
    let mut delays = REPLACE_RETRY_DELAYS_MS.iter();
    loop {
        match replace_file(src, dst) {
            Err(e) if is_transient_replace_error(&e) => match delays.next() {
                Some(ms) => std::thread::sleep(Duration::from_millis(*ms)),
                None => return Err(e),
            },
            result => return result,
        }
    }
}

#[cfg(windows)]
fn replace_file(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let src_wide: Vec<u16> = src.as_os_str().encode_wide().chain(Some(0)).collect();
    let dst_wide: Vec<u16> = dst.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        MoveFileExW(
            src_wide.as_ptr(),
            dst_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::rename(src, dst)
}

#[cfg(windows)]
fn is_transient_replace_error(e: &io::Error) -> bool {
    // ERROR_ACCESS_DENIED (a held destination) and ERROR_SHARING_VIOLATION.
    matches!(e.raw_os_error(), Some(5) | Some(32))
}

#[cfg(not(windows))]
fn is_transient_replace_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
}

// ---------------------------------------------------------------------------
// The create-only move
// ---------------------------------------------------------------------------

/// Moves `from` onto `to` only when `to` does not exist: no placeholder, no
/// handle kept open, and `ErrorKind::AlreadyExists` means "take the next
/// ordinal", not failure.
#[cfg(windows)]
fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    #[cfg(test)]
    if let Some(kind) = take_migration_path_fault("move_no_replace", to) {
        return Err(io::Error::new(kind, "injected move_no_replace failure"));
    }
    let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // Deliberately WITHOUT `MOVEFILE_REPLACE_EXISTING`: an existing `to` fails
    // with ERROR_ALREADY_EXISTS (183) and its bytes are never replaced.
    let ok = unsafe { MoveFileExW(from_wide.as_ptr(), to_wide.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// `hard_link` is create-only; `std::fs::rename` is not used here because a
/// POSIX rename silently replaces an existing destination.
#[cfg(not(windows))]
fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(kind) = take_migration_path_fault("move_no_replace", to) {
        return Err(io::Error::new(kind, "injected move_no_replace failure"));
    }
    std::fs::hard_link(from, to)?;
    #[cfg(test)]
    if let Some(kind) = take_migration_path_fault("remove_file", from) {
        return Err(io::Error::new(kind, "injected remove_file failure"));
    }
    std::fs::remove_file(from)
}

// ---------------------------------------------------------------------------
// The step
// ---------------------------------------------------------------------------

/// `<from>.deprecated-<n>.no-git`, `n` the smallest positive integer absent in
/// `dir`, probed under the caller's held locks. Past `n == 999` it is an `Io`
/// refusal.
pub(crate) fn set_aside_name(dir: &Path, from: &str, _held: &ScopeLock) -> Result<String, Refusal> {
    for n in 1..=SET_ASIDE_CEILING {
        let candidate = format!("{from}.deprecated-{n}.no-git");
        match present(&dir.join(&candidate)) {
            Ok(true) => {}
            Ok(false) => return Ok(candidate),
            Err(e) => {
                return Err(Refusal::Io(format!(
                    "failed to probe set-aside name {candidate} in {}: {e}",
                    dir.display()
                )))
            }
        }
    }
    Err(Refusal::Io(format!(
        "set-aside ceiling of {SET_ASIDE_CEILING} reached for {from} in {}: \
         {from}.deprecated-{SET_ASIDE_CEILING}.no-git exists",
        dir.display()
    )))
}

fn commit_step(
    journal_dir: Option<&Path>,
    scope: &str,
    dir: &Path,
    from: &str,
    to: &str,
    state: StepState,
    set_aside: Option<String>,
) -> Result<(), Refusal> {
    update_journal(journal_dir, |j| {
        let record = StepRecord {
            set_aside,
            ..StepRecord::new(dir, from, to, state)
        };
        j.record_step(scope, record);
        j.set_status(scope, ScopeStatus::InProgress);
    })
    .map(|_| ())
}

/// Records a `Refused` step, best effort, and returns the refusal.
fn refuse_step(
    journal_dir: Option<&Path>,
    scope: &str,
    dir: &Path,
    from: &str,
    to: &str,
    refusal: Refusal,
    note: Option<String>,
) -> Outcome {
    let reason = match &refusal {
        Refusal::LockUnavailable => "lock unavailable".to_string(),
        Refusal::Io(message) => message.clone(),
    };
    let _ = update_journal(journal_dir, |j| {
        let record = StepRecord {
            reason: Some(reason),
            ..StepRecord::new(dir, from, to, StepState::Refused)
        };
        j.record_step(scope, record);
        j.set_status(scope, ScopeStatus::InProgress);
        if let Some(note) = &note {
            j.note(scope, note);
        }
    });
    Outcome::Refused(refusal)
}

/// A failed move that still left the bytes at `candidate` (a unix `hard_link`
/// that took before `remove_file` failed): one duplicate copy, recorded.
fn leftover_note(from: &str, candidate: &Path) -> Option<String> {
    matches!(present(candidate), Ok(true)).then(|| {
        format!(
            "leftover: {from} and {} hold the same bytes after a failed move",
            candidate.display()
        )
    })
}

/// Both names present: the new file wins and `from` is moved, create-only, to
/// the first absent `<from>.deprecated-<n>.no-git`.
fn set_aside(
    dir: &Path,
    from: &str,
    to: &str,
    scope: &str,
    journal_dir: Option<&Path>,
    held: &ScopeLock,
) -> Outcome {
    let from_path = dir.join(from);
    for _ in 0..SET_ASIDE_CEILING {
        let candidate = match set_aside_name(dir, from, held) {
            Ok(candidate) => candidate,
            Err(refusal) => return refuse_step(journal_dir, scope, dir, from, to, refusal, None),
        };
        migration_pause_hook("before_set_aside_move", dir);
        let candidate_path = dir.join(&candidate);
        match move_no_replace(&from_path, &candidate_path) {
            Ok(()) => {
                return match commit_step(
                    journal_dir,
                    scope,
                    dir,
                    from,
                    to,
                    StepState::SetAside,
                    Some(candidate.clone()),
                ) {
                    Ok(()) => Outcome::SetAside(candidate),
                    Err(refusal) => Outcome::Refused(refusal),
                };
            }
            // Someone else's file took the ordinal: take the next one.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                let note = leftover_note(from, &candidate_path);
                let refusal = Refusal::Io(format!(
                    "failed to set {} aside as {candidate}: {e}",
                    from_path.display()
                ));
                return refuse_step(journal_dir, scope, dir, from, to, refusal, note);
            }
        }
    }
    let refusal = Refusal::Io(format!(
        "set-aside ceiling of {SET_ASIDE_CEILING} reached for {from} in {}",
        dir.display()
    ));
    refuse_step(journal_dir, scope, dir, from, to, refusal, None)
}

fn step(
    dir: &Path,
    from: &str,
    to: &str,
    scope: &str,
    journal_dir: Option<&Path>,
    held: &ScopeLock,
) -> Outcome {
    let recorded = match read_journal(journal_dir) {
        Ok(journal) => journal.and_then(|j| j.step_state(scope, dir, from, to)),
        Err(refusal) => return Outcome::Refused(refusal),
    };
    let from_path = dir.join(from);
    let to_path = dir.join(to);
    let (from_present, to_present) = match (present(&from_path), present(&to_path)) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(e), _) | (_, Err(e)) => {
            let refusal = Refusal::Io(format!(
                "failed to probe {from} / {to} in {}: {e}",
                dir.display()
            ));
            return refuse_step(journal_dir, scope, dir, from, to, refusal, None);
        }
    };
    let settled = recorded.is_some_and(StepState::is_settled);
    match (from_present, to_present) {
        (true, true) => set_aside(dir, from, to, scope, journal_dir, held),
        (true, false) => {
            if let Err(refusal) =
                commit_step(journal_dir, scope, dir, from, to, StepState::Renaming, None)
            {
                return Outcome::Refused(refusal);
            }
            migration_pause_hook("before_rename", dir);
            match move_no_replace(&from_path, &to_path) {
                Ok(()) => {}
                // `to` appeared since the probe: both names present.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    return set_aside(dir, from, to, scope, journal_dir, held)
                }
                Err(e) => {
                    let note = leftover_note(from, &to_path);
                    let refusal = Refusal::Io(format!(
                        "failed to rename {} to {to}: {e}",
                        from_path.display()
                    ));
                    return refuse_step(journal_dir, scope, dir, from, to, refusal, note);
                }
            }
            migration_pause_hook("after_rename", dir);
            match commit_step(journal_dir, scope, dir, from, to, StepState::Renamed, None) {
                Ok(()) => Outcome::Renamed,
                Err(refusal) => Outcome::Refused(refusal),
            }
        }
        (false, true) => {
            if !settled {
                if let Err(refusal) =
                    commit_step(journal_dir, scope, dir, from, to, StepState::Renamed, None)
                {
                    return Outcome::Refused(refusal);
                }
            }
            Outcome::AlreadyDone
        }
        (false, false) => {
            if !settled {
                if let Err(refusal) = commit_step(
                    journal_dir,
                    scope,
                    dir,
                    from,
                    to,
                    StepState::SourceAbsent,
                    None,
                ) {
                    return Outcome::Refused(refusal);
                }
            }
            Outcome::SourceAbsent
        }
    }
}

/// One journalled rename of `dir/from` to `dir/to`, decided by the disk:
/// commits `Renaming` before the rename and `Renamed` after it. The `ScopeLock`
/// is never read: it is the type-level proof that the scope's locks are held.
pub(crate) fn rename_step(
    dir: &Path,
    r: &Rename,
    scope: &str,
    journal_dir: Option<&Path>,
    held: &ScopeLock,
) -> Outcome {
    step(dir, r.from, r.to, scope, journal_dir, held)
}

/// `rename_step` for every `dir` entry between `old_prefix` and `suffix`,
/// renamed to `new_prefix` + the middle + `suffix`, in name order.
pub(crate) fn rename_prefix_family(
    dir: &Path,
    old_prefix: &str,
    new_prefix: &str,
    suffix: &str,
    scope: &str,
    journal_dir: Option<&Path>,
    held: &ScopeLock,
) -> Vec<(String, Outcome)> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            let refusal = Refusal::Io(format!("failed to list {}: {e}", dir.display()));
            return vec![(old_prefix.to_string(), Outcome::Refused(refusal))];
        }
    };
    let in_family = |name: &str| {
        name.len() >= old_prefix.len() + suffix.len()
            && name.starts_with(old_prefix)
            && name.ends_with(suffix)
    };
    // An entry this pass cannot read or name may be a member of the family, so
    // it refuses the whole family: a skipped member would let the scope reach
    // `Complete` with its old name still on disk.
    let refuse = |reason: String| {
        vec![(
            old_prefix.to_string(),
            Outcome::Refused(Refusal::Io(reason)),
        )]
    };
    let mut names = Vec::new();
    for entry in entries {
        #[cfg(test)]
        let entry = match take_migration_path_fault("read_dir_entry", dir) {
            Some(kind) => Err(io::Error::new(kind, "injected read_dir entry failure")),
            None => entry,
        };
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return refuse(format!("failed to list an entry of {}: {e}", dir.display())),
        };
        match entry.file_name().into_string() {
            Ok(name) if in_family(&name) => names.push(name),
            Ok(_) => {}
            Err(raw) if in_family(&raw.to_string_lossy()) => {
                return refuse(format!(
                    "entry {} of {} is not valid UTF-8",
                    raw.to_string_lossy(),
                    dir.display()
                ))
            }
            Err(_) => {}
        }
    }
    names.sort();
    names
        .into_iter()
        .map(|from| {
            let middle = &from[old_prefix.len()..from.len() - suffix.len()];
            let to = format!("{new_prefix}{middle}{suffix}");
            let outcome = step(dir, &from, &to, scope, journal_dir, held);
            (from, outcome)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Test seams
// ---------------------------------------------------------------------------

#[cfg(not(test))]
#[inline(always)]
fn migration_pause_hook(_stage: &str, _dir: &Path) {}

/// Test-only pause point. Inert unless a test arms it.
#[cfg(test)]
fn migration_pause_hook(stage: &str, dir: &Path) {
    pause::hook(stage, dir);
}

#[cfg(test)]
struct MigrationPathFault {
    op: &'static str,
    path: PathBuf,
    kind: io::ErrorKind,
}

#[cfg(test)]
thread_local! {
    static MIGRATION_PATH_FAULTS: std::cell::RefCell<Vec<MigrationPathFault>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Arms one injected failure of `op` (`move_no_replace`, or `remove_file` off
/// Windows) on `path`, for the current thread.
#[cfg(test)]
fn arm_migration_path_fault(op: &'static str, path: &Path, kind: io::ErrorKind) {
    MIGRATION_PATH_FAULTS.with(|faults| {
        faults.borrow_mut().push(MigrationPathFault {
            op,
            path: path.to_path_buf(),
            kind,
        })
    });
}

#[cfg(test)]
fn take_migration_path_fault(op: &str, path: &Path) -> Option<io::ErrorKind> {
    MIGRATION_PATH_FAULTS.with(|faults| {
        let mut faults = faults.borrow_mut();
        let index = faults
            .iter()
            .position(|fault| fault.op == op && fault.path == path)?;
        Some(faults.remove(index).kind)
    })
}

/// A copy of the #2682 harness in `config/loops.rs`, which is inside the SCC
/// this leaf must not name. In-process legs arm a `(stage, dir)` pair; a
/// cross-process child is armed by environment through a rendezvous directory.
#[cfg(test)]
pub(crate) mod pause;

#[cfg(test)]
mod tests;
