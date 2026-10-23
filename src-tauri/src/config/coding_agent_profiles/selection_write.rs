use super::paths::validate_profile_selection_agent_path;
use super::selection_state::describe_selection_pair;
use super::selection_state::{
    read_replica_selection_state, selection_state_from_value, PreparedSourceSelection,
    ReplicaSelectionExpectation, ReplicaSelectionPair, ReplicaSelectionState, SelectionGuardMarker,
    SelectionLockTransition, SelectionWriteIntent, SelectionWriteOutcome, NO_PUBLISH_SENTINEL,
};
use crate::config::settings::{normalize_profile_letter, AppSettings};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn validated_replica_dir(
    settings: &AppSettings,
    replica_path: &Path,
) -> Result<PathBuf, String> {
    let validated = validate_profile_selection_agent_path(settings, replica_path)?;
    let name = validated
        .launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !name.starts_with("__agent_") {
        return Err(format!(
            "Profile assignment target '{}' must be a Room replica",
            validated.launch_path.display()
        ));
    }
    Ok(validated.launch_path)
}

/// #1939 - strict reread plus expected-state comparison, executed INSIDE the
/// #1938 guarded callback (read-only; it never takes the mutation guard
/// recursively). Every mismatch is a stale error before any mutation.
pub(super) fn validate_selection_write_state(
    replica_dir: &Path,
    expected: &ReplicaSelectionExpectation,
) -> Result<(Option<ReplicaSelectionPair>, bool), String> {
    let (config, identity) = crate::config::replica_identity::read_wg_replica_config_read_only(
        replica_dir,
    )
    .map_err(|error| {
        format!(
            "stale replica selection at {}: {}",
            replica_dir.display(),
            error
        )
    })?;
    let (current_pair, current_locked) =
        match selection_state_from_value(&config, &identity.identity) {
            ReplicaSelectionState::Unlocked { pair, .. } => (pair, false),
            ReplicaSelectionState::Locked { pair, .. } => (Some(pair), true),
            ReplicaSelectionState::Invalid { diagnostic } => {
                return Err(format!(
                    "invalid replica selection at {}: {}",
                    replica_dir.display(),
                    diagnostic
                ))
            }
        };
    if identity.identity != expected.identity {
        return Err(format!(
            "stale replica selection at {}: expected identity '{}', found '{}'",
            replica_dir.display(),
            expected.identity,
            identity.identity
        ));
    }
    if current_pair != expected.pair {
        return Err(format!(
            "stale replica selection at {}: expected pair {}, found {}",
            replica_dir.display(),
            describe_selection_pair(expected.pair.as_ref()),
            describe_selection_pair(current_pair.as_ref())
        ));
    }
    let current = selection_state_from_value(&config, &identity.identity).expectation()?;
    if current.configuration_ref != expected.configuration_ref
        || current.profile_override_source != expected.profile_override_source
    {
        return Err("stale replica selection reference or profile marker".into());
    }
    if current_locked != expected.locked {
        return Err(format!(
            "stale replica selection at {}: expected selectionLocked={}, found selectionLocked={}",
            replica_dir.display(),
            expected.locked,
            current_locked
        ));
    }
    Ok((current_pair, current_locked))
}

/// #1939 - guarded selection write. The pair is written as one publish
/// (`currentCodingAgent`, `profile`, legacy `instanceProfileOverride`, source
/// `manual`); assign-lock intents add `selectionLocked: true` (a false->true
/// flip is reported in [`SelectionWriteOutcome::lock_transition`]) while
/// ordinary intents preserve the stored flag exactly. Unknown keys,
/// `lastCodingAgent` and `profileContentHash` are preserved.
pub fn write_replica_selection(
    settings: &AppSettings,
    replica_path: &Path,
    pair: &ReplicaSelectionPair,
    intent: SelectionWriteIntent,
    expected: &ReplicaSelectionExpectation,
) -> Result<SelectionWriteOutcome, String> {
    write_replica_selection_prepared(settings, replica_path, pair, intent, expected, None)
}

/// Dormant source companion; no source IO or runtime activation in this layer.
#[allow(dead_code)]
pub(crate) fn write_prepared_source_selection(
    settings: &AppSettings,
    replica_path: &Path,
    prepared: &PreparedSourceSelection,
    intent: SelectionWriteIntent,
    expected: &ReplicaSelectionExpectation,
) -> Result<SelectionWriteOutcome, String> {
    prepared.validate()?;
    if prepared.configuration_ref.requested_profile.is_none() {
        if let Some(prior) = &expected.pair {
            if expected.profile_override_source.as_deref() != Some("source-default") {
                if prepared.pair.requested_profile != prior.requested_profile
                    || prepared.profile_override_source != expected.profile_override_source
                {
                    return Err("source-null assignment must preserve the authoritative pin".into());
                }
            } else if prepared.profile_override_source.as_deref() != Some("source-default") {
                return Err("derived source-null aliases require source-default".into());
            }
        } else if prepared.profile_override_source.as_deref() != Some("source-default") {
            return Err("unpinned source-null aliases require source-default".into());
        }
    } else if prepared.profile_override_source.as_deref() != Some("manual") {
        return Err("explicit source aliases require manual".into());
    }
    write_replica_selection_prepared(
        settings,
        replica_path,
        &prepared.pair,
        intent,
        expected,
        Some(prepared),
    )
}

pub(super) fn write_replica_selection_prepared(
    settings: &AppSettings,
    replica_path: &Path,
    pair: &ReplicaSelectionPair,
    intent: SelectionWriteIntent,
    expected: &ReplicaSelectionExpectation,
    prepared: Option<&PreparedSourceSelection>,
) -> Result<SelectionWriteOutcome, String> {
    let profile = normalize_profile_letter(&pair.requested_profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?;
    if pair.coding_agent_id.trim().is_empty() {
        return Err("Coding agent id must not be empty".to_string());
    }
    let replica_dir = validated_replica_dir(settings, replica_path)?;
    let config_path = replica_dir.join("config.json");
    let requested_pair = ReplicaSelectionPair {
        coding_agent_id: pair.coding_agent_id.clone(),
        requested_profile: profile.clone(),
    };

    let marker = std::cell::Cell::new(None::<SelectionGuardMarker>);
    let transition = std::cell::Cell::new(None::<SelectionLockTransition>);

    let written =
        crate::config::agent_config::update_existing_agent_config(&config_path, |obj, _state| {
            let (current_pair, current_locked) =
                validate_selection_write_state(&replica_dir, expected)?;
            if current_locked {
                match intent {
                    SelectionWriteIntent::Individual
                    | SelectionWriteIntent::IndividualAssignLock => {
                        // #2010 - KEEP: the same stored pair is a no-op, not an
                        // error; the lock stays and nothing is published.
                        if current_pair.as_ref() == Some(&requested_pair)
                            && expected.configuration_ref.as_ref()
                                == prepared.map(|value| &value.configuration_ref)
                            && prepared.is_none_or(|value| {
                                value.profile_override_source == expected.profile_override_source
                            })
                        {
                            marker.set(Some(SelectionGuardMarker::SkippedLocked));
                            return Err(NO_PUBLISH_SENTINEL.to_string());
                        }
                        return Err(format!(
                            "replica selection at {} is locked; unlock it or use a reviewed force",
                            replica_dir.display()
                        ));
                    }
                    SelectionWriteIntent::BulkOrdinary
                    | SelectionWriteIntent::BulkAssignLockUnlockedOnly => {
                        // Skip locked replicas without publishing: the same
                        // private marker path as the already-unlocked clear.
                        marker.set(Some(SelectionGuardMarker::SkippedLocked));
                        return Err(NO_PUBLISH_SENTINEL.to_string());
                    }
                    SelectionWriteIntent::BulkForceReviewed => {}
                }
            }
            let target_locked = match intent {
                SelectionWriteIntent::Individual | SelectionWriteIntent::BulkOrdinary => {
                    current_locked
                }
                SelectionWriteIntent::IndividualAssignLock
                | SelectionWriteIntent::BulkAssignLockUnlockedOnly
                | SelectionWriteIntent::BulkForceReviewed => true,
            };
            if !current_locked && target_locked {
                transition.set(Some(SelectionLockTransition::UnlockedToLocked));
            }
            let tooling_value = obj
                .entry("tooling".to_string())
                .or_insert_with(|| serde_json::json!({}));
            let tooling = tooling_value.as_object_mut().ok_or_else(|| {
                format!(
                    "invalid replica selection at {}: tooling must be a JSON object",
                    replica_dir.display()
                )
            })?;
            tooling.insert(
                "currentCodingAgent".to_string(),
                Value::String(pair.coding_agent_id.clone()),
            );
            tooling.insert("profile".to_string(), Value::String(profile.clone()));
            tooling.insert(
                "instanceProfileOverride".to_string(),
                Value::String(profile.clone()),
            );
            publish_prepared_reference(tooling, prepared)?;
            if target_locked {
                tooling.insert("selectionLocked".to_string(), Value::Bool(true));
            }
            Ok(())
        });

    match written {
        Ok(_) => Ok(SelectionWriteOutcome {
            changed: true,
            published: true,
            lock_transition: transition.get(),
        }),
        Err(error) => match marker.get() {
            Some(SelectionGuardMarker::SkippedLocked) => Ok(SelectionWriteOutcome::UNCHANGED),
            _ => Err(error),
        },
    }
}

pub(super) fn publish_prepared_reference(
    tooling: &mut serde_json::Map<String, Value>,
    prepared: Option<&PreparedSourceSelection>,
) -> Result<(), String> {
    if let Some(prepared) = prepared {
        tooling.insert(
            "configurationRef".into(),
            serde_json::to_value(&prepared.configuration_ref).map_err(|e| e.to_string())?,
        );
        match &prepared.profile_override_source {
            Some(marker) => {
                tooling.insert(
                    "instanceProfileOverrideSource".into(),
                    Value::String(marker.clone()),
                );
            }
            None => {
                tooling.remove("instanceProfileOverrideSource");
            }
        }
    } else {
        tooling.remove("configurationRef");
        tooling.insert(
            "instanceProfileOverrideSource".into(),
            Value::String("manual".into()),
        );
    }
    Ok(())
}

/// Dedicated manual setter companion keeps the existing setter's lock treatment.
/// The caller has reauthorized/reloaded the source and prepared its fresh pair.
#[allow(dead_code)]
pub(crate) fn set_prepared_source_profile_override(
    settings: &AppSettings,
    replica_path: &Path,
    prepared: &PreparedSourceSelection,
    expected: &ReplicaSelectionExpectation,
) -> Result<(), String> {
    prepared.validate()?;
    let previous = expected
        .configuration_ref
        .as_ref()
        .ok_or("source reference required")?;
    if previous.source != prepared.configuration_ref.source
        || previous.calculated_id != prepared.configuration_ref.calculated_id
        || previous.legacy_entry_hint != prepared.configuration_ref.legacy_entry_hint
        || previous.requested_profile.is_none()
            != prepared.configuration_ref.requested_profile.is_none()
        || prepared.profile_override_source.as_deref() != Some("manual")
    {
        return Err("manual source override must preserve source and null intent".into());
    }
    let replica_dir = validated_replica_dir(settings, replica_path)?;
    crate::config::agent_config::update_existing_agent_config(
        &replica_dir.join("config.json"),
        |obj, _state| {
            validate_selection_write_state(&replica_dir, expected)?;
            let tooling = obj
                .get_mut("tooling")
                .and_then(Value::as_object_mut)
                .ok_or("tooling must be an object")?;
            tooling.insert(
                "profile".into(),
                Value::String(prepared.pair.requested_profile.clone()),
            );
            tooling.insert(
                "instanceProfileOverride".into(),
                Value::String(prepared.pair.requested_profile.clone()),
            );
            publish_prepared_reference(tooling, Some(prepared))
        },
    )?;
    Ok(())
}

/// #1939 - guarded unlock. Only the flag is written (`false`); the pair and
/// every other field stay untouched. An already-false/absent flag exits through
/// the private callback marker before any object mutation, so nothing of the
/// caller's intent is serialized or published, though a pending state migration
/// may already have been (see [`SelectionWriteOutcome`]). Every
/// other error — malformed state, stale expected state, lock timeout, IO —
/// propagates unchanged and can never be reported as success.
pub fn clear_replica_selection_lock(
    settings: &AppSettings,
    replica_path: &Path,
    expected: &ReplicaSelectionExpectation,
) -> Result<SelectionWriteOutcome, String> {
    clear_replica_selection_lock_with_publish(settings, replica_path, expected, None)
}

pub(super) fn clear_replica_selection_lock_with_publish(
    settings: &AppSettings,
    replica_path: &Path,
    expected: &ReplicaSelectionExpectation,
    publish: Option<crate::config::agent_config::ConfigPublisher<'_>>,
) -> Result<SelectionWriteOutcome, String> {
    let replica_dir = validated_replica_dir(settings, replica_path)?;
    let config_path = replica_dir.join("config.json");
    let marker = std::cell::Cell::new(None::<SelectionGuardMarker>);

    let written = crate::config::agent_config::update_existing_agent_config_with_publish(
        &config_path,
        |obj, _state| {
            let (_current_pair, current_locked) =
                validate_selection_write_state(&replica_dir, expected)?;
            if !current_locked {
                marker.set(Some(SelectionGuardMarker::AlreadyUnlocked));
                return Err(NO_PUBLISH_SENTINEL.to_string());
            }
            let tooling = obj
                .get_mut("tooling")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| {
                    format!(
                        "invalid replica selection at {}: tooling must be a JSON object",
                        replica_dir.display()
                    )
                })?;
            tooling.insert("selectionLocked".to_string(), Value::Bool(false));
            Ok(())
        },
        publish,
    );

    match written {
        Ok(_) => Ok(SelectionWriteOutcome {
            changed: true,
            published: true,
            lock_transition: Some(SelectionLockTransition::LockedToUnlocked),
        }),
        Err(error) => match marker.get() {
            Some(SelectionGuardMarker::AlreadyUnlocked) => Ok(SelectionWriteOutcome::UNCHANGED),
            _ => Err(error),
        },
    }
}

/// #1939 - deliberate per-replica assignment. Preserves the stored flag through
/// [`SelectionWriteIntent::Individual`] and cannot bypass a lock: a locked
/// replica with a different pair is rejected with the stored bytes untouched,
/// while the same stored pair is a no-op that returns without writing.
pub fn set_replica_coding_agent_selection(
    settings: &AppSettings,
    replica_path: &Path,
    coding_agent_id: &str,
    profile: &str,
) -> Result<(), String> {
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == coding_agent_id)
    {
        return Err(format!("Agent '{}' is not configured", coding_agent_id));
    }
    let profile = normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?;
    let replica_dir = validated_replica_dir(settings, replica_path)?;
    let state = read_replica_selection_state(&replica_dir);
    let expected = state.expectation()?;
    let pair = ReplicaSelectionPair {
        coding_agent_id: coding_agent_id.to_string(),
        requested_profile: profile,
    };
    write_replica_selection(
        settings,
        &replica_dir,
        &pair,
        SelectionWriteIntent::Individual,
        &expected,
    )?;
    Ok(())
}
