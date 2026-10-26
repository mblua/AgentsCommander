use std::collections::HashSet;
use std::sync::Mutex;

/// (#617/#668) Sessions with a self context operation awaiting their sustained-idle
/// window. Insert on queue; remove when the deferred task completes, the session
/// dies, or the safety cap expires. A session_id already present means a repeat
/// self operation is a no-op ("already_queued") - requests never stack.
/// In-memory only: a daemon restart drops pending requests (accepted, best-effort).
///
/// Newtype is mandatory: `DetachedSessionsState` (lib.rs:38) is already a managed
/// bare `Arc<Mutex<HashSet<Uuid>>>`, and Tauri keys managed state by Rust type, so
/// a second bare alias would collide. Mirrors `RestoreInProgress`.
#[derive(Default)]
pub struct PendingSelfClear(pub Mutex<HashSet<uuid::Uuid>>);

#[cfg(test)]
impl PendingSelfClear {
    pub(crate) fn seed_legacy(&self, sid: uuid::Uuid) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(sid)
    }

    pub(crate) fn contains(&self, sid: uuid::Uuid) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&sid)
    }

    pub(crate) fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    pub(crate) fn remove_legacy_fixture(&self, sid: uuid::Uuid) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&sid)
    }
}
