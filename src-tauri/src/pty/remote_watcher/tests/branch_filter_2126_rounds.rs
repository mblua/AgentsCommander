use super::*;

/// #2141: a branch with no commits of its own has nothing that a stale base
/// could invalidate, so the notice is suppressed while the chip still turns.
#[tokio::test]
async fn stale_without_own_commits_sends_no_notice_but_keeps_the_chip() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "fix/2141");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 2, 0))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    assert!(
        harness.drain_transitions().is_empty(),
        "no commits of its own: nothing to validate against a stale base"
    );
    let snapshot = harness.snapshot();
    let activity = snapshot.get(&repo).expect("entry");
    assert_eq!(
        activity.staleness,
        StalenessState::Stale,
        "the chip keeps its orange bar"
    );
    assert_eq!(activity.behind_by, Some(2));
}

/// #2141, the whole point: the suppressed round records `Current`, the commit
/// moves `head_sha` and the carry-over hands that `Current` to the new key,
/// so the notice arrives in the round OF the commit and exactly once.
///
/// The `head_sha` MUST change here. A same-sha version of this test exercises
/// one key and proves nothing about the carry-over.
#[tokio::test]
async fn stale_without_own_commits_notifies_once_after_the_first_own_commit() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "fix/2141");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 2, 0))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 5, 1))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    assert!(
        harness.drain_transitions().is_empty(),
        "round 1: empty branch, no notice"
    );
    drain(&harness);

    // The first commit of its own: a new `head_sha`, so a new QueryKey.
    harness
        .git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_head_sha(&repo, 'b');
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    let transitions = harness.drain_transitions();
    assert_eq!(
        transitions.len(),
        1,
        "round 2: the branch now has work of its own, and it is behind"
    );
    assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
    assert_eq!(transitions[0].behind_by, Some(2));

    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;
    assert!(
        harness.drain_transitions().is_empty(),
        "round 3: still stale on the same key, no second notice"
    );
}

/// #2141 closes a defect that predates it: a branch that is current, commits,
/// and only then falls behind used to observe `None -> Stale` on its new key,
/// which is not the edge the notice fires on, so it was never told at all.
#[tokio::test]
async fn branch_that_commits_while_behind_notifies_after_the_carry_over() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "fix/2141");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    harness
        .git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_head_sha(&repo, 'b');
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    let transitions = harness.drain_transitions();
    assert_eq!(
        transitions.len(),
        1,
        "the carry-over preserves the `Current` the commit used to destroy"
    );
    assert_eq!(transitions[0].behind_by, Some(2));
}

/// The other direction of the same carry: a `Stale` that already notified is
/// carried too, so a commit does not re-open the edge and notify twice. The
/// #2131 dedup now survives a commit, which it did not before.
#[tokio::test]
async fn carry_over_does_not_duplicate_a_notice_across_a_commit() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "fix/2141");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        // `ahead 0/1`, not `identical 0/1`: GitHub answers `ahead` for an
        // up-to-date branch that carries commits of its own.
        gh.compare
            .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 2, 2))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;
    assert_eq!(
        harness.drain_transitions().len(),
        1,
        "the branch is behind with work of its own: one notice"
    );

    harness
        .git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_head_sha(&repo, 'b');
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;
    assert!(
        harness.drain_transitions().is_empty(),
        "the new key inherits `Stale`, so there is no second edge"
    );
}

/// The carry moves the staleness axis ONLY. A new commit has genuinely
/// unknown CI, and seeding it would assert a run state nobody observed.
#[tokio::test]
async fn carry_over_leaves_the_ci_axis_unknown_on_a_new_sha() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings {
        ci_activity_enabled: true,
        ..AppSettings::default()
    });
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "fix/2141");
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 2, 0))));
        gh.ci.push_back(Ok(ok_output(&ci_rows(
            &sha_of('a'),
            &[("fix/2141", "completed")],
        ))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    {
        let state = harness.sweeper.lock_state();
        let (_, entry) = state.keys.iter().next().expect("one key");
        assert_eq!(entry.ci.confirmed, Some(CiState::Idle), "CI answered once");
    }

    harness
        .git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_head_sha(&repo, 'b');
    tick(&mut now, &mut wall, 300);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 2, 0))));
        gh.ci.push_back(Err(FailureKind::Other));
    }
    harness.round(now, wall).await;

    let state = harness.sweeper.lock_state();
    let (_, entry) = state.keys.iter().next().expect("one key");
    assert_eq!(
        entry.staleness.confirmed,
        Some(StalenessState::Current),
        "the staleness axis is carried across the new sha"
    );
    assert_eq!(
        entry.ci.confirmed, None,
        "the CI axis is NOT carried: a new commit has unknown CI"
    );
}

/// Two live paths on one `(nwo, branch)` at different commits — worktrees, or
/// two clones on the same branch. Neither is retired, so neither may donate:
/// a live key answering for itself must not be overwritten by the other.
///
/// The shape is load-bearing, and the obvious shapes do not test the rule.
/// One round proves nothing: on the first round `state.keys` is empty, so
/// there are no donors of any kind and the donor filter is never reached. Two
/// clean rounds prove nothing either: both keys would hold
/// `confirmed = Some(..)` and the `confirmed.is_none()` guard would protect
/// them whatever the filter said. One live key has to sit at
/// `confirmed = None` while the other holds a `Current`, and a failed compare
/// is the only way to get there.
///
/// Drop the `!groups.contains_key` filter and B is seeded from A's `Current`,
/// reaches `Current -> Stale` in round 2 and notifies, so this test fails.
/// Each key is scripted by its own sha through `route`, because two due keys
/// are polled concurrently and a shared queue would not be deterministic.
#[tokio::test]
async fn two_live_keys_on_one_branch_never_seed_each_other() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let first = harness.repo("repo-a");
    let second = harness.repo("repo-b");
    publish_branch(&first, "fix/2141");
    publish_branch(&second, "fix/2141");
    harness.set_work(&[first.clone(), second.clone()]);
    {
        let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
        git.set_head_sha(&first, 'a');
        git.set_head_sha(&second, 'b');
    }
    let sha_a = sha_of('a');
    let sha_b = sha_of('b');
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        // Round 1: A answers and records `Current`. B's compare fails, so B
        // keeps `confirmed = None` while both keys stay live.
        gh.route(
            &format!("repos/mblua/AgentsCommander/compare/HEAD...{sha_a}"),
            Ok(ok_output(&compare_body("ahead", 0, 1))),
        );
        gh.route(
            &format!("repos/mblua/AgentsCommander/compare/HEAD...{sha_b}"),
            Err(FailureKind::Other),
        );
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);

    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        // Round 2: A unchanged, B behind with work of its own. B's own history
        // is `None`, so `None -> Stale` is not an edge and B stays quiet —
        // unless it was wrongly seeded with A's `Current`.
        gh.route(
            &format!("repos/mblua/AgentsCommander/compare/HEAD...{sha_a}"),
            Ok(ok_output(&compare_body("ahead", 0, 1))),
        );
        gh.route(
            &format!("repos/mblua/AgentsCommander/compare/HEAD...{sha_b}"),
            Ok(ok_output(&compare_body("behind", 3, 1))),
        );
    }
    // 900s, not 300s: B's failed round backs off to 600s, and the rule is
    // only exercised when BOTH keys are due and live in the same round.
    tick(&mut now, &mut wall, 900);
    harness.round(now, wall).await;

    assert!(
        harness.drain_transitions().is_empty(),
        "a live key must not donate to another live key on the same branch"
    );
}

/// #2141 fails open on a missing `ahead_by`: the field is a suppression
/// input, and an absent one is not evidence that the branch is empty.
#[tokio::test]
async fn stale_with_a_missing_ahead_by_still_sends_the_notice() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "fix/2141");
    harness.set_work(std::slice::from_ref(&repo));
    let no_ahead_by = serde_json::json!({ "status": "behind", "behind_by": 4 }).to_string();
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        gh.compare.push_back(Ok(ok_output(&no_ahead_by)));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    let transitions = harness.drain_transitions();
    assert_eq!(transitions.len(), 1, "an unknown ahead_by never suppresses");
    assert_eq!(transitions[0].behind_by, Some(4));
}

#[tokio::test]
async fn stale_on_a_feature_branch_still_sends_the_notice() {
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
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 4, 1))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    let transitions = harness.drain_transitions();
    assert_eq!(transitions.len(), 1, "a feature branch still notifies");
    assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
    assert_eq!(transitions[0].base_branch, "main");
    assert_eq!(transitions[0].behind_by, Some(4));
    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").staleness,
        StalenessState::Stale
    );
}

#[tokio::test]
async fn stale_with_an_unresolved_default_branch_still_sends_the_notice() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    // Overwrite `Harness::repo`'s `main` with the sentinel literal itself: an
    // unresolved base and a branch equal to that base are the one pair a bare
    // `key.branch == base_label` comparison would suppress, so this test fails
    // if `base_label != DEFAULT_BRANCH_LABEL &&` is removed. Production cannot
    // reach this pair (git refnames forbid spaces), which is why publishing it
    // is safe here and why the sentinel guard is the only thing under test.
    crate::pty::git_watcher::publish_git_status(
        &repo,
        Some(crate::pty::git_watcher::GitStatus {
            branch: Some(DEFAULT_BRANCH_LABEL.to_string()),
            dirty: false,
        }),
    );
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        // No repo_info and no symbolic-ref answer is queued, so the default
        // branch stays unresolved and the sentinel is the base label; the
        // published branch equals that sentinel, so the sentinel guard is the
        // only thing keeping the notice alive.
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("behind", 4, 1))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    drain(&harness);
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    let transitions = harness.drain_transitions();
    assert_eq!(
        transitions.len(),
        1,
        "an unresolved default branch cannot suppress"
    );
    assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
    assert_eq!(transitions[0].base_branch, DEFAULT_BRANCH_LABEL);
    // Companion premise assertion: the branch this notice was produced for is
    // the sentinel itself, so the test cannot pass on a live `main` by accident.
    assert_eq!(transitions[0].branch, DEFAULT_BRANCH_LABEL);
    assert_eq!(transitions[0].behind_by, Some(4));
}

/// #2329: two rooms sharing one repo and one commit on the resolved default
/// branch behave identically - both stay idle, neither gets a CI notice -
/// while the shared key still costs exactly one CI query per round.
#[tokio::test]
async fn two_rooms_on_one_default_commit_are_one_call_and_no_ci_transition() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let first = harness.repo("repo-a");
    let second = harness.repo("repo-b");
    harness.set_work(&[first.clone(), second.clone()]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.ci
            .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    let first_round_calls = harness
        .gh
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .count("/actions/runs?");
    assert_eq!(first_round_calls, 1, "one call for one commit");

    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;

    assert!(
        harness.drain_transitions().is_empty(),
        "the resolved default branch emits no CI notice for either room"
    );
    assert_eq!(
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .count("/actions/runs?"),
        2,
        "one call per round for the shared key"
    );
    let snapshot = harness.snapshot();
    for path in [&first, &second] {
        assert_eq!(
            snapshot.get(path).expect("entry").ci,
            CiState::Idle,
            "both rooms publish the same idle chip"
        );
    }
}

#[tokio::test]
async fn backoff_is_per_key_and_per_axis() {
    let _guard = round_test_lock().await;
    let settings = AppSettings {
        ci_sweep_min_interval_secs: 30,
        branch_staleness_interval_secs: 300,
        ..AppSettings::default()
    };
    let harness = Harness::new(settings);
    let first = harness.repo("repo-a");
    let second = harness.repo("repo-b");
    harness.set_work(&[first.clone(), second.clone()]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..4 {
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        }
        // The failing repository is scripted by endpoint, so the assertion
        // cannot depend on which of two concurrent queries is polled first.
        // Only the compare calls for the failing nwo fail, so a routed marker
        // cannot be consumed by the base-label or CI calls.
        gh.route(
            &format!("repos/mblua/failing/compare/HEAD...{}", sha_of('a')),
            Err(FailureKind::Timeout),
        );
        gh.route(
            &format!("repos/mblua/failing/compare/HEAD...{}", sha_of('a')),
            Err(FailureKind::Timeout),
        );
    }
    {
        let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
        git.set_origin(&first, "git@github.com:mblua/failing.git");
        git.set_origin(&second, "git@github.com:mblua/healthy.git");
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    assert_eq!(compare_count(&harness, "failing"), 1);
    assert_eq!(compare_count(&harness, "healthy"), 1);

    tick(&mut now, &mut wall, 29);
    harness.round(now, wall).await;
    eprintln!(
        "DEBUG calls: {:?}",
        harness.gh.lock().unwrap_or_else(|e| e.into_inner()).calls
    );
    assert_eq!(
        ci_count(&harness),
        2,
        "two keys, no CI query before their due time"
    );

    tick(&mut now, &mut wall, 1);
    harness.round(now, wall).await;
    assert_eq!(
        ci_count(&harness),
        4,
        "the failing staleness axis must not disturb the CI cadence"
    );
    assert_eq!(
        compare_count(&harness, "failing"),
        1,
        "the failing key doubles: 300 -> 600 and is not due yet"
    );

    tick(&mut now, &mut wall, 569);
    harness.round(now, wall).await;
    assert_eq!(
        compare_count(&harness, "failing"),
        1,
        "not queried at due - 1s"
    );

    tick(&mut now, &mut wall, 1);
    harness.round(now, wall).await;
    assert_eq!(
        compare_count(&harness, "failing"),
        2,
        "queried at the doubled interval"
    );
    assert_eq!(
        compare_count(&harness, "healthy"),
        2,
        "the healthy key follows its own cadence"
    );
}

#[tokio::test]
async fn rate_limit_jumps_straight_to_the_cap() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(&[repo]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        gh.ci.push_back(Ok(GhCallOutput {
            stdout: String::new(),
            stderr: "gh: API rate limit exceeded for user ID 1 (HTTP 403)".to_string(),
            success: false,
        }));
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
    assert_eq!(due_at, now + Duration::from_secs(900));
}
