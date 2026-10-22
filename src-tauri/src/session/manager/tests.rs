use super::super::profile::CodingAgentKind;
use super::super::session::{SessionCommunication, SessionRepo};
use super::*;
use crate::config::settings::WindowGeometry;
use crate::errors::AppError;
use crate::session::selection::TrustedCreateIntent;
use std::path::PathBuf;

async fn pending_fixture(
    manager: &SessionManager,
    coordinator: bool,
) -> (Session, PendingCreateBinding) {
    manager
        .create_transaction_pending_session(
            "shell".to_string(),
            Vec::new(),
            "C:/work".to_string(),
            None,
            None,
            Vec::new(),
            coordinator,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("pending session")
}

fn live_witness(session_id: Uuid) -> LiveRuntimeWitness {
    LiveRuntimeWitness {
        session_id,
        has_pty: true,
        detached: false,
    }
}

fn dormant_witness(session_id: Uuid) -> DormantRuntimeWitness {
    DormantRuntimeWitness {
        session_id,
        has_pty: false,
        detached: false,
    }
}

mod active_selection;
mod activity;
mod binding;
mod create_transaction;
mod finalization;
mod metadata;
mod raise_hand;
mod team;

#[cfg(test)]
impl SessionManager {
    pub(crate) async fn enroll_failed_binding_recipe_for_test(
        &self,
        binding: String,
        recipe: serde_json::Value,
    ) {
        self.state
            .write()
            .await
            .failed_binding_recipes
            .insert(binding, recipe);
    }

    pub async fn set_communication_for_test(&self, id: Uuid, communication: SessionCommunication) {
        if let Some(session) = self.state.write().await.sessions.get_mut(&id) {
            session.communication = Some(communication);
        }
    }

    /// #1063: insert a pending create (hidden from public reads, visible to the
    /// deletion-only snapshot) at `working_directory`, for cross-module deletion
    /// race tests.
    pub(crate) async fn insert_pending_session_for_test(&self, working_directory: String) -> Uuid {
        let (session, _binding) = self
            .create_transaction_pending_session(
                "sh".to_string(),
                Vec::new(),
                working_directory,
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("insert pending session for test");
        session.id
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_session(
        &self,
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
        let permit = acquire_carrier_mutation_permit().await;
        let mut state = self.carrier_state_write(&permit).await;
        let name = format!("Session {}", state.next_number);
        state.next_number = state.next_number.saturating_add(1);
        let mut session = Session {
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
        if state.selection.mode() == SelectionMode::None {
            session.status = SessionStatus::Active;
            state.revision += 1;
            state.selection =
                SessionSelection::live(state.epoch, state.revision, SelectionCause::UserSwitch, id);
        }
        state.order.push(id);
        state.sessions.insert(id, session.clone());
        Ok(session)
    }

    pub async fn get_active(&self) -> Option<Uuid> {
        self.state.read().await.selection.id()
    }

    pub async fn switch_session(&self, id: Uuid) -> Result<(), AppError> {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) || !state.sessions.contains_key(&id) {
            return Err(AppError::SessionNotFound(id.to_string()));
        }
        if let Some(old_id) = state.selection.id() {
            if old_id != id {
                if let Some(old) = state.sessions.get_mut(&old_id) {
                    if old.status == SessionStatus::Active {
                        old.status = SessionStatus::Running;
                    }
                }
            }
        }
        if let Some(target) = state.sessions.get_mut(&id) {
            target.status = SessionStatus::Active;
        }
        state.revision += 1;
        state.selection =
            SessionSelection::live(state.epoch, state.revision, SelectionCause::UserSwitch, id);
        Ok(())
    }

    pub async fn set_active_only(&self, id: Uuid) -> Result<(), AppError> {
        let mut state = self.state.write().await;
        let status = state
            .sessions
            .get(&id)
            .filter(|_| !state.pending_create.contains_key(&id))
            .map(|session| session.status.clone())
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
        if let Some(old_id) = state.selection.id() {
            if let Some(old) = state.sessions.get_mut(&old_id) {
                if old.status == SessionStatus::Active {
                    old.status = SessionStatus::Running;
                }
            }
        }
        state.revision += 1;
        state.selection = match status {
            SessionStatus::Exited(code) => SessionSelection::dormant(
                state.epoch,
                state.revision,
                SelectionCause::Restore,
                id,
                code,
                false,
            ),
            _ => {
                SessionSelection::live(state.epoch, state.revision, SelectionCause::UserSwitch, id)
            }
        };
        Ok(())
    }

    pub async fn clear_active(&self) {
        let mut state = self.state.write().await;
        if let Some(old_id) = state.selection.id() {
            if let Some(old) = state.sessions.get_mut(&old_id) {
                if old.status == SessionStatus::Active {
                    old.status = SessionStatus::Running;
                }
            }
            state.revision += 1;
            state.selection = SessionSelection::none(
                state.epoch,
                state.revision,
                SelectionCause::BackgroundCleanup,
            );
        }
    }

    pub async fn clear_active_if(&self, id: Uuid) {
        if self.get_active().await == Some(id) {
            self.clear_active().await;
        }
    }

    pub async fn mark_exited(&self, id: Uuid, code: i32) -> bool {
        let mut state = self.state.write().await;
        let Some(session) = state.sessions.get_mut(&id) else {
            return false;
        };
        let cleared = session.communication.as_ref().is_some_and(|communication| {
            communication.kind == SessionCommunicationKind::RaiseHand && communication.visible
        });
        if cleared {
            session.communication = None;
        }
        session.status = SessionStatus::Exited(code);
        cleared
    }

    pub async fn destroy_session(&self, id: Uuid) -> Result<Option<Uuid>, AppError> {
        let mut state = self.state.write().await;
        if state.sessions.remove(&id).is_none() {
            return Err(AppError::SessionNotFound(id.to_string()));
        }
        retire_coding_agent_binding(&mut state, id);
        state.order.retain(|candidate| *candidate != id);
        if state.selection.id() != Some(id) {
            return Ok(None);
        }
        let next = state
            .order
            .iter()
            .copied()
            .find(|candidate| !state.pending_create.contains_key(candidate));
        state.revision += 1;
        if let Some(next_id) = next {
            if let Some(session) = state.sessions.get_mut(&next_id) {
                session.status = SessionStatus::Active;
            }
            state.selection = SessionSelection::live(
                state.epoch,
                state.revision,
                SelectionCause::ManualClose,
                next_id,
            );
        } else {
            state.selection =
                SessionSelection::none(state.epoch, state.revision, SelectionCause::ManualClose);
        }
        Ok(next)
    }
}
