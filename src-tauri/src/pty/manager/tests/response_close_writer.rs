use super::*;

/// Includes store execute-failure and late-bind dispatcher recheck only;
/// the latter is neither valid startup order nor a production commit/admission race.
#[tokio::test]
async fn response_close_waiting_writer_is_counted_and_cancellation_releases_only_its_admission() {
    use std::future::Future;
    let id = Uuid::new_v4();
    let backend = Arc::new(RecordingBackend::default());
    let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    manager
        .lock()
        .unwrap()
        .try_record_route(id, SessionBackendKind::LocalProcess)
        .unwrap();
    let state = manager.lock().unwrap().response_close_state();
    let first = PtyManager::acquire_input_writer(&manager, id)
        .await
        .unwrap();
    let mut waiting = Box::pin(PtyManager::acquire_input_writer(&manager, id));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(waiting.as_mut().poll(&mut context).is_pending());
    assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 2);
    drop(waiting);
    assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 1);
    assert!(backend.calls().is_empty());
    PtyManager::write_with_permit(&first, b"first").unwrap();
    drop(first);
    assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 0);
    assert_eq!(backend.calls(), vec![Call::Write(id, b"first".to_vec())]);
    // Integrated store execute-failure oracle, not the SQL commit-failure branch.
    use crate::api::message_store::{EnqueueRequest, MessageStore};
    for fail_execute in [false, true] {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("store-oracle.sqlite3");
        let store = MessageStore::open(path.clone()).unwrap();
        let tracker = ResponseCloseState::default();
        store.bind_response_close(tracker.clone());
        let row = store
            .enqueue(EnqueueRequest {
                sender_fqn: "project:room/sender".into(),
                target_fqn: "project:room/member".into(),
                op_id: "store-oracle".into(),
                content_type: crate::api::message_store::DEFAULT_CONTENT_TYPE.into(),
                body: "durable-body".into(),
                source_plane: "api_inline".into(),
                source_ref: None,
            })
            .unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let snapshot = || {
            connection
                .query_row(
                    "SELECT message_id,target_fqn,status,body,body_sha256,\
                     attempt,lease_owner,lease_until FROM messages \
                     WHERE message_id=?1",
                    [&row.message_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, Option<String>>(6)?,
                            r.get::<_, Option<String>>(7)?,
                        ))
                    },
                )
                .unwrap()
        };
        let before = snapshot();
        let key = format!(
            "db:{}:{}:{}",
            "project:room/sender".len(),
            "project:room/sender",
            "store-oracle"
        );
        assert_eq!(
            tracker.inner.lock().unwrap().work[&row.target_fqn].pending[&key],
            before.4
        );
        let delivery = tracker.begin_delivery(&row.target_fqn).unwrap();
        if fail_execute {
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_terminal BEFORE UPDATE ON \
                     messages BEGIN SELECT RAISE(ABORT, 'controlled \
                     execute failure'); END;",
                )
                .unwrap();
            assert!(store
                .mark_delivered(&row.message_id, chrono::Utc::now())
                .is_err());
            assert_eq!(snapshot(), before);
            assert_eq!(before.2, "queued");
            tracker.settle_terminal(&row.target_fqn, &key, std::time::Instant::now());
            drop(delivery);
            let inner = tracker.inner.lock().unwrap();
            assert!(inner.failed);
            assert_eq!(inner.work[&row.target_fqn].pending[&key], before.4);
            assert!(!inner.work[&row.target_fqn].terminal.contains_key(&key));
        } else {
            store
                .mark_delivered(&row.message_id, chrono::Utc::now())
                .unwrap();
            drop(delivery);
            assert_eq!(snapshot().2, "delivered");
            let inner = tracker.inner.lock().unwrap();
            assert!(!inner.failed);
            assert!(!inner.work[&row.target_fqn].pending.contains_key(&key));
            assert_eq!(inner.work[&row.target_fqn].terminal[&key], before.4);
        }
    }

    // C: PTY terminal persistence SQL failure before commit, not commit ambiguity.
    for fail_execute in [false, true] {
        use crate::phone::types::{PtyInputPublicStatus, PtyInputReasonCode, PtyInputSourcePlane};
        assert!(crate::phone::types::pty_input_reason_allowed_for_status(
            PtyInputPublicStatus::Rejected,
            Some(PtyInputReasonCode::Expired)
        ));
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("pty-terminal-oracle.sqlite3");
        let store = MessageStore::open(path.clone()).unwrap();
        let tracker = ResponseCloseState::default();
        store.bind_response_close(tracker.clone());
        let id = Uuid::new_v4().to_string();
        let issued = chrono::Utc::now();
        store
            .enqueue_pty_input(crate::api::message_store::PtyInputEnqueueRequest {
                injection_id: id.clone(),
                sender_fqn: "proj:wg-1-team/lead".into(),
                target_fqn: "proj:wg-1-team/dev".into(),
                op_id: id.clone(),
                nonce_sha256: "a".repeat(64),
                request_fingerprint: crate::phone::types::sha256_hex(b"pty-terminal-body"),
                confirmation_tag: Some("b".repeat(64)),
                requested_agent_id: None,
                payload: b"pty-terminal-body".to_vec(),
                source_plane: PtyInputSourcePlane::HostCli,
                sender_incarnation_fingerprint: "c".repeat(64),
                sender_identity_fingerprint: "d".repeat(64),
                target_identity_fingerprint: "e".repeat(64),
                authority_session_id: Uuid::new_v4().to_string(),
                authority_client_id: None,
                authority_client_generation: None,
                issued_at: crate::phone::types::canonical_pty_timestamp(issued),
                expires_at: crate::phone::types::canonical_pty_timestamp(
                    issued + chrono::Duration::minutes(10),
                ),
            })
            .unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let snapshot = || {
            connection
                .query_row(
                    "SELECT injection_id,sender_fqn,target_fqn,op_id,\
                     request_fingerprint,status,payload,attempt,lease_owner,\
                     lease_until,updated_at FROM pty_input_operations \
                     WHERE injection_id=?1",
                    [&id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, Option<Vec<u8>>>(6)?,
                            r.get::<_, i64>(7)?,
                            r.get::<_, Option<String>>(8)?,
                            r.get::<_, Option<String>>(9)?,
                            r.get::<_, String>(10)?,
                        ))
                    },
                )
                .unwrap()
        };
        let before = snapshot();
        let key = format!("pty:{}:{}:{}", before.1.len(), before.1, before.3);
        assert_eq!(before.5, "queued");
        assert_eq!(before.6.as_deref(), Some(b"pty-terminal-body".as_slice()));
        assert_eq!(
            tracker.inner.lock().unwrap().work[&before.2].pending[&key],
            before.4
        );
        if fail_execute {
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_pty_terminal BEFORE UPDATE ON pty_input_operations \
                     BEGIN SELECT RAISE(ABORT, 'controlled precommit SQL failure'); END;",
                )
                .unwrap();
            assert!(matches!(
                store.terminalize_pty_input(
                    &id,
                    PtyInputPublicStatus::Rejected,
                    Some(PtyInputReasonCode::Expired),
                    chrono::Utc::now()
                ),
                Err(crate::api::message_store::MessageStoreError::Sqlite(_))
            ));
            assert_eq!(snapshot(), before);
            let inner = tracker.inner.lock().unwrap();
            assert!(inner.failed);
            assert_eq!(inner.work[&before.2].pending[&key], before.4);
            assert!(!inner.work[&before.2].terminal.contains_key(&key));
            let tombstones: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM pty_input_tombstones WHERE injection_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(tombstones, 0);
        } else {
            let result = store
                .terminalize_pty_input(
                    &id,
                    PtyInputPublicStatus::Rejected,
                    Some(PtyInputReasonCode::Expired),
                    chrono::Utc::now(),
                )
                .unwrap();
            assert_eq!(result.status, PtyInputPublicStatus::Rejected);
            assert!(result.terminal);
            assert_eq!(result.reason.unwrap().code, PtyInputReasonCode::Expired);
            let tombstone: (String, String) = connection
                .query_row(
                    "SELECT status,request_fingerprint FROM pty_input_tombstones \
                     WHERE injection_id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(tombstone, ("rejected".into(), before.4.clone()));
            let inner = tracker.inner.lock().unwrap();
            assert!(!inner.failed);
            assert!(!inner.work[&before.2].pending.contains_key(&key));
            assert_eq!(inner.work[&before.2].terminal[&key], before.4);
        }
    }

    // Late-bind recheck only: not valid startup order or a production commit/admission race.
    use crate::api::dispatcher::{dispatch_due_with, DispatcherConfig};
    for closing in [false, true] {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("late-bind-recheck.sqlite3");
        let store = MessageStore::open(path.clone()).unwrap();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let tracker = manager.lock().unwrap().response_close_state();
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
        let now = std::time::Instant::now() + Duration::from_secs(3600);
        tracker.apply_settings(true, 30, now);
        let member_id = Uuid::new_v4();
        let coordinator_id = Uuid::new_v4();
        for id in [member_id, coordinator_id] {
            manager
                .lock()
                .unwrap()
                .try_record_route(id, SessionBackendKind::LocalProcess)
                .unwrap();
        }
        let member = ResponseRouteIdentity {
            target: "project:room/member".into(),
            session_id: member_id,
            token: Uuid::new_v4(),
            generation: tracker.generation(member_id).unwrap(),
            carrier_version: 1,
            backend: SessionBackendKind::LocalProcess,
        };
        let coordinator = ResponseRouteIdentity {
            target: "project:room/coordinator".into(),
            session_id: coordinator_id,
            token: Uuid::new_v4(),
            generation: tracker.generation(coordinator_id).unwrap(),
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
        tracker.observe_detector(member_id, false, now);
        tracker.publish_menu(member_id, Some(false), now);
        tracker.publish_hold(member_id, None, true, now);
        let mut admission = tracker
            .reserve_work(&member.target, "policy-request")
            .unwrap();
        admission.accept_durable("policy-body").unwrap();
        drop(admission);
        let context = tracker
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                "policy-request".into(),
                "policy-body".into(),
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
            .admit_return(&member, &coordinator, "ack".into(), "ack-body".into(), now)
            .unwrap();
        tracker
            .observe_return_success(ReturnDeliveryProof { context }, now)
            .unwrap();
        let at = now + Duration::from_secs(30);
        let ticket = tracker.select_ticket(member_id, at).unwrap();
        if closing {
            PtyManager::authority_route_proof(&manager, member_id)
                .unwrap()
                .try_commit_response_close(&tracker, &ticket, at)
                .unwrap();
        }
        let enqueue = |op: &str, target: &str, body: &str| {
            store
                .enqueue(EnqueueRequest {
                    sender_fqn: "project:room/sender".into(),
                    target_fqn: target.into(),
                    op_id: op.into(),
                    content_type: crate::api::message_store::DEFAULT_CONTENT_TYPE.into(),
                    body: body.into(),
                    source_plane: "api_inline".into(),
                    source_ref: None,
                })
                .unwrap()
        };
        let first = enqueue("first", &coordinator.target, "first-body");
        let second = enqueue("second", &member.target, "second-body");
        let connection = rusqlite::Connection::open(&path).unwrap();
        // Explicit stable order without clock sleeps or timestamp collisions.
        connection
            .execute(
                "UPDATE messages SET created_at='2000-01-01T00:00:00Z' WHERE message_id=?1",
                [&first.message_id],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE messages SET created_at='2000-01-01T00:00:01Z' WHERE message_id=?1",
                [&second.message_id],
            )
            .unwrap();
        let snapshot = |id: &str| {
            connection
                .query_row(
                    "SELECT message_id,target_fqn,status,body,body_sha256,\
                     attempt,lease_owner,lease_until,next_attempt_at,\
                     sender_fqn,last_error FROM messages WHERE message_id=?1",
                    [id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, Option<String>>(6)?,
                            r.get::<_, Option<String>>(7)?,
                            r.get::<_, String>(8)?,
                            r.get::<_, String>(9)?,
                            r.get::<_, Option<String>>(10)?,
                        ))
                    },
                )
                .unwrap()
        };
        let before = snapshot(&second.message_id);
        let first_key = format!(
            "db:{}:{}:first",
            "project:room/sender".len(),
            "project:room/sender"
        );
        let second_key = format!(
            "db:{}:{}:second",
            "project:room/sender".len(),
            "project:room/sender"
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let copy = store.clone();
        let bound = tracker.clone();
        let first_id = first.message_id.clone();
        let dispatch_before = chrono::Utc::now();
        let processed = dispatch_due_with(
            &store,
            chrono::Utc::now(),
            &DispatcherConfig::default(),
            move |message| {
                recorded.lock().unwrap().push(message.id.clone());
                if message.id == first_id {
                    copy.bind_response_close(bound.clone());
                    let inner = bound.inner.lock().unwrap();
                    assert!(!inner.failed);
                    assert!(inner.work_bootstrapped);
                    assert_eq!(
                        inner.work["project:room/coordinator"].pending[&first_key],
                        crate::phone::types::sha256_hex(b"first-body")
                    );
                    assert_eq!(
                        inner.work["project:room/member"].pending[&second_key],
                        crate::phone::types::sha256_hex(b"second-body")
                    );
                }
                async { Ok(()) }
            },
        )
        .await
        .unwrap();
        let dispatch_after = chrono::Utc::now();
        assert_eq!(processed, 2);
        assert_eq!(snapshot(&first.message_id).2, "delivered");
        let after = snapshot(&second.message_id);
        if closing {
            assert_eq!(*calls.lock().unwrap(), vec![first.message_id.clone()]);
            assert_eq!(after.2, "queued");
            assert_eq!(after.9, before.9);
            assert_eq!(
                after.10.as_deref(),
                Some(crate::api::message_store::RESPONSE_CLOSING_DEFERRED)
            );
            let due = chrono::DateTime::parse_from_rfc3339(&after.8)
                .unwrap()
                .with_timezone(&chrono::Utc);
            assert!(dispatch_before <= due && due <= dispatch_after);
            let audit: (String, Option<String>) = connection
                .query_row(
                    "SELECT status,detail FROM message_audit WHERE \
                     message_id=?1 AND status='lease-released-deferred'",
                    [&second.message_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(audit.0, "lease-released-deferred");
            assert_eq!(
                audit.1.as_deref(),
                Some(crate::api::message_store::RESPONSE_CLOSING_DEFERRED)
            );
            assert_eq!(
                (after.0, after.1, after.3, after.4, after.5, after.6, after.7),
                (before.0, before.1, before.3, before.4, before.5, None, None)
            );
            assert!(tracker.target_is_closing(&member.target));
            assert_eq!(
                tracker.inner.lock().unwrap().work[&member.target].pending[&format!(
                    "db:{}:{}:second",
                    "project:room/sender".len(),
                    "project:room/sender"
                )],
                crate::phone::types::sha256_hex(b"second-body")
            );
        } else {
            assert_eq!(
                *calls.lock().unwrap(),
                vec![first.message_id.clone(), second.message_id.clone()]
            );
            assert_eq!(after.2, "delivered");
        }
        assert!(backend.calls().is_empty());
        // Separate bound-from-start SQL Closing filter, same positive twin.
        let third = enqueue("bound-filter", &member.target, "filter-body");
        let before = snapshot(&third.message_id);
        let rows = store
            .lease_due(chrono::Utc::now(), 10, Duration::from_secs(30), "filter")
            .unwrap();
        if closing {
            assert!(rows.is_empty());
            assert_eq!(snapshot(&third.message_id), before);
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].message_id, third.message_id);
            assert_eq!(rows[0].body, "filter-body");
        }
    }
}

#[test]
fn writer_closing_query_preserves_policy() {
    for failed in [false, true] {
        for poison in [false, true] {
            let state = ResponseCloseState::default();
            let id = Uuid::new_v4();
            let now = std::time::Instant::now();
            state.register_route(id, 7, now);
            if failed {
                state.fail_closed();
            }
            let writer = state.reserve_writer(id, 7).unwrap();
            assert_eq!(writer.counted, !failed);
            assert_eq!(writer.is_closing(), Ok(false));
            state
                .inner
                .lock()
                .unwrap()
                .routes
                .get_mut(&id)
                .unwrap()
                .closing = Some(Uuid::new_v4());
            assert_eq!(writer.is_closing(), Ok(true));
            if poison {
                let copy = state.clone();
                assert!(std::thread::spawn(move || {
                    let _guard = copy.inner.lock().unwrap();
                    panic!("poison tracker query control");
                })
                .join()
                .is_err());
                assert_eq!(writer.is_closing(), Ok(true));
            }
            {
                let mut inner = state
                    .inner
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let route = inner.routes.get_mut(&id).unwrap();
                route.generation += 1;
                route.closing = None;
            }
            let expected = if failed {
                Ok(false)
            } else {
                Err(ResponseCloseError::Unknown)
            };
            assert_eq!(writer.is_closing(), expected);
            assert_eq!(writer.is_closing().unwrap_or(true), !failed);
        }
    }
}
