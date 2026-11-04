use uuid::Uuid;

use super::HoldPolicy;
use crate::session::manager::{CarrierSnapshot, SessionManager};
use crate::session::session::Session;

pub(super) async fn prepare_text<'a, F>(
    manager: &SessionManager,
    session_id: Uuid,
    session: Option<Session>,
    hold_policy: &HoldPolicy<'_>,
    pre_write_check: F,
    text: &'a str,
    send_enter: bool,
) -> Result<(Option<F>, &'a str), String>
where
    F: FnOnce(Option<&Session>, Option<&CarrierSnapshot>) -> Result<(), String>,
{
    // Deliberately synchronous and immediately adjacent to the serialized write
    // boundary. Callers of the supported variant perform their final
    // filesystem/config guard here.
    // Peer callbacks use the fresh interior snapshot below, after all waits.
    let legacy_check = if matches!(hold_policy, HoldPolicy::PeerWake(_)) {
        Some(pre_write_check)
    } else {
        // Internal notices must not admit a stale row after waiting for the writer.
        // Refresh without retaining a shared state guard during caller I/O.
        let current = if matches!(hold_policy, HoldPolicy::InternalNotice) {
            manager.get_session(session_id).await
        } else {
            session
        };
        pre_write_check(current.as_ref(), None)?;
        None
    };

    // #2586 D1 - on a `send_enter` shell the two later lone `\r` are the submit;
    // the wake render's trailing `\n\r` rides inside the pasted burst and can
    // only add a blank line. Strip exactly that two-byte suffix and nothing
    // else, never down to empty. Plain shells keep every byte.
    let text = match text.strip_suffix("\n\r") {
        Some(stripped) if send_enter && !stripped.is_empty() => stripped,
        _ => text,
    };

    Ok((legacy_check, text))
}
