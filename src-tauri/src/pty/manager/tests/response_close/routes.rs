use super::*;

const W: Duration = Duration::from_secs(17);

fn base() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

fn one_route(now: Instant) -> (ResponseCloseState, Uuid) {
    let state = ResponseCloseState::default();
    let id = Uuid::new_v4();
    state.register_route(id, 1, now);
    (state, id)
}

type Hold = (Option<Instant>, bool, Option<Duration>, u64, bool);

fn hold(state: &ResponseCloseState, id: Uuid) -> Hold {
    let inner = state.inner.lock().unwrap();
    let r = &inner.routes[&id];
    (
        r.hold_until,
        r.hold_known,
        r.hold_window,
        r.activity_version,
        inner.failed,
    )
}

fn set_hold(
    state: &ResponseCloseState,
    id: Uuid,
    until: Option<Instant>,
    known: bool,
    version: u64,
) {
    let mut inner = state.inner.lock().unwrap();
    let r = inner.routes.get_mut(&id).unwrap();
    r.hold_until = until;
    r.hold_known = known;
    r.hold_window = Some(W);
    r.activity_version = version;
}

#[test]
fn refresh_hold_open_sets_known_once() {
    let now = base();
    let (state, id) = one_route(now);
    state.refresh_hold_snapshot(id, W, false, now);
    assert_eq!(hold(&state, id), (None, true, Some(W), 1, false));
    state.refresh_hold_snapshot(id, W, false, now);
    assert_eq!(hold(&state, id), (None, true, Some(W), 1, false));
    set_hold(&state, id, Some(now), true, 5);
    state.refresh_hold_snapshot(id, W, false, now);
    assert_eq!(hold(&state, id), (None, true, Some(W), 6, false));
}

#[test]
fn refresh_hold_closed_cases() {
    let now = base();
    let (state, id) = one_route(now);
    let other = Duration::from_secs(9);
    // changed window
    set_hold(&state, id, None, true, 0);
    state.refresh_hold_snapshot(id, other, true, now);
    assert_eq!(hold(&state, id), (None, false, Some(other), 1, false));
    // same window, end in the future: no change
    set_hold(&state, id, Some(now + Duration::from_secs(60)), true, 0);
    state.refresh_hold_snapshot(id, W, true, now);
    assert_eq!(
        hold(&state, id),
        (Some(now + Duration::from_secs(60)), true, Some(W), 0, false)
    );
    // same window, end == now: no change (strict `now > end`)
    set_hold(&state, id, Some(now), true, 0);
    state.refresh_hold_snapshot(id, W, true, now);
    assert_eq!(hold(&state, id), (Some(now), true, Some(W), 0, false));
    // same window, end passed
    state.refresh_hold_snapshot(id, W, true, now + Duration::from_secs(1));
    assert_eq!(hold(&state, id), (Some(now), false, Some(W), 1, false));
    // same window, no end
    set_hold(&state, id, None, true, 0);
    state.refresh_hold_snapshot(id, W, true, now);
    assert_eq!(hold(&state, id), (None, false, Some(W), 1, false));
}

#[test]
fn refresh_hold_touch_overflow_marks_failed() {
    let now = base();
    for closed in [false, true] {
        let (state, id) = one_route(now);
        set_hold(&state, id, None, false, u64::MAX);
        state.refresh_hold_snapshot(id, Duration::from_secs(9), closed, now);
        let expected_known = !closed;
        assert_eq!(
            hold(&state, id),
            (
                None,
                expected_known,
                Some(Duration::from_secs(9)),
                u64::MAX,
                true
            )
        );
    }
}

#[test]
fn refresh_hold_unknown_route_is_noop() {
    let now = base();
    let (state, _) = one_route(now);
    state.refresh_hold_snapshot(Uuid::new_v4(), W, true, now);
    assert!(!state.inner.lock().unwrap().failed);
}

type Facts = (bool, bool, Option<bool>, u64, u64, bool);

// (facts == expected, eligible, manager_working, work_epoch, activity_version, policy.is_some())
fn facts(state: &ResponseCloseState, id: Uuid, expected: &ResponseRouteIdentity) -> Facts {
    let inner = state.inner.lock().unwrap();
    let r = &inner.routes[&id];
    (
        r.facts.as_ref() == Some(expected),
        r.eligible,
        r.manager_working,
        r.work_epoch,
        r.activity_version,
        r.policy.is_some(),
    )
}

fn armed() -> (
    ResponseCloseState,
    ResponseRouteIdentity,
    ResponseRouteIdentity,
    Instant,
) {
    let now = base();
    let (state, member, coordinator) = routes(now);
    arm(&state, &member, &coordinator, now);
    (state, member, coordinator, now)
}

fn failed(state: &ResponseCloseState) -> bool {
    state.inner.lock().unwrap().failed
}

#[test]
fn publish_facts_stale_identity_is_ignored() {
    let (state, _member, coordinator, now) = armed();
    let before = facts(&state, coordinator.session_id, &coordinator);
    let mut token = coordinator.clone();
    token.token = Uuid::new_v4();
    let mut generation = coordinator.clone();
    generation.generation += 1;
    let mut version = coordinator.clone();
    version.carrier_version += 1;
    for stale in [token, generation, version] {
        state.publish_manager_facts(stale, true, true, now);
        assert_eq!(facts(&state, coordinator.session_id, &coordinator), before);
    }
    assert!(!failed(&state));
}

#[test]
fn publish_facts_same_identity_paths() {
    let (state, member, coordinator, now) = armed();
    let c = coordinator.session_id;
    let (_, _, _, epoch, activity, _) = facts(&state, c, &coordinator);
    let member_before = facts(&state, member.session_id, &member);
    // nothing changed
    state.publish_manager_facts(coordinator.clone(), false, false, now);
    assert_eq!(
        facts(&state, c, &coordinator),
        (true, false, Some(false), epoch, activity, false)
    );
    // working changed: touch only
    state.publish_manager_facts(coordinator.clone(), false, true, now);
    assert_eq!(
        facts(&state, c, &coordinator),
        (true, false, Some(true), epoch, activity + 1, false)
    );
    // eligible changed: invalidate (epoch + 1, then touch), no dependents
    state.publish_manager_facts(coordinator.clone(), true, true, now);
    assert_eq!(
        facts(&state, c, &coordinator),
        (true, true, Some(true), epoch + 1, activity + 2, false)
    );
    assert_eq!(facts(&state, member.session_id, &member), member_before);
    assert!(!failed(&state));
}

#[test]
fn publish_facts_identity_change_invalidates_dependents() {
    let (state, member, coordinator, now) = armed();
    let c = coordinator.session_id;
    let (_, _, _, epoch, activity, _) = facts(&state, c, &coordinator);
    let (m_facts, m_eligible, m_working, m_epoch, m_activity, m_policy) =
        facts(&state, member.session_id, &member);
    assert!(m_policy);
    let mut changed = coordinator.clone();
    changed.target = "project:room/another-coordinator".into();
    state.publish_manager_facts(changed.clone(), false, false, now);
    assert_eq!(
        facts(&state, c, &changed),
        (true, false, Some(false), epoch + 1, activity + 1, false)
    );
    assert_eq!(
        facts(&state, member.session_id, &member),
        (
            m_facts,
            m_eligible,
            m_working,
            m_epoch + 1,
            m_activity + 1,
            false
        )
    );
    assert!(!failed(&state));
}

#[test]
fn publish_facts_overflow_marks_failed() {
    // (a) changed row invalidate overflow
    let (state, _m, coordinator, now) = armed();
    state
        .inner
        .lock()
        .unwrap()
        .routes
        .get_mut(&coordinator.session_id)
        .unwrap()
        .work_epoch = u64::MAX;
    state.publish_manager_facts(coordinator.clone(), true, false, now);
    assert!(failed(&state));
    // (b) working-only touch overflow
    let (state, _m, coordinator, now) = armed();
    state
        .inner
        .lock()
        .unwrap()
        .routes
        .get_mut(&coordinator.session_id)
        .unwrap()
        .activity_version = u64::MAX;
    state.publish_manager_facts(coordinator.clone(), false, true, now);
    assert!(failed(&state));
    // (c) dependent invalidate overflow; facts still recorded
    let (state, member, coordinator, now) = armed();
    state
        .inner
        .lock()
        .unwrap()
        .routes
        .get_mut(&member.session_id)
        .unwrap()
        .work_epoch = u64::MAX;
    let mut changed = coordinator.clone();
    changed.target = "project:room/another-coordinator".into();
    state.publish_manager_facts(changed.clone(), false, false, now);
    assert!(failed(&state));
    assert!(facts(&state, coordinator.session_id, &changed).0);
}
