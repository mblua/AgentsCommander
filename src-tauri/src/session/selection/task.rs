//! Task ownership needed by selection, independent of the native runtime.

use std::future::Future;
use std::pin::Pin;

use futures::future::BoxFuture;

pub(crate) trait SelectionTask:
    Future<Output = Result<(), String>> + Send + 'static
{
    // Removed when worker ownership migrates in P12.
    #[allow(dead_code)]
    fn abort(&self);
}

pub(crate) trait SelectionTasks: Send + Sync {
    // Removed when owned turns consume the capability in P02.
    #[allow(dead_code)]
    fn spawn(&self, future: BoxFuture<'static, ()>) -> Pin<Box<dyn SelectionTask>>;
}
