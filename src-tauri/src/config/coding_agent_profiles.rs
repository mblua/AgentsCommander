use crate::config::settings::ProfileCellConfig;
use crate::filesystem::coding_agent_profiles::{profile_path_symlink_metadata, read_profile_text};
use serde_json::Value;
use std::path::{Path, PathBuf};

mod paths;
mod replica_profile;
mod resolve;
mod selection_state;
mod selection_write;
mod source_selection;

pub use paths::validate_profile_selection_agent_path;
pub(crate) use paths::{agent_name_from_dir, configured_ac_roots};
pub use replica_profile::{
    read_replica_current_coding_agent, read_replica_profile, read_replica_profile_content_hash,
    read_replica_profile_result, set_agent_default_profile, set_instance_profile_override,
    set_replica_profile_content_hash,
};
pub use resolve::{resolve_profile, resolve_profile_selection};
#[cfg(test)]
use selection_state::SelectionWriteOutcome;
pub(crate) use selection_state::{
    apply_replica_selection_default, selection_state_from_value, PreparedSourceSelection,
};
pub use selection_state::{
    read_replica_selection_default, read_replica_selection_state, write_replica_selection_default,
    ReplicaSelectionDefault, ReplicaSelectionExpectation, ReplicaSelectionPair,
    ReplicaSelectionState, SelectionLockTransition, SelectionWriteIntent,
};
pub use selection_write::{
    clear_replica_selection_lock, set_replica_coding_agent_selection, write_replica_selection,
};
pub(crate) use source_selection::{
    apply_identity_reference_patch, discover_source_inventory, prepare_identity_reference_patch,
    SourceSelectionTarget,
};
use source_selection::{identity_reference_value, plan_identity_reference_patch};

#[derive(Debug, Clone)]
pub struct ProfileResolutionRequest<'a> {
    pub coding_agent_id: &'a str,
    pub launch_path: Option<&'a Path>,
    pub agent_matrix_name: Option<&'a str>,
    pub requested_profile: Option<&'a str>,
    /// When true and `requested_profile` is present, the explicit request
    /// outranks the instance override (replica pin) for this resolution only.
    /// Wake dispatch sets this; all other callers leave it false.
    pub requested_profile_authoritative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileResolution {
    pub requested_profile: String,
    pub effective_profile: String,
    pub fallback_chain: Vec<String>,
    pub fallback_applied: bool,
    pub cell: ProfileCellConfig,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedProfileAgentPath {
    pub launch_path: PathBuf,
    pub origin_matrix_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSelectionResolution {
    pub requested_profile_input: Option<String>,
    pub instance_profile_override: Option<String>,
    pub origin_default_profile: Option<String>,
    pub agent_default_profile: Option<String>,
    pub resolution: ProfileResolution,
}

fn read_json_object(path: &Path) -> Option<Value> {
    let content = read_profile_text(path).ok()?;
    serde_json::from_str::<Value>(&content).ok()
}

/// #2786 (C1) - through the shared loader, so a state key such as
/// `profileContentHash` is served from the state file first.
fn read_tooling_string(agent_dir: &Path, key: &str) -> Option<String> {
    crate::config::agent_config::read_agent_local_config_json(agent_dir)
        .ok()??
        .get("tooling")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// #2816 (C3) - which file of the config pair a `tooling` string lives in.
#[derive(Clone, Copy)]
enum ToolingSide {
    Decisions,
    State,
}

fn write_tooling_string(
    agent_dir: &Path,
    side: ToolingSide,
    key: &str,
    value: Option<&str>,
) -> Result<(), String> {
    let metadata = profile_path_symlink_metadata(agent_dir).map_err(|e| {
        format!(
            "Agent config dir '{}' is not readable: {}",
            agent_dir.display(),
            e
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "Agent config dir '{}' is not a real directory",
            agent_dir.display()
        ));
    }
    let config_path = agent_dir.join("config.json");
    crate::config::agent_config::update_agent_config(&config_path, |decisions, state| {
        let obj = match side {
            ToolingSide::Decisions => decisions,
            ToolingSide::State => state,
        };
        let tooling_value = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        // #1939 - a malformed `tooling` is never silently reset: the write fails
        // with the stored bytes untouched (shape-based, like agent_config).
        let tooling = tooling_value
            .as_object_mut()
            .ok_or_else(|| format!("tooling must be a JSON object at {}", config_path.display()))?;
        match value {
            Some(value) => {
                tooling.insert(key.to_string(), Value::String(value.to_string()));
            }
            None => {
                tooling.remove(key);
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// Complete semantic slots captured from the SAME original physical snapshots.
#[allow(dead_code)]
pub(crate) fn source_original_slots(
    target: &SourceSelectionTarget,
) -> Result<
    Vec<(
        crate::config::settings::SourceOriginalSlot,
        crate::config::settings::ConfigurationRef,
    )>,
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::{ConfigurationRef, IdentityReferenceError, SourceOriginalSlot};
    let mut slots = Vec::new();
    let mut capture = |slot, value: &Value| -> Result<(), IdentityReferenceError> {
        if !value.is_object() {
            return Err(IdentityReferenceError::Conflict);
        }
        if let Some(reference) = value.get("configurationRef") {
            let reference: ConfigurationRef = serde_json::from_value(reference.clone())
                .map_err(|_| IdentityReferenceError::Conflict)?;
            if reference.version != 1 {
                return Err(IdentityReferenceError::Conflict);
            }
            slots.push((slot, reference));
        }
        Ok(())
    };
    let decisions = identity_reference_value(&target.physical_revision[0])?;
    let state = identity_reference_value(&target.physical_revision[1])?;
    if let Some(tooling) = decisions.get("tooling") {
        capture(SourceOriginalSlot::Decision, tooling)?;
        if let Some(default) = tooling.get("replicaSelectionDefault") {
            let slot = if target
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("_agent_"))
            {
                SourceOriginalSlot::MatrixDefault
            } else {
                SourceOriginalSlot::ReplicaDefault
            };
            capture(slot, default)?;
        }
    }
    if let Some(tooling) = state.get("tooling") {
        if !tooling.is_object() {
            return Err(IdentityReferenceError::Conflict);
        }
        if let Some(history) = tooling.get("codingAgents") {
            for (key, entry) in history
                .as_object()
                .ok_or(IdentityReferenceError::Conflict)?
            {
                capture(SourceOriginalSlot::HistoryKey(key.clone()), entry)?;
            }
        }
        if let Some(history) = tooling.get("sourceReferences") {
            for (index, entry) in history
                .as_array()
                .ok_or(IdentityReferenceError::Conflict)?
                .iter()
                .enumerate()
            {
                capture(SourceOriginalSlot::HistoryIndex(index), entry)?;
            }
        }
    }
    Ok(slots)
}

#[allow(dead_code)]
pub(crate) fn source_target_is_affected(
    target: &SourceSelectionTarget,
    source: &crate::config::settings::SourceRef,
    mappings: &[crate::config::settings::SourceIdentityMapping],
) -> Result<bool, crate::config::settings::IdentityReferenceError> {
    let patch = plan_identity_reference_patch(target.physical_revision.clone(), source, mappings)?;
    let original = [
        identity_reference_value(&target.physical_revision[0])?,
        identity_reference_value(&target.physical_revision[1])?,
    ];
    Ok(patch.after != original)
}

#[cfg(test)]
thread_local! {
    static SOURCE_DISCOVERY_CALLS:
        std::cell::RefCell<std::collections::BTreeMap<&'static str, usize>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
    static SOURCE_DISCOVERY_FAILURE: std::cell::Cell<Option<(&'static str, usize)>> =
        const { std::cell::Cell::new(None) };
}

fn source_discovery_cut(_stage: &'static str) -> Result<(), std::io::Error> {
    #[cfg(test)]
    SOURCE_DISCOVERY_CALLS.with(|calls| *calls.borrow_mut().entry(_stage).or_default() += 1);
    #[cfg(test)]
    SOURCE_DISCOVERY_FAILURE.with(|failure| {
        if let Some((stage, remaining)) = failure.get() {
            if stage == _stage {
                if remaining == 1 {
                    failure.set(None);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "source inventory fail-only cut",
                    ));
                }
                failure.set(Some((stage, remaining - 1)));
            }
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;
