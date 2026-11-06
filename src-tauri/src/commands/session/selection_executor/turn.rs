//! Native task ownership behind selection's neutral capability.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::future::BoxFuture;

use crate::session::selection::task::{SelectionTask, SelectionTasks};

struct NativeSelectionTask {
    inner: tauri::async_runtime::JoinHandle<()>,
}

impl Future for NativeSelectionTask {
    type Output = Result<(), String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.inner)
            .poll(cx)
            .map(|result| result.map_err(|error| error.to_string()))
    }
}

impl SelectionTask for NativeSelectionTask {
    fn abort(&self) {
        self.inner.abort();
    }
}

pub(crate) struct NativeSelectionTasks;

// Removed when native turn callers migrate in P03.
#[allow(dead_code)]
pub(crate) static NATIVE_SELECTION_TASKS: NativeSelectionTasks = NativeSelectionTasks;

impl SelectionTasks for NativeSelectionTasks {
    fn spawn(&self, future: BoxFuture<'static, ()>) -> Pin<Box<dyn SelectionTask>> {
        Box::pin(NativeSelectionTask {
            inner: tauri::async_runtime::spawn(future),
        })
    }
}

// Removed when native turn callers migrate in P03.
#[allow(dead_code)]
pub(crate) async fn run_owned_selection_operation<F, Fut, T>(operation: F) -> Result<T, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send,
    T: Send + 'static,
{
    crate::session::selection::run_owned_selection_operation_with_tasks(
        &NATIVE_SELECTION_TASKS,
        operation,
    )
    .await
}

// Removed when native turn callers migrate in P03.
#[allow(dead_code)]
pub(crate) async fn run_owned_selection_read_timed<F, Fut, T>(
    command: &'static str,
    operation: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send,
    T: Send + 'static,
{
    crate::session::selection::run_owned_selection_read_timed_with_tasks(
        &NATIVE_SELECTION_TASKS,
        command,
        operation,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn run_owned_selection_operation_timed<F, Fut, T>(
    command: &'static str,
    operation: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>> + Send,
    T: Send + 'static,
{
    crate::session::selection::run_owned_selection_operation_timed_with_tasks(
        &NATIVE_SELECTION_TASKS,
        command,
        operation,
    )
    .await
}

#[cfg(test)]
mod tests;
