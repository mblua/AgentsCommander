use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::ac_root::existing_ac_root;
use crate::filesystem::projects::entities::agent_matrix_directory_present;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamAgentSource {
    pub agent_ref: String,
    pub source_project_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoAssignment {
    pub url: String,
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamConfigResult {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub coordinator: String,
    #[serde(default)]
    pub repos: Vec<RepoAssignment>,
    #[serde(default)]
    pub context_alert_percentages: Vec<u8>,
}

/// Sanitize a user-provided name into a safe directory component:
/// lowercase, only a-z 0-9 and hyphens, no leading/trailing hyphens.
pub(crate) fn sanitize_name(raw: &str) -> Result<String, String> {
    let sanitized: String = raw
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if sanitized.is_empty() {
        return Err("Name must contain at least one alphanumeric character".into());
    }
    Ok(sanitized)
}

/// Validate that an existing entity name is safe for path operations.
/// Unlike `sanitize_name`, this does NOT transform the name — it just rejects
/// names that contain path traversal or separator characters.
///
/// `pub(crate)` so the sentinel-collision invariant test in
/// `wg_delete_diagnostic::tests` can prove that no valid WG name can collide
/// with the `BLOCKERS:` / `DIRTY_REPOS:` sentinel prefixes.
pub(crate) fn validate_existing_name(name: &str, entity_label: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{} name cannot be empty", entity_label));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!(
            "Invalid {} name: only alphanumeric characters and hyphens are allowed",
            entity_label
        ));
    }
    Ok(())
}

pub(crate) fn selected_ac_root(project: &Path) -> Result<PathBuf, String> {
    existing_ac_root(project)
        .ok_or_else(|| format!(".ac directory not found in {}", project.display()))
}

pub(crate) fn normalize_context_alert_percentages(values: &[u8]) -> Result<Vec<u8>, String> {
    if values.len() > 3 {
        return Err("contextAlertPercentages must contain at most 3 values".to_string());
    }
    if values.iter().any(|value| !(1..=100).contains(value)) {
        return Err(
            "contextAlertPercentages values must be integer percentages from 1 through 100"
                .to_string(),
        );
    }
    let mut normalized = values.to_vec();
    normalized.sort_unstable();
    if normalized.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("contextAlertPercentages values must be distinct".to_string());
    }
    Ok(normalized)
}

pub(crate) fn normalize_team_agent_refs(
    ac_root: &Path,
    refs: &[String],
) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for agent in refs {
        let resolved = resolve_agent_ref(ac_root, agent)?;
        if !normalized.contains(&resolved) {
            normalized.push(resolved);
        }
    }
    Ok(normalized)
}

pub(crate) fn resolve_agent_ref(ac_root: &Path, raw_agent: &str) -> Result<String, String> {
    let trimmed = raw_agent.trim();
    if trimmed.is_empty() {
        return Err("Agent reference cannot be empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("Agent reference must not contain NUL".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains(':') {
        return Err(format!(
            "Invalid agent reference '{}': team configs must use portable refs like '_agent_name' \
             or 'name', not filesystem paths",
            raw_agent
        ));
    }
    if Path::new(trimmed).is_absolute() {
        return Err(format!(
            "Invalid agent reference '{}': absolute paths are not allowed in team configs",
            raw_agent
        ));
    }
    let bare = trimmed.strip_prefix("_agent_").unwrap_or(trimmed);
    validate_existing_name(bare, "Agent")?;
    let canonical_ref = format!("_agent_{}", bare);
    let matrix_dir = ac_root.join(&canonical_ref);
    if !agent_matrix_directory_present(&matrix_dir) {
        return Err(format!("Agent '{}' not found", bare));
    }
    Ok(canonical_ref)
}
