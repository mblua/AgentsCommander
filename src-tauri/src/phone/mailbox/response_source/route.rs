use super::super::notice::sender_name_for_session_cwd_with_root_flag;
use crate::phone::types::OutboxMessage;
use crate::pty::manager::{ResponseCloseState, ResponseRouteIdentity};
use crate::session::manager::CarrierSnapshot;
use crate::session::session::{Session, SessionStatus};
use uuid::Uuid;

pub(super) fn response_return_route<'a>(
    snapshot: &'a CarrierSnapshot,
    state: &ResponseCloseState,
    msg: &OutboxMessage,
    target: &str,
) -> Option<(ResponseRouteIdentity, &'a Session)> {
    let mut matches = snapshot.rows.iter().filter(|row| {
        !snapshot.pending_ids.contains(&row.id)
            && sender_name_for_session_cwd_with_root_flag(&row.working_directory, row.is_root_agent)
                == target
            && !matches!(row.status, SessionStatus::Exited(_))
    });
    let row = response_diag_option_local!(
        matches.next(),
        "admit_source",
        "route_missing",
        Uuid::parse_str(&msg.id).ok()
    )?;
    if matches.next().is_some() {
        response_diag_local!(
            "admit_source",
            "route_ambiguous",
            Uuid::parse_str(&msg.id).ok()
        );
        return None;
    }
    Some((
        ResponseRouteIdentity {
            target: target.to_string(),
            session_id: row.id,
            token: row.token,
            generation: response_diag_option_local!(
                state.generation(row.id),
                "admit_source",
                "generation_unavailable",
                Uuid::parse_str(&msg.id).ok()
            )?,
            carrier_version: *response_diag_option_local!(
                snapshot.versions.get(&row.id),
                "admit_source",
                "version_unavailable",
                Uuid::parse_str(&msg.id).ok()
            )?,
            backend: row.backend_kind,
        },
        row,
    ))
}
