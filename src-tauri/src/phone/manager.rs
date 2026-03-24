use std::path::{Path, PathBuf};

use crate::config::dark_factory::{DarkFactoryConfig, Team};
use super::types::{AgentInfo, Conversation, PhoneMessage};

/// Returns ~/.agentscommander/conversations/
fn conversations_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".agentscommander").join("conversations"))
}

/// Check if two agents can communicate based on team routing rules
pub fn can_communicate(from: &str, to: &str, config: &DarkFactoryConfig) -> bool {
    // Find all teams where both are members
    let shared_teams: Vec<&Team> = config
        .teams
        .iter()
        .filter(|t| {
            let has_from = t.members.iter().any(|m| m.name == from);
            let has_to = t.members.iter().any(|m| m.name == to);
            has_from && has_to
        })
        .collect();

    // If any shared team allows it
    for team in &shared_teams {
        match &team.coordinator_name {
            None => return true, // No coordinator = free communication
            Some(coord) => {
                // Allow if either party is the coordinator
                if coord == from || coord == to {
                    return true;
                }
                // Otherwise blocked in this team, check others
            }
        }
    }

    // If no shared team: check coordinator-to-coordinator across teams
    if shared_teams.is_empty() {
        let from_is_coord = config
            .teams
            .iter()
            .any(|t| t.coordinator_name.as_deref() == Some(from) && t.members.iter().any(|m| m.name == from));
        let to_is_coord = config
            .teams
            .iter()
            .any(|t| t.coordinator_name.as_deref() == Some(to) && t.members.iter().any(|m| m.name == to));

        if from_is_coord && to_is_coord {
            return true;
        }
    }

    false
}

/// List all agents from teams with their team memberships
pub fn list_agents(config: &DarkFactoryConfig) -> Vec<AgentInfo> {
    let mut agents: std::collections::HashMap<String, AgentInfo> = std::collections::HashMap::new();

    for team in &config.teams {
        for member in &team.members {
            let entry = agents.entry(member.name.clone()).or_insert_with(|| AgentInfo {
                name: member.name.clone(),
                path: member.path.clone(),
                teams: Vec::new(),
                is_coordinator_of: Vec::new(),
            });
            entry.teams.push(team.name.clone());
            if team.coordinator_name.as_deref() == Some(&member.name) {
                entry.is_coordinator_of.push(team.name.clone());
            }
        }
    }

    agents.into_values().collect()
}

/// Scan conversation files in ~/.agentscommander/conversations/
fn scan_files(dir: &Path) -> Vec<(u32, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut results: Vec<(u32, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?;
            let prefix_str = stem.split('-').next()?;
            let num: u32 = prefix_str.parse().ok()?;
            Some((num, path))
        })
        .collect();
    results.sort_by_key(|(n, _)| *n);
    results
}

fn next_id(files: &[(u32, PathBuf)]) -> String {
    let max = files.iter().map(|(n, _)| *n).max().unwrap_or(0);
    format!("{:04}", max + 1)
}

fn find_existing(files: &[(u32, PathBuf)], a: &str, b: &str) -> Option<PathBuf> {
    for (_, path) in files.iter().rev() {
        if let Ok(data) = std::fs::read_to_string(path) {
            if let Ok(conv) = serde_json::from_str::<Conversation>(&data) {
                let has_a = conv.participants.iter().any(|p| p == a);
                let has_b = conv.participants.iter().any(|p| p == b);
                if has_a && has_b {
                    return Some(path.clone());
                }
            }
        }
    }
    None
}

fn save_conversation(path: &Path, conv: &Conversation) -> Result<(), String> {
    let json = serde_json::to_string_pretty(conv)
        .map_err(|e| format!("Failed to serialize conversation: {}", e))?;
    std::fs::write(path, json)
        .map_err(|e| format!("Failed to write conversation: {}", e))?;
    Ok(())
}

/// Send a message from one agent to another
pub fn send_message(
    from: &str,
    to: &str,
    body: &str,
    team: &str,
    config: &DarkFactoryConfig,
) -> Result<String, String> {
    // Validate routing
    if !can_communicate(from, to, config) {
        return Err(format!(
            "Agent '{}' cannot communicate with '{}' — not in the same team or must go through coordinator",
            from, to
        ));
    }

    let conv_dir = conversations_dir().ok_or("Could not determine conversations directory")?;
    std::fs::create_dir_all(&conv_dir)
        .map_err(|e| format!("Failed to create conversations dir: {}", e))?;

    let files = scan_files(&conv_dir);

    // Find or create conversation
    let (conv_path, mut conv) = match find_existing(&files, from, to) {
        Some(path) => {
            let data = std::fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read conversation: {}", e))?;
            let conv: Conversation = serde_json::from_str(&data)
                .map_err(|e| format!("Failed to parse conversation: {}", e))?;
            (path, conv)
        }
        None => {
            let fresh_files = scan_files(&conv_dir);
            let id = next_id(&fresh_files);
            let filename = format!("{}-{}_{}.json", id, from, to);
            let path = conv_dir.join(filename);
            let conv = Conversation {
                id: id.clone(),
                participants: vec![from.to_string(), to.to_string()],
                created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                messages: vec![],
            };
            save_conversation(&path, &conv)?;
            (path, conv)
        }
    };

    let msg_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    conv.messages.push(PhoneMessage {
        id: msg_id.clone(),
        from: from.to_string(),
        to: to.to_string(),
        team: team.to_string(),
        content: body.to_string(),
        timestamp: now,
        status: "delivered".to_string(),
    });

    save_conversation(&conv_path, &conv)?;
    Ok(conv.id)
}

/// Get all unread messages for an agent
pub fn get_inbox(agent_name: &str) -> Result<Vec<PhoneMessage>, String> {
    let conv_dir = conversations_dir().ok_or("Could not determine conversations directory")?;
    if !conv_dir.exists() {
        return Ok(vec![]);
    }

    let files = scan_files(&conv_dir);
    let mut inbox = Vec::new();

    for (_, path) in &files {
        if let Ok(data) = std::fs::read_to_string(path) {
            if let Ok(conv) = serde_json::from_str::<Conversation>(&data) {
                if conv.participants.iter().any(|p| p == agent_name) {
                    for msg in &conv.messages {
                        if msg.to == agent_name && msg.status != "read" {
                            inbox.push(msg.clone());
                        }
                    }
                }
            }
        }
    }

    inbox.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    Ok(inbox)
}

/// Mark messages as read
pub fn ack_messages(agent_name: &str, message_ids: &[String]) -> Result<(), String> {
    let conv_dir = conversations_dir().ok_or("Could not determine conversations directory")?;
    if !conv_dir.exists() {
        return Ok(());
    }

    let files = scan_files(&conv_dir);
    let id_set: std::collections::HashSet<&str> = message_ids.iter().map(|s| s.as_str()).collect();

    for (_, path) in &files {
        if let Ok(data) = std::fs::read_to_string(path) {
            if let Ok(mut conv) = serde_json::from_str::<Conversation>(&data) {
                if !conv.participants.iter().any(|p| p == agent_name) {
                    continue;
                }
                let mut changed = false;
                for msg in &mut conv.messages {
                    if msg.to == agent_name && id_set.contains(msg.id.as_str()) {
                        msg.status = "read".to_string();
                        changed = true;
                    }
                }
                if changed {
                    save_conversation(path, &conv)?;
                }
            }
        }
    }

    Ok(())
}
