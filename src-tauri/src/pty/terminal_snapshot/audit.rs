use super::{TerminalSnapshotServiceRequest, TerminalSnapshotSourcePlane};
use crate::config::teams::VerifiedTerminalSnapshotRoute;
use crate::pty::backend::SessionBackendKind;
use crate::session::manager::TerminalSnapshotSessionFact;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use terminal_snapshot_renderer::{TerminalScreenModel, TerminalSnapshotReasonCode};

pub(crate) struct AuditInner {
    pub(crate) finalized: AtomicBool,
    pub(crate) metadata: Mutex<crate::api::audit::TerminalSnapshotAuditMetadata>,
}

impl Drop for AuditInner {
    fn drop(&mut self) {
        if self.finalized.swap(true, Ordering::SeqCst) {
            return;
        }
        let Ok(mut metadata) = self.metadata.lock() else {
            return;
        };
        metadata.completed_at = terminal_snapshot_renderer::canonical_timestamp(chrono::Utc::now());
        metadata.status = "failed".to_string();
        metadata.reason_code = Some("internal".to_string());
        crate::api::audit::record_terminal_snapshot(&metadata);
    }
}

#[derive(Clone)]
pub(crate) struct TerminalSnapshotAuditGuard {
    pub(crate) inner: Arc<AuditInner>,
}

impl TerminalSnapshotAuditGuard {
    pub(crate) fn pre_admission(source_plane: TerminalSnapshotSourcePlane) -> Self {
        Self {
            inner: Arc::new(AuditInner {
                finalized: AtomicBool::new(false),
                metadata: Mutex::new(crate::api::audit::TerminalSnapshotAuditMetadata {
                    event: "terminal_snapshot".to_string(),
                    request_id: None,
                    requester_fqn: None,
                    target_fqn: None,
                    source_plane: source_plane.as_str().to_string(),
                    format: None,
                    selected_session_id: None,
                    selected_backend: None,
                    rows: None,
                    columns: None,
                    sequence: None,
                    captured_at: None,
                    payload_bytes: None,
                    accepted_at: None,
                    completed_at: String::new(),
                    status: "failed".to_string(),
                    reason_code: None,
                }),
            }),
        }
    }

    pub(crate) fn accept_request(&self, request: &TerminalSnapshotServiceRequest) {
        self.update(|metadata| {
            metadata.request_id = Some(request.request_id.to_string());
            metadata.format = Some(request.format.to_string());
        });
    }

    pub(super) fn update(
        &self,
        update: impl FnOnce(&mut crate::api::audit::TerminalSnapshotAuditMetadata),
    ) {
        if let Ok(mut metadata) = self.inner.metadata.lock() {
            update(&mut metadata);
        }
    }

    pub(super) fn accept_requester(&self, requester: &str) {
        self.update(|metadata| {
            metadata.requester_fqn = Some(requester.to_string());
            metadata.accepted_at = Some(terminal_snapshot_renderer::canonical_timestamp(
                chrono::Utc::now(),
            ));
        });
    }

    pub(super) fn accept_route(&self, route: &VerifiedTerminalSnapshotRoute) {
        self.update(|metadata| metadata.target_fqn = Some(route.target.canonical_fqn.clone()));
    }

    pub(super) fn accept_selected(&self, fact: &TerminalSnapshotSessionFact) {
        self.update(|metadata| {
            metadata.selected_session_id = Some(fact.id.to_string());
            metadata.selected_backend = Some(match fact.backend_kind {
                SessionBackendKind::LocalProcess => "localProcess".to_string(),
                SessionBackendKind::ContainerTransport => "containerTransport".to_string(),
            });
        });
    }

    pub(super) fn accept_model(&self, model: &TerminalScreenModel) {
        self.update(|metadata| {
            metadata.rows = Some(model.screen.dimensions.rows);
            metadata.columns = Some(model.screen.dimensions.columns);
            metadata.sequence = Some(model.screen.sequence);
            metadata.captured_at = Some(model.captured_at.clone());
        });
    }

    pub(super) fn accept_payload(&self, payload_bytes: u64) {
        self.update(|metadata| metadata.payload_bytes = Some(payload_bytes));
    }

    pub(crate) async fn wait_for_retained_owners(&self, expected_owners: usize) {
        debug_assert!(expected_owners > 0);
        while Arc::strong_count(&self.inner) > expected_owners {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    pub(crate) fn finalize_failure(&self, reason: TerminalSnapshotReasonCode) {
        let status = match reason {
            TerminalSnapshotReasonCode::TerminalSnapshotsDisabled
            | TerminalSnapshotReasonCode::NotAuthorized
            | TerminalSnapshotReasonCode::InvalidRequest
            | TerminalSnapshotReasonCode::RequesterUnavailable => "rejected",
            _ => "failed",
        };
        self.finalize(status, Some(reason.as_str()));
    }

    pub(super) fn finalize(&self, status: &str, reason: Option<&str>) {
        if self.inner.finalized.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut metadata) = self.inner.metadata.lock() {
            metadata.completed_at =
                terminal_snapshot_renderer::canonical_timestamp(chrono::Utc::now());
            metadata.status = status.to_string();
            metadata.reason_code = reason.map(str::to_string);
            crate::api::audit::record_terminal_snapshot(&metadata);
        }
    }
}
