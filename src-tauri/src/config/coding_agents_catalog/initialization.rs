use std::path::Path;

use chrono::{DateTime, Utc};

use crate::config::naming_migration;
use crate::config::seed_manifest::{
    acquire_project_gate_soft, has_catalog_publication, ManifestActivationToken,
    ManifestPathIdentity, ProjectSeedManifestGuard, PublishedManifestRow, SoftProjectGate,
};
use crate::filesystem::{catalog, config_seed};

use super::{
    acquire_catalog_lock, analyze_base_bytes, catalog_diagnostic, catalog_dir, ensure_catalog_dir,
    ensure_seeded_masters, fresh_initialize_catalog, migrate_legacy, read_instance_legacy_source,
    read_optional_regular_file, recover_interrupted_migration, refresh_managed_base,
    resume_backup_only_migration, CatalogBaseKind, CatalogDiagnostic, CatalogInitOutcome,
    CatalogPaths, CATALOG_DIR_NAME, CATALOG_LOCK_FILENAME, CATALOG_MANIFEST_FILENAME,
    LOCAL_CATALOG_FILENAME, MIGRATION_BACKUP_FILENAME, MIGRATION_JOURNAL_FILENAME,
    MIGRATION_SOURCE_INSTANCE, MIGRATION_SOURCE_PROJECT, REPORT_CODE_MIGRATION_CONFLICT,
    REPORT_CODE_REFRESH_FAILED,
};

/// Which surface is being initialized. `Project` keeps the complete project
/// state machine above it (recovery, migration, seed-manifest ownership);
/// `Instance` is the #2021 no-project instance catalog, which may only
/// fresh-seed an absent base or refresh a stale managed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitScope {
    Project,
    Instance,
}

/// Recover project migration sidecars before inspecting the persisted base.
/// Returns false when a diagnostic prevents initialization from continuing.
fn recover_project_catalog(
    paths: &CatalogPaths,
    legacy_catalog_dir: Option<&Path>,
    outcome: &mut CatalogInitOutcome,
) -> bool {
    let journal_exists = match config_seed::inspect_seed_destination(&paths.journal) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            outcome.warnings.push(catalog_diagnostic(
                REPORT_CODE_REFRESH_FAILED,
                &paths.journal,
                format!("the migration journal could not be inspected ({error})"),
            ));
            return false;
        }
    };
    let backup_exists = match config_seed::inspect_seed_destination(&paths.backup) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            outcome.warnings.push(catalog_diagnostic(
                REPORT_CODE_REFRESH_FAILED,
                &paths.backup,
                format!("the migration backup could not be inspected ({error})"),
            ));
            return false;
        }
    };

    if journal_exists {
        match recover_interrupted_migration(paths) {
            Ok(published_at) => outcome.published_at = published_at,
            Err(reason) => {
                outcome.warnings.push(catalog_diagnostic(
                    REPORT_CODE_MIGRATION_CONFLICT,
                    &paths.journal,
                    reason,
                ));
                return false;
            }
        }
    } else if backup_exists {
        match resume_backup_only_migration(paths, legacy_catalog_dir) {
            Ok(published_at) => outcome.published_at = published_at,
            Err(reason) => {
                outcome.warnings.push(catalog_diagnostic(
                    REPORT_CODE_MIGRATION_CONFLICT,
                    &paths.backup,
                    reason,
                ));
                return false;
            }
        }
    }
    true
}

/// One initialization pass UNDER THE HELD CATALOG LOCK: recover an interrupted
/// transaction, then refresh, migrate or fresh-seed exactly one base. Order is
/// fixed by the plan: recovery first, then the base state machine. `scope`
/// selects the #2021 instance policy: instance initialization never resumes a
/// migration journal or backup, and never migrates a legacy file in place.
pub(crate) fn initialize_catalog_under_lock(
    paths: &CatalogPaths,
    legacy_catalog_dir: Option<&Path>,
    scope: InitScope,
) -> CatalogInitOutcome {
    let mut outcome = CatalogInitOutcome::default();

    // #2021: no production caller has ever run this initialization on the
    // config dir, so a journal or backup beside an instance catalog cannot be
    // ours. Leave any such sidecar untouched and never resume it.
    if scope == InitScope::Project
        && !recover_project_catalog(paths, legacy_catalog_dir, &mut outcome)
    {
        return outcome;
    }

    let base_bytes = match read_optional_regular_file(&paths.base, "persisted catalog") {
        Ok(bytes) => bytes,
        Err(reason) => {
            outcome.warnings.push(catalog_diagnostic(
                REPORT_CODE_REFRESH_FAILED,
                &paths.base,
                reason,
            ));
            return outcome;
        }
    };

    match base_bytes {
        Some(bytes) => {
            let analysis = match analyze_base_bytes(&paths.base, &bytes) {
                Ok(analysis) => analysis,
                Err(diagnostic) => {
                    // Corrupt bytes are preserved and unavailable; nothing is
                    // rewritten, refreshed or migrated.
                    outcome.warnings.push(diagnostic);
                    return outcome;
                }
            };
            outcome.base_verified_managed =
                analysis.kind == CatalogBaseKind::Managed && !analysis.edited;
            outcome.warnings.extend(analysis.warnings.iter().cloned());
            match analysis.kind {
                CatalogBaseKind::Managed => {
                    if !analysis.edited && !analysis.refresh_blocked {
                        match refresh_managed_base(paths, &analysis) {
                            Ok(Some(published_at)) => outcome.published_at = Some(published_at),
                            Ok(None) => {}
                            Err(reason) => {
                                outcome.warnings.push(catalog_diagnostic(
                                    REPORT_CODE_REFRESH_FAILED,
                                    &paths.base,
                                    reason,
                                ));
                            }
                        }
                    }
                }
                CatalogBaseKind::ForeignManaged => {}
                CatalogBaseKind::Legacy => {
                    // #2021: a legacy instance file is the migration source for
                    // future projects; the instance scope never migrates or
                    // rewrites it in place.
                    if scope == InitScope::Project {
                        match migrate_legacy(paths, MIGRATION_SOURCE_PROJECT, &paths.base, &bytes) {
                            Ok(published_at) => {
                                outcome.published_at = published_at.or(outcome.published_at)
                            }
                            Err(reason) => outcome.warnings.push(catalog_diagnostic(
                                REPORT_CODE_MIGRATION_CONFLICT,
                                &paths.base,
                                reason,
                            )),
                        }
                    }
                }
            }
        }
        None => {
            // An absent project base may import ONLY the instance agents.json.
            let instance_path = legacy_catalog_dir.map(|dir| dir.join(CATALOG_MANIFEST_FILENAME));
            let instance_bytes = match instance_path.as_ref() {
                Some(path) => match read_instance_legacy_source(path) {
                    Ok(bytes) => bytes,
                    Err(reason) => {
                        outcome.warnings.push(catalog_diagnostic(
                            REPORT_CODE_MIGRATION_CONFLICT,
                            path,
                            reason,
                        ));
                        return outcome;
                    }
                },
                None => None,
            };
            // #2021 2.3: a MANAGED (or foreign-managed) instance base is not
            // importable legacy territory; importing it would fail with
            // MIGRATION_CONFLICT and leave the project without a catalog, so
            // fresh-seed the project's own managed base instead. A marker-less
            // legacy file, or bytes this build cannot parse, stay on the
            // existing migration path unchanged (it already reports corrupt
            // input).
            let instance_is_managed = match (instance_bytes.as_deref(), instance_path.as_ref()) {
                (Some(bytes), Some(path)) => matches!(
                    analyze_base_bytes(path, bytes),
                    Ok(analysis)
                        if matches!(
                            analysis.kind,
                            CatalogBaseKind::Managed | CatalogBaseKind::ForeignManaged
                        )
                ),
                _ => false,
            };
            match (instance_bytes, instance_path) {
                (Some(bytes), Some(path)) if !instance_is_managed => {
                    match migrate_legacy(paths, MIGRATION_SOURCE_INSTANCE, &path, &bytes) {
                        Ok(published_at) => outcome.published_at = published_at,
                        Err(reason) => outcome.warnings.push(catalog_diagnostic(
                            REPORT_CODE_MIGRATION_CONFLICT,
                            &path,
                            reason,
                        )),
                    }
                }
                _ => match fresh_initialize_catalog(paths) {
                    Ok((published_at, stub_warning)) => {
                        outcome.published_at = published_at;
                        outcome.warnings.extend(stub_warning);
                    }
                    Err(reason) => outcome.warnings.push(catalog_diagnostic(
                        REPORT_CODE_REFRESH_FAILED,
                        &paths.base,
                        reason,
                    )),
                },
            }
        }
    }

    outcome
}

pub(crate) fn run_catalog_initialization(
    ac_dir: &Path,
    legacy_catalog_dir: Option<&Path>,
    journal_dir: Option<&Path>,
) -> CatalogInitOutcome {
    run_catalog_initialization_with_scope(
        ac_dir,
        legacy_catalog_dir,
        journal_dir,
        InitScope::Project,
    )
}

/// #2021: initialize or refresh the INSTANCE catalog at
/// `<config_dir>/coding-agents/agents.json` when no project is registered, so
/// the first-run Welcome surface lists the built-in coding agents.
///
/// Same lock discipline, path resolution and fail-soft logging as
/// [`run_catalog_initialization`], with instance semantics: a fresh or stale
/// managed base is initialized or refreshed, while a legacy, foreign-managed,
/// edited or corrupt instance file is never rewritten and never migrated. The
/// instance neither imports from nor publishes to another location. Returns
/// the publication time when the managed base was actually written.
pub(crate) fn ensure_seeded_instance(config_dir: &Path) -> Option<DateTime<Utc>> {
    ensure_seeded_instance_in(config_dir, Some(config_dir))
}

/// [`ensure_seeded_instance`] with the naming-migration journal directory
/// injected (#2715), so a test never touches the process-wide `config_dir()`.
pub(crate) fn ensure_seeded_instance_in(
    config_dir: &Path,
    journal_dir: Option<&Path>,
) -> Option<DateTime<Utc>> {
    run_catalog_initialization_with_scope(config_dir, None, journal_dir, InitScope::Instance)
        .published_at
}

pub(crate) fn run_catalog_initialization_with_scope(
    ac_dir: &Path,
    legacy_catalog_dir: Option<&Path>,
    journal_dir: Option<&Path>,
    scope: InitScope,
) -> CatalogInitOutcome {
    // #2715 5.4: the legacy instance catalog first, before any project lock
    // exists for this pass, so the two catalog locks never nest. A failure
    // seeds nothing: a project must not get defaults beside a source that
    // could not be moved.
    if let Some(legacy) = legacy_catalog_dir {
        if let Err(diagnostic) = migrate_legacy_catalog_scope(legacy, journal_dir) {
            return fail_soft_outcome(diagnostic);
        }
    }
    let dir = match ensure_catalog_dir(ac_dir) {
        Ok(dir) => dir,
        Err(reason) => {
            // Recorded and retried: an unreachable directory is never settled.
            let _ = naming_migration::update_journal(journal_dir, |journal| {
                journal.set_status(
                    &catalog_scope_key(&catalog_dir(ac_dir)),
                    naming_migration::ScopeStatus::Unreachable,
                );
            });
            return CatalogInitOutcome {
                published_at: None,
                base_verified_managed: false,
                warnings: vec![catalog_diagnostic(
                    REPORT_CODE_REFRESH_FAILED,
                    ac_dir,
                    reason,
                )],
            };
        }
    };
    let paths = CatalogPaths::new(&dir);
    let _lock = match acquire_catalog_lock(&paths) {
        Ok(lock) => lock,
        Err(reason) => {
            return CatalogInitOutcome {
                published_at: None,
                base_verified_managed: false,
                warnings: vec![catalog_diagnostic(
                    REPORT_CODE_REFRESH_FAILED,
                    &paths.lock,
                    reason,
                )],
            }
        }
    };
    // #2715 5.1: the family rename under the held catalog lock, immediately
    // before the base can be created. `lock_scope_under_held` adds only the old
    // sidecar: re-opening the new one would block on this pass's own handle.
    let migrated = naming_migration::lock_scope_under_held(
        &dir,
        Some(RETIRED_CATALOG_LOCK_FILENAME),
        naming_migration::MIGRATION_LOCK_BUDGET,
    )
    .map_err(|refusal| {
        catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            &dir.join(RETIRED_CATALOG_LOCK_FILENAME),
            format!(
                "the naming migration could not lock the retired catalog sidecar: {}",
                naming_refusal_reason(refusal)
            ),
        )
    })
    .and_then(|held| migrate_catalog_family(&dir, legacy_catalog_dir, journal_dir, scope, &held));
    let migration_published_at = match migrated {
        Ok(published_at) => published_at,
        Err(diagnostic) => return fail_soft_outcome(diagnostic),
    };
    let mut outcome = initialize_catalog_under_lock(&paths, legacy_catalog_dir, scope);
    outcome.published_at = outcome.published_at.or(migration_published_at);
    for warning in &outcome.warnings {
        log::warn!(
            "[coding-agents] {} at {}: {}",
            warning.code,
            warning.path,
            warning.reason
        );
    }
    outcome
}

// ---------------------------------------------------------------------------
// #2715 (phase B2 of #2703) - the `agents.*` family rename, per catalog
// directory, inside the one function every seeding entry point reaches.
// ---------------------------------------------------------------------------

/// The pre-migration names of the two catalog data files, in rename order.
pub(crate) const CATALOG_FAMILY_RENAMES: [naming_migration::Rename; 2] = [
    naming_migration::Rename {
        from: "agents.json",
        to: CATALOG_MANIFEST_FILENAME,
    },
    naming_migration::Rename {
        from: "agents.local.json",
        to: LOCAL_CATALOG_FILENAME,
    },
];

/// The pre-migration catalog lock sidecar. Held beside the new one for the
/// whole scope, so a pre-migration binary is still excluded; never renamed.
pub(crate) const RETIRED_CATALOG_LOCK_FILENAME: &str = ".agents.json.lock";

/// One scope per canonical catalog directory, whether it is reached as a
/// project `ac_dir` or as the legacy instance directory.
pub(crate) fn catalog_scope_key(canonical_dir: &Path) -> String {
    format!("catalog:{}", canonical_dir.display())
}

pub(crate) fn naming_refusal_reason(refusal: naming_migration::Refusal) -> String {
    match refusal {
        naming_migration::Refusal::LockUnavailable => "lock unavailable".to_string(),
        naming_migration::Refusal::Io(message) => message,
    }
}

pub(crate) fn catalog_path_present(path: &Path) -> Result<bool, CatalogDiagnostic> {
    match config_seed::inspect_seed_destination(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            path,
            format!("the naming migration could not inspect this path ({error})"),
        )),
    }
}

/// Move the catalog family in `dir` (canonical) to its target names under the
/// caller's held locks. Rule F1: in a project directory an interrupted #1968
/// migration is finished first, over the PRE-migration names, and a blocked
/// one defers the scope and renames nothing. The instance directory never
/// resumes a sidecar (#2021). Returns the recovery's publication instant.
pub(crate) fn migrate_catalog_family(
    dir: &Path,
    legacy_catalog_dir: Option<&Path>,
    journal_dir: Option<&Path>,
    scope: InitScope,
    held: &naming_migration::ScopeLock,
) -> Result<Option<DateTime<Utc>>, CatalogDiagnostic> {
    let key = catalog_scope_key(dir);
    let journal_error = |refusal| {
        catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            dir,
            format!(
                "the naming-migration journal failed: {}",
                naming_refusal_reason(refusal)
            ),
        )
    };
    // `scope_is_settled`, never `is_complete`: a `Complete` the disk
    // contradicts is re-run.
    let journal = naming_migration::read_journal(journal_dir).map_err(journal_error)?;
    if naming_migration::scope_is_settled(journal.as_ref(), &key) {
        return Ok(None);
    }

    let old = CatalogPaths::with_names(
        dir,
        CATALOG_FAMILY_RENAMES[0].from,
        CATALOG_FAMILY_RENAMES[1].from,
    );
    let journal_present = catalog_path_present(&old.journal)?;
    let backup_present = catalog_path_present(&old.backup)?;
    // With neither old data file on disk there is no rename to protect, so F1
    // has nothing to do; `initialize_catalog_under_lock` recovers over the new
    // names as before.
    let has_old_data = catalog_path_present(&old.base)? || catalog_path_present(&old.local)?;
    let mut published_at = None;
    if scope == InitScope::Project && has_old_data && (journal_present || backup_present) {
        let (recovered, sidecar) = if journal_present {
            (recover_interrupted_migration(&old), &old.journal)
        } else {
            (
                resume_backup_only_migration(&old, legacy_catalog_dir),
                &old.backup,
            )
        };
        match recovered {
            Ok(at) => published_at = at,
            Err(reason) => {
                // Blocked: the genuinely in-flight case. Rename nothing; the
                // next pass retries, so restoring the backup heals it.
                let _ = naming_migration::update_journal(journal_dir, |j| {
                    j.note(&key, &format!("deferred: {reason}"));
                    j.set_status(&key, naming_migration::ScopeStatus::Deferred);
                });
                return Err(catalog_diagnostic(
                    REPORT_CODE_MIGRATION_CONFLICT,
                    sidecar,
                    reason,
                ));
            }
        }
    }

    let mut base_moved = false;
    for (index, rename) in CATALOG_FAMILY_RENAMES.iter().enumerate() {
        match naming_migration::rename_step(dir, rename, &key, journal_dir, held) {
            naming_migration::Outcome::Refused(refusal) => {
                return Err(catalog_diagnostic(
                    REPORT_CODE_REFRESH_FAILED,
                    &dir.join(rename.from),
                    format!(
                        "the naming migration could not move {} to {}: {}",
                        rename.from,
                        rename.to,
                        naming_refusal_reason(refusal)
                    ),
                ))
            }
            naming_migration::Outcome::Renamed | naming_migration::Outcome::SetAside(_) => {
                base_moved |= index == 0;
            }
            naming_migration::Outcome::AlreadyDone | naming_migration::Outcome::SourceAbsent => {}
        }
    }

    // A lock sidecar and the two immutable #1968 sidecars are never renamed
    // or deleted, only noted.
    let mut notes = Vec::new();
    for (name, why) in [
        (
            RETIRED_CATALOG_LOCK_FILENAME,
            "a lock sidecar is never renamed or deleted",
        ),
        (
            MIGRATION_JOURNAL_FILENAME,
            "an immutable #1968 migration sidecar is never renamed or deleted",
        ),
        (
            MIGRATION_BACKUP_FILENAME,
            "an immutable #1968 migration sidecar is never renamed or deleted",
        ),
    ] {
        if matches!(catalog_path_present(&dir.join(name)), Ok(true)) {
            notes.push(format!("left on disk: {name} ({why})"));
        }
    }
    naming_migration::update_journal(journal_dir, |j| {
        for note in &notes {
            j.note(&key, note);
        }
        j.set_status(&key, naming_migration::ScopeStatus::Complete);
    })
    .map_err(journal_error)?;

    if base_moved && scope == InitScope::Project {
        log::warn!(
            "[coding-agents] renamed {} to {} in {}; Git shows the tracked {} as deleted and {} as \
                new (a delete plus an add), and AC runs no Git",
            CATALOG_FAMILY_RENAMES[0].from,
            CATALOG_FAMILY_RENAMES[0].to,
            dir.display(),
            CATALOG_FAMILY_RENAMES[0].from,
            CATALOG_FAMILY_RENAMES[0].to,
        );
    }
    Ok(published_at)
}

/// #2715 5.4 - the legacy instance catalog is its own scope, migrated before
/// any project lock is taken, so a new project imports the real instance
/// catalog. It holds no caller lock, so it takes both sidecars itself. A
/// directory that does not exist holds nothing to rename and is not created.
pub(crate) fn migrate_legacy_catalog_scope(
    legacy_catalog_dir: &Path,
    journal_dir: Option<&Path>,
) -> Result<(), CatalogDiagnostic> {
    let conflict = |reason: String| {
        catalog_diagnostic(
            REPORT_CODE_MIGRATION_CONFLICT,
            legacy_catalog_dir,
            format!(
                "the instance catalog {} could not be moved to {}: {reason}",
                legacy_catalog_dir
                    .join(CATALOG_FAMILY_RENAMES[0].from)
                    .display(),
                legacy_catalog_dir
                    .join(CATALOG_FAMILY_RENAMES[0].to)
                    .display()
            ),
        )
    };
    let dir = match catalog::canonical_catalog_path(legacy_catalog_dir) {
        Ok(dir) => dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(conflict(error.to_string())),
    };
    let key = catalog_scope_key(&dir);
    // Settled already: skip without contending for the instance lock.
    let journal = naming_migration::read_journal(journal_dir)
        .map_err(|refusal| conflict(naming_refusal_reason(refusal)))?;
    if naming_migration::scope_is_settled(journal.as_ref(), &key) {
        return Ok(());
    }
    let held = naming_migration::lock_scope(
        &dir,
        CATALOG_LOCK_FILENAME,
        Some(RETIRED_CATALOG_LOCK_FILENAME),
        naming_migration::MIGRATION_LOCK_BUDGET,
    )
    .map_err(|r| conflict(naming_refusal_reason(r)))?;
    migrate_catalog_family(&dir, None, journal_dir, InitScope::Instance, &held)
        .map(|_| ())
        .map_err(|diagnostic| conflict(diagnostic.reason))
}

/// A fail-soft outcome carrying one diagnostic: nothing published, nothing
/// verified, and `initialize_catalog_under_lock` not reached.
pub(crate) fn fail_soft_outcome(diagnostic: CatalogDiagnostic) -> CatalogInitOutcome {
    log::warn!(
        "[coding-agents] {} at {}: {}",
        diagnostic.code,
        diagnostic.path,
        diagnostic.reason
    );
    CatalogInitOutcome {
        published_at: None,
        base_verified_managed: false,
        warnings: vec![diagnostic],
    }
}

/// Initialize the catalog for `ac_dir` under the catalog lock: recover or
/// refresh a managed base, migrate a legacy catalog, or fresh-seed the managed
/// defaults plus the create-once local stub. Returns the `Utc::now()`
/// publication time sampled at the commit point when the managed base was
/// actually written or replaced; `None` means no base publication. Fail-soft:
/// every failure is logged and surfaced as a warning, never a panic.
#[cfg(test)]
pub fn ensure_seeded(ac_dir: &Path, legacy_catalog_dir: Option<&Path>) -> Option<DateTime<Utc>> {
    run_catalog_initialization(ac_dir, legacy_catalog_dir, None).published_at
}

/// Token-injectable twin of [`ensure_seeded_for_project`], mirroring
/// `perform_config_seed_recorded` (`config_seed.rs`): a `None` activation runs
/// the plain ungated initialization under the catalog lock; under the soft
/// project gate the Held arm runs the catalog initialization and the masters,
/// then records the catalog row when a base was published - or records the
/// VERIFIED CURRENT base when the manifest has no catalog row yet (the retry
/// that repairs a failed recording without republishing anything).
/// `DegradedUntracked` still takes the catalog lock but records nothing;
/// `Unavailable` logs and skips every catalog mutation (never race a
/// cooperating writer).
pub(crate) fn ensure_seeded_for_project_with_token(
    project_root: &Path,
    activation: Option<&ManifestActivationToken>,
) {
    // #2715: the one place this seeding chain reads `config_dir()`; it names
    // both the legacy instance catalog and the naming-migration journal.
    let config_dir = crate::config::config_dir();
    let legacy = config_dir.as_ref().map(|dir| dir.join(CATALOG_DIR_NAME));
    ensure_seeded_for_project_in(
        project_root,
        activation,
        legacy.as_deref(),
        config_dir.as_deref(),
    );
}

/// [`ensure_seeded_for_project_with_token`] with the legacy catalog and the
/// naming-migration journal directory injected (#2715).
pub(crate) fn ensure_seeded_for_project_in(
    project_root: &Path,
    activation: Option<&ManifestActivationToken>,
    legacy: Option<&Path>,
    journal_dir: Option<&Path>,
) {
    if !project_root.is_absolute() {
        log::warn!(
            "[coding-agents] skipping seed for non-absolute registered project root {}",
            project_root.display()
        );
        return;
    }
    if !crate::filesystem::directories::registered_project_is_dir(project_root) {
        log::warn!(
            "[coding-agents] skipping seed for missing registered project root {}",
            project_root.display()
        );
        return;
    }
    let ac_dir = project_root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);

    let Some(token) = activation else {
        run_catalog_initialization(&ac_dir, legacy, journal_dir);
        ensure_seeded_masters(&ac_dir, legacy);
        return;
    };

    match acquire_project_gate_soft(project_root) {
        SoftProjectGate::Held(mut guard) => {
            let outcome = run_catalog_initialization(&ac_dir, legacy, journal_dir);
            ensure_seeded_masters(&ac_dir, legacy);
            let recorded_at = match outcome.published_at {
                Some(published_at) => Some(published_at),
                None if outcome.base_verified_managed => {
                    match has_catalog_publication(project_root) {
                        Ok(true) => None,
                        Ok(false) => Some(Utc::now()),
                        Err(error) => {
                            log::debug!(
                                "[coding-agents] seed-manifest bookkeeping query failed for {} \
                                    ({error}); recording is retried on the next initialization",
                                project_root.display()
                            );
                            Some(Utc::now())
                        }
                    }
                }
                None => None,
            };
            if let Some(recorded_at) = recorded_at {
                record_catalog_publication(&mut guard, token, recorded_at);
            }
            guard.release();
        }
        SoftProjectGate::DegradedUntracked => {
            run_catalog_initialization(&ac_dir, legacy, journal_dir);
            ensure_seeded_masters(&ac_dir, legacy);
        }
        SoftProjectGate::Unavailable(error) => {
            log::warn!(
                "[coding-agents] project gate unavailable for {}: {}; skipping seed to avoid \
                    racing a cooperating writer",
                project_root.display(),
                error
            );
        }
    }
}

/// Record a catalog publication into the project seed manifest under an
/// already-held gate, mirroring `session_context::record_project_context_publication`
/// step for step. `published_at` is the `Utc::now()` sampled inside
/// `ensure_seeded` at the commit point of the atomic write; the recorder never
/// re-samples a later clock. The row records the WRITE, not content validity.
/// Fail-soft (log-only) on every error path; never blocks or retracts the seed.
pub(crate) fn record_catalog_publication(
    guard: &mut ProjectSeedManifestGuard,
    activation: &ManifestActivationToken,
    published_at: DateTime<Utc>,
) {
    let identity = match ManifestPathIdentity::from_relative_path(Path::new(
        ".ac/coding-agents/agents.json",
    )) {
        Ok(identity) => identity,
        Err(error) => {
            log::warn!(
                "[coding-agents] seed-manifest catalog row rejected path error={}",
                error
            );
            return;
        }
    };
    let row = match PublishedManifestRow::coding_agent_catalog(identity, published_at) {
        Ok(row) => row,
        Err(error) => {
            log::warn!(
                "[coding-agents] seed-manifest catalog row rejected error={}",
                error
            );
            return;
        }
    };
    let outcome = guard.publication_permit().record_file(activation, row);
    log::debug!(
        "[coding-agents] seed-manifest catalog publication outcome={:?}",
        outcome
    );
}
