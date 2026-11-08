use super::pair_image::{
    prepared_pair_image, read_config_pair_physical, ConfigPairCleanup, PairTargetGuard,
    PhysicalState, PreparedPairError,
};
use super::pair_revision::{
    changed_pair_sides, merge_monotonic_activity, pair_plan_digest, pair_target_identity,
    protected_matches, protected_plan, protected_revision, read_pair_reservation, read_pair_tuple,
    timestamp_physical, timestamp_value, validate_pair_binding, validate_pair_paths,
    PairActivityIntent, PairReservation, PairReservationRequest, TimestampPhysical,
};
use super::prepared_pair::{
    prepare_config_pair_plan_inner, publish_prepared_pair_image,
    publish_prepared_pair_image_guarded,
};
use super::source_publication::publish_coordination_bytes_guarded;
use super::write_lock::{
    acquire_config_file_write_lock, lock_local_config_writes, ConfigWriterActive,
    CONFIG_LOCK_TIMEOUT,
};
use crate::filesystem::local_config;
use serde_json::Map;
use serde_json::Value;
use std::path::Path;

pub(super) fn publish_pair_reservation(
    path: &Path,
    physical: &mut PhysicalState,
    packet: &PairReservation,
) -> Result<(), PreparedPairError> {
    publish_pair_reservation_guarded(path, physical, packet, &PairTargetGuard::new(&|| Ok(())))
}

pub(super) fn publish_pair_reservation_guarded(
    path: &Path,
    physical: &mut PhysicalState,
    packet: &PairReservation,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    packet.validate()?;
    let mut bytes =
        serde_json::to_vec_pretty(packet).map_err(|e| PreparedPairError::Io(e.to_string()))?;
    bytes.push(b'\n');
    publish_coordination_bytes_guarded(path, physical, &bytes, guard)?;
    *physical = PhysicalState::from_bytes(bytes);
    guard.check()?;
    Ok(())
}

/// Source/C proofs and inventory are acquired by the caller before entry.
/// Snapshot, planning and the one complete packet publication never unlock.
#[cfg(test)]
pub(crate) fn prepare_and_reserve_config_pair<F>(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    request: &PairReservationRequest,
    cleanup: Option<ConfigPairCleanup<'_>>,
    mutate: F,
) -> Result<PairReservation, PreparedPairError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    prepare_and_reserve_config_pair_guarded(
        decisions,
        state,
        reservation,
        request,
        cleanup,
        mutate,
        &|| Ok(()),
    )
}

pub(crate) fn prepare_and_reserve_config_pair_guarded<F>(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    request: &PairReservationRequest,
    cleanup: Option<ConfigPairCleanup<'_>>,
    mutate: F,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<PairReservation, PreparedPairError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    let _writer = ConfigWriterActive::enter("prepare_and_reserve_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    guard.check()?;
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let existing = read_pair_reservation(reservation)?;
    guard.check()?;
    let current = read_pair_tuple(decisions, state)?;
    let before = existing
        .as_ref()
        .map(|(packet, _)| packet.plan_images.stages[0].clone())
        .unwrap_or(current.clone());
    if request.before_protected
        != [
            protected_revision(&before[0], false)?,
            protected_revision(&before[1], true)?,
        ]
    {
        return Err(PreparedPairError::Conflict);
    }
    let plan = prepare_config_pair_plan_inner(before, cleanup, mutate)?;
    guard.check()?;
    let images = protected_plan(&plan)?;
    let candidate = PairReservation {
        version: 2,
        operation_id: request.operation_id.clone(),
        source_physical_key: request.source_physical_key.clone(),
        plan_digest: pair_plan_digest(&plan, &request.mappings_digest)?,
        physical_target_identity: pair_target_identity(decisions, state)?,
        protected_policy_version: 1,
        mappings_digest: request.mappings_digest.clone(),
        before_tuple: plan.stages[0].clone(),
        protected_images: images,
        changed_sides: changed_pair_sides(&plan),
        timestamp_floor: timestamp_value(&current[1])?,
        activity_intent: None,
        plan_images: plan,
    };
    candidate.validate()?;
    candidate.activity_max(&current, None)?;
    if let Some((mut packet, mut physical)) = existing {
        validate_pair_binding(decisions, state, &packet)?;
        if packet.operation_id != candidate.operation_id
            || packet.source_physical_key != candidate.source_physical_key
            || packet.plan_digest != candidate.plan_digest
            || packet.before_tuple != candidate.before_tuple
        {
            return Err(PreparedPairError::Conflict);
        }
        reconcile_pair_activity_guarded(
            decisions,
            state,
            reservation,
            &mut packet,
            &mut physical,
            &guard,
        )?;
        guard.check()?;
        packet.recognize(&read_pair_tuple(decisions, state)?)?;
        guard.check()?;
        return Ok(packet);
    }
    candidate.recognize(&current)?;
    let mut physical = PhysicalState::Absent;
    publish_pair_reservation_guarded(reservation, &mut physical, &candidate, &guard)?;
    guard.check()?;
    Ok(candidate)
}

pub(super) fn image_with_timestamp(
    image: &PhysicalState,
    timestamp: Option<String>,
) -> Result<PhysicalState, PreparedPairError> {
    let before = image.map()?;
    let mut after = before.clone();
    if let Some(timestamp) = timestamp {
        let tooling = after
            .entry("tooling")
            .or_insert_with(|| serde_json::json!({}));
        tooling
            .as_object_mut()
            .ok_or(PreparedPairError::Conflict)?
            .insert("lastAgentMessageAt".into(), Value::String(timestamp));
    }
    prepared_pair_image(image, &before, &after)
}

pub(super) fn reconcile_pair_activity(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
) -> Result<(), PreparedPairError> {
    reconcile_pair_activity_guarded(
        decisions,
        state,
        reservation,
        packet,
        physical,
        &PairTargetGuard::new(&|| Ok(())),
    )
}

pub(super) fn reconcile_pair_activity_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    let Some(intent) = packet.activity_intent.clone() else {
        return Ok(());
    };
    let current = read_pair_tuple(decisions, state)?;
    packet.recognize(&current)?;
    let actual = timestamp_physical(&current[1])?;
    if actual == intent.before_timestamp_physical {
        let after = image_with_timestamp(&current[1], Some(intent.after_timestamp.clone()))?;
        publish_prepared_pair_image_guarded(state, &current[1], &after, guard)?;
    } else if actual != TimestampPhysical::Value(intent.after_timestamp.clone()) {
        return Err(PreparedPairError::Conflict);
    }
    packet.timestamp_floor = Some(intent.after_timestamp);
    packet.activity_intent = None;
    guard.check()?;
    packet.recognize(&read_pair_tuple(decisions, state)?)?;
    publish_pair_reservation_guarded(reservation, physical, packet, guard)
}

pub(super) fn land_pair_activity(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
    input: &str,
    on_stage: &dyn Fn(&str) -> Result<(), PreparedPairError>,
) -> Result<bool, PreparedPairError> {
    reconcile_pair_activity(decisions, state, reservation, packet, physical)?;
    let current = read_pair_tuple(decisions, state)?;
    packet.recognize(&current)?;
    let maximum = packet
        .activity_max(&current, Some(input.to_string()))?
        .ok_or(PreparedPairError::Conflict)?;
    if timestamp_value(&current[1])? == Some(maximum.clone()) {
        return Ok(false);
    }
    let after = image_with_timestamp(&current[1], Some(maximum.clone()))?;
    if !protected_matches(&current[1], &after, true)? {
        return Err(PreparedPairError::Conflict);
    }
    packet.activity_intent = Some(PairActivityIntent {
        before_timestamp_physical: timestamp_physical(&current[1])?,
        after_timestamp: maximum.clone(),
        floor_before: packet.timestamp_floor.clone(),
    });
    publish_pair_reservation(reservation, physical, packet)?;
    on_stage("activity_intent_durable")?;
    publish_prepared_pair_image(state, &current[1], &after)?;
    on_stage("activity_state_durable")?;
    reconcile_pair_activity(decisions, state, reservation, packet, physical)?;
    on_stage("activity_floor_durable")?;
    Ok(true)
}

/// Execute pending stages of the original operation under current target authority.
#[cfg(test)]
pub(crate) fn execute_reserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
) -> Result<(), PreparedPairError> {
    execute_reserved_config_pair_with_stage(
        decisions,
        state,
        reservation,
        operation_id,
        &|_| Ok(()),
    )
}

/// Verify the original target without applying stages or consolidating activity.
#[cfg(test)]
pub(crate) fn verify_reserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    plan_digest: &str,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    verify_reserved_config_pair_guarded(
        decisions,
        state,
        reservation,
        operation_id,
        plan_digest,
        &|| Ok(()),
    )
}

pub(crate) fn verify_reserved_config_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    plan_digest: &str,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    let _writer = ConfigWriterActive::enter("verify_reserved_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    guard.check()?;
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let (packet, _) = read_pair_reservation(reservation)?.ok_or(PreparedPairError::Conflict)?;
    validate_pair_binding(decisions, state, &packet)?;
    if packet.operation_id != operation_id || packet.plan_digest != plan_digest {
        return Err(PreparedPairError::Conflict);
    }
    guard.check()?;
    let current = read_pair_tuple(decisions, state)?;
    if packet.recognize(&current)? != 4 || packet.activity_intent.is_some() {
        return Err(PreparedPairError::Pending);
    }
    guard.check()?;
    Ok(current)
}

#[cfg(test)]
pub(super) fn execute_reserved_config_pair_with_stage(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    on_stage: &dyn Fn(usize) -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    execute_reserved_config_pair_with_validation(
        decisions,
        state,
        reservation,
        operation_id,
        on_stage,
        &|| Ok(()),
    )
}

pub(super) fn execute_reserved_config_pair_with_validation(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    on_stage: &dyn Fn(usize) -> Result<(), PreparedPairError>,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    let _writer = ConfigWriterActive::enter("execute_reserved_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    guard.check()?;
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let (mut packet, mut physical) =
        read_pair_reservation(reservation)?.ok_or(PreparedPairError::Conflict)?;
    validate_pair_binding(decisions, state, &packet)?;
    if packet.operation_id != operation_id {
        return Err(PreparedPairError::Conflict);
    }
    reconcile_pair_activity_guarded(
        decisions,
        state,
        reservation,
        &mut packet,
        &mut physical,
        &guard,
    )?;
    guard.check()?;
    let start = packet.recognize(&read_pair_tuple(decisions, state)?)?;
    confirm_pair_activity_floor_guarded(state, reservation, &mut packet, &mut physical, &guard)?;
    for stage in start + 1..5 {
        guard.check()?;
        let current = read_pair_tuple(decisions, state)?;
        let observed = packet.recognize(&current)?;
        if observed >= stage {
            continue;
        }
        if observed != stage - 1 {
            return Err(PreparedPairError::Conflict);
        }
        let side = usize::from(stage == 1 || stage == 3);
        let planned = &packet.plan_images.stages[stage][side];
        let after = if side == 1 {
            image_with_timestamp(planned, packet.activity_max(&current, None)?)?
        } else {
            planned.clone()
        };
        if after != current[side] {
            let mut proposed = current.clone();
            proposed[side] = after.clone();
            if packet.recognize(&proposed)? < stage {
                return Err(PreparedPairError::Conflict);
            }
            let path = if side == 0 { decisions } else { state };
            // Absence plus an activity overlay is already the planned absent
            // side; it must never be removed to reproduce the original bytes.
            if after == PhysicalState::Absent {
                return Err(PreparedPairError::Conflict);
            }
            publish_prepared_pair_image_guarded(path, &current[side], &after, &guard)?;
            guard.check()?;
            if packet.recognize(&read_pair_tuple(decisions, state)?)? < stage {
                return Err(PreparedPairError::Conflict);
            }
            confirm_pair_activity_floor_guarded(
                state,
                reservation,
                &mut packet,
                &mut physical,
                &guard,
            )?;
            on_stage(stage)?;
        }
    }
    confirm_pair_activity_floor_guarded(state, reservation, &mut packet, &mut physical, &guard)?;
    guard.check()?;
    if packet.recognize(&read_pair_tuple(decisions, state)?)? != 4 {
        return Err(PreparedPairError::Conflict);
    }
    guard.check()?;
    Ok(())
}

pub(super) fn confirm_pair_activity_floor_guarded(
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    let floor = merge_monotonic_activity(&[
        packet.timestamp_floor.clone(),
        timestamp_value(&read_config_pair_physical(state)?)?,
    ])?;
    if floor != packet.timestamp_floor {
        packet.timestamp_floor = floor;
        publish_pair_reservation_guarded(reservation, physical, packet, guard)?;
    }
    Ok(())
}

/// IO consumes a proof assembled above PAIR from an authenticated, validated
/// local complete receipt under C. It never reads another config directory.
#[derive(Clone, Debug)]
pub(crate) struct PairCompleteProof {
    operation_id: String,
    source_physical_key: String,
    target_id: String,
    plan_digest: String,
}

impl PairCompleteProof {
    pub(crate) fn from_single_editor_complete(
        projection: &Value,
        operation_id: &str,
        target_id: &str,
        current_source_revision: &Value,
    ) -> Result<Self, PreparedPairError> {
        let receipt = projection
            .get("receipt")
            .filter(|v| v.is_object())
            .ok_or(PreparedPairError::Conflict)?;
        let after = projection
            .get("afterSourceRevision")
            .filter(|v| {
                v.get("kind").and_then(Value::as_str) == Some("bytes")
                    && v.get("sha256").and_then(Value::as_str).is_some_and(|h| {
                        h.len() == 64
                            && h.bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
            })
            .ok_or(PreparedPairError::Conflict)?;
        if projection.get("schemaVersion").and_then(Value::as_u64) != Some(2)
            || projection.get("operationId").and_then(Value::as_str) != Some(operation_id)
            || projection.get("phase").and_then(Value::as_str) != Some("complete")
            || [
                "packetsPrepared",
                "sourcePublished",
                "refsVerified",
                "reloaded",
            ]
            .iter()
            .any(|key| projection.get(key).and_then(Value::as_bool) != Some(true))
            || receipt.get("operationId").and_then(Value::as_str) != Some(operation_id)
            || receipt.get("sourceRevision") != Some(after)
            || current_source_revision != after
        {
            return Err(PreparedPairError::Conflict);
        }
        let rows = projection
            .get("packets")
            .and_then(Value::as_array)
            .ok_or(PreparedPairError::Conflict)?;
        let mut matches = rows
            .iter()
            .filter(|row| row.get("targetId").and_then(Value::as_str) == Some(target_id));
        let target = matches.next().ok_or(PreparedPairError::Conflict)?;
        if matches.next().is_some() {
            return Err(PreparedPairError::Conflict);
        }
        let text = |value: &Value, key: &str| -> Result<String, PreparedPairError> {
            value
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or(PreparedPairError::Conflict)
        };
        Ok(Self {
            operation_id: operation_id.into(),
            target_id: target_id.into(),
            source_physical_key: text(projection, "physicalSourceKey")?,
            plan_digest: text(target, "planDigest")?,
        })
    }
}

/// Complete receipt is necessary but insufficient: release also verifies the
/// own final protected tuple and confirmed floor before removing this packet.
#[cfg(test)]
pub(crate) fn release_reserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    proof: &PairCompleteProof,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    release_reserved_config_pair_guarded(decisions, state, reservation, proof, &|| Ok(()))
}

pub(crate) fn release_reserved_config_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    proof: &PairCompleteProof,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    let _writer = ConfigWriterActive::enter("release_reserved_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    guard.check()?;
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    if proof.target_id != pair_target_identity(decisions, state)? {
        return Err(PreparedPairError::Conflict);
    }
    let Some((mut packet, mut physical)) = read_pair_reservation(reservation)? else {
        let current = read_pair_tuple(decisions, state)?;
        guard.check()?;
        return Ok(current);
    };
    validate_pair_binding(decisions, state, &packet)?;
    if packet.operation_id != proof.operation_id
        || packet.source_physical_key != proof.source_physical_key
        || packet.plan_digest != proof.plan_digest
    {
        return Err(PreparedPairError::Conflict);
    }
    reconcile_pair_activity_guarded(
        decisions,
        state,
        reservation,
        &mut packet,
        &mut physical,
        &guard,
    )?;
    guard.check()?;
    let current = read_pair_tuple(decisions, state)?;
    if packet.recognize(&current)? != 4 {
        return Err(PreparedPairError::Conflict);
    }
    guard.check()?;
    local_config::remove_local_config_stage(reservation)
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    guard.check()?;
    #[cfg(not(windows))]
    local_config::sync_local_config_directory(
        reservation
            .parent()
            .ok_or(PreparedPairError::InvalidPlan("reservation parent"))?,
    )
    .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    guard.check()?;
    Ok(current)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn execute_reserved_config_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    execute_reserved_config_pair_with_validation(
        decisions,
        state,
        reservation,
        operation_id,
        &|_| Ok(()),
        validate,
    )
}
