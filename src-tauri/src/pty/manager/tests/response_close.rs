use super::*;
use crate::pty::manager::response_close::StaleOrInvalidSettlement;
use std::time::{Duration, Instant};
mod ordinary_ack;

#[test]
fn response_close_readiness_requires_all_publishers_and_work_bootstrap() {
    let state = ResponseCloseState::default();
    assert!(state.is_healthy());
    assert_eq!(
        state.try_enable_readiness(),
        Err(ResponseCloseError::Unavailable)
    );
    for publisher in [
        ResponseClosePublisher::Manager,
        ResponseClosePublisher::Pty,
        ResponseClosePublisher::RawOutput,
        ResponseClosePublisher::Detector,
        ResponseClosePublisher::Menu,
        ResponseClosePublisher::Hold,
    ] {
        state.bind_publisher(publisher);
        assert!(state.is_healthy());
    }
    assert!(state.publishers_bound());
    assert!(state.is_healthy());
    assert_eq!(
        state.try_enable_readiness(),
        Err(ResponseCloseError::Unavailable)
    );
    state.mark_work_bootstrapped();
    assert_eq!(state.try_enable_readiness(), Ok(()));
    assert!(state.is_healthy());
    state.apply_settings(true, 30, Instant::now());
    assert!(state.is_healthy());
    state.apply_settings(false, 30, Instant::now());
    assert!(state.is_healthy());
    state.fail_closed();
    assert!(!state.is_healthy());
    assert_eq!(
        state.try_enable_readiness(),
        Err(ResponseCloseError::Unavailable)
    );
    let other = ResponseCloseState::default();
    assert!(!state.same_state(&other));
    assert!(state.same_state(&state.clone()));

    let state = ResponseCloseState::default();
    let poisoned = state.clone();
    let joined = std::thread::spawn(move || {
        let _guard = poisoned.inner.lock().unwrap();
        panic!("poison tracker mutex");
    })
    .join();
    assert!(joined.is_err());
    assert!(!state.is_healthy());
    assert!(
        state
            .inner
            .lock()
            .err()
            .expect("tracker mutex remains poisoned")
            .into_inner()
            .failed
    );
}

#[tokio::test]
async fn response_close_cancelled_delivery_keeps_durable_work_and_terminal_dedupe() {
    let state = ResponseCloseState::default();
    let mut admission = state.reserve_work("target", "request").unwrap();
    assert_eq!(admission.accept_durable("digest"), Ok(true));
    drop(admission);
    let delivery = state.begin_delivery("target").unwrap();
    drop(delivery);
    {
        let inner = state.inner.lock().unwrap();
        assert_eq!(
            inner.work["target"]
                .pending
                .get("request")
                .map(String::as_str),
            Some("digest")
        );
        assert_eq!(inner.work["target"].dispatch, 0);
        assert_eq!(inner.work["target"].provisional, 0);
    }
    state.settle_terminal("target", "request", Instant::now());
    let mut duplicate = state.reserve_work("target", "request").unwrap();
    assert_eq!(duplicate.accept_durable("digest"), Ok(false));
    assert_eq!(
        duplicate.accept_durable("collision"),
        Err(ResponseCloseError::Conflict)
    );
    assert!(state.inner.lock().unwrap().work["target"]
        .pending
        .is_empty());
    for wait_for_entry in [false, true] {
        let state = ResponseCloseState::default();
        let mut work = state.reserve_work("target", "cancelled").unwrap();
        assert_eq!(work.accept_durable("durable-body"), Ok(true));
        drop(work);
        let delivery = state.begin_delivery("target").unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (_release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let worker = tokio::spawn(async move {
            let _delivery = delivery;
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        });
        if wait_for_entry {
            tokio::time::timeout(Duration::from_secs(10), entered_rx)
                .await
                .unwrap()
                .unwrap();
        }
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        let inner = state.inner.lock().unwrap();
        assert_eq!(inner.work["target"].dispatch, 0);
        assert_eq!(inner.work["target"].provisional, 0);
        assert_eq!(inner.work["target"].pending["cancelled"], "durable-body");
    }
    let state = ResponseCloseState::default();
    let mut work = state.reserve_work("failed-target", "failed-key").unwrap();
    assert_eq!(work.accept_durable("failed-body"), Ok(true));
    let delivery = state.begin_delivery("failed-target").unwrap();
    assert_eq!(
        state.inner.lock().unwrap().work["failed-target"].pending["failed-key"],
        "failed-body"
    );
    state.fail_closed();
    state.settle_terminal("failed-target", "failed-key", Instant::now());
    drop(delivery);
    drop(work);
    let inner = state.inner.lock().unwrap();
    assert!(inner.failed);
    assert_eq!(
        inner.work["failed-target"].pending["failed-key"],
        "failed-body"
    );
    assert!(!inner.work["failed-target"]
        .terminal
        .contains_key("failed-key"));
}

#[test]
fn response_close_create_owner_drop_cannot_release_successor_nonce() {
    let now = Instant::now() + Duration::from_secs(3600);
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    let first = Uuid::new_v4();
    let foreign = || CreateClassification::Foreign {
        publication_epoch: 1,
    };
    let protected_classification = || CreateClassification::Protected {
        identity: member.clone(),
        publication_epoch: 1,
    };
    let CreateAdmission::Reserved(owner) = state
        .reserve_create_owner(&member.target, first, foreign())
        .unwrap()
    else {
        panic!("Foreign ownership must progress");
    };
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
    let successor = Uuid::new_v4();
    let CreateAdmission::Reserved(successor_owner) = state
        .reserve_create_owner(&member.target, successor, foreign())
        .unwrap()
    else {
        panic!("retained Foreign core cannot defer another Foreign owner");
    };
    assert!(matches!(
        state.reserve_create_owner(
            &member.target,
            Uuid::new_v4(),
            CreateClassification::Unknown,
        ),
        Ok(CreateAdmission::WaitingIdentity)
    ));
    assert!(matches!(
        state.reserve_create_owner(
            &member.target,
            Uuid::new_v4(),
            CreateClassification::Preparing {
                owner_nonce: first,
                identity: member.clone(),
                publication_epoch: 1,
            },
        ),
        Ok(CreateAdmission::ClosingDeferred)
    ));
    drop(owner);
    assert_eq!(
        state
            .inner
            .lock()
            .unwrap()
            .create_owners
            .get(&member.target)
            .and_then(|owners| owners.get(&successor)),
        Some(&false)
    );
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
    drop(successor_owner);
    let CreateAdmission::Reserved(protected) = state
        .reserve_create_owner(&member.target, first, protected_classification())
        .unwrap()
    else {
        panic!("first protected owner");
    };
    assert!(matches!(
        state.reserve_create_owner(&member.target, successor, protected_classification()),
        Ok(CreateAdmission::ClosingDeferred)
    ));
    drop(protected);
    assert!(!state
        .inner
        .lock()
        .unwrap()
        .create_owners
        .contains_key(&member.target));
    // Foreign does not demand a canonical FQN before legacy preflight.
    let CreateAdmission::Reserved(alias) = state
        .reserve_create_owner("legacy/alias", first, foreign())
        .unwrap()
    else {
        panic!("legacy alias Foreign ownership");
    };
    assert!(matches!(
        alias.classification(),
        CreateClassification::Foreign {
            publication_epoch: 1
        }
    ));
    drop(alias);
}

#[test]
fn response_close_old_writer_drop_cannot_change_replacement_generation() {
    let now = Instant::now();
    let state = ResponseCloseState::default();
    let id = Uuid::new_v4();
    state.register_route(id, 1, now);
    let old = state.reserve_writer(id, 1).unwrap();
    state.remove_route(id, 1);
    state.register_route(id, 2, now);
    let new = state.reserve_writer(id, 2).unwrap();
    drop(old);
    assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 1);
    assert!(state.reserve_writer(id, 1).is_err());
    drop(new);
    assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 0);
}

fn assert_offloaded_enqueue_consistency(
    db_path: &std::path::Path,
    tracker: &ResponseCloseState,
    request: &crate::api::message_store::EnqueueRequest,
    key: &str,
    digest: &str,
    cancelled: bool,
) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let rows = {
        let mut statement = conn
            .prepare(
                "SELECT sender_fqn,target_fqn,op_id,body,body_sha256,\
                 content_type,source_plane,source_ref,status FROM \
                 messages WHERE sender_fqn=?1 AND op_id=?2",
            )
            .unwrap();
        let rows = statement
            .query_map(
                rusqlite::params![request.sender_fqn, request.op_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    assert!(rows.len() <= 1, "duplicate sender/op row");
    let inner = tracker.inner.lock().unwrap();
    assert!(!inner.failed);
    let work = inner.work.get(&request.target_fqn);
    assert_eq!(work.map_or(0, |work| work.provisional), 0);
    assert_eq!(work.map_or(0, |work| work.dispatch), 0);
    assert!(!work.is_some_and(|work| work.terminal.contains_key(key)));
    if let Some(row) = rows.first() {
        assert_eq!(
            row,
            &(
                request.sender_fqn.clone(),
                request.target_fqn.clone(),
                request.op_id.clone(),
                request.body.clone(),
                digest.to_owned(),
                request.content_type.clone(),
                request.source_plane.clone(),
                request.source_ref.clone(),
                "queued".into()
            )
        );
        assert_eq!(work.and_then(|work| work.pending.get(key)), Some(&row.4));
    } else {
        assert!(cancelled, "positive twin must publish a row");
        assert!(work.is_none_or(|work| !work.pending.contains_key(key)));
    }
    drop(inner);
    drop(conn);
}

#[test]
fn response_close_counter_overflow_disables_feature_but_preserves_ordinary_writer() {
    let state = ResponseCloseState::default();
    let id = Uuid::new_v4();
    state.register_route(id, 1, Instant::now());
    state
        .inner
        .lock()
        .unwrap()
        .work
        .entry("target".into())
        .or_default()
        .provisional = usize::MAX;
    assert!(matches!(
        state.reserve_work("target", "request"),
        Err(ResponseCloseError::Unavailable)
    ));
    assert!(state.inner.lock().unwrap().failed);
    assert!(state.reserve_writer(id, 1).is_ok());
    let state = ResponseCloseState::default();
    let delivery = state.begin_delivery("target").unwrap();
    drop(delivery);
    assert_eq!(state.inner.lock().unwrap().work["target"].dispatch, 0);
    state
        .inner
        .lock()
        .unwrap()
        .work
        .get_mut("target")
        .unwrap()
        .dispatch = usize::MAX;
    assert!(matches!(
        state.begin_delivery("target"),
        Err(ResponseCloseError::Unavailable)
    ));
    assert!(state.inner.lock().unwrap().failed);
    assert!(matches!(
        state.begin_delivery("other"),
        Err(ResponseCloseError::Unavailable)
    ));
    // H: caller cancellation before/around blocking-worker start. Runtime
    // shutdown may discard a queued job; either complete consistent branch
    // is valid. This is not a post-commit publication-window barrier.
    for cancelled in [true, false] {
        use crate::api::message_store::{EnqueueRequest, MessageStore};
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("offloaded-cancel.sqlite3");
        let store = MessageStore::open(db_path.clone()).unwrap();
        let tracker = ResponseCloseState::default();
        store.bind_response_close(tracker.clone());
        let request = EnqueueRequest {
            sender_fqn: "project:room/coordinator".into(),
            target_fqn: "project:room/member".into(),
            op_id: Uuid::new_v4().to_string(),
            content_type: "text/plain".into(),
            body: "offloaded durable body".into(),
            source_plane: "test".into(),
            source_ref: Some("same fixture".into()),
        };
        let key = format!(
            "db:{}:{}:{}",
            request.sender_fqn.len(),
            request.sender_fqn,
            request.op_id
        );
        let digest = crate::phone::types::sha256_hex(request.body.as_bytes());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        if cancelled {
            {
                let _entered = runtime.enter();
                let mut caller = Box::pin(store.enqueue_offloaded(request.clone()));
                let waker = std::task::Waker::noop();
                let mut context = std::task::Context::from_waker(waker);
                assert!(matches!(
                    std::future::Future::poll(caller.as_mut(), &mut context),
                    std::task::Poll::Pending
                ));
                drop(caller);
            }
        } else {
            // Positive twin: the identical fixture really awaits enqueue.
            let result = runtime
                .block_on(store.enqueue_offloaded(request.clone()))
                .unwrap();
            assert!(!result.duplicate);
            assert_eq!(result.target_fqn, request.target_fqn);
        }
        // No connection, state guard or runtime-enter guard is held here.
        // Dropping the runtime joins running jobs, without forcing queued ones.
        drop(runtime);
        assert_offloaded_enqueue_consistency(
            &db_path, &tracker, &request, &key, &digest, cancelled,
        );
    }
}

fn routes(
    now: Instant,
) -> (
    ResponseCloseState,
    ResponseRouteIdentity,
    ResponseRouteIdentity,
) {
    let state = ResponseCloseState::default();
    state.apply_settings(true, 30, now);
    state.inner.lock().unwrap().ready = true;
    state.mark_work_bootstrapped();
    let member = ResponseRouteIdentity {
        target: "project:room/member".into(),
        session_id: Uuid::new_v4(),
        token: Uuid::new_v4(),
        generation: 1,
        carrier_version: 1,
        backend: SessionBackendKind::LocalProcess,
    };
    let coordinator = ResponseRouteIdentity {
        target: "project:room/coordinator".into(),
        session_id: Uuid::new_v4(),
        token: Uuid::new_v4(),
        generation: 2,
        carrier_version: 1,
        backend: SessionBackendKind::LocalProcess,
    };
    for (identity, eligible) in [(&member, true), (&coordinator, false)] {
        state.register_route(identity.session_id, identity.generation, now);
        state.publish_manager_row(
            identity.session_id,
            identity.token,
            identity.generation,
            identity.carrier_version,
            eligible,
            false,
            now,
        );
        state.publish_manager_facts(identity.clone(), eligible, false, now);
    }
    state.observe_detector(member.session_id, false, now);
    state.publish_menu(member.session_id, Some(false), now);
    state.publish_hold(member.session_id, None, true, now);
    (state, member, coordinator)
}

fn candidate(
    state: &ResponseCloseState,
    member: &ResponseRouteIdentity,
    coordinator: &ResponseRouteIdentity,
    key: &str,
    now: Instant,
) -> ResponseDeliveryContext {
    let mut work = state.reserve_work(&member.target, key).unwrap();
    work.accept_durable(key).unwrap();
    drop(work);
    let writer = state
        .reserve_writer(member.session_id, member.generation)
        .unwrap();
    let context = state
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            key.into(),
            key.into(),
            now,
        )
        .unwrap();
    drop(writer);
    context
}

fn register(
    state: &ResponseCloseState,
    member: &ResponseRouteIdentity,
    coordinator: &ResponseRouteIdentity,
    context: ResponseDeliveryContext,
    now: Instant,
) {
    state.mark_request_submitted(&context, true);
    assert!(state
        .register_request(
            ResponseDeliveryProof {
                context,
                member: member.clone(),
                coordinator: coordinator.clone(),
                published_carrier_version: member.carrier_version
            },
            now
        )
        .is_ok());
}

fn arm(
    state: &ResponseCloseState,
    member: &ResponseRouteIdentity,
    coordinator: &ResponseRouteIdentity,
    now: Instant,
) {
    let context = candidate(state, member, coordinator, "request", now);
    register(state, member, coordinator, context, now);
    let context = state
        .admit_return(member, coordinator, "ack".into(), "ack".into(), now)
        .unwrap();
    state
        .observe_return_success(ReturnDeliveryProof { context }, now)
        .unwrap();
}

#[test]
fn response_close_readiness_is_disabled_even_with_enabled_settings() {
    let now = Instant::now();
    let (state, member, coordinator) = routes(now);
    state.inner.lock().unwrap().ready = false;
    assert!(state
        .reserve_policy_candidate(member, coordinator, "req".into(), "digest".into(), now)
        .is_err());
}

#[test]
fn response_close_coordinator_identity_change_and_restore_never_resurrects_policy() {
    let now = Instant::now() + Duration::from_secs(3600);
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    let mut changed = coordinator.clone();
    changed.target = "project:room/another-coordinator".into();
    state.publish_manager_facts(changed, false, false, now);
    state.publish_manager_facts(coordinator, false, false, now);
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
    assert!(state.inner.lock().unwrap().routes[&member.session_id]
        .policy
        .is_none());
}

#[test]
fn response_close_early_return_waits_for_complete_submission_and_registration() {
    let now = Instant::now() + Duration::from_secs(3600);
    let (state, member, coordinator) = routes(now);
    let mut work = state.reserve_work(&member.target, "request").unwrap();
    work.accept_durable("request").unwrap();
    drop(work);
    let writer = state
        .reserve_writer(member.session_id, member.generation)
        .unwrap();
    let candidate = state
        .reserve_policy_candidate(
            member.clone(),
            coordinator.clone(),
            "request".into(),
            "request".into(),
            now,
        )
        .unwrap();
    let early = state
        .admit_return(
            &member,
            &coordinator,
            "early".into(),
            "early".into(),
            now + Duration::from_secs(1),
        )
        .unwrap();
    state
        .observe_return_success(
            ReturnDeliveryProof { context: early },
            now + Duration::from_secs(2),
        )
        .unwrap();
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
    drop(writer);
    register(
        &state,
        &member,
        &coordinator,
        candidate,
        now + Duration::from_secs(20),
    );
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(49))
        .is_err());
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(50))
        .is_ok());
}

#[test]
fn response_close_stale_return_and_failed_submission_never_arm_successor() {
    let now = Instant::now() + Duration::from_secs(3600);
    let (state, member, coordinator) = routes(now);
    assert!(state
        .admit_return(&member, &coordinator, "pre".into(), "pre".into(), now)
        .is_err());
    let old = candidate(&state, &member, &coordinator, "old", now);
    let early = state
        .admit_return(&member, &coordinator, "reply".into(), "reply".into(), now)
        .unwrap();
    state.mark_request_submitted(&old, false);
    state.settle_terminal(&member.target, "old", now);
    let next = candidate(&state, &member, &coordinator, "next", now);
    register(&state, &member, &coordinator, next, now);
    assert!(state
        .observe_return_success(ReturnDeliveryProof { context: early }, now)
        .is_err());
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
}

#[test]
fn response_close_pending_returns_dedupe_and_out_of_order_settlement_restart_clock() {
    let now = Instant::now() + Duration::from_secs(3600);
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    let a = state
        .admit_return(
            &member,
            &coordinator,
            "a".into(),
            "a".into(),
            now + Duration::from_secs(10),
        )
        .unwrap();
    let b = state
        .admit_return(
            &member,
            &coordinator,
            "b".into(),
            "b".into(),
            now + Duration::from_secs(11),
        )
        .unwrap();
    let duplicate = state
        .admit_return(
            &member,
            &coordinator,
            "b".into(),
            "b".into(),
            now + Duration::from_secs(12),
        )
        .unwrap();
    assert_eq!(b.sequence, duplicate.sequence);
    assert!(matches!(
        state.admit_return(&member, &coordinator, "b".into(), "collision".into(), now),
        Err(ResponseCloseError::Conflict)
    ));
    state
        .observe_return_success(
            ReturnDeliveryProof { context: b.clone() },
            now + Duration::from_secs(20),
        )
        .unwrap();
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
    state
        .settle_return_terminal(&a, now + Duration::from_secs(25))
        .unwrap();
    state
        .observe_return_success(
            ReturnDeliveryProof { context: b },
            now + Duration::from_secs(40),
        )
        .unwrap();
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(54))
        .is_err());
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(55))
        .is_ok());
}

#[test]
fn response_close_unknown_hold_held_keys_and_native_busy_veto_delayed_manager_idle() {
    let now = Instant::now() + Duration::from_secs(3600);
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    state.publish_hold(member.session_id, None, false, now);
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
    state.publish_hold(
        member.session_id,
        Some(now + Duration::from_secs(40)),
        true,
        now,
    );
    state.publish_held_key(member.session_id, "held", true, now);
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
    state.publish_held_key(
        member.session_id,
        "held",
        false,
        now + Duration::from_secs(50),
    );
    state.observe_detector(member.session_id, true, now + Duration::from_secs(51));
    state.publish_manager_facts(member.clone(), true, false, now + Duration::from_secs(52));
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
    state.observe_detector(member.session_id, false, now + Duration::from_secs(60));
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(89))
        .is_err());
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(90))
        .is_ok());
}

#[test]
fn response_close_setting_epoch_changes_revoke_tickets_and_old_return_success() {
    let now = Instant::now() + Duration::from_secs(3600);
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    let old = state
        .select_ticket(member.session_id, now + Duration::from_secs(30))
        .unwrap();
    let late = state
        .admit_return(
            &member,
            &coordinator,
            "late".into(),
            "late".into(),
            now + Duration::from_secs(31),
        )
        .unwrap();
    state.apply_settings(true, 10, now + Duration::from_secs(32));
    state
        .observe_return_success(
            ReturnDeliveryProof { context: late },
            now + Duration::from_secs(33),
        )
        .unwrap();
    assert!(state
        .try_commit_response_close(&old, now + Duration::from_secs(100))
        .is_err());
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(42))
        .is_err());
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(43))
        .is_ok());
    state.apply_settings(true, 0, now + Duration::from_secs(44));
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
}

#[test]
fn ticket_identity_matches_original_route() {
    let now = Instant::now() + Duration::from_secs(3600);
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    let ticket = state
        .select_ticket(member.session_id, now + Duration::from_secs(30))
        .unwrap();
    let expected = (member.session_id, member.generation, member.backend);
    assert_eq!(ticket.route_identity(), expected);
    state
        .try_commit_response_close(&ticket, now + Duration::from_secs(30))
        .unwrap();
    assert_eq!(ticket.route_identity(), expected);
    state
        .inner
        .lock()
        .unwrap()
        .routes
        .get_mut(&member.session_id)
        .unwrap()
        .generation += 1;
    assert_eq!(
        state.is_closing(member.session_id, member.generation),
        Err(ResponseCloseError::Unknown)
    );
    assert_eq!(ticket.route_identity(), expected);
    state.fail_closed();
    assert_eq!(ticket.route_identity(), expected);
}
