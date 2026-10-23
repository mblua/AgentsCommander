use super::*;

#[test]
fn response_close_ordinary_ack_requires_full_interval_and_raw_output_revokes_ticket() {
    let now = Instant::now() + Duration::from_secs(3600);
    let nonce = ordinary_ack_dormant_settlement(now);
    ordinary_ack_reopen_and_fresh_return(now, nonce);
    for fault in ["healthy", "failed", "poison", "overflow"] {
        let (state, member, coordinator) = routes(now);
        arm(&state, &member, &coordinator, now);
        let at = now + Duration::from_secs(30);
        let ticket = state.select_ticket(member.session_id, at).unwrap();
        let admission = state.reserve_work(&member.target, "before-commit").unwrap();
        assert!(state.try_commit_response_close(&ticket, at).is_err());
        drop(admission);
        let delivery = state.begin_delivery(&member.target).unwrap();
        assert!(state.select_ticket(member.session_id, at).is_err());
        drop(delivery);
        // Admission invalidates the old ticket. A fresh complete quiet interval is required.
        let at = at + Duration::from_secs(30);
        let ticket = state.select_ticket(member.session_id, at).unwrap();
        let nonce = state.try_commit_response_close(&ticket, at).unwrap();
        let mut work = state.reserve_work(&member.target, "after-commit").unwrap();
        assert_eq!(work.accept_durable("body"), Ok(true));
        drop(work);
        state.remove_route(member.session_id, member.generation);
        assert_eq!(
            state.inner.lock().unwrap().routes[&member.session_id].closing,
            Some(nonce)
        );
        match fault {
            "failed" => state.fail_closed(),
            "poison" => {
                let cloned = state.clone();
                assert!(std::thread::spawn(move || {
                    let _guard = cloned.inner.lock().unwrap();
                    panic!("controlled tracker poison");
                })
                .join()
                .is_err());
            }
            "overflow" => {
                state
                    .inner
                    .lock()
                    .unwrap()
                    .work
                    .get_mut(&member.target)
                    .unwrap()
                    .dispatch = usize::MAX
            }
            _ => {}
        }
        assert!(state.target_is_closing(&member.target));
        assert!(!state.target_is_closing(&(member.target.clone() + "-other")));
        assert!(matches!(
            state.begin_delivery(&member.target),
            Err(ResponseCloseError::ClosingDeferred)
        ));
        let inner = state.inner.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(inner.work[&member.target].pending["after-commit"], "body");
        assert_eq!(inner.routes[&member.session_id].closing, Some(nonce));
        assert_eq!(inner.failed, matches!(fault, "failed" | "poison"));
        drop(inner);
        if matches!(fault, "failed" | "poison") {
            assert!(matches!(
                state.begin_delivery("unrelated"),
                Err(ResponseCloseError::Unavailable)
            ));
        } else {
            drop(state.begin_delivery("unrelated").unwrap());
        }
    }
}

fn ordinary_ack_dormant_settlement(now: Instant) -> Uuid {
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(29))
        .is_err());
    let ticket = state
        .select_ticket(member.session_id, now + Duration::from_secs(30))
        .unwrap();
    state.observe_raw_output(member.session_id, now + Duration::from_secs(30));
    assert!(state
        .try_commit_response_close(&ticket, now + Duration::from_secs(60))
        .is_err());
    assert!(!state
        .is_closing(member.session_id, member.generation)
        .unwrap());
    let ticket = state
        .select_ticket(member.session_id, now + Duration::from_secs(60))
        .unwrap();
    let nonce = state
        .try_commit_response_close(&ticket, now + Duration::from_secs(60))
        .unwrap();
    assert!(matches!(
        state.reserve_writer(member.session_id, member.generation),
        Err(ResponseCloseError::ClosingDeferred)
    ));
    let mut pending = state.reserve_work(&member.target, "after-commit").unwrap();
    assert_eq!(pending.accept_durable("retained-body"), Ok(true));
    drop(pending);
    assert!(matches!(
        state.begin_delivery(&member.target),
        Err(ResponseCloseError::ClosingDeferred)
    ));
    assert_eq!(
        state.inner.lock().unwrap().work[&member.target]
            .pending
            .get("after-commit")
            .map(String::as_str),
        Some("retained-body")
    );
    state.observe_detector(member.session_id, true, now + Duration::from_secs(61));
    state.apply_settings(false, 30, now + Duration::from_secs(62));
    assert!(state
        .is_closing(member.session_id, member.generation)
        .unwrap());
    assert_eq!(ticket.committed_binding().close_nonce, nonce);
    let dormant = || ResponseCloseSettlementOutcome::Dormant {
        exact_backend_absent: true,
        all_effects_completed: true,
        retained_exited_confirmed: true,
        selection_settled: true,
    };
    let evidence = |version, outcome| {
        let checkpoint_nonce = Uuid::new_v4();
        ResponseCloseSettlementEvidence {
            committed_binding: ticket.committed_binding(),
            current_settlement: ResponseCloseCurrentSettlement {
                current_carrier_version: version,
                confirmed_carrier_version: version,
                checkpoint_nonce,
                current_checkpoint_nonce: checkpoint_nonce,
                publication_epoch: 2,
                current_publication_epoch: 2,
            },
            outcome,
        }
    };
    // Same-UUID stable carrier changes advance current facts without
    // relabelling the committed ticket or reviving its consumed lease.
    state.publish_manager_row(
        member.session_id,
        member.token,
        member.generation,
        2,
        false,
        false,
        now + Duration::from_secs(63),
    );
    assert_eq!(ticket.committed_binding().member.carrier_version, 1);
    let mut stale = evidence(2, dormant());
    stale.current_settlement.confirmed_carrier_version = 1;
    assert_eq!(
        state.finish_response_close(&ticket, stale),
        Err(StaleOrInvalidSettlement)
    );
    assert_eq!(
        state.finish_response_close(&ticket, evidence(1, dormant())),
        Err(StaleOrInvalidSettlement)
    );
    let mut stale = evidence(2, dormant());
    stale.current_settlement.current_checkpoint_nonce = Uuid::new_v4();
    assert_eq!(
        state.finish_response_close(&ticket, stale),
        Err(StaleOrInvalidSettlement)
    );
    let mut stale = evidence(2, dormant());
    stale.current_settlement.current_publication_epoch += 1;
    assert_eq!(
        state.finish_response_close(&ticket, stale),
        Err(StaleOrInvalidSettlement)
    );
    let mut stale = evidence(2, dormant());
    stale.committed_binding.member.backend = SessionBackendKind::ContainerTransport;
    assert_eq!(
        state.finish_response_close(&ticket, stale),
        Err(StaleOrInvalidSettlement)
    );
    let mut stale = evidence(2, dormant());
    stale.committed_binding.member.generation += 1;
    assert_eq!(
        state.finish_response_close(&ticket, stale),
        Err(StaleOrInvalidSettlement)
    );
    // None facts are not proof of completion. Running, NotStarted or
    // Ambiguous must be represented by false, not Completed(Err).
    assert_eq!(
        state.finish_response_close(
            &ticket,
            evidence(
                2,
                ResponseCloseSettlementOutcome::Dormant {
                    exact_backend_absent: true,
                    all_effects_completed: false,
                    retained_exited_confirmed: true,
                    selection_settled: true,
                },
            ),
        ),
        Err(StaleOrInvalidSettlement)
    );
    assert!(state
        .is_closing(member.session_id, member.generation)
        .unwrap());
    assert_eq!(
        state.finish_response_close(&ticket, evidence(2, dormant())),
        Ok(())
    );
    assert_eq!(
        state.inner.lock().unwrap().work[&member.target].pending["after-commit"],
        "retained-body"
    );
    nonce
}

fn ordinary_ack_reopen_and_fresh_return(now: Instant, nonce: Uuid) {
    // Contrast kill failure: latest LIVE Confirmed carrier, no Exited or
    // selection-clear claim. Every effect must still be terminal.
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    let ticket = state
        .select_ticket(member.session_id, now + Duration::from_secs(30))
        .unwrap();
    state
        .try_commit_response_close(&ticket, now + Duration::from_secs(30))
        .unwrap();
    state.publish_manager_row(
        member.session_id,
        member.token,
        member.generation,
        2,
        true,
        false,
        now,
    );
    let reopen = |all_effects_terminal| ResponseCloseSettlementEvidence {
        committed_binding: ticket.committed_binding(),
        current_settlement: ResponseCloseCurrentSettlement {
            current_carrier_version: 2,
            confirmed_carrier_version: 2,
            checkpoint_nonce: nonce,
            current_checkpoint_nonce: nonce,
            publication_epoch: 3,
            current_publication_epoch: 3,
        },
        outcome: ResponseCloseSettlementOutcome::ReopenAfterKillFailure {
            terminal_kill_failure: true,
            all_effects_terminal,
            original_backend_alive: true,
            original_manager_identity_intact: true,
        },
    };
    assert_eq!(
        state.finish_response_close(&ticket, reopen(false)),
        Err(StaleOrInvalidSettlement)
    );
    assert!(state
        .is_closing(member.session_id, member.generation)
        .unwrap());
    assert_eq!(state.finish_response_close(&ticket, reopen(true)), Ok(()));
    assert!(!state
        .is_closing(member.session_id, member.generation)
        .unwrap());
    assert!(state.begin_delivery(&member.target).is_ok());
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(100))
        .is_err());
    assert!(state
        .try_commit_response_close(&ticket, now + Duration::from_secs(100))
        .is_err());

    let mut current_member = member.clone();
    current_member.carrier_version = 2;
    state.publish_manager_facts(current_member.clone(), true, false, now);
    // Pure terminal-ledger replay cannot mint another candidate. P05/P06
    // separately verify authenticated admission and on-disk receipt replay.
    assert!(matches!(
        state.reserve_policy_candidate(
            current_member.clone(),
            coordinator.clone(),
            "request".into(),
            "request".into(),
            now + Duration::from_secs(100),
        ),
        Err(ResponseCloseError::Conflict)
    ));

    let new_request = candidate(
        &state,
        &current_member,
        &coordinator,
        "new-request",
        now + Duration::from_secs(100),
    );
    register(
        &state,
        &current_member,
        &coordinator,
        new_request,
        now + Duration::from_secs(100),
    );
    // Registration alone cannot reuse the old ACK or its idle interval.
    assert!(state
        .select_ticket(member.session_id, now + Duration::from_secs(200))
        .is_err());
    let returned_at = now + Duration::from_secs(200);
    let fresh_return = state
        .admit_return(
            &current_member,
            &coordinator,
            "fresh-ack".into(),
            "fresh-ack".into(),
            returned_at,
        )
        .unwrap();
    state
        .observe_return_success(
            ReturnDeliveryProof {
                context: fresh_return,
            },
            returned_at,
        )
        .unwrap();
    assert!(matches!(
        state.select_ticket(
            member.session_id,
            returned_at + Duration::from_secs(30) - Duration::from_nanos(1),
        ),
        Err(ResponseCloseError::Unknown)
    ));
    assert!(state
        .select_ticket(member.session_id, returned_at + Duration::from_secs(30))
        .is_ok());
}
