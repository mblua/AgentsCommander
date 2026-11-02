use crate::filesystem::config_seed as seed_disk;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::config::seed_manifest::PinnedDirectory;

use super::copy::{
    clear_stale_seed_scratch, config_scope_for_seed, copy_tree_collecting, is_nonempty_seed_dir,
    is_readable_dir, remove_seed_scratch, SeedFileCollector,
};
use super::{
    os_marker, ConfigSeedCandidate, ConfigSeedFailure, ConfigSeedPublication, ConfigSeedReport,
    ConfigSeedRollbackFailure, ConfigSeedSkipReason, ConfigSeedTier, ResolvedConfigSeed,
};

/// Best-effort, Windows-safe seed execution (H1). NEVER aborts the spawn.
/// `unique_sfx` (the session id) keeps the temp/trash leaves unique per spawn
/// (M3) and short (I2/MAX_PATH).
///
/// CONCURRENCY CONTRACT (grinch HIGH-1): callers MUST serialize this for the
/// same replica, because step 2 sweeps ALL `<dest>.acseed-*` scratch by prefix
/// (see [`clear_stale_seed_scratch`]) and would otherwise delete a concurrent
/// spawn's in-flight temp/trash mid-swap. The only caller, the session spawn
/// chokepoint, holds `ConfigSeedLockState` across this call for all dests.
pub(crate) fn perform_config_seed(seed: &ResolvedConfigSeed, unique_sfx: &str) -> ConfigSeedReport {
    perform_config_seed_with_clock(seed, unique_sfx, &Utc::now)
}

/// Run the config seed under the per-project seed-manifest gate.
///
/// The caller (session-create chokepoint) already holds the process-local
/// `ConfigSeedLockState`; this acquires the project kernel gate second, holds it
/// across the install and drops it before
/// returning. Ordering by gate state:
/// * held: install under the gate.
/// * pre-contention degradation: install untracked (no manifest write, no race).
/// * contention/unsafe: skip the seed so the install never races a cooperating
///   writer; config seed stays fail-soft, so this never aborts the launch.
///
/// `activation` is `None` for a `#[cfg(test)]` lib build or an unactivated caller,
/// which runs the plain ungated seed. A launch root with no project workspace
/// (e.g. Root Agent) is not recorded and runs ungated (plan section 5.3).
pub(crate) fn perform_config_seed_recorded(
    seed: &ResolvedConfigSeed,
    unique_sfx: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> ConfigSeedReport {
    perform_config_seed_recorded_with(seed, activation, || perform_config_seed(seed, unique_sfx))
}

pub(super) fn perform_config_seed_recorded_with(
    seed: &ResolvedConfigSeed,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    perform_seed: impl FnOnce() -> ConfigSeedReport,
) -> ConfigSeedReport {
    use crate::config::seed_manifest::SoftProjectGate;
    let Some(_activation) = activation else {
        return perform_seed();
    };
    let Some(project_root) = seed
        .context
        .ac_root
        .as_ref()
        .and_then(|ac_root| ac_root.parent())
    else {
        return perform_seed();
    };
    match crate::config::seed_manifest::acquire_project_gate_soft(project_root) {
        SoftProjectGate::Held(guard) => {
            let report = perform_seed();
            guard.release();
            report
        }
        SoftProjectGate::DegradedUntracked => perform_seed(),
        SoftProjectGate::Unavailable(error) => {
            log::warn!(
                "[config-seed] project gate unavailable for {}: {}; skipping seed to avoid racing \
                a cooperating writer",
                seed.dest.display(),
                error
            );
            ConfigSeedReport::Skipped(ConfigSeedSkipReason::GateUnavailable {
                reason: error.to_string(),
            })
        }
    }
}

#[derive(Default)]
pub(super) struct ConfigSeedTestHooks {
    #[cfg(test)]
    pub(super) fail_install: bool,
    #[cfg(test)]
    pub(super) fail_restore: bool,
    #[cfg(test)]
    pub(super) destination_appears_before_restore: bool,
    #[cfg(test)]
    pub(super) fail_staged_metadata: bool,
}

impl ConfigSeedTestHooks {
    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "self controla fallo inyectado cfg(test); preservar hook y orden de \
                publicación."
        )
    )]
    fn install(&self, staging: &Path, dest: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        if self.fail_install {
            return Err(std::io::Error::other(
                "injected config-seed install failure",
            ));
        }
        seed_disk::rename_seed_tree(staging, dest)
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "self controla creación ajena inyectada cfg(test); conservar seam."
        )
    )]
    fn before_restore(&self, dest: &Path) {
        #[cfg(test)]
        if self.destination_appears_before_restore {
            seed_disk::ensure_seed_directory(dest).expect("inject replacement destination");
            seed_disk::write_seed_file(dest.join("foreign.txt"), b"FOREIGN")
                .expect("populate replacement destination");
        }
        #[cfg(not(test))]
        let _ = dest;
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "self controla fallo inyectado cfg(test); preservar recuperación."
        )
    )]
    fn restore(&self, trash: &Path, dest: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        if self.fail_restore {
            return Err(std::io::Error::other(
                "injected config-seed restore failure",
            ));
        }
        seed_disk::rename_seed_tree(trash, dest)
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "self controla metadata fallida en cfg(test); conservar diagnóstico \
                normal y seam."
        )
    )]
    fn staged_diagnostic(&self, staged: &PinnedDirectory, trash: &Path) -> String {
        #[cfg(test)]
        if self.fail_staged_metadata {
            return "staged-tree metadata inspection failed: injected metadata failure".to_string();
        }
        match staged.revalidate_at(trash) {
            Ok(()) => format!(
                "operation-owned previous config remains staged at {}",
                trash.display()
            ),
            Err(error) => format!(
                "staged-tree metadata inspection failed at {}: {}",
                trash.display(),
                error
            ),
        }
    }
}

pub(super) fn perform_config_seed_with_clock(
    seed: &ResolvedConfigSeed,
    unique_sfx: &str,
    clock: &dyn Fn() -> DateTime<Utc>,
) -> ConfigSeedReport {
    perform_config_seed_with_clock_and_hooks(
        seed,
        unique_sfx,
        clock,
        &ConfigSeedTestHooks::default(),
    )
}

pub(super) fn perform_config_seed_with_clock_and_hooks(
    seed: &ResolvedConfigSeed,
    unique_sfx: &str,
    clock: &dyn Fn() -> DateTime<Utc>,
    hooks: &ConfigSeedTestHooks,
) -> ConfigSeedReport {
    if let Err(reason) = revalidate_seed_owner(seed) {
        log::warn!(
            "[config-seed] stale replica owner {}: {}; skipping",
            seed.context.replica_root.display(),
            reason
        );
        return ConfigSeedReport::Skipped(ConfigSeedSkipReason::StaleReplica {
            replica_root: seed.context.replica_root.clone(),
            reason,
        });
    }

    let candidate = match select_seed_candidate(seed) {
        Ok(candidate) => candidate,
        Err(report) => return report,
    };

    let (Some(parent), Some(dest_name)) = (
        seed.dest.parent(),
        seed.dest.file_name().and_then(|s| s.to_str()),
    ) else {
        log::warn!(
            "[config-seed] dest {} has no parent/name; skipping",
            seed.dest.display()
        );
        return ConfigSeedReport::Skipped(ConfigSeedSkipReason::InvalidDestination {
            dest: seed.dest.clone(),
            reason: "destination has no parent or UTF-8 file name".to_string(),
        });
    };

    // M3: unique per spawn. Same-volume siblings of dest, so renames are atomic.
    let temp = parent.join(format!("{}.acseed-tmp-{}", dest_name, unique_sfx));
    let trash = parent.join(format!("{}.acseed-old-{}", dest_name, unique_sfx));

    // 2. Reclaim ALL stale seed scratch for this dest (any session id), not just
    //    this spawn's. A trash dir whose removal failed earlier (locked by a
    //    lingering handle) carries a DIFFERENT session id, so a per-suffix
    //    cleanup would leak it forever; sweep by prefix so it is reclaimed on the
    //    first run after the lock releases.
    clear_stale_seed_scratch(parent, dest_name);

    // 3. Stage the template into temp. On any error the dest is untouched.
    //    master->replica copy substitutes the 3 AC tokens (substitute = true).
    let tier = candidate.tier;
    let src = &candidate.path;
    let mut collector = SeedFileCollector::new(seed, tier);
    if let Err(e) = copy_tree_collecting(src, &temp, Some(&seed.context), true, &mut collector) {
        log::warn!(
            "[config-seed] failed to stage template {} -> {}: {}",
            src.display(),
            temp.display(),
            e
        );
        remove_seed_scratch(&temp, "failed staging cleanup", unique_sfx);
        return ConfigSeedReport::Failed(ConfigSeedFailure::Staging {
            source: src.clone(),
            staging_path: temp,
            error: e,
        });
    }
    let files = collector.finish();

    if let Err(report) = install_seed_or_restore(seed, unique_sfx, dest_name, &temp, &trash, hooks)
    {
        return report;
    }

    // The successful directory-install syscall is the publication boundary.
    let published_at = clock();

    // 6. Drop the old config (ignore if locked; it lingers, cleared next run).
    remove_seed_scratch(&trash, "published old-destination cleanup", unique_sfx);

    // 7. Emit the deferred config-dir warning + the success line.
    if let Some(w) = &seed.config_dir_warning {
        log::warn!("[config-seed] {}", w);
    }
    log::info!(
        "[config-seed] seeded '{}' into replica from {:?}{} source '{}'",
        seed.dest.display(),
        candidate.tier,
        os_marker(candidate.os_specific),
        src.display()
    );
    // #1596: pi's bash tool runs through `shellPath`; project `.pi/settings.json`
    // overrides the global one, so an absent-only seed routes pi through Git Bash
    // on Windows. Best-effort: failure logs and never changes the seed outcome.
    #[cfg(windows)]
    if dest_name == ".pi" {
        if let Err(error) = ensure_pi_bash_shell_path(&seed.dest) {
            log::warn!(
                "[config-seed] could not seed pi shellPath at {}: {}",
                seed.dest.display(),
                error
            );
        }
    }
    ConfigSeedReport::Published(ConfigSeedPublication {
        tier,
        dest: seed.dest.clone(),
        files,
        published_at,
    })
}

fn install_seed_or_restore(
    seed: &ResolvedConfigSeed,
    unique_sfx: &str,
    dest_name: &str,
    temp: &Path,
    trash: &Path,
    hooks: &ConfigSeedTestHooks,
) -> Result<(), ConfigSeedReport> {
    // 4. Move the existing dest aside (atomic). If it is locked by a live
    //    process, keep the existing config and SKIP (dest stays fully intact).
    let previous_scope_staged = match pin_seed_destination_if_present(&seed.dest) {
        Ok(None) => None,
        Ok(Some(pinned)) => {
            if let Err(e) = seed_disk::rename_seed_tree(&seed.dest, trash) {
                log::warn!(
                    "[config-seed] dest {} is in use ({}); keeping existing config, skipping this \
                    seed",
                    seed.dest.display(),
                    e
                );
                remove_seed_scratch(temp, "destination-in-use cleanup", unique_sfx);
                return Err(ConfigSeedReport::Skipped(
                    ConfigSeedSkipReason::DestinationInUse {
                        dest: seed.dest.clone(),
                        error: e.to_string(),
                    },
                ));
            }
            Some(pinned)
        }
        Err(error) => {
            log::warn!(
                "[config-seed] cannot inspect destination {} without following links: {}; keeping \
                existing config, skipping this seed",
                seed.dest.display(),
                error
            );
            remove_seed_scratch(temp, "destination-inspection cleanup", unique_sfx);
            return Err(ConfigSeedReport::Skipped(
                ConfigSeedSkipReason::DestinationInUse {
                    dest: seed.dest.clone(),
                    error,
                },
            ));
        }
    };

    // 5. Install the freshly staged temp (atomic).
    if let Err(e) = hooks.install(temp, &seed.dest) {
        let install_error = e.to_string();
        log::warn!(
            "[config-seed] failed to install seed at {}: {}",
            seed.dest.display(),
            e
        );
        // A successful dest-to-trash rename is operation-owned proof that the
        // previous logical scope was removed. Always attempt its inverse after
        // an install failure. A later destination appearance or metadata error
        // must not collapse an un-restored old tree into ordinary `Failed`.
        if let Some(staged) = previous_scope_staged.as_ref() {
            hooks.before_restore(&seed.dest);
            if let Err(re) = hooks.restore(trash, &seed.dest) {
                let staged_diagnostic = hooks.staged_diagnostic(staged, trash);
                let restore_error = re.to_string();
                log::warn!(
                    "[config-seed] failed to restore previous config at {}: {}; {}",
                    seed.dest.display(),
                    restore_error,
                    staged_diagnostic
                );
                remove_seed_scratch(temp, "failed install cleanup", unique_sfx);
                return Err(ConfigSeedReport::FailedAfterLogicalRemoval(
                    ConfigSeedRollbackFailure::PreviousScopeStillStaged {
                        scope: config_scope_for_seed(seed).unwrap_or_else(|reason| {
                            log::warn!(
                                "[config-seed] failed to derive config scope for rollback at {}: \
                                {}",
                                seed.dest.display(),
                                reason
                            );
                            format!("config:{}", dest_name)
                        }),
                        trash_path: trash.to_path_buf(),
                        install_error,
                        restore_error: format!("{restore_error}; {staged_diagnostic}"),
                    },
                ));
            }
        }
        remove_seed_scratch(temp, "failed install cleanup", unique_sfx);
        return Err(ConfigSeedReport::Failed(ConfigSeedFailure::Install {
            staging_path: temp.to_path_buf(),
            dest: seed.dest.clone(),
            error: install_error,
        }));
    }

    Ok(())
}

fn select_seed_candidate(
    seed: &ResolvedConfigSeed,
) -> Result<&ConfigSeedCandidate, ConfigSeedReport> {
    // 1. Pick the winning tier by source-folder presence (highest precedence first).
    //    The four legacy tiers are unchanged (readable dir wins and overwrites).
    //    The #769 P2 CatalogDefault tier is gated ABSENT-ONLY + NON-EMPTY here, so
    //    it never overwrites an existing replica config and an empty/absent master
    //    reads as "not present".
    let mut selected = seed
        .candidates
        .iter()
        .find(|c| c.tier != ConfigSeedTier::CatalogDefault && is_readable_dir(&c.path));
    if selected.is_none() {
        if let Some(candidate) = seed
            .candidates
            .iter()
            .find(|c| c.tier == ConfigSeedTier::CatalogDefault && is_nonempty_seed_dir(&c.path))
        {
            match destination_absent_no_follow(&seed.dest) {
                Ok(true) => selected = Some(candidate),
                Ok(false) => {}
                Err(error) => {
                    log::warn!(
                        "[config-seed] cannot inspect catalog-default destination {} without \
                        following links: {}; skipping",
                        seed.dest.display(),
                        error
                    );
                    return Err(ConfigSeedReport::Skipped(
                        ConfigSeedSkipReason::DestinationInUse {
                            dest: seed.dest.clone(),
                            error,
                        },
                    ));
                }
            }
        }
    }
    let Some(candidate) = selected else {
        // #598: seeding is active but no template exists at any tier. This is a
        // benign no-op (nothing to copy), but a SILENT one confused users who
        // enabled the feature, created no template, and saw nothing happen. Log
        // it at info with the exact candidate paths so they know WHERE to create
        // a template. Only fires here, at the real spawn chokepoint (callers gate
        // on a resolved seed), never during prevalidation/delivery builds.
        let replica_label = seed
            .context
            .replica_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?");
        let looked_in = seed
            .candidates
            .iter()
            .map(|c| {
                format!(
                    "{:?}{}={}",
                    c.tier,
                    os_marker(c.os_specific),
                    c.path.display()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        log::info!(
            "[config-seed] active for replica '{}' dest '{}' but no source template found at any \
            tier; skipped (looked for: {})",
            replica_label,
            seed.dest.display(),
            looked_in
        );
        return Err(ConfigSeedReport::Skipped(ConfigSeedSkipReason::NoSource));
    };

    Ok(candidate)
}

pub(super) fn destination_absent_no_follow(path: &Path) -> Result<bool, String> {
    match seed_disk::inspect_seed_destination(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!("inspect destination metadata: {error}")),
    }
}

/// #1596: absent-only, best-effort seed of pi's project-level `shellPath` to
/// Git Bash on Windows. pi merges project settings over global settings, so a
/// project `.pi/settings.json` routes the pi bash tool through bash.exe even
/// when the global settings name pwsh. An existing `settings.json` (operator-
/// authored or template-shipped) is never overwritten: the no-follow presence
/// check mirrors `destination_absent_no_follow` and the `create_new` open makes
/// the write race-safe. A write failure logs and never changes the seed
/// outcome. Windows-only: the shellPath value is a Windows path.
#[cfg(target_os = "windows")]
pub(super) fn ensure_pi_bash_shell_path(pi_dir: &Path) -> std::io::Result<()> {
    const PI_SETTINGS_JSON: &[u8] = b"{\"shellPath\": \"C:/Program Files/Git/bin/bash.exe\"}";
    let settings_path = pi_dir.join("settings.json");
    match seed_disk::inspect_seed_destination(&settings_path) {
        Ok(_) => return Ok(()), // operator-authored or template-shipped: untouched
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match seed_disk::create_pi_seed_settings(&settings_path) {
        Ok(mut file) => {
            use std::io::Write as _;
            file.write_all(PI_SETTINGS_JSON)
        }
        // A racing writer created the file between the check and the open:
        // absent-only semantics hold, treat it as untouched.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn pin_seed_destination_if_present(
    path: &Path,
) -> Result<Option<PinnedDirectory>, String> {
    let metadata = match seed_disk::inspect_seed_destination(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("inspect destination metadata: {error}")),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata_is_reparse(&metadata) {
        return Err("destination is not an ordinary non-reparse directory".to_string());
    }
    let pinned = PinnedDirectory::open(path)
        .map_err(|error| format!("pin destination directory identity: {error}"))?;
    pinned
        .revalidate()
        .map_err(|error| format!("revalidate destination directory identity: {error}"))?;
    Ok(Some(pinned))
}

/// True iff `path` is a readable directory. Follows symlinks on the ROOT (the
/// template folder itself may be a real or symlinked dir); only ENTRIES are
/// filtered for reparse points during the copy (see [`is_symlink_or_reparse`]).
pub(super) fn revalidate_seed_owner(seed: &ResolvedConfigSeed) -> Result<(), String> {
    let replica_root = &seed.context.replica_root;
    let metadata = seed_disk::inspect_seed_destination(replica_root)
        .map_err(|error| format!("inspect replica root: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata_is_reparse(&metadata) {
        return Err("replica root is not an existing ordinary directory".to_string());
    }
    let parent = seed
        .dest
        .parent()
        .ok_or_else(|| "destination has no parent".to_string())?;
    if !parent.starts_with(replica_root) {
        return Err(format!(
            "destination parent {} is outside replica root",
            parent.display()
        ));
    }
    let parent_metadata = seed_disk::inspect_seed_destination(parent)
        .map_err(|error| format!("inspect destination parent: {error}"))?;
    if !parent_metadata.is_dir()
        || parent_metadata.file_type().is_symlink()
        || metadata_is_reparse(&parent_metadata)
    {
        return Err("destination parent is not an existing ordinary directory".to_string());
    }
    if let Ok(dest_metadata) = seed_disk::inspect_seed_destination(&seed.dest) {
        if dest_metadata.file_type().is_symlink() || metadata_is_reparse(&dest_metadata) {
            return Err("destination is a symlink or reparse point".to_string());
        }
    }
    Ok(())
}

use crate::filesystem::config_seed::seed_metadata_is_reparse as metadata_is_reparse;
