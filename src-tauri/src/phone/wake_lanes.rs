//! (#1399) Per-target lane reservation for detached wake delivery. Same shape as
//! `api::dispatcher::PtyWorkerTargets`, which has held this invariant for
//! per-target PTY work in production.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// 8 concurrent deliveries; the cold-spawn portion is further serialised
/// downstream by the per-target create gate.
///
/// Rule that picks the value: it matches `api::dispatcher::PTY_WORKER_LIMIT`,
/// which the daemon already tolerates for concurrent per-target PTY work, and 8
/// covers the largest batch actually observed (7 messages inside 61 ms, #1394)
/// in one slot width. Uncapped, a coordinator fan-out over the 115 replicas
/// discovered in one live instance would attempt 115 simultaneous cold spawns.
///
/// This is a separate constant from the dispatcher's because the two pools are
/// independent, so the combined ceiling is 16 concurrent PTY operations. That is
/// deliberately NOT stated as a claim about machine load: cold spawns are
/// already bounded below this cap by `AgentUpdateGate::wait_until_done`, the
/// per-target create gate's `acquire_target_lock` / `acquire_exact` and
/// `SelectionCoordinator::reserve_create` (all in `commands/session.rs`) - all
/// per-target keys, acquired in a uniform order, so this cap adds no lock
/// cycle. What 8 bounds is the number of deliveries in flight, not the number
/// of processes starting.
pub(crate) const WAKE_WORKER_LIMIT: usize = 8;

#[derive(Default)]
pub(crate) struct WakeLanes {
    active: Mutex<HashSet<String>>,
}

pub(crate) struct WakeLaneReservation {
    owner: Arc<WakeLanes>,
    target: String,
}

impl WakeLanes {
    /// One call gives both invariants: per-target uniqueness (the `insert`
    /// test) and the global cap. Release is RAII via the reservation's `Drop`.
    /// Lock poisoning is absorbed so a panic holding the lock cannot wedge the
    /// set.
    pub(crate) fn try_reserve(
        self: &Arc<Self>,
        target: &str,
    ) -> Option<WakeLaneReservation> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.len() >= WAKE_WORKER_LIMIT || !active.insert(target.to_string()) {
            return None;
        }
        Some(WakeLaneReservation {
            owner: Arc::clone(self),
            target: target.to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

impl Drop for WakeLaneReservation {
    fn drop(&mut self) {
        self.owner
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (#1399 T1) A second reservation for the same target is refused while the
    /// first is alive; dropping the first releases the lane for the same target.
    #[test]
    fn same_target_is_exclusive_until_the_reservation_drops() {
        let lanes = Arc::new(WakeLanes::default());
        let first = lanes.try_reserve("proj:wg-1/dev-rust");
        assert!(first.is_some());
        assert!(lanes.try_reserve("proj:wg-1/dev-rust").is_none());
        drop(first);
        assert_eq!(lanes.len(), 0);
        assert!(lanes.try_reserve("proj:wg-1/dev-rust").is_some());
    }

    /// (#1399 T1) A ninth distinct target is refused at `limit = 8` and
    /// succeeds after one release.
    #[test]
    fn ninth_distinct_target_is_refused_until_a_release() {
        let lanes = Arc::new(WakeLanes::default());
        let mut held: Vec<WakeLaneReservation> = (0..WAKE_WORKER_LIMIT)
            .map(|slot| {
                lanes
                    .try_reserve(&format!("target-{slot}"))
                    .expect("distinct target under the cap must reserve")
            })
            .collect();
        assert!(lanes.try_reserve("target-8").is_none());
        held.pop();
        let ninth = lanes.try_reserve("target-8");
        assert!(ninth.is_some());
        drop(ninth);
        drop(held);
        assert_eq!(lanes.len(), 0);
    }
}
