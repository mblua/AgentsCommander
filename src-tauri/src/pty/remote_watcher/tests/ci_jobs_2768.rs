use super::*;

#[tokio::test]
async fn i2768_missing_active_run_does_not_finish() {
    let _guard = round_test_lock().await;
    for empty in [false, true] {
        let (h, repo) = i2768_harness();
        h.sweeper.settings.write().await.ci_sweep_min_interval_secs = 10;
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![]);
        h.round(now, wall).await;
        i2768_list(
            &h,
            vec![i2768_row(5, "completed"), i2768_row(50, "in_progress")],
        );
        tick(&mut now, &mut wall, 60);
        h.round(now, wall).await;
        let mut expected = Vec::new();
        for _ in 0..2 {
            queue_completion(&h, &i2768_key().nwo, i2768_row(5, "completed"));
            queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/50",
                Ok(ok_output(&i2768_row(50, "in_progress").to_string())),
            );
            for _ in 0..2 {
                i2768_list(
                    &h,
                    if empty {
                        vec![]
                    } else {
                        vec![i2768_row(5, "completed")]
                    },
                );
                tick(&mut now, &mut wall, 10);
                h.round(now, wall).await;
            }
            expected.extend(
                [
                    "repos/mblua/AgentsCommander/actions/runs/5",
                    "repos/mblua/AgentsCommander/actions/runs/5/attempts/1/jobs?per_page=100",
                    "repos/mblua/AgentsCommander/actions/runs/5",
                    "repos/mblua/AgentsCommander/actions/runs/50",
                ]
                .map(str::to_string),
            );
        }
        i2768_list(&h, vec![i2768_row(50, "in_progress")]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        // Edge assertion comes first so the M1/M2 probes fail on behavior.
        i2768_assert_edges(&h, 1, 0);
        i2768_assert_running(&h, &repo);
        assert!(h.sweeper.lock_state().keys[&i2768_key()]
            .ci
            .completion
            .known
            .contains_key(&50));
        assert_eq!(i2768_details(&h), expected);
        for id in [5, 50] {
            i2768_list(
                &h,
                vec![i2768_row(5, "completed"), i2768_row(50, "completed")],
            );
            queue_completion(&h, &i2768_key().nwo, i2768_row(id, "completed"));
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_running(&h, &repo);
        }
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 1);
        assert_eq!(h.snapshot()[&repo].ci, CiState::Idle);
        i2768_assert_consumed(&h);
    }
}

#[tokio::test]
async fn i2768_jobs_contradict_terminal_run() {
    let _guard = round_test_lock().await;
    let (h, repo) = i2768_harness();
    let mut now = Instant::now();
    let mut wall = Local::now();
    i2768_list(&h, vec![]);
    h.round(now, wall).await;
    i2768_list(&h, vec![i2768_row(50, "in_progress")]);
    tick(&mut now, &mut wall, 60);
    h.round(now, wall).await;
    let endpoint = "repos/mblua/AgentsCommander/actions/runs/50";
    let jobs = format!("{endpoint}/attempts/1/jobs?per_page=100");
    // Three terminal detail responses include the C responses needed by M3.
    for _ in 0..4 {
        queue_endpoint(
            &h,
            endpoint,
            Ok(ok_output(&i2768_row(50, "completed").to_string())),
        );
    }
    for _ in 0..2 {
        queue_endpoint(
            &h,
            &jobs,
            Ok(ok_output(&ci_jobs_body(
                50,
                &sha_of('a'),
                "main",
                1,
                &["in_progress"],
            ))),
        );
        i2768_list(&h, vec![i2768_row(50, "completed")]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
    }
    // M3 can accept on its second false list; baseline has no proof.
    i2768_assert_edges(&h, 1, 0);
    i2768_assert_running(&h, &repo);
    assert!(h.sweeper.lock_state().keys[&i2768_key()]
        .ci
        .completion
        .proofs
        .is_empty());
    assert_eq!(
        i2768_details(&h),
        vec![
            endpoint.to_string(),
            jobs.clone(),
            endpoint.to_string(),
            jobs.clone()
        ]
    );
    queue_endpoint(
        &h,
        &jobs,
        Ok(ok_output(&ci_jobs_body(
            50,
            &sha_of('a'),
            "main",
            1,
            &["completed"],
        ))),
    );
    i2768_list(&h, vec![i2768_row(50, "completed")]);
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 0);
    i2768_list(&h, vec![]);
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 1);
    i2768_assert_consumed(&h);
}

#[tokio::test]
async fn i2768_closing_round_is_required() {
    let _guard = round_test_lock().await;
    let (h, repo) = i2768_harness();
    let mut now = Instant::now();
    let mut wall = Local::now();
    i2768_list(&h, vec![i2768_row(1, "in_progress")]);
    h.round(now, wall).await;
    let confirmed_wall = wall;
    i2768_list(&h, vec![i2768_row(1, "completed")]);
    queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 0);
    i2768_assert_running(&h, &repo);
    {
        let state = h.sweeper.lock_state();
        let ci = &state.keys[&i2768_key()].ci;
        assert_eq!(ci.last_confirmed_at, Some(confirmed_wall));
        assert_eq!(ci.run_ids, vec![1]);
        assert_eq!(ci.completion.proofs.len(), 1);
    }
    i2768_list(&h, vec![]);
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    let edges = h.drain_transitions();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].kind, TransitionKind::CiFinished);
    assert_eq!(edges[0].observed_at, wall);
    assert_eq!(edges[0].last_confirmed_at, Some(confirmed_wall));
    assert_eq!(i2768_details(&h).len(), 3);
    i2768_assert_consumed(&h);
}
#[tokio::test]
async fn i2768_stale_completed_list_does_not_finish() {
    let _guard = round_test_lock().await;
    let (h, repo) = i2768_harness();
    let mut now = Instant::now();
    let mut wall = Local::now();
    i2768_list(&h, vec![i2768_row(50, "in_progress")]);
    h.round(now, wall).await;
    let list = "repos/mblua/AgentsCommander/actions/runs/50";
    for _ in 0..2 {
        i2768_list(&h, vec![i2768_row(50, "completed")]);
        queue_endpoint(
            &h,
            list,
            Ok(ok_output(&i2768_row(50, "in_progress").to_string())),
        );
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
    }
    i2768_list(&h, vec![i2768_row(50, "in_progress")]);
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 0);
    i2768_assert_running(&h, &repo);
    assert_eq!(i2768_details(&h), vec![list.to_string(); 2]);
    i2768_assert_consumed(&h);
}

#[tokio::test]
async fn i2768_all_runs_finish_once() {
    let _guard = round_test_lock().await;
    let (h, repo) = i2768_harness();
    let mut now = Instant::now();
    let mut wall = Local::now();
    let rows = |status| {
        (1..=12)
            .rev()
            .map(|id| {
                let mut row = i2768_row(id, status);
                row["workflow_id"] = serde_json::json!(700 + id % 2);
                row
            })
            .collect::<Vec<_>>()
    };
    i2768_list(&h, rows("in_progress"));
    h.round(now, wall).await;
    let confirmed_wall = wall;
    let proof_start = now + Duration::from_secs(10);
    let mut expected = vec![
        "repos/mblua/AgentsCommander".to_string(),
        format!(
            "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
            sha_of('a')
        ),
    ];
    for id in 1..=12 {
        let mut row = i2768_row(id, "completed");
        row["workflow_id"] = serde_json::json!(700 + id % 2);
        if id == 7 {
            let proofs = h.sweeper.lock_state().keys[&i2768_key()]
                .ci
                .completion
                .proofs
                .clone();
            assert_eq!(proofs.len(), 6);
            i2768_list(&h, rows("completed"));
            queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/7",
                Ok(ok_output(&row.to_string())),
            );
            queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/7/attempts/1/jobs?per_page=100",
                Err(FailureKind::Timeout),
            );
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
            {
                let state = h.sweeper.lock_state();
                let ci = &state.keys[&i2768_key()].ci;
                assert_eq!(ci.failure, Some(FailureKind::Timeout));
                assert_eq!(ci.completion.proofs, proofs);
                assert!(!ci.completion.closing_ready);
            }
            expected.extend([
                format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                "repos/mblua/AgentsCommander/actions/runs/7".to_string(),
                "repos/mblua/AgentsCommander/actions/runs/7/attempts/1/jobs?per_page=100"
                    .to_string(),
            ]);
        }
        i2768_list(&h, rows("completed"));
        queue_completion(&h, &i2768_key().nwo, row);
        tick(&mut now, &mut wall, if id == 7 { 20 } else { 10 });
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
        {
            let state = h.sweeper.lock_state();
            let ci = &state.keys[&i2768_key()].ci;
            assert_eq!(ci.completion.proofs.len(), id as usize);
            assert_eq!(ci.last_confirmed_at, Some(confirmed_wall));
            assert_eq!(ci.run_ids, (1..=12).rev().collect::<Vec<_>>());
            assert!(state.budget_tokens >= 0.0);
        }
        expected.extend([
            format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            format!("repos/mblua/AgentsCommander/actions/runs/{id}"),
            format!("repos/mblua/AgentsCommander/actions/runs/{id}/attempts/1/jobs?per_page=100"),
            format!("repos/mblua/AgentsCommander/actions/runs/{id}"),
        ]);
    }
    i2768_list(&h, rows("completed"));
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 1);
    assert_eq!(h.snapshot()[&repo].ci, CiState::Idle);
    assert_eq!(now.duration_since(proof_start), Duration::from_secs(140));
    expected.push(format!(
        "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
        sha_of('a')
    ));
    assert_eq!(i2768_calls(&h), expected);
    {
        let state = h.sweeper.lock_state();
        let ci = &state.keys[&i2768_key()].ci;
        assert!(ci.run_ids.is_empty() && ci.pull_requests.is_empty());
    }
    println!(
        "N=12 proofs=12 timeout=B7 elapsed_first_proof_to_accept=140s calls={} budget={}",
        h.gh_call_count(),
        h.sweeper.lock_state().budget_tokens
    );
    i2768_list(&h, rows("completed"));
    tick(&mut now, &mut wall, 60);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 0);
    i2768_assert_consumed(&h);
}

#[test]
fn i2768_parser_filter_and_count_contract() {
    let key = i2768_key();
    for total in [0, 2] {
        let body =
            serde_json::json!({"total_count":total,"workflow_runs":[i2768_row(1,"completed")]})
                .to_string();
        assert_eq!(
            parse_ci_response(&body, &key.branch, &key.sha40),
            Err(FailureKind::Incomplete)
        );
    }
    let mut bad = i2768_row(2, "completed");
    bad["id"] = serde_json::Value::Null;
    for branch in [
        serde_json::Value::Null,
        serde_json::json!(42),
        serde_json::json!("other"),
    ] {
        bad["head_branch"] = branch;
        let body = runs_body(vec![bad.clone(), i2768_row(1, "completed")]);
        let answer = parse_ci_response(&body, &key.branch, &key.sha40).unwrap();
        assert_eq!(answer.observations.len(), 1);
    }
    bad.as_object_mut().unwrap().remove("head_branch");
    assert!(
        parse_ci_response(&runs_body(vec![bad.clone()]), &key.branch, &key.sha40)
            .unwrap()
            .observations
            .is_empty()
    );
    bad["head_branch"] = serde_json::json!("main");
    assert_eq!(
        parse_ci_response(&runs_body(vec![bad]), &key.branch, &key.sha40),
        Err(FailureKind::Other)
    );
    for conclusion in [
        serde_json::Value::Null,
        serde_json::json!(""),
        serde_json::json!("   "),
        serde_json::json!(9),
    ] {
        let mut row = i2768_row(1, "completed");
        row["conclusion"] = conclusion;
        assert_eq!(
            parse_ci_response(&runs_body(vec![row]), &key.branch, &key.sha40),
            Err(FailureKind::Incomplete)
        );
    }
    let mut no_conclusion = i2768_row(1, "completed");
    no_conclusion.as_object_mut().unwrap().remove("conclusion");
    assert_eq!(
        parse_ci_response(&runs_body(vec![no_conclusion]), &key.branch, &key.sha40),
        Err(FailureKind::Incomplete)
    );
    for status in [None, Some("future")] {
        let mut row = i2768_row(1, "in_progress");
        if let Some(status) = status {
            row["status"] = serde_json::json!(status);
        } else {
            row.as_object_mut().unwrap().remove("status");
        }
        assert_eq!(
            parse_ci_response(&runs_body(vec![row]), &key.branch, &key.sha40)
                .unwrap()
                .state,
            CiState::Running
        );
    }
    for conclusion in ["failure", "cancelled", "skipped", "timed_out"] {
        let mut row = i2768_row(1, "completed");
        row["conclusion"] = serde_json::json!(conclusion);
        assert_eq!(
            parse_ci_response(&runs_body(vec![row]), &key.branch, &key.sha40)
                .unwrap()
                .state,
            CiState::Idle
        );
    }
    let row = i2768_row(1, "completed");
    assert_eq!(
        parse_ci_response(&runs_body(vec![row.clone(), row]), &key.branch, &key.sha40),
        Err(FailureKind::Other)
    );
    for branch in [serde_json::Value::Null, serde_json::json!("other")] {
        let mut row = i2768_row(1, "completed");
        row["head_branch"] = branch;
        assert_eq!(
            parse_ci_run_response(&row.to_string(), &key),
            Err(FailureKind::Other)
        );
    }
}
#[tokio::test]
async fn i2768_identity_and_attempt_fail_closed() {
    let _guard = round_test_lock().await;
    for (field, value) in [
        ("head_sha", serde_json::json!(sha_of('b'))),
        ("head_branch", serde_json::json!("other")),
        ("workflow_id", serde_json::json!(701)),
        ("id", serde_json::json!(50)),
        ("id", serde_json::Value::Null),
        ("run_attempt", serde_json::json!(0)),
        ("updated_at", serde_json::json!("not a date")),
    ] {
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        h.round(now, wall).await;
        let old = h.sweeper.lock_state().keys[&i2768_key()]
            .ci
            .completion
            .known
            .clone();
        let mut bad = i2768_row(1, "completed");
        bad[field] = value;
        queue_endpoint(
            &h,
            "repos/mblua/AgentsCommander/actions/runs/1",
            Ok(ok_output(&bad.to_string())),
        );
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
        let state = h.sweeper.lock_state();
        let ci = &state.keys[&i2768_key()].ci;
        assert_eq!(ci.failure, Some(FailureKind::Other));
        assert_eq!(ci.completion.known, old);
        drop(state);
        i2768_assert_consumed(&h);
    }
    let mut ledger = CiCompletionState::default();
    let mut row = i2768_row(1, "in_progress");
    row["run_attempt"] = serde_json::json!(2);
    let high = parse_ci_run_response(&row.to_string(), &i2768_key()).unwrap();
    merge_ci_observations(&mut ledger, std::slice::from_ref(&high), true).unwrap();
    let low = parse_ci_run_response(&i2768_row(1, "completed").to_string(), &i2768_key()).unwrap();
    assert_eq!(
        merge_ci_observations(&mut ledger, &[low], true),
        Err(FailureKind::Incomplete)
    );
    assert_eq!(ledger.known[&1], high);
    assert_eq!(
        parse_ci_run_response("{", &i2768_key()),
        Err(FailureKind::Other)
    );
    let mut row = i2768_row(1, "completed");
    row.as_object_mut().unwrap().remove("conclusion");
    assert_eq!(
        parse_ci_run_response(&row.to_string(), &i2768_key()),
        Err(FailureKind::Incomplete)
    );
    let rows = vec![i2768_row(1, "completed"), i2768_row(2, "completed")];
    let answer = parse_ci_response(&runs_body(rows), "main", &sha_of('a')).unwrap();
    assert_eq!(answer.observations.len(), 2);
}

#[tokio::test]
async fn i2768_incomplete_evidence_never_finishes() {
    let _guard = round_test_lock().await;
    let run = parse_ci_run_response(&i2768_row(1, "completed").to_string(), &i2768_key()).unwrap();
    for count in [0, 2, 101] {
        let mut jobs: serde_json::Value =
            serde_json::from_str(&ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"]))
                .unwrap();
        jobs["total_count"] = serde_json::json!(count);
        assert_eq!(
            parse_ci_jobs_response(&jobs.to_string(), &run),
            Err(FailureKind::Incomplete)
        );
    }
    let jobs = ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"; 101]);
    assert_eq!(
        parse_ci_jobs_response(&jobs, &run),
        Err(FailureKind::Incomplete)
    );
    for (field, value, kind) in [
        ("run_id", serde_json::json!(50), FailureKind::Other),
        (
            "head_sha",
            serde_json::json!(sha_of('b')),
            FailureKind::Other,
        ),
        ("run_attempt", serde_json::Value::Null, FailureKind::Other),
        ("head_branch", serde_json::Value::Null, FailureKind::Other),
        ("id", serde_json::json!(0), FailureKind::Other),
        (
            "conclusion",
            serde_json::Value::Null,
            FailureKind::Incomplete,
        ),
    ] {
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        h.round(now, wall).await;
        i2768_list(&h, vec![i2768_row(1, "completed")]);
        queue_endpoint(
            &h,
            "repos/mblua/AgentsCommander/actions/runs/1",
            Ok(ok_output(&i2768_row(1, "completed").to_string())),
        );
        let mut jobs: serde_json::Value =
            serde_json::from_str(&ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"]))
                .unwrap();
        jobs["jobs"][0][field] = value;
        queue_endpoint(
            &h,
            "repos/mblua/AgentsCommander/actions/runs/1/attempts/1/jobs?per_page=100",
            Ok(ok_output(&jobs.to_string())),
        );
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
        assert_eq!(
            h.sweeper.lock_state().keys[&i2768_key()].ci.failure,
            Some(kind)
        );
        i2768_assert_consumed(&h);
    }
    let mut jobs: serde_json::Value =
        serde_json::from_str(&ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"])).unwrap();
    jobs["jobs"][0].as_object_mut().unwrap().remove("status");
    assert_eq!(parse_ci_jobs_response(&jobs.to_string(), &run), Ok(false));
    let mut jobs: serde_json::Value =
        serde_json::from_str(&ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"])).unwrap();
    jobs["jobs"][0]
        .as_object_mut()
        .unwrap()
        .remove("run_attempt");
    jobs["jobs"][0]
        .as_object_mut()
        .unwrap()
        .remove("head_branch");
    assert_eq!(parse_ci_jobs_response(&jobs.to_string(), &run), Ok(true));
    let duplicate = jobs["jobs"][0].clone();
    jobs["jobs"].as_array_mut().unwrap().push(duplicate);
    jobs["total_count"] = serde_json::json!(2);
    assert_eq!(
        parse_ci_jobs_response(&jobs.to_string(), &run),
        Err(FailureKind::Other)
    );
    let mut ledger = CiCompletionState::default();
    let runs = (1..=100)
        .map(|id| {
            parse_ci_run_response(&i2768_row(id, "completed").to_string(), &i2768_key()).unwrap()
        })
        .collect::<Vec<_>>();
    merge_ci_observations(&mut ledger, &runs, true).unwrap();
    let overflow =
        parse_ci_run_response(&i2768_row(101, "completed").to_string(), &i2768_key()).unwrap();
    assert_eq!(
        merge_ci_observations(&mut ledger, &[overflow], true),
        Err(FailureKind::Incomplete)
    );
    assert!(ledger.overflow);
    assert_eq!(
        merge_ci_observations(&mut ledger, &[], true),
        Err(FailureKind::Incomplete)
    );
    assert_eq!(ledger.known.len(), 100);
    // A cancelled run with no jobs still needs matching latest details.
    let (h, repo) = i2768_harness();
    let mut now = Instant::now();
    let mut wall = Local::now();
    i2768_list(&h, vec![i2768_row(1, "in_progress")]);
    h.round(now, wall).await;
    for changed in [true, false] {
        i2768_list(&h, vec![i2768_row(1, "completed")]);
        let a = i2768_row(1, "completed");
        let mut c = a.clone();
        if changed {
            c["updated_at"] = serde_json::json!("2026-10-01T00:00:01Z");
        }
        queue_endpoint(
            &h,
            "repos/mblua/AgentsCommander/actions/runs/1",
            Ok(ok_output(&a.to_string())),
        );
        queue_endpoint(
            &h,
            "repos/mblua/AgentsCommander/actions/runs/1/attempts/1/jobs?per_page=100",
            Ok(ok_output(&ci_jobs_body(1, &sha_of('a'), "main", 1, &[]))),
        );
        queue_endpoint(
            &h,
            "repos/mblua/AgentsCommander/actions/runs/1",
            Ok(ok_output(&c.to_string())),
        );
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
        assert_eq!(
            h.sweeper.lock_state().keys[&i2768_key()]
                .ci
                .completion
                .proofs
                .len(),
            usize::from(!changed)
        );
    }
    i2768_list(&h, vec![]);
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 1);
    i2768_assert_consumed(&h);
}

#[tokio::test]
async fn i2768_rerun_invalidates_completion() {
    let _guard = round_test_lock().await;
    for point in ["A", "C", "closing"] {
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![]);
        h.round(now, wall).await;
        i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        tick(&mut now, &mut wall, 60);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 1, 0);
        let terminal = i2768_row(1, "completed");
        let mut active = i2768_row(1, "in_progress");
        active["run_attempt"] = serde_json::json!(2);
        i2768_list(&h, vec![terminal.clone()]);
        match point {
            "A" => queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/1",
                Ok(ok_output(&active.to_string())),
            ),
            "C" => {
                queue_endpoint(
                    &h,
                    "repos/mblua/AgentsCommander/actions/runs/1",
                    Ok(ok_output(&terminal.to_string())),
                );
                queue_endpoint(
                    &h,
                    "repos/mblua/AgentsCommander/actions/runs/1/attempts/1/jobs?per_page=100",
                    Ok(ok_output(&ci_jobs_body(
                        1,
                        &sha_of('a'),
                        "main",
                        1,
                        &["completed"],
                    ))),
                );
                queue_endpoint(
                    &h,
                    "repos/mblua/AgentsCommander/actions/runs/1",
                    Ok(ok_output(&active.to_string())),
                );
            }
            _ => queue_completion(&h, &i2768_key().nwo, terminal),
        }
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        if point == "closing" {
            i2768_list(&h, vec![active.clone()]);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
        }
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
        {
            let state = h.sweeper.lock_state();
            let ledger = &state.keys[&i2768_key()].ci.completion;
            assert_eq!(ledger.known[&1].run_attempt, 2);
            assert!(ledger.proofs.is_empty());
        }
        let mut terminal = i2768_row(1, "completed");
        terminal["run_attempt"] = serde_json::json!(2);
        i2768_list(&h, vec![terminal.clone()]);
        queue_completion(&h, &i2768_key().nwo, terminal);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 1);
        // A real rerun after accepted completion is a new legitimate pair.
        active["run_attempt"] = serde_json::json!(3);
        i2768_list(&h, vec![active]);
        tick(&mut now, &mut wall, 60);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 1, 0);
        let mut terminal = i2768_row(1, "completed");
        terminal["run_attempt"] = serde_json::json!(3);
        i2768_list(&h, vec![terminal.clone()]);
        queue_completion(&h, &i2768_key().nwo, terminal);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 1);
        i2768_assert_consumed(&h);
    }
}
#[tokio::test]
async fn i2768_failures_preserve_known_runs() {
    let _guard = round_test_lock().await;
    let failures = [
        Err(FailureKind::Timeout),
        Ok(ok_output("{")),
        Ok(GhCallOutput {
            success: false,
            stdout: String::new(),
            stderr: "HTTP 401".to_string(),
        }),
        Ok(GhCallOutput {
            success: false,
            stdout: String::new(),
            stderr: "HTTP 404 deleted".to_string(),
        }),
        Ok(primary_limit_output()),
        Ok(secondary_limit_output()),
        Err(FailureKind::Other),
    ];
    let kinds = [
        FailureKind::Timeout,
        FailureKind::Other,
        FailureKind::NotAuthenticated,
        FailureKind::Other,
        FailureKind::RateLimited,
        FailureKind::SecondaryRateLimited,
        FailureKind::Other,
    ];
    for point in ["A", "B", "C", "list", "compare"] {
        for (failure, kind) in failures.iter().zip(kinds) {
            let failure = || match failure {
                Ok(output) => Ok(GhCallOutput {
                    success: output.success,
                    stdout: output.stdout.clone(),
                    stderr: output.stderr.clone(),
                }),
                Err(kind) => Err(*kind),
            };
            let (h, repo) = i2768_harness();
            let mut now = Instant::now();
            let mut wall = Local::now();
            let terminal = || vec![i2768_row(1, "completed"), i2768_row(2, "completed")];
            i2768_list(
                &h,
                vec![i2768_row(1, "in_progress"), i2768_row(2, "in_progress")],
            );
            h.round(now, wall).await;
            i2768_list(&h, terminal());
            queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            let proofs = h.sweeper.lock_state().keys[&i2768_key()]
                .ci
                .completion
                .proofs
                .clone();
            assert_eq!(proofs.len(), 1);
            if point == "list" {
                queue_endpoint(
                    &h,
                    &format!(
                        "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                        sha_of('a')
                    ),
                    failure(),
                );
            } else {
                i2768_list(&h, terminal());
            }
            if point == "compare" {
                h.sweeper
                    .lock_state()
                    .base_branches
                    .insert(i2768_key().nwo, "base".to_string());
                queue_endpoint(
                    &h,
                    &format!("repos/mblua/AgentsCommander/compare/HEAD...{}", sha_of('a')),
                    failure(),
                );
            }
            if ["A", "B", "C"].contains(&point) {
                let endpoint = "repos/mblua/AgentsCommander/actions/runs/2";
                queue_endpoint(
                    &h,
                    endpoint,
                    if point == "A" {
                        failure()
                    } else {
                        Ok(ok_output(&i2768_row(2, "completed").to_string()))
                    },
                );
                if point != "A" {
                    queue_endpoint(
                        &h,
                        &format!("{endpoint}/attempts/1/jobs?per_page=100"),
                        if point == "B" {
                            failure()
                        } else {
                            Ok(ok_output(&ci_jobs_body(
                                2,
                                &sha_of('a'),
                                "main",
                                1,
                                &["completed"],
                            )))
                        },
                    );
                }
                if point == "C" {
                    queue_endpoint(&h, endpoint, failure());
                }
            }
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
            {
                let state = h.sweeper.lock_state();
                let ci = &state.keys[&i2768_key()].ci;
                assert_eq!(ci.failure, Some(kind), "point={point}");
                assert_eq!(ci.completion.known.len(), 2);
                assert_eq!(ci.completion.active_ids, vec![1, 2]);
                assert_eq!(ci.completion.proofs, proofs);
                assert!(!ci.completion.closing_ready);
            }
            // Three permanent failures remain unknown; no age-based success.
            if point == "A" && kind == FailureKind::Other {
                for _ in 0..3 {
                    i2768_list(&h, terminal());
                    queue_endpoint(
                        &h,
                        "repos/mblua/AgentsCommander/actions/runs/2",
                        Ok(GhCallOutput {
                            success: false,
                            stdout: String::new(),
                            stderr: "HTTP 404".to_string(),
                        }),
                    );
                    tick(&mut now, &mut wall, 900);
                    h.round(now, wall).await;
                    i2768_assert_edges(&h, 0, 0);
                    assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
                }
            }
            if point == "compare" {
                h.sweeper
                    .lock_state()
                    .base_branches
                    .insert(i2768_key().nwo, DEFAULT_BRANCH_LABEL.to_string());
            }
            i2768_list(&h, terminal());
            queue_completion(&h, &i2768_key().nwo, i2768_row(2, "completed"));
            tick(&mut now, &mut wall, 900);
            h.round(now, wall).await;
            i2768_assert_running(&h, &repo);
            i2768_assert_edges(&h, 0, 0);
            i2768_list(&h, vec![]);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 1);
            let details = i2768_details(&h);
            assert_eq!(
                details
                    .iter()
                    .filter(|endpoint| endpoint.as_str()
                        == "repos/mblua/AgentsCommander/actions/runs/1")
                    .count(),
                2
            );
            i2768_assert_consumed(&h);
        }
    }
    // Even all old proofs cannot finish through a failed closing list.
    let (h, repo) = i2768_harness();
    let mut now = Instant::now();
    let mut wall = Local::now();
    i2768_list(&h, vec![i2768_row(1, "in_progress")]);
    h.round(now, wall).await;
    i2768_list(&h, vec![]);
    queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    queue_endpoint(
        &h,
        &format!(
            "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
            sha_of('a')
        ),
        Err(FailureKind::Timeout),
    );
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 0);
    assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
    assert!(
        !h.sweeper.lock_state().keys[&i2768_key()]
            .ci
            .completion
            .closing_ready
    );
    i2768_list(&h, vec![]);
    tick(&mut now, &mut wall, 20);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 0);
    i2768_assert_running(&h, &repo);
    i2768_list(&h, vec![]);
    tick(&mut now, &mut wall, 10);
    h.round(now, wall).await;
    i2768_assert_edges(&h, 0, 1);
    assert_eq!(i2768_details(&h).len(), 3);
    i2768_assert_consumed(&h);
}
