use super::*;

#[test]
fn ci_query_endpoint_is_exactly_the_expected_string() {
    let gh = Path::new("gh");
    let nwo = "mblua/AgentsCommander";
    let sha = sha_of('a');

    let ci = build_gh_command_spec(gh, GhQuery::Ci { nwo, sha40: &sha }).expect("ci spec");
    assert_eq!(
        ci.args,
        vec![
            "api".to_string(),
            format!("repos/{nwo}/actions/runs?head_sha={sha}&per_page=100"),
        ]
    );

    let compare =
        build_gh_command_spec(gh, GhQuery::Compare { nwo, sha40: &sha }).expect("compare spec");
    assert_eq!(
        compare.args,
        vec![
            "api".to_string(),
            format!("repos/{nwo}/compare/HEAD...{sha}")
        ]
    );

    let info = build_gh_command_spec(gh, GhQuery::RepoInfo { nwo }).expect("info spec");
    assert_eq!(info.args, vec!["api".to_string(), format!("repos/{nwo}")]);

    // Rejection by construction: an abbreviated SHA is impossible to send.
    assert!(build_gh_command_spec(
        gh,
        GhQuery::Ci {
            nwo,
            sha40: "abc1234"
        }
    )
    .is_err());
    assert!(build_gh_command_spec(
        gh,
        GhQuery::Compare {
            nwo,
            sha40: "abc1234"
        }
    )
    .is_err());
    assert!(
        build_gh_command_spec(
            gh,
            GhQuery::Ci {
                nwo,
                sha40: &format!("{sha}a")
            }
        )
        .is_err(),
        "41 characters must be rejected"
    );
    assert!(
        build_gh_command_spec(
            gh,
            GhQuery::Ci {
                nwo,
                sha40: &sha.to_uppercase()
            }
        )
        .is_err(),
        "uppercase hex must be rejected"
    );
    assert!(build_gh_command_spec(gh, GhQuery::RepoInfo { nwo: "no-slash" }).is_err());
}

#[test]
fn ci_running_when_any_row_is_not_completed() {
    assert_eq!(
        parse_ci_response(
            &ci_body(&sha_of('a'), &["completed", "in_progress"]),
            "main",
            &sha_of('a')
        ),
        Ok(CiAnswer {
            state: CiState::Running,
            branch_has_runs: true,
            observations: vec![
                expected_run(1, "main", "completed", &[]),
                expected_run(2, "main", "in_progress", &[])
            ],
            run_ids: vec![2],
            pull_requests: Vec::new(),
        })
    );
    assert_eq!(
        parse_ci_response(&ci_body(&sha_of('a'), &["queued"]), "main", &sha_of('a')),
        Ok(CiAnswer {
            state: CiState::Running,
            branch_has_runs: true,
            observations: vec![expected_run(1, "main", "queued", &[])],
            run_ids: vec![1],
            pull_requests: Vec::new(),
        }),
        "an unknown future status counts as running, never as idle"
    );
}

#[test]
fn ci_idle_when_every_row_is_completed() {
    assert_eq!(
        parse_ci_response(&ci_body(&sha_of('a'), &[]), "main", &sha_of('a')),
        Ok(CiAnswer {
            state: CiState::Idle,
            branch_has_runs: false,
            observations: vec![],
            run_ids: Vec::new(),
            pull_requests: Vec::new(),
        })
    );
    assert_eq!(
        parse_ci_response(
            &ci_body(&sha_of('a'), &["completed", "completed"]),
            "main",
            &sha_of('a')
        ),
        Ok(CiAnswer {
            state: CiState::Idle,
            branch_has_runs: true,
            observations: vec![
                expected_run(1, "main", "completed", &[]),
                expected_run(2, "main", "completed", &[])
            ],
            run_ids: Vec::new(),
            pull_requests: Vec::new(),
        })
    );
}

#[test]
fn ci_incomplete_page_is_unknown_not_idle() {
    let body = ci_body_with_total(&sha_of('a'), &["completed"; 99], 240);
    assert_eq!(
        parse_ci_response(&body, "main", &sha_of('a')),
        Err(FailureKind::Incomplete)
    );
}

#[test]
fn rows_without_a_branch_are_dropped_and_the_page_stays_complete() {
    let body = serde_json::json!({
        "total_count": 2,
        "workflow_runs": [
            { "status": "in_progress" },
            run_row(1, &sha_of('a'), "main", "completed", &[]),
        ],
    })
    .to_string();
    assert_eq!(
        parse_ci_response(&body, "main", &sha_of('a')),
        Ok(CiAnswer {
            state: CiState::Idle,
            branch_has_runs: true,
            observations: vec![expected_run(1, "main", "completed", &[])],
            run_ids: Vec::new(),
            pull_requests: Vec::new(),
        }),
        "a row with no head_branch cannot answer for any branch"
    );

    let short_page = serde_json::json!({
        "total_count": 5,
        "workflow_runs": [{ "head_branch": "main", "status": "completed" }],
    })
    .to_string();
    assert_eq!(
        parse_ci_response(&short_page, "main", &sha_of('a')),
        Err(FailureKind::Incomplete),
        "the missing rows might be other branches, so the page cannot answer"
    );
}

#[test]
fn branch_filter_is_exact_match() {
    let body = ci_rows(
        &sha_of('a'),
        &[
            ("main-2", "in_progress"),
            ("origin/main", "in_progress"),
            ("main", "completed"),
        ],
    );
    assert_eq!(
        parse_ci_response(&body, "main", &sha_of('a')),
        Ok(CiAnswer {
            state: CiState::Idle,
            branch_has_runs: true,
            observations: vec![expected_run(3, "main", "completed", &[])],
            run_ids: Vec::new(),
            pull_requests: Vec::new(),
        })
    );
    assert_eq!(
        parse_ci_response(&body, "main-2", &sha_of('a')),
        Ok(CiAnswer {
            state: CiState::Running,
            branch_has_runs: true,
            observations: vec![expected_run(1, "main-2", "in_progress", &[])],
            run_ids: vec![1],
            pull_requests: Vec::new(),
        })
    );
}

#[test]
fn compare_status_table_maps_to_staleness() {
    assert_eq!(
        parse_compare_response(&compare_body("identical", 0, 0)),
        Ok((StalenessState::Current, None, Some(0), true))
    );
    assert_eq!(
        parse_compare_response(&compare_body("ahead", 0, 1)),
        Ok((StalenessState::Current, None, Some(1), false)),
        "ahead with behind_by 0 is not stale"
    );
    assert_eq!(
        parse_compare_response(&compare_body("behind", 3, 0)),
        Ok((StalenessState::Stale, Some(3), Some(0), false)),
        "behind carries ahead_by 0: nothing of its own to validate"
    );
    assert_eq!(
        parse_compare_response(&compare_body("diverged", 7, 2)),
        Ok((StalenessState::Stale, Some(7), Some(2), false))
    );
}

/// #2141 fail-open: `ahead_by` is a suppression input, not part of the
/// classification, so a response without the field still maps to the same
/// staleness state and reads `None`, which never suppresses.
#[test]
fn compare_without_ahead_by_reads_none_and_keeps_its_state() {
    let body = serde_json::json!({ "status": "behind", "behind_by": 4 }).to_string();
    assert_eq!(
        parse_compare_response(&body),
        Ok((StalenessState::Stale, Some(4), None, false))
    );
    let out_of_range = serde_json::json!({
        "status": "behind",
        "behind_by": 4,
        "ahead_by": u64::from(u32::MAX) + 1,
    })
    .to_string();
    assert_eq!(
        parse_compare_response(&out_of_range),
        Ok((StalenessState::Stale, Some(4), None, false))
    );
}

#[tokio::test]
async fn unpushed_head_404_maps_to_unknown() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        // CI: no runs exist for an unpushed SHA, which is honestly idle.
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        // Staleness: compare returns 404, which is unknown, not current.
        gh.compare.push_back(Ok(GhCallOutput {
            stdout: String::new(),
            stderr: "gh: Not Found (HTTP 404)".to_string(),
            success: false,
        }));
    }

    harness.round(Instant::now(), Local::now()).await;

    let snapshot = harness.snapshot();
    let activity = snapshot.get(&repo).expect("snapshot entry");
    assert_eq!(activity.ci, CiState::Idle);
    assert_eq!(activity.staleness, StalenessState::Unknown);
}

#[tokio::test]
async fn every_failure_maps_to_unknown() {
    let _guard = round_test_lock().await;
    type ScriptedFailure = Box<dyn Fn(&mut GhScripts)>;
    let cases: Vec<ScriptedFailure> = vec![
        Box::new(|gh: &mut GhScripts| gh.ci.push_back(Err(FailureKind::Timeout))),
        Box::new(|gh: &mut GhScripts| {
            gh.ci.push_back(Ok(GhCallOutput {
                stdout: String::new(),
                stderr: "gh: Forbidden (HTTP 403)".to_string(),
                success: false,
            }));
        }),
        Box::new(|gh: &mut GhScripts| {
            gh.ci.push_back(Ok(GhCallOutput {
                stdout: String::new(),
                stderr: "gh: Not Found (HTTP 404)".to_string(),
                success: false,
            }));
        }),
        Box::new(|gh: &mut GhScripts| {
            gh.ci.push_back(Ok(GhCallOutput {
                stdout: String::new(),
                stderr: "fatal: something".to_string(),
                success: false,
            }));
        }),
        Box::new(|gh: &mut GhScripts| gh.ci.push_back(Ok(ok_output("{not json")))),
        // No origin and a non-GitHub remote: no key at all.
        Box::new(|_gh: &mut GhScripts| {}),
        Box::new(|_gh: &mut GhScripts| {}),
    ];

    for (index, case) in cases.iter().enumerate() {
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo(&format!("repo-{index}"));
        harness.set_work(std::slice::from_ref(&repo));
        if index == 5 {
            harness
                .git
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_origin_failure(&repo, "no origin");
        }
        if index == 6 {
            harness
                .git
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_origin(&repo, "https://gitlab.com/a/b.git");
        }
        case(&mut harness.gh.lock().unwrap_or_else(|e| e.into_inner()));

        harness.round(Instant::now(), Local::now()).await;

        let snapshot = harness.snapshot();
        let activity = snapshot.get(&repo).expect("snapshot entry");
        assert_eq!(
            activity.ci,
            CiState::Unknown,
            "case {index} must read Unknown, never Idle/Running"
        );
    }
}

#[tokio::test]
async fn failure_does_not_retain_the_previous_state() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci
            .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
        gh.ci.push_back(Err(FailureKind::Timeout));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").ci,
        CiState::Running
    );

    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;
    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").ci,
        CiState::Unknown,
        "a retained Running would be a permanent yellow chip over a finished CI"
    );
}

#[tokio::test]
async fn cold_start_emits_no_transition() {
    let _guard = round_test_lock().await;
    for body in [
        ci_body(&sha_of('a'), &["in_progress"]),
        ci_body(&sha_of('a'), &[]),
    ] {
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ci
            .push_back(Ok(ok_output(&body)));

        harness.round(Instant::now(), Local::now()).await;

        assert!(
            harness.drain_transitions().is_empty(),
            "no edge out of Unknown exists"
        );
    }
}
