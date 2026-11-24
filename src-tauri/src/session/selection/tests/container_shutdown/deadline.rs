//! Paused-clock driver for one bounded selection close (#1234).
//!
//! The parent prepares the domain and the sweep. This leaf owns the pinned close future,
//! the Tokio schedule, the receipts of each product wait, the attributable bound `C`,
//! the physical watchdog and the join of every handle still owned after the close returns.

use super::*;
use crate::pty::container_backend::ShutdownStage;
use crate::session::selection::shutdown_cleanup::probe::{CloseProbe, CLOSE_PROBE};
use crate::session::selection::task::SelectionTask;
use futures::future::BoxFuture;
use futures::task::{waker, ArcWake};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::Condvar;
use std::task::{Context, Poll};

const CAPPED_LIMIT: Duration = Duration::from_millis(750);
const NORMAL_LIMIT: Duration = Duration::from_millis(1000);
const ACK_LIMIT: Duration = Duration::from_secs(1);
const RETURN_LIMIT: Duration = Duration::from_secs(2);
const POST_LIMIT: Duration = Duration::from_secs(5);
const RESCUE_LIMIT: Duration = Duration::from_secs(5);
const STEP: Duration = Duration::from_millis(2);

fn ms(value: u64) -> Duration {
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
    pub(super) wait: Option<ReturnWait>,
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

// ---------------------------------------------------------------- physical watchdog

/// Physical stamps the evaluator reads; `joins` are the handles owned after R.
#[derive(Clone, Debug, Default)]
pub(super) struct Stamps {
    pub(super) s: Option<Instant>,
    pub(super) a: Option<Instant>,
    pub(super) r: Option<Instant>,
    pub(super) joins: Vec<(String, Option<Instant>)>,
}

/// First physical failure of the stamps at `now`, with inclusive limits.
pub(super) fn physical_failure(stamps: &Stamps, now: Instant) -> Option<String> {
    let s = stamps.s?;
    let ack = s + ACK_LIMIT;
    let a = match stamps.a {
        Some(a) if a > ack => return Some(format!("ack late by {:?}", a - ack)),
        Some(a) => a,
        None if now > ack => return Some("ack missing at S+1s".to_string()),
        None => return None,
    };
    let ret = a + RETURN_LIMIT;
    let r = match stamps.r {
        Some(r) if r > ret => return Some(format!("return late by {:?}", r - ret)),
        Some(r) => r,
        None if now > ret => return Some("return missing at A+2s".to_string()),
        None => return None,
    };
    let post = r + POST_LIMIT;
    stamps.joins.iter().find_map(|(id, joined)| match joined {
        Some(j) if *j > post => Some(format!("post/{id} joined late by {:?}", *j - post)),
        None if now > post => Some(format!("post/{id} still pending at R+5s")),
        _ => None,
    })
}

#[derive(Default)]
pub(super) struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    pub(super) fn open(&self) {
        *self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*open {
            open = self
                .changed
                .wait(open)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

#[derive(Default)]
struct WatchState {
    stamps: Stamps,
    first_failure: Option<String>,
    findings: Vec<String>,
    rescue_at: Option<Instant>,
    done: bool,
}

/// Wakes the driver from a waker, the watchdog or a native thread; permits are kept.
#[derive(Default)]
pub(super) struct WakeFlag {
    woke: AtomicBool,
    notify: tokio::sync::Notify,
}

impl ArcWake for WakeFlag {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.woke.store(true, Ordering::SeqCst);
        arc_self.notify.notify_one();
    }
}

pub(super) struct Watchdog {
    state: Mutex<WatchState>,
    changed: Condvar,
    gates: Vec<Arc<Gate>>,
    wake: Arc<WakeFlag>,
    case: String,
}

impl Watchdog {
    fn lock(&self) -> std::sync::MutexGuard<'_, WatchState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn stamp(&self, update: impl FnOnce(&mut Stamps)) {
        update(&mut self.lock().stamps);
        self.changed.notify_all();
    }

    fn join_stamp(&self, id: &str) {
        let now = Instant::now();
        self.stamp(|stamps| {
            if let Some(slot) = stamps.joins.iter_mut().find(|(owned, _)| owned == id) {
                slot.1.get_or_insert(now);
            }
        });
    }

    pub(super) fn first_failure(&self) -> Option<String> {
        self.lock().first_failure.clone()
    }

    fn fail(&self, state: &mut WatchState, finding: String, at: Instant) {
        let line = format!(
            "case={} watchdog {finding} stamps={:?}",
            self.case, state.stamps
        );
        eprintln!("{line}");
        if state.first_failure.is_none() {
            state.first_failure = Some(line);
            state.rescue_at = Some(at + RESCUE_LIMIT);
            for gate in &self.gates {
                gate.open();
            }
        } else {
            state.findings.push(line);
        }
        WakeFlag::wake_by_ref(&self.wake);
    }

    /// Runs on a blocking thread, which also keeps Tokio from auto-advancing time.
    fn watch(&self, started: oneshot::Sender<()>) {
        let _ = started.send(());
        let mut state = self.lock();
        let mut reported = Vec::new();
        while !state.done {
            let now = Instant::now();
            if let Some(finding) = physical_failure(&state.stamps, now) {
                if !reported.contains(&finding) {
                    reported.push(finding.clone());
                    self.fail(&mut state, finding, now);
                }
            }
            if state.rescue_at.is_some_and(|at| now > at) {
                eprintln!(
                    "case={} rescue expired; first={:?} stamps={:?}",
                    self.case, state.first_failure, state.stamps
                );
                state.rescue_at = None;
            }
            state = self
                .changed
                .wait_timeout(state, ms(5))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    fn finish(&self) {
        let mut state = self.lock();
        if let Some(finding) = physical_failure(&state.stamps, Instant::now()) {
            if state.first_failure.is_none() {
                self.fail(&mut state, finding, Instant::now());
            }
        }
        state.done = true;
        self.changed.notify_all();
    }
}

// ---------------------------------------------------------------- receipts and probe

#[derive(Clone, Debug)]
pub(super) struct Receipt {
    pub(super) wait: ReturnWait,
    pub(super) arm_seq: u64,
    pub(super) q: Duration,
    pub(super) p: Option<Duration>,
    pub(super) release: Option<(u64, Duration)>,
    pub(super) native_waited: Option<Duration>,
    pub(super) consume_seq: Option<u64>,
}

#[derive(Default)]
struct ProbeLog {
    joins: BTreeMap<&'static str, (tokio::task::Id, bool)>,
    transfers: Vec<(&'static str, tokio::task::Id)>,
    receipts: Vec<Receipt>,
    stages: Vec<ShutdownStage>,
    worker_polls: usize,
    worker_aborts: usize,
    worker_ready: Vec<Result<(), String>>,
}

pub(super) struct FixtureProbe {
    log: Mutex<ProbeLog>,
    external: AtomicBool,
    seq: AtomicU64,
    t0: Mutex<Option<tokio::time::Instant>>,
    wait: Option<ReturnWait>,
    wake: Arc<WakeFlag>,
    watchdog: Mutex<Option<Arc<Watchdog>>>,
    me: std::sync::Weak<FixtureProbe>,
}

impl FixtureProbe {
    fn log(&self) -> std::sync::MutexGuard<'_, ProbeLog> {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn next(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }

    fn since_t0(&self) -> Duration {
        let t0 = self
            .t0
            .lock()
            .unwrap()
            .expect("t0 recorded before the first poll");
        tokio::time::Instant::now() - t0
    }

    fn stage(&self, stage: ShutdownStage) {
        if matches!(stage, ShutdownStage::Begin(_)) {
            if let Some(watchdog) = self.watchdog.lock().unwrap().clone() {
                watchdog.stamp(|stamps| {
                    stamps.a.get_or_insert_with(Instant::now);
                });
            }
        }
        self.log().stages.push(stage);
        WakeFlag::wake_by_ref(&self.wake);
    }

    fn arm(&self, wait: ReturnWait, p: Option<Duration>) -> usize {
        let receipt = Receipt {
            wait,
            arm_seq: self.next(),
            q: self.since_t0(),
            p,
            release: None,
            native_waited: None,
            consume_seq: None,
        };
        let mut log = self.log();
        log.receipts.push(receipt);
        log.receipts.len() - 1
    }

    fn released(&self, index: usize, native_waited: Option<Duration>) {
        let release = (self.next(), self.since_t0());
        let mut log = self.log();
        log.receipts[index].release = Some(release);
        log.receipts[index].native_waited = native_waited;
    }

    fn consumed(&self, index: usize) {
        let seq = self.next();
        self.log().receipts[index].consume_seq = Some(seq);
    }

    /// Tokio wait built in the arming poll; its deadline is read before it is pinned.
    async fn tokio_wait(self: Arc<Self>, wait: ReturnWait) {
        let t0 = self.t0.lock().unwrap().expect("t0 recorded");
        let q = tokio::time::Instant::now();
        let sleep = match wait {
            ReturnWait::TokioSleep(d) => tokio::time::sleep(d),
            ReturnWait::TokioSleepUntil(at) => tokio::time::sleep_until(t0 + at),
            ReturnWait::Native(_) => unreachable!("native wait is served by a thread"),
        };
        let p = sleep.deadline() - t0;
        if let ReturnWait::TokioSleep(d) = wait {
            assert_eq!(sleep.deadline() - q, d, "relative sleep deadline is q + d");
        }
        let index = self.arm(wait, Some(p));
        let mut sleep = Box::pin(sleep);
        let probe = Arc::clone(&self);
        let mut armed = false;
        std::future::poll_fn(|cx| match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => {
                probe.released(index, None);
                Poll::Ready(())
            }
            Poll::Pending => {
                if !std::mem::replace(&mut armed, true) {
                    WakeFlag::wake_by_ref(&probe.wake);
                }
                Poll::Pending
            }
        })
        .await;
        self.consumed(index);
    }

    /// Native wait: a service thread other than the watchdog opens the gate at entry+d.
    fn native_wait(&self, d: Duration) {
        let index = self.arm(ReturnWait::Native(d), None);
        let gate = Arc::new(Gate::default());
        let entry = Instant::now();
        let service_gate = Arc::clone(&gate);
        let service = std::thread::spawn(move || {
            std::thread::sleep((entry + d).saturating_duration_since(Instant::now()));
            let released = Instant::now();
            service_gate.open();
            released
        });
        gate.wait();
        let released = service.join().expect("native wait service joins");
        assert!(released - entry >= d, "native release precedes entry + d");
        self.released(index, Some(d));
        self.consumed(index);
    }
}

impl CloseProbe for FixtureProbe {
    fn join_polled(&self, site: &'static str, id: tokio::task::Id, pending: bool) {
        self.log().joins.insert(site, (id, pending));
        self.external.store(pending, Ordering::SeqCst);
    }

    fn transferred(&self, site: &'static str, id: tokio::task::Id) {
        self.log().transfers.push((site, id));
    }

    fn return_wait(&self) -> Option<BoxFuture<'static, ()>> {
        let wait = self.wait?;
        let probe = self.me.upgrade()?;
        Some(match wait {
            ReturnWait::Native(d) => Box::pin(async move { probe.native_wait(d) }),
            tokio_wait => Box::pin(probe.tokio_wait(tokio_wait)),
        })
    }
}

/// The same worker the coordinator owns, with poll and abort delegated and recorded.
struct ProbeWorker {
    inner: Pin<Box<dyn SelectionTask>>,
    probe: Arc<FixtureProbe>,
}

impl Future for ProbeWorker {
    type Output = Result<(), String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let polled = self.inner.as_mut().poll(cx);
        let mut log = self.probe.log();
        log.worker_polls += 1;
        if let Poll::Ready(result) = &polled {
            log.worker_ready.push(result.clone());
        }
        drop(log);
        self.probe
            .external
            .store(polled.is_pending(), Ordering::SeqCst);
        polled
    }
}

impl SelectionTask for ProbeWorker {
    fn abort(&self) {
        self.probe.log().worker_aborts += 1;
        self.inner.abort();
    }
}

// ---------------------------------------------------------------- bound evaluator

/// Attributable delay `C`: expired product deadlines and the waits that conditioned R.
pub(super) fn attributable(
    worker_phase: Option<Duration>,
    shared: Option<Duration>,
    receipts: &[Receipt],
    ready_seq: u64,
) -> Result<Duration, String> {
    let mut c = worker_phase
        .into_iter()
        .chain(shared)
        .max()
        .unwrap_or_default();
    for receipt in receipts {
        let release = receipt.release.ok_or("wait never released")?;
        let consumed = receipt.consume_seq.ok_or("wait never consumed")?;
        if !(receipt.arm_seq < release.0 && release.0 < consumed && consumed < ready_seq) {
            return Err(format!(
                "wait trace out of order: {receipt:?} ready={ready_seq}"
            ));
        }
        c = match receipt.wait {
            ReturnWait::TokioSleep(d) => {
                let p = receipt.p.ok_or("tokio wait without deadline")?;
                if release.1 < p || p - receipt.q != d {
                    return Err(format!("tokio relative wait mismatch: {receipt:?}"));
                }
                c + d
            }
            ReturnWait::TokioSleepUntil(at) => {
                if receipt.p != Some(at) || release.1 < at {
                    return Err(format!("tokio absolute wait mismatch: {receipt:?}"));
                }
                c.max(at)
            }
            ReturnWait::Native(_) => c + receipt.native_waited.ok_or("native wait unmeasured")?,
        };
    }
    Ok(c)
}

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
            if self.probe.external.load(Ordering::SeqCst) {
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

pub(super) async fn drive_close(mut domain: Domain<'_>, case: CloseCase) -> CloseRun {
    let CloseCase {
        capped,
        hold,
        wait,
        mut schedule,
    } = case;
    let budget = if capped {
        ms(200)
    } else {
        Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS)
    };
    let worker_phase = budget - (budget / 4).min(SHUTDOWN_FINALIZATION_RESERVE_MAX);
    let wake = Arc::new(WakeFlag::default());
    let probe = Arc::new_cyclic(|me| FixtureProbe {
        me: me.clone(),
        log: Mutex::default(),
        external: AtomicBool::new(false),
        seq: AtomicU64::new(0),
        t0: Mutex::new(None),
        wait,
        wake: Arc::clone(&wake),
        watchdog: Mutex::new(None),
    });
    let drain_gate = Arc::new(Gate::default());
    let observer_probe = Arc::clone(&probe);
    let observer_gate = (hold == Hold::Drain).then(|| Arc::clone(&drain_gate));
    domain
        .container_backend
        .set_shutdown_stage_observer_for_test(Arc::new(move |stage| {
            observer_probe.stage(stage);
            if let (ShutdownStage::Drain(_), Some(gate)) = (stage, observer_gate.as_ref()) {
                gate.wait();
            }
        }));
    install_worker_probe(domain.coordinator, &probe);
    let mut finalizer = spawn_finalizer(domain.ticket).await;
    let finalizer_id = finalizer.id();
    assert_domain_ready(domain.coordinator, domain.container_backend);
    let watchdog = Arc::new(Watchdog {
        state: Mutex::default(),
        changed: Condvar::new(),
        gates: vec![Arc::clone(&drain_gate)],
        wake: Arc::clone(&wake),
        case: format!("capped={capped} hold={hold:?} wait={wait:?}"),
    });
    *probe.watchdog.lock().unwrap() = Some(Arc::clone(&watchdog));
    let (started, started_rx) = oneshot::channel();
    let watch = Arc::clone(&watchdog);
    let watcher = tokio::task::spawn_blocking(move || watch.watch(started));
    started_rx.await.expect("watchdog acknowledges its start");
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
    assert!(report.is_none(), "first poll returns Pending");
    assert!(
        domain.container_backend.shutdown_observation_for_test().0,
        "backend sealed"
    );
    assert!(probe.log().worker_polls >= 1, "worker polled by the close");

    let worker_state = |probe: &FixtureProbe| {
        let log = probe.log();
        (log.worker_aborts, log.worker_ready.len())
    };
    if capped {
        report = driver.tick_to(ms(149)).await;
        assert!(report.is_none(), "capped close pending at 149 ms");
        assert_eq!(
            worker_state(&probe),
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
        assert!(domain.coordinator.inner.worker.lock().unwrap().is_none());
        assert!(domain
            .coordinator
            .inner
            .retained_workers
            .lock()
            .unwrap()
            .is_empty());
    } else {
        report = driver.tick_to(ms(300)).await;
        assert!(report.is_none(), "normal close pending at 300 ms");
        assert_eq!(
            worker_state(&probe),
            (0, 0),
            "normal worker pending, no abort"
        );
        assert!(
            probe.log().joins.is_empty(),
            "no cleanup join before the worker"
        );
        driver.mark("worker held at 300".into());
        if let Some(guard) = domain.restore_guard.take() {
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

    if report.is_none() && hold != Hold::None {
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
        let inner = &domain.coordinator.inner;
        assert!(inner.retained_rollback_tasks.lock().unwrap().is_empty());
        assert!(inner.retained_cleanup_tasks.lock().unwrap().is_empty());
        let log = probe.log();
        assert!(log.joins[site].1, "{site} join pending at 199 ms");
        if site == "rollback" {
            assert!(!log.joins.contains_key("drain"), "drain not entered");
        } else {
            assert!(!log.joins["rollback"].1, "rollback joined before the drain");
        }
        drop(log);
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
    } else if report.is_none() {
        report = driver.settle(&mut schedule, &|_| false).await;
    }
    let l = report.as_ref().map(|_| driver.now());
    let ready_seq = driver.ready_seq.unwrap_or(u64::MAX);
    let checkpoints = std::mem::take(&mut driver.checkpoints);
    drop(driver);

    let log = probe.log();
    let worker_expired = log.worker_aborts > 0;
    let shared_expired = !log.transfers.is_empty();
    let c = attributable(
        worker_expired.then_some(worker_phase),
        shared_expired.then_some(budget),
        &log.receipts,
        ready_seq,
    );
    let transfers = log.transfers.clone();
    let begin_arg = log.stages.iter().find_map(|s| match s {
        ShutdownStage::Begin(at) => Some(*at),
        ShutdownStage::Drain(_) => None,
    });
    let drain_arg = log.stages.iter().find_map(|s| match s {
        ShutdownStage::Drain(at) => Some(*at),
        ShutdownStage::Begin(_) => None,
    });
    drop(log);

    // Release product guards, resume Tokio and join every owned handle once.
    drop(manager_hold);
    drain_gate.open();
    if let Some(guard) = domain.restore_guard.take() {
        guard.finish();
    }
    tokio::time::resume();
    let post = join_after_return(domain.coordinator, &watchdog, &mut finalizer, finalizer_id).await;
    let finalizer_result = post.1;
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
    std::future::poll_fn(|cx| {
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
