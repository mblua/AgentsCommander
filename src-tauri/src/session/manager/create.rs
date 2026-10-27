use super::super::session::{Session, SessionRepo, SessionStatus};
use super::binding::{binding_matches, retire_coding_agent_binding};
use super::carrier::{acquire_carrier_mutation_permit, CarrierDisposition, CarrierMutationPermit};
use super::{
    CommitDecision, CommitResult, LifecycleMutations, PendingCreateBinding, SessionManager,
    SessionManagerState,
};
use crate::errors::AppError;
use crate::pty::backend::SessionBackendKind;
use crate::session::selection::authority::{CommitCapability, CreateBindingTicket};
use crate::session::selection::payload::{SelectionCause, SessionSelection};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

impl SessionManager {
    pub(super) fn validate_owned_commit(
        state: &SessionManagerState,
        request: Option<(
            super::OwnedMutationHeader,
            super::OwnedRowIdentity,
            &super::OwnedAction,
        )>,
        m: &LifecycleMutations,
        reentry: bool,
    ) -> Result<(), String> {
        let Some((header, a, action)) = request else {
            return Ok(());
        };
        let b = header.binding;
        let finalization = m.finalizations.first();
        let finalizes_b = match finalization {
            Some(super::FinalizeMutation::Live(x, witness)) => {
                *x == b
                    && witness.session_id == b.session_id
                    && witness.has_pty
                    && !witness.detached
            }
            _ => false,
        };
        let valid = match action {
            super::OwnedAction::FinalizeB { .. } => {
                m.removals.is_empty()
                    && m.mark_exited.is_empty()
                    && m.detached_intent.is_empty()
                    && m.finalizations.len() == 1
                    && finalizes_b
                    && (m.prompt_transfers.is_empty() || m.prompt_transfers == [(a.id, b)])
            }
            super::OwnedAction::RemoveA { committed_b } => {
                committed_b.initial_b.is_some_and(|(token, _, version)| {
                    m.removals == [a.id]
                        && m.mark_exited.is_empty()
                        && m.detached_intent.is_empty()
                        && m.finalizations.is_empty()
                        && m.prompt_transfers.is_empty()
                        && (reentry
                            || (!state.pending_create.contains_key(&b.session_id)
                                && state.carrier_versions.get(&b.session_id) == Some(&version)
                                && state
                                    .sessions
                                    .get(&b.session_id)
                                    .is_some_and(|row| row.token == token)))
                })
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err("owned manager mutation set conflict".into())
        }
    }

    #[allow(dead_code, reason = "P02 owned route activates in P13")]
    pub(crate) async fn rollback_owned_pending(
        &self,
        mut permit: CarrierMutationPermit,
        binding: PendingCreateBinding,
        claim: super::OwnedMutationClaim,
    ) -> super::OwnedMutationObservation {
        #[cfg(test)]
        let probe = claim.probe();
        permit.attach(claim);
        let cell = permit.owned_cell().expect("original owned claim");
        if let Some(observation) = self
            .observe_owned_terminal(&permit, None, Some(binding))
            .await
        {
            return observation;
        }
        let result = self
            .rollback_pending_create_with_carrier_permit(&permit, binding)
            .await;
        permit.fail_unapplied(if result.is_ok() {
            super::OwnedMutationFailure::AbsentWithoutReceipt
        } else {
            super::OwnedMutationFailure::Validation
        });
        drop(permit);
        let observation = self.observe_owned_mutation(&cell);
        #[cfg(test)]
        if matches!(observation, super::OwnedMutationObservation::Applied(_)) {
            if let Some(probe) = probe {
                probe
                    .pause(crate::config::sessions_persistence::ProbeEvent::ManagerReceiptFinalized)
                    .await;
            }
        }
        observation
    }

    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_activity_coalescer(Arc::new(
            crate::config::activity_log::ActivityCoalescer::new(),
        ))
    }

    pub(crate) fn with_response_close(
        state: crate::pty::response_close::ResponseCloseState,
        owner: Arc<crate::config::activity_log::ActivityCoalescer>,
    ) -> Self {
        state.bind_publisher(crate::pty::response_close::ResponseClosePublisher::Manager);
        Self {
            response_close: state,
            ..Self::with_activity_coalescer(owner)
        }
    }

    pub fn with_activity_coalescer(
        activity: Arc<crate::config::activity_log::ActivityCoalescer>,
    ) -> Self {
        let epoch = Uuid::new_v4();
        Self {
            activity,
            response_close: crate::pty::response_close::ResponseCloseState::default(),
            prompt_initialization: Arc::new(tokio::sync::OnceCell::new()),
            state: Arc::new(RwLock::new(SessionManagerState {
                retained_prompts: HashMap::new(),
                prompt_keys: HashMap::new(),
                pending_prompts: HashMap::new(),
                prompt_seed_blocked: HashSet::new(),
                prompts_writable: false,
                prompts_reusable: false,
                coding_agent_bindings: HashMap::new(),
                binding_seeds: HashMap::new(),
                binding_origins: HashMap::new(),
                failed_binding_recipes: HashMap::new(),
                carrier_versions: HashMap::new(),
                raise_nonces: HashMap::new(),
                sessions: HashMap::new(),
                order: Vec::new(),
                pending_create: HashMap::new(),
                next_number: 1,
                epoch,
                revision: 0,
                selection: SessionSelection::initial(epoch),
            })),
        }
    }

    // Session record is created with the full set of fields up front; splitting
    // into a builder would just defer the same parameter list.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) async fn create_pending_session<T: CreateBindingTicket<PendingCreateBinding>>(
        &self,
        ticket: &mut T,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<Session, AppError> {
        let id = Uuid::new_v4();
        let binding = ticket.bind(id).map_err(AppError::Other)?;
        self.insert_new_pending_session(
            binding,
            shell,
            shell_args,
            working_directory,
            agent_id,
            agent_label,
            git_repos,
            is_coordinator,
            backend_kind,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(in crate::session) async fn create_transaction_pending_session(
        &self,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<(Session, PendingCreateBinding), AppError> {
        let binding = PendingCreateBinding::new(Uuid::new_v4(), Uuid::new_v4());
        let session = self
            .insert_new_pending_session(
                binding,
                shell,
                shell_args,
                working_directory,
                agent_id,
                agent_label,
                git_repos,
                is_coordinator,
                backend_kind,
            )
            .await?;
        Ok((session, binding))
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(super) async fn insert_new_pending_session(
        &self,
        binding: PendingCreateBinding,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<Session, AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.insert_new_pending_session_with_carrier_permit(
            &carrier_permit,
            binding,
            shell,
            shell_args,
            working_directory,
            agent_id,
            agent_label,
            git_repos,
            is_coordinator,
            backend_kind,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn insert_new_pending_session_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<Session, AppError> {
        let id = binding.session_id;
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.sessions.contains_key(&id) {
            return Err(AppError::Other(
                "pending create binding already used".to_string(),
            ));
        }
        let name = format!("Session {}", state.next_number);
        state.next_number = state
            .next_number
            .checked_add(1)
            .ok_or_else(|| AppError::Other("session number overflow".to_string()))?;

        let session = Session {
            id,
            name,
            shell,
            shell_args,
            backend_kind,
            effective_shell_args: None,
            created_at: chrono::Utc::now(),
            working_directory,
            status: SessionStatus::Running,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id,
            agent_label,
            git_repos,
            is_coordinator,
            is_root_agent: false,
            git_repos_gen: 0,
            agent_turn_armed: false,
            token: Uuid::new_v4(),
            agent_kind: None,
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            match_tier: None,
            original_profile_letter: None,
            effective_codex_home: None,
            resolved_claude_projects_dir: None,
            profile_content_hash: None,
            trusted_configured_spawn: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
            context_percent: None,
        };
        state.sessions.insert(id, session.clone());
        state.order.push(id);
        state.pending_create.insert(id, binding.nonce);

        Ok(session)
    }

    #[allow(clippy::too_many_arguments)]
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) async fn create_pending_session_with_binding_and_carrier_permit(
        &self,
        binding: PendingCreateBinding,
        carrier_permit: &CarrierMutationPermit,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
        backend_kind: SessionBackendKind,
    ) -> Result<Session, AppError> {
        self.insert_new_pending_session_with_carrier_permit(
            carrier_permit,
            binding,
            shell,
            shell_args,
            working_directory,
            agent_id,
            agent_label,
            git_repos,
            is_coordinator,
            backend_kind,
        )
        .await
    }

    pub async fn rename_session(&self, id: Uuid, name: String) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.rename_session_with_carrier_permit(&carrier_permit, id, name)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn rename_session_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        name: String,
    ) -> Result<(), AppError> {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return Err(AppError::SessionNotFound(id.to_string()));
        }
        let session = state
            .sessions
            .get_mut(&id)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
        session.name = name;
        Ok(())
    }

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn rename_pending_session(
        &self,
        binding: PendingCreateBinding,
        name: String,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.rename_pending_session_with_carrier_permit(&carrier_permit, binding, name)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn rename_pending_session_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        name: String,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.name = name
        })
        .await
    }

    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(super) async fn update_pending<F>(
        &self,
        binding: PendingCreateBinding,
        update: F,
    ) -> Result<(), AppError>
    where
        F: FnOnce(&mut Session),
    {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.update_pending_with_carrier_permit(&carrier_permit, binding, update)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn update_pending_with_carrier_permit<F>(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        update: F,
    ) -> Result<(), AppError>
    where
        F: FnOnce(&mut Session),
    {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if !binding_matches(&state, binding) {
            return Err(AppError::Other(format!(
                "pending create capability mismatch: {}",
                binding.session_id
            )));
        }
        let session = state
            .sessions
            .get_mut(&binding.session_id)
            .ok_or_else(|| AppError::SessionNotFound(binding.session_id.to_string()))?;
        update(session);
        Ok(())
    }

    pub(in crate::session) async fn rollback_pending_create(
        &self,
        binding: PendingCreateBinding,
    ) -> Result<(), String> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.rollback_pending_create_with_carrier_permit(&carrier_permit, binding)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::session) async fn rollback_pending_create_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
    ) -> Result<(), String> {
        let mut state = self.carrier_state_write(carrier_permit).await;
        state.validate_owned_identity()?;
        if let Some((header, _, action)) = state.owned_request() {
            if header.binding != binding
                || !matches!(action, super::OwnedAction::RollbackPendingB { .. })
            {
                return Err("owned rollback binding conflict".into());
            }
        }
        if !binding_matches(&state, binding) {
            if !state.sessions.contains_key(&binding.session_id) {
                return Ok(());
            }
            return Err(format!(
                "pending create capability mismatch: {}",
                binding.session_id
            ));
        }
        state.start_owned_mutation();
        state.pending_create.remove(&binding.session_id);
        state.pending_prompts.remove(&binding.session_id);
        state.sessions.remove(&binding.session_id);
        retire_coding_agent_binding(&mut state, binding.session_id);
        state.order.retain(|id| *id != binding.session_id);
        state.finish();
        Ok(())
    }

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(in crate::session) async fn insert_transaction_pending_record(
        &self,
        session: Session,
        binding: PendingCreateBinding,
    ) -> Result<(), String> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.insert_transaction_pending_record_with_carrier_permit(
            &carrier_permit,
            session,
            binding,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::session) async fn insert_transaction_pending_record_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        session: Session,
        binding: PendingCreateBinding,
    ) -> Result<(), String> {
        if session.id != binding.session_id {
            return Err("transaction pending binding targets another session".to_string());
        }
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.sessions.contains_key(&session.id) {
            return Err(format!("session already exists: {}", session.id));
        }
        state.next_number = state
            .next_number
            .checked_add(1)
            .ok_or_else(|| "session number overflow".to_string())?;
        state.order.push(session.id);
        state.pending_create.insert(session.id, binding.nonce);
        state.sessions.insert(session.id, session);
        Ok(())
    }

    pub(in crate::session) async fn commit_selection_transition(
        &self,
        _capability: &CommitCapability,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Result<CommitResult, String> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.commit_selection_transition_with_carrier_permit(
            &carrier_permit,
            _capability,
            decision,
            cause,
            mutations,
        )
        .await
    }

    /// §1295 5.6 — remove dormant orphan rows by id (production impl). The
    /// ORACLE is re-verified right here against the LIVE row: a row is removed
    /// only if it exists AND `status == Exited(_)` AND it has no pending create.
    /// This also skips a row that was `Exited` at snapshot time but got restarted
    /// in between.
    ///
    /// The section between acquiring and dropping the interior write lock is
    /// PURELY SYNCHRONOUS (no awaits), mirroring `destroy_session` (dev-rust
    /// caveat A). Selection repair mirrors `destroy_session` but with
    /// `SelectionCause::BackgroundCleanup` (NOT ManualClose): the prune is not
    /// user-initiated, so the next selection is not attributed to a close.
    ///
    /// Missing ids are not an error. Returns the number of rows removed. The
    /// method never calls any persist helper, never touches the archive, and
    /// never awaits while holding the interior write. A benign race: a
    /// concurrent `close-session` that listed an id just before the prune fails
    /// once with `AppError::SessionNotFound` and reports a now-missing row;
    /// one-shot, no retry loop (dev-rust caveat B).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) async fn remove_exited_sessions(&self, ids: &[Uuid]) -> usize {
        let permit = acquire_carrier_mutation_permit().await;
        self.remove_exited_sessions_with_carrier_permit(ids, &permit, |_, _| {
            CarrierDisposition::Foreign
        })
        .await
    }
}
