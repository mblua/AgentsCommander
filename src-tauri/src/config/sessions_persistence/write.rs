use super::carrier::{
    prepare_protected_publication, publication_slot, resolve_publication_slot,
    same_carrier_projection, same_written_projection, CarrierProtection, InitialCarrierIntent,
    PreparedCarrierRelation, PublicationCheckpoint, PublicationDirectoryIdentity, PublicationSlot,
    PublicationState,
};
use super::project_scope::OrphanDrop;
use super::record::PersistedSession;
use super::store::{
    PreparedSessionsWrite, RenameDiagnostics, SnapshotLeafGuard, RENAME_ATTEMPTS,
    RENAME_BACKOFFS_MS,
};
use crate::session::manager::{CarrierDisposition, CarrierMutationPermit, SessionManager};
use std::collections::HashMap;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;
use uuid::Uuid;

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
pub(super) enum PersistOperationState<T> {
    Running,
    Completed(Option<T>),
    Ambiguous,
}

pub(super) struct PersistOperationCell<T> {
    pub(super) state: Mutex<PersistOperationState<T>>,
    pub(super) changed: tokio::sync::Notify,
}

pub(super) struct PersistOperationOwner<T> {
    pub(super) cell: std::sync::Arc<PersistOperationCell<T>>,
    pub(super) completed: bool,
}

impl<T> PersistOperationOwner<T> {
    pub(super) fn complete(mut self, result: T) {
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
    if let Some(scope) = super::operation::OperationScope::current() {
        return scope.run_owned(operation).await;
    }
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

/// #291 — counter feeding the per-call unique temp filename for
/// `save_sessions`. Combined with the PID it makes the temp filename
/// `sessions.json.<pid>.<op_id>.tmp` distinct from any concurrent in-process
/// or cross-process save, and from any leftover temp file written by a
/// prior crashed run. Kept separate from `rename_with_retry`'s `OP_ID` so
/// the two counters can be reasoned about independently in diagnostics.
static SAVE_OP_ID: AtomicU64 = AtomicU64::new(0);

static OP_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn snapshot_store() -> super::store::SnapshotStore<'static> {
    super::store::SnapshotStore::new(&SAVE_SESSIONS_LOCK, &SAVE_OP_ID, &OP_ID)
}

pub(crate) fn rename_with_retry(tmp: &Path, dst: &Path) -> Result<(), (String, RenameDiagnostics)> {
    snapshot_store().rename_with_retry(tmp, dst)
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
pub(super) fn save_sessions_to_config_dir(sessions: &[PersistedSession]) -> Result<(), String> {
    let dir = super::super::config_dir().ok_or("Could not determine home directory")?;
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
pub(super) fn save_sessions_to_dir(
    dir: &Path,
    sessions: &[PersistedSession],
) -> Result<(), String> {
    if super::super::settings::agents_layer_unreadable_for_dir(dir) {
        return Ok(());
    }
    let _guard = snapshot_store().lock_snapshot();
    let write = snapshot_store().prepare(dir, sessions)?;
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

pub(super) enum GuardedSaveAttempt {
    Completed(Result<bool, String>),
    Recapture,
}

pub(super) enum RenamePublicationAttempt {
    Renamed {
        leaf: SnapshotLeafGuard<'static>,
        retired: PublicationState,
    },
    Suppressed,
    Retry,
    Stale,
    Failed(std::io::Error),
}

pub(super) fn current_capture_matches(
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

pub(super) fn rename_and_publish(
    dir: &Path,
    slot: &PublicationSlot,
    expected_epoch: u64,
    next: &mut Option<PublicationState>,
    write: &PreparedSessionsWrite,
) -> RenamePublicationAttempt {
    let guarded =
        super::super::settings::try_with_agents_layer_publication_state(dir, |suppressed| {
            if suppressed {
                return RenamePublicationAttempt::Suppressed;
            }
            let leaf = match snapshot_store().try_lock_snapshot() {
                Some(leaf) => leaf,
                None => return RenamePublicationAttempt::Retry,
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
            match snapshot_store().replace_prepared_once(write) {
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

pub(super) fn confirm_publication_readback(
    slot: &PublicationSlot,
    path: &Path,
    nonce: Uuid,
    epoch: u64,
) {
    let actual = super::store::readback_snapshot(path);
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

pub(super) fn file_preparation_survives(
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

pub(super) async fn save_file_snapshot_guarded(
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
    if super::super::settings::agents_layer_unreadable_for_dir(dir) {
        return GuardedSaveAttempt::Completed(Ok(false));
    }
    let write = match snapshot_store().prepare(dir, rows) {
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
                log::debug!(
                    "[sessions] rename attempt {}/{} failed — op_id={} pid={} os_error={:?} \
                     kind={:?}",
                    attempt + 1,
                    RENAME_ATTEMPTS,
                    write.op_id,
                    write.pid,
                    error.raw_os_error(),
                    error.kind()
                );
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

pub(super) enum SettledRenameAttempt {
    Completed(bool),
    Recapture,
    Failed(std::io::Error),
}

/// Consume every non-Send leaf guard synchronously. Only this Send result can
/// reach the caller's retry await; readback still holds the exact original leaf.
pub(super) fn settle_rename_attempt(
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
                        crate::filesystem::corrupt_session_container_for_test(path)
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

pub(super) async fn save_current_snapshot_guarded(
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
    if super::super::settings::agents_layer_unreadable_for_dir(dir) {
        return GuardedSaveAttempt::Completed(Ok(false));
    }
    let current = mgr
        .with_current_carrier_snapshot(permit, |current| current_capture_matches(current, captured))
        .await;
    if !current {
        return GuardedSaveAttempt::Recapture;
    }
    let write = match snapshot_store().prepare(dir, rows) {
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
                    "[sessions] rename attempt {}/{} failed — op_id={} pid={} os_error={:?} \
                     kind={:?}",
                    attempt + 1,
                    RENAME_ATTEMPTS,
                    write.op_id,
                    write.pid,
                    error.raw_os_error(),
                    error.kind(),
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
pub(super) fn sessions_save_lock() -> &'static tokio::sync::Mutex<()> {
    crate::session::manager::process_carrier_publication_gate().as_ref()
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum FixturePublicationActionKind {
    CorruptFixtureLeafOnce,
    PanicOnce,
}

#[cfg(test)]
pub(super) struct FixturePublicationAction {
    pub(super) operation: Uuid,
    pub(super) nonce: Uuid,
    pub(super) directory: PathBuf,
    pub(super) kind: FixturePublicationActionKind,
}
