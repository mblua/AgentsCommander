use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriticalAdmissionOutcome<T> {
    Completed(T),
    AlreadyPending,
}

pub(crate) trait ContainerLifecycle: Send + Sync {
    fn route_lost(
        &self,
        id: Uuid,
        exit_code: i32,
    ) -> Pin<Box<dyn Future<Output = Result<CriticalAdmissionOutcome<()>, String>> + Send + '_>>;
}

#[derive(Clone)]
pub struct ContainerLifecycleSender {
    inner: Arc<dyn ContainerLifecycle>,
}

impl ContainerLifecycleSender {
    pub(crate) fn new(inner: Arc<dyn ContainerLifecycle>) -> Self {
        Self { inner }
    }

    pub async fn route_lost(
        &self,
        session_id: Uuid,
        exit_code: i32,
    ) -> Result<CriticalAdmissionOutcome<()>, String> {
        self.inner.route_lost(session_id, exit_code).await
    }
}
