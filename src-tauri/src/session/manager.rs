use super::session::{Session, SessionCommunicationKind, SessionInfo, SessionStatus};
use crate::pty::backend::SessionBackendKind;
use crate::session::selection::{
    CommitCapability, DormantRuntimeWitness, LiveRuntimeWitness, SelectionCause, SelectionMode,
    SessionSelection,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

mod binding;
mod carrier;
mod commit;
mod communication;
mod create;
mod lookup;
mod metadata;
mod prompt;
mod turn;

use binding::{clear_failed_recipe_for_runtime_binding, retire_coding_agent_binding};
pub(crate) use carrier::{
    acquire_carrier_mutation_permit, process_carrier_publication_gate, CarrierDisposition,
    CarrierMutationPermit, CarrierPhysicalIdentity, CarrierSnapshot, RaiseCarrierOutcome,
    RetiredCarrierRouteObservation,
};
use commit::{
    append_finalized_live_activity, resolve_selection_commit_target, validate_lifecycle_mutations,
};
pub(crate) use lookup::{
    TerminalSnapshotFactsError, TerminalSnapshotRequesterFact, TerminalSnapshotSessionFact,
};
use prompt::apply_commit_prompt_domains;

/// Verified host directory and exact launch profiles; IO stays with admission.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PromptKey {
    pub canonical_cwd: String,
    pub agent_id: String,
    pub requested_profile: Option<String>,
    pub effective_profile: String,
}

impl PromptKey {
    fn matches(&self, row: &Session) -> bool {
        row.agent_id.as_ref() == Some(&self.agent_id)
            && row.requested_profile == self.requested_profile
            && row.effective_profile.as_ref() == Some(&self.effective_profile)
            && row.backend_kind == SessionBackendKind::LocalProcess
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RetainedPrompt {
    pub last_prompt: Option<String>,
    pub blocked: bool,
}

struct PendingPrompt {
    key: PromptKey,
    continuation: bool,
    fresh: bool,
    allow_retained_lookup: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PendingCreateBinding {
    session_id: Uuid,
    nonce: Uuid,
}

impl PendingCreateBinding {
    pub(super) const fn new(session_id: Uuid, nonce: Uuid) -> Self {
        Self { session_id, nonce }
    }

    pub const fn session_id(self) -> Uuid {
        self.session_id
    }
}

#[derive(Clone)]
pub struct SessionManager {
    response_close: crate::pty::manager::ResponseCloseState,
    state: Arc<RwLock<SessionManagerState>>,
    pub(crate) prompt_initialization: Arc<tokio::sync::OnceCell<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniqueLiveTokenError {
    NotFound,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub(crate) struct CodingAgentBinding {
    pub id: String,
    pub seed: Option<serde_json::Value>,
    pub origin: Option<serde_json::Value>,
}

struct SessionManagerState {
    retained_prompts: HashMap<PromptKey, RetainedPrompt>,
    prompt_keys: HashMap<Uuid, PromptKey>,
    pending_prompts: HashMap<Uuid, PendingPrompt>,
    prompt_seed_blocked: HashSet<Uuid>,
    prompts_writable: bool,
    prompts_reusable: bool,
    coding_agent_bindings: HashMap<Uuid, String>,
    binding_seeds: HashMap<String, serde_json::Value>,
    binding_origins: HashMap<String, serde_json::Value>,
    failed_binding_recipes: HashMap<String, serde_json::Value>,
    carrier_versions: HashMap<Uuid, u64>,
    raise_nonces: HashMap<Uuid, Uuid>,
    sessions: HashMap<Uuid, Session>,
    order: Vec<Uuid>,
    pending_create: HashMap<Uuid, Uuid>,
    next_number: u32,
    epoch: Uuid,
    revision: u64,
    selection: SessionSelection,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagerAggregateSnapshot {
    pub coding_agent_bindings: HashMap<Uuid, String>,
    pub binding_seeds: HashMap<String, serde_json::Value>,
    pub binding_origins: HashMap<String, serde_json::Value>,
    pub failed_binding_recipes: HashMap<String, serde_json::Value>,
    pub sessions: Vec<Session>,
    pub order: Vec<Uuid>,
    pub pending_ids: HashSet<Uuid>,
    pub selection: SessionSelection,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CommitDecision {
    Keep,
    Clear,
    Live(LiveRuntimeWitness),
    Dormant(DormantRuntimeWitness),
}

#[derive(Debug, Clone, Copy)]
enum FinalizeMutation {
    Live(PendingCreateBinding, LiveRuntimeWitness),
    Dormant(PendingCreateBinding, DormantRuntimeWitness, i32),
}

#[derive(Debug, Default)]
pub(crate) struct LifecycleMutations {
    removals: Vec<Uuid>,
    mark_exited: Vec<(Uuid, i32)>,
    detached_intent: Vec<(Uuid, bool)>,
    finalizations: Vec<FinalizeMutation>,
    prompt_transfers: Vec<(Uuid, PendingCreateBinding)>,
}

impl LifecycleMutations {
    pub(crate) fn transfer_prompt(&mut self, source: Uuid, target: PendingCreateBinding) {
        self.prompt_transfers.push((source, target));
    }

    pub(crate) fn remove(&mut self, session_id: Uuid) {
        self.removals.push(session_id);
    }

    pub(crate) fn mark_exited(&mut self, session_id: Uuid, exit_code: i32) {
        self.mark_exited.push((session_id, exit_code));
    }

    pub(crate) fn set_detached_intent(&mut self, session_id: Uuid, value: bool) {
        self.detached_intent.push((session_id, value));
    }

    pub(crate) fn finalize_live(
        &mut self,
        binding: PendingCreateBinding,
        witness: LiveRuntimeWitness,
    ) {
        self.finalizations
            .push(FinalizeMutation::Live(binding, witness));
    }

    pub(crate) fn finalize_dormant(
        &mut self,
        binding: PendingCreateBinding,
        witness: DormantRuntimeWitness,
        exit_code: i32,
    ) {
        self.finalizations
            .push(FinalizeMutation::Dormant(binding, witness, exit_code));
    }
}

#[derive(Debug, Default)]
pub(crate) struct CommitResult {
    pub removed_rows: Vec<SessionInfo>,
    pub missing_ids: Vec<Uuid>,
    pub changed_rows: Vec<SessionInfo>,
    pub finalized_rows: Vec<SessionInfo>,
    pub cleared_raise_hand_ids: Vec<Uuid>,
    pub selection: Option<SessionSelection>,
    /// #1149 - activity records built under the write guard and appended by
    /// `SelectionCoordinator::commit` once the guard is gone. Carried out rather
    /// than appended in place because the guard is a local of
    /// `commit_selection_transition` and no append may hold it.
    pub activity: Vec<crate::config::activity_log::ActivityRecord>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn mark_idle_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) {
        let record = {
            let mut state = self.carrier_state_write(carrier_permit).await;
            if state.pending_create.contains_key(&id) {
                return;
            }
            let Some(s) = state.sessions.get_mut(&id) else {
                return;
            };
            let was_working = crate::session::session::is_working(s);
            // MUST stay above the assignment: the literal `false` is correct only
            // because this line reads the pre-mutation state under `was_working`.
            // Below the assignment it would print `true → true` forever.
            if was_working {
                log::info!(
                    "[session-state] {} '{}': waiting_for_input false → true",
                    &id.to_string()[..8],
                    s.name
                );
            }
            s.waiting_for_input = true;
            if matches!(s.status, SessionStatus::Running) {
                s.status = SessionStatus::Idle;
            }
            if was_working && !crate::session::session::is_working(s) {
                // Built under the same guard that stamps `at`, so the block
                // annotation can never disagree with its own timestamp.
                Some(crate::config::activity_log::build_idle(
                    id,
                    s,
                    crate::config::activity_log::IdleReason::MarkIdle,
                ))
            } else {
                None
            }
        }; // write guard released here
        if let Some(record) = record {
            crate::config::activity_log::append(record);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn mark_busy_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) {
        let record = {
            let mut state = self.carrier_state_write(carrier_permit).await;
            if state.pending_create.contains_key(&id) {
                return;
            }
            let Some(s) = state.sessions.get_mut(&id) else {
                return;
            };
            let was_working = crate::session::session::is_working(s);
            // The post-mutation predicate, computable before the mutation: this
            // clears `waiting_for_input` and promotes `Idle` to `Running`, so the
            // session ends up working iff it is not `Exited`. The `Exited` term is
            // required, not decorative: `mark_exited` leaves `waiting_for_input`
            // untouched, so a session that died while working sits at
            // `(Exited, false)`, where the literal `true` below would be a lie and
            // the line would repeat a target value A9 forbids repeating.
            let becomes_working = !matches!(s.status, SessionStatus::Exited(_));
            // MUST stay above the assignment; see `mark_idle`.
            if !was_working && becomes_working {
                log::info!(
                    "[session-state] {} '{}': waiting_for_input true → false",
                    &id.to_string()[..8],
                    s.name
                );
            }
            s.waiting_for_input = false;
            if matches!(s.status, SessionStatus::Idle) {
                s.status = SessionStatus::Running;
            }
            // The post-check is load-bearing here, unlike in `mark_idle` where
            // `was_working` implies it: on an `Exited` session this clears
            // `waiting_for_input` and leaves the status alone, so `is_working`
            // stays false and no record may be emitted.
            if !was_working && crate::session::session::is_working(s) {
                Some(crate::config::activity_log::build_busy(
                    id,
                    s,
                    crate::config::activity_log::BusyReason::MarkBusy,
                ))
            } else {
                None
            }
        }; // write guard released here
        if let Some(record) = record {
            crate::config::activity_log::append(record);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_pty_input_boundary_with_carrier_permit<'a, F, G>(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        expected_target: &crate::config::teams::VerifiedPtyInputIdentity,
        expected_backend: SessionBackendKind,
        authority_id: Uuid,
        expected_sender: &crate::config::teams::VerifiedPtyInputIdentity,
        authority_backend: SessionBackendKind,
        authority_route: &'a crate::pty::manager::PtyAuthorityRouteProof,
        permit: &'a crate::pty::manager::PtyInputPermit,
        idle_detector: &crate::pty::idle_detector::IdleDetector,
        settings: &crate::config::settings::SettingsState,
        final_recipe_check: G,
        final_external_check: F,
    ) -> Result<
        crate::pty::manager::PtyRouteWriteGuard<'a>,
        crate::pty::idle_detector::PtyInputBoundaryFailure,
    >
    where
        F: FnOnce() -> bool,
        G: FnOnce(&Session, &crate::config::settings::AppSettings) -> bool,
    {
        use crate::pty::idle_detector::PtyInputBoundaryFailure as Failure;

        let (route_guard, was_idle, activity) = {
            let settings_guard = Arc::clone(settings)
                .try_current_read_owned()
                .map_err(|_| Failure::RouteUnavailable)?;
            let mut state = self.carrier_state_write(carrier_permit).await;
            if state.pending_create.contains_key(&id)
                || state.pending_create.contains_key(&authority_id)
            {
                return Err(Failure::RouteUnavailable);
            }
            let authority = state
                .sessions
                .get(&authority_id)
                .ok_or(Failure::RouteUnavailable)?;
            if matches!(authority.status, SessionStatus::Exited(_))
                || authority.backend_kind != authority_backend
            {
                return Err(Failure::RouteUnavailable);
            }
            if authority_backend == SessionBackendKind::LocalProcess {
                let live_token_matches = state
                    .sessions
                    .values()
                    .filter(|candidate| !state.pending_create.contains_key(&candidate.id))
                    .filter(|candidate| candidate.token == authority.token)
                    .filter(|candidate| !matches!(candidate.status, SessionStatus::Exited(_)))
                    .count();
                if live_token_matches != 1 {
                    return Err(Failure::RouteUnavailable);
                }
            }
            let authority_cwd = crate::path_identity::verify_directory(std::path::Path::new(
                &authority.working_directory,
            ))
            .map_err(|_| Failure::RouteUnavailable)?;
            let expected_authority_replica = if expected_sender.canonical_fqn
                == crate::config::root_agent::ROOT_AGENT_SENDER
            {
                let root = crate::config::root_agent::verify_live_root_agent_path(
                    std::path::Path::new(&authority.working_directory),
                )
                .map_err(|_| Failure::RouteUnavailable)?;
                if !authority.is_root_agent
                    || !crate::path_identity::same_object(&root, &expected_sender.replica_identity)
                {
                    return Err(Failure::RouteUnavailable);
                }
                None
            } else {
                let sender = crate::config::teams::verify_pty_input_replica_cwd(
                    std::path::Path::new(&authority.working_directory),
                )
                .map_err(|_| Failure::RouteUnavailable)?;
                if authority.is_root_agent
                    || sender.canonical_fqn != expected_sender.canonical_fqn
                    || sender.authority_fingerprint != expected_sender.authority_fingerprint
                    || !crate::path_identity::same_object(
                        &sender.replica_identity,
                        &expected_sender.replica_identity,
                    )
                {
                    return Err(Failure::RouteUnavailable);
                }
                Some(sender.replica_identity)
            };
            let authority_route_guard = authority_route
                .lock_verified(
                    authority_backend,
                    &authority_cwd,
                    expected_authority_replica.as_ref(),
                )
                .map_err(|_| Failure::RouteUnavailable)?;
            let session = state
                .sessions
                .get_mut(&id)
                .ok_or(Failure::RouteUnavailable)?;
            if matches!(session.status, SessionStatus::Exited(_))
                || !session.waiting_for_input
                || session.backend_kind != expected_backend
                || !final_recipe_check(session, &settings_guard)
            {
                return Err(Failure::Busy);
            }
            let cwd_identity = crate::path_identity::verify_directory(std::path::Path::new(
                &session.working_directory,
            ))
            .map_err(|_| Failure::RouteUnavailable)?;
            let current_target = crate::config::teams::verify_pty_input_replica_cwd(
                std::path::Path::new(&session.working_directory),
            )
            .map_err(|_| Failure::RouteUnavailable)?;
            if current_target.canonical_fqn != expected_target.canonical_fqn
                || current_target.authority_fingerprint != expected_target.authority_fingerprint
                || !crate::path_identity::same_object(
                    &current_target.replica_identity,
                    &expected_target.replica_identity,
                )
            {
                return Err(Failure::RouteUnavailable);
            }
            let prepared = idle_detector.prepare_pty_input_boundary(id, || {
                let route = crate::pty::manager::PtyManager::lock_route_for_verified_write(
                    permit,
                    expected_backend,
                    &cwd_identity,
                    &expected_target.replica_identity,
                )?;
                if !final_external_check() {
                    return Err(crate::errors::AppError::PtyError(
                        "pty_authority_changed".to_string(),
                    ));
                }
                Ok(route)
            })?;
            session.waiting_for_input = false;
            if matches!(session.status, SessionStatus::Idle) {
                session.status = SessionStatus::Running;
            }
            // #1682 - the boundary transaction arms this session, so its busy->idle
            // edges stamp `tooling.lastAgentMessageAt`. It keys on that arming, not on
            // proven delivery: R7 is the branch that arms without delivering. This site
            // is required because the plane bypasses `mark_busy` (see the #1149 note
            // below). It arms BEFORE the caller's write at `phone/mailbox.rs:6029`,
            // because this block, not that write, is this function's point of no return.
            session.agent_turn_armed = true;
            // #1149 - the third working-state mutation site, and the one that
            // bypasses `mark_busy`: `waiting_for_input` is already false by the
            // time `notify_pty_input_busy` reaches it, so hooking only
            // `mark_idle`/`mark_busy` would silently lose every inter-agent
            // injection edge.
            //
            // Unconditional here, unlike the other two sites: the checks above
            // rejected this call unless the session was non-`Exited` and waiting
            // for input, and `SessionStatus` has exactly four variants, so the
            // session is necessarily `Active` or `Running` with
            // `waiting_for_input == false` now. Nothing below can discard the
            // record either: no `?` and no early return remains in this block.
            let activity = crate::config::activity_log::build_busy(
                id,
                session,
                crate::config::activity_log::BusyReason::PtyInputBoundary,
            );
            let (mut route_guard, was_idle) = prepared;
            route_guard.retain_authority_guard(authority_route_guard);
            route_guard.retain_settings_guard(settings_guard);
            (route_guard, was_idle, activity)
        }; // write guard released here
        crate::config::activity_log::append(activity);
        // The `mark_busy` this reaches through the idle callback then sees no
        // edge and emits nothing, so there is exactly one record per injection.
        idle_detector.notify_pty_input_busy(id, was_idle);
        // #1682 - the privileged exact-agent-input plane ARMS the turn here, and the
        // clear keys on that arming, not on proven delivery: R7 is the branch that
        // arms without delivering. It never reaches `mark_successful_pty_write_busy`,
        // so it sets no mark of its own; this cancels one a preceding control write
        // left, so a focus report before a `send --pty-input` cannot suppress its edge.
        idle_detector.clear_control_write(id);
        Ok(route_guard)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn commit_selection_transition_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        _capability: &CommitCapability,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Result<CommitResult, String> {
        let mut state = self.carrier_state_write(carrier_permit).await;
        let old_selection = state.selection.clone();
        let old_id = old_selection.id();
        let old_mode = old_selection.mode();

        let lifecycle_sets = validate_lifecycle_mutations(&state, &mutations)?;

        let (target_mode, target_id, target_status) = resolve_selection_commit_target(
            &state,
            decision,
            &mutations,
            &lifecycle_sets,
            &old_selection,
            old_id,
            old_mode,
        )?;

        let selection_changed = target_mode != old_mode
            || target_id != old_id
            || (target_mode == SelectionMode::Dormant
                && target_status.as_ref() != old_selection.status());
        let next_revision = if selection_changed {
            state.revision.checked_add(1).ok_or_else(|| {
                log::error!(
                    "[selection] revision overflow epoch={} oldId={:?} source={}",
                    state.epoch,
                    old_id,
                    cause.source()
                );
                "selection revision overflow".to_string()
            })?
        } else {
            state.revision
        };

        // Every fallible check runs before either row receives transferred text.
        if !matches!(
            (target_mode, target_id, target_status.as_ref()),
            (SelectionMode::None, None, _)
                | (SelectionMode::Live, Some(_), _)
                | (
                    SelectionMode::Dormant,
                    Some(_),
                    Some(SessionStatus::Exited(_))
                )
        ) {
            return Err("invalid canonical selection target".to_string());
        }
        apply_commit_prompt_domains(&mut state, &mutations, cause);
        let mut result = CommitResult::default();
        let mut changed_ids = HashSet::new();
        // #1149 - ids only. The record itself is built after every fallible exit
        // below, so a later `?` can never strand a record for a session this
        // commit did not actually finalize.
        let mut finalized_live_ids: Vec<Uuid> = Vec::new();
        for mutation in mutations.finalizations {
            match mutation {
                FinalizeMutation::Live(binding, _) => {
                    state.pending_create.remove(&binding.session_id);
                    clear_failed_recipe_for_runtime_binding(&mut state, binding.session_id);
                    if let Some(record) = state.sessions.get_mut(&binding.session_id) {
                        record.status = SessionStatus::Running;
                        finalized_live_ids.push(binding.session_id);
                        result.finalized_rows.push(SessionInfo::from(&*record));
                    }
                }
                FinalizeMutation::Dormant(binding, _, exit_code) => {
                    state.pending_create.remove(&binding.session_id);
                    clear_failed_recipe_for_runtime_binding(&mut state, binding.session_id);
                    if let Some(record) = state.sessions.get_mut(&binding.session_id) {
                        record.status = SessionStatus::Exited(exit_code);
                        result.finalized_rows.push(SessionInfo::from(&*record));
                    }
                }
            }
        }

        for (id, exit_code) in mutations.mark_exited {
            let Some(record) = state.sessions.get_mut(&id) else {
                result.missing_ids.push(id);
                continue;
            };
            if matches!(record.status, SessionStatus::Exited(_)) {
                continue;
            }
            let cleared_raise_hand = record.communication.as_ref().is_some_and(|communication| {
                communication.kind == SessionCommunicationKind::RaiseHand && communication.visible
            });
            if cleared_raise_hand {
                record.communication = None;
                result.cleared_raise_hand_ids.push(id);
            }
            record.status = SessionStatus::Exited(exit_code);
            changed_ids.insert(id);
        }

        for (id, detached) in mutations.detached_intent {
            let Some(record) = state.sessions.get_mut(&id) else {
                result.missing_ids.push(id);
                continue;
            };
            if record.was_detached != detached {
                record.was_detached = detached;
                changed_ids.insert(id);
            }
        }

        for id in mutations.removals {
            if let Some(record) = state.sessions.remove(&id) {
                state.order.retain(|candidate| *candidate != id);
                state.prompt_keys.remove(&id);
                state.prompt_seed_blocked.remove(&id);
                state.pending_prompts.remove(&id);
                state.pending_create.remove(&id);
                retire_coding_agent_binding(&mut state, id);
                result.removed_rows.push(SessionInfo::from(&record));
            } else {
                result.missing_ids.push(id);
            }
        }

        if selection_changed {
            if let Some(previous_id) = old_id {
                let previous_remains_live_target =
                    target_mode == SelectionMode::Live && target_id == Some(previous_id);
                if !previous_remains_live_target {
                    if let Some(previous) = state.sessions.get_mut(&previous_id) {
                        if previous.status == SessionStatus::Active {
                            previous.status = SessionStatus::Running;
                            changed_ids.insert(previous_id);
                        }
                    }
                }
            }

            state.revision = next_revision;
            state.selection = match (target_mode, target_id, target_status) {
                (SelectionMode::None, None, _) => {
                    SessionSelection::none(state.epoch, next_revision, cause)
                }
                (SelectionMode::Live, Some(id), _) => {
                    let target = state
                        .sessions
                        .get_mut(&id)
                        .expect("selection target validated before mutations");
                    target.status = SessionStatus::Active;
                    changed_ids.insert(id);
                    SessionSelection::live(state.epoch, next_revision, cause, id)
                }
                (SelectionMode::Dormant, Some(id), Some(SessionStatus::Exited(code))) => {
                    let has_pty = match decision {
                        CommitDecision::Dormant(witness) => witness.has_pty,
                        _ => false,
                    };
                    SessionSelection::dormant(state.epoch, next_revision, cause, id, code, has_pty)
                }
                _ => unreachable!("selection target prevalidated"),
            };
            result.selection = Some(state.selection.clone());
        }

        result.changed_rows = state
            .order
            .iter()
            .filter(|id| changed_ids.contains(id) && !state.pending_create.contains_key(id))
            .filter_map(|id| state.sessions.get(id).map(SessionInfo::from))
            .collect();

        log::info!(
            concat!(
                "[selection] epoch={} revision={} oldId={:?} newId={:?} source={} ",
                "userInitiated={} oldMode={} newMode={} oldStatus={:?} newStatus={:?} ",
                "targetHasPty={} targetDetached={} result={}"
            ),
            state.epoch,
            state.revision,
            old_id,
            state.selection.id(),
            cause.source(),
            cause.user_initiated(),
            old_mode,
            state.selection.mode(),
            old_selection.status(),
            state.selection.status(),
            state.selection.has_pty(),
            state.selection.detached(),
            if selection_changed {
                "committed"
            } else {
                "noop"
            },
        );

        // #1149 - the opening edge at session birth, deferred past every fallible
        // exit above so no record can survive a commit that returned `Err`. The
        // write guard is still held, which is what keeps the coalescer exclusive.
        //
        // The re-read is also semantically better than emitting inside the
        // finalization arm: a session finalized and then removed or marked exited
        // reads back as absent or `Exited`, so `is_working` is false and nothing
        // is recorded. Such a session was never observably working. One promoted
        // to `Active` still reads as working and is recorded.
        append_finalized_live_activity(&state, finalized_live_ids, &mut result);

        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn remove_exited_sessions_with_carrier_permit(
        &self,
        ids: &[Uuid],
        permit: &CarrierMutationPermit,
        mut classify: impl FnMut(&Session, u64) -> CarrierDisposition,
    ) -> usize {
        let mut state = self.carrier_state_write(permit).await;
        let dispositions: HashMap<Uuid, CarrierDisposition> = state
            .sessions
            .iter()
            .map(|(id, row)| {
                (
                    *id,
                    classify(row, state.carrier_versions.get(id).copied().unwrap_or(0)),
                )
            })
            .collect();
        let clear_surviving = state.selection.id().is_some_and(|id| {
            ids.contains(&id) && dispositions.get(&id) == Some(&CarrierDisposition::Protected)
        });
        let will_remove_selected = state.selection.id().is_some_and(|id| {
            ids.contains(&id)
                && dispositions.get(&id) == Some(&CarrierDisposition::Foreign)
                && !state.pending_create.contains_key(&id)
                && state
                    .sessions
                    .get(&id)
                    .is_some_and(|row| matches!(row.status, SessionStatus::Exited(_)))
        });
        let next_revision = if clear_surviving || will_remove_selected {
            let Some(next) = state.revision.checked_add(1) else {
                self.response_close.fail_closed();
                return 0;
            };
            next
        } else {
            state.revision
        };
        let mut removed = 0usize;
        let mut removed_selection = clear_surviving;
        let mut removed_ids: Vec<Uuid> = Vec::new();
        for &id in ids {
            if dispositions.get(&id) != Some(&CarrierDisposition::Foreign) {
                continue;
            }
            let Some(session) = state.sessions.get(&id) else {
                continue;
            };
            if !matches!(session.status, SessionStatus::Exited(_)) {
                continue;
            }
            if state.pending_create.contains_key(&id) {
                continue;
            }
            state.sessions.remove(&id);
            retire_coding_agent_binding(&mut state, id);
            removed += 1;
            removed_ids.push(id);
            if state.selection.id() == Some(id) {
                removed_selection = true;
            }
        }
        if !removed_ids.is_empty() {
            state
                .order
                .retain(|candidate| !removed_ids.contains(candidate));
        }
        if removed_selection {
            if clear_surviving {
                if let Some(id) = state.selection.id() {
                    if let Some(row) = state.sessions.get_mut(&id) {
                        if row.status == SessionStatus::Active {
                            row.status = SessionStatus::Running;
                        }
                    }
                }
            }
            state.revision = next_revision;
            let next = state.order.iter().copied().find(|candidate| {
                !state.pending_create.contains_key(candidate)
                    && dispositions.get(candidate) == Some(&CarrierDisposition::Foreign)
            });
            if let Some(next_id) = next {
                if let Some(session) = state.sessions.get_mut(&next_id) {
                    session.status = SessionStatus::Active;
                }
                state.selection = SessionSelection::live(
                    state.epoch,
                    state.revision,
                    SelectionCause::BackgroundCleanup,
                    next_id,
                );
            } else {
                state.selection = SessionSelection::none(
                    state.epoch,
                    state.revision,
                    SelectionCause::BackgroundCleanup,
                );
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod response_close_carrier_tests;
