use super::*;

#[test]
fn setting_off_always_defers() {
    assert!(!should_wake_on_restore(
        false,
        true,
        Some(&SessionStatus::Running)
    ));
    assert!(!should_wake_on_restore(false, false, None));
}

#[test]
fn non_coord_always_defers_when_on() {
    assert!(!should_wake_on_restore(
        true,
        false,
        Some(&SessionStatus::Running)
    ));
}

#[test]
fn coord_awake_at_shutdown_wakes_when_on() {
    assert!(should_wake_on_restore(
        true,
        true,
        Some(&SessionStatus::Running)
    ));
    assert!(should_wake_on_restore(
        true,
        true,
        Some(&SessionStatus::Idle)
    ));
    assert!(should_wake_on_restore(
        true,
        true,
        Some(&SessionStatus::Active)
    ));
}

#[test]
fn coord_asleep_at_shutdown_defers_when_on() {
    assert!(!should_wake_on_restore(
        true,
        true,
        Some(&SessionStatus::Exited(0))
    ));
    assert!(!should_wake_on_restore(
        true,
        true,
        Some(&SessionStatus::Exited(137))
    ));
}

#[test]
fn coord_unknown_status_fails_open_when_on() {
    assert!(should_wake_on_restore(true, true, None));
}

#[test]
fn archived_project_session_is_forced_dormant_on_restore_decision() {
    assert!(!restore_session_should_wake(
        true,
        true,
        false,
        true,
        Some(&SessionStatus::Running),
        false
    ));
    assert!(restore_session_should_wake(
        false,
        true,
        false,
        true,
        Some(&SessionStatus::Running),
        false
    ));
}

#[test]
fn should_wake_working_agent_on_restore_truth_table() {
    for resume_agents_on in [false, true] {
        for is_coord in [false, true] {
            for persisted_working in [false, true] {
                let got = should_wake_working_agent_on_restore(
                    resume_agents_on,
                    is_coord,
                    persisted_working,
                );
                let expected = resume_agents_on && !is_coord && persisted_working;
                assert_eq!(
                    got, expected,
                    "resume_agents_on={resume_agents_on} is_coord={is_coord} \
                        persisted_working={persisted_working}"
                );
            }
        }
    }
}

#[test]
fn restore_session_should_wake_wakes_a_working_replica_only_when_opted_in() {
    assert!(!restore_session_should_wake(
        false,
        true,
        false,
        false,
        Some(&SessionStatus::Running),
        true
    ));
    assert!(restore_session_should_wake(
        false,
        true,
        true,
        false,
        Some(&SessionStatus::Running),
        true
    ));
    // The two arms are independent: the replica arm fires with the
    // coordinator setting OFF.
    assert!(restore_session_should_wake(
        false,
        false,
        true,
        false,
        Some(&SessionStatus::Running),
        true
    ));
}

#[test]
fn restore_session_should_wake_never_wakes_an_idle_replica() {
    for status in [
        Some(SessionStatus::Idle),
        Some(SessionStatus::Running),
        None,
    ] {
        assert!(
            !restore_session_should_wake(false, true, true, false, status.as_ref(), false),
            "an idle replica must never wake, status={status:?}"
        );
    }
}

#[test]
fn restore_session_should_wake_arms_are_disjoint_on_is_coord() {
    // Coordinator: wakes through the #248 arm, and the replica arm is false
    // for it.
    assert!(restore_session_should_wake(
        false,
        true,
        true,
        true,
        Some(&SessionStatus::Running),
        true
    ));
    assert!(!should_wake_working_agent_on_restore(true, true, true));

    // Non-coordinator: wakes through the new arm, and the #248 arm is false
    // for it.
    assert!(restore_session_should_wake(
        false,
        true,
        true,
        false,
        Some(&SessionStatus::Running),
        true
    ));
    assert!(!should_wake_on_restore(
        true,
        false,
        Some(&SessionStatus::Running)
    ));

    // Archived beats both arms.
    assert!(!restore_session_should_wake(
        true,
        true,
        true,
        true,
        Some(&SessionStatus::Running),
        true
    ));
    assert!(!restore_session_should_wake(
        true,
        true,
        true,
        false,
        Some(&SessionStatus::Running),
        true
    ));
}

#[test]
fn archived_project_session_is_never_adopted_as_active_on_restore() {
    assert!(!restore_session_should_become_active(true, true));
    assert!(restore_session_should_become_active(true, false));
    assert!(!restore_session_should_become_active(false, false));
}

#[test]
fn root_agent_live_or_legacy_status_wakes() {
    assert!(should_wake_root_agent_on_restore(Some(
        &SessionStatus::Running
    )));
    assert!(should_wake_root_agent_on_restore(Some(
        &SessionStatus::Idle
    )));
    assert!(should_wake_root_agent_on_restore(Some(
        &SessionStatus::Active
    )));
    assert!(should_wake_root_agent_on_restore(None));
}

#[test]
fn root_agent_exited_status_stays_dormant() {
    assert!(!should_wake_root_agent_on_restore(Some(
        &SessionStatus::Exited(0)
    )));
    assert!(!should_wake_root_agent_on_restore(Some(
        &SessionStatus::Exited(137)
    )));
}

#[test]
fn first_restore_does_not_auto_create_root_agent_without_agents() {
    let settings = AppSettings::default();

    assert!(!should_auto_create_root_agent_on_first_restore(
        &settings, None
    ));
}

// (#630) Backstop truth table: a real coordinator survives a transient empty
// discover_teams(); a healthy non-empty discovery is trusted as-is.
#[test]
fn resolve_is_coord_for_restore_truth_table() {
    // Empty discovery + persisted coord => backstop wakes the real coord.
    assert!(resolve_is_coord_for_restore(false, true, true));
    // Healthy discovery (non-empty) that no longer lists this agent => trust it.
    assert!(!resolve_is_coord_for_restore(false, false, true));
    // Live discovery already says coord => trust it regardless of the rest.
    assert!(resolve_is_coord_for_restore(true, false, false));
    assert!(resolve_is_coord_for_restore(true, true, false));
    // Empty discovery + not a persisted coord => stays deferred.
    assert!(!resolve_is_coord_for_restore(false, true, false));
}

// (#630/#631) The wake path passes the persisted fresh intent straight through
// as create_session_inner's skip_auto_resume. Guards the lib.rs read seam: if
// this identity is ever broken, restore stops honoring "Restart Session".
#[test]
fn wake_path_passes_persisted_fresh_intent() {
    assert!(skip_auto_resume_for_restore(true)); // restarted fresh => suppress --continue
    assert!(!skip_auto_resume_for_restore(false)); // default => resume
}

#[test]
fn first_restore_auto_creates_root_agent_with_configured_agent() {
    let settings = settings_with_agent();

    assert!(should_auto_create_root_agent_on_first_restore(
        &settings, None
    ));
}

#[test]
fn first_restore_auto_creates_root_agent_with_valid_last_coding_agent() {
    let settings = settings_with_agent();

    assert!(should_auto_create_root_agent_on_first_restore(
        &settings,
        Some("codex")
    ));
}
