use super::*;

#[tokio::test]
async fn i2768_closing_list_changes_restart_candidate() {
    let _guard = round_test_lock().await;
    for change in ["new-completed", "new-active", "metadata", "attempt"] {
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        h.round(now, wall).await;
        i2768_list(&h, vec![]);
        queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        let mut first = i2768_row(1, "completed");
        let mut closing = vec![first.clone()];
        match change {
            "metadata" => {
                first["updated_at"] = serde_json::json!("2026-10-01T00:00:01Z");
                closing = vec![first.clone()];
            }
            "attempt" => {
                first["run_attempt"] = serde_json::json!(2);
                closing = vec![first.clone()];
            }
            "new-active" => closing.push(i2768_row(2, "in_progress")),
            _ => closing.push(i2768_row(2, "completed")),
        }
        i2768_list(&h, closing);
        if change != "new-active" {
            queue_completion(&h, &i2768_key().nwo, first.clone());
        }
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
        if change == "new-active" {
            assert!(h.sweeper.lock_state().keys[&i2768_key()]
                .ci
                .completion
                .proofs
                .is_empty());
            i2768_list(&h, vec![first.clone(), i2768_row(2, "completed")]);
            queue_completion(&h, &i2768_key().nwo, first.clone());
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
        }
        if change.starts_with("new") {
            assert_eq!(
                h.sweeper.lock_state().keys[&i2768_key()]
                    .ci
                    .completion
                    .proofs
                    .len(),
                1
            );
            i2768_list(&h, vec![first.clone(), i2768_row(2, "completed")]);
            queue_completion(&h, &i2768_key().nwo, i2768_row(2, "completed"));
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
        }
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 1);
        i2768_assert_consumed(&h);
    }
}
#[tokio::test]
async fn i2768_lifecycle_isolation() {
    let _guard = round_test_lock().await;
    let (h, repo) = i2768_harness();
    let alias = h.repo("alias");
    h.set_work(&[repo.clone(), alias.clone()]);
    let mut now = Instant::now();
    let mut wall = Local::now();
    i2768_list(&h, vec![]);
    h.round(now, wall).await;
    i2768_list(&h, vec![i2768_row(1, "in_progress")]);
    tick(&mut now, &mut wall, 60);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 2, 0);
    let before = {
        let state = h.sweeper.lock_state();
        let ci = &state.keys[&i2768_key()].ci;
        (
            ci.chip,
            ci.confirmed,
            ci.last_confirmed_at,
            ci.next_due,
            ci.failure_interval,
            ci.failure,
            ci.run_ids.clone(),
            ci.pull_requests.clone(),
            ci.completion.clone(),
        )
    };
    h.sweeper.settings.write().await.ci_activity_enabled = false;
    let calls = h.gh_call_count();
    tick(&mut now, &mut wall, 100);
    h.round(now, wall).await;
    {
        let state = h.sweeper.lock_state();
        let ci = &state.keys[&i2768_key()].ci;
        assert_eq!(
            (
                ci.chip,
                ci.confirmed,
                ci.last_confirmed_at,
                ci.next_due,
                ci.failure_interval,
                ci.failure,
                ci.run_ids.clone(),
                ci.pull_requests.clone(),
                ci.completion.clone()
            ),
            before
        );
    }
    assert_eq!(h.gh_call_count(), calls);
    i2768_assert_edges(&h, 0, 0);
    h.sweeper.settings.write().await.ci_activity_enabled = true;
    i2768_list(&h, vec![]);
    queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
    h.round(now, wall).await;
    i2768_assert_running(&h, &repo);
    i2768_list(&h, vec![]);
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 2);
    i2768_assert_consumed(&h);
    // Distinct heads and branches coexist without carrying CI evidence.
    h.git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_head_sha(&alias, 'b');
    publish_branch(&alias, "feature");
    queue_endpoint(
        &h,
        &format!(
            "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
            sha_of('b')
        ),
        Ok(ok_output(&runs_body(vec![run_row(
            2,
            &sha_of('b'),
            "feature",
            "in_progress",
            &[],
        )]))),
    );
    tick(&mut now, &mut wall, 1);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 0);
    let second = QueryKey {
        sha40: sha_of('b'),
        branch: "feature".to_string(),
        ..i2768_key()
    };
    {
        let state = h.sweeper.lock_state();
        assert_eq!(state.keys.len(), 2);
        assert_eq!(
            state.keys[&second]
                .ci
                .completion
                .known
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(
            state.keys[&i2768_key()]
                .ci
                .completion
                .known
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![1]
        );
    }
    // Suppression clears the ledger and both directions of the chain.
    h.sweeper
        .lock_state()
        .base_branches
        .insert(i2768_key().nwo, "feature".to_string());
    queue_endpoint(
        &h,
        &format!(
            "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
            sha_of('b')
        ),
        Ok(ok_output(&runs_body(vec![run_row(
            2,
            &sha_of('b'),
            "feature",
            "in_progress",
            &[],
        )]))),
    );
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    {
        let state = h.sweeper.lock_state();
        let ci = &state.keys[&second].ci;
        assert!(ci.completion.known.is_empty());
        assert_eq!(ci.confirmed, None);
    }
    h.sweeper.settings.write().await.ci_activity_enabled = false;
    h.set_work(&[]);
    h.round(now, wall).await;
    assert!(h.sweeper.lock_state().keys.is_empty());
    i2768_assert_edges(&h, 0, 0);
    i2768_assert_consumed(&h);
    let restarted = Harness::new(AppSettings::default());
    assert!(restarted.sweeper.lock_state().keys.is_empty());
}

#[tokio::test]
async fn i2768_budget_accounts_for_corroboration() {
    let _guard = round_test_lock().await;
    for point in ["active", "A", "B", "suppressed", "closing"] {
        let (h, _) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        h.round(now, wall).await;
        tick(&mut now, &mut wall, 10);
        h.set_budget(3.0);
        let calls = h.gh_call_count();
        let due = h.sweeper.lock_state().keys[&i2768_key()].ci.next_due;
        h.round(now, wall).await;
        assert_eq!(h.gh_call_count(), calls);
        assert_eq!(h.sweeper.lock_state().keys[&i2768_key()].ci.next_due, due);
        h.set_budget(4.0);
        if point == "active" || point == "suppressed" {
            if point == "suppressed" {
                h.sweeper
                    .lock_state()
                    .base_branches
                    .insert(i2768_key().nwo, "main".to_string());
            }
            i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        } else {
            i2768_list(&h, vec![]);
            if point == "closing" {
                queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
            } else {
                queue_endpoint(
                    &h,
                    "repos/mblua/AgentsCommander/actions/runs/1",
                    if point == "A" {
                        Err(FailureKind::Timeout)
                    } else {
                        Ok(ok_output(&i2768_row(1, "completed").to_string()))
                    },
                );
                if point == "B" {
                    queue_endpoint(
                        &h,
                        "repos/mblua/AgentsCommander/actions/runs/1/attempts/1/jobs?per_page=100",
                        Err(FailureKind::Timeout),
                    );
                }
            }
        }
        h.round(now, wall).await;
        let expected = match point {
            "A" => 2.0,
            "B" => 1.0,
            "closing" => 0.0,
            _ => 3.0,
        };
        assert_eq!(h.sweeper.lock_state().budget_tokens, expected);
        if point == "closing" {
            h.set_budget(4.0);
            i2768_list(&h, vec![]);
            tick(&mut now, &mut wall, 10);
            h.set_budget(4.0);
            h.round(now, wall).await;
            assert_eq!(h.sweeper.lock_state().budget_tokens, 3.0);
            i2768_assert_edges(&h, 0, 1);
        } else {
            i2768_assert_edges(&h, 0, 0);
        }
        i2768_assert_consumed(&h);
    }
    // Three Running keys contend with an Idle key in a finite fixture.
    let mut h = Harness::new(AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    });
    let inflight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let s = Arc::get_mut(&mut h.sweeper).unwrap();
        let original = Arc::clone(&s.spawner);
        let live = Arc::clone(&inflight);
        let max = Arc::clone(&peak);
        s.spawner = Arc::new(move |spec| {
            let original = Arc::clone(&original);
            let live = Arc::clone(&live);
            let max = Arc::clone(&max);
            Box::pin(async move {
                let count = live.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                max.fetch_max(count, std::sync::atomic::Ordering::SeqCst);
                tokio::task::yield_now().await;
                let result = original(spec).await;
                live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                result
            })
        });
    }
    let paths = ["a", "b", "c", "z"].map(|name| {
        let path = h.repo(name);
        h.git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_origin(&path, &format!("git@github.com:mblua/{name}.git"));
        path
    });
    h.set_work(&paths);
    let list = |name: &str| {
        format!(
            "repos/mblua/{name}/actions/runs?head_sha={}&per_page=100",
            sha_of('a')
        )
    };
    for name in ["a", "b", "c", "z"] {
        queue_endpoint(&h, &format!("repos/mblua/{name}"), Err(FailureKind::Other));
        queue_endpoint(
            &h,
            &list(name),
            Ok(ok_output(&runs_body(if name == "z" {
                vec![]
            } else {
                vec![i2768_row(1, "in_progress")]
            }))),
        );
    }
    let mut now = Instant::now();
    let mut wall = Local::now();
    h.round(now, wall).await;
    tick(&mut now, &mut wall, 60);
    h.set_budget(3.0);
    queue_endpoint(&h, &list("z"), Ok(ok_output(&runs_body(vec![]))));
    let calls = h.gh_call_count();
    h.round(now, wall).await;
    assert_eq!(h.gh_call_count(), calls + 1);
    for (closing, proof) in [
        (None, Some("a")),
        (Some("a"), Some("b")),
        (Some("b"), Some("c")),
        (Some("c"), None),
    ] {
        if closing == Some("c") {
            queue_endpoint(&h, &list("z"), Ok(ok_output(&runs_body(vec![]))));
        }
        if let Some(name) = closing {
            queue_endpoint(&h, &list(name), Ok(ok_output(&runs_body(vec![]))));
        }
        if let Some(name) = proof {
            queue_endpoint(
                &h,
                &list(name),
                Ok(ok_output(&runs_body(vec![i2768_row(1, "completed")]))),
            );
            queue_completion(&h, &format!("mblua/{name}"), i2768_row(1, "completed"));
        }
        tick(&mut now, &mut wall, 10);
        let eligible: Vec<_> = {
            let state = h.sweeper.lock_state();
            state
                .keys
                .iter()
                .filter(|(_, entry)| entry.ci.next_due.is_none_or(|due| due <= now))
                .map(|(key, _)| key.nwo.clone())
                .collect()
        };
        let calls_before = h.gh_call_count();
        h.round(now, wall).await;
        assert!(h.sweeper.lock_state().budget_tokens >= 0.0);
        let state = h.sweeper.lock_state();
        println!(
            "contention fixture calls={} tokens={} deferred={:?}",
            h.gh_call_count(),
            state.budget_tokens,
            eligible
                .into_iter()
                .filter(|nwo| {
                    let endpoint = format!(
                        "repos/{nwo}/actions/runs?head_sha={}&per_page=100",
                        sha_of('a')
                    );
                    !h.gh.lock().unwrap_or_else(|e| e.into_inner()).calls[calls_before..]
                        .iter()
                        .any(|call| call.get(1) == Some(&endpoint))
                })
                .collect::<Vec<_>>()
        );
    }
    i2768_assert_edges(&h, 0, 3);
    for path in &paths {
        assert_eq!(h.snapshot()[path].ci, CiState::Idle);
    }
    assert_eq!(peak.load(std::sync::atomic::Ordering::SeqCst), 2);
    i2768_assert_consumed(&h);
}
#[test]
fn i2768_scripts_route_exactly_and_panic() {
    let mut scripts = GhScripts::default();
    for (endpoint, text) in [
        ("repos/mblua/AgentsCommander/actions/runs/5", "five"),
        ("repos/mblua/AgentsCommander/actions/runs/50", "fifty"),
        (
            "repos/mblua/AgentsCommander/actions/runs/5/attempts/1/jobs?per_page=100",
            "jobs",
        ),
        ("repos/mblua/AgentsCommander", "repo"),
    ] {
        scripts.route(endpoint, Ok(ok_output(text)));
    }
    let gh = Path::new("gh");
    let nwo = "mblua/AgentsCommander";
    for (query, text) in [
        (GhQuery::CiRun { nwo, run_id: 50 }, "fifty"),
        (
            GhQuery::CiAttemptJobs {
                nwo,
                run_id: 5,
                attempt: 1,
            },
            "jobs",
        ),
        (GhQuery::CiRun { nwo, run_id: 5 }, "five"),
        (GhQuery::RepoInfo { nwo }, "repo"),
    ] {
        assert_eq!(
            scripts
                .next(&build_gh_command_spec(gh, query).unwrap())
                .unwrap()
                .stdout,
            text
        );
    }
    assert_eq!(scripts.count_repo_info(), 1);
    assert!(scripts.routes.values().all(VecDeque::is_empty));
    for query in [
        GhQuery::CiRun { nwo, run_id: 0 },
        GhQuery::CiAttemptJobs {
            nwo,
            run_id: 0,
            attempt: 1,
        },
        GhQuery::CiAttemptJobs {
            nwo,
            run_id: 5,
            attempt: 0,
        },
        GhQuery::CiRun {
            nwo: "bad",
            run_id: 5,
        },
    ] {
        assert!(build_gh_command_spec(gh, query).is_err());
    }
    let detail = build_gh_command_spec(gh, GhQuery::CiRun { nwo, run_id: 5 }).unwrap();
    assert_eq!(
        detail.envs,
        vec![
            ("GH_PROMPT_DISABLED".to_string(), "1".to_string()),
            ("GH_NO_UPDATE_NOTIFIER".to_string(), "1".to_string()),
            ("GH_PAGER".to_string(), "cat".to_string())
        ]
    );
}

#[test]
#[should_panic(expected = "missing gh route")]
fn i2768_scripts_missing_detail_panics() {
    let mut scripts = GhScripts::default();
    scripts
        .next(
            &build_gh_command_spec(
                Path::new("gh"),
                GhQuery::CiRun {
                    nwo: "mblua/AgentsCommander",
                    run_id: 5,
                },
            )
            .unwrap(),
        )
        .unwrap();
}

#[test]
#[should_panic(expected = "missing gh route")]
fn i2768_scripts_missing_jobs_panics() {
    let mut scripts = GhScripts::default();
    scripts
        .next(
            &build_gh_command_spec(
                Path::new("gh"),
                GhQuery::CiAttemptJobs {
                    nwo: "mblua/AgentsCommander",
                    run_id: 5,
                    attempt: 1,
                },
            )
            .unwrap(),
        )
        .unwrap();
}

#[test]
#[should_panic(expected = "exhausted gh route")]
fn i2768_scripts_exhausted_detail_panics() {
    let mut scripts = GhScripts::default();
    let spec = build_gh_command_spec(
        Path::new("gh"),
        GhQuery::CiRun {
            nwo: "mblua/AgentsCommander",
            run_id: 5,
        },
    )
    .unwrap();
    scripts.route(&spec.args[1], Ok(ok_output("{}")));
    scripts.next(&spec).unwrap();
    scripts.next(&spec).unwrap();
}

#[test]
#[should_panic(expected = "exhausted gh route")]
fn i2768_scripts_exhausted_jobs_panics() {
    let mut scripts = GhScripts::default();
    let spec = build_gh_command_spec(
        Path::new("gh"),
        GhQuery::CiAttemptJobs {
            nwo: "mblua/AgentsCommander",
            run_id: 5,
            attempt: 1,
        },
    )
    .unwrap();
    scripts.route(&spec.args[1], Ok(ok_output("{}")));
    scripts.next(&spec).unwrap();
    scripts.next(&spec).unwrap();
}
#[tokio::test]
async fn i2768_merge_keeps_active_rows_after_stale_attempt() {
    let _guard = round_test_lock().await;
    for bad_first in [true, false] {
        let (h, repo) = i2768_harness();
        h.sweeper.settings.write().await.ci_sweep_min_interval_secs = 10;
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![]);
        h.round(now, wall).await;
        let mut active2 = i2768_row(5, "in_progress");
        active2["run_attempt"] = serde_json::json!(2);
        i2768_list(&h, vec![active2]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        let stale = i2768_row(5, "completed");
        let fresh = i2768_row(50, "in_progress");
        i2768_list(
            &h,
            if bad_first {
                vec![stale, fresh]
            } else {
                vec![fresh, stale]
            },
        );
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 1, 0);
        assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
        {
            let state = h.sweeper.lock_state();
            let ci = &state.keys[&i2768_key()].ci;
            assert_eq!(ci.failure, Some(FailureKind::Incomplete));
            assert_eq!(ci.completion.known[&5].run_attempt, 2);
            assert!(ci.completion.known.contains_key(&50));
            assert!(ci.completion.proofs.is_empty());
            assert!(!ci.completion.closing_ready);
            assert_eq!(ci.completion.active_ids, vec![50]);
            assert_eq!(ci.completion.active_prs, vec![150]);
        }
        let mut done2 = i2768_row(5, "completed");
        done2["run_attempt"] = serde_json::json!(2);
        i2768_list(&h, vec![done2.clone()]);
        queue_completion(&h, &i2768_key().nwo, done2.clone());
        tick(&mut now, &mut wall, 20);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
        i2768_list(&h, vec![done2.clone()]);
        queue_endpoint(
            &h,
            "repos/mblua/AgentsCommander/actions/runs/50",
            Ok(ok_output(&i2768_row(50, "in_progress").to_string())),
        );
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
        {
            let state = h.sweeper.lock_state();
            let ci = &state.keys[&i2768_key()].ci;
            assert_eq!(ci.run_ids, vec![50]);
            assert_eq!(ci.pull_requests, vec![150]);
            assert!(ci.completion.proofs.is_empty());
            assert!(!ci.completion.closing_ready);
        }
        assert_eq!(
            i2768_details(&h),
            vec![
                "repos/mblua/AgentsCommander/actions/runs/5",
                "repos/mblua/AgentsCommander/actions/runs/5/attempts/2/jobs?per_page=100",
                "repos/mblua/AgentsCommander/actions/runs/5",
                "repos/mblua/AgentsCommander/actions/runs/50",
            ]
        );
        for row in [done2.clone(), i2768_row(50, "completed")] {
            i2768_list(&h, vec![done2.clone()]);
            queue_completion(&h, &i2768_key().nwo, row);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            i2768_assert_running(&h, &repo);
        }
        i2768_list(&h, vec![done2.clone()]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 1);
        assert_eq!(h.snapshot()[&repo].ci, CiState::Idle);
        i2768_list(&h, vec![done2]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_consumed(&h);
    }
}

#[test]
fn i2768_merge_errors_and_overflow_absorb_compatible_rows() {
    let parse =
        |row: serde_json::Value| parse_ci_run_response(&row.to_string(), &i2768_key()).unwrap();
    for reversed in [false, true] {
        let mut state = CiCompletionState::default();
        let mut initial = i2768_row(5, "completed");
        initial["run_attempt"] = serde_json::json!(2);
        let initial = parse(initial);
        let other = parse(i2768_row(60, "completed"));
        merge_ci_observations(&mut state, &[initial.clone(), other.clone()], true).unwrap();
        state.proofs.insert(5, initial.proof());
        state.proofs.insert(60, other.proof());
        state.closing_ready = true;
        let mut wrong = i2768_row(60, "completed");
        wrong["workflow_id"] = serde_json::json!(999);
        let mut rows = vec![
            parse(i2768_row(5, "completed")),
            parse(wrong),
            parse(i2768_row(50, "in_progress")),
        ];
        if reversed {
            rows.reverse();
        }
        assert_eq!(
            merge_ci_observations(&mut state, &rows, true),
            Err(FailureKind::Other)
        );
        assert_eq!(state.known[&5], initial);
        assert_eq!(state.known[&60], other);
        assert!(state.known.contains_key(&50));
        assert!(state.proofs.is_empty());
        assert!(!state.closing_ready);
        assert_eq!(state.active_ids, vec![50]);
    }
    let mut state = CiCompletionState::default();
    let initial: Vec<_> = (1..=100)
        .map(|id| parse(i2768_row(id, "completed")))
        .collect();
    merge_ci_observations(&mut state, &initial, true).unwrap();
    for run in &initial {
        state.proofs.insert(run.id, run.proof());
    }
    state.closing_ready = true;
    let mut higher = i2768_row(100, "in_progress");
    higher["run_attempt"] = serde_json::json!(2);
    assert_eq!(
        merge_ci_observations(
            &mut state,
            &[parse(i2768_row(101, "completed")), parse(higher)],
            true
        ),
        Err(FailureKind::Incomplete)
    );
    assert!(state.overflow);
    assert_eq!(state.known.len(), 100);
    assert!(!state.known.contains_key(&101));
    assert_eq!(state.known[&100].run_attempt, 2);
    assert_eq!(state.active_ids, vec![100]);
    assert!(state.proofs.is_empty());
    assert!(!state.closing_ready);
    assert_eq!(
        merge_ci_observations(&mut state, &[], true),
        Err(FailureKind::Incomplete)
    );
}

#[tokio::test]
async fn i2768_corroboration_limits_arm_the_global_gate() {
    let _guard = round_test_lock().await;
    for point in ["A", "B", "C"] {
        for (kind, delay) in [
            (FailureKind::RateLimited, 900),
            (FailureKind::SecondaryRateLimited, 60),
        ] {
            let (h, repo) = i2768_harness();
            h.set_jitter(0.0);
            let mut now = Instant::now();
            let mut wall = Local::now();
            i2768_list(&h, vec![i2768_row(1, "in_progress")]);
            h.round(now, wall).await;
            i2768_list(&h, vec![]);
            let detail = "repos/mblua/AgentsCommander/actions/runs/1";
            let jobs = "repos/mblua/AgentsCommander/actions/runs/1/attempts/1/jobs?per_page=100";
            if point != "A" {
                queue_endpoint(
                    &h,
                    detail,
                    Ok(ok_output(&i2768_row(1, "completed").to_string())),
                );
            }
            if point == "C" {
                queue_endpoint(
                    &h,
                    jobs,
                    Ok(ok_output(&ci_jobs_body(
                        1,
                        &sha_of('a'),
                        "main",
                        1,
                        &["completed"],
                    ))),
                );
            }
            queue_endpoint(
                &h,
                if point == "B" { jobs } else { detail },
                Ok(if kind == FailureKind::RateLimited {
                    primary_limit_output()
                } else {
                    secondary_limit_output()
                }),
            );
            tick(&mut now, &mut wall, 10);
            h.set_budget(4.0);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            assert_eq!(throttled_until(&h), Some(now + Duration::from_secs(delay)));
            assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
            let used = match point {
                "A" => 1,
                "B" => 2,
                _ => 3,
            };
            assert_eq!(h.sweeper.lock_state().budget_tokens, f64::from(3 - used));
            {
                let state = h.sweeper.lock_state();
                let ci = &state.keys[&i2768_key()].ci;
                assert_eq!(ci.failure, Some(kind));
                assert!(ci.completion.known.contains_key(&1));
                assert!(ci.completion.proofs.is_empty());
                assert!(!ci.completion.closing_ready);
            }
            // A separate eligible key proves this is an account gate, not
            // merely the failed key's own retry interval.
            let healthy = h.repo("healthy");
            h.git
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_origin(&healthy, "git@github.com:mblua/healthy.git");
            h.set_work(&[repo.clone(), healthy.clone()]);
            let before = h.gh_call_count();
            tick(&mut now, &mut wall, delay - 1);
            h.set_budget(30.0);
            h.round(now, wall).await;
            assert_eq!(
                h.gh_call_count(),
                before,
                "eligible healthy key is globally gated"
            );
            assert_eq!(throttled_until(&h), Some(now + Duration::from_secs(1)));
            queue_endpoint(&h, "repos/mblua/healthy", Err(FailureKind::Other));
            queue_endpoint(
                &h,
                &format!(
                    "repos/mblua/healthy/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&runs_body(vec![]))),
            );
            i2768_list(&h, vec![]);
            queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
            tick(&mut now, &mut wall, 1);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            assert_eq!(throttled_until(&h), None);
            assert_eq!(h.snapshot()[&healthy].ci, CiState::Idle);
            i2768_assert_running(&h, &repo);
            assert_eq!(h.gh_call_count(), before + 6);
            i2768_assert_consumed(&h);
        }
    }
}
