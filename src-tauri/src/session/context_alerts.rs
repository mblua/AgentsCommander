use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::pty::context_scrape::ContextSample;

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::time::{Duration, Instant};

#[cfg(test)]
use futures::future::BoxFuture;
#[cfg(test)]
use uuid::Uuid;

#[cfg(test)]
use crate::phone::mailbox::{InternalNoticeGuard, InternalSystemNotice, InternalSystemTarget};

#[cfg(test)]
use crate::commands::entity_creation::parse_team_from_workgroup_name;
#[cfg(test)]
use crate::session::manager::SessionManager;
#[cfg(test)]
use crate::session::session::Session;
#[cfg(test)]
use crate::session::session::SessionStatus;

mod actor;
mod model;
mod runtime;

use actor::run_context_alert_actor;
#[cfg(test)]
use actor::{
    apply_numeric_resolution, dispatch_due_batch, fail_batch, handle_delivery_completion,
    process_sample, reconcile_policy, retry_delay, run_maintenance,
};
use runtime::ProductionContextAlertRuntime;
#[cfg(test)]
use runtime::{
    prepare_internal_route_blocking, resolve_member_policy_blocking,
    session_matches_policy_snapshot, validate_attempt_guard, validated_project_fqn_component,
};

pub(crate) use crate::session::context_sample_queue::CONTEXT_SAMPLE_QUEUE_CAPACITY;
pub(crate) use model::ContextAlertServices;
#[cfg(test)]
use model::{
    ActorState, AlertBatch, AttemptPreparation, AttemptSnapshot, DeliveryCompletion, InFlightSlot,
    LatchState, MemberIdentityFingerprint, MemberPolicyResolution, ReadyAttempt,
    ResolutionRecovery, ResolvedPolicy, CONTEXT_ALERT_MAINTENANCE_INTERVAL, FIRST_RETRY_DELAY,
    HOLD_RECHECK_DELAY,
};
use model::{AlertClock, ContextAlertRuntime, ProductionAlertClock};

pub(crate) struct ContextAlertMonitor {
    sender: mpsc::Sender<ContextSample>,
    cancellation: CancellationToken,
    join: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ContextAlertMonitor {
    pub(crate) fn start_with_services(
        services: Arc<dyn ContextAlertServices>,
        cancellation: CancellationToken,
        handle: tokio::runtime::Handle,
    ) -> Arc<Self> {
        Self::start_with_runtime_on(
            Arc::new(ProductionContextAlertRuntime { services }),
            Arc::new(ProductionAlertClock),
            cancellation,
            handle,
        )
    }

    #[cfg(test)]
    fn start_with_runtime(
        runtime: Arc<dyn ContextAlertRuntime>,
        clock: Arc<dyn AlertClock>,
        cancellation: CancellationToken,
    ) -> Arc<Self> {
        Self::start_with_runtime_on(
            runtime,
            clock,
            cancellation,
            tokio::runtime::Handle::current(),
        )
    }

    fn start_with_runtime_on(
        runtime: Arc<dyn ContextAlertRuntime>,
        clock: Arc<dyn AlertClock>,
        cancellation: CancellationToken,
        handle: tokio::runtime::Handle,
    ) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(CONTEXT_SAMPLE_QUEUE_CAPACITY);
        let actor_cancellation = cancellation.clone();
        let join = handle.spawn(async move {
            run_context_alert_actor(runtime, clock, receiver, actor_cancellation).await;
        });
        Arc::new(Self {
            sender,
            cancellation,
            join: Mutex::new(Some(join)),
        })
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<ContextSample> {
        self.sender.clone()
    }

    pub(crate) fn request_close(&self) {
        self.cancellation.cancel();
    }

    pub(crate) async fn close_and_join(&self) -> Result<(), String> {
        self.request_close();
        let handle = self
            .join
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(handle) = handle else {
            return Ok(());
        };
        handle
            .await
            .map_err(|error| format!("Context alert actor join failed: {error}"))
    }
}

#[cfg(test)]
mod tests;
