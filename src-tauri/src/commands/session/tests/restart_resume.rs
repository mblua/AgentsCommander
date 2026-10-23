use super::*;

// (#1793) Phase 1b: restart auto-resume wake policy and target selection.

fn restart_resume_info(id: &str, status: SessionStatus, waiting_for_input: bool) -> SessionInfo {
    SessionInfo {
        id: id.to_string(),
        name: "s".to_string(),
        shell: "claude".to_string(),
        shell_args: Vec::new(),
        backend_kind: SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: "2026-09-07T00:00:00Z".to_string(),
        working_directory: "/w".to_string(),
        status,
        waiting_for_input,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: None,
        agent_label: None,
        git_repos: Vec::new(),
        workgroup_task: None,
        is_coordinator: false,
        is_root_agent: false,
        token: "t".to_string(),
        agent_kind: None,
        requested_profile: None,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        profile_outdated: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

fn restart_resume_target(session_id: Uuid, name: &str) -> RestartResumeTarget {
    RestartResumeTarget {
        session_id,
        name: name.to_string(),
        working_directory: "/w".to_string(),
        prompt: ".".to_string(),
        collected_at: std::time::Instant::now(),
    }
}

#[test]
fn restart_resume_prompt_for_truth_table() {
    let settings = AppSettings::default();
    let orchestrator = settings.restart_resume_orchestrator_prompt.clone();
    for is_coordinator in [false, true] {
        for start_fresh_on_restore in [false, true] {
            for persisted_working in [false, true] {
                let got = restart_resume_prompt_for(
                    &settings,
                    is_coordinator,
                    start_fresh_on_restore,
                    persisted_working,
                );
                let expected = if start_fresh_on_restore || !persisted_working {
                    None
                } else if is_coordinator {
                    Some(orchestrator.as_str())
                } else {
                    Some(".")
                };
                assert_eq!(
                    got, expected,
                    "is_coordinator={is_coordinator} \
                         start_fresh_on_restore={start_fresh_on_restore} \
                         persisted_working={persisted_working}"
                );
            }
        }
    }
}

#[test]
fn restart_resume_prompt_for_returns_none_for_an_empty_class_prompt() {
    let settings = AppSettings {
        restart_resume_orchestrator_prompt: String::new(),
        ..Default::default()
    };
    assert_eq!(
        restart_resume_prompt_for(&settings, true, false, true),
        None
    );
    assert_eq!(
        restart_resume_prompt_for(&settings, false, false, true),
        Some(".")
    );

    let settings = AppSettings {
        restart_resume_agent_prompt: String::new(),
        ..Default::default()
    };
    assert_eq!(
        restart_resume_prompt_for(&settings, false, false, true),
        None
    );
    assert_eq!(
        restart_resume_prompt_for(&settings, true, false, true),
        Some(settings.restart_resume_orchestrator_prompt.as_str())
    );
}

#[test]
fn restart_resume_prompt_for_keeps_a_whitespace_only_prompt() {
    let settings = AppSettings {
        restart_resume_agent_prompt: " ".to_string(),
        ..Default::default()
    };
    assert_eq!(
        restart_resume_prompt_for(&settings, false, false, true),
        Some(" ")
    );
}

#[test]
fn restart_resume_session_is_ready_truth_table() {
    assert!(!restart_resume_session_is_ready(None));
    for status in [
        SessionStatus::Active,
        SessionStatus::Running,
        SessionStatus::Idle,
        SessionStatus::Exited(0),
    ] {
        let not_waiting = restart_resume_info("id", status.clone(), false);
        assert!(
            !restart_resume_session_is_ready(Some(&not_waiting)),
            "waiting_for_input=false must never be ready, status={status:?}"
        );
    }
    for status in [
        SessionStatus::Active,
        SessionStatus::Running,
        SessionStatus::Idle,
    ] {
        let waiting = restart_resume_info("id", status.clone(), true);
        assert!(
            restart_resume_session_is_ready(Some(&waiting)),
            "status={status:?} must be ready when waiting_for_input"
        );
    }
    let exited = restart_resume_info("id", SessionStatus::Exited(0), true);
    assert!(!restart_resume_session_is_ready(Some(&exited)));
}

#[test]
fn partition_restart_resume_ready_requires_both_conjuncts() {
    let id = Uuid::new_v4();
    let live = vec![restart_resume_info(
        &id.to_string(),
        SessionStatus::Running,
        true,
    )];

    let (remaining, ready) =
        partition_restart_resume_ready(vec![restart_resume_target(id, "a")], &live, |_target| true);
    assert_eq!(ready.len(), 1);
    assert!(remaining.is_empty());

    let (remaining, ready) =
        partition_restart_resume_ready(vec![restart_resume_target(id, "a")], &live, |_target| {
            false
        });
    assert!(
        ready.is_empty(),
        "a session that has printed nothing must not be selected on waiting_for_input alone"
    );
    assert_eq!(remaining.len(), 1);
}

#[test]
fn partition_restart_resume_ready_removes_what_it_selects() {
    let first = Uuid::new_v4();
    let middle = Uuid::new_v4();
    let last = Uuid::new_v4();
    let live = vec![restart_resume_info(
        &middle.to_string(),
        SessionStatus::Running,
        true,
    )];
    let targets = vec![
        restart_resume_target(first, "first"),
        restart_resume_target(middle, "middle"),
        restart_resume_target(last, "last"),
    ];

    let (remaining, ready) = partition_restart_resume_ready(targets, &live, |_target| true);

    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].session_id, middle);
    assert_eq!(
        remaining
            .iter()
            .map(|t| t.session_id)
            .collect::<Vec<Uuid>>(),
        vec![first, last]
    );
    assert_eq!(remaining.len() + ready.len(), 3);
}

#[test]
fn partition_restart_resume_ready_never_offers_the_same_target_twice() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let live = vec![restart_resume_info(
        &first.to_string(),
        SessionStatus::Running,
        true,
    )];
    let targets = vec![
        restart_resume_target(first, "first"),
        restart_resume_target(second, "second"),
    ];

    let (remaining, ready) = partition_restart_resume_ready(targets, &live, |_target| true);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].session_id, first);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].session_id, second);

    let (remaining, ready) = partition_restart_resume_ready(remaining, &live, |_target| true);
    assert!(
        ready.is_empty(),
        "an already-injected target must never be offered a second time"
    );
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].session_id, second);
}
