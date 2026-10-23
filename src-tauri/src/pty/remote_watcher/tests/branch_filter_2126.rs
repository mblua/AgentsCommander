use super::*;

#[tokio::test]
async fn ci_counts_only_runs_on_the_current_branch() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci.push_back(Ok(ok_output(&ci_rows(
            &sha_of('a'),
            &[
                ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                ("main", "completed"),
            ],
        ))));
    }

    harness.round(Instant::now(), Local::now()).await;

    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").ci,
        CiState::Idle,
        "only the run on the repo's current branch counts"
    );
}

#[tokio::test]
async fn incident_replay_identical_branch_and_default_get_no_notice() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let default_repo = harness.repo("repo-a");
    let feature_repo = harness.repo("repo-b");
    crate::pty::git_watcher::publish_git_status(
        &feature_repo,
        Some(crate::pty::git_watcher::GitStatus {
            branch: Some("fix/2124-ignore-short-idle-bursts".to_string()),
            dirty: false,
        }),
    );
    harness.set_work(&[default_repo.clone(), feature_repo.clone()]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        let round_one = ci_rows(
            &sha_of('a'),
            &[
                ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                ("main", "completed"),
            ],
        );
        let round_two = ci_rows(
            &sha_of('a'),
            &[
                ("fix/2124-ignore-short-idle-bursts", "completed"),
                ("fix/2124-ignore-short-idle-bursts", "completed"),
                ("main", "completed"),
            ],
        );
        // Both keys poll every round and the queue is drained in call order,
        // so both orders must see that round's body.
        for body in [&round_one, &round_two, &round_one, &round_two] {
            gh.ci.push_back(Ok(ok_output(body)));
        }
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    let round_one_feature = harness
        .snapshot()
        .get(&feature_repo)
        .expect("feature repo")
        .ci;
    let round_one_default = harness
        .snapshot()
        .get(&default_repo)
        .expect("default repo")
        .ci;

    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;

    let transitions = harness.drain_transitions();
    assert_eq!(
        transitions.len(),
        0,
        "the incident emitted CiFinished to both rooms"
    );
    let snapshot = harness.snapshot();
    assert_eq!(
        round_one_feature,
        CiState::Idle,
        "round 1: the feature branch is identical to the default branch"
    );
    assert_eq!(round_one_default, CiState::Idle);
    assert_eq!(
        snapshot.get(&feature_repo).expect("feature repo").ci,
        CiState::Idle
    );
    assert_eq!(
        snapshot.get(&default_repo).expect("default repo").ci,
        CiState::Idle
    );
}

#[tokio::test]
async fn non_identical_branch_notifies_only_its_own_rooms() {
    let _guard = round_test_lock().await;
    let settings = AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::new(settings);
    let default_repo = harness.repo("repo-a");
    let feature_repo = harness.repo("repo-b");
    crate::pty::git_watcher::publish_git_status(
        &feature_repo,
        Some(crate::pty::git_watcher::GitStatus {
            branch: Some("fix/2124-ignore-short-idle-bursts".to_string()),
            dirty: false,
        }),
    );
    harness.set_work(&[default_repo.clone(), feature_repo.clone()]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        for _ in 0..4 {
            gh.compare
                .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
        }
        let bodies = [
            ci_rows(
                &sha_of('a'),
                &[("fix/2124-ignore-short-idle-bursts", "completed")],
            ),
            ci_rows(
                &sha_of('a'),
                &[("fix/2124-ignore-short-idle-bursts", "in_progress")],
            ),
            ci_rows(
                &sha_of('a'),
                &[("fix/2124-ignore-short-idle-bursts", "completed")],
            ),
            ci_rows(
                &sha_of('a'),
                &[("fix/2124-ignore-short-idle-bursts", "completed")],
            ),
        ];
        for body in bodies {
            gh.ci.push_back(Ok(ok_output(&body)));
            gh.ci.push_back(Ok(ok_output(&body)));
        }
    }

    queue_completion(
        &harness,
        "mblua/AgentsCommander",
        run_row(
            1,
            &sha_of('a'),
            "fix/2124-ignore-short-idle-bursts",
            "completed",
            &[],
        ),
    );
    let mut now = Instant::now();
    let mut wall = Local::now();
    let mut transitions = Vec::new();
    for round in 1..=4 {
        harness.round(now, wall).await;
        let edges = harness.drain_transitions();
        match round {
            2 | 4 => {
                assert_eq!(edges.len(), 1, "round {round}");
                assert_eq!(
                    edges[0].kind,
                    if round == 2 {
                        TransitionKind::CiStarted
                    } else {
                        TransitionKind::CiFinished
                    }
                );
                assert_eq!(edges[0].observed_at, wall);
            }
            _ => assert!(edges.is_empty(), "round {round} must have no edge"),
        }
        transitions.extend(edges);
        tick(&mut now, &mut wall, 60);
    }
    assert_eq!(transitions.len(), 2);
    assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
    assert_eq!(transitions[0].repo_path, feature_repo);
    assert_eq!(transitions[1].kind, TransitionKind::CiFinished);
    assert_eq!(transitions[1].repo_path, feature_repo);
    assert_eq!(
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .count("/compare/"),
        4,
        "one identity compare per round for the non-default branch, none for the default one"
    );
}

/// #2329: a repo on its RESOLVED default branch publishes no CI at all,
/// even while GitHub has a run in progress on that exact branch and SHA.
/// The CI query is still issued (the reserved token is spent); only its
/// answer is suppressed.
#[tokio::test]
async fn resolved_default_branch_ci_is_suppressed() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        gh.ci
            .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    for round in 0..3 {
        harness.round(now, wall).await;
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Idle,
            "round {round}: a resolved default branch publishes Idle, never Running"
        );
        tick(&mut now, &mut wall, 60);
    }

    assert!(
        harness.drain_transitions().is_empty(),
        "a suppressed CI answer has no edge into or out of it"
    );
    {
        let state = harness.sweeper.lock_state();
        let (_, entry) = state.keys.iter().next().expect("one key");
        assert_eq!(
            entry.ci.confirmed, None,
            "the transition chain stays broken across suppressed rounds"
        );
    }
    let gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        gh.count("/actions/runs?"),
        3,
        "the CI call is still issued every round; only its answer is suppressed"
    );
    assert_eq!(
        gh.count("/compare/"),
        1,
        "one staleness compare in round 1; the default branch adds no identity compare"
    );
}

#[tokio::test]
async fn unresolved_default_branch_does_not_suppress() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    crate::pty::git_watcher::publish_git_status(
        &repo,
        Some(crate::pty::git_watcher::GitStatus {
            branch: Some("fix/x".to_string()),
            dirty: false,
        }),
    );
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        // No repo_info answer is queued and no symbolic-ref answer is queued,
        // so the label stays unresolved and rule 2 cannot be decided.
        gh.ci.push_back(Ok(ok_output(&ci_rows(
            &sha_of('a'),
            &[("fix/x", "completed")],
        ))));
        gh.ci.push_back(Ok(ok_output(&ci_rows(
            &sha_of('a'),
            &[("fix/x", "in_progress")],
        ))));
        gh.ci.push_back(Ok(ok_output(&ci_rows(
            &sha_of('a'),
            &[("fix/x", "completed")],
        ))));
        gh.ci.push_back(Ok(ok_output(&ci_rows(
            &sha_of('a'),
            &[("fix/x", "completed")],
        ))));
    }

    queue_completion(
        &harness,
        "mblua/AgentsCommander",
        run_row(1, &sha_of('a'), "fix/x", "completed", &[]),
    );
    let mut now = Instant::now();
    let mut wall = Local::now();
    let mut transitions = Vec::new();
    for round in 1..=4 {
        harness.round(now, wall).await;
        let edges = harness.drain_transitions();
        match round {
            2 | 4 => {
                assert_eq!(edges.len(), 1, "round {round}");
                assert_eq!(
                    edges[0].kind,
                    if round == 2 {
                        TransitionKind::CiStarted
                    } else {
                        TransitionKind::CiFinished
                    }
                );
                assert_eq!(edges[0].observed_at, wall);
            }
            _ => assert!(edges.is_empty(), "round {round} must have no edge"),
        }
        transitions.extend(edges);
        tick(&mut now, &mut wall, 60);
    }
    assert_eq!(
        transitions.len(),
        2,
        "an unresolved default branch cannot suppress"
    );
    assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
    assert_eq!(transitions[1].kind, TransitionKind::CiFinished);
}

#[tokio::test]
async fn identity_compare_failure_is_unknown() {
    let _guard = round_test_lock().await;
    let settings = AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::new(settings);
    let repo = harness.repo("repo-a");
    crate::pty::git_watcher::publish_git_status(
        &repo,
        Some(crate::pty::git_watcher::GitStatus {
            branch: Some("fix/x".to_string()),
            dirty: false,
        }),
    );
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.ci.push_back(Ok(ok_output(&ci_rows(
            &sha_of('a'),
            &[("fix/x", "in_progress")],
        ))));
        gh.compare.push_back(Err(FailureKind::Other));
    }

    harness.round(Instant::now(), Local::now()).await;

    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").ci,
        CiState::Unknown,
        "a failed identity compare is Unknown, never a silent suppression"
    );
}

#[tokio::test]
async fn branch_change_on_same_commit_retires_the_key_without_a_transition() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci.push_back(Ok(ok_output(&ci_rows(
            &sha_of('a'),
            &[("main", "in_progress")],
        ))));
        gh.ci.push_back(Ok(ok_output(&ci_rows(
            &sha_of('a'),
            &[("fix/x", "completed")],
        ))));
    }
    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    crate::pty::git_watcher::publish_git_status(
        &repo,
        Some(crate::pty::git_watcher::GitStatus {
            branch: Some("fix/x".to_string()),
            dirty: false,
        }),
    );
    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;

    assert!(
        harness.drain_transitions().is_empty(),
        "a branch change on the same commit is a new key, not an edge"
    );
    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").ci,
        CiState::Idle
    );
}

#[tokio::test]
async fn unknown_is_transparent_and_never_expires() {
    let _guard = round_test_lock().await;
    // 1000s clears any backoff interval the failing rounds could have set,
    // and 3600s is the phase's long-advance variant; the point is that the
    // `Unknown` rounds never expire the previous confirmed state.
    for advance in [1000_u64, 3600] {
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
            gh.ci.push_back(Err(FailureKind::Timeout));
            gh.ci.push_back(Err(FailureKind::Timeout));
            gh.ci.push_back(Err(FailureKind::Timeout));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        }

        queue_completion(
            &harness,
            "mblua/AgentsCommander",
            run_row(1, &sha_of('a'), "main", "completed", &[]),
        );
        let mut now = Instant::now();
        let mut wall = Local::now();
        let running_wall = wall;
        harness.round(now, wall).await;
        for _ in 0..3 {
            tick(&mut now, &mut wall, advance);
            harness.round(now, wall).await;
        }
        tick(&mut now, &mut wall, advance);
        harness.round(now, wall).await;
        assert_eq!(
            harness.snapshot().values().next().unwrap().ci,
            CiState::Running
        );
        assert!(harness.drain_transitions().is_empty());
        tick(&mut now, &mut wall, 10);
        let closing_wall = wall;
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1, "advance={advance}");
        let transition = &transitions[0];
        assert_eq!(transition.kind, TransitionKind::CiFinished);
        assert_eq!(transition.last_confirmed_at, Some(running_wall));
        assert_eq!(transition.observed_at, closing_wall);
    }
}

#[tokio::test]
async fn last_confirmed_at_advances_on_every_confirmed_round() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(&[repo]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..20 {
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
        }
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
    }

    queue_completion(
        &harness,
        "mblua/AgentsCommander",
        run_row(1, &sha_of('a'), "main", "completed", &[]),
    );
    let mut now = Instant::now();
    let mut wall = Local::now();
    let first_wall = wall;
    let mut twentieth_wall = wall;
    for index in 0..20 {
        if index > 0 {
            tick(&mut now, &mut wall, 60);
        }
        if index == 19 {
            twentieth_wall = wall;
        }
        harness.round(now, wall).await;
    }
    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;

    assert!(harness.drain_transitions().is_empty());
    tick(&mut now, &mut wall, 10);
    harness.round(now, wall).await;
    let transitions = harness.drain_transitions();
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].last_confirmed_at, Some(twentieth_wall));
    assert_ne!(
        transitions[0].last_confirmed_at,
        Some(first_wall),
        "must be the 20th round, not the first"
    );
}

#[tokio::test]
async fn head_sha_change_retires_the_key_without_a_transition() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci
            .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
    }
    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    harness
        .git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_head_sha(&repo, 'b');
    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;

    assert!(
        harness.drain_transitions().is_empty(),
        "a new SHA starts at Unknown and must not emit the old SHA's finish"
    );
    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").ci,
        CiState::Idle
    );
}

#[tokio::test]
async fn stale_to_current_emits_nothing() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(&[repo]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    assert!(harness.drain_transitions().is_empty());
}

#[tokio::test]
async fn current_to_stale_emits_once_only() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    crate::pty::git_watcher::publish_git_status(
        &repo,
        Some(crate::pty::git_watcher::GitStatus {
            branch: Some("fix/2131".to_string()),
            dirty: false,
        }),
    );
    harness.set_work(&[repo]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        for _ in 0..6 {
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4, 1))));
        }
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    for _ in 0..6 {
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;
    }

    let transitions = harness.drain_transitions();
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
    assert_eq!(transitions[0].behind_by, Some(4));
}

#[tokio::test]
async fn stale_on_default_branch_sends_no_notice_but_keeps_the_chip() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 4, 0))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    assert!(
        harness.drain_transitions().is_empty(),
        "the default branch has no branch of its own to rebase: no notice"
    );
    let snapshot = harness.snapshot();
    let activity = snapshot.get(&repo).expect("entry");
    assert_eq!(
        activity.staleness,
        StalenessState::Stale,
        "the chip keeps its orange bar"
    );
    assert_eq!(activity.behind_by, Some(4));
}

/// #2141: a repo on its default branch reports `ahead_by == 0` too, so the
/// #2131 gate is evaluated FIRST and the `ahead_by` rule never runs there.
/// Own commits on the default branch change nothing.
#[tokio::test]
async fn stale_on_default_branch_with_own_commits_still_sends_no_notice() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("diverged", 4, 3))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    assert!(
        harness.drain_transitions().is_empty(),
        "#2131 wins over the #2141 rule on the default branch"
    );
    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").staleness,
        StalenessState::Stale
    );
}
