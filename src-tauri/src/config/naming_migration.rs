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
    NAMING_MIGRATION_LOCK_NAME, NAMING_MIGRATION_STATE_NAME, PROJECT_SETTINGS_TARGET_NAME,
    SET_ASIDE_GLOB,
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

/// #2717 (B4a) - the three `.ac/.gitignore` rows the project-settings rename
/// needs, as `(pattern, comment)`. The one home of this table: the writer at
/// registration (`commands::ac_discovery`) and the migration's own sweep
/// (`config::project_settings`) both read it here. Composed from the registry
/// at runtime, so no second spelling of a name exists in production.
pub(crate) fn project_settings_ignore_rows() -> [(String, &'static str); 3] {
    [
        (
            format!("/{PROJECT_SETTINGS_TARGET_NAME}"),
            "# AgentsCommander: exclude generated project-local settings.",
        ),
        (
            format!("/.{PROJECT_SETTINGS_TARGET_NAME}.lock"),
            "# AgentsCommander: exclude the project settings write-lock sidecar.",
        ),
        (
            format!("/{SET_ASIDE_GLOB}"),
            "# AgentsCommander: exclude project files the naming migration set aside.",
        ),
    ]
}

/// The managed blocks, `\n<comment>\n<pattern>\n`, of each `(pattern, comment)`
/// pair of `rows` whose pattern line is absent from `content`; empty when there
/// is nothing to add. Presence is exact line equality, so a line with a leading
/// space, which Git does not read as the rule, does not count. Pure text.
pub(crate) fn missing_ignore_rows(content: &str, rows: &[(String, &str)]) -> String {
    let mut blocks = String::new();
    for (pattern, comment) in rows {
        if !content.lines().any(|line| line == pattern) {
            blocks.push_str(&format!("\n{comment}\n{pattern}\n"));
        }
    }
    blocks
}

/// Removes exactly the AC-written `comment` + `pattern` pairs named in `retired`
/// from an ignore body, with the one blank line the managed block wrote before
/// the comment. A `pattern` line with no comment of its own directly above it
/// is user intent and is preserved. CRLF and a missing final newline survive
/// byte for byte. Pure text. Generalizes `migrate_legacy_seed_manifest_gitignore`.
pub(crate) fn retire_ignore_pairs(content: &str, retired: &[(&str, &str)]) -> (String, bool) {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut removed = vec![false; lines.len()];
    let mut changed = false;
    for (index, line) in lines.iter().enumerate() {
        let Some(previous) = index.checked_sub(1) else {
            continue;
        };
        let is_pair = retired.iter().any(|(comment, pattern)| {
            line.trim() == *pattern && lines[previous].trim() == *comment
        });
        if !is_pair {
            continue;
        }
        removed[previous] = true;
        removed[index] = true;
        if let Some(blank) = previous.checked_sub(1) {
            if lines[blank].trim().is_empty() && !removed[blank] {
                removed[blank] = true;
            }
        }
        changed = true;
    }
    if !changed {
        return (content.to_string(), false);
    }
    let migrated: String = lines
        .iter()
        .zip(&removed)
        .filter(|(_, drop)| !**drop)
        .map(|(line, _)| *line)
        .collect();
    (migrated, true)
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
pub(crate) mod pause {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    pub(crate) const PAUSE_DIR_ENV: &str = "AC_2713_MIGRATION_PAUSE_DIR";
    pub(crate) const PAUSE_STAGE_ENV: &str = "AC_2713_MIGRATION_PAUSE_STAGE";
    pub(crate) const READY_FILE: &str = "paused.ready";
    pub(crate) const RELEASE_FILE: &str = "paused.release";
    const BOUND: Duration = Duration::from_secs(60);

    type Slot = (mpsc::Sender<()>, mpsc::Receiver<()>);

    fn registry() -> &'static Mutex<HashMap<(String, PathBuf), Slot>> {
        static REGISTRY: OnceLock<Mutex<HashMap<(String, PathBuf), Slot>>> = OnceLock::new();
        REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// One armed pause: `reached` fires when the engine stops at the stage, and
    /// it resumes on `release`.
    pub(crate) struct Armed {
        pub(crate) reached: mpsc::Receiver<()>,
        pub(crate) release: mpsc::Sender<()>,
    }

    pub(crate) fn arm(stage: &str, dir: &Path) -> Armed {
        let (reached_tx, reached) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        registry().lock().expect("pause registry").insert(
            (stage.to_string(), dir.to_path_buf()),
            (reached_tx, release_rx),
        );
        Armed { reached, release }
    }

    pub(crate) fn hook(stage: &str, dir: &Path) {
        let slot = registry()
            .lock()
            .expect("pause registry")
            .remove(&(stage.to_string(), dir.to_path_buf()));
        if let Some((reached, release)) = slot {
            let _ = reached.send(());
            release
                .recv_timeout(BOUND)
                .expect("in-process pause released within its 60 s bound");
            return;
        }
        let (Some(rendezvous), Some(armed_stage)) = (
            std::env::var_os(PAUSE_DIR_ENV),
            std::env::var_os(PAUSE_STAGE_ENV),
        ) else {
            return;
        };
        if armed_stage != stage {
            return;
        }
        let rendezvous = PathBuf::from(rendezvous);
        std::fs::write(rendezvous.join(READY_FILE), b"ready").expect("announce pause");
        let started = Instant::now();
        while !rendezvous.join(RELEASE_FILE).exists() {
            assert!(
                started.elapsed() < BOUND,
                "cross-process pause exceeded 60 s"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as Map;
    use std::ffi::OsString;
    use std::process::{Child, Command, Stdio};

    // Synthetic names only: a product name here would make the engine agree
    // with a later phase by construction instead of by observation.
    const FROM: &str = "legacy.json";
    const TO: &str = "target.json";
    const NEW_LOCK: &str = ".target.json.lock";
    const OLD_LOCK: &str = ".legacy.json.lock";
    const SCOPE: &str = "synthetic";
    const STEP: Rename = Rename { from: FROM, to: TO };
    const ONE_STEP: [Rename; 1] = [STEP];
    const MANY: [Rename; 6] = [
        Rename {
            from: "legacy-0.json",
            to: "target-0.json",
        },
        Rename {
            from: "legacy-1.json",
            to: "target-1.json",
        },
        Rename {
            from: "legacy-2.json",
            to: "target-2.json",
        },
        Rename {
            from: "legacy-3.json",
            to: "target-3.json",
        },
        Rename {
            from: "legacy-4.json",
            to: "target-4.json",
        },
        Rename {
            from: "legacy-5.json",
            to: "target-5.json",
        },
    ];

    const CHILD_FQN: &str = "config::naming_migration::tests::child_process_entry";
    const CHILD_ROLE_ENV: &str = "AC_2713_CHILD_ROLE";
    const CHILD_DATA_ENV: &str = "AC_2713_CHILD_DATA_DIR";
    const CHILD_JOURNAL_ENV: &str = "AC_2713_CHILD_JOURNAL_DIR";
    const CHILD_SCOPE_ENV: &str = "AC_2713_CHILD_SCOPE";
    const CHILD_RENDEZVOUS_ENV: &str = "AC_2713_CHILD_RENDEZVOUS";
    const WAIT_BOUND: Duration = Duration::from_secs(60);

    fn aside(n: u32) -> String {
        format!("{FROM}.deprecated-{n}.no-git")
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        data: PathBuf,
        cfg: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data = tmp.path().join("data");
        let cfg = tmp.path().join("cfg");
        std::fs::create_dir_all(&data).expect("data dir");
        std::fs::create_dir_all(&cfg).expect("cfg dir");
        Fixture {
            _tmp: tmp,
            data,
            cfg,
        }
    }

    fn put(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::write(dir.join(name), bytes).expect("seed file");
    }

    fn get(dir: &Path, name: &str) -> Option<Vec<u8>> {
        match std::fs::read(dir.join(name)) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => panic!("read {name}: {e}"),
        }
    }

    /// Every file in `dir` with its bytes; lock sidecars by name only.
    fn snapshot(dir: &Path) -> Map<String, Vec<u8>> {
        std::fs::read_dir(dir)
            .expect("list dir")
            .map(|entry| {
                let entry = entry.expect("dir entry");
                let name = entry.file_name().into_string().expect("utf-8 name");
                // A held sidecar cannot be read on Windows; its bytes are empty.
                let bytes = if name.ends_with(".lock") {
                    Vec::new()
                } else {
                    std::fs::read(entry.path()).expect("read entry")
                };
                (name, bytes)
            })
            .collect()
    }

    fn journal(cfg: &Path) -> Journal {
        read_journal(Some(cfg))
            .expect("readable journal")
            .expect("journal exists")
    }

    fn steps_of(cfg: &Path, scope: &str) -> Vec<StepRecord> {
        journal(cfg)
            .scope(scope)
            .expect("scope recorded")
            .steps
            .clone()
    }

    fn lock(dir: &Path) -> ScopeLock {
        lock_scope(dir, NEW_LOCK, Some(OLD_LOCK), MIGRATION_LOCK_BUDGET).expect("scope lock")
    }

    /// Creates the two lock sidecars, so an entry count taken afterwards sees
    /// only what the migration itself adds.
    fn prime_locks(dir: &Path) {
        drop(lock(dir));
    }

    /// The caller shape every phase follows: a pre-check, the scope lock, the
    /// re-check under it through `update_journal`, the steps, then `Complete`.
    fn run_scope_with(
        dir: &Path,
        steps: &[Rename],
        family: Option<(&str, &str, &str)>,
        scope: &str,
        journal_dir: Option<&Path>,
        timeout: Duration,
    ) -> Result<Vec<Outcome>, Refusal> {
        if scope_is_settled(read_journal(journal_dir)?.as_ref(), scope) {
            return Ok(Vec::new());
        }
        let held = lock_scope(dir, NEW_LOCK, Some(OLD_LOCK), timeout)?;
        if update_journal(journal_dir, |j| scope_is_settled(Some(j), scope))?.unwrap_or(false) {
            return Ok(Vec::new());
        }
        let mut outcomes: Vec<Outcome> = steps
            .iter()
            .map(|r| rename_step(dir, r, scope, journal_dir, &held))
            .collect();
        if let Some((old_prefix, new_prefix, suffix)) = family {
            outcomes.extend(
                rename_prefix_family(
                    dir,
                    old_prefix,
                    new_prefix,
                    suffix,
                    scope,
                    journal_dir,
                    &held,
                )
                .into_iter()
                .map(|(_, outcome)| outcome),
            );
        }
        if outcomes.iter().any(|o| matches!(o, Outcome::Refused(_))) {
            return Ok(outcomes);
        }
        migration_pause_hook("before_scope_complete", dir);
        update_journal(journal_dir, |j| j.set_status(scope, ScopeStatus::Complete))?;
        Ok(outcomes)
    }

    fn run_scope(
        dir: &Path,
        steps: &[Rename],
        scope: &str,
        journal_dir: Option<&Path>,
    ) -> Vec<Outcome> {
        run_scope_with(dir, steps, None, scope, journal_dir, MIGRATION_LOCK_BUDGET)
            .expect("scope ran")
    }

    fn assert_complete(cfg: &Path, scope: &str) {
        assert_eq!(status(&journal(cfg), scope), Some(ScopeStatus::Complete));
    }

    // -- cross-process plumbing ----------------------------------------------

    fn spawn_child(role: &str, envs: &[(&str, OsString)]) -> Child {
        let mut command = Command::new(std::env::current_exe().expect("current test exe"));
        command
            .args(["--exact", CHILD_FQN, "--nocapture", "--test-threads=1"])
            .env(CHILD_ROLE_ENV, role)
            .env_remove(pause::PAUSE_DIR_ENV)
            .env_remove(pause::PAUSE_STAGE_ENV)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in envs {
            command.env(key, value);
        }
        command.spawn().expect("spawn child")
    }

    fn wait_for_file(path: &Path) {
        let started = Instant::now();
        while !path.exists() {
            assert!(
                started.elapsed() < WAIT_BOUND,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_child(mut child: Child) {
        let started = Instant::now();
        while child.try_wait().expect("poll child").is_none() {
            if started.elapsed() >= WAIT_BOUND {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child exceeded {WAIT_BOUND:?} and was killed");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().expect("child output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "child failed or ran nothing: {}\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn seed_many(dir: &Path, tag: &str) {
        for r in &MANY {
            put(dir, r.from, format!("{tag}:{}", r.from).as_bytes());
        }
    }

    fn assert_many_complete(cfg: &Path, data: &Path, scope: &str, tag: &str) {
        let steps = steps_of(cfg, scope);
        assert_eq!(
            steps.len(),
            MANY.len(),
            "{scope} lost step records: {steps:?}"
        );
        for r in &MANY {
            let record = steps
                .iter()
                .find(|s| s.from == r.from && s.to == r.to)
                .unwrap_or_else(|| panic!("{scope} lost the record of {}", r.from));
            assert_eq!(record.state, StepState::Renamed);
            assert_eq!(
                get(data, r.to),
                Some(format!("{tag}:{}", r.from).into_bytes())
            );
        }
        assert_complete(cfg, scope);
    }

    /// Not a test of its own: the body a spawned child runs, inert otherwise.
    #[test]
    fn child_process_entry() {
        let Some(role) = std::env::var_os(CHILD_ROLE_ENV) else {
            return;
        };
        let data = PathBuf::from(std::env::var_os(CHILD_DATA_ENV).expect("data dir"));
        match role.to_str() {
            Some("scope") => {
                let cfg = PathBuf::from(std::env::var_os(CHILD_JOURNAL_ENV).expect("journal dir"));
                let scope = std::env::var(CHILD_SCOPE_ENV).expect("scope");
                let outcomes = run_scope(&data, &MANY, &scope, Some(&cfg));
                assert!(
                    outcomes.iter().all(|o| *o == Outcome::Renamed),
                    "{outcomes:?}"
                );
            }
            Some("hold") => {
                let rendezvous =
                    PathBuf::from(std::env::var_os(CHILD_RENDEZVOUS_ENV).expect("rendezvous"));
                let _held = lock(&data);
                std::fs::write(rendezvous.join(pause::READY_FILE), b"ready").expect("ready");
                wait_for_file(&rendezvous.join(pause::RELEASE_FILE));
            }
            other => panic!("unknown child role {other:?}"),
        }
    }

    #[test]
    fn pause_hook_is_disarmed_by_default() {
        assert!(std::env::var_os(pause::PAUSE_DIR_ENV).is_none());
        assert!(std::env::var_os(pause::PAUSE_STAGE_ENV).is_none());
        let tmp = tempfile::tempdir().expect("tempdir");
        let started = Instant::now();
        for stage in ["before_rename", "after_rename", "before_set_aside_move"] {
            migration_pause_hook(stage, tmp.path());
        }
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    // -- E4 ------------------------------------------------------------------

    #[test]
    fn happy_path_renames_byte_for_byte_and_is_idempotent() {
        let f = fixture();
        put(&f.data, FROM, b"legacy bytes\r\n");
        put(&f.data, "legacy.backup.1.json", b"backup one");
        put(&f.data, "legacy.backup.2.json", b"backup two\n");
        let family = Some(("legacy.backup.", "target.backup.", ".json"));

        let outcomes = run_scope_with(
            &f.data,
            &[STEP],
            family,
            SCOPE,
            Some(&f.cfg),
            MIGRATION_LOCK_BUDGET,
        )
        .expect("first run");
        assert_eq!(outcomes, vec![Outcome::Renamed; 3]);
        assert_eq!(get(&f.data, TO), Some(b"legacy bytes\r\n".to_vec()));
        assert_eq!(
            get(&f.data, "target.backup.1.json"),
            Some(b"backup one".to_vec())
        );
        assert_eq!(
            get(&f.data, "target.backup.2.json"),
            Some(b"backup two\n".to_vec())
        );
        for gone in [FROM, "legacy.backup.1.json", "legacy.backup.2.json"] {
            assert_eq!(get(&f.data, gone), None, "{gone} survived");
        }
        assert_complete(&f.cfg, SCOPE);
        assert!(scope_is_settled(Some(&journal(&f.cfg)), SCOPE));

        let before = snapshot(&f.data);
        let again = run_scope_with(
            &f.data,
            &[STEP],
            family,
            SCOPE,
            Some(&f.cfg),
            MIGRATION_LOCK_BUDGET,
        )
        .expect("second run");
        assert!(again.is_empty(), "a settled scope ran again: {again:?}");
        let held = lock(&f.data);
        assert_eq!(
            rename_step(&f.data, &STEP, SCOPE, Some(&f.cfg), &held),
            Outcome::AlreadyDone
        );
        drop(held);
        assert_eq!(snapshot(&f.data), before, "the second run changed bytes");
    }

    // -- E5, E5b, E5c, E5d ---------------------------------------------------

    #[test]
    fn both_present_sets_the_old_file_aside_and_keeps_the_new_one() {
        let f = fixture();
        put(&f.data, FROM, b"old bytes");
        put(&f.data, TO, b"new bytes");
        prime_locks(&f.data);
        let before = snapshot(&f.data);

        let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
        assert_eq!(outcomes, vec![Outcome::SetAside(aside(1))]);
        let after = snapshot(&f.data);
        // A move: `from` leaves and the set-aside name arrives, so the count
        // holds. One fewer would be a deletion.
        assert_eq!(after.len(), before.len(), "an entry was lost: {after:?}");
        assert_eq!(after.get(TO), before.get(TO), "the new file was touched");
        assert_eq!(get(&f.data, FROM), None);
        assert_eq!(get(&f.data, &aside(1)), Some(b"old bytes".to_vec()));
        let steps = steps_of(&f.cfg, SCOPE);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].state, StepState::SetAside);
        assert_eq!(steps[0].set_aside.as_deref(), Some(aside(1).as_str()));
        assert_complete(&f.cfg, SCOPE);
    }

    #[test]
    fn set_aside_picks_the_first_absent_ordinal() {
        let f = fixture();
        put(&f.data, FROM, b"old");
        put(&f.data, TO, b"new");
        put(&f.data, &aside(1), b"first");
        put(&f.data, &aside(3), b"third");
        assert_eq!(
            run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
            vec![Outcome::SetAside(aside(2))]
        );
        assert_eq!(get(&f.data, &aside(1)), Some(b"first".to_vec()));
        assert_eq!(get(&f.data, &aside(2)), Some(b"old".to_vec()));
        assert_eq!(get(&f.data, &aside(3)), Some(b"third".to_vec()));

        let g = fixture();
        put(&g.data, FROM, b"old");
        put(&g.data, TO, b"new");
        for n in 1..=999 {
            put(&g.data, &aside(n), n.to_string().as_bytes());
        }
        prime_locks(&g.data);
        let before = snapshot(&g.data);
        let outcomes = run_scope(&g.data, &[STEP], SCOPE, Some(&g.cfg));
        match outcomes.as_slice() {
            [Outcome::Refused(Refusal::Io(message))] => {
                assert!(message.contains("ceiling of 999"), "{message}")
            }
            other => panic!("the 999 ceiling must be an Io refusal: {other:?}"),
        }
        assert_eq!(
            snapshot(&g.data),
            before,
            "a file was touched at the ceiling"
        );
    }

    #[test]
    fn an_outsider_created_ordinal_is_never_replaced() {
        let f = fixture();
        put(&f.data, FROM, b"old");
        put(&f.data, TO, b"new");
        prime_locks(&f.data);
        let base = snapshot(&f.data).len();

        let first = pause::arm("before_set_aside_move", &f.data);
        let (data, cfg) = (f.data.clone(), f.cfg.clone());
        let worker = std::thread::spawn(move || run_scope(&data, &[STEP], SCOPE, Some(&cfg)));
        first
            .reached
            .recv_timeout(WAIT_BOUND)
            .expect("first pause reached");
        assert_eq!(snapshot(&f.data).len(), base, "a placeholder was created");
        assert_eq!(get(&f.data, &aside(1)), None);
        let second = pause::arm("before_set_aside_move", &f.data);
        // A sync tool creates the candidate in the one gap before the move.
        put(&f.data, &aside(1), b"outsider bytes");
        first.release.send(()).expect("release first");
        second
            .reached
            .recv_timeout(WAIT_BOUND)
            .expect("second pause reached");
        assert_eq!(
            snapshot(&f.data).len(),
            base + 1,
            "a placeholder was created"
        );
        assert_eq!(get(&f.data, &aside(2)), None);
        second.release.send(()).expect("release second");
        let outcomes = worker.join().expect("worker");

        assert_eq!(
            get(&f.data, &aside(1)),
            Some(b"outsider bytes".to_vec()),
            "the outsider's file was replaced"
        );
        assert_eq!(outcomes, vec![Outcome::SetAside(aside(2))]);
        assert_eq!(get(&f.data, &aside(2)), Some(b"old".to_vec()));
        assert_eq!(get(&f.data, TO), Some(b"new".to_vec()));
        assert_eq!(
            steps_of(&f.cfg, SCOPE)[0].set_aside.as_deref(),
            Some(aside(2).as_str())
        );
        assert_complete(&f.cfg, SCOPE);
    }

    #[test]
    fn a_failed_set_aside_move_never_loses_bytes_and_never_leaves_a_stub() {
        let f = fixture();
        put(&f.data, FROM, b"old");
        put(&f.data, TO, b"new");
        prime_locks(&f.data);
        let before = snapshot(&f.data);

        #[cfg(windows)]
        {
            let candidate = f.data.join(aside(1));
            arm_migration_path_fault(
                "move_no_replace",
                &candidate,
                io::ErrorKind::PermissionDenied,
            );
            let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
            match outcomes.as_slice() {
                [Outcome::Refused(Refusal::Io(message))] => {
                    assert!(message.contains(&aside(1)), "{message}")
                }
                other => panic!("expected an Io refusal: {other:?}"),
            }
            assert_eq!(snapshot(&f.data), before, "entries or bytes changed");
        }

        #[cfg(unix)]
        {
            arm_migration_path_fault(
                "remove_file",
                &f.data.join(FROM),
                io::ErrorKind::PermissionDenied,
            );
            let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
            assert!(
                matches!(outcomes.as_slice(), [Outcome::Refused(Refusal::Io(_))]),
                "{outcomes:?}"
            );
            assert_eq!(get(&f.data, FROM), Some(b"old".to_vec()));
            assert_eq!(get(&f.data, &aside(1)), Some(b"old".to_vec()));
            assert_eq!(get(&f.data, TO), Some(b"new".to_vec()));
            assert_eq!(snapshot(&f.data).len(), before.len() + 1);
            let notes = journal(&f.cfg).scope(SCOPE).expect("scope").notes.clone();
            assert!(notes.iter().any(|n| n.contains("leftover")), "{notes:?}");
            assert_eq!(
                run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
                vec![Outcome::SetAside(aside(2))]
            );
            assert_complete(&f.cfg, SCOPE);
        }
    }

    #[test]
    fn a_family_enumeration_error_refuses_and_the_scope_cannot_complete() {
        let f = fixture();
        put(&f.data, "legacy.backup.1.json", b"backup one");
        let family = Some(("legacy.backup.", "target.backup.", ".json"));
        arm_migration_path_fault("read_dir_entry", &f.data, io::ErrorKind::PermissionDenied);
        let outcomes = run_scope_with(
            &f.data,
            &[],
            family,
            SCOPE,
            Some(&f.cfg),
            MIGRATION_LOCK_BUDGET,
        )
        .expect("the scope ran");
        assert!(
            matches!(outcomes.as_slice(), [Outcome::Refused(Refusal::Io(_))]),
            "an unreadable entry was skipped: {outcomes:?}"
        );
        assert_ne!(status(&journal(&f.cfg), SCOPE), Some(ScopeStatus::Complete));
        assert!(!scope_is_settled(Some(&journal(&f.cfg)), SCOPE));
        assert_eq!(
            get(&f.data, "legacy.backup.1.json"),
            Some(b"backup one".to_vec())
        );

        let outcomes = run_scope_with(
            &f.data,
            &[],
            family,
            SCOPE,
            Some(&f.cfg),
            MIGRATION_LOCK_BUDGET,
        )
        .expect("the scope ran again");
        assert_eq!(outcomes, vec![Outcome::Renamed]);
        assert_eq!(
            get(&f.data, "target.backup.1.json"),
            Some(b"backup one".to_vec())
        );
        assert_complete(&f.cfg, SCOPE);
    }

    // -- E6, E7 --------------------------------------------------------------

    #[derive(Clone, Copy, Debug)]
    enum Disk {
        FromOnly,
        ToOnly,
        Neither,
        Both,
        BothWithAside,
    }

    fn seed(dir: &Path, disk: Disk) {
        match disk {
            Disk::FromOnly => put(dir, FROM, b"old"),
            Disk::ToOnly => put(dir, TO, b"new"),
            Disk::Neither => {}
            Disk::Both => {
                put(dir, FROM, b"old");
                put(dir, TO, b"new");
            }
            Disk::BothWithAside => {
                put(dir, FROM, b"old");
                put(dir, TO, b"new");
                put(dir, &aside(1), b"earlier");
            }
        }
    }

    #[test]
    fn the_disk_decides_when_there_is_no_record() {
        const OTHER: Rename = Rename {
            from: "other.json",
            to: "other-target.json",
        };
        let cases = [
            (Disk::ToOnly, Outcome::AlreadyDone, StepState::Renamed),
            (
                Disk::Neither,
                Outcome::SourceAbsent,
                StepState::SourceAbsent,
            ),
            (Disk::FromOnly, Outcome::Renamed, StepState::Renamed),
        ];
        for (disk, outcome, state) in cases.clone() {
            let f = fixture();
            seed(&f.data, disk);
            update_journal(Some(&f.cfg), |_| ()).expect("empty journal");
            assert_eq!(
                run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
                vec![outcome],
                "{disk:?}"
            );
            assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, state, "{disk:?}");
            assert_complete(&f.cfg, SCOPE);
        }
        // The same three with the journal file deleted mid-scope.
        for (disk, outcome, state) in cases {
            let f = fixture();
            seed(&f.data, disk);
            let held = lock(&f.data);
            assert_eq!(
                rename_step(&f.data, &OTHER, SCOPE, Some(&f.cfg), &held),
                Outcome::SourceAbsent
            );
            std::fs::remove_file(f.cfg.join(NAMING_MIGRATION_STATE_NAME)).expect("delete journal");
            assert_eq!(
                rename_step(&f.data, &STEP, SCOPE, Some(&f.cfg), &held),
                outcome,
                "{disk:?}"
            );
            assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, state, "{disk:?}");
        }
    }

    #[test]
    fn a_refused_step_re_evaluates_after_the_state_changes() {
        let f = fixture();
        put(&f.data, FROM, b"old");
        let holder = lock(&f.data);
        let refused = run_scope_with(
            &f.data,
            &[STEP],
            None,
            SCOPE,
            Some(&f.cfg),
            Duration::from_millis(200),
        );
        assert_eq!(refused, Err(Refusal::LockUnavailable));
        // What the caller records for a lock expiry.
        update_journal(Some(&f.cfg), |j| {
            let record = StepRecord {
                reason: Some("lock unavailable".into()),
                ..StepRecord::new(&f.data, FROM, TO, StepState::Refused)
            };
            j.record_step(SCOPE, record)
        })
        .expect("record refusal");
        assert_eq!(get(&f.data, TO), None);
        assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, StepState::Refused);
        drop(holder);

        assert_eq!(
            run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
            vec![Outcome::Renamed]
        );
        assert_eq!(get(&f.data, TO), Some(b"old".to_vec()));
        assert_complete(&f.cfg, SCOPE);
    }

    // -- E8, E9, E10: a crash at each of the three gaps ----------------------

    /// Runs the scope on a thread armed at `stage` and abandons it there: the
    /// release sender is dropped, the paused thread panics and its locks drop,
    /// which is what a crash leaves behind.
    fn crash_at(stage: &str, f: &Fixture, steps: &'static [Rename]) {
        let armed = pause::arm(stage, &f.data);
        let (data, cfg) = (f.data.clone(), f.cfg.clone());
        let worker = std::thread::spawn(move || run_scope(&data, steps, SCOPE, Some(&cfg)));
        armed
            .reached
            .recv_timeout(WAIT_BOUND)
            .expect("pause reached");
        drop(armed.release);
        assert!(worker.join().is_err(), "the abandoned leg must not finish");
    }

    #[test]
    fn crash_before_the_rename_resumes_and_completes() {
        let f = fixture();
        put(&f.data, FROM, b"old");
        crash_at("before_rename", &f, &ONE_STEP);
        assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, StepState::Renaming);
        assert_eq!(get(&f.data, FROM), Some(b"old".to_vec()));
        assert_eq!(
            run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
            vec![Outcome::Renamed]
        );
        assert_eq!(get(&f.data, TO), Some(b"old".to_vec()));
        assert_complete(&f.cfg, SCOPE);
    }

    #[test]
    fn crash_after_the_rename_is_not_read_as_a_pre_existing_destination() {
        let f = fixture();
        put(&f.data, FROM, b"old");
        crash_at("after_rename", &f, &ONE_STEP);
        assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, StepState::Renaming);
        assert_eq!(get(&f.data, FROM), None);
        let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
        assert_eq!(
            outcomes,
            vec![Outcome::AlreadyDone],
            "the crash was misclassified"
        );
        assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, StepState::Renamed);
        assert_eq!(get(&f.data, TO), Some(b"old".to_vec()));
        assert_eq!(get(&f.data, &aside(1)), None);
        assert_complete(&f.cfg, SCOPE);
    }

    #[test]
    fn crash_before_the_scope_completes_replays_nothing() {
        let f = fixture();
        seed_many(&f.data, "t");
        crash_at("before_scope_complete", &f, &MANY);
        assert_eq!(
            status(&journal(&f.cfg), SCOPE),
            Some(ScopeStatus::InProgress)
        );
        assert!(steps_of(&f.cfg, SCOPE)
            .iter()
            .all(|s| s.state == StepState::Renamed));
        let before = snapshot(&f.data);
        let outcomes = run_scope(&f.data, &MANY, SCOPE, Some(&f.cfg));
        assert!(
            outcomes.iter().all(|o| *o == Outcome::AlreadyDone),
            "{outcomes:?}"
        );
        assert_eq!(snapshot(&f.data), before);
        assert_many_complete(&f.cfg, &f.data, SCOPE, "t");
    }

    // -- E11, E12: two scopes, one journal -----------------------------------

    fn two_scope_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("cfg");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        for dir in [&cfg, &a, &b] {
            std::fs::create_dir_all(dir).expect("dir");
        }
        seed_many(&a, "a");
        seed_many(&b, "b");
        (tmp, cfg, a, b)
    }

    fn spawn_scope_child(
        cfg: &Path,
        data: &Path,
        scope: &str,
        rendezvous: &Path,
        stage: &str,
    ) -> Child {
        spawn_child(
            "scope",
            &[
                (CHILD_DATA_ENV, data.as_os_str().to_owned()),
                (CHILD_JOURNAL_ENV, cfg.as_os_str().to_owned()),
                (CHILD_SCOPE_ENV, scope.into()),
                (pause::PAUSE_DIR_ENV, rendezvous.as_os_str().to_owned()),
                (pause::PAUSE_STAGE_ENV, stage.into()),
            ],
        )
    }

    #[test]
    fn two_scopes_cannot_lose_each_others_records() {
        // Two threads, two data locks, one journal.
        let (_tmp, cfg, a, b) = two_scope_fixture();
        let threads: Vec<_> = [(a.clone(), "scope-a"), (b.clone(), "scope-b")]
            .into_iter()
            .map(|(data, scope)| {
                let cfg = cfg.clone();
                std::thread::spawn(move || run_scope(&data, &MANY, scope, Some(&cfg)))
            })
            .collect();
        for thread in threads {
            thread.join().expect("scope thread");
        }
        assert_many_complete(&cfg, &a, "scope-a", "a");
        assert_many_complete(&cfg, &b, "scope-b", "b");

        // Two real processes: the child stops after its first `Renaming`
        // commit while the parent commits a whole scope, then it finishes.
        let (tmp, cfg, a, b) = two_scope_fixture();
        let rendezvous = tmp.path().join("rendezvous");
        std::fs::create_dir_all(&rendezvous).expect("rendezvous");
        let child = spawn_scope_child(&cfg, &b, "scope-b", &rendezvous, "before_rename");
        wait_for_file(&rendezvous.join(pause::READY_FILE));
        run_scope(&a, &MANY, "scope-a", Some(&cfg));
        std::fs::write(rendezvous.join(pause::RELEASE_FILE), b"go").expect("release");
        wait_child(child);
        assert_many_complete(&cfg, &a, "scope-a", "a");
        assert_many_complete(&cfg, &b, "scope-b", "b");
    }

    #[test]
    fn a_commit_survives_a_crash_in_the_other_scope() {
        let (tmp, cfg, a, b) = two_scope_fixture();
        let rendezvous = tmp.path().join("rendezvous");
        std::fs::create_dir_all(&rendezvous).expect("rendezvous");
        let mut child = spawn_scope_child(&cfg, &b, "scope-b", &rendezvous, "before_rename");
        wait_for_file(&rendezvous.join(pause::READY_FILE));
        // Killed between its `Renaming` commit and its rename.
        child.kill().expect("kill child");
        child.wait().expect("reap child");

        run_scope(&a, &MANY, "scope-a", Some(&cfg));
        assert_many_complete(&cfg, &a, "scope-a", "a");
        let b_steps = steps_of(&cfg, "scope-b");
        assert_eq!(b_steps.len(), 1, "{b_steps:?}");
        assert_eq!(b_steps[0].state, StepState::Renaming);
        assert_eq!(get(&b, MANY[0].from), Some(b"b:legacy-0.json".to_vec()));
    }

    // -- E13, E14: the scope lock --------------------------------------------

    fn spawn_holder(data: &Path, rendezvous: &Path) -> Child {
        std::fs::create_dir_all(rendezvous).expect("rendezvous");
        let child = spawn_child(
            "hold",
            &[
                (CHILD_DATA_ENV, data.as_os_str().to_owned()),
                (CHILD_RENDEZVOUS_ENV, rendezvous.as_os_str().to_owned()),
            ],
        );
        wait_for_file(&rendezvous.join(pause::READY_FILE));
        child
    }

    #[test]
    fn the_scope_lock_excludes_a_second_process_and_then_expires() {
        let f = fixture();
        put(&f.data, FROM, b"old");

        let rendezvous = f.cfg.join("hold-1");
        let child = spawn_holder(&f.data, &rendezvous);
        let started = Instant::now();
        let refused = run_scope_with(
            &f.data,
            &[STEP],
            None,
            SCOPE,
            Some(&f.cfg),
            MIGRATION_LOCK_BUDGET,
        );
        let waited = started.elapsed();
        assert_eq!(
            refused,
            Err(Refusal::LockUnavailable),
            "the lock did not exclude"
        );
        assert!(waited >= MIGRATION_LOCK_BUDGET, "gave up after {waited:?}");
        assert_eq!(get(&f.data, FROM), Some(b"old".to_vec()));
        assert_eq!(get(&f.data, TO), None);
        std::fs::write(rendezvous.join(pause::RELEASE_FILE), b"go").expect("release");
        wait_child(child);

        let rendezvous = f.cfg.join("hold-2");
        let child = spawn_holder(&f.data, &rendezvous);
        let release = rendezvous.join(pause::RELEASE_FILE);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            std::fs::write(release, b"go").expect("release");
        });
        let started = Instant::now();
        let outcomes = run_scope_with(
            &f.data,
            &[STEP],
            None,
            SCOPE,
            Some(&f.cfg),
            MIGRATION_LOCK_BUDGET,
        )
        .expect("proceeds once released");
        assert!(started.elapsed() < MIGRATION_LOCK_BUDGET);
        assert_eq!(outcomes, vec![Outcome::Renamed]);
        releaser.join().expect("releaser");
        wait_child(child);
    }

    #[test]
    fn lock_scope_under_held_does_not_re_acquire_the_callers_handle() {
        let f = fixture();
        let own = open_sidecar(&f.data.join(NEW_LOCK)).expect("open own lock");
        own.lock().expect("caller's own lock");
        let started = Instant::now();
        let held = lock_scope_under_held(&f.data, Some(OLD_LOCK), MIGRATION_LOCK_BUDGET)
            .expect("under-held lock");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "it waited on the caller's handle"
        );
        assert_eq!(held._files.len(), 1, "it took more than the old sidecar");
        let probe = open_sidecar(&f.data.join(OLD_LOCK)).expect("probe old");
        assert!(
            matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
            "the old sidecar is not held"
        );
        drop(held);
        probe.try_lock().expect("released with the guard");
    }

    // -- E15 -----------------------------------------------------------------

    #[test]
    fn retire_ignore_pairs_is_byte_exact() {
        let content = "keep\r\n\r\n# AC: old rule\r\n/legacy.json\r\n/legacy.json\r\nuser\r\n\n# AC: other\n/other.tmp\ntail";
        let retired = [
            ("# AC: old rule", "/legacy.json"),
            ("# AC: other", "/other.tmp"),
        ];
        let (body, changed) = retire_ignore_pairs(content, &retired);
        assert!(changed);
        assert_eq!(body, "keep\r\n/legacy.json\r\nuser\r\ntail");

        let bare = "/legacy.json\n# AC: old rule\n";
        assert_eq!(
            retire_ignore_pairs(bare, &retired),
            (bare.to_string(), false)
        );
    }

    // -- E19, E20, E21 -------------------------------------------------------

    #[test]
    fn a_complete_scope_is_re_run_when_the_old_name_is_back() {
        for both in [false, true] {
            let f = fixture();
            put(&f.data, FROM, b"old");
            if both {
                put(&f.data, TO, b"new");
            }
            update_journal(Some(&f.cfg), |j| {
                j.record_step(
                    SCOPE,
                    StepRecord::new(&f.data, FROM, TO, StepState::Renamed),
                );
                j.set_status(SCOPE, ScopeStatus::Complete);
            })
            .expect("seed a stale Complete");
            assert!(is_complete(&journal(&f.cfg), SCOPE));

            let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
            if both {
                assert_eq!(
                    outcomes,
                    vec![Outcome::SetAside(aside(1))],
                    "a stale Complete was skipped"
                );
                assert_eq!(get(&f.data, TO), Some(b"new".to_vec()));
                assert_eq!(get(&f.data, &aside(1)), Some(b"old".to_vec()));
            } else {
                assert_eq!(
                    outcomes,
                    vec![Outcome::Renamed],
                    "a stale Complete was skipped"
                );
                assert_eq!(get(&f.data, TO), Some(b"old".to_vec()));
            }
            assert_eq!(get(&f.data, FROM), None);
            assert_complete(&f.cfg, SCOPE);
        }
    }

    fn files_under(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(root).expect("list") {
            let entry = entry.expect("entry");
            if entry.file_type().expect("type").is_dir() {
                found.extend(files_under(&entry.path()));
            } else {
                found.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        found
    }

    #[test]
    fn a_journal_less_run_decides_every_step_from_the_disk() {
        for disk in [
            Disk::FromOnly,
            Disk::ToOnly,
            Disk::Neither,
            Disk::Both,
            Disk::BothWithAside,
        ] {
            let with = fixture();
            let without = tempfile::tempdir().expect("tempdir");
            seed(&with.data, disk);
            seed(without.path(), disk);
            let journalled = run_scope(&with.data, &[STEP], SCOPE, Some(&with.cfg));
            let journal_less = run_scope(without.path(), &[STEP], SCOPE, None);
            assert_eq!(journalled, journal_less, "{disk:?}");
            assert_eq!(snapshot(&with.data), snapshot(without.path()), "{disk:?}");
            let stray: Vec<String> = files_under(without.path())
                .into_iter()
                .filter(|name| name.contains("naming-migration"))
                .collect();
            assert!(stray.is_empty(), "the journal-less run wrote {stray:?}");
        }
    }

    #[test]
    fn a_repeated_step_record_is_replaced_not_appended() {
        let f = fixture();
        put(&f.data, FROM, b"old-0");
        put(&f.data, TO, b"new");
        let journal_len = || {
            std::fs::metadata(f.cfg.join(NAMING_MIGRATION_STATE_NAME))
                .expect("journal")
                .len()
        };
        let mut first_at = String::new();
        let mut last_at = String::new();
        let mut len_after_two = 0;
        for cycle in 1..=50u32 {
            assert_eq!(
                run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
                vec![Outcome::SetAside(aside(cycle))]
            );
            let steps = steps_of(&f.cfg, SCOPE);
            assert_eq!(
                steps.len(),
                1,
                "cycle {cycle}: the record was appended, not replaced"
            );
            assert!(steps[0].at >= last_at, "cycle {cycle}: at went backwards");
            last_at = steps[0].at.clone();
            if cycle == 1 {
                first_at = last_at.clone();
            }
            if cycle == 2 {
                len_after_two = journal_len();
            }
            put(&f.data, FROM, format!("old-{cycle}").as_bytes());
        }
        assert!(last_at > first_at, "at never advanced");
        // The only growth allowed is the chosen name's ordinal, `-2` to `-50`.
        let ordinal_growth = ("50".len() - "2".len()) as u64;
        assert_eq!(
            journal_len(),
            len_after_two + ordinal_growth,
            "the journal grew"
        );
        for n in 1..=50 {
            assert_eq!(
                get(&f.data, &aside(n)),
                Some(format!("old-{}", n - 1).into_bytes())
            );
        }
    }
}
