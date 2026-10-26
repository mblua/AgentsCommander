use super::carrier::{all_carrier_protections, PreparedCarrierRelation};
#[cfg(test)]
use super::project_scope::record_filter_project_paths_thread_id;
use super::project_scope::{normalize_project_roots, partition_orphaned_sessions, OrphanDrop};
use super::record::{normalized_cwd_key, PersistedSession};
#[cfg(test)]
use super::retained_prompt::flush_retained_prompts_locked;
#[cfg(test)]
use super::write::save_sessions_to_dir;
use super::write::{run_owned_persist_core, PersistMode, PersistPreparation};
use crate::session::manager::{
    acquire_carrier_mutation_permit, CarrierDisposition, CarrierMutationPermit, SessionManager,
};
use crate::session::session::SessionStatus;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

/// §1295 — orphaned-sessions archive store (mechanism C / B1). See §5.4.
/// NDJSON (one JSON object per line), append-only, never parsed back by any
/// version (old AC versions ignore it). The EXISTS probe at drop time decides
/// the disposition label, never whether the recipe is kept.
pub(crate) const ORPHAN_ARCHIVE_FILENAME: &str =
    crate::config::instance_artifacts::ORPHAN_ARCHIVE_FILENAME;

/// §1295 — soft cap on the ACTIVE archive file before best-effort rotation.
pub(super) const ORPHAN_ARCHIVE_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// §1295 — number of rotated archive generations kept. Mirrors `logging.rs`
/// `APP_LOG_KEEP` semantics: the active file, then `.1` .. `.KEEP - 1`; the
/// oldest `.KEEP` copy is dropped on the next rotation.
pub(super) const ORPHAN_ARCHIVE_KEEP: u32 = 3;

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
pub(super) fn note_orphan_cwd(cwd: &str) -> bool {
    let key = normalized_cwd_key(cwd);
    let mut registry = orphan_warned_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if registry.insert(key) {
        #[cfg(test)]
        bump_orphan_warned();
        log::warn!(
            "[sessions] Dropping orphan persisted session at '{}' (outside current projectPaths); \
             one WARN per orphan per run",
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
pub(super) fn note_orphan_archived_id(uuid: &Uuid) -> bool {
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
    static ORPHAN_WARNED_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
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
pub(super) fn bump_orphan_warned() {
    ORPHAN_WARNED_COUNT.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(super) fn accumulate_orphan_counters(counts: OrphanSweepCounts) {
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

pub(super) fn orphan_archive_max_bytes() -> u64 {
    #[cfg(test)]
    {
        let slot = TEST_ORPHAN_ARCHIVE_CAP.get_or_init(|| Mutex::new(None));
        if let Some(cap) = *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) {
            return cap;
        }
    }
    ORPHAN_ARCHIVE_MAX_BYTES
}

/// §1295 5.4 — one NDJSON record for one orphaned session drop (B1: every drop
/// writes exactly one record, always). `droppedAt` is RFC3339 UTC.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OrphanArchiveRecord<'a> {
    pub(super) schema_version: u32,
    pub(super) dropped_at: String,
    pub(super) reason: &'a str,
    pub(super) disposition: &'a str,
    pub(super) session: &'a PersistedSession,
}

/// §1295 5.4 — append archive records are written with one `writeln!` per
/// record under `sessions_save_lock` (locked variant for site C). Best-effort:
/// an append failure logs ERROR and NEVER fails the persist (a write failure
/// must not resurrect the WARN loop). Cross-instance interleaving may produce
/// torn/chunked lines (accepted, the archive is never parsed).
pub(super) fn append_orphan_archive_record_locked(
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
pub(super) fn rotate_orphan_archive(archive_path: &Path) {
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
                    "[sessions] orphan archive rotation: failed to rename {} to {}: {} \
                     (continuing)",
                    from.display(),
                    to.display(),
                    e
                );
            }
        }
    }

    if let Err(e) = std::fs::rename(archive_path, numbered(1)) {
        log::error!(
            "[sessions] orphan archive rotation: failed to rename {} to {}: {} (active file stays \
             in place)",
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

pub(crate) async fn prepare_persist_snapshot(
    snapshot: Vec<PersistedSession>,
    project_paths: &[String],
) -> Result<PersistPreparation, String> {
    let project_paths = project_paths.to_vec();
    let scope = super::operation::OperationScope::current();
    let work = move || {
        #[cfg(test)]
        let scope = super::operation::OperationScope::current();
        #[cfg(test)]
        if let Some(scope) = &scope {
            scope.point_blocking(super::operation::ProbeEvent::BlockingPrepareEntered);
        }
        #[cfg(test)]
        record_filter_project_paths_thread_id();
        let roots = normalize_project_roots(&project_paths);
        let (kept, mut drops) = partition_orphaned_sessions(snapshot, &roots);
        for drop in &mut drops {
            drop.cwd_exists = Path::new(&drop.session.working_directory).exists();
        }
        let prepared = PersistPreparation { kept, drops };
        #[cfg(test)]
        if let Some(scope) = &scope {
            scope.point_blocking(super::operation::ProbeEvent::BlockingPrepareExited);
        }
        prepared
    };
    if let Some(scope) = scope {
        return scope
            .reserve("prepare_persist_snapshot")
            .map_err(|error| format!("session persistence prepare registration failed: {error:?}"))?
            .run_blocking_single(work)
            .await
            .map_err(|error| format!("session persistence prepare task failed: {error:?}"));
    }
    tokio::task::spawn_blocking(work)
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
pub(super) async fn handle_orphan_drops(
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
pub(super) async fn persist_prepared_locked(
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
