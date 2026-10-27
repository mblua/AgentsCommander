use super::*;

#[test]
fn p1_parse_ci_response_collects_ids_and_prs_only_from_running_kept_rows() {
    assert_eq!(
        parse_ci_response(
            &runs_body(vec![
                serde_json::json!({"head_branch":"feature","status":"in_progress"})
            ]),
            "feature",
            &sha_of('a')
        ),
        Err(FailureKind::Other)
    );
    let body = runs_body(vec![
        run_row(11, &sha_of('a'), "feature", "in_progress", &[5, 6]),
        run_row(12, &sha_of('a'), "feature", "queued", &[5]),
        run_row(13, &sha_of('a'), "feature", "completed", &[99]),
        run_row(14, &sha_of('a'), "other", "in_progress", &[98]),
    ]);
    assert_eq!(
        parse_ci_response(&body, "feature", &sha_of('a')),
        Ok(CiAnswer {
            state: CiState::Running,
            branch_has_runs: true,
            observations: vec![
                expected_run(11, "feature", "in_progress", &[5, 6]),
                expected_run(12, "feature", "queued", &[5]),
                expected_run(13, "feature", "completed", &[99])
            ],
            run_ids: vec![11, 12],
            pull_requests: vec![5, 6, 5],
        })
    );
    let idle = runs_body(vec![
        run_row(13, &sha_of('a'), "feature", "completed", &[99]),
        run_row(14, &sha_of('a'), "other", "in_progress", &[98]),
    ]);
    assert_eq!(
        parse_ci_response(&idle, "feature", &sha_of('a')),
        Ok(CiAnswer {
            state: CiState::Idle,
            branch_has_runs: true,
            observations: vec![expected_run(13, "feature", "completed", &[99])],
            run_ids: Vec::new(),
            pull_requests: Vec::new(),
        })
    );
}

#[tokio::test]
async fn p2_apply_ci_clears_runs_on_suppressed_and_err_and_sets_failure_only_on_plain_err() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let key = QueryKey {
        nwo: "mblua/AgentsCommander".to_string(),
        sha40: sha_of('a'),
        branch: "feature".to_string(),
    };
    let now = Instant::now();
    let ctx = RoundCtx {
        facts: &[],
        now,
        wall: Local::now(),
        ci_dial: Duration::from_secs(60),
        staleness_dial: Duration::from_secs(60),
    };
    let runs = || (vec![1, 2], vec![7]);
    let direct_completion = || {
        let rows = vec![
            run_row(1, &key.sha40, &key.branch, "in_progress", &[7]),
            run_row(2, &key.sha40, &key.branch, "queued", &[]),
        ];
        let answer = parse_ci_response(&runs_body(rows), &key.branch, &key.sha40).unwrap();
        let mut ledger = CiCompletionState::default();
        merge_ci_observations(&mut ledger, &answer.observations, true).unwrap();
        ledger
    };
    let read = |harness: &Harness| {
        let state = harness.sweeper.lock_state();
        let ci = &state.keys[&key].ci;
        (
            ci.chip,
            ci.run_ids.clone(),
            ci.pull_requests.clone(),
            ci.failure,
        )
    };
    let s = &harness.sweeper;

    s.apply_ci(
        &key,
        KeyOutcome {
            ci: Some(Ok(CiState::Running)),
            ci_suppressed: false,
            ci_runs: runs(),
            ci_completion: Some(direct_completion()),
            ci_decision: Some(CiCompletionDecision::Accepted),
            ..KeyOutcome::default()
        },
        &[],
        &ctx,
    );
    assert_eq!(
        read(&harness),
        (CiState::Running, vec![1, 2], vec![7], None)
    );

    s.apply_ci(
        &key,
        KeyOutcome {
            ci: Some(Ok(CiState::Idle)),
            ci_suppressed: true,
            ci_runs: runs(),
            ci_completion: Some(direct_completion()),
            ci_decision: Some(CiCompletionDecision::Accepted),
            ..KeyOutcome::default()
        },
        &[],
        &ctx,
    );
    assert_eq!(read(&harness), (CiState::Idle, vec![], vec![], None));
    assert!(s.lock_state().keys[&key].ci.completion.known.is_empty());

    s.apply_ci(
        &key,
        KeyOutcome {
            ci: Some(Ok(CiState::Running)),
            ci_suppressed: false,
            ci_runs: runs(),
            ci_completion: Some(direct_completion()),
            ci_decision: Some(CiCompletionDecision::Accepted),
            ..KeyOutcome::default()
        },
        &[],
        &ctx,
    );
    s.apply_ci(
        &key,
        KeyOutcome {
            ci: Some(Err(FailureKind::Timeout)),
            ci_suppressed: true,
            ci_runs: runs(),
            ci_completion: Some(direct_completion()),
            ci_decision: Some(CiCompletionDecision::Accepted),
            ..KeyOutcome::default()
        },
        &[],
        &ctx,
    );
    assert_eq!(read(&harness), (CiState::Idle, vec![], vec![], None));
    assert!(s.lock_state().keys[&key].ci.completion.known.is_empty());

    s.apply_ci(
        &key,
        KeyOutcome {
            ci: Some(Ok(CiState::Running)),
            ci_suppressed: false,
            ci_runs: runs(),
            ci_completion: Some(direct_completion()),
            ci_decision: Some(CiCompletionDecision::Accepted),
            ..KeyOutcome::default()
        },
        &[],
        &ctx,
    );
    s.apply_ci(
        &key,
        KeyOutcome {
            ci: Some(Err(FailureKind::Incomplete)),
            ci_suppressed: false,
            ci_runs: runs(),
            ci_completion: Some(direct_completion()),
            ci_decision: Some(CiCompletionDecision::Accepted),
            ..KeyOutcome::default()
        },
        &[],
        &ctx,
    );
    assert_eq!(
        read(&harness),
        (
            CiState::Unknown,
            vec![],
            vec![],
            Some(FailureKind::Incomplete)
        )
    );

    assert_eq!(s.lock_state().keys[&key].ci.completion.known.len(), 2);
    s.apply_ci(
        &key,
        KeyOutcome {
            ci: Some(Ok(CiState::Idle)),
            ci_suppressed: false,
            ci_runs: runs(),
            ci_completion: Some(direct_completion()),
            ci_decision: Some(CiCompletionDecision::Accepted),
            ..KeyOutcome::default()
        },
        &[],
        &ctx,
    );
    assert_eq!(read(&harness), (CiState::Idle, vec![], vec![], None));
    assert_eq!(s.lock_state().keys[&key].ci.completion.known.len(), 2);

    s.apply_ci(
        &key,
        KeyOutcome {
            ci: None,
            ci_suppressed: false,
            ci_runs: runs(),
            ci_completion: Some(direct_completion()),
            ci_decision: Some(CiCompletionDecision::Accepted),
            ..KeyOutcome::default()
        },
        &[],
        &ctx,
    );
    assert_eq!(
        read(&harness),
        (CiState::Idle, vec![], vec![], None),
        "a key that is not due keeps its published detail"
    );
}

#[test]
fn persisted_failure_reason_maps_every_kind() {
    assert_eq!(
        persisted_failure_reason(FailureKind::Timeout),
        "ci-query-timeout"
    );
    assert_eq!(
        persisted_failure_reason(FailureKind::RateLimited),
        "ci-query-rate-limited"
    );
    assert_eq!(
        persisted_failure_reason(FailureKind::SecondaryRateLimited),
        "ci-query-secondary-rate-limited"
    );
    assert_eq!(
        persisted_failure_reason(FailureKind::NotAuthenticated),
        "ci-query-not-authenticated"
    );
    assert_eq!(
        persisted_failure_reason(FailureKind::Incomplete),
        "ci-query-incomplete"
    );
    assert_eq!(
        persisted_failure_reason(FailureKind::Other),
        "ci-query-failed"
    );
}

/// P3 a, c, d, f: no resolved default branch, so arm `Ok(answer)` answers
/// without any identity compare.
#[tokio::test]
async fn p3_producer_publishes_run_ids_prs_and_reason_end_to_end() {
    let _guard = round_test_lock().await;
    let dir = tempfile::tempdir().expect("snapshot dir");
    let settings = AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::with_snapshot_dir(settings, Some(dir.path().to_path_buf()));
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "feature-a", harness.git_state.as_ref());
    harness
        .git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_head_sha(&repo, 'a');
    harness.set_work(std::slice::from_ref(&repo));
    let marker = format!(
        "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
        sha_of('a')
    );
    let script = |result: Result<GhCallOutput, FailureKind>| {
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .route(&marker, result);
    };

    // a. running: exact ids / PRs from real `gh` JSON, no compare issued.
    script(Ok(ok_output(&runs_body(vec![
        run_row(502, &sha_of('a'), "feature-a", "in_progress", &[40, 41]),
        run_row(501, &sha_of('a'), "feature-a", "queued", &[40]),
        run_row(400, &sha_of('a'), "feature-a", "completed", &[39]),
        run_row(600, &sha_of('a'), "main", "in_progress", &[1]),
    ]))));
    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    assert_eq!(
        persisted_of(dir.path(), &repo),
        expected_ci(PersistedCiState::Running, &[501, 502], &[40, 41], None)
    );
    assert_eq!(
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .count("/compare/"),
        0,
        "no identity compare without a resolved default branch"
    );

    // f. the CI axis is not due one second later: the same detail again.
    tick(&mut now, &mut wall, 1);
    let calls = harness.gh_call_count();
    harness.round(now, wall).await;
    assert_eq!(
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .count("/actions/runs?"),
        1,
        "the CI axis was not due (calls before: {calls})"
    );
    assert_eq!(
        persisted_of(dir.path(), &repo),
        expected_ci(PersistedCiState::Running, &[501, 502], &[40, 41], None)
    );

    // All remembered runs need proof, followed by a distinct closing list.
    for id in [400, 501, 502] {
        script(Ok(ok_output(&runs_body(vec![run_row(
            502,
            &sha_of('a'),
            "feature-a",
            "completed",
            &[40],
        )]))));
        queue_completion(
            &harness,
            "mblua/AgentsCommander",
            run_row(id, &sha_of('a'), "feature-a", "completed", &[]),
        );
        tick(&mut now, &mut wall, if id == 400 { 3600 } else { 10 });
        harness.round(now, wall).await;
        assert_eq!(
            persisted_of(dir.path(), &repo),
            expected_ci(PersistedCiState::Running, &[501, 502], &[40, 41], None)
        );
        assert!(harness.drain_transitions().is_empty());
    }
    script(Ok(ok_output(&runs_body(vec![run_row(
        502,
        &sha_of('a'),
        "feature-a",
        "completed",
        &[40],
    )]))));
    tick(&mut now, &mut wall, 10);
    harness.round(now, wall).await;
    assert_eq!(
        persisted_of(dir.path(), &repo),
        expected_ci(PersistedCiState::Idle, &[], &[], None)
    );
    let edges = harness.drain_transitions();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].observed_at, wall);
    // d. Running, then a timeout -> Unknown with the reason, then recovery.
    script(Ok(ok_output(&runs_body(vec![run_row(
        700,
        &sha_of('a'),
        "feature-a",
        "in_progress",
        &[50],
    )]))));
    tick(&mut now, &mut wall, 3600);
    harness.round(now, wall).await;
    assert_eq!(
        persisted_of(dir.path(), &repo),
        expected_ci(PersistedCiState::Running, &[700], &[50], None)
    );
    script(Err(FailureKind::Timeout));
    tick(&mut now, &mut wall, 3600);
    harness.round(now, wall).await;
    assert_eq!(
        persisted_of(dir.path(), &repo),
        expected_ci(
            PersistedCiState::Unknown,
            &[],
            &[],
            Some("ci-query-timeout")
        )
    );
    script(Ok(ok_output(&runs_body(vec![run_row(
        800,
        &sha_of('a'),
        "feature-a",
        "in_progress",
        &[51],
    )]))));
    tick(&mut now, &mut wall, 3600);
    harness.round(now, wall).await;
    assert_eq!(
        persisted_of(dir.path(), &repo),
        expected_ci(PersistedCiState::Running, &[800], &[51], None)
    );
}

/// P3 b and e: a resolved default branch. A non-default branch whose
/// compare is not identical publishes its runs; the default branch and an
/// identical-to-default branch are suppressed to Idle with no detail.
#[tokio::test]
async fn p3_producer_publishes_runs_after_identity_compare_and_clears_them_when_suppressed() {
    let _guard = round_test_lock().await;
    let dir = tempfile::tempdir().expect("snapshot dir");
    let settings = AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::with_snapshot_dir(settings, Some(dir.path().to_path_buf()));
    let feature = harness.repo("repo-a");
    let default = harness.repo("repo-b");
    publish_branch(&feature, "feature-a", harness.git_state.as_ref());
    {
        let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
        git.set_head_sha(&feature, 'a');
        git.set_head_sha(&default, 'b');
    }
    harness.set_work(&[feature.clone(), default.clone()]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&runs_body(vec![
                run_row(902, &sha_of('a'), "feature-a", "in_progress", &[77]),
                run_row(901, &sha_of('a'), "feature-a", "in_progress", &[76, 77]),
            ]))),
        );
        gh.route(
            &format!("repos/mblua/AgentsCommander/compare/HEAD...{}", sha_of('a')),
            Ok(ok_output(&compare_body("ahead", 0, 1))),
        );
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('b')
            ),
            Ok(ok_output(&runs_body(vec![run_row(
                990,
                &sha_of('b'),
                "main",
                "in_progress",
                &[3],
            )]))),
        );
    }
    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    assert_eq!(
        persisted_of(dir.path(), &feature),
        expected_ci(PersistedCiState::Running, &[901, 902], &[76, 77], None)
    );
    assert_eq!(
        persisted_of(dir.path(), &default),
        expected_ci(PersistedCiState::Idle, &[], &[], None),
        "the default branch is suppressed"
    );

    // e. identical to the default branch: suppressed, detail cleared.
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&runs_body(vec![run_row(
                903,
                &sha_of('a'),
                "feature-a",
                "in_progress",
                &[78],
            )]))),
        );
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('b')
            ),
            Ok(ok_output(&ci_body(&sha_of('b'), &[]))),
        );
        gh.route(
            &format!("repos/mblua/AgentsCommander/compare/HEAD...{}", sha_of('a')),
            Ok(ok_output(&compare_body("identical", 0, 0))),
        );
    }
    tick(&mut now, &mut wall, 3600);
    harness.round(now, wall).await;
    assert_eq!(
        persisted_of(dir.path(), &feature),
        expected_ci(PersistedCiState::Idle, &[], &[], None)
    );
}
