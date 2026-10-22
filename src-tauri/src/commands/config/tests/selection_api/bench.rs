use super::*;

// ── #2475 synthetic selection-lock bench ─────────────────────────
//
// Synthetic numbers from a temporary directory: they are NOT the numbers
// of the real app. Latency is only reported under AC_SELECTION_BENCH=1
// with --test-threads=1; without it the bench asserts fixture,
// collection, correlation and occupancy invariants only.

use crate::config::replica_identity::strict_read_probe;
use crate::session::selection::timing_probe::{
    bench_serialization_lock, BenchContext, Occupancy, OccupancyGuard, SelectionTimingSample,
    TimingSinkRegistration,
};

const BENCH_COMMANDS: [&str; 3] = [
    "preview_selection_lock_removal",
    "preview_coding_agent_profile_selection",
    "get_replica_selection_default",
];
/// One budget per batch: it covers the serialization lock, `between` and
/// the wait for the reads, all measured against one `deadline_at`.
const BENCH_BATCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);
/// Drain grace after the budget, as an absolute `grace_at = deadline_at +
/// grace`, never counted from when the expiry is observed.
const BENCH_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(30);
const BENCH_REPETITIONS: usize = 10;

/// R rooms per project path, M project paths, one replica per room. The R
/// replicas of project 1 share the anchor's Matrix; projects 2..M hold
/// `other-<j>` replicas with their own Matrix, excluded by the Kind
/// Matrix filter after their strict read.
struct BenchFixture {
    /// Taken (and kept on disk) by the drain-expiry path.
    temp: std::sync::Mutex<Option<tempfile::TempDir>>,
    root: PathBuf,
    settings: AppSettings,
    anchor: PathBuf,
    ac_roots: Vec<PathBuf>,
    configs: Vec<PathBuf>,
}

impl BenchFixture {
    fn root(&self) -> &Path {
        &self.root
    }
}

fn bench_fixture(r: usize, m: usize) -> BenchFixture {
    let fixture = selection_api_fixture();
    let mut projects = vec![fixture.project.clone()];
    let mut ac_roots = vec![fixture.ac_root.clone()];
    for index in 2..=m {
        let project = fixture._temp.path().join(format!("project-{index}"));
        let ac_root = project.join(".ac");
        std::fs::create_dir_all(&ac_root).expect("create .ac");
        projects.push(project);
        ac_roots.push(ac_root);
    }
    let mut configs = Vec::new();
    for (index, ac_root) in ac_roots.iter().enumerate() {
        let name = if index == 0 {
            "bench-agent".to_string()
        } else {
            format!("other-{}", index + 1)
        };
        let matrix = ac_root.join(format!("_agent_{name}"));
        for room in 1..=r {
            let replica = ac_root
                .join(format!("room-{room}-bench"))
                .join(format!("__agent_{name}"));
            selection_api_replica_at(
                &fixture,
                &matrix,
                &replica,
                &name,
                locked_tooling("B", "agent-0"),
            );
            configs.push(replica.join("config.json"));
        }
    }
    let anchor = ac_roots[0].join("room-1-bench").join("__agent_bench-agent");
    let mut settings = selection_api_settings(&fixture);
    settings.project_paths = projects
        .iter()
        .map(|project| project.to_string_lossy().to_string())
        .collect();
    let SelectionApiFixture { _temp: temp, .. } = fixture;
    BenchFixture {
        root: temp.path().to_path_buf(),
        temp: std::sync::Mutex::new(Some(temp)),
        settings,
        anchor,
        ac_roots,
        configs,
    }
}

type BenchRead =
    std::pin::Pin<Box<dyn std::future::Future<Output = (u64, bool, std::time::Instant)>>>;
type BenchWait =
    std::pin::Pin<Box<dyn std::future::Future<Output = Vec<(u64, bool, std::time::Instant)>>>>;

struct BenchRun {
    samples: Vec<SelectionTimingSample>,
    by_invocation: std::collections::BTreeMap<u64, (&'static str, super::ProfileAssignmentScope)>,
    completed: std::collections::BTreeMap<u64, std::time::Instant>,
    batch_wall_ms: f64,
    occupancy_max: usize,
    body_threads: std::collections::HashSet<std::thread::ThreadId>,
}

/// What the harness does once the drain grace also expired: it lost
/// control of reads it launched. `Abort` is terminal; `Record` hands the
/// retained state back to a test that owns the stuck read.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum DrainExpiryAction {
    #[default]
    Abort,
    Record,
}

/// Instants the batch records, so a test can check the budget arithmetic.
#[derive(Clone, Copy, Debug, Default)]
struct BenchMilestones {
    deadline_at: Option<std::time::Instant>,
    grace_at: Option<std::time::Instant>,
    acquired: Option<std::time::Instant>,
    expired_observed: Option<std::time::Instant>,
}

#[derive(Default)]
struct BenchOptions {
    /// Measurement mode: an uncorrelated timed sample fails the run.
    measurement: bool,
    /// Test-only: record one timed sample WITHOUT a run context.
    inject_uncorrelated: bool,
    /// Test-only: a witness entered while the serialization lock is held.
    witness: Option<Arc<Occupancy>>,
    /// `None` means `BENCH_BATCH_BUDGET`.
    budget: Option<std::time::Duration>,
    /// `None` means `BENCH_DRAIN_GRACE`.
    grace: Option<std::time::Duration>,
    drain_expiry: DrainExpiryAction,
    /// Test-only: notified when the budget expires.
    on_deadline: Option<Arc<tokio::sync::Notify>>,
    /// Test-only: where the batch records its instants.
    milestones: Option<Arc<std::sync::Mutex<BenchMilestones>>>,
}

impl BenchOptions {
    fn record(&self, update: impl FnOnce(&mut BenchMilestones)) {
        if let Some(milestones) = &self.milestones {
            update(&mut milestones.lock().expect("milestones"));
        }
    }
}

/// The drain grace expired and the terminal action is `Record`: this
/// owns everything the batch still has in flight. Nothing is released
/// until the owner drains `wait`.
struct DrainExpired {
    diagnostic: String,
    pending: Vec<(u64, &'static str)>,
    retained: PathBuf,
    wait: BenchWait,
    sinks: Vec<TimingSinkRegistration>,
    serial: tokio::sync::MutexGuard<'static, ()>,
    _app: tauri::App<tauri::test::MockRuntime>,
}

impl DrainExpired {
    async fn drain(&mut self) {
        self.wait.as_mut().await;
    }
}

enum BenchError {
    Failed(String),
    DrainExpired(Box<DrainExpired>),
}

impl BenchError {
    fn message(&self) -> String {
        match self {
            Self::Failed(message) => message.clone(),
            Self::DrainExpired(expired) => expired.diagnostic.clone(),
        }
    }
}

impl std::fmt::Debug for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

fn bench_deadline_at(options: &BenchOptions) -> std::time::Instant {
    std::time::Instant::now() + options.budget.unwrap_or(BENCH_BATCH_BUDGET)
}

/// The only way to take the bench serialization lock: bounded by the
/// batch's `deadline_at`.
async fn acquire_bench_serialization(
    deadline_at: std::time::Instant,
) -> Result<tokio::sync::MutexGuard<'static, ()>, String> {
    tokio::time::timeout_at(
        tokio::time::Instant::from_std(deadline_at),
        bench_serialization_lock().lock(),
    )
    .await
    .map_err(|_| "bench serialization lock not acquired in time".to_string())
}

fn bench_measurement_mode() -> bool {
    std::env::var("AC_SELECTION_BENCH").as_deref() == Ok("1")
}

fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Percentile with linear interpolation. Never an average.
fn pct(values: &mut [f64], q: f64) -> f64 {
    assert!(!values.is_empty(), "percentile of nothing");
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    let rank = q * (values.len() - 1) as f64;
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    values[low] + (values[high] - values[low]) * (rank - low as f64)
}

async fn bench_batch(bench: &BenchFixture) -> BenchRun {
    bench_batch_with(
        bench,
        BenchOptions {
            measurement: bench_measurement_mode(),
            ..BenchOptions::default()
        },
    )
    .await
    .expect("bench batch")
}

async fn bench_batch_with(
    bench: &BenchFixture,
    options: BenchOptions,
) -> Result<BenchRun, BenchError> {
    let deadline_at = bench_deadline_at(&options);
    let serial = acquire_bench_serialization(deadline_at)
        .await
        .map_err(BenchError::Failed)?;
    options.record(|m| m.acquired = Some(std::time::Instant::now()));
    bench_batch_locked(serial, bench, &options, deadline_at, async {}).await
}

/// The batch of 13 reads, each through its real command and the timed
/// turn wrapper. Every read is SUBMITTED before `between` runs. The batch
/// owns `serial` and its sinks until every submitted read has finished.
///
/// Cooperative budget: no WAIT of the batch path goes past `deadline_at`
/// (the caller used the same instant for the lock) and no drain wait
/// goes past `grace_at`. Fixture setup, the submitting polls and any
/// future that does not yield are outside the timer.
async fn bench_batch_locked(
    serial: tokio::sync::MutexGuard<'static, ()>,
    bench: &BenchFixture,
    options: &BenchOptions,
    deadline_at: std::time::Instant,
    between: impl std::future::Future<Output = ()> + 'static,
) -> Result<BenchRun, BenchError> {
    use std::task::Poll;
    use tauri::Manager;
    let grace_at = deadline_at + options.grace.unwrap_or(BENCH_DRAIN_GRACE);
    options.record(|m| {
        m.deadline_at = Some(deadline_at);
        m.grace_at = Some(grace_at);
    });
    let anchor = bench.anchor.as_path();
    let _witness = options.witness.as_ref().map(OccupancyGuard::enter);
    let app = tauri::test::mock_app();
    app.manage(empty_session_manager());
    app.manage(state_for(bench.settings.clone()));
    let handle = app.handle().clone();
    let done = Arc::new(std::sync::Mutex::new(
        std::collections::BTreeSet::<u64>::new(),
    ));

    let (sample_tx, sample_rx) = std::sync::mpsc::channel::<SelectionTimingSample>();
    let sample_tx = Arc::new(std::sync::Mutex::new(sample_tx));
    let _sinks: Vec<TimingSinkRegistration> = BENCH_COMMANDS
        .iter()
        .map(|command| {
            let sample_tx = Arc::clone(&sample_tx);
            TimingSinkRegistration::new_sample(
                command,
                Arc::new(move |sample: &SelectionTimingSample, _line: &str| {
                    let _ = sample_tx.lock().expect("sample tx").send(sample.clone());
                }),
            )
        })
        .collect();
    if options.inject_uncorrelated {
        crate::session::selection::run_owned_selection_operation_timed(
            "get_replica_selection_default",
            || async { Ok::<(), String>(()) },
        )
        .await
        .map_err(BenchError::Failed)?;
    }

    let occupancy = Arc::new(Occupancy::default());
    let mut by_invocation = std::collections::BTreeMap::new();
    let mut reads: Vec<BenchRead> = Vec::new();
    let scopes = [
        super::ProfileAssignmentScope::Replica,
        super::ProfileAssignmentScope::Kind,
        super::ProfileAssignmentScope::Workgroup,
    ];
    for scope in &scopes {
        for _ in 0..2 {
            let context = BenchContext::new(&occupancy);
            let id = context.invocation;
            by_invocation.insert(id, ("preview_selection_lock_removal", scope.clone()));
            let request = api_removal_preview_request(anchor, scope.clone());
            let (handle, done) = (handle.clone(), Arc::clone(&done));
            reads.push(Box::pin(async move {
                let ok = crate::session::selection::BENCH_CONTEXT
                    .scope(
                        context,
                        super::preview_selection_lock_removal(
                            handle.state(),
                            handle.state(),
                            request,
                        ),
                    )
                    .await
                    .is_ok();
                done.lock().expect("done").insert(id);
                (id, ok, std::time::Instant::now())
            }));
        }
    }
    for scope in &scopes {
        for _ in 0..2 {
            let context = BenchContext::new(&occupancy);
            let id = context.invocation;
            by_invocation.insert(
                id,
                ("preview_coding_agent_profile_selection", scope.clone()),
            );
            let request =
                api_preview_request(anchor, scope.clone(), super::AssignmentMode::Ordinary);
            let (handle, done) = (handle.clone(), Arc::clone(&done));
            reads.push(Box::pin(async move {
                let ok = crate::session::selection::BENCH_CONTEXT
                    .scope(
                        context,
                        super::preview_coding_agent_profile_selection(
                            handle.state(),
                            handle.state(),
                            request,
                        ),
                    )
                    .await
                    .is_ok();
                done.lock().expect("done").insert(id);
                (id, ok, std::time::Instant::now())
            }));
        }
    }
    {
        let context = BenchContext::new(&occupancy);
        let id = context.invocation;
        by_invocation.insert(
            id,
            (
                "get_replica_selection_default",
                super::ProfileAssignmentScope::Replica,
            ),
        );
        let request = api_default_request(anchor);
        let (handle, done) = (handle.clone(), Arc::clone(&done));
        reads.push(Box::pin(async move {
            let ok = crate::session::selection::BENCH_CONTEXT
                .scope(
                    context,
                    super::get_replica_selection_default(handle.state(), request),
                )
                .await
                .is_ok();
            done.lock().expect("done").insert(id);
            (id, ok, std::time::Instant::now())
        }));
    }

    // One poll per read runs its submission: `submitted` is stamped and
    // the owned task is spawned before the next read is built.
    let mut finished = Vec::new();
    let mut pending = Vec::new();
    for mut read in reads {
        match futures::poll!(read.as_mut()) {
            Poll::Ready(done) => finished.push(done),
            Poll::Pending => pending.push(read),
        }
    }
    let submitted_reads = pending.len();
    let mut wait: BenchWait = Box::pin(async move {
        between.await;
        futures::future::join_all(pending).await
    });
    let joined =
        match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline_at), wait.as_mut())
            .await
        {
            Ok(joined) => joined,
            Err(_) => {
                options.record(|m| m.expired_observed = Some(std::time::Instant::now()));
                if let Some(notify) = &options.on_deadline {
                    notify.notify_one();
                }
                // The owned reads are detached, not cancelled, when their
                // waiters drop: keep the sinks and the serialization lock
                // until every one of them has finished.
                if tokio::time::timeout_at(tokio::time::Instant::from_std(grace_at), wait.as_mut())
                    .await
                    .is_ok()
                {
                    drop(serial);
                    return Err(BenchError::Failed(format!(
                        "bench batch timed out; {submitted_reads} submitted reads drained before \
                    release"
                    )));
                }
                // The harness lost control of reads it launched. In order:
                // 1. keep the fixture on disk, so no orphan reads a deleted path;
                let retained = bench
                    .temp
                    .lock()
                    .expect("fixture temp")
                    .take()
                    .map(tempfile::TempDir::keep)
                    .unwrap_or_else(|| bench.root.clone());
                // 2. keep the sinks registered and 3. keep `serial` held;
                // 4. name what is still pending and where the fixture is.
                let finished_ids = done.lock().expect("done").clone();
                let pending_reads: Vec<(u64, &'static str)> = by_invocation
                    .iter()
                    .filter(|(id, _)| !finished_ids.contains(id))
                    .map(|(id, (command, _))| (*id, *command))
                    .collect();
                let diagnostic = format!(
                "bench drain expired: pending invocations {pending_reads:?}; retained fixture {}",
                retained.display()
            );
                eprintln!("{diagnostic}");
                // 5. the terminal action.
                match options.drain_expiry {
                    DrainExpiryAction::Abort => std::process::abort(),
                    DrainExpiryAction::Record => {
                        return Err(BenchError::DrainExpired(Box::new(DrainExpired {
                            diagnostic,
                            pending: pending_reads,
                            retained,
                            wait,
                            sinks: _sinks,
                            serial,
                            _app: app,
                        })));
                    }
                }
            }
        };
    finished.extend(joined);

    let mut completed = std::collections::BTreeMap::new();
    for (id, ok, at) in finished {
        if !ok {
            return Err(BenchError::Failed(format!("bench read {id} failed")));
        }
        completed.insert(id, at);
    }

    // Every sample is recorded inside its owned task before that task
    // completes, so all of this batch's samples are already queued.
    let mut samples = Vec::new();
    let mut uncorrelated = 0usize;
    for sample in sample_rx.try_iter() {
        match sample.invocation {
            Some(id) if by_invocation.contains_key(&id) => samples.push(sample),
            Some(_) => {}
            None => uncorrelated += 1,
        }
    }
    if options.measurement && uncorrelated > 0 {
        return Err(BenchError::Failed(format!(
            "measurement run received {uncorrelated} uncorrelated timed sample(s)"
        )));
    }
    let received: std::collections::BTreeSet<u64> = samples
        .iter()
        .filter_map(|sample| sample.invocation)
        .collect();
    if samples.len() != by_invocation.len() || received.len() != by_invocation.len() {
        let missing: Vec<u64> = by_invocation
            .keys()
            .filter(|id| !received.contains(id))
            .copied()
            .collect();
        return Err(BenchError::Failed(format!(
            "received {} of {} samples; missing ids {missing:?}",
            samples.len(),
            by_invocation.len()
        )));
    }
    let first_submit = samples
        .iter()
        .map(|sample| sample.submitted)
        .min()
        .expect("samples");
    let last_done = *completed.values().max().expect("completions");
    Ok(BenchRun {
        batch_wall_ms: ms(last_done.saturating_duration_since(first_submit)),
        occupancy_max: occupancy.max(),
        body_threads: samples.iter().map(|sample| sample.body_thread).collect(),
        samples,
        by_invocation,
        completed,
    })
}

fn bench_key(run: &BenchRun, sample: &SelectionTimingSample) -> String {
    let (command, scope) = &run.by_invocation[&sample.invocation.expect("correlated")];
    format!("{command}/{scope:?}")
}

fn bench_sum_queue_wait_ms(run: &BenchRun) -> f64 {
    run.samples.iter().map(|sample| ms(sample.queue_wait)).sum()
}

/// One report block for a set of runs (the cold one, or the warm ones).
fn bench_report(label: &str, runs: &[&BenchRun]) {
    let mut walls: Vec<f64> = runs.iter().map(|run| run.batch_wall_ms).collect();
    let mut sums: Vec<f64> = runs
        .iter()
        .map(|run| bench_sum_queue_wait_ms(run))
        .collect();
    let mut work_by_key: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    let mut work_by_command: std::collections::BTreeMap<&str, Vec<f64>> = Default::default();
    let mut wait_by_command: std::collections::BTreeMap<&str, Vec<f64>> = Default::default();
    let mut wait_all = Vec::new();
    for run in runs {
        for sample in &run.samples {
            work_by_key
                .entry(bench_key(run, sample))
                .or_default()
                .push(ms(sample.work));
            work_by_command
                .entry(sample.command)
                .or_default()
                .push(ms(sample.work));
            wait_by_command
                .entry(sample.command)
                .or_default()
                .push(ms(sample.queue_wait));
            wait_all.push(ms(sample.queue_wait));
        }
    }
    let (slowest_key, slowest_p90) = work_by_key
        .iter_mut()
        .map(|(key, values)| (key.clone(), pct(values, 0.9)))
        .fold((String::new(), f64::MIN), |best, next| {
            if next.1 > best.1 {
                next
            } else {
                best
            }
        });
    let occupancy = runs.iter().map(|run| run.occupancy_max).max().unwrap_or(0);
    let threads = runs
        .iter()
        .map(|run| run.body_threads.len())
        .max()
        .unwrap_or(0);
    println!(
        "[bench] {label} runs={} batch_wall_ms p50={:.1} p90={:.1} sum_queue_wait_ms p50={:.1} \
         slowest_work_p90_ms={:.1} ({slowest_key}) occupancy_max={occupancy} \
         diag_body_threads={threads}",
        runs.len(),
        pct(&mut walls.clone(), 0.5),
        pct(&mut walls, 0.9),
        pct(&mut sums, 0.5),
        slowest_p90,
    );
    println!(
        "[bench] {label} queue_wait_ms aggregate p50={:.1} p90={:.1}",
        pct(&mut wait_all.clone(), 0.5),
        pct(&mut wait_all, 0.9)
    );
    for (command, values) in wait_by_command.iter_mut() {
        let work = work_by_command.get_mut(command).expect("work");
        println!(
            "[bench] {label} {command} queue_wait_ms p50={:.1} p90={:.1} work_ms p50={:.1} \
            p90={:.1}",
            pct(&mut values.clone(), 0.5),
            pct(values, 0.9),
            pct(&mut work.clone(), 0.5),
            pct(work, 0.9)
        );
    }
    for (key, values) in work_by_key.iter_mut() {
        println!(
            "[bench] {label} {key} work_ms p50={:.1} p90={:.1}",
            pct(&mut values.clone(), 0.5),
            pct(values, 0.9)
        );
    }
}

fn assert_bench_correlation(run: &BenchRun) {
    assert_eq!(run.samples.len(), 13, "13 samples");
    let ids: std::collections::BTreeSet<u64> = run
        .samples
        .iter()
        .map(|sample| sample.invocation.expect("correlated sample"))
        .collect();
    assert_eq!(ids.len(), 13, "distinct invocation ids");
    for sample in &run.samples {
        let id = sample.invocation.expect("correlated sample");
        let (command, _scope) = run.by_invocation.get(&id).expect("known invocation");
        assert_eq!(*command, sample.command, "sample matches its invocation");
        assert!(sample.ok, "read {id} succeeded");
        let done = run.completed[&id];
        assert!(
            sample.submitted + sample.queue_wait + sample.work <= done,
            "invocation {id} submitted before it finished"
        );
    }
    let mut per_scope: std::collections::BTreeMap<String, usize> = Default::default();
    for (command, scope) in run.by_invocation.values() {
        *per_scope.entry(format!("{command}/{scope:?}")).or_default() += 1;
    }
    assert_eq!(per_scope.len(), 7, "declared mix: {per_scope:?}");
}

mod drain;
mod enumeration;
