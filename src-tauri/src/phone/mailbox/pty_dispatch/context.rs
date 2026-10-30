use super::super::settle::session_has_current_pty_submission_provenance;
use super::super::settle::{OwnedPtyWorkTransfer, PtySpawnStep};
use crate::api::message_store::{
    ClaimedPtyInputOperation, MessageStore, PreparationHeartbeatGuard, PtyInputStripeGuard,
    PtyInputTargetGate, PtyInputTargetGuard,
};
use crate::config::teams::VerifiedPtyInputIdentity;
use crate::phone::mailbox::pty_claim::PtyInputPreBoundaryStop;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
/// Session chosen (or spawned) for a PTY input dispatch, with the verified
/// target identity and the managers the boundary write needs.
pub(super) type SelectedPtyTarget = (
    SessionInfo,
    bool,
    VerifiedPtyInputIdentity,
    Arc<tokio::sync::RwLock<SessionManager>>,
    Arc<Mutex<PtyManager>>,
);

/// Claimed operation facts the session selection reads but never changes.
pub(super) struct PtyDispatchContext<'a> {
    pub(super) store: &'a Arc<MessageStore>,
    pub(super) initial: &'a crate::phone::types::PtyInputResult,
    pub(super) claimed: &'a ClaimedPtyInputOperation,
    pub(super) injection_id: &'a str,
    pub(super) lease_owner: &'a str,
    pub(super) source_plane: crate::phone::types::PtyInputSourcePlane,
    pub(super) api_client_store: Option<&'a Arc<crate::api::auth::ApiClientStore>>,
    pub(super) target_gate: &'a Arc<PtyInputTargetGate>,
    pub(super) target: &'a String,
    pub(super) expires: chrono::DateTime<chrono::Utc>,
    pub(super) shutdown: &'a tokio_util::sync::CancellationToken,
}

/// Dispatch guards the session selection may release, reacquire or fill.
pub(super) struct PtyDispatchGuards<'a> {
    pub(super) target_guard: &'a mut Option<PtyInputTargetGuard>,
    pub(super) target_stripe: &'a mut Option<PtyInputStripeGuard>,
    pub(super) heartbeat: &'a mut PreparationHeartbeatGuard,
    pub(super) owned_work: &'a mut Option<OwnedPtyWorkTransfer>,
    pub(super) source_executor: &'a mut crate::session::selection::SourceExecutorGuard,
}

pub(super) struct SelectionCandidates {
    pub(super) matching: Vec<SessionInfo>,
    pub(super) eligible: Vec<SessionInfo>,
    pub(super) saw_supported_busy: bool,
    pub(super) saw_live_unsupported: bool,
    pub(super) saw_live_inconsistent: bool,
    pub(super) saw_live_temporary: bool,
}

pub(super) type SpawnAttempt =
    Result<Result<PtySpawnStep, crate::phone::types::PtyInputReasonCode>, PtyInputPreBoundaryStop>;
pub(super) struct InitialDispatchContext<'a> {
    pub(super) store: &'a Arc<crate::api::message_store::MessageStore>,
    pub(super) initial: &'a crate::phone::types::PtyInputResult,
    pub(super) injection_id: &'a str,
    pub(super) source_plane: crate::phone::types::PtyInputSourcePlane,
    pub(super) api_client_store: Option<&'a Arc<crate::api::auth::ApiClientStore>>,
    pub(super) target_gate: &'a Arc<crate::api::message_store::PtyInputTargetGate>,
    pub(super) target: &'a String,
    pub(super) shutdown: &'a tokio_util::sync::CancellationToken,
}
#[derive(Clone, Copy)]
pub(super) struct DispatchSelection<'a> {
    pub(super) selected: &'a SessionInfo,
    pub(super) spawned: bool,
    pub(super) verified_target: &'a crate::config::teams::VerifiedPtyInputIdentity,
    pub(super) session_manager:
        &'a Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>,
    pub(super) pty_manager: &'a Arc<std::sync::Mutex<PtyManager>>,
    pub(super) selected_id: Uuid,
}
pub(super) struct BoundaryAuthority {
    pub(super) manager: crate::session::manager::SessionManager,
    pub(super) idle_detector: Arc<crate::pty::idle_detector::IdleDetector>,
    pub(super) authority_id: Uuid,
    pub(super) authority_backend: SessionBackendKind,
    pub(super) authority_container_backend:
        Arc<crate::pty::container_backend::ContainerTransportBackend>,
    pub(super) authority_route: crate::pty::manager::PtyAuthorityRouteProof,
    pub(super) expected_api_binding: Option<(
        String,
        String,
        String,
        crate::path_identity::FileObjectId,
        String,
    )>,
}

impl PtyDispatchGuards<'_> {
    pub(super) fn reborrow(&mut self) -> PtyDispatchGuards<'_> {
        PtyDispatchGuards {
            target_guard: self.target_guard,
            target_stripe: self.target_stripe,
            heartbeat: self.heartbeat,
            owned_work: self.owned_work,
            source_executor: self.source_executor,
        }
    }
}

pub(super) async fn dispatch_selection_candidates<
    F: std::future::Future<Output = crate::config::settings::AppSettings>,
>(
    claimed: &ClaimedPtyInputOperation,
    session_manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_manager: &Arc<Mutex<PtyManager>>,
    verified_target: &VerifiedPtyInputIdentity,
    settings: impl FnOnce() -> F,
) -> SelectionCandidates {
    let sessions = {
        let guard = session_manager.read().await;
        guard.list_sessions().await
    };
    let mut matching: Vec<SessionInfo> = sessions
        .into_iter()
        .filter(|session| {
            crate::config::teams::agent_fqn_from_path(&session.working_directory)
                == claimed.target_fqn
        })
        .collect();
    matching.sort_by(|left, right| {
        fn rank(status: &SessionStatus) -> u8 {
            match status {
                SessionStatus::Active => 0,
                SessionStatus::Running => 1,
                SessionStatus::Idle => 2,
                SessionStatus::Exited(_) => 3,
            }
        }
        rank(&left.status)
            .cmp(&rank(&right.status))
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });

    let pty_profile_settings = settings().await;
    let mut eligible = Vec::new();
    let mut saw_supported_busy = false;
    let mut saw_live_unsupported = false;
    let mut saw_live_inconsistent = false;
    let mut saw_live_temporary = false;
    for session in &matching {
        if matches!(session.status, SessionStatus::Exited(_)) {
            continue;
        }
        if session
            .name
            .starts_with(crate::session::session::TEMP_SESSION_PREFIX)
        {
            saw_live_temporary = true;
            continue;
        }
        let Ok(id) = Uuid::parse_str(&session.id) else {
            saw_live_unsupported = true;
            continue;
        };
        let (live, route_backend, route_identities) = {
            let manager = pty_manager
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (
                manager.has_session(id),
                manager.backend_kind(id),
                manager.route_identities(id),
            )
        };
        let path_matches = route_identities.is_some_and(|(cwd_identity, replica_anchor, _)| {
            let current_cwd =
                crate::path_identity::verify_directory(Path::new(&session.working_directory));
            let current_target = crate::config::teams::verify_pty_input_replica_cwd(Path::new(
                &session.working_directory,
            ));
            matches!(
                (cwd_identity, current_cwd),
                (Some(saved), Ok(current))
                    if crate::path_identity::same_object(&saved, &current)
            ) && matches!(
                (replica_anchor, current_target),
                (Some(saved), Ok(current))
                    if current.canonical_fqn == verified_target.canonical_fqn
                        && current.authority_fingerprint
                            == verified_target.authority_fingerprint
                        && crate::path_identity::same_object(
                            &saved,
                            &verified_target.replica_identity,
                        )
            )
        });
        if !live || route_backend != Some(session.backend_kind) || !path_matches {
            saw_live_inconsistent = true;
            continue;
        }
        if !session_has_current_pty_submission_provenance(session, &pty_profile_settings) {
            saw_live_unsupported = true;
            continue;
        }
        if session.waiting_for_input {
            eligible.push(session.clone());
        } else {
            saw_supported_busy = true;
        }
    }

    SelectionCandidates {
        matching,
        eligible,
        saw_supported_busy,
        saw_live_unsupported,
        saw_live_inconsistent,
        saw_live_temporary,
    }
}
