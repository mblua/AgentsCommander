use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// A one-shot rendezvous: the create signals `reached` when it hits the barrier,
/// then awaits `release`. The controlling test awaits `reached`, interleaves its
/// deletion, and finally signals `release`.
#[derive(Default)]
pub(crate) struct SeedRaceBarrier {
    pub(crate) reached: Notify,
    pub(crate) release: Notify,
}

type BarrierMap = Mutex<Option<HashMap<String, Arc<SeedRaceBarrier>>>>;

static BEFORE_PROJECT_GATE: BarrierMap = Mutex::new(None);
static AFTER_SEED_BEFORE_PTY: BarrierMap = Mutex::new(None);

fn install(slot: &BarrierMap, key: &str) -> Arc<SeedRaceBarrier> {
    let barrier = Arc::new(SeedRaceBarrier::default());
    slot.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(key.to_string(), barrier.clone());
    barrier
}

async fn hit(slot: &BarrierMap, key: &str) {
    let barrier = slot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|map| map.remove(key));
    if let Some(barrier) = barrier {
        barrier.reached.notify_one();
        barrier.release.notified().await;
    }
}

/// Arm the `before_project_gate` barrier for the create whose normalized cwd is
/// `key`; the returned handle observes `reached` and drives `release`.
pub(crate) fn install_before_project_gate(key: &str) -> Arc<SeedRaceBarrier> {
    install(&BEFORE_PROJECT_GATE, key)
}

/// Arm the `after_seed_before_pty` barrier for the create whose normalized cwd is `key`.
pub(crate) fn install_after_seed_before_pty(key: &str) -> Arc<SeedRaceBarrier> {
    install(&AFTER_SEED_BEFORE_PTY, key)
}

pub(crate) async fn hit_before_project_gate(key: &str) {
    hit(&BEFORE_PROJECT_GATE, key).await;
}

pub(crate) async fn hit_after_seed_before_pty(key: &str) {
    hit(&AFTER_SEED_BEFORE_PTY, key).await;
}
