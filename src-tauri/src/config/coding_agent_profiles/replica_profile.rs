use super::paths::validate_profile_selection_agent_path;
use serde_json::Value;

use super::{read_tooling_string, write_tooling_string, ToolingSide};
use crate::config::settings::{normalize_profile_letter, AppSettings};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaProfileRead {
    pub profile: Option<String>,
    pub warning: Option<String>,
}

pub fn read_replica_profile_result(launch_path: &Path) -> ReplicaProfileRead {
    let v2 = read_tooling_string(launch_path, "profile");
    let legacy = read_tooling_string(launch_path, "instanceProfileOverride");
    let v2_normalized = v2.as_deref().and_then(normalize_profile_letter);
    let legacy_normalized = legacy.as_deref().and_then(normalize_profile_letter);
    let warning = match (&v2_normalized, &legacy_normalized) {
        (Some(v2), Some(legacy)) if v2 != legacy => Some(format!(
            "tooling.profile ({}) differs from legacy instanceProfileOverride ({}) at {}",
            v2,
            legacy,
            launch_path.display()
        )),
        _ => None,
    };
    ReplicaProfileRead {
        profile: v2_normalized.or(legacy_normalized),
        warning,
    }
}

pub fn read_replica_profile(launch_path: &Path) -> Option<String> {
    read_replica_profile_result(launch_path).profile
}

pub fn read_replica_current_coding_agent(launch_path: &Path) -> Option<String> {
    read_tooling_string(launch_path, "currentCodingAgent")
}

/// #592 - read the loaded profile content-hash persisted at last spawn.
pub fn read_replica_profile_content_hash(launch_path: &Path) -> Option<String> {
    read_tooling_string(launch_path, "profileContentHash")
}

/// #592 - persist the loaded profile content-hash. No-op for non-replica /
/// non-matrix / non-root-agent launch roots (a normal repo has no per-agent
/// config.json and must not be littered with one). Best-effort: callers log +
/// ignore errors.
pub fn set_replica_profile_content_hash(launch_path: &Path, hash: &str) -> Result<(), String> {
    let is_agent_dir = launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_") || name.starts_with("_agent_"))
        .unwrap_or(false);
    // #592 - the Root Agent (`ac-root-agent`) is a legitimate replica that runs a
    // coding agent and must support drift like any other; its dir name does not
    // match the `__agent_`/`_agent_` prefix, so accept it explicitly. Name-only
    // (vs path-validated `is_root_agent_path`) is sufficient here: the sole caller
    // `create_session_inner` already writes this dir's `config.state.no-git.json`
    // unconditionally via `set_last_coding_agent` right before this call, so the
    // gate only governs whether the hash field is added, never whether a stray
    // state file is created. The read side is ungated, so it round-trips.
    let is_root_agent = launch_path
        .to_str()
        .map(crate::config::root_agent::is_root_agent_dir_name)
        .unwrap_or(false);
    if !is_agent_dir && !is_root_agent {
        return Ok(());
    }
    log::debug!(
        "[profile-hash] persist: writing profileContentHash={} to {}",
        hash,
        launch_path.display(),
    );
    write_tooling_string(
        launch_path,
        ToolingSide::State,
        "profileContentHash",
        Some(hash),
    )
}

pub fn set_agent_default_profile(
    settings: &AppSettings,
    launch_path: &Path,
    profile: &str,
) -> Result<(), String> {
    let profile = normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?;
    let validated = validate_profile_selection_agent_path(settings, launch_path)?;
    write_tooling_string(
        &validated.origin_matrix_dir,
        ToolingSide::Decisions,
        "defaultProfile",
        Some(&profile),
    )
}

pub fn set_instance_profile_override(
    settings: &AppSettings,
    launch_path: &Path,
    profile: Option<&str>,
) -> Result<(), String> {
    let normalized = match profile {
        Some(profile) => Some(
            normalize_profile_letter(profile)
                .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?,
        ),
        None => None,
    };
    let validated = validate_profile_selection_agent_path(settings, launch_path)?;
    write_profile_to_launch_path(&validated.launch_path, normalized.as_deref())?;
    Ok(())
}

// ── #1939 replica selection lock state ──────────────────
//
// Stored contract: replica top-level `tooling.selectionLocked` (optional;
// absent means false) protects the saved pair — `tooling.currentCodingAgent`
// plus the requested `tooling.profile` letter. It never protects the resolved
// command/environment/model/content, which stay resolver-owned: a fallback is
// never written back over the requested letter.
//
// Matrix contract: top-level `tooling.replicaSelectionDefault` is the creation
// default {codingAgentId, requestedProfile, selectionLocked}. Absent keeps the
// old creation behavior; a present one must be complete and valid.

pub(super) fn write_profile_to_launch_path(
    launch_path: &Path,
    profile: Option<&str>,
) -> Result<(), String> {
    let config_path = launch_path.join("config.json");
    crate::config::agent_config::update_agent_config(&config_path, |obj, _state| {
        // #1939 - shape-based, like agent_config: absent tooling becomes an
        // object, a present non-object is an error, never silently reset.
        let tooling_value = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let tooling = tooling_value
            .as_object_mut()
            .ok_or_else(|| format!("tooling must be a JSON object at {}", config_path.display()))?;
        match profile {
            Some(profile) => {
                if tooling.contains_key("configurationRef") {
                    return Err(
                        "source profile override requires a prepared same-source selection".into(),
                    );
                }
                // A new valid requested letter is permitted; the Coding Agent
                // and the lock flag are preserved exactly as stored.
                tooling.insert("profile".to_string(), Value::String(profile.to_string()));
                tooling.insert(
                    "instanceProfileOverride".to_string(),
                    Value::String(profile.to_string()),
                );
                tooling.insert(
                    "instanceProfileOverrideSource".to_string(),
                    Value::String("manual".to_string()),
                );
            }
            None => {
                if protected_selection_pair_present(tooling)
                    || tooling.contains_key("configurationRef")
                {
                    return Err(format!(
                        "Cannot clear the profile at {} while a protected replica selection \
                         pair exists",
                        launch_path.display()
                    ));
                }
                tooling.remove("profile");
                tooling.remove("instanceProfileOverride");
                tooling.remove("instanceProfileOverrideSource");
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// #1939 - a complete saved pair (`currentCodingAgent` plus a valid profile
/// letter) is the protected unit; a null clear must not erase half of it.
pub(super) fn protected_selection_pair_present(tooling: &serde_json::Map<String, Value>) -> bool {
    let coding_agent = tooling
        .get("currentCodingAgent")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty());
    let profile = tooling
        .get("profile")
        .and_then(Value::as_str)
        .and_then(normalize_profile_letter)
        .or_else(|| {
            tooling
                .get("instanceProfileOverride")
                .and_then(Value::as_str)
                .and_then(normalize_profile_letter)
        });
    coding_agent && profile.is_some()
}
