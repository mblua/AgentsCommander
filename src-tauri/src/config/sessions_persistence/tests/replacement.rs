use super::*;

#[tokio::test]
async fn replacement_preparing_requires_current_confirmed_exited_key_and_exact_owner() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("member");
    let directory = temp.path().join("config");
    std::fs::create_dir(&cwd).unwrap();
    let physical_cwd = std::fs::canonicalize(&cwd).unwrap();
    let physical_replica = crate::path_identity::verify_directory(&cwd).unwrap();
    let other_replica = temp.path().join("other-member");
    std::fs::create_dir(&other_replica).unwrap();
    let mut replaced_object = crate::path_identity::verify_directory(&other_replica).unwrap();
    // Fault control: retain the original name with another actual file id.
    // This models directory replacement without requiring a symlink.
    replaced_object.canonical_path = physical_replica.canonical_path.clone();
    let manager = SessionManager::new();
    let row = manager
        .create_session(
            "bash".into(),
            vec![],
            cwd.to_string_lossy().into_owned(),
            None,
            None,
            vec![],
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    manager.set_start_fresh_on_restore(row.id, true).await;
    let permit = acquire_carrier_mutation_permit().await;
    let version = manager
        .with_current_carrier_snapshot(&permit, |current| current.versions[&row.id])
        .await;
    drop(permit);
    let target = format!("replacement:{}", uuid::Uuid::new_v4());
    let restore_exclusion = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let initial_intent = super::super::InitialCarrierIntent::new(
        target.clone(),
        row.id,
        row.token,
        version,
        physical_cwd.clone(),
    )
    .with_restore_exclusion(restore_exclusion.clone());
    assert!(
        super::super::persist_initial_carrier(
            &manager,
            directory.clone(),
            vec![temp.path().to_string_lossy().into_owned()],
            initial_intent.clone(),
        )
        .await
        .unwrap()
        .is_none(),
        "active restore cannot mint a protection"
    );
    assert!(
        !directory.join("sessions.json").exists(),
        "excluded initial publication has no rename"
    );
    restore_exclusion.store(false, std::sync::atomic::Ordering::Release);
    let initial = super::super::persist_initial_carrier(
        &manager,
        directory.clone(),
        vec![temp.path().to_string_lossy().into_owned()],
        initial_intent,
    )
    .await
    .unwrap()
    .expect("initial Confirmed key");
    let permit = acquire_carrier_mutation_permit().await;
    assert_eq!(
        super::super::restore_carrier_scopes_are_empty(&permit),
        Some(false)
    );
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            assert!(
                super::super::retained_carrier_for_replacement(
                    &permit, &directory, &target, row.id, current
                )
                .is_none(),
                "live row cannot become retained replacement A"
            );
            let manual = super::super::manual_discard_carrier(
                &permit,
                &physical_cwd,
                &target,
                row.id,
                current,
            )
            .expect("live strict ManualDiscard key");
            assert_eq!(manual.nonce, initial.nonce);
            assert_eq!(manual.retired_local_generation(), None);
            let mut wrong_nonce = manual.clone();
            wrong_nonce.nonce = uuid::Uuid::new_v4();
            assert!(
                !super::super::retire_manual_discard_carrier(
                    &permit,
                    &wrong_nonce,
                    &physical_cwd,
                    current
                ),
                "manual discard cannot retire another nonce"
            );
            assert!(
                super::super::manual_discard_carrier(
                    &permit,
                    &physical_cwd,
                    &target,
                    row.id,
                    current
                )
                .is_some(),
                "failed discard leaves protection intact"
            );
            assert!(super::super::with_confirmed_carrier(
                &permit,
                &initial,
                current,
                super::super::CarrierCheckpointTransition::BeginClosing,
                |_| Some(())
            )
            .is_some());
        })
        .await;
    drop(permit);
    manager.mark_exited(row.id, 0).await;
    let permit = acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            assert_eq!(current.selection.id(), Some(row.id));
            let physical_domain = physical_replica.clone().into();
            assert!(
                super::super::with_confirmed_carrier(
                    &permit,
                    &initial,
                    current,
                    super::super::CarrierCheckpointTransition::FinishDormant {
                        generation: 7,
                        committed_version: initial.version,
                        replica: &physical_domain,
                    },
                    |_| Some(())
                )
                .is_none(),
                "selected Exited carrier cannot finish Dormant"
            );
        })
        .await;
    drop(permit);
    manager.clear_active().await;
    assert_eq!(manager.selection_payload().await.id(), None);
    manager.set_start_fresh_on_restore(row.id, false).await;
    persist_current_state_to_dir_result(&manager, &directory)
        .await
        .unwrap();
    let owner = uuid::Uuid::new_v4();
    let binding = manager.reserve_carrier_create_binding();
    let permit = acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            assert!(
                super::super::retained_carrier_for_replacement(
                    &permit, &directory, &target, row.id, current
                )
                .is_none(),
                "Exited plus Confirmed without Dormant settlement is not retirement evidence"
            );
            assert!(
                super::super::manual_discard_carrier(
                    &permit,
                    &physical_cwd,
                    &target,
                    row.id,
                    current
                )
                .is_none(),
                "Closing cannot be manually adopted"
            );
            let physical_domain = physical_replica.clone().into();
            let transition = super::super::CarrierCheckpointTransition::FinishDormant {
                generation: 7,
                committed_version: initial.version,
                replica: &physical_domain,
            };
            assert!(
                super::super::with_confirmed_carrier(
                    &permit,
                    &initial,
                    current,
                    transition,
                    |_| None::<()>
                )
                .is_none(),
                "failed settlement mints nothing"
            );
            assert!(super::super::retained_carrier_for_replacement(
                &permit, &directory, &target, row.id, current
            )
            .is_none());
            assert!(super::super::with_confirmed_carrier(
                &permit,
                &initial,
                current,
                transition,
                |_| Some(())
            )
            .is_some());
        })
        .await;
    drop(permit);
    manager.set_start_fresh_on_restore(row.id, true).await;
    persist_current_state_to_dir_result(&manager, &directory)
        .await
        .unwrap();
    let permit = acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            let carrier = super::super::retained_carrier_for_replacement(
                &permit, &directory, &target, row.id, current,
            )
            .expect("fresh Confirmed Exited A");
            assert_retired_replacement_current_facts(
                &permit,
                current,
                &carrier,
                &initial,
                &directory,
                &target,
                row.id,
                &physical_cwd,
                &physical_replica,
                &replaced_object,
                owner,
                binding,
            );
            assert_retired_replacement_epoch_exhaustion(
                &permit, current, &carrier, &directory, &target, row.id, owner, binding,
            );
            assert_retired_replacement_preparing_owner(
                &permit, current, &carrier, owner, binding, &target, &cwd, &row, &directory,
            );
        })
        .await;
    drop(permit);
}

#[tokio::test]
async fn native_publication_readback_fault_retains_pending_then_confirms_same_epoch() {
    for malformed in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new();
        let permit = acquire_carrier_mutation_permit().await;
        let slot = super::super::publication_slot(temp.path()).unwrap();
        let expected_epoch = slot.lock().unwrap().epoch;
        let nonce = uuid::Uuid::new_v4();
        let epoch = expected_epoch.checked_add(1).unwrap();
        let rows = vec![PersistedSession {
            name: "published-row".into(),
            working_directory: temp.path().to_string_lossy().into_owned(),
            ..Default::default()
        }];
        let versions = manager
            .with_current_carrier_snapshot(&permit, |current| current.versions.clone())
            .await;
        let mut pending = slot.lock().unwrap().clone();
        pending.epoch = epoch;
        pending.checkpoint = Some(super::super::PublicationCheckpoint {
            nonce,
            epoch,
            rows: rows.clone(),
            versions,
            confirmed: false,
        });
        let mut next = Some(pending);
        let write = super::super::PreparedSessionsWrite::prepare(temp.path(), &rows).unwrap();
        let renamed = manager
            .with_current_carrier_snapshot(&permit, |_| {
                super::super::rename_and_publish(
                    temp.path(),
                    &slot,
                    expected_epoch,
                    &mut next,
                    &write,
                )
            })
            .await;
        let super::super::RenamePublicationAttempt::Renamed { leaf, retired } = renamed else {
            panic!("real native rename must publish Pending");
        };
        assert!(next.is_none());
        assert!(!write.tmp_path.exists());
        let published_bytes = std::fs::read(&write.path).unwrap();
        // Controlled post-rename readback faults, after M/suppression were
        // released, while this operation still owns U and the same leaf.
        if malformed {
            std::fs::write(&write.path, b"invalid JSON").unwrap();
        } else {
            let mut different = rows.clone();
            different[0].name = "another projection".into();
            std::fs::write(&write.path, serde_json::to_vec(&different).unwrap()).unwrap();
        }
        super::super::confirm_publication_readback(&slot, &write.path, nonce, epoch);
        {
            let state = slot.lock().unwrap();
            assert_eq!(state.epoch, epoch);
            let checkpoint = state.checkpoint.as_ref().unwrap();
            assert_eq!(checkpoint.nonce, nonce);
            assert!(
                !checkpoint.confirmed,
                "unreadable or mismatched bytes cannot mint proof"
            );
        }
        // Restore readable native bytes, then reconcile only readback.
        // No second PreparedSessionsWrite or sessions rename is invoked.
        std::fs::write(&write.path, published_bytes).unwrap();
        super::super::confirm_publication_readback(&slot, &write.path, nonce, epoch);
        {
            let state = slot.lock().unwrap();
            assert_eq!(state.epoch, epoch);
            let checkpoint = state.checkpoint.as_ref().unwrap();
            assert_eq!(checkpoint.nonce, nonce);
            assert!(checkpoint.confirmed);
        }
        drop(leaf);
        drop(retired);
        drop(permit);
    }
}

#[tokio::test]
async fn native_rename_failure_keeps_epoch_checkpoint_and_own_pending_payload() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("sessions.json")).unwrap();
    let manager = SessionManager::new();
    let permit = acquire_carrier_mutation_permit().await;
    let slot = super::super::publication_slot(temp.path()).unwrap();
    let before = slot.lock().unwrap().clone();
    let epoch = before.epoch.checked_add(1).unwrap();
    let nonce = uuid::Uuid::new_v4();
    let mut pending = before.clone();
    pending.epoch = epoch;
    pending.checkpoint = Some(super::super::PublicationCheckpoint {
        nonce,
        epoch,
        rows: vec![],
        versions: Default::default(),
        confirmed: false,
    });
    let mut next = Some(pending);
    let write = super::super::PreparedSessionsWrite::prepare(temp.path(), &[]).unwrap();
    let attempt = manager
        .with_current_carrier_snapshot(&permit, |_| {
            super::super::rename_and_publish(temp.path(), &slot, before.epoch, &mut next, &write)
        })
        .await;
    let super::super::SettledRenameAttempt::Failed(error) =
        super::super::settle_rename_attempt(attempt, &slot, &write.path, nonce, epoch, 0, None)
    else {
        panic!("file cannot replace an existing directory");
    };
    assert!(write
        .terminal_error(&error)
        .starts_with("Failed to rename sessions file:"));
    assert!(
        write.tmp_path.is_file(),
        "failed rename still owns its original temp"
    );
    {
        let state = slot.lock().unwrap();
        assert_eq!(state.epoch, before.epoch);
        assert_eq!(
            state.checkpoint.as_ref().map(|checkpoint| checkpoint.nonce),
            before
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.nonce)
        );
        assert_eq!(
            next.as_ref().unwrap().checkpoint.as_ref().unwrap().nonce,
            nonce
        );
    }
    let owned_temp = write.tmp_path.clone();
    drop(write);
    assert!(!owned_temp.exists());
    assert!(
        temp.path().join("sessions.json").is_dir(),
        "cleanup never removes destination"
    );
    drop(permit);
}
