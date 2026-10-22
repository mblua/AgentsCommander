use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::Manager;

use uuid::Uuid;

use crate::config::settings::WindowGeometry;
use crate::session::manager::{
    acquire_carrier_mutation_permit, CarrierDisposition, CarrierMutationPermit, PromptKey,
    RaiseCarrierOutcome, RetainedPrompt, SessionManager,
};
use crate::session::profile::CodingAgentKind;
use crate::session::session::{
    SessionCommunication, SessionInfo, SessionStatus, TEMP_SESSION_PREFIX,
};

/// #291 — in-process mutex serializing all `save_sessions` calls.
///
/// The historical race: two concurrent callers both wrote to the shared
/// `sessions.json.tmp`, then both tried to `rename` it. The first rename
/// consumed the temp file, the second rename returned Windows
/// `ERROR_FILE_NOT_FOUND` (os error 2), and the second caller's snapshot
/// silently failed to persist.
///
/// The fix is twofold:
///   1. This mutex serializes the write+rename window so the order of saves
///      matches the order of `lock()` acquisition. "Last writer wins."
///   2. Per-call unique temp filenames (`sessions.json.<pid>.<op_id>.tmp`)
///      provide defense-in-depth: even if a future caller forgets the lock,
///      two callers can never collide on the same temp filename.
///
/// Lock window: held only across the synchronous serialize + write + rename
/// inside `save_sessions_to_dir` (no await points). Worst-case contention
/// per waiter is bounded by `RENAME_ATTEMPTS * max(RENAME_BACKOFFS_MS)`
/// (~260 ms), same as the existing rename window.
///
/// Poisoning is tolerated via `into_inner()`: a poisoned lock means a prior
/// holder panicked mid-write, but the persisted file is atomic (tmp+rename),
/// so picking up the lock cleanly is safe.
static SAVE_SESSIONS_LOCK: Mutex<()> = Mutex::new(());

/// The accepted core owns completion independently of its caller. A lost task
/// outcome is ambiguous; it never becomes a successful save or a retry request.
enum PersistOperationState<T> {
    Running,
    Completed(Option<T>),
    Ambiguous,
}

struct PersistOperationCell<T> {
    state: Mutex<PersistOperationState<T>>,
    changed: tokio::sync::Notify,
}

struct PersistOperationOwner<T> {
    cell: std::sync::Arc<PersistOperationCell<T>>,
    completed: bool,
}

impl<T> PersistOperationOwner<T> {
    fn complete(mut self, result: T) {
        {
            match self.cell.state.lock() {
                Ok(mut state) => *state = PersistOperationState::Completed(Some(result)),
                Err(error) => {
                    *error.into_inner() = PersistOperationState::Ambiguous;
                    // Poison cannot turn an untrusted cell into observed completion.
                    drop(result);
                }
            }
        }
        self.completed = true;
        self.cell.changed.notify_one();
    }
}

impl<T> Drop for PersistOperationOwner<T> {
    fn drop(&mut self) {
        if !self.completed {
            let mut state = self
                .cell
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *state = PersistOperationState::Ambiguous;
            drop(state);
            self.cell.changed.notify_one();
        }
    }
}

pub(crate) async fn run_owned_persist_core<T, F>(operation: F) -> T
where
    T: Send + 'static,
    F: std::future::Future<Output = T> + Send + 'static,
{
    let cell = std::sync::Arc::new(PersistOperationCell {
        state: Mutex::new(PersistOperationState::Running),
        changed: tokio::sync::Notify::new(),
    });
    let owner = PersistOperationOwner {
        cell: cell.clone(),
        completed: false,
    };
    tokio::spawn(async move {
        let result = operation.await;
        owner.complete(result);
    });
    loop {
        let changed = cell.changed.notified();
        let result = {
            match cell.state.lock() {
                Ok(mut state) => match &mut *state {
                    PersistOperationState::Completed(result) => result.take(),
                    PersistOperationState::Running | PersistOperationState::Ambiguous => None,
                },
                Err(_) => None,
            }
        };
        if let Some(result) = result {
            return result;
        }
        changed.await;
    }
}

const RETAINED_PROMPTS: &str = super::instance_artifacts::RETAINED_PROMPTS;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PromptKeyWire {
    canonical_cwd: String,
    agent_id: String,
    requested_profile: Option<String>,
    effective_profile: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetainedPromptWire {
    key: PromptKeyWire,
    last_prompt: Option<String>,
    blocked: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedPromptsWire {
    version: u32,
    entries: Vec<RetainedPromptWire>,
}

/// Host identity only. Failed canonicalization never falls back to path text.
pub(crate) fn prompt_key(
    cwd: &str,
    agent_id: Option<&str>,
    requested_profile: Option<&str>,
    effective_profile: Option<&str>,
) -> Option<PromptKey> {
    let agent_id = agent_id.filter(|value| !value.is_empty())?;
    let effective_profile = effective_profile.filter(|value| !value.is_empty())?;
    let canonical = std::fs::canonicalize(cwd).ok()?;
    if !canonical.is_dir() {
        return None;
    }
    let mut canonical_cwd = strip_long_prefix_str(canonical.to_str()?).to_string();
    if cfg!(windows) {
        canonical_cwd.make_ascii_lowercase();
    }
    Some(PromptKey {
        canonical_cwd,
        agent_id: agent_id.to_string(),
        requested_profile: requested_profile.map(str::to_string),
        effective_profile: effective_profile.to_string(),
    })
}

fn load_retained_prompts(dir: &Path) -> Result<HashMap<PromptKey, RetainedPrompt>, ()> {
    let bytes = match std::fs::read(dir.join(RETAINED_PROMPTS)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(_) => return Err(()),
    };
    let wire: RetainedPromptsWire = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if wire.version != 1 {
        return Err(());
    }
    let mut entries = HashMap::new();
    for row in wire.entries {
        let key = PromptKey {
            canonical_cwd: row.key.canonical_cwd,
            agent_id: row.key.agent_id,
            requested_profile: row.key.requested_profile,
            effective_profile: row.key.effective_profile,
        };
        if let Some(previous) = entries.get_mut(&key) {
            let previous: &mut RetainedPrompt = previous;
            previous.last_prompt = None;
            previous.blocked = true;
        } else {
            entries.insert(
                key,
                RetainedPrompt {
                    last_prompt: row.last_prompt,
                    blocked: row.blocked,
                },
            );
        }
    }
    Ok(entries)
}

#[cfg(not(test))]
pub(crate) async fn initialize_prompt_retention(mgr: &SessionManager) {
    if let Some(dir) = super::config_dir() {
        initialize_prompt_retention_in_dir(mgr, &dir).await;
    } else {
        mgr.prompt_initialization
            .get_or_init(|| async {
                mgr.install_retained_prompts(None).await;
            })
            .await;
    }
}

#[cfg(test)]
pub(crate) async fn initialize_prompt_retention(mgr: &SessionManager) {
    // Lifecycle mocks never consult the developer's real instance directory.
    // IO regressions call the explicit directory initializer below.
    mgr.prompt_initialization
        .get_or_init(|| async {
            mgr.install_retained_prompts(Some(HashMap::new())).await;
        })
        .await;
}

pub(crate) async fn initialize_prompt_retention_in_dir(mgr: &SessionManager, dir: &Path) {
    mgr.prompt_initialization
        .get_or_init(|| async {
            let entries = load_retained_prompts(dir);
            if entries.is_err() {
                log::warn!("[last-prompt] cache unavailable; preserving existing checkpoint");
            }
            mgr.install_retained_prompts(entries.ok()).await;
        })
        .await;
}

/// Cold restore admits rows sequentially, so inspect its complete recipe set
/// before the first cache lookup. This creates no session associations.
pub(crate) async fn block_duplicate_restore_prompt_domains(
    mgr: &SessionManager,
    rows: &[PersistedSession],
) {
    let mut counts = HashMap::new();
    for row in rows {
        if let Some(key) = prompt_key(
            &row.working_directory,
            row.agent_id.as_deref(),
            row.requested_profile.as_deref(),
            row.effective_profile.as_deref(),
        ) {
            *counts.entry(key).or_insert(0usize) += 1;
        }
    }
    mgr.block_retained_prompt_domains(
        counts
            .into_iter()
            .filter_map(|(key, count)| (count > 1).then_some(key))
            .collect(),
    )
    .await;
}

/// A Windows comparison spelling has lost its canonical case, so NotFound on
/// that spelling cannot prove deletion under per-directory case sensitivity.
fn prompt_directory_gc_confirmed(windows: bool, metadata: Result<(), std::io::ErrorKind>) -> bool {
    !windows && metadata == Err(std::io::ErrorKind::NotFound)
}

/// Called only after sessions_save_lock; always reads the current manager.
async fn flush_retained_prompts_locked(mgr: &SessionManager, dir: &Path) -> Result<(), String> {
    flush_retained_prompts_locked_with_probe(mgr, dir, cfg!(windows), |path| {
        std::fs::metadata(path)
            .map(|_| ())
            .map_err(|error| error.kind())
    })
    .await
}

async fn flush_retained_prompts_locked_with_probe(
    mgr: &SessionManager,
    dir: &Path,
    windows: bool,
    metadata: impl Fn(&str) -> Result<(), std::io::ErrorKind>,
) -> Result<(), String> {
    let Some(snapshot) = mgr.retained_prompt_snapshot().await else {
        return Ok(());
    };
    let mut entries = Vec::new();
    for (key, value) in snapshot {
        // A folded Windows key is never a deletion witness. Other platforms
        // still collect confirmed missing paths; transient failures keep bytes.
        if prompt_directory_gc_confirmed(windows, metadata(&key.canonical_cwd)) {
            mgr.forget_retained_prompt(&key).await;
            continue;
        }
        entries.push(RetainedPromptWire {
            key: PromptKeyWire {
                canonical_cwd: key.canonical_cwd,
                agent_id: key.agent_id,
                requested_profile: key.requested_profile,
                effective_profile: key.effective_profile,
            },
            last_prompt: value.last_prompt,
            blocked: value.blocked,
        });
    }
    entries.sort_by(|a, b| {
        (
            &a.key.canonical_cwd,
            &a.key.agent_id,
            &a.key.requested_profile,
            &a.key.effective_profile,
        )
            .cmp(&(
                &b.key.canonical_cwd,
                &b.key.agent_id,
                &b.key.requested_profile,
                &b.key.effective_profile,
            ))
    });
    static PROMPT_OP: AtomicU64 = AtomicU64::new(0);
    let temp = dir.join(format!(
        "{RETAINED_PROMPTS}.{}.{}.tmp",
        std::process::id(),
        PROMPT_OP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let bytes = serde_json::to_vec(&RetainedPromptsWire {
            version: 1,
            entries,
        })
        .map_err(|_| "last-prompt serialization failed".to_string())?;
        std::fs::write(&temp, bytes).map_err(|_| "last-prompt write failed".to_string())?;
        rename_with_retry(&temp, &dir.join(RETAINED_PROMPTS))
            .map_err(|_| "last-prompt rename failed".to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
        mgr.disable_prompt_reuse().await;
        log::warn!("[last-prompt] checkpoint failed; cache reuse disabled");
    }
    result
}

/// #291 — counter feeding the per-call unique temp filename for
/// `save_sessions`. Combined with the PID it makes the temp filename
/// `sessions.json.<pid>.<op_id>.tmp` distinct from any concurrent in-process
/// or cross-process save, and from any leftover temp file written by a
/// prior crashed run. Kept separate from `rename_with_retry`'s `OP_ID` so
/// the two counters can be reasoned about independently in diagnostics.
static SAVE_OP_ID: AtomicU64 = AtomicU64::new(0);

/// §1295 — orphaned-sessions archive store (mechanism C / B1). See §5.4.
/// NDJSON (one JSON object per line), append-only, never parsed back by any
/// version (old AC versions ignore it). The EXISTS probe at drop time decides
/// the disposition label, never whether the recipe is kept.
pub(crate) const ORPHAN_ARCHIVE_FILENAME: &str =
    crate::config::instance_artifacts::ORPHAN_ARCHIVE_FILENAME;

/// §1295 — soft cap on the ACTIVE archive file before best-effort rotation.
const ORPHAN_ARCHIVE_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// §1295 — number of rotated archive generations kept. Mirrors `logging.rs`
/// `APP_LOG_KEEP` semantics: the active file, then `.1` .. `.KEEP - 1`; the
/// oldest `.KEEP` copy is dropped on the next rotation.
const ORPHAN_ARCHIVE_KEEP: u32 = 3;

/// §1295 — per-process-run dedup registry for orphan WARNs, keyed by
/// normalized cwd (5.7 / N1 resolution). The first sighting of a cwd this run
/// logs at WARN; later sightings of the SAME cwd log at DEBUG. Never cleared
/// mid-run: the run-level semantic is exactly constraint 3 ("same orphan
/// resolved once; no repeated WARN every 25 s"). Each AC process keeps its own
/// registry, so multi-instance setups warn at most once per orphan per run.
/// Tests reset it via `#[cfg(test)]` helpers.
fn orphan_warned_registry() -> &'static Mutex<HashSet<String>> {
    static REGISTRY: std::sync::OnceLock<Mutex<HashSet<String>>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// §1295 — shared orphan-sweep WARN dedup + counters. Returns true on the
/// FIRST sighting of `cwd` this run (caller should have WARNed), false on
/// repeats (caller should DEBUG instead). Used by all three drop sites (A, B
/// hot + merge, C) so a persist storm collapses to one WARN per orphan.
fn note_orphan_cwd(cwd: &str) -> bool {
    let key = normalized_cwd_key(cwd);
    let mut registry = orphan_warned_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if registry.insert(key) {
        #[cfg(test)]
        bump_orphan_warned();
        log::warn!(
            "[sessions] Dropping orphan persisted session at '{}' (outside current projectPaths); one WARN per orphan per run",
            cwd
        );
        true
    } else {
        log::debug!(
            "[sessions] Orphan session at '{}' already warned this run; suppressing repeat WARN",
            cwd
        );
        false
    }
}

/// §1295 5.4/5.7 — per-process-run B1-once registry for ARCHIVED orphan
/// session ids. The FIRST sighting of a dropped session id this run writes the
/// archive record; every later sighting (e.g. a no-prune site archives it, then
/// the steady prune site removes it from RAM) is suppressed so no recipe is
/// appended twice. A row that is still LIVE is never archived at all, so this
/// registry tracks only ids that were actually archived.
fn orphan_archived_registry() -> &'static Mutex<HashSet<Uuid>> {
    static REGISTRY: std::sync::OnceLock<Mutex<HashSet<Uuid>>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Returns true only on the FIRST archive of `uuid` this process run.
fn note_orphan_archived_id(uuid: &Uuid) -> bool {
    orphan_archived_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(*uuid)
}

/// §1295 — clear the B1-once archive registry. Test-only.
#[cfg(test)]
pub(crate) fn reset_orphan_archived() {
    orphan_archived_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// §1295 — sweep outcome counters. Production reads them for the summary log
/// line; `#[cfg(test)]` routes them through accumulating statics for assertions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OrphanSweepCounts {
    /// Drops recorded with disposition "archived" (cwd exists at drop time).
    pub archived: usize,
    /// Drops recorded with disposition "reaped" (cwd missing at drop time).
    pub reaped: usize,
    /// Orphan rows that stayed live in RAM (non-Exited or pending) and were
    /// dropped only from the disk snapshot.
    pub live_kept: usize,
    /// Repeat WARNs suppressed this sweep because the cwd was already warned.
    pub suppressed_repeat: usize,
}

// §1295 — accumulating sweep counters, used ONLY by `#[cfg(test)]`. These are
// THREAD-LOCAL (mirroring the `NORMALIZE_CALLS` pattern) so many persistence
// tests can run in parallel in one process without one test's purge sneaking
// `reaped++` into another test's assertion. Each `#[tokio::test]` body (and
// the awaited persist/purge it drives) runs on one thread, so a test resets,
// drives, and reads its own thread's counters deterministically.
#[cfg(test)]
thread_local! {
    static ORPHAN_SWEEP_COUNTERS: std::cell::Cell<OrphanSweepCounts> = const {
        std::cell::Cell::new(OrphanSweepCounts {
            archived: 0,
            reaped: 0,
            live_kept: 0,
            suppressed_repeat: 0,
        })
    };
    static ORPHAN_WARNED_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_orphan_counters() {
    ORPHAN_SWEEP_COUNTERS.with(|slot| slot.set(OrphanSweepCounts::default()));
}

#[cfg(test)]
pub(crate) fn orphan_counters() -> OrphanSweepCounts {
    ORPHAN_SWEEP_COUNTERS.with(|slot| slot.get())
}

#[cfg(test)]
pub(crate) fn reset_orphan_warned() {
    ORPHAN_WARNED_COUNT.with(|count| count.set(0));
    orphan_warned_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// §1295 — number of distinct orphan cwds warned on the CURRENT test thread
/// (`note_orphan_cwd` bumps the thread-local counter on each first sighting).
#[cfg(test)]
pub(crate) fn orphan_warned_len() -> usize {
    ORPHAN_WARNED_COUNT.with(|count| count.get())
}

#[cfg(test)]
fn bump_orphan_warned() {
    ORPHAN_WARNED_COUNT.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
fn accumulate_orphan_counters(counts: OrphanSweepCounts) {
    ORPHAN_SWEEP_COUNTERS.with(|slot| {
        let current = slot.get();
        slot.set(OrphanSweepCounts {
            archived: current.archived + counts.archived,
            reaped: current.reaped + counts.reaped,
            live_kept: current.live_kept + counts.live_kept,
            suppressed_repeat: current.suppressed_repeat + counts.suppressed_repeat,
        });
    });
}

/// §1295 — test-only soft-cap override so a test can force rotation with a
/// tiny cap (test 8). Process-global but only mutated under `ORPHAN_TEST_LOCK`,
/// which serializes every archive-sweep counter test.
#[cfg(test)]
static TEST_ORPHAN_ARCHIVE_CAP: std::sync::OnceLock<Mutex<Option<u64>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) fn set_test_orphan_archive_cap(cap: Option<u64>) {
    let slot = TEST_ORPHAN_ARCHIVE_CAP.get_or_init(|| Mutex::new(None));
    *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = cap;
}

fn orphan_archive_max_bytes() -> u64 {
    #[cfg(test)]
    {
        let slot = TEST_ORPHAN_ARCHIVE_CAP.get_or_init(|| Mutex::new(None));
        if let Some(cap) = *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) {
            return cap;
        }
    }
    ORPHAN_ARCHIVE_MAX_BYTES
}

/// #280 §3.1 — diagnostic context captured when an atomic rename exhausts
/// its retry budget. Surfaced in the caller's error string so a single
/// ERROR line carries enough state to investigate the AV / Indexer / second
/// instance contention pattern.
#[derive(Debug)]
pub(crate) struct RenameDiagnostics {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    op_id: u64,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    pid: u32,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    instance_id: String,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    tmp_exists_before: bool,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    final_exists_before: bool,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    attempts: u32,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    last_os_error: Option<i32>,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    duration: std::time::Duration,
}

/// #280 §3.1 — number of `std::fs::rename` attempts. With the
/// `BACKOFFS_MS = [10, 50, 200]` schedule below, this gives a worst-case
/// 260 ms blocking window before surfacing the error. G-MED-2 fix:
/// 4 attempts so all three backoff entries are actually used (with
/// `ATTEMPTS = 3` the 200 ms entry would be dead code).
const RENAME_ATTEMPTS: u32 = 4;

/// #280 §3.1 — backoff schedule between rename attempts in milliseconds.
/// Tuned for Windows AV / Indexer holds which typically clear within
/// ~50 ms but occasionally take >100 ms on cold caches. The terminal
/// attempt has no backoff (we already failed `RENAME_ATTEMPTS - 1` times).
const RENAME_BACKOFFS_MS: [u64; 3] = [10, 50, 200];

/// Atomic rename with bounded retries. Returns the diagnostic context so
/// the caller can fold it into a single ERROR line. Successful retries are
/// logged at INFO (low-frequency, useful signal that the race is
/// happening). Intermediate failures log at DEBUG.
///
/// This retry loop targets *cross-process* contention on the destination
/// (`sessions.json`): AV scanners, the Windows Indexer, or a second AC
/// instance briefly holding the file. It is NOT the mitigation for #291,
/// which was an in-process race on a shared *temp* filename — that one is
/// solved upstream in `save_sessions_to_dir` by a per-call unique temp
/// name plus `SAVE_SESSIONS_LOCK`, so by the time we get here the source
/// path is guaranteed unique to this save.
///
/// Risk note (deliberately accepted for #280 scope): this uses
/// `std::thread::sleep`, which blocks the calling tokio worker thread for
/// up to `sum(RENAME_BACKOFFS_MS)` = 260 ms during a contended rename.
/// `save_sessions` is invoked from async Tauri command handlers, but the
/// surrounding code (`std::fs::write`, `std::fs::rename`) is already sync,
/// so this does not introduce a new class of blocking — only enlarges an
/// existing one. The clean fix (`tokio::task::spawn_blocking` for the
/// whole persistence block) is a wider signature/caller refactor and is
/// out of scope for #280 (observability hardening, not concurrency
/// rework). File a follow-up if the 260 ms tail latency becomes
/// user-perceptible.
pub(crate) fn rename_with_retry(tmp: &Path, dst: &Path) -> Result<(), (String, RenameDiagnostics)> {
    static OP_ID: AtomicU64 = AtomicU64::new(0);
    let op_id = OP_ID.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let instance_id = crate::config::agent_local_dir_name();
    let start = std::time::Instant::now();
    let tmp_exists_before = tmp.exists();
    let final_exists_before = dst.exists();

    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..RENAME_ATTEMPTS {
        match std::fs::rename(tmp, dst) {
            Ok(()) => {
                if attempt > 0 {
                    log::info!(
                        "[sessions] rename succeeded after retry — op_id={} pid={} instance={} attempt={}/{} duration={:?}",
                        op_id,
                        pid,
                        instance_id,
                        attempt + 1,
                        RENAME_ATTEMPTS,
                        start.elapsed()
                    );
                }
                return Ok(());
            }
            Err(e) => {
                log::debug!(
                    "[sessions] rename attempt {}/{} failed — op_id={} pid={} os_error={:?} kind={:?}",
                    attempt + 1,
                    RENAME_ATTEMPTS,
                    op_id,
                    pid,
                    e.raw_os_error(),
                    e.kind()
                );
                last_err = Some(e);
                let backoff_idx = attempt as usize;
                if backoff_idx < RENAME_BACKOFFS_MS.len() {
                    std::thread::sleep(std::time::Duration::from_millis(
                        RENAME_BACKOFFS_MS[backoff_idx],
                    ));
                }
            }
        }
    }

    let e = last_err.expect("RENAME_ATTEMPTS >= 1, so the loop runs at least once");
    let diag = RenameDiagnostics {
        op_id,
        pid,
        instance_id,
        tmp_exists_before,
        final_exists_before,
        attempts: RENAME_ATTEMPTS,
        last_os_error: e.raw_os_error(),
        duration: start.elapsed(),
    };
    Err((e.to_string(), diag))
}

/// Minimal session data needed to restore a session on next app start.
/// No UUID, just the "recipe" to re-create it.
///
/// The optional runtime fields (id, waiting_for_input, communication,
/// created_at) are populated during live snapshots so the CLI can read session
/// state from the file without requiring an HTTP request. They are ignored on
/// restore.
///
/// `status` is also populated during live snapshots for CLI consumption AND is
/// now **consumed on restore** by the issue #248 startup wake policy: the
/// restore-task closure in `lib.rs` reads `status` to decide whether a
/// coordinator should be auto-woken (was awake at shutdown) or left dormant
/// (was asleep at shutdown). See `should_wake_on_restore` in `lib.rs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_agent_binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_agent_binding_seed: Option<CodingAgentBindingSeed>,
    /// Original byte witness transported in RAM, never persisted.
    #[serde(skip)]
    pub coding_agent_binding_origin: Option<serde_json::Value>,
    #[serde(skip)]
    pub coding_agent_binding_conflict: bool,
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub working_directory: String,
    /// True for the session that was active when the app closed
    #[serde(default)]
    pub was_active: bool,
    /// Authoritative repo list. Empty = no repo badge rendered.
    #[serde(default)]
    pub git_repos: Vec<crate::session::session::SessionRepo>,
    /// Recomputed on restore; persisted for forward-compat only.
    #[serde(default)]
    pub is_coordinator: bool,
    /// True for the global Root Agent session. Defaults false for old sessions.json.
    #[serde(default)]
    pub is_root_agent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
    /// Effective profile letter the session actually spawned with (after the
    /// cell fallback walk). Absent for plain-shell sessions and for rows
    /// written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_profile: Option<String>,
    /// Telegram bot id that was ON for this session at the last successful
    /// bridge attach. None means Telegram was OFF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot_id: Option<String>,

    /// True if the session was detached into its own window at snapshot time.
    /// Phase 3 restore re-spawns a detached window for every persisted row with
    /// `was_detached=true` (except deferred sessions — see plan §R.9). Sourced
    /// from `Session::was_detached` under Fix A — NOT from `DetachedSessionsState`.
    #[serde(default)]
    pub was_detached: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,

    /// Last-known geometry of this session's detached window. `None` for sessions
    /// that were never detached, or detached without any drag/resize yet. Auto-GC'd
    /// when the session is destroyed (field travels with the PersistedSession row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_geometry: Option<WindowGeometry>,

    /// (#630/#631) Durable resume intent. `true` => start fresh on restore
    /// ("Restart Session"); `false` => resume. `#[serde(default)]` so pre-existing
    /// records deserialize to `false` = resume (polarity is deliberate: the safe
    /// serde/Default value must mean "resume"). No `skip_serializing_if`: always
    /// written so the on-disk record is explicit after the first save.
    #[serde(default)]
    pub start_fresh_on_restore: bool,

    // ── Legacy fields — read-only, consumed by the upgrade pass in load_sessions. ──
    // `skip_serializing_if = "Option::is_none"` means snapshot_sessions never writes them
    // back, and the first save after upgrade retires them from disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_prefix: Option<String>,

    // ── Runtime fields (populated during live snapshots; `status` consumed on
    //    restore per issue #248 and `communication` consumed on restore per
    //    issue #747, the others ignored on restore) ──
    /// Session UUID (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Current session status. Populated during live snapshots; **consumed on
    /// restore** by the issue #248 startup wake policy (see
    /// `should_wake_on_restore` in `lib.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
    /// Whether the session is waiting for user input (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for_input: Option<bool>,
    /// Current visible session communication state. Populated during live
    /// snapshots; **consumed on restore** since issue #747 (a persisted raised
    /// hand re-applies to the restored record), and preserved on
    /// failed-recoverable rows by `sanitize_failed_recoverable` so the
    /// next-startup retry can restore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub communication: Option<SessionCommunication>,
    /// ISO 8601 creation timestamp (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Live context-usage percent (0-100), the same figure the Sidebar badge
    /// shows. Populated during live snapshots so the CLI (`list-peers` /
    /// `list-peers-lean`) can read it from disk without a running daemon;
    /// ignored on restore. `None` (absent key) means no reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<u8>,
}

fn sessions_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("sessions.json"))
}

fn strip_long_prefix_str(s: &str) -> String {
    crate::path_utils::normalize_windows_verbatim_path(s)
}

#[cfg(test)]
static FILTER_PROJECT_PATHS_THREAD_IDS: std::sync::OnceLock<
    std::sync::Mutex<Vec<std::thread::ThreadId>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
thread_local! {
    static NORMALIZE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_normalize_call_count() {
    NORMALIZE_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
pub(crate) fn normalize_call_count() -> usize {
    NORMALIZE_CALLS.with(|calls| calls.get())
}

fn normalize_for_project_compare(path: &Path) -> String {
    #[cfg(test)]
    NORMALIZE_CALLS.with(|calls| calls.set(calls.get() + 1));
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut s = strip_long_prefix_str(&path.to_string_lossy()).replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s.make_ascii_lowercase();
    }
    s
}

fn path_is_under_or_equal(candidate: &str, root: &str) -> bool {
    if root.is_empty() {
        return false;
    }
    if candidate == root {
        return true;
    }
    if root == "/" {
        return candidate.starts_with('/');
    }
    candidate.starts_with(&format!("{}/", root))
}

/// Raw-root convenience for active project lists where each candidate session
/// is checked against a short per-command scope, such as `archive_blockers`.
///
/// Do not call this with `archived_project_paths`,
/// `session_retention_project_paths`, or inside a per-dir loop. In those paths,
/// normalize roots once with `normalize_project_roots` and use the normalized
/// helpers.
pub(crate) fn working_directory_under_any_project_path(
    working_directory: &str,
    project_paths: &[String],
) -> bool {
    let cwd = normalize_for_project_compare(Path::new(working_directory));
    let roots = normalize_project_roots(project_paths);
    working_directory_under_any_normalized_root(&cwd, &roots)
}

/// #881: canonicalize a project-root list once. Blocking
/// (`std::fs::canonicalize` per root). Hoist the call out of per-session or
/// per-dir loops, and off the async runtime when the caller is on a poll
/// interval. Empty input yields empty output with zero syscalls.
pub(crate) fn normalize_project_roots(project_paths: &[String]) -> Vec<String> {
    project_paths
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| normalize_for_project_compare(Path::new(p)))
        .collect()
}

fn working_directory_under_any_normalized_root(cwd: &str, roots: &[String]) -> bool {
    roots
        .iter()
        .any(|project| path_is_under_or_equal(cwd, project))
}

/// #881: paths that may retain persisted sessions. Active projects plus archived
/// projects are both registered from the session store's point of view; only
/// active projects should be used for discovery and background project work.
pub(crate) fn session_retention_project_paths(
    settings: &crate::config::settings::AppSettings,
) -> Result<Vec<String>, String> {
    if let Some(message) = &settings.project_paths_persistence_error {
        return Err(message.clone());
    }
    let mut paths = settings.project_paths.clone();
    paths.extend(settings.archived_project_paths.iter().cloned());
    Ok(paths)
}

/// #881: true when `path` lives under one of `normalized_archived_roots`,
/// which the caller produced with `normalize_project_roots` once per poll tick,
/// restore loop, or resolution.
///
/// The empty-list fast path lives here, before `path` itself is canonicalized,
/// so the common case of no archived projects pays zero syscalls.
///
/// Do not add a raw-root convenience wrapper. The per-root canonicalize must
/// not hide inside a per-dir loop.
pub(crate) fn is_under_normalized_archived_roots(
    path: &str,
    normalized_archived_roots: &[String],
) -> bool {
    if normalized_archived_roots.is_empty() {
        return false;
    }
    working_directory_under_any_normalized_root(
        &normalize_for_project_compare(Path::new(path)),
        normalized_archived_roots,
    )
}

/// #881: return the stored raw root containing `path`, while matching on the
/// same normalized form used by `normalize_project_roots`.
pub(crate) fn raw_project_path_containing(path: &str, project_paths: &[String]) -> Option<String> {
    if project_paths.is_empty() {
        return None;
    }
    let cwd = normalize_for_project_compare(Path::new(path));
    project_paths.iter().find_map(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let root = normalize_for_project_compare(Path::new(trimmed));
        if path_is_under_or_equal(&cwd, &root) {
            Some(raw.clone())
        } else {
            None
        }
    })
}

fn is_root_persisted_session(session: &PersistedSession) -> bool {
    session.is_root_agent
        || crate::config::root_agent::is_root_agent_dir_name(&session.working_directory)
}

/// §1295 5.6 — one orphaned session, already classified as a drop by
/// `partition_orphaned_sessions`. `cwd_exists` is filled by the caller (a
/// `Path::exists` probe, one per drop, inside the same blocking chunk) so the
/// partition itself stays pure (zero fs calls).
#[derive(Clone)]
pub(crate) struct OrphanDrop {
    pub session: PersistedSession,
    pub cwd_exists: bool,
}

/// §1295 — a single pass that partitions a session snapshot into the kept set
/// (root-agent keep + under-roots keep, exactly today's filter predicate) and
/// the orphaned drops. Pure: performs ZERO fs calls (S2/B). The `#[cfg(test)]`
/// panic hook and `FILTER_PROJECT_PATHS_THREAD_IDS` thread-id recording survive
/// here so both the keep-set filter and the persist-time prepare inherit them.
pub(crate) fn partition_orphaned_sessions(
    sessions: Vec<PersistedSession>,
    normalized_roots: &[String],
) -> (Vec<PersistedSession>, Vec<OrphanDrop>) {
    #[cfg(test)]
    if sessions
        .iter()
        .any(|session| session.name == "__panic_filter_for_test__")
    {
        panic!("test-only session filter panic");
    }

    let total = sessions.len();
    let mut kept = Vec::with_capacity(total);
    let mut drops = Vec::new();
    for session in sessions {
        if is_root_persisted_session(&session) {
            kept.push(session);
            continue;
        }
        let cwd = normalize_for_project_compare(Path::new(&session.working_directory));
        if working_directory_under_any_normalized_root(&cwd, normalized_roots) {
            kept.push(session);
        } else {
            drops.push(OrphanDrop {
                session,
                cwd_exists: false,
            });
        }
    }
    (kept, drops)
}

fn filter_sessions_for_project_paths(
    sessions: Vec<PersistedSession>,
    project_paths: &[String],
) -> Vec<PersistedSession> {
    let roots = normalize_project_roots(project_paths);
    filter_sessions_for_normalized_roots(sessions, &roots)
}

/// §1295 — keep-set filter (unchanged behavior). It now DELEGATES to
/// `partition_orphaned_sessions`; the per-row orphan WARN (:450) and the INFO
/// summary (:461) moved to the shared dedup layer (`note_orphan_cwd` +
/// `handle_orphan_drops`), which is where drop semantics now live.
fn filter_sessions_for_normalized_roots(
    sessions: Vec<PersistedSession>,
    roots: &[String],
) -> Vec<PersistedSession> {
    partition_orphaned_sessions(sessions, roots).0
}

#[cfg(test)]
fn record_filter_project_paths_thread_id() {
    let slot = FILTER_PROJECT_PATHS_THREAD_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    slot.lock()
        .expect("thread id mutex poisoned")
        .push(std::thread::current().id());
}

async fn filter_sessions_for_project_paths_blocking(
    sessions: Vec<PersistedSession>,
    project_paths: &[String],
) -> Result<Vec<PersistedSession>, String> {
    let project_paths = project_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        record_filter_project_paths_thread_id();
        filter_sessions_for_project_paths(sessions, &project_paths)
    })
    .await
    .map_err(|e| format!("session filter task failed: {}", e))
}

/// §1295 5.4 — one NDJSON record for one orphaned session drop (B1: every drop
/// writes exactly one record, always). `droppedAt` is RFC3339 UTC.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OrphanArchiveRecord<'a> {
    schema_version: u32,
    dropped_at: String,
    reason: &'a str,
    disposition: &'a str,
    session: &'a PersistedSession,
}

/// §1295 5.4 — append archive records are written with one `writeln!` per
/// record under `sessions_save_lock` (locked variant for site C). Best-effort:
/// an append failure logs ERROR and NEVER fails the persist (a write failure
/// must not resurrect the WARN loop). Cross-instance interleaving may produce
/// torn/chunked lines (accepted, the archive is never parsed).
fn append_orphan_archive_record_locked(
    archive_path: &Path,
    reason: &str,
    disposition: &str,
    session: &PersistedSession,
) {
    let record = OrphanArchiveRecord {
        schema_version: 1,
        dropped_at: chrono::Utc::now().to_rfc3339(),
        reason,
        disposition,
        session,
    };
    let line = match serde_json::to_string(&record) {
        Ok(line) => line,
        Err(e) => {
            log::error!(
                "[sessions] Failed to serialize orphan archive record: {}",
                e
            );
            return;
        }
    };
    let wrote = {
        let result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(archive_path)
            .and_then(|mut file| writeln!(file, "{}", line));
        if let Err(e) = result {
            log::error!(
                "[sessions] Failed to append orphan archive record to {:?}: {}",
                archive_path,
                e
            );
            false
        } else {
            true
        }
    };
    if wrote {
        rotate_orphan_archive(archive_path);
    }
}

/// §1295 S4 — locking public variant used by site C (restore-skip in lib.rs),
/// which is async and holds no `sessions_save_lock` at that point.
pub(crate) async fn append_orphan_archive_record(
    dir: &Path,
    reason: &str,
    disposition: &str,
    session: &PersistedSession,
) {
    let dir = dir.to_path_buf();
    let reason = reason.to_owned();
    let disposition = disposition.to_owned();
    let session = session.clone();
    run_owned_persist_core(async move {
        loop {
            let permit = acquire_carrier_mutation_permit().await;
            if all_carrier_protections().is_some_and(|protections| {
                PreparedCarrierRelation::capture(&session, &protections).classify(&protections)
                    == CarrierDisposition::Foreign
            }) {
                append_orphan_archive_record_locked(
                    &dir.join(ORPHAN_ARCHIVE_FILENAME),
                    &reason,
                    &disposition,
                    &session,
                );
                return;
            }
            drop(permit);
            tokio::task::yield_now().await;
        }
    })
    .await;
}

/// §1295 N5 — best-effort rotation of the active archive file, mirroring
/// `logging::rotate` (logging.rs:275-365): shift `orphaned-sessions.archive.json`
/// -> `.1` -> `.2` -> `.3`, dropping `.3`, ONLY when the ACTIVE file exceeds the
/// soft cap. A Windows sharing violation (second instance holding the append
/// handle without FILE_SHARE_DELETE) fails the rename: log ERROR and continue;
/// the file may exceed the cap until the next successful rotation. Rotation
/// failures never fail the persist. Deliberate LOCAL duplicate of the logging
/// algorithm (logging.rs untouched; documented cross-reference).
fn rotate_orphan_archive(archive_path: &Path) {
    let len = match std::fs::metadata(archive_path) {
        Ok(metadata) => metadata.len(),
        Err(_) => return,
    };
    if len < orphan_archive_max_bytes() {
        return;
    }
    let parent = match archive_path.parent() {
        Some(parent) => parent.to_path_buf(),
        None => return,
    };
    let stem = match archive_path.file_name().and_then(|name| name.to_str()) {
        Some(stem) => stem.to_string(),
        None => return,
    };
    let numbered = |i: u32| parent.join(format!("{stem}.{i}"));

    if ORPHAN_ARCHIVE_KEEP >= 2 {
        for i in (1..=ORPHAN_ARCHIVE_KEEP - 1).rev() {
            let from = numbered(i);
            if !from.exists() {
                continue;
            }
            let to = numbered(i + 1);
            if let Err(e) = std::fs::rename(&from, &to) {
                log::error!(
                    "[sessions] orphan archive rotation: failed to rename {} to {}: {} (continuing)",
                    from.display(),
                    to.display(),
                    e
                );
            }
        }
    }

    if let Err(e) = std::fs::rename(archive_path, numbered(1)) {
        log::error!(
            "[sessions] orphan archive rotation: failed to rename {} to {}: {} (active file stays in place)",
            archive_path.display(),
            numbered(1).display(),
            e
        );
        return;
    }

    // Recreate a fresh active file so the next append starts clean.
    if let Err(e) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(archive_path)
    {
        log::error!(
            "[sessions] orphan archive rotation: failed to recreate {}: {}",
            archive_path.display(),
            e
        );
    }
}

/// §1295 S2/B — one `spawn_blocking` chunk that (a) partitions the snapshot and
/// (b) probes `Path::exists` per drop to decide the disposition label.
///
/// Runs PRE-LOCK on the hot path so a hung/offline SMB share during classify
/// stalls NO other persistence writer. The in-lock callers (the #698 atomic
/// helpers and the startup merge path) accept the bounded stall (S2's allowed
/// branch): they concern rare user actions and one startup call, never the
/// ~25 s loop, and B1 guarantees no data loss.
pub(crate) struct PersistPreparation {
    pub kept: Vec<PersistedSession>,
    pub drops: Vec<OrphanDrop>,
}

/// §1295 (O1-refined) — the typed prune boundary. `PersistMode::PruneDormant`
/// is the ONLY mode that removes dormant orphan rows from the live manager
/// (`remove_exited_sessions`); it is used ONLY by the three steady-state
/// background persist drivers (lib.rs:731/1096/1121). Every other persist site
/// (all `SelectionTransaction::persist` transitions, mailbox, lifecycle,
/// app-exit, #698 helpers, purge) is `PersistMode::NoPrune`: it still archives
/// each dropped recipe once and drops out-of-roots rows from disk, but leaves
/// transition-owned rows in RAM. The enum is threaded so the compiler forces
/// every call site to declare its intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistMode {
    PruneDormant,
    NoPrune,
}

pub(crate) async fn prepare_persist_snapshot(
    snapshot: Vec<PersistedSession>,
    project_paths: &[String],
) -> Result<PersistPreparation, String> {
    let project_paths = project_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        record_filter_project_paths_thread_id();
        let roots = normalize_project_roots(&project_paths);
        let (kept, mut drops) = partition_orphaned_sessions(snapshot, &roots);
        for drop in &mut drops {
            drop.cwd_exists = Path::new(&drop.session.working_directory).exists();
        }
        PersistPreparation { kept, drops }
    })
    .await
    .map_err(|e| format!("session persistence prepare task failed: {}", e))
}

/// §1295 — the shared drop handler. The caller MUST already hold
/// `sessions_save_lock()`.
///
/// Distinguishes two drop kinds (behavior §7.6/§7.7): DORMANT orphans (the
/// row is `Exited` and not pending, or there is no live RAM row) are ARCHIVED
/// (B1-once: one record per dropped session id per run) and, ONLY under
/// `PersistMode::PruneDormant`, pruned from the manager; LIVE orphans
/// (non-Exited or pending) stay running in RAM and are dropped ONLY from the
/// disk snapshot — they get a WARN and a `live_kept` count, NO archive record
/// (their recipe is still live in the manager, so B1 does not apply).
///
/// Under `PersistMode::NoPrune` (every transition/mailbox/lifecycle site) the
/// same archive + disk-drop + WARN happen but `remove_exited_sessions` is NOT
/// called, so a transition-owned dormant row stays in RAM until a later steady
/// `PruneDormant` persist removes it.
///
/// `mgr` is `None` at site A (purge, no live RAM to mutate): every drop is
/// archived and no row is pruned regardless of mode. Returns the per-sweep
/// counts.
async fn handle_orphan_drops(
    mgr: Option<&SessionManager>,
    dir: &Path,
    drops: &[OrphanDrop],
    mode: PersistMode,
    carrier_permit: &CarrierMutationPermit,
    dispositions: Option<&HashMap<Uuid, CarrierDisposition>>,
) -> OrphanSweepCounts {
    let mut counts = OrphanSweepCounts::default();
    if drops.is_empty() {
        return counts;
    }
    let archive_path = dir.join(ORPHAN_ARCHIVE_FILENAME);

    // Decide which drop ids are DORMANT (will be archived + pruned) vs LIVE
    // (kept in RAM). At site A (mgr=None) everything is dormant. At site B this
    // mirrors the same oracle `remove_exited_sessions` re-verifies (status
    // Exited AND not pending); `remove_exited_sessions` stays the source of
    // truth for the actual mutation, this set only decides what gets archived.
    let dormant: HashSet<Uuid> = match mgr {
        Some(mgr) => {
            let snap = mgr.aggregate_snapshot().await;
            drops
                .iter()
                .filter_map(|drop| drop.session.id.as_deref())
                .filter_map(|id| Uuid::parse_str(id).ok())
                .filter(|uuid| {
                    snap.sessions
                        .iter()
                        .any(|s| &s.id == uuid && matches!(s.status, SessionStatus::Exited(_)))
                        && !snap.pending_ids.contains(uuid)
                })
                .collect()
        }
        None => drops
            .iter()
            .filter_map(|drop| drop.session.id.as_deref())
            .filter_map(|id| Uuid::parse_str(id).ok())
            .collect(),
    };

    let mut candidate_ids: Vec<Uuid> = Vec::new();
    for drop in drops {
        if !note_orphan_cwd(&drop.session.working_directory) {
            counts.suppressed_repeat += 1;
        }
        let is_dormant = match drop.session.id.as_deref() {
            Some(id) => Uuid::parse_str(id)
                .map(|uuid| dormant.contains(&uuid))
                .unwrap_or(false),
            // No live row (recipe-only row, e.g. a failed-recoverable merge or
            // a restore-skipped entry): treat as dormant and record it.
            None => true,
        };
        if is_dormant {
            // B1-once: append the archive record only on the first sighting of
            // this session id this run. A row first archived by a no-prune site
            // is NOT re-appended when the steady site later prunes it from RAM.
            let first_archive = match drop.session.id.as_deref() {
                Some(id) => Uuid::parse_str(id)
                    .map(|uuid| note_orphan_archived_id(&uuid))
                    .unwrap_or(true),
                None => true,
            };
            if first_archive {
                if drop.cwd_exists {
                    counts.archived += 1;
                    append_orphan_archive_record_locked(
                        &archive_path,
                        "outsideRetainedRoots",
                        "archived",
                        &drop.session,
                    );
                } else {
                    counts.reaped += 1;
                    append_orphan_archive_record_locked(
                        &archive_path,
                        "outsideRetainedRootsMissing",
                        "reaped",
                        &drop.session,
                    );
                }
            }
            if let Some(id) = drop.session.id.as_deref() {
                if let Ok(uuid) = Uuid::parse_str(id) {
                    candidate_ids.push(uuid);
                }
            }
        } else {
            counts.live_kept += 1;
        }
    }
    if mode == PersistMode::PruneDormant {
        if let Some(mgr) = mgr {
            let removed = mgr
                .remove_exited_sessions_with_carrier_permit(
                    &candidate_ids,
                    carrier_permit,
                    |row, _| {
                        dispositions.map_or(CarrierDisposition::Foreign, |dispositions| {
                            dispositions
                                .get(&row.id)
                                .copied()
                                .unwrap_or(CarrierDisposition::Unknown)
                        })
                    },
                )
                .await;
            // A row classified dormant here but not removed by the manager (e.g. it
            // got restarted between snapshot and prune) stays live: reclassify to
            // live_kept so the counters stay honest.
            counts.live_kept += candidate_ids.len().saturating_sub(removed);
        }
    }
    #[cfg(test)]
    accumulate_orphan_counters(counts);
    log::info!(
        "[sessions] orphan sweep: archived={} reaped={} liveKept={} repeatWarnSuppressed={}",
        counts.archived,
        counts.reaped,
        counts.live_kept,
        counts.suppressed_repeat
    );
    counts
}

/// §1295 — persist a prepared snapshot under a lock the caller already holds.
/// Archives drops (B1), prunes dormant manager rows, dedups WARNs, then saves
/// the kept set with the unchanged atomic tmp+rename stack.
#[cfg(test)]
async fn persist_prepared_locked(
    mgr: Option<&SessionManager>,
    dir: &Path,
    prep: PersistPreparation,
    mode: PersistMode,
    carrier_permit: &CarrierMutationPermit,
) -> Result<(), String> {
    handle_orphan_drops(mgr, dir, &prep.drops, mode, carrier_permit, None).await;
    if let Err(error) = save_sessions_to_dir(dir, &prep.kept) {
        if let Some(mgr) = mgr {
            mgr.disable_prompt_reuse().await;
        }
        return Err(error);
    }
    if let Some(mgr) = mgr {
        flush_retained_prompts_locked(mgr, dir).await?;
    }
    Ok(())
}

/// Structural (non-canonicalizing) form of a raw path for archived-root membership: the
/// same separator normalization as `normalize_for_project_compare` (long-prefix strip,
/// `/` separators, trailing-slash trim, Windows case-fold) WITHOUT the `canonicalize`
/// step. The gate's archived exemption is intentionally existence-independent, so it must
/// stay deterministic whether or not the archived root (or a symlink it resolves through)
/// exists on disk.
fn normalize_archived_root_for_compare(path: &str) -> String {
    let mut s = strip_long_prefix_str(path).replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s.make_ascii_lowercase();
    }
    s
}

/// Structural (raw, non-canonicalizing) containment of `path` under one of `roots`,
/// returning the first matching raw root. Used ONLY by the archived-root gate exemption
/// (§1295 5.1a rule 2), where membership in the archived list is the retention signal and
/// must hold regardless of on-disk existence. Canonicalizing here would be wrong: when an
/// archived root resolves through a symlink/junction but a cwd below it does not yet exist,
/// `canonicalize(root)` resolves the symlink while `canonicalize(cwd)` falls back to the raw
/// path, so the canonical root no longer prefixes the raw cwd and the exemption would
/// falsely fail (CI-observed on Ubuntu with a symlinked temp root).
fn archived_root_containing_raw(path: &str, roots: &[String]) -> Option<String> {
    let cwd = normalize_archived_root_for_compare(path);
    roots.iter().find_map(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let root = normalize_archived_root_for_compare(trimmed);
        if path_is_under_or_equal(&cwd, &root) {
            Some(raw.clone())
        } else {
            None
        }
    })
}

/// §1295 5.1a — creation-time gate predicate (pure). Rules, in order:
///   1. Root-agent path -> Ok (the directory is AC-owned).
///   2. Under an archived root -> Ok with the existence check WAIVED
///      (temp-unmount / drive-down safety; `enforce_unarchived_for_spawn`
///      auto-unarchives on an actual spawn). The membership check is structural
///      (raw-string, non-canonicalizing) so it is deterministic across platforms
///      and independent of symlink resolution or `Path::exists()`.
///   3. Outside every registered (active + archived) root -> Err with stable
///      prefix `sessionCreateBlocked:` and fragment "outside all registered
///      projects".
///   4. Registered but the directory does not exist (deleted workgroup or
///      unmounted dir) -> Err with the same prefix and fragment "does not
///      exist on disk".
///   5. Otherwise Ok.
///
/// The error strings surface verbatim through the existing String plumbing.
pub(crate) fn validate_session_creation_cwd(
    cwd: &str,
    retained_project_paths: &[String],
    archived_project_paths: &[String],
) -> Result<(), String> {
    // 1. Root-agent path is exempt (the archive-gate exceptions cover it too).
    if crate::config::root_agent::is_root_agent_path(cwd) {
        return Ok(());
    }
    // 2. Archived root (structural membership; existence deliberately waived for
    //    unmount/drive-down, so a missing cwd under an archived root is Ok).
    if archived_root_containing_raw(cwd, archived_project_paths).is_some() {
        return Ok(());
    }
    // 3. Outside every registered (active + archived) root.
    let retained_roots = normalize_project_roots(retained_project_paths);
    let cwd_norm = normalize_for_project_compare(Path::new(cwd));
    if !working_directory_under_any_normalized_root(&cwd_norm, &retained_roots) {
        return Err(format!(
            "sessionCreateBlocked: cwd '{}' is outside all registered projects",
            cwd
        ));
    }
    // 4. Registered but the directory does not exist.
    if !Path::new(cwd).exists() {
        return Err(format!(
            "sessionCreateBlocked: cwd '{}' does not exist on disk",
            cwd
        ));
    }
    // 5. Ok.
    Ok(())
}

/// §1295 5.5 — creation-gate enforcement rides the call stack (S3): NO process-
/// global toggle. `Skip` is constructed only by test code; production builds
/// compile only the `Enforce` path via `default_creation_gate_enforcement()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreationGateEnforcement {
    Enforce,
    #[allow(dead_code)] // constructed only by test code (§5.5); production sees only Enforce
    Skip,
}

/// §1295 5.5 — returns `Skip` under `cfg!(test)` and `Enforce` otherwise, so
/// existing test fixtures that call the wrappers do not trip the gate while a
/// concurrent or parallel test can still opt in by calling the impl (or
/// `with_intent`) directly with `Enforce`.
pub(crate) fn default_creation_gate_enforcement() -> CreationGateEnforcement {
    if cfg!(test) {
        CreationGateEnforcement::Skip
    } else {
        CreationGateEnforcement::Enforce
    }
}

/// §1295 5.1 — run the creation gate. Reads the settings once (cloned read
/// guard), computes retained + archived roots, and runs the pure predicate in
/// `tokio::task::spawn_blocking`. `Skip` short-circuits to Ok.
pub(crate) async fn enforce_creation_gate<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &str,
    enforcement: CreationGateEnforcement,
) -> Result<(), String> {
    match enforcement {
        CreationGateEnforcement::Skip => Ok(()),
        CreationGateEnforcement::Enforce => {
            let settings = app.state::<crate::config::settings::SettingsState>();
            let cfg = settings.read().await;
            let retained = session_retention_project_paths(&cfg)?;
            let archived = cfg.archived_project_paths.clone();
            drop(cfg);
            let cwd = cwd.to_string();
            tokio::task::spawn_blocking(move || {
                validate_session_creation_cwd(&cwd, &retained, &archived)
            })
            .await
            .map_err(|e| format!("session creation gate task failed: {}", e))?
        }
    }
}

/// Remove duplicate sessions by name AND working_directory.
/// When duplicates share the same key (name or CWD), keep the one with
/// `was_active=true`; if none (or both) are active, keep the last occurrence.
/// Note: callers are expected to filter out temp sessions before calling this.
fn deduplicate(sessions: Vec<PersistedSession>) -> Vec<PersistedSession> {
    let total = sessions.len();
    let mut name_index: HashMap<String, usize> = HashMap::new();
    let mut cwd_index: HashMap<String, usize> = HashMap::new();
    let mut root_index: Option<usize> = None;
    let mut result: Vec<PersistedSession> = Vec::with_capacity(total);

    for session in sessions {
        let norm_cwd = normalized_cwd_key(&session.working_directory);
        let is_root_agent = session.is_root_agent
            || crate::config::root_agent::is_root_agent_path(&session.working_directory);

        if is_root_agent {
            if let Some(idx) = root_index {
                log::warn!(
                    "[sessions] Dropping duplicate root agent session '{}' at '{}'",
                    session.name,
                    session.working_directory
                );
                if !result[idx].was_active || session.was_active {
                    let old_cwd = normalized_cwd_key(&result[idx].working_directory);
                    name_index.remove(&result[idx].name);
                    cwd_index.remove(&old_cwd);
                    name_index.insert(session.name.clone(), idx);
                    cwd_index.insert(norm_cwd, idx);
                    result[idx] = session;
                    result[idx].is_root_agent = true;
                } else {
                    result[idx].is_root_agent = true;
                }
                continue;
            }
        }

        // Check name-based duplicate
        if let Some(&idx) = name_index.get(&session.name) {
            log::warn!(
                "[sessions] Dropping duplicate session by name '{}'",
                session.name
            );
            if !result[idx].was_active || session.was_active {
                // Patch cwd_index if the CWD changed
                let old_cwd = normalized_cwd_key(&result[idx].working_directory);
                if old_cwd != norm_cwd {
                    cwd_index.remove(&old_cwd);
                    cwd_index.insert(norm_cwd, idx);
                }
                result[idx] = session;
            }
            continue;
        }

        // Check CWD-based duplicate
        if let Some(&idx) = cwd_index.get(&norm_cwd) {
            log::warn!(
                "[sessions] Dropping duplicate session by CWD '{}' (existing='{}', incoming='{}')",
                session.working_directory,
                result[idx].name,
                session.name
            );
            if !result[idx].was_active || session.was_active {
                name_index.remove(&result[idx].name);
                name_index.insert(session.name.clone(), idx);
                result[idx] = session;
            }
            continue;
        }

        // New unique session
        name_index.insert(session.name.clone(), result.len());
        cwd_index.insert(norm_cwd, result.len());
        if is_root_agent {
            root_index = Some(result.len());
        }
        result.push(session);
        if is_root_agent {
            if let Some(last) = result.last_mut() {
                last.is_root_agent = true;
            }
        }
    }

    if result.len() < total {
        log::info!(
            "[sessions] Deduplicated: {} → {} sessions",
            total,
            result.len()
        );
    }

    result
}

fn normalized_cwd_key(cwd: &str) -> String {
    crate::path_utils::normalize_windows_verbatim_path(cwd)
        .replace('\\', "/")
        .to_lowercase()
}

fn normalize_persisted_session_paths(session: &mut PersistedSession) {
    session.working_directory =
        crate::path_utils::normalize_windows_verbatim_path(&session.working_directory);
    for repo in &mut session.git_repos {
        repo.source_path = crate::path_utils::normalize_windows_verbatim_path(&repo.source_path);
    }
}

/// Load sessions from disk without deduplication or temp-session filtering.
/// Used by the CLI to read the live snapshot as-is.
pub fn load_sessions_raw() -> Vec<PersistedSession> {
    let path = match sessions_path() {
        Some(p) => p,
        None => return vec![],
    };
    load_sessions_raw_from_path(&path)
}

#[cfg(any(test, debug_assertions))]
pub fn load_sessions_raw_from_dir_for_test(dir: &Path) -> Vec<PersistedSession> {
    load_sessions_raw_from_path(&dir.join("sessions.json"))
}

fn load_sessions_raw_from_path(path: &Path) -> Vec<PersistedSession> {
    if !path.exists() {
        return vec![];
    }
    match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => vec![],
    }
}

/// Load persisted sessions from the app config directory (see config_dir()).
/// Returns empty vec on any error (missing file, corrupt JSON, etc.)
pub fn load_sessions() -> Vec<PersistedSession> {
    let path = match sessions_path() {
        Some(p) => p,
        None => {
            log::warn!("Could not determine home directory for session restore");
            return vec![];
        }
    };

    load_sessions_from_path(&path)
}

#[cfg(test)]
pub(crate) fn load_sessions_from_dir(dir: &Path) -> Vec<PersistedSession> {
    load_sessions_from_path(&dir.join("sessions.json"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentBindingSeed {
    pub identity_revision: String,
    pub original_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_ref: Option<crate::config::settings::ConfigurationRef>,
}

impl CodingAgentBindingSeed {
    fn validate_source(&self) -> Result<(), String> {
        let Some(reference) = &self.configuration_ref else {
            return if self.identity_revision.starts_with("s1:") {
                Err("invalid_coding_agent_binding_seed".into())
            } else {
                Ok(())
            };
        };
        let suffix = format!(":{}", reference.calculated_id);
        let digest = self
            .identity_revision
            .strip_prefix("s1:")
            .and_then(|value| value.strip_suffix(&suffix));
        let valid_digest = digest.is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        let full_identity = |value: &str| {
            value.strip_prefix("cid1:").is_some_and(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        };
        if !valid_digest
            || reference.version != 1
            || reference.source.context_root.is_empty()
            || self.original_agent_id.is_empty()
            || reference.legacy_entry_hint.as_deref() != Some(self.original_agent_id.as_str())
            || !full_identity(&reference.calculated_id)
            || !reference
                .profile_calculated_id
                .as_deref()
                .is_some_and(full_identity)
            || reference.requested_profile.as_ref().is_some_and(|letter| {
                crate::config::settings::normalize_profile_letter(letter).as_ref() != Some(letter)
            })
        {
            return Err("invalid_coding_agent_binding_seed".into());
        }
        Ok(())
    }
}

fn capture_binding_origins(path: &Path, bytes: &[u8], sessions: &mut [PersistedSession]) {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(bytes));
    let mut counts = HashMap::new();
    for ps in sessions.iter_mut() {
        if ps.agent_id.is_some() {
            ps.coding_agent_binding_id
                .get_or_insert_with(|| Uuid::new_v4().to_string());
        }
        if let Some(id) = ps.coding_agent_binding_id.as_ref() {
            let key = Uuid::parse_str(id)
                .map(|id| id.to_string())
                .unwrap_or_else(|_| id.clone());
            *counts.entry(key).or_insert(0usize) += 1;
        }
    }
    for (ordinal, ps) in sessions.iter_mut().enumerate() {
        if let Some(id) = ps.coding_agent_binding_id.as_ref() {
            ps.coding_agent_binding_origin = Some(serde_json::json!({
                "sessionsPath": path.to_string_lossy(),
                "rawFileSHA256": digest,
                "originalOrdinal": ordinal,
            }));
            let key = Uuid::parse_str(id)
                .map(|id| id.to_string())
                .unwrap_or_else(|_| id.clone());
            ps.coding_agent_binding_conflict = counts.get(&key).copied().unwrap_or(0) > 1;
        }
    }
}

impl PersistedSession {
    pub(crate) fn binding_metadata(
        &self,
    ) -> Result<Option<crate::session::manager::CodingAgentBinding>, String> {
        if self.coding_agent_binding_conflict {
            return Err("duplicate_coding_agent_binding".to_string());
        }
        self.coding_agent_binding_id
            .as_ref()
            .map(|id| {
                Uuid::parse_str(id).map_err(|_| "invalid_coding_agent_binding".to_string())?;
                if let Some(seed) = &self.coding_agent_binding_seed {
                    seed.validate_source()?;
                }
                if self.coding_agent_binding_seed.as_ref().is_some_and(|seed| {
                    seed.identity_revision.is_empty() || seed.original_agent_id.is_empty()
                }) {
                    return Err("invalid_coding_agent_binding_seed".to_string());
                }
                let seed = self
                    .coding_agent_binding_seed
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|e| e.to_string())?;
                Ok(crate::session::manager::CodingAgentBinding {
                    id: id.clone(),
                    seed,
                    origin: self.coding_agent_binding_origin.clone(),
                })
            })
            .transpose()
    }

    pub(crate) fn capture_binding_seed(
        &mut self,
        settings: &crate::config::settings::AppSettings,
    ) -> Result<(), String> {
        if let Some(seed) = &self.coding_agent_binding_seed {
            seed.validate_source()?;
        }
        if self.coding_agent_binding_seed.is_none() {
            if let Some(agent_id) = self.agent_id.as_ref() {
                self.coding_agent_binding_seed = Some(CodingAgentBindingSeed {
                    identity_revision: crate::config::settings::bootstrap_binding_revision(
                        settings, agent_id,
                    )?,
                    original_agent_id: agent_id.clone(),
                    configuration_ref: None,
                });
            }
        }
        Ok(())
    }
}

fn load_sessions_from_path(path: &Path) -> Vec<PersistedSession> {
    load_sessions_from_path_with_settings(path, None)
}

fn load_sessions_from_path_with_settings(
    path: &Path,
    settings: Option<&super::settings::AppSettings>,
) -> Vec<PersistedSession> {
    if !path.exists() {
        return vec![];
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<Vec<PersistedSession>>(&contents) {
            Ok(mut sessions) => {
                capture_binding_origins(path, contents.as_bytes(), &mut sessions);
                if let Some(settings) = settings {
                    for ps in &mut sessions {
                        if let Err(error) = ps.capture_binding_seed(settings) {
                            log::error!(
                                "[sessions] binding seed unavailable for '{}': {}",
                                ps.name,
                                error
                            );
                        }
                    }
                }
                // Safety net: filter out [temp] sessions that should never survive a restart
                let temp_count = sessions
                    .iter()
                    .filter(|s| s.name.starts_with(TEMP_SESSION_PREFIX))
                    .count();
                let filtered: Vec<PersistedSession> = sessions
                    .into_iter()
                    .filter(|s| {
                        if s.name.starts_with(TEMP_SESSION_PREFIX) {
                            log::warn!(
                                "[sessions] Filtering out temp session '{}' from persistence",
                                s.name
                            );
                            false
                        } else {
                            true
                        }
                    })
                    .collect();
                if temp_count > 0 {
                    log::info!(
                        "[sessions] Removed {} temp sessions from persistence file",
                        temp_count
                    );
                }
                let mut deduped = deduplicate(filtered);

                // Legacy-schema upgrade: run AFTER deduplicate() so each row's legacy
                // payload travels with its own entry. `.take()` clears the Options and
                // `skip_serializing_if` in PersistedSession elides them on next save.
                for ps in deduped.iter_mut() {
                    if !ps.git_repos.is_empty() {
                        // Already new-schema; drop any ghost legacy values.
                        ps.git_branch_source = None;
                        ps.git_branch_prefix = None;
                        continue;
                    }
                    match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
                        (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                            log::info!(
                                "[sessions] Upgrading legacy single-repo session '{}' → git_repos[1]={{label:{}, source:{}}}",
                                ps.name, prefix, source
                            );
                            ps.git_repos.push(crate::session::session::SessionRepo {
                                label: prefix,
                                source_path: source,
                                branch: None,
                                dirty: None,
                            });
                        }
                        (Some(source), None) => {
                            // Shouldn't happen in data this codebase produces, but serde(default)
                            // + hand-edited files can land here. Synthesize label from dir name.
                            let dir = source
                                .replace('\\', "/")
                                .split('/')
                                .next_back()
                                .unwrap_or("")
                                .to_string();
                            let label =
                                dir.strip_prefix("repo-").map(str::to_string).unwrap_or(dir);
                            log::warn!(
                                "[sessions] Upgrading legacy session '{}' with source but no prefix; synthesized label '{}'",
                                ps.name, label
                            );
                            ps.git_repos.push(crate::session::session::SessionRepo {
                                label,
                                source_path: source,
                                branch: None,
                                dirty: None,
                            });
                        }
                        (None, Some(prefix)) if prefix == "multi-repo" => {
                            log::info!(
                                "[sessions] Legacy multi-repo session '{}' → git_repos left empty; DiscoveryBranchWatcher will backfill",
                                ps.name
                            );
                        }
                        (None, Some(other)) => {
                            log::warn!(
                                "[sessions] Legacy session '{}' has unknown prefix '{}' without source; dropping",
                                ps.name, other
                            );
                        }
                        (None, None) => {}
                        (Some(_), Some(_)) => {
                            // prefix == "multi-repo" with a source — ambiguous legacy shape.
                            log::warn!(
                                "[sessions] Legacy session '{}' had source + multi-repo prefix; leaving git_repos empty for discovery backfill",
                                ps.name
                            );
                        }
                    }
                }

                for ps in deduped.iter_mut() {
                    normalize_persisted_session_paths(ps);
                }

                log::info!(
                    "Loaded {} persisted sessions from {:?}",
                    deduped.len(),
                    path
                );
                deduped
            }
            Err(e) => {
                log::error!("Failed to parse sessions file: {}", e);
                vec![]
            }
        },
        Err(e) => {
            log::error!("Failed to read sessions file: {}", e);
            vec![]
        }
    }
}

pub async fn load_sessions_purging_outside_project_paths(
    project_paths: &[String],
    settings: &super::settings::AppSettings,
) -> Vec<PersistedSession> {
    let dir = match super::config_dir() {
        Some(d) => d,
        None => {
            log::warn!("Could not determine home directory for session restore");
            return vec![];
        }
    };

    load_sessions_purging_outside_project_paths_in_dir(&dir, project_paths, Some(settings)).await
}

async fn load_sessions_purging_outside_project_paths_in_dir(
    dir: &Path,
    project_paths: &[String],
    settings: Option<&super::settings::AppSettings>,
) -> Vec<PersistedSession> {
    match purge_sessions_outside_project_paths_in_dir_locked(dir, project_paths, settings).await {
        Ok((_, filtered)) => filtered,
        Err(e) => {
            log::error!(
                "Failed to rewrite sessions.json after orphan-session purge: {}",
                e
            );
            // Read-only fallback (no save): a stale read here cannot violate the
            // atomicity contract because nothing is written back. Restore
            // reconciles on the next persist.
            match filter_sessions_for_project_paths_blocking(
                load_sessions_from_path_with_settings(&dir.join("sessions.json"), settings),
                project_paths,
            )
            .await
            {
                Ok(filtered) => filtered,
                Err(e) => {
                    log::error!("Failed to filter sessions after purge failure: {}", e);
                    load_sessions_from_path_with_settings(&dir.join("sessions.json"), settings)
                }
            }
        }
    }
}

pub(crate) async fn purge_sessions_outside_project_paths_in_dir(
    dir: &Path,
    project_paths: &[String],
) -> Result<Vec<PersistedSession>, String> {
    let (_before_len, filtered) =
        purge_sessions_outside_project_paths_in_dir_locked(dir, project_paths, None).await?;
    Ok(filtered)
}

/// #698 HIGH fix: the lock-holding core shared by both live purge callers:
/// `commands::config::purge_sessions_after_settings_update_in_dir` (settings
/// update path) and `load_sessions_purging_outside_project_paths` (startup
/// restore path).
///
/// The orphan purge is a load -> filter -> save sequence. Before this fix the
/// load and save ran OUTSIDE `sessions_save_lock()`, so another locked
/// persistence writer could land its durable write between them: the purge read
/// a stale `sessions.json`, a raise-hand then persisted `communication:
/// raiseHand`, and the purge's already-computed stale copy overwrote it,
/// silently dropping a successfully-emitted raise from disk and from CLI
/// `list-sessions`.
///
/// Holding `sessions_save_lock()` across the whole load+filter+save makes the
/// purge a full participant in the same mutual exclusion the raise-hand and
/// `persist_current_state` helpers use, so every sessions persistence writer
/// now serializes on one lock. Whichever writer acquires the lock second
/// re-reads the fresh on-disk state, so neither can clobber the other with stale
/// data. The save is the sync, leaf-level `save_sessions_to_dir` (its own
/// `SAVE_SESSIONS_LOCK` never re-enters this one), so there is no deadlock and
/// no await is held across a second acquisition of this lock.
///
/// Returns `(before_len, filtered)` so callers can retain the filtered snapshot
/// and keep the removed count available for diagnostics.
async fn purge_sessions_outside_project_paths_in_dir_locked(
    dir: &Path,
    project_paths: &[String],
    settings: Option<&super::settings::AppSettings>,
) -> Result<(usize, Vec<PersistedSession>), String> {
    let dir = dir.to_path_buf();
    let roots = project_paths.to_vec();
    let settings = settings.cloned();
    run_owned_persist_core(async move {
        let mut permit = acquire_carrier_mutation_permit().await;
        let settings = settings.unwrap_or_else(super::settings::load_settings_for_cli);
        let mut foreign_done = HashSet::new();
        loop {
            let before =
                load_sessions_from_path_with_settings(&dir.join("sessions.json"), Some(&settings));
            let before_len = before.len();
            let prep = prepare_persist_snapshot(before, &roots).await?;
            let protections = all_carrier_protections();
            let identity = probe_publication_directory(&dir, false);
            let slot = resolve_publication_slot(&dir, identity);
            if let (Some(protections), Some(slot)) = (protections, slot) {
                if file_preparation_survives(&slot, &protections, &prep) {
                    let drops = prep
                        .drops
                        .iter()
                        .filter(|drop| foreign_done.insert(orphan_effect_key(drop)))
                        .cloned()
                        .collect::<Vec<_>>();
                    handle_orphan_drops(None, &dir, &drops, PersistMode::NoPrune, &permit, None)
                        .await;
                    // Protection is checked before this legacy length/no-write gate.
                    if prep.kept.len() == before_len {
                        return Ok((before_len, prep.kept));
                    }
                    match save_file_snapshot_guarded(&dir, &slot, &prep.kept).await {
                        GuardedSaveAttempt::Completed(result) => {
                            return result.map(|_| (before_len, prep.kept))
                        }
                        GuardedSaveAttempt::Recapture => {}
                    }
                }
            }
            drop(permit);
            tokio::task::yield_now().await;
            permit = acquire_carrier_mutation_permit().await;
        }
    })
    .await
}

/// Save current sessions to the app config directory (see config_dir()).
///
/// Concurrency model (#291):
/// - **In-process callers are serialized** by `SAVE_SESSIONS_LOCK`; the
///   rename order matches the order of `lock()` acquisition, so "last
///   writer wins" deterministically.
/// - **Per-call unique temp filenames** (`sessions.json.<pid>.<op_id>.tmp`)
///   prevent two callers from ever colliding on a shared `.tmp`, which was
///   the historical Windows `ERROR_FILE_NOT_FOUND` (os error 2) race.
/// - **Cross-process contention** on the destination (a second AC instance,
///   AV, or the Indexer) is absorbed by `rename_with_retry`. Per-call temp
///   names also keep cross-process callers from colliding on `.tmp`.
///
/// What this does NOT solve (out of scope for #291):
/// - **Snapshot freshness races.** Callers usually call
///   `snapshot_sessions(&mgr).await` and then `save_sessions(&snapshot)`
///   non-atomically. A concurrent `SessionManager` mutation between the
///   two can leave the snapshot stale by the time it lands on disk. The
///   next session-lifecycle event re-persists. See §224 G-IMPL-4 in
///   `phone/mailbox.rs` for the documented behavior.
#[cfg(test)]
pub fn save_sessions(sessions: &[PersistedSession]) -> Result<(), String> {
    save_sessions_to_config_dir(sessions)
}

#[cfg(test)]
fn save_sessions_to_config_dir(sessions: &[PersistedSession]) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    save_sessions_to_dir(&dir, sessions)
}

/// Path-injected core of `save_sessions`. Same concurrency guarantees as
/// `save_sessions`; the explicit `dir` parameter lets tests drive the
/// persistence path through a `tempfile::tempdir()` without touching the
/// process-wide `config_dir()` once-cell.
#[cfg(test)]
pub(crate) fn save_sessions_to_dir_for_test(
    dir: &Path,
    sessions: &[PersistedSession],
) -> Result<(), String> {
    save_sessions_to_dir(dir, sessions)
}

#[cfg(test)]
fn save_sessions_to_dir(dir: &Path, sessions: &[PersistedSession]) -> Result<(), String> {
    if super::settings::agents_layer_unreadable_for_dir(dir) {
        return Ok(());
    }
    let _guard = SAVE_SESSIONS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let write = PreparedSessionsWrite::prepare(dir, sessions)?;
    if let Err((error, diagnostics)) = rename_with_retry(&write.tmp_path, &write.path) {
        return Err(format!(
            "Failed to rename sessions file: {} [op_id={} pid={} instance={} attempts={} \
             tmp_existed_before={} final_existed_before={} os_error={:?} duration={:?}]",
            error,
            diagnostics.op_id,
            diagnostics.pid,
            diagnostics.instance_id,
            diagnostics.attempts,
            diagnostics.tmp_exists_before,
            diagnostics.final_exists_before,
            diagnostics.last_os_error,
            diagnostics.duration,
        ));
    }
    // A raw/seed write advances the same epoch, but cannot mint a manager
    // checkpoint or feature proof. A stale prepared core must recapture.
    if let Some(slot) = publication_slot(dir) {
        if let Ok(mut state) = slot.lock() {
            if let Some(epoch) = state.epoch.checked_add(1) {
                state.epoch = epoch;
            }
            state.checkpoint = None;
        }
    }
    log::info!("Saved {} sessions to {:?}", sessions.len(), write.path);
    Ok(())
}

#[derive(Clone)]
struct PublicationCheckpoint {
    nonce: Uuid,
    epoch: u64,
    rows: Vec<PersistedSession>,
    versions: HashMap<Uuid, u64>,
    confirmed: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum ProtectionPhase {
    Protected,
    Preparing {
        owner_nonce: Uuid,
        previous: Uuid,
        replacement: Uuid,
    },
    Closing,
}

/// Historical evidence minted only after the real Dormant settlement succeeds.
/// Current authority always comes from a new Confirmed snapshot under U.
#[derive(Clone, PartialEq, Eq)]
struct RetiredLocalCarrierFact {
    session_id: Uuid,
    token: Uuid,
    target: String,
    protection_nonce: Uuid,
    canonical_replica: PathBuf,
    physical_replica: crate::session::manager::CarrierPhysicalIdentity,
    generation: u64,
    committed_version: u64,
    settled_version: u64,
    checkpoint_nonce: Uuid,
    publication_epoch: u64,
}

#[derive(Clone)]
struct CarrierProtection {
    target: String,
    session_id: Uuid,
    token: Uuid,
    lifetime: std::sync::Weak<()>,
    retained: Option<std::sync::Arc<()>>,
    pending_lifetime: Option<std::sync::Arc<()>>,
    active: Option<std::sync::Arc<()>>,
    nonce: Uuid,
    epoch: u64,
    version: u64,
    row: PersistedSession,
    canonical_cwd: PathBuf,
    phase: ProtectionPhase,
    retired_local: Option<RetiredLocalCarrierFact>,
}

#[derive(Clone, Default)]
struct PublicationState {
    canonical_directory: Option<PathBuf>,
    epoch: u64,
    checkpoint: Option<PublicationCheckpoint>,
    protections: Vec<CarrierProtection>,
}

type PublicationSlot = std::sync::Arc<Mutex<PublicationState>>;

fn publication_slots() -> &'static Mutex<HashMap<PathBuf, PublicationSlot>> {
    static SLOTS: std::sync::OnceLock<Mutex<HashMap<PathBuf, PublicationSlot>>> =
        std::sync::OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

fn publication_slot(dir: &Path) -> Option<PublicationSlot> {
    let mut slots = publication_slots().lock().ok()?;
    Some(slots.entry(dir.to_path_buf()).or_default().clone())
}

enum PublicationDirectoryIdentity {
    Legacy,
    Known(PathBuf),
    Unknown,
}

/// Native identity probes run before hot U/M capture. The zero-protection
/// legacy path performs no additional filesystem probes.
fn probe_publication_directory(dir: &Path, initial: bool) -> PublicationDirectoryIdentity {
    match all_carrier_protections() {
        Some(protections) if protections.is_empty() => {
            if !initial {
                return PublicationDirectoryIdentity::Legacy;
            }
            std::fs::canonicalize(dir)
                .map(PublicationDirectoryIdentity::Known)
                .unwrap_or(PublicationDirectoryIdentity::Legacy)
        }
        Some(_) => std::fs::canonicalize(dir)
            .map(PublicationDirectoryIdentity::Known)
            .unwrap_or(PublicationDirectoryIdentity::Unknown),
        None => PublicationDirectoryIdentity::Unknown,
    }
}

/// Pure alias adoption under U. Every physical directory shares one slot, so
/// an alternate path cannot retain a separately confirmed epoch or protection.
fn resolve_publication_slot(
    dir: &Path,
    identity: PublicationDirectoryIdentity,
) -> Option<PublicationSlot> {
    let canonical = match identity {
        PublicationDirectoryIdentity::Legacy => {
            return all_carrier_protections()?
                .is_empty()
                .then(|| publication_slot(dir))
                .flatten();
        }
        PublicationDirectoryIdentity::Known(path) => path,
        PublicationDirectoryIdentity::Unknown => return None,
    };
    let mut slots = publication_slots().lock().ok()?;
    let mut matching: Option<PublicationSlot> = None;
    for slot in slots.values() {
        let state = slot.lock().ok()?;
        if state.canonical_directory.as_ref() == Some(&canonical) {
            if matching
                .as_ref()
                .is_some_and(|previous| !std::sync::Arc::ptr_eq(previous, slot))
            {
                return None;
            }
            matching = Some(slot.clone());
        }
    }
    let slot = matching.unwrap_or_else(|| slots.entry(dir.to_path_buf()).or_default().clone());
    slot.lock().ok()?.canonical_directory = Some(canonical);
    slots.insert(dir.to_path_buf(), slot.clone());
    Some(slot)
}

/// Owned upper binding. The caller supplies the authenticated canonical target
/// and verified replica root; this module never derives target authority from CWD.
#[derive(Clone)]
pub(crate) struct InitialCarrierIntent {
    pub target: String,
    pub session_id: Uuid,
    pub token: Uuid,
    pub version: u64,
    pub canonical_replica: PathBuf,
    restore_exclusion: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    lifetime: std::sync::Arc<()>,
}

impl InitialCarrierIntent {
    pub(crate) fn new(
        target: String,
        session_id: Uuid,
        token: Uuid,
        version: u64,
        canonical_replica: PathBuf,
    ) -> Self {
        Self {
            target,
            session_id,
            token,
            version,
            canonical_replica,
            restore_exclusion: None,
            lifetime: std::sync::Arc::new(()),
        }
    }

    /// Upper passes the same data-only restore exclusion held by its coordinator.
    pub(crate) fn with_restore_exclusion(
        mut self,
        exclusion: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.restore_exclusion = Some(exclusion);
        self
    }

    fn restore_allows_publication(&self) -> bool {
        !self
            .restore_exclusion
            .as_ref()
            .is_some_and(|active| active.load(Ordering::Acquire))
    }

    fn matches(
        &self,
        captured: &crate::session::manager::CarrierSnapshot,
        rows: &[PersistedSession],
    ) -> bool {
        !captured.pending_ids.contains(&self.session_id)
            && captured.versions.get(&self.session_id) == Some(&self.version)
            && captured.rows.iter().any(|row| {
                row.id == self.session_id
                    && row.token == self.token
                    && !row.is_root_agent
                    && !row.is_coordinator
                    && !matches!(row.status, SessionStatus::Exited(_))
            })
            && rows
                .iter()
                .filter(|row| {
                    row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok())
                        == Some(self.session_id)
                })
                .count()
                == 1
    }
}

#[derive(Clone)]
pub(crate) struct PublishedCarrier {
    pub target: String,
    pub session_id: Uuid,
    pub token: Uuid,
    pub nonce: Uuid,
    pub version: u64,
    slot: PublicationSlot,
    pub directory: PathBuf,
    lifetime: std::sync::Arc<()>,
    retired_local: Option<RetiredLocalCarrierFact>,
}

impl PublishedCarrier {
    /// Historical generation, never a live route or writer capability.
    pub(crate) fn retired_local_generation(&self) -> Option<u64> {
        self.retired_local.as_ref().map(|fact| fact.generation)
    }
    pub(crate) fn retired_local_replica_matches(
        &self,
        physical: &crate::session::manager::CarrierPhysicalIdentity,
    ) -> bool {
        self.retired_local
            .as_ref()
            .is_some_and(|fact| fact.physical_replica.same_object(physical))
    }

    /// Read-only first-write check under the injector's existing current M and
    /// route writer guard. No U acquisition, filesystem probe or publication.
    pub(crate) fn matches_owned_work_snapshot(
        &self,
        current: &crate::session::manager::CarrierSnapshot,
        canonical_cwd: &Path,
    ) -> bool {
        if self.version == u64::MAX
            || current.versions.get(&self.session_id) != Some(&self.version)
            || current.pending_ids.contains(&self.session_id)
            || !current
                .rows
                .iter()
                .any(|row| row.id == self.session_id && row.token == self.token)
        {
            return false;
        }
        let Some(protections) = all_carrier_protections() else {
            return false;
        };
        let mut related = protections.iter().filter(|protection| {
            protection.target == self.target
                || canonical_cwd.starts_with(&protection.canonical_cwd)
                || protection.canonical_cwd.starts_with(canonical_cwd)
        });
        let Some(protection) = related.next() else {
            return false;
        };
        if related.next().is_some()
            || protection.target != self.target
            || protection.session_id != self.session_id
            || protection.nonce != self.nonce
            || protection.token != self.token
            || protection.version != self.version
            || protection.phase != ProtectionPhase::Protected
            || !canonical_cwd.starts_with(&protection.canonical_cwd)
        {
            return false;
        }
        let Ok(state) = self.slot.lock() else {
            return false;
        };
        state.checkpoint.as_ref().is_some_and(|checkpoint| {
            checkpoint.confirmed
                && checkpoint.epoch < u64::MAX
                && checkpoint.epoch == state.epoch
                && protection.epoch == checkpoint.epoch
                && checkpoint.versions.get(&self.session_id) == Some(&self.version)
                && checkpoint
                    .rows
                    .iter()
                    .any(|row| same_carrier_projection(row, &protection.row))
        })
    }
}

pub(crate) struct CurrentCarrierCheckpoint {
    pub nonce: Uuid,
    pub epoch: u64,
    pub version: u64,
}

fn prepare_protected_publication(
    next: &mut PublicationState,
    rows: &[PersistedSession],
    captured: &crate::session::manager::CarrierSnapshot,
    initial: Option<&InitialCarrierIntent>,
) -> bool {
    next.protections
        .retain(|protection| protection.lifetime.strong_count() != 0);
    for protection in &mut next.protections {
        let mut matches = rows.iter().filter(|row| {
            row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) == Some(protection.session_id)
        });
        let Some(row) = matches.next() else {
            return false;
        };
        let Some(version) = captured.versions.get(&protection.session_id) else {
            return false;
        };
        if matches.next().is_some()
            || captured.pending_ids.contains(&protection.session_id)
            || !captured.rows.iter().any(|current| {
                current.id == protection.session_id && current.token == protection.token
            })
        {
            return false;
        }
        if !captured.rows.iter().any(|current| {
            current.id == protection.session_id
                && current.token == protection.token
                && crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(current)
                && matches!(current.status, SessionStatus::Exited(0))
                && current.working_directory == protection.row.working_directory
        }) {
            protection.retired_local = None;
        }
        protection.row = row.clone();
        protection.version = *version;
        protection.epoch = next.epoch;
    }
    let Some(initial) = initial else {
        return true;
    };
    if !initial.matches(captured, rows) {
        return false;
    }
    let Some(all) = all_carrier_protections() else {
        return false;
    };
    if all.iter().any(|protection| {
        (protection.session_id == initial.session_id || protection.target == initial.target)
            && !next
                .protections
                .iter()
                .any(|own| own.nonce == protection.nonce && own.session_id == protection.session_id)
    }) {
        return false;
    }
    if let Some(protection) = next
        .protections
        .iter_mut()
        .find(|protection| protection.session_id == initial.session_id)
    {
        if protection.target != initial.target
            || protection.token != initial.token
            || !matches!(protection.phase, ProtectionPhase::Protected)
        {
            return false;
        }
        if protection.retained.is_none() {
            protection.lifetime = std::sync::Arc::downgrade(&initial.lifetime);
            protection.pending_lifetime = Some(initial.lifetime.clone());
        }
        return true;
    }
    let Some(row) = rows.iter().find(|row| {
        row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) == Some(initial.session_id)
    }) else {
        return false;
    };
    next.protections.push(CarrierProtection {
        target: initial.target.clone(),
        session_id: initial.session_id,
        token: initial.token,
        nonce: Uuid::new_v4(),
        epoch: next.epoch,
        version: initial.version,
        row: row.clone(),
        canonical_cwd: initial.canonical_replica.clone(),
        phase: ProtectionPhase::Protected,
        retired_local: None,
        lifetime: std::sync::Arc::downgrade(&initial.lifetime),
        retained: None,
        pending_lifetime: Some(initial.lifetime.clone()),
        active: None,
    });
    true
}

fn published_initial_carrier(
    slot: &PublicationSlot,
    initial: &InitialCarrierIntent,
    dir: &Path,
) -> Option<PublishedCarrier> {
    let state = slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed && checkpoint.epoch < u64::MAX && checkpoint.epoch == state.epoch
    })?;
    let protection = state.protections.iter().find(|protection| {
        protection.target == initial.target
            && protection.session_id == initial.session_id
            && protection.token == initial.token
            && protection.version == initial.version
            && protection.epoch == checkpoint.epoch
            && matches!(protection.phase, ProtectionPhase::Protected)
    })?;
    if checkpoint.versions.get(&initial.session_id) != Some(&initial.version) {
        return None;
    }
    let lifetime = protection.lifetime.upgrade()?;
    Some(PublishedCarrier {
        target: protection.target.clone(),
        session_id: protection.session_id,
        token: protection.token,
        nonce: protection.nonce,
        version: protection.version,
        slot: slot.clone(),
        directory: dir.to_path_buf(),
        lifetime,
        retired_local: None,
    })
}

/// Capture the retained current A key for an authenticated upper replacement.
/// No CWD, recipe, or UUID-only lookup creates replacement authority.
pub(crate) fn retained_carrier_for_replacement(
    _permit: &CarrierMutationPermit,
    directory: &Path,
    target: &str,
    session_id: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let row = current.rows.iter().find(|row| {
        row.id == session_id
            && matches!(row.status, SessionStatus::Exited(0))
            && crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(row)
            && !current.pending_ids.contains(&row.id)
    })?;
    let version = *current.versions.get(&session_id)?;
    let scope = classify_create_carrier_scope(_permit, target, Path::new(&row.working_directory));
    let slot = publication_slots().lock().ok()?.get(directory)?.clone();
    let state = slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch < u64::MAX
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&session_id) == Some(&version)
    })?;
    let mut matches = state.protections.iter().filter(|protection| {
        protection.session_id == session_id
            && protection.target == target
            && protection.token == row.token
            && protection.version == version
            && protection.epoch == checkpoint.epoch
            && protection.phase == ProtectionPhase::Protected
            && protection.lifetime.strong_count() != 0
    });
    let protection = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let fact = protection.retired_local.as_ref()?;
    if fact.session_id != session_id
        || fact.token != row.token
        || fact.target != target
        || fact.protection_nonce != protection.nonce
        || fact.canonical_replica != protection.canonical_cwd
        || fact.committed_version > fact.settled_version
        || fact.settled_version > version
        || fact.publication_epoch > checkpoint.epoch
        || fact.publication_epoch == u64::MAX
        || (fact.publication_epoch == checkpoint.epoch
            && (fact.checkpoint_nonce != checkpoint.nonce || fact.settled_version != version))
        || !checkpoint
            .rows
            .iter()
            .any(|written| same_carrier_projection(written, &protection.row))
        || !matches!(scope,
            CreateCarrierScope::Protected { session_id: actual, token, version: actual_version,
                epoch, preparing_owner: None, .. }
            if actual == session_id && token == row.token && actual_version == version && epoch == checkpoint.epoch)
    {
        return None;
    }
    let lifetime = protection.lifetime.upgrade()?;
    Some(PublishedCarrier {
        target: target.to_owned(),
        session_id,
        token: row.token,
        nonce: protection.nonce,
        version,
        slot: slot.clone(),
        directory: directory.to_path_buf(),
        lifetime,
        retired_local: protection.retired_local.clone(),
    })
}

/// A strict current protection for an explicit manual discard. This proves no
/// route ownership; the transient upper executor must also own the tracker claim.
pub(crate) fn manual_discard_carrier(
    permit: &CarrierMutationPermit,
    canonical_source: &Path,
    target: &str,
    session_id: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let row = current
        .rows
        .iter()
        .find(|row| row.id == session_id && !current.pending_ids.contains(&row.id))?;
    let version = *current.versions.get(&session_id)?;
    if !matches!(classify_lifecycle_carrier_scope(permit, Some(target), canonical_source),
        CreateCarrierScope::Protected { session_id: id, token, version: actual, preparing_owner: None, .. }
        if id == session_id && token == row.token && actual == version)
    {
        return None;
    }
    let protections = all_carrier_protections()?;
    let mut own = protections.iter().filter(|p| p.session_id == session_id);
    let protection = own.next()?;
    if own.next().is_some()
        || protection.target != target
        || protection.token != row.token
        || protection.version != version
        || protection.phase != ProtectionPhase::Protected
    {
        return None;
    }
    let candidates = publication_slots()
        .lock()
        .ok()?
        .iter()
        .map(|(directory, slot)| (directory.clone(), slot.clone()))
        .collect::<Vec<_>>();
    let mut found: Option<PublishedCarrier> = None;
    for (directory, slot) in candidates {
        if found
            .as_ref()
            .is_some_and(|prior| std::sync::Arc::ptr_eq(&prior.slot, &slot))
        {
            continue;
        }
        let state = slot.lock().ok()?;
        if !state
            .protections
            .iter()
            .any(|p| p.nonce == protection.nonce && p.session_id == session_id)
        {
            continue;
        }
        if found.is_some() {
            return None;
        }
        state.checkpoint.as_ref().filter(|checkpoint| {
            checkpoint.confirmed
                && checkpoint.epoch < u64::MAX
                && checkpoint.epoch == state.epoch
                && checkpoint.epoch == protection.epoch
                && checkpoint.versions.get(&session_id) == Some(&version)
                && checkpoint
                    .rows
                    .iter()
                    .any(|written| same_carrier_projection(written, &protection.row))
        })?;
        found = Some(PublishedCarrier {
            target: target.to_owned(),
            session_id,
            token: row.token,
            nonce: protection.nonce,
            version,
            slot: slot.clone(),
            directory,
            lifetime: protection.lifetime.upgrade()?,
            retired_local: protection.retired_local.clone(),
        });
    }
    let carrier = found?;
    if carrier.retired_local.is_some() {
        // Historical evidence is accepted only through the full retired accessor.
        return retained_carrier_for_replacement(
            permit,
            &carrier.directory,
            target,
            session_id,
            current,
        );
    }
    Some(carrier)
}

pub(crate) fn retire_manual_discard_carrier(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    canonical_source: &Path,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    let Some(actual) = manual_discard_carrier(
        permit,
        canonical_source,
        &carrier.target,
        carrier.session_id,
        current,
    ) else {
        return false;
    };
    actual.nonce == carrier.nonce
        && actual.token == carrier.token
        && actual.version == carrier.version
        && std::sync::Arc::ptr_eq(&actual.slot, &carrier.slot)
        && retire_carrier_for_discard(permit, carrier.session_id)
}

pub(crate) fn begin_carrier_replacement(
    _permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    replacement: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    if carrier.retired_local.is_none()
        || replacement == carrier.session_id
        || current.rows.iter().any(|row| row.id == replacement)
        || current.versions.get(&carrier.session_id) != Some(&carrier.version)
        || !current.rows.iter().any(|row| {
            row.id == carrier.session_id
                && row.token == carrier.token
                && matches!(row.status, SessionStatus::Exited(_))
                && !current.pending_ids.contains(&row.id)
        })
    {
        return false;
    }
    let Ok(mut state) = carrier.slot.lock() else {
        return false;
    };
    let Some(checkpoint) = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch < u64::MAX
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&carrier.session_id) == Some(&carrier.version)
    }) else {
        return false;
    };
    let epoch = checkpoint.epoch;
    let Some(protection) = state.protections.iter_mut().find(|protection| {
        protection.target == carrier.target
            && protection.session_id == carrier.session_id
            && protection.token == carrier.token
            && protection.nonce == carrier.nonce
            && protection.version == carrier.version
            && protection.epoch == epoch
            && protection.phase == ProtectionPhase::Protected
            && protection.retired_local == carrier.retired_local
    }) else {
        return false;
    };
    protection.phase = ProtectionPhase::Preparing {
        owner_nonce,
        previous: carrier.session_id,
        replacement,
    };
    true
}

/// Renew only the same operation's full Preparing relation. A historical key
/// never changes UUID; the returned current handle keeps its actual slot/nonce.
pub(crate) fn renew_carrier_replacement(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    replacement: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let row = current.rows.iter().find(|row| {
        row.id == carrier.session_id
            && row.token == carrier.token
            && !current.pending_ids.contains(&row.id)
    })?;
    if carrier.session_id == previous
        && (!matches!(row.status, SessionStatus::Exited(0))
            || !crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(row))
    {
        return None;
    }
    let version = *current.versions.get(&row.id)?;
    if !matches!(classify_create_carrier_scope(permit, &carrier.target,
        Path::new(&row.working_directory)), CreateCarrierScope::Protected {
            session_id, token, version: actual, preparing_owner: Some(owner), ..
        } if session_id == row.id && token == row.token && actual == version && owner == owner_nonce)
    {
        return None;
    }
    let state = carrier.slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch < u64::MAX
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&row.id) == Some(&version)
    })?;
    let protection = state.protections.iter().find(|protection| {
        protection.session_id == row.id
            && protection.token == row.token
            && protection.nonce == carrier.nonce
            && protection.target == carrier.target
            && protection.version == version
            && protection.epoch == checkpoint.epoch
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous,
                    replacement,
                }
            && checkpoint
                .rows
                .iter()
                .any(|written| same_carrier_projection(written, &protection.row))
    })?;
    Some(PublishedCarrier {
        target: carrier.target.clone(),
        session_id: row.id,
        token: row.token,
        nonce: protection.nonce,
        version,
        slot: carrier.slot.clone(),
        directory: carrier.directory.clone(),
        lifetime: protection.lifetime.upgrade()?,
        retired_local: protection.retired_local.clone(),
    })
}

/// Current Protected-B completion proof; this reads the actual checkpoint and
/// never updates a handle to a version that has not been confirmed on disk.
pub(crate) fn current_protected_carrier(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let row = current.rows.iter().find(|row| {
        row.id == carrier.session_id
            && row.token == carrier.token
            && !current.pending_ids.contains(&row.id)
    })?;
    let version = *current.versions.get(&row.id)?;
    if version == u64::MAX {
        return None;
    }
    if !matches!(classify_create_carrier_scope(permit, &carrier.target, Path::new(&row.working_directory)),
        CreateCarrierScope::Protected { session_id, token, version: actual, preparing_owner: None, .. }
        if session_id == row.id && token == row.token && actual == version)
    {
        return None;
    }
    let state = carrier.slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch < u64::MAX
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&row.id) == Some(&version)
    })?;
    let protection = state.protections.iter().find(|protection| {
        protection.session_id == row.id
            && protection.nonce == carrier.nonce
            && protection.token == row.token
            && protection.version == version
            && protection.epoch == checkpoint.epoch
            && protection.phase == ProtectionPhase::Protected
            && checkpoint
                .rows
                .iter()
                .any(|written| same_carrier_projection(written, &protection.row))
    })?;
    Some(PublishedCarrier {
        target: carrier.target.clone(),
        session_id: row.id,
        token: row.token,
        nonce: protection.nonce,
        version,
        slot: carrier.slot.clone(),
        directory: carrier.directory.clone(),
        lifetime: protection.lifetime.upgrade()?,
        retired_local: None,
    })
}

/// Release the operation's Preparing only after B is strict Confirmed and A
/// has really been removed from the current manager snapshot under this same U.
pub(crate) fn finish_carrier_replacement(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    if current.rows.iter().any(|row| row.id == previous) {
        return false;
    }
    let Some(actual) = renew_carrier_replacement(
        permit,
        carrier,
        owner_nonce,
        previous,
        carrier.session_id,
        current,
    ) else {
        return false;
    };
    let Ok(mut state) = carrier.slot.lock() else {
        return false;
    };
    let Some(protection) = state.protections.iter_mut().find(|protection| {
        protection.session_id == actual.session_id
            && protection.nonce == actual.nonce
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous,
                    replacement: actual.session_id,
                }
    }) else {
        return false;
    };
    protection.phase = ProtectionPhase::Protected;
    true
}

/// Pre-rename rollback releases only this operation's A-side Preparing.
/// A current strict checkpoint must still prove A, and B must be absent.
pub(crate) fn abort_owned_carrier_replacement(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    replacement: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    if current.rows.iter().any(|row| row.id == replacement) {
        return false;
    }
    let Some(actual) = renew_carrier_replacement(
        permit,
        carrier,
        owner_nonce,
        carrier.session_id,
        replacement,
        current,
    ) else {
        return false;
    };
    let Ok(mut state) = carrier.slot.lock() else {
        return false;
    };
    let Some(protection) = state.protections.iter_mut().find(|protection| {
        protection.session_id == actual.session_id
            && protection.nonce == actual.nonce
            && protection.token == actual.token
            && protection.version == actual.version
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous: actual.session_id,
                    replacement,
                }
    }) else {
        return false;
    };
    protection.phase = ProtectionPhase::Protected;
    true
}

/// The generic callback runs only under the caller's U/current M region. It
/// supplies tracker primitives in the upper layer; no tracker dependency enters here.
pub(crate) fn register_published_carrier<T>(
    _permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    current: &crate::session::manager::CarrierSnapshot,
    register: impl FnOnce(CurrentCarrierCheckpoint) -> Option<T>,
) -> Option<T> {
    let mut state = carrier.slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed && checkpoint.epoch < u64::MAX && checkpoint.epoch == state.epoch
    })?;
    if current.versions.get(&carrier.session_id) != Some(&carrier.version)
        || checkpoint.versions.get(&carrier.session_id) != Some(&carrier.version)
        || !current
            .rows
            .iter()
            .any(|row| row.id == carrier.session_id && row.token == carrier.token)
        || current.pending_ids.contains(&carrier.session_id)
    {
        return None;
    }
    let stamp = CurrentCarrierCheckpoint {
        nonce: checkpoint.nonce,
        epoch: checkpoint.epoch,
        version: carrier.version,
    };
    let protection = state.protections.iter_mut().find(|protection| {
        protection.session_id == carrier.session_id
            && protection.token == carrier.token
            && protection.nonce == carrier.nonce
            && protection.target == carrier.target
            && protection.version == carrier.version
            && protection.epoch == stamp.epoch
            && matches!(protection.phase, ProtectionPhase::Protected)
    })?;
    let result = register(stamp)?;
    protection.retained = Some(carrier.lifetime.clone());
    Some(result)
}

#[derive(Clone, Copy)]
pub(crate) enum CarrierCheckpointTransition<'a> {
    BeginClosing,
    FinishClosing,
    FinishDormant {
        generation: u64,
        committed_version: u64,
        replica: &'a crate::session::manager::CarrierPhysicalIdentity,
    },
}

/// Caller holds the current interior manager read under U. The opaque upper
/// transition runs while this same Confirmed slot remains locked; phase changes
/// occur only after an observed successful tracker transition.
pub(crate) fn with_confirmed_carrier<T>(
    _permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    current: &crate::session::manager::CarrierSnapshot,
    transition: CarrierCheckpointTransition<'_>,
    apply: impl FnOnce(CurrentCarrierCheckpoint) -> Option<T>,
) -> Option<T> {
    let mut state = carrier.slot.lock().ok()?;
    let version = *current.versions.get(&carrier.session_id)?;
    if version < carrier.version
        || current.pending_ids.contains(&carrier.session_id)
        || !current
            .rows
            .iter()
            .any(|row| row.id == carrier.session_id && row.token == carrier.token)
    {
        return None;
    }
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&carrier.session_id) == Some(&version)
    })?;
    let stamp = CurrentCarrierCheckpoint {
        nonce: checkpoint.nonce,
        epoch: checkpoint.epoch,
        version,
    };
    let protection = state.protections.iter_mut().find(|protection| {
        protection.session_id == carrier.session_id
            && protection.token == carrier.token
            && protection.target == carrier.target
            && protection.nonce == carrier.nonce
            && protection.epoch == stamp.epoch
            && protection.version == stamp.version
    })?;
    let phase_matches = match transition {
        CarrierCheckpointTransition::BeginClosing => {
            stamp.epoch < u64::MAX
                && protection.phase == ProtectionPhase::Protected
                && version == carrier.version
        }
        CarrierCheckpointTransition::FinishClosing
        | CarrierCheckpointTransition::FinishDormant { .. } => {
            protection.phase == ProtectionPhase::Closing
        }
    };
    if !phase_matches {
        return None;
    }
    // Prepare owned data BEFORE the irreversible tracker transition. Afterwards
    // only infallible moves publish history and Protected under the same slot/U.
    let retirement = match transition {
        CarrierCheckpointTransition::FinishDormant {
            generation,
            committed_version,
            replica,
        } => {
            let row = current
                .rows
                .iter()
                .find(|row| row.id == carrier.session_id)?;
            if committed_version != carrier.version
                || committed_version > version
                || row.token != carrier.token
                || !crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(row)
                || !matches!(row.status, SessionStatus::Exited(0))
                || current.selection.id() == Some(row.id)
                || replica.canonical_path() != protection.canonical_cwd
                || !replica.is_dir()
            {
                return None;
            }
            Some(RetiredLocalCarrierFact {
                session_id: carrier.session_id,
                token: carrier.token,
                target: carrier.target.clone(),
                protection_nonce: carrier.nonce,
                canonical_replica: protection.canonical_cwd.clone(),
                physical_replica: replica.clone(),
                generation,
                committed_version,
                settled_version: stamp.version,
                checkpoint_nonce: stamp.nonce,
                publication_epoch: stamp.epoch,
            })
        }
        _ => None,
    };
    let result = apply(stamp)?;
    protection.retired_local = retirement;
    protection.phase = match transition {
        CarrierCheckpointTransition::BeginClosing => ProtectionPhase::Closing,
        CarrierCheckpointTransition::FinishClosing
        | CarrierCheckpointTransition::FinishDormant { .. } => ProtectionPhase::Protected,
    };
    Some(result)
}

pub(crate) fn retire_carrier_for_discard(_permit: &CarrierMutationPermit, id: Uuid) -> bool {
    let slots = match publication_slots().lock() {
        Ok(slots) => slots.values().cloned().collect::<Vec<_>>(),
        Err(_) => return false,
    };
    for slot in &slots {
        let Ok(state) = slot.lock() else {
            return false;
        };
        if state
            .protections
            .iter()
            .any(|protection| match protection.phase {
                ProtectionPhase::Closing => protection.session_id == id,
                ProtectionPhase::Preparing {
                    previous,
                    replacement,
                    ..
                } => protection.session_id == id || previous == id || replacement == id,
                ProtectionPhase::Protected => false,
            })
        {
            return false;
        }
    }
    for slot in slots {
        let Ok(mut state) = slot.lock() else {
            return false;
        };
        state
            .protections
            .retain(|protection| protection.session_id != id);
    }
    true
}

fn all_carrier_protections() -> Option<Vec<CarrierProtection>> {
    let slots = publication_slots()
        .lock()
        .ok()?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let mut protections = Vec::new();
    let mut seen = HashSet::new();
    for slot in slots {
        if !seen.insert(std::sync::Arc::as_ptr(&slot) as usize) {
            continue;
        }
        let state = slot.lock().ok()?;
        for protection in &state.protections {
            if let Some(active) = protection.lifetime.upgrade() {
                let mut protection = protection.clone();
                protection.active = Some(active);
                protections.push(protection);
            }
        }
    }
    Some(protections)
}

/// Veto only: an unowned delivery cannot use an unconfirmed replacement B.
/// False supplies no Foreign, checkpoint, route or replacement authority.
pub(crate) fn unconfirmed_replacement_work_waits(target: &str, id: Uuid) -> bool {
    all_carrier_protections().is_none_or(|protections| {
        protections.iter().any(|protection| {
            protection.target == target
                && matches!(protection.phase,
            ProtectionPhase::Preparing { replacement, .. } if replacement == id)
        })
    })
}

/// Pure ALL-scopes startup precondition. None is unreadable ownership, never
/// evidence that no publication protection exists.
pub(crate) fn restore_carrier_scopes_are_empty(_permit: &CarrierMutationPermit) -> Option<bool> {
    Some(all_carrier_protections()?.is_empty())
}

fn same_carrier_projection(a: &PersistedSession, b: &PersistedSession) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.shell == b.shell
        && a.shell_args == b.shell_args
        && a.working_directory == b.working_directory
        && a.was_active == b.was_active
        && a.git_repos.len() == b.git_repos.len()
        && a.git_repos
            .iter()
            .zip(&b.git_repos)
            .all(|(a, b)| a.label == b.label && a.source_path == b.source_path)
        && a.is_coordinator == b.is_coordinator
        && a.is_root_agent == b.is_root_agent
        && a.agent_id == b.agent_id
        && a.agent_label == b.agent_label
        && a.requested_profile == b.requested_profile
        && a.effective_profile == b.effective_profile
        && a.telegram_bot_id == b.telegram_bot_id
        && a.was_detached == b.was_detached
        && a.start_fresh_on_restore == b.start_fresh_on_restore
        && a.communication == b.communication
        && match (&a.status, &b.status) {
            (Some(SessionStatus::Exited(a)), Some(SessionStatus::Exited(b))) => a == b,
            (Some(SessionStatus::Exited(_)), _) | (_, Some(SessionStatus::Exited(_))) => false,
            _ => true,
        }
}

fn same_written_projection(expected: &[PersistedSession], actual: &[PersistedSession]) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(expected, actual)| {
            same_carrier_projection(expected, actual) && expected.status == actual.status
        })
}

/// Current ALL-scopes create relation. This is a publication classification,
/// never route authority; the upper owner validates current manager facts.
pub(crate) enum CreateCarrierScope {
    Foreign {
        publication_epoch: u64,
    },
    Protected {
        target: String,
        session_id: Uuid,
        token: Uuid,
        version: u64,
        epoch: u64,
        preparing_owner: Option<Uuid>,
    },
    Waiting,
}

pub(crate) fn classify_create_carrier_scope(
    permit: &CarrierMutationPermit,
    target: &str,
    cwd: &Path,
) -> CreateCarrierScope {
    // Preserve the literal zero-protection legacy path, including absent CWDs.
    let Some(protections) = all_carrier_protections() else {
        return CreateCarrierScope::Waiting;
    };
    if protections.is_empty() {
        return CreateCarrierScope::Foreign {
            publication_epoch: 0,
        };
    }
    let Ok(canonical) = std::fs::canonicalize(cwd) else {
        return CreateCarrierScope::Waiting;
    };
    classify_lifecycle_carrier_scope(permit, Some(target), &canonical)
}

pub(crate) fn classify_lifecycle_carrier_scope(
    _permit: &CarrierMutationPermit,
    target: Option<&str>,
    canonical: &Path,
) -> CreateCarrierScope {
    let Some(protections) = all_carrier_protections() else {
        return CreateCarrierScope::Waiting;
    };
    if protections.is_empty() {
        return CreateCarrierScope::Foreign {
            publication_epoch: 0,
        };
    }
    let publication_epoch = protections
        .iter()
        .map(|protection| protection.epoch)
        .max()
        .unwrap_or(0);
    let mut related = None;
    for protection in protections {
        let intersects = canonical.starts_with(&protection.canonical_cwd)
            || protection.canonical_cwd.starts_with(canonical);
        let same_target = target.is_some_and(|target| protection.target == target);
        if !same_target && !intersects {
            continue;
        }
        if target.is_some_and(|target| protection.target != target)
            || !canonical.starts_with(&protection.canonical_cwd)
            || related.is_some()
            || protection.phase == ProtectionPhase::Closing
        {
            return CreateCarrierScope::Waiting;
        }
        related = Some(protection);
    }
    match related {
        None => CreateCarrierScope::Foreign { publication_epoch },
        Some(protection) => CreateCarrierScope::Protected {
            target: protection.target,
            session_id: protection.session_id,
            token: protection.token,
            version: protection.version,
            epoch: protection.epoch,
            preparing_owner: match protection.phase {
                ProtectionPhase::Preparing { owner_nonce, .. } => Some(owner_nonce),
                _ => None,
            },
        },
    }
}

struct PreparedCarrierRelation {
    id: Option<Uuid>,
    cwd: String,
    canonical_cwd: Option<PathBuf>,
}

impl PreparedCarrierRelation {
    fn capture(row: &PersistedSession, protections: &[CarrierProtection]) -> Self {
        Self {
            id: row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()),
            cwd: row.working_directory.clone(),
            canonical_cwd: (!protections.is_empty())
                .then(|| std::fs::canonicalize(&row.working_directory).ok())
                .flatten(),
        }
    }

    fn capture_manager(
        row: &crate::session::session::Session,
        protections: &[CarrierProtection],
    ) -> Self {
        Self {
            id: Some(row.id),
            cwd: row.working_directory.clone(),
            canonical_cwd: (!protections.is_empty())
                .then(|| std::fs::canonicalize(&row.working_directory).ok())
                .flatten(),
        }
    }

    fn classify(&self, protections: &[CarrierProtection]) -> CarrierDisposition {
        if protections.is_empty() {
            return CarrierDisposition::Foreign;
        }
        for protection in protections {
            if self.id == Some(protection.session_id) {
                return CarrierDisposition::Protected;
            }
            let Some(cwd) = self.canonical_cwd.as_ref() else {
                return CarrierDisposition::Unknown;
            };
            if self.cwd == protection.row.working_directory
                || cwd.starts_with(&protection.canonical_cwd)
                || protection.canonical_cwd.starts_with(cwd)
            {
                return CarrierDisposition::Unknown;
            }
        }
        CarrierDisposition::Foreign
    }
}

enum GuardedSaveAttempt {
    Completed(Result<bool, String>),
    Recapture,
}

enum RenamePublicationAttempt {
    Renamed {
        leaf: std::sync::MutexGuard<'static, ()>,
        retired: PublicationState,
    },
    Suppressed,
    Retry,
    Stale,
    Failed(std::io::Error),
}

fn current_capture_matches(
    current: &crate::session::manager::CarrierSnapshot,
    captured: &crate::session::manager::CarrierSnapshot,
) -> bool {
    current.versions == captured.versions
        && current.pending_ids == captured.pending_ids
        && current.order == captured.order
        && current.selection.id() == captured.selection.id()
        && current.selection.mode() == captured.selection.mode()
        && current.selection.epoch() == captured.selection.epoch()
        && current.selection.revision() == captured.selection.revision()
}

struct PreparedSessionsWrite {
    path: PathBuf,
    tmp_path: PathBuf,
    op_id: u64,
    pid: u32,
    instance: String,
    started: std::time::Instant,
    final_existed: bool,
}

impl PreparedSessionsWrite {
    fn prepare(dir: &Path, sessions: &[PersistedSession]) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("Failed to create config directory: {}", error))?;
        let json = serde_json::to_string_pretty(sessions)
            .map_err(|error| format!("Failed to serialize sessions: {}", error))?;
        let op_id = SAVE_OP_ID.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_path = dir.join(format!("sessions.json.{}.{}.tmp", pid, op_id));
        if let Err(error) = std::fs::write(&tmp_path, json) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("Failed to write temp sessions file: {}", error));
        }
        let path = dir.join("sessions.json");
        Ok(Self {
            final_existed: path.exists(),
            path,
            tmp_path,
            op_id,
            pid,
            instance: crate::config::agent_local_dir_name().to_owned(),
            started: std::time::Instant::now(),
        })
    }

    fn terminal_error(&self, error: &std::io::Error) -> String {
        format!(
            "Failed to rename sessions file: {} [op_id={} pid={} instance={} attempts={} \
             tmp_existed_before={} final_existed_before={} os_error={:?} duration={:?}]",
            error,
            self.op_id,
            self.pid,
            self.instance,
            RENAME_ATTEMPTS,
            true,
            self.final_existed,
            error.raw_os_error(),
            self.started.elapsed(),
        )
    }
}

impl Drop for PreparedSessionsWrite {
    fn drop(&mut self) {
        // The unique temporary name belongs to this operation only. After
        // rename it is absent; cleanup never removes the published file.
        let _ = std::fs::remove_file(&self.tmp_path);
    }
}

fn rename_and_publish(
    dir: &Path,
    slot: &PublicationSlot,
    expected_epoch: u64,
    next: &mut Option<PublicationState>,
    write: &PreparedSessionsWrite,
) -> RenamePublicationAttempt {
    let guarded = super::settings::try_with_agents_layer_publication_state(dir, |suppressed| {
        if suppressed {
            return RenamePublicationAttempt::Suppressed;
        }
        let leaf = match SAVE_SESSIONS_LOCK.try_lock() {
            Ok(leaf) => leaf,
            Err(_) => return RenamePublicationAttempt::Retry,
        };
        let mut state = match slot.try_lock() {
            Ok(state) => state,
            Err(_) => return RenamePublicationAttempt::Retry,
        };
        if state.epoch != expected_epoch {
            return RenamePublicationAttempt::Stale;
        }
        let Some(pending) = next.take() else {
            return RenamePublicationAttempt::Stale;
        };
        match std::fs::rename(&write.tmp_path, &write.path) {
            Ok(()) => {
                // The slot and payload are resolved BEFORE the syscall. Keep
                // the displaced payload alive beyond this critical transition.
                let retired = std::mem::replace(&mut *state, pending);
                RenamePublicationAttempt::Renamed { leaf, retired }
            }
            Err(error) => {
                *next = Some(pending);
                RenamePublicationAttempt::Failed(error)
            }
        }
    });
    guarded.unwrap_or(RenamePublicationAttempt::Retry)
}

fn confirm_publication_readback(slot: &PublicationSlot, path: &Path, nonce: Uuid, epoch: u64) {
    let actual = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<PersistedSession>>(&bytes).ok());
    let Some(actual) = actual else {
        return;
    };
    let Ok(mut state) = slot.lock() else {
        return;
    };
    if state.epoch != epoch {
        return;
    }
    if let Some(checkpoint) = state.checkpoint.as_mut().filter(|checkpoint| {
        checkpoint.nonce == nonce && checkpoint.epoch == epoch && !checkpoint.confirmed
    }) {
        if same_written_projection(&checkpoint.rows, &actual) {
            checkpoint.confirmed = true;
        }
    }
    let confirmed = state.checkpoint.as_ref().is_some_and(|checkpoint| {
        checkpoint.confirmed && checkpoint.nonce == nonce && checkpoint.epoch == epoch
    });
    if confirmed {
        for protection in &mut state.protections {
            if protection.epoch == epoch {
                protection.pending_lifetime = None;
            }
        }
    }
}

fn file_preparation_survives(
    slot: &PublicationSlot,
    protections: &[CarrierProtection],
    prep: &PersistPreparation,
) -> bool {
    if prep.drops.iter().any(|drop| {
        PreparedCarrierRelation::capture(&drop.session, protections).classify(protections)
            != CarrierDisposition::Foreign
    }) {
        return false;
    }
    if prep.kept.iter().any(|row| {
        PreparedCarrierRelation::capture(row, protections).classify(protections)
            == CarrierDisposition::Unknown
    }) {
        return false;
    }
    let Ok(state) = slot.lock() else {
        return false;
    };
    state
        .protections
        .iter()
        .filter(|protection| protection.lifetime.strong_count() != 0)
        .all(|protection| {
            let mut rows = prep.kept.iter().filter(|row| {
                row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok())
                    == Some(protection.session_id)
            });
            let Some(row) = rows.next() else {
                return false;
            };
            rows.next().is_none() && same_carrier_projection(row, &protection.row)
        })
}

async fn save_file_snapshot_guarded(
    dir: &Path,
    slot: &PublicationSlot,
    rows: &[PersistedSession],
) -> GuardedSaveAttempt {
    let (expected, mut prepared) = {
        let Ok(state) = slot.lock() else {
            return GuardedSaveAttempt::Recapture;
        };
        let Some(epoch) = state.epoch.checked_add(1) else {
            return GuardedSaveAttempt::Recapture;
        };
        let mut next = state.clone();
        next.epoch = epoch;
        for protection in &mut next.protections {
            protection.epoch = epoch;
        }
        next.checkpoint = Some(PublicationCheckpoint {
            nonce: Uuid::new_v4(),
            epoch,
            rows: rows.to_vec(),
            versions: HashMap::new(),
            confirmed: false,
        });
        (state.epoch, next)
    };
    if super::settings::agents_layer_unreadable_for_dir(dir) {
        return GuardedSaveAttempt::Completed(Ok(false));
    }
    let write = match PreparedSessionsWrite::prepare(dir, rows) {
        Ok(write) => write,
        Err(error) => return GuardedSaveAttempt::Completed(Err(error)),
    };
    let checkpoint = prepared
        .checkpoint
        .as_ref()
        .expect("prepared file checkpoint");
    let (nonce, epoch) = (checkpoint.nonce, checkpoint.epoch);
    let mut next = Some(std::mem::take(&mut prepared));
    for attempt in 0..RENAME_ATTEMPTS {
        match settle_rename_attempt(
            rename_and_publish(dir, slot, expected, &mut next, &write),
            slot,
            &write.path,
            nonce,
            epoch,
            rows.len(),
            #[cfg(test)]
            None,
        ) {
            SettledRenameAttempt::Completed(written) => {
                return GuardedSaveAttempt::Completed(Ok(written))
            }
            SettledRenameAttempt::Recapture => return GuardedSaveAttempt::Recapture,
            SettledRenameAttempt::Failed(error) => {
                log::debug!("[sessions] rename attempt {}/{} failed — op_id={} pid={} os_error={:?} kind={:?}", attempt + 1, RENAME_ATTEMPTS, write.op_id, write.pid, error.raw_os_error(), error.kind());
                if attempt + 1 == RENAME_ATTEMPTS {
                    return GuardedSaveAttempt::Completed(Err(write.terminal_error(&error)));
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(
            RENAME_BACKOFFS_MS[attempt as usize],
        ))
        .await;
    }
    unreachable!("RENAME_ATTEMPTS is positive")
}

enum SettledRenameAttempt {
    Completed(bool),
    Recapture,
    Failed(std::io::Error),
}

/// Consume every non-Send leaf guard synchronously. Only this Send result can
/// reach the caller's retry await; readback still holds the exact original leaf.
fn settle_rename_attempt(
    attempt: RenamePublicationAttempt,
    slot: &PublicationSlot,
    path: &Path,
    nonce: Uuid,
    epoch: u64,
    rows: usize,
    #[cfg(test)] fixture: Option<FixturePublicationAction>,
) -> SettledRenameAttempt {
    match attempt {
        RenamePublicationAttempt::Renamed { leaf, retired } => {
            #[cfg(test)]
            if let Some(fixture) = fixture {
                assert_eq!(fixture.nonce, nonce, "fixture checkpoint nonce");
                assert_eq!(
                    path.parent(),
                    Some(fixture.directory.as_path()),
                    "fixture exact directory"
                );
                assert!(!fixture.operation.is_nil(), "fixture actual operation");
                match fixture.kind {
                    FixturePublicationActionKind::CorruptFixtureLeafOnce => {
                        std::fs::write(path, b"{invalid fixture publication")
                            .expect("corrupt exact fixture leaf after actual rename");
                    }
                    FixturePublicationActionKind::PanicOnce => {
                        panic!("selected postrename/prereadback fixture unwind")
                    }
                }
            }
            confirm_publication_readback(slot, path, nonce, epoch);
            drop(leaf);
            drop(retired);
            log::info!("Saved {} sessions to {:?}", rows, path);
            SettledRenameAttempt::Completed(true)
        }
        RenamePublicationAttempt::Suppressed => SettledRenameAttempt::Completed(false),
        RenamePublicationAttempt::Retry | RenamePublicationAttempt::Stale => {
            SettledRenameAttempt::Recapture
        }
        RenamePublicationAttempt::Failed(error) => SettledRenameAttempt::Failed(error),
    }
}

async fn save_current_snapshot_guarded(
    mgr: &SessionManager,
    permit: &CarrierMutationPermit,
    dir: &Path,
    rows: &[PersistedSession],
    captured: &crate::session::manager::CarrierSnapshot,
    expected_epoch: u64,
    initial: Option<&InitialCarrierIntent>,
) -> GuardedSaveAttempt {
    let Some(slot) = publication_slot(dir) else {
        return GuardedSaveAttempt::Recapture;
    };
    let mut prepared = {
        let Ok(state) = slot.lock() else {
            return GuardedSaveAttempt::Recapture;
        };
        if state.epoch != expected_epoch {
            return GuardedSaveAttempt::Recapture;
        }
        let Some(epoch) = state.epoch.checked_add(1) else {
            return GuardedSaveAttempt::Recapture;
        };
        let mut next = state.clone();
        next.epoch = epoch;
        next.checkpoint = Some(PublicationCheckpoint {
            nonce: Uuid::new_v4(),
            epoch,
            rows: rows.to_vec(),
            versions: captured.versions.clone(),
            confirmed: false,
        });
        next
    };
    if !prepare_protected_publication(&mut prepared, rows, captured, initial) {
        return GuardedSaveAttempt::Recapture;
    }
    // Suppression keeps its original successful no-write disposition. It does
    // not advance the epoch or create a Confirmed checkpoint.
    if super::settings::agents_layer_unreadable_for_dir(dir) {
        return GuardedSaveAttempt::Completed(Ok(false));
    }
    let current = mgr
        .with_current_carrier_snapshot(permit, |current| current_capture_matches(current, captured))
        .await;
    if !current {
        return GuardedSaveAttempt::Recapture;
    }
    let write = match PreparedSessionsWrite::prepare(dir, rows) {
        Ok(write) => write,
        Err(error) => return GuardedSaveAttempt::Completed(Err(error)),
    };
    if initial.is_some() && prepared.canonical_directory.is_none() {
        if let Ok(canonical) = std::fs::canonicalize(dir) {
            // Native preparation may have created this directory. Adopt any
            // already-known physical alias before publishing the first key;
            // its epoch and protections require a whole-snapshot recapture.
            let Some(resolved) = resolve_publication_slot(
                dir,
                PublicationDirectoryIdentity::Known(canonical.clone()),
            ) else {
                return GuardedSaveAttempt::Recapture;
            };
            if !std::sync::Arc::ptr_eq(&slot, &resolved) {
                return GuardedSaveAttempt::Recapture;
            }
            prepared.canonical_directory = Some(canonical);
        } else {
            prepared
                .protections
                .retain(|protection| protection.retained.is_some());
        }
    }
    let checkpoint = prepared.checkpoint.as_ref().expect("prepared checkpoint");
    let nonce = checkpoint.nonce;
    let epoch = checkpoint.epoch;
    let mut next = Some(std::mem::take(&mut prepared));
    for attempt in 0..RENAME_ATTEMPTS {
        let result = settle_rename_attempt(
            mgr.with_current_carrier_snapshot(permit, |current| {
                if !current_capture_matches(current, captured) {
                    return RenamePublicationAttempt::Stale;
                }
                rename_and_publish(dir, &slot, expected_epoch, &mut next, &write)
            })
            .await,
            &slot,
            &write.path,
            nonce,
            epoch,
            rows.len(),
            #[cfg(test)]
            None,
        );
        match result {
            SettledRenameAttempt::Completed(written) => {
                return GuardedSaveAttempt::Completed(Ok(written))
            }
            SettledRenameAttempt::Recapture => return GuardedSaveAttempt::Recapture,
            SettledRenameAttempt::Failed(error) => {
                log::debug!(
                    "[sessions] rename attempt {}/{} failed — op_id={} pid={} os_error={:?} kind={:?}",
                    attempt + 1, RENAME_ATTEMPTS, write.op_id, write.pid,
                    error.raw_os_error(), error.kind(),
                );
                if attempt + 1 == RENAME_ATTEMPTS {
                    return GuardedSaveAttempt::Completed(Err(write.terminal_error(&error)));
                }
            }
        }
        // No M, suppression, lifecycle, or leaf guard crosses this backoff.
        tokio::time::sleep(std::time::Duration::from_millis(
            RENAME_BACKOFFS_MS[attempt as usize],
        ))
        .await;
    }
    unreachable!("RENAME_ATTEMPTS is positive")
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )
)]
fn sessions_save_lock() -> &'static tokio::sync::Mutex<()> {
    crate::session::manager::process_carrier_publication_gate().as_ref()
}

/// Transient prepared native publication. The owner retains destination_key
/// before publishing; this value itself never enters the retained root.
pub(crate) struct PreparedOwnedReplacementPublication {
    #[cfg(test)]
    fixture_action: Option<FixturePublicationAction>,
    source: PublishedCarrier,
    destination_key: PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    captured: crate::session::manager::CarrierSnapshot,
    protections: Vec<CarrierProtection>,
    expected_epoch: u64,
    next: Option<PublicationState>,
    write: PreparedSessionsWrite,
    nonce: Uuid,
    epoch: u64,
    rows: usize,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum FixturePublicationActionKind {
    CorruptFixtureLeafOnce,
    PanicOnce,
}

#[cfg(test)]
struct FixturePublicationAction {
    operation: Uuid,
    nonce: Uuid,
    directory: PathBuf,
    kind: FixturePublicationActionKind,
}

impl PreparedOwnedReplacementPublication {
    #[cfg(test)]
    pub(crate) fn attach_fixture_action(
        &mut self,
        operation: Uuid,
        directory: &Path,
        kind: FixturePublicationActionKind,
    ) {
        assert_eq!(
            operation, self.owner_nonce,
            "fixture actual publication owner"
        );
        assert_eq!(
            directory,
            self.source.directory.as_path(),
            "fixture prepared directory"
        );
        assert!(
            self.fixture_action.is_none(),
            "fixture action consumed once"
        );
        self.fixture_action = Some(FixturePublicationAction {
            operation,
            nonce: self.nonce,
            directory: directory.to_path_buf(),
            kind,
        });
    }
    pub(crate) fn destination_key(&self) -> PublishedCarrier {
        self.destination_key.clone()
    }
}

pub(crate) enum OwnedReplacementPublicationOutcome {
    Confirmed(PublishedCarrier),
    Pending(PublishedCarrier),
    Recapture,
    Failed(String),
}

/// A current A still needs strict Confirmed retired authority. After a real
/// Confirmed B rename, its exact retained key also admits a fresh-manager
/// checkpoint of newer B, without relabeling the old proof as current.
fn replacement_publication_authorized(
    permit: &CarrierMutationPermit,
    source: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    replacement: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    if source.session_id == previous {
        return renew_carrier_replacement(
            permit,
            source,
            owner_nonce,
            previous,
            replacement,
            current,
        )
        .is_some();
    }
    if source.session_id != replacement
        || current.pending_ids.contains(&replacement)
        || !current
            .rows
            .iter()
            .any(|row| row.id == replacement && row.token == source.token)
        || !current
            .versions
            .get(&replacement)
            .is_some_and(|version| *version >= source.version && *version < u64::MAX)
    {
        return false;
    }
    let Ok(state) = source.slot.lock() else {
        return false;
    };
    let Some(checkpoint) = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed && checkpoint.epoch == state.epoch && checkpoint.epoch < u64::MAX
    }) else {
        return false;
    };
    state.protections.iter().any(|protection| {
        protection.session_id == replacement
            && protection.token == source.token
            && protection.nonce == source.nonce
            && protection.target == source.target
            && protection.epoch == checkpoint.epoch
            && protection.version >= source.version
            && checkpoint.versions.get(&replacement) == Some(&protection.version)
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous,
                    replacement,
                }
            && checkpoint
                .rows
                .iter()
                .any(|row| same_carrier_projection(row, &protection.row))
    })
}

/// Capture under U, then release it for filtering, physical probes, serialization
/// and temporary-file preparation. Only A is omitted before normal deduplication.
pub(crate) async fn prepare_owned_replacement_publication<V, VF>(
    mgr: &SessionManager,
    source: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    replacement: Uuid,
    project_paths: &[String],
    validate_source: V,
) -> Result<Option<PreparedOwnedReplacementPublication>, String>
where
    V: Fn() -> VF,
    VF: std::future::Future<Output = bool>,
{
    let (captured, aggregate, expected_epoch, mut next, protections) = {
        let permit = acquire_carrier_mutation_permit().await;
        let captured = mgr
            .with_current_carrier_snapshot(&permit, Clone::clone)
            .await;
        if !replacement_publication_authorized(
            &permit,
            source,
            owner_nonce,
            previous,
            replacement,
            &captured,
        ) {
            return Ok(None);
        }
        let protections = match all_carrier_protections() {
            Some(value) => value,
            None => return Ok(None),
        };
        let state = match source.slot.lock() {
            Ok(value) => value.clone(),
            Err(_) => return Ok(None),
        };
        let aggregate = mgr.aggregate_snapshot().await;
        (captured, aggregate, state.epoch, state, protections)
    };
    let Some(b) = captured.rows.iter().find(|row| {
        row.id == replacement
            && crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(row)
            && !matches!(row.status, SessionStatus::Exited(_))
            && !captured.pending_ids.contains(&replacement)
    }) else {
        return Ok(None);
    };
    let Some(version) = captured.versions.get(&replacement).copied() else {
        return Ok(None);
    };
    let omitted = HashSet::from([previous]);
    let rows = snapshot_aggregate(&aggregate, &omitted);
    let prep = prepare_persist_snapshot(rows, project_paths).await?;
    // A filtered/deduplicated B never authorizes a target write. Protected
    // unrelated rows must also survive the ordinary preparation unchanged.
    let mut matches = prep.kept.iter().filter(|row| {
        row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) == Some(replacement)
    });
    let Some(b_row) = matches.next().cloned() else {
        return Ok(None);
    };
    if matches.next().is_some()
        || prep
            .kept
            .iter()
            .any(|row| row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) == Some(previous))
    {
        return Ok(None);
    }
    let Some(index) = next.protections.iter().position(|protection| {
        protection.session_id == source.session_id
            && protection.nonce == source.nonce
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous,
                    replacement,
                }
    }) else {
        return Ok(None);
    };
    let old = next.protections.remove(index);
    let Some(epoch) = expected_epoch.checked_add(1) else {
        return Ok(None);
    };
    next.epoch = epoch;
    if !prepare_protected_publication(&mut next, &prep.kept, &captured, None) {
        return Ok(None);
    }
    let lifetime = std::sync::Arc::new(());
    let destination_key = PublishedCarrier {
        target: source.target.clone(),
        session_id: replacement,
        token: b.token,
        nonce: Uuid::new_v4(),
        version,
        slot: source.slot.clone(),
        directory: source.directory.clone(),
        lifetime: lifetime.clone(),
        retired_local: None,
    };
    next.protections.push(CarrierProtection {
        target: source.target.clone(),
        session_id: replacement,
        token: b.token,
        nonce: destination_key.nonce,
        epoch,
        version,
        row: b_row,
        canonical_cwd: old.canonical_cwd.clone(),
        phase: ProtectionPhase::Preparing {
            owner_nonce,
            previous,
            replacement,
        },
        retired_local: None,
        lifetime: std::sync::Arc::downgrade(&lifetime),
        retained: None,
        pending_lifetime: Some(lifetime),
        active: None,
    });
    let nonce = Uuid::new_v4();
    next.checkpoint = Some(PublicationCheckpoint {
        nonce,
        epoch,
        rows: prep.kept.clone(),
        versions: captured.versions.clone(),
        confirmed: false,
    });
    if super::settings::agents_layer_unreadable_for_dir(&source.directory) {
        return Ok(None);
    }
    if !validate_source().await {
        return Ok(None);
    }
    let permit = acquire_carrier_mutation_permit().await;
    let valid = mgr
        .with_current_carrier_snapshot(&permit, |current| {
            current_capture_matches(current, &captured)
                && replacement_publication_authorized(
                    &permit,
                    source,
                    owner_nonce,
                    previous,
                    replacement,
                    current,
                )
        })
        .await;
    if !valid
        || !all_carrier_protections()
            .is_some_and(|actual| same_protection_set(&actual, &protections))
    {
        return Ok(None);
    }
    let write = PreparedSessionsWrite::prepare(&source.directory, &prep.kept)?;
    drop(permit);
    Ok(Some(PreparedOwnedReplacementPublication {
        #[cfg(test)]
        fixture_action: None,
        source: source.clone(),
        destination_key,
        owner_nonce,
        previous,
        captured,
        protections,
        expected_epoch,
        next: Some(next),
        write,
        nonce,
        epoch,
        rows: prep.kept.len(),
    }))
}

/// One actual rename attempt. The caller releases U before any backoff. M and
/// suppression are gone before readback; the exact native leaf remains held.
pub(crate) async fn publish_owned_replacement(
    mgr: &SessionManager,
    permit: &CarrierMutationPermit,
    prepared: &mut PreparedOwnedReplacementPublication,
    validate_current: impl Fn(&crate::session::manager::CarrierSnapshot) -> bool,
) -> OwnedReplacementPublicationOutcome {
    let protections_current = all_carrier_protections()
        .is_some_and(|current| same_protection_set(&current, &prepared.protections));
    if !protections_current {
        return OwnedReplacementPublicationOutcome::Recapture;
    }
    let authority_current = mgr
        .with_current_carrier_snapshot(permit, |current| {
            validate_current(current)
                && current_capture_matches(current, &prepared.captured)
                && replacement_publication_authorized(
                    permit,
                    &prepared.source,
                    prepared.owner_nonce,
                    prepared.previous,
                    prepared.destination_key.session_id,
                    current,
                )
        })
        .await;
    if !authority_current {
        return OwnedReplacementPublicationOutcome::Recapture;
    }
    let attempt = mgr
        .with_current_carrier_snapshot(permit, |current| {
            if !validate_current(current) || !current_capture_matches(current, &prepared.captured) {
                return RenamePublicationAttempt::Stale;
            }
            rename_and_publish(
                &prepared.source.directory,
                &prepared.source.slot,
                prepared.expected_epoch,
                &mut prepared.next,
                &prepared.write,
            )
        })
        .await;
    #[cfg(test)]
    let fixture = if matches!(&attempt, RenamePublicationAttempt::Renamed { .. }) {
        prepared.fixture_action.take()
    } else {
        None
    };
    let result = settle_rename_attempt(
        attempt,
        &prepared.source.slot,
        &prepared.write.path,
        prepared.nonce,
        prepared.epoch,
        prepared.rows,
        #[cfg(test)]
        fixture,
    );
    match result {
        SettledRenameAttempt::Completed(true) => {
            match renew_carrier_replacement(
                permit,
                &prepared.destination_key,
                prepared.owner_nonce,
                prepared.previous,
                prepared.destination_key.session_id,
                &prepared.captured,
            ) {
                Some(carrier) => OwnedReplacementPublicationOutcome::Confirmed(carrier),
                None => {
                    OwnedReplacementPublicationOutcome::Pending(prepared.destination_key.clone())
                }
            }
        }
        SettledRenameAttempt::Completed(false) | SettledRenameAttempt::Recapture => {
            OwnedReplacementPublicationOutcome::Recapture
        }
        SettledRenameAttempt::Failed(error) => {
            OwnedReplacementPublicationOutcome::Failed(prepared.write.terminal_error(&error))
        }
    }
}

/// Post-rename recovery only reads back the same leaf. It never prepares A or
/// performs another rename after an observed successful publication.
pub(crate) fn reconcile_owned_replacement_readback(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let (nonce, epoch) = {
        let state = carrier.slot.lock().ok()?;
        let checkpoint = state.checkpoint.as_ref()?;
        let protection = state.protections.iter().find(|protection| {
            protection.session_id == carrier.session_id
                && protection.nonce == carrier.nonce
                && protection.token == carrier.token
                && protection.phase
                    == ProtectionPhase::Preparing {
                        owner_nonce,
                        previous,
                        replacement: carrier.session_id,
                    }
        })?;
        if checkpoint.epoch != state.epoch || protection.epoch != checkpoint.epoch {
            return None;
        }
        (checkpoint.nonce, checkpoint.epoch)
    };
    let leaf = SAVE_SESSIONS_LOCK.try_lock().ok()?;
    confirm_publication_readback(
        &carrier.slot,
        &carrier.directory.join("sessions.json"),
        nonce,
        epoch,
    );
    drop(leaf);
    renew_carrier_replacement(
        permit,
        carrier,
        owner_nonce,
        previous,
        carrier.session_id,
        current,
    )
}

/// Snapshot current live sessions into the persisted format.
/// Applies provider-specific persistence cleanup to the configured recipe.
/// Pi runtime injection never enters this field, so configured Pi args are preserved.
pub async fn snapshot_sessions(mgr: &SessionManager) -> Vec<PersistedSession> {
    let aggregate = mgr.aggregate_snapshot().await;
    let Some(protections) = all_carrier_protections() else {
        // An unreadable publication registry cannot authorize a snapshot that
        // silently refills A over an already-published replacement B.
        return std::future::pending().await;
    };
    let omitted = protections
        .iter()
        .filter_map(|protection| match protection.phase {
            ProtectionPhase::Preparing {
                previous,
                replacement,
                ..
            } if protection.session_id == replacement && previous != replacement => Some(previous),
            _ => None,
        })
        .collect::<HashSet<_>>();
    snapshot_aggregate(&aggregate, &omitted)
}

fn snapshot_aggregate(
    aggregate: &crate::session::manager::ManagerAggregateSnapshot,
    omitted: &HashSet<Uuid>,
) -> Vec<PersistedSession> {
    let sessions = aggregate
        .sessions
        .iter()
        .filter(|row| !omitted.contains(&row.id))
        .map(SessionInfo::from)
        .collect::<Vec<_>>();
    let active_id = aggregate.selection.id().map(|id| id.to_string());

    let all: Vec<PersistedSession> = sessions
        .iter()
        .filter(|s| {
            if s.name.starts_with(TEMP_SESSION_PREFIX) {
                log::debug!(
                    "[sessions] Excluding temp session '{}' from snapshot",
                    s.name
                );
                false
            } else {
                true
            }
        })
        .map(|s| PersistedSession {
            coding_agent_binding_id: Uuid::parse_str(&s.id)
                .ok()
                .and_then(|id| aggregate.coding_agent_bindings.get(&id).cloned()),
            coding_agent_binding_seed: Uuid::parse_str(&s.id)
                .ok()
                .and_then(|id| aggregate.coding_agent_bindings.get(&id))
                .and_then(|id| aggregate.binding_seeds.get(id))
                .and_then(|seed| serde_json::from_value(seed.clone()).ok()),
            coding_agent_binding_origin: Uuid::parse_str(&s.id)
                .ok()
                .and_then(|id| aggregate.coding_agent_bindings.get(&id))
                .and_then(|id| aggregate.binding_origins.get(id).cloned()),
            coding_agent_binding_conflict: false,
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: strip_auto_injected_args(&s.shell, &s.shell_args),
            working_directory: s.working_directory.clone(),
            was_active: active_id.as_deref() == Some(&s.id),
            git_repos: s.git_repos.clone(),
            is_coordinator: s.is_coordinator,
            is_root_agent: s.is_root_agent,
            agent_id: s.agent_id.clone(),
            agent_label: s.agent_label.clone(),
            requested_profile: s.requested_profile.clone(),
            effective_profile: s.effective_profile.clone(),
            telegram_bot_id: s.telegram_bot_id.clone(),
            // Fix A: read detach state directly from the Session (via SessionInfo). The
            // `DetachedSessionsState` set is NOT consulted at persist time — the Destroyed
            // handler clears the set before `RunEvent::Exit` runs the final persist.
            was_detached: s.was_detached,
            last_prompt: s.last_prompt.clone(),
            detached_geometry: s.detached_geometry.clone(),
            start_fresh_on_restore: s.start_fresh_on_restore,
            // Legacy fields are always None on new saves; skip_serializing_if elides them.
            git_branch_source: None,
            git_branch_prefix: None,
            // Runtime fields for CLI consumption
            id: Some(s.id.clone()),
            status: Some(s.status.clone()),
            waiting_for_input: Some(s.waiting_for_input),
            communication: s.communication.clone(),
            created_at: Some(s.created_at.clone()),
            context_percent: s.context_percent,
        })
        .collect();

    deduplicate_preserving_affected(
        all,
        &aggregate.failed_binding_recipes,
        &aggregate.binding_origins,
    )
}

fn deduplicate_preserving_affected(
    sessions: Vec<PersistedSession>,
    affected: &HashMap<String, serde_json::Value>,
    origins: &HashMap<String, serde_json::Value>,
) -> Vec<PersistedSession> {
    let mut result = deduplicate(sessions);
    for (binding, recipe) in affected {
        if result
            .iter()
            .any(|row| row.coding_agent_binding_id.as_ref() == Some(binding))
        {
            continue;
        }
        match serde_json::from_value::<PersistedSession>(recipe.clone()) {
            Ok(mut recipe) => {
                recipe.coding_agent_binding_origin = origins.get(binding).cloned();
                result.push(sanitize_failed_recoverable(&recipe));
            }
            Err(error) => log::error!(
                "[sessions] invalid affected binding recipe {}: {}",
                binding,
                error
            ),
        }
    }
    result
}

/// Strip AC-managed provider args from saved shell arguments.
/// Current launch-time injections are Claude's `--continue`, Codex's
/// `resume --last`, Antigravity's `--continue`, and Pi's `--continue`.
/// The first three are stripped so they cannot self-perpetuate across restarts.
/// Pi is preserved because this boundary receives the configured recipe, where
/// a `--continue` token is user-authored and has no injection provenance.
///
/// Handles two injection modes:
/// - **Direct-exec**: args are separate tokens like `["--continue", ...]` or `["resume", "--last", ...]`
/// - **cmd.exe wrapper**: tokens may be separate args (`["/C", "codex", "resume", "--last"]`)
///   or embedded in a single arg string (`["/K", "git pull && codex resume --last"]`)
pub(crate) fn strip_auto_injected_args(shell: &str, args: &[String]) -> Vec<String> {
    fn strip_claude_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260: Claude's resume flag from the CodingAgentProfile. resume_tokens
        // is a 1-element const for Claude, so [0] is provably in bounds.
        let continue_flag = CodingAgentKind::Claude.profile().resume_tokens[0];
        let mut idx = start;
        while idx < tokens.len() {
            if tokens[idx].eq_ignore_ascii_case(continue_flag) {
                tokens.remove(idx);
                continue;
            }
            // (#756) Rider belt: strip the launcher-minted identity so it can
            // never self-perpetuate; a replayed stale --session-id hard-fails
            // the spawn (transcript collision), unlike a stale --continue.
            // UUID-gated: only the AC-injected `--session-id <v4>` shape is
            // removed; user-authored non-UUID values win.
            let lower = tokens[idx].to_lowercase();
            if lower == "--session-id"
                && tokens
                    .get(idx + 1)
                    .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
            {
                tokens.remove(idx);
                tokens.remove(idx); // paired UUID value
                continue;
            }
            if lower
                .strip_prefix("--session-id=")
                .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
            {
                tokens.remove(idx);
                continue;
            }
            idx += 1;
        }
    }

    fn strip_codex_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260 — resume tokens from the CodingAgentProfile. G6: slice-pattern
        // destructure, never index; a wrong-arity slice no-ops gracefully.
        let &[resume_subcmd, resume_flag] = CodingAgentKind::Codex.profile().resume_tokens else {
            debug_assert!(false, "Codex resume_tokens must have exactly 2 elements");
            return;
        };
        if tokens
            .get(start)
            .is_some_and(|token| token.eq_ignore_ascii_case(resume_subcmd))
            && tokens
                .get(start + 1)
                .is_some_and(|token| token.eq_ignore_ascii_case(resume_flag))
        {
            tokens.remove(start);
            tokens.remove(start);
        }
    }

    fn strip_antigravity_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260/#1482 — resume token from the CodingAgentProfile. Only the
        // AC-injected `--continue` is stripped; `-c` and `--conversation <ID>`
        // are user-authored resume forms and are preserved. resume_tokens is a
        // 1-element const for Antigravity, so [0] is provably in bounds.
        let continue_flag = CodingAgentKind::Antigravity.profile().resume_tokens[0];
        let mut idx = start;
        while idx < tokens.len() {
            if tokens[idx].eq_ignore_ascii_case(continue_flag) {
                tokens.remove(idx);
                continue;
            }
            idx += 1;
        }
    }

    // #260 — consult the single detector (session/profile.rs) instead of
    // re-deriving agent identity here. Guarantees this stripper agrees with
    // the `agent_kind` that `create_session_inner` stamped on the session.
    let (is_claude, is_codex, is_antigravity) = match CodingAgentKind::detect(shell, args) {
        // #1873 - Muse takes Pi's preserve path: `resume --last` in a persisted
        // recipe is user-authored (AC tokens live only in the effective vector).
        Some(CodingAgentKind::Pi | CodingAgentKind::Muse) => return args.to_vec(),
        Some(CodingAgentKind::Claude) => (true, false, false),
        Some(CodingAgentKind::Codex) => (false, true, false),
        Some(CodingAgentKind::Antigravity) => (false, false, true),
        None => return args.to_vec(),
    };

    let is_cmd = crate::commands::session::executable_basename(shell) == "cmd";

    if is_cmd {
        let mut result = args.to_vec();

        if is_claude {
            if let Some(idx) = result.iter().position(|arg| {
                crate::commands::session::executable_basename(arg).starts_with("claude")
            }) {
                strip_claude_tokens(&mut result, idx + 1);
            }
        }
        if is_codex {
            if let Some(idx) = result
                .iter()
                .position(|arg| crate::commands::session::executable_basename(arg) == "codex")
            {
                strip_codex_tokens(&mut result, idx + 1);
            }
        }
        if is_antigravity {
            if let Some(idx) = result.iter().position(|arg| {
                matches!(
                    crate::commands::session::executable_basename(arg).as_str(),
                    "agy" | "antigravity"
                )
            }) {
                strip_antigravity_tokens(&mut result, idx + 1);
            }
        }

        for arg in &mut result {
            let mut tokens: Vec<String> = arg
                .split_whitespace()
                .map(|token| token.to_string())
                .collect();
            let mut changed = false;

            if is_claude {
                if let Some(idx) = tokens.iter().position(|token| {
                    crate::commands::session::executable_basename(token).starts_with("claude")
                }) {
                    let before = tokens.len();
                    strip_claude_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if is_codex {
                if let Some(idx) = tokens.iter().position(|token| {
                    crate::commands::session::executable_basename(token) == "codex"
                }) {
                    let before = tokens.len();
                    strip_codex_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if is_antigravity {
                if let Some(idx) = tokens.iter().position(|token| {
                    matches!(
                        crate::commands::session::executable_basename(token).as_str(),
                        "agy" | "antigravity"
                    )
                }) {
                    let before = tokens.len();
                    strip_antigravity_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if changed {
                *arg = tokens.join(" ");
            }
        }

        result
    } else {
        let mut result = Vec::with_capacity(args.len());
        let mut skip_next = false;
        for (idx, a) in args.iter().enumerate() {
            if skip_next {
                skip_next = false;
                continue;
            }
            if is_codex
                && idx == 0
                && a.eq_ignore_ascii_case("resume")
                && args
                    .get(1)
                    .is_some_and(|next| next.eq_ignore_ascii_case("--last"))
            {
                continue;
            }
            if is_codex
                && idx == 1
                && args
                    .first()
                    .is_some_and(|first| first.eq_ignore_ascii_case("resume"))
                && a.eq_ignore_ascii_case("--last")
            {
                continue;
            }

            if is_antigravity && a.eq_ignore_ascii_case("--continue") {
                continue;
            }
            if is_claude && a.eq_ignore_ascii_case("--continue") {
                continue;
            }
            // (#756) Rider belt, direct-exec form: strip the launcher-minted
            // `--session-id <v4>` pair / `--session-id=<v4>` joined token.
            // UUID-gated so user-authored non-UUID values are preserved.
            if is_claude {
                let lower = a.to_lowercase();
                if lower == "--session-id"
                    && args
                        .get(idx + 1)
                        .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
                {
                    skip_next = true; // consume the paired UUID value too
                    continue;
                }
                if lower
                    .strip_prefix("--session-id=")
                    .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
                {
                    continue;
                }
            }
            result.push(a.clone());
        }
        result
    }
}

/// Pure: produce a sanitized copy of a failed-recoverable PersistedSession
/// suitable for merging into a fresh snapshot. Drops runtime fields (`id`,
/// `status`, `waiting_for_input`, `created_at`) since those describe the
/// PRIOR run's state; the session is no longer live, and persisting them
/// would make `list-sessions` (which filters on `id.is_some()`) report the
/// session as alive when it is not. See §224.
///
/// `communication` is deliberately PRESERVED (#747): a raised hand is durable
/// intent, not prior-run runtime state, so a transiently failed restore keeps
/// it for the next startup attempt. `list-sessions` still reports
/// `raisedHand: false` for these rows because `id` and `status` are stripped
/// (both the `id.is_some()` row filter and the `status.is_none()` gate exclude
/// them). Best-effort with the SAME LIFETIME AS THE RECIPE ROW: the row (hand
/// included) survives only until the next `persist_current_state` (§224 G5).
pub(crate) fn sanitize_failed_recoverable(ps: &PersistedSession) -> PersistedSession {
    let mut clean = ps.clone();
    clean.id = None;
    clean.status = None;
    clean.waiting_for_input = None;
    clean.created_at = None;
    clean
}

/// Persist live sessions plus stripped recipes for entries that failed to
/// restore. Stripped recipes survive on disk only until the next
/// `persist_current_state` call (any session-lifecycle event) overwrites the
/// snapshot — so retry-on-next-startup is best-effort. §224 G5/G8.
pub async fn persist_merging_failed_result(
    mgr: &SessionManager,
    failed: &[PersistedSession],
) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    persist_merging_failed_to_dir_for_project_paths_result(mgr, failed, &dir, Some(&project_paths))
        .await
}

async fn persist_merging_failed_to_dir_for_project_paths_result(
    mgr: &SessionManager,
    failed: &[PersistedSession],
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<(), String> {
    let manager = mgr.clone();
    let failed = failed.to_vec();
    let dir = dir.to_path_buf();
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        persist_merging_failed_to_dir_for_project_paths_result_core(
            &manager,
            &failed,
            &dir,
            roots.as_deref(),
        )
        .await
    })
    .await
}

async fn persist_merging_failed_to_dir_for_project_paths_result_core(
    mgr: &SessionManager,
    failed: &[PersistedSession],
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<(), String> {
    let carrier_permit = acquire_carrier_mutation_permit().await;
    let mut snapshot = snapshot_sessions(mgr).await;
    // §224 — strip stale runtime fields (`id`, `status`, `waiting_for_input`,
    // `created_at`) from failed-recoverable entries (`communication` is kept,
    // #747, see `sanitize_failed_recoverable`). Without this, the prior
    // run's runtime fields travel into the new snapshot, and `list-sessions`
    // reports a session as alive (its `s.id.is_some()` filter passes) while
    // the in-memory `SessionManager` does not contain it. `close-session`
    // then can't find the session and rejects with "No active session found".
    // Stripping enforces the invariant: any persisted row with `id.is_some()`
    // is guaranteed to be live in SessionManager.
    //
    // NOTE: this strip preserves the recipe (working_directory, shell,
    // agent_id, was_active, was_detached, etc.) so the next-startup restore
    // can retry. However, "retry-on-next-startup" persistence is best-effort:
    // any subsequent idle/busy event after this call invokes
    // `persist_current_state`, which rebuilds the snapshot from the live
    // SessionManager ONLY and silently drops failed-recoverable entries (§224
    // G5). Proper plumb of `failed_recoverable` into SessionManager is filed
    // as a follow-up.
    let binding_failures = failed
        .iter()
        .filter_map(|ps| {
            let binding = ps.binding_metadata().ok().flatten()?;
            serde_json::to_value(sanitize_failed_recoverable(ps))
                .ok()
                .map(|recipe| (binding, recipe))
        })
        .collect();
    mgr.refresh_failed_binding_recipes_with_carrier_permit(&carrier_permit, binding_failures)
        .await;
    snapshot.extend(failed.iter().map(sanitize_failed_recoverable));
    let aggregate = mgr.aggregate_snapshot().await;
    let snapshot = deduplicate_preserving_affected(
        snapshot,
        &aggregate.failed_binding_recipes,
        &aggregate.binding_origins,
    );
    let prep = match project_paths {
        Some(project_paths) => prepare_persist_snapshot(snapshot, project_paths).await?,
        None => PersistPreparation {
            kept: snapshot,
            drops: Vec::new(),
        },
    };
    // §1295 site B (merge): startup-only, keeps lock-first; the inline filter
    // is replaced by the same prepare (in-lock, accepted bounded stall) + handle
    // + save.
    let failed = failed
        .iter()
        .map(sanitize_failed_recoverable)
        .collect::<Vec<_>>();
    persist_preparation_owned(
        mgr,
        dir,
        prep,
        PersistMode::NoPrune,
        carrier_permit,
        project_paths,
        &failed,
    )
    .await
}

pub async fn persist_merging_failed(mgr: &SessionManager, failed: &[PersistedSession]) {
    if let Err(e) = persist_merging_failed_result(mgr, failed).await {
        log::error!("Failed to persist sessions (with merge): {}", e);
    }
}

/// Convenience: snapshot and save in one call. Logs errors but never fails.
pub async fn persist_current_state_result(mgr: &SessionManager) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    persist_current_state_to_dir_for_project_paths_result(
        mgr,
        &dir,
        Some(&project_paths),
        PersistMode::NoPrune,
    )
    .await
}

async fn persist_current_state_to_dir_for_project_paths_result(
    mgr: &SessionManager,
    dir: &Path,
    project_paths: Option<&[String]>,
    mode: PersistMode,
) -> Result<(), String> {
    let manager = mgr.clone();
    let dir = dir.to_path_buf();
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        persist_current_state_to_dir_for_project_paths_result_core(
            &manager,
            &dir,
            roots.as_deref(),
            mode,
            None,
        )
        .await
        .map(|_| ())
    })
    .await
}

async fn persist_current_state_to_dir_for_project_paths_result_core(
    mgr: &SessionManager,
    dir: &Path,
    project_paths: Option<&[String]>,
    mode: PersistMode,
    initial: Option<InitialCarrierIntent>,
) -> Result<Option<PublishedCarrier>, String> {
    let mut foreign_done = HashSet::new();
    loop {
        let directory_identity = probe_publication_directory(dir, initial.is_some());
        let captured = {
            let permit = acquire_carrier_mutation_permit().await;
            if initial
                .as_ref()
                .is_some_and(|intent| !intent.restore_allows_publication())
            {
                return Ok(None);
            }
            if resolve_publication_slot(dir, directory_identity).is_none() {
                drop(permit);
                tokio::task::yield_now().await;
                continue;
            }
            let epoch =
                publication_slot(dir).and_then(|slot| slot.lock().ok().map(|state| state.epoch));
            let protections = all_carrier_protections();
            let (Some(epoch), Some(protections)) = (epoch, protections) else {
                drop(permit);
                return std::future::pending().await;
            };
            let captured = mgr
                .with_current_carrier_snapshot(&permit, Clone::clone)
                .await;
            let snapshot = snapshot_sessions(mgr).await;
            (captured, snapshot, epoch, protections)
        };
        let mut prep = match project_paths {
            Some(project_paths) => prepare_persist_snapshot(captured.1, project_paths).await?,
            None => PersistPreparation {
                kept: captured.1,
                drops: Vec::new(),
            },
        };
        // Slow path identity probes are performed with neither U nor M held.
        let relations = prep
            .kept
            .iter()
            .chain(prep.drops.iter().map(|drop| &drop.session))
            .map(|row| PreparedCarrierRelation::capture(row, &captured.3))
            .collect::<Vec<_>>();
        let drop_keys = prep.drops.iter().map(orphan_effect_key).collect::<Vec<_>>();
        let manager_relations = captured
            .0
            .rows
            .iter()
            .map(|row| {
                (
                    row.id,
                    PreparedCarrierRelation::capture_manager(row, &captured.3),
                )
            })
            .collect::<HashMap<_, _>>();
        let carrier_permit = acquire_carrier_mutation_permit().await;
        if initial
            .as_ref()
            .is_some_and(|intent| !intent.restore_allows_publication())
        {
            return Ok(None);
        }
        let protections = all_carrier_protections();
        let epoch =
            publication_slot(dir).and_then(|slot| slot.lock().ok().map(|state| state.epoch));
        let current = mgr
            .with_current_carrier_snapshot(&carrier_permit, |current| {
                current_capture_matches(current, &captured.0)
            })
            .await;
        let valid_protections = protections.as_ref().is_some_and(|current| {
            same_protection_set(current, &captured.3)
                && relations
                    .iter()
                    .all(|relation| relation.classify(current) != CarrierDisposition::Unknown)
                && prep.drops.iter().all(|drop| {
                    !current.iter().any(|protection| {
                        drop.session
                            .id
                            .as_deref()
                            .and_then(|id| Uuid::parse_str(id).ok())
                            == Some(protection.session_id)
                    })
                })
        });
        let keys_survive = publication_slot(dir)
            .is_some_and(|slot| protected_keys_survive(&slot, &prep.kept, &captured.0));
        if !current || epoch != Some(captured.2) || !valid_protections || !keys_survive {
            drop(carrier_permit);
            tokio::task::yield_now().await;
            continue;
        }
        // ForeignDone belongs to this accepted core. A whole-snapshot recapture
        // after our prune/repair must not repeat its completed archive effects.
        let drops = std::mem::take(&mut prep.drops)
            .into_iter()
            .zip(drop_keys)
            .filter_map(|(drop, key)| foreign_done.insert(key).then_some(drop))
            .collect::<Vec<_>>();
        let dispositions = manager_relations
            .iter()
            .map(|(id, relation)| (*id, relation.classify(&captured.3)))
            .collect::<HashMap<_, _>>();
        handle_orphan_drops(
            Some(mgr),
            dir,
            &drops,
            mode,
            &carrier_permit,
            Some(&dispositions),
        )
        .await;
        match save_current_snapshot_guarded(
            mgr,
            &carrier_permit,
            dir,
            &prep.kept,
            &captured.0,
            captured.2,
            initial
                .as_ref()
                .filter(|intent| intent.matches(&captured.0, &prep.kept)),
        )
        .await
        {
            GuardedSaveAttempt::Completed(result) => {
                let written = match result {
                    Ok(written) => written,
                    Err(error) => {
                        mgr.disable_prompt_reuse().await;
                        return Err(error);
                    }
                };
                let initial = initial
                    .as_ref()
                    .filter(|intent| intent.matches(&captured.0, &prep.kept));
                if !written {
                    return Ok(None);
                }
                let prompt_checkpoint = flush_retained_prompts_locked(mgr, dir).await;
                let Some(initial) = initial else {
                    prompt_checkpoint?;
                    return Ok(None);
                };
                if let Err(error) = prompt_checkpoint {
                    // The sessions rename already published a real carrier. A
                    // sidecar error cannot replace its receipt or reconciliation.
                    log::warn!("[last-prompt] initial carrier checkpoint retained after sidecar failure: {error}");
                }
                drop(carrier_permit);
                return Ok(wait_initial_publication(dir, initial).await);
            }
            GuardedSaveAttempt::Recapture => {
                drop(carrier_permit);
                tokio::task::yield_now().await;
            }
        }
    }
}

/// A post-rename readback loss is Pending, not a save failure or a new write.
/// The accepted owner retains the Initial lifetime while reconciliation reads
/// the same directory leaf. A superseding version cannot register this intent.
async fn wait_initial_publication(
    dir: &Path,
    initial: &InitialCarrierIntent,
) -> Option<PublishedCarrier> {
    loop {
        let permit = acquire_carrier_mutation_permit().await;
        let slot = publication_slot(dir)?;
        if let Some(carrier) = published_initial_carrier(&slot, initial, dir) {
            return Some(carrier);
        }
        let checkpoint = {
            let state = slot.lock().ok()?;
            if !state.protections.iter().any(|protection| {
                protection.session_id == initial.session_id && protection.token == initial.token
            }) {
                return None;
            }
            let checkpoint = state.checkpoint.as_ref()?;
            if checkpoint.versions.get(&initial.session_id) != Some(&initial.version) {
                return None;
            }
            (checkpoint.nonce, checkpoint.epoch)
        };
        if let Ok(leaf) = SAVE_SESSIONS_LOCK.try_lock() {
            confirm_publication_readback(
                &slot,
                &dir.join("sessions.json"),
                checkpoint.0,
                checkpoint.1,
            );
            drop(leaf);
        }
        if let Some(carrier) = published_initial_carrier(&slot, initial, dir) {
            return Some(carrier);
        }
        drop(permit);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

pub(crate) async fn persist_initial_carrier(
    mgr: &SessionManager,
    dir: PathBuf,
    project_paths: Vec<String>,
    initial: InitialCarrierIntent,
) -> Result<Option<PublishedCarrier>, String> {
    if super::settings::agents_layer_unreadable_for_dir(&dir) {
        return Ok(None);
    }
    let manager = mgr.clone();
    run_owned_persist_core(async move {
        persist_current_state_to_dir_for_project_paths_result_core(
            &manager,
            &dir,
            Some(&project_paths),
            PersistMode::NoPrune,
            Some(initial),
        )
        .await
    })
    .await
}

fn orphan_effect_key(drop: &OrphanDrop) -> (Option<String>, String, String, String, Vec<String>) {
    (
        drop.session.id.clone(),
        drop.session.working_directory.clone(),
        drop.session.name.clone(),
        drop.session.shell.clone(),
        drop.session.shell_args.clone(),
    )
}

fn protected_keys_survive(
    slot: &PublicationSlot,
    rows: &[PersistedSession],
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    let Ok(state) = slot.lock() else {
        return false;
    };
    state
        .protections
        .iter()
        .filter(|protection| protection.lifetime.strong_count() != 0)
        .all(|protection| {
            let count = rows
                .iter()
                .filter(|row| {
                    row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok())
                        == Some(protection.session_id)
                })
                .count();
            count == 1
                && !current.pending_ids.contains(&protection.session_id)
                && current
                    .rows
                    .iter()
                    .any(|row| row.id == protection.session_id && row.token == protection.token)
        })
}

fn protected_preparation_survives(
    slot: &PublicationSlot,
    protections: &[CarrierProtection],
    prep: &PersistPreparation,
    current: &[PersistedSession],
) -> bool {
    if prep.drops.iter().any(|drop| {
        PreparedCarrierRelation::capture(&drop.session, protections).classify(protections)
            != CarrierDisposition::Foreign
    }) {
        return false;
    }
    if prep.kept.iter().any(|row| {
        PreparedCarrierRelation::capture(row, protections).classify(protections)
            == CarrierDisposition::Unknown
    }) {
        return false;
    }
    let Ok(state) = slot.lock() else {
        return false;
    };
    state
        .protections
        .iter()
        .filter(|protection| protection.lifetime.strong_count() != 0)
        .all(|protection| {
            let matching = |row: &&PersistedSession| {
                row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok())
                    == Some(protection.session_id)
            };
            let mut kept = prep.kept.iter().filter(matching);
            let Some(row) = kept.next() else {
                return false;
            };
            kept.next().is_none()
                && current
                    .iter()
                    .filter(matching)
                    .any(|expected| same_carrier_projection(row, expected))
        })
}

/// The accepted operation owns U and may release it on an actual protection
/// conflict. Its RAM transition is never replayed; only the whole snapshot is
/// recaptured. Failed-recoverable recipes remain explicit owned inputs.
async fn persist_preparation_owned(
    mgr: &SessionManager,
    dir: &Path,
    mut prep: PersistPreparation,
    mode: PersistMode,
    mut permit: CarrierMutationPermit,
    roots: Option<&[String]>,
    failed: &[PersistedSession],
) -> Result<(), String> {
    let mut foreign_done = HashSet::new();
    loop {
        let identity = probe_publication_directory(dir, false);
        let slot = resolve_publication_slot(dir, identity);
        let Some(slot) = slot else {
            drop(permit);
            tokio::task::yield_now().await;
            permit = acquire_carrier_mutation_permit().await;
            continue;
        };
        let epoch = slot.lock().ok().map(|state| state.epoch);
        let protections = all_carrier_protections();
        let captured = mgr
            .with_current_carrier_snapshot(&permit, Clone::clone)
            .await;
        let current = snapshot_sessions(mgr).await;
        if let (Some(epoch), Some(protections)) = (epoch, protections) {
            if protected_keys_survive(&slot, &prep.kept, &captured)
                && protected_preparation_survives(&slot, &protections, &prep, &current)
            {
                let dispositions = captured
                    .rows
                    .iter()
                    .map(|row| {
                        (
                            row.id,
                            PreparedCarrierRelation::capture_manager(row, &protections)
                                .classify(&protections),
                        )
                    })
                    .collect::<HashMap<_, _>>();
                let drops = std::mem::take(&mut prep.drops)
                    .into_iter()
                    .filter(|drop| foreign_done.insert(orphan_effect_key(drop)))
                    .collect::<Vec<_>>();
                handle_orphan_drops(Some(mgr), dir, &drops, mode, &permit, Some(&dispositions))
                    .await;
                match save_current_snapshot_guarded(
                    mgr, &permit, dir, &prep.kept, &captured, epoch, None,
                )
                .await
                {
                    GuardedSaveAttempt::Completed(result) => {
                        let written = match result {
                            Ok(written) => written,
                            Err(error) => {
                                mgr.disable_prompt_reuse().await;
                                return Err(error);
                            }
                        };
                        if written {
                            flush_retained_prompts_locked(mgr, dir).await?;
                        }
                        return Ok(());
                    }
                    GuardedSaveAttempt::Recapture => {}
                }
            }
        }
        drop(permit);
        tokio::task::yield_now().await;
        permit = acquire_carrier_mutation_permit().await;
        let mut snapshot = snapshot_sessions(mgr).await;
        snapshot.extend(failed.iter().cloned());
        let aggregate = mgr.aggregate_snapshot().await;
        let snapshot = deduplicate_preserving_affected(
            snapshot,
            &aggregate.failed_binding_recipes,
            &aggregate.binding_origins,
        );
        prep = match roots {
            Some(roots) => prepare_persist_snapshot(snapshot, roots).await?,
            None => PersistPreparation {
                kept: snapshot,
                drops: Vec::new(),
            },
        };
    }
}

fn same_protection_set(a: &[CarrierProtection], b: &[CarrierProtection]) -> bool {
    a.len() == b.len()
        && a.iter().all(|a| {
            b.iter().any(|b| {
                a.target == b.target
                    && a.session_id == b.session_id
                    && a.nonce == b.nonce
                    && a.epoch == b.epoch
                    && a.version == b.version
                    && a.phase == b.phase
                    && a.retired_local == b.retired_local
                    && a.canonical_cwd == b.canonical_cwd
                    && same_carrier_projection(&a.row, &b.row)
            })
        })
}

/// Snapshot the live sessions, apply the project-path filter, and save.
///
/// CONTRACT: the caller MUST already hold `sessions_save_lock()`. This is the
/// lock-free body shared by `persist_current_state_to_dir_for_project_paths_result`
/// and the #698 atomic mutate-then-persist helpers
/// (`raise_hand_and_persist_*`, `clear_user_input_transitions_and_persist_*`),
/// which take that lock once and run a `SessionManager` mutation plus this save
/// under it. Splitting it out keeps those helpers from re-acquiring the tokio
/// mutex (which is not reentrant and would deadlock).
///
/// §1295: the body is the same prepare + handle + save as the hot path; its
/// in-lock prepare is the rare, documented bounded stall (S2's allowed branch)
/// for the #698 atomic helpers.
async fn snapshot_and_save_locked(
    mgr: &SessionManager,
    dir: &Path,
    project_paths: Option<&[String]>,
    carrier_permit: CarrierMutationPermit,
) -> Result<(), String> {
    let snapshot = snapshot_sessions(mgr).await;
    let prep = match project_paths {
        Some(project_paths) => prepare_persist_snapshot(snapshot, project_paths).await?,
        None => PersistPreparation {
            kept: snapshot,
            drops: Vec::new(),
        },
    };
    persist_preparation_owned(
        mgr,
        dir,
        prep,
        PersistMode::NoPrune,
        carrier_permit,
        project_paths,
        &[],
    )
    .await
}

#[cfg(test)]
pub(crate) async fn persist_current_state_to_dir_result(
    mgr: &SessionManager,
    dir: &Path,
) -> Result<(), String> {
    // Test-only path preserving the snapshot-INSIDE-lock contract that the
    // §1295 hot-path PRE-LOCK prepare deliberately weakens (S2): the production
    // filter/prune hot path snapshots before the lock, but this helper (no
    // project-path filter/prune) keeps the snapshot under the lock so a parked
    // persist reflects state that changed while it waited (pinned by
    // `persist_current_state_captures_snapshot_inside_save_lock`).
    let carrier_permit = acquire_carrier_mutation_permit().await;
    snapshot_and_save_locked(mgr, dir, None, carrier_permit).await
}

pub async fn persist_current_state(mgr: &SessionManager) {
    if let Err(e) = persist_current_state_result(mgr).await {
        log::error!("Failed to persist sessions: {}", e);
    }
}

/// §1295 (O1-refined) — the ONLY persist entry that prunes dormant orphan rows
/// from the live `SessionManager` (`PersistMode::PruneDormant`). Used ONLY by
/// the three steady-state background drivers in lib.rs (context-scraper sink
/// :731, busy :1096, idle :1121). Every other persist caller is `NoPrune`.
pub async fn persist_current_state_prune_dormant(mgr: &SessionManager) {
    let dir = super::config_dir().ok_or("Could not determine home directory");
    let dir = match dir {
        Ok(dir) => dir,
        Err(e) => {
            log::error!("Failed to persist sessions (prune): {}", e);
            return;
        }
    };
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = match session_retention_project_paths(&settings) {
        Ok(paths) => paths,
        Err(error) => {
            log::error!("Session persistence blocked: {}", error);
            return;
        }
    };
    if let Err(e) = persist_current_state_to_dir_for_project_paths_result(
        mgr,
        &dir,
        Some(&project_paths),
        PersistMode::PruneDormant,
    )
    .await
    {
        log::error!("Failed to persist sessions (prune): {}", e);
    }
}

/// #698 — outcome of an atomic raise-hand-and-persist attempt.
#[derive(Debug)]
pub enum RaiseHandPersistOutcome {
    /// The hand was newly raised and the snapshot was saved durably. The caller
    /// should emit `session_communication_changed` with this communication.
    Raised(SessionCommunication),
    /// A visible raise-hand was already present; nothing changed and nothing was
    /// persisted. The raise is still active (`raised: true`, status `already_visible`).
    AlreadyVisible,
    /// The session cannot raise its hand (missing, non-coordinator, or exited);
    /// `raised: false`, status `not_visible`.
    NotRaisable,
}

/// #698 — apply the raise-hand transition and persist the resulting snapshot
/// atomically with respect to ALL session persistence.
///
/// Fix for the HIGH grinch finding: the live mutation and its durable snapshot
/// must not be observable independently. We take `sessions_save_lock()` FIRST,
/// so no concurrent `persist_current_state` caller can snapshot or write the
/// raised state between our mutation and our save. We then mutate, snapshot, and
/// save under that single lock. On save failure we roll back the live
/// communication BEFORE releasing the lock, so neither memory nor disk retains a
/// raise that did not survive: any later persist can only ever snapshot the
/// cleared state.
///
/// We deliberately do NOT call `persist_current_state_result` here; it would
/// re-acquire the same (non-reentrant) tokio mutex and deadlock. We call the
/// lock-free `snapshot_and_save_locked` instead.
pub async fn raise_hand_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<RaiseHandPersistOutcome, String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    raise_hand_and_persist_to_dir_result(mgr, session_id, now, &dir, Some(&project_paths)).await
}

async fn raise_hand_and_persist_to_dir_result(
    mgr: &SessionManager,
    session_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<RaiseHandPersistOutcome, String> {
    let manager = mgr.clone();
    let dir = dir.to_path_buf();
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        raise_hand_and_persist_to_dir_result_core(&manager, session_id, now, &dir, roots.as_deref())
            .await
    })
    .await
}

async fn raise_hand_and_persist_to_dir_result_core(
    mgr: &SessionManager,
    session_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<RaiseHandPersistOutcome, String> {
    let carrier_permit = acquire_carrier_mutation_permit().await;

    let (communication, receipt) = match mgr
        .raise_hand_with_carrier_permit(&carrier_permit, session_id, now)
        .await
    {
        RaiseCarrierOutcome::Changed(communication, receipt) => (communication, receipt),
        RaiseCarrierOutcome::AlreadyVisible(_) => {
            return Ok(RaiseHandPersistOutcome::AlreadyVisible)
        }
        RaiseCarrierOutcome::NotRaisable => return Ok(RaiseHandPersistOutcome::NotRaisable),
    };

    if let Err(e) = snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit).await {
        // Reacquire U and roll back only this nonce/version receipt. A successor
        // transition after a waiting recapture must remain visible.
        let carrier_permit = acquire_carrier_mutation_permit().await;
        let _ = mgr
            .rollback_raise_with_carrier_permit(&carrier_permit, receipt)
            .await;
        return Err(e);
    }

    Ok(RaiseHandPersistOutcome::Raised(communication))
}

/// #1669 - outcome of a set-blocked-menu-and-persist attempt. There is
/// deliberately no failure variant: a save failure cannot change the outcome,
/// because the in-memory transition stands and the caller must still publish it.
#[derive(Debug)]
pub enum BlockedMenuPersistOutcome {
    /// A real transition applied. The caller MUST emit
    /// `session_communication_changed` with this communication, whether or not
    /// the snapshot reached disk.
    Changed(SessionCommunication),
    /// The identical blocked-menu message was already visible. Nothing mutated,
    /// nothing written. This is the 250 ms scan-loop steady state and is why a
    /// blocked session does not write once per tick.
    Unchanged,
    /// The session is missing, pending-create, or exited. Nothing mutated,
    /// nothing written.
    NotApplicable,
}

/// #1669 - apply the blocked-menu set transition and persist the resulting
/// snapshot atomically with respect to ALL session persistence: take
/// `sessions_save_lock()` FIRST, then mutate, snapshot and save under that single
/// acquisition. Do NOT call `persist_current_state_result` here; it re-acquires
/// the same non-reentrant tokio mutex and would deadlock.
///
/// FAILURE POLICY DIFFERS FROM RAISE-HAND, deliberately: NO rollback, matching the
/// #756 contract in `write_start_fresh_and_persist_to_dir_result`; but the transition
/// is reported even when the save fails, which DIVERGES from #756 (it returns `Err`
/// and reports nothing). Raise-hand can roll back because a user can raise a hand
/// again; this cannot be retried. `MenuGuard::scan_tick`
/// evaluates only on a CHANGED screen (`ScreenRowsSince::Unchanged` is an empty
/// arm) and a menu-parked terminal emits no further frames, and
/// `evaluate_logical_rows` has already set `MenuGuardSessionState.is_blocked`,
/// which this function cannot reach. A rollback would therefore freeze PTY
/// injection permanently with no toast, no Sidebar indicator and no disk row.
///
/// `settings` is the caller's already-resolved `AppSettings`, not a
/// `load_settings_for_cli()` call: this runs on the 250 ms scan loop, that loader
/// is uncached, and `scan_tick` already holds a fresh clone.
pub async fn set_blocked_menu_and_persist(
    mgr: &SessionManager,
    session_id: Uuid,
    message: String,
    now: chrono::DateTime<chrono::Utc>,
    settings: &crate::config::settings::AppSettings,
) -> BlockedMenuPersistOutcome {
    let dir = super::config_dir();
    let project_paths = match session_retention_project_paths(settings) {
        Ok(paths) => paths,
        Err(error) => {
            log::error!("Blocked-menu persistence blocked: {}", error);
            return BlockedMenuPersistOutcome::NotApplicable;
        }
    };
    let (outcome, error) = set_blocked_menu_and_persist_to_dir(
        mgr,
        session_id,
        message,
        now,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await;
    if let Some(e) = error {
        log::error!("Failed to persist blocked-menu set: {}", e);
    }
    outcome
}

/// Returns `(outcome, save_error)`. `outcome` is the caller's publish gate;
/// `save_error` is reported separately because it must not suppress the publish.
async fn set_blocked_menu_and_persist_to_dir(
    mgr: &SessionManager,
    session_id: Uuid,
    message: String,
    now: chrono::DateTime<chrono::Utc>,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> (BlockedMenuPersistOutcome, Option<String>) {
    let manager = mgr.clone();
    let dir = dir.map(Path::to_path_buf);
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        set_blocked_menu_and_persist_to_dir_core(
            &manager,
            session_id,
            message,
            now,
            dir.as_deref(),
            roots.as_deref(),
        )
        .await
    })
    .await
}

async fn set_blocked_menu_and_persist_to_dir_core(
    mgr: &SessionManager,
    session_id: Uuid,
    message: String,
    now: chrono::DateTime<chrono::Utc>,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> (BlockedMenuPersistOutcome, Option<String>) {
    let carrier_permit = acquire_carrier_mutation_permit().await;

    let communication = match mgr
        .set_blocked_menu_with_carrier_permit(&carrier_permit, session_id, message, now)
        .await
    {
        Some((true, communication)) => communication,
        Some((false, _)) => return (BlockedMenuPersistOutcome::Unchanged, None),
        None => return (BlockedMenuPersistOutcome::NotApplicable, None),
    };

    let Some(dir) = dir else {
        return (
            BlockedMenuPersistOutcome::Changed(communication),
            Some("Could not determine home directory".to_string()),
        );
    };
    let error = snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit)
        .await
        .err();
    (BlockedMenuPersistOutcome::Changed(communication), error)
}

/// #1669 - clear a visible blocked-menu communication and persist the result
/// atomically. Returns `true` when a visible blocked-menu was actually cleared,
/// which is exactly when the caller must publish `communication: null`. When
/// nothing was visible to clear it returns BEFORE any save, so a scan tick over
/// an unblocked session writes nothing.
///
/// Same failure policy as the set path: no rollback, and the clear is reported
/// even when the save fails. This DIVERGES from
/// `clear_user_input_transitions_and_persist_result`, which suppresses its clear
/// event on save failure so `list-sessions` and the UI agree. That trade is right
/// for a raise-hand lower, whose absence is invisible; it is wrong here, because
/// this event dismisses a STICKY toast for a menu already gone from the screen and
/// the clear has no retry path. Do not "fix" it back. Failures are logged.
///
/// Unlike the set path this resolves its own settings: it is not on the scan loop
/// (at most one clear per episode, per guard-disable, and per user click).
pub async fn clear_blocked_menu_and_persist(mgr: &SessionManager, session_id: Uuid) -> bool {
    let dir = super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = match session_retention_project_paths(&settings) {
        Ok(paths) => paths,
        Err(error) => {
            log::error!("Session persistence blocked: {}", error);
            return false;
        }
    };
    let (cleared, error) = clear_blocked_menu_and_persist_to_dir(
        mgr,
        session_id,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await;
    if let Some(e) = error {
        log::error!("Failed to persist blocked-menu clear: {}", e);
    }
    cleared
}

/// Returns `(cleared, save_error)`. `cleared` is the caller's publish gate;
/// `save_error` is reported separately because it must not suppress the clear.
async fn clear_blocked_menu_and_persist_to_dir(
    mgr: &SessionManager,
    session_id: Uuid,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> (bool, Option<String>) {
    let manager = mgr.clone();
    let dir = dir.map(Path::to_path_buf);
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        clear_blocked_menu_and_persist_to_dir_core(
            &manager,
            session_id,
            dir.as_deref(),
            roots.as_deref(),
        )
        .await
    })
    .await
}

async fn clear_blocked_menu_and_persist_to_dir_core(
    mgr: &SessionManager,
    session_id: Uuid,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> (bool, Option<String>) {
    let carrier_permit = acquire_carrier_mutation_permit().await;

    if !mgr
        .clear_blocked_menu_with_carrier_permit(&carrier_permit, session_id)
        .await
    {
        return (false, None);
    }
    let Some(dir) = dir else {
        return (true, Some("Could not determine home directory".to_string()));
    };
    match snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit).await {
        Ok(()) => (true, None),
        Err(e) => (true, Some(e)),
    }
}

/// #698: the user-input session-state transitions cleared by
/// `clear_user_input_transitions_and_persist_result`, reported so the caller can
/// decide whether to emit the raise-hand clear event.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClearedUserInputTransitions {
    /// `start_fresh_on_restore` flipped `true -> false` (#630/#631 re-arm).
    pub cleared_start_fresh: bool,
    /// A visible raise-hand was lowered (#698).
    pub cleared_raise_hand: bool,
}

/// #698: clear the user-input session-state transitions and persist the result
/// atomically with respect to all session persistence. `clear_fresh` gates the
/// `start_fresh_on_restore` re-arm to substantive post-boundary submissions
/// (#871); lowering any visible raise-hand remains unconditional.
///
/// Fix for the MEDIUM grinch finding: the two field clears previously ran in two
/// separate critical sections with an await between them, so a concurrent persist
/// could snapshot a half-applied state (`startFreshOnRestore: false` with a still
/// visible `raiseHand`). Here both fields flip in ONE `SessionManager` critical
/// section (`clear_user_input_transitions`), and the mutation plus its snapshot
/// save run under a single `sessions_save_lock()` acquisition, so no other
/// persistence caller can write an intermediate state.
///
/// Unlike the raise-hand path there is no rollback: a real user message means the
/// hand is lowered and the fresh intent re-armed regardless of whether this
/// snapshot reaches disk. The in-memory clear is therefore applied unconditionally
/// (even when the home directory cannot be resolved); on save failure it stands
/// and the next persist reconciles the file. The caller learns of any failure
/// through `Err` and, per existing behavior, suppresses the clear event.
pub async fn clear_user_input_transitions_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
    clear_fresh: bool,
) -> Result<ClearedUserInputTransitions, String> {
    let dir = super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    clear_user_input_transitions_and_persist_to_dir_result(
        mgr,
        session_id,
        clear_fresh,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await
}

async fn clear_user_input_transitions_and_persist_to_dir_result(
    mgr: &SessionManager,
    session_id: Uuid,
    clear_fresh: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<ClearedUserInputTransitions, String> {
    let manager = mgr.clone();
    let dir = dir.map(Path::to_path_buf);
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        clear_user_input_transitions_and_persist_to_dir_result_core(
            &manager,
            session_id,
            clear_fresh,
            dir.as_deref(),
            roots.as_deref(),
        )
        .await
    })
    .await
}

async fn clear_user_input_transitions_and_persist_to_dir_result_core(
    mgr: &SessionManager,
    session_id: Uuid,
    clear_fresh: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<ClearedUserInputTransitions, String> {
    let carrier_permit = acquire_carrier_mutation_permit().await;

    // Mutate FIRST and unconditionally: the user typed, so the hand is lowered
    // and, when gated, the fresh intent is re-armed even if we cannot persist.
    // Both fields flip in one critical section, so no snapshot can capture a
    // half-applied state.
    let (cleared_start_fresh, cleared_raise_hand) = mgr
        .clear_user_input_transitions_with_carrier_permit(&carrier_permit, session_id, clear_fresh)
        .await;
    let cleared = ClearedUserInputTransitions {
        cleared_start_fresh,
        cleared_raise_hand,
    };

    if cleared_start_fresh || cleared_raise_hand {
        let dir = dir.ok_or("Could not determine home directory")?;
        snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit).await?;
    }

    Ok(cleared)
}

/// (#756) Stamp the durable fresh intent (AC-driven clear boundary) and persist
/// the result atomically with respect to all session persistence. Returns
/// Ok(true) iff the field transitioned false -> true (a snapshot was saved).
/// Same lock discipline as `clear_user_input_transitions_and_persist_result`:
/// mutation + snapshot + save under a single `sessions_save_lock()` acquisition,
/// so no concurrent persist can write an intermediate state.
pub async fn set_start_fresh_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
) -> Result<bool, String> {
    let dir = super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    write_start_fresh_and_persist_to_dir_result(
        mgr,
        session_id,
        true,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await
}

/// (#756) Drop the durable fresh intent (AC injected post-boundary CONTENT) and
/// persist. Returns Ok(true) iff the field transitioned true -> false.
/// DELIBERATELY NARROW: touches only `start_fresh_on_restore`; never raise-hand,
/// silence, or badge state (this is NOT the user-input choke point).
pub async fn clear_start_fresh_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
) -> Result<bool, String> {
    let dir = super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    write_start_fresh_and_persist_to_dir_result(
        mgr,
        session_id,
        false,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await
}

/// (#756) Shared core for the fresh-intent stamp/drop persist wrappers. Like the
/// #698 helper, the in-memory mutation stands even if the save fails (Err
/// reports the save failure; callers log a warn, no rollback).
async fn write_start_fresh_and_persist_to_dir_result(
    mgr: &SessionManager,
    session_id: Uuid,
    value: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<bool, String> {
    let manager = mgr.clone();
    let dir = dir.map(Path::to_path_buf);
    let roots = project_paths.map(<[String]>::to_vec);
    run_owned_persist_core(async move {
        write_start_fresh_and_persist_to_dir_result_core(
            &manager,
            session_id,
            value,
            dir.as_deref(),
            roots.as_deref(),
        )
        .await
    })
    .await
}

async fn write_start_fresh_and_persist_to_dir_result_core(
    mgr: &SessionManager,
    session_id: Uuid,
    value: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<bool, String> {
    let carrier_permit = acquire_carrier_mutation_permit().await;
    let changed = if value {
        mgr.set_start_fresh_on_restore_if_unset_with_carrier_permit(&carrier_permit, session_id)
            .await
    } else {
        mgr.clear_start_fresh_on_restore_if_set_with_carrier_permit(&carrier_permit, session_id)
            .await
    };
    if changed {
        let dir = dir.ok_or("Could not determine home directory")?;
        snapshot_and_save_locked(mgr, dir, project_paths, carrier_permit).await?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod issue_3145_io_tests;

#[cfg(test)]
mod derived_agent_binding_tests;
