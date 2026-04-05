use clap::Args;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::dark_factory::{AgentLocalConfig, CodingAgentEntry};

#[derive(Args)]
#[command(after_help = "\
OUTPUT: JSON array of reachable peers. Each entry contains:\n  \
  name              Agent name to use with `send --to` (e.g., \"repos/my-project\")\n  \
  path              Full filesystem path to the agent's root directory\n  \
  status            \"active\" if the agent has a running session, \"unknown\" otherwise\n  \
  role              Summary extracted from the agent's CLAUDE.md\n  \
  teams             List of shared team names\n  \
  lastCodingAgent   Last coding CLI used (e.g., \"claude\", \"codex\"), if known\n\n\
Only agents that share a team with you are listed. If you have no teams, the result is an empty array.")]
pub struct ListPeersArgs {
    /// Session token for authentication (from '# === Session Credentials ===' block)
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory — used to identify you and your teams
    #[arg(long)]
    pub root: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerInfo {
    name: String,
    path: String,
    status: String,
    role: String,
    teams: Vec<String>,
    last_coding_agent: Option<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    coding_agents: HashMap<String, CodingAgentEntry>,
}

/// Strip `__agent_` and `_agent_` prefixes from agent directory names.
fn strip_agent_prefix(name: &str) -> &str {
    name.strip_prefix("__agent_")
        .or_else(|| name.strip_prefix("_agent_"))
        .unwrap_or(name)
}

/// Get the agent name (parent/repo) from a path, stripping agent prefixes.
fn agent_name_from_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if components.len() >= 2 {
        let parent = components[components.len() - 2];
        let last = strip_agent_prefix(components[components.len() - 1]);
        format!("{}/{}", parent, last)
    } else {
        normalized
    }
}

/// Extract a role description from markdown content: finds the `## Role` section
/// and returns up to 3 lines. Falls back to the first `fallback_lines` non-heading lines.
fn extract_role_section(content: &str, fallback_lines: usize, default_msg: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut in_role = false;
    let mut role_lines: Vec<&str> = Vec::new();

    for line in &lines {
        if line.starts_with("## Role Prompt") || line.starts_with("## Role") {
            in_role = true;
            continue;
        }
        if in_role {
            if line.starts_with("## ") || line.starts_with("---") {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                role_lines.push(trimmed);
            }
        }
    }

    if !role_lines.is_empty() {
        return role_lines.into_iter().take(3).collect::<Vec<_>>().join(" ");
    }

    let fallback: Vec<&str> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .take(fallback_lines)
        .collect();

    if fallback.is_empty() {
        default_msg.to_string()
    } else {
        fallback.join(" ")
    }
}

/// Read role from CLAUDE.md: extract ## Role section, or first 5 lines.
fn read_role(repo_path: &str) -> String {
    let claude_md = Path::new(repo_path).join("CLAUDE.md");
    match std::fs::read_to_string(&claude_md) {
        Ok(content) => extract_role_section(&content, 5, "No role description available."),
        Err(_) => "No role description available.".to_string(),
    }
}

/// Canonicalize a path, stripping `\\?\` UNC prefix on Windows.
fn canon_str(path: &Path) -> Option<String> {
    let canon = std::fs::canonicalize(path).ok()?;
    let s = canon.to_string_lossy().to_string();
    Some(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

struct WgReplicaInfo {
    my_agent_name: String,
    my_wg_name: String,
    my_wg_dir: PathBuf,
    ac_new_dir: PathBuf,
}

/// Detect if `root` is a WG replica: path matches `*/.ac-new/wg-*/__agent_*/`.
fn detect_wg_replica(root: &str) -> Option<WgReplicaInfo> {
    let path = PathBuf::from(root);
    let canon = match std::fs::canonicalize(&path) {
        Ok(c) => c,
        Err(_) => return None,
    };

    let my_dir_name = canon.file_name()?.to_str()?;
    if !my_dir_name.starts_with("__agent_") {
        return None;
    }
    let my_agent_name = my_dir_name.strip_prefix("__agent_")?.to_string();

    let wg_dir = canon.parent()?;
    let wg_name = wg_dir.file_name()?.to_str()?;
    if !wg_name.starts_with("wg-") {
        return None;
    }

    let ac_new_dir = wg_dir.parent()?;
    let ac_new_name = ac_new_dir.file_name()?.to_str()?;
    if ac_new_name != ".ac-new" {
        return None;
    }

    Some(WgReplicaInfo {
        my_agent_name,
        my_wg_name: wg_name.to_string(),
        my_wg_dir: wg_dir.to_path_buf(),
        ac_new_dir: ac_new_dir.to_path_buf(),
    })
}

/// Resolve the coordinator agent name for a WG by matching replica identity
/// paths against the team coordinator path in `.ac-new/_team_*/config.json`.
/// Only checks the team whose name matches the WG suffix (e.g. `wg-1-ac-devs` → `_team_ac-devs`).
fn resolve_wg_coordinator(ac_new_dir: &Path, wg_dir: &Path) -> Option<String> {
    // Derive expected team dir from WG name: "wg-1-ac-devs" → "_team_ac-devs"
    let wg_name = wg_dir.file_name()?.to_str()?;
    let team_suffix = wg_name
        .strip_prefix("wg-")
        .and_then(|s| s.split_once('-').map(|(_, rest)| rest))?;
    let expected_team_dir = format!("_team_{}", team_suffix);

    let entries = match std::fs::read_dir(ac_new_dir) {
        Ok(e) => e,
        Err(_) => return None,
    };

    for entry in entries.flatten() {
        let team_dir = entry.path();
        if !team_dir.is_dir() {
            continue;
        }
        match team_dir.file_name().and_then(|n| n.to_str()) {
            Some(n) if n == expected_team_dir => {}
            _ => continue,
        }

        let team_config: serde_json::Value =
            match std::fs::read_to_string(team_dir.join("config.json"))
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
            {
                Some(v) => v,
                None => continue,
            };

        let coordinator_ref = match team_config.get("coordinator").and_then(|c| c.as_str()) {
            Some(c) => c.to_string(),
            None => continue,
        };

        let coordinator_abs = match canon_str(&team_dir.join(&coordinator_ref)) {
            Some(s) => s,
            None => continue,
        };

        // Check each replica in the WG for identity match
        let replica_entries = match std::fs::read_dir(wg_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for replica_entry in replica_entries.flatten() {
            let replica_dir = replica_entry.path();
            if !replica_dir.is_dir() {
                continue;
            }
            let dir_name = match replica_dir.file_name().and_then(|n| n.to_str()) {
                Some(n) if n.starts_with("__agent_") => n,
                _ => continue,
            };

            let config: serde_json::Value =
                match std::fs::read_to_string(replica_dir.join("config.json"))
                    .ok()
                    .and_then(|c| serde_json::from_str(&c).ok())
                {
                    Some(v) => v,
                    None => continue,
                };

            let identity_ref = match config.get("identity").and_then(|i| i.as_str()) {
                Some(i) => i.to_string(),
                None => continue,
            };

            let identity_abs = match canon_str(&replica_dir.join(&identity_ref)) {
                Some(s) => s,
                None => continue,
            };

            if identity_abs == coordinator_abs {
                return Some(
                    dir_name
                        .strip_prefix("__agent_")
                        .unwrap_or(dir_name)
                        .to_string(),
                );
            }
        }
    }

    None
}

/// Read role from a WG replica's identity matrix Role.md, falling back to CLAUDE.md.
fn read_wg_role(replica_dir: &Path) -> String {
    let config: serde_json::Value = match std::fs::read_to_string(replica_dir.join("config.json"))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
    {
        Some(v) => v,
        None => return "WG replica agent.".to_string(),
    };

    let identity_ref = match config.get("identity").and_then(|i| i.as_str()) {
        Some(i) => i,
        None => return "WG replica agent.".to_string(),
    };

    let matrix_dir = replica_dir.join(identity_ref);
    let role_path = matrix_dir.join("Role.md");
    match std::fs::read_to_string(&role_path) {
        Ok(content) => extract_role_section(&content, 3, "WG replica agent."),
        Err(_) => read_role(&matrix_dir.to_string_lossy()),
    }
}

/// Build a PeerInfo for a WG replica directory. Also bootstraps IPC dirs.
fn build_wg_peer(agent_name: &str, wg_name: &str, agent_path: &Path) -> PeerInfo {
    let replica_ac = agent_path.join(crate::config::agent_local_dir_name());
    let _ = std::fs::create_dir_all(replica_ac.join("inbox"));
    let _ = std::fs::create_dir_all(replica_ac.join("outbox"));

    let status = if replica_ac.join("active").exists() {
        "active"
    } else {
        "unknown"
    };

    let peer_config: AgentLocalConfig = replica_ac
        .join("config.json")
        .to_str()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();

    PeerInfo {
        name: format!("{}/{}", wg_name, agent_name),
        path: agent_path.to_string_lossy().to_string(),
        status: status.to_string(),
        role: read_wg_role(agent_path),
        teams: vec![wg_name.to_string()],
        last_coding_agent: peer_config.tooling.last_coding_agent,
        coding_agents: peer_config.tooling.coding_agents,
    }
}

/// WG-specific peer discovery — self-contained, returns exit code.
fn execute_wg_discovery(wg: WgReplicaInfo) -> i32 {
    let mut peers: Vec<PeerInfo> = Vec::new();

    let coordinator = resolve_wg_coordinator(&wg.ac_new_dir, &wg.my_wg_dir);
    let i_am_coordinator = coordinator.as_deref() == Some(wg.my_agent_name.as_str());

    if coordinator.is_none() {
        eprintln!(
            "Warning: no coordinator found for WG '{}', showing all replicas",
            wg.my_wg_name
        );
    }

    // Collect all replicas in my WG
    let replicas: Vec<(String, PathBuf)> = std::fs::read_dir(&wg.my_wg_dir)
        .into_iter()
        .flat_map(|rd| rd.flatten())
        .filter_map(|e| {
            let p = e.path();
            if !p.is_dir() {
                return None;
            }
            let name = p.file_name()?.to_str()?;
            let agent = name.strip_prefix("__agent_")?.to_string();
            Some((agent, p))
        })
        .collect();

    for (agent_name, agent_path) in &replicas {
        if *agent_name == wg.my_agent_name {
            continue;
        }
        // Communication rules: non-coordinator sees only coordinator.
        // If coordinator is unknown, fall back to flat topology (show all).
        if coordinator.is_some()
            && !i_am_coordinator
            && coordinator.as_deref() != Some(agent_name.as_str())
        {
            continue;
        }
        peers.push(build_wg_peer(agent_name, &wg.my_wg_name, agent_path));
    }

    // Coordinator also sees coordinators of OTHER WGs in the same .ac-new
    if i_am_coordinator {
        if let Ok(entries) = std::fs::read_dir(&wg.ac_new_dir) {
            for entry in entries.flatten() {
                let other_wg_dir = entry.path();
                if !other_wg_dir.is_dir() {
                    continue;
                }
                let other_wg_name = match other_wg_dir.file_name().and_then(|n| n.to_str()) {
                    Some(n) if n.starts_with("wg-") && n != wg.my_wg_name => n.to_string(),
                    _ => continue,
                };

                if let Some(other_coord) = resolve_wg_coordinator(&wg.ac_new_dir, &other_wg_dir) {
                    let coord_dir = other_wg_dir.join(format!("__agent_{}", other_coord));
                    if !coord_dir.is_dir() {
                        continue;
                    }
                    let peer_name = format!("{}/{}", other_wg_name, other_coord);
                    if peers.iter().any(|p| p.name == peer_name) {
                        continue;
                    }
                    peers.push(build_wg_peer(&other_coord, &other_wg_name, &coord_dir));
                }
            }
        }
    }

    match serde_json::to_string_pretty(&peers) {
        Ok(json) => {
            println!("{}", json);
            0
        }
        Err(e) => {
            eprintln!("Error: failed to serialize peers: {}", e);
            1
        }
    }
}

/// Load the teams.json from the global config directory.
fn load_teams_config() -> Option<serde_json::Value> {
    let teams_path = crate::config::config_dir()?.join("teams.json");
    let content = std::fs::read_to_string(teams_path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn execute(args: ListPeersArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };
    // ── WG replica fast path ──────────────────────────────────────────
    // If we're a WG replica, use dedicated discovery and return early.
    if let Some(wg) = detect_wg_replica(&root) {
        return execute_wg_discovery(wg);
    }

    // ── Standard teams-based discovery ───────────────────────────────
    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());
    let my_name = agent_name_from_path(&root);

    // Read our own config
    let config_path = ac_dir.join("config.json");
    let my_config: AgentLocalConfig = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();

    let mut peers: Vec<PeerInfo> = Vec::new();

    if my_config.dark_factory.teams.is_empty() {
        // No teams → no peers. Only team members can communicate.
        println!("[]");
        return 0;
    }

    // Show all members of our teams
    if let Some(teams_json) = load_teams_config() {
        if let Some(teams) = teams_json.get("teams").and_then(|t| t.as_array()) {
            for team in teams {
                let team_name = team.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if !my_config.dark_factory.teams.contains(&team_name.to_string()) {
                    continue;
                }

                if let Some(members) = team.get("members").and_then(|m| m.as_array()) {
                    for member in members {
                        let member_path = member.get("path").and_then(|p| p.as_str()).unwrap_or("");

                        // Skip ourselves
                        let peer_name = agent_name_from_path(member_path);
                        if peer_name == my_name {
                            continue;
                        }

                        // Skip duplicates — add team to existing peer
                        if peers.iter().any(|p| p.name == peer_name) {
                            if let Some(existing) = peers.iter_mut().find(|p| p.name == peer_name) {
                                if !existing.teams.contains(&team_name.to_string()) {
                                    existing.teams.push(team_name.to_string());
                                }
                            }
                            continue;
                        }

                        let peer_ac = Path::new(member_path).join(crate::config::agent_local_dir_name());
                        let status = if peer_ac.join("active").exists() {
                            "active"
                        } else {
                            "unknown"
                        };

                        let peer_config: AgentLocalConfig = peer_ac
                            .join("config.json")
                            .to_str()
                            .and_then(|p| std::fs::read_to_string(p).ok())
                            .and_then(|c| serde_json::from_str(&c).ok())
                            .unwrap_or_default();

                        peers.push(PeerInfo {
                            name: peer_name,
                            path: member_path.to_string(),
                            status: status.to_string(),
                            role: read_role(member_path),
                            teams: vec![team_name.to_string()],
                            last_coding_agent: peer_config.tooling.last_coding_agent,
                            coding_agents: peer_config.tooling.coding_agents,
                        });
                    }
                }
            }
        }
    }

    // ── WG replica discovery ──────────────────────────────────────────────
    // Scan repo_paths for .ac-new/wg-*/__agent_* replicas
    let settings = crate::config::settings::load_settings();
    for base_path in &settings.repo_paths {
        let base = Path::new(base_path);
        if !base.is_dir() {
            continue;
        }
        // Check base and its immediate children (same pattern as ac_discovery)
        let mut dirs_to_check = vec![base.to_path_buf()];
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') {
                        dirs_to_check.push(p);
                    }
                }
            }
        }
        for repo_dir in dirs_to_check {
            let ac_new_dir = repo_dir.join(".ac-new");
            if !ac_new_dir.is_dir() {
                continue;
            }
            let wg_entries = match std::fs::read_dir(&ac_new_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for wg_entry in wg_entries.flatten() {
                let wg_path = wg_entry.path();
                if !wg_path.is_dir() {
                    continue;
                }
                let wg_name = match wg_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) if n.starts_with("wg-") => n.to_string(),
                    _ => continue,
                };
                // Derive team name from WG name: "wg-1-ac-devs" → "ac-devs"
                let wg_team = wg_name
                    .strip_prefix("wg-")
                    .and_then(|s| s.split_once('-').map(|(_, rest)| rest))
                    .unwrap_or(&wg_name)
                    .to_string();

                let agent_entries = match std::fs::read_dir(&wg_path) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for agent_entry in agent_entries.flatten() {
                    let agent_path = agent_entry.path();
                    if !agent_path.is_dir() {
                        continue;
                    }
                    let agent_dir = match agent_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) if n.starts_with("__agent_") => n.to_string(),
                        _ => continue,
                    };
                    let agent_short = agent_dir
                        .strip_prefix("__agent_")
                        .unwrap_or(&agent_dir)
                        .to_string();
                    let peer_name = format!("{}/{}", wg_name, agent_short);

                    // Skip self
                    if peer_name == my_name {
                        continue;
                    }
                    // Skip duplicates
                    if peers.iter().any(|p| p.name == peer_name) {
                        continue;
                    }

                    let mut peer = build_wg_peer(&agent_short, &wg_name, &agent_path);
                    peer.teams = vec![wg_team.clone()];
                    peers.push(peer);
                }
            }
        }
    }

    // Output as JSON
    match serde_json::to_string_pretty(&peers) {
        Ok(json) => {
            println!("{}", json);
            let _ = args; // token validated if needed
            0
        }
        Err(e) => {
            eprintln!("Error: failed to serialize peers: {}", e);
            1
        }
    }
}
