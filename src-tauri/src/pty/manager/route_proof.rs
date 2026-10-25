use crate::errors::AppError;
use crate::pty::backend::{SessionBackendKind, TerminalScreenCopyRead, TerminalScreenRead};
use crate::pty::context_scrape::ContextSessionLiveness;
use crate::pty::manager::{
    PtyAuthorityRouteProof, PtyInputPermit, PtyManager, PtyRouteWriteGuard, PtySnapshotRouteProof,
};
use std::sync::Arc;
use uuid::Uuid;

pub(super) fn snapshot_route_proof(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtySnapshotRouteProof, AppError> {
    let manager = manager
        .lock()
        .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
    let proof = manager
        .registry
        .snapshot_route_proof(&[session_id], true)?
        .pop()
        .flatten()
        .expect("required route proof");
    Ok(PtySnapshotRouteProof {
        backend: manager.backend_for_kind(proof.backend_kind()),
        proof,
        session_id,
    })
}

pub(super) fn authority_route_proof(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtyAuthorityRouteProof, AppError> {
    let manager = manager
        .lock()
        .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
    manager.registry.authority_route_proof(session_id)
}

pub async fn acquire_input_writer(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtyInputPermit, AppError> {
    let preparation = {
        let manager = manager
            .lock()
            .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
        manager
            .registry
            .prepare(session_id, &manager.response_close)?
    };
    preparation.acquire().await
}

impl PtySnapshotRouteProof {
    pub(crate) fn backend_kind(&self) -> SessionBackendKind {
        self.proof.backend_kind()
    }
    pub(crate) fn liveness(&self) -> ContextSessionLiveness {
        self.backend.context_session_liveness(self.session_id)
    }
    pub(crate) fn saved_cwd(&self) -> &crate::path_identity::VerifiedPathIdentity {
        self.proof.saved_cwd()
    }
    pub(crate) fn saved_cwd_matches_path(&self, path: &std::path::Path) -> bool {
        self.proof.saved_cwd_matches_path(path)
    }
    pub(crate) fn saved_replica(&self) -> Option<&crate::path_identity::VerifiedPathIdentity> {
        self.proof.saved_replica()
    }
    pub(crate) fn matches_requester_route(
        &self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: Option<&crate::path_identity::VerifiedPathIdentity>,
    ) -> bool {
        self.proof
            .matches_requester_route(expected_kind, expected_cwd, expected_replica)
    }
    pub(crate) fn matches_current(
        &self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> bool {
        self.proof
            .matches_current(expected_kind, expected_cwd, expected_replica)
    }
    pub(crate) fn capture_verified(
        &self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> TerminalScreenRead {
        let lifecycle =
            match self
                .proof
                .lock_verified(expected_kind, expected_cwd, expected_replica)
            {
                Ok(guard) => guard,
                Err(_) => return TerminalScreenRead::Unavailable,
            };
        let copied = self.backend.copy_terminal_screen(self.session_id);
        drop(lifecycle);
        match copied {
            TerminalScreenCopyRead::Copied(captured) => captured
                .into_model(self.session_id, self.proof.backend_kind())
                .map(TerminalScreenRead::Captured)
                .unwrap_or(TerminalScreenRead::Unavailable),
            TerminalScreenCopyRead::Unavailable => TerminalScreenRead::Unavailable,
            TerminalScreenCopyRead::TooLarge => TerminalScreenRead::TooLarge,
        }
    }
}

pub fn lock_route_for_write(permit: &PtyInputPermit) -> Result<PtyRouteWriteGuard<'_>, AppError> {
    crate::pty::input_route::lock_route_for_write(permit)
}

pub(crate) fn lock_route_for_verified_write<'a>(
    permit: &'a PtyInputPermit,
    expected_kind: SessionBackendKind,
    expected_cwd: &crate::path_identity::VerifiedPathIdentity,
    expected_replica: &crate::path_identity::VerifiedPathIdentity,
) -> Result<PtyRouteWriteGuard<'a>, AppError> {
    crate::pty::input_route::lock_route_for_verified_write(
        permit,
        expected_kind,
        expected_cwd,
        expected_replica,
    )
}

pub fn write_with_permit(permit: &PtyInputPermit, bytes: &[u8]) -> Result<(), AppError> {
    crate::pty::input_route::write_with_permit(permit, bytes)
}
