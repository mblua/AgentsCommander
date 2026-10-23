use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct RestartReaderBarrier {
    pub(crate) reached: Notify,
    pub(crate) release: Notify,
}

type BarrierMap = Mutex<Option<HashMap<String, Arc<RestartReaderBarrier>>>>;

static AFTER_OLD_RELEASE: BarrierMap = Mutex::new(None);

/// Arm the barrier for the restart of `key` (the old session uuid).
pub(crate) fn install_after_old_release(key: &str) -> Arc<RestartReaderBarrier> {
    let barrier = Arc::new(RestartReaderBarrier::default());
    AFTER_OLD_RELEASE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(key.to_string(), Arc::clone(&barrier));
    barrier
}

pub(crate) async fn hit_after_old_release(key: &str) {
    let barrier = AFTER_OLD_RELEASE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|map| map.remove(key));
    if let Some(barrier) = barrier {
        barrier.reached.notify_one();
        barrier.release.notified().await;
    }
}
