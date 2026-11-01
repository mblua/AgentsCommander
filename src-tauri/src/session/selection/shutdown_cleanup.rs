use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use crate::pty::manager::PtyManager;
use crate::session::manager::{PendingCreateBinding, SessionManager};

use super::admission::CriticalAdmissionKey;
use super::replacement::{OwnedReplacementContinuation, ReplacementOwner};
use super::CoordinatorInner;

pub(super) async fn execute_rollback_create_parts(
    manager: SessionManager,
    pty: Option<Arc<Mutex<PtyManager>>>,
    binding: PendingCreateBinding,
) {
    let session_id = binding.session_id();
    let backend_kind = manager
        .get_pending_session(binding)
        .await
        .map(|session| session.backend_kind);
    if let Some(pty) = pty {
        let pty = pty.lock().unwrap_or_else(|error| error.into_inner());
        let kill_result = match backend_kind {
            Some(kind) => Some(pty.kill_for_kind(session_id, kind)),
            None if pty.has_session(session_id) => Some(pty.kill(session_id)),
            None => None,
        };
        if let Some(Err(error)) = kill_result {
            log::debug!(
                "[selection] pending create PTY cleanup session={} result={}",
                session_id,
                error
            );
        }
    }
    if backend_kind.is_none() {
        return;
    }
    if let Err(error) = manager.rollback_pending_create(binding).await {
        log::warn!(
            "[selection] pending create manager rollback failed session={}: {}",
            session_id,
            error
        );
    }
}

pub(super) fn begin_container_shutdown(inner: &Arc<CoordinatorInner>, deadline: Instant) -> bool {
    let container_backend = match inner.cleanup_container_backend.try_lock() {
        Ok(backend) => backend.as_ref().and_then(Weak::upgrade),
        Err(error) => {
            log::error!(
                "[selection] container backend ownership lock unavailable during shutdown signal: \
                    {}",
                error
            );
            return false;
        }
    };
    if let Some(container_backend) = container_backend {
        return container_backend.begin_shutdown(deadline);
    }
    true
}

/// Clone one retained owner at a time; no root lock spans owner inspection,
/// U acquisition, native cleanup, or resource destruction.
pub(super) fn retained_entry_owns_binding(
    inner: &CoordinatorInner,
    binding: PendingCreateBinding,
) -> Result<Option<Arc<ReplacementOwner>>, ()> {
    let mut cursor = None;
    loop {
        let next = {
            let root = inner.retained_replacements.try_lock().map_err(|_| ())?;
            if root.unavailable {
                return Err(());
            }
            root.entries
                .iter()
                .filter(|(operation, _)| cursor.is_none_or(|last| **operation > last))
                .min_by_key(|(operation, _)| **operation)
                .map(|(operation, owner)| (*operation, owner.clone()))
        };
        let Some((operation, owner)) = next else {
            return Ok(None);
        };
        cursor = Some(operation);
        let state = owner.state.try_lock().map_err(|_| ())?;
        if let Some(progress) = state.replacement.as_ref() {
            if progress.binding.session_id() == binding.session_id() {
                return if progress.binding == binding {
                    Ok(Some(owner.clone()))
                } else {
                    Err(())
                };
            }
        }
        if let Some(create) = state.entry_create.as_ref() {
            if create.binding.session_id() == binding.session_id() {
                return if create.binding == binding {
                    Ok(Some(owner.clone()))
                } else {
                    Err(())
                };
            }
        }
    }
}

fn pending_create_cleanup_pty(
    inner: &CoordinatorInner,
) -> Result<Arc<Mutex<PtyManager>>, Vec<String>> {
    let pty = match inner.cleanup_pty.try_lock() {
        Ok(pty) => pty.as_ref().and_then(Weak::upgrade),
        Err(error) => {
            log::error!(
                "[selection] pending-create PTY ownership lock unavailable: {}",
                error
            );
            return Err(vec!["reason=pending-pty-owner state=retained".to_string()]);
        }
    };
    let Some(pty) = pty else {
        log::error!(
            "[selection] pending-create PTY owner unavailable after worker join state=retained"
        );
        return Err(vec!["reason=pending-pty-owner state=retained".to_string()]);
    };
    Ok(pty)
}

pub(super) async fn cleanup_pending_creates_after_join(
    inner: &Arc<CoordinatorInner>,
    deadline: Instant,
) -> Vec<String> {
    let pty = match pending_create_cleanup_pty(inner) {
        Ok(pty) => pty,
        Err(retained) => return retained,
    };
    let manager = Arc::clone(&inner.manager);
    let cleanup_inner = Arc::clone(inner);
    let cleanup = tokio::spawn(async move {
        let manager = manager.read().await.clone();
        let bindings = manager.pending_create_bindings().await;
        if !bindings.is_empty() {
            log::warn!(
                "[selection] cleaning {} pending create(s) after worker join",
                bindings.len()
            );
        }
        let mut retained = Vec::new();
        for binding in bindings {
            let session_id = binding.session_id();
            match retained_entry_owns_binding(&cleanup_inner, binding) {
                Ok(None) => {}
                Ok(Some(owner)) => {
                    let nonce =
                        owner.state.lock().ok().and_then(|state| {
                            state.replacement.as_ref().map(|_| state.executor_nonce)
                        });
                    let cleaned = if let Some(executor_nonce) = nonce {
                        OwnedReplacementContinuation {
                            owner,
                            coordinator: Arc::downgrade(&cleanup_inner),
                            executor_nonce,
                            source_validation: None,
                            #[cfg(test)]
                            fixture: None,
                        }
                        .cleanup_stopped_source(&manager, &pty)
                        .await
                    } else {
                        false
                    };
                    if cleaned {
                        continue;
                    }
                    retained.push(format!(
                        "session={session_id} reason=owned-entry-cleanup-authority state=retained"
                    ));
                    continue;
                }
                Err(()) => {
                    retained.push(format!(
                        "session={session_id} reason=owned-entry-unreadable state=retained"
                    ));
                    continue;
                }
            }
            let Some(backend_kind) = manager
                .get_pending_session(binding)
                .await
                .map(|session| session.backend_kind)
            else {
                continue;
            };
            let kill_pty = Arc::clone(&pty);
            let kill = tokio::task::spawn_blocking(move || {
                kill_pty
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .kill_for_kind(session_id, backend_kind)
            })
            .await;
            match kill {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    log::error!(
                        "[selection] pending-create PTY cleanup failed session={} \
                            reason=pending-pty-kill state=retained error={}",
                        session_id,
                        error
                    );
                    retained.push(format!(
                        "session={session_id} reason=pending-pty-kill state=retained"
                    ));
                    continue;
                }
                Err(error) => {
                    log::error!(
                        "[selection] pending-create PTY cleanup task failed session={} \
                            reason=pending-pty-kill state=retained error={}",
                        session_id,
                        error
                    );
                    retained.push(format!(
                        "session={session_id} reason=pending-pty-kill state=retained"
                    ));
                    continue;
                }
            }
            if let Err(error) = manager.rollback_pending_create(binding).await {
                log::warn!(
                    "[selection] pending create manager rollback failed session={} \
                        reason=pending-manager-rollback state=retained error={}",
                    session_id,
                    error
                );
                retained.push(format!(
                    "session={session_id} reason=pending-manager-rollback state=retained"
                ));
            }
        }
        retained
    });
    await_pending_create_cleanup(inner, deadline, cleanup).await
}

async fn await_pending_create_cleanup(
    inner: &Arc<CoordinatorInner>,
    deadline: Instant,
    mut cleanup: tokio::task::JoinHandle<Vec<String>>,
) -> Vec<String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    match tokio::time::timeout(remaining, &mut cleanup).await {
        Ok(Ok(retained)) => retained,
        Ok(Err(error)) => {
            log::error!(
                "[selection] pending-create cleanup owner failed reason=pending-cleanup-task \
                    state=retained error={}",
                error
            );
            vec!["reason=pending-cleanup-task state=retained".to_string()]
        }
        Err(_) => {
            log::error!(
                "[selection] pending-create cleanup owner retained after absolute deadline \
                    reason=pending-cleanup-await state=retained"
            );
            inner
                .retained_rollback_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(cleanup);
            vec!["reason=pending-cleanup-await state=retained".to_string()]
        }
    }
}

pub(super) async fn seal_and_drain_container_shutdown_work_after_join(
    inner: &Arc<CoordinatorInner>,
    deadline: Instant,
) -> Vec<String> {
    let mut retained = Vec::new();
    let container_backend = match inner.cleanup_container_backend.try_lock() {
        Ok(backend) => backend.as_ref().and_then(Weak::upgrade),
        Err(error) => {
            log::error!(
                "[selection] container backend ownership lock unavailable before bounded drain: {}",
                error
            );
            retained.push("reason=container-drain-owner state=retained".to_string());
            return retained;
        }
    };
    let Some(container_backend) = container_backend else {
        return retained;
    };
    let mut task = tokio::task::spawn_blocking(move || {
        container_backend.seal_and_drain_shutdown_work_blocking(deadline)
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    match tokio::time::timeout(remaining, &mut task).await {
        Ok(Ok(report)) => retained.extend(report.retained),
        Ok(Err(error)) => {
            log::error!(
                "[selection] container shutdown-work drain failed after coordinator join: {}",
                error
            );
            retained.push("reason=container-drain-task state=retained".to_string());
        }
        Err(_) => {
            log::error!(
                "[selection] container shutdown-work blocking task retained after absolute \
                    deadline reason=container-drain-await state=retained"
            );
            retained.push("reason=container-drain-await state=retained".to_string());
            inner
                .retained_cleanup_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(task);
        }
    }
    retained
}

pub(super) fn remove_critical_key(inner: &Arc<CoordinatorInner>, key: CriticalAdmissionKey) {
    match inner.critical_keys.lock() {
        Ok(mut keys) => {
            keys.remove(&key);
        }
        Err(error) => {
            log::error!(
                "[selection] critical-key lock poisoned session={} kind={:?}: {}",
                key.session_id,
                key.kind,
                error
            );
        }
    }
}
