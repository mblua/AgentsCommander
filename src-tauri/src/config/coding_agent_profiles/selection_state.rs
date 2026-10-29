use super::replica_profile::ReplicaProfileRead;
use crate::filesystem::coding_agent_profiles::read_selection_default_bytes;

use crate::config::settings::{normalize_profile_letter, ConfigurationRef};
use serde_json::Value;
use std::path::Path;

/// The protected replica selection pair: coding agent id plus the requested
/// profile letter. The resolved command/environment/model/content are
/// deliberately not part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaSelectionPair {
    pub coding_agent_id: String,
    pub requested_profile: String,
}

/// Shape and compatibility validation only; this does not authorize a source or
/// resolve its current cells. Commands must prepare those before publication.
pub(crate) fn validate_configuration_ref_pair(
    reference: &ConfigurationRef,
    pair: &ReplicaSelectionPair,
) -> Result<(), String> {
    let full_identity = |value: &str| {
        value.strip_prefix("cid1:").is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    };
    if reference.version != 1
        || reference.source.context_root.is_empty()
        || pair.coding_agent_id.trim().is_empty()
        || !full_identity(&reference.calculated_id)
        || !reference
            .profile_calculated_id
            .as_deref()
            .is_some_and(full_identity)
        || reference.legacy_entry_hint.as_deref() != Some(pair.coding_agent_id.as_str())
        || normalize_profile_letter(&pair.requested_profile).as_deref()
            != Some(pair.requested_profile.as_str())
        || reference
            .requested_profile
            .as_ref()
            .is_some_and(|letter| letter != &pair.requested_profile)
    {
        return Err("invalid configurationRef or incompatible saved selection pair".into());
    }
    Ok(())
}

pub(super) fn configuration_ref_from_tooling(
    tooling: &serde_json::Map<String, Value>,
    pair: Option<&ReplicaSelectionPair>,
) -> Result<Option<ConfigurationRef>, String> {
    let Some(raw) = tooling.get("configurationRef") else {
        return Ok(None);
    };
    let reference: ConfigurationRef =
        serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
    validate_configuration_ref_pair(
        &reference,
        pair.ok_or("configurationRef requires a complete saved pair")?,
    )?;
    Ok(Some(reference))
}

/// Read a coherent source-null pin without treating derived aliases as manual.
/// Historical and absent markers keep the existing alias authority and warning.
#[allow(dead_code)]
pub(crate) fn read_source_null_profile_result(
    launch_path: &Path,
) -> Result<ReplicaProfileRead, String> {
    let (config, identity) =
        crate::config::replica_identity::read_wg_replica_config_read_only(launch_path)?;
    let state = selection_state_from_value(&config, &identity.identity);
    let expected = state.expectation()?;
    let warning = match state {
        ReplicaSelectionState::Unlocked { warning, .. }
        | ReplicaSelectionState::Locked { warning, .. } => warning,
        ReplicaSelectionState::Invalid { .. } => unreachable!("expectation rejects invalid state"),
    };
    let mut read = ReplicaProfileRead {
        profile: config
            .pointer("/tooling/profile")
            .and_then(Value::as_str)
            .and_then(normalize_profile_letter)
            .or_else(|| {
                config
                    .pointer("/tooling/instanceProfileOverride")
                    .and_then(Value::as_str)
                    .and_then(normalize_profile_letter)
            }),
        warning,
    };
    if expected
        .configuration_ref
        .as_ref()
        .is_some_and(|reference| reference.requested_profile.is_none())
        && expected.profile_override_source.as_deref() == Some("source-default")
    {
        read.profile = None;
    }
    Ok(read)
}

/// Already authorized, source-local resolution. The aliases retain requested
/// intent while the reference witnesses the effective full pair, even fallback.
#[derive(Debug, Clone)]
pub(crate) struct PreparedSourceSelection {
    pub pair: ReplicaSelectionPair,
    pub configuration_ref: ConfigurationRef,
    pub profile_override_source: Option<String>,
}

impl PreparedSourceSelection {
    pub(super) fn validate(&self) -> Result<(), String> {
        validate_configuration_ref_pair(&self.configuration_ref, &self.pair)
    }
}

/// A prior selection state a write must still observe (CAS). Built from
/// [`read_replica_selection_state`] so a write can never overwrite a state it
/// did not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaSelectionExpectation {
    /// Canonical replica identity (`../../_agent_<name>`), as validated by the
    /// strict read.
    pub identity: String,
    /// The complete saved pair, or `None` for a legacy config without one.
    pub pair: Option<ReplicaSelectionPair>,
    pub locked: bool,
    pub configuration_ref: Option<ConfigurationRef>,
    pub profile_override_source: Option<String>,
}

/// #1939 - strict, read-only view of a replica's stored selection.
///
/// `Invalid` is a diagnostic, never a permissive state: invalid/duplicate JSON,
/// a non-object `tooling`, a non-boolean `selectionLocked`, or `true` with an
/// incomplete/invalid pair can never be read as `Unlocked`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicaSelectionState {
    Unlocked {
        identity: String,
        configuration_ref: Option<ConfigurationRef>,
        profile_override_source: Option<String>,
        /// `None` when no complete pair is stored (legacy unlocked config, which
        /// remains valid and may be deliberately assigned).
        pair: Option<ReplicaSelectionPair>,
        /// Present when `tooling.profile` diverges from the legacy
        /// `instanceProfileOverride`; the modern field wins, as before.
        warning: Option<String>,
    },
    Locked {
        identity: String,
        configuration_ref: Option<ConfigurationRef>,
        profile_override_source: Option<String>,
        pair: ReplicaSelectionPair,
        warning: Option<String>,
    },
    Invalid {
        diagnostic: String,
    },
}

impl ReplicaSelectionState {
    #[cfg(test)]
    pub fn pair(&self) -> Option<&ReplicaSelectionPair> {
        match self {
            Self::Unlocked { pair, .. } => pair.as_ref(),
            Self::Locked { pair, .. } => Some(pair),
            Self::Invalid { .. } => None,
        }
    }

    #[cfg(test)]
    pub fn is_locked(&self) -> bool {
        matches!(self, Self::Locked { .. })
    }

    /// The CAS value a write must pass. An `Invalid` state has none: it can
    /// never be force-repaired implicitly.
    pub fn expectation(&self) -> Result<ReplicaSelectionExpectation, String> {
        match self {
            Self::Unlocked {
                identity,
                pair,
                configuration_ref,
                profile_override_source,
                ..
            } => Ok(ReplicaSelectionExpectation {
                identity: identity.clone(),
                pair: pair.clone(),
                locked: false,
                configuration_ref: configuration_ref.clone(),
                profile_override_source: profile_override_source.clone(),
            }),
            Self::Locked {
                identity,
                pair,
                configuration_ref,
                profile_override_source,
                ..
            } => Ok(ReplicaSelectionExpectation {
                identity: identity.clone(),
                pair: Some(pair.clone()),
                locked: true,
                configuration_ref: configuration_ref.clone(),
                profile_override_source: profile_override_source.clone(),
            }),
            Self::Invalid { diagnostic } => Err(format!(
                "replica selection state is invalid: {}",
                diagnostic
            )),
        }
    }
}

/// #1939 - which policy a selection write applies. There is deliberately no
/// free `bool`: every caller names an intent, so none can bypass the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionWriteIntent {
    /// Deliberate single-replica assignment; preserves the existing flag.
    /// A locked replica is rejected unless the request equals its saved pair,
    /// which returns UNCHANGED without publishing.
    Individual,
    /// Deliberate single-replica assignment plus `selectionLocked: true`.
    /// A locked replica is rejected unless the request equals its saved pair,
    /// which returns UNCHANGED without publishing.
    IndividualAssignLock,
    /// Bulk assignment preserving flags; a locked replica is skipped.
    BulkOrdinary,
    /// Bulk assign-lock for unlocked replicas only; a locked replica is skipped.
    BulkAssignLockUnlockedOnly,
    /// Bulk assign-lock after an explicit review; locked replicas are written
    /// too (forced), and the flag ends true.
    BulkForceReviewed,
}

/// #1939 - a lock flip, tracked separately from the publication itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionLockTransition {
    UnlockedToLocked,
    LockedToUnlocked,
}

/// #1939 - outcome of a guarded selection mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionWriteOutcome {
    /// True when the guarded callback applied a semantic change.
    pub changed: bool,
    /// True when the funnel published a new config file.
    pub published: bool,
    /// The lock flip this write caused, when any.
    pub lock_transition: Option<SelectionLockTransition>,
}

impl SelectionWriteOutcome {
    /// Nothing to change and nothing of the caller's intent published
    /// (already-unlocked / skipped / same locked pair); a pending state
    /// migration may already have been.
    pub(super) const UNCHANGED: Self = Self {
        changed: false,
        published: false,
        lock_transition: None,
    };
    const WRITTEN: Self = Self {
        changed: true,
        published: true,
        lock_transition: None,
    };
}

/// #1939 - call-local typed marker carrying a no-publish guard outcome back
/// through the #1938 funnel's `Err` channel. The marker decides; the error text
/// is never inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectionGuardMarker {
    AlreadyUnlocked,
    SkippedLocked,
}

/// Private sentinel for the marker-only funnel exit; never displayed and never
/// string-matched.
pub(super) const NO_PUBLISH_SENTINEL: &str = "#1939 guarded selection no-publish exit";

pub(crate) fn selection_state_from_value(config: &Value, identity: &str) -> ReplicaSelectionState {
    let tooling = match config.get("tooling") {
        None => {
            return ReplicaSelectionState::Unlocked {
                identity: identity.to_string(),
                configuration_ref: None,
                profile_override_source: None,
                pair: None,
                warning: None,
            }
        }
        Some(Value::Object(tooling)) => tooling,
        Some(_) => {
            return ReplicaSelectionState::Invalid {
                diagnostic: "tooling must be a JSON object".to_string(),
            }
        }
    };

    let locked = match tooling.get("selectionLocked") {
        None => false,
        Some(Value::Bool(locked)) => *locked,
        Some(_) => {
            return ReplicaSelectionState::Invalid {
                diagnostic: "tooling.selectionLocked must be a boolean".to_string(),
            }
        }
    };

    let profile = tooling
        .get("profile")
        .and_then(Value::as_str)
        .and_then(normalize_profile_letter);
    let legacy = tooling
        .get("instanceProfileOverride")
        .and_then(Value::as_str)
        .and_then(normalize_profile_letter);
    let warning = match (&profile, &legacy) {
        (Some(profile), Some(legacy)) if profile != legacy => Some(format!(
            "tooling.profile ({}) differs from legacy instanceProfileOverride ({})",
            profile, legacy
        )),
        _ => None,
    };
    let coding_agent = tooling
        .get("currentCodingAgent")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let pair = match (coding_agent, profile.or(legacy)) {
        (Some(coding_agent_id), Some(requested_profile)) => Some(ReplicaSelectionPair {
            coding_agent_id,
            requested_profile,
        }),
        _ => None,
    };

    let configuration_ref = match configuration_ref_from_tooling(tooling, pair.as_ref()) {
        Ok(reference) => reference,
        Err(diagnostic) => return ReplicaSelectionState::Invalid { diagnostic },
    };
    let profile_override_source = match tooling.get("instanceProfileOverrideSource") {
        None => None,
        Some(Value::String(marker)) => Some(marker.clone()),
        Some(_) if configuration_ref.is_some() => {
            return ReplicaSelectionState::Invalid {
                diagnostic: "instanceProfileOverrideSource must be a string".into(),
            }
        }
        Some(_) => None,
    };
    match (locked, pair) {
        (true, None) => ReplicaSelectionState::Invalid {
            diagnostic: "tooling.selectionLocked is true but the saved selection pair is \
                         incomplete or invalid"
                .to_string(),
        },
        (true, Some(pair)) => ReplicaSelectionState::Locked {
            identity: identity.to_string(),
            configuration_ref,
            profile_override_source,
            pair,
            warning,
        },
        (false, pair) => ReplicaSelectionState::Unlocked {
            identity: identity.to_string(),
            configuration_ref,
            profile_override_source,
            pair,
            warning,
        },
    }
}

/// #1939 - strict read-only view of `replica_dir`'s stored selection. Uses the
/// existing strict reader (bounded path-safe read, duplicate-free JSON, object
/// and identity validation); every strict-read failure becomes `Invalid`.
pub fn read_replica_selection_state(replica_dir: &Path) -> ReplicaSelectionState {
    match crate::config::replica_identity::read_wg_replica_config_read_only(replica_dir) {
        Ok((config, identity)) => selection_state_from_value(&config, &identity.identity),
        Err(error) => ReplicaSelectionState::Invalid {
            diagnostic: format!("{}: {}", replica_dir.display(), error),
        },
    }
}

/// #1939 - Matrix creation default: `tooling.replicaSelectionDefault`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaSelectionDefault {
    pub coding_agent_id: String,
    pub requested_profile: String,
    pub selection_locked: bool,
    pub configuration_ref: Option<ConfigurationRef>,
}

impl ReplicaSelectionDefault {
    pub(super) fn normalized(self) -> Result<Self, String> {
        let requested_profile =
            normalize_profile_letter(&self.requested_profile).ok_or_else(|| {
                "replicaSelectionDefault.requestedProfile must be a single letter A through Z"
                    .to_string()
            })?;
        let coding_agent_id = self.coding_agent_id.trim().to_string();
        if coding_agent_id.is_empty() {
            return Err(
                "replicaSelectionDefault.codingAgentId must be a non-empty string".to_string(),
            );
        }
        if let Some(reference) = &self.configuration_ref {
            validate_configuration_ref_pair(
                reference,
                &ReplicaSelectionPair {
                    coding_agent_id: coding_agent_id.clone(),
                    requested_profile: requested_profile.clone(),
                },
            )?;
        }
        Ok(Self {
            coding_agent_id,
            requested_profile,
            selection_locked: self.selection_locked,
            configuration_ref: self.configuration_ref,
        })
    }
}

pub(super) fn parse_replica_selection_default(
    config: &Value,
) -> Result<Option<ReplicaSelectionDefault>, String> {
    let Some(tooling) = config.get("tooling") else {
        return Ok(None);
    };
    let tooling = tooling
        .as_object()
        .ok_or_else(|| "tooling must be a JSON object".to_string())?;
    let Some(default) = tooling.get("replicaSelectionDefault") else {
        return Ok(None);
    };
    let default = default
        .as_object()
        .ok_or_else(|| "tooling.replicaSelectionDefault must be a JSON object".to_string())?;
    let coding_agent_id = default
        .get("codingAgentId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "tooling.replicaSelectionDefault.codingAgentId must be a non-empty string".to_string()
        })?
        .to_string();
    let requested_profile = default
        .get("requestedProfile")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "tooling.replicaSelectionDefault.requestedProfile must be a single letter A through Z"
                .to_string()
        })?
        .to_string();
    let selection_locked = default
        .get("selectionLocked")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            "tooling.replicaSelectionDefault.selectionLocked must be a boolean".to_string()
        })?;
    let normalized = ReplicaSelectionDefault {
        coding_agent_id,
        requested_profile,
        selection_locked,
        configuration_ref: default
            .get("configurationRef")
            .map(|raw| serde_json::from_value(raw.clone()).map_err(|e| e.to_string()))
            .transpose()?,
    }
    .normalized()?;
    Ok(Some(normalized))
}

/// #1939 - read the Matrix creation default from one atomic JSON snapshot.
///
/// A missing Matrix config, missing `tooling`, or missing
/// `tooling.replicaSelectionDefault` all mean "old creation behavior"
/// (`Ok(None)`). A present default must be complete and valid; an invalid one
/// is an error, never silently ignored.
pub fn read_replica_selection_default(
    matrix_dir: &Path,
) -> Result<Option<ReplicaSelectionDefault>, String> {
    let config_path = matrix_dir.join("config.json");
    let bytes = match read_selection_default_bytes(&config_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Failed to read Matrix config {}: {}",
                config_path.display(),
                error
            ))
        }
    };
    let config: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Matrix config {} is not valid JSON: {}",
            config_path.display(),
            error
        )
    })?;
    if !config.is_object() {
        return Err(format!(
            "Matrix config {} must be a JSON object",
            config_path.display()
        ));
    }
    parse_replica_selection_default(&config)
        .map_err(|error| format!("Matrix config {}: {}", config_path.display(), error))
}

/// #1939 - validate a default and write `tooling.replicaSelectionDefault` under
/// the #1938 sidecar. `expected_prior` is the CAS value read before the call; a
/// mismatch is a stale error and publishes nothing.
pub fn write_replica_selection_default(
    matrix_dir: &Path,
    default: &ReplicaSelectionDefault,
    expected_prior: Option<&ReplicaSelectionDefault>,
) -> Result<SelectionWriteOutcome, String> {
    let normalized = default.clone().normalized()?;
    let config_path = matrix_dir.join("config.json");
    crate::config::agent_config::update_agent_config(&config_path, |obj, _state| {
        let current = read_replica_selection_default(matrix_dir)?;
        if current.as_ref() != expected_prior {
            return Err(format!(
                "stale replicaSelectionDefault at {}: expected {}, found {}",
                config_path.display(),
                describe_selection_default(expected_prior),
                describe_selection_default(current.as_ref()),
            ));
        }
        let tooling_value = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let tooling = tooling_value
            .as_object_mut()
            .ok_or_else(|| format!("tooling must be a JSON object at {}", config_path.display()))?;
        let mut value = serde_json::json!({
            "codingAgentId": normalized.coding_agent_id,
            "requestedProfile": normalized.requested_profile,
            "selectionLocked": normalized.selection_locked,
        });
        if let Some(reference) = &normalized.configuration_ref {
            value["configurationRef"] =
                serde_json::to_value(reference).map_err(|e| e.to_string())?;
        }
        tooling.insert("replicaSelectionDefault".to_string(), value);
        Ok(())
    })?;
    Ok(SelectionWriteOutcome::WRITTEN)
}

/// #1939 - materialize a validated Matrix default into a first-creation replica
/// config object. The caller owns the surrounding guarded write.
pub(crate) fn apply_replica_selection_default(
    config: &mut Value,
    default: &ReplicaSelectionDefault,
) -> Result<(), String> {
    let default = &default.clone().normalized()?;
    let root = config
        .as_object_mut()
        .ok_or_else(|| "Replica config must be a JSON object".to_string())?;
    let tooling_value = root
        .entry("tooling".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let tooling = tooling_value
        .as_object_mut()
        .ok_or_else(|| "Replica tooling must be a JSON object".to_string())?;
    tooling.insert(
        "currentCodingAgent".to_string(),
        Value::String(default.coding_agent_id.clone()),
    );
    tooling.insert(
        "profile".to_string(),
        Value::String(default.requested_profile.clone()),
    );
    tooling.insert(
        "instanceProfileOverride".to_string(),
        Value::String(default.requested_profile.clone()),
    );
    tooling.insert(
        "instanceProfileOverrideSource".to_string(),
        Value::String(
            if default
                .configuration_ref
                .as_ref()
                .is_some_and(|reference| reference.requested_profile.is_none())
            {
                "source-default"
            } else {
                "manual"
            }
            .to_string(),
        ),
    );
    tooling.insert(
        "selectionLocked".to_string(),
        Value::Bool(default.selection_locked),
    );
    match &default.configuration_ref {
        Some(reference) => {
            tooling.insert(
                "configurationRef".into(),
                serde_json::to_value(reference).map_err(|e| e.to_string())?,
            );
        }
        None => {
            tooling.remove("configurationRef");
        }
    }
    Ok(())
}

pub(super) fn describe_selection_default(default: Option<&ReplicaSelectionDefault>) -> String {
    match default {
        Some(default) => format!(
            "agent '{}' profile {} locked={}",
            default.coding_agent_id, default.requested_profile, default.selection_locked
        ),
        None => "absent".to_string(),
    }
}

pub(super) fn describe_selection_pair(pair: Option<&ReplicaSelectionPair>) -> String {
    match pair {
        Some(pair) => format!(
            "agent '{}' profile {}",
            pair.coding_agent_id, pair.requested_profile
        ),
        None => "absent".to_string(),
    }
}
