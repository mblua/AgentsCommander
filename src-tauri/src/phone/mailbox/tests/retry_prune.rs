use super::*;

/// (#1399 T9 / R1) A panicking worker cannot strand its claim: the
/// WakeOutcomeReport guard reports a transient error from the unwind, the
/// drain releases ownership, and the attempt is counted. Without the guard
/// the claim stays owned forever and reclamation skips it on every cycle.
#[tokio::test]
async fn panicking_worker_reports_through_the_guard_and_frees_the_ledger() {
    for panic in [true, false] {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let session_id = add_mailbox_session_with_shell(
            &app,
            &fixture.target_cwd,
            "wake-panic",
            "codex",
            SessionStatus::Idle,
        )
        .await;
        let hooks = MailboxTestHooks::default();
        hooks.pty_presence.lock().unwrap().insert(session_id, true);
        if panic {
            hooks.panic_on_inject.lock().unwrap().insert(session_id);
        }
        hooks
            .inject_results
            .lock()
            .unwrap()
            .push_back(Err("transient twin failure".into()));

        use crate::pty::manager::{
            ResponseClosePublisher, ResponseCloseState, ResponseDeliveryProof,
            ResponseRouteIdentity, ReturnDeliveryProof,
        };
        let tracker = ResponseCloseState::default();
        assert!(app.manage(tracker.clone()));
        let (coordinator_id, coordinator_token) =
            seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running).await;
        let manager = {
            let state = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = state.read().await;
            guard.clone()
        };
        let target_row = manager.get_session(session_id).await.unwrap();
        let member = ResponseRouteIdentity {
            target: CANONICAL_WAKE_TO.into(),
            session_id,
            token: target_row.token,
            generation: 1,
            carrier_version: 1,
            backend: crate::pty::backend::SessionBackendKind::LocalProcess,
        };
        let coordinator = ResponseRouteIdentity {
            target: CANONICAL_WAKE_FROM.into(),
            session_id: coordinator_id,
            token: coordinator_token,
            generation: 2,
            ..member.clone()
        };

        let now = std::time::Instant::now() + Duration::from_secs(3600);
        for publisher in [
            ResponseClosePublisher::Manager,
            ResponseClosePublisher::Pty,
            ResponseClosePublisher::RawOutput,
            ResponseClosePublisher::Detector,
            ResponseClosePublisher::Menu,
            ResponseClosePublisher::Hold,
        ] {
            tracker.bind_publisher(publisher);
        }
        tracker.mark_work_bootstrapped();
        tracker.try_enable_readiness().unwrap();
        tracker.apply_settings(true, 30, now);
        for (identity, eligible) in [(&member, true), (&coordinator, false)] {
            tracker.register_route(identity.session_id, identity.generation, now);
            tracker.publish_manager_row(
                identity.session_id,
                identity.token,
                identity.generation,
                identity.carrier_version,
                eligible,
                false,
                now,
            );
            tracker.publish_manager_facts(identity.clone(), eligible, false, now);
        }
        tracker.observe_detector(member.session_id, false, now);
        tracker.publish_menu(member.session_id, Some(false), now);
        tracker.publish_hold(member.session_id, None, true, now);

        {
            let settings = app.state::<SettingsState>();
            settings
                .write()
                .await
                .project_paths
                .push(fixture.sender_cwd.to_string_lossy().to_string());
        }
        let source = write_wake_message(
            &fixture.sender_cwd,
            "panicking-wake",
            "2026-07-15T00:00:01+00:00",
        );
        let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());

        let content = std::fs::read_to_string(&source).unwrap();
        let mut slot = None;
        poller
            .process_message_content(
                &app,
                &source,
                OutboxOrigin::Replica,
                &content,
                Some(&mut slot),
            )
            .await
            .unwrap();
        let validated = slot.unwrap();
        let original = validated.response.unwrap();
        assert!(original.state.same_state(&tracker));
        assert_eq!(original.target, member.target);
        assert!(original.delivery_context.is_none());
        assert!(tracker
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                original.key.clone(),
                original.digest.clone(),
                now
            )
            .is_ok());
        poller.poll(&app).await.unwrap();
        poller.settle_wake_workers().await;

        assert!(poller.live_claims.is_empty());
        assert_eq!(
            poller
                .retry_tracker
                .get(&source)
                .map(|state| state.attempt_count),
            Some(1)
        );
        assert!(!wake_claim_path(&source).exists());
        assert!(source.exists());
        // Phase 1: preservation before synthetic discharge. Exact pending proves
        // that the scenario target has a work entry; no counters are inferred here.
        let retained = poller
            .response_sources
            .lock()
            .unwrap()
            .get(&source)
            .unwrap()
            .clone();
        assert!(retained.state.same_state(&tracker));
        assert_eq!(
            (&retained.target, &retained.key, &retained.digest),
            (&original.target, &original.key, &original.digest)
        );
        assert_eq!(member.target, retained.target);
        assert!(retained.delivery_context.is_none());
        assert!(retained.return_context.is_none());
        assert!(!tracker.target_is_closing(&member.target));
        assert_eq!(
            tracker.generation(member.session_id),
            Some(member.generation)
        );
        assert!(tracker
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                original.key.clone(),
                original.digest.clone(),
                now
            )
            .is_ok());
        let snapshot_path = if source.exists() {
            source.clone()
        } else {
            wake_claim_path(&source)
        };
        let preservation_snapshot = (
            retained.target.clone(),
            retained.key.clone(),
            retained.digest.clone(),
            std::fs::read(snapshot_path).unwrap(),
        );
        assert_eq!(preservation_snapshot.3, content.as_bytes());
        assert_eq!(poller.wake_workers.len(), 0);
        assert_eq!(poller.wake_lanes.len(), 0);
        assert!(hooks.spawn_calls.lock().unwrap().is_empty());
        assert!(hooks.destroy_calls.lock().unwrap().is_empty());
        assert!(matches!(
            poller.wake_outcomes_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        let scenario_target = original.target.clone();
        let scenario_key = original.key.clone();

        // Phase 2: synthetic TEST-LOCAL discharge ONLY. This does not prove FS/SQL
        // terminalization, peer submission, or P05 proof. Phase 1 alone proves retention.
        assert_eq!(member.target, scenario_target);
        assert!(!tracker.target_is_closing(&scenario_target));
        tracker.settle_terminal(&scenario_target, &scenario_key, std::time::Instant::now());
        // A leaked dispatch/provisional counter is not changed by settle_terminal.
        // Each fresh probe uses the SAME tracker, target, generation and facts.
        for sensitivity in 0..5 {
            let dispatch = if sensitivity == 1 {
                Some(tracker.begin_delivery(&scenario_target).unwrap())
            } else {
                None
            };
            let provisional = if sensitivity == 3 {
                Some(
                    tracker
                        .reserve_work(
                            &scenario_target,
                            &format!("diagnostic-held-{}", Uuid::new_v4()),
                        )
                        .unwrap(),
                )
            } else {
                None
            };
            let probe_key = format!("diagnostic-request-{}", Uuid::new_v4());
            let probe_digest = "diagnostic-body";
            let mut admission = tracker.reserve_work(&scenario_target, &probe_key).unwrap();
            assert_eq!(admission.accept_durable(probe_digest), Ok(true));
            drop(admission);
            let probe_at = std::time::Instant::now() + Duration::from_secs(7200 + sensitivity * 60);
            let context = tracker
                .reserve_policy_candidate(
                    member.clone(),
                    coordinator.clone(),
                    probe_key.clone(),
                    probe_digest.into(),
                    probe_at,
                )
                .unwrap();
            // Unit-returning API; successful register below validates submitted=true.
            tracker.mark_request_submitted(&context, true);
            let registered = tracker.register_request(
                ResponseDeliveryProof {
                    context,
                    member: member.clone(),
                    coordinator: coordinator.clone(),
                    published_carrier_version: member.carrier_version,
                },
                probe_at,
            );
            if sensitivity == 3 {
                assert!(
                    registered.is_err(),
                    "retained real provisional admission must be detected"
                );
                tracker.settle_terminal(&scenario_target, &probe_key, probe_at);
                drop(provisional);
                drop(dispatch);
                continue;
            }
            registered.unwrap();
            let context = tracker
                .admit_return(
                    &member,
                    &coordinator,
                    format!("diagnostic-return-{}", Uuid::new_v4()),
                    "diagnostic-return-body".into(),
                    probe_at,
                )
                .unwrap();
            tracker
                .observe_return_success(ReturnDeliveryProof { context }, probe_at)
                .unwrap();
            let ticket =
                tracker.select_ticket(member.session_id, probe_at + Duration::from_secs(30));
            if sensitivity == 1 {
                assert!(
                    ticket.is_err(),
                    "retained real SAME-target dispatch must be detected"
                );
            } else {
                assert!(
                    ticket.is_ok(),
                    "same-object final success proves dispatch/provisional/writer counters zero"
                );
            }
            drop(dispatch);
            drop(provisional);
        }
        // Synthetic tracker discharge leaves phase1 association/disk intact.
        let retained_after = poller
            .response_sources
            .lock()
            .unwrap()
            .get(&source)
            .unwrap()
            .clone();
        assert!(retained_after.state.same_state(&tracker));
        assert_eq!(
            (
                retained_after.target,
                retained_after.key,
                retained_after.digest
            ),
            (
                preservation_snapshot.0,
                preservation_snapshot.1,
                preservation_snapshot.2
            )
        );
        assert!(retained_after.delivery_context.is_none());
        assert!(retained_after.return_context.is_none());
        let path_after = if source.exists() {
            source.clone()
        } else {
            wake_claim_path(&source)
        };
        assert_eq!(std::fs::read(path_after).unwrap(), preservation_snapshot.3);
    }
}

/// (#1399 T12) A real Windows exclusive directory handle makes production
/// verification fail for both raw spellings; the cycle drops both aliases
/// before any scan or reclamation, and the next cycle recovers exactly one
/// canonical survivor whose live claim is untouched.
#[cfg(windows)]
#[test]
fn outbox_verification_failure_drops_aliases_before_reclamation() {
    use std::os::windows::fs::OpenOptionsExt;

    let temp = tempfile::TempDir::new().unwrap();
    let outbox = temp.path().join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();

    // Verify before locking, derive the canonical spelling, seed a live
    // claim under the canonical key, and move the physical origin to its
    // ordinary claim.
    let identity = crate::path_identity::verify_directory(&outbox).expect("verify outbox");
    let canonical_outbox =
        crate::path_utils::normalize_windows_verbatim_path_buf(&identity.canonical_path);
    let ordinary_origin = outbox.join("m.json");
    let ordinary_claim = wake_claim_path(&ordinary_origin);
    std::fs::write(&ordinary_origin, "{}").unwrap();
    std::fs::rename(&ordinary_origin, &ordinary_claim).unwrap();
    let canonical_origin = canonical_outbox.join("m.json");
    let canonical_live_claim = wake_claim_path(&canonical_origin);
    let mut poller = MailboxPoller::new();
    assert!(poller.live_claims.insert(canonical_live_claim.clone()));

    // The ordinary spelling and a case-altered alias differ as raw values
    // and verify to the same object before locking. NTFS is case-insensitive
    // by default, so the uppercased final component resolves to the same
    // physical directory on every standard Windows host, while the raw
    // `OsStr` is guaranteed distinct from the canonical spelling.
    let escaped_alias =
        outbox.with_file_name(outbox.file_name().unwrap().to_string_lossy().to_uppercase());
    assert_ne!(escaped_alias.as_os_str(), canonical_outbox.as_os_str());
    assert_eq!(
        crate::path_identity::verify_directory(&escaped_alias)
            .expect("alias verifies to the same object")
            .object_id,
        identity.object_id
    );
    let mut current_cycle_candidates = vec![escaped_alias.clone(), canonical_outbox.clone()];

    // Hold a real exclusive directory handle through the ordinary spelling:
    // share_mode 0 plus FILE_FLAG_BACKUP_SEMANTICS (0x0200_0000).
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .custom_flags(0x0200_0000)
        .open(&outbox)
        .expect("exclusive directory handle");

    // While the handle is alive both spellings still pass `is_dir()`, both
    // verifier calls fail, and production dedup drops every alias.
    assert!(outbox.is_dir());
    assert!(canonical_outbox.is_dir());
    assert!(crate::path_identity::verify_directory(&outbox).is_err());
    assert!(crate::path_identity::verify_directory(&canonical_outbox).is_err());
    dedup_outbox_dirs_by_object_id(&mut current_cycle_candidates);
    assert!(current_cycle_candidates.is_empty());

    // The emptied vector means zero scan and zero reclamation work.
    let mut visits = 0usize;
    let mut scans = 0usize;
    let mut reclamations = 0usize;
    for outbox_dir in &current_cycle_candidates {
        visits += 1;
        let Ok(rd) = std::fs::read_dir(outbox_dir) else {
            continue;
        };
        scans += 1;
        let claims: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("in-flight"))
            .collect();
        if crate::path_identity::verify_directory(outbox_dir).is_ok() {
            poller.reclaim_unowned_wake_claims(outbox_dir, &claims);
            reclamations += 1;
        }
    }
    assert_eq!((visits, scans, reclamations), (0, 0, 0));

    // Durable state is untouched while the handle is held; drop it.
    drop(handle);
    assert!(!ordinary_origin.exists());
    assert!(ordinary_claim.exists());
    assert!(poller.live_claims.contains(&canonical_live_claim));
    assert!(!outbox.join("delivered").join("m.json").exists());
    assert!(!outbox.join("rejected").join("m.json").exists());

    // The next cycle rebuilds candidates in reverse alias order, dedups to
    // exactly the canonical survivor, and reclamation leaves the live
    // claim untouched under its canonical key.
    let mut next_cycle_candidates = vec![canonical_outbox.clone(), escaped_alias.clone()];
    dedup_outbox_dirs_by_object_id(&mut next_cycle_candidates);
    assert_eq!(next_cycle_candidates.len(), 1);
    assert_eq!(
        next_cycle_candidates[0].as_os_str(),
        canonical_outbox.as_os_str()
    );
    let claims: Vec<PathBuf> = std::fs::read_dir(&next_cycle_candidates[0])
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("in-flight"))
        .collect();
    poller.reclaim_unowned_wake_claims(&next_cycle_candidates[0], &claims);
    assert!(!ordinary_origin.exists());
    assert!(ordinary_claim.exists());
    assert!(poller.live_claims.contains(&canonical_live_claim));
    assert_eq!(poller.live_claims.len(), 1);
}

/// (#1399 T10) Ten real guard drops from claim-only state restore the
/// origin before counting and reach the unchanged terminal rejection path.
#[tokio::test]
async fn ten_guard_drops_reach_terminal_rejection() {
    let temp = tempfile::TempDir::new().unwrap();
    let origin = write_wake_message(temp.path(), "ten-guard-drops", "2026-07-15T00:00:03+00:00");
    let expected: OutboxMessage =
        serde_json::from_str(&std::fs::read_to_string(&origin).expect("read token-bearing source"))
            .expect("parse token-bearing source");
    assert!(expected.token.is_some());

    let outbox = origin.parent().unwrap();
    let rejected_payload = outbox
        .join("rejected")
        .join(format!("{}.json", expected.id));
    let rejected_reason = outbox
        .join("rejected")
        .join(format!("{}.reason.txt", expected.id));
    let delivered_payload = outbox
        .join("delivered")
        .join(format!("{}.json", expected.id));
    let mut poller = MailboxPoller::new();

    for attempt in 1..=10u32 {
        let claim = wake_claim_path(&origin);
        std::fs::rename(&origin, &claim).unwrap();
        assert!(poller.live_claims.insert(claim.clone()));
        assert!(!origin.exists());
        assert!(claim.exists());

        let report = WakeOutcomeReport::new(poller.wake_outcomes_tx.clone(), origin.clone());
        drop(report);
        poller.drain_wake_outcomes().await;

        assert!(matches!(
            poller.wake_outcomes_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        assert!(poller.live_claims.is_empty());
        assert!(!claim.exists());
        assert!(!delivered_payload.exists());

        if attempt < MAX_DELIVERY_ATTEMPTS {
            assert!(origin.exists());
            assert_eq!(
                poller
                    .retry_tracker
                    .get(&origin)
                    .map(|state| state.attempt_count),
                Some(attempt)
            );
            assert!(!rejected_payload.exists());
            assert!(!rejected_reason.exists());
        } else {
            assert!(!origin.exists());
            assert!(!poller.retry_tracker.contains_key(&origin));
            assert!(rejected_payload.exists());
            assert!(rejected_reason.exists());

            let rejected: OutboxMessage =
                serde_json::from_str(&std::fs::read_to_string(&rejected_payload).unwrap()).unwrap();
            assert_eq!(rejected.id, expected.id);
            assert_eq!(rejected.from, expected.from);
            assert_eq!(rejected.to, expected.to);
            assert_eq!(rejected.body, expected.body);
            assert_eq!(rejected.timestamp, expected.timestamp);
            assert_eq!(rejected.token, None);

            let reason = std::fs::read(&rejected_reason).unwrap();
            assert_eq!(
                reason,
                "Undeliverable after 10 attempts. Last error: wake worker ended without \
                    reporting an outcome"
                    .as_bytes()
            );
        }
    }
}

/// (#1399 T11) Observation failure retains exact retry state, while a
/// successfully observed missing origin and claim are pruned.
#[test]
fn retry_prune_retains_state_when_presence_observation_fails() {
    let temp = tempfile::TempDir::new().unwrap();
    let unobservable = temp.path().join("retry-observation\0.json");
    assert!(unobservable.try_exists().is_err());
    let missing = temp.path().join("definitely-missing.json");
    assert!(matches!(missing.try_exists(), Ok(false)));
    assert!(matches!(wake_claim_path(&missing).try_exists(), Ok(false)));

    let mut retry_tracker = std::collections::HashMap::new();
    retry_tracker.insert(
        unobservable.clone(),
        RetryState {
            attempt_count: 7,
            logged: true,
        },
    );
    retry_tracker.insert(
        missing.clone(),
        RetryState {
            attempt_count: 3,
            logged: false,
        },
    );

    let live_claims = std::collections::HashSet::new();
    prune_retry_tracker(&mut retry_tracker, &live_claims, None);

    assert_eq!(retry_tracker.len(), 1);
    let retained = retry_tracker.get(&unobservable).unwrap();
    assert_eq!(retained.attempt_count, 7);
    assert!(retained.logged);
    assert!(!retry_tracker.contains_key(&missing));
}

/// (#1399 T13) A worker restoring the origin between the two absence probes
/// must not lose the counted attempt: the live ledger retains the entry.
#[test]
fn retry_prune_retains_state_when_live_claim_restores_between_probes() {
    let temp = tempfile::TempDir::new().unwrap();
    let origin = temp.path().join("restore-race.json");
    let claim = wake_claim_path(&origin);
    std::fs::write(&claim, "{}").unwrap();
    let mut live_claims = std::collections::HashSet::new();
    live_claims.insert(claim.clone());
    let mut retry_tracker = std::collections::HashMap::new();
    retry_tracker.insert(
        origin.clone(),
        RetryState {
            attempt_count: MAX_DELIVERY_ATTEMPTS - 1,
            logged: true,
        },
    );

    let probe_barrier = RetryPruneProbeBarrier {
        origin_absent: std::sync::Arc::new(std::sync::Barrier::new(2)),
        restore_complete: std::sync::Arc::new(std::sync::Barrier::new(2)),
    };
    let worker_origin = origin.clone();
    let worker_claim = claim.clone();
    let barrier_first = probe_barrier.origin_absent.clone();
    let barrier_second = probe_barrier.restore_complete.clone();
    let worker = std::thread::spawn(move || {
        barrier_first.wait();
        release_wake_claim(&worker_claim, &worker_origin);
        let origin_present = worker_origin.try_exists();
        let claim_present = worker_claim.try_exists();
        barrier_second.wait();
        (origin_present, claim_present)
    });

    prune_retry_tracker(&mut retry_tracker, &live_claims, Some(&probe_barrier));

    let (origin_present, claim_present) = worker.join().unwrap();
    assert!(matches!(origin_present, Ok(true)));
    assert!(matches!(claim_present, Ok(false)));
    assert_eq!(retry_tracker.len(), 1);
    let state = retry_tracker.get(&origin).unwrap();
    assert_eq!(state.attempt_count, MAX_DELIVERY_ATTEMPTS - 1);
    assert!(state.logged);
    assert!(origin.exists());
    assert!(!claim.exists());
    assert!(live_claims.contains(&claim));
}

/// (#1399 AC-2) Two messages to one target inside one cycle: the first is
/// claimed and handed off, the second gets LaneBusy and stays untouched in
/// the outbox, and exactly one spawn happens while the delivery is in
/// flight.
#[tokio::test]
async fn poll_serializes_same_target_wakes_behind_one_lane() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let hooks = MailboxTestHooks::default();
    let (spawn_release, spawn_gate) = tokio::sync::oneshot::channel();
    *hooks.internal_spawn_gate.lock().unwrap() = Some(spawn_gate);
    {
        let settings = app.state::<SettingsState>();
        settings
            .write()
            .await
            .project_paths
            .push(fixture.sender_cwd.to_string_lossy().to_string());
    }
    // Distinct timestamps make admission order deterministic via the sort.
    let first = write_wake_message(
        &fixture.sender_cwd,
        "same-target-first",
        "2026-07-15T00:00:01+00:00",
    );
    let second = write_wake_message(
        &fixture.sender_cwd,
        "same-target-second",
        "2026-07-15T00:00:02+00:00",
    );
    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    poller.poll(&app).await.unwrap();
    // The worker is parked inside the spawn hook, still holding the lane.
    hooks.internal_spawn_started.notified().await;

    assert!(wake_claim_path(&first).exists());
    assert!(!first.exists());
    assert!(second.exists());
    assert!(!wake_claim_path(&second).exists());
    assert_eq!(hooks.spawn_calls.lock().unwrap().len(), 1);

    drop(spawn_release);
    poller.settle_wake_workers().await;

    let delivered = first
        .parent()
        .unwrap()
        .join("delivered")
        .join("same-target-first.json");
    assert!(delivered.exists());
    assert!(!wake_claim_path(&first).exists());
    assert!(second.exists());
    assert!(poller.live_claims.is_empty());
    assert_eq!(hooks.spawn_calls.lock().unwrap().len(), 1);
}
