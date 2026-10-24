//! #598 - seed a template config folder (e.g. `.claude`) into a coding-agent
//! replica at spawn time.
//!
//! Two phases:
//! - [`resolve_config_seed`] is PURE path math (no filesystem) and fail-soft:
//!   it never returns an error that aborts a launch (M7).
//! - [`perform_config_seed`](perform::perform_config_seed) is the side-effecting,
//!   best-effort, Windows-safe rename-to-trash-first atomic swap (H1): the
//!   destination is always either fully-old or fully-new, never half-written.
//!   It never aborts the spawn.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::config::placeholders::{PlaceholderContext, PlaceholderRootKind};
use crate::config::seed_manifest::ResourceBoundKind;
use crate::config::settings::{validate_config_seed_dest, ConfigSeedConfig};

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

mod copy;
mod perform;

pub(crate) use copy::copy_tree;
#[cfg(test)]
pub(crate) use copy::is_nonempty_seed_dir;
#[cfg(test)]
pub(crate) use perform::perform_config_seed;
pub(crate) use perform::perform_config_seed_recorded;

#[cfg(test)]
mod tests;
