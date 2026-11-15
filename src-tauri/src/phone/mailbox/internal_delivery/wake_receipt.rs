use crate::config::coordinator_clocks::CoordinatorClocksState;
use crate::phone::mailbox::handoff::internal_wake_resumes;
use crate::phone::mailbox::pty_claim::ResponseWakeWork;
use crate::phone::types::OutboxMessage;
use uuid::Uuid;

// #2411 An idle auto-close destroys the record, so a no-session wake of an
// auto-closed orchestrator resumes its conversation. Same key as create and
// auto-close. The std mutex is never held across an await.
pub(super) fn background_resume(
    clocks: Option<&CoordinatorClocksState>,
    marker_fqn: &str,
    target_fqn: &str,
    spawn_with_resume: bool,
) -> (bool, String) {
    let auto_closed_at = clocks.and_then(|clocks| {
        clocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .auto_closed_at(marker_fqn)
    });
    let spawn_with_resume = internal_wake_resumes(spawn_with_resume, auto_closed_at.is_some());
    log::info!(
        "[internal-wake] '{target_fqn}' no-live-session spawn resume={spawn_with_resume} \
         auto_closed_at={auto_closed_at:?}"
    );
    let (_, local) = crate::config::teams::split_project_prefix(target_fqn);
    (spawn_with_resume, local.to_string())
}

pub(super) fn mark_response_submitted(response: Option<&ResponseWakeWork>, msg: &OutboxMessage) {
    if let Some(attempt) = response.and_then(|work| match work.attempt.lock() {
        Ok(attempt) => attempt.clone(),
        Err(_) => {
            response_diag_local!(
                "mark_request_submitted",
                "attempt_lock_failure",
                work.diagnostic_message_id
            );
            None
        }
    }) {
        match attempt.candidate.lock() {
            Ok(candidate) => {
                if let Some(candidate) = candidate.as_ref() {
                    response_diag_local!(
                        "mark_request_submitted",
                        "true_invocation",
                        Uuid::parse_str(&msg.id).ok()
                    );
                    attempt
                        .state
                        .mark_request_submitted(&candidate.context, true);
                } else {
                    response_diag_local!(
                        "mark_request_submitted",
                        "candidate_absent",
                        None::<Uuid>
                    );
                }
            }
            _ => {
                response_diag_local!(
                    "mark_request_submitted",
                    "candidate_lock_failure",
                    None::<Uuid>
                );
            }
        }
    } else {
        response_diag_local!(
            "mark_request_submitted",
            "work_or_attempt_absent",
            None::<Uuid>
        );
    }
}

pub(super) fn check_cancellation(
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err("Context alert delivery was canceled".to_string());
    }
    Ok(())
}

pub(super) fn check_purge(
    purge: &crate::session::purge_guard::PurgeGuard,
    target_fqn: &str,
) -> Result<(), String> {
    if purge.blocks_agent(target_fqn) {
        return Err(format!(
            "purge-room in progress for '{target_fqn}'; context alert deferred"
        ));
    }
    Ok(())
}
