use super::*;

/// The batch test. Without AC_SELECTION_BENCH it asserts one batch's
/// collection and correlation; with AC_SELECTION_BENCH=1 (run it with
/// --test-threads=1) it also runs 10 repetitions of both fixtures and
/// prints the baseline. Rep 1 is cold, reps 2..10 are warm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_samples_carry_invocation_scope_and_submit_instant() {
    let bench = bench_fixture(8, 1);
    let run = bench_batch(&bench).await;
    assert_bench_correlation(&run);
    assert!(run.occupancy_max >= 1, "bodies entered the occupancy");

    if !bench_measurement_mode() {
        return;
    }
    println!(
        "[bench] synthetic numbers from a temporary directory, not the real app; \
         mix 6 preview_selection_lock_removal + 6 preview_coding_agent_profile_selection \
         (2 per scope) + 1 get_replica_selection_default; available_parallelism={}",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    for (r, m) in [(8usize, 1usize), (40, 3)] {
        let bench = bench_fixture(r, m);
        let mut runs = Vec::new();
        for _ in 0..BENCH_REPETITIONS {
            let run = bench_batch(&bench).await;
            assert_bench_correlation(&run);
            runs.push(run);
        }
        let cold = [&runs[0]];
        let warm: Vec<&BenchRun> = runs[1..].iter().collect();
        bench_report(&format!("R={r} M={m} cold"), &cold);
        bench_report(&format!("R={r} M={m} warm"), &warm);
    }
}

/// Writer control: every reader's wait covers the part of the writer's
/// hold it actually overlapped, `submitted` taken from the sample.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_reader_wait_covers_the_measured_overlap() {
    const TOLERANCE_MS: f64 = 0.05;
    let bench = bench_fixture(3, 1);
    let options = BenchOptions::default();
    // The test takes the shared lock under its own bound; the small
    // batch budget starts once it holds it.
    let serial =
        acquire_bench_serialization(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .await
            .expect("serialization");
    let deadline_at = bench_deadline_at(&options);
    let writer = crate::session::selection::acquire_selection_operation_turn()
        .await
        .expect("writer turn");
    let acquired = std::time::Instant::now();
    let released = Arc::new(std::sync::Mutex::new(None::<std::time::Instant>));
    let release_slot = Arc::clone(&released);
    let run = bench_batch_locked(serial, &bench, &options, deadline_at, async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        *release_slot.lock().expect("release slot") = Some(std::time::Instant::now());
        drop(writer);
    })
    .await
    .expect("bench batch");
    let released = released
        .lock()
        .expect("release slot")
        .expect("writer released");
    assert_bench_correlation(&run);
    for sample in &run.samples {
        let start = sample.submitted.max(acquired);
        assert!(start < released, "reader submitted while the writer held");
        let overlap = ms(released - start);
        assert!(
            ms(sample.queue_wait) + TOLERANCE_MS >= overlap,
            "queue_wait {:.3} ms must cover the {overlap:.3} ms overlap",
            ms(sample.queue_wait)
        );
    }
}

#[test]
fn issue_2475_bench_percentiles_are_not_averages() {
    let values = vec![1.0, 2.0, 3.0, 4.0, 100.0];
    assert_eq!(pct(&mut values.clone(), 0.5), 3.0);
    assert!(pct(&mut values.clone(), 0.9) >= 60.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_rejects_an_uncorrelated_timed_sample() {
    let bench = bench_fixture(1, 1);
    let rejected = bench_batch_with(
        &bench,
        BenchOptions {
            measurement: true,
            inject_uncorrelated: true,
            ..BenchOptions::default()
        },
    )
    .await;
    let error = rejected.err().expect("measurement run must fail").message();
    assert!(error.contains("uncorrelated"), "{error}");
    // Outside measurement mode the same batch is accepted.
    let accepted = bench_batch_with(
        &bench,
        BenchOptions {
            inject_uncorrelated: true,
            ..BenchOptions::default()
        },
    )
    .await
    .expect("non-measurement run");
    assert_bench_correlation(&accepted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_fixture_leaves_config_bytes_unchanged() {
    let bench = bench_fixture(3, 2);
    let before: Vec<Vec<u8>> = bench
        .configs
        .iter()
        .map(|path| std::fs::read(path).expect("read config.json"))
        .collect();
    let run = bench_batch(&bench).await;
    assert_bench_correlation(&run);
    let after: Vec<Vec<u8>> = bench
        .configs
        .iter()
        .map(|path| std::fs::read(path).expect("read config.json"))
        .collect();
    assert_eq!(before.len(), 6);
    assert!(before == after, "the bench must not write any config.json");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_holds_the_serialization_lock() {
    let bench = bench_fixture(3, 1);
    let witness = Arc::new(Occupancy::default());
    let options = || BenchOptions {
        witness: Some(Arc::clone(&witness)),
        ..BenchOptions::default()
    };
    let (first, second) = tokio::join!(
        bench_batch_with(&bench, options()),
        bench_batch_with(&bench, options()),
    );
    assert_bench_correlation(&first.expect("first run"));
    assert_bench_correlation(&second.expect("second run"));
    assert_eq!(witness.max(), 1, "two bench runs never overlap");
    assert_eq!(witness.in_flight(), 0);
}

/// Correlated samples seen for the three bench commands, via sinks the
/// test itself owns.
fn bench_correlated_counter() -> (
    Arc<std::sync::atomic::AtomicUsize>,
    Vec<TimingSinkRegistration>,
) {
    let correlated = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sinks = BENCH_COMMANDS
        .iter()
        .map(|command| {
            let correlated = Arc::clone(&correlated);
            TimingSinkRegistration::new_sample(
                command,
                Arc::new(move |sample: &SelectionTimingSample, _line: &str| {
                    if sample.invocation.is_some() {
                        correlated.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }),
            )
        })
        .collect();
    (correlated, sinks)
}

fn bench_registered_sinks() -> usize {
    BENCH_COMMANDS
        .iter()
        .map(|command| crate::session::selection::timing_probe::registered_sinks(command))
        .sum()
}

/// Step 7.6 test 1: the batch returns only after every submitted read
/// finished. Observed at the instant of the return, not after a join.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_deadline_drains_submitted_reads_before_release() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let bench = bench_fixture(3, 1);
    let expired = Arc::new(tokio::sync::Notify::new());
    let options = BenchOptions {
        budget: Some(std::time::Duration::from_millis(50)),
        grace: Some(std::time::Duration::from_secs(10)),
        drain_expiry: DrainExpiryAction::Record,
        on_deadline: Some(Arc::clone(&expired)),
        ..BenchOptions::default()
    };
    // The test takes the shared lock under its own bound; the small
    // batch budget starts once it holds it.
    let serial =
        acquire_bench_serialization(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .await
            .expect("serialization");
    let deadline_at = bench_deadline_at(&options);
    let (correlated, _probe_sinks) = bench_correlated_counter();
    let writer = crate::session::selection::acquire_selection_operation_turn()
        .await
        .expect("writer turn");
    let released = Arc::new(AtomicBool::new(false));
    let wake_releaser = Arc::clone(&expired);
    let releaser = tokio::spawn({
        let released = Arc::clone(&released);
        let correlated = Arc::clone(&correlated);
        async move {
            // Bounded: the writer turn is process-wide and must come back.
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(60), expired.notified()).await;
            let before = correlated.load(Ordering::SeqCst);
            released.store(true, Ordering::SeqCst);
            drop(writer);
            before
        }
    });
    let (result, at_return) = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let result = bench_batch_locked(serial, &bench, &options, deadline_at, async {}).await;
        // Copied in the immediate continuation of the batch's return.
        let at_return = (
            released.load(Ordering::SeqCst),
            correlated.load(Ordering::SeqCst),
        );
        (result, at_return)
    })
    .await
    .expect("the test itself is bounded");
    // If the batch returned without reaching its deadline, the releaser
    // must still give the process-wide turn back now.
    wake_releaser.notify_one();
    let before_release = releaser.await.expect("releaser");
    let error = match result {
        Err(BenchError::Failed(error)) => error,
        Err(BenchError::DrainExpired(mut expired)) => {
            expired.drain().await;
            panic!("unexpected drain expiry: {}", expired.diagnostic);
        }
        Ok(_) => panic!("the batch must fail on its budget"),
    };
    assert!(error.contains("timed out"), "{error}");
    assert_eq!(
        before_release, 0,
        "no read ran while the writer held the turn"
    );
    assert!(
        at_return.0,
        "the batch returned only after the writer released"
    );
    assert_eq!(
        at_return.1, 13,
        "every submitted read finished before the batch returned"
    );
}

/// Step 7.6 test 2: while a batch drains, a bounded acquisition of the
/// serialization lock fails with its own error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_draining_batch_does_not_hand_over_serialization() {
    let bench = bench_fixture(3, 1);
    let expired = Arc::new(tokio::sync::Notify::new());
    let options = BenchOptions {
        budget: Some(std::time::Duration::from_millis(50)),
        grace: Some(std::time::Duration::from_secs(10)),
        drain_expiry: DrainExpiryAction::Record,
        on_deadline: Some(Arc::clone(&expired)),
        ..BenchOptions::default()
    };
    // The test takes the shared lock under its own bound; the small
    // batch budget starts once it holds it.
    let serial =
        acquire_bench_serialization(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .await
            .expect("serialization");
    let deadline_at = bench_deadline_at(&options);
    let writer = crate::session::selection::acquire_selection_operation_turn()
        .await
        .expect("writer turn");
    let batch = bench_batch_locked(serial, &bench, &options, deadline_at, async {});
    tokio::pin!(batch);
    let reached_drain = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        tokio::select! {
            _ = &mut batch => false,
            _ = expired.notified() => true,
        }
    })
    .await
    .expect("the test itself is bounded");
    // The batch is now draining and must still own the lock.
    let second = acquire_bench_serialization(
        std::time::Instant::now() + std::time::Duration::from_millis(100),
    )
    .await;
    drop(writer);
    let result = tokio::time::timeout(std::time::Duration::from_secs(60), batch)
        .await
        .expect("the test itself is bounded");
    assert!(reached_drain, "the batch reached its drain");
    assert_eq!(
        second.err().as_deref(),
        Some("bench serialization lock not acquired in time"),
        "no batch takes the turn from a draining one"
    );
    assert!(result
        .err()
        .expect("budget failure")
        .message()
        .contains("timed out"));
}

/// Step 7.6 test 3: with `Record`, the terminal boundary keeps everything
/// (fixture on disk, sinks registered, lock held, diagnostic), and only
/// after the stuck read is released and drained does the test recover.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_drain_expiry_retains_then_recovers_the_shared_state() {
    let bench = bench_fixture(3, 1);
    let options = BenchOptions {
        budget: Some(std::time::Duration::from_millis(50)),
        grace: Some(std::time::Duration::from_millis(50)),
        drain_expiry: DrainExpiryAction::Record,
        ..BenchOptions::default()
    };
    // The test takes the shared lock under its own bound; the small
    // batch budget starts once it holds it.
    let serial =
        acquire_bench_serialization(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .await
            .expect("serialization");
    let deadline_at = bench_deadline_at(&options);
    // The test owns the stuck read: this writer keeps it queued past grace_at.
    let writer = crate::session::selection::acquire_selection_operation_turn()
        .await
        .expect("writer turn");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        bench_batch_locked(serial, &bench, &options, deadline_at, async {}),
    )
    .await
    .expect("the test itself is bounded");

    // Observations first, at the terminal boundary.
    let mut expired = match result {
        Err(BenchError::DrainExpired(expired)) => Some(expired),
        _ => None,
    };
    let observed = expired.as_ref().map(|expired| {
        let pending_named = expired.pending.iter().all(|(id, command)| {
            expired.diagnostic.contains(&id.to_string()) && expired.diagnostic.contains(command)
        });
        (
            expired.retained.is_dir(),
            bench.temp.lock().expect("temp").is_none(),
            bench_registered_sinks(),
            expired.pending.len(),
            pending_named,
            expired
                .diagnostic
                .contains(&expired.retained.display().to_string()),
        )
    });
    let lock_held = acquire_bench_serialization(
        std::time::Instant::now() + std::time::Duration::from_millis(100),
    )
    .await
    .is_err();

    // Deterministic recovery, whatever the observations say: release the
    // read, join ALL submitted work, then sinks, lock, fixture.
    drop(writer);
    let mut sinks_after = None;
    if let Some(mut expired) = expired.take() {
        tokio::time::timeout(std::time::Duration::from_secs(60), expired.drain())
            .await
            .expect("the released read drains");
        let DrainExpired {
            sinks,
            serial,
            retained,
            _app,
            ..
        } = *expired;
        drop(sinks);
        sinks_after = Some(bench_registered_sinks());
        drop(serial);
        std::fs::remove_dir_all(&retained).expect("remove the retained fixture");
    }
    let later = bench_fixture(3, 1);
    let later_run = tokio::time::timeout(std::time::Duration::from_secs(60), bench_batch(&later))
        .await
        .expect("the test itself is bounded");

    let observed = observed.expect("the drain grace expired with Record");
    assert!(observed.0, "the fixture stays on disk");
    assert!(observed.1, "the fixture TempDir was taken and kept");
    assert_eq!(observed.2, 3, "the batch sinks stay registered");
    assert_eq!(observed.3, 13, "every read is still pending");
    assert!(observed.4, "the diagnostic names every pending invocation");
    assert!(observed.5, "the diagnostic names the retained fixture");
    assert!(lock_held, "the serialization lock was not released");
    assert_eq!(sinks_after, Some(0), "recovery unregistered the sinks");
    assert_bench_correlation(&later_run);
}

/// Step 7.6 test 4: the default value only, not its dispatch.
#[test]
fn issue_2475_bench_default_drain_expiry_action_is_abort() {
    assert_eq!(
        BenchOptions::default().drain_expiry,
        DrainExpiryAction::Abort
    );
}

const BENCH_ABORT_CHILD_MARKER: &str = "AC_BENCH_ABORT_CHILD";
const BENCH_ABORT_CHILD_TEST: &str = concat!(
    "commands::config::tests::selection_api::bench::drain::",
    "issue_2475_bench_drain_expiry_aborts_in_a_child_process",
);

/// Step 7.6 test 6: the default action really aborts. The terminal path
/// runs in a child process (this same test, marked), so the suite
/// survives it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_drain_expiry_aborts_in_a_child_process() {
    if std::env::var_os(BENCH_ABORT_CHILD_MARKER).is_some() {
        let bench = bench_fixture(1, 1);
        let options = BenchOptions {
            budget: Some(std::time::Duration::from_millis(50)),
            grace: Some(std::time::Duration::from_millis(50)),
            ..BenchOptions::default()
        };
        let deadline_at = bench_deadline_at(&options);
        let serial = acquire_bench_serialization(deadline_at)
            .await
            .expect("serialization");
        let _writer = crate::session::selection::acquire_selection_operation_turn()
            .await
            .expect("writer turn");
        let _ = bench_batch_locked(serial, &bench, &options, deadline_at, async {}).await;
        panic!("the drain expiry returned instead of aborting");
    }

    let child = std::process::Command::new(std::env::current_exe().expect("test exe"))
        .args([
            "--exact",
            BENCH_ABORT_CHILD_TEST,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(BENCH_ABORT_CHILD_MARKER, "1")
        .env_remove("AC_SELECTION_BENCH")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the child");
    // Bounded wait: kill and reap on expiry; pipes are read on their own
    // threads so a full pipe never blocks the child.
    let (status, stdout, stderr) = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut child = child;
        let mut out = child.stdout.take().expect("stdout");
        let mut err = child.stderr.take().expect("stderr");
        let out = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = out.read_to_end(&mut bytes);
            bytes
        });
        let err = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = err.read_to_end(&mut bytes);
            bytes
        });
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll the child") {
                break Some(status);
            }
            if std::time::Instant::now() >= limit {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let stdout = String::from_utf8_lossy(&out.join().expect("stdout reader")).to_string();
        let stderr = String::from_utf8_lossy(&err.join().expect("stderr reader")).to_string();
        (status, stdout, stderr)
    })
    .await
    .expect("join the waiter");
    let status = status.unwrap_or_else(|| {
        panic!("the child did not terminate in time\nstdout:\n{stdout}\nstderr:\n{stderr}")
    });

    // Clean the known retained fixture before asserting.
    let retained = stderr
        .lines()
        .find_map(|line| {
            line.split_once("retained fixture ")
                .map(|(_, path)| path.trim())
        })
        .map(PathBuf::from);
    if let Some(retained) = &retained {
        let _ = std::fs::remove_dir_all(retained);
    }

    assert!(
        stdout.contains("running 1 test") && stdout.contains(BENCH_ABORT_CHILD_TEST),
        "the child ran exactly the terminal test:\n{stdout}"
    );
    assert!(!status.success(), "an abort is never a success: {status:?}");
    #[cfg(windows)]
    assert_eq!(
        status.code(),
        Some(0xC000_0409_u32 as i32),
        "abort exit code, not a panic or another crash: {status:?}\n{stderr}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(6), "SIGABRT: {status:?}\n{stderr}");
    }
    assert!(
        stderr.contains("bench drain expired: pending invocations"),
        "diagnostic with the pending invocations:\n{stderr}"
    );
    assert!(
        retained.is_some(),
        "diagnostic names the retained fixture:\n{stderr}"
    );
}

/// Step 7.6 test 5: acquisition and work share ONE deadline. A third
/// party holds the serialization for 0.6 x budget; the late batch must
/// still expire against the original `deadline_at`. Only an attempt where
/// a FOREIGN test got the lock first is retried; the arithmetic is
/// asserted on the first clean attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_one_budget_is_shared_between_acquisition_and_work() {
    let budget = std::time::Duration::from_millis(400);
    let grace = std::time::Duration::from_secs(10);
    // A reset budget would expire ~0.6 x budget later than allowed; this
    // tolerance stays well below that.
    let tolerance = budget / 4;
    let hold = budget.mul_f64(0.6);
    let bench = bench_fixture(1, 1);
    for attempt in 1..=5 {
        let expired = Arc::new(tokio::sync::Notify::new());
        let milestones = Arc::new(std::sync::Mutex::new(BenchMilestones::default()));
        let options = BenchOptions {
            budget: Some(budget),
            grace: Some(grace),
            drain_expiry: DrainExpiryAction::Record,
            on_deadline: Some(Arc::clone(&expired)),
            milestones: Some(Arc::clone(&milestones)),
            ..BenchOptions::default()
        };
        let third_party = acquire_bench_serialization(
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        )
        .await
        .expect("third party serialization");
        let writer = crate::session::selection::acquire_selection_operation_turn()
            .await
            .expect("writer turn");
        let wake_releaser = Arc::clone(&expired);
        let releaser = tokio::spawn(async move {
            // Bounded: the writer turn is process-wide and must come back.
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(60), expired.notified()).await;
            drop(writer);
        });
        let (result, third_released) =
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                tokio::join!(bench_batch_with(&bench, options), async move {
                    tokio::time::sleep(hold).await;
                    let released = std::time::Instant::now();
                    drop(third_party);
                    released
                })
            })
            .await
            .expect("the test itself is bounded");
        wake_releaser.notify_one();
        releaser.await.expect("releaser");
        let error = match result {
            Err(BenchError::Failed(error)) => error,
            Err(BenchError::DrainExpired(mut expired)) => {
                expired.drain().await;
                panic!("unexpected drain expiry: {}", expired.diagnostic);
            }
            Ok(_) => panic!("the batch must fail on its budget"),
        };
        if error == "bench serialization lock not acquired in time" {
            // A foreign test was queued ahead of this batch.
            println!("[bench] budget test attempt {attempt}: foreign lock holder, retrying");
            continue;
        }
        let m = *milestones.lock().expect("milestones");
        let deadline_at = m.deadline_at.expect("deadline_at");
        let acquired = m.acquired.expect("acquired");
        let observed = m.expired_observed.expect("expiry observed");
        assert!(error.contains("timed out"), "{error}");
        assert!(acquired >= third_released, "the batch acquired late");
        assert!(observed >= deadline_at, "never expires early");
        assert!(
            observed - acquired <= budget.mul_f64(0.4) + tolerance,
            "expired {:?} after a late acquisition; a shared deadline allows {:?}",
            observed - acquired,
            budget.mul_f64(0.4) + tolerance
        );
        assert_eq!(
            m.grace_at,
            Some(deadline_at + grace),
            "grace_at derives from deadline_at"
        );
        return;
    }
    panic!("five attempts in a row lost the lock to a foreign test");
}

/// p3 test 8: the p1 batch of 13 on both fixtures, through the read side.
/// Without AC_SELECTION_BENCH it asserts collection and correlation; the
/// latency gates are computed over 5 interleaved serial runs per side
/// (p3 measurement protocol), from the lines printed under the variable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_batch_of_thirteen_reads_drops_queue_wait() {
    for (r, m) in [(8usize, 1usize), (40, 3)] {
        let bench = bench_fixture(r, m);
        let run = bench_batch(&bench).await;
        assert_bench_correlation(&run);
        if bench_measurement_mode() {
            println!(
                "[bench-p3] R={r} M={m} batch_wall_ms={:.1} sum_queue_wait_ms={:.1} \
                occupancy_max={}",
                run.batch_wall_ms,
                bench_sum_queue_wait_ms(&run),
                run.occupancy_max
            );
        }
    }
}

/// p3 test 9: in the batch of 13, read bodies are inside the turn at the
/// same time. The occupancy object belongs to this run only, and is read
/// after all of its work has been awaited. With the old Mutex it is 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_concurrent_read_bodies_occupy_the_turn_simultaneously() {
    let bench = bench_fixture(8, 1);
    let run = bench_batch(&bench).await;
    assert_bench_correlation(&run);
    assert!(
        run.occupancy_max >= 2,
        "read bodies must overlap: occupancy.max() = {}",
        run.occupancy_max
    );
}
