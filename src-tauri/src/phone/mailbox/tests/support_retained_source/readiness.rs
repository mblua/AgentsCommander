use super::*;

fn retained_readiness_claim(
    ctx: &RetainedSourceCtx<'_>,
) -> (
    String,
    crate::phone::types::PtyInputResult,
    crate::api::message_store::ClaimedPtyInputOperation,
    chrono::DateTime<chrono::Utc>,
    crate::api::message_store::PreparationHeartbeatGuard,
) {
    let (store, route, sender) = (ctx.store, ctx.route, ctx.sender);
    let waiting_id =
        enqueue_shared_pty_engine_operation(store, route, sender, b"waiting-preboundary");
    let initial = store
        .query_pty_input_by_injection(&waiting_id)
        .unwrap()
        .unwrap();
    let claimed = store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::HostCli,
            Some(&waiting_id),
            &Uuid::new_v4().to_string(),
            chrono::Utc::now(),
        )
        .unwrap()
        .unwrap();
    let expires = crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at).unwrap();
    let heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
        store.clone(),
        waiting_id.clone(),
        claimed.lease_owner.clone(),
        expires,
    );
    (waiting_id, initial, claimed, expires, heartbeat)
}

async fn retained_readiness_reacquire(
    ctx: &RetainedSourceCtx<'_>,
    id: uuid::Uuid,
    source: (
        &crate::phone::types::PtyInputResult,
        &crate::api::message_store::ClaimedPtyInputOperation,
    ),
    expires: chrono::DateTime<chrono::Utc>,
    heartbeat: &mut crate::api::message_store::PreparationHeartbeatGuard,
    wait_observed: tokio::sync::oneshot::Sender<()>,
    delivery_gate: tokio::sync::oneshot::Receiver<()>,
) {
    use std::future::Future;
    let (app, store, route, poller) = (ctx.app, ctx.store, ctx.route, ctx.poller);
    let (initial, claimed) = source;
    let gate = ctx.state.target_gate.as_ref().unwrap().clone();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let waiting = reacquire_pty_dispatch_target(
        app,
        (store, initial, claimed),
        (&gate, &route.target, PtyReacquireGoal::Spawn),
        (expires, &shutdown),
        heartbeat,
    );
    tokio::pin!(waiting);
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(waiting.as_mut().poll(cx).is_pending()))
            .await
    );
    wait_observed.send(()).unwrap();
    tokio::select! {
        biased;
        _ = &mut waiting => panic!("not-ready operation reacquired before real state \
            change"),
        done = delivery_gate => done.expect("whole first source returned"),
    }
    // The first source has released its guards; the suspended
    // operation must still own none, including exact/U/M.
    let stripe = gate
        .try_target_lock(&route.target.canonical_fqn)
        .unwrap()
        .unwrap();
    let exact = gate.acquire_exact(&route.target.canonical_fqn).await;
    drop(exact);
    drop(stripe);
    assert!(!pty_spawn_scope_may_resume(app, &route.target).await);
    retire_retained_fixture_member(ctx.fixture, ctx.coordinator, route, ctx.sender, id).await;
    let (stripe, exact) = waiting
        .await
        .expect("real retirement resumes the SAME claim");
    let ownership = gate
        .target_ownership(&route.target.canonical_fqn, &stripe, &exact)
        .unwrap();
    let mut source = crate::session::selection::SourceExecutorGuard::unbound();
    let actual = poller
        .spawn_pty_input_target(
            app,
            claimed,
            &[],
            &route.sender,
            &route.target,
            crate::phone::types::PtyInputSourcePlane::HostCli,
            None,
            &ownership,
            initial,
            None,
            &mut source,
            None,
        )
        .await;
    assert!(matches!(
        actual,
        Err(crate::phone::types::PtyInputReasonCode::InconsistentSession)
    ));
    source.returned(false);
    drop(exact);
    drop(stripe);
}

async fn retained_readiness_finish(
    ctx: &RetainedSourceCtx<'_>,
    heartbeat: &mut crate::api::message_store::PreparationHeartbeatGuard,
    waiting_id: &str,
    claimed: &crate::api::message_store::ClaimedPtyInputOperation,
) {
    let store = ctx.store;
    assert!(
        finish_pty_input_before_boundary(
            store,
            heartbeat,
            waiting_id,
            &claimed.lease_owner,
            crate::phone::types::PtyInputReasonCode::InconsistentSession
        )
        .await
    );
    let actual = store
        .query_pty_input_by_injection(waiting_id)
        .unwrap()
        .unwrap();
    assert!(actual.terminal);
    assert_eq!(
        actual.reason.unwrap().code,
        crate::phone::types::PtyInputReasonCode::InconsistentSession
    );
    let connection =
        rusqlite::Connection::open(ctx.fixture._temp.path().join("retained-respawn.sqlite3"))
            .unwrap();
    let (attempt, retries): (i64, i64) = connection
        .query_row(
            "SELECT attempt,(SELECT count(*) FROM pty_input_audit WHERE injection_id=?1 \
                AND status='retry') FROM pty_input_operations WHERE injection_id=?1",
            [waiting_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((attempt, retries), (claimed.attempt, 0));
}

pub(super) async fn retained_readiness_observer(
    ctx: &RetainedSourceCtx<'_>,
    wait_observed: tokio::sync::oneshot::Sender<()>,
    delivery_gate: tokio::sync::oneshot::Receiver<()>,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    if !ctx.flags.check_reacquire {
        return;
    }
    let baseline = retained_source_wait_cut(ctx.control, Cut::Baseline).await;
    let id = baseline.binding.unwrap().session_id();
    assert!(
        !pty_spawn_scope_may_resume(ctx.app, &ctx.route.target).await,
        "real Preparing B is not strict retired A"
    );
    let (waiting_id, initial, claimed, expires, mut heartbeat) = retained_readiness_claim(ctx);
    retained_readiness_reacquire(
        ctx,
        id,
        (&initial, &claimed),
        expires,
        &mut heartbeat,
        wait_observed,
        delivery_gate,
    )
    .await;
    retained_readiness_finish(ctx, &mut heartbeat, &waiting_id, &claimed).await;
}
