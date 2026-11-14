use super::*;

fn committed_dormant() -> (
    ResponseCloseState,
    ResponseRouteIdentity,
    ResponseCloseTicket,
) {
    let now = Instant::now() + Duration::from_secs(3600);
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    let at = now + Duration::from_secs(30);
    let ticket = state.select_ticket(member.session_id, at).unwrap();
    state.try_commit_response_close(&ticket, at).unwrap();
    (state, member, ticket)
}

fn set_row(
    state: &ResponseCloseState,
    member: &ResponseRouteIdentity,
    working: bool,
    eligible: bool,
) {
    let mut inner = state.inner.lock().unwrap();
    let route = inner.routes.get_mut(&member.session_id).unwrap();
    route.manager_working = Some(working);
    route.eligible = eligible;
}

fn dormant(ticket: &ResponseCloseTicket) -> ResponseCloseSettlementEvidence {
    let checkpoint_nonce = Uuid::new_v4();
    ResponseCloseSettlementEvidence {
        committed_binding: ticket.committed_binding(),
        current_settlement: ResponseCloseCurrentSettlement {
            current_carrier_version: 1,
            confirmed_carrier_version: 1,
            checkpoint_nonce,
            current_checkpoint_nonce: checkpoint_nonce,
            publication_epoch: 2,
            current_publication_epoch: 2,
        },
        outcome: ResponseCloseSettlementOutcome::Dormant {
            exact_backend_absent: true,
            all_effects_completed: true,
            retained_exited_confirmed: true,
            selection_settled: true,
        },
    }
}

#[test]
fn finish_response_close_route_missing_is_stale() {
    let (state, member, ticket) = committed_dormant();
    state
        .inner
        .lock()
        .unwrap()
        .routes
        .remove(&member.session_id);
    assert_eq!(
        state.finish_response_close(&ticket, dormant(&ticket)),
        Err(StaleOrInvalidSettlement)
    );
}

#[test]
fn finish_response_close_dormant_rejects_working_or_eligible_row() {
    for (working, eligible) in [(true, false), (false, true)] {
        let (state, member, ticket) = committed_dormant();
        set_row(&state, &member, working, eligible);
        assert_eq!(
            state.finish_response_close(&ticket, dormant(&ticket)),
            Err(StaleOrInvalidSettlement)
        );
        assert!(state
            .inner
            .lock()
            .unwrap()
            .routes
            .contains_key(&member.session_id));
        assert_eq!(
            state.is_closing(member.session_id, member.generation),
            Ok(true)
        );
    }
}

#[test]
fn finish_response_close_dormant_removes_route() {
    let (state, member, ticket) = committed_dormant();
    set_row(&state, &member, false, false);
    assert_eq!(
        state.finish_response_close(&ticket, dormant(&ticket)),
        Ok(())
    );
    assert!(!state
        .inner
        .lock()
        .unwrap()
        .routes
        .contains_key(&member.session_id));
}
