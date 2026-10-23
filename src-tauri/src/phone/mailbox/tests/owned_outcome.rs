use super::*;

/// (#1399 T7 / G4) The shared drain settles the full owned disk-state
/// matrix directly and requeues each failed settlement exactly once.
#[tokio::test]
async fn owned_outcome_drain_settles_or_requeues_every_disk_state() {
    let now = std::time::Instant::now() + Duration::from_secs(3600);
    let prepare = || {
        use crate::pty::manager::{
            ResponseClosePublisher, ResponseCloseState, ResponseDeliveryProof,
            ResponseRouteIdentity,
        };
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
                1,
                true,
                false,
                now,
            );
            tracker.publish_manager_facts(identity.clone(), true, false, now);
        }
        tracker.observe_detector(member.session_id, false, now);
        tracker.publish_menu(member.session_id, Some(false), now);
        tracker.publish_hold(member.session_id, None, true, now);
        let mut request = tracker.reserve_work(&member.target, "request").unwrap();
        request.accept_durable("request-body").unwrap();
        drop(request);
        let context = tracker
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                "request".into(),
                "request-body".into(),
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
        let return_context = tracker
            .admit_return(
                &member,
                &coordinator,
                "return".into(),
                "return-body".into(),
                now,
            )
            .unwrap();
        let mut admission = tracker.reserve_work(&coordinator.target, "fs-key").unwrap();
        assert_eq!(admission.accept_durable("fs-body"), Ok(true));
        drop(admission);
        let work = ResponseWakeWork {
            diagnostic_message_id: None,
            state: tracker.clone(),
            target: coordinator.target.clone(),
            key: "fs-key".into(),
            digest: "fs-body".into(),
            attempt: Arc::default(),
            return_context: Some(return_context),
            return_routes: Some((member.clone(), coordinator.clone())),
            delivery_context: None,
        };
        (tracker, member, coordinator, work)
    };
    // A source-only completion has no current app/route authority and cannot arm.
    let (positive, member, _, work) = prepare();
    let positive_poller = MailboxPoller::new();
    let positive_path = PathBuf::from("positive.json");
    positive_poller
        .response_sources
        .lock()
        .unwrap()
        .insert(positive_path.clone(), work);
    positive_poller.settle_response_source(&positive_path, true);
    assert!(positive
        .select_ticket(member.session_id, now + Duration::from_secs(60))
        .is_err());

    // Restore succeeds: the origin is actionable before the Err is counted.
    {
        let temp = tempfile::TempDir::new().unwrap();
        let outbox = temp.path().join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        let origin = outbox.join("restore-success.json");
        let claim = wake_claim_path(&origin);
        std::fs::write(&claim, "{}").unwrap();
        let mut poller = MailboxPoller::new();
        let (tracker, member, coordinator, work) = prepare();
        assert!(tracker
            .reserve_policy_candidate(
                coordinator.clone(),
                member.clone(),
                work.key.clone(),
                work.digest.clone(),
                now
            )
            .is_ok());
        poller
            .response_sources
            .lock()
            .unwrap()
            .insert(origin.clone(), work);
        assert!(poller.live_claims.insert(claim.clone()));
        poller
            .wake_outcomes_tx
            .send((origin.clone(), Err("restore success".to_string())))
            .unwrap();

        poller.drain_wake_outcomes().await;
        assert!(poller
            .response_sources
            .lock()
            .unwrap()
            .contains_key(&origin));
        let retained = poller
            .response_sources
            .lock()
            .unwrap()
            .get(&origin)
            .unwrap()
            .clone();
        assert!(retained.state.same_state(&tracker));
        assert_eq!(retained.target, coordinator.target);
        assert_eq!(retained.key, "fs-key");
        assert_eq!(retained.digest, "fs-body");
        assert!(retained.delivery_context.is_none());
        tracker
            .settle_return_terminal(retained.return_context.as_ref().unwrap(), now)
            .unwrap();

        let pending = tracker.reserve_policy_candidate(
            coordinator.clone(),
            member.clone(),
            "fs-key".into(),
            "fs-body".into(),
            now,
        );
        assert!(pending.is_ok());
        assert!(tracker
            .select_ticket(member.session_id, now + Duration::from_secs(60))
            .is_err());

        assert!(!claim.exists());
        assert!(origin.exists());
        assert!(poller.live_claims.is_empty());
        assert_eq!(
            poller
                .retry_tracker
                .get(&origin)
                .map(|state| state.attempt_count),
            Some(1)
        );
        assert!(matches!(
            poller.wake_outcomes_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    // Claim plus receipt is authoritative and clears existing retry state.
    {
        let temp = tempfile::TempDir::new().unwrap();
        let outbox = temp.path().join("outbox");
        let delivered_dir = outbox.join("delivered");
        std::fs::create_dir_all(&delivered_dir).unwrap();
        let origin = outbox.join("claim-with-receipt.json");
        let claim = wake_claim_path(&origin);
        let receipt = delivered_dir.join("claim-with-receipt.json");
        std::fs::write(&claim, "{}").unwrap();
        std::fs::write(&receipt, "{}").unwrap();
        let mut poller = MailboxPoller::new();
        let (tracker, member, coordinator, work) = prepare();
        assert!(tracker
            .reserve_policy_candidate(
                coordinator.clone(),
                member.clone(),
                work.key.clone(),
                work.digest.clone(),
                now
            )
            .is_ok());
        poller
            .response_sources
            .lock()
            .unwrap()
            .insert(origin.clone(), work);
        poller.retry_tracker.insert(
            origin.clone(),
            RetryState {
                attempt_count: 4,
                logged: true,
            },
        );
        assert!(poller.live_claims.insert(claim.clone()));
        poller
            .wake_outcomes_tx
            .send((origin.clone(), Err("receipt wins".to_string())))
            .unwrap();

        poller.drain_wake_outcomes().await;
        assert!(!poller
            .response_sources
            .lock()
            .unwrap()
            .contains_key(&origin));
        let pending = tracker.reserve_policy_candidate(
            coordinator.clone(),
            member.clone(),
            "fs-key".into(),
            "fs-body".into(),
            now,
        );
        assert!(matches!(
            pending,
            Err(crate::pty::manager::ResponseCloseError::Conflict)
        ));
        assert!(tracker
            .select_ticket(member.session_id, now + Duration::from_secs(60))
            .is_err());

        assert!(receipt.exists());
        assert!(!claim.exists());
        assert!(!origin.exists());
        assert!(poller.live_claims.is_empty());
        assert!(!poller.retry_tracker.contains_key(&origin));
        assert!(!outbox
            .join("rejected")
            .join("claim-with-receipt.json")
            .exists());
        assert!(matches!(
            poller.wake_outcomes_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    // Claim plus receipt is authoritative and clears existing retry state.
    {
        let temp = tempfile::TempDir::new().unwrap();
        let outbox = temp.path().join("outbox");
        let rejected_dir = outbox.join("rejected");
        std::fs::create_dir_all(&rejected_dir).unwrap();
        let origin = outbox.join("claim-with-rejected-receipt.json");
        let claim = wake_claim_path(&origin);
        let receipt = rejected_dir.join("claim-with-rejected-receipt.json");
        std::fs::write(&claim, "{}").unwrap();
        std::fs::write(&receipt, "{}").unwrap();
        let mut poller = MailboxPoller::new();
        let (tracker, member, coordinator, work) = prepare();
        assert!(tracker
            .reserve_policy_candidate(
                coordinator.clone(),
                member.clone(),
                work.key.clone(),
                work.digest.clone(),
                now
            )
            .is_ok());
        poller
            .response_sources
            .lock()
            .unwrap()
            .insert(origin.clone(), work);
        poller.retry_tracker.insert(
            origin.clone(),
            RetryState {
                attempt_count: 4,
                logged: true,
            },
        );
        assert!(poller.live_claims.insert(claim.clone()));
        poller
            .wake_outcomes_tx
            .send((origin.clone(), Err("receipt wins".to_string())))
            .unwrap();

        poller.drain_wake_outcomes().await;
        assert!(!poller
            .response_sources
            .lock()
            .unwrap()
            .contains_key(&origin));
        let pending = tracker.reserve_policy_candidate(
            coordinator.clone(),
            member.clone(),
            "fs-key".into(),
            "fs-body".into(),
            now,
        );
        assert!(matches!(
            pending,
            Err(crate::pty::manager::ResponseCloseError::Conflict)
        ));
        assert!(tracker
            .select_ticket(member.session_id, now + Duration::from_secs(60))
            .is_err());

        assert!(receipt.exists());
        assert!(!claim.exists());
        assert!(!origin.exists());
        assert!(poller.live_claims.is_empty());
        assert!(!poller.retry_tracker.contains_key(&origin));
        assert!(!outbox
            .join("delivered")
            .join("claim-with-rejected-receipt.json")
            .exists());
        assert!(matches!(
            poller.wake_outcomes_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    // A receipt beside an already-restored origin does not broaden the
    // receipt-wins rule; the original Err is still counted.
    {
        let temp = tempfile::TempDir::new().unwrap();
        let outbox = temp.path().join("outbox");
        let delivered_dir = outbox.join("delivered");
        std::fs::create_dir_all(&delivered_dir).unwrap();
        let origin = outbox.join("restored-with-receipt.json");
        let claim = wake_claim_path(&origin);
        let receipt = delivered_dir.join("restored-with-receipt.json");
        std::fs::write(&origin, "{}").unwrap();
        std::fs::write(&receipt, "{}").unwrap();
        let mut poller = MailboxPoller::new();
        let (tracker, member, coordinator, work) = prepare();
        assert!(tracker
            .reserve_policy_candidate(
                coordinator.clone(),
                member.clone(),
                work.key.clone(),
                work.digest.clone(),
                now
            )
            .is_ok());
        poller
            .response_sources
            .lock()
            .unwrap()
            .insert(origin.clone(), work);
        assert!(poller.live_claims.insert(claim.clone()));
        poller
            .wake_outcomes_tx
            .send((origin.clone(), Err("origin still actionable".to_string())))
            .unwrap();

        poller.drain_wake_outcomes().await;
        assert!(poller
            .response_sources
            .lock()
            .unwrap()
            .contains_key(&origin));
        let retained = poller
            .response_sources
            .lock()
            .unwrap()
            .get(&origin)
            .unwrap()
            .clone();
        assert!(retained.state.same_state(&tracker));
        assert_eq!(retained.target, coordinator.target);
        assert_eq!(retained.key, "fs-key");
        assert_eq!(retained.digest, "fs-body");
        assert!(retained.delivery_context.is_none());
        tracker
            .settle_return_terminal(retained.return_context.as_ref().unwrap(), now)
            .unwrap();

        let pending = tracker.reserve_policy_candidate(
            coordinator.clone(),
            member.clone(),
            "fs-key".into(),
            "fs-body".into(),
            now,
        );
        assert!(pending.is_ok());
        assert!(tracker
            .select_ticket(member.session_id, now + Duration::from_secs(60))
            .is_err());

        assert!(origin.exists());
        assert!(receipt.exists());
        assert!(!claim.exists());
        assert!(poller.live_claims.is_empty());
        assert_eq!(
            poller
                .retry_tracker
                .get(&origin)
                .map(|state| state.attempt_count),
            Some(1)
        );
        assert!(!outbox
            .join("rejected")
            .join("restored-with-receipt.json")
            .exists());
        assert!(matches!(
            poller.wake_outcomes_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    // Restore failure keeps ownership, retry fields, and one pending report.
    {
        let temp = tempfile::TempDir::new().unwrap();
        let outbox = temp.path().join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        let origin = outbox.join("restore-failure.json");
        let claim = wake_claim_path(&origin);
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(&claim, "{}").unwrap();
        let outcome = Err("restore remains pending".to_string());
        let mut poller = MailboxPoller::new();
        let (tracker, member, coordinator, work) = prepare();
        assert!(tracker
            .reserve_policy_candidate(
                coordinator.clone(),
                member.clone(),
                work.key.clone(),
                work.digest.clone(),
                now
            )
            .is_ok());
        poller
            .response_sources
            .lock()
            .unwrap()
            .insert(origin.clone(), work);
        poller.retry_tracker.insert(
            origin.clone(),
            RetryState {
                attempt_count: 5,
                logged: true,
            },
        );
        assert!(poller.live_claims.insert(claim.clone()));
        poller
            .wake_outcomes_tx
            .send((origin.clone(), outcome.clone()))
            .unwrap();

        poller.drain_wake_outcomes().await;
        assert!(poller
            .response_sources
            .lock()
            .unwrap()
            .contains_key(&origin));
        let retained = poller
            .response_sources
            .lock()
            .unwrap()
            .get(&origin)
            .unwrap()
            .clone();
        assert!(retained.state.same_state(&tracker));
        assert_eq!(retained.target, coordinator.target);
        assert_eq!(retained.key, "fs-key");
        assert_eq!(retained.digest, "fs-body");
        assert!(retained.delivery_context.is_none());
        tracker
            .settle_return_terminal(retained.return_context.as_ref().unwrap(), now)
            .unwrap();

        let pending = tracker.reserve_policy_candidate(
            coordinator.clone(),
            member.clone(),
            "fs-key".into(),
            "fs-body".into(),
            now,
        );
        assert!(pending.is_ok());
        assert!(tracker
            .select_ticket(member.session_id, now + Duration::from_secs(60))
            .is_err());

        assert!(claim.is_file());
        assert!(origin.is_dir());
        assert!(poller.live_claims.contains(&claim));
        let state = poller.retry_tracker.get(&origin).unwrap();
        assert_eq!(state.attempt_count, 5);
        assert!(state.logged);
        assert_eq!(
            poller.wake_outcomes_rx.try_recv().unwrap(),
            (origin.clone(), outcome)
        );
        assert!(matches!(
            poller.wake_outcomes_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    // Receipt-claim deletion failure has the same exact retry semantics.
    {
        let temp = tempfile::TempDir::new().unwrap();
        let outbox = temp.path().join("outbox");
        let delivered_dir = outbox.join("delivered");
        std::fs::create_dir_all(&delivered_dir).unwrap();
        let origin = outbox.join("delete-failure.json");
        let claim = wake_claim_path(&origin);
        let receipt = delivered_dir.join("delete-failure.json");
        std::fs::create_dir_all(&claim).unwrap();
        std::fs::write(&receipt, "{}").unwrap();
        let outcome = Err("delete remains pending".to_string());
        let mut poller = MailboxPoller::new();
        let (tracker, member, coordinator, work) = prepare();
        assert!(tracker
            .reserve_policy_candidate(
                coordinator.clone(),
                member.clone(),
                work.key.clone(),
                work.digest.clone(),
                now
            )
            .is_ok());
        poller
            .response_sources
            .lock()
            .unwrap()
            .insert(origin.clone(), work);
        poller.retry_tracker.insert(
            origin.clone(),
            RetryState {
                attempt_count: 6,
                logged: false,
            },
        );
        assert!(poller.live_claims.insert(claim.clone()));
        poller
            .wake_outcomes_tx
            .send((origin.clone(), outcome.clone()))
            .unwrap();

        poller.drain_wake_outcomes().await;
        assert!(poller
            .response_sources
            .lock()
            .unwrap()
            .contains_key(&origin));
        let retained = poller
            .response_sources
            .lock()
            .unwrap()
            .get(&origin)
            .unwrap()
            .clone();
        assert!(retained.state.same_state(&tracker));
        assert_eq!(retained.target, coordinator.target);
        assert_eq!(retained.key, "fs-key");
        assert_eq!(retained.digest, "fs-body");
        assert!(retained.delivery_context.is_none());
        tracker
            .settle_return_terminal(retained.return_context.as_ref().unwrap(), now)
            .unwrap();

        let pending = tracker.reserve_policy_candidate(
            coordinator.clone(),
            member.clone(),
            "fs-key".into(),
            "fs-body".into(),
            now,
        );
        assert!(pending.is_ok());
        assert!(tracker
            .select_ticket(member.session_id, now + Duration::from_secs(60))
            .is_err());

        assert!(claim.is_dir());
        assert!(receipt.exists());
        assert!(!origin.exists());
        assert!(poller.live_claims.contains(&claim));
        let state = poller.retry_tracker.get(&origin).unwrap();
        assert_eq!(state.attempt_count, 6);
        assert!(!state.logged);
        assert_eq!(
            poller.wake_outcomes_rx.try_recv().unwrap(),
            (origin.clone(), outcome)
        );
        assert!(matches!(
            poller.wake_outcomes_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    // Observation failure is pending state, not evidence of absence.
    {
        let temp = tempfile::TempDir::new().unwrap();
        let outbox = temp.path().join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        let origin = outbox.join("settlement-observation\0.json");
        let claim = wake_claim_path(&origin);
        assert!(claim.try_exists().is_err());
        assert!(wake_claim_has_settled_receipt(&origin).is_err());
        let outcome = Err("observation remains pending".to_string());
        let mut poller = MailboxPoller::new();
        let (tracker, member, coordinator, work) = prepare();
        assert!(tracker
            .reserve_policy_candidate(
                coordinator.clone(),
                member.clone(),
                work.key.clone(),
                work.digest.clone(),
                now
            )
            .is_ok());
        poller
            .response_sources
            .lock()
            .unwrap()
            .insert(origin.clone(), work);
        poller.retry_tracker.insert(
            origin.clone(),
            RetryState {
                attempt_count: 8,
                logged: true,
            },
        );
        assert!(poller.live_claims.insert(claim.clone()));
        poller
            .wake_outcomes_tx
            .send((origin.clone(), outcome.clone()))
            .unwrap();

        poller.drain_wake_outcomes().await;
        assert!(poller
            .response_sources
            .lock()
            .unwrap()
            .contains_key(&origin));
        let retained = poller
            .response_sources
            .lock()
            .unwrap()
            .get(&origin)
            .unwrap()
            .clone();
        assert!(retained.state.same_state(&tracker));
        assert_eq!(retained.target, coordinator.target);
        assert_eq!(retained.key, "fs-key");
        assert_eq!(retained.digest, "fs-body");
        assert!(retained.delivery_context.is_none());
        tracker
            .settle_return_terminal(retained.return_context.as_ref().unwrap(), now)
            .unwrap();

        let pending = tracker.reserve_policy_candidate(
            coordinator.clone(),
            member.clone(),
            "fs-key".into(),
            "fs-body".into(),
            now,
        );
        assert!(pending.is_ok());
        assert!(tracker
            .select_ticket(member.session_id, now + Duration::from_secs(60))
            .is_err());

        assert!(poller.live_claims.contains(&claim));
        let state = poller.retry_tracker.get(&origin).unwrap();
        assert_eq!(state.attempt_count, 8);
        assert!(state.logged);
        assert_eq!(
            poller.wake_outcomes_rx.try_recv().unwrap(),
            (origin.clone(), outcome)
        );
        assert!(matches!(
            poller.wake_outcomes_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
}
