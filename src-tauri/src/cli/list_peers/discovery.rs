use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use super::status::{build_session_index, compute_peer_status, CandidateSession};
use super::{extract_role_section, read_role, PeerInfo};
use crate::config::ac_root::existing_ac_root;
use crate::config::agent_config::read_agent_local_config;

pub(super) struct WgReplicaInfo {
    pub(super) my_agent_name: String,
    pub(super) my_wg_name: String,
    pub(super) my_wg_dir: PathBuf,
    pub(super) ac_root: PathBuf,
    /// Project folder name (the dir containing the Project AC Root). Forms the
    /// LHS of the canonical FQN for WG replicas.
    pub(super) my_project: String,
}

/// Detect if `root` is a WG replica: path matches `*/<project_ac_root>/wg-*/__agent_*/`.
///
/// Thin wrapper over the shared walk-up in
/// `config::ac_root::wg_replica_layout_from_agent_dir` (single source with
/// `send::derive_root_project_dir` and `mailbox::derive_project_from_outbox_path`,
/// see #726). This caller keeps the project FOLDER NAME (`my_project`), the LHS
/// of a WG replica's canonical FQN, rather than the full project path.
pub(super) fn detect_wg_replica(root: &str) -> Result<Option<WgReplicaInfo>, String> {
    let canon = match crate::filesystem::directories::canonical_cli_discovery_path(Path::new(root))
    {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let Some(layout) = crate::config::ac_root::wg_replica_layout_from_agent_dir(&canon)? else {
        return Ok(None);
    };
    let Some(my_project) = layout
        .project_dir
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return Ok(None);
    };

    Ok(Some(WgReplicaInfo {
        my_agent_name: layout.agent_name,
        my_wg_name: layout.wg_name,
        my_wg_dir: layout.wg_dir,
        ac_root: layout.ac_root,
        my_project: my_project.to_string(),
    }))
}

/// Resolve the coordinator agent name for a WG by matching replica identity
/// paths against the team coordinator path in Project AC Root `_team_*/config.json`.
/// Only checks the team whose name matches the WG suffix (e.g. `wg-1-ac-devs` → `_team_ac-devs`).
pub(super) fn resolve_wg_coordinator(ac_root: &Path, wg_dir: &Path) -> Option<String> {
    crate::config::teams::resolve_wg_coordinator_replica(ac_root, wg_dir)
        .map(|replica| replica.agent_name)
}

/// Read role from a WG replica's identity matrix Role.md, falling back to CLAUDE.md.
pub(super) fn read_wg_role(replica_dir: &Path) -> String {
    let matrix_dir = match crate::config::replica_identity::read_and_repair_wg_replica_config(
        replica_dir,
        crate::config::replica_identity::WG_REPLICA_REQUIRED_CONTEXT,
    ) {
        Ok((_config, identity)) => identity.matrix_dir,
        Err(e) => {
            log::warn!(
                "[list-peers] Rejected invalid WG replica identity for '{}': {}",
                replica_dir.display(),
                e
            );
            return "Room replica agent.".to_string();
        }
    };
    let role_path = matrix_dir.join("Role.md");
    match crate::filesystem::documents::read_repository_role(&role_path) {
        Ok(content) => extract_role_section(&content, 3, "Room replica agent."),
        Err(_) => read_role(&matrix_dir.to_string_lossy()),
    }
}

/// Build a PeerInfo for a WG replica directory. Also bootstraps IPC dirs.
/// `project` is the project folder name (dir containing the Project AC Root) and forms
/// the LHS of the canonical FQN.
pub(super) fn build_wg_peer(
    project: &str,
    agent_name: &str,
    wg_name: &str,
    agent_path: &Path,
    reachable: bool,
    session_index: &HashMap<String, Vec<CandidateSession>>,
) -> PeerInfo {
    let replica_ac = agent_path.join(crate::config::agent_local_dir_name());
    let _ = crate::filesystem::directories::ensure_cli_peer_mailbox(&replica_ac.join("inbox"));
    let _ = crate::filesystem::directories::ensure_cli_peer_mailbox(&replica_ac.join("outbox"));

    // #2786 (C1) - through the shared loader: state file first, key-wise.
    let peer_config = read_agent_local_config(&replica_ac).unwrap_or_default();

    let expected_session_name = format!("{}/{}", wg_name, agent_name);
    let ps = compute_peer_status(
        &agent_path.to_string_lossy(),
        Some(&expected_session_name),
        session_index,
    );

    PeerInfo {
        name: format!("{}:{}/{}", project, wg_name, agent_name),
        path: agent_path.to_string_lossy().to_string(),
        status: ps.status_legacy.to_string(),
        role: read_wg_role(agent_path),
        teams: vec![wg_name.to_string()],
        reachable,
        last_coding_agent: peer_config.tooling.last_coding_agent,
        working: ps.working,
        session_status: ps.session_status.to_string(),
        session_id: ps.session_id,
        waiting_for_input: ps.waiting_for_input,
        blocked_menu: ps.blocked_menu,
        blocked_menu_message: ps.blocked_menu_message,
        exit_code: ps.exit_code,
        context_percent: ps.context_percent,
        agent: ps.agent_id,
        profile: ps.effective_profile,
        coding_agents: peer_config.tooling.coding_agents,
    }
}

/// Pure (filesystem-bound) discovery for WG replicas. Returns the same vector
/// that the legacy `execute_wg_discovery` would have serialized. Shared by
/// `execute` and `execute_lean` so the peer set is identical by construction
/// for both verbs (see issue #252).
pub(super) fn discover_wg_peers(wg: WgReplicaInfo, project_paths: &[String]) -> Vec<PeerInfo> {
    let session_index = build_session_index();
    let mut peers: Vec<PeerInfo> = Vec::new();
    let discovered = crate::config::teams::discover_teams_from_project_paths(project_paths);
    // Canonical FQN: `<project>:<wg>/<agent>`. All downstream routing checks
    // (`can_communicate`) compare project-qualified strings.
    let my_full_name = format!("{}:{}/{}", wg.my_project, wg.my_wg_name, wg.my_agent_name);

    let coordinator = resolve_wg_coordinator(&wg.ac_root, &wg.my_wg_dir);
    let i_am_coordinator = coordinator.as_deref() == Some(wg.my_agent_name.as_str());

    if coordinator.is_none() {
        if crate::config::environment::machine_output_string().is_err() {
            eprintln!(
                "Warning: no orchestrator found for Room '{}', showing all replicas",
                wg.my_wg_name
            );
        } else {
            log::warn!(
                "Warning: no orchestrator found for Room '{}', showing all replicas",
                wg.my_wg_name
            );
        }
    }

    // Collect all replicas in my WG
    let mut replicas: Vec<(String, PathBuf)> = Vec::new();
    let _ = crate::filesystem::directories::visit_cli_discovery_entries(&wg.my_wg_dir, |entry| {
        let Ok((p, _)) = entry else {
            return ControlFlow::<()>::Continue(());
        };
        if !crate::filesystem::directories::cli_discovery_is_dir(&p) {
            return ControlFlow::Continue(());
        }
        let agent = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("__agent_"))
            .map(str::to_string);
        if let Some(agent) = agent {
            replicas.push((agent, p));
        }
        ControlFlow::Continue(())
    });

    for (agent_name, agent_path) in &replicas {
        if *agent_name == wg.my_agent_name {
            continue;
        }
        let peer_full_name = format!("{}:{}/{}", wg.my_project, wg.my_wg_name, agent_name);
        let reachable =
            crate::config::teams::can_communicate(&my_full_name, &peer_full_name, &discovered);
        peers.push(build_wg_peer(
            &wg.my_project,
            agent_name,
            &wg.my_wg_name,
            agent_path,
            reachable,
            &session_index,
        ));
    }

    // Coordinator also sees coordinators of other WGs in the same Project AC Root
    // (same project, different WG — still qualified with `wg.my_project`).
    if i_am_coordinator {
        let _ = crate::filesystem::directories::visit_cli_discovery_entries(&wg.ac_root, |entry| {
            let Ok((other_wg_dir, _)) = entry else {
                return ControlFlow::<()>::Continue(());
            };
            if !crate::filesystem::directories::cli_discovery_is_dir(&other_wg_dir) {
                return ControlFlow::Continue(());
            }
            let other_wg_name = match other_wg_dir.file_name().and_then(|n| n.to_str()) {
                Some(n)
                    if crate::config::entity_prefix::has_entity_prefix(n) && n != wg.my_wg_name =>
                {
                    n.to_string()
                }
                _ => return ControlFlow::Continue(()),
            };

            if let Some(other_coord) = resolve_wg_coordinator(&wg.ac_root, &other_wg_dir) {
                let coord_dir = other_wg_dir.join(format!("__agent_{}", other_coord));
                if !crate::filesystem::directories::cli_discovery_is_dir(&coord_dir) {
                    return ControlFlow::Continue(());
                }
                let peer_name = format!("{}:{}/{}", wg.my_project, other_wg_name, other_coord);
                if peers.iter().any(|p| p.name == peer_name) {
                    return ControlFlow::Continue(());
                }
                let reachable =
                    crate::config::teams::can_communicate(&my_full_name, &peer_name, &discovered);
                peers.push(build_wg_peer(
                    &wg.my_project,
                    &other_coord,
                    &other_wg_name,
                    &coord_dir,
                    reachable,
                    &session_index,
                ));
            }

            ControlFlow::Continue(())
        });
    }

    // #293 — synthetic Root Agent reply peer (identity-verified coordinators
    // only). Augment the captured runtime project paths with the caller's own project
    // dir so `verified_wg_coordinator_target` sees the replica even when the
    // project isn't in settings. Mirrors `cli/send.rs::effective_project_paths`
    // (send.rs `derive_root_project_dir` augmentation) and the mailbox
    // root-recipient arm's `derive_project_from_outbox_path` augmentation.
    let mut effective_paths = project_paths.to_vec();
    if let Some(project_dir) = wg.ac_root.parent() {
        if let Some(project_str) = project_dir.to_str() {
            let p = project_str.to_string();
            let canon_p =
                crate::filesystem::projects::canonical_refresh_project(Path::new(&p)).ok();
            let already_present = effective_paths.iter().any(|x| match &canon_p {
                Some(c) => {
                    crate::filesystem::projects::canonical_refresh_project(Path::new(x))
                        .ok()
                        .as_ref()
                        == Some(c)
                }
                None => x == &p,
            });
            if !already_present {
                effective_paths.push(p);
            }
        }
    }
    if let Some(root_peer) = build_root_agent_synthetic_peer(&wg, &effective_paths) {
        peers.push(root_peer);
    }

    peers
}

pub(super) fn append_origin_team_peers(
    my_name: &str,
    discovered: &[crate::config::teams::DiscoveredTeam],
    session_index: &HashMap<String, Vec<CandidateSession>>,
    peers: &mut Vec<PeerInfo>,
) {
    // Find teams where I'm a member, then list their other members.
    // Also: if I'm a coordinator, show other coordinators (cross-team).
    let i_am_coordinator = discovered.iter().any(|t| {
        crate::config::teams::is_in_team(my_name, t)
            && t.coordinator_name
                .as_deref()
                .is_some_and(|cn| cn == my_name)
    });

    for team in discovered {
        let i_am_in_team = crate::config::teams::is_in_team(my_name, team);

        if !i_am_in_team && !i_am_coordinator {
            continue;
        }

        for (i, display_name) in team.agent_names.iter().enumerate() {
            let member_path = team.agent_paths.get(i).and_then(|p| p.as_ref());
            let peer_name = member_path
                .map(|p| crate::config::teams::agent_fqn_from_path(&p.to_string_lossy()))
                .unwrap_or_else(|| display_name.clone());

            // Skip ourselves
            if peer_name == my_name || display_name == my_name {
                continue;
            }

            // Determine reachability using the canonical routing rules
            let reachable = crate::config::teams::can_communicate(my_name, &peer_name, discovered);

            // Skip duplicates — add team to existing peer, upgrade reachable if needed
            if let Some(existing) = peers.iter_mut().find(|p| p.name == peer_name) {
                if !existing.teams.contains(&team.name) {
                    existing.teams.push(team.name.clone());
                }
                // If reachable via any team, mark as reachable
                if reachable {
                    existing.reachable = true;
                }
                continue;
            }

            let path_str = member_path
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let peer_ac = member_path
                .map(|p| p.join(crate::config::agent_local_dir_name()))
                .unwrap_or_else(|| {
                    PathBuf::from(&path_str).join(crate::config::agent_local_dir_name())
                });

            let peer_config = read_agent_local_config(&peer_ac).unwrap_or_default();

            let ps = compute_peer_status(&path_str, None, session_index);
            peers.push(PeerInfo {
                name: peer_name,
                path: path_str,
                status: ps.status_legacy.to_string(),
                role: member_path
                    .map(|p| read_role(&p.to_string_lossy()))
                    .unwrap_or_else(|| "No role description available.".to_string()),
                teams: vec![team.name.clone()],
                reachable,
                last_coding_agent: peer_config.tooling.last_coding_agent,
                working: ps.working,
                session_status: ps.session_status.to_string(),
                session_id: ps.session_id,
                waiting_for_input: ps.waiting_for_input,
                blocked_menu: ps.blocked_menu,
                blocked_menu_message: ps.blocked_menu_message,
                exit_code: ps.exit_code,
                context_percent: ps.context_percent,
                agent: ps.agent_id,
                profile: ps.effective_profile,
                coding_agents: peer_config.tooling.coding_agents,
            });
        }
    }
}

/// Discovery for non-WG-replica roots: standard team membership scan + a
/// WG-replica scan across captured registered projects. Returns the same vector
/// that the legacy `execute` body would have serialized. Shared by `execute`
/// and `execute_lean` so the peer set is identical by construction for both
/// verbs (see issue #252).
pub(super) fn discover_origin_peers(root: &str, project_paths: &[String]) -> Vec<PeerInfo> {
    // ── Standard discovery-based peer listing ────────────────────────
    //
    // `execute` is the non-WG-replica path (WG replicas return early above).
    // `root` is an origin matrix agent CWD → `agent_fqn_from_path` gives the
    // origin form `project/agent` (identical to the legacy behavior for
    // non-WG paths). Using the canonical helper eliminates the shadow.
    let my_name = crate::config::teams::agent_fqn_from_path(root);
    let discovered = crate::config::teams::discover_teams_from_project_paths(project_paths);
    let session_index = build_session_index();

    let mut peers: Vec<PeerInfo> = Vec::new();

    append_origin_team_peers(&my_name, &discovered, &session_index, &mut peers);

    // ── WG replica discovery ──────────────────────────────────────────────
    // Scan project_paths for Project AC Root wg-*/__agent_* replicas
    for base_path in project_paths {
        let base = Path::new(base_path);
        if !crate::filesystem::projects::project_path_is_dir(base) {
            continue;
        }
        // Check base and its immediate children (same pattern as ac_discovery)
        let mut dirs_to_check = vec![base.to_path_buf()];
        crate::filesystem::projects::visit_discovery_project_children(base, |p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') {
                dirs_to_check.push(p);
            }
        });
        for repo_dir in dirs_to_check {
            let Some(ac_root) = existing_ac_root(&repo_dir) else {
                continue;
            };
            // Project folder name (parent of Project AC Root) is the LHS of the canonical FQN.
            let project_folder = match repo_dir.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let _ =
                crate::filesystem::directories::visit_cli_discovery_entries(&ac_root, |entry| {
                    let Ok((wg_path, _)) = entry else {
                        return ControlFlow::<()>::Continue(());
                    };
                    if !crate::filesystem::directories::cli_discovery_is_dir(&wg_path) {
                        return ControlFlow::Continue(());
                    }
                    let wg_name = match wg_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) if crate::config::entity_prefix::has_entity_prefix(n) => {
                            n.to_string()
                        }
                        _ => return ControlFlow::Continue(()),
                    };
                    // Derive team name from the entity name: "room-1-ac-devs" or
                    // the legacy "wg-1-ac-devs" → "ac-devs"
                    let wg_team = crate::config::entity_prefix::strip_entity_prefix(&wg_name)
                        .and_then(|s| s.split_once('-').map(|(_, rest)| rest))
                        .unwrap_or(&wg_name)
                        .to_string();

                    let _ = crate::filesystem::directories::visit_cli_discovery_entries(
                        &wg_path,
                        |entry| {
                            let Ok((agent_path, _)) = entry else {
                                return ControlFlow::<()>::Continue(());
                            };
                            if !crate::filesystem::directories::cli_discovery_is_dir(&agent_path) {
                                return ControlFlow::Continue(());
                            }
                            let agent_dir = match agent_path.file_name().and_then(|n| n.to_str()) {
                                Some(n) if n.starts_with("__agent_") => n.to_string(),
                                _ => return ControlFlow::Continue(()),
                            };
                            let agent_short = agent_dir
                                .strip_prefix("__agent_")
                                .unwrap_or(&agent_dir)
                                .to_string();
                            let peer_name =
                                format!("{}:{}/{}", project_folder, wg_name, agent_short);

                            // Skip self
                            if peer_name == my_name {
                                return ControlFlow::Continue(());
                            }
                            // Skip duplicates
                            if peers.iter().any(|p| p.name == peer_name) {
                                return ControlFlow::Continue(());
                            }

                            let reachable = crate::config::teams::can_communicate(
                                &my_name,
                                &peer_name,
                                &discovered,
                            );
                            let mut peer = build_wg_peer(
                                &project_folder,
                                &agent_short,
                                &wg_name,
                                &agent_path,
                                reachable,
                                &session_index,
                            );
                            peer.teams = vec![wg_team.clone()];
                            peers.push(peer);

                            ControlFlow::Continue(())
                        },
                    );

                    ControlFlow::Continue(())
                });
        }
    }

    peers
}

pub(super) fn discover_root_coordinator_peers(project_paths: &[String]) -> Vec<PeerInfo> {
    discover_root_coordinator_peers_from_project_paths(project_paths)
}

pub(super) fn discover_root_coordinator_peers_from_project_paths(
    project_paths: &[String],
) -> Vec<PeerInfo> {
    let session_index = build_session_index();
    let mut peers: Vec<PeerInfo> = Vec::new();

    for base_path in project_paths {
        let base = Path::new(base_path);
        if !crate::filesystem::projects::project_path_is_dir(base) {
            continue;
        }

        let mut dirs_to_check = vec![base.to_path_buf()];
        crate::filesystem::projects::visit_discovery_project_children(base, |p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') {
                dirs_to_check.push(p);
            }
        });

        for repo_dir in dirs_to_check {
            let Some(ac_root) = existing_ac_root(&repo_dir) else {
                continue;
            };

            let _ =
                crate::filesystem::directories::visit_cli_discovery_entries(&ac_root, |entry| {
                    let Ok((wg_path, _)) = entry else {
                        return ControlFlow::<()>::Continue(());
                    };
                    if !crate::filesystem::directories::cli_discovery_is_dir(&wg_path) {
                        return ControlFlow::Continue(());
                    }
                    match wg_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) if crate::config::entity_prefix::has_entity_prefix(n) => {}
                        _ => return ControlFlow::Continue(()),
                    }

                    let Some(coord) =
                        crate::config::teams::resolve_wg_coordinator_replica(&ac_root, &wg_path)
                    else {
                        return ControlFlow::Continue(());
                    };
                    let peer_name =
                        format!("{}:{}/{}", coord.project, coord.wg_name, coord.agent_name);
                    if peers.iter().any(|p| p.name == peer_name) {
                        return ControlFlow::Continue(());
                    }
                    let mut peer = build_wg_peer(
                        &coord.project,
                        &coord.agent_name,
                        &coord.wg_name,
                        &coord.replica_dir,
                        true,
                        &session_index,
                    );
                    peer.teams = vec![coord.team];
                    peers.push(peer);

                    ControlFlow::Continue(())
                });
        }
    }

    peers
}

/// Build a synthetic `PeerInfo` for the canonical Root Agent reply target,
/// or `None` if the caller is not an identity-verified WG coordinator (the
/// only relationship that can address the Root Agent — see #293 §3).
///
/// `wg` is the caller's WG-replica detection result (already computed by
/// `discover_peers`); `project_paths` is the effective settings slice the
/// caller scanned.
pub(super) fn build_root_agent_synthetic_peer(
    wg: &WgReplicaInfo,
    project_paths: &[String],
) -> Option<PeerInfo> {
    // The caller is a coordinator iff `verified_wg_coordinator_target`
    // accepts its own FQN. Re-uses the same identity-cross-reference path
    // (#277 §2.3) used by the root-sender side.
    let my_fqn = format!("{}:{}/{}", wg.my_project, wg.my_wg_name, wg.my_agent_name);
    crate::config::teams::verified_wg_coordinator_target(&my_fqn, project_paths)?;

    let root_dir = crate::config::root_agent::root_agent_dir().ok()?;
    Some(PeerInfo {
        name: crate::config::root_agent::ROOT_AGENT_SENDER.to_string(),
        path: root_dir,
        // `status`/`session_status` reflect "we don't track it from here".
        // The root session is in a different working-directory namespace
        // than this verb scans — its working state is invisible to
        // `compute_peer_status`.
        status: "unknown".to_string(),
        role: "AgentsCommander Root Agent (canonical reply target).".to_string(),
        teams: Vec::new(),
        reachable: true,
        last_coding_agent: None,
        working: false,
        session_status: "none".to_string(),
        session_id: None,
        waiting_for_input: false,
        // Synthetic root-agent peer: no session in this verb's cwd namespace,
        // so it can never be parked on a blocking terminal menu.
        blocked_menu: false,
        blocked_menu_message: None,
        exit_code: None,
        // Synthetic root-agent peer: no session in this verb's cwd namespace.
        context_percent: None,
        agent: None,
        profile: None,
        coding_agents: std::collections::HashMap::new(),
    })
}

/// Apply the `--peer` filter to a discovered peer list.
///
/// Consumes `peers` by value so filtering does not require `PeerInfo: Clone`.
pub(super) fn apply_peer_filter(
    peers: Vec<PeerInfo>,
    requested: &[String],
) -> Result<Vec<PeerInfo>, Vec<String>> {
    if requested.is_empty() {
        return Ok(peers);
    }

    // Dedupe `requested` while preserving the user's order for unique entries.
    let mut unique: Vec<String> = Vec::with_capacity(requested.len());
    for name in requested {
        if !unique.iter().any(|n| n == name) {
            unique.push(name.clone());
        }
    }

    // Move peers into a name→PeerInfo map so emission is O(1) per requested
    // name and we don't need PeerInfo: Clone. Discovery never produces
    // duplicate names (see `discover_origin_peers` / `discover_wg_peers`),
    // so this collapse is lossless.
    let mut by_name: HashMap<String, PeerInfo> =
        peers.into_iter().map(|p| (p.name.clone(), p)).collect();

    let unknown: Vec<String> = unique
        .iter()
        .filter(|n| !by_name.contains_key(n.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(unknown);
    }

    let filtered: Vec<PeerInfo> = unique
        .into_iter()
        .filter_map(|name| by_name.remove(&name))
        .collect();
    Ok(filtered)
}

/// Emit the unknown-peer error to stderr and return exit code 1. Used by
/// both `execute` and `execute_lean` so the message shape is identical.
pub(super) fn report_unknown_peers(unknown: &[String], available: &[String]) -> i32 {
    eprintln!(
        "Error: unknown peer(s) requested via --peer: {}",
        unknown.join(", ")
    );
    if available.is_empty() {
        eprintln!("No peers were discovered for this --root.");
    } else {
        eprintln!("Available peers: {}", available.join(", "));
    }
    1
}

/// Run the shared discovery — WG-replica path when `root` is a replica, the
/// origin-agent path otherwise. Factored so `execute` and `execute_lean`
/// share the dispatch.
pub(super) fn discover_peers(root: &str) -> Result<Vec<PeerInfo>, String> {
    let settings = crate::config::settings::load_settings_for_cli_strict()?;
    if crate::config::root_agent::is_root_agent_path(root) {
        Ok(discover_root_coordinator_peers(&settings.project_paths))
    } else if let Some(wg) = detect_wg_replica(root)? {
        Ok(discover_wg_peers(wg, &settings.project_paths))
    } else {
        Ok(discover_origin_peers(root, &settings.project_paths))
    }
}
