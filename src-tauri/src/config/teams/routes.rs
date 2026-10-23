use super::agent_fqn_from_path;
use super::enumerate_project_dirs_strict;
use super::replica::identity_fingerprint;
use super::replica::incarnation_fingerprint;
use super::replica::parse_strict_pty_fqn;
use super::replica::terminal_snapshot_target_syntax;
use super::replica::verify_replica;
use super::replica::PtyInputAuthorityKind;
use super::replica::StrictPtyFqn;
use super::replica::TerminalSnapshotAuthorityKind;
use super::replica::TerminalSnapshotTargetIdentity;
use super::replica::TerminalSnapshotTargetSyntax;
use super::replica::VerifiedPtyInputIdentity;
use super::replica::VerifiedPtyInputRoute;
use super::replica::VerifiedTerminalSnapshotRoute;
use super::strict_project_ac_root;
use super::ProjectOrigin;
use std::path::Path;
use std::path::PathBuf;

pub(crate) fn strict_wg_replica_anchor_from_cwd(cwd: &Path) -> Result<Option<PathBuf>, String> {
    let cwd_identity = crate::path_identity::verify_directory(cwd)?;
    // #1535: an anchor binds only when it belongs to the project that owns
    // the cwd, i.e. its `wg_replica_layout.ac_root` equals the closest `.ac`
    // ancestor of the cwd. A `__agent_*` replica of an ANCESTOR project is
    // never an anchor for a nested-project cwd, so the ancestor's live
    // session can no longer produce a sessionRace false positive.
    let owning_ac_root = cwd_identity.canonical_path.ancestors().find(|path| {
        path.file_name()
            .and_then(|value| value.to_str())
            .is_some_and(crate::config::ac_root::is_ac_root_name)
    });
    for path in cwd_identity.canonical_path.ancestors() {
        let is_replica = path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("__agent_"));
        if !is_replica {
            continue;
        }
        // No `.ac` ancestor at all: no project owns the cwd, so no anchor can
        // bind (every valid wg-replica layout requires an `.ac` root).
        let Some(owning_ac_root) = owning_ac_root else {
            return Ok(None);
        };
        if !path.starts_with(owning_ac_root) {
            // The anchor belongs to an ancestor project's `.ac`; it never
            // binds to this cwd. Keep walking: the cwd's own project may
            // still hold a valid anchor above the current path.
            continue;
        }
        let Some(layout) = crate::config::ac_root::wg_replica_layout_from_agent_dir(path)? else {
            continue;
        };
        if layout.ac_root != owning_ac_root {
            // Belt-and-braces: the layout's own root must be the owning root.
            // Unreachable for real on-disk layouts (a valid layout under the
            // owning root always reports the owning root), kept as a guard.
            continue;
        }
        return Ok(Some(path.to_path_buf()));
    }
    Ok(None)
}

/// Derive the universal create-gate key from the verified directory layout
/// alone. Ordinary session creation must not depend on team membership,
/// replica config, matrix config, or the privileged SQLite store being healthy.
/// For a target that is eligible for privileged PTY input this reconstructs the
/// exact canonical FQN, so ordinary and privileged creates contend on the same
/// stripe and exact key. Structurally valid legacy layouts that cannot form a
/// privileged FQN still receive a stable physical-replica key.
pub(crate) fn pty_input_create_gate_key_from_cwd(cwd: &Path) -> Result<Option<String>, String> {
    let Some(replica_root) = strict_wg_replica_anchor_from_cwd(cwd)? else {
        return Ok(None);
    };
    let layout = crate::config::ac_root::wg_replica_layout_from_agent_dir(&replica_root)?
        .ok_or_else(|| "target_create_gate_unavailable".to_string())?;
    let project = layout
        .project_dir
        .file_name()
        .and_then(|value| value.to_str());
    if let Some(project) = project {
        let candidate = format!("{}:{}/{}", project, layout.wg_name, layout.agent_name);
        if parse_strict_pty_fqn(&candidate).is_ok() {
            return Ok(Some(candidate));
        }
    }

    let replica = crate::path_identity::verify_directory(&replica_root)?;
    Ok(Some(format!(
        "physical-wg-replica:{:016x}:{:016x}",
        replica.object_id.volume, replica.object_id.file
    )))
}

fn replica_anchor_from_cwd(cwd: &Path) -> Result<PathBuf, String> {
    strict_wg_replica_anchor_from_cwd(cwd)?.ok_or_else(|| "sender_identity_invalid".to_string())
}

fn verify_sender_replica(cwd: &Path) -> Result<VerifiedPtyInputIdentity, String> {
    let replica = replica_anchor_from_cwd(cwd)?;
    let workgroup = replica
        .parent()
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    let ac_root = workgroup
        .parent()
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    let project = ac_root
        .parent()
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    let fqn = agent_fqn_from_path(
        replica
            .to_str()
            .ok_or_else(|| "sender_identity_invalid".to_string())?,
    );
    let parsed = parse_strict_pty_fqn(&fqn)?;
    let identity = verify_replica(project, ac_root, &parsed)?;
    let cwd_identity = crate::path_identity::verify_directory(cwd)?;
    if !crate::path_identity::is_verified_descendant(&cwd_identity, &identity.replica_identity) {
        return Err("sender_identity_invalid".to_string());
    }
    Ok(identity)
}

/// Read-only coordinator proof used while minting a container scope and while
/// revalidating live coordinator authority.
pub(crate) fn verify_pty_input_replica_cwd(
    root: &Path,
) -> Result<VerifiedPtyInputIdentity, String> {
    verify_sender_replica(root)
}

pub fn verify_pty_input_coordinator_root(root: &Path) -> Result<VerifiedPtyInputIdentity, String> {
    let identity = verify_sender_replica(root)?;
    if !identity.is_coordinator {
        return Err("sender_not_coordinator".to_string());
    }
    Ok(identity)
}

fn find_target_identity(
    parsed: &StrictPtyFqn,
    project_paths: &[String],
) -> Result<VerifiedPtyInputIdentity, String> {
    if project_paths.len() > 1_024 {
        return Err("invalid_target".to_string());
    }
    let mut _skipped = 0usize;
    let candidates = enumerate_project_dirs_strict(project_paths, &mut _skipped)?;
    let mut matching_projects = Vec::new();
    let mut seen_objects = std::collections::HashSet::new();
    for (project_name, project_dir, _origin) in candidates {
        if !crate::path_identity::paths_equivalent(
            Path::new(&project_name),
            Path::new(&parsed.project),
        ) {
            continue;
        }
        let identity = crate::path_identity::verify_directory(&project_dir)?;
        if seen_objects.insert(identity.object_id) {
            matching_projects.push((project_name, project_dir));
        }
    }
    if matching_projects.len() != 1 || matching_projects[0].0 != parsed.project {
        return Err("invalid_target".to_string());
    }
    let (_, project_dir) = matching_projects.remove(0);
    let ac_root =
        strict_project_ac_root(&project_dir)?.ok_or_else(|| "invalid_target".to_string())?;
    crate::path_identity::verify_directory(&ac_root.join(&parsed.workgroup))?;
    let identity = verify_replica(&project_dir, &ac_root, parsed)?;
    let reconstructed = format!(
        "{}:{}/{}",
        identity.project, identity.workgroup, identity.agent
    );
    let supplied = format!("{}:{}/{}", parsed.project, parsed.workgroup, parsed.agent);
    if reconstructed != supplied {
        return Err("invalid_target".to_string());
    }
    Ok(identity)
}

fn resolve_verified_wg_target(
    target_fqn: &str,
    project_paths: &[String],
) -> Result<VerifiedPtyInputIdentity, String> {
    let parsed = parse_strict_pty_fqn(target_fqn)?;
    let target = find_target_identity(&parsed, project_paths)?;
    if target.canonical_fqn != target_fqn {
        return Err("invalid_target".to_string());
    }
    Ok(target)
}

pub(crate) fn resolve_pty_input_target(
    target_fqn: &str,
    project_paths: &[String],
) -> Result<VerifiedPtyInputIdentity, String> {
    resolve_verified_wg_target(target_fqn, project_paths)
}

/// How many candidates the snapshot-target scan skipped, by kind. Counts only:
/// nothing here may carry a path, a name, an FQN or a rule name (#2228 Tier B).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SnapshotDiscoverySkips {
    pub project_children: usize,
    pub rooms: usize,
    pub replicas: usize,
}

impl SnapshotDiscoverySkips {
    pub(crate) fn any(self) -> bool {
        self.project_children > 0 || self.rooms > 0 || self.replicas > 0
    }
}

pub(crate) struct DiscoveredSnapshotTargets {
    pub targets: Vec<TerminalSnapshotTargetIdentity>,
    pub skips: SnapshotDiscoverySkips,
}

/// Signature-preserving wrapper. `phone/mailbox.rs` iterates the returned
/// `Vec` directly, so the counted form is additive and that caller compiles
/// untouched (#2228).
pub(crate) fn discover_verified_terminal_snapshot_targets(
    project_paths: &[String],
) -> Result<Vec<TerminalSnapshotTargetIdentity>, String> {
    discover_verified_terminal_snapshot_targets_counted(project_paths).map(|d| d.targets)
}

pub(crate) fn discover_verified_terminal_snapshot_targets_counted(
    project_paths: &[String],
) -> Result<DiscoveredSnapshotTargets, String> {
    const TARGET_CAP: usize = 4_096;
    let mut skips = SnapshotDiscoverySkips::default();
    let projects = enumerate_project_dirs_strict(project_paths, &mut skips.project_children)?;
    let mut project_names = std::collections::HashSet::new();
    let mut project_objects = std::collections::HashSet::new();
    let mut verified_projects = Vec::new();
    for (name, path, origin) in projects {
        // Defence-in-depth (#2228): after the skip at the enumeration site a
        // scanned child that fails here can only have lost a TOCTOU race, so a
        // race degrades to a counted skip instead of aborting the whole scan.
        // No test covers this and none is claimed.
        let identity = match crate::path_identity::verify_directory(&path) {
            Ok(identity) => identity,
            Err(error) => {
                if origin == ProjectOrigin::ScannedChild {
                    skips.project_children = skips.project_children.saturating_add(1);
                    continue;
                }
                return Err(error);
            }
        };
        if !project_names.insert(name.clone()) || !project_objects.insert(identity.object_id) {
            return Err("ambiguous_project".to_string());
        }
        verified_projects.push((name, path, origin));
    }
    let projects = verified_projects;

    let mut targets = Vec::new();
    let mut target_names = std::collections::HashSet::new();
    let mut target_objects = std::collections::HashSet::new();
    let mut scanned_entries = 0usize;
    for (project, project_dir, origin) in projects {
        // Same defence-in-depth as the pre-pass above: no probe, no claimed
        // coverage (#2228).
        let ac_root = match strict_project_ac_root(&project_dir) {
            Ok(Some(ac_root)) => ac_root,
            Ok(None) | Err(_) if origin == ProjectOrigin::ScannedChild => {
                skips.project_children = skips.project_children.saturating_add(1);
                continue;
            }
            Ok(None) => return Err("unsafe_path".to_string()),
            Err(error) => return Err(error),
        };
        let workgroups = std::fs::read_dir(&ac_root).map_err(|_| "unsafe_path".to_string())?;
        for workgroup in workgroups {
            let workgroup = workgroup.map_err(|_| "unsafe_path".to_string())?;
            scanned_entries = scanned_entries.saturating_add(1);
            if scanned_entries > TARGET_CAP * 4 {
                return Err("target_limit".to_string());
            }
            let name = workgroup
                .file_name()
                .into_string()
                .map_err(|_| "unsafe_path".to_string())?;
            if !crate::config::entity_prefix::has_entity_prefix(&name) {
                continue;
            }
            // #2228 Site B: a linked or junctioned room entry reaches this
            // call with no metadata gate in front of it. Skip and count it
            // rather than aborting every other target of the scan.
            let Ok(workgroup_identity) = crate::path_identity::verify_directory(&workgroup.path())
            else {
                skips.rooms = skips.rooms.saturating_add(1);
                continue;
            };
            let replicas = std::fs::read_dir(&workgroup_identity.canonical_path)
                .map_err(|_| "unsafe_path".to_string())?;
            for replica in replicas {
                let replica = replica.map_err(|_| "unsafe_path".to_string())?;
                scanned_entries = scanned_entries.saturating_add(1);
                if scanned_entries > TARGET_CAP * 4 {
                    return Err("target_limit".to_string());
                }
                let replica_name = replica
                    .file_name()
                    .into_string()
                    .map_err(|_| "unsafe_path".to_string())?;
                let Some(agent) = replica_name.strip_prefix("__agent_") else {
                    continue;
                };
                let candidate = format!("{project}:{name}/{agent}");
                // #2228 Site C: one stale or half-removed replica must not hide
                // every other target.
                let Ok(parsed) = parse_strict_pty_fqn(&candidate) else {
                    skips.replicas = skips.replicas.saturating_add(1);
                    continue;
                };
                let Ok(identity) = verify_replica(&project_dir, &ac_root, &parsed) else {
                    skips.replicas = skips.replicas.saturating_add(1);
                    continue;
                };
                if !target_names.insert(identity.canonical_fqn.clone())
                    || !target_objects.insert(identity.replica_identity.object_id)
                {
                    return Err("ambiguous_target".to_string());
                }
                targets.push(TerminalSnapshotTargetIdentity {
                    canonical_fqn: identity.canonical_fqn,
                    replica_root: identity.replica_root,
                    project: identity.project,
                    workgroup: identity.workgroup,
                    team: parsed.team,
                    is_coordinator: identity.is_coordinator,
                });
                if targets.len() > TARGET_CAP {
                    return Err("target_limit".to_string());
                }
            }
        }
    }
    targets.sort_by(|left, right| left.canonical_fqn.cmp(&right.canonical_fqn));
    Ok(DiscoveredSnapshotTargets { targets, skips })
}

pub(crate) fn verify_terminal_snapshot_root_identity(
    root: &Path,
) -> Result<VerifiedPtyInputIdentity, String> {
    let root_identity = crate::config::root_agent::verify_live_root_agent_path(root)?;
    Ok(VerifiedPtyInputIdentity {
        canonical_fqn: crate::config::root_agent::ROOT_AGENT_SENDER.to_string(),
        project: String::new(),
        workgroup: String::new(),
        agent: "root-agent".to_string(),
        replica_root: root_identity.canonical_path.clone(),
        matrix_root: root_identity.canonical_path.clone(),
        is_coordinator: false,
        project_identity: root_identity.clone(),
        ac_root_identity: root_identity.clone(),
        workgroup_identity: root_identity.clone(),
        replica_identity: root_identity.clone(),
        matrix_identity: root_identity.clone(),
        incarnation_fingerprint: incarnation_fingerprint(&root_identity),
        authority_fingerprint: identity_fingerprint(&[&root_identity]),
    })
}

/// Verify the only two privileged routes. No discovery repair or broad
/// communication predicate is used.
pub fn verify_pty_input_route(
    sender_cwd: &Path,
    sender_is_root: bool,
    target_fqn: &str,
    project_paths: &[String],
) -> Result<VerifiedPtyInputRoute, String> {
    if sender_is_root {
        let sender = verify_terminal_snapshot_root_identity(sender_cwd)?;
        let target = resolve_pty_input_target(target_fqn, project_paths)?;
        if !target.is_coordinator {
            return Err("target_out_of_scope".to_string());
        }
        return Ok(VerifiedPtyInputRoute {
            sender,
            target,
            kind: PtyInputAuthorityKind::Root,
        });
    }

    // Prove coordinator authority before any target hierarchy lookup. Negative
    // senders therefore cannot use this resolver as a privileged target oracle.
    let sender = verify_pty_input_coordinator_root(sender_cwd)?;
    let target = resolve_pty_input_target(target_fqn, project_paths)?;
    if sender.project != target.project || sender.workgroup != target.workgroup {
        return Err("target_out_of_scope".to_string());
    }
    if target.is_coordinator {
        return Err("target_is_coordinator".to_string());
    }
    if sender.canonical_fqn == target.canonical_fqn {
        return Err("target_out_of_scope".to_string());
    }
    Ok(VerifiedPtyInputRoute {
        sender,
        target,
        kind: PtyInputAuthorityKind::Coordinator,
    })
}

/// Verify the distinct #1173 read capability. This deliberately does not call
/// the PTY-input route policy, mint input authority, or change Root actuation.
pub(crate) fn verify_terminal_snapshot_route(
    sender_cwd: &Path,
    sender_is_root: bool,
    target_fqn: &str,
    project_paths: &[String],
) -> Result<VerifiedTerminalSnapshotRoute, String> {
    let syntax = terminal_snapshot_target_syntax(target_fqn)?;
    if sender_is_root {
        let sender = verify_terminal_snapshot_root_identity(sender_cwd)?;
        if syntax != TerminalSnapshotTargetSyntax::Workgroup {
            return Err("target_out_of_scope".to_string());
        }
        let target = resolve_verified_wg_target(target_fqn, project_paths)?;
        return Ok(VerifiedTerminalSnapshotRoute {
            sender,
            target,
            kind: TerminalSnapshotAuthorityKind::Root,
        });
    }

    // Prove the Coordinator before any target identity walk. This preserves the
    // no-oracle ordering for workers and origin senders.
    let sender = verify_pty_input_coordinator_root(sender_cwd)?;
    if syntax != TerminalSnapshotTargetSyntax::Workgroup {
        return Err("target_out_of_scope".to_string());
    }
    let target = resolve_verified_wg_target(target_fqn, project_paths)?;
    if sender.project != target.project
        || sender.workgroup != target.workgroup
        || target.is_coordinator
        || sender.canonical_fqn == target.canonical_fqn
    {
        return Err("target_out_of_scope".to_string());
    }
    Ok(VerifiedTerminalSnapshotRoute {
        sender,
        target,
        kind: TerminalSnapshotAuthorityKind::Coordinator,
    })
}
