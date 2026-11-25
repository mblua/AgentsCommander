//! Paused-clock driver for one bounded selection close (#1234).
//!
//! The parent prepares the domain and the sweep. This leaf owns the pinned close future,
//! the Tokio schedule, the receipts of each product wait, the attributable bound `C`,
//! the physical watchdog and the join of every handle still owned after the close returns.

use super::*;
use crate::pty::container_backend::ShutdownStage;
use crate::session::selection::shutdown_cleanup::{CloseProbe, CLOSE_PROBE};
use crate::session::selection::task::SelectionTask;
use futures::task::waker;
use std::sync::atomic::AtomicU64;
use std::sync::Condvar;
use std::task::{Context, Poll};

const CAPPED_LIMIT: Duration = Duration::from_millis(750);
const NORMAL_LIMIT: Duration = Duration::from_millis(1000);
const STEP: Duration = Duration::from_millis(2);

pub(super) fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

/// Await the parent asks the probe to perform at the common return site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReturnWait {
    TokioSleep(Duration),
    TokioSleepUntil(Duration),
    Native(Duration),
}

/// One explicit Tokio read point, absolute from t0 or relative to the return wait arm.
#[derive(Clone, Copy, Debug)]
pub(super) enum ReadAt {
    FromStart(Duration),
    FromArm(Duration),
}

/// Which close leg the fixture holds past its deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hold {
    None,
    Rollback,
    Drain,
}

pub(super) struct CloseCase {
    pub(super) capped: bool,
    pub(super) hold: Hold,
    pub(super) wait: Option<SiteWait>,
    pub(super) schedule: Vec<ReadAt>,
}

impl CloseCase {
    pub(super) fn nominal(capped: bool) -> Self {
        Self {
            capped,
            hold: Hold::None,
            wait: None,
            schedule: Vec::new(),
        }
    }
}

/// Return-bound pairs and totals: (capped, site wait, read schedule, expected C, expected L).
pub(super) type BoundCase = (bool, SiteWait, Vec<ReadAt>, u64, Option<u64>);

pub(super) fn return_bound_cases() -> Vec<BoundCase> {
    use ReadAt::{FromArm, FromStart};
    use ReturnWait::{Native, TokioSleep, TokioSleepUntil};
    let abs = |a, b| vec![FromStart(ms(a)), FromStart(ms(b))];
    let rel = |a, b| vec![FromArm(ms(a)), FromArm(ms(b))];
    // (capped, wait, read schedule, expected C in ms, expected L in ms)
    let cases = vec![
        (
            true,
            TokioSleepUntil(ms(750)),
            abs(748, 752),
            750,
            Some(752),
        ),
        (
            true,
            TokioSleepUntil(ms(751)),
            abs(748, 752),
            751,
            Some(752),
        ),
        (
            false,
            TokioSleepUntil(ms(1000)),
            abs(998, 1002),
            1000,
            Some(1002),
        ),
        (
            false,
            TokioSleepUntil(ms(1001)),
            abs(998, 1002),
            1001,
            Some(1002),
        ),
        (true, TokioSleep(ms(600)), rel(598, 602), 750, None),
        (true, TokioSleep(ms(601)), rel(598, 602), 751, None),
        (false, TokioSleep(ms(1000)), rel(998, 1002), 1000, None),
        (false, TokioSleep(ms(1001)), rel(998, 1002), 1001, None),
        (true, Native(ms(600)), Vec::new(), 750, None),
        (true, Native(ms(601)), Vec::new(), 751, None),
        (false, Native(ms(1000)), Vec::new(), 1000, None),
        (false, Native(ms(1001)), Vec::new(), 1001, None),
    ];
    let mut cases: Vec<_> = cases
        .into_iter()
        .map(|(capped, wait, schedule, c, l)| {
            (capped, SiteWait::inline("return", wait), schedule, c, l)
        })
        .collect();
    for request in [400, 800, 1100] {
        for capped in [true, false] {
            let prefix = if capped { 150 } else { 0 };
            let absolute = TokioSleepUntil(ms(request));
            for wait in [TokioSleep(ms(request)), Native(ms(request))] {
                let return_site = SiteWait::inline("return", wait);
                cases.push((capped, return_site, Vec::new(), prefix + request, None));
            }
            let return_site = SiteWait::inline("return", absolute);
            cases.push((capped, return_site, Vec::new(), request.max(prefix), None));
            // The same totals from children the close joins between rollback and drain.
            for wait in [TokioSleep(ms(request)), Native(ms(request))] {
                let child = SiteWait::children("between-joins", vec![wait]);
                cases.push((capped, child, Vec::new(), prefix + request, None));
            }
            let child = SiteWait::children("between-joins", vec![absolute]);
            cases.push((capped, child, Vec::new(), request.max(prefix), None));
        }
    }
    // 1 ms: one Tokio sleep at each site, and a native/Tokio child pair between joins.
    for capped in [true, false] {
        let expected = if capped { 151 } else { 1 };
        for site in ["between-joins", "return"] {
            let wait = SiteWait::inline(site, TokioSleep(ms(1)));
            cases.push((capped, wait, Vec::new(), expected, None));
        }
        let pair = SiteWait::children("between-joins", vec![Native(ms(1)), TokioSleep(ms(1))]);
        cases.push((capped, pair, Vec::new(), expected, None));
    }
    cases
}

use super::receipts::{attributable, FixtureProbe, ProbeWorker, SiteWait};
use super::watchdog::{physical_failure, Gate, Stamps, StopOnPanic, WakeFlag, Watchdog};

// ---------------------------------------------------------------- driver

pub(super) struct CloseRun {
    pub(super) report: Option<SelectionShutdownReport>,
    pub(super) c: Result<Duration, String>,
    pub(super) limit: Duration,
    pub(super) l: Option<Duration>,
    pub(super) begin_arg: Option<Instant>,
    pub(super) drain_arg: Option<Instant>,
    pub(super) expected_arg: Instant,
    pub(super) transfers: Vec<(&'static str, tokio::task::Id)>,
    pub(super) checkpoints: Vec<String>,
    pub(super) finalizer: Result<Result<SessionInfo, String>, JoinError>,
    pub(super) post: Vec<String>,
    pub(super) first_failure: Option<String>,
}

impl CloseRun {
    pub(super) fn within_bound(&self) -> bool {
        self.c.as_ref().is_ok_and(|c| *c <= self.limit)
    }
}

struct Driver<'a> {
    close: Pin<&'a mut (dyn Future<Output = SelectionShutdownReport> + Send + 'a)>,
    probe: Arc<FixtureProbe>,
    watchdog: Arc<Watchdog>,
    t0: tokio::time::Instant,
    ready_seq: Option<u64>,
    checkpoints: Vec<String>,
}

impl Driver<'_> {
    fn now(&self) -> Duration {
        tokio::time::Instant::now() - self.t0
    }

    /// One poll with the external mark cleared; R is stamped inside the Ready poll.
    fn poll(&mut self) -> Option<SelectionShutdownReport> {
        self.probe.external.store(false, Ordering::SeqCst);
        self.probe.wake.woke.store(false, Ordering::SeqCst);
        let waker = waker(Arc::clone(&self.probe.wake));
        let polled = self.close.as_mut().poll(&mut Context::from_waker(&waker));
        let Poll::Ready(report) = polled else {
            return None;
        };
        let now = Instant::now();
        self.ready_seq = Some(self.probe.next());
        self.watchdog.stamp(|stamps| stamps.r = Some(now));
        Some(report)
    }

    /// Sentinel registered before the jump, delivered by the wheel, then a fresh poll.
    async fn tick_to(&mut self, target: Duration) -> Option<SelectionShutdownReport> {
        let now = self.now();
        if target > now + ms(1) {
            let mut sentinel = Box::pin(tokio::time::sleep_until(self.t0 + target - ms(1)));
            assert!(futures::poll!(sentinel.as_mut()).is_pending());
            tokio::time::advance(target - now).await;
            sentinel.await;
        }
        self.poll()
    }

    fn mark(&mut self, line: String) {
        self.checkpoints.push(format!("{:?} {line}", self.now()));
    }

    /// Holds Tokio while the close waits on an external join; ticks only internal waits.
    async fn settle(
        &mut self,
        schedule: &mut Vec<ReadAt>,
        stop: &dyn Fn(&FixtureProbe) -> bool,
    ) -> Option<SelectionShutdownReport> {
        loop {
            if let Some(report) = self.poll() {
                return Some(report);
            }
            if stop(&self.probe) || self.watchdog.first_failure().is_some() {
                return None;
            }
            let child_timer = self.probe.child_timers.load(Ordering::SeqCst) > 0;
            if self.probe.external.load(Ordering::SeqCst) && !child_timer {
                self.probe.wake.notify.notified().await;
                continue;
            }
            for _ in 0..16 {
                tokio::task::yield_now().await;
            }
            if self.probe.wake.woke.load(Ordering::SeqCst) {
                continue;
            }
            let arm = self.probe.log().receipts.last().map(|receipt| receipt.q);
            let target = match schedule.first().copied() {
                Some(ReadAt::FromStart(at)) => at,
                Some(ReadAt::FromArm(after)) => arm.map_or(self.now() + STEP, |q| q + after),
                None => self.now() + STEP,
            };
            if !schedule.is_empty()
                && (arm.is_some() || matches!(schedule[0], ReadAt::FromStart(_)))
            {
                schedule.remove(0);
            }
            if let Some(report) = self.tick_to(target).await {
                return Some(report);
            }
        }
    }
}

/// Everything the leaf needs from the parent's prepared domain.
pub(super) struct Domain<'a> {
    pub(super) coordinator: &'a SelectionCoordinator,
    pub(super) manager: &'a Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) container_backend: &'a Arc<ContainerTransportBackend>,
    pub(super) ticket: CreateFinalizationTicket,
    pub(super) restore_guard: Option<RestoreBarrierGuard>,
}

fn install_worker_probe(coordinator: &SelectionCoordinator, probe: &Arc<FixtureProbe>) {
    let mut slot = coordinator
        .inner
        .worker
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let inner = slot.take().expect("started coordinator owns its worker");
    *slot = Some(Box::pin(ProbeWorker {
        inner,
        probe: Arc::clone(probe),
    }));
}

/// Real finalizer, spawned once; its first Pending is acknowledged after enqueueing.
async fn spawn_finalizer(
    ticket: CreateFinalizationTicket,
) -> JoinHandle<Result<SessionInfo, String>> {
    let (queued, queued_rx) = oneshot::channel();
    let finalizer = tokio::spawn(async move {
        let mut finalize = Box::pin(ticket.finalize(Vec::new()));
        let mut queued = Some(queued);
        std::future::poll_fn(|cx| {
            let polled = finalize.as_mut().poll(cx);
            if polled.is_pending() {
                if let Some(queued) = queued.take() {
                    let _ = queued.send(());
                }
            }
            polled
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), queued_rx)
        .await
        .expect("finalizer queues its envelope")
        .expect("finalizer acknowledges its first Pending");
    finalizer
}

fn assert_domain_ready(coordinator: &SelectionCoordinator, backend: &ContainerTransportBackend) {
    let inner = &coordinator.inner;
    assert_eq!(
        backend.shutdown_observation_for_test(),
        (false, 0, 0, 0),
        "backend accepts with no producers, work or workers"
    );
    assert_eq!(coordinator.fixture_native_work_counts(), Some((0, 0)));
    assert!(inner.retained_workers.lock().unwrap().is_empty());
    assert!(inner.retained_rollback_tasks.lock().unwrap().is_empty());
    assert!(inner.retained_cleanup_tasks.lock().unwrap().is_empty());
}

async fn drive_worker_phase(
    driver: &mut Driver<'_>,
    coordinator: &SelectionCoordinator,
    restore_guard: &mut Option<RestoreBarrierGuard>,
    probe: &FixtureProbe,
    capped: bool,
) -> Option<SelectionShutdownReport> {
    let mut report;
    let worker_state = |probe: &FixtureProbe| {
        let log = probe.log();
        (log.worker_aborts, log.worker_ready.len())
    };
    if capped {
        report = driver.tick_to(ms(149)).await;
        assert!(report.is_none(), "capped close pending at 149 ms");
        assert_eq!(
            worker_state(probe),
            (0, 0),
            "worker pending, no abort at 149 ms"
        );
        driver.mark("worker pending at 149".into());
        report = driver.tick_to(ms(152)).await;
        assert!(report.is_none(), "capped close pending at 152 ms");
        assert_eq!(
            probe.log().worker_aborts,
            1,
            "one delegated abort at 152 ms"
        );
        driver.mark("worker aborted at 152".into());
        report = driver
            .settle(&mut Vec::new(), &|probe| {
                !probe.log().worker_ready.is_empty()
            })
            .await;
        let log = probe.log();
        assert!(
            matches!(log.worker_ready.as_slice(), [Err(_)]),
            "aborted worker joins once with an error: {:?}",
            log.worker_ready
        );
        drop(log);
        assert!(coordinator.inner.worker.lock().unwrap().is_none());
        assert!(coordinator
            .inner
            .retained_workers
            .lock()
            .unwrap()
            .is_empty());
    } else {
        report = driver.tick_to(ms(300)).await;
        assert!(report.is_none(), "normal close pending at 300 ms");
        assert_eq!(
            worker_state(probe),
            (0, 0),
            "normal worker pending, no abort"
        );
        assert!(
            probe.log().joins.is_empty(),
            "no cleanup join before the worker"
        );
        driver.mark("worker held at 300".into());
        if let Some(guard) = restore_guard.take() {
            guard.finish();
        }
        report = driver
            .settle(&mut Vec::new(), &|probe| {
                !probe.log().worker_ready.is_empty()
            })
            .await;
        assert!(
            matches!(probe.log().worker_ready.as_slice(), [Ok(())]),
            "released worker returns Ok"
        );
    }
    report
}

async fn drive_held_join(
    driver: &mut Driver<'_>,
    coordinator: &SelectionCoordinator,
    probe: &FixtureProbe,
    hold: Hold,
) -> Option<SelectionShutdownReport> {
    let mut report;
    let site = if hold == Hold::Rollback {
        "rollback"
    } else {
        "drain"
    };
    let entered = move |probe: &FixtureProbe| {
        let log = probe.log();
        log.joins.get(site).is_some_and(|(_, pending)| *pending)
            && (site == "rollback"
                || log
                    .stages
                    .iter()
                    .any(|s| matches!(s, ShutdownStage::Drain(_))))
    };
    report = driver.settle(&mut Vec::new(), &entered).await;
    assert!(
        report.is_none(),
        "{site} join still pending under the close"
    );
    report = driver.tick_to(ms(199)).await;
    assert!(report.is_none(), "close pending at 199 ms with {site} held");
    let inner = &coordinator.inner;
    assert!(inner.retained_rollback_tasks.lock().unwrap().is_empty());
    assert!(inner.retained_cleanup_tasks.lock().unwrap().is_empty());
    {
        let log = probe.log();
        assert!(log.joins[site].1, "{site} join pending at 199 ms");
        if site == "rollback" {
            assert!(!log.joins.contains_key("drain"), "drain not entered");
        } else {
            assert!(!log.joins["rollback"].1, "rollback joined before the drain");
        }
    }
    driver.mark(format!("{site} pending at 199"));
    report = driver.tick_to(ms(202)).await;
    driver.mark(format!("{site} expired at 202 ready={}", report.is_some()));
    let held_id = probe.log().joins[site].0;
    let retained: Vec<_> = if site == "rollback" {
        inner
            .retained_rollback_tasks
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.id())
            .collect()
    } else {
        inner
            .retained_cleanup_tasks
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.id())
            .collect()
    };
    assert_eq!(
        retained,
        vec![held_id],
        "exactly the held {site} join is retained"
    );
    if report.is_none() {
        // A drain entered with zero remaining expires on the next wheel tick.
        report = driver.tick_to(ms(204)).await;
    }
    assert!(report.is_some(), "close returns after {site} expiry");
    report
}

fn stage_args(probe: &FixtureProbe) -> (Option<Instant>, Option<Instant>) {
    let stages = probe.log().stages.clone();
    let begin_arg = stages.iter().find_map(|s| match s {
        ShutdownStage::Begin(at) => Some(*at),
        ShutdownStage::Drain(_) => None,
    });
    let drain_arg = stages.iter().find_map(|s| match s {
        ShutdownStage::Drain(at) => Some(*at),
        ShutdownStage::Begin(_) => None,
    });
    (begin_arg, drain_arg)
}

fn close_budget(capped: bool) -> Duration {
    if capped {
        ms(200)
    } else {
        Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS)
    }
}

fn new_probe(wait: Option<SiteWait>, wake: &Arc<WakeFlag>) -> Arc<FixtureProbe> {
    Arc::new_cyclic(|me| FixtureProbe {
        me: me.clone(),
        log: Mutex::default(),
        external: AtomicBool::new(false),
        child_timers: std::sync::atomic::AtomicUsize::new(0),
        seq: AtomicU64::new(0),
        t0: Mutex::new(None),
        wait,
        wake: Arc::clone(wake),
        watchdog: Mutex::new(None),
    })
}

fn install_stage_observer(
    backend: &ContainerTransportBackend,
    probe: &Arc<FixtureProbe>,
    gate: Option<Arc<Gate>>,
) {
    let observer_probe = Arc::clone(probe);
    backend.set_shutdown_stage_observer_for_test(Arc::new(move |stage| {
        observer_probe.stage(stage);
        if let (ShutdownStage::Drain(_), Some(gate)) = (stage, gate.as_ref()) {
            gate.wait();
        }
    }));
}

/// Start the watchdog thread; the receiver fires once it is watching.
fn start_watchdog(
    probe: &FixtureProbe,
    drain_gate: &Arc<Gate>,
    wake: &Arc<WakeFlag>,
    case: String,
) -> (
    Arc<Watchdog>,
    (tokio::task::JoinHandle<()>, oneshot::Receiver<()>),
) {
    let watchdog = Arc::new(Watchdog {
        state: Mutex::default(),
        changed: Condvar::new(),
        gates: vec![Arc::clone(drain_gate)],
        wake: Arc::clone(wake),
        case,
    });
    *probe.watchdog.lock().unwrap() = Some(Arc::clone(&watchdog));
    let (started, started_rx) = oneshot::channel();
    let watch = Arc::clone(&watchdog);
    let watcher = tokio::task::spawn_blocking(move || watch.watch(started));
    (watchdog, (watcher, started_rx))
}

type Attribution = (
    Result<Duration, String>,
    Vec<(&'static str, tokio::task::Id)>,
);

fn attribution(
    probe: &FixtureProbe,
    worker_phase: Duration,
    budget: Duration,
    ready_seq: u64,
) -> Attribution {
    let log = probe.log();
    let worker_expired = log.worker_aborts > 0;
    let shared_expired = !log.transfers.is_empty();
    let c = attributable(
        worker_expired.then_some(worker_phase),
        shared_expired.then_some(budget),
        &log.receipts,
        log.drain_seq,
        ready_seq,
    );
    (c, log.transfers.clone())
}

fn assert_first_poll(
    report: &Option<SelectionShutdownReport>,
    backend: &ContainerTransportBackend,
    probe: &FixtureProbe,
) {
    assert!(report.is_none(), "first poll returns Pending");
    assert!(backend.shutdown_observation_for_test().0, "backend sealed");
    assert!(probe.log().worker_polls >= 1, "worker polled by the close");
}

pub(super) async fn drive_close(mut domain: Domain<'_>, case: CloseCase) -> CloseRun {
    let CloseCase {
        capped,
        hold,
        wait,
        mut schedule,
    } = case;
    let budget = close_budget(capped);
    let worker_phase = budget - (budget / 4).min(SHUTDOWN_FINALIZATION_RESERVE_MAX);
    let wake = Arc::new(WakeFlag::default());
    let probe = new_probe(wait.clone(), &wake);
    let drain_gate = Arc::new(Gate::default());
    let observer_gate = (hold == Hold::Drain).then(|| Arc::clone(&drain_gate));
    install_stage_observer(domain.container_backend, &probe, observer_gate);
    install_worker_probe(domain.coordinator, &probe);
    let mut finalizer = spawn_finalizer(domain.ticket).await;
    let finalizer_id = finalizer.id();
    assert_domain_ready(domain.coordinator, domain.container_backend);
    let case = format!("capped={capped} hold={hold:?} wait={wait:?}");
    let (watchdog, watcher) = start_watchdog(&probe, &drain_gate, &wake, case);
    let _stop_on_panic = StopOnPanic(Arc::clone(&watchdog));
    watcher.1.await.expect("watchdog acknowledges its start");
    let watcher = watcher.0;
    let manager_hold = if hold == Hold::Rollback {
        Some(domain.manager.write().await)
    } else {
        None
    };

    tokio::time::pause();
    let coordinator = domain.coordinator.clone();
    let probe_scope: Arc<dyn CloseProbe> = probe.clone();
    let close = CLOSE_PROBE.scope(probe_scope, async move {
        if capped {
            coordinator.close_and_join_with_budget(budget).await
        } else {
            coordinator.close_and_join().await
        }
    });
    let mut close = std::pin::pin!(close);
    let t0 = tokio::time::Instant::now();
    *probe.t0.lock().unwrap() = Some(t0);
    let expected_arg = t0.into_std() + budget;
    watchdog.stamp(|stamps| stamps.s = Some(Instant::now()));
    let mut driver = Driver {
        close: close.as_mut(),
        probe: Arc::clone(&probe),
        watchdog: Arc::clone(&watchdog),
        t0,
        ready_seq: None,
        checkpoints: Vec::new(),
    };
    let mut report = driver.poll();
    assert_first_poll(&report, domain.container_backend, &probe);

    report = drive_worker_phase(
        &mut driver,
        domain.coordinator,
        &mut domain.restore_guard,
        &probe,
        capped,
    )
    .await;

    if report.is_none() && hold != Hold::None {
        report = drive_held_join(&mut driver, domain.coordinator, &probe, hold).await;
    } else if report.is_none() {
        report = driver.settle(&mut schedule, &|_| false).await;
    }
    let l = report.as_ref().map(|_| driver.now());
    let ready_seq = driver.ready_seq.unwrap_or(u64::MAX);
    let checkpoints = std::mem::take(&mut driver.checkpoints);
    drop(driver);

    let (c, transfers) = attribution(&probe, worker_phase, budget, ready_seq);

    // Release product guards, resume Tokio and join every owned handle once.
    drop(manager_hold);
    drain_gate.open();
    if let Some(guard) = domain.restore_guard.take() {
        guard.finish();
    }
    tokio::time::resume();
    let post = join_after_return(domain.coordinator, &watchdog, &mut finalizer, finalizer_id).await;
    let finalizer_result = post.1;
    // A drain entered with zero remaining may run after R; read both receivers now.
    let (begin_arg, drain_arg) = stage_args(&probe);
    watchdog.finish();
    watcher.await.expect("watchdog thread joins");
    CloseRun {
        report,
        c,
        limit: if capped { CAPPED_LIMIT } else { NORMAL_LIMIT },
        l,
        begin_arg,
        drain_arg,
        expected_arg,
        transfers,
        checkpoints,
        finalizer: finalizer_result,
        post: post.0,
        first_failure: watchdog.first_failure(),
    }
}

enum Owned {
    Worker(Pin<Box<dyn SelectionTask>>),
    Rollback(JoinHandle<Vec<String>>),
    Drain(JoinHandle<ContainerShutdownReport>),
}

fn take_owned(coordinator: &SelectionCoordinator) -> Vec<(String, Owned)> {
    let inner = &coordinator.inner;
    let workers = std::mem::take(&mut *inner.retained_workers.lock().unwrap());
    let rollbacks = std::mem::take(&mut *inner.retained_rollback_tasks.lock().unwrap());
    let drains = std::mem::take(&mut *inner.retained_cleanup_tasks.lock().unwrap());
    let mut owned = Vec::new();
    for (index, worker) in workers.into_iter().enumerate() {
        owned.push((format!("worker#{index}"), Owned::Worker(worker)));
    }
    for task in rollbacks {
        owned.push((format!("rollback:{}", task.id()), Owned::Rollback(task)));
    }
    for task in drains {
        owned.push((format!("drain:{}", task.id()), Owned::Drain(task)));
    }
    owned
}

/// Polls the finalizer and every taken handle concurrently, stamping J in the consuming poll.
async fn join_after_return(
    coordinator: &SelectionCoordinator,
    watchdog: &Watchdog,
    finalizer: &mut JoinHandle<Result<SessionInfo, String>>,
    finalizer_id: tokio::task::Id,
) -> (Vec<String>, Result<Result<SessionInfo, String>, JoinError>) {
    let mut owned: Vec<(String, Option<Owned>)> = take_owned(coordinator)
        .into_iter()
        .map(|(id, handle)| (id, Some(handle)))
        .collect();
    let finalizer_key = format!("finalizer:{finalizer_id}");
    watchdog.stamp(|stamps| {
        stamps.joins = owned.iter().map(|(id, _)| (id.clone(), None)).collect();
        stamps.joins.push((finalizer_key.clone(), None));
    });
    let mut results = Vec::new();
    let mut finalizer_result = None;
    let mut finalizer_aborted = false;
    // Re-polls every 10 ms so a watchdog failure can abort a held finalizer.
    let mut recheck = tokio::time::interval(ms(10));
    std::future::poll_fn(|cx| {
        while recheck.poll_tick(cx).is_ready() {}
        for (id, slot) in owned.iter_mut() {
            let Some(handle) = slot.as_mut() else {
                continue;
            };
            let joined = match handle {
                Owned::Worker(task) => task.as_mut().poll(cx).map(|r| format!("{r:?}")),
                Owned::Rollback(task) => Pin::new(task).poll(cx).map(|r| format!("{r:?}")),
                Owned::Drain(task) => Pin::new(task)
                    .poll(cx)
                    .map(|r| format!("{:?}", r.map(|report| report.terminal))),
            };
            if let Poll::Ready(value) = joined {
                watchdog.join_stamp(id);
                results.push(format!("{id}={value}"));
                *slot = None;
            }
        }
        if finalizer_result.is_none() {
            if !finalizer_aborted
                && watchdog
                    .first_failure()
                    .is_some_and(|f| f.contains(&finalizer_key))
            {
                finalizer.abort();
                finalizer_aborted = true;
            }
            if let Poll::Ready(value) = Pin::new(&mut *finalizer).poll(cx) {
                watchdog.join_stamp(&finalizer_key);
                finalizer_result = Some(value);
            }
        }
        if finalizer_result.is_some() && owned.iter().all(|(_, slot)| slot.is_none()) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    (results, finalizer_result.expect("finalizer joined"))
}

// ---------------------------------------------------------------- physical evaluator unit

#[test]
fn pending_container_watchdog_uses_producer_stamps() {
    let base = Instant::now();
    let at = |value: u64| base + ms(value);
    let stamps = |a: Option<u64>, r: Option<u64>| Stamps {
        s: Some(base),
        a: a.map(at),
        r: r.map(at),
        joins: Vec::new(),
    };
    let read = at(3000);
    assert!(physical_failure(&stamps(Some(1010), Some(1100)), read)
        .is_some_and(|f| f.starts_with("ack late")));
    assert!(physical_failure(&stamps(Some(100), Some(2110)), read)
        .is_some_and(|f| f.starts_with("return late")));
    assert_eq!(physical_failure(&stamps(Some(100), Some(1900)), read), None);
    // Inclusive edges.
    assert_eq!(
        physical_failure(&stamps(Some(1000), Some(3000)), read),
        None
    );
    // An absent event is a failure once its limit passed, even if it arrives later.
    assert!(physical_failure(&stamps(None, None), at(1001))
        .is_some_and(|f| f.starts_with("ack missing")));
    assert_eq!(physical_failure(&stamps(None, None), at(1000)), None);
    let dog = Watchdog {
        state: Mutex::default(),
        changed: Condvar::new(),
        gates: Vec::new(),
        wake: Arc::default(),
        case: "unit".into(),
    };
    {
        let mut state = dog.lock();
        state.stamps = stamps(None, None);
        let finding = physical_failure(&state.stamps, at(1001)).unwrap();
        dog.fail(&mut state, finding, at(1001));
    }
    dog.stamp(|stamps| stamps.a = Some(at(1050)));
    assert!(dog
        .first_failure()
        .is_some_and(|f| f.contains("ack missing")));
    // Post joins: a punctual stamp read late passes; a late stamp fails with its id.
    let mut post = stamps(Some(5), Some(210));
    post.joins = vec![("drain:17".into(), Some(at(210 + 4999)))];
    assert_eq!(physical_failure(&post, at(210 + 5100)), None);
    post.joins = vec![("drain:17".into(), Some(at(210 + 5001)))];
    assert!(physical_failure(&post, at(210 + 5100)).is_some_and(|f| f.contains("drain:17")));
    post.joins = vec![("finalizer:23".into(), None)];
    assert!(physical_failure(&post, at(210 + 5001)).is_some_and(|f| f.contains("finalizer:23")));
    assert_eq!(physical_failure(&post, at(210 + 5000)), None);
}
