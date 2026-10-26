use super::super::session::SessionStatus;
use super::binding::binding_matches;
use super::{
    CommitDecision, CommitResult, FinalizeMutation, LifecycleMutations, SessionManagerState,
};
use crate::session::selection::payload::{SelectionMode, SessionSelection};
use std::collections::HashSet;
use uuid::Uuid;

pub(super) fn append_finalized_live_activity(
    state: &SessionManagerState,
    finalized_live_ids: Vec<Uuid>,
    result: &mut CommitResult,
) {
    for id in finalized_live_ids {
        if let Some(record) = state.sessions.get(&id) {
            if crate::session::session::is_working(record) {
                result
                    .activity
                    .push(crate::config::activity_log::build_busy(
                        id,
                        record,
                        crate::config::activity_log::BusyReason::SessionStart,
                    ));
            }
        }
    }
}

/// Lifecycle mutation sets in removals, exits, finalizations order.
pub(super) type LifecycleMutationSets = (HashSet<Uuid>, HashSet<Uuid>, HashSet<Uuid>);

pub(super) fn validate_lifecycle_mutations(
    state: &SessionManagerState,
    mutations: &LifecycleMutations,
) -> Result<LifecycleMutationSets, String> {
    validate_unique_ids("removal", mutations.removals.iter().copied())?;
    validate_unique_ids(
        "markExited",
        mutations.mark_exited.iter().map(|(id, _)| *id),
    )?;
    validate_unique_ids(
        "detachedIntent",
        mutations.detached_intent.iter().map(|(id, _)| *id),
    )?;
    validate_unique_ids(
        "finalization",
        mutations
            .finalizations
            .iter()
            .map(|mutation| match mutation {
                FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                    binding.session_id
                }
            }),
    )?;

    let removals = mutations.removals.iter().copied().collect::<HashSet<_>>();
    let exits = mutations
        .mark_exited
        .iter()
        .map(|(id, _)| *id)
        .collect::<HashSet<_>>();
    let finalizations = mutations
        .finalizations
        .iter()
        .map(|mutation| match mutation {
            FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                binding.session_id
            }
        })
        .collect::<HashSet<_>>();

    if let Some(overlap) = removals.intersection(&exits).next() {
        return Err(format!(
            "lifecycle mutation remove+markExited conflict: {overlap}"
        ));
    }
    if let Some(overlap) = removals.intersection(&finalizations).next() {
        return Err(format!(
            "lifecycle mutation remove+finalize conflict: {overlap}"
        ));
    }
    if let Some(overlap) = exits.intersection(&finalizations).next() {
        return Err(format!(
            "lifecycle mutation markExited+finalize conflict: {overlap}"
        ));
    }
    for id in removals.iter().chain(exits.iter()) {
        if state.pending_create.contains_key(id) {
            return Err(format!(
                "public lifecycle mutation cannot address pending create: {id}"
            ));
        }
    }

    for mutation in &mutations.finalizations {
        match *mutation {
            FinalizeMutation::Live(binding, witness) => {
                if !binding_matches(state, binding) {
                    return Err(format!(
                        "create finalization capability mismatch: {}",
                        binding.session_id
                    ));
                }
                if witness.session_id != binding.session_id || !witness.has_pty || witness.detached
                {
                    return Err(format!(
                        "live create finalization has invalid runtime witness: {}",
                        binding.session_id
                    ));
                }
                let record = state.sessions.get(&binding.session_id).ok_or_else(|| {
                    format!("pending create record missing: {}", binding.session_id)
                })?;
                if matches!(record.status, SessionStatus::Exited(_)) {
                    return Err(format!(
                        "live create finalization targets exited record: {}",
                        binding.session_id
                    ));
                }
            }
            FinalizeMutation::Dormant(binding, witness, _) => {
                if !binding_matches(state, binding)
                    || witness.session_id != binding.session_id
                    || witness.detached
                {
                    return Err(format!(
                        "dormant create finalization has invalid capability/witness: {}",
                        binding.session_id
                    ));
                }
            }
        }
    }
    validate_unique_ids(
        "prompt transfer source",
        mutations.prompt_transfers.iter().map(|(id, _)| *id),
    )?;
    validate_unique_ids(
        "prompt transfer target",
        mutations
            .prompt_transfers
            .iter()
            .map(|(_, binding)| binding.session_id),
    )?;
    for (source, target) in &mutations.prompt_transfers {
        let from = state
            .sessions
            .get(source)
            .ok_or("prompt transfer source missing")?;
        let to = state
            .sessions
            .get(&target.session_id)
            .ok_or("prompt transfer target missing")?;
        if !removals.contains(source)
            || !finalizations.contains(&target.session_id)
            || !binding_matches(state, *target)
            || from.start_fresh_on_restore
            || to.start_fresh_on_restore
            || from.agent_id != to.agent_id
            || from.requested_profile != to.requested_profile
            || from.effective_profile != to.effective_profile
            || from.working_directory != to.working_directory
        {
            return Err("invalid prompt transfer".to_string());
        }
        if let (Some(from), Some(to)) = (
            state.prompt_keys.get(source),
            state.pending_prompts.get(&target.session_id),
        ) {
            if *from != to.key || !to.continuation || to.fresh {
                return Err("incompatible prompt transfer domain".to_string());
            }
        }
    }
    Ok((removals, exits, finalizations))
}

pub(super) fn resolve_selection_commit_target(
    state: &SessionManagerState,
    decision: CommitDecision,
    mutations: &LifecycleMutations,
    lifecycle_sets: &LifecycleMutationSets,
    old_selection: &SessionSelection,
    old_id: Option<Uuid>,
    old_mode: SelectionMode,
) -> Result<(SelectionMode, Option<Uuid>, Option<SessionStatus>), String> {
    let (removals, exits, finalizations) = lifecycle_sets;

    let projected_status = |id: Uuid| -> Option<SessionStatus> {
        if removals.contains(&id) {
            return None;
        }
        if let Some(mutation) = mutations
            .finalizations
            .iter()
            .find(|mutation| match mutation {
                FinalizeMutation::Live(binding, _) | FinalizeMutation::Dormant(binding, _, _) => {
                    binding.session_id == id
                }
            })
        {
            return Some(match mutation {
                FinalizeMutation::Live(_, _) => SessionStatus::Running,
                FinalizeMutation::Dormant(_, _, code) => SessionStatus::Exited(*code),
            });
        }
        let current = state.sessions.get(&id)?.status.clone();
        if matches!(current, SessionStatus::Exited(_)) {
            return Some(current);
        }
        mutations
            .mark_exited
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .map(|(_, code)| SessionStatus::Exited(*code))
            .or(Some(current))
    };

    let (target_mode, target_id, target_status) = match decision {
        CommitDecision::Keep => {
            if old_id.is_some_and(|id| {
                removals.contains(&id)
                    || exits.contains(&id)
                    || mutations
                        .detached_intent
                        .iter()
                        .any(|(candidate, detached)| *candidate == id && *detached)
            }) {
                return Err("Keep cannot remove, exit, or detach the selected record".to_string());
            }
            (old_mode, old_id, old_selection.status().cloned())
        }
        CommitDecision::Clear => (SelectionMode::None, None, None),
        CommitDecision::Live(witness) => {
            let id = witness.session_id;
            if !witness.has_pty || witness.detached {
                return Err(format!("live target has invalid runtime witness: {id}"));
            }
            if state.pending_create.contains_key(&id) && !finalizations.contains(&id) {
                return Err(format!("live target is still pending: {id}"));
            }
            if removals.contains(&id) {
                return Err(format!("live target is also removed: {id}"));
            }
            match projected_status(id) {
                Some(SessionStatus::Exited(_)) => {
                    return Err(format!("live target is exited: {id}"));
                }
                Some(_) => {}
                None => return Err(format!("live target is missing: {id}")),
            }
            (SelectionMode::Live, Some(id), Some(SessionStatus::Active))
        }
        CommitDecision::Dormant(witness) => {
            let id = witness.session_id;
            if witness.detached {
                return Err(format!("dormant target is detached: {id}"));
            }
            if state.pending_create.contains_key(&id) && !finalizations.contains(&id) {
                return Err(format!("dormant target is still pending: {id}"));
            }
            if removals.contains(&id) {
                return Err(format!("dormant target is also removed: {id}"));
            }
            let status =
                projected_status(id).ok_or_else(|| format!("dormant target is missing: {id}"))?;
            if !matches!(status, SessionStatus::Exited(_)) {
                return Err(format!("dormant target is not exited: {id}"));
            }
            (SelectionMode::Dormant, Some(id), Some(status))
        }
    };

    Ok((target_mode, target_id, target_status))
}

pub(super) fn validate_unique_ids(
    label: &str,
    ids: impl IntoIterator<Item = Uuid>,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(format!("duplicate {label} lifecycle mutation: {id}"));
        }
    }
    Ok(())
}
