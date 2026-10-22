use super::*;

#[tokio::test]
async fn issue_1937_selection_api_barrier_unlock_waits_for_paused_apply_publication() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let settings_for_holder = settings.clone();
    let replica_for_holder = replica.clone();
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let holder = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || {
            let settings = settings_for_holder;
            let replica = replica_for_holder;
            async move {
                // Publication phase of an apply, then the held restart span.
                let snapshot = settings.read().await.clone();
                let enumeration = super::enumerate_profile_assignment_targets(
                    &snapshot,
                    &replica,
                    &super::ProfileAssignmentScope::Replica,
                    &[],
                )
                .expect("enumerate");
                let target = &enumeration.candidates[0].target;
                let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
                    configuration_ref: None,
                    profile_override_source: None,
                    identity: target.identity_path.clone(),
                    pair: super::target_pair(target),
                    locked: false,
                };
                crate::config::coding_agent_profiles::write_replica_selection(
                    &snapshot,
                    &replica,
                    &crate::config::coding_agent_profiles::ReplicaSelectionPair {
                        coding_agent_id: "agent-0".to_string(),
                        requested_profile: "B".to_string(),
                    },
                    SelectionWriteIntent::IndividualAssignLock,
                    &expected,
                )
                .expect("publish the lock");
                entered_tx.send(()).expect("signal entered");
                release_rx.await.expect("release the publication span");
                Ok::<(), String>(())
            }
        },
    ));
    entered_rx.await.expect("holder entered");

    let preview =
        api_removal_preview(&settings, &replica, super::ProfileAssignmentScope::Replica).await;
    assert_eq!(preview.protected_count, 1);
    let fingerprint = preview.target_fingerprint.clone();
    let settings_for_unlock = settings.clone();
    let replica_for_unlock = replica.clone();
    let unlock = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || {
            let settings = settings_for_unlock;
            let replica = replica_for_unlock;
            let fingerprint = fingerprint.clone();
            async move {
                super::apply_selection_lock_removal_inner(
                    &settings,
                    api_removal_apply_request(
                        &replica,
                        super::ProfileAssignmentScope::Replica,
                        &fingerprint,
                    ),
                )
                .await
                .map(|(result, _payload)| result)
            }
        },
    ));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !unlock.is_finished(),
        "the unlock must not acknowledge while the earlier apply still owns the turn"
    );

    release_tx.send(()).expect("release holder");
    holder
        .await
        .expect("holder join")
        .expect("holder operation");
    let removed = unlock
        .await
        .expect("unlock join")
        .expect("unlock operation");
    assert_eq!(removed.removed_count, 1);
    assert_eq!(removed.remaining_protected_count, Some(0));
    let saved: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
    assert_eq!(saved["tooling"]["selectionLocked"], json!(false));
    assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
}

#[tokio::test]
async fn issue_1937_selection_api_barrier_read_cannot_observe_half_settled_apply() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let settings_for_holder = settings.clone();
    let replica_for_holder = replica.clone();
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let holder = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || {
            let settings = settings_for_holder;
            let replica = replica_for_holder;
            async move {
                let snapshot = settings.read().await.clone();
                let enumeration = super::enumerate_profile_assignment_targets(
                    &snapshot,
                    &replica,
                    &super::ProfileAssignmentScope::Replica,
                    &[],
                )
                .expect("enumerate");
                let target = &enumeration.candidates[0].target;
                let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
                    configuration_ref: None,
                    profile_override_source: None,
                    identity: target.identity_path.clone(),
                    pair: super::target_pair(target),
                    locked: false,
                };
                crate::config::coding_agent_profiles::write_replica_selection(
                    &snapshot,
                    &replica,
                    &crate::config::coding_agent_profiles::ReplicaSelectionPair {
                        coding_agent_id: "agent-0".to_string(),
                        requested_profile: "B".to_string(),
                    },
                    SelectionWriteIntent::IndividualAssignLock,
                    &expected,
                )
                .expect("publish the lock");
                entered_tx.send(()).expect("signal entered");
                release_rx.await.expect("release the publication span");
                Ok::<(), String>(())
            }
        },
    ));
    entered_rx.await.expect("holder entered");
    let published_bytes = config_bytes(&replica);
    let mut dir_before: Vec<std::ffi::OsString> = std::fs::read_dir(&replica)
        .expect("read replica dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    dir_before.sort();

    let settings_for_read = settings.clone();
    let replica_for_read = replica.clone();
    let read = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || {
            let settings = settings_for_read;
            let replica = replica_for_read;
            async move {
                super::preview_selection_lock_removal_inner(
                    &empty_session_manager(),
                    &settings,
                    api_removal_preview_request(&replica, super::ProfileAssignmentScope::Replica),
                )
                .await
                .map(|preview| preview.protected_count)
            }
        },
    ));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !read.is_finished(),
        "the read must not observe the state while the mutation still owns the turn"
    );

    // Dropping the read waiter leaves the earlier mutation's ownership intact.
    read.abort();
    let _ = read.await;
    let probe = tokio::time::timeout(
        Duration::from_millis(150),
        crate::session::selection::acquire_selection_operation_turn(),
    )
    .await;
    assert!(
        probe.is_err(),
        "the earlier mutation must still own the turn after the read waiter is dropped"
    );

    release_tx.send(()).expect("release holder");
    holder
        .await
        .expect("holder join")
        .expect("holder operation");
    assert_eq!(
        config_bytes(&replica),
        published_bytes,
        "the read wrote nothing"
    );
    let mut dir_after: Vec<std::ffi::OsString> = std::fs::read_dir(&replica)
        .expect("read replica dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    dir_after.sort();
    assert_eq!(dir_after, dir_before, "the read created no files");

    // A fresh read after release observes the settled publication.
    let preview =
        api_removal_preview(&settings, &replica, super::ProfileAssignmentScope::Replica).await;
    assert_eq!(preview.protected_count, 1);
    assert_eq!(
        preview.targets[0].saved_pair,
        Some(super::SavedPair {
            coding_agent_id: Some("agent-0".to_string()),
            requested_profile: Some("B".to_string()),
        })
    );
}
