use super::{SNAPSHOT_LIMITER_KEY_CAP, SNAPSHOT_RATE_WINDOW, SNAPSHOT_TARGET_RATE};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use terminal_snapshot_renderer::TerminalSnapshotReasonCode;

#[derive(Default)]
pub(crate) struct RollingState {
    pub(crate) ingress: HashMap<String, VecDeque<Instant>>,
    pub(crate) requester: HashMap<String, VecDeque<Instant>>,
    pub(crate) target: HashMap<String, VecDeque<Instant>>,
    pub(crate) requester_in_flight: HashMap<String, usize>,
    pub(crate) target_in_flight: HashMap<String, usize>,
    pub(crate) global_in_flight: usize,
}

pub(crate) struct LimiterLeaseInner {
    pub(crate) limiter: Arc<Mutex<RollingState>>,
    pub(crate) requester_key: String,
    pub(crate) target_key: Mutex<Option<String>>,
}

impl Drop for LimiterLeaseInner {
    fn drop(&mut self) {
        let Ok(mut limiter) = self.limiter.lock() else {
            return;
        };
        decrement_counter(&mut limiter.requester_in_flight, &self.requester_key);
        if let Ok(target_key) = self.target_key.lock() {
            if let Some(target_key) = target_key.as_ref() {
                decrement_counter(&mut limiter.target_in_flight, target_key);
            }
        }
        limiter.global_in_flight = limiter.global_in_flight.saturating_sub(1);
    }
}

#[derive(Clone)]
pub(crate) struct RequesterSnapshotPermit {
    pub(crate) inner: Arc<LimiterLeaseInner>,
}

impl RequesterSnapshotPermit {
    pub(super) fn promote_target(&self, key: String) -> Result<(), TerminalSnapshotReasonCode> {
        let now = Instant::now();
        let mut target_slot = self
            .inner
            .target_key
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        if target_slot.is_some() {
            return Err(TerminalSnapshotReasonCode::Internal);
        }
        let mut limiter = self
            .inner
            .limiter
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        prune_map(&mut limiter.target, now);
        if !limiter.target.contains_key(&key) && limiter.target.len() >= SNAPSHOT_LIMITER_KEY_CAP {
            return Err(TerminalSnapshotReasonCode::RateLimited);
        }
        let in_flight = limiter.target_in_flight.get(&key).copied().unwrap_or(0);
        let attempts = limiter.target.get(&key).map(VecDeque::len).unwrap_or(0);
        if attempts >= SNAPSHOT_TARGET_RATE || in_flight >= 1 {
            return Err(TerminalSnapshotReasonCode::RateLimited);
        }
        limiter
            .target
            .entry(key.clone())
            .or_default()
            .push_back(now);
        limiter.target_in_flight.insert(key.clone(), 1);
        *target_slot = Some(key);
        Ok(())
    }
}

pub(super) fn decrement_counter<K: Eq + std::hash::Hash + Clone>(
    map: &mut HashMap<K, usize>,
    key: &K,
) {
    if let Some(value) = map.get_mut(key) {
        *value = value.saturating_sub(1);
        if *value == 0 {
            map.remove(key);
        }
    }
}

pub(super) fn prune_window(window: &mut VecDeque<Instant>, now: Instant) {
    while window
        .front()
        .is_some_and(|accepted| now.saturating_duration_since(*accepted) >= SNAPSHOT_RATE_WINDOW)
    {
        window.pop_front();
    }
}

pub(super) fn prune_map(map: &mut HashMap<String, VecDeque<Instant>>, now: Instant) {
    map.retain(|_, window| {
        prune_window(window, now);
        !window.is_empty()
    });
}
