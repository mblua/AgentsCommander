use super::*;

#[tokio::test]
async fn secondary_rate_limit_backs_off_one_minute() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(&[repo]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        gh.ci.push_back(Ok(secondary_limit_output()));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    tick(&mut now, &mut wall, 30);
    harness.round(now, wall).await;

    let key = QueryKey {
        nwo: "mblua/AgentsCommander".to_string(),
        sha40: sha_of('a'),
        branch: "main".to_string(),
    };
    let due_at = harness
        .sweeper
        .lock_state()
        .keys
        .get(&key)
        .expect("key state")
        .ci
        .next_due
        .expect("due time");
    assert_eq!(
        due_at,
        now + Duration::from_secs(60),
        "a secondary limit is a burst signal, not an exhausted quota"
    );
}

/// #2329: a failing CI query on the resolved default branch keeps the chip
/// idle instead of falling back to Unknown, but the failure itself is not
/// swallowed: the per-key backoff and the round gate match the kind, and the
/// early next round issues no call at all. `branchStalenessEnabled` is off
/// so the ONLY failure kind in play is the CI one.
#[tokio::test]
async fn default_branch_ci_failure_keeps_idle_chip_and_arms_the_backoff() {
    let _guard = round_test_lock().await;
    for (failure, expected) in [
        (primary_limit_output(), BACKOFF_CAP),
        (secondary_limit_output(), SECONDARY_BACKOFF_BASE),
    ] {
        let harness = Harness::new(AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        });
        harness.set_jitter(0.0);
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Idle,
            "round 1: the resolved default branch is suppressed"
        );
        assert!(harness.drain_transitions().is_empty());

        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ci
            .push_back(Ok(failure));
        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Idle,
            "a failed default-branch CI query keeps the idle chip"
        );
        assert!(
            harness.drain_transitions().is_empty(),
            "no CI edge is ever published for the default branch"
        );
        let key = QueryKey {
            nwo: "mblua/AgentsCommander".to_string(),
            sha40: sha_of('a'),
            branch: "main".to_string(),
        };
        {
            let state = harness.sweeper.lock_state();
            let entry = state.keys.get(&key).expect("key state");
            assert_eq!(
                entry.ci.failure_interval,
                Some(expected),
                "the failure kind schedules its own retry"
            );
            assert_eq!(
                entry.ci.next_due,
                Some(now + expected),
                "the per-key retry is due on the failure interval"
            );
            assert_eq!(entry.ci.confirmed, None, "confirmed stays cleared");
            assert!(
                state.warned_failures.contains(&(key.clone(), Axis::Ci)),
                "warn_failure ran for the CI axis"
            );
            assert_eq!(
                state.throttled_until,
                Some(now + expected),
                "the round gate is armed for the kind: 900s primary, 60s secondary"
            );
        }

        let calls_before = harness.gh_call_count();
        tick(&mut now, &mut wall, 30);
        harness.round(now, wall).await;
        assert_eq!(
            harness.gh_call_count(),
            calls_before,
            "the early next round is gated and issues no call"
        );
        assert!(
            harness.drain_transitions().is_empty(),
            "a gated round changes no chip"
        );
    }
}

#[test]
fn failure_kind_classifies_the_three_403_texts() {
    assert_eq!(
        failure_kind(&primary_limit_output()),
        FailureKind::RateLimited
    );
    assert_eq!(
        failure_kind(&secondary_limit_output()),
        FailureKind::SecondaryRateLimited
    );
    assert_eq!(
        failure_kind(&GhCallOutput {
            stdout: String::new(),
            stderr: "You have triggered an abuse detection mechanism".to_string(),
            success: false,
        }),
        FailureKind::SecondaryRateLimited
    );
}

#[tokio::test]
async fn budget_starved_key_is_not_queried_and_keeps_its_chip() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let _paths = two_repos(&harness);
    // One resolution plus two axis queries: exactly one key fits.
    harness.set_budget(3.0);

    let now = Instant::now();
    let wall = Local::now();
    harness.round(now, wall).await;

    assert_eq!(
        compare_count(&harness, "repo-b"),
        0,
        "the starved key must issue no gh call at all"
    );
    assert_eq!(ci_count_for(&harness, "repo-b"), 0);
    assert_eq!(
        ci_count(&harness),
        1,
        "exactly the admitted key's CI query was issued"
    );
    assert!(
        ci_next_due(&harness, "repo-b").is_none(),
        "a key without budget never fails and never gets a due time"
    );
    assert_eq!(
        harness
            .snapshot()
            .values()
            .filter(|activity| activity.ci != CiState::Unknown)
            .count(),
        1,
        "the starved path keeps its transparent chip"
    );

    // Positive control: the identical fixture with a full bucket queries both.
    let full = Harness::new(AppSettings::default());
    let _paths = two_repos(&full);
    full.set_budget(GH_BUDGET_BURST);
    full.round(now, wall).await;
    assert_eq!(ci_count_for(&full, "repo-a"), 1);
    assert_eq!(ci_count_for(&full, "repo-b"), 1);
}

#[tokio::test]
async fn running_ci_wins_the_last_token() {
    let _guard = round_test_lock().await;
    for budget in [4.0, 1.0] {
        let harness = Harness::new(AppSettings::default());
        let first = harness.repo("a");
        let second = harness.repo("z");
        harness.set_work(&[first.clone(), second.clone()]);
        {
            let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
            git.set_origin(&first, "git@github.com:mblua/repo-a.git");
            git.set_origin(&second, "git@github.com:mblua/repo-z.git");
        }
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.route(
                &format!(
                    "repos/mblua/repo-z/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))),
            );
            if budget == 4.0 {
                gh.route(
                    &format!(
                        "repos/mblua/repo-z/actions/runs?head_sha={}&per_page=100",
                        sha_of('a')
                    ),
                    Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))),
                );
            }
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        assert_eq!(ci_count_for(&harness, "repo-a"), 1);
        assert_eq!(ci_count_for(&harness, "repo-z"), 1);

        // The `Running` key is made to lose every fallback: it is last
        // alphabetically and it was confirmed most recently.
        {
            let mut state = harness.sweeper.lock_state();
            let key = QueryKey {
                nwo: "mblua/repo-z".to_string(),
                sha40: sha_of('a'),
                branch: "main".to_string(),
            };
            let entry = state.keys.get_mut(&key).expect("running key state");
            assert_eq!(entry.ci.confirmed, Some(CiState::Running));
            entry.ci.last_confirmed_at = Some(wall + chrono::Duration::seconds(1));
        }

        tick(&mut now, &mut wall, 30);
        // Both nwo are resolved, staleness is not due: one token, one CI query.
        harness.set_budget(budget);
        let z_before = {
            let state = harness.sweeper.lock_state();
            let ci = &state.keys[&QueryKey {
                nwo: "mblua/repo-z".to_string(),
                sha40: sha_of('a'),
                branch: "main".to_string(),
            }]
                .ci;
            (ci.failure, ci.next_due)
        };
        harness.round(now, wall).await;
        if budget == 1.0 {
            let state = harness.sweeper.lock_state();
            let ci = &state.keys[&QueryKey {
                nwo: "mblua/repo-z".to_string(),
                sha40: sha_of('a'),
                branch: "main".to_string(),
            }]
                .ci;
            assert_eq!(
                (ci.failure, ci.next_due),
                z_before,
                "unadmitted z preserves failure and due time"
            );
        }

        assert_eq!(
            ci_count_for(&harness, "repo-z"),
            if budget == 4.0 { 2 } else { 1 },
            "a Running key requires the full reservation"
        );
        assert_eq!(
            ci_count_for(&harness, "repo-a"),
            if budget == 4.0 { 1 } else { 2 },
            "an affordable Idle key can proceed when Running cannot pay"
        );
        assert_eq!(
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .count("/actions/runs/"),
            0
        );
        if budget == 4.0 {
            assert_eq!(harness.sweeper.lock_state().budget_tokens, 3.0);
        }
    }
}

#[tokio::test]
async fn a_secondary_limit_silences_the_next_round() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    harness.set_jitter(0.0);
    let _paths = two_repos(&harness);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.route(
            &format!(
                "repos/mblua/repo-a/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(secondary_limit_output()),
        );
        gh.route(
            &format!(
                "repos/mblua/repo-a/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&ci_body(&sha_of('a'), &[]))),
        );
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    let after_first = harness.gh_call_count();
    let emitted = harness.emitted_count();
    assert_eq!(
        throttled_until(&harness),
        Some(now + Duration::from_secs(60)),
        "the gate is armed for the whole process, not just the failing key"
    );
    assert!(ci_count_for(&harness, "repo-b") > 0);
    let healthy_before = ci_count_for(&harness, "repo-b");

    tick(&mut now, &mut wall, 30);
    harness.round(now, wall).await;
    assert_eq!(
        harness.gh_call_count(),
        after_first,
        "the healthy key was due, and the gate still issued nothing"
    );
    assert_eq!(
        harness.emitted_count(),
        emitted,
        "a gated round changes no chip"
    );

    tick(&mut now, &mut wall, 31);
    harness.round(now, wall).await;
    assert!(
        ci_count_for(&harness, "repo-b") > healthy_before,
        "the gate opens after the armed interval"
    );
}

#[tokio::test]
async fn jitter_spans_the_half_open_range() {
    let _guard = round_test_lock().await;
    let now = Instant::now();
    let wall = Local::now();

    let low = Harness::new(AppSettings::default());
    low.set_jitter(0.0);
    let repo = low.repo("repo-a");
    low.set_work(&[repo]);
    low.gh
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .ci
        .push_back(Ok(secondary_limit_output()));
    low.round(now, wall).await;
    assert_eq!(
        throttled_until(&low),
        Some(now + SECONDARY_BACKOFF_BASE),
        "the low end of the range is closed"
    );

    let high = Harness::new(AppSettings::default());
    high.set_jitter(1.0 - f64::EPSILON);
    let repo = high.repo("repo-a");
    high.set_work(&[repo]);
    high.gh
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .ci
        .push_back(Ok(secondary_limit_output()));
    high.round(now, wall).await;
    let gate = throttled_until(&high).expect("armed gate");
    assert!(gate > now + SECONDARY_BACKOFF_BASE);
    assert!(
        gate < now + 2 * SECONDARY_BACKOFF_BASE,
        "the high end of the range is open"
    );
}

#[tokio::test]
async fn a_primary_limit_closes_the_gate_for_fifteen_minutes() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    harness.set_jitter(0.0);
    let _paths = two_repos(&harness);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.route(
            &format!(
                "repos/mblua/repo-a/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(primary_limit_output()),
        );
        gh.route(
            &format!(
                "repos/mblua/repo-a/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&ci_body(&sha_of('a'), &[]))),
        );
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    assert_eq!(
        throttled_until(&harness),
        Some(now + BACKOFF_CAP),
        "an exhausted quota closes the gate for the cap"
    );
    let after_first = harness.gh_call_count();
    let healthy_before = ci_count_for(&harness, "repo-b");

    tick(&mut now, &mut wall, 899);
    harness.round(now, wall).await;
    assert_eq!(harness.gh_call_count(), after_first, "still gated at +899s");

    tick(&mut now, &mut wall, 2);
    harness.round(now, wall).await;
    assert!(
        ci_count_for(&harness, "repo-b") > healthy_before,
        "the healthy key resumes once the gate opens"
    );
}

#[tokio::test]
async fn sustained_spend_never_exceeds_the_ceiling() {
    let _guard = round_test_lock().await;
    let (harness, _paths) = budget_fixture();

    let mut now = Instant::now();
    let mut wall = Local::now();
    let mut totals: Vec<usize> = Vec::new();
    for step in 0..12 {
        if step > 0 {
            tick(&mut now, &mut wall, INTERVAL_FLOOR_SECS);
        }
        harness.round(now, wall).await;
        totals.push(harness.gh_call_count());
    }

    // Every call is reserved before it is issued and the bucket is clamped,
    // so a window of T seconds can spend at most one burst plus T's refill.
    let bound_60 = GH_BUDGET_BURST + 60.0 * MAX_GH_CALLS_PER_MINUTE / 60.0;
    let bound_120 = GH_BUDGET_BURST + 120.0 * MAX_GH_CALLS_PER_MINUTE / 60.0;
    let first_minute = totals[5];
    let second_minute = totals[11] - totals[5];
    assert!(
        first_minute as f64 <= bound_60,
        "{first_minute} calls in [0, 60) exceeds {bound_60}"
    );
    assert!(
        second_minute as f64 <= bound_60,
        "{second_minute} calls in [60, 120) exceeds {bound_60}"
    );
    assert!(
        totals[11] as f64 <= bound_120,
        "{} calls in [0, 120) exceeds {bound_120}",
        totals[11]
    );
    // Positive control: a sweeper that issues nothing must not pass.
    assert!(totals[11] >= 60, "only {} calls issued", totals[11]);
}

#[tokio::test]
async fn starved_keys_are_served_in_a_later_round() {
    let _guard = round_test_lock().await;
    let (harness, _paths) = budget_fixture();

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;

    let first_unqueried = (0..BUDGET_FIXTURE_REPOS)
        .map(fixture_nwo)
        .find(|nwo| ci_count_for(&harness, nwo) == 0)
        .expect("the cold-start bucket cannot pay for every key");

    for _ in 0..6 {
        tick(&mut now, &mut wall, INTERVAL_FLOOR_SECS);
        harness.round(now, wall).await;
    }
    assert!(
        ci_count_for(&harness, &first_unqueried) > 0,
        "{first_unqueried} was first in line and still unserved after a minute"
    );

    for _ in 0..30 {
        tick(&mut now, &mut wall, INTERVAL_FLOOR_SECS);
        harness.round(now, wall).await;
    }
    for index in 0..BUDGET_FIXTURE_REPOS {
        let nwo = fixture_nwo(index);
        assert!(
            ci_count_for(&harness, &nwo) > 0,
            "{nwo} was never queried in 360s"
        );
    }
}

#[test]
fn missing_gh_spawns_no_process() {
    let probes = Arc::new(AtomicUsize::new(0));
    let spawns = Arc::new(AtomicUsize::new(0));
    let probe: GhProbe = {
        let probes = Arc::clone(&probes);
        Arc::new(move || {
            probes.fetch_add(1, Ordering::SeqCst);
            None
        })
    };
    let spawner: GhSpawner = {
        let spawns = Arc::clone(&spawns);
        Arc::new(move |_spec| {
            spawns.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(FailureKind::Other) })
        })
    };
    let local_git: LocalGitRunner =
        Arc::new(|_path, _args| Box::pin(async { Err("unused".to_string()) }));
    let settings: SettingsState =
        crate::config::settings::new_settings_state(AppSettings::default());
    let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (sweeper, _receiver) = RemoteSweeper::new(
        sessions,
        settings,
        Box::new(|_payload| {}),
        RemoteSweeperSeams {
            probe,
            spawner,
            local_git,
            jitter: Arc::new(|| 0.0),
            snapshot_dir: None,
            publication_clock: Arc::new(Utc::now),
        },
    );
    let shutdown = ShutdownSignal::new();

    let handle = sweeper.start(shutdown).expect("thread handle");
    handle.join().expect("thread returns after one probe");
    assert_eq!(probes.load(Ordering::SeqCst), 1, "never probes again");
    assert_eq!(spawns.load(Ordering::SeqCst), 0);
}

#[test]
fn start_does_no_io_on_the_callers_thread() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let spawns = Arc::new(AtomicUsize::new(0));
    let probe: GhProbe = {
        let barrier = Arc::clone(&barrier);
        Arc::new(move || {
            let _ = entered_tx.send(());
            barrier.wait();
            Some(PathBuf::from("gh"))
        })
    };
    let spawner: GhSpawner = {
        let spawns = Arc::clone(&spawns);
        Arc::new(move |_spec| {
            spawns.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(FailureKind::Other) })
        })
    };
    let local_git: LocalGitRunner =
        Arc::new(|_path, _args| Box::pin(async { Err("unused".to_string()) }));
    let settings: SettingsState =
        crate::config::settings::new_settings_state(AppSettings::default());
    let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (sweeper, _receiver) = RemoteSweeper::new(
        sessions,
        settings,
        Box::new(|_payload| {}),
        RemoteSweeperSeams {
            probe,
            spawner,
            local_git,
            jitter: Arc::new(|| 0.0),
            snapshot_dir: None,
            publication_clock: Arc::new(Utc::now),
        },
    );
    let shutdown = ShutdownSignal::new();

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let started_sweeper = Arc::clone(&sweeper);
    let started_shutdown = shutdown.clone();
    let starter = std::thread::spawn(move || {
        let handle = started_sweeper.start(started_shutdown);
        started_tx.send(handle.is_some()).expect("send");
        handle
    });

    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("start must return while the probe is still blocked");
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the probe runs on the sweeper thread");
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        0,
        "start must not spawn anything on the caller's thread"
    );

    barrier.wait();
    shutdown.trigger();
    let handle = starter.join().expect("starter thread").expect("handle");
    handle.join().expect("sweeper thread exits on shutdown");
}
