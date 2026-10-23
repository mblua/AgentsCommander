use std::collections::HashMap;
use std::path::Path;

use super::discovery::detect_wg_replica;
use super::LeanPeerInfo;

pub(super) fn snapshot_target_projection(
    target: crate::config::teams::TerminalSnapshotTargetIdentity,
    reachable: bool,
) -> LeanPeerInfo {
    LeanPeerInfo {
        name: target.canonical_fqn,
        working: false,
        session_status: "unknown".to_string(),
        waiting_for_input: false,
        blocked_menu: false,
        blocked_menu_message: None,
        reachable,
        teams: vec![target.team],
        role_summary: String::new(),
        context_percent: None,
        agent: None,
        profile: None,
    }
}

/// Why `--snapshot-targets` returned `[]` without scanning. The vocabulary is
/// closed and compile-time: no path, name or FQN can reach it (#2228).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SnapshotTargetsSilentReason {
    NotRoomOrchestrator,
    NotRoomReplica,
}

impl SnapshotTargetsSilentReason {
    const fn as_str(self) -> &'static str {
        match self {
            SnapshotTargetsSilentReason::NotRoomOrchestrator => "requester_not_room_orchestrator",
            SnapshotTargetsSilentReason::NotRoomReplica => "requester_not_room_replica",
        }
    }
}

/// Tier B: integers only. Never a path, a name, an FQN or a rule name, because
/// every one of these candidates was found by scanning the caller's neighbours
/// rather than supplied by the caller (#2228).
pub(super) fn snapshot_targets_counts_note(
    skips: crate::config::teams::SnapshotDiscoverySkips,
) -> String {
    format!(
        "snapshot_targets_note skipped_project_children={} skipped_rooms={} skipped_replicas={}",
        skips.project_children, skips.rooms, skips.replicas
    )
}

pub(super) fn snapshot_targets_reason_note(reason: SnapshotTargetsSilentReason) -> String {
    format!("snapshot_targets_note reason={}", reason.as_str())
}

/// Tier A: the caller supplied `--root`, so naming the rule that rejected it
/// discloses nothing the caller does not already own (#2228).
pub(super) fn verify_snapshot_root(
    root_path: &Path,
) -> Result<crate::path_identity::VerifiedPathIdentity, String> {
    crate::path_identity::verify_directory_reason(root_path)
        .map_err(|rule| format!("unsafe_path field=root reason={}", rule.as_str()))
}

pub(super) fn discover_snapshot_targets(
    root: &str,
) -> Result<
    (
        Vec<LeanPeerInfo>,
        crate::config::teams::SnapshotDiscoverySkips,
    ),
    String,
> {
    let root_path = Path::new(root);
    if crate::config::root_agent::is_root_agent_path(root) {
        let expected = crate::config::root_agent::verify_live_root_agent_path(root_path)?;
        let supplied = verify_snapshot_root(root_path)?;
        if !crate::path_identity::same_object(&expected, &supplied) {
            return Err("invalid snapshot discovery root".to_string());
        }
        let project_paths = crate::config::settings::read_terminal_snapshot_project_paths_strict()?;
        let discovered = crate::config::teams::discover_verified_terminal_snapshot_targets_counted(
            &project_paths,
        )?;
        return Ok((
            discovered
                .targets
                .into_iter()
                .map(|target| {
                    let reachable = target.is_coordinator;
                    snapshot_target_projection(target, reachable)
                })
                .collect(),
            discovered.skips,
        ));
    }

    let Some(wg) = detect_wg_replica(root)? else {
        eprintln!(
            "{}",
            snapshot_targets_reason_note(SnapshotTargetsSilentReason::NotRoomReplica)
        );
        return Ok((Vec::new(), Default::default()));
    };
    let requester = crate::config::teams::verify_pty_input_replica_cwd(root_path)?;
    let supplied = verify_snapshot_root(root_path)?;
    if !crate::path_identity::same_object(&requester.replica_identity, &supplied) {
        return Err("invalid snapshot discovery root".to_string());
    }
    if !requester.is_coordinator {
        eprintln!(
            "{}",
            snapshot_targets_reason_note(SnapshotTargetsSilentReason::NotRoomOrchestrator)
        );
        return Ok((Vec::new(), Default::default()));
    }
    let project_dir = wg
        .ac_root
        .parent()
        .ok_or_else(|| "snapshot project path unavailable".to_string())?
        .to_string_lossy()
        .to_string();
    let discovered =
        crate::config::teams::discover_verified_terminal_snapshot_targets_counted(&[project_dir])?;
    Ok((
        discovered
            .targets
            .into_iter()
            .filter(|target| {
                target.project == requester.project
                    && target.workgroup == requester.workgroup
                    && !target.is_coordinator
            })
            .map(|target| snapshot_target_projection(target, true))
            .collect(),
        discovered.skips,
    ))
}

pub(super) fn apply_snapshot_target_filter(
    peers: Vec<LeanPeerInfo>,
    requested: &[String],
) -> Result<Vec<LeanPeerInfo>, Vec<String>> {
    if requested.is_empty() {
        return Ok(peers);
    }
    let mut by_name: HashMap<String, LeanPeerInfo> = peers
        .into_iter()
        .map(|peer| (peer.name.clone(), peer))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut filtered = Vec::new();
    let mut unknown = Vec::new();
    for name in requested {
        if !seen.insert(name.clone()) {
            continue;
        }
        match by_name.remove(name) {
            Some(peer) => filtered.push(peer),
            None => unknown.push(name.clone()),
        }
    }
    if unknown.is_empty() {
        Ok(filtered)
    } else {
        Err(unknown)
    }
}

pub(super) fn serialize_snapshot_targets(peers: &[LeanPeerInfo]) -> i32 {
    match serde_json::to_string_pretty(peers) {
        Ok(json) => {
            crate::cli_println!("{}", json);
            0
        }
        Err(_) => {
            eprintln!("Error: failed to serialize terminal snapshot targets");
            1
        }
    }
}
