use super::super::profile::CodingAgentKind;
use super::super::session::SessionRepo;
use super::carrier::{acquire_carrier_mutation_permit, CarrierMutationPermit};
use super::{PendingCreateBinding, SessionManager};
use crate::config::settings::WindowGeometry;
use crate::errors::AppError;
use crate::session::selection::authority::{FinalizedOwnedCarry, FinalizedOwnedCarryOutcome};
use std::collections::HashSet;
use std::path::PathBuf;
use uuid::Uuid;

impl SessionManager {
    /// Set the resolved coding-agent identity. Called once by
    /// `create_session_inner` immediately after `CodingAgentKind::detect`.
    pub async fn set_agent_kind(&self, id: Uuid, kind: Option<CodingAgentKind>) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_agent_kind_with_carrier_permit(&carrier_permit, id, kind)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_agent_kind_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        kind: Option<CodingAgentKind>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.agent_kind = kind;
        }
    }

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn set_pending_agent_kind(
        &self,
        binding: PendingCreateBinding,
        kind: Option<CodingAgentKind>,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_agent_kind_with_carrier_permit(&carrier_permit, binding, kind)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_agent_kind_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        kind: Option<CodingAgentKind>,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.agent_kind = kind
        })
        .await
    }

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn set_pending_trusted_configured_spawn(
        &self,
        binding: PendingCreateBinding,
        trusted: bool,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_trusted_configured_spawn_with_carrier_permit(
            &carrier_permit,
            binding,
            trusted,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_trusted_configured_spawn_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        trusted: bool,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.trusted_configured_spawn = trusted;
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_profile_metadata(
        &self,
        id: Uuid,
        requested_profile: Option<String>,
        effective_profile: Option<String>,
        profile_fallback_chain: Vec<String>,
        profile_fallback_applied: bool,
        match_tier: Option<String>,
        original_profile_letter: Option<String>,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_profile_metadata_with_carrier_permit(
            &carrier_permit,
            id,
            requested_profile,
            effective_profile,
            profile_fallback_chain,
            profile_fallback_applied,
            match_tier,
            original_profile_letter,
            effective_codex_home,
            profile_content_hash,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_profile_metadata_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        requested_profile: Option<String>,
        effective_profile: Option<String>,
        profile_fallback_chain: Vec<String>,
        profile_fallback_applied: bool,
        match_tier: Option<String>,
        original_profile_letter: Option<String>,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.requested_profile = requested_profile;
            s.effective_profile = effective_profile;
            s.profile_fallback_chain = profile_fallback_chain;
            s.profile_fallback_applied = profile_fallback_applied;
            s.match_tier = match_tier;
            s.original_profile_letter = original_profile_letter;
            s.effective_codex_home = effective_codex_home;
            s.profile_content_hash = profile_content_hash;
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) async fn set_pending_profile_metadata(
        &self,
        binding: PendingCreateBinding,
        requested_profile: Option<String>,
        effective_profile: Option<String>,
        profile_fallback_chain: Vec<String>,
        profile_fallback_applied: bool,
        match_tier: Option<String>,
        original_profile_letter: Option<String>,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_profile_metadata_with_carrier_permit(
            &carrier_permit,
            binding,
            requested_profile,
            effective_profile,
            profile_fallback_chain,
            profile_fallback_applied,
            match_tier,
            original_profile_letter,
            effective_codex_home,
            profile_content_hash,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_profile_metadata_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        requested_profile: Option<String>,
        effective_profile: Option<String>,
        profile_fallback_chain: Vec<String>,
        profile_fallback_applied: bool,
        match_tier: Option<String>,
        original_profile_letter: Option<String>,
        effective_codex_home: Option<String>,
        profile_content_hash: Option<String>,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.requested_profile = requested_profile;
            session.effective_profile = effective_profile;
            session.profile_fallback_chain = profile_fallback_chain;
            session.profile_fallback_applied = profile_fallback_applied;
            session.match_tier = match_tier;
            session.original_profile_letter = original_profile_letter;
            session.effective_codex_home = effective_codex_home;
            session.profile_content_hash = profile_content_hash;
        })
        .await
    }

    pub async fn set_resolved_claude_projects_dir(&self, id: Uuid, path: Option<PathBuf>) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_resolved_claude_projects_dir_with_carrier_permit(&carrier_permit, id, path)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_resolved_claude_projects_dir_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        path: Option<PathBuf>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.resolved_claude_projects_dir = path;
        }
    }

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn set_pending_resolved_claude_projects_dir(
        &self,
        binding: PendingCreateBinding,
        path: Option<PathBuf>,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_resolved_claude_projects_dir_with_carrier_permit(
            &carrier_permit,
            binding,
            path,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_resolved_claude_projects_dir_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        path: Option<PathBuf>,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.resolved_claude_projects_dir = path;
        })
        .await
    }

    /// The upper continuation owns the A/Preparing/launch check. This one
    /// interior mutation accepts only its exact finalized B and initial version.
    pub(in crate::session) async fn apply_finalized_owned_replacement_carry(
        &self,
        permit: &CarrierMutationPermit,
        capability: impl FinalizedOwnedCarry,
    ) -> FinalizedOwnedCarryOutcome {
        if !capability.bound_operation() {
            return FinalizedOwnedCarryOutcome::Recapture;
        }
        let id = capability.session_id();
        let token = capability.token();
        let version = capability.expected_version();
        if version == u64::MAX {
            return FinalizedOwnedCarryOutcome::Recapture;
        }
        let telegram = capability.telegram_bot_id().clone();
        let communication = capability.communication().clone();
        let fresh = capability.start_fresh_on_restore();
        let mut state = self.carrier_state_write(permit).await;
        if state.pending_create.contains_key(&id)
            || state.carrier_versions.get(&id) != Some(&version)
            || !state
                .sessions
                .get(&id)
                .is_some_and(|row| row.token == token)
        {
            drop(state);
            return FinalizedOwnedCarryOutcome::Recapture;
        }
        if let Some(row) = state.sessions.get_mut(&id) {
            row.telegram_bot_id = telegram;
            row.communication = communication;
            row.start_fresh_on_restore = fresh;
        }
        // Drop publishes the actual version and invalidation while U remains
        // borrowed by upper. The capability/owner is dropped outside the write.
        drop(state);
        drop(capability);
        FinalizedOwnedCarryOutcome::Applied
    }

    /// Persisted Telegram ON/OFF state for the session. Some(bot_id) means the
    /// bridge should be reattached when the session is restored or woken.
    pub async fn set_telegram_bot_id(&self, id: Uuid, bot_id: Option<String>) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_telegram_bot_id_with_carrier_permit(&carrier_permit, id, bot_id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_telegram_bot_id_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        bot_id: Option<String>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.telegram_bot_id = bot_id;
        }
    }

    /// True when the session currently has a persisted Telegram bot assignment.
    /// Used by auto-close; does not expose the bot id.
    pub async fn session_has_telegram_bot(&self, id: Uuid) -> bool {
        let state = self.state.read().await;
        !state.pending_create.contains_key(&id)
            && state
                .sessions
                .get(&id)
                .is_some_and(|s| s.telegram_bot_id.is_some())
    }

    /// Set `was_detached` on the session. Authoritative store for persistence under
    /// Fix A (plan §A3.2). Mutated ONLY by `detach_terminal_inner` (→true) and
    /// `attach_terminal` (→false). See plan §10 rule — the `WindowEvent::Destroyed`
    /// handler must NOT call this.
    pub async fn set_was_detached(&self, id: Uuid, detached: bool) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_was_detached_with_carrier_permit(&carrier_permit, id, detached)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_was_detached_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        detached: bool,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.was_detached = detached;
        }
    }

    /// Record the detached window's last-known geometry. Called by the frontend on
    /// drag/resize via the `set_detached_geometry` Tauri command. Read at spawn
    /// time by `detach_terminal_inner` (including the Phase 3 restore path).
    pub async fn set_detached_geometry(&self, id: Uuid, geometry: WindowGeometry) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.detached_geometry = Some(geometry);
        }
    }

    /// Register the effective arg vector actually handed to portable-pty
    /// at spawn time. Called by `create_session_inner` immediately before
    /// `pty_mgr.spawn`. Idempotent: callers write the final vec once per
    /// session lifetime. Overwrites on re-call (defensive; not expected in
    /// normal flow).
    pub async fn set_effective_shell_args(&self, id: Uuid, args: Vec<String>) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_effective_shell_args_with_carrier_permit(&carrier_permit, id, args)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_effective_shell_args_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        args: Vec<String>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.effective_shell_args = Some(args);
        }
    }

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn set_pending_effective_shell_args(
        &self,
        binding: PendingCreateBinding,
        args: Vec<String>,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_effective_shell_args_with_carrier_permit(&carrier_permit, binding, args)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_effective_shell_args_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        args: Vec<String>,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.effective_shell_args = Some(args);
        })
        .await
    }

    /// Overwrite `git_repos` atomically. Bumps `git_repos_gen`. Invariant:
    /// callers preserve insertion order (replica config.json `repos` array order).
    pub async fn set_git_repos(&self, id: Uuid, repos: Vec<SessionRepo>) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_git_repos_with_carrier_permit(&carrier_permit, id, repos)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_git_repos_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        repos: Vec<SessionRepo>,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.git_repos = repos;
            s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
        }
    }

    /// Compare-and-swap variant for the watcher. Only writes if `expected_gen` still
    /// matches `git_repos_gen`. On mismatch a concurrent refresh has landed; the watcher
    /// discards its stale detection to prevent emit reordering (see §2.1.d / Grinch #14).
    /// Returns true on successful write.
    pub async fn set_git_repos_if_gen(
        &self,
        id: Uuid,
        repos: Vec<SessionRepo>,
        expected_gen: u64,
    ) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_git_repos_if_gen_with_carrier_permit(&carrier_permit, id, repos, expected_gen)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_git_repos_if_gen_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        repos: Vec<SessionRepo>,
        expected_gen: u64,
    ) -> bool {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            if s.git_repos_gen == expected_gen {
                s.git_repos = repos;
                s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
                return true;
            }
        }
        false
    }

    /// Snapshot the current `git_repos_gen` for a session. Used by watchers to capture
    /// generation at the start of a poll so `set_git_repos_if_gen` can detect a race.
    pub async fn get_git_repos_gen(&self, id: Uuid) -> Option<u64> {
        let state = self.state.read().await;
        (!state.pending_create.contains_key(&id))
            .then(|| state.sessions.get(&id).map(|s| s.git_repos_gen))
            .flatten()
    }

    /// Overwrite `is_coordinator`. Use after a team-config refresh.
    pub async fn set_is_coordinator(&self, id: Uuid, is_coordinator: bool) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_is_coordinator_with_carrier_permit(&carrier_permit, id, is_coordinator)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_is_coordinator_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        is_coordinator: bool,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.is_coordinator = is_coordinator;
        }
    }

    /// Recompute `is_coordinator` for every session using the current team snapshot.
    /// Returns the list of (session_id, new_value) pairs whose flag actually changed,
    /// so callers can emit a single event batch.
    pub async fn refresh_coordinator_flags(
        &self,
        teams: &[crate::config::teams::DiscoveredTeam],
    ) -> Vec<(Uuid, bool)> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.refresh_coordinator_flags_with_carrier_permit(&carrier_permit, teams)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn refresh_coordinator_flags_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        teams: &[crate::config::teams::DiscoveredTeam],
    ) -> Vec<(Uuid, bool)> {
        let mut state = self.carrier_state_write(carrier_permit).await;
        let pending = state.pending_create.keys().copied().collect::<HashSet<_>>();
        let mut changes = Vec::new();
        for (id, s) in state
            .sessions
            .iter_mut()
            .filter(|(id, _)| !pending.contains(id))
        {
            let new_val = crate::config::teams::is_coordinator_for_cwd(&s.working_directory, teams);
            if s.is_coordinator != new_val {
                s.is_coordinator = new_val;
                changes.push((*id, new_val));
            }
        }
        changes
    }

    /// Replace `git_repos` for sessions whose name matches. Bumps `git_repos_gen` on every
    /// write so an in-flight `GitWatcher::poll` that captured the pre-refresh snapshot
    /// cannot overwrite us (see §2.1.d / Grinch #14).
    /// Returns the list of (session_id, new_repos) pairs where a write actually happened.
    pub async fn refresh_git_repos_for_sessions(
        &self,
        updates: &[(String, Vec<SessionRepo>)],
    ) -> Vec<(Uuid, Vec<SessionRepo>)> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.refresh_git_repos_for_sessions_with_carrier_permit(&carrier_permit, updates)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn refresh_git_repos_for_sessions_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        updates: &[(String, Vec<SessionRepo>)],
    ) -> Vec<(Uuid, Vec<SessionRepo>)> {
        let mut state = self.carrier_state_write(carrier_permit).await;
        let pending = state.pending_create.keys().copied().collect::<HashSet<_>>();
        let mut changed = Vec::new();
        for (name, repos) in updates {
            if let Some((id, s)) = state
                .sessions
                .iter_mut()
                .filter(|(id, _)| !pending.contains(id))
                .find(|(_, s)| &s.name == name)
            {
                if &s.git_repos != repos {
                    s.git_repos = repos.clone();
                    s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
                    changed.push((*id, repos.clone()));
                }
            }
        }
        changed
    }
}
