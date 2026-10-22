use super::*;

#[test]
fn sqlite_identity_check_rejects_a_dangling_sidecar_link_when_supported() {
    let temp = tempfile::TempDir::new().unwrap();
    let database = temp.path().join(DB_FILENAME);
    let sidecar = sqlite_sidecar_paths(&database)[0].clone();
    let missing = temp.path().join("missing-wal");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&missing, &sidecar).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&missing, &sidecar).is_ok();
    #[cfg(not(any(unix, windows)))]
    let linked = false;
    if linked {
        assert!(matches!(
            verify_existing_sqlite_files(&database),
            Err(MessageStoreError::UnsafePath)
        ));
    }
}

fn bootstrap_pty_status_fixture(store: &MessageStore, status: &str) -> (String, String, bool) {
    let id = Uuid::new_v4().to_string();
    let req = pty_request(&id, status);
    let fingerprint = req.request_fingerprint.clone();
    store.enqueue_pty_input(req).unwrap();
    let at = Utc::now();
    if matches!(
        status,
        "preparing" | "retry" | "actuating" | "injected" | "indeterminate"
    ) {
        assert!(store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(&id),
                "bootstrap",
                at
            )
            .unwrap()
            .is_some());
    }
    if status == "retry" {
        store
            .retry_pty_input(
                &id,
                "bootstrap",
                crate::phone::types::PtyInputReasonCode::MenuGuardBlocked,
                at,
            )
            .unwrap();
    }
    if matches!(status, "actuating" | "injected" | "indeterminate") {
        store
            .begin_pty_actuating(
                &id,
                "bootstrap",
                &Uuid::new_v4().to_string(),
                "localProcess",
                at,
            )
            .unwrap();
    }
    match status {
        "injected" => {
            store
                .terminalize_pty_input(
                    &id,
                    crate::phone::types::PtyInputPublicStatus::Injected,
                    None,
                    at,
                )
                .unwrap();
        }
        "rejected" => {
            store
                .terminalize_pty_input(
                    &id,
                    crate::phone::types::PtyInputPublicStatus::Rejected,
                    Some(crate::phone::types::PtyInputReasonCode::Busy),
                    at,
                )
                .unwrap();
        }
        "indeterminate" => {
            store
                .terminalize_pty_input(
                    &id,
                    crate::phone::types::PtyInputPublicStatus::Indeterminate,
                    Some(crate::phone::types::PtyInputReasonCode::DaemonRestartAfterActuation),
                    at,
                )
                .unwrap();
        }
        _ => {}
    }
    (
        id,
        fingerprint,
        matches!(status, "queued" | "preparing" | "retry" | "actuating"),
    )
}

#[test]
fn migration_creates_schema_and_enqueue_writes_row() {
    let store = store();
    let result = store.enqueue(request("op1", "hello")).unwrap();

    assert!(!result.message_id.is_empty());
    assert_eq!(result.status, STATUS_QUEUED);
    assert_eq!(store.count_by_status(STATUS_QUEUED), 1);
    let mut seeds = Vec::new();
    for status in ["queued", "retry", "delivering", "delivered", "poisoned"] {
        let row = store.enqueue(request(status, status)).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE messages SET status=?1 WHERE message_id=?2",
                params![status, row.message_id],
            )
            .unwrap();
        seeds.push((
            "db",
            "proj:wg-1/a",
            "proj:wg-1/b",
            status.to_owned(),
            sha256_hex(status.as_bytes()),
            matches!(status, "queued" | "retry" | "delivering"),
        ));
    }
    for status in [
        "queued",
        "preparing",
        "retry",
        "actuating",
        "injected",
        "rejected",
        "indeterminate",
    ] {
        let (id, fingerprint, pending) = bootstrap_pty_status_fixture(&store, status);
        seeds.push((
            "pty",
            "proj:wg-1-team/lead",
            "proj:wg-1-team/dev",
            id,
            fingerprint,
            pending,
        ));
    }
    let tracker = ResponseCloseState::default();
    for publisher in [
        crate::pty::manager::ResponseClosePublisher::Manager,
        crate::pty::manager::ResponseClosePublisher::Pty,
        crate::pty::manager::ResponseClosePublisher::RawOutput,
        crate::pty::manager::ResponseClosePublisher::Detector,
        crate::pty::manager::ResponseClosePublisher::Menu,
        crate::pty::manager::ResponseClosePublisher::Hold,
    ] {
        tracker.bind_publisher(publisher);
    }
    tracker.apply_settings(true, 30, std::time::Instant::now());
    assert_eq!(
        tracker.try_enable_readiness(),
        Err(crate::pty::manager::ResponseCloseError::Unavailable)
    );
    store.bind_response_close(tracker.clone());
    assert_eq!(tracker.try_enable_readiness(), Ok(()));
    store.bind_response_close(tracker.clone());
    assert_eq!(tracker.try_enable_readiness(), Ok(()));
    for (plane, sender, target, op, digest, pending) in seeds {
        let now = std::time::Instant::now();
        let member = crate::pty::manager::ResponseRouteIdentity {
            target: target.into(),
            session_id: Uuid::new_v4(),
            token: Uuid::new_v4(),
            generation: 1,
            carrier_version: 1,
            backend: crate::pty::backend::SessionBackendKind::LocalProcess,
        };
        let coordinator = crate::pty::manager::ResponseRouteIdentity {
            target: "proj:wg-1/coordinator".into(),
            session_id: Uuid::new_v4(),
            ..member.clone()
        };
        tracker.register_route(member.session_id, 1, now);
        tracker.publish_manager_row(
            member.session_id,
            member.token,
            member.generation,
            member.carrier_version,
            true,
            false,
            now,
        );
        tracker.publish_manager_facts(member.clone(), true, false, now);
        let result = tracker.reserve_policy_candidate(
            member,
            coordinator,
            response_work_key(plane, sender, &op),
            digest,
            now,
        );
        if pending {
            assert!(result.is_ok(), "{plane}/{op}");
        } else {
            assert!(
                matches!(
                    result,
                    Err(crate::pty::manager::ResponseCloseError::Unknown)
                ),
                "{plane}/{op}"
            );
        }
    }
    let other = ResponseCloseState::default();
    store.bind_response_close(other.clone());
    assert_eq!(
        tracker.try_enable_readiness(),
        Err(crate::pty::manager::ResponseCloseError::Unavailable)
    );
    assert_eq!(
        other.try_enable_readiness(),
        Err(crate::pty::manager::ResponseCloseError::Unavailable)
    );
    let broken = self::store();
    broken
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE pty_input_operations;")
        .unwrap();
    let broken_tracker = ResponseCloseState::default();
    for publisher in [
        crate::pty::manager::ResponseClosePublisher::Manager,
        crate::pty::manager::ResponseClosePublisher::Pty,
        crate::pty::manager::ResponseClosePublisher::RawOutput,
        crate::pty::manager::ResponseClosePublisher::Detector,
        crate::pty::manager::ResponseClosePublisher::Menu,
        crate::pty::manager::ResponseClosePublisher::Hold,
    ] {
        broken_tracker.bind_publisher(publisher);
    }
    broken.bind_response_close(broken_tracker.clone());
    assert_eq!(
        broken_tracker.try_enable_readiness(),
        Err(crate::pty::manager::ResponseCloseError::Unavailable)
    );
}

#[test]
fn duplicate_sender_op_id_returns_existing_row() {
    let store = store();
    let tracker = ResponseCloseState::default();
    for publisher in [
        crate::pty::manager::ResponseClosePublisher::Manager,
        crate::pty::manager::ResponseClosePublisher::Pty,
        crate::pty::manager::ResponseClosePublisher::RawOutput,
        crate::pty::manager::ResponseClosePublisher::Detector,
        crate::pty::manager::ResponseClosePublisher::Menu,
        crate::pty::manager::ResponseClosePublisher::Hold,
    ] {
        tracker.bind_publisher(publisher);
    }
    tracker.apply_settings(true, 30, std::time::Instant::now());
    store.bind_response_close(tracker.clone());
    assert_eq!(tracker.try_enable_readiness(), Ok(()));
    let probe = |target: &str, key: &str, digest: &str| {
        let now = std::time::Instant::now();
        let member = crate::pty::manager::ResponseRouteIdentity {
            target: target.into(),
            session_id: Uuid::new_v4(),
            token: Uuid::new_v4(),
            generation: 1,
            carrier_version: 1,
            backend: crate::pty::backend::SessionBackendKind::LocalProcess,
        };
        let coordinator = crate::pty::manager::ResponseRouteIdentity {
            target: "proj:wg-1/coordinator".into(),
            session_id: Uuid::new_v4(),
            ..member.clone()
        };
        tracker.register_route(member.session_id, member.generation, now);
        tracker.publish_manager_row(
            member.session_id,
            member.token,
            member.generation,
            member.carrier_version,
            true,
            false,
            now,
        );
        tracker.publish_manager_facts(member.clone(), true, false, now);
        tracker.reserve_policy_candidate(member, coordinator, key.into(), digest.into(), now)
    };

    let first = store.enqueue(request("op1", "hello")).unwrap();
    let mut changed = request("op1", "changed");
    changed.target_fqn = "proj:wg-1/other".into();
    let second = store.enqueue(changed).unwrap();

    assert_eq!(first.message_id, second.message_id);
    assert_eq!(first.target_fqn, second.target_fqn);
    assert!(second.duplicate);
    assert_eq!(store.count_by_status(STATUS_QUEUED), 1);
    let key = response_work_key("db", "proj:wg-1/a", "op1");
    assert!(probe(&first.target_fqn, &key, &sha256_hex(b"hello")).is_ok());
    assert!(matches!(
        probe("proj:wg-1/other", &key, &sha256_hex(b"changed")),
        Err(crate::pty::manager::ResponseCloseError::Unknown)
    ));
    store.mark_delivered(&first.message_id, Utc::now()).unwrap();
    assert!(matches!(
        probe(&first.target_fqn, &key, &sha256_hex(b"hello")),
        Err(crate::pty::manager::ResponseCloseError::Conflict)
    ));
    let duplicate = store.enqueue(request("op1", "hello")).unwrap();
    assert!(duplicate.duplicate);
    assert!(matches!(
        probe(&first.target_fqn, &key, &sha256_hex(b"hello")),
        Err(crate::pty::manager::ResponseCloseError::Conflict)
    ));
    let pty_id = Uuid::new_v4().to_string();
    let operation = pty_request(&pty_id, "original");
    let fingerprint = operation.request_fingerprint.clone();
    let target = operation.target_fqn.clone();
    let pty_key = response_work_key("pty", &operation.sender_fqn, &operation.op_id);
    store.enqueue_pty_input(operation).unwrap();
    assert!(probe(&target, &pty_key, &fingerprint).is_ok());
    let mut collision = pty_request(&pty_id, "different");
    collision.request_fingerprint = sha256_hex(b"different");
    assert!(store.enqueue_pty_input(collision).is_err());
    assert!(probe("proj:wg-1-team/dev", &pty_key, &fingerprint).is_ok());
}

#[test]
fn lease_due_hides_active_lease_until_expiry_then_redelivers() {
    let store = store();
    let tracker = ResponseCloseState::default();
    for publisher in [
        crate::pty::manager::ResponseClosePublisher::Manager,
        crate::pty::manager::ResponseClosePublisher::Pty,
        crate::pty::manager::ResponseClosePublisher::RawOutput,
        crate::pty::manager::ResponseClosePublisher::Detector,
        crate::pty::manager::ResponseClosePublisher::Menu,
        crate::pty::manager::ResponseClosePublisher::Hold,
    ] {
        tracker.bind_publisher(publisher);
    }
    tracker.apply_settings(true, 30, std::time::Instant::now());
    store.bind_response_close(tracker.clone());
    assert_eq!(tracker.try_enable_readiness(), Ok(()));
    let probe = |target: &str, key: &str, digest: &str| {
        let now = std::time::Instant::now();
        let member = crate::pty::manager::ResponseRouteIdentity {
            target: target.into(),
            session_id: Uuid::new_v4(),
            token: Uuid::new_v4(),
            generation: 1,
            carrier_version: 1,
            backend: crate::pty::backend::SessionBackendKind::LocalProcess,
        };
        let coordinator = crate::pty::manager::ResponseRouteIdentity {
            target: "proj:wg-1/coordinator".into(),
            session_id: Uuid::new_v4(),
            ..member.clone()
        };
        tracker.register_route(member.session_id, member.generation, now);
        tracker.publish_manager_row(
            member.session_id,
            member.token,
            member.generation,
            member.carrier_version,
            true,
            false,
            now,
        );
        tracker.publish_manager_facts(member.clone(), true, false, now);
        tracker.reserve_policy_candidate(member, coordinator, key.into(), digest.into(), now)
    };

    let first = store.enqueue(request("op1", "hello")).unwrap();
    let now = Utc::now();

    let leased = store
        .lease_due(now, 10, Duration::from_secs(30), "worker")
        .unwrap();
    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].message_id, first.message_id);

    let none = store
        .lease_due(
            now + chrono::Duration::seconds(1),
            10,
            Duration::from_secs(30),
            "worker",
        )
        .unwrap();
    assert!(none.is_empty());

    let again = store
        .lease_due(
            now + chrono::Duration::seconds(31),
            10,
            Duration::from_secs(30),
            "worker",
        )
        .unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].message_id, first.message_id);
    let future = store.enqueue(request("future", "future-body")).unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE messages SET next_attempt_at=?1 WHERE message_id=?2",
            params![
                (now + chrono::Duration::days(1)).to_rfc3339(),
                future.message_id
            ],
        )
        .unwrap();
    let outside = store.enqueue(request("outside", "outside-body")).unwrap();
    let other = store.enqueue(request("other", "other-body")).unwrap();
    let batch = store
        .lease_due(
            now + chrono::Duration::seconds(32),
            1,
            Duration::from_secs(30),
            "limited",
        )
        .unwrap();
    assert_eq!(batch.len(), 1);
    assert_ne!(batch[0].message_id, future.message_id);
    assert!(batch[0].message_id == outside.message_id || batch[0].message_id == other.message_id);
    for (op, body) in [
        ("op1", "hello"),
        ("future", "future-body"),
        ("outside", "outside-body"),
        ("other", "other-body"),
    ] {
        assert!(probe(
            "proj:wg-1/b",
            &response_work_key("db", "proj:wg-1/a", op),
            &sha256_hex(body.as_bytes())
        )
        .is_ok());
    }
}

#[test]
fn delivered_retry_poison_and_reaper_transitions() {
    let store = store();
    let tracker = ResponseCloseState::default();
    for publisher in [
        crate::pty::manager::ResponseClosePublisher::Manager,
        crate::pty::manager::ResponseClosePublisher::Pty,
        crate::pty::manager::ResponseClosePublisher::RawOutput,
        crate::pty::manager::ResponseClosePublisher::Detector,
        crate::pty::manager::ResponseClosePublisher::Menu,
        crate::pty::manager::ResponseClosePublisher::Hold,
    ] {
        tracker.bind_publisher(publisher);
    }
    tracker.apply_settings(true, 30, std::time::Instant::now());
    store.bind_response_close(tracker.clone());
    assert_eq!(tracker.try_enable_readiness(), Ok(()));
    let probe = |target: &str, key: &str, digest: &str| {
        let now = std::time::Instant::now();
        let member = crate::pty::manager::ResponseRouteIdentity {
            target: target.into(),
            session_id: Uuid::new_v4(),
            token: Uuid::new_v4(),
            generation: 1,
            carrier_version: 1,
            backend: crate::pty::backend::SessionBackendKind::LocalProcess,
        };
        let coordinator = crate::pty::manager::ResponseRouteIdentity {
            target: "proj:wg-1/coordinator".into(),
            session_id: Uuid::new_v4(),
            ..member.clone()
        };
        tracker.register_route(member.session_id, member.generation, now);
        tracker.publish_manager_row(
            member.session_id,
            member.token,
            member.generation,
            member.carrier_version,
            true,
            false,
            now,
        );
        tracker.publish_manager_facts(member.clone(), true, false, now);
        tracker.reserve_policy_candidate(member, coordinator, key.into(), digest.into(), now)
    };

    let delivered = store.enqueue(request("op1", "ok")).unwrap();
    let retry = store.enqueue(request("op2", "retry")).unwrap();
    let now = Utc::now();
    let delivered_key = response_work_key("db", "proj:wg-1/a", "op1");
    let retry_key = response_work_key("db", "proj:wg-1/a", "op2");
    assert!(probe(&delivered.target_fqn, &delivered_key, &sha256_hex(b"ok")).is_ok());
    assert!(probe(&retry.target_fqn, &retry_key, &sha256_hex(b"retry")).is_ok());

    store.mark_delivered(&delivered.message_id, now).unwrap();
    assert_eq!(store.count_by_status(STATUS_DELIVERED), 1);

    let status = store
        .mark_delivery_failed(&retry.message_id, "boom", now, 2)
        .unwrap();
    assert_eq!(status, STATUS_RETRY);
    assert!(probe(&retry.target_fqn, &retry_key, &sha256_hex(b"retry")).is_ok());
    let status = store
        .mark_delivery_failed(&retry.message_id, "boom again", now, 2)
        .unwrap();
    assert_eq!(status, STATUS_POISONED);

    let deleted = store
        .reap_terminal_before(now + chrono::Duration::days(2))
        .unwrap();
    assert_eq!(deleted, 2);
    assert!(matches!(
        probe(&delivered.target_fqn, &delivered_key, &sha256_hex(b"ok")),
        Err(crate::pty::manager::ResponseCloseError::Conflict)
    ));
    assert!(matches!(
        probe(&retry.target_fqn, &retry_key, &sha256_hex(b"retry")),
        Err(crate::pty::manager::ResponseCloseError::Conflict)
    ));
    let failed = store.enqueue(request("failure", "retained")).unwrap();
    let failed_key = response_work_key("db", "proj:wg-1/a", "failure");
    assert!(probe(&failed.target_fqn, &failed_key, &sha256_hex(b"retained")).is_ok());
    store.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_terminal BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'controlled failure'); END;").unwrap();
    assert!(store.mark_delivered(&failed.message_id, now).is_err());
    assert_eq!(
        tracker.try_enable_readiness(),
        Err(crate::pty::manager::ResponseCloseError::Unavailable)
    );
    assert_eq!(store.count_by_status(STATUS_QUEUED), 1);
    assert!(store.begin_response_delivery(&failed.target_fqn).is_ok());
}

#[test]
fn oversize_body_rejected() {
    let store = store();
    let too_large = "x".repeat(INLINE_BODY_MAX_BYTES + 1);
    let err = store.enqueue(request("op1", &too_large)).unwrap_err();
    assert!(matches!(err, MessageStoreError::BodyTooLarge));
}

#[test]
fn host_ingress_rejection_is_a_permanent_idempotent_tombstone() {
    let store = store();
    let injection_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let request = || HostPtyInputRejectionRequest {
        injection_id: injection_id.clone(),
        sender_fqn: "proj:wg-1-team/lead".to_string(),
        target_fqn: "proj:wg-1-team/dev".to_string(),
        op_id: injection_id.clone(),
        nonce_sha256: "a".repeat(64),
        request_fingerprint: "b".repeat(64),
        confirmation_tag: "c".repeat(64),
        sender_incarnation_fingerprint: "d".repeat(64),
        payload_sha256: sha256_hex(b"exact text"),
        payload_bytes: 10,
        issued_at: crate::phone::types::canonical_pty_timestamp(now),
        expires_at: crate::phone::types::canonical_pty_timestamp(
            now + chrono::Duration::minutes(10),
        ),
        reason: crate::phone::types::PtyInputReasonCode::InvalidSessionToken,
    };

    let first = store
        .record_host_pty_input_rejection(request(), now)
        .unwrap();
    let duplicate = store
        .record_host_pty_input_rejection(request(), now)
        .unwrap();
    assert_eq!(first, duplicate);
    assert_eq!(
        first.status,
        crate::phone::types::PtyInputPublicStatus::Rejected
    );
    assert_eq!(
        first.reason.as_ref().map(|reason| reason.code),
        Some(crate::phone::types::PtyInputReasonCode::InvalidSessionToken)
    );
    assert!(store
        .due_pty_input_ids(
            crate::phone::types::PtyInputSourcePlane::HostCli,
            now + chrono::Duration::seconds(1),
            64,
        )
        .unwrap()
        .is_empty());
}

#[test]
fn future_skewed_pty_input_is_queued_at_issuance_and_not_claimed_early() {
    let store = store();
    let op_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let issued_at = now + chrono::Duration::seconds(20);
    let mut request = pty_request(&op_id, "exact text");
    request.issued_at = crate::phone::types::canonical_pty_timestamp(issued_at);
    request.expires_at =
        crate::phone::types::canonical_pty_timestamp(issued_at + chrono::Duration::minutes(10));

    store.enqueue_pty_input(request).unwrap();
    assert!(store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&op_id),
            "test-owner",
            now,
        )
        .unwrap()
        .is_none());
    assert!(store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&op_id),
            "test-owner",
            issued_at + chrono::Duration::seconds(1),
        )
        .unwrap()
        .is_some());
}

#[test]
fn pty_input_actuating_clears_payload_and_tombstone_prevents_replay() {
    let store = store();
    let op_id = Uuid::new_v4().to_string();
    let request = pty_request(&op_id, "exact text");
    let first = store.enqueue_pty_input(request).unwrap();
    assert!(!first.duplicate);
    assert_eq!(
        first.result.status,
        crate::phone::types::PtyInputPublicStatus::Queued
    );

    let now = Utc::now();
    let claimed = store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&op_id),
            "lease",
            now,
        )
        .unwrap()
        .unwrap();
    assert_eq!(claimed.attempt, 1);
    let payload = store
        .begin_pty_actuating(
            &op_id,
            "lease",
            &Uuid::new_v4().to_string(),
            "localProcess",
            now,
        )
        .unwrap();
    assert_eq!(payload, b"exact text");
    assert_eq!(
        store
            .query_pty_input_by_injection(&op_id)
            .unwrap()
            .unwrap()
            .status,
        crate::phone::types::PtyInputPublicStatus::Actuating
    );
    let terminal = store
        .terminalize_pty_input(
            &op_id,
            crate::phone::types::PtyInputPublicStatus::Injected,
            None,
            now,
        )
        .unwrap();
    assert!(terminal.terminal);
    assert_eq!(
        store
            .compact_pty_terminal_before(now + chrono::Duration::days(8), 64)
            .unwrap(),
        1
    );
    let tombstone = store.query_pty_input_by_injection(&op_id).unwrap().unwrap();
    assert_eq!(
        tombstone.status,
        crate::phone::types::PtyInputPublicStatus::Injected
    );
    let duplicate = store
        .enqueue_pty_input(pty_request(&op_id, "exact text"))
        .unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.result.injection_id, op_id);
    let conflict = store
        .enqueue_pty_input(pty_request(&op_id, "changed text"))
        .unwrap_err();
    assert!(matches!(conflict, MessageStoreError::IdempotencyConflict));
}

#[test]
fn permanent_incarnation_keeps_get_and_duplicate_stable_while_mutable_authority_is_pinned() {
    let store = store();
    let op_id = Uuid::new_v4().to_string();
    let first = pty_request(&op_id, "exact text");
    let incarnation = first.sender_incarnation_fingerprint.clone();
    let original_authority = first.sender_identity_fingerprint.clone();
    store.enqueue_pty_input(first).unwrap();

    let mut duplicate = pty_request(&op_id, "exact text");
    duplicate.sender_incarnation_fingerprint = incarnation.clone();
    duplicate.sender_identity_fingerprint = "d".repeat(64);
    duplicate.target_identity_fingerprint = "e".repeat(64);
    let duplicate = store.enqueue_pty_input(duplicate).unwrap();
    assert!(duplicate.duplicate);
    assert!(store
        .query_pty_input("proj:wg-1-team/lead", &op_id, &incarnation,)
        .unwrap()
        .is_some());
    assert!(store
        .query_pty_input("proj:wg-1-team/lead", &op_id, &"f".repeat(64),)
        .unwrap()
        .is_none());

    let claimed = store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&op_id),
            "lease",
            Utc::now() + chrono::Duration::seconds(1),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.sender_identity_fingerprint, original_authority,
        "an exact duplicate must not overwrite the queued authority snapshot"
    );
}

#[test]
fn terminal_repetition_requires_identical_status_and_fixed_reason() {
    let store = store();
    let op_id = Uuid::new_v4().to_string();
    store
        .enqueue_pty_input(pty_request(&op_id, "exact text"))
        .unwrap();
    let now = Utc::now();
    store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&op_id),
            "lease",
            now,
        )
        .unwrap()
        .unwrap();
    store
        .begin_pty_actuating(
            &op_id,
            "lease",
            &Uuid::new_v4().to_string(),
            "localProcess",
            now,
        )
        .unwrap();

    let invalid_reason = store
        .terminalize_pty_input(
            &op_id,
            crate::phone::types::PtyInputPublicStatus::Injected,
            Some(crate::phone::types::PtyInputReasonCode::Busy),
            now,
        )
        .unwrap_err();
    assert!(matches!(
        invalid_reason,
        MessageStoreError::InvalidTransition
    ));

    store
        .terminalize_pty_input(
            &op_id,
            crate::phone::types::PtyInputPublicStatus::Injected,
            Some(crate::phone::types::PtyInputReasonCode::RedundantEnterFailed),
            now,
        )
        .unwrap();
    store
        .terminalize_pty_input(
            &op_id,
            crate::phone::types::PtyInputPublicStatus::Injected,
            Some(crate::phone::types::PtyInputReasonCode::RedundantEnterFailed),
            now,
        )
        .unwrap();
    let conflicting_repeat = store
        .terminalize_pty_input(
            &op_id,
            crate::phone::types::PtyInputPublicStatus::Injected,
            None,
            now,
        )
        .unwrap_err();
    assert!(matches!(
        conflicting_repeat,
        MessageStoreError::InvalidTransition
    ));
}
