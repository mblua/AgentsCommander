use super::super::session::{SessionCommunication, SessionCommunicationKind, SessionStatus};
use super::carrier::{
    acquire_carrier_mutation_permit, CarrierMutationPermit, RaiseCarrierOutcome,
    RaiseMutationReceipt, RaiseRollbackOutcome,
};
use super::{PendingCreateBinding, SessionManager};
use crate::errors::AppError;
use uuid::Uuid;

impl SessionManager {
    /// #1088 - write the scraper's latest context-usage percent onto one
    /// session so it rides `Session -> SessionInfo -> snapshot_sessions` into
    /// `sessions.json` for the disk-reading CLI. No logging (unlike
    /// `mark_idle`/`mark_busy`) to avoid a log line up to once per 5s per
    /// changing session. A write for an absent/pending id is a silent no-op,
    /// matching this mutator family and the scraper's "session may have ended
    /// between sample and commit" reality.
    pub async fn set_context_percent(&self, id: Uuid, percent: Option<u8>) {
        let mut state = self.state.write().await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.context_percent = percent;
        }
    }

    pub async fn raise_hand(
        &self,
        id: Uuid,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        match self
            .raise_hand_with_carrier_permit(&carrier_permit, id, updated_at)
            .await
        {
            RaiseCarrierOutcome::Changed(communication, _) => Some((true, communication)),
            RaiseCarrierOutcome::AlreadyVisible(communication) => Some((false, communication)),
            RaiseCarrierOutcome::NotRaisable => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn raise_hand_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> RaiseCarrierOutcome {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return RaiseCarrierOutcome::NotRaisable;
        }
        let Some(session) = state.sessions.get_mut(&id) else {
            return RaiseCarrierOutcome::NotRaisable;
        };
        if !session.is_coordinator || matches!(session.status, SessionStatus::Exited(_)) {
            return RaiseCarrierOutcome::NotRaisable;
        }

        if let Some(existing) = session.communication.as_ref() {
            if existing.kind == SessionCommunicationKind::RaiseHand && existing.visible {
                return RaiseCarrierOutcome::AlreadyVisible(existing.clone());
            }
        }

        let communication = SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: updated_at.to_rfc3339(),
            message: None,
        };
        session.communication = Some(communication.clone());
        drop(state);
        let mut state = self.state.write().await;
        let nonce = Uuid::new_v4();
        let version = state.carrier_versions.get(&id).copied().unwrap_or(0);
        state.raise_nonces.insert(id, nonce);
        RaiseCarrierOutcome::Changed(
            communication,
            RaiseMutationReceipt {
                uuid: id,
                nonce,
                version,
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) async fn rollback_raise_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        receipt: RaiseMutationReceipt,
    ) -> RaiseRollbackOutcome {
        let mut state = self.carrier_state_write(carrier_permit).await;
        let Some(session) = state.sessions.get(&receipt.uuid) else {
            return RaiseRollbackOutcome::Missing;
        };
        if state.raise_nonces.get(&receipt.uuid) != Some(&receipt.nonce)
            || state.carrier_versions.get(&receipt.uuid) != Some(&receipt.version)
            || !session.communication.as_ref().is_some_and(|comm| {
                comm.kind == SessionCommunicationKind::RaiseHand && comm.visible
            })
        {
            return RaiseRollbackOutcome::Superseded;
        }
        state.sessions.get_mut(&receipt.uuid).unwrap().communication = None;
        state.raise_nonces.remove(&receipt.uuid);
        RaiseRollbackOutcome::Reverted
    }

    pub async fn clear_communication_if_kind(
        &self,
        id: Uuid,
        kind: SessionCommunicationKind,
    ) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_communication_if_kind_with_carrier_permit(&carrier_permit, id, kind)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_communication_if_kind_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        kind: SessionCommunicationKind,
    ) -> bool {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        let Some(session) = state.sessions.get_mut(&id) else {
            return false;
        };
        let should_clear = session
            .communication
            .as_ref()
            .is_some_and(|communication| communication.kind == kind && communication.visible);
        if should_clear {
            session.communication = None;
        }
        should_clear
    }

    /// #1646 / #1647 - Sets `session.communication` to `BlockedMenu` with a custom message.
    /// Returns `Some((changed, communication))` or `None` if the session was not found
    /// or is exited.
    pub async fn set_blocked_menu(
        &self,
        id: Uuid,
        message: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_blocked_menu_with_carrier_permit(&carrier_permit, id, message, updated_at)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_blocked_menu_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        message: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return None;
        }
        let session = state.sessions.get_mut(&id)?;
        if matches!(session.status, SessionStatus::Exited(_)) {
            return None;
        }

        if let Some(existing) = session.communication.as_ref() {
            if existing.kind == SessionCommunicationKind::BlockedMenu
                && existing.visible
                && existing.message.as_deref() == Some(&message)
            {
                return Some((false, existing.clone()));
            }
        }

        let communication = SessionCommunication {
            kind: SessionCommunicationKind::BlockedMenu,
            visible: true,
            updated_at: updated_at.to_rfc3339(),
            message: Some(message),
        };
        session.communication = Some(communication.clone());
        Some((true, communication))
    }

    /// #1646 / #1647 - Clears `session.communication` if currently `Some` with
    /// `kind == SessionCommunicationKind::BlockedMenu` and `visible == true`.
    /// Returns `true` if cleared.
    pub async fn clear_blocked_menu(&self, id: Uuid) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_blocked_menu_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_blocked_menu_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) -> bool {
        self.clear_communication_if_kind_with_carrier_permit(
            carrier_permit,
            id,
            SessionCommunicationKind::BlockedMenu,
        )
        .await
    }

    /// #2232 phase 7 - sets `session.communication` to `CoManaged` with the
    /// visible text the Co-managed feature produced (reason, excerpt and the
    /// messaging-file path). Same shape and dedup rule as `set_blocked_menu`:
    /// re-setting the identical visible message reports `changed == false`.
    ///
    /// The slot is single and shared with `raise_hand` / `set_blocked_menu`:
    /// the newer writer overwrites, which is the documented phase-7 behaviour.
    pub async fn set_co_managed(
        &self,
        id: Uuid,
        message: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_co_managed_with_carrier_permit(&carrier_permit, id, message, updated_at)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_co_managed_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        message: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<(bool, SessionCommunication)> {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return None;
        }
        let session = state.sessions.get_mut(&id)?;
        if matches!(session.status, SessionStatus::Exited(_)) {
            return None;
        }

        if let Some(existing) = session.communication.as_ref() {
            if existing.kind == SessionCommunicationKind::CoManaged
                && existing.visible
                && existing.message.as_deref() == Some(&message)
            {
                return Some((false, existing.clone()));
            }
        }

        let communication = SessionCommunication {
            kind: SessionCommunicationKind::CoManaged,
            visible: true,
            updated_at: updated_at.to_rfc3339(),
            message: Some(message),
        };
        session.communication = Some(communication.clone());
        Some((true, communication))
    }

    /// #2232 phase 7 - clears the session's communication **only when the
    /// single slot currently holds a visible Co-managed message**, leaving any
    /// other kind untouched; returns whether it cleared one.
    ///
    /// Nothing in production calls this today. Plan section 7 makes the
    /// Co-managed communication the user's reading surface (reason, excerpt
    /// and message-file path) and gives the slot the single-slot overwrite
    /// lifecycle: `raise_hand`, `set_blocked_menu` or a newer Co-managed
    /// message replaces the current entry. The readiness-loss clear of plan
    /// 9.3 clears the armed flag and emits its event; it deliberately does not
    /// erase this user-visible pointer while the referenced message file stays
    /// on disk.
    pub async fn clear_co_managed(&self, id: Uuid) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_co_managed_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_co_managed_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) -> bool {
        self.clear_communication_if_kind_with_carrier_permit(
            carrier_permit,
            id,
            SessionCommunicationKind::CoManaged,
        )
        .await
    }

    /// (#747) Re-apply a persisted raise-hand onto a restored session record.
    /// Unlike `raise_hand`, this deliberately ACCEPTS records in
    /// `SessionStatus::Exited(_)`: the startup defer arm restores dormant
    /// placeholders that must keep their raised hand until real user input
    /// (issue #747 supersedes #676's ephemeral-only rule). Gates: coordinator
    /// records only, visible `RaiseHand` payloads only. Preserves the caller's
    /// `updated_at` (the original raise time) so the indicator's age stays
    /// truthful. Returns true when the communication was applied.
    pub async fn restore_communication(
        &self,
        id: Uuid,
        communication: SessionCommunication,
    ) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.restore_communication_with_carrier_permit(&carrier_permit, id, communication)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn restore_communication_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        communication: SessionCommunication,
    ) -> bool {
        if communication.kind != SessionCommunicationKind::RaiseHand || !communication.visible {
            return false;
        }
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        let Some(session) = state.sessions.get_mut(&id) else {
            return false;
        };
        if !session.is_coordinator {
            return false;
        }
        session.communication = Some(communication);
        true
    }

    /// (#630/#631) Stamp the durable resume intent. Used by the restart path and
    /// the restore wake/defer paths.
    pub async fn set_start_fresh_on_restore(&self, id: Uuid, value: bool) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_start_fresh_on_restore_with_carrier_permit(&carrier_permit, id, value)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_start_fresh_on_restore_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        value: bool,
    ) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            s.start_fresh_on_restore = value;
        }
    }

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn set_pending_start_fresh_on_restore(
        &self,
        binding: PendingCreateBinding,
        value: bool,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_start_fresh_on_restore_with_carrier_permit(&carrier_permit, binding, value)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_start_fresh_on_restore_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        value: bool,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.start_fresh_on_restore = value;
        })
        .await
    }

    #[expect(
        dead_code,
        reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
    )]
    pub(crate) async fn set_pending_communication(
        &self,
        binding: PendingCreateBinding,
        communication: SessionCommunication,
    ) -> Result<(), AppError> {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_pending_communication_with_carrier_permit(&carrier_permit, binding, communication)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_pending_communication_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        binding: PendingCreateBinding,
        communication: SessionCommunication,
    ) -> Result<(), AppError> {
        self.update_pending_with_carrier_permit(carrier_permit, binding, |session| {
            session.communication = Some(communication);
        })
        .await
    }

    /// (#630/#631) Re-arm on first real user message: clear the fresh intent.
    /// Returns true ONLY on the `true -> false` transition, so the caller persists
    /// exactly once (not on every subsequent keystroke).
    pub async fn clear_start_fresh_on_restore_if_set(&self, id: Uuid) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_start_fresh_on_restore_if_set_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_start_fresh_on_restore_if_set_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) -> bool {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            if s.start_fresh_on_restore {
                log::info!(
                    "[session-state] {} '{}': start_fresh_on_restore true -> false (re-armed)",
                    &id.to_string()[..8],
                    s.name
                );
                s.start_fresh_on_restore = false;
                return true;
            }
        }
        false
    }

    /// (#756) Stamp the durable fresh intent on an AC-driven clear boundary.
    /// Returns true ONLY on the `false -> true` transition, so the caller
    /// persists exactly once. Missing id -> false. Counterpart of
    /// `clear_start_fresh_on_restore_if_set` (re-arm direction).
    pub async fn set_start_fresh_on_restore_if_unset(&self, id: Uuid) -> bool {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.set_start_fresh_on_restore_if_unset_with_carrier_permit(&carrier_permit, id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_start_fresh_on_restore_if_unset_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
    ) -> bool {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return false;
        }
        if let Some(s) = state.sessions.get_mut(&id) {
            if !s.start_fresh_on_restore {
                log::info!(
                    concat!(
                        "[session-state] {} '{}': start_fresh_on_restore false -> true ",
                        "(AC clear boundary, #756)"
                    ),
                    &id.to_string()[..8],
                    s.name
                );
                s.start_fresh_on_restore = true;
                return true;
            }
        }
        false
    }

    /// #698: clear BOTH user-input session-state transitions in a SINGLE
    /// critical section. `clear_fresh` gates re-arming the durable resume intent
    /// (`start_fresh_on_restore` true -> false, #871); lowering any visible
    /// raise-hand remains unconditional. Returns
    /// `(cleared_start_fresh, cleared_raise_hand)` so the caller can persist once
    /// and emit the raise-hand clear event only when a hand was actually lowered.
    ///
    /// Doing both under one `sessions.write()` acquisition is the MEDIUM grinch
    /// fix: with the previous two-call shape a concurrent snapshot could observe a
    /// half-applied state (`start_fresh_on_restore` already cleared while the hand
    /// was still raised). Mutating both fields atomically removes that window.
    pub async fn clear_user_input_transitions(&self, id: Uuid, clear_fresh: bool) -> (bool, bool) {
        let carrier_permit = acquire_carrier_mutation_permit().await;
        self.clear_user_input_transitions_with_carrier_permit(&carrier_permit, id, clear_fresh)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn clear_user_input_transitions_with_carrier_permit(
        &self,
        carrier_permit: &CarrierMutationPermit,
        id: Uuid,
        clear_fresh: bool,
    ) -> (bool, bool) {
        let mut state = self.carrier_state_write(carrier_permit).await;
        if state.pending_create.contains_key(&id) {
            return (false, false);
        }
        let Some(s) = state.sessions.get_mut(&id) else {
            return (false, false);
        };

        let cleared_start_fresh = if clear_fresh && s.start_fresh_on_restore {
            log::debug!(
                concat!(
                    "[session-state] {} '{}': start_fresh_on_restore true -> false ",
                    "(substantive user message, re-armed)"
                ),
                &id.to_string()[..8],
                s.name
            );
            s.start_fresh_on_restore = false;
            true
        } else {
            false
        };

        let cleared_raise_hand = s
            .communication
            .as_ref()
            .is_some_and(|c| c.kind == SessionCommunicationKind::RaiseHand && c.visible);
        if cleared_raise_hand {
            s.communication = None;
        }

        (cleared_start_fresh, cleared_raise_hand)
    }
}
