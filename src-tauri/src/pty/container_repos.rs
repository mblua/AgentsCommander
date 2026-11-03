//! #935 - resolve a container replica's enabled repos into read-write bind mounts.
//!
//! The replica's own `config.json` `repos[]` array is already per-agent (team
//! creation filters it, see plan Sec 2). This module turns that array into the
//! Docker `--mount` list and the injected-context path list, from ONE ordered
//! resolution so the two consumers cannot drift (plan Sec 5).
//!
//! SECURITY (plan Sec 4): `config.json` sits on the container's own read-write
//! mount, so `repos[]` is attacker-controlled. Every admissibility check runs on
//! the CANONICAL path (after resolving symlinks/junctions), never on the raw
//! entry or its apparent name, and the container mount target is derived from the
//! canonical name too. A `repo-`-named symlink whose target is a sibling
//! `__agent_*` replica or `messaging/` must be REJECTED, and the only thing that
//! rejects it is the canonical-name check: `container_mount_source_rejection`
//! does not catch a sibling replica (plan Sec 7 S3).

use std::path::{Path, PathBuf};

use crate::filesystem::{directories, documents};

use crate::pty::container_paths::{container_mount_source_rejection, root_key};

/// Container-side root under which each enabled repo is mounted (plan Sec 3.2).
pub const CONTAINER_REPOS_ROOT: &str = "/repos";

/// A single admissible repo mount: the canonical host source and its container
/// target. Both the Docker args and the injected context read from this, so they
/// cannot disagree (plan Sec 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRepoMount {
    pub host_path: PathBuf,
    pub container_path: String,
}

/// The outcome of resolving one `repos[]` entry. Two variants only: an entry that
/// EXISTS but is not admissible never reaches here, it is the `Err` of
/// [`resolve_repo_mounts`] and the session does not launch (plan Sec 4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoOutcome {
    /// Admissible (plan Sec 4.2). `name` and `container_path` both derive from
    /// `canonical.file_name()`, never from the entry string.
    Mounted {
        name: String,
        host_path: PathBuf,
        container_path: String,
    },
    /// Does not canonicalize (the directory is absent). No mount, the session
    /// still launches, renders `**(NOT FOUND)**` as today (plan Sec 4.3).
    NotFound,
}

/// One `repos[]` entry with its outcome, in `config.json` order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRepo {
    /// The verbatim entry, for the error/log text only.
    pub config_entry: String,
    pub outcome: RepoOutcome,
}

/// Every `repos[]` entry, resolved, in config order. One structure feeds BOTH the
/// Docker mounts and the context renderer (plan Sec 5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoMountResolution {
    pub entries: Vec<ResolvedRepo>,
}

impl RepoMountResolution {
    /// The Docker consumer: canonical host path + container target for each
    /// mounted repo, in config order.
    pub fn mounts(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.entries
            .iter()
            .filter_map(|entry| match &entry.outcome {
                RepoOutcome::Mounted {
                    host_path,
                    container_path,
                    ..
                } => Some((host_path.as_path(), container_path.as_str())),
                RepoOutcome::NotFound => None,
            })
    }
}

/// Read this replica's `repos[]` from `<replica_root>/config.json`. Best-effort:
/// a missing/unreadable/unparseable config, or a non-array `repos`, yields no
/// entries (no repo mounts), never an error. `session.rs` does not parse
/// `config.json` on the spawn path (dev-rust plan Sec 14.5), so the resolver reads
/// it itself rather than threading a `serde_json::Value`.
fn read_repos_array(replica_root: &Path) -> Vec<String> {
    let config_path = replica_root.join("config.json");
    let Ok(text) = documents::read_container_repo_config(&config_path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    value
        .get("repos")
        .and_then(|repos| repos.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve this replica's enabled repos (its own `config.json` `repos` array,
/// which is already per-agent: plan Sec 2) into container mounts.
///
/// Entries are resolved as `canonicalize(replica_root.join(entry))` - the SAME
/// base and join `render_agent_repos_string` uses (in `config::session_context`).
/// Do NOT change the base: a different one reintroduces D2 (the mount and the
/// injected context disagreeing).
///
/// `Err` = an entry that EXISTS but is not admissible (plan Sec 4.2). Escape
///         signature: the caller MUST refuse to launch the session (plan Sec 4.3/4.4).
/// `Ok`  = every entry, in config order, `Mounted` or `NotFound`.
pub fn resolve_repo_mounts(replica_root: &Path) -> Result<RepoMountResolution, String> {
    // The workgroup root is the replica's parent. `replica_root` is already
    // canonical (it is `container_path_context.host_root`), but do not assume the
    // parent of a canonical path is itself canonical: canonicalize it explicitly,
    // which is cheap and also resolves a linked workgroup dir.
    let Some(parent) = replica_root.parent() else {
        return Ok(RepoMountResolution::default());
    };
    let Ok(wg_root) = directories::canonical_container_mount(parent) else {
        return Ok(RepoMountResolution::default());
    };

    // If the session cwd is not a workgroup replica, there are no repo mounts,
    // full stop. Same predicate `container_mount_source_rejection:285-296` uses:
    // the parent name starts with `wg-` and a workspace ancestor exists.
    let is_workgroup_root = wg_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(crate::config::entity_prefix::has_entity_prefix)
        .unwrap_or(false)
        && crate::config::ac_root::find_ac_root_ancestor(&wg_root).is_some();
    if !is_workgroup_root {
        return Ok(RepoMountResolution::default());
    }

    let wg_key = root_key(&wg_root.to_string_lossy());

    let mut entries = Vec::new();
    for entry in read_repos_array(replica_root) {
        // STEP 1 (plan Sec 4.3): does it exist? Resolve against the replica root,
        // the SAME base the renderer uses.
        let joined = replica_root.join(&entry);
        let canonical = match crate::filesystem::directories::canonical_container_mount(&joined) {
            Ok(canonical) => canonical,
            Err(_) => {
                // NOT FOUND: benign, AC-produced state (a best-effort clone can
                // leave a repos[] entry with no dir). No mount, session launches.
                entries.push(ResolvedRepo {
                    config_entry: entry,
                    outcome: RepoOutcome::NotFound,
                });
                continue;
            }
        };

        // STEP 2 (plan Sec 4.2): admissible? ALL four checks on the CANONICAL
        // path. Never on `entry` or on `joined.file_name()` (the apparent name):
        // a `repo-`-named symlink to `../__agent_other` passes a parent check and
        // an apparent-name check but must be rejected by the canonical-name check.
        let parent_is_wg_root = canonical
            .parent()
            .map(|dir| root_key(&dir.to_string_lossy()) == wg_key)
            .unwrap_or(false);
        let canonical_name = canonical.file_name().and_then(|name| name.to_str());
        let name_has_repo_prefix = canonical_name
            .map(|name| name.starts_with("repo-"))
            .unwrap_or(false);
        let is_dir = crate::filesystem::directories::container_mount_is_dir(&canonical);
        let not_rejected = container_mount_source_rejection(&canonical).is_none();

        if parent_is_wg_root && name_has_repo_prefix && is_dir && not_rejected {
            // Safe: `name_has_repo_prefix` implies `canonical_name` is `Some`.
            let name = canonical_name.unwrap_or_default().to_string();
            let container_path = format!("{}/{}", CONTAINER_REPOS_ROOT, name);
            entries.push(ResolvedRepo {
                config_entry: entry,
                outcome: RepoOutcome::Mounted {
                    name,
                    // #993 - dockerd rejects a verbatim \\?\ mount source. Strip it
                    // AFTER the Sec 4.2 checks above, which run on the verbatim path.
                    host_path: crate::path_utils::normalize_windows_verbatim_path_buf(&canonical),
                    container_path,
                },
            });
        } else {
            // An EXISTING inadmissible path is an escape signature: hard-fail the
            // spawn, loudly, naming the entry, its canonical target and the failed
            // check(s).
            return Err(format!(
                "container repo mount refused: repos[] entry '{}' resolves to '{}', \
                 which is not an admissible repo-* directory directly under the room root \
                 (direct_wg_child={}, repo_prefix={}, is_dir={}, not_reserved={})",
                entry,
                crate::path_utils::path_to_string_without_windows_verbatim_prefix(&canonical),
                parent_is_wg_root,
                name_has_repo_prefix,
                is_dir,
                not_rejected
            ));
        }
    }

    // Collisions are structurally impossible under Sec 4.2 (every mount is a
    // direct child of ONE dir, so names are unique). Assert it in debug and move
    // on; do NOT build a suffix scheme for a case that cannot occur (plan Sec 3.3).
    #[cfg(debug_assertions)]
    {
        let mut seen = std::collections::HashSet::new();
        for entry in &entries {
            if let RepoOutcome::Mounted { container_path, .. } = &entry.outcome {
                let inserted = seen.insert(container_path.clone());
                debug_assert!(
                    inserted,
                    "duplicate container mount target {}",
                    container_path
                );
            }
        }
    }

    Ok(RepoMountResolution { entries })
}

#[cfg(test)]
mod tests;
