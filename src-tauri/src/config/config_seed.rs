//! #598 - seed a template config folder (e.g. `.claude`) into a coding-agent
//! replica at spawn time.
//!
//! Two phases:
//! - [`resolve_config_seed`] is PURE path math (no filesystem) and fail-soft:
//!   it never returns an error that aborts a launch (M7).
//! - [`perform_config_seed`] is the side-effecting, best-effort, Windows-safe
//!   rename-to-trash-first atomic swap (H1): the destination is always either
//!   fully-old or fully-new, never half-written. It never aborts the spawn.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::config::placeholders::{
    expand_placeholders_in_content, PlaceholderContext, PlaceholderRootKind,
};
use crate::config::seed_manifest::{
    config_batch_base_serialized_len, exact_config_row_serialized_len, ManifestSource,
    PinnedDirectory, ResourceBoundKind, SeedManifestError, MAX_FIELD_BYTES, MAX_MANIFEST_BYTES,
    MAX_MANIFEST_ROWS,
};
use crate::config::settings::{validate_config_seed_dest, ConfigSeedConfig};
use crate::session::profile::CodingAgentKind;

/// Files at or below this size are read so the 3 AC tokens can be substituted in
/// their CONTENT. Larger files are streamed via `std::fs::copy` (M4), never read
/// whole into memory.
const CONTENT_SUBSTITUTION_CAP: u64 = 5 * 1024 * 1024; // 5 MiB

/// Recursion-depth bound for the template tree (M6). Defends against a
/// pathological or cyclic-via-junction layout; junction/symlink entries are
/// already skipped, so this is a belt-and-braces safety net.
const MAX_SEED_DEPTH: u32 = 64;

/// Source tiers, highest precedence first. All matrix tiers outrank all
/// workspace tiers; within a location the profile-lettered folder outranks the
/// base folder. The matrix's bare `<dest>` (e.g. `_agent_<role>/.claude`) is
/// deliberately NOT a tier: that is the agent's OWN live config (the matrix dir
/// is itself a launch root), not a template, so seeding from it would copy real
/// config and clobber it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSeedTier {
    MatrixProfile,
    MatrixBase,
    WorkspaceProfile,
    WorkspaceBase,
    /// #769 Phase 2 - the shipped default config-folder master at
    /// `<config_dir>/coding-agents/_seed/<dest>/`. Appended LAST (lowest
    /// precedence) by the spawn caller. Unlike the four tiers above, this one is
    /// ABSENT-ONLY and NON-EMPTY-GATED in `perform_config_seed`: it fills
    /// `%AC_REPLICA_ROOT%/<dest>` only when the dest does not already exist AND the
    /// master has at least one regular file, so it can never overwrite accumulated
    /// replica config, and an empty/absent master reads as "tier not present"
    /// (today's spawn behavior stays byte-identical).
    CatalogDefault,
}

/// #2162 - one candidate source folder: its tier, whether it carries the
/// `.<os>` suffix, and the composed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSeedCandidate {
    pub tier: ConfigSeedTier,
    /// True when this path carries the `.<os>` suffix.
    pub os_specific: bool,
    pub path: PathBuf,
}

/// #2162 - compile-time host OS token. `None` on any other target, which then
/// contributes no OS candidates. Not user-overridable.
pub fn host_os_token() -> Option<&'static str> {
    if cfg!(target_os = "linux") {
        Some("linux")
    } else if cfg!(target_os = "windows") {
        Some("windows")
    } else if cfg!(target_os = "macos") {
        Some("macos")
    } else {
        None
    }
}

/// #2162 - marker appended to the tier name in logs when an OS variant won.
pub(crate) fn os_marker(os_specific: bool) -> &'static str {
    if os_specific {
        "+os"
    } else {
        ""
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedConfigSeed {
    /// Candidate source folders, highest precedence first: matrix profile,
    /// matrix base, workspace profile, workspace base. #2162: each tier is
    /// preceded by its `.<os>` variant when an OS token is supplied. Pure path
    /// math; no filesystem touched at resolve time.
    pub candidates: Vec<ConfigSeedCandidate>,
    /// Absolute destination, under the replica root.
    pub dest: PathBuf,
    /// Carried for CONTENT substitution at copy time.
    pub context: PlaceholderContext,
    /// Heuristic, log-only warning about a dest vs config-dir-env mismatch
    /// (computed at build time; emitted once at execution).
    pub config_dir_warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigSeedPublication {
    pub tier: ConfigSeedTier,
    pub dest: PathBuf,
    pub files: CollectedSeedFiles,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CollectedSeedFiles {
    Exact(Vec<PathBuf>),
    OverBound {
        reason: ResourceBoundKind,
        observed_at_least: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigSeedSkipReason {
    NoSource,
    InvalidDestination {
        dest: PathBuf,
        reason: String,
    },
    StaleReplica {
        replica_root: PathBuf,
        reason: String,
    },
    DestinationInUse {
        dest: PathBuf,
        error: String,
    },
    /// #1065 Stage F: the per-project seed-manifest gate timed out behind a
    /// cooperating writer (or an unsafe/reparse project path failed closed), so
    /// seeding is skipped rather than racing that writer (plan section 6.5).
    GateUnavailable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigSeedFailure {
    Staging {
        source: PathBuf,
        staging_path: PathBuf,
        error: String,
    },
    Install {
        staging_path: PathBuf,
        dest: PathBuf,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigSeedRollbackFailure {
    PreviousScopeStillStaged {
        scope: String,
        trash_path: PathBuf,
        install_error: String,
        restore_error: String,
    },
}

/// Outcome of [`perform_config_seed`]. The publication boundary carries the
/// exact winning tier, staged regular files, destination, and commit-point time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigSeedReport {
    Published(ConfigSeedPublication),
    Skipped(ConfigSeedSkipReason),
    Failed(ConfigSeedFailure),
    FailedAfterLogicalRemoval(ConfigSeedRollbackFailure),
}

/// Resolve the seed candidates + destination. Returns `None` (and warns/debug-
/// logs) on any problem; NEVER returns an error that aborts the launch (M7).
pub fn resolve_config_seed(
    cfg: &ConfigSeedConfig,
    effective_letter: &str,
    context: Option<&PlaceholderContext>,
    os_token: Option<&str>,
) -> Option<ResolvedConfigSeed> {
    let context = context?;
    if context.root_kind != PlaceholderRootKind::AcReplicaOrRootAgent {
        log::debug!("[config-seed] launch root is not an AC replica/root-agent; skipping seed");
        return None;
    }
    let dest_name = cfg.dest.trim();
    if let Err(e) = validate_config_seed_dest(dest_name) {
        // M7: a settings.json that bypassed save-time validation can still carry
        // a bad dest. Fail soft rather than abort the launch.
        log::warn!(
            "[config-seed] invalid dest '{}', skipping seed: {}",
            cfg.dest,
            e
        );
        return None;
    }
    // The user's example uses a lowercase profile letter; effective_profile is UPPERCASE.
    let letter = effective_letter.to_ascii_lowercase();

    // Precedence: all matrix candidates beat all workspace candidates; within a
    // location, the profile-lettered folder beats the base folder. The matrix's
    // bare `<dest>` is intentionally never a source (it is the agent's own live
    // config; see ConfigSeedTier), so the matrix tiers mirror the workspace
    // naming (`default_profile_<l><dest>` / `default<dest>`) instead.
    //
    // #2162: within EACH tier the `.<os>` variant is pushed first and the base
    // second, so an OS folder refines its own tier and never jumps one. With
    // `os_token == None` only the four plain candidates are produced.
    let mut candidates: Vec<ConfigSeedCandidate> = Vec::new();
    let push_tier = |candidates: &mut Vec<ConfigSeedCandidate>,
                     tier: ConfigSeedTier,
                     root: &Path,
                     folder: String| {
        if let Some(os) = os_token {
            candidates.push(ConfigSeedCandidate {
                tier,
                os_specific: true,
                path: root.join(format!("{}.{}", folder, os)),
            });
        }
        candidates.push(ConfigSeedCandidate {
            tier,
            os_specific: false,
            path: root.join(folder),
        });
    };
    if let Some(mx) = context.matrix_root.as_ref() {
        push_tier(
            &mut candidates,
            ConfigSeedTier::MatrixProfile,
            mx,
            format!("default_profile_{}{}", letter, dest_name),
        );
        push_tier(
            &mut candidates,
            ConfigSeedTier::MatrixBase,
            mx,
            format!("default{}", dest_name),
        );
    }
    if let Some(ws) = context.ac_root.as_ref() {
        push_tier(
            &mut candidates,
            ConfigSeedTier::WorkspaceProfile,
            ws,
            format!("default_profile_{}{}", letter, dest_name),
        );
        push_tier(
            &mut candidates,
            ConfigSeedTier::WorkspaceBase,
            ws,
            format!("default{}", dest_name),
        );
    }
    if candidates.is_empty() {
        return None;
    }

    Some(ResolvedConfigSeed {
        candidates,
        dest: context.replica_root.join(dest_name),
        context: context.clone(),
        config_dir_warning: None,
    })
}

/// Heuristic, log-only warning when the seed dest does not line up with the
/// coding agent's configured config-dir env (so the tool would read its config
/// from elsewhere), OR when it EXACTLY matches it (L1: the seed overwrites the
/// tool's ACTIVE config dir every spawn). Never blocks; may warn spuriously if a
/// config-dir env is injected by a path this does not inspect.
///
/// Receives the real launch `shell`/`shell_args` (the agent command) so the
/// coding-agent kind is detected from the actual command.
pub fn compute_config_dir_warning(
    dest: &Path,
    shell: &str,
    shell_args: &[String],
    agent_env: &BTreeMap<String, String>,
    profile_env: &BTreeMap<String, String>,
    effective_codex_home: Option<&Path>,
) -> Option<String> {
    let key = match CodingAgentKind::detect(shell, shell_args) {
        Some(CodingAgentKind::Claude) => "CLAUDE_CONFIG_DIR",
        Some(CodingAgentKind::Codex) => "CODEX_HOME",
        // Antigravity, Pi and Muse (#1873) have no AC-managed config-dir env to
        // compare against.
        Some(CodingAgentKind::Antigravity | CodingAgentKind::Pi | CodingAgentKind::Muse) => {
            return None
        }
        None => {
            if crate::config::agent_command::command_runs_opencode(shell, shell_args) {
                "OPENCODE_CONFIG_DIR"
            } else {
                return None;
            }
        }
    };

    let dest_seg = dest.file_name().and_then(|s| s.to_str())?;

    // CODEX_HOME already went through the full resolver (isolation/profile/agent/
    // inherited); prefer its resolved value. Others: profile over agent layer.
    let configured: Option<String> = if key == "CODEX_HOME" {
        effective_codex_home
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| lookup_env_ci(profile_env, agent_env, key).cloned())
    } else {
        lookup_env_ci(profile_env, agent_env, key).cloned()
    };

    match configured {
        Some(value) => {
            let matches = final_segment(&value)
                .as_deref()
                .map(|seg| segments_eq(seg, dest_seg))
                .unwrap_or(false);
            if matches {
                // L1 case 2: seed overwrites the tool's ACTIVE config dir.
                Some(format!(
                    "config seed destination '{}' has the same final folder name as the tool's active {}='{}'; this heuristic does not inspect filesystem identity. The seeded folder is overwritten on every spawn, so any credentials or session state living there are reset each launch (ensure the template is self-sufficient)",
                    dest_seg, key, value
                ))
            } else {
                // Case 1: dest does not match the configured config-dir env.
                Some(format!(
                    "config seed destination '{}' does not match the final folder name of {}='{}'; this heuristic does not inspect filesystem identity, and the tool may read config from that path rather than the seeded folder",
                    dest_seg, key, value
                ))
            }
        }
        // Case 1 (unset but expected for this tool).
        None => Some(format!(
            "config seed destination '{}' is set but {} is not configured; the tool may ignore the seeded folder",
            dest_seg, key
        )),
    }
}

/// Case-insensitive env lookup (Windows env keys are case-insensitive, and the
/// stored key casing is user-controlled), profile layer over agent layer. The
/// plan's illustrative `.get(key)` would miss a lowercase-keyed value; this
/// mirrors `agent_command::find_env_value`.
fn lookup_env_ci<'a>(
    profile_env: &'a BTreeMap<String, String>,
    agent_env: &'a BTreeMap<String, String>,
    key: &str,
) -> Option<&'a String> {
    let want = key.to_ascii_uppercase();
    profile_env
        .iter()
        .find(|(k, _)| k.to_ascii_uppercase() == want)
        .or_else(|| {
            agent_env
                .iter()
                .find(|(k, _)| k.to_ascii_uppercase() == want)
        })
        .map(|(_, v)| v)
}

/// Final path segment of a configured value, trailing separators trimmed.
fn final_segment(value: &str) -> Option<String> {
    let trimmed = value.trim_end_matches(['/', '\\']);
    Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

fn segments_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

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

fn perform_config_seed_recorded_with(
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
                "[config-seed] project gate unavailable for {}: {}; skipping seed to avoid racing a cooperating writer",
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
struct ConfigSeedTestHooks {
    #[cfg(test)]
    fail_install: bool,
    #[cfg(test)]
    fail_restore: bool,
    #[cfg(test)]
    destination_appears_before_restore: bool,
    #[cfg(test)]
    fail_staged_metadata: bool,
}

impl ConfigSeedTestHooks {
    fn install(&self, staging: &Path, dest: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        if self.fail_install {
            return Err(std::io::Error::other(
                "injected config-seed install failure",
            ));
        }
        std::fs::rename(staging, dest)
    }

    fn before_restore(&self, dest: &Path) {
        #[cfg(test)]
        if self.destination_appears_before_restore {
            std::fs::create_dir_all(dest).expect("inject replacement destination");
            std::fs::write(dest.join("foreign.txt"), b"FOREIGN")
                .expect("populate replacement destination");
        }
        #[cfg(not(test))]
        let _ = dest;
    }

    fn restore(&self, trash: &Path, dest: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        if self.fail_restore {
            return Err(std::io::Error::other(
                "injected config-seed restore failure",
            ));
        }
        std::fs::rename(trash, dest)
    }

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

fn perform_config_seed_with_clock(
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

fn perform_config_seed_with_clock_and_hooks(
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
                        "[config-seed] cannot inspect catalog-default destination {} without following links: {}; skipping",
                        seed.dest.display(),
                        error
                    );
                    return ConfigSeedReport::Skipped(ConfigSeedSkipReason::DestinationInUse {
                        dest: seed.dest.clone(),
                        error,
                    });
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
            "[config-seed] active for replica '{}' dest '{}' but no source template found at any tier; skipped (looked for: {})",
            replica_label,
            seed.dest.display(),
            looked_in
        );
        return ConfigSeedReport::Skipped(ConfigSeedSkipReason::NoSource);
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

    // 4. Move the existing dest aside (atomic). If it is locked by a live
    //    process, keep the existing config and SKIP (dest stays fully intact).
    let previous_scope_staged = match pin_seed_destination_if_present(&seed.dest) {
        Ok(None) => None,
        Ok(Some(pinned)) => {
            if let Err(e) = std::fs::rename(&seed.dest, &trash) {
                log::warn!(
                    "[config-seed] dest {} is in use ({}); keeping existing config, skipping this seed",
                    seed.dest.display(),
                    e
                );
                remove_seed_scratch(&temp, "destination-in-use cleanup", unique_sfx);
                return ConfigSeedReport::Skipped(ConfigSeedSkipReason::DestinationInUse {
                    dest: seed.dest.clone(),
                    error: e.to_string(),
                });
            }
            Some(pinned)
        }
        Err(error) => {
            log::warn!(
                "[config-seed] cannot inspect destination {} without following links: {}; keeping existing config, skipping this seed",
                seed.dest.display(),
                error
            );
            remove_seed_scratch(&temp, "destination-inspection cleanup", unique_sfx);
            return ConfigSeedReport::Skipped(ConfigSeedSkipReason::DestinationInUse {
                dest: seed.dest.clone(),
                error,
            });
        }
    };

    // 5. Install the freshly staged temp (atomic).
    if let Err(e) = hooks.install(&temp, &seed.dest) {
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
            if let Err(re) = hooks.restore(&trash, &seed.dest) {
                let staged_diagnostic = hooks.staged_diagnostic(staged, &trash);
                let restore_error = re.to_string();
                log::warn!(
                    "[config-seed] failed to restore previous config at {}: {}; {}",
                    seed.dest.display(),
                    restore_error,
                    staged_diagnostic
                );
                remove_seed_scratch(&temp, "failed install cleanup", unique_sfx);
                return ConfigSeedReport::FailedAfterLogicalRemoval(
                    ConfigSeedRollbackFailure::PreviousScopeStillStaged {
                        scope: config_scope_for_seed(seed).unwrap_or_else(|reason| {
                            log::warn!(
                                "[config-seed] failed to derive config scope for rollback at {}: {}",
                                seed.dest.display(),
                                reason
                            );
                            format!("config:{}", dest_name)
                        }),
                        trash_path: trash,
                        install_error,
                        restore_error: format!("{restore_error}; {staged_diagnostic}"),
                    },
                );
            }
        }
        remove_seed_scratch(&temp, "failed install cleanup", unique_sfx);
        return ConfigSeedReport::Failed(ConfigSeedFailure::Install {
            staging_path: temp,
            dest: seed.dest.clone(),
            error: install_error,
        });
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

fn destination_absent_no_follow(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
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
fn ensure_pi_bash_shell_path(pi_dir: &Path) -> std::io::Result<()> {
    const PI_SETTINGS_JSON: &[u8] = b"{\"shellPath\": \"C:/Program Files/Git/bin/bash.exe\"}";
    let settings_path = pi_dir.join("settings.json");
    match std::fs::symlink_metadata(&settings_path) {
        Ok(_) => return Ok(()), // operator-authored or template-shipped: untouched
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&settings_path)
    {
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

fn pin_seed_destination_if_present(path: &Path) -> Result<Option<PinnedDirectory>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
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
fn is_readable_dir(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

fn revalidate_seed_owner(seed: &ResolvedConfigSeed) -> Result<(), String> {
    let replica_root = &seed.context.replica_root;
    let metadata = std::fs::symlink_metadata(replica_root)
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
    let parent_metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| format!("inspect destination parent: {error}"))?;
    if !parent_metadata.is_dir()
        || parent_metadata.file_type().is_symlink()
        || metadata_is_reparse(&parent_metadata)
    {
        return Err("destination parent is not an existing ordinary directory".to_string());
    }
    if let Ok(dest_metadata) = std::fs::symlink_metadata(&seed.dest) {
        if dest_metadata.file_type().is_symlink() || metadata_is_reparse(&dest_metadata) {
            return Err("destination is a symlink or reparse point".to_string());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes() & 0x0400 != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn manifest_source_for_tier(tier: ConfigSeedTier) -> ManifestSource {
    match tier {
        ConfigSeedTier::WorkspaceProfile => ManifestSource::WorkspaceProfile,
        ConfigSeedTier::WorkspaceBase => ManifestSource::WorkspaceBase,
        ConfigSeedTier::MatrixProfile => ManifestSource::MatrixProfile,
        ConfigSeedTier::MatrixBase => ManifestSource::MatrixBase,
        ConfigSeedTier::CatalogDefault => ManifestSource::CatalogDefault,
    }
}

fn project_relative_dest(seed: &ResolvedConfigSeed) -> Result<PathBuf, String> {
    let ac_root = seed
        .context
        .ac_root
        .as_ref()
        .ok_or_else(|| "launch root has no project workspace".to_string())?;
    let inside_ac_root = seed.dest.strip_prefix(ac_root).map_err(|_| {
        format!(
            "destination {} is outside workspace {}",
            seed.dest.display(),
            ac_root.display()
        )
    })?;
    // `ac_root` is the `.ac` workspace in production. Focused unit tests
    // use a synthetic leaf, so add the stable project-relative `.ac` component
    // rather than depending on that leaf's fixture name.
    let relative = PathBuf::from(".ac").join(inside_ac_root);
    if relative.is_absolute()
        || relative.components().next() != Some(Component::Normal(".ac".as_ref()))
    {
        return Err("project-relative config destination is not under .ac".to_string());
    }
    Ok(relative)
}

fn config_scope_for_seed(seed: &ResolvedConfigSeed) -> Result<String, String> {
    let relative = project_relative_dest(seed)?;
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err("config scope contains a non-normal component".to_string());
        };
        let component = component
            .to_str()
            .ok_or_else(|| "config scope is not valid UTF-8".to_string())?;
        components.push(component);
    }
    Ok(format!("config:{}", components.join("/")))
}

struct SeedFileCollector {
    files: CollectedSeedFiles,
    project_relative_dest: Option<PathBuf>,
    scope: Option<String>,
    source: ManifestSource,
    prospective_bytes: usize,
}

impl SeedFileCollector {
    fn new(seed: &ResolvedConfigSeed, tier: ConfigSeedTier) -> Self {
        let project_relative_dest = project_relative_dest(seed).ok();
        let scope = config_scope_for_seed(seed).ok();
        let files = if project_relative_dest.is_some() && scope.is_some() {
            CollectedSeedFiles::Exact(Vec::new())
        } else {
            CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::ScopeBytes,
                observed_at_least: 1,
            }
        };
        Self {
            files,
            project_relative_dest,
            scope,
            source: manifest_source_for_tier(tier),
            prospective_bytes: config_batch_base_serialized_len(),
        }
    }

    fn record(&mut self, staging_relative_path: PathBuf) {
        let CollectedSeedFiles::Exact(paths) = &self.files else {
            return;
        };
        let observed_rows = paths.len().saturating_add(1);
        if observed_rows > MAX_MANIFEST_ROWS {
            self.files = CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::Rows,
                observed_at_least: observed_rows as u64,
            };
            return;
        }
        let Some(project_relative_dest) = self.project_relative_dest.as_ref() else {
            return;
        };
        let Some(scope) = self.scope.as_deref() else {
            return;
        };
        if scope.len() > MAX_FIELD_BYTES {
            self.files = CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::ScopeBytes,
                observed_at_least: scope.len() as u64,
            };
            return;
        }
        let project_relative_path = project_relative_dest.join(&staging_relative_path);
        let row_bytes =
            match exact_config_row_serialized_len(&project_relative_path, scope, self.source) {
                Ok(bytes) => bytes,
                Err(SeedManifestError::ResourceBound {
                    kind,
                    observed_at_least,
                    ..
                }) => {
                    self.files = CollectedSeedFiles::OverBound {
                        reason: kind,
                        observed_at_least,
                    };
                    return;
                }
                Err(_) => {
                    self.files = CollectedSeedFiles::OverBound {
                        reason: ResourceBoundKind::PathBytes,
                        observed_at_least: MAX_FIELD_BYTES.saturating_add(1) as u64,
                    };
                    return;
                }
            };
        let Some(prospective_bytes) = self.prospective_bytes.checked_add(row_bytes) else {
            self.files = CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::ArithmeticOverflow,
                observed_at_least: u64::MAX,
            };
            return;
        };
        if prospective_bytes as u64 > MAX_MANIFEST_BYTES {
            self.files = CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::OutputBytes,
                observed_at_least: prospective_bytes as u64,
            };
            return;
        }
        self.prospective_bytes = prospective_bytes;
        if let CollectedSeedFiles::Exact(paths) = &mut self.files {
            paths.push(staging_relative_path);
        }
    }

    fn finish(self) -> CollectedSeedFiles {
        self.files
    }
}

fn remove_seed_scratch(path: &Path, context: &str, unique_sfx: &str) {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!(
            "[config-seed] {} failed for owned scratch {} session={}: {}",
            context,
            path.display(),
            unique_sfx,
            error
        ),
    }
}

/// #769 P2 - true iff `path` is a readable directory containing at least one
/// regular, non-reparse FILE (searched recursively). The CatalogDefault tier uses
/// this so an empty master (or a dir of only subdirs/links) reads as "not present"
/// and the tier stays inert, keeping today's spawn behavior byte-identical.
pub(crate) fn is_nonempty_seed_dir(path: &Path) -> bool {
    fn has_file(dir: &Path, depth: u32) -> bool {
        if depth > MAX_SEED_DEPTH {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            if is_symlink_or_reparse(&entry) {
                continue;
            }
            match entry.file_type() {
                Ok(ft) if ft.is_file() => return true,
                // A directory counts only if it (recursively) contains a regular
                // file; a dir with no files falls through to `_` (== "keep
                // scanning"), so an empty dir and a dir-of-empty-dirs both yield
                // false, exactly as before the collapse.
                Ok(ft) if ft.is_dir() && has_file(&entry.path(), depth + 1) => return true,
                _ => {}
            }
        }
        false
    }
    is_readable_dir(path) && has_file(path, 0)
}

/// Remove every leftover seed-scratch sibling for `dest_name` (any session-id
/// suffix), best-effort. Reclaims a temp/trash dir leaked by an earlier locked
/// removal; the current spawn's `temp`/`trash` (same prefix) are cleared too.
///
/// Sweeps across ALL session ids, so it is SAFE only when the caller has
/// serialized seeding for this replica (see [`perform_config_seed`]'s
/// concurrency contract); otherwise it can delete a concurrent spawn's in-flight
/// scratch (grinch HIGH-1).
fn clear_stale_seed_scratch(parent: &Path, dest_name: &str) {
    let tmp_prefix = format!("{}.acseed-tmp-", dest_name);
    let old_prefix = format!("{}.acseed-old-", dest_name);
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) => {
            log::warn!(
                "[config-seed] failed to inspect stale scratch siblings for {}: {}",
                parent.display(),
                error
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!(
                    "[config-seed] failed to inspect a stale scratch entry under {}: {}",
                    parent.display(),
                    error
                );
                continue;
            }
        };
        if let Some(name) = entry.file_name().to_str() {
            if name.starts_with(&tmp_prefix) || name.starts_with(&old_prefix) {
                let path = entry.path();
                if let Err(error) = std::fs::remove_dir_all(&path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        log::warn!(
                            "[config-seed] failed to reclaim stale scratch {} scope={}: {}",
                            path.display(),
                            dest_name,
                            error
                        );
                    }
                }
            }
        }
    }
}

pub(crate) fn copy_tree(
    src_dir: &Path,
    dst_dir: &Path,
    depth: u32,
    ctx: Option<&PlaceholderContext>,
    substitute: bool,
) -> Result<(), String> {
    let mut collector = None;
    copy_tree_internal(
        src_dir,
        dst_dir,
        depth,
        Path::new(""),
        ctx,
        substitute,
        &mut collector,
    )
}

fn copy_tree_collecting(
    src_dir: &Path,
    dst_dir: &Path,
    ctx: Option<&PlaceholderContext>,
    substitute: bool,
    collector: &mut SeedFileCollector,
) -> Result<(), String> {
    let mut collector = Some(collector);
    copy_tree_internal(
        src_dir,
        dst_dir,
        0,
        Path::new(""),
        ctx,
        substitute,
        &mut collector,
    )
}

fn copy_tree_internal(
    src_dir: &Path,
    dst_dir: &Path,
    depth: u32,
    relative_dir: &Path,
    ctx: Option<&PlaceholderContext>,
    substitute: bool,
    collector: &mut Option<&mut SeedFileCollector>,
) -> Result<(), String> {
    if depth > MAX_SEED_DEPTH {
        log::warn!(
            "[config-seed] template tree deeper than {} levels at {}; truncating",
            MAX_SEED_DEPTH,
            src_dir.display()
        );
        return Ok(());
    }
    std::fs::create_dir_all(dst_dir).map_err(|e| format!("create {}: {}", dst_dir.display(), e))?;
    let entries =
        std::fs::read_dir(src_dir).map_err(|e| format!("read {}: {}", src_dir.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read entry in {}: {}", src_dir.display(), e))?;
        // M6/I3: skip symlinks AND Windows junctions/reparse points (warn, not debug).
        if is_symlink_or_reparse(&entry) {
            log::warn!(
                "[config-seed] skipping symlink/reparse entry {}",
                entry.path().display()
            );
            continue;
        }
        let ft = entry
            .file_type()
            .map_err(|e| format!("file type of {}: {}", entry.path().display(), e))?;
        let dst = dst_dir.join(entry.file_name());
        let relative = relative_dir.join(entry.file_name());
        if ft.is_dir() {
            copy_tree_internal(
                &entry.path(),
                &dst,
                depth + 1,
                &relative,
                ctx,
                substitute,
                collector,
            )?;
        } else if ft.is_file() {
            copy_file_substituted(&entry.path(), &dst, ctx, substitute)?;
            if let Some(collector) = collector.as_deref_mut() {
                collector.record(relative);
            }
        }
        // Anything else (already-filtered symlinks/reparse) is ignored.
    }
    Ok(())
}

fn copy_file_substituted(
    src: &Path,
    dst: &Path,
    ctx: Option<&PlaceholderContext>,
    substitute: bool,
) -> Result<(), String> {
    // #769 P2: verbatim stream-copy when substitution is off (embedded->master and
    // the reseed backup keep raw %AC_% tokens intact) or when no context is given.
    let ctx = match (substitute, ctx) {
        (true, Some(ctx)) => ctx,
        _ => {
            return std::fs::copy(src, dst)
                .map(|_| ())
                .map_err(|e| format!("copy {} -> {}: {}", src.display(), dst.display(), e));
        }
    };
    let len = std::fs::metadata(src)
        .map_err(|e| format!("stat {}: {}", src.display(), e))?
        .len();
    // M4: over-cap files are streamed, never read whole into memory.
    if len > CONTENT_SUBSTITUTION_CAP {
        std::fs::copy(src, dst)
            .map_err(|e| format!("copy {} -> {}: {}", src.display(), dst.display(), e))?;
        return Ok(());
    }
    let bytes = std::fs::read(src).map_err(|e| format!("read {}: {}", src.display(), e))?;
    // Binary sniff: a NUL in the leading window means "copy verbatim".
    let probe = bytes.len().min(8192);
    if bytes[..probe].contains(&0u8) {
        return std::fs::write(dst, &bytes).map_err(|e| format!("write {}: {}", dst.display(), e));
    }
    match std::str::from_utf8(&bytes) {
        // CRLF and a UTF-8 BOM survive (only known token substrings are swapped).
        Ok(text) => std::fs::write(dst, expand_placeholders_in_content(text, ctx).as_bytes())
            .map_err(|e| format!("write {}: {}", dst.display(), e)),
        // Non-UTF-8 (no NUL): copy verbatim.
        Err(_) => {
            std::fs::write(dst, &bytes).map_err(|e| format!("write {}: {}", dst.display(), e))
        }
    }
}

/// True for a symlink OR a Windows junction/reparse point. `is_symlink()` alone
/// is FALSE for junctions, so the reparse-attribute check is required (M6).
/// `DirEntry::metadata` does NOT traverse, so the junction's own attributes are
/// observed. An undeterminable file type is treated as skip-worthy.
fn is_symlink_or_reparse(entry: &std::fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(ft) if ft.is_symlink() => return true,
        Ok(_) => {}
        Err(_) => return true,
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if let Ok(md) = entry.metadata() {
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests;
