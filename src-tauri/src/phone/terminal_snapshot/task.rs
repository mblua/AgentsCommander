use futures::future::BoxFuture;
use std::future::Future;
use std::pin::Pin;

#[allow(dead_code, reason = "3946 phased consumer migration")]
pub(crate) trait SnapshotTask: Future<Output = Result<(), String>> + Send {
    fn abort(&self);
}

#[allow(dead_code, reason = "3946 phased consumer migration")]
pub(crate) trait SnapshotTaskHost: Clone + Send + Sync + 'static {
    fn spawn_snapshot_task(&self, future: BoxFuture<'static, ()>) -> Pin<Box<dyn SnapshotTask>>;
}
