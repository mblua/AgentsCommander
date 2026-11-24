//! Per-instance close probe: wait receipts, child joins and the attributable bound `C`.

use super::*;

/// Waits the probe performs at one close site, inline or as children joined by the close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in super::super) struct SiteWait {
    pub(in super::super) site: &'static str,
    pub(in super::super) waits: Vec<ReturnWait>,
    pub(in super::super) children: bool,
}

impl SiteWait {
    pub(in super::super) fn inline(site: &'static str, wait: ReturnWait) -> Self {
        Self {
            site,
            waits: vec![wait],
            children: false,
        }
    }

    pub(in super::super) fn children(site: &'static str, waits: Vec<ReturnWait>) -> Self {
        Self {
            site,
            waits,
            children: true,
        }
    }
}

#[derive(Clone, Debug)]
pub(in super::super) struct Receipt {
    pub(in super::super) wait: ReturnWait,
    pub(in super::super) site: &'static str,
    /// Child task that performed the wait; `None` when the close awaited it inline.
    pub(in super::super) owner: Option<tokio::task::Id>,
    pub(in super::super) arm_seq: u64,
    pub(in super::super) q: Duration,
    pub(in super::super) p: Option<Duration>,
    pub(in super::super) release: Option<(u64, Duration)>,
    pub(in super::super) native_waited: Option<Duration>,
    pub(in super::super) consume_seq: Option<u64>,
    /// First Pending of the parent's join of `owner`, before the child may arm.
    pub(in super::super) permit_seq: Option<u64>,
    /// The close's join that returned the child's result.
    pub(in super::super) joined_seq: Option<u64>,
}

#[derive(Default)]
pub(super) struct ProbeLog {
    pub(super) joins: BTreeMap<&'static str, (tokio::task::Id, bool)>,
    pub(super) transfers: Vec<(&'static str, tokio::task::Id)>,
    pub(super) receipts: Vec<Receipt>,
    pub(super) stages: Vec<ShutdownStage>,
    pub(super) drain_seq: Option<u64>,
    pub(super) permits: BTreeMap<tokio::task::Id, u64>,
    pub(super) joined: BTreeMap<tokio::task::Id, u64>,
    pub(super) worker_polls: usize,
    pub(super) worker_aborts: usize,
    pub(super) worker_ready: Vec<Result<(), String>>,
}

pub(in super::super) struct FixtureProbe {
    pub(super) log: Mutex<ProbeLog>,
    pub(super) external: AtomicBool,
    /// Child Tokio timers armed and not yet released; the driver ticks while any is armed.
    pub(super) child_timers: std::sync::atomic::AtomicUsize,
    pub(super) seq: AtomicU64,
    pub(super) t0: Mutex<Option<tokio::time::Instant>>,
    pub(super) wait: Option<SiteWait>,
    pub(super) wake: Arc<WakeFlag>,
    pub(super) watchdog: Mutex<Option<Arc<Watchdog>>>,
    pub(super) me: std::sync::Weak<FixtureProbe>,
}

enum Permit {
    Native(std::sync::mpsc::Sender<tokio::task::Id>),
    Tokio(oneshot::Sender<tokio::task::Id>),
}

impl FixtureProbe {
    pub(super) fn log(&self) -> std::sync::MutexGuard<'_, ProbeLog> {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn next(&self) -> u64 {
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

    pub(super) fn stage(&self, stage: ShutdownStage) {
        if matches!(stage, ShutdownStage::Begin(_)) {
            if let Some(watchdog) = self.watchdog.lock().unwrap().clone() {
                watchdog.stamp(|stamps| {
                    stamps.a.get_or_insert_with(Instant::now);
                });
            }
        }
        let seq = self.next();
        let mut log = self.log();
        if matches!(stage, ShutdownStage::Drain(_)) {
            log.drain_seq.get_or_insert(seq);
        }
        log.stages.push(stage);
        drop(log);
        WakeFlag::wake_by_ref(&self.wake);
    }

    fn arm(
        &self,
        site: &'static str,
        owner: Option<tokio::task::Id>,
        wait: ReturnWait,
        p: Option<Duration>,
    ) -> usize {
        let arm_seq = self.next();
        let q = self.since_t0();
        let mut log = self.log();
        let permit_seq = owner.and_then(|id| log.permits.get(&id).copied());
        log.receipts.push(Receipt {
            wait,
            site,
            owner,
            arm_seq,
            q,
            p,
            release: None,
            native_waited: None,
            consume_seq: None,
            permit_seq,
            joined_seq: None,
        });
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
    async fn tokio_wait(
        self: Arc<Self>,
        site: &'static str,
        owner: Option<tokio::task::Id>,
        wait: ReturnWait,
    ) {
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
        let index = self.arm(site, owner, wait, Some(p));
        let mut sleep = Box::pin(sleep);
        let probe = Arc::clone(&self);
        let mut armed = false;
        std::future::poll_fn(|cx| match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => {
                probe.released(index, None);
                if armed && owner.is_some() {
                    probe.child_timers.fetch_sub(1, Ordering::SeqCst);
                }
                Poll::Ready(())
            }
            Poll::Pending => {
                if !std::mem::replace(&mut armed, true) {
                    if owner.is_some() {
                        probe.child_timers.fetch_add(1, Ordering::SeqCst);
                    }
                    WakeFlag::wake_by_ref(&probe.wake);
                }
                Poll::Pending
            }
        })
        .await;
        self.consumed(index);
    }

    /// Native wait: a service thread other than the watchdog opens the gate at entry+d.
    fn native_wait(&self, site: &'static str, owner: Option<tokio::task::Id>, d: Duration) {
        let index = self.arm(site, owner, ReturnWait::Native(d), None);
        let gate = Arc::new(Gate::default());
        let entry = Instant::now();
        let service_gate = Arc::clone(&gate);
        let watchdog = self.watchdog.lock().unwrap().clone();
        let service = std::thread::spawn(move || {
            std::thread::sleep((entry + d).saturating_duration_since(Instant::now()));
            let released = Instant::now();
            let r_pending = watchdog.is_none_or(|dog| dog.lock().stamps.r.is_none());
            service_gate.open();
            (released, r_pending)
        });
        gate.wait();
        let (released, r_pending) = service.join().expect("native wait service joins");
        assert!(released - entry >= d, "native release precedes entry + d");
        assert!(r_pending, "native wait released after R");
        self.released(index, Some(d));
        self.consumed(index);
    }

    /// Spawns one child per wait; each arms only after the close's join saw it Pending.
    async fn children(self: Arc<Self>, site: &'static str, waits: Vec<ReturnWait>) {
        let mut handles = Vec::new();
        let mut permits = Vec::new();
        for wait in waits {
            let probe = Arc::clone(&self);
            if let ReturnWait::Native(d) = wait {
                let (permit, permitted) = std::sync::mpsc::channel();
                handles.push(tokio::task::spawn_blocking(move || {
                    let id = permitted.recv().expect("close permits the native child");
                    probe.native_wait(site, Some(id), d);
                }));
                permits.push(Some(Permit::Native(permit)));
            } else {
                let (permit, permitted) = oneshot::channel();
                handles.push(tokio::spawn(async move {
                    let id = permitted.await.expect("close permits the Tokio child");
                    probe.tokio_wait(site, Some(id), wait).await;
                }));
                permits.push(Some(Permit::Tokio(permit)));
            }
        }
        let ids: Vec<_> = handles.iter().map(JoinHandle::id).collect();
        let mut done = vec![false; handles.len()];
        std::future::poll_fn(|cx| {
            for (index, handle) in handles.iter_mut().enumerate() {
                if done[index] {
                    continue;
                }
                match Pin::new(handle).poll(cx) {
                    Poll::Ready(result) => {
                        result.expect("child wait joins");
                        let seq = self.next();
                        self.log().joined.insert(ids[index], seq);
                        done[index] = true;
                    }
                    Poll::Pending => {
                        if let Some(permit) = permits[index].take() {
                            let seq = self.next();
                            self.log().permits.insert(ids[index], seq);
                            let sent = match permit {
                                Permit::Native(permit) => permit.send(ids[index]).is_ok(),
                                Permit::Tokio(permit) => permit.send(ids[index]).is_ok(),
                            };
                            assert!(sent, "child awaits its permit");
                        }
                    }
                }
            }
            let all = done.iter().all(|done| *done);
            self.external.store(!all, Ordering::SeqCst);
            if all {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        let mut log = self.log();
        let joined = log.joined.clone();
        for receipt in log.receipts.iter_mut() {
            if let Some(id) = receipt.owner {
                receipt.joined_seq = joined.get(&id).copied();
            }
        }
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

    fn return_wait(&self, site: &'static str) -> Option<BoxFuture<'static, ()>> {
        let wait = self.wait.clone().filter(|wait| wait.site == site)?;
        let probe = self.me.upgrade()?;
        if wait.children {
            return Some(Box::pin(probe.children(site, wait.waits)));
        }
        Some(Box::pin(async move {
            for each in wait.waits {
                match each {
                    ReturnWait::Native(d) => probe.native_wait(site, None, d),
                    tokio_wait => Arc::clone(&probe).tokio_wait(site, None, tokio_wait).await,
                }
            }
        }))
    }
}

/// The same worker the coordinator owns, with poll and abort delegated and recorded.
pub(super) struct ProbeWorker {
    pub(super) inner: Pin<Box<dyn SelectionTask>>,
    pub(super) probe: Arc<FixtureProbe>,
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

/// Checks one receipt's trace: request, release, consumption, child join, drain and Ready.
fn traced(receipt: &Receipt, drain_seq: Option<u64>, ready_seq: u64) -> Result<(), String> {
    let release = receipt.release.ok_or("wait never released")?.0;
    let consumed = receipt.consume_seq.ok_or("wait never consumed")?;
    let mut chain = vec![receipt.arm_seq, release, consumed];
    if receipt.owner.is_some() {
        let permit = receipt
            .permit_seq
            .ok_or("child armed without a join Pending")?;
        chain.insert(0, permit);
        chain.push(receipt.joined_seq.ok_or("child result never joined")?);
    }
    if receipt.site == "between-joins" {
        chain.push(drain_seq.ok_or("drain never entered after the wait")?);
    }
    chain.push(ready_seq);
    if chain.windows(2).all(|pair| pair[0] < pair[1]) {
        Ok(())
    } else {
        Err(format!(
            "wait trace out of order: {receipt:?} drain={drain_seq:?} ready={ready_seq}"
        ))
    }
}

/// One wait's cursor after it completes from `start`.
fn credited(receipt: &Receipt, start: Duration) -> Result<Duration, String> {
    let release = receipt.release.ok_or("wait never released")?.1;
    Ok(match receipt.wait {
        ReturnWait::TokioSleep(d) => {
            let p = receipt.p.ok_or("tokio wait without deadline")?;
            if release < p || p - receipt.q != d {
                return Err(format!("tokio relative wait mismatch: {receipt:?}"));
            }
            start + d
        }
        ReturnWait::TokioSleepUntil(at) => {
            if receipt.p != Some(at) || release < at {
                return Err(format!("tokio absolute wait mismatch: {receipt:?}"));
            }
            start.max(at)
        }
        ReturnWait::Native(_) => start + receipt.native_waited.ok_or("native wait unmeasured")?,
    })
}

/// Attributable delay `C`: expired product deadlines and the waits that conditioned R.
/// Inline waits add in series; children of one site start together from the parent cursor.
pub(in super::super) fn attributable(
    worker_phase: Option<Duration>,
    shared: Option<Duration>,
    receipts: &[Receipt],
    drain_seq: Option<u64>,
    ready_seq: u64,
) -> Result<Duration, String> {
    let mut c = worker_phase
        .into_iter()
        .chain(shared)
        .max()
        .unwrap_or_default();
    let mut group: Option<(&'static str, Duration)> = None;
    for receipt in receipts {
        traced(receipt, drain_seq, ready_seq)?;
        c = match (receipt.owner, group) {
            (Some(_), Some((site, start))) if site == receipt.site => {
                c.max(credited(receipt, start)?)
            }
            (Some(_), _) => {
                group = Some((receipt.site, c));
                credited(receipt, c)?
            }
            (None, _) => {
                group = None;
                credited(receipt, c)?
            }
        };
    }
    Ok(c)
}
