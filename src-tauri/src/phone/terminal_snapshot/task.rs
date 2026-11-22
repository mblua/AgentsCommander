//! Snapshot task traits: `SnapshotTask` is an abortable future returning `Result<(), String>`,
//! and `SnapshotTaskHost` spawns one from a boxed future.
//! Used by the scanner and `shutdown` to spawn and abort tasks without a fixed runtime.

use futures::future::BoxFuture;
use std::future::Future;
use std::pin::Pin;

pub(crate) trait SnapshotTask: Future<Output = Result<(), String>> + Send {
    fn abort(&self);
}

pub(crate) trait SnapshotTaskHost: Clone + Send + Sync + 'static {
    fn spawn_snapshot_task(&self, future: BoxFuture<'static, ()>) -> Pin<Box<dyn SnapshotTask>>;
}
