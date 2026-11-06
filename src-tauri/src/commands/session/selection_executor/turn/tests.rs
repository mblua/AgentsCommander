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
