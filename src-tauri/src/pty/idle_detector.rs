use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::session::profile::{ActivityDetectorKind, IdleTuning};

const CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// #1149 - watcher ticks between `app_alive` heartbeats. At `CHECK_INTERVAL`
/// that is one record per minute, which is also the worst-case granularity of
/// the backstop that closes intervals no mutation site ever closes: a session
/// that exits or is removed while working simply stops being listed.
const HEARTBEAT_TICKS: u64 = 120;

type Callback = Arc<dyn Fn(Uuid) + Send + Sync>;
type RawOutputObserver = Arc<dyn Fn(Uuid, Instant) + Send + Sync>;

/// #2124 - one session's unconfirmed output burst: when it opened, when its
/// last chunk arrived, how many bytes it accumulated (printable or not) and
/// whether any of them was printable.
#[derive(Debug, Clone, Copy)]
struct PendingBurst {
    start: Instant,
    last: Instant,
    bytes: u64,
    printable: bool,
}

/// #2879 - one session's semantic activity registration. Its PRESENCE is the active
/// registration; `observation` is the single token of the current consumer attach, and
/// `last_valid` the last valid frame verdict that attach delivered.
struct SemanticRegistration {
    kind: ActivityDetectorKind,
    observation: Option<Arc<()>>,
    last_valid: Option<bool>,
}

/// #2879 - everything guarded by the `activity` mutex: the #260 printable-output clock and
/// the semantic registrations, so an observation and the watcher's commit are linearized by
/// the one lock the arbiter already takes. Derefs to the clock map, which every pre-#2879
/// reader of `activity` keeps using unchanged.
#[derive(Default)]
struct ActivityState {
    timestamps: HashMap<Uuid, Instant>,
    semantic: HashMap<Uuid, SemanticRegistration>,
}

impl std::ops::Deref for ActivityState {
    type Target = HashMap<Uuid, Instant>;

    fn deref(&self) -> &Self::Target {
        &self.timestamps
    }
}

impl std::ops::DerefMut for ActivityState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.timestamps
    }
}

impl ActivityState {
    /// The registration `token` was attached to, if it is still the current CodexWorking
    /// attach. The single identity check `observe` and `retire` both use.
    fn current_attach(&mut self, id: Uuid, token: &Arc<()>) -> Option<&mut SemanticRegistration> {
        self.semantic.get_mut(&id).filter(|registration| {
            registration.kind == ActivityDetectorKind::CodexWorking
                && registration
                    .observation
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, token))
        })
    }
}

/// #2879 - the per-kind arbitration between the timed clock and semantic evidence.
trait ActivityDetector {
    fn is_busy(&self, timed_busy: bool, positive: bool) -> bool;
}

struct TimedDetector;
struct CodexWorkingDetector;

impl ActivityDetector for TimedDetector {
    fn is_busy(&self, timed_busy: bool, _positive: bool) -> bool {
        timed_busy
    }
}

impl ActivityDetector for CodexWorkingDetector {
    fn is_busy(&self, timed_busy: bool, positive: bool) -> bool {
        positive || timed_busy
    }
}

fn activity_detector(kind: ActivityDetectorKind) -> &'static dyn ActivityDetector {
    match kind {
        ActivityDetectorKind::Timed => &TimedDetector,
        ActivityDetectorKind::CodexWorking => &CodexWorkingDetector,
    }
}

/// #2879 - one consumer attach. `observe(Some(b))` records a valid frame verdict,
/// `observe(None)` only validates; `retire()` releases this attach's evidence. Each returns
/// whether it was accepted, i.e. whether this attach is still the current one.
pub(crate) struct ActivityConsumerHandle {
    pub(crate) observe: Arc<dyn Fn(Option<bool>) -> bool + Send + Sync>,
    pub(crate) retire: Arc<dyn Fn() -> bool + Send + Sync>,
}

pub struct IdleDetector {
    raw_output_observer: std::sync::OnceLock<(usize, RawOutputObserver)>,
    activity: Arc<Mutex<ActivityState>>,
    /// #552 auto-close silence clock. SEPARATE from `activity` (which drives the
    /// #260 idle dot). Reset by user input, inter-agent delivery and PTY output
    /// that the #2124 burst filter counts; a short burst after long silence is
    /// ignored instead of resetting it. Never read by the idle-dot watcher, so
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
    /// #1682 - per-session instant of the last PTY write that was NOT a delivered message and
    /// that either FOUND this session idle, which is "re-opened its busy period" everywhere
    /// except inside an active `resize_grace`, or arrived while a mark already stood. It is
    /// NOT an exact "the current busy period is attributable to this write": R4 and R6 in
    /// phase 03's D1 are the two ways it is not. Set and refreshed by
    /// `record_control_write_activity`, cleared by `clear_control_write` at every ARMING site
    /// and by `remove_session` on teardown, read by `control_write_age`. NEVER read by the
    /// watcher, by `record_activity_with_bytes`, by `purge_readiness` or by auto-close, so it
    /// cannot affect the #260 idle dot, the #552 silence clock or #885 readiness.
    control_write: Arc<Mutex<HashMap<Uuid, Instant>>>,
    /// Per-session idle tuning, populated by `register_session` at PTY spawn.
    /// A session missing here falls back to `IdleTuning::DEFAULT`.
    tuning: Arc<Mutex<HashMap<Uuid, IdleTuning>>>,
    /// #2124 - pending short-burst candidates (see `record_output_at`): one
    /// entry per session whose latest output chunk opened a burst that is not
    /// yet confirmed as real work. Taken alone, never nested inside another
    /// detector lock, and always released before `touch_silence` or
    /// `record_activity_with_bytes` runs.
    burst: Arc<Mutex<HashMap<Uuid, PendingBurst>>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PtyInputBoundaryFailure {
    Busy,
    ResizeUnsettled,
    UntrackedReadiness,
    RouteUnavailable,
}

impl IdleDetector {
    pub(crate) fn bind_raw_output_observer(
        &self,
        identity: usize,
        observer: impl Fn(Uuid, Instant) + Send + Sync + 'static,
    ) -> bool {
        self.raw_output_observer
            .set((identity, Arc::new(observer)))
            .is_ok()
    }
    pub(crate) fn raw_output_observer_identity(&self) -> Option<usize> {
        self.raw_output_observer
            .get()
            .map(|(identity, _)| *identity)
    }
    pub fn new(
        on_idle: impl Fn(Uuid) + Send + Sync + 'static,
        on_busy: impl Fn(Uuid) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            raw_output_observer: std::sync::OnceLock::new(),
            activity: Arc::new(Mutex::new(ActivityState::default())),
            silence: Arc::new(Mutex::new(HashMap::new())),
            registered_at: Arc::new(Mutex::new(HashMap::new())),
            idle_set: Arc::new(Mutex::new(HashSet::new())),
            resize_grace: Arc::new(Mutex::new(HashMap::new())),
            control_write: Arc::new(Mutex::new(HashMap::new())),
            tuning: Arc::new(Mutex::new(HashMap::new())),
            burst: Arc::new(Mutex::new(HashMap::new())),
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
        let mut activity = self.activity.lock().unwrap();
        // #2879 - always a FRESH registration: a re-registration drops any evidence and
        // invalidates every earlier consumer attach, whatever the seed below decides.
        activity.semantic.insert(
            session_id,
            SemanticRegistration {
                kind: tuning.activity_detector,
                observation: None,
                last_valid: None,
            },
        );
        if tuning.seed_initial_activity {
            activity.insert(session_id, Instant::now());
            log::debug!(
                "[idle] SEEDED activity for {} at spawn (idle_threshold={}ms)",
                &session_id.to_string()[..8],
                tuning.idle_threshold.as_millis()
            );
        }
        drop(activity);
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

    /// #2124 - record one PTY output chunk (printable or escape-only),
    /// applying the session's idle-burst filter. See `record_output_at`.
    pub fn record_output(&self, session_id: Uuid, byte_count: usize, printable: bool) {
        self.record_output_at(session_id, byte_count, printable, Instant::now());
    }

    /// #2124 - the injectable core of [`record_output`].
    ///
    /// A short periodic burst after long silence (a status-line repaint) must
    /// neither reset the auto-close silence clock nor reopen the #260 busy
    /// period. So a chunk that finds `burst_prior_silence` of silence opens a
    /// PENDING burst instead of counting, and nothing is touched while one is
    /// pending. The burst is confirmed - and only then counted, with its TOTAL
    /// byte count - when it reaches `burst_max_bytes` or spans `burst_window`; a
    /// burst whose next chunk arrives more than `burst_window` after its last is
    /// discarded as short. Input and inter-agent delivery still touch
    /// `silence`, so a pending candidate that finds a fresh clock is dropped and
    /// the current chunk counts immediately (its bytes are not added to the
    /// candidate's). Order: filter-off, gap discard, fresh-clock drop,
    /// accumulate/confirm, then open-or-count.
    pub(crate) fn record_output_at(
        &self,
        session_id: Uuid,
        byte_count: usize,
        printable: bool,
        now: Instant,
    ) {
        if let Some((_, observer)) = self.raw_output_observer.get() {
            observer(session_id, now);
        }
        let tuning = self
            .tuning
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&session_id)
            .copied()
            .unwrap_or(IdleTuning::DEFAULT);
        // 1. Filter off for this session: the exact pre-#2124 behavior.
        if tuning.burst_max_bytes == 0 || tuning.burst_window.is_zero() {
            self.touch_silence(session_id);
            if printable {
                self.record_activity_with_bytes(session_id, byte_count);
            }
            return;
        }

        let sid = &session_id.to_string()[..8];
        let silence_age = self.silence_age_at(session_id, now);
        let mut counts_now = false;
        let mut confirmed: Option<(u64, bool)> = None;
        {
            let mut bursts = self.burst.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pending) = bursts.get_mut(&session_id) {
                if now.saturating_duration_since(pending.last) > tuning.burst_window {
                    // 2. The gap closed the window: the candidate was a short
                    //    burst. Discard it and evaluate this chunk as fresh.
                    log::debug!(
                        "[idle] BURST IGNORED {} ({} bytes, {}ms)",
                        sid,
                        pending.bytes,
                        now.saturating_duration_since(pending.start).as_millis()
                    );
                    bursts.remove(&session_id);
                } else if silence_age < tuning.burst_prior_silence {
                    // 3a. Input or delivery touched the clock under the
                    //     candidate: it is not a burst, and this chunk counts.
                    bursts.remove(&session_id);
                    counts_now = true;
                } else {
                    // 3. Still inside the window: accumulate, then confirm on
                    //    bytes or on total elapsed window.
                    pending.bytes += byte_count as u64;
                    pending.last = now;
                    pending.printable |= printable;
                    if pending.bytes >= tuning.burst_max_bytes
                        || now.saturating_duration_since(pending.start) >= tuning.burst_window
                    {
                        confirmed = Some((pending.bytes, pending.printable));
                        bursts.remove(&session_id);
                    }
                }
            }
            // 4. No pending candidate. A chunk that finds real silence opens
            //    one; a single chunk at or over the byte limit confirms at once
            //    (the elapsed-window leg cannot fire on the chunk that opens it).
            if confirmed.is_none() && !counts_now && !bursts.contains_key(&session_id) {
                if silence_age < tuning.burst_prior_silence {
                    counts_now = true;
                } else if byte_count as u64 >= tuning.burst_max_bytes {
                    confirmed = Some((byte_count as u64, printable));
                } else {
                    bursts.insert(
                        session_id,
                        PendingBurst {
                            start: now,
                            last: now,
                            bytes: byte_count as u64,
                            printable,
                        },
                    );
                }
            }
        }

        if counts_now {
            self.touch_silence(session_id);
            if printable {
                self.record_activity_with_bytes(session_id, byte_count);
            }
        } else if let Some((bytes, saw_printable)) = confirmed {
            log::debug!("[idle] BURST CONFIRMED {} ({} bytes)", sid, bytes);
            self.touch_silence(session_id);
            if saw_printable {
                self.record_activity_with_bytes(session_id, bytes as usize);
            }
        }
    }

    /// #2124 - silence age against an injected instant. An untracked session
    /// (or a stamp newer than `now`) reads as zero prior silence: count now.
    fn silence_age_at(&self, session_id: Uuid, now: Instant) -> Duration {
        self.silence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&session_id)
            .and_then(|&stamp| now.checked_duration_since(stamp))
            .unwrap_or(Duration::ZERO)
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

    /// #1682 - record PTY activity for a write that is NOT a delivered message.
    /// Marks the session when this write FOUND it idle, and refreshes a mark that
    /// already stands; both halves are load-bearing and phase 03's D1 says why.
    /// "Found it idle" is "re-opened the busy period" everywhere except inside an
    /// active `resize_grace`, where nothing is re-opened and the mark still stands.
    /// Everything after the mark is `record_activity_with_bytes` unchanged, so
    /// the #260 idle dot, the #552 clock and the resize grace are unaffected.
    pub fn record_control_write_activity(&self, session_id: Uuid, byte_count: usize) {
        let reopened = self
            .idle_set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&session_id);
        {
            let mut marks = self.control_write.lock().unwrap_or_else(|e| e.into_inner());
            if reopened || marks.contains_key(&session_id) {
                marks.insert(session_id, Instant::now());
            }
        }
        self.record_activity_with_bytes(session_id, byte_count);
    }

    /// #1682 - arming an agent turn cancels the standing mark. Idempotent, and a
    /// no-op for a session that carries none.
    pub fn clear_control_write(&self, session_id: Uuid) {
        self.control_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id);
    }

    /// #1682 - age of the standing control-write mark, or `None` when none
    /// stands. Read once per busy->idle edge by the idle callback.
    pub fn control_write_age(&self, session_id: Uuid) -> Option<Duration> {
        self.control_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&session_id)
            .map(|since| since.elapsed())
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

    #[cfg(test)]
    pub(crate) fn set_control_write_age_for_test(&self, session_id: Uuid, age: Duration) {
        let now = Instant::now();
        let mut marks = self.control_write.lock().unwrap_or_else(|e| e.into_inner());
        if marks.contains_key(&session_id) {
            marks.insert(session_id, now.checked_sub(age).unwrap_or(now));
        }
    }

    #[cfg(test)]
    pub(crate) fn set_pty_input_ready_for_test(&self, session_id: Uuid) {
        let tuning = self
            .tuning
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&session_id)
            .copied()
            .unwrap_or(IdleTuning::DEFAULT);
        let age = tuning
            .idle_threshold
            .checked_add(Duration::from_secs(3))
            .expect("test idle age");
        self.activity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(session_id, Instant::now() - age);
        self.idle_set
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(session_id);
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
        let tuning = self
            .tuning
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
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
                    activity_age: activity
                        .get(id)
                        .and_then(|&x| now.checked_duration_since(x)),
                    watcher_idle: idle_set.contains(id),
                    last_resize_age: resizes.get(id).and_then(|&x| now.checked_duration_since(x)),
                    resize_grace: t.resize_grace,
                    idle_threshold: t.idle_threshold,
                    silence_age: silence.get(id).and_then(|&x| now.checked_duration_since(x)),
                }
            })
            .collect()
    }

    /// Validate all privileged readiness legs, acquire the caller's route
    /// lifecycle guard in the documented lock order, and stamp the watcher busy
    /// as one synchronous boundary. The callback is deliberately deferred until
    /// the SessionManager guard has also been released.
    pub(crate) fn prepare_pty_input_boundary<T, F>(
        &self,
        session_id: Uuid,
        lock_route: F,
    ) -> Result<(T, bool), PtyInputBoundaryFailure>
    where
        F: FnOnce() -> Result<T, crate::errors::AppError>,
    {
        let tuning = self
            .tuning
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let resizes = self
            .resize_grace
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut activity = self
            .activity
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut idle_set = self
            .idle_set
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        let resolved = tuning
            .get(&session_id)
            .copied()
            .ok_or(PtyInputBoundaryFailure::UntrackedReadiness)?;
        let activity_at = activity
            .get(&session_id)
            .copied()
            .ok_or(PtyInputBoundaryFailure::UntrackedReadiness)?;
        let activity_age = now
            .checked_duration_since(activity_at)
            .ok_or(PtyInputBoundaryFailure::UntrackedReadiness)?;
        let required_activity = resolved
            .idle_threshold
            .checked_add(Duration::from_secs(2))
            .ok_or(PtyInputBoundaryFailure::UntrackedReadiness)?;
        if !idle_set.contains(&session_id) || activity_age < required_activity {
            return Err(PtyInputBoundaryFailure::Busy);
        }
        if let Some(last_resize) = resizes.get(&session_id).copied() {
            let resize_age = now
                .checked_duration_since(last_resize)
                .ok_or(PtyInputBoundaryFailure::ResizeUnsettled)?;
            let required_resize = resolved
                .resize_grace
                .checked_add(Duration::from_secs(2))
                .ok_or(PtyInputBoundaryFailure::ResizeUnsettled)?;
            if resize_age < required_resize {
                return Err(PtyInputBoundaryFailure::ResizeUnsettled);
            }
        }

        let route = lock_route().map_err(|_| PtyInputBoundaryFailure::RouteUnavailable)?;
        activity.insert(session_id, now);
        let was_idle = idle_set.remove(&session_id);
        drop(idle_set);
        drop(activity);
        drop(resizes);
        drop(tuning);
        self.silence
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(session_id, now);
        Ok((route, was_idle))
    }

    pub(crate) fn notify_pty_input_busy(&self, session_id: Uuid, was_idle: bool) {
        if was_idle {
            (self.on_busy)(session_id);
        }
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
        {
            // #2879 - the current registration goes first, under `activity`: every attach
            // of it is invalid from here on.
            let mut activity = self.activity.lock().unwrap();
            activity.semantic.remove(&session_id);
            activity.remove(&session_id);
        }
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
        self.control_write
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id);
        self.burst
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id);
        self.tuning.lock().unwrap().remove(&session_id);
    }

    /// #2879 - attach a semantic consumer to the CURRENT CodexWorking registration of `id`.
    /// Every call is a new attach: it replaces the registration's observation token, which
    /// invalidates every earlier handle, and keeps the registration's evidence. `None`, and
    /// nothing changed, when `id` has no active CodexWorking registration. Never creates a
    /// registration or a clock.
    pub(crate) fn activity_consumer(self: &Arc<Self>, id: Uuid) -> Option<ActivityConsumerHandle> {
        let token = {
            let mut activity = self.activity.lock().unwrap_or_else(|e| e.into_inner());
            let registration = activity
                .semantic
                .get_mut(&id)
                .filter(|registration| registration.kind == ActivityDetectorKind::CodexWorking)?;
            let token = Arc::new(());
            registration.observation = Some(Arc::clone(&token));
            token
        };
        let observe_detector = Arc::clone(self);
        let observe_token = Arc::clone(&token);
        let retire_detector = Arc::clone(self);
        Some(ActivityConsumerHandle {
            observe: Arc::new(move |value: Option<bool>| {
                let mut activity = observe_detector
                    .activity
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let Some(registration) = activity.current_attach(id, &observe_token) else {
                    return false;
                };
                if let Some(value) = value {
                    registration.last_valid = Some(value);
                }
                true
            }),
            retire: Arc::new(move || {
                let mut activity = retire_detector
                    .activity
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let Some(registration) = activity.current_attach(id, &token) else {
                    return false;
                };
                registration.observation = None;
                registration.last_valid = None;
                true
            }),
        })
    }

    /// One watcher tick's commit, shared by `start` and the tests. `now` is taken AFTER the
    /// locks unless a test injects it. Returns the heartbeat's working set when asked for one.
    ///
    /// Each session's timed verdict is exactly the pre-#2879 rule (`> idle_threshold` since a
    /// PRESENT `activity` stamp); its detector kind decides whether positive semantic
    /// evidence vetoes the idle edge. A positive that finds the session idle re-opens it with
    /// the existing `on_busy`, once. Both callbacks run under `activity -> idle_set`, which
    /// is what orders them against observations and output.
    fn evaluate_tick(&self, now: Option<Instant>, heartbeat: bool) -> Option<Vec<Uuid>> {
        // Snapshot tuning (clone, lock released) so it is never held
        // across the activity/idle_set critical section. Lock order:
        // tuning → activity → idle_set (consistent everywhere).
        let tuning = self.tuning.lock().unwrap().clone();
        let activity = self.activity.lock().unwrap();
        let mut idle_set = self.idle_set.lock().unwrap();
        let now = now.unwrap_or_else(Instant::now);

        let crossing: HashMap<Uuid, Duration> =
            sessions_crossing_idle_threshold(now, &activity, &idle_set, &tuning)
                .into_iter()
                .collect();
        let mut ids: Vec<Uuid> = activity
            .timestamps
            .keys()
            .chain(activity.semantic.keys())
            .copied()
            .collect::<HashSet<Uuid>>()
            .into_iter()
            .collect();
        ids.sort_unstable();
        for session_id in ids {
            let registration = activity.semantic.get(&session_id);
            let kind = registration.map_or(ActivityDetectorKind::Timed, |r| r.kind);
            let positive = registration.is_some_and(|r| {
                r.kind == ActivityDetectorKind::CodexWorking && r.last_valid == Some(true)
            });
            let elapsed = crossing.get(&session_id).copied();
            if !activity_detector(kind).is_busy(elapsed.is_none(), positive) {
                // Only a timed crossing reaches here, so it is the pre-#2879 idle edge.
                let elapsed = elapsed.unwrap_or_default();
                idle_set.insert(session_id);
                log::debug!(
                    "[idle] IDLE {} ({}ms since last activity)",
                    &session_id.to_string()[..8],
                    elapsed.as_millis()
                );
                // Callback inside the lock scope preserves delivery order:
                // on_idle always fires before any on_busy for new activity.
                (self.on_idle)(session_id);
            } else if positive && idle_set.remove(&session_id) {
                log::debug!(
                    "[idle] BUSY {} (Codex Working indicator visible)",
                    &session_id.to_string()[..8]
                );
                (self.on_busy)(session_id);
            }
        }

        // #1149 - the heartbeat, and the universal backstop for closing
        // edges. A session that exits or is removed while working never
        // reaches `mark_idle`, so it emits no closing record; it simply
        // stops appearing here and the consumer closes it at the last
        // heartbeat that listed it. This is the detector's own view
        // deliberately, because this loop is a native thread and must not
        // take the async manager lock. It may therefore list a session
        // still pending creation, which is inert: the closing rule only
        // ever closes an interval a `busy` record opened.
        heartbeat.then(|| {
            activity
                .keys()
                .copied()
                .filter(|id| !idle_set.contains(id))
                .collect::<Vec<Uuid>>()
        })
    }

    /// Start the watcher thread that polls for idle transitions.
    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let detector = Arc::clone(self);
        std::thread::spawn(move || {
            let mut ticks: u64 = 0;
            loop {
                std::thread::sleep(CHECK_INTERVAL);

                if shutdown.is_cancelled() {
                    log::info!("[IdleDetector] Shutdown signal received, stopping");
                    break;
                }

                ticks = ticks.wrapping_add(1);
                let heartbeat = detector.evaluate_tick(None, ticks.is_multiple_of(HEARTBEAT_TICKS));
                // Both guards MUST be released before the append (`evaluate_tick`
                // drops them before it returns). These are the
                // two the PTY reader threads take together on every output chunk,
                // so appending under them would stall every terminal in the fleet
                // on a 60 s beat for the length of a file open, write and close,
                // and once per rotation for four renames. On Windows the config
                // directory is a real-time-scan target and may sit under a file
                // watcher, so a rename that normally costs 1 ms can cost
                // hundreds. Fleet-wide terminal jank with no log line explaining
                // it is the one failure here a user would actually feel.
                if let Some(working_session_ids) = heartbeat {
                    crate::config::activity_log::append(
                        crate::config::activity_log::build_app_alive(working_session_ids),
                    );
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
        let detector = IdleDetector::new(
            move |_| {},
            move |_| {
                busy_calls_cb.fetch_add(1, Ordering::SeqCst);
            },
        );
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

    #[test]
    fn pty_input_boundary_revalidates_idle_and_defers_busy_callback() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let busy_calls = Arc::new(AtomicUsize::new(0));
        let busy_calls_for_callback = Arc::clone(&busy_calls);
        let detector = IdleDetector::new(
            |_| {},
            move |_| {
                busy_calls_for_callback.fetch_add(1, Ordering::SeqCst);
            },
        );
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        detector.activity.lock().unwrap().insert(
            id,
            Instant::now() - IdleTuning::DEFAULT.idle_threshold - Duration::from_secs(3),
        );
        detector.idle_set.lock().unwrap().insert(id);
        let route_calls = AtomicUsize::new(0);
        let (marker, was_idle) = detector
            .prepare_pty_input_boundary(id, || {
                route_calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, crate::errors::AppError>("route")
            })
            .unwrap();
        assert_eq!(marker, "route");
        assert!(was_idle);
        assert_eq!(route_calls.load(Ordering::SeqCst), 1);
        assert_eq!(busy_calls.load(Ordering::SeqCst), 0);
        assert!(!detector.idle_set.lock().unwrap().contains(&id));
        assert!(detector.purge_readiness(&[id])[0]
            .activity_age
            .is_some_and(|age| age < Duration::from_secs(1)));
        detector.notify_pty_input_busy(id, was_idle);
        assert_eq!(busy_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn pty_input_boundary_rejects_busy_and_unsettled_resize_before_route_lock() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        let route_calls = AtomicUsize::new(0);
        let busy = detector.prepare_pty_input_boundary(id, || {
            route_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, crate::errors::AppError>(())
        });
        assert_eq!(busy.unwrap_err(), PtyInputBoundaryFailure::Busy);
        assert_eq!(route_calls.load(Ordering::SeqCst), 0);

        detector.activity.lock().unwrap().insert(
            id,
            Instant::now() - IdleTuning::DEFAULT.idle_threshold - Duration::from_secs(3),
        );
        detector.idle_set.lock().unwrap().insert(id);
        detector.record_resize(id);
        let resize = detector.prepare_pty_input_boundary(id, || {
            route_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, crate::errors::AppError>(())
        });
        assert_eq!(
            resize.unwrap_err(),
            PtyInputBoundaryFailure::ResizeUnsettled
        );
        assert_eq!(route_calls.load(Ordering::SeqCst), 0);
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

    /// #1682 - the executable form of the `idle_set` half of D1. Its exact
    /// property is "this write found the session idle"; the first half is the
    /// child-solicited device-status reply arriving mid-render, which must not
    /// mark and so must not cancel the genuine turn that is under way.
    #[test]
    fn a_control_write_marks_only_when_it_finds_the_session_idle() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        assert!(
            detector.control_write_age(id).is_none(),
            "a freshly registered session carries no mark"
        );

        detector.record_control_write_activity(id, 3);
        assert!(
            detector.control_write_age(id).is_none(),
            "a control write into a BUSY session must not mark: this is the \
             assertion that goes red if the idle_set half is dropped"
        );

        // The in-test control: the same call on an idle session does mark.
        detector.set_pty_input_ready_for_test(id);
        detector.record_control_write_activity(id, 3);
        assert!(
            detector.control_write_age(id).is_some(),
            "a control write that FOUND the session idle must mark"
        );
    }

    /// #1682 - the refresh half of D1: a ten-second burst of arrow keys must not
    /// age out mid-burst, even though only its first write finds the session idle.
    #[test]
    fn a_standing_control_write_mark_refreshes_while_the_session_is_busy() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        detector.set_pty_input_ready_for_test(id);
        detector.record_control_write_activity(id, 1);
        assert!(detector.control_write_age(id).is_some());

        detector.set_control_write_age_for_test(id, Duration::from_secs(30));
        assert!(
            detector.control_write_age(id).expect("the mark stands") >= Duration::from_secs(30),
            "the backdating helper must age the standing mark"
        );
        assert!(
            !detector.idle_set.lock().unwrap().contains(&id),
            "the first write made the session busy"
        );

        detector.record_control_write_activity(id, 1);
        assert!(
            detector
                .control_write_age(id)
                .expect("the mark still stands")
                < Duration::from_secs(1),
            "a later write must REFRESH the standing mark: this is the assertion \
             that goes red if the refresh half is dropped"
        );
    }

    /// #1682 - arming clears the mark, and the tail is the control: without it the
    /// test passes on a `control_write_age` that always returns `None`.
    #[test]
    fn clear_control_write_cancels_the_mark_and_a_later_write_sets_it_again() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        detector.set_pty_input_ready_for_test(id);
        detector.record_control_write_activity(id, 1);
        assert!(detector.control_write_age(id).is_some());

        detector.clear_control_write(id);
        assert!(
            detector.control_write_age(id).is_none(),
            "clear_control_write must cancel the standing mark"
        );

        detector.set_pty_input_ready_for_test(id);
        detector.record_control_write_activity(id, 1);
        assert!(
            detector.control_write_age(id).is_some(),
            "a later control write must be able to set the mark again"
        );
    }

    /// #1682 - the regression guard for the #260 idle dot: apart from the mark,
    /// `record_control_write_activity` must behave exactly as
    /// `record_activity_with_bytes` does.
    #[test]
    fn record_control_write_activity_still_produces_the_busy_edge() {
        let busy: Arc<Mutex<Vec<Uuid>>> = Arc::new(Mutex::new(Vec::new()));
        let busy_cb = Arc::clone(&busy);
        let detector = IdleDetector::new(
            |_| {},
            move |id| {
                busy_cb.lock().unwrap().push(id);
            },
        );
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        detector.set_pty_input_ready_for_test(id);
        assert!(
            busy.lock().unwrap().is_empty(),
            "no busy edge before the write"
        );

        detector.record_control_write_activity(id, 7);

        assert_eq!(
            *busy.lock().unwrap(),
            vec![id],
            "the new method must still fire on_busy exactly once"
        );
        assert!(
            !detector.idle_set.lock().unwrap().contains(&id),
            "the session must have left idle_set"
        );
    }

    /// #1682 - teardown drops the mark, so a destroyed and recreated session id
    /// cannot inherit one. Mirrors `remove_session_clears_tuning`.
    #[test]
    fn remove_session_clears_the_control_write_mark() {
        let detector = IdleDetector::new(|_| {}, |_| {});
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        detector.set_pty_input_ready_for_test(id);
        detector.record_control_write_activity(id, 1);
        assert!(detector.control_write_age(id).is_some());

        detector.remove_session(id);
        assert!(
            detector.control_write_age(id).is_none(),
            "remove_session must drop the control-write mark"
        );
    }

    // -----------------------------------------------------------------
    // #2124 - the idle-burst filter (T1-T8, T11, T13 of the plan).
    // -----------------------------------------------------------------

    /// The common fixture: a detector whose `on_busy` counts calls, one session
    /// registered with the shipped burst tuning, 30 minutes of accumulated
    /// silence and the #260 busy state standing, so any count must clear it.
    struct BurstFixture {
        detector: Arc<IdleDetector>,
        id: Uuid,
        busy_calls: Arc<std::sync::atomic::AtomicUsize>,
        t0: Instant,
    }

    impl BurstFixture {
        fn new() -> Self {
            let busy_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let busy_calls_cb = Arc::clone(&busy_calls);
            let detector = IdleDetector::new(
                |_| {},
                move |_| {
                    busy_calls_cb.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                },
            );
            let id = Uuid::new_v4();
            detector.register_session(
                id,
                IdleTuning {
                    burst_max_bytes: 1024,
                    burst_window: Duration::from_secs(3),
                    burst_prior_silence: Duration::from_secs(60),
                    ..IdleTuning::DEFAULT
                },
            );
            detector.set_auto_close_ages_for_test(
                id,
                Duration::from_secs(30 * 60),
                Duration::from_secs(31 * 60),
            );
            detector.set_pty_input_ready_for_test(id);
            Self {
                detector,
                id,
                busy_calls,
                t0: Instant::now(),
            }
        }

        fn busy(&self) -> usize {
            self.busy_calls.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn record(&self, bytes: usize, printable: bool, at: Instant) {
            self.detector
                .record_output_at(self.id, bytes, printable, at);
        }

        fn silence_age(&self) -> Duration {
            self.detector.silence_age(self.id).expect("tracked silence")
        }

        fn pending(&self) -> bool {
            self.detector
                .burst
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&self.id)
        }
    }

    /// T1 - the reported bug: a periodic status-line burst (64 B printable +
    /// 31 B escape-only, 296 ms apart) after 30 minutes of silence must not
    /// reopen the busy period nor reset the auto-close clock.
    #[test]
    fn t1_periodic_short_burst_is_ignored() {
        let fx = BurstFixture::new();
        fx.record(64, true, fx.t0);
        fx.record(31, false, fx.t0 + Duration::from_millis(296));
        assert_eq!(fx.busy(), 0, "a status-line burst must not fire on_busy");
        assert!(
            fx.silence_age() >= Duration::from_secs(30 * 60),
            "the auto-close clock must not move"
        );
        assert!(
            fx.detector.idle_set.lock().unwrap().contains(&fx.id),
            "the session must stay in the idle set"
        );
    }

    /// T2 - positive control: sustained work (250 x 47 B printable, 24 ms
    /// apart) confirms the burst; the count lands exactly on the 22nd chunk
    /// (1024 B / 47 B) and the session leaves the idle set.
    #[test]
    fn t2_sustained_work_confirms_the_burst() {
        let fx = BurstFixture::new();
        for chunk in 0..250u32 {
            fx.record(
                47,
                true,
                fx.t0 + Duration::from_millis(24 * u64::from(chunk)),
            );
            let expected = usize::from(chunk >= 21);
            assert_eq!(fx.busy(), expected, "chunk {chunk}");
        }
        assert!(fx.silence_age() < Duration::from_secs(1));
        assert!(!fx.detector.idle_set.lock().unwrap().contains(&fx.id));
    }

    /// T3 - the time leg: sub-threshold chunks every second confirm when the
    /// pending burst reaches `burst_window`, not before.
    #[test]
    fn t3_burst_confirms_on_the_elapsed_window() {
        let fx = BurstFixture::new();
        for second in 0..3u64 {
            fx.record(10, true, fx.t0 + Duration::from_secs(second));
            assert_eq!(fx.busy(), 0, "t0+{second}s is still inside the window");
        }
        fx.record(10, true, fx.t0 + Duration::from_secs(3));
        assert_eq!(fx.busy(), 1, "the window leg confirms at t0+3s");
        assert!(fx.silence_age() < Duration::from_secs(1));
    }

    /// T3b - the discard boundary: a gap of EXACTLY `burst_window` must not
    /// discard (the gap test is strictly greater), so the second chunk
    /// confirms on the elapsed-window leg. Kills `>` -> `>=` on that test.
    #[test]
    fn t3b_exact_window_gap_is_not_a_discard() {
        let fx = BurstFixture::new();
        fx.record(10, true, fx.t0);
        assert!(fx.pending());
        fx.record(10, true, fx.t0 + Duration::from_secs(3));
        assert_eq!(fx.busy(), 1, "a 3 s gap is not a discard");
        assert!(!fx.pending());
        assert!(fx.silence_age() < Duration::from_secs(1));
    }

    /// T4 - the byte limit is inclusive: 1023 B stays pending, 1024 B confirms
    /// immediately, and 1024 B reached ACROSS chunks confirms too. The last
    /// leg kills `>=` -> `>` on the accumulated total (T2 reaches 1034, never
    /// 1024 exactly).
    #[test]
    fn t4_burst_byte_limit_is_inclusive() {
        let fx = BurstFixture::new();
        fx.record(1023, true, fx.t0);
        assert_eq!(fx.busy(), 0);
        assert!(fx.pending(), "1023 B is below the limit and stays pending");

        let fx = BurstFixture::new();
        fx.record(1024, true, fx.t0);
        assert_eq!(fx.busy(), 1);
        assert!(!fx.pending());

        let fx = BurstFixture::new();
        fx.record(1023, true, fx.t0);
        assert_eq!(fx.busy(), 0);
        fx.record(1, true, fx.t0 + Duration::from_secs(1));
        assert_eq!(fx.busy(), 1, "1023 B + 1 B reaches the limit exactly");
        assert!(!fx.pending());
        assert!(fx.silence_age() < Duration::from_secs(1));
    }

    /// T5 - a candidate discarded by the gap never counts, and the same shape
    /// much later still does not.
    #[test]
    fn t5_burst_expires_on_gap_and_stays_ignored() {
        let fx = BurstFixture::new();
        fx.record(95, true, fx.t0);
        fx.record(95, true, fx.t0 + Duration::from_millis(3500));
        assert_eq!(fx.busy(), 0);
        assert!(fx.silence_age() >= Duration::from_secs(30 * 60));

        let late = fx.t0 + Duration::from_secs(30 * 60);
        fx.record(95, true, late);
        fx.record(95, true, late + Duration::from_millis(296));
        assert_eq!(fx.busy(), 0, "the periodic pattern never confirms");
        assert!(fx.silence_age() >= Duration::from_secs(30 * 60));
    }

    /// T6 - a short prior silence (input, delivery) counts immediately.
    #[test]
    fn t6_short_prior_silence_counts_immediately() {
        let fx = BurstFixture::new();
        fx.detector.touch_silence(fx.id);
        fx.record(5, true, Instant::now() + Duration::from_millis(10));
        assert_eq!(fx.busy(), 1);
        assert!(!fx.pending());
    }

    /// T7 - a session with the filter disabled keeps the exact old behavior.
    #[test]
    fn t7_disabled_filter_counts_every_chunk() {
        let busy_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let busy_calls_cb = Arc::clone(&busy_calls);
        let detector = IdleDetector::new(
            |_| {},
            move |_| {
                busy_calls_cb.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        );
        let id = Uuid::new_v4();
        detector.register_session(id, IdleTuning::DEFAULT);
        detector.set_auto_close_ages_for_test(
            id,
            Duration::from_secs(30 * 60),
            Duration::from_secs(31 * 60),
        );
        detector.set_pty_input_ready_for_test(id);

        detector.record_output_at(id, 5, true, Instant::now());
        assert_eq!(busy_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(detector.silence_age(id).unwrap() < Duration::from_secs(1));
        assert!(detector.burst.lock().unwrap().is_empty());
    }

    /// T8 - a large escape-only chunk confirms by bytes but records no
    /// printable activity: the silence clock resets, the idle dot does not.
    #[test]
    fn t8_large_escape_only_chunk_confirms_without_busy() {
        let fx = BurstFixture::new();
        fx.record(2000, false, fx.t0);
        assert_eq!(
            fx.busy(),
            0,
            "escape-only output must not flip the idle dot"
        );
        assert!(fx.silence_age() < Duration::from_secs(1));
        assert!(!fx.pending());
    }

    /// T11 - `remove_session` drops the pending candidate.
    #[test]
    fn t11_remove_session_clears_the_pending_burst() {
        let fx = BurstFixture::new();
        fx.record(95, true, fx.t0);
        assert!(fx.pending());
        fx.detector.remove_session(fx.id);
        assert!(!fx.pending(), "no candidate may survive its session");
    }

    /// T13 - a candidate that finds a freshly touched clock (input or a
    /// delivered message during the candidate) is dropped, and this chunk
    /// counts immediately without the candidate's bytes.
    #[test]
    fn t13_candidate_is_dropped_when_the_clock_moves_under_it() {
        let fx = BurstFixture::new();
        fx.record(64, true, fx.t0);
        assert!(fx.pending());
        fx.detector.touch_silence(fx.id);
        fx.record(5, true, Instant::now() + Duration::from_secs(1));
        assert_eq!(fx.busy(), 1);
        assert!(fx.silence_age() < Duration::from_secs(1));
        assert!(!fx.pending());
    }

    // -----------------------------------------------------------------
    // #2879 - the Codex Working detector, driven through `evaluate_tick`
    // with injected instants, so no test sleeps.
    // -----------------------------------------------------------------

    type I2879Edges = Arc<Mutex<Vec<(&'static str, Uuid, usize)>>>;

    fn i2879_codex() -> IdleTuning {
        IdleTuning {
            activity_detector: ActivityDetectorKind::CodexWorking,
            ..IdleTuning::DEFAULT
        }
    }

    /// A detector whose callbacks record `(edge, id, global order)`.
    fn i2879_detector() -> (
        Arc<IdleDetector>,
        I2879Edges,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let edges: I2879Edges = Arc::new(Mutex::new(Vec::new()));
        let order = Arc::new(AtomicUsize::new(0));
        let (idle_edges, busy_edges) = (Arc::clone(&edges), Arc::clone(&edges));
        let (idle_order, busy_order) = (Arc::clone(&order), Arc::clone(&order));
        let detector = IdleDetector::new(
            move |id| {
                let n = idle_order.fetch_add(1, Ordering::SeqCst);
                idle_edges.lock().unwrap().push(("idle", id, n));
            },
            move |id| {
                let n = busy_order.fetch_add(1, Ordering::SeqCst);
                busy_edges.lock().unwrap().push(("busy", id, n));
            },
        );
        (detector, edges, order)
    }

    fn i2879_edges(edges: &I2879Edges) -> Vec<(&'static str, Uuid)> {
        edges
            .lock()
            .unwrap()
            .iter()
            .map(|(edge, id, _)| (*edge, *id))
            .collect()
    }

    fn i2879_stamp(detector: &IdleDetector, id: Uuid) -> Instant {
        *detector.activity.lock().unwrap().get(&id).expect("seeded")
    }

    /// `(kind, attached, last_valid)` of the current registration.
    fn i2879_registration(
        detector: &IdleDetector,
        id: Uuid,
    ) -> Option<(ActivityDetectorKind, bool, Option<bool>)> {
        detector
            .activity
            .lock()
            .unwrap()
            .semantic
            .get(&id)
            .map(|r| (r.kind, r.observation.is_some(), r.last_valid))
    }

    /// A visible indicator keeps a Codex session busy long after the threshold, with no
    /// bytes and no refresh of any clock; the Timed control on the same instants goes idle.
    #[test]
    fn i2879_feature_frozen() {
        let (detector, edges, _) = i2879_detector();
        let codex = Uuid::new_v4();
        let timed = Uuid::new_v4();
        detector.register_session(codex, i2879_codex());
        detector.register_session(timed, IdleTuning::DEFAULT);
        let t0 = i2879_stamp(&detector, codex);
        detector.activity.lock().unwrap().insert(timed, t0);
        let silence = *detector.silence.lock().unwrap().get(&codex).unwrap();

        let handle = detector.activity_consumer(codex).expect("Codex attaches");
        assert!((handle.observe)(Some(true)));

        let threshold = i2879_codex().idle_threshold;
        for after in [
            threshold + Duration::from_millis(1),
            threshold * 4,
            Duration::from_secs(10 * 60 * 60),
        ] {
            assert!(detector.evaluate_tick(Some(t0 + after), false).is_none());
            // An Unchanged frame or a validation delivers nothing new and keeps the evidence.
            assert!((handle.observe)(None));
        }

        assert_eq!(
            i2879_edges(&edges),
            vec![("idle", timed)],
            "the Timed control idles once; the visible indicator never lets Codex idle"
        );
        assert!(!detector.idle_set.lock().unwrap().contains(&codex));
        assert_eq!(i2879_stamp(&detector, codex), t0, "no activity refresh");
        assert_eq!(
            *detector.silence.lock().unwrap().get(&codex).unwrap(),
            silence,
            "no silence refresh"
        );
        assert_eq!(
            i2879_registration(&detector, codex),
            Some((ActivityDetectorKind::CodexWorking, true, Some(true)))
        );
    }

    /// Without positive evidence - none yet, an initial Unavailable, or a valid negative -
    /// Codex is exactly Timed on both sides of the threshold; a positive followed by a
    /// negative never ends the turn before the timed clock does.
    #[test]
    fn i2879_feature_fallback() {
        let threshold = i2879_codex().idle_threshold;
        for case in ["none", "unavailable", "negative", "positive-then-negative"] {
            let (detector, edges, _) = i2879_detector();
            let id = Uuid::new_v4();
            detector.register_session(id, i2879_codex());
            let t0 = i2879_stamp(&detector, id);
            let handle = detector.activity_consumer(id).expect("Codex attaches");
            match case {
                "none" => {}
                "unavailable" => assert!((handle.observe)(None)),
                "negative" => assert!((handle.observe)(Some(false))),
                _ => {
                    assert!((handle.observe)(Some(true)));
                    detector.evaluate_tick(Some(t0 + Duration::from_millis(1)), false);
                    assert!((handle.observe)(Some(false)));
                }
            }
            detector.evaluate_tick(Some(t0 + threshold), false);
            assert!(
                i2879_edges(&edges).is_empty(),
                "{case}: not past the threshold"
            );
            detector.evaluate_tick(Some(t0 + threshold + Duration::from_millis(1)), false);
            detector.evaluate_tick(Some(t0 + Duration::from_secs(3600)), false);
            assert_eq!(i2879_edges(&edges), vec![("idle", id)], "{case}");
        }

        // The control: the same instants with positive evidence never idle.
        let (detector, edges, _) = i2879_detector();
        let id = Uuid::new_v4();
        detector.register_session(id, i2879_codex());
        let t0 = i2879_stamp(&detector, id);
        let handle = detector.activity_consumer(id).expect("Codex attaches");
        assert!((handle.observe)(Some(true)));
        detector.evaluate_tick(Some(t0 + threshold + Duration::from_millis(1)), false);
        assert!(i2879_edges(&edges).is_empty());
    }

    /// From idle, a positive under a resize grace and an unconfirmed burst - output that
    /// moves no clock - emits `on_busy` exactly once through the real commit, and the
    /// observation itself touches no output or silence clock.
    #[test]
    fn i2879_feature_suppressed_output() {
        let (detector, edges, _) = i2879_detector();
        let id = Uuid::new_v4();
        detector.register_session(
            id,
            IdleTuning {
                activity_detector: ActivityDetectorKind::CodexWorking,
                ..IdleTuning::AGENT_DEFAULT
            },
        );
        detector.set_auto_close_ages_for_test(
            id,
            Duration::from_secs(30 * 60),
            Duration::from_secs(31 * 60),
        );
        detector.set_pty_input_ready_for_test(id);
        let stamp = i2879_stamp(&detector, id);
        let silence = *detector.silence.lock().unwrap().get(&id).unwrap();

        detector.record_resize(id);
        detector.record_activity_with_bytes(id, 500);
        detector.record_output_at(id, 64, true, Instant::now());
        assert!(
            detector.burst.lock().unwrap().contains_key(&id),
            "burst pending"
        );
        detector.evaluate_tick(Some(stamp + Duration::from_secs(3600)), false);
        assert!(
            i2879_edges(&edges).is_empty(),
            "suppressed output and no evidence: still idle, no edge"
        );

        let handle = detector.activity_consumer(id).expect("Codex attaches");
        assert!((handle.observe)(Some(true)));
        assert!(
            i2879_edges(&edges).is_empty(),
            "observing alone emits nothing"
        );
        detector.evaluate_tick(Some(stamp + Duration::from_secs(3600)), false);
        detector.evaluate_tick(Some(stamp + Duration::from_secs(7200)), false);
        assert_eq!(i2879_edges(&edges), vec![("busy", id)], "exactly once");
        assert!(!detector.idle_set.lock().unwrap().contains(&id));
        assert_eq!(
            i2879_stamp(&detector, id),
            stamp,
            "activity clock untouched"
        );
        assert_eq!(
            *detector.silence.lock().unwrap().get(&id).unwrap(),
            silence,
            "silence clock untouched"
        );
    }

    /// Token identity: a new attach, a retire, `remove_session` and a re-registration each
    /// reject every older handle without mutating anything, and nothing positive is
    /// inherited by a re-registration or a new UUID. Timed registrations never attach.
    #[test]
    fn i2879_feature_retire() {
        let (detector, edges, _) = i2879_detector();
        let id = Uuid::new_v4();
        detector.register_session(id, i2879_codex());

        let first = detector.activity_consumer(id).expect("Codex attaches");
        assert!((first.observe)(Some(true)));
        let second = detector.activity_consumer(id).expect("a new attach");
        assert!(
            !(first.observe)(Some(false)),
            "a replaced attach is rejected"
        );
        assert!(
            !(first.retire)(),
            "a late retire of a replaced attach is rejected"
        );
        assert_eq!(
            i2879_registration(&detector, id),
            Some((ActivityDetectorKind::CodexWorking, true, Some(true))),
            "the rejected calls mutated nothing and the new attach kept the evidence"
        );
        assert!((second.observe)(None));

        assert!((second.retire)(), "the current attach retires");
        assert_eq!(
            i2879_registration(&detector, id),
            Some((ActivityDetectorKind::CodexWorking, false, None)),
            "retire releases the evidence and keeps the registration"
        );
        assert!(
            !(second.observe)(Some(true)),
            "a retired attach is rejected"
        );
        assert!(!(second.retire)());

        // Removal (a confirmed Gone or a destroy) invalidates the current attach.
        let third = detector.activity_consumer(id).expect("re-attach");
        assert!((third.observe)(Some(true)));
        detector.remove_session(id);
        assert!(!(third.observe)(Some(true)));
        assert!(!(third.retire)());
        assert!(i2879_registration(&detector, id).is_none());
        assert!(
            detector.activity_consumer(id).is_none(),
            "nothing to attach to"
        );

        // Re-registration of the same UUID starts clean and rejects the old handle.
        detector.register_session(id, i2879_codex());
        assert!(
            !(third.observe)(Some(true)),
            "late callback after re-registration"
        );
        assert_eq!(
            i2879_registration(&detector, id),
            Some((ActivityDetectorKind::CodexWorking, false, None))
        );
        let fourth = detector.activity_consumer(id).expect("attach");
        assert!((fourth.observe)(Some(true)));
        detector.register_session(id, i2879_codex());
        assert!(!(fourth.observe)(Some(true)));
        assert!(!(fourth.retire)());
        assert_eq!(
            i2879_registration(&detector, id),
            Some((ActivityDetectorKind::CodexWorking, false, None)),
            "no positive is inherited"
        );

        // A new UUID never sees another session's evidence; Timed never attaches.
        let other = Uuid::new_v4();
        detector.register_session(other, i2879_codex());
        assert_eq!(
            i2879_registration(&detector, other),
            Some((ActivityDetectorKind::CodexWorking, false, None))
        );
        let timed = Uuid::new_v4();
        detector.register_session(timed, IdleTuning::DEFAULT);
        assert!(detector.activity_consumer(timed).is_none());
        assert_eq!(
            i2879_registration(&detector, timed),
            Some((ActivityDetectorKind::Timed, false, None)),
            "a refused attach mutates nothing"
        );

        // With all evidence gone, the re-registered session follows the timed clock.
        let t0 = i2879_stamp(&detector, id);
        detector.evaluate_tick(Some(t0 + Duration::from_secs(3600)), false);
        assert!(i2879_edges(&edges).contains(&("idle", id)));
    }

    /// The commit order: every edge is emitted under `activity -> idle_set`, so an edge
    /// recorded after an observation returned saw that observation. Deterministic phases
    /// first, then a real concurrent interleaving of observation and ticks.
    #[test]
    fn i2879_feature_commit_order() {
        use std::sync::atomic::Ordering;
        let (detector, edges, order) = i2879_detector();
        let id = Uuid::new_v4();
        detector.register_session(id, i2879_codex());
        let t0 = i2879_stamp(&detector, id);
        let threshold = i2879_codex().idle_threshold;
        let handle = detector.activity_consumer(id).expect("Codex attaches");
        let far = || Instant::now() + Duration::from_secs(3600);

        detector.evaluate_tick(Some(t0 + threshold + Duration::from_millis(1)), false);
        detector.record_activity_with_bytes(id, 3); // input re-opens the turn
        assert!((handle.observe)(Some(true)));
        detector.evaluate_tick(Some(far()), false); // positive vetoes idle
        assert!((handle.observe)(Some(false)));
        detector.evaluate_tick(Some(far()), false); // negative: timed idle
        assert!((handle.observe)(Some(true)));
        detector.evaluate_tick(Some(far()), false); // positive re-opens, once
        detector.evaluate_tick(Some(far()), false);
        detector.record_activity_with_bytes(id, 3); // already busy: no edge
        assert_eq!(
            i2879_edges(&edges),
            vec![("idle", id), ("busy", id), ("idle", id), ("busy", id)]
        );

        // Concurrent: a tick thread races one observation. No idle may be ordered after
        // the observation's commit returned.
        let racing = Uuid::new_v4();
        detector.register_session(racing, i2879_codex());
        let racing_handle = detector.activity_consumer(racing).expect("attach");
        edges.lock().unwrap().clear();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let ticker = {
            let detector = Arc::clone(&detector);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..500 {
                    detector.evaluate_tick(Some(Instant::now() + Duration::from_secs(3600)), false);
                }
            })
        };
        barrier.wait();
        assert!((racing_handle.observe)(Some(true)));
        let committed = order.fetch_add(1, Ordering::SeqCst);
        ticker.join().expect("ticker");
        detector.evaluate_tick(Some(far()), false);

        let racing_edges: Vec<(&str, usize)> = edges
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, edge_id, _)| *edge_id == racing)
            .map(|(edge, _, n)| (*edge, *n))
            .collect();
        assert!(
            racing_edges
                .iter()
                .all(|(edge, n)| *edge != "idle" || *n < committed),
            "an idle edge after a committed positive: {racing_edges:?} committed={committed}"
        );
        assert!(racing_edges.iter().filter(|(e, _)| *e == "idle").count() <= 1);
        if racing_edges.iter().any(|(e, _)| *e == "idle") {
            assert_eq!(racing_edges.last().map(|(e, _)| *e), Some("busy"));
        }
        assert!(!detector.idle_set.lock().unwrap().contains(&racing));
    }
}

#[cfg(test)]
mod response_close_raw_output_tests {
    use super::*;

    #[test]
    fn response_close_raw_output_observer_precedes_filter_and_receives_escape_only_output() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let captured = observed.clone();
        let detector = IdleDetector::new(|_| {}, |_| {});
        assert!(detector
            .bind_raw_output_observer(1, move |id, now| captured.lock().unwrap().push((id, now))));
        assert!(!detector.bind_raw_output_observer(2, |_, _| {}));
        let id = Uuid::new_v4();
        let now = Instant::now();
        detector.record_output_at(id, 3, false, now);
        assert_eq!(observed.lock().unwrap().as_slice(), &[(id, now)]);
        assert_eq!(detector.raw_output_observer_identity(), Some(1));
    }
}
