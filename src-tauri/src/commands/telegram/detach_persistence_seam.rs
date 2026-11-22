//! Test seam that forces the Telegram detach persistence step to fail for chosen sessions.
//! `arm` marks a session id; `take` consumes the mark, and the detach path in the parent
//! `telegram` module reports a persistence failure when it returns true.
//! It owns only a process-wide set of armed session ids and does no persistence itself.

use std::collections::HashSet;
use std::sync::Mutex;
use uuid::Uuid;

static FORCE_FAILURE: Mutex<Option<HashSet<Uuid>>> = Mutex::new(None);

pub(crate) fn arm(session_id: Uuid) {
    FORCE_FAILURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashSet::new)
        .insert(session_id);
}

pub(crate) fn take(session_id: Uuid) -> bool {
    FORCE_FAILURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .is_some_and(|armed| armed.remove(&session_id))
}
