use super::*;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("owned operation test exceeded 5s")
}

fn assert_pending<F: Future>(future: std::pin::Pin<&mut F>) {
    assert!(matches!(
        future.poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
}

// Always release a blocking probe on assertion failure as well.
struct ReleaseGate(Arc<ProbeGate>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_3263_operation_producer_probe_reachable() {
    bounded(async {
        let operation = Uuid::new_v4();
        let probe = Arc::new(OperationProbe::default());
        let gate = ReleaseGate(probe.gate(
            "run_owned_persist_core",
            ProbeEvent::ProducerResultBeforeExit,
        ));
        let tasks = OwnedTaskSet::new_with_probe(operation, probe.clone());
        let ticket = run_observed_owned_core(tasks.clone(), async {
            super::super::write::run_owned_persist_core(async { 41usize }).await + 1
        });
        let source = ticket.task_id();
        let context = probe
            .observed(
                "run_owned_persist_core",
                ProbeEvent::ProducerResultBeforeExit,
            )
            .await;
        assert_eq!(context.operation, operation);
        assert_eq!(context.parent, Some(source));
        assert!(!context.attempt.is_nil());
        assert_eq!(*ticket.result().await.unwrap(), 42);
        assert!(Arc::ptr_eq(
            &ticket.result().await.unwrap(),
            &ticket.result().await.unwrap()
        ));
        // Negative control: actual result is published while a real child is still alive.
        let proof = tasks.join_producers();
        tokio::pin!(proof);
        assert_pending(proof.as_mut());
        drop(ticket);
        gate.0.release();
        let proof = proof.await.unwrap();
        assert_eq!(proof.operation, operation);
        assert_eq!(proof.exits.len(), 2);
        assert!(proof.exits.iter().all(|exit| exit.outcome.is_ok()));
        let joined = probe
            .observed("run_owned_persist_core", ProbeEvent::ProducerJoined)
            .await;
        assert_eq!(joined.task, context.task);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_3263_operation_waiter_dropped_before_launch() {
    bounded(async {
        let probe = Arc::new(OperationProbe::default());
        let gate = ReleaseGate(probe.gate("owned_source", ProbeEvent::ChildRegisteredBeforeLaunch));
        let tasks = OwnedTaskSet::new_with_probe(Uuid::new_v4(), probe.clone());
        let (ran, received) = oneshot::channel();
        let ticket = run_observed_owned_core(tasks.clone(), async move {
            ran.send(7).unwrap();
            7
        });
        let source = ticket.task_id();
        let context = probe
            .observed("owned_source", ProbeEvent::ChildRegisteredBeforeLaunch)
            .await;
        assert_eq!(context.task, Some(source));
        assert!(lock(&tasks.state).records[&source].exit.is_none());
        drop(ticket);
        let proof = tasks.join_producers();
        tokio::pin!(proof);
        assert_pending(proof.as_mut());
        gate.0.release();
        assert_eq!(received.await.unwrap(), 7);
        assert_eq!(proof.await.unwrap().exits.len(), 1);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_3263_operation_blocking_prepare_probe_reachable() {
    bounded(async {
        let probe = Arc::new(OperationProbe::default());
        let entered = ReleaseGate(probe.gate(
            "prepare_persist_snapshot",
            ProbeEvent::BlockingPrepareEntered,
        ));
        let exited = ReleaseGate(probe.gate(
            "prepare_persist_snapshot",
            ProbeEvent::BlockingPrepareExited,
        ));
        let result = ReleaseGate(probe.gate(
            "prepare_persist_snapshot",
            ProbeEvent::ProducerResultBeforeExit,
        ));
        let tasks = OwnedTaskSet::new_with_probe(Uuid::new_v4(), probe.clone());
        let ticket = run_observed_owned_core(tasks.clone(), async {
            super::super::orphan::prepare_persist_snapshot(Vec::new(), &[])
                .await
                .unwrap()
                .kept
                .len()
        });
        let source = ticket.task_id();
        let context = probe
            .observed(
                "prepare_persist_snapshot",
                ProbeEvent::BlockingPrepareEntered,
            )
            .await;
        assert_eq!(context.parent, Some(source));
        drop(ticket);
        let proof = tasks.join_producers();
        tokio::pin!(proof);
        assert_pending(proof.as_mut());
        entered.0.release();
        let after = probe
            .observed(
                "prepare_persist_snapshot",
                ProbeEvent::BlockingPrepareExited,
            )
            .await;
        assert_eq!(context.task, after.task);
        assert_pending(proof.as_mut());
        exited.0.release();
        probe
            .observed(
                "prepare_persist_snapshot",
                ProbeEvent::ProducerResultBeforeExit,
            )
            .await;
        assert_pending(proof.as_mut());
        result.0.release();
        assert_eq!(proof.await.unwrap().exits.len(), 2);
        let joined = probe
            .observed("prepare_persist_snapshot", ProbeEvent::ProducerJoined)
            .await;
        assert_eq!(joined.task, context.task);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_3263_operation_child_registration_races_close() {
    bounded(async {
        for close_first in [true, false] {
            let tasks = OwnedTaskSet::new(Uuid::new_v4());
            let (register, allowed) = oneshot::channel();
            let (registered, seen) = oneshot::channel();
            let (finish, finished) = oneshot::channel();
            let ticket = run_observed_owned_core(tasks.clone(), async move {
                let child = OperationScope::current()
                    .unwrap()
                    .reserve("live_parent")
                    .unwrap()
                    .launch(async move {
                        allowed.await.unwrap();
                        let grandchild = OperationScope::current()
                            .unwrap()
                            .reserve("late_child")
                            .unwrap()
                            .launch(async move {
                                finished.await.unwrap();
                            });
                        registered.send(()).unwrap();
                        grandchild.result().await.unwrap();
                    });
                child.task_id()
            });
            let child_id = *ticket.result().await.unwrap();
            // Source exit, rather than its result, permits close.
            loop {
                let changed = tasks.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if lock(&tasks.state).records[&ticket.id].exit.is_some() {
                    break;
                }
                changed.await;
            }
            let closed;
            if close_first {
                closed = tasks.close_producers();
                register.send(()).unwrap();
                seen.await.unwrap();
            } else {
                register.send(()).unwrap();
                seen.await.unwrap();
                closed = tasks.close_producers();
            }
            assert!(matches!(
                tasks.reserve(None, "late_root"),
                Err(TaskRegistrationError::Closed)
            ));
            assert!(matches!(
                tasks.reserve(Some(ticket.id), "dead_parent"),
                Err(TaskRegistrationError::ParentNotLive)
            ));
            let proof = OwnedTaskSet::join_closed(&closed);
            tokio::pin!(proof);
            assert_pending(proof.as_mut());
            finish.send(()).unwrap();
            let proof = proof.await;
            assert_eq!(proof.exits.len(), 3);
            assert!(proof.exits.iter().any(|exit| exit.parent == Some(child_id)));
            assert!(matches!(
                tasks.reserve(Some(child_id), "sealed"),
                Err(TaskRegistrationError::Closed)
            ));
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_3263_operation_panic_and_not_launched_are_failures() {
    bounded(async {
        let tasks = OwnedTaskSet::new(Uuid::new_v4());
        let ticket: OwnedTaskTicket<()> =
            run_observed_owned_core(tasks.clone(), async { panic!("source panic payload") });
        assert_eq!(
            ticket.result().await.unwrap_err(),
            TaskExitFailure::Panicked
        );
        assert_eq!(
            tasks.join_producers().await.unwrap_err(),
            TaskExitFailure::Panicked
        );
        let tasks = OwnedTaskSet::new(Uuid::new_v4());
        drop(tasks.reserve(None, "not_launched").unwrap());
        assert_eq!(
            tasks.join_producers().await.unwrap_err(),
            TaskExitFailure::NotLaunched
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_3263_operation_cancelled_handle_needs_actual_join() {
    bounded(async {
        let tasks = OwnedTaskSet::new(Uuid::new_v4());
        let mut reserved = tasks.reserve(None, "cancelled").unwrap();
        let id = reserved.id;
        let cell = ResultCell::<Arc<usize>>::new();
        let (entered, started) = oneshot::channel();
        let handle = tokio::spawn(async move {
            entered.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let abort = handle.abort_handle();
        assert!(reserved.install(handle, cell.clone()));
        started.await.unwrap();
        let ticket = OwnedTaskTicket {
            id,
            cell,
            tasks: tasks.clone(),
        };
        let closed = tasks.close_producers();
        let proof = OwnedTaskSet::join_closed(&closed);
        tokio::pin!(proof);
        assert_pending(proof.as_mut());
        abort.abort();
        assert_eq!(
            ticket.result().await.unwrap_err(),
            TaskExitFailure::Cancelled
        );
        let proof = proof.await;
        assert_eq!(proof.exits[0].outcome, Err(TaskExitFailure::Cancelled));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_3263_operation_legacy_non_sync_result_and_panic_payload() {
    bounded(async {
        let tasks = OwnedTaskSet::new(Uuid::new_v4());
        let value = OperationScope::run(
            tasks.clone(),
            super::super::write::run_owned_persist_core(async { std::cell::Cell::new(9) }),
        )
        .await;
        assert_eq!(value.get(), 9);
        assert_eq!(tasks.join_producers().await.unwrap().exits.len(), 1);
        let tasks = OwnedTaskSet::new(Uuid::new_v4());
        let inside = tasks.clone();
        let waiter = tokio::spawn(async move {
            OperationScope::run(
                inside,
                super::super::write::run_owned_persist_core(async {
                    std::panic::panic_any(1234usize);
                }),
            )
            .await;
        });
        let panic = waiter.await.unwrap_err().into_panic();
        assert_eq!(*panic.downcast::<usize>().unwrap(), 1234);
        assert_eq!(
            tasks.join_producers().await.unwrap_err(),
            TaskExitFailure::Panicked
        );
        // Scope-free legacy return API remains available.
        assert_eq!(
            super::super::write::run_owned_persist_core(async { 12 }).await,
            12
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_3263_operation_blocking_ticket_result_is_not_exit() {
    bounded(async {
        let probe = Arc::new(OperationProbe::default());
        let gate = ReleaseGate(probe.gate("blocking_ticket", ProbeEvent::ProducerResultBeforeExit));
        let tasks = OwnedTaskSet::new_with_probe(Uuid::new_v4(), probe.clone());
        let ticket = tasks
            .reserve(None, "blocking_ticket")
            .unwrap()
            .launch_blocking(|| 81usize);
        probe
            .observed("blocking_ticket", ProbeEvent::ProducerResultBeforeExit)
            .await;
        assert_eq!(*ticket.result().await.unwrap(), 81);
        let closed = tasks.close_producers();
        let proof = OwnedTaskSet::join_closed(&closed);
        tokio::pin!(proof);
        assert_pending(proof.as_mut());
        drop(ticket);
        gate.0.release();
        let proof = proof.await;
        assert_eq!(proof.exits.len(), 1);
        assert!(proof.exits[0].outcome.is_ok());
    })
    .await;
}
