use super::*;

fn profile_assignment_request(
    scope: super::ProfileAssignmentScope,
    confirmed_target_fingerprint: Option<&str>,
    typed_confirmation: Option<&str>,
) -> super::ApplyCodingAgentProfileSelectionRequest {
    super::ApplyCodingAgentProfileSelectionRequest {
        target_replica_path: "C:/ac/wg-1/__agent_dev-rust".to_string(),
        coding_agent_id: "dev-rust".to_string(),
        profile: "B".to_string(),
        scope,
        restart_sessions: false,
        confirmed_target_fingerprint: confirmed_target_fingerprint.map(|value| value.to_string()),
        typed_confirmation: typed_confirmation.map(|value| value.to_string()),
        assignment_mode: super::AssignmentMode::Ordinary,
        conflict_decision: None,
    }
}

/// #1941 - a synthetic enumeration lets the fingerprint unit tests exercise
/// the typed tuple without a filesystem fixture.
fn fingerprint_enumeration(paths: &[&str]) -> super::ProfileTargetEnumeration {
    super::ProfileTargetEnumeration {
        anchor_dir: PathBuf::from("c:/ac/wg-1/__agent_a"),
        anchor_matrix_dir: PathBuf::from("c:/ac/_agent_a"),
        candidates: paths
            .iter()
            .enumerate()
            .map(|(index, path)| super::ProfileCandidate {
                expected: None,
                target: super::ProfileAssignmentTarget {
                    workgroup_name: "wg-1".to_string(),
                    workgroup_path: "c:/ac/wg-1".to_string(),
                    replica_name: format!("agent_{index}"),
                    replica_path: (*path).to_string(),
                    identity_path: "../../_agent_a".to_string(),
                    origin_project: Some("ac".to_string()),
                    live_session_ids: Vec::new(),
                    saved_pair: None,
                    selection_state: super::SelectionState::Unlocked,
                    selection_error: None,
                },
                matrix_dir: PathBuf::from("c:/ac/_agent_a"),
            })
            .collect(),
        warnings: Vec::new(),
        scope_faults: Vec::new(),
        counts_complete: true,
    }
}

#[test]
fn profile_assignment_fingerprint_uses_typed_fields() {
    let enumeration = fingerprint_enumeration(&["c:/ac/wg-1/__agent_a"]);
    let other = fingerprint_enumeration(&["c:/ac/wg-1/__agent_b"]);
    let eligible = vec!["c:/ac/wg-1/__agent_a".to_string()];

    let base = super::assignment_fingerprint(
        &enumeration,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "ab",
        "C",
        false,
        None,
        &eligible,
    )
    .expect("fingerprint");
    let agent_bc = super::assignment_fingerprint(
        &enumeration,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "a",
        "BC",
        false,
        None,
        &eligible,
    )
    .expect("fingerprint");
    let restart = super::assignment_fingerprint(
        &enumeration,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "ab",
        "C",
        true,
        None,
        &eligible,
    )
    .expect("fingerprint");
    let decision = super::assignment_fingerprint(
        &enumeration,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "ab",
        "C",
        false,
        Some(super::ConflictDecision::ForceReviewed),
        &eligible,
    )
    .expect("fingerprint");
    let other_targets = super::assignment_fingerprint(
        &other,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "ab",
        "C",
        false,
        None,
        &["c:/ac/wg-1/__agent_b".to_string()],
    )
    .expect("fingerprint");

    assert_eq!(base.len(), 64, "sha256 hex");
    assert_eq!(base, base.to_lowercase(), "lowercase hex");
    assert_ne!(base, agent_bc);
    assert_ne!(base, restart);
    assert_ne!(base, decision);
    assert_ne!(base, other_targets);
}

#[test]
fn profile_assignment_confirmation_hint_tracks_broad_scopes() {
    assert!(!super::profile_assignment_requires_explicit_confirmation(
        &super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary
    ));
    assert!(super::profile_assignment_requires_explicit_confirmation(
        &super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock
    ));
    assert!(super::profile_assignment_requires_explicit_confirmation(
        &super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary
    ));
    assert!(super::profile_assignment_requires_explicit_confirmation(
        &super::ProfileAssignmentScope::Kind,
        super::AssignmentMode::Ordinary
    ));
}

#[test]
fn kind_assignment_accepts_fingerprint_without_typed_confirmation() {
    assert_eq!(
        super::validate_profile_assignment_confirmation(
            &super::ProfileAssignmentScope::Kind,
            super::AssignmentMode::Ordinary,
            Some("fp-kind"),
            "fp-kind",
        ),
        Ok(())
    );
}

#[test]
fn kind_assignment_ignores_legacy_typed_confirmation() {
    let request = profile_assignment_request(
        super::ProfileAssignmentScope::Kind,
        Some("fp-kind"),
        Some("not the old exact phrase"),
    );

    assert_eq!(
        super::validate_profile_assignment_confirmation(
            &request.scope,
            request.assignment_mode,
            request.confirmed_target_fingerprint.as_deref(),
            "fp-kind",
        ),
        Ok(())
    );
}

#[test]
fn broad_assignment_rejects_missing_or_stale_fingerprint() {
    for scope in [
        super::ProfileAssignmentScope::Kind,
        super::ProfileAssignmentScope::Workgroup,
    ] {
        let missing = profile_assignment_request(scope.clone(), None, None);
        let err = super::validate_profile_assignment_confirmation(
            &missing.scope,
            missing.assignment_mode,
            missing.confirmed_target_fingerprint.as_deref(),
            "current-fp",
        )
        .unwrap_err();
        // #2051 - a missing fingerprint is a contract failure, not a change.
        assert!(err.contains("confirmationMissing"), "{err}");
        assert!(!err.contains("Target selection changed"), "{err}");

        let stale = profile_assignment_request(scope, Some("old-fp"), None);
        let err = super::validate_profile_assignment_confirmation(
            &stale.scope,
            stale.assignment_mode,
            stale.confirmed_target_fingerprint.as_deref(),
            "current-fp",
        )
        .unwrap_err();
        assert!(err.contains("Target selection changed"), "{err}");
    }
}

#[test]
fn replica_assign_and_lock_confirmation_shapes() {
    let validate = |mode: super::AssignmentMode, provided: Option<&str>| {
        super::validate_profile_assignment_confirmation(
            &super::ProfileAssignmentScope::Replica,
            mode,
            provided,
            "fp-replica",
        )
    };

    // (a) the preview's own fingerprint is accepted...
    assert_eq!(
        validate(super::AssignmentMode::AssignAndLock, Some("fp-replica")),
        Ok(())
    );
    // (b) ...while NO fingerprint at all is a contract failure, never stale.
    let err = validate(super::AssignmentMode::AssignAndLock, None).unwrap_err();
    assert!(err.contains("confirmationMissing"), "{err}");
    assert!(!err.contains("Target selection changed"), "{err}");
    // (c) a provided, non-matching value is stale.
    let err = validate(super::AssignmentMode::AssignAndLock, Some("old-fp")).unwrap_err();
    assert!(err.contains("stalePreview"), "{err}");
    // (d) the legacy replica ordinary path still accepts no fingerprint (D2).
    assert_eq!(validate(super::AssignmentMode::Ordinary, None), Ok(()));
    // (e) even ordinary is stale when a provided value does not match.
    let err = validate(super::AssignmentMode::Ordinary, Some("old-fp")).unwrap_err();
    assert!(err.contains("stalePreview"), "{err}");
}
