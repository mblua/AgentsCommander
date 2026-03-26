use serde::Serialize;
use std::path::Path;
use tauri::State;

use crate::config::settings::SettingsState;

/// Agent detection: label + list of possible markers (files or dirs)
/// An agent is detected if ANY of its markers exist in the repo.
const AGENT_DETECTORS: &[(&str, &[&str])] = &[
    ("Claude", &[".claude", "CLAUDE.md"]),
    ("Codex", &[".codex"]),
    ("OpenCode", &[".opencode", "opencode.json"]),
    ("Cursor", &[".cursor", ".cursorrules"]),
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoMatch {
    pub name: String,
    pub path: String,
    /// Which agent tools are detected in this repo (e.g. ["Claude", "Codex"])
    pub agents: Vec<String>,
}

/// Detect which agent tools are configured in a repo directory
fn detect_agents(repo_path: &Path) -> Vec<String> {
    AGENT_DETECTORS
        .iter()
        .filter(|(_, markers)| markers.iter().any(|m| repo_path.join(m).exists()))
        .map(|(label, _)| label.to_string())
        .collect()
}

/// Derive extended repo name as "parent/repo" from an absolute path.
/// Always uses forward slash as separator regardless of OS.
pub fn derive_repo_name(path: &Path) -> Option<String> {
    let file_name = path.file_name().and_then(|n| n.to_str())?;
    let name = match path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
        Some(parent) => format!("{}/{}", parent, file_name),
        None => file_name.to_string(),
    };
    Some(name)
}

/// Add a repo to results if it matches the query and hasn't been seen yet
fn try_add_repo(
    path: &Path,
    query_lower: &str,
    seen_paths: &mut std::collections::HashSet<String>,
    results: &mut Vec<RepoMatch>,
) {
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return,
    };

    // Skip DEPRECATED (check on repo dir name only, not extended name)
    if file_name.to_uppercase().starts_with("DEPRECATED") {
        return;
    }

    let name = match derive_repo_name(path) {
        Some(n) => n,
        None => return,
    };

    if !query_lower.is_empty() && !name.to_lowercase().contains(query_lower) {
        return;
    }

    let path_str = path.to_string_lossy().to_string();
    if seen_paths.insert(path_str.clone()) {
        let agents = detect_agents(path);
        results.push(RepoMatch {
            name,
            path: path_str,
            agents,
        });
    }
}

/// Scan configured paths for potential agents.
/// Each configured path is treated as both:
/// - A potential agent folder itself, AND
/// - A parent folder whose children are also potential agents
///
/// For each folder found, detects agent tooling (.claude, .codex, .cursor).
#[tauri::command]
pub async fn search_repos(
    settings: State<'_, SettingsState>,
    query: String,
) -> Result<Vec<RepoMatch>, String> {
    let cfg = settings.read().await;
    let query_lower = query.to_lowercase();
    let mut results: Vec<RepoMatch> = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    for base_path in &cfg.repo_paths {
        let base = Path::new(base_path);
        if !base.is_dir() {
            continue;
        }

        // The folder itself is a potential agent
        try_add_repo(base, &query_lower, &mut seen_paths, &mut results);

        // Its children are also potential agents
        let entries = match std::fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip hidden directories
            if name.starts_with('.') {
                continue;
            }

            try_add_repo(&path, &query_lower, &mut seen_paths, &mut results);
        }
    }

    // Sort alphabetically
    results.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(results)
}
