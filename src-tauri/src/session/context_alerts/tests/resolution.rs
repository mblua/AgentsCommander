use super::*;

#[tokio::test]
async fn blocking_identity_resolution_classifies_valid_malformed_and_missing_policy() {
    let fixture = identity_fixture().await;
    let policy = match resolve_member_policy_blocking(&fixture.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected eligible resolution, got {other:?}"),
    };
    assert_eq!(policy.thresholds, vec![50, 75]);
    assert_eq!(policy.fingerprint.project, "project-a");
    assert_eq!(policy.fingerprint.workgroup, "wg-2-dev-team");
    assert_eq!(policy.fingerprint.member, "member");
    assert!(policy.fingerprint.replica_dir.is_absolute());

    let (target, notice) =
        prepare_internal_route_blocking(&policy.fingerprint, 80, &[50, 75]).unwrap();
    assert_eq!(target.fqn(), "project-a:wg-2-dev-team/coordinator");
    assert_eq!(
        notice
            .thresholds()
            .expect("a context-alert notice always carries thresholds"),
        &[50, 75]
    );

    std::fs::write(&fixture.team_config, b"{partial").unwrap();
    match resolve_member_policy_blocking(&fixture.session) {
        MemberPolicyResolution::Paused { class, .. } => assert_eq!(class, "malformed"),
        other => panic!("expected paused malformed resolution, got {other:?}"),
    }

    std::fs::remove_file(&fixture.team_config).unwrap();
    assert!(matches!(
        resolve_member_policy_blocking(&fixture.session),
        MemberPolicyResolution::PermanentIneligible(_)
    ));
}

#[tokio::test]
async fn production_attempt_guard_rechecks_cancellation_purge_registration_policy_and_route() {
    let fixture = identity_fixture().await;
    let policy = match resolve_member_policy_blocking(&fixture.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected eligible resolution, got {other:?}"),
    };
    let (target, _) = prepare_internal_route_blocking(&policy.fingerprint, 80, &[50, 75]).unwrap();
    let services = FakeContextAlertServices::default();
    services
        .registrations
        .lock()
        .unwrap()
        .insert(fixture.session.id);
    let cancellation = CancellationToken::new();

    validate_attempt_guard(
        &services,
        &policy.fingerprint,
        &target,
        &[50, 75],
        &cancellation,
    )
    .unwrap();

    let checks_before_cancel = services.state_checks.load(Ordering::SeqCst);
    let canceled = CancellationToken::new();
    canceled.cancel();
    assert!(validate_attempt_guard(
        &services,
        &policy.fingerprint,
        &target,
        &[50, 75],
        &canceled,
    )
    .unwrap_err()
    .contains("canceled"));

    assert_eq!(
        services.state_checks.load(Ordering::SeqCst),
        checks_before_cancel,
        "cancellation must precede state validation",
    );
    services.blocked.store(true, Ordering::SeqCst);
    assert!(validate_attempt_guard(
        &services,
        &policy.fingerprint,
        &target,
        &[50, 75],
        &cancellation,
    )
    .unwrap_err()
    .contains("purge-room"));
    let mut missing_paths = policy.fingerprint.clone();
    missing_paths.replica_dir = fixture.replica.join("missing");
    assert!(
        validate_attempt_guard(&services, &missing_paths, &target, &[50, 75], &cancellation,)
            .unwrap_err()
            .contains("purge-room")
    );
    services.blocked.store(false, Ordering::SeqCst);

    std::fs::write(
        &fixture.team_config,
        concat!(
            r#"{"agents":["_agent_member","_agent_coordinator"],"#,
            r#""coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[75]}"#
        ),
    )
    .unwrap();
    assert!(validate_attempt_guard(
        &services,
        &policy.fingerprint,
        &target,
        &[50, 75],
        &cancellation,
    )
    .unwrap_err()
    .contains("policy changed"));

    std::fs::write(
        &fixture.team_config,
        concat!(
            r#"{"agents":["_agent_member","_agent_coordinator"],"#,
            r#""coordinator":"_agent_member","repos":[],"contextAlertPercentages":[50,75]}"#
        ),
    )
    .unwrap();
    assert!(validate_attempt_guard(
        &services,
        &policy.fingerprint,
        &target,
        &[50, 75],
        &cancellation,
    )
    .unwrap_err()
    .contains("Orchestrator target changed"));

    std::fs::write(
        &fixture.team_config,
        concat!(
            r#"{"agents":["_agent_coordinator"],"#,
            r#""coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[50,75]}"#
        ),
    )
    .unwrap();
    assert!(validate_attempt_guard(
        &services,
        &policy.fingerprint,
        &target,
        &[50, 75],
        &cancellation,
    )
    .unwrap_err()
    .contains("roster"));

    services.retire_sample_registration(fixture.session.id);
    assert!(validate_attempt_guard(
        &services,
        &policy.fingerprint,
        &target,
        &[50, 75],
        &cancellation,
    )
    .unwrap_err()
    .contains("no longer registered"));
}

#[tokio::test]
async fn identity_resolution_rejects_invalid_roster_matrix_and_cwd_shapes() {
    let roster = identity_fixture().await;
    std::fs::write(
        &roster.team_config,
        concat!(
            r#"{"agents":["_agent_coordinator"],"#,
            r#""coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[50]}"#
        ),
    )
    .unwrap();
    assert!(matches!(
        resolve_member_policy_blocking(&roster.session),
        MemberPolicyResolution::PermanentIneligible(_)
    ));

    let missing_matrix = identity_fixture().await;
    std::fs::remove_dir_all(&missing_matrix.member_matrix).unwrap();
    assert!(matches!(
        resolve_member_policy_blocking(&missing_matrix.session),
        MemberPolicyResolution::PermanentIneligible(_)
    ));

    let ad_hoc = identity_fixture().await;
    let mut ad_hoc_session = ad_hoc.session.clone();
    ad_hoc_session.working_directory = ad_hoc.ac_root.to_string_lossy().to_string();
    assert!(matches!(
        resolve_member_policy_blocking(&ad_hoc_session),
        MemberPolicyResolution::PermanentIneligible(_)
    ));

    let origin = identity_fixture().await;
    let mut origin_session = origin.session.clone();
    origin_session.working_directory = origin.member_matrix.to_string_lossy().to_string();
    assert!(matches!(
        resolve_member_policy_blocking(&origin_session),
        MemberPolicyResolution::PermanentIneligible(_)
    ));

    let lookalike = identity_fixture().await;
    let nested_lookalike = lookalike
        .replica
        .join("repo-one")
        .join("__agent_impostor")
        .join("src");
    std::fs::create_dir_all(&nested_lookalike).unwrap();
    let mut lookalike_session = lookalike.session.clone();
    lookalike_session.working_directory = nested_lookalike.to_string_lossy().to_string();
    assert!(matches!(
        resolve_member_policy_blocking(&lookalike_session),
        MemberPolicyResolution::PermanentIneligible(_)
    ));
}

#[tokio::test]
async fn identity_resolution_distinguishes_unreadable_malformed_invalid_and_missing_config() {
    let unreadable = identity_fixture().await;
    std::fs::remove_file(&unreadable.team_config).unwrap();
    std::fs::create_dir(&unreadable.team_config).unwrap();
    match resolve_member_policy_blocking(&unreadable.session) {
        MemberPolicyResolution::Paused { class, .. } => assert_eq!(class, "unreadable"),
        other => panic!("expected unreadable pause, got {other:?}"),
    }

    let malformed = identity_fixture().await;
    std::fs::write(&malformed.team_config, b"{partial").unwrap();
    match resolve_member_policy_blocking(&malformed.session) {
        MemberPolicyResolution::Paused { class, .. } => assert_eq!(class, "malformed"),
        other => panic!("expected malformed pause, got {other:?}"),
    }

    let invalid = identity_fixture().await;
    std::fs::write(
        &invalid.team_config,
        concat!(
            r#"{"agents":["_agent_member","_agent_coordinator"],"#,
            r#""coordinator":"_agent_coordinator","repos":[],"contextAlertPercentages":[0]}"#
        ),
    )
    .unwrap();
    match resolve_member_policy_blocking(&invalid.session) {
        MemberPolicyResolution::Paused { class, .. } => assert_eq!(class, "invalid"),
        other => panic!("expected invalid pause, got {other:?}"),
    }

    let missing = identity_fixture().await;
    std::fs::remove_file(&missing.team_config).unwrap();
    assert!(matches!(
        resolve_member_policy_blocking(&missing.session),
        MemberPolicyResolution::PermanentIneligible(_)
    ));
}

#[tokio::test]
async fn exact_workgroups_and_same_spelled_filesystem_roots_never_share_target_capability() {
    let first_workgroup = identity_fixture_named("same-project", "wg-1-dev-team").await;
    let second_workgroup = identity_fixture_named("same-project", "wg-2-dev-team").await;
    let first_policy = match resolve_member_policy_blocking(&first_workgroup.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected first policy, got {other:?}"),
    };
    let second_policy = match resolve_member_policy_blocking(&second_workgroup.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected second policy, got {other:?}"),
    };
    let (first_target, _) =
        prepare_internal_route_blocking(&first_policy.fingerprint, 80, &[50]).unwrap();
    let (second_target, _) =
        prepare_internal_route_blocking(&second_policy.fingerprint, 80, &[50]).unwrap();
    assert_eq!(first_target.fqn(), "same-project:wg-1-dev-team/coordinator");
    assert_eq!(
        second_target.fqn(),
        "same-project:wg-2-dev-team/coordinator"
    );
    assert_ne!(first_target, second_target);

    let other_project = identity_fixture_named("other-project", "wg-1-dev-team").await;
    let other_policy = match resolve_member_policy_blocking(&other_project.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected other-project policy, got {other:?}"),
    };
    let (other_target, _) =
        prepare_internal_route_blocking(&other_policy.fingerprint, 80, &[50]).unwrap();
    assert_eq!(
        other_target.fqn(),
        "other-project:wg-1-dev-team/coordinator"
    );
    assert_ne!(first_target, other_target);

    let same_spelling_a = identity_fixture_named("same-project", "wg-3-dev-team").await;
    let same_spelling_b = identity_fixture_named("same-project", "wg-3-dev-team").await;
    let policy_a = match resolve_member_policy_blocking(&same_spelling_a.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected same-spelling policy A, got {other:?}"),
    };
    let policy_b = match resolve_member_policy_blocking(&same_spelling_b.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected same-spelling policy B, got {other:?}"),
    };
    let (target_a, _) = prepare_internal_route_blocking(&policy_a.fingerprint, 80, &[50]).unwrap();
    let (target_b, _) = prepare_internal_route_blocking(&policy_b.fingerprint, 80, &[50]).unwrap();
    assert_eq!(target_a.fqn(), target_b.fqn());
    assert_ne!(target_a.replica_dir(), target_b.replica_dir());
    assert_ne!(target_a, target_b);
}

#[cfg(unix)]
#[tokio::test]
async fn coordinator_route_rejects_symlink_before_repair_capable_resolver() {
    use std::os::unix::fs::symlink;

    let fixture = identity_fixture().await;
    let policy = match resolve_member_policy_blocking(&fixture.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected eligible policy, got {other:?}"),
    };
    let expected = fixture.workgroup.join("__agent_coordinator");
    let real = fixture.workgroup.join("coordinator-real");
    std::fs::rename(&expected, &real).unwrap();
    symlink(&real, &expected).unwrap();

    let error = prepare_internal_route_blocking(&policy.fingerprint, 80, &[50]).unwrap_err();
    assert!(error.contains("real non-link directory"));
}

#[cfg(windows)]
#[tokio::test]
async fn coordinator_route_rejects_reparse_before_repair_capable_resolver() {
    let fixture = identity_fixture().await;
    let policy = match resolve_member_policy_blocking(&fixture.session) {
        MemberPolicyResolution::Eligible(policy) => policy,
        other => panic!("expected eligible policy, got {other:?}"),
    };
    let expected = fixture.workgroup.join("__agent_coordinator");
    let real = fixture.workgroup.join("coordinator-real");
    std::fs::rename(&expected, &real).unwrap();
    let output = std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            expected.to_str().unwrap(),
            real.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    if !output.status.success() {
        println!(
            "skipping coordinator reparse check: stdout: {} stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let error = prepare_internal_route_blocking(&policy.fingerprint, 80, &[50]).unwrap_err();
    assert!(error.contains("real non-link directory"));
    let _ = std::fs::remove_dir(&expected);
}

#[tokio::test]
async fn post_filesystem_snapshot_recheck_rejects_end_removal_and_identity_changes() {
    let fixture = identity_fixture().await;
    let original = fixture.session.clone();
    let agent = original.agent_id.as_deref().unwrap();
    assert!(session_matches_policy_snapshot(
        Some(&original),
        original.id,
        agent,
        &original.working_directory,
        true,
    ));

    let mut normal_transition = original.clone();
    normal_transition.status = SessionStatus::Idle;
    assert!(session_matches_policy_snapshot(
        Some(&normal_transition),
        original.id,
        agent,
        &original.working_directory,
        true,
    ));

    let mut active_transition = original.clone();
    active_transition.status = SessionStatus::Active;
    assert!(session_matches_policy_snapshot(
        Some(&active_transition),
        original.id,
        agent,
        &original.working_directory,
        true,
    ));

    let mut exited = original.clone();
    exited.status = SessionStatus::Exited(0);
    assert!(!session_matches_policy_snapshot(
        Some(&exited),
        original.id,
        agent,
        &original.working_directory,
        true,
    ));
    let mut root = original.clone();
    root.is_root_agent = true;
    assert!(!session_matches_policy_snapshot(
        Some(&root),
        original.id,
        agent,
        &original.working_directory,
        true,
    ));
    let mut agentless = original.clone();
    agentless.agent_id = None;
    assert!(!session_matches_policy_snapshot(
        Some(&agentless),
        original.id,
        agent,
        &original.working_directory,
        true,
    ));
    assert!(!session_matches_policy_snapshot(
        None,
        original.id,
        agent,
        &original.working_directory,
        true,
    ));
    assert!(!session_matches_policy_snapshot(
        Some(&original),
        original.id,
        agent,
        &original.working_directory,
        false,
    ));

    let mut changed_agent = original.clone();
    changed_agent.agent_id = Some("other-agent".to_string());
    assert!(!session_matches_policy_snapshot(
        Some(&changed_agent),
        original.id,
        agent,
        &original.working_directory,
        true,
    ));
    let mut changed_cwd = original.clone();
    changed_cwd.working_directory = fixture.workgroup.to_string_lossy().to_string();
    assert!(!session_matches_policy_snapshot(
        Some(&changed_cwd),
        original.id,
        agent,
        &original.working_directory,
        true,
    ));
}

#[test]
fn project_fqn_component_rejects_empty_colon_control_and_missing_utf8_name() {
    assert_eq!(
        validated_project_fqn_component(Some("project-a")).unwrap(),
        "project-a"
    );
    for invalid in [None, Some(""), Some("project:a"), Some("project\nname")] {
        assert!(validated_project_fqn_component(invalid).is_err());
    }
}

#[test]
fn generation_exhaustion_never_wraps() {
    let mut state = ActorState::new();
    state.next_generation = Some(u64::MAX);
    assert_eq!(state.allocate_generation(), Some(u64::MAX));
    assert_eq!(state.allocate_generation(), None);
}
