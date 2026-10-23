use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
use uuid::Uuid;

/// Maps the agent submit outcome to the terminal PTY input status and reason,
/// stamping boundary metadata on the selected session after a submit.
pub(super) async fn pty_submit_terminal_status<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    payload: &[u8],
    selected_id: Uuid,
    outcome: crate::pty::inject::AgentSubmitOutcome,
) -> (S, Option<C>) {
    match outcome {
        crate::pty::inject::AgentSubmitOutcome::TextWriteFailed => {
            (S::Indeterminate, Some(C::TextWriteFailed))
        }
        crate::pty::inject::AgentSubmitOutcome::RequiredEnterFailed => {
            (S::Indeterminate, Some(C::RequiredEnterFailed))
        }
        crate::pty::inject::AgentSubmitOutcome::Submitted {
            redundant_enter_failed,
        } => {
            let metadata = if payload == b"/clear" {
                crate::commands::pty::stamp_fresh_boundary_to_session(app, selected_id).await
            } else if payload == b"/compact" {
                crate::commands::pty::BoundaryMetadataOutcome::Unchanged
            } else {
                crate::commands::pty::note_post_boundary_content_to_session(app, selected_id).await
            };
            let reason = if metadata == crate::commands::pty::BoundaryMetadataOutcome::Failed {
                Some(C::BoundaryMetadataFailed)
            } else {
                redundant_enter_failed.then_some(C::RedundantEnterFailed)
            };
            (S::Injected, reason)
        }
    }
}
