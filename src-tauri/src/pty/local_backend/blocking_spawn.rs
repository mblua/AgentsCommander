use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use uuid::Uuid;

use crate::errors::AppError;

pub(super) type BlockingSpawnCleanup = dyn Fn(Uuid) + Send + Sync + 'static;

pub(super) struct BlockingSpawnCancelGuard {
    pub(super) id: Uuid,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) cleanup: Arc<BlockingSpawnCleanup>,
    pub(super) disarmed: bool,
}

impl BlockingSpawnCancelGuard {
    pub(super) fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for BlockingSpawnCancelGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }

        self.cancelled.store(true, Ordering::SeqCst);
        (self.cleanup)(self.id);
    }
}

pub(super) fn spawn_blocking_cancel_safe<F, C>(
    id: Uuid,
    work: F,
    cleanup: C,
    join_error_context: &'static str,
) -> futures::future::BoxFuture<'static, Result<(), AppError>>
where
    F: FnOnce() -> Result<(), AppError> + Send + 'static,
    C: Fn(Uuid) + Send + Sync + 'static,
{
    Box::pin(async move {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cleanup: Arc<BlockingSpawnCleanup> = Arc::new(cleanup);
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_cleanup = Arc::clone(&cleanup);
        let mut guard = BlockingSpawnCancelGuard {
            id,
            cancelled: Arc::clone(&cancelled),
            cleanup: Arc::clone(&cleanup),
            disarmed: false,
        };
        let handle = tokio::task::spawn_blocking(move || {
            let result = work();
            if worker_cancelled.load(Ordering::SeqCst) && result.is_ok() {
                (worker_cleanup)(id);
            }
            result
        });
        let result = handle
            .await
            .map_err(|e| AppError::Other(format!("{join_error_context}: {e}")))?;
        guard.disarm();
        result
    })
}
