//! (#1399) Per-target lane reservation for detached wake delivery: per-target
//! exclusivity and a global active-operation cap, with RAII release.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 8 concurrent active injection/spawn deliveries, excluding post-T2 observation.
/// The cold-spawn portion is further serialised
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
/// cycle. Post-T2 observations keep target exclusivity without active capacity.
pub(crate) const WAKE_WORKER_LIMIT: usize = 8;

#[derive(Default)]
pub(crate) struct WakeLanes {
    active: Mutex<HashMap<String, WakeLaneState>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WakeLaneState {
    Active,
    Observing,
}

pub(crate) struct WakeLaneReservation {
    owner: Arc<WakeLanes>,
    target: String,
}

impl WakeLanes {
    /// One call gives per-target uniqueness and the active cap.
    /// Release is RAII via the reservation's `Drop`.
    /// Lock poisoning is absorbed so a panic holding the lock cannot wedge the
    /// set.
    pub(crate) fn try_reserve(self: &Arc<Self>, target: &str) -> Option<WakeLaneReservation> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.contains_key(target)
            || active
                .values()
                .filter(|state| **state == WakeLaneState::Active)
                .count()
                >= WAKE_WORKER_LIMIT
        {
            return None;
        }
        active.insert(target.to_string(), WakeLaneState::Active);
        Some(WakeLaneReservation {
            owner: Arc::clone(self),
            target: target.to_string(),
        })
    }

    pub(crate) fn observing_count(&self) -> usize {
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|state| **state == WakeLaneState::Observing)
            .count()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

impl WakeLaneReservation {
    #[cfg_attr(not(test), expect(dead_code, reason = "P04 activates the T2 callback"))]
    pub(crate) fn begin_observation(&self) {
        let mut active = self
            .owner
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(state) = active.get_mut(&self.target) {
            *state = WakeLaneState::Observing;
        }
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
mod tests;
