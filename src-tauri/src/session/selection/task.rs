//! Task ownership needed by selection, independent of the native runtime.

use std::future::Future;
use std::pin::Pin;

use futures::future::BoxFuture;

pub(crate) trait SelectionTask:
    Future<Output = Result<(), String>> + Send + 'static
{
    fn abort(&self);
}

pub(crate) trait SelectionTasks: Send + Sync {
    fn spawn(&self, future: BoxFuture<'static, ()>) -> Pin<Box<dyn SelectionTask>>;
}
