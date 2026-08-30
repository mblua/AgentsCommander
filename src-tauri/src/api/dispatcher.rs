//! Background dispatcher for DB-backed API sends.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::Future;
use tauri::Manager;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::api::message_store::{DuePtyInputCandidate, LeasedMessage, MessageStore};
use crate::phone::types::OutboxMessage;

#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    pub poll_interval: Duration,
    pub lease_for: Duration,
    pub batch_limit: usize,
    pub max_attempts: i64,
    pub retention: chrono::Duration,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(500),
            lease_for: Duration::from_secs(30),
            batch_limit: 8,
            max_attempts: 5,
            retention: chrono::Duration::days(7),
        }
    }
}

const PTY_WORKER_LIMIT: usize = 8;
const PTY_WORKER_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const PTY_CANDIDATE_SCAN_LIMIT: usize = 64;

#[derive(Default)]
struct PtyWorkerTargets {
    active: Mutex<HashSet<String>>,
}

struct PtyWorkerTargetReservation {
    owner: Arc<PtyWorkerTargets>,
    target: String,
}

impl PtyWorkerTargets {
    fn try_reserve(
        self: &Arc<Self>,
        target: &str,
        limit: usize,
    ) -> Option<PtyWorkerTargetReservation> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.len() >= limit || !active.insert(target.to_string()) {
            return None;
        }
        Some(PtyWorkerTargetReservation {
            owner: Arc::clone(self),
            target: target.to_string(),
        })
    }

    fn contains(&self, target: &str) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(target)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

impl Drop for PtyWorkerTargetReservation {
    fn drop(&mut self) {
        self.owner
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.target);
    }
}

fn reap_pty_workers(workers: &mut JoinSet<()>) {
    while let Some(result) = workers.try_join_next() {
        if let Err(error) = result {
            log::warn!("[api-dispatcher] PTY worker failed: {error}");
        }
    }
}

fn spawn_pty_worker<F>(
    workers: &mut JoinSet<()>,
    targets: &Arc<PtyWorkerTargets>,
    shutdown: &CancellationToken,
    candidate: DuePtyInputCandidate,
    worker: F,
) -> bool
where
    F: Future<Output = ()> + Send + 'static,
{
    if workers.len() >= PTY_WORKER_LIMIT {
        return false;
    }
    let Some(reservation) = targets.try_reserve(&candidate.target_fqn, PTY_WORKER_LIMIT) else {
        return false;
    };
    let worker_shutdown = shutdown.clone();
    workers.spawn(async move {
        let _reservation = reservation;
        tokio::select! {
            biased;
            _ = worker_shutdown.cancelled() => {}
            _ = worker => {}
        }
    });
    true
}

async fn stop_pty_workers(workers: &mut JoinSet<()>, grace: Duration) {
    let drained = tokio::time::timeout(grace, async {
        while let Some(result) = workers.join_next().await {
            if let Err(error) = result {
                log::warn!("[api-dispatcher] PTY worker failed during shutdown: {error}");
            }
        }
    })
    .await;
    if drained.is_err() {
        let remaining = workers.len();
        workers.abort_all();
        log::warn!("[api-dispatcher] {remaining} PTY worker(s) exceeded shutdown grace; aborting");
    }
}

pub fn start_dispatcher(
    store: Arc<MessageStore>,
    client_store: Arc<crate::api::auth::ApiClientStore>,
    app: tauri::AppHandle,
    shutdown: CancellationToken,
    config: DispatcherConfig,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(config.poll_interval);
        let mut pty_workers = JoinSet::new();
        let pty_targets = Arc::new(PtyWorkerTargets::default());
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    log::info!("[api-dispatcher] shutdown signal received, stopping");
                    stop_pty_workers(&mut pty_workers, PTY_WORKER_SHUTDOWN_GRACE).await;
                    break;
                }
                _ = interval.tick() => {
                    reap_pty_workers(&mut pty_workers);
                    // (#885 F-5) Skip DISPATCH (not the reaper) while a purge
                    // holds a lease. Leasing a row we cannot deliver burns an
                    // attempt against max_attempts (5) and can poison the
                    // message permanently. A purge is bounded; the next tick
                    // delivers.
                    let purging = app
                        .try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>()
                        .map(|g| g.is_active())
                        .unwrap_or(false);
                    if !purging {
                        if let Err(e) = dispatch_due_with(
                            &store,
                            chrono::Utc::now(),
                            &config,
                            |msg| {
                                    let app = app.clone();
                                    async move {
                                        crate::phone::mailbox::MailboxPoller::new()
                                            .deliver_wake_with_origin(
                                                &app,
                                                &msg,
                                                crate::phone::mailbox::WakeDeliveryOrigin::DbQueue,
                                            )
                                            .await
                                    }
                                },
                            )
                            .await
                        {
                            log::warn!("[api-dispatcher] dispatch tick failed: {}", e);
                        }
                    }
                    if let Some(managed_state) =
                        app.try_state::<crate::api::message_store::MessageStoreState>()
                    {
                        let state = managed_state.inner().clone();
                        let active = state.active_operations.snapshot();
                        if store
                            .recover_pty_input_runtime_offloaded(active, chrono::Utc::now())
                            .await
                            .is_err()
                        {
                            log::warn!(
                                "[api-dispatcher] PTY recovery failed code=store_transient"
                            );
                        }
                        let restoring = app
                            .try_state::<Arc<crate::RestoreInProgress>>()
                            .is_some_and(|flag| {
                                flag.0.load(std::sync::atomic::Ordering::SeqCst)
                            });
                        if !purging && !restoring && pty_workers.len() < PTY_WORKER_LIMIT {
                            match store
                                .due_container_pty_input_candidates_fair_offloaded(
                                    chrono::Utc::now(),
                                    PTY_CANDIDATE_SCAN_LIMIT,
                                )
                                .await
                            {
                                Ok(candidates) => {
                                    if let Some(candidate) = candidates
                                        .into_iter()
                                        .find(|candidate| !pty_targets.contains(&candidate.target_fqn))
                                    {
                                        let worker_app = app.clone();
                                        let worker_state = state.clone();
                                        let worker_client_store = Arc::clone(&client_store);
                                        let injection_id = candidate.injection_id.clone();
                                        spawn_pty_worker(
                                            &mut pty_workers,
                                            &pty_targets,
                                            &shutdown,
                                            candidate,
                                            async move {
                                                crate::phone::mailbox::MailboxPoller::new()
                                                    .dispatch_pty_input_operation(
                                                        &worker_app,
                                                        &worker_state,
                                                        &injection_id,
                                                        crate::phone::types::PtyInputSourcePlane::ContainerApi,
                                                        Some(&worker_client_store),
                                                    )
                                                    .await;
                                            },
                                        );
                                    }
                                }
                                Err(_) => log::warn!(
                                    "[api-dispatcher] PTY input tick failed code=store_transient"
                                ),
                            }
                        }
                    }
                    let cutoff = chrono::Utc::now() - config.retention;
                    if let Err(e) = store.reap_terminal_before_offloaded(cutoff).await {
                        log::warn!("[api-dispatcher] reaper failed: {}", e);
                    }
                    if store
                        .compact_pty_terminal_maintenance_offloaded(chrono::Utc::now(), 64)
                        .await
                        .is_err()
                    {
                        log::warn!(
                            "[api-dispatcher] PTY compaction failed code=store_transient"
                        );
                    }
                }
            }
        }
    })
}

pub async fn dispatch_due_with<F, Fut>(
    store: &MessageStore,
    now: chrono::DateTime<chrono::Utc>,
    config: &DispatcherConfig,
    mut deliver: F,
) -> Result<usize, crate::api::message_store::MessageStoreError>
where
    F: FnMut(OutboxMessage) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let rows = store
        .lease_due_offloaded(
            now,
            config.batch_limit,
            config.lease_for,
            "api-dispatcher".to_string(),
        )
        .await?;
    let mut processed = 0;
    for row in rows {
        let message = build_outbox_message(&row);
        let result =
            if crate::api::actuation::reject_if_root(&row.sender_fqn, &row.target_fqn).is_err() {
                Err("root-agent is not reachable over DB send".to_string())
            } else {
                deliver(message).await
            };

        match result {
            Ok(()) => {
                store
                    .mark_delivered_offloaded(row.message_id.clone(), chrono::Utc::now())
                    .await?;
                crate::api::audit::record("-", &row.sender_fqn, "send-dispatch", "delivered");
            }
            Err(reason) => {
                let status = store
                    .mark_delivery_failed_offloaded(
                        row.message_id.clone(),
                        reason,
                        chrono::Utc::now(),
                        config.max_attempts,
                    )
                    .await?;
                crate::api::audit::record("-", &row.sender_fqn, "send-dispatch", &status);
            }
        }
        processed += 1;
    }
    Ok(processed)
}

pub fn build_outbox_message(row: &LeasedMessage) -> OutboxMessage {
    OutboxMessage {
        id: row.message_id.clone(),
        token: None,
        from: row.sender_fqn.clone(),
        to: row.target_fqn.clone(),
        body: row.body.clone(),
        mode: "wake".to_string(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::message_store::{EnqueueRequest, DB_FILENAME, DEFAULT_CONTENT_TYPE};
    use std::sync::{Arc, Mutex};

    fn store() -> MessageStore {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(DB_FILENAME);
        let store = MessageStore::open(path).unwrap();
        std::mem::forget(dir);
        store
    }

    fn enqueue(store: &MessageStore, op_id: &str, body: &str) -> String {
        store
            .enqueue(EnqueueRequest {
                sender_fqn: "proj:wg-1/a".into(),
                target_fqn: "proj:wg-1/b".into(),
                op_id: op_id.into(),
                content_type: DEFAULT_CONTENT_TYPE.into(),
                body: body.into(),
                source_plane: "api_inline".into(),
                source_ref: None,
            })
            .unwrap()
            .message_id
    }

    #[test]
    fn build_outbox_message_has_no_privileged_fields() {
        let row = LeasedMessage {
            message_id: "m1".into(),
            sender_fqn: "proj:wg-1/a".into(),
            target_fqn: "proj:wg-1/b".into(),
            op_id: "op1".into(),
            content_type: DEFAULT_CONTENT_TYPE.into(),
            body: "hi".into(),
            attempt: 0,
        };

        let msg = build_outbox_message(&row);

        assert_eq!(msg.mode, "wake");
        assert_eq!(msg.body, "hi");
        assert!(msg.command.is_none());
        assert!(msg.action.is_none());
        assert!(msg.token.is_none());
        assert!(msg.target.is_none());
    }

    #[tokio::test]
    async fn dispatch_success_marks_delivered_and_calls_deliver() {
        let store = store();
        let message_id = enqueue(&store, "op1", "hello");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_deliver = seen.clone();

        let processed = dispatch_due_with(
            &store,
            chrono::Utc::now(),
            &DispatcherConfig::default(),
            move |msg| {
                let seen = seen_for_deliver.clone();
                async move {
                    seen.lock().unwrap().push(msg.body);
                    Ok(())
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(processed, 1);
        assert_eq!(*seen.lock().unwrap(), vec!["hello".to_string()]);
        let leased = store
            .lease_due(
                chrono::Utc::now() + chrono::Duration::hours(1),
                10,
                Duration::from_secs(1),
                "test",
            )
            .unwrap();
        assert!(leased.is_empty(), "{message_id} should be terminal");
    }

    #[tokio::test]
    async fn dispatch_failure_retries_then_poisons() {
        let store = store();
        enqueue(&store, "op1", "hello");
        let config = DispatcherConfig {
            max_attempts: 1,
            ..DispatcherConfig::default()
        };

        dispatch_due_with(&store, chrono::Utc::now(), &config, |_msg| async {
            Err("no route".to_string())
        })
        .await
        .unwrap();

        let leased = store
            .lease_due(
                chrono::Utc::now() + chrono::Duration::hours(1),
                10,
                Duration::from_secs(1),
                "test",
            )
            .unwrap();
        assert!(leased.is_empty(), "poisoned rows must not lease again");
    }

    #[tokio::test]
    async fn blocked_pty_worker_does_not_block_ordinary_or_different_target_progress() {
        let store = store();
        enqueue(&store, "ordinary-op", "ordinary body");
        let shutdown = CancellationToken::new();
        let targets = Arc::new(PtyWorkerTargets::default());
        let mut workers = JoinSet::new();
        let (blocked_started_tx, blocked_started_rx) = tokio::sync::oneshot::channel();

        assert!(spawn_pty_worker(
            &mut workers,
            &targets,
            &shutdown,
            DuePtyInputCandidate {
                injection_id: "blocked-operation".to_string(),
                target_fqn: "proj:wg-1-team/blocked".to_string(),
            },
            async move {
                blocked_started_tx.send(()).unwrap();
                std::future::pending::<()>().await;
            },
        ));
        blocked_started_rx.await.unwrap();
        assert_eq!(targets.len(), 1);
        assert!(
            !spawn_pty_worker(
                &mut workers,
                &targets,
                &shutdown,
                DuePtyInputCandidate {
                    injection_id: "same-target-operation".to_string(),
                    target_fqn: "proj:wg-1-team/blocked".to_string(),
                },
                async {},
            ),
            "only one lifecycle worker may wait on a logical target"
        );

        let processed = tokio::time::timeout(
            Duration::from_secs(1),
            dispatch_due_with(
                &store,
                chrono::Utc::now(),
                &DispatcherConfig::default(),
                |message| async move {
                    assert_eq!(message.body, "ordinary body");
                    Ok(())
                },
            ),
        )
        .await
        .expect("ordinary dispatch must not wait for PTY lifecycle")
        .unwrap();
        assert_eq!(processed, 1);

        let (different_done_tx, different_done_rx) = tokio::sync::oneshot::channel();
        assert!(spawn_pty_worker(
            &mut workers,
            &targets,
            &shutdown,
            DuePtyInputCandidate {
                injection_id: "different-target-operation".to_string(),
                target_fqn: "proj:wg-1-team/different".to_string(),
            },
            async move {
                different_done_tx.send(()).unwrap();
            },
        ));
        tokio::time::timeout(Duration::from_secs(1), different_done_rx)
            .await
            .expect("a different target must progress while the first is blocked")
            .unwrap();

        shutdown.cancel();
        tokio::time::timeout(
            Duration::from_secs(1),
            stop_pty_workers(&mut workers, Duration::from_millis(100)),
        )
        .await
        .expect("dispatcher shutdown must be bounded");
        assert_eq!(targets.len(), 0);
    }
}
