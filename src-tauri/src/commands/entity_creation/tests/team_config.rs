use super::*;

#[test]
fn team_config_serde_defaults_only_a_missing_alert_field() {
    let legacy: TeamConfigResult = serde_json::from_value(serde_json::json!({
        "agents": [],
        "coordinator": "",
        "repos": []
    }))
    .expect("legacy config");
    assert!(legacy.context_alert_percentages.is_empty());

    for invalid in [
        serde_json::Value::Null,
        serde_json::json!("50"),
        serde_json::json!({}),
        serde_json::json!([50.5]),
        serde_json::json!([-1]),
        serde_json::json!([256]),
    ] {
        let mut root = serde_json::json!({
            "agents": [],
            "coordinator": "",
            "repos": []
        });
        root["contextAlertPercentages"] = invalid;
        assert!(
            serde_json::from_value::<TeamConfigResult>(root).is_err(),
            "present unrepresentable alert data must fail"
        );
    }
}

#[test]
fn team_config_serializes_exact_camel_case_alert_key() {
    let empty = serde_json::to_value(empty_team_config(Vec::new())).expect("serialize empty");
    assert_eq!(empty["contextAlertPercentages"], serde_json::json!([]));
    assert!(empty.get("context_alert_percentages").is_none());

    let populated =
        serde_json::to_value(empty_team_config(vec![50, 75, 90])).expect("serialize populated");
    assert_eq!(
        populated["contextAlertPercentages"],
        serde_json::json!([50, 75, 90])
    );
    let round_trip: TeamConfigResult =
        serde_json::from_value(populated).expect("deserialize populated");
    assert_eq!(round_trip.context_alert_percentages, vec![50, 75, 90]);
}

#[test]
fn context_alert_normalizer_accepts_boundaries_and_sorts() {
    for (input, expected) in [
        (vec![], vec![]),
        (vec![1], vec![1]),
        (vec![100], vec![100]),
        (vec![50, 75], vec![50, 75]),
        (vec![50, 75, 90], vec![50, 75, 90]),
        (vec![90, 50, 75], vec![50, 75, 90]),
    ] {
        assert_eq!(
            normalize_context_alert_percentages(&input).expect("valid percentages"),
            expected
        );
    }
}

#[test]
fn context_alert_normalizer_rejects_product_invalid_values() {
    for invalid in [
        vec![0],
        vec![101],
        vec![1, 2, 3, 4],
        vec![50, 50],
        vec![75, 50, 75],
    ] {
        assert!(
            normalize_context_alert_percentages(&invalid).is_err(),
            "invalid percentages were accepted: {:?}",
            invalid
        );
    }
}

#[test]
fn context_alert_argument_resolution_pins_create_update_compatibility() {
    assert_eq!(
        resolve_context_alert_argument(ContextAlertWriteMode::Create, None, None)
            .expect("legacy create"),
        Vec::<u8>::new()
    );
    assert_eq!(
        resolve_context_alert_argument(
            ContextAlertWriteMode::Create,
            Some(vec![90, 50, 75]),
            None,
        )
        .expect("explicit create"),
        vec![50, 75, 90]
    );
    assert_eq!(
        resolve_context_alert_argument(ContextAlertWriteMode::Update, None, Some(&[50, 75]),)
            .expect("legacy update"),
        vec![50, 75]
    );
    assert_eq!(
        resolve_context_alert_argument(
            ContextAlertWriteMode::Update,
            Some(Vec::new()),
            Some(&[50, 75]),
        )
        .expect("explicit clear"),
        Vec::<u8>::new()
    );
    assert!(resolve_context_alert_argument(
        ContextAlertWriteMode::Update,
        Some(vec![0]),
        Some(&[50]),
    )
    .is_err());
    assert!(resolve_context_alert_argument(ContextAlertWriteMode::Update, None, None).is_err());
}

#[test]
fn classified_team_config_reader_reports_stable_error_classes() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);

    let missing = read_team_config_classified(&base, "missing").expect_err("missing");
    assert_eq!(missing.class(), "not_found");
    assert!(missing.to_string().contains("_team_missing"));

    let unreadable_dir = base.join("_team_unreadable");
    std::fs::create_dir_all(unreadable_dir.join("config.json")).expect("config directory");
    let unreadable = read_team_config_classified(&base, "unreadable").expect_err("unreadable");
    assert_eq!(unreadable.class(), "unreadable");
    assert!(unreadable.to_string().contains("config.json"));

    let partial_dir = base.join("_team_partial");
    std::fs::create_dir_all(&partial_dir).expect("partial team");
    std::fs::write(partial_dir.join("config.json"), b"{\"agents\": [").expect("partial config");
    let malformed = read_team_config_classified(&base, "partial").expect_err("malformed");
    assert_eq!(malformed.class(), "malformed");
    assert!(malformed.to_string().contains("_team_partial"));

    let typed_dir = base.join("_team_wrong-type");
    std::fs::create_dir_all(&typed_dir).expect("typed team");
    std::fs::write(
        typed_dir.join("config.json"),
        br#"{"agents":[],"coordinator":"","repos":[],"contextAlertPercentages":"50"}"#,
    )
    .expect("typed config");
    let malformed = read_team_config_classified(&base, "wrong-type").expect_err("wrong type");
    assert_eq!(malformed.class(), "malformed");

    let invalid_dir = base.join("_team_invalid");
    std::fs::create_dir_all(&invalid_dir).expect("invalid team");
    std::fs::write(
        invalid_dir.join("config.json"),
        br#"{"agents":[],"coordinator":"","repos":[],"contextAlertPercentages":[0]}"#,
    )
    .expect("invalid config");
    let invalid = read_team_config_classified(&base, "invalid").expect_err("invalid product");
    assert_eq!(invalid.class(), "invalid");
    assert!(invalid.to_string().contains("1 through 100"));

    let invalid_name =
        read_team_config_classified(&base, "../escape").expect_err("invalid team name");
    assert_eq!(invalid_name.class(), "invalid");
}

#[test]
fn team_config_writer_round_trips_legacy_and_canonical_alerts() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let legacy_dir = base.join("_team_legacy");
    std::fs::create_dir_all(&legacy_dir).expect("legacy team");
    std::fs::write(
        legacy_dir.join("config.json"),
        br#"{"agents":[],"coordinator":"","repos":[]}"#,
    )
    .expect("legacy config");
    assert!(read_team_config(&base, "legacy")
        .expect("read legacy")
        .context_alert_percentages
        .is_empty());

    write_team_config(&base, "legacy", &empty_team_config(vec![90, 50, 75]))
        .expect("canonical write");
    let read_back = read_team_config(&base, "legacy").expect("read canonical");
    assert_eq!(read_back.context_alert_percentages, vec![50, 75, 90]);
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(legacy_dir.join("config.json")).expect("read bytes"))
            .expect("parse bytes");
    assert_eq!(
        json["contextAlertPercentages"],
        serde_json::json!([50, 75, 90])
    );
}

#[test]
fn invalid_new_team_alerts_have_no_team_directory_side_effects() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let team_dir = base.join("_team_invalid-create");

    let err = create_new_team_config_on_disk(&base, "invalid-create", &empty_team_config(vec![0]))
        .expect_err("invalid create");
    assert!(err.contains("1 through 100"));
    assert!(!team_dir.exists());
    assert!(!team_dir.join("memory").exists());
    assert!(!team_dir.join("conventions.md").exists());
    assert!(base.join(TEAM_CONFIG_MUTATION_LOCK_NAME).is_file());

    TeamConfigMutationGuard::acquire_with_timing(
        &base,
        TeamConfigLockTiming {
            poll_interval: Duration::ZERO,
            timeout: Duration::ZERO,
        },
    )
    .expect("invalid create released lock");
}

#[test]
fn existing_team_publish_failure_keeps_prior_bytes_and_artifacts() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let team_dir =
        create_new_team_config_on_disk(&base, "publish-failure", &empty_team_config(vec![50]))
            .expect("create team");
    let config_path = team_dir.join("config.json");
    let before = std::fs::read(&config_path).expect("prior bytes");
    let marker = team_dir.join("memory").join("marker");
    std::fs::write(&marker, b"keep").expect("marker");
    let guard = TeamConfigMutationGuard::acquire(&base).expect("guard");
    let invalid_publisher_called = AtomicBool::new(false);
    let invalid_err = write_team_config_guarded_with_publisher(
        &base,
        "publish-failure",
        &empty_team_config(vec![0]),
        &guard,
        |_, _| {
            invalid_publisher_called.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect_err("invalid config");
    assert!(invalid_err.contains("1 through 100"));
    assert!(!invalid_publisher_called.load(Ordering::SeqCst));
    assert_eq!(std::fs::read(&config_path).expect("invalid bytes"), before);

    let publisher_called = AtomicBool::new(false);
    let err = write_team_config_guarded_with_publisher(
        &base,
        "publish-failure",
        &empty_team_config(vec![75]),
        &guard,
        |_, _| {
            publisher_called.store(true, Ordering::SeqCst);
            Err("forced publish failure".to_string())
        },
    )
    .expect_err("publish failure");

    assert!(err.contains("forced publish failure"));
    assert!(publisher_called.load(Ordering::SeqCst));
    assert_eq!(std::fs::read(&config_path).expect("current bytes"), before);
    assert_eq!(std::fs::read(&marker).expect("marker bytes"), b"keep");
    assert!(team_dir.join("conventions.md").is_file());
}

#[test]
fn explicit_alert_value_repairs_product_invalid_existing_config() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let team_dir = base.join("_team_repair");
    std::fs::create_dir_all(&team_dir).expect("team");
    std::fs::write(
        team_dir.join("config.json"),
        br#"{"agents":[],"coordinator":"","repos":[],"contextAlertPercentages":[0]}"#,
    )
    .expect("invalid old config");
    assert_eq!(
        read_team_config_classified(&base, "repair")
            .expect_err("old config invalid")
            .class(),
        "invalid"
    );

    let guard = TeamConfigMutationGuard::acquire(&base).expect("guard");
    write_existing_team_config_guarded(&base, "repair", &empty_team_config(vec![75]), &guard)
        .expect("repair write");
    drop(guard);
    assert_eq!(
        read_team_config(&base, "repair")
            .expect("repaired config")
            .context_alert_percentages,
        vec![75]
    );
}

#[test]
fn team_config_guard_serializes_threads_and_releases_on_drop() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let first = TeamConfigMutationGuard::acquire(&base).expect("first guard");
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let thread_barrier = Arc::clone(&barrier);
    let thread_base = base.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        thread_barrier.wait();
        let acquired = TeamConfigMutationGuard::acquire_with_timing(
            &thread_base,
            TeamConfigLockTiming {
                poll_interval: Duration::from_millis(10),
                timeout: Duration::from_secs(2),
            },
        )
        .is_ok();
        tx.send(acquired).expect("send result");
    });

    barrier.wait();
    assert!(
        rx.recv_timeout(Duration::from_millis(150)).is_err(),
        "second writer acquired a held OS lock"
    );
    drop(first);
    assert!(
        rx.recv_timeout(Duration::from_secs(2))
            .expect("waiter result"),
        "second writer did not acquire after release"
    );
    waiter.join().expect("waiter join");
}

#[test]
fn team_config_guard_rejects_wrong_ac_root_and_bad_sentinel() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root_a = tmp.path().join("a");
    let root_b = tmp.path().join("b");
    std::fs::create_dir_all(&root_a).expect("root a");
    std::fs::create_dir_all(&root_b).expect("root b");
    let guard = TeamConfigMutationGuard::acquire(&root_a).expect("guard a");
    let err = write_team_config_guarded(
        &root_b,
        "wrong-root",
        &empty_team_config(Vec::new()),
        &guard,
    )
    .expect_err("wrong root");
    assert!(err.contains("belongs to workspace"));
    assert!(!root_b.join("_team_wrong-root").exists());
    drop(guard);

    let bad_root = tmp.path().join("bad");
    std::fs::create_dir_all(bad_root.join(TEAM_CONFIG_MUTATION_LOCK_NAME))
        .expect("directory sentinel");
    let err = TeamConfigMutationGuard::acquire(&bad_root).expect_err("directory sentinel");
    assert!(err.contains(TEAM_CONFIG_MUTATION_LOCK_NAME));
    assert!(err.contains("regular non-link file"));
}

#[test]
fn held_team_config_lock_times_out_without_writing_and_is_reacquirable() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let team_dir = create_new_team_config_on_disk(&base, "timeout", &empty_team_config(vec![50]))
        .expect("create team");
    let config_path = team_dir.join("config.json");
    let before = std::fs::read(&config_path).expect("before");
    let held = TeamConfigMutationGuard::acquire(&base).expect("held guard");
    let err = TeamConfigMutationGuard::acquire_with_timing(
        &base,
        TeamConfigLockTiming {
            poll_interval: Duration::ZERO,
            timeout: Duration::ZERO,
        },
    )
    .expect_err("held lock timeout");
    assert!(err.contains("Timed out"));
    assert!(err.contains(TEAM_CONFIG_MUTATION_LOCK_NAME));
    assert_eq!(std::fs::read(&config_path).expect("after"), before);
    drop(held);

    TeamConfigMutationGuard::acquire_with_timing(
        &base,
        TeamConfigLockTiming {
            poll_interval: Duration::ZERO,
            timeout: Duration::ZERO,
        },
    )
    .expect("reacquire after timeout holder drops");
}

#[test]
fn delete_and_update_linearize_without_recreating_deleted_team() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    create_new_team_config_on_disk(&base, "race", &empty_team_config(vec![50]))
        .expect("create team");

    let update_guard = TeamConfigMutationGuard::acquire(&base).expect("update guard");
    write_existing_team_config_guarded(&base, "race", &empty_team_config(vec![75]), &update_guard)
        .expect("update first");
    drop(update_guard);
    let delete_guard = TeamConfigMutationGuard::acquire(&base).expect("delete guard");
    std::fs::remove_dir_all(base.join("_team_race")).expect("delete after update");
    drop(delete_guard);
    assert!(!base.join("_team_race").exists());

    create_new_team_config_on_disk(&base, "race", &empty_team_config(vec![50]))
        .expect("recreate for reverse order");
    let delete_guard = TeamConfigMutationGuard::acquire(&base).expect("delete guard");
    std::fs::remove_dir_all(base.join("_team_race")).expect("delete first");
    drop(delete_guard);
    let update_guard = TeamConfigMutationGuard::acquire(&base).expect("update guard");
    let err = write_existing_team_config_guarded(
        &base,
        "race",
        &empty_team_config(vec![90]),
        &update_guard,
    )
    .expect_err("update must not recreate");
    assert!(err.contains("not found"));
    assert!(!base.join("_team_race").exists());
}

#[test]
fn guarded_update_none_and_membership_preserve_alerts_in_both_orders() {
    fn add_member(base: &Path) {
        let guard = TeamConfigMutationGuard::acquire(base).expect("membership guard");
        let mut current = read_team_config(base, "linear").expect("membership read");
        if !current.agents.contains(&"_agent_beta".to_string()) {
            current.agents.push("_agent_beta".to_string());
        }
        write_existing_team_config_guarded(base, "linear", &current, &guard)
            .expect("membership write");
    }

    fn legacy_update(base: &Path) {
        let guard = TeamConfigMutationGuard::acquire(base).expect("update guard");
        let mut current =
            read_team_config_classified(base, "linear").expect("classified current config");
        current.context_alert_percentages = resolve_context_alert_argument(
            ContextAlertWriteMode::Update,
            None,
            Some(&current.context_alert_percentages),
        )
        .expect("preserve omitted alert argument");
        write_existing_team_config_guarded(base, "linear", &current, &guard)
            .expect("legacy update write");
    }

    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    create_test_agent(&base, "alpha", "Alpha");
    create_test_agent(&base, "beta", "Beta");
    let initial = TeamConfigResult {
        agents: vec!["_agent_alpha".to_string()],
        coordinator: "_agent_alpha".to_string(),
        repos: Vec::new(),
        context_alert_percentages: vec![90, 50],
    };
    create_new_team_config_on_disk(&base, "linear", &initial).expect("create team");

    legacy_update(&base);
    add_member(&base);
    let update_then_member = read_team_config(&base, "linear").expect("final config");
    assert_eq!(update_then_member.context_alert_percentages, vec![50, 90]);
    assert!(update_then_member
        .agents
        .contains(&"_agent_beta".to_string()));

    let guard = TeamConfigMutationGuard::acquire(&base).expect("reset guard");
    write_existing_team_config_guarded(&base, "linear", &initial, &guard).expect("reset config");
    drop(guard);
    add_member(&base);
    legacy_update(&base);
    let member_then_update = read_team_config(&base, "linear").expect("reverse final config");
    assert_eq!(member_then_update.context_alert_percentages, vec![50, 90]);
    assert!(member_then_update
        .agents
        .contains(&"_agent_beta".to_string()));
}

#[cfg(windows)]
#[test]
fn team_config_guard_rejects_reparse_sentinel_escape() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join("workspace");
    let outside = tmp.path().join("outside");
    let sentinel = ac_root.join(TEAM_CONFIG_MUTATION_LOCK_NAME);
    std::fs::create_dir_all(&ac_root).expect("workspace");
    std::fs::create_dir_all(&outside).expect("outside");
    let output = std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            sentinel.to_str().expect("sentinel path"),
            outside.to_str().expect("outside path"),
        ])
        .output()
        .expect("run mklink");
    if !output.status.success() {
        println!(
            "skipping team guard reparse sentinel check: stdout: {} stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let err = TeamConfigMutationGuard::acquire(&ac_root).expect_err("reparse escape");
    assert!(err.contains("regular non-link file"));
}

#[cfg(unix)]
#[test]
fn team_config_guard_rejects_symlink_sentinel_escape() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join("workspace");
    let outside = tmp.path().join("outside.lock");
    std::fs::create_dir_all(&ac_root).expect("workspace");
    std::fs::write(&outside, b"").expect("outside lock");
    symlink(&outside, ac_root.join(TEAM_CONFIG_MUTATION_LOCK_NAME)).expect("symlink sentinel");

    let err = TeamConfigMutationGuard::acquire(&ac_root).expect_err("symlink escape");
    assert!(err.contains("regular non-link file"));
    assert_eq!(std::fs::read(&outside).expect("outside bytes"), b"");
}

#[test]
fn issue_3294_team_lock_production_timeout_preserves_bytes() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let team = create_new_team_config_on_disk(&base, "timeout-prod", &empty_team_config(vec![50]))
        .expect("create team");
    let conventions = team.join("conventions.md");
    std::fs::write(&conventions, b"keep conventions\r\n").unwrap();
    let before_config = std::fs::read(team.join("config.json")).unwrap();
    let before_conventions = std::fs::read(&conventions).unwrap();
    let before_inventory = issue_3068_inventory(&base);
    let holder = TeamConfigMutationGuard::acquire(&base).expect("holder");
    let started = std::time::Instant::now();
    let error = TeamConfigMutationGuard::acquire(&base).expect_err("production timeout");
    assert!(started.elapsed() >= Duration::from_millis(5000));
    let canonical = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(&base).unwrap(),
    );
    assert_eq!(
        error,
        format!(
            "Timed out after 5000 ms waiting for team config lock '{}'",
            canonical.join(TEAM_CONFIG_MUTATION_LOCK_NAME).display()
        )
    );
    assert_eq!(
        std::fs::read(team.join("config.json")).unwrap(),
        before_config
    );
    assert_eq!(std::fs::read(&conventions).unwrap(), before_conventions);
    drop(holder);
    assert_eq!(issue_3068_inventory(&base), before_inventory);
    let guard = TeamConfigMutationGuard::acquire(&base).expect("reacquire");
    drop(guard);
    assert_eq!(issue_3068_inventory(&base), before_inventory);
}
