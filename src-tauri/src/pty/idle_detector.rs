use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::config::activity_log::ActivityCoalescer;
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
    activity_log_owner: Arc<ActivityCoalescer>,
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
        Self::with_activity_coalescer(Arc::new(ActivityCoalescer::new()), on_idle, on_busy)
    }

    pub(crate) fn with_activity_coalescer(
        owner: Arc<ActivityCoalescer>,
        on_idle: impl Fn(Uuid) + Send + Sync + 'static,
        on_busy: impl Fn(Uuid) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            activity_log_owner: owner,
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

    pub(crate) fn notify_raw_output_at(&self, session_id: Uuid, at: Instant) {
        if let Some((_, observer)) = self.raw_output_observer.get() {
            observer(session_id, at);
        }
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
        self.notify_raw_output_at(session_id, now);
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
                        crate::config::activity_log::build_app_alive(
                            working_session_ids,
                            &detector.activity_log_owner,
                        ),
                        &detector.activity_log_owner,
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
