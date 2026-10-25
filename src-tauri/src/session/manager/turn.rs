use super::super::session::Session;
use super::carrier::{acquire_carrier_mutation_permit, CarrierMutationPermit};
use super::SessionManager;
use crate::pty::backend::SessionBackendKind;
use uuid::Uuid;

impl SessionManager {
    /// #1149 - the guard added here is emit-only. Every mutation below runs
    /// exactly as it did before, on every call, in every state; only the
    /// `log::info!` and the activity record are conditional. That is what turns
    /// 55,816 `[session-state]` lines into the roughly 8,054 real transitions
    /// among them without changing any behavior.
    pub async fn mark_idle(&self, id: Uuid) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.mark_idle_with_carrier_permit(&carrier_permit, id)
            .await
    }

    /// Mirror of [`Self::mark_idle`]. See the emit-only note there.
    pub async fn mark_busy(&self, id: Uuid) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.mark_busy_with_carrier_permit(&carrier_permit, id)
            .await
    }

    /// #1682 - record that a message write reached an arming site for `id`, NOT that a
    /// message was delivered and submitted: R7 and R8 both arm with nothing submitted.
    /// Idempotent, and never undone: see this phase's D2. Deliberately NOT called from
    /// `mark_busy`, whose output-driven caller would arm a startup repaint.
    pub async fn arm_agent_turn(&self, id: Uuid) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.arm_agent_turn_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn arm_agent_turn_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        let Some(s) = state.sessions.get_mut(&id) else {
            return;
        };
        s.agent_turn_armed = true;
    }

    /// #1682 - where to persist the stamp for `id` on a busy->idle edge, or
    /// `None` when no message ever armed this session or it has no coding
    /// agent. Read-only: the latch is not consumed, so it never blocks a later
    /// edge; the value converges on the last edge the stamp gates let through.
    pub async fn agent_stamp_target(&self, id: Uuid) -> Option<(String, String)> {
        let state = self.state.read().await;
        if state.pending_create.contains_key(&id) {
            return None;
        }
        let s = state.sessions.get(&id)?;
        if !s.agent_turn_armed {
            return None;
        }
        let agent_id = s.agent_id.clone()?;
        Some((s.working_directory.clone(), agent_id))
    }

    /// #1149 - the working-session set, sampled without an await.
    ///
    /// `RunEvent::Exit` runs on the main thread and needs this set while the
    /// session map is still populated, but the only public read path,
    /// `list_sessions`, is `async` and unbounded, and the outer
    /// `Arc<RwLock<SessionManager>>` has no production writer so spinning on it
    /// would bound nothing. `None` means a writer holds or has merely queued for
    /// `state` (`tokio::sync::RwLock` is write-preferring), and the caller is
    /// expected to bound its own retry rather than block.
    ///
    /// Uses `is_working` internally so exactly one definition of "working"
    /// exists, and carries only the four fields an `idle` record needs: no
    /// `token`, no `last_prompt`, no shell args.
    ///
    /// `pub fn`, not `pub async fn`, deliberately: the architecture guard rejects
    /// new public async manager mutators, and a shutdown caller must not await.
    pub fn try_snapshot_working_sessions(
        &self,
    ) -> Option<Vec<crate::config::activity_log::WorkingSessionSnapshot>> {
        let state = self.state.try_read().ok()?;
        Some(
            state
                .order
                .iter()
                .filter(|id| !state.pending_create.contains_key(id))
                .filter_map(|id| state.sessions.get(id))
                .filter(|session| crate::session::session::is_working(session))
                .map(
                    |session| crate::config::activity_log::WorkingSessionSnapshot {
                        id: session.id,
                        name: session.name.clone(),
                        cwd: session.working_directory.clone(),
                        agent_kind: session.agent_kind,
                    },
                )
                .collect(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) async fn prepare_pty_input_boundary<'a, F, G>(
        &self,
        id: Uuid,
        expected_target: &crate::config::teams::VerifiedPtyInputIdentity,
        expected_backend: SessionBackendKind,
        authority_id: Uuid,
        expected_sender: &crate::config::teams::VerifiedPtyInputIdentity,
        authority_backend: SessionBackendKind,
        authority_route: &'a crate::pty::input_route::PtyAuthorityRouteProof,
        permit: &'a crate::pty::input_route::PtyInputPermit,
        idle_detector: &crate::pty::idle_detector::IdleDetector,
        settings: &crate::config::settings::SettingsState,
        final_recipe_check: G,
        final_external_check: F,
    ) -> Result<
        crate::pty::input_route::PtyRouteWriteGuard<'a>,
        crate::pty::idle_detector::PtyInputBoundaryFailure,
    >
    where
        F: FnOnce() -> bool,
        G: FnOnce(&Session, &crate::config::settings::AppSettings) -> bool,
    {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.prepare_pty_input_boundary_with_carrier_permit(
            &carrier_permit,
            id,
            expected_target,
            expected_backend,
            authority_id,
            expected_sender,
            authority_backend,
            authority_route,
            permit,
            idle_detector,
            settings,
            final_recipe_check,
            final_external_check,
        )
        .await
    }
}
