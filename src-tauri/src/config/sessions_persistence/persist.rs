mod snapshot;

pub use self::snapshot::snapshot_sessions;
pub(super) use self::snapshot::{deduplicate_preserving_affected, snapshot_aggregate};
pub(crate) use self::snapshot::{sanitize_failed_recoverable, strip_auto_injected_args};
use super::carrier::{
    all_carrier_protections, probe_publication_directory, publication_slot,
    published_initial_carrier, resolve_publication_slot, same_carrier_projection,
    CarrierProtection, InitialCarrierIntent, PreparedCarrierRelation, PublicationSlot,
    PublishedCarrier,
};
use super::orphan::{handle_orphan_drops, prepare_persist_snapshot};
use super::project_scope::{
    filter_sessions_for_project_paths_blocking, session_retention_project_paths, OrphanDrop,
};
use super::record::PersistedSession;
use super::retained_prompt::flush_retained_prompts_locked;
use super::store::{load_sessions_from_path_with_settings, snapshot_path};
use super::write::{
    confirm_publication_readback, current_capture_matches, file_preparation_survives,
    run_owned_persist_core, save_current_snapshot_guarded, save_file_snapshot_guarded,
    snapshot_store, GuardedSaveAttempt, PersistMode, PersistPreparation,
};
use crate::session::manager::{
    acquire_carrier_mutation_permit, CarrierDisposition, CarrierMutationPermit, SessionManager,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub async fn load_sessions_purging_outside_project_paths(
    project_paths: &[String],
    settings: &super::super::settings::AppSettings,
) -> Vec<PersistedSession> {
    let dir = match super::super::config_dir() {
        Some(d) => d,
        None => {
            log::warn!("Could not determine home directory for session restore");
            return vec![];
        }
    };

    load_sessions_purging_outside_project_paths_in_dir(&dir, project_paths, Some(settings)).await
}

pub(super) async fn load_sessions_purging_outside_project_paths_in_dir(
    dir: &Path,
    project_paths: &[String],
    settings: Option<&super::super::settings::AppSettings>,
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
                load_sessions_from_path_with_settings(&snapshot_path(dir), settings),
                project_paths,
            )
            .await
            {
                Ok(filtered) => filtered,
                Err(e) => {
                    log::error!("Failed to filter sessions after purge failure: {}", e);
                    load_sessions_from_path_with_settings(&snapshot_path(dir), settings)
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
pub(super) async fn purge_sessions_outside_project_paths_in_dir_locked(
    dir: &Path,
    project_paths: &[String],
    settings: Option<&super::super::settings::AppSettings>,
) -> Result<(usize, Vec<PersistedSession>), String> {
    let dir = dir.to_path_buf();
    let roots = project_paths.to_vec();
    let settings = settings.cloned();
    run_owned_persist_core(async move {
        let mut permit = acquire_carrier_mutation_permit().await;
        let settings = settings.unwrap_or_else(super::super::settings::load_settings_for_cli);
        let mut foreign_done = HashSet::new();
        loop {
            let before =
                load_sessions_from_path_with_settings(&snapshot_path(&dir), Some(&settings));
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

/// Persist live sessions plus stripped recipes for entries that failed to
/// restore. Stripped recipes survive on disk only until the next
/// `persist_current_state` call (any session-lifecycle event) overwrites the
/// snapshot — so retry-on-next-startup is best-effort. §224 G5/G8.
pub async fn persist_merging_failed_result(
    mgr: &SessionManager,
    failed: &[PersistedSession],
) -> Result<(), String> {
    let dir = super::super::config_dir().ok_or("Could not determine home directory")?;
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings)?;
    persist_merging_failed_to_dir_for_project_paths_result(mgr, failed, &dir, Some(&project_paths))
        .await
}

pub(super) async fn persist_merging_failed_to_dir_for_project_paths_result(
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

pub(super) async fn persist_merging_failed_to_dir_for_project_paths_result_core(
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
    let dir = super::super::config_dir().ok_or("Could not determine home directory")?;
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

pub(super) async fn persist_current_state_to_dir_for_project_paths_result(
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

pub(super) async fn persist_current_state_to_dir_for_project_paths_result_core(
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
            let epoch = current_publication_epoch(dir);
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
        let relations = prepare_carrier_relations(&prep, &captured.3);
        let drop_keys = prep.drops.iter().map(orphan_effect_key).collect::<Vec<_>>();
        let manager_relations = prepare_manager_relations(&captured.0, &captured.3);
        let carrier_permit = acquire_carrier_mutation_permit().await;
        if initial
            .as_ref()
            .is_some_and(|intent| !intent.restore_allows_publication())
        {
            return Ok(None);
        }
        let protections = all_carrier_protections();
        let epoch = current_publication_epoch(dir);
        let current = mgr
            .with_current_carrier_snapshot(&carrier_permit, |current| {
                current_capture_matches(current, &captured.0)
            })
            .await;
        let valid_protections = protections.as_ref().is_some_and(|current| {
            current_protections_match(current, &captured.3, &relations, &prep.drops)
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
        let dispositions = classify_manager_relations(&manager_relations, &captured.3);
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
                return complete_current_publication(
                    mgr,
                    dir,
                    initial.as_ref(),
                    &prep.kept,
                    &captured.0,
                    result,
                    carrier_permit,
                )
                .await;
            }
            GuardedSaveAttempt::Recapture => {
                drop(carrier_permit);
                tokio::task::yield_now().await;
            }
        }
    }
}

fn current_publication_epoch(dir: &Path) -> Option<u64> {
    publication_slot(dir).and_then(|slot| slot.lock().ok().map(|state| state.epoch))
}

async fn complete_current_publication(
    mgr: &SessionManager,
    dir: &Path,
    initial: Option<&InitialCarrierIntent>,
    kept: &[PersistedSession],
    captured: &crate::session::manager::CarrierSnapshot,
    result: Result<bool, String>,
    carrier_permit: CarrierMutationPermit,
) -> Result<Option<PublishedCarrier>, String> {
    let written = match result {
        Ok(written) => written,
        Err(error) => {
            mgr.disable_prompt_reuse().await;
            return Err(error);
        }
    };
    let initial = initial.filter(|intent| intent.matches(captured, kept));
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
        log::warn!(
            "[last-prompt] initial carrier checkpoint retained after sidecar \
             failure: {error}"
        );
    }
    drop(carrier_permit);
    Ok(wait_initial_publication(dir, initial).await)
}

fn prepare_manager_relations(
    captured: &crate::session::manager::CarrierSnapshot,
    protections: &[CarrierProtection],
) -> HashMap<Uuid, PreparedCarrierRelation> {
    captured
        .rows
        .iter()
        .map(|row| {
            (
                row.id,
                PreparedCarrierRelation::capture_manager(row, protections),
            )
        })
        .collect()
}

fn classify_manager_relations(
    relations: &HashMap<Uuid, PreparedCarrierRelation>,
    protections: &[CarrierProtection],
) -> HashMap<Uuid, CarrierDisposition> {
    relations
        .iter()
        .map(|(id, relation)| (*id, relation.classify(protections)))
        .collect()
}

fn prepare_carrier_relations(
    prep: &PersistPreparation,
    protections: &[CarrierProtection],
) -> Vec<PreparedCarrierRelation> {
    prep.kept
        .iter()
        .chain(prep.drops.iter().map(|drop| &drop.session))
        .map(|row| PreparedCarrierRelation::capture(row, protections))
        .collect()
}

fn current_protections_match(
    current: &[CarrierProtection],
    captured: &[CarrierProtection],
    relations: &[PreparedCarrierRelation],
    drops: &[OrphanDrop],
) -> bool {
    same_protection_set(current, captured)
        && relations
            .iter()
            .all(|relation| relation.classify(current) != CarrierDisposition::Unknown)
        && drops.iter().all(|drop| {
            !current.iter().any(|protection| {
                drop.session
                    .id
                    .as_deref()
                    .and_then(|id| Uuid::parse_str(id).ok())
                    == Some(protection.session_id)
            })
        })
}

/// A post-rename readback loss is Pending, not a save failure or a new write.
/// The accepted owner retains the Initial lifetime while reconciliation reads
/// the same directory leaf. A superseding version cannot register this intent.
pub(super) async fn wait_initial_publication(
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
        if let Some(leaf) = snapshot_store().try_lock_snapshot() {
            confirm_publication_readback(&slot, &snapshot_path(dir), checkpoint.0, checkpoint.1);
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
    if super::super::settings::agents_layer_unreadable_for_dir(&dir) {
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

pub(super) fn orphan_effect_key(
    drop: &OrphanDrop,
) -> (Option<String>, String, String, String, Vec<String>) {
    (
        drop.session.id.clone(),
        drop.session.working_directory.clone(),
        drop.session.name.clone(),
        drop.session.shell.clone(),
        drop.session.shell_args.clone(),
    )
}

pub(super) fn protected_keys_survive(
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

pub(super) fn protected_preparation_survives(
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
pub(super) async fn persist_preparation_owned(
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

pub(super) fn same_protection_set(a: &[CarrierProtection], b: &[CarrierProtection]) -> bool {
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
pub(super) async fn snapshot_and_save_locked(
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
    let dir = super::super::config_dir().ok_or("Could not determine home directory");
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
