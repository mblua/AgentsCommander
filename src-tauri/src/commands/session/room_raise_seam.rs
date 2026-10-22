//! Test-only rendezvous inside [`super::raise_room_reader_demand_in`]
//! (#2516), modelled on `commands::telegram::reader_demand_seam`.
//!
//! Two independent hit points, each armed for one session id and consumed
//! on first hit: `before_readiness` sits after the demand-epoch sample and
//! before the readiness read, `before_add` sits in the `Ready` arm before
//! the guarded raise. Both are upstream of the raise's fast/slow split.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct RoomRaiseBarrier {
    pub(crate) reached: Notify,
    pub(crate) release: Notify,
}

type BarrierMap = Mutex<Option<HashMap<String, Arc<RoomRaiseBarrier>>>>;

static BEFORE_READINESS: BarrierMap = Mutex::new(None);
static BEFORE_ADD: BarrierMap = Mutex::new(None);

fn install(map: &BarrierMap, key: &str) -> Arc<RoomRaiseBarrier> {
    let barrier = Arc::new(RoomRaiseBarrier::default());
    map.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(key.to_string(), Arc::clone(&barrier));
    barrier
}

async fn hit(map: &BarrierMap, key: &str) {
    let barrier = map
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|map| map.remove(key));
    if let Some(barrier) = barrier {
        barrier.reached.notify_one();
        barrier.release.notified().await;
    }
}

pub(crate) fn install_before_readiness(key: &str) -> Arc<RoomRaiseBarrier> {
    install(&BEFORE_READINESS, key)
}

pub(crate) async fn hit_before_readiness(key: &str) {
    hit(&BEFORE_READINESS, key).await;
}

pub(crate) fn install_before_add(key: &str) -> Arc<RoomRaiseBarrier> {
    install(&BEFORE_ADD, key)
}

pub(crate) async fn hit_before_add(key: &str) {
    hit(&BEFORE_ADD, key).await;
}
