use std::collections::HashSet;
use std::sync::Mutex;
use uuid::Uuid;

static FORCE_FAILURE: Mutex<Option<HashSet<Uuid>>> = Mutex::new(None);

pub(crate) fn arm(session_id: Uuid) {
    FORCE_FAILURE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(session_id);
}

pub(crate) fn take(session_id: Uuid) -> bool {
    FORCE_FAILURE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .is_some_and(|armed| armed.remove(&session_id))
}
