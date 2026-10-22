use super::*;
use crate::session::selection::TrustedCreateIntent;

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
