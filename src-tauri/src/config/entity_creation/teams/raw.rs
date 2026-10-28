//! Exact raw team membership mutations; preserves unrelated JSON values.
use super::super::{directories, names::normalize_context_alert_percentages};
use super::store;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct AgentTeamMutation {
    pub(crate) team_name: String,
    pub(crate) config_path: PathBuf,
    pub(crate) before_json: Vec<u8>,
    pub(crate) after_json: Vec<u8>,
}

pub(crate) fn collect_agent_team_mutations_raw(
    base: &Path,
    agent_name: &str,
    agent_ref: &str,
) -> Result<Vec<AgentTeamMutation>, String> {
    let team_dirs = directories::strict_team_candidates(base)?;

    let mut coordinator_blockers = Vec::new();
    let mut mutations = Vec::new();
    for team_dir in team_dirs {
        let dir_name = team_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("_team_unknown");
        let team_name = dir_name.strip_prefix("_team_").unwrap_or(dir_name);
        let Some((config_path, before_json)) = store::read_raw_team_config(base, team_name)? else {
            continue;
        };
        let mut value: serde_json::Value = match serde_json::from_slice(&before_json) {
            Ok(value) => value,
            Err(e) => {
                let raw = String::from_utf8_lossy(&before_json);
                if raw_text_mentions_agent_token(&raw, agent_name, agent_ref) {
                    return Err(format!(
                        "Cannot delete agent '{}': cannot verify team '{}' references because \
                         config.json is invalid: {}",
                        agent_name, team_name, e
                    ));
                }
                log::warn!(
                    "[entity_creation] Skipping unrelated malformed team config for '{}': {}",
                    team_name,
                    e
                );
                continue;
            }
        };

        if value
            .get("coordinator")
            .and_then(|coordinator| coordinator.as_str())
            .is_some_and(|coordinator| agent_ref_matches_target(coordinator, agent_name))
        {
            coordinator_blockers.push(team_name.to_string());
        }

        let mut changed = false;
        if let Some(agents) = value
            .get_mut("agents")
            .and_then(|agents| agents.as_array_mut())
        {
            let before_len = agents.len();
            agents.retain(|agent| match agent.as_str() {
                Some(agent) => !agent_ref_matches_target(agent, agent_name),
                None => true,
            });
            changed |= agents.len() != before_len;
        }

        if let Some(repos) = value
            .get_mut("repos")
            .and_then(|repos| repos.as_array_mut())
        {
            for repo in repos {
                let Some(repo_obj) = repo.as_object_mut() else {
                    continue;
                };
                let Some(agents) = repo_obj
                    .get_mut("agents")
                    .and_then(|agents| agents.as_array_mut())
                else {
                    continue;
                };
                let before_len = agents.len();
                agents.retain(|agent| match agent.as_str() {
                    Some(agent) => !agent_ref_matches_target(agent, agent_name),
                    None => true,
                });
                changed |= agents.len() != before_len;
            }
        }

        if changed {
            let object = value.as_object_mut().ok_or_else(|| {
                format!(
                    "Cannot delete agent '{}': team '{}' config.json must be an object",
                    agent_name, team_name
                )
            })?;
            normalize_raw_context_alert_percentages(object).map_err(|e| {
                format!(
                    "Cannot delete agent '{}': team '{}' has invalid contextAlertPercentages: {}",
                    agent_name, team_name, e
                )
            })?;
            let mut after_json = serde_json::to_vec_pretty(&value)
                .map_err(|e| format!("Cannot serialize team '{}' config.json: {}", team_name, e))?;
            after_json.push(b'\n');
            mutations.push(AgentTeamMutation {
                team_name: team_name.to_string(),
                config_path,
                before_json,
                after_json,
            });
        }
    }

    if !coordinator_blockers.is_empty() {
        coordinator_blockers.sort();
        return Err(format!(
            "Cannot delete agent '{}': orchestrator of team(s): {}. Reassign the orchestrator \
             first.",
            agent_name,
            coordinator_blockers.join(", ")
        ));
    }

    Ok(mutations)
}

fn normalize_raw_context_alert_percentages(
    object: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let values = match object.get("contextAlertPercentages") {
        Some(value) => serde_json::from_value::<Vec<u8>>(value.clone()).map_err(|e| {
            format!(
                "expected an array of integer percentages representable as u8: {}",
                e
            )
        })?,
        None => Vec::new(),
    };
    let normalized = normalize_context_alert_percentages(&values)?;
    object.insert(
        "contextAlertPercentages".to_string(),
        serde_json::to_value(normalized)
            .map_err(|e| format!("failed to serialize canonical percentages: {}", e))?,
    );
    Ok(())
}

fn agent_ref_matches_target(raw_ref: &str, agent_name: &str) -> bool {
    let bare = raw_agent_ref_bare_name(raw_ref);
    agent_name_matches(&bare, agent_name)
}

fn raw_agent_ref_bare_name(raw_ref: &str) -> String {
    let normalized = raw_ref.replace('\\', "/");
    let final_segment = normalized.rsplit('/').next().unwrap_or(&normalized);
    final_segment
        .strip_prefix("_agent_")
        .unwrap_or(final_segment)
        .to_string()
}

fn raw_text_mentions_agent_token(raw: &str, agent_name: &str, agent_ref: &str) -> bool {
    contains_whole_agent_token(raw, agent_ref) || contains_whole_agent_token(raw, agent_name)
}

fn contains_whole_agent_token(raw: &str, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    #[cfg(windows)]
    let (haystack, needle) = (raw.to_ascii_lowercase(), token.to_ascii_lowercase());
    #[cfg(not(windows))]
    let (haystack, needle) = (raw.to_string(), token.to_string());

    let mut start = 0;
    while let Some(offset) = haystack[start..].find(&needle) {
        let match_start = start + offset;
        let match_end = match_start + needle.len();
        let before_ok = haystack[..match_start]
            .chars()
            .next_back()
            .map(|c| !is_agent_token_char(c))
            .unwrap_or(true);
        let after_ok = haystack[match_end..]
            .chars()
            .next()
            .map(|c| !is_agent_token_char(c))
            .unwrap_or(true);
        if before_ok && after_ok {
            return true;
        }
        start = match_end;
    }
    false
}

fn is_agent_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn agent_name_matches(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}
