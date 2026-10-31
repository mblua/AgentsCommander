use super::{
    acquire_config_file_write_lock, land_pair_activity, lock_local_config_writes,
    prepared_pair_image, protected_revision, publish_pair_sides, publish_temp_config,
    read_pair_reservation, read_pair_side, read_pair_tuple, timestamp_cmp, timestamp_only_overlay,
    timestamp_value, validate_pair_binding, validate_pair_paths, ConfigWriterActive, PhysicalState,
    CONFIG_LOCK_TIMEOUT,
};
use crate::filesystem::local_config;
use serde_json::Map;
use serde_json::Value;
use std::cell::Cell;
use std::path::Path;

/// #2786 (C1) - the caller-supplied cleanup sequence of `update_config_pair`:
/// it edits the decisions map and the state map before `mutate` runs.
pub(crate) type ConfigPairCleanup<'a> =
    &'a dyn Fn(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>;

pub(in crate::config) type ConfigPublisher<'a> = &'a dyn Fn(&Path, &Path) -> Result<(), String>;

pub(crate) type ConfigPairFinish<'a> = &'a dyn Fn(&mut Map<String, Value>) -> Result<(), String>;

/// #2786 (C1) - write the decisions file and the state file as one guarded
/// pair. `state` is the state file's path, `state_keys` the keys that live in
/// it, and `on_stage` a named pause point (a no-op in production); all three
/// are parameters so this module names no other module.
///
/// One critical section covers both sequences: the process `Mutex`, then the
/// decisions sidecar, then the state sidecar, each taken once for the whole
/// call. Inside it the optional `cleanup` runs first, then `mutate`, on the
/// same two maps read once. Each sequence publishes the state file before the
/// decisions file, and a side whose map the sequence did not change is
/// neither written nor created, so its stage does not fire. An absent state
/// file is an empty map; an unparseable one blocks the call before any write.
/// A `cleanup` error aborts before any write; a `mutate` error keeps whatever
/// the cleanup already published. The primitive moves, removes and marks no
/// key itself, deletes no file, and never rolls back or retries.
///
/// Neither closure may call a config writer: a nested call on the same thread
/// returns an error, and one waited on from another thread would deadlock.
#[cfg(test)]
pub(super) fn update_config_pair<F>(
    decisions: &Path,
    state: &Path,
    state_keys: &[&str],
    cleanup: Option<ConfigPairCleanup<'_>>,
    on_stage: &dyn Fn(&str),
    mutate: F,
) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    update_config_pair_inner(
        decisions,
        state,
        None,
        state_keys,
        cleanup,
        on_stage,
        mutate,
        &|_| Ok(()),
        None,
        None,
        &publish_temp_config,
    )
}

/// The production PAIR boundary. Pending deltas are computed on private maps
/// before cleanup or the caller's split-marker policy can change either file.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_config_pair_guarded<F>(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    state_keys: &[&str],
    nonpending_preflight: Option<&dyn Fn() -> Result<(), String>>,
    cleanup: Option<ConfigPairCleanup<'_>>,
    on_stage: &dyn Fn(&str),
    mutate: F,
    finish: ConfigPairFinish<'_>,
    activity: Option<(&str, &Cell<bool>)>,
) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    update_config_pair_inner(
        decisions,
        state,
        Some(reservation),
        state_keys,
        cleanup,
        on_stage,
        mutate,
        finish,
        activity,
        nonpending_preflight,
        &publish_temp_config,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::config) fn update_config_pair_inner<F>(
    decisions: &Path,
    state: &Path,
    reservation: Option<&Path>,
    state_keys: &[&str],
    cleanup: Option<ConfigPairCleanup<'_>>,
    on_stage: &dyn Fn(&str),
    mutate: F,
    finish: ConfigPairFinish<'_>,
    activity: Option<(&str, &Cell<bool>)>,
    nonpending_preflight: Option<&dyn Fn() -> Result<(), String>>,
    publish: ConfigPublisher<'_>,
) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    let _writer = ConfigWriterActive::enter("update_config_pair")?;
    let _guard = lock_local_config_writes();
    // C1 moves no key, so the list is carried for the cleanup policy of C2.
    let _ = state_keys;

    prepare_pair_paths(decisions, state, reservation)?;
    let _decisions_lock = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)?;
    // Every reservation publisher takes decisions first. Keep this lease while
    // classifying presence, so a packet cannot appear during ignore preflight.
    // Protect state/reservation artifacts before creating their sidecars, even
    // when a later read fails. Pending or unreadable packets never preflight.
    run_nonpending_preflight(reservation, nonpending_preflight)?;
    let _state_lock = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)?;

    let _reservation_lock = reservation
        .map(|path| acquire_config_file_write_lock(path, CONFIG_LOCK_TIMEOUT))
        .transpose()?;
    if let Some(path) = reservation {
        if let Some((mut packet, mut physical)) =
            read_pair_reservation(path).map_err(|e| e.to_string())?
        {
            validate_pair_binding(decisions, state, &packet).map_err(|e| e.to_string())?;
            if let Some((input, landed)) = activity {
                landed.set(
                    land_pair_activity(
                        decisions,
                        state,
                        path,
                        &mut packet,
                        &mut physical,
                        input,
                        &|_| Ok(()),
                    )
                    .map_err(|e| e.to_string())?,
                );
                return Ok(());
            }
            let current = read_pair_tuple(decisions, state).map_err(|e| e.to_string())?;
            packet.recognize(&current).map_err(|e| e.to_string())?;
            let mut d = current[0].map().map_err(|e| e.to_string())?;
            let mut s = current[1].map().map_err(|e| e.to_string())?;
            mutate(&mut d, &mut s)?;
            let proposed = [
                prepared_pair_image(
                    &current[0],
                    &current[0].map().map_err(|e| e.to_string())?,
                    &d,
                )
                .map_err(|e| e.to_string())?,
                prepared_pair_image(
                    &current[1],
                    &current[1].map().map_err(|e| e.to_string())?,
                    &s,
                )
                .map_err(|e| e.to_string())?,
            ];
            if proposed == current {
                return Ok(());
            }
            // Structural changes, history, selection and unknown fields all
            // remain frozen. Only the exact timestamp leaf may move forward.
            if proposed[0] != current[0]
                || protected_revision(&proposed[1], true).map_err(|e| e.to_string())?
                    != protected_revision(&current[1], true).map_err(|e| e.to_string())?
            {
                // Absent state permits only its timestamp-only overlay.
                if proposed[0] != current[0]
                    || current[1] != PhysicalState::Absent
                    || !timestamp_only_overlay(&proposed[1]).map_err(|e| e.to_string())?
                {
                    return Err("targetTransitionPending: PAIR is reserved".into());
                }
            }
            let input = timestamp_value(&proposed[1])
                .map_err(|e| e.to_string())?
                .ok_or_else(|| {
                    "targetTransitionPending: timestamp cannot be deleted".to_string()
                })?;
            if timestamp_value(&current[1])
                .map_err(|e| e.to_string())?
                .as_deref()
                .is_some_and(|old| timestamp_cmp(&input, old).is_ok_and(|order| order.is_lt()))
            {
                return Err("targetTransitionPending: timestamp cannot decrease".into());
            }
            land_pair_activity(
                decisions,
                state,
                path,
                &mut packet,
                &mut physical,
                &input,
                &|_| Ok(()),
            )
            .map_err(|e| e.to_string())?;
            return Ok(());
        }
    }

    let both = || format!("{} and {}", decisions.display(), state.display());
    let read = |path: &Path| {
        read_pair_side(path).map_err(|e| format!("Config pair {} blocked: {}", both(), e))
    };
    let mut decisions_map = read(decisions)?;
    let mut state_map = read(state)?;

    if let Some(cleanup) = cleanup {
        let decisions_before = decisions_map.clone();
        let state_before = state_map.clone();
        cleanup(&mut decisions_map, &mut state_map)
            .map_err(|e| format!("Config pair {} cleanup failed: {}", both(), e))?;
        publish_pair_sides(
            (decisions, &decisions_before, &decisions_map),
            (state, &state_before, &state_map),
            on_stage,
            ["after_cleanup_state_publish", "after_cleanup_tracked_write"],
            publish,
        )?;
    }

    let decisions_before = decisions_map.clone();
    let state_before = state_map.clone();
    mutate(&mut decisions_map, &mut state_map)?;
    finish(&mut state_map)?;
    publish_pair_sides(
        (decisions, &decisions_before, &decisions_map),
        (state, &state_before, &state_map),
        on_stage,
        ["after_caller_state_publish", "after_caller_decisions_write"],
        publish,
    )
}

fn prepare_pair_paths(
    decisions: &Path,
    state: &Path,
    reservation: Option<&Path>,
) -> Result<(), String> {
    for path in [decisions, state] {
        let parent = path
            .parent()
            .ok_or_else(|| format!("Local config {} has no parent directory", path.display()))?;
        local_config::ensure_local_config_parent(parent)
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    if let Some(path) = reservation {
        validate_pair_paths(decisions, state, path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn run_nonpending_preflight(
    reservation: Option<&Path>,
    nonpending_preflight: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<(), String> {
    if let Some(path) = reservation {
        if read_pair_reservation(path)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            if let Some(preflight) = nonpending_preflight {
                preflight()?;
            }
        }
    }
    Ok(())
}
