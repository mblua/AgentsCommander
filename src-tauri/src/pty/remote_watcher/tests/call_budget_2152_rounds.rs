use super::*;

#[test]
fn both_features_disabled_does_not_spawn_the_thread() {
    let probe_calls = Arc::new(AtomicUsize::new(0));
    let probe: GhProbe = {
        let probe_calls = Arc::clone(&probe_calls);
        Arc::new(move || {
            probe_calls.fetch_add(1, Ordering::SeqCst);
            Some(PathBuf::from("gh"))
        })
    };
    let spawner: GhSpawner = Arc::new(|_spec| Box::pin(async { Err(FailureKind::Other) }));
    let local_git: LocalGitRunner =
        Arc::new(|_path, _args| Box::pin(async { Err("unused".to_string()) }));
    let settings = AppSettings {
        ci_activity_enabled: false,
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let settings: SettingsState = crate::config::settings::new_settings_state(settings);
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

    assert!(sweeper.start(ShutdownSignal::new()).is_none());
    assert_eq!(probe_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn disabled_axis_is_skipped_in_the_round() {
    let _guard = round_test_lock().await;
    let settings = AppSettings {
        ci_activity_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::new(settings);
    let repo = harness.repo("repo-a");
    harness.set_work(&[repo]);

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    tick(&mut now, &mut wall, 300);
    harness.round(now, wall).await;

    let gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(gh.count("/actions/runs?"), 0);
    assert!(gh.count("/compare/") > 0, "staleness still runs");
}

#[test]
fn clamps_hold_at_the_floor_and_ceiling() {
    assert_eq!(clamp_interval_secs("ci", 0), 10);
    assert_eq!(clamp_interval_secs("ci", 99999), 3600);
    assert_eq!(clamp_interval_secs("staleness", 0), 10);
    assert_eq!(clamp_interval_secs("staleness", 99999), 3600);
    assert_eq!(clamp_interval_secs("ci", 30), 30);
}

#[tokio::test]
async fn snapshot_is_keyed_by_the_exact_unnormalized_path() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    harness.repo("repo-b");
    let unnormalized = format!(
        "{}/./repo-b",
        harness.temp.path().to_string_lossy().replace('\\', "/")
    );
    crate::pty::git_watcher::publish_git_status(
        &unnormalized,
        Some(crate::pty::git_watcher::GitStatus {
            branch: Some("main".to_string()),
            dirty: false,
        }),
        harness.git_state.as_ref(),
    );
    harness.set_work(std::slice::from_ref(&unnormalized));

    harness.round(Instant::now(), Local::now()).await;

    let snapshot = harness.snapshot();
    assert!(
        snapshot.contains_key(&unnormalized),
        "the exact path string must be the key, never a normalized form"
    );
}

#[tokio::test]
async fn gc_removes_a_path_that_left_the_work_list() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let first = harness.repo("repo-a");
    let second = harness.repo("repo-b");
    harness.set_work(&[first.clone(), second.clone()]);
    harness.round(Instant::now(), Local::now()).await;
    assert_eq!(harness.snapshot().len(), 2);

    harness.set_work(std::slice::from_ref(&first));
    harness.round(Instant::now(), Local::now()).await;

    let snapshot = harness.snapshot();
    assert!(snapshot.contains_key(&first));
    assert!(!snapshot.contains_key(&second));
}

#[tokio::test]
async fn gc_runs_on_an_empty_work_list() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let first = harness.repo("repo-a");
    let second = harness.repo("repo-b");
    harness.set_work(&[first.clone(), second.clone()]);
    harness.round(Instant::now(), Local::now()).await;
    assert_eq!(harness.snapshot().len(), 2);
    let emitted_before = harness.emitted_count();

    harness.set_work(&[]);
    harness.round(Instant::now(), Local::now()).await;

    assert!(harness.snapshot().is_empty());
    assert_eq!(
        harness.emitted_count(),
        emitted_before + 1,
        "the emptied payload is emitted exactly once"
    );
}

#[test]
fn payload_wire_shape_is_camel_case() {
    let payload = RemoteActivityPayload {
        repo_paths: vec!["C:/wg/repo-a".to_string()],
        ci_states: vec![CiState::Running],
        staleness_states: vec![StalenessState::Stale],
        behind_by: vec![Some(3)],
    };

    assert_eq!(
        serde_json::to_value(&payload).expect("serialize"),
        serde_json::json!({
            "repoPaths": ["C:/wg/repo-a"],
            "ciStates": ["running"],
            "stalenessStates": ["stale"],
            "behindBy": [3],
        })
    );
}

#[tokio::test]
async fn payload_vectors_are_the_same_length_and_sorted_by_path() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let first = harness.repo("repo-a");
    let second = harness.repo("repo-b");
    // Deliberately reversed so the sort has something to do.
    harness.set_work(&[second.clone(), first.clone()]);

    harness.round(Instant::now(), Local::now()).await;

    let emitted = harness.emitted.lock().unwrap_or_else(|e| e.into_inner());
    let payload = emitted.last().expect("payload");
    assert_eq!(payload.repo_paths, vec![first, second]);
    assert_eq!(payload.repo_paths.len(), payload.ci_states.len());
    assert_eq!(payload.repo_paths.len(), payload.staleness_states.len());
    assert_eq!(payload.repo_paths.len(), payload.behind_by.len());
}

#[tokio::test]
async fn event_is_gated_on_whole_payload_equality() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let repo = harness.repo("repo-a");
    harness.set_work(&[repo]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        gh.ci
            .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    assert_eq!(harness.emitted_count(), 1);

    // Nothing due: identical payload, no second emission.
    harness.round(now, wall).await;
    assert_eq!(harness.emitted_count(), 1);

    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;
    assert_eq!(
        harness.emitted_count(),
        2,
        "a single state flip emits again"
    );
}

#[tokio::test]
async fn transition_channel_full_drops_and_keeps_publishing() {
    let _guard = round_test_lock().await;
    let harness = Harness::with_capacity(AppSettings::default(), 1);
    let first = harness.repo("repo-a");
    let second = harness.repo("repo-b");
    harness.set_work(&[first.clone(), second.clone()]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci
            .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
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
    harness.round(now, wall).await;
    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;

    assert!(harness.try_recv_transition().is_none());
    tick(&mut now, &mut wall, 10);
    harness.round(now, wall).await;
    assert!(harness.try_recv_transition().is_some());
    assert!(harness.try_recv_transition().is_none(), "one was dropped");
    let snapshot = harness.snapshot();
    assert_eq!(snapshot.get(&first).expect("first").ci, CiState::Idle);
    assert_eq!(snapshot.get(&second).expect("second").ci, CiState::Idle);
    assert_eq!(harness.emitted_count(), 2, "publishing continued");
}

#[tokio::test]
async fn transition_channel_closed_drops_and_keeps_publishing() {
    let _guard = round_test_lock().await;
    let harness = Harness::with_capacity(AppSettings::default(), 4);
    let repo = harness.repo("repo-a");
    harness.set_work(std::slice::from_ref(&repo));
    harness.close_transitions();
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        gh.ci
            .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
        gh.ci
            .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;

    assert!(harness.sweeper.lock_state().closed_warned);
    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").ci,
        CiState::Running
    );

    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;
    assert!(
        harness.sweeper.lock_state().closed_warned,
        "the latch stays set after a second round"
    );
    assert_eq!(harness.emitted_count(), 2, "publishing continued");
}

#[tokio::test]
async fn ci_transitions_carry_an_empty_base_branch() {
    let _guard = round_test_lock().await;
    let settings = AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::new(settings);
    let repo = harness.repo("repo-a");
    harness.set_work(&[repo]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        gh.ci
            .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
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
    assert_eq!(transitions[1].kind, TransitionKind::CiFinished);
    for transition in &transitions {
        assert_eq!(transition.base_branch, "");
        assert_eq!(transition.behind_by, None);
    }
    assert_eq!(
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .count_repo_info(),
        1,
        "the label is resolved once per nwo: CI needs it to decide suppression"
    );
}

#[tokio::test]
async fn base_branch_resolution_walks_the_three_step_chain() {
    let _guard = round_test_lock().await;

    // Step 1: the GitHub default branch.
    {
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/2131".to_string()),
                dirty: false,
            }),
            harness.git_state.as_ref(),
        );
        harness.set_work(&[repo]);
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
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;
        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].base_branch, "main");
    }

    // Step 2: the local clone's cached origin HEAD, with `origin/` stripped.
    {
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
        }
        harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .symbolic_ref
            .push_back(Ok("origin/develop".to_string()));
        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;
        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].base_branch, "develop");
    }

    // Step 3: the literal label.
    {
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
        }
        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;
        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].base_branch, DEFAULT_BRANCH_LABEL);
    }
}

#[tokio::test]
async fn base_branch_is_resolved_once_per_nwo_and_cached() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let first = harness.repo("repo-a");
    let second = harness.repo("repo-b");
    harness.set_work(&[first.clone(), second.clone()]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..10 {
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
        }
    }
    {
        let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
        git.set_origin(&first, "git@github.com:mblua/AgentsCommander.git");
        git.set_origin(&second, "https://github.com/mblua/other.git");
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    for _ in 0..5 {
        harness.round(now, wall).await;
        tick(&mut now, &mut wall, 300);
    }

    assert_eq!(
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .count_repo_info(),
        2,
        "exactly one repos/<nwo> call per distinct nwo, for the process lifetime"
    );
    let snapshot = harness.snapshot();
    assert_eq!(
        snapshot.get(&first).expect("first").staleness,
        StalenessState::Stale,
        "a failed label resolution never changes the answer"
    );
    assert_eq!(
        snapshot.get(&second).expect("second").staleness,
        StalenessState::Stale
    );
}

#[test]
fn gh_command_spec_pins_the_child_environment_and_creation_flags() {
    let spec = build_gh_command_spec(
        Path::new("gh"),
        GhQuery::RepoInfo {
            nwo: "mblua/AgentsCommander",
        },
    )
    .expect("spec");

    let mut envs = spec.envs.clone();
    envs.sort();
    assert_eq!(
        envs,
        vec![
            ("GH_NO_UPDATE_NOTIFIER".to_string(), "1".to_string()),
            ("GH_PAGER".to_string(), "cat".to_string()),
            ("GH_PROMPT_DISABLED".to_string(), "1".to_string()),
        ],
        "whole-vector equality: a silently dropped variable must fail"
    );

    #[cfg(windows)]
    assert_eq!(spec.creation_flags, 0x08000000);
    #[cfg(not(windows))]
    assert_eq!(spec.creation_flags, 0);
}

#[tokio::test]
async fn every_round_process_goes_through_the_spawner_seam() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let first = harness.repo("repo-a");
    let second = harness.repo("repo-b");
    harness.set_work(&[first.clone(), second.clone()]);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        gh.compare
            .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
    }
    {
        let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
        git.set_head_sha(&first, 'a');
        git.set_head_sha(&second, 'b');
        git.set_origin(&first, "git@github.com:mblua/AgentsCommander.git");
        git.set_origin(&second, "https://github.com/mblua/other.git");
    }

    let now = Instant::now();
    let wall = Local::now();
    harness.round(now, wall).await;

    let gh_calls = harness
        .gh
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .calls
        .len();
    let git_calls = harness
        .git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .calls
        .len();
    assert_eq!(gh_calls, 6, "2 CI + 2 compare + 2 base labels");
    assert_eq!(git_calls, 4, "2 paths, 2 local reads each");
}

#[test]
fn no_code_path_can_build_a_gh_auth_invocation() {
    let gh = Path::new("gh");
    let sha = sha_of('a');
    let specs = [
        build_gh_command_spec(
            gh,
            GhQuery::Ci {
                nwo: "a/b",
                sha40: &sha,
            },
        )
        .expect("ci"),
        build_gh_command_spec(
            gh,
            GhQuery::Compare {
                nwo: "a/b",
                sha40: &sha,
            },
        )
        .expect("compare"),
        build_gh_command_spec(gh, GhQuery::RepoInfo { nwo: "a/b" }).expect("info"),
    ];
    for spec in &specs {
        assert_eq!(spec.args.first().map(String::as_str), Some("api"));
        assert!(
            !spec.args.iter().any(|arg| arg == "auth"),
            "gh auth login is never invoked on any path"
        );
    }
}

#[test]
fn round_log_level_is_debug() {
    assert_eq!(ROUND_LOG_LEVEL, log::Level::Debug);
}

#[tokio::test]
async fn archived_roots_are_normalized_before_filtering() {
    let _guard = round_test_lock().await;
    let harness = Harness::new(AppSettings::default());
    let archived_root = harness.temp.path().join("archived");
    let repo = archived_root.join(".ac").join("wg-1").join("repo-x");
    std::fs::create_dir_all(&repo).expect("create archived repo");
    let repo = repo.to_string_lossy().replace('\\', "/");
    let raw_root = format!("{}/.", archived_root.to_string_lossy().replace('\\', "/"));
    harness.set_work(std::slice::from_ref(&repo));

    let filtered = harness.sweeper.work_list(vec![raw_root.clone()]).await;
    assert!(
        !filtered.contains(&repo),
        "the normalized archived root must exclude the repo"
    );

    let unfiltered = crate::pty::git_watcher::build_work_list(
        vec![repo.clone()],
        vec![],
        std::slice::from_ref(&raw_root),
    );
    assert!(
        unfiltered.contains(&repo),
        "the raw spelling is the round-2 bug: it defeats the filter"
    );
}

#[test]
fn nwo_is_parsed_from_github_urls_only() {
    for origin in [
        "git@github.com:mblua/AgentsCommander.git",
        "https://github.com/mblua/AgentsCommander.git",
        "https://github.com/mblua/AgentsCommander",
        "ssh://git@github.com/mblua/AgentsCommander.git",
    ] {
        assert_eq!(
            parse_github_nwo(origin).as_deref(),
            Some("mblua/AgentsCommander"),
            "{origin}"
        );
    }

    for origin in [
        "https://gitlab.com/a/b.git",
        "https://github.com.evil.example/a/b",
        "https://github.com/a",
        "",
    ] {
        assert_eq!(parse_github_nwo(origin), None, "{origin}");
    }
}

#[test]
fn failure_interval_never_drops_below_base() {
    let one_hour = Duration::from_secs(3600);
    assert_eq!(
        failure_interval(
            FailureKind::Timeout,
            one_hour,
            Some(Duration::from_secs(900))
        ),
        one_hour
    );
    assert_eq!(
        failure_interval(FailureKind::RateLimited, one_hour, None),
        one_hour,
        "a rate-limited key must not re-query faster than a healthy one"
    );

    let thirty = Duration::from_secs(30);
    assert_eq!(
        failure_interval(FailureKind::RateLimited, thirty, None),
        Duration::from_secs(900)
    );
    assert_eq!(
        failure_interval(FailureKind::Timeout, thirty, None),
        Duration::from_secs(60)
    );
    assert_eq!(
        failure_interval(FailureKind::Timeout, thirty, Some(Duration::from_secs(900))),
        Duration::from_secs(900),
        "the cap holds"
    );
}
/// #2129 - T6: the three cases of the rendering rule, plus an empty label,
/// which takes case 3 and is unreachable for stale notices because
/// `for_remote_activity` rejects an empty stale base.
#[test]
fn issue_2129_base_branch_display_covers_every_case() {
    assert_eq!(
        base_branch_display("main", DEFAULT_BRANCH_LABEL),
        "the default branch on GitHub"
    );
    assert_eq!(
        base_branch_display("main", "main"),
        "its counterpart on GitHub"
    );
    assert_eq!(
        base_branch_display("feature/2083-2064-remote-alerts", "main"),
        "main on GitHub"
    );
    assert_eq!(base_branch_display("main", ""), " on GitHub");
}
