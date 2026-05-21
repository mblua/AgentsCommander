use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::session::profile::IdleTuning;

const CHECK_INTERVAL: Duration = Duration::from_millis(500);

type Callback = Arc<dyn Fn(Uuid) + Send + Sync>;

pub struct IdleDetector {
    activity: Arc<Mutex<HashMap<Uuid, Instant>>>,
    idle_set: Arc<Mutex<HashSet<Uuid>>>,
    resize_grace: Arc<Mutex<HashMap<Uuid, Instant>>>,
    /// Per-session idle tuning, populated by `register_session` at PTY spawn.
    /// A session missing here falls back to `IdleTuning::DEFAULT`.
    tuning: Arc<Mutex<HashMap<Uuid, IdleTuning>>>,
    on_idle: Callback,
    on_busy: Callback,
}

/// Pure: the sessions that should transition busy→idle on this watcher tick,
/// paired with how long they have been silent (for logging). No locks, no
/// callbacks — unit-testable.
///
/// A session is only a candidate if it is present in `activity`. #260's
/// `register_session` seed is what guarantees presence for an otherwise-silent
/// session whose PTY output was entirely suppressed or escape-only.
fn sessions_crossing_idle_threshold(
    now: Instant,
    activity: &HashMap<Uuid, Instant>,
    idle_set: &HashSet<Uuid>,
    tuning: &HashMap<Uuid, IdleTuning>,
) -> Vec<(Uuid, Duration)> {
    activity
        .iter()
        .filter_map(|(&id, &last_seen)| {
            let threshold = tuning
                .get(&id)
                .copied()
                .unwrap_or(IdleTuning::DEFAULT)
                .idle_threshold;
            // checked_duration_since avoids a panic if a PTY thread updated
            // last_seen between Instant::now() and the lock acquisition.
            let elapsed = now.checked_duration_since(last_seen)?;
            if elapsed > threshold && !idle_set.contains(&id) {
                Some((id, elapsed))
            } else {
                None
            }
        })
        .collect()
}

impl IdleDetector {
    pub fn new(
        on_idle: impl Fn(Uuid) + Send + Sync + 'static,
        on_busy: impl Fn(Uuid) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            activity: Arc::new(Mutex::new(HashMap::new())),
            idle_set: Arc::new(Mutex::new(HashSet::new())),
            resize_grace: Arc::new(Mutex::new(HashMap::new())),
            tuning: Arc::new(Mutex::new(HashMap::new())),
            on_idle: Arc::new(on_idle),
            on_busy: Arc::new(on_busy),
        })
    }

    /// Register a session with the detector at PTY spawn time. Stores the
    /// session's idle `tuning` and — when `tuning.seed_initial_activity` is
    /// set (#260) — seeds `activity[id] = now` so the watcher evaluates the
    /// session from t=0 even if no un-suppressed, printable PTY chunk ever
    /// arrives (the grinch stuck-session bug — see plan §1).
    pub fn register_session(&self, session_id: Uuid, tuning: IdleTuning) {
        debug_assert!(
            tuning.resize_grace >= tuning.idle_threshold,
            "resize_grace must be >= idle_threshold or a resize repaint can \
             trigger a false busy→idle transition"
        );
        self.tuning.lock().unwrap().insert(session_id, tuning);
        if tuning.seed_initial_activity {
            self.activity
                .lock()
                .unwrap()
                .insert(session_id, Instant::now());
            log::info!(
                "[idle] SEEDED activity for {} at spawn (idle_threshold={}ms)",
                &session_id.to_string()[..8],
                tuning.idle_threshold.as_millis()
            );
        }
    }

    /// Mark that a resize just happened for this session.
    /// PTY output within RESIZE_GRACE will be ignored (prompt repaint noise).
    pub fn record_resize(&self, session_id: Uuid) {
        log::info!(
            "[idle] RESIZE recorded for {}",
            &session_id.to_string()[..8]
        );
        self.resize_grace
            .lock()
            .unwrap()
            .insert(session_id, Instant::now());
    }

    /// Record PTY activity (with byte count for diagnostics).
    pub fn record_activity_with_bytes(&self, session_id: Uuid, byte_count: usize) {
        let sid = &session_id.to_string()[..8];
        // Per-session resize grace (#260) — copy out under a brief lock.
        let resize_grace = self
            .tuning
            .lock()
            .unwrap()
            .get(&session_id)
            .copied()
            .unwrap_or(IdleTuning::DEFAULT)
            .resize_grace;
        // Suppress activity caused by resize prompt repaint.
        if let Some(&last_resize) = self.resize_grace.lock().unwrap().get(&session_id) {
            let elapsed = last_resize.elapsed();
            if elapsed < resize_grace {
                log::info!(
                    "[idle] SUPPRESSED {} ({} bytes, {}ms after resize)",
                    sid,
                    byte_count,
                    elapsed.as_millis()
                );
                return;
            }
        }
        let was_idle = {
            // Hold both locks together so insert + remove is atomic
            // w.r.t. the watcher thread (same order: activity → idle_set).
            let mut activity = self.activity.lock().unwrap();
            let mut idle_set = self.idle_set.lock().unwrap();
            activity.insert(session_id, Instant::now());
            idle_set.remove(&session_id)
        };
        if was_idle {
            log::info!(
                "[idle] BUSY {} ({} bytes, was idle → now busy)",
                sid,
                byte_count
            );
            (self.on_busy)(session_id);
        }
    }

    /// Record PTY activity for a session (backwards-compatible wrapper).
    pub fn record_activity(&self, session_id: Uuid) {
        self.record_activity_with_bytes(session_id, 0);
    }

    /// Remove a session from tracking (called on session destroy).
    pub fn remove_session(&self, session_id: Uuid) {
        self.activity.lock().unwrap().remove(&session_id);
        self.idle_set.lock().unwrap().remove(&session_id);
        self.resize_grace.lock().unwrap().remove(&session_id);
        self.tuning.lock().unwrap().remove(&session_id);
    }

    /// Start the watcher thread that polls for idle transitions.
    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let detector = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(CHECK_INTERVAL);

                if shutdown.is_cancelled() {
                    log::info!("[IdleDetector] Shutdown signal received, stopping");
                    break;
                }

                let now = Instant::now();
                // Snapshot tuning (clone, lock released) so it is never held
                // across the activity/idle_set critical section. Lock order:
                // tuning → activity → idle_set (consistent everywhere).
                let tuning = detector.tuning.lock().unwrap().clone();
                let activity = detector.activity.lock().unwrap();
                let mut idle_set = detector.idle_set.lock().unwrap();

                let crossing =
                    sessions_crossing_idle_threshold(now, &activity, &idle_set, &tuning);
                for (session_id, elapsed) in crossing {
                    idle_set.insert(session_id);
                    log::info!(
                        "[idle] IDLE {} ({}ms since last activity)",
                        &session_id.to_string()[..8],
                        elapsed.as_millis()
                    );
                    // Callback inside the lock scope preserves delivery order:
                    // on_idle always fires before any on_busy for new activity.
                    (detector.on_idle)(session_id);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_session_seeds_activity_when_profile_opts_in() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT); // seed = true
        assert!(
            detector.activity.lock().unwrap().contains_key(&id),
            "register_session must seed activity[id] — the #260 fix"
        );
    }

    #[test]
    fn register_session_does_not_seed_when_opted_out() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(
            id,
            IdleTuning {
                seed_initial_activity: false,
                ..IdleTuning::DEFAULT
            },
        );
        assert!(!detector.activity.lock().unwrap().contains_key(&id));
        // ...but the tuning is still recorded.
        assert!(detector.tuning.lock().unwrap().contains_key(&id));
    }

    /// Acceptance criterion #1 — the grinch stuck-session regression test.
    /// A codex session whose entire visible output was suppressed (resize
    /// grace) / escape-only (SKIPPED), so `record_activity_with_bytes` NEVER
    /// ran. With the #260 seed it is still in `activity` and the watcher
    /// transitions it busy→idle after `idle_threshold`. Revert the seed in
    /// `register_session` and the `.expect(...)` below panics → this fails.
    #[test]
    fn seeded_silent_session_crosses_idle_threshold() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        let tuning = IdleTuning::DEFAULT;
        detector.register_session(id, tuning);

        let seeded_at = *detector
            .activity
            .lock()
            .unwrap()
            .get(&id)
            .expect("register_session must seed activity[id] — the #260 fix");

        let activity = detector.activity.lock().unwrap().clone();
        let idle_set: HashSet<Uuid> = HashSet::new();
        let mut tuning_map = HashMap::new();
        tuning_map.insert(id, tuning);

        // Before the threshold: no transition.
        let early = sessions_crossing_idle_threshold(
            seeded_at + tuning.idle_threshold - Duration::from_millis(100),
            &activity,
            &idle_set,
            &tuning_map,
        );
        assert!(early.is_empty(), "must not transition before idle_threshold");

        // After idle_threshold of pure silence: transition fires even though
        // record_activity_with_bytes was NEVER called for this session.
        let crossed = sessions_crossing_idle_threshold(
            seeded_at + tuning.idle_threshold + Duration::from_millis(100),
            &activity,
            &idle_set,
            &tuning_map,
        );
        let ids: Vec<Uuid> = crossed.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![id], "seeded silent session must reach idle");
    }

    /// Documents the bug mechanism: WITHOUT a seed, an all-suppressed session
    /// is absent from `activity` and the watcher never even evaluates it —
    /// no matter how much time passes.
    #[test]
    fn unseeded_session_never_transitions() {
        let id = Uuid::new_v4();
        let activity: HashMap<Uuid, Instant> = HashMap::new(); // never seeded
        let idle_set: HashSet<Uuid> = HashSet::new();
        let mut tuning_map = HashMap::new();
        tuning_map.insert(id, IdleTuning::DEFAULT);

        let crossed = sessions_crossing_idle_threshold(
            Instant::now() + Duration::from_secs(3600),
            &activity,
            &idle_set,
            &tuning_map,
        );
        assert!(
            crossed.is_empty(),
            "an un-seeded session is invisible to the watcher — the #260 bug"
        );
    }

    /// The resize-grace suppression that contributes to the bug must not
    /// clear the seed: output arriving inside the grace window is suppressed,
    /// but the seeded `activity[id]` survives so the watcher can still act.
    #[test]
    fn resize_grace_suppression_preserves_the_seed() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        detector.record_resize(id);
        // "Initial output" arrives inside RESIZE_GRACE → suppressed.
        detector.record_activity_with_bytes(id, 500);
        assert!(
            detector.activity.lock().unwrap().contains_key(&id),
            "the seed must survive resize-grace suppression"
        );
    }

    /// dev-rust R1.5 — guards the `tuning.remove` line §6.1 adds to
    /// `remove_session`; without it a future detector-map leak is silent.
    #[test]
    fn remove_session_clears_tuning() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        assert!(detector.tuning.lock().unwrap().contains_key(&id));
        detector.remove_session(id);
        assert!(
            !detector.tuning.lock().unwrap().contains_key(&id),
            "remove_session must drop the tuning entry"
        );
        assert!(!detector.activity.lock().unwrap().contains_key(&id));
    }
}
