use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::session::profile::IdleTuning;

const CHECK_INTERVAL: Duration = Duration::from_millis(500);

type Callback = Arc<dyn Fn(Uuid) + Send + Sync>;

pub struct IdleDetector {
    activity: Arc<Mutex<HashMap<Uuid, Instant>>>,
    /// #552 auto-close silence clock. SEPARATE from `activity` (which drives the
    /// #260 idle dot). Reset by ANY PTY output (printable OR escape-only), user
    /// input, and inter-agent delivery. Never read by the idle-dot watcher, so
    /// it cannot mask #260 idle detection.
    silence: Arc<Mutex<HashMap<Uuid, Instant>>>,
    /// (#580) Per-session "alive since" clock: the `Instant` the PTY was
    /// registered (spawned). Mirrors `silence` (seeded in `register_session`,
    /// cleared in `remove_session`). Read via `alive_age` so auto-close can apply
    /// WAKE_GRACE: a freshly-woken session is neither kill-eligible nor allowed to
    /// advance the persisted idle anchor until it is alive past the grace.
    registered_at: Arc<Mutex<HashMap<Uuid, Instant>>>,
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

/// (#885) One peer's purge-readiness inputs, sampled at a single instant.
#[derive(Debug, Clone, Copy)]
pub struct PurgeReadiness {
    pub session_id: Uuid,
    /// Age of the last PRINTABLE output. `None` when the session is untracked.
    pub activity_age: Option<Duration>,
    /// The watcher has already crossed this session into the idle set.
    pub watcher_idle: bool,
    /// (#885 F-1) Age of the last `record_resize`, or `None` if never resized.
    /// `activity_age` is FROZEN and untrustworthy for `resize_grace` after this
    /// instant: `record_activity_with_bytes` early-returns without touching
    /// `activity` inside the grace window (`:150-161`).
    pub last_resize_age: Option<Duration>,
    /// This session's resolved `resize_grace` and `idle_threshold`, so the
    /// caller's gate is per-session rather than against a global constant.
    pub resize_grace: Duration,
    pub idle_threshold: Duration,
    /// Age of the last output of any kind, printable or escape-only.
    /// DIAGNOSTIC ONLY. Never gate on this: `pty/output.rs:127` resets it for
    /// escape-only chunks, so a repainting TUI would keep it permanently fresh
    /// and the gate could never pass.
    pub silence_age: Option<Duration>,
}

impl IdleDetector {
    pub fn new(
        on_idle: impl Fn(Uuid) + Send + Sync + 'static,
        on_busy: impl Fn(Uuid) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            activity: Arc::new(Mutex::new(HashMap::new())),
            silence: Arc::new(Mutex::new(HashMap::new())),
            registered_at: Arc::new(Mutex::new(HashMap::new())),
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
            log::debug!(
                "[idle] SEEDED activity for {} at spawn (idle_threshold={}ms)",
                &session_id.to_string()[..8],
                tuning.idle_threshold.as_millis()
            );
        }
        // #552 always seed the silence clock so a freshly spawned session has a
        // baseline age (the auto-close evaluator treats untracked-but-live as a
        // spawn race; see auto_close.rs). Independent of the #260 activity seed
        // above, which is gated on `seed_initial_activity`.
        self.silence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id, Instant::now());
        // (#580) seed the alive-since clock alongside silence so auto-close can
        // apply WAKE_GRACE (kill-eligibility / anchor-advance gating) from spawn.
        self.registered_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id, Instant::now());
    }

    /// Mark that a resize just happened for this session.
    /// PTY output within RESIZE_GRACE will be ignored (prompt repaint noise).
    pub fn record_resize(&self, session_id: Uuid) {
        log::debug!(
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
                log::trace!(
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
            log::debug!(
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

    /// #552 Reset the auto-close silence clock for a session. Does NOT touch
    /// `activity`/`idle_set`/`on_busy`, so it never affects the #260 idle dot.
    /// Called for ANY PTY output, user input, and inter-agent delivery.
    pub fn touch_silence(&self, session_id: Uuid) {
        self.silence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id, Instant::now());
    }

    #[cfg(test)]
    pub(crate) fn set_auto_close_ages_for_test(
        &self,
        session_id: Uuid,
        silence_age: Duration,
        alive_age: Duration,
    ) {
        let now = Instant::now();
        self.silence
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(session_id, now.checked_sub(silence_age).unwrap_or(now));
        self.registered_at
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(session_id, now.checked_sub(alive_age).unwrap_or(now));
    }

    /// #552 Time since last silence-reset for a session, or None if untracked.
    /// Read by the auto-close evaluator.
    pub fn silence_age(&self, session_id: Uuid) -> Option<Duration> {
        let now = Instant::now();
        self.silence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&session_id)
            .and_then(|&t| now.checked_duration_since(t))
    }

    /// (#1001 PR1 / grinch G7) True iff this session's last PRINTABLE-output
    /// instant is strictly after `t`. Compares the stored `activity` stamp to
    /// `t` directly under a single lock, with NO synthesized `now`, so it avoids
    /// the false-positive skew of a `now - activity_age` round-trip (a driver
    /// `now` later than the accessor `now` would inflate the reconstructed
    /// instant and read a stale echo as fresh). This is the timestamp-gate
    /// candidate signal the wake-consumption harness evaluates; `activity_age`
    /// (for B) is intentionally NOT reused here.
    ///
    /// CAVEAT (grinch G8): `activity` is frozen during `resize_grace`
    /// (`record_activity_with_bytes` early-returns without stamping), so a
    /// post-resize repaint does NOT count as activity here; callers needing
    /// resize-awareness must consult `last_resize_age` (via `purge_readiness`).
    pub fn has_printable_activity_since(&self, session_id: Uuid, t: Instant) -> bool {
        self.activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&session_id)
            .is_some_and(|&stamp| stamp > t)
    }

    /// (#885) Sample readiness for `ids` under a SINGLE critical section.
    ///
    /// A PTY reader thread must take `activity` then `idle_set` to record activity
    /// (`record_activity_with_bytes`), so holding both here yields a view no reader
    /// thread can interleave. That is the strongest consistency available for the
    /// purge busy-gate.
    ///
    /// LOCK ORDER. `tuning` and `resize_grace` are cloned under their own locks
    /// and released before `activity` is taken, exactly as the watcher does with
    /// `tuning` (`start`, `:251`). This introduces ZERO new nesting: the only
    /// nested acquisition anywhere in this file is `activity -> idle_set`
    /// (`:165-166`, `:252-253`), and `silence` is only ever taken alone
    /// (`:112`, `:189`, `:199`, `:222`), so appending it at the tail cannot
    /// deadlock.
    ///
    /// Reading `resize_grace` before `activity` can only MISS a resize newer than
    /// the `activity` value we read. Such a resize begins its freeze after that
    /// value was written, so it cannot have corrupted it. The skew is safe.
    pub fn purge_readiness(&self, ids: &[Uuid]) -> Vec<PurgeReadiness> {
        // Phase 1: clone the small maps, each under its own lock, then release.
        let tuning = self.tuning.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let resizes = self
            .resize_grace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        // Phase 2: the consistent snapshot. Nesting matches the reader thread's.
        let now = Instant::now();
        let activity = self.activity.lock().unwrap_or_else(|e| e.into_inner());
        let idle_set = self.idle_set.lock().unwrap_or_else(|e| e.into_inner());
        let silence = self.silence.lock().unwrap_or_else(|e| e.into_inner());

        ids.iter()
            .map(|id| {
                let t = tuning.get(id).copied().unwrap_or(IdleTuning::DEFAULT);
                PurgeReadiness {
                    session_id: *id,
                    activity_age: activity.get(id).and_then(|&x| now.checked_duration_since(x)),
                    watcher_idle: idle_set.contains(id),
                    last_resize_age: resizes
                        .get(id)
                        .and_then(|&x| now.checked_duration_since(x)),
                    resize_grace: t.resize_grace,
                    idle_threshold: t.idle_threshold,
                    silence_age: silence.get(id).and_then(|&x| now.checked_duration_since(x)),
                }
            })
            .collect()
    }

    /// (#580) Time since this session's PTY was registered (spawned), or None if
    /// untracked. Used by auto-close to apply WAKE_GRACE: a freshly-woken session
    /// is neither kill-eligible nor allowed to advance the persisted idle anchor
    /// until alive past the grace (so wake-time scrollback repaint cannot reset it).
    pub fn alive_age(&self, session_id: Uuid) -> Option<Duration> {
        let now = Instant::now();
        self.registered_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&session_id)
            .and_then(|&t| now.checked_duration_since(t))
    }

    /// Remove a session from tracking (called on session destroy).
    pub fn remove_session(&self, session_id: Uuid) {
        self.activity.lock().unwrap().remove(&session_id);
        self.silence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id);
        self.registered_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id);
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

                let crossing = sessions_crossing_idle_threshold(now, &activity, &idle_set, &tuning);
                for (session_id, elapsed) in crossing {
                    idle_set.insert(session_id);
                    log::debug!(
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
        assert!(
            early.is_empty(),
            "must not transition before idle_threshold"
        );

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

    /// #552: touch_silence updates only the `silence` map; it must NOT insert
    /// into `activity`/`idle_set` or fire on_busy, or it would couple the
    /// auto-close clock to the #260 idle dot.
    #[test]
    fn touch_silence_is_decoupled_from_idle_dot() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let busy_calls = Arc::new(AtomicUsize::new(0));
        let busy_calls_cb = Arc::clone(&busy_calls);
        let detector = IdleDetector::new(move |_| {}, move |_| {
            busy_calls_cb.fetch_add(1, Ordering::SeqCst);
        });
        let id = Uuid::new_v4();

        detector.touch_silence(id);

        assert!(
            detector.silence.lock().unwrap().contains_key(&id),
            "touch_silence must populate the silence map"
        );
        assert!(
            !detector.activity.lock().unwrap().contains_key(&id),
            "touch_silence must NOT touch activity (the #260 idle-dot signal)"
        );
        assert!(
            !detector.idle_set.lock().unwrap().contains(&id),
            "touch_silence must NOT touch idle_set"
        );
        assert_eq!(
            busy_calls.load(Ordering::SeqCst),
            0,
            "touch_silence must NOT fire on_busy"
        );
    }

    /// #552: silence_age grows over time and is None for an untracked id.
    #[test]
    fn silence_age_increases_and_is_none_when_untracked() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();

        assert!(
            detector.silence_age(id).is_none(),
            "an untracked session has no silence age"
        );

        detector.touch_silence(id);
        let first = detector.silence_age(id).expect("tracked now");
        std::thread::sleep(Duration::from_millis(10));
        let second = detector.silence_age(id).expect("still tracked");
        assert!(
            second >= first,
            "silence age must be monotonic non-decreasing ({second:?} >= {first:?})"
        );
    }

    /// #552: the silence seed in register_session is UNCONDITIONAL, unlike the
    /// activity seed which is gated on `seed_initial_activity`. A live member
    /// with no silence baseline would be treated as a spawn race and protected,
    /// so the seed must fire even when the profile opts out of the activity seed.
    #[test]
    fn register_session_seeds_silence_even_when_activity_seed_disabled() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(
            id,
            IdleTuning {
                seed_initial_activity: false,
                ..IdleTuning::DEFAULT
            },
        );
        assert!(
            !detector.activity.lock().unwrap().contains_key(&id),
            "activity seed is gated off here"
        );
        assert!(
            detector.silence.lock().unwrap().contains_key(&id),
            "silence seed must be unconditional (#552)"
        );
        assert!(
            detector.silence_age(id).is_some(),
            "a registered session has a silence baseline age"
        );
    }

    /// #552: remove_session must clear the silence entry (no leak on destroy).
    #[test]
    fn remove_session_clears_silence() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        assert!(detector.silence.lock().unwrap().contains_key(&id));
        detector.remove_session(id);
        assert!(
            !detector.silence.lock().unwrap().contains_key(&id),
            "remove_session must drop the silence entry"
        );
    }

    /// (#580): register_session seeds registered_at; alive_age is Some after
    /// register and None after remove_session (the WAKE_GRACE clock lifecycle).
    #[test]
    fn register_session_seeds_registered_at_and_alive_age() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();

        assert!(
            detector.alive_age(id).is_none(),
            "an unregistered session has no alive age"
        );

        detector.register_session(id, IdleTuning::DEFAULT);
        assert!(
            detector.registered_at.lock().unwrap().contains_key(&id),
            "register_session must seed registered_at (#580)"
        );
        assert!(
            detector.alive_age(id).is_some(),
            "a registered session has an alive age"
        );

        detector.remove_session(id);
        assert!(
            !detector.registered_at.lock().unwrap().contains_key(&id),
            "remove_session must drop the registered_at entry"
        );
        assert!(
            detector.alive_age(id).is_none(),
            "alive_age is None after remove_session"
        );
    }

    /// (#580): alive_age grows monotonically (non-decreasing).
    #[test]
    fn alive_age_is_monotonic() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);

        let first = detector.alive_age(id).expect("tracked after register");
        std::thread::sleep(Duration::from_millis(10));
        let second = detector.alive_age(id).expect("still tracked");
        assert!(
            second >= first,
            "alive age must be monotonic non-decreasing ({second:?} >= {first:?})"
        );
    }

    // ── (#885) purge_readiness tests ──

    #[test]
    fn purge_readiness_snapshot_is_consistent() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        detector.record_activity_with_bytes(id, 42);

        let readiness = detector.purge_readiness(&[id]);
        assert_eq!(readiness.len(), 1);
        let r = &readiness[0];
        assert_eq!(r.session_id, id);
        assert!(
            r.activity_age.is_some(),
            "activity_age must be Some after record_activity"
        );
        assert!(
            r.activity_age.unwrap() < Duration::from_secs(1),
            "activity_age must be small right after record_activity"
        );
        assert!(
            !r.watcher_idle,
            "watcher_idle must be false right after activity"
        );
    }

    #[test]
    fn purge_readiness_untracked_is_none() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let untracked = Uuid::new_v4();

        let readiness = detector.purge_readiness(&[untracked]);
        assert_eq!(readiness.len(), 1);
        assert!(
            readiness[0].activity_age.is_none(),
            "activity_age must be None for an unregistered session"
        );
    }

    #[test]
    fn has_printable_activity_since_compares_stamp_to_reference() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);

        // A reference that predates the recorded stamp: activity-since is true.
        let before = Instant::now() - Duration::from_millis(50);
        detector.record_activity_with_bytes(id, 5);
        assert!(
            detector.has_printable_activity_since(id, before),
            "a recorded stamp is strictly after an earlier reference"
        );

        // A reference strictly after the last stamp: no newer activity.
        let after = Instant::now() + Duration::from_millis(50);
        assert!(
            !detector.has_printable_activity_since(id, after),
            "no activity after a future reference"
        );

        // Untracked session: never any activity.
        assert!(!detector.has_printable_activity_since(Uuid::new_v4(), before));
    }

    #[test]
    fn purge_readiness_reports_resize_age() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        detector.record_resize(id);

        let readiness = detector.purge_readiness(&[id]);
        let r = &readiness[0];
        assert!(
            r.last_resize_age.is_some(),
            "last_resize_age must be Some after record_resize"
        );
        assert!(
            r.last_resize_age.unwrap() < Duration::from_secs(1),
            "last_resize_age must be small right after record_resize"
        );
        assert_eq!(
            r.resize_grace,
            IdleTuning::DEFAULT.resize_grace,
            "resize_grace must be the session's tuning value"
        );
    }

    /// (#885 F-1) The core bug: inside resize grace, `record_activity_with_bytes`
    /// early-returns WITHOUT touching `activity`, so `activity_age` grows
    /// without bound while the child is "printing". This test pins the bug so
    /// nobody deletes the fourth gate leg.
    #[test]
    fn resize_grace_freezes_activity_age() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);

        // Record initial activity.
        detector.record_activity_with_bytes(id, 10);
        // Enter resize grace.
        detector.record_resize(id);
        // Sleep so a refresh would be detectable: if the early-return were
        // removed, the next record_activity_with_bytes would reset activity
        // to ~0ms, making age << 200ms.
        std::thread::sleep(Duration::from_millis(200));
        // Simulate the child printing during grace.
        detector.record_activity_with_bytes(id, 99);

        // activity must NOT have moved: the early-return at :159 suppressed it.
        // An un-suppressed refresh would reset age to ~0, so age would be < 200ms.
        let age = detector.purge_readiness(&[id])[0].activity_age.unwrap();
        assert!(
            age >= Duration::from_millis(200),
            "activity must be FROZEN during resize grace; an un-suppressed \
             refresh would reset age to ~0. age={age:?}. \
             If this fails, record_activity_with_bytes is no longer \
             early-returning during grace, and the F-1 fourth gate leg \
             may be unnecessary."
        );
    }
}
