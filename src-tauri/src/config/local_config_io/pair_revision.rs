use super::pair_image::read_config_pair_physical;
use super::{PhysicalState, PreparedConfigPairPlan, PreparedPairError};
use crate::filesystem::local_config;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Canonical JSON with structural presence retained. Only the state timestamp
/// is removed; decisions, nulls, empty objects and unknown values remain data.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ProtectedRevision {
    Absent,
    Json(Value),
}

pub(super) fn protected_revision(
    image: &PhysicalState,
    state: bool,
) -> Result<ProtectedRevision, PreparedPairError> {
    if *image == PhysicalState::Absent {
        return Ok(ProtectedRevision::Absent);
    }
    let mut map = image.map()?;
    if state {
        timestamp_value(image)?;
        if let Some(tooling) = map.get_mut("tooling") {
            tooling
                .as_object_mut()
                .ok_or(PreparedPairError::Conflict)?
                .remove("lastAgentMessageAt");
        }
    }
    Ok(ProtectedRevision::Json(canonical_json(Value::Object(map))))
}

pub(super) fn canonical_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut rows: Vec<_> = map.into_iter().collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                rows.into_iter()
                    .map(|(key, value)| (key, canonical_json(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        value => value,
    }
}

pub(super) fn changed_pair_sides(plan: &PreparedConfigPairPlan) -> [bool; 2] {
    [0, 1].map(|side| {
        plan.stages
            .windows(2)
            .any(|images| images[0][side] != images[1][side])
    })
}

pub(super) fn timestamp_value(image: &PhysicalState) -> Result<Option<String>, PreparedPairError> {
    let map = image.map()?;
    let Some(tooling) = map.get("tooling") else {
        return Ok(None);
    };
    let tooling = tooling.as_object().ok_or(PreparedPairError::Conflict)?;
    let Some(value) = tooling.get("lastAgentMessageAt") else {
        return Ok(None);
    };
    let text = value.as_str().ok_or(PreparedPairError::Conflict)?;
    chrono::DateTime::parse_from_rfc3339(text).map_err(|_| PreparedPairError::Conflict)?;
    Ok(Some(text.to_string()))
}

pub(super) fn timestamp_cmp(a: &str, b: &str) -> Result<std::cmp::Ordering, PreparedPairError> {
    let a = chrono::DateTime::parse_from_rfc3339(a).map_err(|_| PreparedPairError::Conflict)?;
    let b = chrono::DateTime::parse_from_rfc3339(b).map_err(|_| PreparedPairError::Conflict)?;
    Ok(a.cmp(&b))
}

pub(crate) fn merge_monotonic_activity(
    values: &[Option<String>],
) -> Result<Option<String>, PreparedPairError> {
    let mut maximum: Option<String> = None;
    for value in values.iter().flatten() {
        timestamp_cmp(value, value)?;
        if maximum
            .as_deref()
            .map(|old| timestamp_cmp(value, old))
            .transpose()?
            .is_none_or(std::cmp::Ordering::is_gt)
        {
            maximum = Some(value.clone());
        }
    }
    Ok(maximum)
}

pub(super) fn timestamp_only_overlay(image: &PhysicalState) -> Result<bool, PreparedPairError> {
    let map = image.map()?;
    Ok(map.len() == 1
        && map
            .get("tooling")
            .and_then(Value::as_object)
            .is_some_and(|tooling| {
                tooling.len() == 1 && tooling.contains_key("lastAgentMessageAt")
            })
        && timestamp_value(image)?.is_some())
}

pub(super) fn protected_matches(
    expected: &PhysicalState,
    observed: &PhysicalState,
    state: bool,
) -> Result<bool, PreparedPairError> {
    if state && *expected == PhysicalState::Absent && *observed != PhysicalState::Absent {
        return timestamp_only_overlay(observed);
    }
    Ok(protected_revision(expected, state)? == protected_revision(observed, state)?)
}

/// Private caller data; authorization and C/source proofs belong above IO.
/// Mappings are bound by their canonical digest, never copied source commands.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PairReservationRequest {
    pub(crate) operation_id: String,
    pub(crate) source_physical_key: String,
    pub(crate) mappings_digest: String,
    pub(crate) before_protected: [ProtectedRevision; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub(super) enum TimestampPhysical {
    AbsentState,
    AbsentField,
    Value(String),
}

pub(super) fn timestamp_physical(
    image: &PhysicalState,
) -> Result<TimestampPhysical, PreparedPairError> {
    if *image == PhysicalState::Absent {
        return Ok(TimestampPhysical::AbsentState);
    }
    Ok(match timestamp_value(image)? {
        Some(value) => TimestampPhysical::Value(value),
        None => TimestampPhysical::AbsentField,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PairActivityIntent {
    pub(super) before_timestamp_physical: TimestampPhysical,
    pub(super) after_timestamp: String,
    pub(super) floor_before: Option<String>,
}

/// The single physical recovery authority: presence means complete images and
/// floor are durable. Owner and floor are deliberately outside planDigest.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PairReservation {
    pub(super) version: u32,
    pub(crate) operation_id: String,
    pub(crate) source_physical_key: String,
    pub(crate) plan_digest: String,
    pub(crate) physical_target_identity: String,
    pub(super) protected_policy_version: u32,
    pub(super) mappings_digest: String,
    pub(super) before_tuple: [PhysicalState; 2],
    pub(super) protected_images: [[ProtectedRevision; 2]; 5],
    pub(super) changed_sides: [bool; 2],
    pub(super) timestamp_floor: Option<String>,
    pub(super) activity_intent: Option<PairActivityIntent>,
    pub(super) plan_images: PreparedConfigPairPlan,
}

pub(super) fn protected_plan(
    plan: &PreparedConfigPairPlan,
) -> Result<[[ProtectedRevision; 2]; 5], PreparedPairError> {
    let mut images = Vec::new();
    for tuple in plan.stages() {
        images.push([
            protected_revision(&tuple[0], false)?,
            protected_revision(&tuple[1], true)?,
        ]);
    }
    images
        .try_into()
        .map_err(|_| PreparedPairError::InvalidPlan("PAIR stage count"))
}

pub(crate) fn protected_config_pair_revision(
    before: &[PhysicalState; 2],
) -> Result<[ProtectedRevision; 2], PreparedPairError> {
    Ok([
        protected_revision(&before[0], false)?,
        protected_revision(&before[1], true)?,
    ])
}

pub(super) fn pair_plan_digest(
    plan: &PreparedConfigPairPlan,
    mappings_digest: &str,
) -> Result<String, PreparedPairError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(1u32, protected_plan(plan)?, mappings_digest))
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl PairReservation {
    pub(crate) fn protected_revision_digests(&self) -> Result<[String; 2], PreparedPairError> {
        use sha2::{Digest, Sha256};
        let digest = |tuple: &[ProtectedRevision; 2]| -> Result<String, PreparedPairError> {
            Ok(format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(tuple).map_err(|e| PreparedPairError::Io(e.to_string()))?
                )
            ))
        };
        Ok([
            digest(&self.protected_images[0])?,
            digest(&self.protected_images[4])?,
        ])
    }

    pub(crate) fn validate(&self) -> Result<(), PreparedPairError> {
        self.plan_images.validate()?;
        if self.version != 2
            || self.protected_policy_version != 1
            || self.operation_id.is_empty()
            || self.source_physical_key.is_empty()
            || self.physical_target_identity.is_empty()
            || self.mappings_digest.is_empty()
            || self.plan_digest != pair_plan_digest(&self.plan_images, &self.mappings_digest)?
            || self.protected_images != protected_plan(&self.plan_images)?
            || self.before_tuple != self.plan_images.stages[0]
            || self.changed_sides != changed_pair_sides(&self.plan_images)
        {
            return Err(PreparedPairError::InvalidPlan("invalid reservation packet"));
        }
        merge_monotonic_activity(std::slice::from_ref(&self.timestamp_floor))?;
        if let Some(intent) = &self.activity_intent {
            if intent.floor_before != self.timestamp_floor
                || merge_monotonic_activity(&[
                    intent.floor_before.clone(),
                    Some(intent.after_timestamp.clone()),
                ])? != Some(intent.after_timestamp.clone())
            {
                return Err(PreparedPairError::Conflict);
            }
        }
        Ok(())
    }

    pub(super) fn recognize(
        &self,
        current: &[PhysicalState; 2],
    ) -> Result<usize, PreparedPairError> {
        let timestamp = timestamp_value(&current[1])?;
        if let Some(floor) = &self.timestamp_floor {
            if timestamp
                .as_deref()
                .map(|value| timestamp_cmp(value, floor))
                .transpose()?
                .is_none_or(std::cmp::Ordering::is_lt)
            {
                return Err(PreparedPairError::Conflict);
            }
        }
        for stage in (0..5).rev() {
            let expected = &self.plan_images.stages[stage];
            if protected_matches(&expected[0], &current[0], false)?
                && protected_matches(&expected[1], &current[1], true)?
            {
                let planned = if stage == 0 {
                    timestamp_value(&expected[1])?
                } else {
                    self.planned_activity_max()?
                };
                if let Some(planned) = planned {
                    if timestamp
                        .as_deref()
                        .map(|value| timestamp_cmp(value, &planned))
                        .transpose()?
                        .is_none_or(std::cmp::Ordering::is_lt)
                    {
                        continue;
                    }
                }
                return Ok(stage);
            }
        }
        Err(PreparedPairError::Conflict)
    }

    pub(crate) fn activity_max(
        &self,
        current: &[PhysicalState; 2],
        input: Option<String>,
    ) -> Result<Option<String>, PreparedPairError> {
        let mut values = vec![
            self.timestamp_floor.clone(),
            timestamp_value(&current[0])?,
            timestamp_value(&current[1])?,
            self.planned_activity_max()?,
            input,
        ];
        if let Some(intent) = &self.activity_intent {
            values.push(Some(intent.after_timestamp.clone()));
        }
        merge_monotonic_activity(&values)
    }

    fn planned_activity_max(&self) -> Result<Option<String>, PreparedPairError> {
        let mut values = vec![timestamp_value(&self.plan_images.stages[0][0])?];
        for tuple in &self.plan_images.stages {
            values.push(timestamp_value(&tuple[1])?);
        }
        merge_monotonic_activity(&values)
    }
}

pub(super) fn read_pair_tuple(
    decisions: &Path,
    state: &Path,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    Ok([
        read_config_pair_physical(decisions)?,
        read_config_pair_physical(state)?,
    ])
}

pub(super) fn pair_target_identity(
    decisions: &Path,
    state: &Path,
) -> Result<String, PreparedPairError> {
    let resolve = |path: &Path| -> Result<String, PreparedPairError> {
        let parent = path
            .parent()
            .ok_or(PreparedPairError::InvalidPlan("PAIR parent"))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(PreparedPairError::InvalidPlan("PAIR name"))?;
        let parent = local_config::canonical_local_config_parent(parent)
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        let path = parent.join(name).to_string_lossy().into_owned();
        Ok(if cfg!(windows) {
            path.to_lowercase()
        } else {
            path
        })
    };
    let d = resolve(decisions)?;
    let s = resolve(state)?;
    if d == s {
        return Err(PreparedPairError::InvalidPlan(
            "PAIR targets must be distinct",
        ));
    }
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(d, s)).map_err(|e| PreparedPairError::Io(e.to_string()))?
        )
    ))
}

pub(super) fn validate_pair_binding(
    decisions: &Path,
    state: &Path,
    packet: &PairReservation,
) -> Result<(), PreparedPairError> {
    packet.validate()?;
    if packet.physical_target_identity != pair_target_identity(decisions, state)? {
        return Err(PreparedPairError::Conflict);
    }
    Ok(())
}

pub(super) fn validate_pair_paths(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
) -> Result<(), PreparedPairError> {
    pair_target_identity(decisions, state)?;
    let resolve = |path: &Path| -> Result<PathBuf, PreparedPairError> {
        let parent = local_config::canonical_local_config_parent(
            path.parent()
                .ok_or(PreparedPairError::InvalidPlan("PAIR parent"))?,
        )
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        Ok(parent.join(
            path.file_name()
                .ok_or(PreparedPairError::InvalidPlan("PAIR name"))?,
        ))
    };
    let paths = [resolve(decisions)?, resolve(state)?, resolve(reservation)?];
    if paths.iter().any(|path| path.parent() != paths[0].parent())
        || paths[..2].iter().any(|path| {
            path == &paths[2]
                || (cfg!(windows)
                    && path.to_string_lossy().to_lowercase()
                        == paths[2].to_string_lossy().to_lowercase())
        })
    {
        return Err(PreparedPairError::InvalidPlan(
            "reservation must be a distinct PAIR sibling",
        ));
    }
    Ok(())
}

pub(super) fn read_pair_reservation(
    path: &Path,
) -> Result<Option<(PairReservation, PhysicalState)>, PreparedPairError> {
    let physical = read_config_pair_physical(path)?;
    let PhysicalState::Bytes { bytes, .. } = &physical else {
        return Ok(None);
    };
    let packet: PairReservation = serde_json::from_slice(bytes)
        .map_err(|_| PreparedPairError::InvalidPlan("invalid reservation packet"))?;
    packet.validate()?;
    Ok(Some((packet, physical)))
}
