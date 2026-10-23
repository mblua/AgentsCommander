//! Test-only rendezvous inside [`super::raise_reader_demand`].
//!
//! Armed for one session id; the raise signals `reached` and then awaits
//! `release`. Phase 4 test 19 uses it to destroy a session while the
//! detached create-time raise sits between eligibility resolution and
//! install.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct ReaderDemandBarrier {
    pub(crate) reached: Notify,
    pub(crate) release: Notify,
}

type BarrierMap = Mutex<Option<HashMap<String, Arc<ReaderDemandBarrier>>>>;

static BEFORE_INSTALL: BarrierMap = Mutex::new(None);

pub(crate) fn install_before_install(key: &str) -> Arc<ReaderDemandBarrier> {
    let barrier = Arc::new(ReaderDemandBarrier::default());
    BEFORE_INSTALL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(key.to_string(), Arc::clone(&barrier));
    barrier
}

pub(crate) async fn hit_before_install(key: &str) {
    let barrier = BEFORE_INSTALL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|map| map.remove(key));
    if let Some(barrier) = barrier {
        barrier.reached.notify_one();
        barrier.release.notified().await;
    }
}
