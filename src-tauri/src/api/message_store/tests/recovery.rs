use super::*;

#[tokio::test]
async fn exact_target_lock_entry_is_removed_after_last_reservation() {
    let locks = Arc::new(PtyInputTargetLocks::default());
    let first = locks.acquire("proj:wg-1-team/dev").await;
    assert_eq!(locks.entry_count(), 1);
    let locks_for_waiter = Arc::clone(&locks);
    let waiter = tokio::spawn(async move { locks_for_waiter.acquire("proj:wg-1-team/dev").await });
    tokio::task::yield_now().await;
    drop(first);
    let second = waiter.await.unwrap();
    assert_eq!(locks.entry_count(), 1);
    drop(second);
    assert_eq!(locks.entry_count(), 0);
}

#[tokio::test]
async fn cancelled_target_waiter_releases_its_reservation() {
    let locks = Arc::new(PtyInputTargetLocks::default());
    let held = locks.acquire("proj:wg-1-team/dev").await;
    let locks_for_waiter = Arc::clone(&locks);
    let waiter = tokio::spawn(async move { locks_for_waiter.acquire("proj:wg-1-team/dev").await });
    tokio::task::yield_now().await;

    waiter.abort();
    let cancelled = waiter.await;
    assert!(matches!(cancelled, Err(error) if error.is_cancelled()));
    assert_eq!(
        locks.entry_count(),
        1,
        "the holder reservation remains, but the cancelled waiter must be gone"
    );

    drop(held);
    assert_eq!(locks.entry_count(), 0);
}

#[test]
fn operation_stripes_are_stable_and_exclusive() {
    let store = store();
    let id = Uuid::new_v4().to_string();
    let first = store.try_operation_lock(&id).unwrap().unwrap();
    assert!(store.try_operation_lock(&id).unwrap().is_none());
    drop(first);
    assert!(store.try_operation_lock(&id).unwrap().is_some());
}

#[test]
fn startup_recovery_marks_expired_actuating_indeterminate() {
    let store = store();
    let (id, _) = enqueue_claim_actuating(&store, 98);
    let now = Utc::now();
    let issued = crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(20));
    let queued = crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(19));
    let preparing =
        crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(18));
    let actuating =
        crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(17));
    let expires = crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(10));
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            r#"UPDATE pty_input_operations
               SET issued_at=?1,expires_at=?2,queued_at=?3,preparing_at=?4,
                   actuating_at=?5,next_attempt_at=?3,updated_at=?5
               WHERE injection_id=?6"#,
            params![issued, expires, queued, preparing, actuating, id],
        )
        .unwrap();

    store.recover_pty_input_startup().unwrap();
    let result = store.query_pty_input_by_injection(&id).unwrap().unwrap();
    assert_eq!(
        result.status,
        crate::phone::types::PtyInputPublicStatus::Indeterminate
    );
    assert_eq!(
        result.reason.unwrap().code,
        crate::phone::types::PtyInputReasonCode::DaemonRestartAfterActuation
    );
}

#[test]
fn ambiguous_actuation_commit_returns_no_payload_and_preserves_no_replay_state() {
    let store = store();
    // ACTUATING is nonterminal: this tests pending/no replay, not a failure latch.
    let tracker = ResponseCloseState::default();
    store.bind_response_close(tracker.clone());
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
    tracker.try_enable_readiness().unwrap();
    let policy_now = std::time::Instant::now();
    tracker.apply_settings(true, 30, policy_now);
    let member = crate::pty::manager::ResponseRouteIdentity {
        target: "proj:wg-1-team/dev".into(),
        session_id: Uuid::new_v4(),
        token: Uuid::new_v4(),
        generation: 1,
        carrier_version: 1,
        backend: crate::pty::backend::SessionBackendKind::LocalProcess,
    };
    let coordinator = crate::pty::manager::ResponseRouteIdentity {
        target: "proj:wg-1-team/lead".into(),
        session_id: Uuid::new_v4(),
        token: Uuid::new_v4(),
        generation: 2,
        ..member.clone()
    };
    for (identity, eligible) in [(&member, true), (&coordinator, false)] {
        tracker.register_route(identity.session_id, identity.generation, policy_now);
        tracker.publish_manager_row(
            identity.session_id,
            identity.token,
            identity.generation,
            1,
            eligible,
            false,
            policy_now,
        );
        tracker.publish_manager_facts(identity.clone(), eligible, false, policy_now);
    }
    let id = Uuid::new_v4().to_string();
    let request = pty_request(&id, "secret text");
    let key = response_work_key("pty", &request.sender_fqn, &request.op_id);
    let digest = request.request_fingerprint.clone();
    store.enqueue_pty_input(request).unwrap();
    assert!(tracker
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            key.clone(),
            digest.clone(),
            policy_now
        )
        .is_ok());
    let now = Utc::now();
    store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&id),
            "lease",
            now,
        )
        .unwrap()
        .unwrap();
    store.inject_actuation_commit_ambiguous();
    let error = store
        .begin_pty_actuating(
            &id,
            "lease",
            &Uuid::new_v4().to_string(),
            "containerTransport",
            now,
        )
        .unwrap_err();
    assert!(matches!(error, MessageStoreError::ActuationCommitAmbiguous));
    let result = store.query_pty_input_by_injection(&id).unwrap().unwrap();
    assert_eq!(
        result.status,
        crate::phone::types::PtyInputPublicStatus::Actuating
    );
    let payload_is_null: bool = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT payload IS NULL FROM pty_input_operations WHERE injection_id=?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(payload_is_null);
    // Exact same healthy state/identities/key/digest, without re-enqueue or accept_durable.
    assert!(tracker
        .reserve_policy_candidate(member, coordinator, key, digest, std::time::Instant::now())
        .is_ok());
    assert!(store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&id),
            "replay",
            Utc::now()
        )
        .unwrap()
        .is_none());
    assert!(matches!(
        store.begin_pty_actuating(
            &id,
            "lease",
            &Uuid::new_v4().to_string(),
            "containerTransport",
            Utc::now()
        ),
        Err(MessageStoreError::InvalidTransition)
    ));
}

#[test]
fn runtime_recovery_requeues_expired_preparation_and_terminalizes_attempt_five() {
    let store = store();
    let retry_id = Uuid::new_v4().to_string();
    let final_id = Uuid::new_v4().to_string();
    for id in [&retry_id, &final_id] {
        store
            .enqueue_pty_input(pty_request(id, "exact text"))
            .unwrap();
    }
    let recovery_at = Utc::now() + chrono::Duration::seconds(1);
    for id in [&retry_id, &final_id] {
        store
            .claim_pty_input(
                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                Some(id),
                &format!("lease-{id}"),
                recovery_at,
            )
            .unwrap()
            .unwrap();
    }
    let expired =
        crate::phone::types::canonical_pty_timestamp(recovery_at - chrono::Duration::seconds(1));
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE pty_input_operations SET lease_until=?1,attempt=5 WHERE injection_id=?2",
            params![expired, final_id],
        )
        .unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE pty_input_operations SET lease_until=?1 WHERE injection_id=?2",
            params![expired, retry_id],
        )
        .unwrap();

    assert_eq!(
        store
            .recover_pty_input_runtime(&HashSet::new(), recovery_at)
            .unwrap(),
        2
    );
    let retry = store
        .query_pty_input_by_injection(&retry_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        retry.status,
        crate::phone::types::PtyInputPublicStatus::Queued
    );
    assert_eq!(
        retry.reason.unwrap().code,
        crate::phone::types::PtyInputReasonCode::LeaseLost
    );
    let final_result = store
        .query_pty_input_by_injection(&final_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        final_result.status,
        crate::phone::types::PtyInputPublicStatus::Rejected
    );
    assert_eq!(
        final_result.reason.unwrap().code,
        crate::phone::types::PtyInputReasonCode::LeaseLost
    );
}

#[test]
fn runtime_orphan_recovery_waits_for_local_and_cross_process_ownership() {
    let store = store();
    let (id, actuating_at) = enqueue_claim_actuating(&store, 0);
    let recovery_now = actuating_at + chrono::Duration::seconds(16);
    let mut active = HashSet::new();
    active.insert(id.clone());
    assert_eq!(
        store
            .recover_pty_input_runtime(&active, recovery_now)
            .unwrap(),
        0
    );
    let operation_lock = store.try_operation_lock(&id).unwrap().unwrap();
    assert_eq!(
        store
            .recover_pty_input_runtime(&HashSet::new(), recovery_now)
            .unwrap(),
        0
    );
    drop(operation_lock);
    assert_eq!(
        store
            .recover_pty_input_runtime(&HashSet::new(), recovery_now)
            .unwrap(),
        1
    );
    let result = store.query_pty_input_by_injection(&id).unwrap().unwrap();
    assert_eq!(
        result.status,
        crate::phone::types::PtyInputPublicStatus::Indeterminate
    );
    assert_eq!(
        result.reason.unwrap().code,
        crate::phone::types::PtyInputReasonCode::RuntimeActuationOrphan
    );
}

#[test]
fn admission_expiry_advances_past_sixty_four_locked_rows() {
    let store = store();
    let now = Utc::now();
    let mut ids = Vec::new();
    for index in 0..65 {
        let id = Uuid::new_v4().to_string();
        let mut request = pty_request(&id, "exact text");
        request.sender_fqn = format!("proj:wg-1-team/lead-{index}");
        request.request_fingerprint = sha256_hex(format!("request:{index}").as_bytes());
        store.enqueue_pty_input(request).unwrap();
        ids.push(id);
    }
    let stripe = |id: &str| {
        let digest = Sha256::digest(id.as_bytes());
        ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
            % PTY_INPUT_OPERATION_LOCK_STRIPES
    };
    let tail = ids
        .iter()
        .find(|candidate| {
            ids.iter()
                .filter(|other| stripe(other) == stripe(candidate))
                .count()
                == 1
        })
        .expect("fixture includes a unique operation stripe")
        .clone();
    let issued = crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(20));
    let queued = crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(19));
    let expired = crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(10));
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE pty_input_operations SET \
                issued_at=?1,queued_at=?2,expires_at=?3,next_attempt_at=?2,updated_at=?2",
            params![issued, queued, expired],
        )
        .unwrap();
    let tail_issued =
        crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(19));
    let tail_queued =
        crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(18));
    let tail_expired =
        crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::minutes(9));
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE pty_input_operations SET \
                issued_at=?1,queued_at=?2,expires_at=?3,next_attempt_at=?2,updated_at=?2 WHERE \
                injection_id=?4",
            params![tail_issued, tail_queued, tail_expired, tail],
        )
        .unwrap();
        conn.execute(
            "UPDATE pty_input_tombstones SET issued_at=?1,queued_at=?2,expires_at=?3 WHERE \
                injection_id=?4",
            params![tail_issued, tail_queued, tail_expired, tail],
        )
        .unwrap();
    }
    let ordered = {
        let conn = store.conn.lock().unwrap();
        let mut statement = conn
            .prepare(
                "SELECT injection_id FROM pty_input_operations ORDER BY expires_at,injection_id",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(ordered[64], tail);
    let locks = ordered[..64]
        .iter()
        .filter_map(|id| store.try_operation_lock(id).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(store.expire_pty_input_for_admission(now).unwrap(), 0);
    assert!(store.query_pty_input_by_injection(&tail).unwrap().is_some());
    assert_eq!(
        store.expire_pty_input_for_admission(now).unwrap(),
        1,
        "a fair admission cursor must reach the independent 65th row"
    );
    drop(locks);
    assert_eq!(
        store.expire_pty_input_for_admission(now).unwrap(),
        64,
        "released prefix rows are revisited immediately after cursor wrap"
    );
}

#[test]
fn runtime_recovery_advances_past_sixty_four_active_rows() {
    let store = store();
    let mut ids = Vec::new();
    let mut latest_transition = Utc::now();
    for index in 0..65 {
        let (id, transition) = enqueue_claim_actuating(&store, index / 16);
        latest_transition = latest_transition.max(transition);
        ids.push(id);
    }
    let tail = ids.pop().unwrap();
    let latest = crate::phone::types::canonical_pty_timestamp(
        latest_transition + chrono::Duration::seconds(1),
    );
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE pty_input_operations SET updated_at=?1,actuating_at=?1 WHERE injection_id=?2",
            params![latest, tail],
        )
        .unwrap();
    let active = ids.into_iter().collect::<HashSet<_>>();
    let recovery_at = latest_transition + chrono::Duration::seconds(20);

    assert_eq!(
        store
            .recover_pty_input_runtime(&active, recovery_at)
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .recover_pty_input_runtime(&active, recovery_at)
            .unwrap(),
        1,
        "a fair runtime cursor must reach the independent 65th orphan"
    );
    let result = store.query_pty_input_by_injection(&tail).unwrap().unwrap();
    assert_eq!(
        result.reason.map(|reason| reason.code),
        Some(crate::phone::types::PtyInputReasonCode::RuntimeActuationOrphan)
    );
    assert_eq!(
        store
            .recover_pty_input_runtime(&active, recovery_at)
            .unwrap(),
        0,
        "active prefix rows remain skipped after the cursor wraps"
    );
    assert_eq!(
        store
            .recover_pty_input_runtime(&HashSet::new(), recovery_at)
            .unwrap(),
        64,
        "formerly active prefix rows are revisited after cursor wrap"
    );
}

#[test]
fn due_container_cursor_advances_past_sixty_four_lock_contended_rows() {
    let store = store();
    let mut ids = Vec::new();
    for index in 0..65 {
        let id = Uuid::new_v4().to_string();
        let mut request = pty_request(&id, "exact text");
        request.sender_fqn = format!("proj:wg-1-team/lead-{index}");
        request.target_fqn = format!("proj:wg-1-team/dev-{index}");
        request.request_fingerprint = sha256_hex(format!("request:{index}").as_bytes());
        store.enqueue_pty_input(request).unwrap();
        ids.push(id);
    }
    let stripe = |id: &str| {
        let digest = Sha256::digest(id.as_bytes());
        ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
            % PTY_INPUT_OPERATION_LOCK_STRIPES
    };
    let tail = ids
        .iter()
        .find(|candidate| {
            ids.iter()
                .filter(|other| stripe(other) == stripe(candidate))
                .count()
                == 1
        })
        .unwrap()
        .clone();
    let base_at = Utc::now() + chrono::Duration::seconds(1);
    let base = crate::phone::types::canonical_pty_timestamp(base_at);
    let tail_next =
        crate::phone::types::canonical_pty_timestamp(base_at + chrono::Duration::milliseconds(1));
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE pty_input_operations SET queued_at=?1,next_attempt_at=?1,updated_at=?1",
            [&base],
        )
        .unwrap();
        conn.execute(
            "UPDATE pty_input_operations SET next_attempt_at=?1,updated_at=?1 WHERE \
                injection_id=?2",
            params![tail_next, tail],
        )
        .unwrap();
    }
    let dispatch_at = base_at + chrono::Duration::seconds(1);
    let first_page = store
        .due_container_pty_input_candidates_fair(dispatch_at, 64)
        .unwrap();
    assert_eq!(first_page.len(), 64);
    assert!(first_page
        .iter()
        .all(|candidate| candidate.injection_id != tail));
    let locks = first_page
        .iter()
        .filter_map(|candidate| store.try_operation_lock(&candidate.injection_id).unwrap())
        .collect::<Vec<_>>();
    assert!(first_page.iter().all(|candidate| store
        .try_operation_lock(&candidate.injection_id)
        .unwrap()
        .is_none()));

    let second_page = store
        .due_container_pty_input_candidates_fair(dispatch_at, 64)
        .unwrap();
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].injection_id, tail);
    assert!(store.try_operation_lock(&tail).unwrap().is_some());
    drop(locks);
}

#[test]
fn startup_recovery_pages_past_sixty_four_locked_rows() {
    let store = store();
    let mut ids = Vec::new();
    for index in 0..65 {
        ids.push(enqueue_claim_actuating(&store, index / 16).0);
    }
    let stripe = |id: &str| {
        let digest = Sha256::digest(id.as_bytes());
        ((usize::from(digest[0]) << 4) | (usize::from(digest[1]) >> 4))
            % PTY_INPUT_OPERATION_LOCK_STRIPES
    };
    let unlocked = ids
        .iter()
        .find(|candidate| {
            let candidate_stripe = stripe(candidate);
            ids.iter()
                .filter(|other| stripe(other) == candidate_stripe)
                .count()
                == 1
        })
        .expect("65 UUIDs must include an operation stripe without a collision")
        .clone();
    let latest =
        crate::phone::types::canonical_pty_timestamp(Utc::now() + chrono::Duration::hours(1));
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE pty_input_operations SET updated_at=?1 WHERE injection_id=?2",
            params![latest, unlocked],
        )
        .unwrap();
    let ordered = {
        let conn = store.conn.lock().unwrap();
        let mut statement = conn
            .prepare(
                "SELECT injection_id FROM pty_input_operations ORDER BY updated_at,injection_id",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(ordered[64], unlocked);
    let locks = ordered[..64]
        .iter()
        .filter_map(|id| store.try_operation_lock(id).unwrap())
        .collect::<Vec<_>>();
    store.recover_pty_input_startup().unwrap();
    for id in &ordered[..64] {
        assert_eq!(
            store
                .query_pty_input_by_injection(id)
                .unwrap()
                .unwrap()
                .status,
            crate::phone::types::PtyInputPublicStatus::Actuating
        );
    }
    assert_eq!(
        store
            .query_pty_input_by_injection(&ordered[64])
            .unwrap()
            .unwrap()
            .status,
        crate::phone::types::PtyInputPublicStatus::Indeterminate
    );
    drop(locks);
    assert_eq!(ids.len(), 65);
}

#[test]
fn startup_recovery_terminalizes_a_fifth_preparation_attempt() {
    let store = store();
    let id = Uuid::new_v4().to_string();
    store
        .enqueue_pty_input(pty_request(&id, "exact text"))
        .unwrap();
    store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&id),
            "lease",
            Utc::now() + chrono::Duration::seconds(1),
        )
        .unwrap()
        .unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE pty_input_operations SET attempt=5 WHERE injection_id=?1",
            [&id],
        )
        .unwrap();
    store.recover_pty_input_startup().unwrap();
    let result = store.query_pty_input_by_injection(&id).unwrap().unwrap();
    assert_eq!(
        result.status,
        crate::phone::types::PtyInputPublicStatus::Rejected
    );
    assert_eq!(
        result.reason.unwrap().code,
        crate::phone::types::PtyInputReasonCode::LeaseLost
    );
}
