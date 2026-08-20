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
    let Ok(text) = std::fs::read_to_string(&config_path) else {
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
    let Ok(wg_root) = std::fs::canonicalize(parent) else {
        return Ok(RepoMountResolution::default());
    };

    // If the session cwd is not a workgroup replica, there are no repo mounts,
    // full stop. Same predicate `container_mount_source_rejection:285-296` uses:
    // the parent name starts with `wg-` and a workspace ancestor exists.
    let is_workgroup_root = wg_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("wg-"))
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
        let canonical = match std::fs::canonicalize(&joined) {
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
        let is_dir = canonical.is_dir();
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
                "container repo mount refused: repos[] entry '{}' resolves to '{}', which is not an \
                 admissible repo-* directory directly under the workgroup root \
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
                debug_assert!(
                    seen.insert(container_path.clone()),
                    "duplicate container mount target {}",
                    container_path
                );
            }
        }
    }

    Ok(RepoMountResolution { entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[cfg(unix)]
    fn try_symlink_dir(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn try_symlink_dir(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }

    /// Build `<tmp>/.ac/wg-test-team/` with the given replica name, and return
    /// (tmp, wg_root, replica_root). `find_ac_root_ancestor` keys on a `.ac`
    /// ancestor and the wg-root name must start with `wg-`, so both are created.
    fn make_workgroup(replica: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("tmp");
        let wg_root = tmp.path().join(".ac").join("wg-test-team");
        let replica_root = wg_root.join(replica);
        std::fs::create_dir_all(&replica_root).expect("replica dir");
        let wg_root = std::fs::canonicalize(&wg_root).expect("canonical wg root");
        let replica_root = std::fs::canonicalize(&replica_root).expect("canonical replica");
        (tmp, wg_root, replica_root)
    }

    fn write_repos(replica_root: &Path, repos: &[&str]) {
        let json = serde_json::json!({ "repos": repos });
        std::fs::write(
            replica_root.join("config.json"),
            serde_json::to_string(&json).unwrap(),
        )
        .expect("write config.json");
    }

    fn mkdir(base: &Path, name: &str) -> PathBuf {
        let path = base.join(name);
        std::fs::create_dir_all(&path).expect("mkdir");
        path
    }

    #[test]
    fn normal_sibling_repo_resolves_to_repos_container_path() {
        let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
        mkdir(&wg_root, "repo-AgentsCommander");
        write_repos(&replica, &["../repo-AgentsCommander"]);

        let resolution = resolve_repo_mounts(&replica).expect("ok");
        assert_eq!(resolution.entries.len(), 1);
        match &resolution.entries[0].outcome {
            RepoOutcome::Mounted {
                name,
                container_path,
                ..
            } => {
                assert_eq!(name, "repo-AgentsCommander");
                assert_eq!(container_path, "/repos/repo-AgentsCommander");
            }
            other => panic!("expected Mounted, got {:?}", other),
        }
        let mounts: Vec<_> = resolution.mounts().collect();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].1, "/repos/repo-AgentsCommander");
    }

    #[test]
    fn mounted_host_path_has_no_windows_verbatim_prefix() {
        // #993 - std::fs::canonicalize yields \\?\C:\... on Windows and dockerd
        // rejects that as a --mount source. make_workgroup canonicalizes the
        // tempdir, so on Windows this fails without the normalize at the store.
        let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
        mkdir(&wg_root, "repo-AgentsCommander");
        write_repos(&replica, &["../repo-AgentsCommander"]);

        let resolution = resolve_repo_mounts(&replica).expect("ok");
        let mounts: Vec<_> = resolution.mounts().collect();
        assert_eq!(mounts.len(), 1);
        let host = mounts[0].0.to_string_lossy();
        assert!(
            !host.starts_with(r"\\?\"),
            "mount source must not carry the verbatim prefix: {host}"
        );
    }

    #[test]
    fn symlink_named_repo_but_targeting_sibling_replica_is_rejected() {
        // grinch Finding 2, THE critical test. A symlink whose APPARENT name
        // starts with `repo-` but whose CANONICAL target is a sibling replica.
        // The parent check PASSES (the target's parent IS the wg root), so only
        // the canonical-NAME check can reject it. Nothing else does:
        // container_mount_source_rejection does not catch a sibling replica.
        let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
        let other = mkdir(&wg_root, "__agent_other");
        let trap = replica.join("repo-trap");
        if !try_symlink_dir(&other, &trap) {
            eprintln!("skipping: no symlink privilege on this platform");
            return;
        }
        write_repos(&replica, &["./repo-trap"]);

        let result = resolve_repo_mounts(&replica);
        assert!(
            result.is_err(),
            "a repo-*-named symlink to a sibling replica must hard-fail, got {:?}",
            result
        );
    }

    #[test]
    fn symlink_named_repo_but_targeting_messaging_is_rejected() {
        // The twin: same shape, canonical target is an AC-owned NON-replica.
        // Parent check passes again; canonical name `messaging` has no `repo-`
        // prefix -> Err. Covers the other class of AC-owned target.
        let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
        let messaging = mkdir(&wg_root, "messaging");
        let trap = replica.join("repo-trap");
        if !try_symlink_dir(&messaging, &trap) {
            eprintln!("skipping: no symlink privilege on this platform");
            return;
        }
        write_repos(&replica, &["./repo-trap"]);

        assert!(
            resolve_repo_mounts(&replica).is_err(),
            "a repo-*-named symlink to messaging/ must hard-fail"
        );
    }

    #[test]
    fn plain_symlink_to_messaging_is_rejected_before_the_name_check() {
        // Apparent name NOT `repo-`-prefixed: proves canonicalize runs before
        // validation. Note this does NOT cover the two tests above: `messaging`
        // fails the parent check on its own and never exercises the name rule.
        let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
        let messaging = mkdir(&wg_root, "messaging");
        let link = replica.join("plain-link");
        if !try_symlink_dir(&messaging, &link) {
            eprintln!("skipping: no symlink privilege on this platform");
            return;
        }
        write_repos(&replica, &["./plain-link"]);

        assert!(resolve_repo_mounts(&replica).is_err());
    }

    #[test]
    fn messaging_sibling_is_rejected() {
        let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
        mkdir(&wg_root, "messaging");
        write_repos(&replica, &["../messaging"]);
        assert!(resolve_repo_mounts(&replica).is_err());
    }

    #[test]
    fn other_agent_replica_is_rejected() {
        let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
        mkdir(&wg_root, "__agent_other");
        write_repos(&replica, &["../__agent_other"]);
        assert!(resolve_repo_mounts(&replica).is_err());
    }

    #[test]
    fn own_replica_is_rejected() {
        let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
        write_repos(&replica, &["../__agent_self"]);
        assert!(resolve_repo_mounts(&replica).is_err());
    }

    #[test]
    fn workgroup_root_itself_is_rejected() {
        let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
        write_repos(&replica, &[".."]);
        assert!(resolve_repo_mounts(&replica).is_err());
    }

    #[test]
    fn path_at_or_above_ac_root_is_rejected() {
        let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
        write_repos(&replica, &["../../.."]);
        assert!(resolve_repo_mounts(&replica).is_err());
    }

    #[test]
    fn absolute_path_to_real_dir_elsewhere_is_rejected() {
        let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        let elsewhere_repo = mkdir(elsewhere.path(), "repo-elsewhere");
        write_repos(&replica, &[elsewhere_repo.to_str().unwrap()]);
        assert!(
            resolve_repo_mounts(&replica).is_err(),
            "Q4: a repo outside the workgroup root is prohibited (post-collapse: hard-fail)"
        );
    }

    #[test]
    fn direct_wg_child_without_repo_prefix_is_rejected() {
        let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
        mkdir(&wg_root, "not-repo-prefixed");
        write_repos(&replica, &["../not-repo-prefixed"]);
        assert!(resolve_repo_mounts(&replica).is_err());
    }

    #[test]
    fn missing_dir_is_not_found_and_still_launches() {
        let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
        write_repos(&replica, &["../repo-missing"]);
        let resolution = resolve_repo_mounts(&replica).expect("NOT FOUND must not hard-fail");
        assert_eq!(resolution.entries.len(), 1);
        assert_eq!(resolution.entries[0].outcome, RepoOutcome::NotFound);
        assert_eq!(resolution.mounts().count(), 0);
    }

    #[test]
    fn a_plain_file_named_repo_is_rejected() {
        let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
        std::fs::write(wg_root.join("repo-file"), b"not a dir").expect("write file");
        write_repos(&replica, &["../repo-file"]);
        assert!(
            resolve_repo_mounts(&replica).is_err(),
            "an existing non-directory is inadmissible"
        );
    }

    #[test]
    fn two_agents_with_different_repos_resolve_to_different_mount_sets() {
        // Per-agent enablement (plan Sec 2): each replica's own repos[] drives its
        // own mount set.
        let (_tmp, wg_root, replica_one) = make_workgroup("__agent_one");
        let replica_two = wg_root.join("__agent_two");
        std::fs::create_dir_all(&replica_two).expect("agent two");
        let replica_two = std::fs::canonicalize(&replica_two).expect("canonical two");
        mkdir(&wg_root, "repo-AgentsCommander");
        mkdir(&wg_root, "repo-webpage");
        write_repos(&replica_one, &["../repo-AgentsCommander"]);
        write_repos(&replica_two, &["../repo-webpage"]);

        let one: Vec<_> = resolve_repo_mounts(&replica_one)
            .expect("ok")
            .mounts()
            .map(|(_, container)| container.to_string())
            .collect();
        let two: Vec<_> = resolve_repo_mounts(&replica_two)
            .expect("ok")
            .mounts()
            .map(|(_, container)| container.to_string())
            .collect();
        assert_eq!(one, vec!["/repos/repo-AgentsCommander".to_string()]);
        assert_eq!(two, vec!["/repos/repo-webpage".to_string()]);
        assert_ne!(one, two);
    }

    #[test]
    fn no_config_json_yields_no_mounts() {
        let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
        // No config.json written.
        let resolution = resolve_repo_mounts(&replica).expect("ok");
        assert!(resolution.entries.is_empty());
    }

    #[test]
    fn not_a_workgroup_replica_yields_no_mounts() {
        // A replica whose parent is not a `wg-` workspace child: no repo mounts,
        // full stop (no error).
        let tmp = tempfile::tempdir().expect("tmp");
        let replica = tmp.path().join("some-dir");
        std::fs::create_dir_all(&replica).expect("dir");
        let replica = std::fs::canonicalize(&replica).expect("canonical");
        write_repos(&replica, &["../repo-x"]);
        let resolution = resolve_repo_mounts(&replica).expect("ok");
        assert!(resolution.entries.is_empty());
    }
}
