use super::*;

/// Positive control for the factory: `production_seams` must FORWARD its
/// argument. `None` could be a dropped default, so only the `Some` case can
/// fail when the parameter is discarded.
#[test]
fn remote_activity_cache_producer_factory_keeps_snapshot_dir() {
    let dir = PathBuf::from("snapshot-dir");
    assert_eq!(
        RemoteSweeper::production_seams(Some(dir.clone())).snapshot_dir,
        Some(dir)
    );
    assert_eq!(RemoteSweeper::production_seams(None).snapshot_dir, None);
}

/// Inspection guard for the real construction site: the factory test above
/// is the positive control, this one refuses a `lib.rs` that stopped
/// passing the instance config directory. Whitespace is flattened so a
/// `rustfmt` re-wrap cannot hide the call.
#[test]
fn remote_activity_cache_producer_lib_call_site_passes_config_dir() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("read lib.rs");
    let flattened: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        flattened.contains("RemoteSweeper::production_seams(crate::config::config_dir()"),
        "lib.rs must construct the sweeper with the instance config directory"
    );
}

/// Pure state, no logger capture and no sleeping: first failure, suppressed
/// inside the window, admitted at the boundary and later, and immediately
/// re-admitted after a success reset.
#[test]
fn remote_activity_cache_producer_warning_limiter_admits_once_per_window() {
    let mut limiter = SnapshotWarningLimiter::default();
    let start = Instant::now();
    assert!(limiter.admit(start), "the first failure warns");
    assert!(
        !limiter.admit(start + Duration::from_secs(29)),
        "just inside the window is suppressed"
    );
    assert!(
        limiter.admit(start + Duration::from_secs(30)),
        "the exact boundary admits"
    );
    assert!(
        !limiter.admit(start + Duration::from_secs(59)),
        "the new window suppresses again"
    );
    limiter.reset();
    assert!(
        limiter.admit(start + Duration::from_secs(59)),
        "a successful write resets the limiter"
    );
}

#[tokio::test]
async fn remote_activity_cache_producer_positive_wiring() {
    let _guard = round_test_lock().await;
    let snapshot_dir = tempfile::tempdir().expect("snapshot dir");
    let settings = AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::with_snapshot_dir(settings, Some(snapshot_dir.path().to_path_buf()));
    let repo_running = harness.repo("repo-a");
    let repo_idle = harness.repo("repo-b");
    publish_branch(&repo_running, "feature-a", harness.git_state.as_ref());
    publish_branch(&repo_idle, "feature-b", harness.git_state.as_ref());
    {
        let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
        git.set_head_sha(&repo_running, 'a');
        git.set_head_sha(&repo_idle, 'b');
    }
    harness.set_work(&[repo_running.clone(), repo_idle.clone()]);

    let publication = Utc::now();
    harness.set_publication_clock(publication);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        // A run on the branch, then an identity compare that says the branch
        // is ahead, so `Running` is published rather than suppressed.
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("feature-a", "in_progress")],
            ))),
        );
        gh.compare
            .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
        // No run on this branch: `Idle` needs no identity call.
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('b')
            ),
            Ok(ok_output(&ci_rows(&sha_of('b'), &[]))),
        );
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;

    let path = snapshot_dir.path().join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
    let bytes = std::fs::read(&path).expect("the exact destination exists");
    let raw: serde_json::Value = serde_json::from_slice(&bytes).expect("snapshot json");
    assert_eq!(raw["schemaVersion"], 1);
    assert_eq!(
        raw["generatedAt"],
        publication.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    let repos = raw["repos"].as_array().expect("repos array");
    assert_eq!(repos.len(), 2);
    let raw_paths: Vec<&str> = repos
        .iter()
        .map(|repo| repo["path"].as_str().expect("path"))
        .collect();
    let mut sorted = raw_paths.clone();
    sorted.sort();
    assert_eq!(raw_paths, sorted, "entries are sorted by raw path string");
    assert_eq!(repos[0]["path"], repo_running);
    assert_eq!(repos[0]["ciState"], "running");
    assert_eq!(repos[1]["path"], repo_idle);
    assert_eq!(repos[1]["ciState"], "idle");

    // Producer-to-reader round trip through the validating reader the CLI
    // uses: the produced bytes carry identical path/state data.
    let snapshot =
        crate::config::remote_activity_cache::read_snapshot(&path).expect("read produced");
    assert_eq!(
        snapshot.repos,
        vec![
            (
                repo_running.clone(),
                crate::config::remote_activity_cache::PersistedRepoCi {
                    state: crate::config::remote_activity_cache::PersistedCiState::Running,
                    run_ids: vec![1],
                    pull_requests: Vec::new(),
                    unknown_reason: None,
                },
            ),
            (
                repo_idle.clone(),
                crate::config::remote_activity_cache::PersistedRepoCi {
                    state: crate::config::remote_activity_cache::PersistedCiState::Idle,
                    run_ids: Vec::new(),
                    pull_requests: Vec::new(),
                    unknown_reason: None,
                },
            ),
        ]
    );

    // Empty-round GC: the next round owns an empty work list, so the file is
    // atomically replaced with `repos: []` instead of keeping survivors.
    tick(&mut now, &mut wall, 60);
    harness.set_work(&[]);
    harness.round(now, wall).await;
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read empty")).expect("empty json");
    assert_eq!(raw["repos"].as_array().expect("repos").len(), 0);
}

#[tokio::test]
async fn remote_activity_cache_producer_forced_write_failure_keeps_round_effects() {
    let _guard = round_test_lock().await;
    let blocker = tempfile::tempdir().expect("blocker dir");
    let blocked_destination = blocker.path().join("not-a-directory");
    std::fs::write(&blocked_destination, b"file").expect("blocker file");
    let settings = AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::with_snapshot_dir(settings, Some(blocked_destination.clone()));
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "feature-a", harness.git_state.as_ref());
    harness
        .git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_head_sha(&repo, 'a');
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("feature-a", "completed")],
            ))),
        );
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("feature-a", "in_progress")],
            ))),
        );
        for _ in 0..2 {
            gh.compare
                .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
        }
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;

    let transitions = harness.drain_transitions();
    assert_eq!(transitions.len(), 1, "the transition still fires");
    assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
    assert_eq!(harness.emitted_count(), 2, "both payloads were emitted");
    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").ci,
        CiState::Running,
        "the in-memory snapshot still updated"
    );
    assert!(
        harness
            .sweeper
            .lock_state()
            .snapshot_warnings
            .last_admitted
            .is_some(),
        "the forced failure was admitted once into the warning limiter"
    );
    assert!(
        !blocked_destination
            .join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME)
            .exists(),
        "the destination stays unwritten"
    );
}

#[tokio::test]
async fn remote_activity_cache_producer_publication_time_is_not_the_round_start() {
    let _guard = round_test_lock().await;
    let snapshot_dir = tempfile::tempdir().expect("snapshot dir");
    let settings = AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::with_snapshot_dir(settings, Some(snapshot_dir.path().to_path_buf()));
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "feature-a", harness.git_state.as_ref());
    harness.set_work(std::slice::from_ref(&repo));

    // Whole-second instant: the codec writes seconds precision.
    let publication =
        chrono::DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("whole second");
    harness.set_publication_clock(publication);
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&ci_rows(&sha_of('a'), &[]))),
        );
    }

    // Round-start wall deliberately more than the freshness window older
    // than the publication instant: reusing it would publish stale bytes.
    let round_start_wall = (publication - chrono::Duration::seconds(45)).with_timezone(&Local);
    harness.round(Instant::now(), round_start_wall).await;

    let path = snapshot_dir.path().join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
    let snapshot =
        crate::config::remote_activity_cache::read_snapshot(&path).expect("read produced");
    assert_eq!(snapshot.generated_at, publication);
    assert!(crate::config::remote_activity_cache::snapshot_is_fresh(
        &snapshot,
        publication
    ));
    assert!(
        !crate::config::remote_activity_cache::snapshot_is_fresh(
            &snapshot,
            round_start_wall.with_timezone(&Utc)
        ),
        "the round-start clock would have published already-stale bytes"
    );
}

#[tokio::test]
async fn remote_activity_cache_producer_missing_directory_skips_persistence_only() {
    let _guard = round_test_lock().await;
    let settings = AppSettings {
        branch_staleness_enabled: false,
        ..AppSettings::default()
    };
    let harness = Harness::with_snapshot_dir(settings, None);
    assert_eq!(
        harness.sweeper.snapshot_persistence,
        SnapshotPersistence::DisabledMissingAtConstruction,
        "construction records the one-time diagnostic state"
    );
    let repo = harness.repo("repo-a");
    publish_branch(&repo, "feature-a", harness.git_state.as_ref());
    harness
        .git
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_head_sha(&repo, 'a');
    harness.set_work(std::slice::from_ref(&repo));
    {
        let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        gh.repo_info
            .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("feature-a", "completed")],
            ))),
        );
        gh.route(
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("feature-a", "in_progress")],
            ))),
        );
        for _ in 0..2 {
            gh.compare
                .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
        }
    }

    let mut now = Instant::now();
    let mut wall = Local::now();
    harness.round(now, wall).await;
    tick(&mut now, &mut wall, 60);
    harness.round(now, wall).await;

    assert_eq!(harness.emitted_count(), 2, "payload emission is untouched");
    let transitions = harness.drain_transitions();
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
    assert_eq!(
        harness.snapshot().get(&repo).expect("entry").ci,
        CiState::Running,
        "post-GC memory is untouched"
    );
    assert!(
        harness
            .sweeper
            .lock_state()
            .snapshot_warnings
            .last_admitted
            .is_none(),
        "a missing directory is not a per-round persistence failure"
    );
}
