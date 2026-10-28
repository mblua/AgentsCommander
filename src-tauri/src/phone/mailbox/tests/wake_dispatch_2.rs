use super::*;

#[tokio::test]
async fn wake_dispatch_no_write_guarantee() {
    // Flagged cold spawn: the recorded spawn decision suppresses tooling
    // writes (skip_tooling_save == true at the test-hook boundary).
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    poller
        .deliver_wake_with_origin(
            &app,
            &dispatch_wake_message("codex", Some("C")),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap();
    let calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].skip_tooling_save);

    // No-flag `auto` wake: today's persist behavior is kept.
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    poller
        .deliver_wake_with_origin(
            &app,
            &dispatch_wake_message("auto", None),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap();
    let calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].skip_tooling_save);
}

#[tokio::test]
async fn wake_dispatch_live_target_annotates_not_applied() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let live_id =
        add_mailbox_session(&app, &fixture.target_cwd, "live", SessionStatus::Idle, None).await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(live_id, true);
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    let delivered = poller
        .deliver_wake_with_origin(
            &app,
            &dispatch_wake_message("codex", Some("C")),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap();

    // Still delivered to the live PTY.
    assert_eq!(hooks.inject_calls.lock().unwrap().as_slice(), &[live_id]);
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    // Annotated as not-applied; no effective values (nothing spawned).
    assert_eq!(
        delivered.dispatch_not_applied.as_deref(),
        Some("live-target")
    );
    assert!(delivered.effective_agent_id.is_none());
    assert!(delivered.effective_profile.is_none());
    assert!(!delivered.profile_fallback_applied);
}

#[tokio::test]
async fn wake_dispatch_invalid_profile_permanently_rejected() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let poller = MailboxPoller::new_with_test_hooks(MailboxTestHooks::default());

    let err = poller
        .deliver_wake_with_origin(
            &app,
            &dispatch_wake_message("codex", Some("3")),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap_err();

    assert!(
        err.contains("single letter A through Z"),
        "reason was: {err}"
    );
    assert!(is_permanent_delivery_error(&err));
}

#[tokio::test]
async fn wake_dispatch_hand_written_invalid_profile_is_rejected_with_reason() {
    // A hand-written outbox JSON bypasses the CLI's pre-enqueue validation
    // (1635-1); the daemon must terminally reject it with the A-Z reason.
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let hooks = MailboxTestHooks::default();
    {
        let settings = app.state::<SettingsState>();
        settings
            .write()
            .await
            .project_paths
            .push(fixture.sender_cwd.to_string_lossy().to_string());
    }
    let outbox_dir = fixture
        .sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let mut msg = dispatch_wake_message("codex", Some("3"));
    msg.id = "hand-written-invalid".to_string();
    msg.token = Some(MAILBOX_MASTER_TOKEN.to_string());
    let path = outbox_dir.join("hand-written-invalid.json");
    std::fs::write(&path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();

    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    poller.poll(&app).await.unwrap();
    poller.settle_wake_workers().await;

    assert!(!path.exists());
    let reason = std::fs::read_to_string(
        outbox_dir
            .join("rejected")
            .join("hand-written-invalid.reason.txt"),
    )
    .unwrap();
    assert!(
        reason.contains("single letter A through Z"),
        "reason was: {reason}"
    );
}

#[tokio::test]
async fn wake_without_flags_produces_legacy_delivered_json() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    let delivered = poller
        .deliver_wake_with_origin(
            &app,
            &dispatch_wake_message("auto", None),
            WakeDeliveryOrigin::FilesystemPoller,
        )
        .await
        .unwrap();

    let json = serde_json::to_value(&delivered).unwrap();
    assert!(json.get("effectiveAgentId").is_none());
    assert!(json.get("effectiveProfile").is_none());
    assert!(json.get("profileFallbackApplied").is_none());
    assert!(json.get("dispatchNotApplied").is_none());
    assert!(json.get("requestedProfile").is_none());
}

#[tokio::test]
async fn wake_db_api_empty_preferred_agent_is_no_flag() {
    // The API/DB shape (`build_outbox_message` writes preferred_agent "")
    // must behave exactly like a no-flag wake: skip_tooling_save stays
    // false and the delivered JSON carries no annotation keys.
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let hooks = MailboxTestHooks::default();
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let mut message = wake_message_to_target();
    message.id = "db-api-empty-agent".to_string();
    message.preferred_agent = String::new();
    message.sender_agent = None;
    message.requested_profile = None;

    let delivered = poller
        .deliver_wake_with_origin(&app, &message, WakeDeliveryOrigin::DbQueue)
        .await
        .unwrap();

    let calls = hooks.spawn_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].skip_tooling_save);
    let json = serde_json::to_value(&delivered).unwrap();
    assert!(json.get("effectiveAgentId").is_none());
    assert!(json.get("effectiveProfile").is_none());
    assert!(json.get("profileFallbackApplied").is_none());
    assert!(json.get("dispatchNotApplied").is_none());
}

#[tokio::test]
async fn poll_rejects_terminal_logical_command_on_first_attempt() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let session_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-live",
        "pi",
        SessionStatus::Idle,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(session_id, true);
    {
        let settings = app.state::<SettingsState>();
        settings
            .write()
            .await
            .project_paths
            .push(fixture.sender_cwd.to_string_lossy().to_string());
    }
    let source = write_logical_command_message(
        &fixture.sender_cwd,
        "poll-pi-compact",
        "compact",
        "follow-up",
    );
    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    poller.poll(&app).await.unwrap();
    poller.settle_wake_workers().await;

    assert!(!source.exists());
    let reason_path = source
        .parent()
        .unwrap()
        .join("rejected")
        .join("poll-pi-compact.reason.txt");
    let reason = std::fs::read_to_string(reason_path).unwrap();
    assert_eq!(
        reason,
        "Cannot execute logical remote command 'compact': session shell 'pi' has no verified \
            mapping. Claude / Codex / Antigravity / Cursor agent direct shells use /clear and \
            /compact; exact Pi uses /new for clear only. cmd / pwsh outer wrappers and Pi compact \
            are unsupported."
    );
    assert!(!reason.contains("Undeliverable after"));
    assert!(!poller.retry_tracker.contains_key(&source));
    assert!(hooks.events.lock().unwrap().is_empty());
    assert!(hooks.settle_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn poll_keeps_supported_busy_command_retriable() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let session_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-busy",
        "pi",
        SessionStatus::Running,
    )
    .await;
    register_mock_pty_route(&app, session_id);
    {
        let settings = app.state::<SettingsState>();
        settings
            .write()
            .await
            .project_paths
            .push(fixture.sender_cwd.to_string_lossy().to_string());
    }
    let source =
        write_logical_command_message(&fixture.sender_cwd, "poll-pi-busy-clear", "clear", "");
    let mut poller = MailboxPoller::default();

    poller.poll(&app).await.unwrap();
    poller.settle_wake_workers().await;

    assert!(source.exists());
    assert_eq!(
        poller
            .retry_tracker
            .get(&source)
            .map(|state| state.attempt_count),
        Some(1)
    );
    assert!(!source
        .parent()
        .unwrap()
        .join("rejected")
        .join("poll-pi-busy-clear.reason.txt")
        .exists());
    assert!(mock_pty_writes_for(&app, session_id).is_empty());
}

/// (#1399 T2) The claim is a same-directory suffix: invisible to the
/// `extension() == "json"` scan filter, same archive parent, exact inverse.
#[test]
fn wake_claim_is_a_same_directory_suffix_with_an_exact_inverse() {
    let origin = Path::new("outbox-dir/abc.json");
    let claim = wake_claim_path(origin);
    assert_eq!(claim, PathBuf::from("outbox-dir/abc.json.in-flight"));
    assert_eq!(
        claim.extension().and_then(|s| s.to_str()),
        Some("in-flight")
    );
    assert_eq!(claim.parent(), origin.parent());
    assert_eq!(wake_claim_origin(&claim), Some(origin.to_path_buf()));
    // An id containing a dot survives the round trip (with_extension would
    // truncate it).
    let dotted = Path::new("outbox-dir/a.b.json");
    assert_eq!(
        wake_claim_origin(&wake_claim_path(dotted)),
        Some(dotted.to_path_buf())
    );
    // Never a claim this poller wrote: no suffix, or not a `.json` origin.
    assert_eq!(wake_claim_origin(Path::new("outbox-dir/abc.json")), None);
    assert_eq!(
        wake_claim_origin(Path::new("outbox-dir/abc.txt.in-flight")),
        None
    );
}

fn assert_reclaimed_response_work(
    poller: &MailboxPoller,
    tracker: &crate::pty::manager::ResponseCloseState,
    member: &crate::pty::manager::ResponseRouteIdentity,
    coordinator: &crate::pty::manager::ResponseRouteIdentity,
    now: std::time::Instant,
    cases: [(&PathBuf, bool); 4],
) {
    for (claim, retained) in cases {
        let origin = wake_claim_origin(claim).unwrap();
        let key = format!("fs:{}", origin.display());
        let digest = crate::phone::types::sha256_hex(b"{}");
        let candidate = tracker.reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            key.clone(),
            digest.clone(),
            now,
        );
        if retained {
            assert!(
                candidate.is_ok(),
                "restored/live work remains pending with exact digest"
            );
            let sources = poller.response_sources.lock().unwrap();
            let work = &sources[&origin];
            assert!(work.state.same_state(tracker));
            assert_eq!(work.target, member.target);
            assert_eq!(work.key, key);
            assert_eq!(work.digest, digest);
            assert!(work.delivery_context.is_none());
            assert!(work.return_context.is_none());
        } else {
            assert!(matches!(
                candidate,
                Err(crate::pty::manager::ResponseCloseError::Conflict)
            ));
            assert!(!poller
                .response_sources
                .lock()
                .unwrap()
                .contains_key(&origin));
        }
        // Receipt recovery never fabricates an ordinary return-success proof.
        assert!(tracker
            .select_ticket(member.session_id, now + Duration::from_secs(60))
            .is_err());
    }
}

/// (#1399 T3) Reclamation, all four branches: an unowned claim with no
/// receipt is returned; a claim whose receipt exists is deleted without
/// recreating the message (the no-double-delivery assertion, for both
/// `delivered/` and `rejected/`); a claim in `live_claims` is untouched.
#[test]
fn reclaim_returns_unowned_claims_and_never_resurrects_settled_or_live_ones() {
    let temp = tempfile::TempDir::new().unwrap();
    let outbox = temp.path().join("outbox");
    std::fs::create_dir_all(outbox.join("delivered")).unwrap();
    std::fs::create_dir_all(outbox.join("rejected")).unwrap();

    let unowned = outbox.join("m-unowned.json.in-flight");
    let delivered = outbox.join("m-delivered.json.in-flight");
    let rejected = outbox.join("m-rejected.json.in-flight");
    let live = outbox.join("m-live.json.in-flight");
    for claim in [&unowned, &delivered, &rejected, &live] {
        std::fs::write(claim, "{}").unwrap();
    }
    std::fs::write(outbox.join("delivered").join("m-delivered.json"), "{}").unwrap();
    std::fs::write(outbox.join("rejected").join("m-rejected.json"), "{}").unwrap();

    let mut poller = MailboxPoller::default();
    poller.live_claims.insert(live.clone());
    let claims = vec![
        unowned.clone(),
        delivered.clone(),
        rejected.clone(),
        live.clone(),
    ];

    // C: the same healthy tracker backs all four existing reclaim cases.
    use crate::pty::manager::{ResponseClosePublisher, ResponseCloseState, ResponseRouteIdentity};
    let now = std::time::Instant::now() + Duration::from_secs(3600);
    let tracker = ResponseCloseState::default();
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
    let member = ResponseRouteIdentity {
        target: "project:room/member".into(),
        session_id: Uuid::new_v4(),
        token: Uuid::new_v4(),
        generation: 1,
        carrier_version: 1,
        backend: crate::pty::backend::SessionBackendKind::LocalProcess,
    };
    let coordinator = ResponseRouteIdentity {
        target: "project:room/coordinator".into(),
        session_id: Uuid::new_v4(),
        token: Uuid::new_v4(),
        generation: 2,
        ..member.clone()
    };
    for identity in [&member, &coordinator] {
        tracker.register_route(identity.session_id, identity.generation, now);
        tracker.publish_manager_row(
            identity.session_id,
            identity.token,
            identity.generation,
            identity.carrier_version,
            true,
            false,
            now,
        );
        tracker.publish_manager_facts(identity.clone(), true, false, now);
    }
    tracker.observe_detector(member.session_id, false, now);
    tracker.publish_menu(member.session_id, Some(false), now);
    tracker.publish_hold(member.session_id, None, true, now);
    // Positive control only: this healthy fixture can arm a real pure ticket.
    // No receipt/reclaim branch receives or manufactures this success proof.
    let mut positive = tracker
        .reserve_work(&member.target, "positive-request")
        .unwrap();
    positive.accept_durable("positive-body").unwrap();
    drop(positive);
    let context = tracker
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            "positive-request".into(),
            "positive-body".into(),
            now,
        )
        .unwrap();
    tracker.mark_request_submitted(&context, true);
    tracker
        .register_request(
            crate::pty::manager::ResponseDeliveryProof {
                context,
                member: member.clone(),
                coordinator: coordinator.clone(),
                published_carrier_version: member.carrier_version,
            },
            now,
        )
        .unwrap();
    let context = tracker
        .admit_return(
            &member,
            &coordinator,
            "positive-return".into(),
            "positive-return-body".into(),
            now,
        )
        .unwrap();
    tracker
        .observe_return_success(crate::pty::manager::ReturnDeliveryProof { context }, now)
        .unwrap();
    assert!(tracker
        .select_ticket(member.session_id, now + Duration::from_secs(30))
        .is_ok());
    for claim in &claims {
        let origin = wake_claim_origin(claim).unwrap();
        let key = format!("fs:{}", origin.display());
        let digest = crate::phone::types::sha256_hex(std::fs::read(claim).unwrap().as_slice());
        let mut admission = tracker.reserve_work(&member.target, &key).unwrap();
        admission.accept_durable(&digest).unwrap();
        drop(admission);
        assert!(tracker
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                key.clone(),
                digest.clone(),
                now
            )
            .is_ok());
        poller.response_sources.lock().unwrap().insert(
            origin,
            ResponseWakeWork {
                diagnostic_message_id: None,
                state: tracker.clone(),
                target: member.target.clone(),
                key,
                digest,
                attempt: Arc::default(),
                return_context: None,
                return_routes: None,
                delivery_context: None,
            },
        );
    }

    poller.reclaim_unowned_wake_claims(&outbox, &claims);

    assert!(!unowned.exists());
    assert!(outbox.join("m-unowned.json").exists());
    assert!(!delivered.exists());
    assert!(!outbox.join("m-delivered.json").exists());
    assert!(!rejected.exists());
    assert!(!outbox.join("m-rejected.json").exists());
    assert!(live.exists());
    assert!(!outbox.join("m-live.json").exists());
    assert_reclaimed_response_work(
        &poller,
        &tracker,
        &member,
        &coordinator,
        now,
        [
            (&unowned, true),
            (&delivered, false),
            (&rejected, false),
            (&live, true),
        ],
    );
}

/// (#1399 T4) The sort key is send time, not filename: an earlier timestamp
/// in `zzzz.json` sorts before a later one in `aaaa.json`; malformed,
/// absent, and unreadable timestamps all yield `None`, which sorts first.
#[test]
fn outbox_send_time_orders_by_timestamp_not_filename() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path();

    let earlier_in_zzzz = dir.join("zzzz.json");
    let mut early = wake_message_to_target();
    early.timestamp = "2026-07-15T00:00:01+00:00".into();
    std::fs::write(
        &earlier_in_zzzz,
        serde_json::to_string_pretty(&early).unwrap(),
    )
    .unwrap();

    let later_in_aaaa = dir.join("aaaa.json");
    let mut late = wake_message_to_target();
    // A different RFC3339 spelling (Z suffix, fractional part) must still
    // compare correctly, which is why the key is parsed, not byte-ordered.
    late.timestamp = "2026-07-15T00:00:02.500Z".into();
    std::fs::write(&later_in_aaaa, serde_json::to_string_pretty(&late).unwrap()).unwrap();

    let early_key = outbox_message_send_time(&earlier_in_zzzz).unwrap();
    let late_key = outbox_message_send_time(&later_in_aaaa).unwrap();
    assert!(early_key < late_key);

    let malformed = dir.join("malformed.json");
    let mut bad = wake_message_to_target();
    bad.timestamp = "not-a-timestamp".into();
    std::fs::write(&malformed, serde_json::to_string_pretty(&bad).unwrap()).unwrap();
    assert_eq!(outbox_message_send_time(&malformed), None);

    let absent = dir.join("absent.json");
    std::fs::write(&absent, "{\"id\":\"x\"}").unwrap();
    assert_eq!(outbox_message_send_time(&absent), None);

    // Not a regular file: the hardened reader refuses it, the key is None.
    assert_eq!(outbox_message_send_time(dir), None);

    // `None` sorts first under `Option::cmp`, so an unreadable document is
    // never delayed behind valid traffic.
    assert!(outbox_message_send_time(&malformed) < Some(early_key));
}

/// (#1399 T8) Both alias orders retain the same canonical spelling, so a
/// later cycle cannot change the path-keyed ownership ledger key.
#[test]
fn outbox_dir_spellings_collapse_to_one_physical_entry() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().join("outbox");
    std::fs::create_dir_all(&dir).unwrap();
    let Ok(identity) = crate::path_identity::verify_directory(&dir) else {
        return;
    };
    let alternate = identity.canonical_path.clone();

    // Not `#[ignore]`: skip at runtime where the platform cannot express
    // a distinct raw spelling that verifies as the same physical object.
    if dir.as_os_str() == alternate.as_os_str() {
        return;
    }
    let same_object = match crate::path_identity::verify_directory(&alternate) {
        Ok(alternate_identity) => alternate_identity.object_id == identity.object_id,
        Err(_) => false,
    };
    if !same_object {
        return;
    }

    let expected = crate::path_utils::normalize_windows_verbatim_path_buf(&identity.canonical_path);
    for mut dirs in [
        vec![dir.clone(), alternate.clone()],
        vec![alternate, dir.clone()],
    ] {
        dedup_outbox_dirs_by_object_id(&mut dirs);
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].as_os_str(), expected.as_os_str());
    }

    // A nonexistent path is unverifiable and must be dropped by the
    // `is_dir()` prefilter, not passed to `verify_directory`.
    let mut with_missing = vec![dir.clone(), temp.path().join("no-such-outbox")];
    dedup_outbox_dirs_by_object_id(&mut with_missing);
    assert_eq!(with_missing.len(), 1);
    assert_eq!(with_missing[0].as_os_str(), expected.as_os_str());
}

/// (#1399 T5) The split preserves the retry ledger, twice over: the prune
/// predicate keeps a claimed path's entry alive across the cycle, and the
/// shared drain releases ownership so the next cycle can re-claim and
/// count a second attempt. With a duplicated drain or without the prune
/// fix, the count stays at Some(1).
#[tokio::test]
async fn poll_split_counts_transient_attempts_across_cycles() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let session_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "wake-transient",
        "codex",
        SessionStatus::Idle,
    )
    .await;
    let hooks = MailboxTestHooks::default();
    hooks.pty_presence.lock().unwrap().insert(session_id, true);
    hooks
        .inject_results
        .lock()
        .unwrap()
        .push_back(Err("transient wake failure".to_string()));
    hooks
        .inject_results
        .lock()
        .unwrap()
        .push_back(Err("transient wake failure".to_string()));
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
        "retriable-wake",
        "2026-07-15T00:00:01+00:00",
    );
    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());

    poller.poll(&app).await.unwrap();
    poller.settle_wake_workers().await;

    assert!(source.exists());
    assert_eq!(
        poller
            .retry_tracker
            .get(&source)
            .map(|state| state.attempt_count),
        Some(1)
    );
    assert!(poller.live_claims.is_empty());

    poller.poll(&app).await.unwrap();
    poller.settle_wake_workers().await;

    assert!(source.exists());
    assert_eq!(
        poller
            .retry_tracker
            .get(&source)
            .map(|state| state.attempt_count),
        Some(2)
    );
    assert!(poller.live_claims.is_empty());
    assert_eq!(hooks.inject_calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn issue_3263_minimal_retained_worker_rejects() {
    use super::support_retained_cells::{assert_minimal_retired, minimal_retained_worker};
    assert_eq!(
        super::super::handoff::ERR_OWNED_WAKE_RETAINED,
        "ownedWakeRetained:"
    );
    assert!(is_permanent_delivery_error("ownedWakeRetained: stage=test"));
    for reason in [
        "xownedWakeRetained:",
        "ownedWakeRetained",
        "OwnedWakeRetained:",
    ] {
        assert!(!is_permanent_delivery_error(reason));
    }
    let mut worker = minimal_retained_worker(false).await;
    assert_minimal_retired(&mut worker).await;
    let dir = worker.origin.parent().unwrap().join("rejected");
    let id = worker.origin.file_stem().unwrap().to_str().unwrap();
    let message: OutboxMessage =
        serde_json::from_slice(&std::fs::read(dir.join(format!("{id}.json"))).unwrap()).unwrap();
    assert!(message.token.is_none());
    let reason = std::fs::read_to_string(dir.join(format!("{id}.reason.txt"))).unwrap();
    assert!(reason.contains("fixture_source_core_refused") && reason.contains("uncertain"));
    assert!(!worker.origin.exists());
}

#[tokio::test]
async fn issue_3263_minimal_retained_input_error_no_replay() {
    use super::support_retained_cells::{assert_minimal_retired, minimal_retained_worker};
    let mut worker = minimal_retained_worker(true).await;
    assert_minimal_retired(&mut worker).await;
    // Backend failure returns before recording bytes; stage/failed receipt proves the attempt.
    assert!(!worker.origin.exists());
}

#[tokio::test]
async fn issue_3263_minimal_retained_reject_io_pending() {
    use super::super::handoff::wake_claim_path;
    use super::support_retained_cells::{assert_minimal_retired, minimal_retained_worker};
    super::support_retained_cells::minimal_retained_missing_eight().await;
    for case in [
        "absent",
        "mismatch",
        "mismatch-delete",
        "collision",
        "absent-collision",
        "json",
        "utf8",
        "bom",
        "raw-blocked",
        "reason-only",
        "payload-delete",
        "raw-reason",
    ] {
        let mut worker = minimal_retained_worker(false).await;
        worker.poller.retry_tracker.insert(
            worker.origin.clone(),
            super::super::notice::RetryState {
                attempt_count: 2,
                logged: true,
            },
        );
        let claim = wake_claim_path(&worker.origin);
        let dir = worker.origin.parent().unwrap().join("rejected");
        std::fs::create_dir_all(&dir).unwrap();
        let id = worker
            .origin
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let bytes = std::fs::read(&claim).unwrap();
        let corrupt: &[u8] = match case {
            "json" => b"{invalid",
            "utf8" | "raw-blocked" | "raw-reason" => &[0x80],
            "bom" => &[0xEF, 0xBB, 0xBF, 0x80],
            _ => &bytes,
        };
        std::fs::write(&claim, corrupt).unwrap();
        if case.contains("absent") {
            std::fs::remove_file(&claim).unwrap();
        }
        if case.contains("collision") {
            std::fs::write(&worker.origin, b"foreign origin").unwrap();
        }
        if case.starts_with("mismatch") {
            let mut message: OutboxMessage = serde_json::from_slice(&bytes).unwrap();
            message.id = "different-id".into();
            std::fs::write(&claim, serde_json::to_vec(&message).unwrap()).unwrap();
        }
        let raw = dir.join(claim.file_name().unwrap());
        let payload = dir.join(format!("{id}.json"));
        let reason = dir.join(format!("{id}.reason.txt"));
        if case == "raw-blocked" {
            std::fs::create_dir(&raw).unwrap();
        }
        if case == "reason-only" {
            std::fs::write(&reason, "partial").unwrap();
            std::fs::create_dir(&payload).unwrap();
        }
        let protected = if cfg!(windows) {
            &claim
        } else {
            worker.origin.parent().unwrap()
        };
        let permissions = case
            .ends_with("-delete")
            .then(|| std::fs::metadata(protected).unwrap().permissions());
        if let Some(permissions) = &permissions {
            let mut readonly = permissions.clone();
            readonly.set_readonly(true);
            std::fs::set_permissions(protected, readonly).unwrap();
        }
        if case == "raw-reason" {
            let stem = claim.file_stem().unwrap().to_str().unwrap();
            let raw_reason = dir.join(format!("{stem}.reason.txt"));
            std::fs::create_dir(raw_reason).unwrap();
        }
        let receipts = worker.control.receipts.lock().unwrap().len();
        worker.poller.drain_wake_outcomes().await;
        if [
            "raw-blocked",
            "reason-only",
            "payload-delete",
            "mismatch-delete",
        ]
        .contains(&case)
        {
            assert_eq!(worker.poller.live_claims.len(), 1);
            assert!(worker
                .poller
                .response_sources
                .lock()
                .unwrap()
                .contains_key(&worker.origin));
            assert!(claim.exists());
            assert!(!worker.origin.exists());
            worker.poller.drain_wake_outcomes().await;
            assert_eq!(worker.poller.live_claims.len(), 1);
            assert_eq!(worker.control.receipts.lock().unwrap().len(), receipts);
            if case.ends_with("-delete") {
                let receipt = if case == "mismatch-delete" {
                    dir.join("different-id.json")
                } else {
                    payload.clone()
                };
                assert!(receipt.is_file());
                std::fs::set_permissions(protected, permissions.unwrap()).unwrap();
            } else {
                std::fs::remove_dir(if case == "raw-blocked" {
                    &raw
                } else {
                    &payload
                })
                .unwrap();
            }
        }
        if case.contains("collision") {
            worker.poller.drain_wake_outcomes().await;
            assert!(worker.poller.live_claims.is_empty());
            assert!(worker.poller.retry_tracker.is_empty());
            assert!(worker.poller.response_sources.lock().unwrap().is_empty());
            assert_eq!(std::fs::read(&worker.origin).unwrap(), b"foreign origin");
        }
        assert_minimal_retired(&mut worker).await;
        if ["json", "utf8", "bom", "raw-blocked", "raw-reason"].contains(&case) {
            assert_eq!(std::fs::read(raw).unwrap(), corrupt);
            assert!(!payload.exists());
            if case != "raw-reason" {
                let stem = claim.file_stem().unwrap().to_str().unwrap();
                let reason =
                    std::fs::read_to_string(dir.join(format!("{stem}.reason.txt"))).unwrap();
                assert!(reason.contains("fixture_source_core_refused"));
                assert!(!reason.contains(WAKE_BODY) && !reason.contains(MAILBOX_MASTER_TOKEN));
            }
        }
        if case.starts_with("mismatch") {
            assert!(dir.join("different-id.json").exists());
            assert!(!payload.exists());
        }
    }
}

#[tokio::test]
async fn issue_3263_minimal_retained_followup_eligibility() {
    {
        use super::support_retained_cells::{assert_minimal_retired, minimal_retained_worker};
        let mut worker = minimal_retained_worker(false).await;
        assert_minimal_retired(&mut worker).await;
        let app = worker.fixture.app.handle();
        let coordinator = app.state::<crate::session::selection::SelectionCoordinator>();
        let roots = coordinator.fixture_root_count();
        assert_eq!(roots, Some(1));
        let mut followup = wake_message_to_target();
        followup.id = "retained-root-followup".into();
        followup.token = Some(MAILBOX_MASTER_TOKEN.into());
        // Observe actual blockage, never clear the accepted retained root to get a success.
        let observation = tokio::time::timeout(
            Duration::from_millis(50),
            worker.poller.deliver_wake(app, &followup),
        )
        .await;
        assert!(
            observation.is_err(),
            "retained accepted root still blocks new source"
        );
        assert_eq!(coordinator.fixture_root_count(), roots);
    }
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle();
    let (coordinator, _, _, _) = prepare_retained_exited_respawn(&fixture).await;
    {
        let state = app.state::<SettingsState>();
        let mut settings = state.write().await;
        settings
            .project_paths
            .push(fixture.sender_cwd.to_string_lossy().to_string());
        let agent = settings
            .agents
            .iter_mut()
            .find(|agent| agent.id == "codex")
            .unwrap();
        agent.backend.kind = SessionBackendKind::ContainerTransport;
        agent.backend.image = Some("agentscommander/ac-codex:latest".into());
    }
    let hooks = MailboxTestHooks::default();
    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let old = write_wake_message(&fixture.sender_cwd, "unsupported", "2026-07-15T00:00:01Z");
    poller.poll(app).await.unwrap();
    poller.settle_wake_workers().await;
    let rejected = old
        .parent()
        .unwrap()
        .join("rejected/unsupported.reason.txt");
    assert!(std::fs::read_to_string(rejected)
        .unwrap()
        .contains("unsupported_backend"));
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    assert!(poller.live_claims.is_empty());
    {
        let state = app.state::<SettingsState>();
        let mut settings = state.write().await;
        let agent = settings
            .agents
            .iter_mut()
            .find(|agent| agent.id == "codex")
            .unwrap();
        agent.backend.kind = SessionBackendKind::LocalProcess;
        agent.backend.image = None;
    }
    let session = add_mailbox_session(
        app,
        &fixture.target_cwd,
        "corrected-live",
        SessionStatus::Idle,
        None,
    )
    .await;
    hooks.pty_presence.lock().unwrap().insert(session, true);
    hooks.inject_results.lock().unwrap().push_back(Ok(()));
    let origin = write_wake_message(&fixture.sender_cwd, "followup", "2026-07-15T00:00:02Z");
    poller.poll(app).await.unwrap();
    poller.settle_wake_workers().await;
    assert!(!origin.exists());
    assert_eq!(hooks.inject_calls.lock().unwrap().as_slice(), &[session]);
    assert!(origin
        .parent()
        .unwrap()
        .join("delivered/followup.json")
        .exists());
}
