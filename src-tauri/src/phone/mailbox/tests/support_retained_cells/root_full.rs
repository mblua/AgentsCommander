use super::*;

struct RootFullCtx<'a> {
    app: &'a tauri::AppHandle<tauri::test::MockRuntime>,
    state: &'a crate::api::message_store::MessageStoreState,
    store: &'a Arc<crate::api::message_store::MessageStore>,
    coordinator: &'a crate::session::selection::SelectionCoordinator,
    manager: &'a SessionManager,
    sender: Uuid,
    pty: bool,
    inline: bool,
}

fn root_full_control(
    inline: bool,
    index: usize,
) -> (
    Arc<crate::session::selection::ReplacementFixtureControl>,
    MailboxTestHooks,
    MailboxPoller,
) {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureControl, ReplacementFixtureCut as Cut,
    };
    let control = Arc::new(ReplacementFixtureControl::default());
    control.ticket_only.store(!inline, Ordering::Release);
    if index < 65 {
        control.actions.lock().unwrap().push(Action::PanicOnce {
            cut: Cut::SourceCoreEntered,
        });
    }
    let hooks = MailboxTestHooks::default();
    *hooks.replacement_fixture.lock().unwrap() = Some(control.clone());
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    (control, hooks, poller)
}

async fn root_full_delivery(
    ctx: &RootFullCtx<'_>,
    poller: &MailboxPoller,
    injection: Option<&str>,
    message: &OutboxMessage,
    index: usize,
) {
    if let Some(injection) = injection {
        poller
            .dispatch_pty_input_operation(
                ctx.app,
                ctx.state,
                injection,
                crate::phone::types::PtyInputSourcePlane::HostCli,
                None,
            )
            .await;
    } else {
        let actual = poller.deliver_wake(ctx.app, message).await;
        if index == 65 {
            let busy = crate::session::selection::SelectionCoordinatorError::Busy;
            assert!(
                actual.unwrap_err().contains(&busy.to_string()),
                "actual full root maps to Busy"
            );
        }
    }
}

async fn root_full_accepted(
    ctx: &RootFullCtx<'_>,
    control: &crate::session::selection::ReplacementFixtureControl,
    index: usize,
    actual_delivery: impl std::future::Future<Output = ()>,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let coordinator = ctx.coordinator;
    let stopped = async {
        loop {
            if control
                .observe_owner(coordinator)
                .is_some_and(|owner| owner.source_stopped)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {
        () = actual_delivery => panic!("real accepted-core panic leaves its outer \
            waiter ambiguous"),
        () = stopped => {},
    }
    let receipt = retained_source_wait_cut(control, Cut::SourceCoreEntered).await;
    let owner = control.observe_owner(coordinator).unwrap();
    assert!(owner.source_stopped && !owner.terminal && !owner.transferred);
    assert_eq!(
        owner.effects_started, 0,
        "no finalization/native capacity consumed"
    );
    assert!(ctx
        .manager
        .get_pending_session(receipt.binding.unwrap())
        .await
        .is_none());
    assert!(!control.submitted.load(Ordering::Acquire));
    assert!(shared_engine_writes(ctx.app, receipt.binding.unwrap().session_id()).is_empty());
    assert_eq!(coordinator.fixture_root_count(), Some(index + 1));
}

fn assert_root_full_injection(
    store: &crate::api::message_store::MessageStore,
    injection: Option<String>,
    index: usize,
) {
    if let Some(injection) = injection {
        let actual = store
            .query_pty_input_by_injection(&injection)
            .unwrap()
            .unwrap();
        assert_ne!(
            actual.status,
            crate::phone::types::PtyInputPublicStatus::Injected
        );
        assert!(
            !actual.terminal,
            "retained work cannot claim a terminal handoff"
        );
        if index == 65 {
            assert_eq!(
                actual.reason.map(|reason| reason.code),
                Some(crate::phone::types::PtyInputReasonCode::StoreTransient)
            );
        }
    }
}

async fn root_full_member(
    ctx: &RootFullCtx<'_>,
    index: usize,
    route: &crate::config::teams::VerifiedPtyInputRoute,
    previous: &Uuid,
    target: &str,
) {
    let (control, hooks, poller) = root_full_control(ctx.inline, index);
    let injection = ctx.pty.then(|| {
        enqueue_shared_pty_engine_operation(
            ctx.store,
            route,
            ctx.sender,
            b"root-full-retained-source",
        )
    });
    let mut message = wake_message_to_target();
    message.id = format!("root-full-{index}-{}", Uuid::new_v4());
    message.to = target.to_owned();
    let actual_delivery = root_full_delivery(ctx, &poller, injection.as_deref(), &message, index);
    if index < 65 {
        root_full_accepted(ctx, &control, index, actual_delivery).await;
    } else {
        actual_delivery.await;
        assert_eq!(ctx.coordinator.fixture_root_count(), Some(65));
        assert!(
            control.receipts.lock().unwrap().is_empty(),
            "refused attempt was \
                never accepted"
        );
    }
    assert!(ctx.manager.get_session(*previous).await.is_some());
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    assert_root_full_injection(ctx.store, injection, index);
}

pub(in super::super) async fn retained_exited_respawn_root_full_cell(pty: bool, inline: bool) {
    let fixture = make_mailbox_fixture();
    // Provision the whole immutable team authority before the sole restore.
    let members: Vec<_> = (0..66)
        .map(|index| provision_retained_fixture_member(&fixture, &format!("root-full-{index}")))
        .collect();
    let app = fixture.app.handle();
    let (coordinator, _, sender, _) = prepare_retained_exited_respawn(&fixture).await;
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    // Retire every real prerequisite before any retained work is admitted;
    // an unrelated later close must not compete with earlier Preparing.
    let mut retired = Vec::with_capacity(members.len());
    for (replica, target) in &members {
        let (route, previous) = tokio::time::timeout(
            Duration::from_secs(20),
            prepare_additional_retained_member(&fixture, &coordinator, sender, replica, target),
        )
        .await
        .expect("actual retired member prerequisite");
        retired.push((route, previous, target.clone()));
    }
    assert_eq!(coordinator.fixture_root_count(), Some(0));
    let ctx = RootFullCtx {
        app,
        state: &state,
        store: &store,
        coordinator: &coordinator,
        manager: &manager,
        sender,
        pty,
        inline,
    };
    for (index, (route, previous, target)) in retired.iter().enumerate() {
        tokio::time::timeout(
            Duration::from_secs(20),
            root_full_member(&ctx, index, route, previous, target),
        )
        .await
        .expect("same-history root member admission/refusal");
    }
    assert_eq!(coordinator.fixture_root_count(), Some(65));
    let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
        .await
        .expect("actual full retained root close");
    assert!(!report.persistence_safe);
    assert_eq!(
        coordinator.fixture_root_count(),
        Some(65),
        "no fake completion/reset during seal"
    );
}
