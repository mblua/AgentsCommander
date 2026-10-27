use super::*;

#[tokio::test]
async fn shared_pty_engine_injects_exact_text_and_enters_for_verified_idle_target() {
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle().clone();
    let (route, sender_id, target_id) =
        seed_shared_pty_engine_sessions(&fixture, SessionStatus::Idle).await;
    // The scope/readiness observation does not turn an ordinary Foreign
    // Container recipe into a protected Local replacement requirement.
    let original_settings = app.state::<SettingsState>().read().await.clone();
    {
        let settings_state = app.state::<SettingsState>();
        let mut settings = settings_state.write().await;
        let agent = settings
            .agents
            .iter_mut()
            .find(|agent| agent.id == "codex")
            .unwrap();
        agent.backend.kind = SessionBackendKind::ContainerTransport;
        agent.backend.image = Some("agentscommander/ac-codex:latest".into());
        let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
            &settings,
            "codex",
            &route.target.replica_root.to_string_lossy(),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            SessionBackendKind::from(&spawn.backend),
            SessionBackendKind::ContainerTransport
        );
    }
    let (scope, retained) = observe_pty_spawn_scope(&app, &route.target).await;
    assert!(matches!(
        scope,
        crate::config::sessions_persistence::CreateCarrierScope::Foreign { .. }
    ));
    assert!(retained.is_none());
    assert!(pty_spawn_scope_may_resume(&app, &route.target).await);
    *app.state::<SettingsState>().write().await = original_settings;
    let store = Arc::new(
        crate::api::message_store::MessageStore::open(
            fixture._temp.path().join("shared-engine-positive.sqlite3"),
        )
        .unwrap(),
    );
    let state = crate::api::message_store::MessageStoreState::ready(Arc::clone(&store));
    let injection_id =
        enqueue_shared_pty_engine_operation(&store, &route, sender_id, b"exact shared input");

    tokio::time::timeout(
        Duration::from_secs(10),
        MailboxPoller::default().dispatch_pty_input_operation(
            &app,
            &state,
            &injection_id,
            crate::phone::types::PtyInputSourcePlane::HostCli,
            None,
        ),
    )
    .await
    .expect("shared engine must finish");

    let result = store
        .query_pty_input_by_injection(&injection_id)
        .unwrap()
        .unwrap();
    assert!(
        result.terminal,
        "actual successful source result is terminal"
    );
    assert_eq!(
        result.status,
        crate::phone::types::PtyInputPublicStatus::Injected
    );
    assert_eq!(
        shared_engine_writes(&app, target_id),
        vec![
            b"exact shared input".to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec()
        ]
    );
    // The same store supplies real Queued metadata and a single actual
    // Preparing claim; resume renews it without reclaiming/burning attempt.
    let resume_id =
        enqueue_shared_pty_engine_operation(&store, &route, sender_id, b"resume metadata");
    let original = store
        .query_pty_input_by_injection(&resume_id)
        .unwrap()
        .unwrap();
    assert!(!original.terminal, "real queued metadata is not a handoff");
    let claimed = store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::HostCli,
            Some(&resume_id),
            &Uuid::new_v4().to_string(),
            chrono::Utc::now(),
        )
        .unwrap()
        .unwrap();
    let current = store
        .query_pty_input_by_injection(&resume_id)
        .unwrap()
        .unwrap();
    assert!(!current.terminal, "real Preparing claim is not a handoff");
    assert!(pty_resume_metadata_matches(&original, &current, &claimed));
    assert_eq!(
        refresh_pty_claim_for_resume(&store, &original, &claimed).await,
        Ok(PtyClaimRefresh::Current),
    );
    let mut wrong_owner = claimed.clone();
    wrong_owner.lease_owner = Uuid::new_v4().to_string();
    assert_eq!(
        refresh_pty_claim_for_resume(&store, &original, &wrong_owner).await,
        Err(crate::phone::types::PtyInputReasonCode::LeaseLost),
        "queued metadata cannot replace actual conditional renew Ok(true)",
    );
    let mut wrong_identity = claimed.clone();
    wrong_identity.op_id = Uuid::new_v4().to_string();
    assert_eq!(
        refresh_pty_claim_for_resume(&store, &original, &wrong_identity).await,
        Err(crate::phone::types::PtyInputReasonCode::AuthorityChanged),
    );
    let mut missing = claimed.clone();
    missing.injection_id = Uuid::new_v4().to_string();
    assert_eq!(
        refresh_pty_claim_for_resume(&store, &original, &missing).await,
        Err(crate::phone::types::PtyInputReasonCode::AuthorityChanged),
    );
    let mut changed = current.clone();
    changed.payload_sha256 = Some("changed".to_string());
    assert!(!pty_resume_metadata_matches(&original, &changed, &claimed));
    changed = current.clone();
    changed.status = crate::phone::types::PtyInputPublicStatus::Actuating;
    assert!(!pty_resume_metadata_matches(&original, &changed, &claimed));
    let connection =
        rusqlite::Connection::open(fixture._temp.path().join("shared-engine-positive.sqlite3"))
            .unwrap();
    let actual: (i64, String, String) = connection
        .query_row(
            "SELECT attempt,status,lease_owner FROM pty_input_operations WHERE injection_id=?1",
            [&resume_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        actual,
        (
            claimed.attempt,
            "preparing".to_string(),
            claimed.lease_owner.clone()
        )
    );
    assert_eq!(claimed.attempt, 1);
    store
        .retry_pty_input_offloaded(
            resume_id,
            claimed.lease_owner,
            crate::phone::types::PtyInputReasonCode::StoreTransient,
            chrono::Utc::now(),
        )
        .await
        .unwrap();

    let terminal_id =
        enqueue_shared_pty_engine_operation(&store, &route, sender_id, b"terminal metadata");
    let terminal_original = store
        .query_pty_input_by_injection(&terminal_id)
        .unwrap()
        .unwrap();
    let terminal_claim = store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::HostCli,
            Some(&terminal_id),
            &Uuid::new_v4().to_string(),
            chrono::Utc::now(),
        )
        .unwrap()
        .unwrap();
    let mut mismatched_original = terminal_original.clone();
    mismatched_original.payload_sha256 = Some("0".repeat(64));
    let reason = refresh_pty_claim_for_resume(&store, &mismatched_original, &terminal_claim)
        .await
        .unwrap_err();
    assert_eq!(
        reason,
        crate::phone::types::PtyInputReasonCode::AuthorityChanged
    );
    let mut terminal_heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
        Arc::clone(&store),
        terminal_id.clone(),
        terminal_claim.lease_owner.clone(),
        crate::phone::types::parse_canonical_pty_timestamp(&terminal_claim.expires_at).unwrap(),
    );
    assert!(
        finish_pty_input_before_boundary(
            &store,
            &mut terminal_heartbeat,
            &terminal_id,
            &terminal_claim.lease_owner,
            reason,
        )
        .await,
        "actual terminal persistence certifies the release"
    );
    let terminal_result = store
        .query_pty_input_by_injection(&terminal_id)
        .unwrap()
        .unwrap();
    assert!(
        terminal_result.terminal,
        "actual rejection result is terminal"
    );
    assert_eq!(
        terminal_result.status,
        crate::phone::types::PtyInputPublicStatus::Rejected
    );
    assert_eq!(
        terminal_result.reason.as_ref().map(|reason| reason.code),
        Some(reason)
    );
    let (attempt, retries): (i64, i64) = connection
        .query_row(
            "SELECT attempt, (SELECT count(*) FROM pty_input_audit WHERE injection_id=?1 AND \
            status='retry') FROM pty_input_operations WHERE injection_id=?1",
            [&terminal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(attempt, terminal_claim.attempt);
    assert_eq!(retries, 0);
    assert_eq!(
        refresh_pty_claim_for_resume(&store, &terminal_original, &terminal_claim).await,
        Ok(PtyClaimRefresh::Terminal),
    );
    let after_terminal = store
        .query_pty_input_by_injection(&terminal_id)
        .unwrap()
        .unwrap();
    assert_eq!(after_terminal, terminal_result);

    // Same real engine, same idle fixture, now committed Closing before enqueue.
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle().clone();
    let tracker = crate::pty::manager::ResponseCloseState::default();
    assert!(app.manage(tracker.clone()));
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    pty.lock().unwrap().bind_response_close(tracker.clone());
    let (route, sender_id, target_id) =
        seed_shared_pty_engine_sessions(&fixture, SessionStatus::Idle).await;
    let database = fixture._temp.path().join("shared-engine-closing.sqlite3");
    let store = Arc::new(crate::api::message_store::MessageStore::open(database.clone()).unwrap());
    store.bind_response_close(tracker.clone());
    use crate::pty::manager::{
        ResponseClosePublisher, ResponseDeliveryProof, ResponseRouteIdentity, ReturnDeliveryProof,
    };
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
    tracker.try_enable_readiness().unwrap();
    let now = std::time::Instant::now() + Duration::from_secs(3600);
    tracker.apply_settings(true, 30, now);
    let member = ResponseRouteIdentity {
        target: route.target.canonical_fqn.clone(),
        session_id: target_id,
        token: Uuid::new_v4(),
        generation: tracker.generation(target_id).unwrap(),
        carrier_version: 1,
        backend: crate::pty::backend::SessionBackendKind::LocalProcess,
    };
    let coordinator = ResponseRouteIdentity {
        target: route.sender.canonical_fqn.clone(),
        session_id: sender_id,
        token: Uuid::new_v4(),
        generation: tracker.generation(sender_id).unwrap(),
        ..member.clone()
    };
    for (identity, eligible) in [(&member, true), (&coordinator, false)] {
        tracker.publish_manager_row(
            identity.session_id,
            identity.token,
            identity.generation,
            1,
            eligible,
            false,
            now,
        );
        tracker.publish_manager_facts(identity.clone(), eligible, false, now);
    }
    tracker.observe_detector(target_id, false, now);
    tracker.publish_menu(target_id, Some(false), now);
    tracker.publish_hold(target_id, None, true, now);
    let mut admission = tracker.reserve_work(&member.target, "request").unwrap();
    admission.accept_durable("body").unwrap();
    drop(admission);
    let context = tracker
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            "request".into(),
            "body".into(),
            now,
        )
        .unwrap();
    tracker.mark_request_submitted(&context, true);
    tracker
        .register_request(
            ResponseDeliveryProof {
                context,
                member: member.clone(),
                coordinator: coordinator.clone(),
                published_carrier_version: 1,
            },
            now,
        )
        .unwrap();
    let context = tracker
        .admit_return(
            &member,
            &coordinator,
            "return".into(),
            "return-body".into(),
            now,
        )
        .unwrap();
    tracker
        .observe_return_success(ReturnDeliveryProof { context }, now)
        .unwrap();
    let at = now + Duration::from_secs(30);
    let ticket = tracker.select_ticket(target_id, at).unwrap();
    PtyManager::authority_route_proof(&pty, target_id)
        .unwrap()
        .try_commit_response_close(&tracker, &ticket, at)
        .unwrap();
    assert!(tracker.target_is_closing(&member.target));
    let state = crate::api::message_store::MessageStoreState::ready(Arc::clone(&store));
    let injection_id =
        enqueue_shared_pty_engine_operation(&store, &route, sender_id, b"must remain queued");
    let connection = rusqlite::Connection::open(&database).unwrap();
    let snapshot = || {
        connection
            .query_row(
                "SELECT \
                 status,attempt,lease_owner,lease_until,payload,queued_at,updated_at,\
                 actuating_at,selected_session_id FROM pty_input_operations WHERE \
                 injection_id=?1",
                [&injection_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<Vec<u8>>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .unwrap()
    };
    let before = snapshot();
    let hooks = MailboxTestHooks::default();
    MailboxPoller::new_with_test_hooks(hooks.clone())
        .dispatch_pty_input_operation(
            &app,
            &state,
            &injection_id,
            crate::phone::types::PtyInputSourcePlane::HostCli,
            None,
        )
        .await;
    assert_eq!(snapshot(), before);
    assert_eq!(before.0, "queued");
    assert_eq!(before.1, 0);
    assert!(shared_engine_writes(&app, target_id).is_empty());
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    let key = format!(
        "pty:{}:{}:{}",
        route.sender.canonical_fqn.len(),
        route.sender.canonical_fqn,
        injection_id
    );
    let digest =
        crate::phone::types::sha256_hex(format!("shared-engine:{injection_id}").as_bytes());
    let mut duplicate = tracker.reserve_work(&member.target, &key).unwrap();
    assert_eq!(duplicate.accept_durable(&digest), Ok(false));
    assert_eq!(
        duplicate.accept_durable("collision"),
        Err(crate::pty::manager::ResponseCloseError::Conflict)
    );
    assert!(tracker.target_is_closing(&member.target));
}

#[tokio::test]
async fn shared_pty_engine_rejects_busy_target_without_any_write() {
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle().clone();
    let (route, sender_id, target_id) =
        seed_shared_pty_engine_sessions(&fixture, SessionStatus::Running).await;
    let store = Arc::new(
        crate::api::message_store::MessageStore::open(
            fixture._temp.path().join("shared-engine-busy.sqlite3"),
        )
        .unwrap(),
    );
    let state = crate::api::message_store::MessageStoreState::ready(Arc::clone(&store));
    let injection_id =
        enqueue_shared_pty_engine_operation(&store, &route, sender_id, b"must not write");

    MailboxPoller::default()
        .dispatch_pty_input_operation(
            &app,
            &state,
            &injection_id,
            crate::phone::types::PtyInputSourcePlane::HostCli,
            None,
        )
        .await;

    let result = store
        .query_pty_input_by_injection(&injection_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        result.status,
        crate::phone::types::PtyInputPublicStatus::Rejected
    );
    assert_eq!(
        result.reason.map(|reason| reason.code),
        Some(crate::phone::types::PtyInputReasonCode::Busy)
    );
    assert!(shared_engine_writes(&app, target_id).is_empty());
}

#[tokio::test]
async fn shared_pty_engine_bounds_held_permit_wait_by_expiry_then_next_target_progresses() {
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle().clone();
    let (route, sender_id, target_id) =
        seed_shared_pty_engine_sessions(&fixture, SessionStatus::Idle).await;
    let store = Arc::new(
        crate::api::message_store::MessageStore::open(
            fixture._temp.path().join("shared-engine-expiry.sqlite3"),
        )
        .unwrap(),
    );
    let state = crate::api::message_store::MessageStoreState::ready(Arc::clone(&store));

    // A user write blocked in backend I/O holds the target's input permit
    // without having stamped the session busy yet. The privileged operation
    // must not wait on that permit past its own operation deadline.
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let held_permit = PtyManager::acquire_input_writer(&pty, target_id)
        .await
        .unwrap();

    // The fixed 10-minute validity window closes ~3s from now, so the engine
    // reaches the permit wait while still valid and the deadline fires during
    // the wait rather than at the dispatch-start expiry check.
    let issued = chrono::Utc::now() - chrono::Duration::minutes(10) + chrono::Duration::seconds(3);
    let expiring_id = enqueue_shared_pty_engine_operation_issued_at(
        &store,
        &route,
        sender_id,
        b"must never be written",
        issued,
    );

    tokio::time::timeout(
        Duration::from_secs(30),
        MailboxPoller::default().dispatch_pty_input_operation(
            &app,
            &state,
            &expiring_id,
            crate::phone::types::PtyInputSourcePlane::HostCli,
            None,
        ),
    )
    .await
    .expect("the engine must return at the deadline, not wait on the held permit forever");

    // Terminalized as expired, with zero PTY writes.
    let expired = store
        .query_pty_input_by_injection(&expiring_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        expired.status,
        crate::phone::types::PtyInputPublicStatus::Rejected
    );
    assert_eq!(
        expired.reason.map(|reason| reason.code),
        Some(crate::phone::types::PtyInputReasonCode::Expired)
    );
    assert!(shared_engine_writes(&app, target_id).is_empty());

    // Every pre-boundary target reservation was released on the timed-out
    // path: the exact-target lock map holds no entry for this target.
    assert_eq!(state.target_gate.as_ref().unwrap().exact_entry_count(), 0);

    // Once the blocking writer releases the permit, a subsequent operation for
    // the same target progresses to a real injection: capacity and ownership
    // are restored after a held writer crosses expiry.
    drop(held_permit);
    let next_id =
        enqueue_shared_pty_engine_operation(&store, &route, sender_id, b"next after expiry");
    tokio::time::timeout(
        Duration::from_secs(10),
        MailboxPoller::default().dispatch_pty_input_operation(
            &app,
            &state,
            &next_id,
            crate::phone::types::PtyInputSourcePlane::HostCli,
            None,
        ),
    )
    .await
    .expect("a fresh operation for the freed target must finish");
    let next = store
        .query_pty_input_by_injection(&next_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        next.status,
        crate::phone::types::PtyInputPublicStatus::Injected
    );
    assert_eq!(
        shared_engine_writes(&app, target_id),
        vec![
            b"next after expiry".to_vec(),
            b"\r".to_vec(),
            b"\r".to_vec()
        ]
    );
    assert_eq!(state.target_gate.as_ref().unwrap().exact_entry_count(), 0);
}
