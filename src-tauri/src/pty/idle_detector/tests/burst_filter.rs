use super::*;

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
