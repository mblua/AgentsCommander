use super::*;
use std::sync::Mutex as StdMutex;

type LivenessHook = Arc<dyn Fn(Uuid) + Send + Sync>;

const CLAUDE: &str = r"^ {2}Context [\u{2591}\u{2588}]+ (\d{1,3})%";
const AGENT: &str = "claude";

fn row(percent: u8) -> String {
    format!("  Context \u{2591}\u{2591}\u{2588} {percent}%")
}

/// A THREE-state rows fake, because the seam is three-state. Under a two-state
/// `Option<Vec<String>>` the H1 defect was not merely untested, it was unsayable: the
/// fake had no way to express "a live child we could not ask", so no test could have
/// caught the design pruning it forever.
#[derive(Default)]
struct RowsFake {
    script: StdMutex<Vec<ScreenRowsRead>>,
    calls: StdMutex<Vec<Uuid>>,
    liveness_script: StdMutex<Vec<ContextSessionLiveness>>,
    liveness_calls: StdMutex<Vec<Uuid>>,
    on_liveness: StdMutex<Option<LivenessHook>>,
}

impl RowsFake {
    fn scripted(reads: Vec<ScreenRowsRead>) -> Arc<Self> {
        Arc::new(Self {
            script: StdMutex::new(reads),
            calls: StdMutex::new(Vec::new()),
            liveness_script: StdMutex::new(Vec::new()),
            liveness_calls: StdMutex::new(Vec::new()),
            on_liveness: StdMutex::new(None),
        })
    }
    fn with_liveness(reads: Vec<ContextSessionLiveness>) -> Arc<Self> {
        Arc::new(Self {
            script: StdMutex::new(Vec::new()),
            calls: StdMutex::new(Vec::new()),
            liveness_script: StdMutex::new(reads),
            liveness_calls: StdMutex::new(Vec::new()),
            on_liveness: StdMutex::new(None),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
    fn liveness_call_count(&self) -> usize {
        self.liveness_calls.lock().unwrap().len()
    }
}

impl ScreenRowsSource for RowsFake {
    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        self.calls.lock().unwrap().push(id);
        let mut script = self.script.lock().unwrap();
        if script.is_empty() {
            ScreenRowsRead::Unavailable
        } else {
            script.remove(0)
        }
    }

    fn get_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        self.liveness_calls.lock().unwrap().push(id);
        let hook = self.on_liveness.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(id);
        }
        let mut script = self.liveness_script.lock().unwrap();
        if script.is_empty() {
            ContextSessionLiveness::Unavailable
        } else {
            script.remove(0)
        }
    }
}

#[derive(Default)]
struct PatternFake {
    patterns: StdMutex<HashMap<String, String>>,
    unavailable: std::sync::atomic::AtomicBool,
    calls: StdMutex<usize>,
}

impl PatternFake {
    fn with(agent: &str, source: &str) -> Arc<Self> {
        let fake = Self::default();
        fake.patterns
            .lock()
            .unwrap()
            .insert(agent.to_string(), source.to_string());
        Arc::new(fake)
    }
    fn set(&self, agent: &str, source: Option<&str>) {
        let mut patterns = self.patterns.lock().unwrap();
        match source {
            Some(source) => patterns.insert(agent.to_string(), source.to_string()),
            None => patterns.remove(agent),
        };
    }
    fn call_count(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

impl ContextPatternSource for PatternFake {
    fn patterns(&self) -> BoxFuture<'_, Option<HashMap<String, String>>> {
        Box::pin(async move {
            *self.calls.lock().unwrap() += 1;
            if self.unavailable.load(Ordering::Relaxed) {
                None
            } else {
                Some(self.patterns.lock().unwrap().clone())
            }
        })
    }
}

#[derive(Default)]
struct SinkFake {
    emitted: StdMutex<Vec<Option<u8>>>,
}

impl SinkFake {
    fn emitted(&self) -> Vec<Option<u8>> {
        self.emitted.lock().unwrap().clone()
    }
}

impl ContextEventSink for SinkFake {
    fn emit(&self, payload: ContextUsagePayload) {
        self.emitted.lock().unwrap().push(payload.percent);
    }
}

#[derive(Default)]
struct SamplesFake {
    observed: StdMutex<Vec<ContextSample>>,
}

impl SamplesFake {
    fn observed(&self) -> Vec<ContextSample> {
        self.observed.lock().unwrap().clone()
    }
}

impl ContextSampleSink for SamplesFake {
    fn observe(&self, sample: ContextSample) {
        self.observed.lock().unwrap().push(sample);
    }
}

/// One recorded `commit` call: the changed `(id, percent)` pairs for one tick.
type CommittedTick = Vec<(Uuid, Option<u8>)>;

/// #1088 - records every `commit(changed)` the scraper hands the persist sink.
/// Each element is one tick's committed vector; `commits().len()` is the commit
/// call count (0 when a tick changed nothing).
#[derive(Default)]
struct PersistFake {
    commits: StdMutex<Vec<CommittedTick>>,
}

impl PersistFake {
    fn commits(&self) -> Vec<CommittedTick> {
        self.commits.lock().unwrap().clone()
    }
}

impl ContextPersistSink for PersistFake {
    fn commit(&self, changed: Vec<(Uuid, Option<u8>)>) -> BoxFuture<'_, ()> {
        self.commits.lock().unwrap().push(changed);
        Box::pin(async {})
    }
}

struct Harness {
    scraper: Arc<ContextScraper>,
    rows: Arc<RowsFake>,
    patterns: Arc<PatternFake>,
    sink: Arc<SinkFake>,
    samples: Arc<SamplesFake>,
    persist: Arc<PersistFake>,
}

fn harness(rows: Arc<RowsFake>, patterns: Arc<PatternFake>) -> Harness {
    let sink = Arc::new(SinkFake::default());
    let samples = Arc::new(SamplesFake::default());
    let persist = Arc::new(PersistFake::default());
    let scraper = ContextScraper::new(
        Arc::clone(&rows) as Arc<dyn ScreenRowsSource>,
        Arc::clone(&patterns) as Arc<dyn ContextPatternSource>,
        Arc::clone(&sink) as Arc<dyn ContextEventSink>,
        Arc::clone(&samples) as Arc<dyn ContextSampleSink>,
        Arc::clone(&persist) as Arc<dyn ContextPersistSink>,
    );
    Harness {
        scraper,
        rows,
        patterns,
        sink,
        samples,
        persist,
    }
}

#[tokio::test]
async fn e2992_p51_unavailable_mid_tick_retirement() {
    let h = harness(RowsFake::scripted(vec![]), PatternFake::with(AGENT, CLAUDE));
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());
    h.patterns.unavailable.store(true, Ordering::Relaxed);
    *h.rows.liveness_script.lock().unwrap() = vec![ContextSessionLiveness::SessionOver];
    let (entered, handshake) = tokio::sync::oneshot::channel();
    let entered = std::sync::Mutex::new(Some(entered));
    let (release, resume) = std::sync::mpsc::sync_channel(1);
    let resume = std::sync::Mutex::new(resume);
    *h.rows.on_liveness.lock().unwrap() = Some(Arc::new(move |_| {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        resume
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
    }));
    let engine = Arc::clone(&h.scraper);
    let tick = tokio::task::spawn_blocking(move || futures::executor::block_on(engine.tick()));
    tokio::time::timeout(Duration::from_secs(2), handshake)
        .await
        .unwrap()
        .unwrap();
    h.scraper.retire_session(id);
    let replacement = Uuid::new_v4();
    h.scraper.register_session(replacement, AGENT.to_string());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), tick)
        .await
        .unwrap()
        .unwrap();
    *h.rows.on_liveness.lock().unwrap() = None;
    assert!(!h.scraper.is_session_registered(id));
    assert!(h.scraper.is_session_registered(replacement));
    assert!(h.samples.observed().is_empty());
    assert!(h.persist.commits().is_empty());
    assert!(h.sink.emitted().is_empty());

    let shutdown = crate::shutdown::ShutdownSignal::new();
    let mut completed_ticks = 0;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.token().cancelled() => break,
                _ = h.scraper.tick() => {
                    completed_ticks += 1;
                    if completed_ticks == 3 { shutdown.trigger(); }
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(completed_ticks, 3);
}

#[tokio::test]
async fn e2992_p51_context_unavailable_preserves() {
    let h = harness(
        RowsFake::scripted(vec![ScreenRowsRead::Rows(vec![row(42)])]),
        PatternFake::with(AGENT, CLAUDE),
    );
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());
    h.scraper.tick().await;
    assert_eq!(h.sink.emitted(), vec![Some(42)]);
    let cursor = h.scraper.sample_cursor.load(Ordering::Relaxed);
    h.patterns.unavailable.store(true, Ordering::Relaxed);
    *h.rows.liveness_script.lock().unwrap() = vec![
        ContextSessionLiveness::Live,
        ContextSessionLiveness::Unavailable,
    ];
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(1), h.scraper.tick())
            .await
            .unwrap();
    }
    assert_eq!(h.scraper.last_reading(id), Some(42));
    assert_eq!(h.rows.call_count(), 1);
    assert_eq!(h.scraper.compile_count(), 1);
    assert_eq!(h.scraper.sample_cursor.load(Ordering::Relaxed), cursor);
    assert_eq!(
        h.samples.observed(),
        vec![ContextSample::Reading {
            session_id: id,
            percent: 42
        }]
    );
    assert_eq!(h.persist.commits(), vec![vec![(id, Some(42))]]);
    h.patterns.set(AGENT, None);
    h.patterns.unavailable.store(false, Ordering::Relaxed);
    h.scraper.tick().await;
    h.scraper.tick().await;
    assert_eq!(h.sink.emitted(), vec![Some(42), None]);
    assert_eq!(
        h.persist.commits(),
        vec![vec![(id, Some(42))], vec![(id, None)]]
    );
    h.patterns.set(AGENT, Some(CLAUDE));
    h.rows
        .script
        .lock()
        .unwrap()
        .push(ScreenRowsRead::Rows(vec![row(70)]));
    h.scraper.tick().await;
    assert_eq!(h.scraper.last_reading(id), Some(70));
}

#[tokio::test]
async fn e2992_p51_unavailable_progress_and_retirement() {
    let h = harness(
        RowsFake::scripted(vec![ScreenRowsRead::Rows(vec![row(42)])]),
        PatternFake::with(AGENT, CLAUDE),
    );
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());
    h.scraper.tick().await;
    h.patterns.unavailable.store(true, Ordering::Relaxed);
    *h.rows.liveness_script.lock().unwrap() = vec![
        ContextSessionLiveness::Unavailable,
        ContextSessionLiveness::SessionOver,
    ];
    for _ in 0..3 {
        tokio::time::timeout(Duration::from_secs(1), h.scraper.tick())
            .await
            .unwrap();
    }
    assert!(!h.scraper.is_registered(id));
    assert_eq!(h.rows.liveness_call_count(), 2);
    assert_eq!(h.rows.call_count(), 1);
    assert_eq!(h.sink.emitted(), vec![Some(42), None]);
    assert_eq!(
        h.persist.commits(),
        vec![vec![(id, Some(42))], vec![(id, None)]]
    );
    assert_eq!(
        h.samples.observed().last(),
        Some(&ContextSample::SessionOver { session_id: id })
    );
    h.scraper.register_session(id, AGENT.to_string());
    h.scraper.retire_session(id);
    h.scraper.tick().await;
    assert!(!h.scraper.is_registered(id));
    assert_eq!(h.sink.emitted().len(), 2);
}

// Stage E (#1064) retirement race sentinel (plan section 10.4 item 21,
// acceptance item 35): retiring a session that never emitted a positive
// reading (last_emitted == None) emits NO final null, and repeated retirement
// is idempotent. A mutation that emitted a duplicate/unconditional null fails.
#[tokio::test]
async fn stage_e_retiring_a_never_emitted_session_emits_no_final_null() {
    let h = harness(RowsFake::scripted(vec![]), Arc::new(PatternFake::default()));
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());

    // Retire before any positive reading was ever emitted.
    h.scraper.retire_session(id);
    assert!(
        h.sink.emitted().is_empty(),
        "a never-emitted session must not produce a final null"
    );
    // Repeated retirement stays idempotent and emits nothing.
    h.scraper.retire_session(id);
    assert!(
        h.sink.emitted().is_empty(),
        "repeated retirement must not emit a duplicate null"
    );
    assert!(
        !h.scraper.is_session_registered(id),
        "a retired session is unregistered"
    );
}

/// Criterion 5. Not "emits nothing", but "costs nothing": the rows fake counts calls,
/// and an unconfigured agent must never reach it - no PTY lock, no 30-row clone.
#[tokio::test]
async fn a_session_whose_agent_has_no_pattern_takes_no_lock_and_emits_nothing() {
    let h = harness(RowsFake::scripted(vec![]), Arc::new(PatternFake::default()));
    h.scraper
        .register_session(Uuid::new_v4(), AGENT.to_string());

    h.scraper.tick().await;
    h.scraper.tick().await;

    assert_eq!(h.rows.call_count(), 0, "no pattern must mean no rows read");
    assert!(h.sink.emitted().is_empty(), "and no event");
    assert_eq!(h.samples.observed().len(), 2, "one sample per tick");
}

#[tokio::test]
async fn missing_pattern_uses_only_lightweight_liveness_and_prunes_on_end() {
    let rows = RowsFake::with_liveness(vec![
        ContextSessionLiveness::Live,
        ContextSessionLiveness::SessionOver,
    ]);
    let h = harness(Arc::clone(&rows), Arc::new(PatternFake::default()));
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());

    h.scraper.tick().await;
    assert!(h.scraper.is_registered(id));
    h.scraper.tick().await;

    assert_eq!(rows.call_count(), 0, "no screen rows may be cloned");
    assert_eq!(rows.liveness_call_count(), 2);
    assert_eq!(
        h.samples.observed(),
        vec![
            ContextSample::Unavailable { session_id: id },
            ContextSample::SessionOver { session_id: id },
        ]
    );
    assert!(!h.scraper.is_registered(id));
}

#[tokio::test]
async fn blank_and_invalid_patterns_use_liveness_without_reading_rows() {
    for (source, liveness) in [
        ("   ", ContextSessionLiveness::Live),
        (r"(unclosed", ContextSessionLiveness::Unavailable),
    ] {
        let rows = RowsFake::with_liveness(vec![liveness]);
        let h = harness(Arc::clone(&rows), PatternFake::with(AGENT, source));
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        h.scraper.tick().await;

        assert_eq!(rows.call_count(), 0, "source={source:?}");
        assert_eq!(rows.liveness_call_count(), 1, "source={source:?}");
        assert_eq!(
            h.samples.observed(),
            vec![ContextSample::Unavailable { session_id: id }]
        );
        assert!(h.scraper.is_registered(id));
    }
}

#[tokio::test]
async fn unregister_is_idempotent_and_prevents_future_probes() {
    let rows = RowsFake::with_liveness(vec![ContextSessionLiveness::Live]);
    let h = harness(Arc::clone(&rows), Arc::new(PatternFake::default()));
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());

    h.scraper.retire_session(id);
    h.scraper.retire_session(id);
    h.scraper.tick().await;

    assert!(!h.scraper.is_session_registered(id));
    assert_eq!(rows.liveness_call_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retirement_discards_an_inflight_row_result_and_emits_final_null_once() {
    struct BlockingRows {
        calls: AtomicUsize,
        started: std::sync::mpsc::Sender<()>,
        release: StdMutex<std::sync::mpsc::Receiver<()>>,
    }

    impl ScreenRowsSource for BlockingRows {
        fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return ScreenRowsRead::Rows(vec![row(42)]);
            }
            self.started.send(()).expect("signal row probe");
            self.release
                .lock()
                .unwrap()
                .recv()
                .expect("release row probe");
            ScreenRowsRead::Rows(vec![row(7)])
        }
    }

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let rows = Arc::new(BlockingRows {
        calls: AtomicUsize::new(0),
        started: started_tx,
        release: StdMutex::new(release_rx),
    });
    let patterns = PatternFake::with(AGENT, CLAUDE);
    let sink = Arc::new(SinkFake::default());
    let samples = Arc::new(SamplesFake::default());
    let persist = Arc::new(PersistFake::default());
    let scraper = ContextScraper::new(
        Arc::clone(&rows) as Arc<dyn ScreenRowsSource>,
        patterns as Arc<dyn ContextPatternSource>,
        Arc::clone(&sink) as Arc<dyn ContextEventSink>,
        Arc::clone(&samples) as Arc<dyn ContextSampleSink>,
        Arc::clone(&persist) as Arc<dyn ContextPersistSink>,
    );
    let id = Uuid::new_v4();
    scraper.register_session(id, AGENT.to_string());
    scraper.tick().await;
    assert_eq!(sink.emitted(), vec![Some(42)]);

    let tick = {
        let scraper = Arc::clone(&scraper);
        tokio::spawn(async move { scraper.tick().await })
    };
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("row probe started");
    scraper.retire_session(id);
    scraper.retire_session(id);
    release_tx.send(()).expect("release row probe");
    tick.await.expect("join tick");

    assert_eq!(sink.emitted(), vec![Some(42), None]);
    assert_eq!(
        samples.observed(),
        vec![ContextSample::Reading {
            session_id: id,
            percent: 42,
        }]
    );
    assert!(!scraper.is_registered(id));
}

/// The empty-map guard, in the only window it covers: app start until the first agent
/// session. It is two lines and it is honest about its scope.
#[tokio::test]
async fn an_empty_registered_map_never_reads_patterns() {
    let h = harness(RowsFake::scripted(vec![]), PatternFake::with(AGENT, CLAUDE));

    h.scraper.tick().await;

    assert_eq!(h.patterns.call_count(), 0);
}

/// Clearing the regex must clear the badge. An earlier design skipped unconfigured
/// agents entirely, which left the badge showing 42 forever for a feature the user had
/// just switched off.
#[tokio::test]
async fn clearing_a_pattern_emits_null_once() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());

    h.scraper.tick().await;
    assert_eq!(h.sink.emitted(), vec![Some(42)]);

    h.patterns.set(AGENT, None);
    h.scraper.tick().await;
    h.scraper.tick().await;

    assert_eq!(
        h.sink.emitted(),
        vec![Some(42), None],
        "null once, then silence"
    );
    assert!(
        h.scraper.is_registered(id),
        "clearing a regex does not end a session"
    );
}

/// The other half: typing a regex passes THROUGH invalid states, and an invalid pattern
/// must clear the badge rather than freeze it.
#[tokio::test]
async fn a_pattern_edited_to_something_invalid_emits_null_once() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    h.scraper
        .register_session(Uuid::new_v4(), AGENT.to_string());

    h.scraper.tick().await;
    h.patterns.set(AGENT, Some(r"^ {2}Context (\d{1,3}%"));
    h.scraper.tick().await;
    h.scraper.tick().await;

    assert_eq!(h.sink.emitted(), vec![Some(42), None]);
}

/// The failure is sticky, so the log is too: the compile site IS the log site, and a
/// broken regex must not write a line every 5 seconds for the rest of the app's life.
#[tokio::test]
async fn an_uncompilable_pattern_logs_once_per_change_not_once_per_tick() {
    let h = harness(
        RowsFake::scripted(vec![]),
        PatternFake::with(AGENT, r"^ {2}Context (\d{1,3}%"),
    );
    h.scraper
        .register_session(Uuid::new_v4(), AGENT.to_string());

    h.scraper.tick().await;
    h.scraper.tick().await;
    h.scraper.tick().await;

    assert_eq!(h.scraper.compile_count(), 1, "one compile, hence one log");
    assert_eq!(h.rows.call_count(), 0, "and a broken pattern reads no rows");
}

/// The compile cache is what makes per-tick resolution affordable, and per-tick
/// resolution is what deletes the respawn requirement: paste a regex, see it work,
/// without restarting every running agent.
#[tokio::test]
async fn a_changed_pattern_is_recompiled_within_one_tick() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec!["  Ctx 7%".to_string()]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    h.scraper
        .register_session(Uuid::new_v4(), AGENT.to_string());

    h.scraper.tick().await;
    assert_eq!(h.sink.emitted(), vec![Some(42)]);

    h.patterns.set(AGENT, Some(r"^ {2}Ctx (\d{1,3})%"));
    h.scraper.tick().await;

    assert_eq!(h.scraper.compile_count(), 2);
    assert_eq!(h.sink.emitted(), vec![Some(42), Some(7)]);
}

#[tokio::test]
async fn an_unchanged_pattern_is_not_recompiled() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    h.scraper
        .register_session(Uuid::new_v4(), AGENT.to_string());

    h.scraper.tick().await;
    h.scraper.tick().await;
    h.scraper.tick().await;

    assert_eq!(h.scraper.compile_count(), 1);
}

#[tokio::test]
async fn session_over_emits_null_once_and_prunes() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::SessionOver,
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());

    h.scraper.tick().await;
    h.scraper.tick().await;
    let reads_at_prune = h.rows.call_count();

    assert_eq!(h.sink.emitted(), vec![Some(42), None]);
    assert!(
        !h.scraper.is_registered(id),
        "a session that is over stops being sampled"
    );

    h.scraper.tick().await;
    assert_eq!(
        h.rows.call_count(),
        reads_at_prune,
        "and is never read again"
    );
    assert_eq!(
        h.sink.emitted(),
        vec![Some(42), None],
        "nor emitted for again"
    );
}

/// Criterion 7, and the reason `ScreenRowsRead` exists.
///
/// A child that is alive but whose handle cannot be queried is NOT a dead session. The
/// design this replaced pruned on it, which is not a 5-second flicker: `registered` is
/// only ever written at spawn, so the entry never comes back and the badge stays N/A for
/// the rest of a session whose child never died. Reproduced against real Windows
/// processes: the same live process reads Alive through a full-rights handle and
/// Unqueryable through one stripped of SYNCHRONIZE.
#[tokio::test]
async fn a_live_child_that_cannot_be_queried_is_not_pruned() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Unavailable,
            ScreenRowsRead::Rows(vec![row(42)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());

    h.scraper.tick().await;
    h.scraper.tick().await;
    assert!(
        h.scraper.is_registered(id),
        "we could not ask is not the child is dead: the entry must survive"
    );

    h.scraper.tick().await;

    assert_eq!(
        h.sink.emitted(),
        vec![Some(42), None, Some(42)],
        "the reading comes back on the tick after the handle answers again"
    );
    assert_eq!(
        h.samples.observed(),
        vec![
            ContextSample::Reading {
                session_id: id,
                percent: 42,
            },
            ContextSample::Unavailable { session_id: id },
            ContextSample::Reading {
                session_id: id,
                percent: 42,
            },
        ]
    );
}

#[tokio::test]
async fn an_unchanged_value_emits_once() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    h.scraper
        .register_session(Uuid::new_v4(), AGENT.to_string());

    h.scraper.tick().await;
    h.scraper.tick().await;
    h.scraper.tick().await;

    assert_eq!(h.sink.emitted(), vec![Some(42)]);
    assert_eq!(
        h.samples.observed(),
        vec![
            ContextSample::Reading {
                session_id: h
                    .scraper
                    .registered
                    .lock()
                    .unwrap()
                    .keys()
                    .next()
                    .copied()
                    .unwrap(),
                percent: 42,
            };
            3
        ]
    );
}

/// No monotonicity: `/clear` and `/compact` are the whole point of watching this number.
#[tokio::test]
async fn a_decrease_is_emitted() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(80)]),
            ScreenRowsRead::Rows(vec![row(12)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    h.scraper
        .register_session(Uuid::new_v4(), AGENT.to_string());

    h.scraper.tick().await;
    h.scraper.tick().await;

    assert_eq!(h.sink.emitted(), vec![Some(80), Some(12)]);
}

/// Two sessions of the same agent share a pattern and nothing else. Each gets its own
/// entry and its own first emit - which is a thing production does every time a user
/// launches a second Claude.
#[tokio::test]
async fn a_second_session_for_the_same_agent_gets_its_own_entry_and_first_emit() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    h.scraper
        .register_session(Uuid::new_v4(), AGENT.to_string());
    h.scraper
        .register_session(Uuid::new_v4(), AGENT.to_string());

    h.scraper.tick().await;

    assert_eq!(
        h.sink.emitted(),
        vec![Some(42), Some(42)],
        "both sessions are new to the badge, so both emit"
    );
    assert_eq!(h.scraper.compile_count(), 1, "one agent, one compile");
}

// ── #1088: the coalesced per-tick persist hand-off ──

#[tokio::test]
async fn a_changed_reading_commits_once_and_is_independent_of_badge_and_samples() {
    let h = harness(
        RowsFake::scripted(vec![ScreenRowsRead::Rows(vec![row(42)])]),
        PatternFake::with(AGENT, CLAUDE),
    );
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());

    h.scraper.tick().await;

    assert_eq!(h.persist.commits(), vec![vec![(id, Some(42))]]);
    // The badge and sample paths still behave exactly as before.
    assert_eq!(h.sink.emitted(), vec![Some(42)]);
    assert_eq!(h.samples.observed().len(), 1);
}

#[tokio::test]
async fn an_unchanged_reading_does_not_commit_again() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());

    h.scraper.tick().await;
    assert_eq!(
        h.persist.commits(),
        vec![vec![(id, Some(42))]],
        "the first reading commits once"
    );

    // Second tick: reading unchanged => change-gate suppresses => no new commit.
    h.scraper.tick().await;
    assert_eq!(
        h.persist.commits().len(),
        1,
        "an unchanged tick performs no commit (idle app => ~0 writes)"
    );
}

#[tokio::test]
async fn two_sessions_changing_in_one_tick_coalesce_into_one_commit() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::Rows(vec![row(42)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    h.scraper.register_session(id1, AGENT.to_string());
    h.scraper.register_session(id2, AGENT.to_string());

    h.scraper.tick().await;

    let commits = h.persist.commits();
    assert_eq!(commits.len(), 1, "coalesced: one commit for the whole tick");
    let committed = &commits[0];
    assert_eq!(committed.len(), 2, "both changed sessions in one commit");
    assert!(committed.contains(&(id1, Some(42))));
    assert!(committed.contains(&(id2, Some(42))));
}

#[tokio::test]
async fn a_cleared_reading_commits_none_once() {
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(42)]),
            ScreenRowsRead::SessionOver,
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    let id = Uuid::new_v4();
    h.scraper.register_session(id, AGENT.to_string());

    h.scraper.tick().await;
    h.scraper.tick().await;

    assert_eq!(
        h.persist.commits(),
        vec![vec![(id, Some(42))], vec![(id, None)]],
        "Some(42) then a single (id, None) on the clearing tick"
    );
}

#[tokio::test]
async fn sorted_sample_order_rotates_one_position_each_tick() {
    let ids = [Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)];
    let h = harness(
        RowsFake::scripted(vec![
            ScreenRowsRead::Rows(vec![row(10)]),
            ScreenRowsRead::Rows(vec![row(20)]),
            ScreenRowsRead::Rows(vec![row(30)]),
            ScreenRowsRead::Rows(vec![row(40)]),
            ScreenRowsRead::Rows(vec![row(50)]),
            ScreenRowsRead::Rows(vec![row(60)]),
        ]),
        PatternFake::with(AGENT, CLAUDE),
    );
    for id in ids {
        h.scraper.register_session(id, AGENT.to_string());
    }

    h.scraper.tick().await;
    h.scraper.tick().await;

    let observed_ids: Vec<Uuid> = h
        .samples
        .observed()
        .into_iter()
        .map(|sample| match sample {
            ContextSample::Reading { session_id, .. }
            | ContextSample::Unavailable { session_id }
            | ContextSample::SessionOver { session_id } => session_id,
        })
        .collect();
    assert_eq!(
        observed_ids,
        vec![ids[0], ids[1], ids[2], ids[1], ids[2], ids[0]]
    );
}

#[test]
fn none_serializes_as_an_explicit_null_not_an_absent_key() {
    let json = serde_json::to_string(&ContextUsagePayload {
        session_id: "3f2a".to_string(),
        percent: None,
    })
    .expect("serializes");
    assert_eq!(json, r#"{"sessionId":"3f2a","percent":null}"#);

    let json = serde_json::to_string(&ContextUsagePayload {
        session_id: "3f2a".to_string(),
        percent: Some(42),
    })
    .expect("serializes");
    assert_eq!(json, r#"{"sessionId":"3f2a","percent":42}"#);
}
