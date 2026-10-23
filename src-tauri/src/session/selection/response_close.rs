use std::sync::atomic::Ordering;

use std::sync::{Arc, Mutex};

use crate::pty::manager::PtyManager;
use crate::session::session::SessionStatus;

#[derive(Clone)]
pub(crate) struct ResponseCloseRegistration {
    pub tracker: crate::pty::manager::ResponseCloseState,
    pub member: crate::pty::manager::ResponseRouteIdentity,
    pub coordinator: crate::pty::manager::ResponseRouteIdentity,
    pub carrier: crate::config::sessions_persistence::PublishedCarrier,
    pub diagnostic_last_poll_outcome: Arc<std::sync::atomic::AtomicU64>,
}

impl ResponseCloseRegistration {
    pub(super) fn diagnostic_poll_outcome(&self, code: u64, outcome: &'static str) {
        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
            && self
                .diagnostic_last_poll_outcome
                .swap(code, Ordering::Relaxed)
                != code
        {
            log::debug!(
                target: "agentscommander_lib::response_close_diag",
                "response_close_diag id=unknown stage=poll_terminal outcome={} member={} \
                 coordinator={} site={}",
                outcome,
                self.member.session_id,
                self.coordinator.session_id,
                line!()
            );
        }
    }
}

pub(super) fn response_registration_row<'a>(
    current: &'a crate::session::manager::CarrierSnapshot,
    identity: &crate::pty::manager::ResponseRouteIdentity,
) -> Option<&'a crate::session::session::Session> {
    current.rows.iter().find(|row| {
        row.id == identity.session_id
            && row.token == identity.token
            && row.backend_kind == identity.backend
            && current.versions.get(&row.id) == Some(&identity.carrier_version)
            && !current.pending_ids.contains(&row.id)
            && !matches!(row.status, SessionStatus::Exited(_))
    })
}

pub(super) fn response_registration_route(
    pty: &Arc<Mutex<PtyManager>>,
    identity: &crate::pty::manager::ResponseRouteIdentity,
    row: &crate::session::session::Session,
) -> bool {
    let Ok(route) = PtyManager::snapshot_route_proof(pty, identity.session_id) else {
        return false;
    };
    route.backend_kind() == identity.backend
        && route.saved_cwd_matches_path(std::path::Path::new(&row.working_directory))
        && route.matches_requester_route(identity.backend, route.saved_cwd(), route.saved_replica())
}
