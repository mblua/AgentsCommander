use std::pin::Pin;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::oneshot;
use tokio::time::timeout;

use super::{SelectionTask, SelectionTasks, NATIVE_SELECTION_TASKS};

const PRESENCE: Duration = Duration::from_secs(10);
const ABSENCE: Duration = Duration::from_millis(100);

#[tokio::test]
async fn issue_3950_native_success() {
    let (send, receive) = oneshot::channel();
    let task = NATIVE_SELECTION_TASKS.spawn(Box::pin(async move {
        send.send(17).unwrap();
    }));
    timeout(PRESENCE, task).await.unwrap().unwrap();
    assert_eq!(receive.await.unwrap(), 17);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_native_success");
}

#[tokio::test]
async fn issue_3950_native_panic() {
    let task = NATIVE_SELECTION_TASKS.spawn(Box::pin(async { panic!("native task proof") }));
    assert!(timeout(PRESENCE, task).await.unwrap().is_err());
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_native_panic");
}

#[tokio::test]
async fn issue_3950_native_abort() {
    let (entered, entry) = oneshot::channel();
    let task = NATIVE_SELECTION_TASKS.spawn(Box::pin(async move {
        entered.send(()).unwrap();
        std::future::pending::<()>().await;
    }));
    timeout(PRESENCE, entry).await.unwrap().unwrap();
    task.abort();
    assert!(timeout(PRESENCE, task).await.unwrap().is_err());
    timeout(PRESENCE, NATIVE_SELECTION_TASKS.spawn(Box::pin(async {})))
        .await
        .unwrap()
        .unwrap();
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_native_abort");
}

struct DropAborts(Pin<Box<dyn SelectionTask>>);

impl Drop for DropAborts {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The same detach contract exercises both native ownership and the bad double.
async fn detach_finishes_after_release<T>(spawn: impl FnOnce(BoxFuture<'static, ()>) -> T) -> bool {
    let (entered, entry) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (finished, mut finish) = oneshot::channel();
    let task = spawn(Box::pin(async move {
        entered.send(()).unwrap();
        if released.await.is_ok() {
            let _ = finished.send(());
        }
    }));
    timeout(PRESENCE, entry).await.unwrap().unwrap();
    assert!(timeout(ABSENCE, &mut finish).await.is_err());
    drop(task);
    let _ = release.send(());
    matches!(timeout(PRESENCE, finish).await, Ok(Ok(())))
}

#[tokio::test]
async fn issue_3950_native_detach() {
    assert!(detach_finishes_after_release(|future| NATIVE_SELECTION_TASKS.spawn(future)).await);
    assert!(
        !detach_finishes_after_release(|future| {
            DropAborts(NATIVE_SELECTION_TASKS.spawn(future))
        })
        .await
    );
    assert!(detach_finishes_after_release(|future| NATIVE_SELECTION_TASKS.spawn(future)).await);
    println!("\nISSUE3950_MUTANT_REJECTED case=drop_aborts");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_native_detach");
}

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::session::selection::{
    run_owned_selection_operation_with_tasks, IN_SELECTION_WORKER, TEST_SELECTION_TURN,
};

#[tokio::test]
async fn issue_3950_typed_resource() {
    struct Resource(Box<usize>);
    let resource = Resource(Box::new(42));
    let address = (&*resource.0) as *const usize as usize;
    let result = super::run_owned_selection_operation(move || async move { Ok(resource) })
        .await
        .unwrap();
    assert_eq!((&*result.0) as *const usize as usize, address);
    assert_eq!(*result.0, 42);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_typed_resource");
}

#[tokio::test]
async fn issue_3950_body_error() {
    let result =
        super::run_owned_selection_operation(|| async { Err::<(), _>("body".into()) }).await;
    assert_eq!(result, Err("body".into()));
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_body_error");
}

struct ScriptedFailure {
    body: BoxFuture<'static, ()>,
}

impl Future for ScriptedFailure {
    type Output = Result<(), String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.body.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(()) => Poll::Ready(Err("scripted native join failure".into())),
        }
    }
}

impl SelectionTask for ScriptedFailure {
    fn abort(&self) {}
}

struct ScriptedTasks;

impl SelectionTasks for ScriptedTasks {
    fn spawn(&self, future: BoxFuture<'static, ()>) -> Pin<Box<dyn SelectionTask>> {
        // Buffer the typed value before join polling; joining still fails.
        let body = tauri::async_runtime::spawn(future);
        Box::pin(ScriptedFailure {
            body: Box::pin(async move { body.await.unwrap() }),
        })
    }
}

#[tokio::test]
async fn issue_3950_turn_panic() {
    let panic =
        super::run_owned_selection_operation::<_, _, ()>(|| async { panic!("owned body panic") });
    assert_eq!(
        timeout(PRESENCE, panic).await.unwrap(),
        Err("selectionOperationTurnJoinFailed".into())
    );
    let scripted = run_owned_selection_operation_with_tasks(&ScriptedTasks, || async { Ok(19) });
    assert_eq!(
        timeout(PRESENCE, scripted).await.unwrap(),
        Err("selectionOperationTurnJoinFailed".into()),
        "native join failure must win over buffered typed value"
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_turn_panic");
}

#[derive(Default)]
struct CountTasks(AtomicUsize);

impl SelectionTasks for CountTasks {
    fn spawn(&self, future: BoxFuture<'static, ()>) -> Pin<Box<dyn SelectionTask>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        NATIVE_SELECTION_TASKS.spawn(future)
    }
}

#[tokio::test]
async fn issue_3950_recursive_zero_spawn() {
    let tasks = CountTasks::default();
    let operation = run_owned_selection_operation_with_tasks(&tasks, || async { Ok(()) });
    assert_eq!(tasks.0.load(Ordering::SeqCst), 0, "call must remain lazy");
    let result = IN_SELECTION_WORKER.scope((), operation).await;
    assert!(result.is_err());
    assert_eq!(
        tasks.0.load(Ordering::SeqCst),
        0,
        "recursion must reject before spawn"
    );
    assert_eq!(
        run_owned_selection_operation_with_tasks(&tasks, || async { Ok(7) }).await,
        Ok(7)
    );
    assert_eq!(tasks.0.load(Ordering::SeqCst), 1);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_recursive_zero_spawn");
}

impl Future for DropAborts {
    type Output = Result<(), String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.as_mut().poll(cx)
    }
}

impl SelectionTask for DropAborts {
    fn abort(&self) {
        self.0.abort();
    }
}

struct AbortOnDropTasks;

impl SelectionTasks for AbortOnDropTasks {
    fn spawn(&self, future: BoxFuture<'static, ()>) -> Pin<Box<dyn SelectionTask>> {
        Box::pin(DropAborts(NATIVE_SELECTION_TASKS.spawn(future)))
    }
}

async fn caller_settles(tasks: &dyn SelectionTasks) -> bool {
    let turn = Arc::new(tokio::sync::RwLock::new(()));
    let body_turn = Arc::clone(&turn);
    let (entered, entry) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (settled, settlement) = oneshot::channel();
    let operation = run_owned_selection_operation_with_tasks(tasks, move || async move {
        entered.send(()).unwrap();
        released.await.unwrap();
        let held = body_turn.try_write().is_err();
        let _ = settled.send(held);
        Ok(())
    });
    let mut caller = Box::pin(TEST_SELECTION_TURN.scope(Arc::clone(&turn), operation));
    tokio::select! {
        result = &mut caller => panic!("caller returned before release: {result:?}"),
        result = entry => result.unwrap(),
        () = tokio::time::sleep(PRESENCE) => panic!("body never entered"),
    }
    assert!(turn.try_write().is_err(), "body must own turn");
    drop(caller);
    let _ = release.send(());
    let finished = matches!(timeout(PRESENCE, settlement).await, Ok(Ok(true)));
    let guard = timeout(PRESENCE, turn.write()).await.unwrap();
    drop(guard);
    finished
}

#[tokio::test]
async fn issue_3950_caller_settles() {
    assert!(caller_settles(&NATIVE_SELECTION_TASKS).await);
    assert!(!caller_settles(&AbortOnDropTasks).await);
    assert!(caller_settles(&NATIVE_SELECTION_TASKS).await);
    println!("\nISSUE3950_MUTANT_REJECTED case=abort_on_drop");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_caller_settles");
}

#[tokio::test]
async fn issue_3950_read_write() {
    let turn = Arc::new(tokio::sync::RwLock::new(()));
    let first_reader = turn.read().await;
    let operation = TEST_SELECTION_TURN.scope(Arc::clone(&turn), async {
        super::run_owned_selection_read_timed("issue3950-overlap", || async { Ok(()) }).await
    });
    assert_eq!(timeout(PRESENCE, operation).await.unwrap(), Ok(()));
    let (entered, mut entry) = oneshot::channel();
    let writer_turn = Arc::clone(&turn);
    let writer = tokio::spawn(TEST_SELECTION_TURN.scope(writer_turn, async move {
        super::run_owned_selection_operation(move || async move {
            entered.send(()).unwrap();
            Ok(())
        })
        .await
    }));
    assert!(timeout(ABSENCE, &mut entry).await.is_err());
    drop(first_reader);
    timeout(PRESENCE, entry).await.unwrap().unwrap();
    assert_eq!(writer.await.unwrap(), Ok(()));
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_read_write");
}

#[tokio::test]
async fn issue_3950_release_before_log() {
    use crate::session::selection::timing_probe::TimingSinkRegistration;
    let turn = Arc::new(tokio::sync::RwLock::new(()));
    let log_turn = Arc::clone(&turn);
    let observed = Arc::new(AtomicUsize::new(0));
    let log_observed = Arc::clone(&observed);
    let _sink = TimingSinkRegistration::new(
        "issue3950-release",
        Arc::new(move |_| {
            let released = log_turn.try_write().is_ok();
            log_observed.store(if released { 1 } else { 2 }, Ordering::SeqCst);
        }),
    );
    let result = TEST_SELECTION_TURN
        .scope(turn, async {
            super::run_owned_selection_operation_timed("issue3950-release", || async { Ok(()) })
                .await
        })
        .await;
    assert_eq!(result, Ok(()));
    assert_eq!(
        observed.load(Ordering::SeqCst),
        1,
        "turn must release before timing log"
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_release_before_log");
}
