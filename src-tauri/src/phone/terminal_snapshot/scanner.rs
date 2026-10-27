use super::failure::record_host_ingress_failure;
use super::process::process_claimed;
use super::publish::cleanup_tracked_artifact;
use super::publish::protocol_cleanup_name;
use super::publish::protocol_request_name;
use super::publish::safe_remove;
use super::publish::sweep_directory;
use super::shutdown::ScannerTaskCleanup;
use super::shutdown::ScannerTaskOwnership;
use super::shutdown::SnapshotScannerShutdownOwner;
use super::CURSOR_CAP;
use super::MAX_DIRECTORY_ENTRIES;
use super::MAX_PER_POLL;
use super::REQUEST_DIRECTORY;
use super::RESPONSE_DIRECTORY;
use super::RESPONSE_TTL;
use crate::filesystem::terminal_snapshots;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;
use terminal_snapshot_renderer::TerminalSnapshotReasonCode;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct SnapshotMailboxScanner {
    shutdown_owner: SnapshotScannerShutdownOwner,
    cursors: HashMap<crate::path_identity::FileObjectId, String>,
    observed: std::collections::HashSet<crate::path_identity::FileObjectId>,
    startup_sweep_complete: bool,
    #[cfg(test)]
    pending_tasks: Vec<tauri::async_runtime::JoinHandle<()>>,
}

pub(crate) fn verified_requester_root_from_discovered_path(path: &Path) -> Option<PathBuf> {
    if let Ok(root) = crate::config::root_agent::verify_live_root_agent_path(path) {
        return Some(root.canonical_path);
    }
    crate::config::teams::verify_pty_input_replica_cwd(path)
        .ok()
        .map(|identity| identity.replica_root)
}

impl SnapshotMailboxScanner {
    /// (#1399) For a delivery-only poller. Reuses whatever owner is currently
    /// registered, so no second owner is created, and falls back to a
    /// non-registering owner rather than to `Default`, whose hand-written impl
    /// writes itself into the process-global `ACTIVE_SCANNER_SHUTDOWN_OWNER`
    /// slot and would leave the long-lived poller's real owner unreachable on
    /// the shutdown drain.
    ///
    /// (R8) Deliberately "whatever is currently registered" and not "the live
    /// owner": `active()` returns whatever registered LAST, and while #1403 is
    /// open that is an `api::dispatcher` throwaway roughly twice a second, not
    /// the long-lived poller's owner. Harmless, because a delivery poller never
    /// spawns a scanner task and so never uses the owner it holds.
    ///
    /// Fields are spelled out, never `..Default::default()`: struct-update
    /// syntax would evaluate the registering `Default` for `shutdown_owner`
    /// and hijack the global before the explicit field overwrote it. The
    /// `unwrap_or_else(unregistered)` arm exists on the structural argument
    /// (registration must be impossible on every path), not because it is
    /// commonly reached.
    pub(crate) fn delivery_only() -> Self {
        Self {
            shutdown_owner: SnapshotScannerShutdownOwner::active()
                .unwrap_or_else(SnapshotScannerShutdownOwner::unregistered),
            cursors: HashMap::new(),
            observed: std::collections::HashSet::new(),
            startup_sweep_complete: false,
            #[cfg(test)]
            pending_tasks: Vec::new(),
        }
    }

    pub(crate) fn begin_cycle(&mut self) {
        self.observed.clear();
    }

    pub(crate) fn startup_sweep_pending(&self) -> bool {
        !self.startup_sweep_complete
    }

    pub(crate) fn finish_startup_sweep(&mut self) {
        self.startup_sweep_complete = true;
    }

    pub(crate) fn startup_sweep_root(
        &mut self,
        state: &crate::pty::terminal_snapshot::TerminalSnapshotState,
        requester_root: &Path,
    ) {
        let Ok(root_identity) = crate::path_identity::verify_directory(requester_root) else {
            return;
        };
        let local = requester_root.join(crate::config::agent_local_dir_name());
        let outbox = local.join("outbox");
        let directories = [
            (outbox.join(REQUEST_DIRECTORY), false),
            (local.join(RESPONSE_DIRECTORY), true),
        ];
        for (directory, response_directory) in directories {
            let Ok(directory_identity) = crate::path_identity::verify_directory(&directory) else {
                continue;
            };
            if !private_directory_mode(&directory)
                || !crate::path_identity::is_verified_descendant(
                    &directory_identity,
                    &root_identity,
                )
            {
                continue;
            }
            let Ok(entries) = terminal_snapshots::snapshot_entries(&directory) else {
                continue;
            };
            for (index, entry) in entries.enumerate() {
                if index >= MAX_DIRECTORY_ENTRIES {
                    log::warn!("[terminal-snapshot] stage=cleanup code=directory_capacity");
                    break;
                }
                let Ok((path, _)) = entry else {
                    break;
                };
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if !protocol_cleanup_name(name, response_directory)
                    || !private_regular_file_mode(&path)
                {
                    continue;
                }
                let Some(age) = terminal_snapshots::snapshot_regular_file_age(&path) else {
                    continue;
                };
                let Ok(identity) = crate::path_identity::verify_regular_file(&path) else {
                    continue;
                };
                if age >= RESPONSE_TTL {
                    safe_remove(&path, &identity);
                    continue;
                }
                let Ok(reservation) = state.reserve_existing_artifact(
                    &directory,
                    &directory_identity,
                    identity.object_id,
                ) else {
                    continue;
                };
                let ttl = RESPONSE_TTL.saturating_sub(age);
                let _ = reservation.commit_with_ttl(path, identity, ttl);
            }
        }
    }

    pub(crate) fn finish_cycle(&mut self) {
        self.cursors
            .retain(|object, _| self.observed.contains(object));
    }

    #[cfg(test)]
    pub(crate) fn shutdown_owner_for_test(&self) -> SnapshotScannerShutdownOwner {
        self.shutdown_owner.clone()
    }

    #[cfg(test)]
    pub(crate) async fn join_pending_tasks_for_test(&mut self) {
        self.shutdown_owner.join_all_for_test().await;
        for task in std::mem::take(&mut self.pending_tasks) {
            task.await.expect("terminal snapshot mailbox task");
        }
    }

    pub(crate) fn scan_root<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        requester_root: &Path,
    ) {
        let Ok(root_identity) = crate::path_identity::verify_directory(requester_root) else {
            return;
        };
        let local = requester_root.join(crate::config::agent_local_dir_name());
        let outbox = local.join("outbox");
        let request_directory = outbox.join(REQUEST_DIRECTORY);
        let response_directory = local.join(RESPONSE_DIRECTORY);
        if !terminal_snapshots::snapshot_is_dir(&request_directory)
            || !terminal_snapshots::snapshot_is_dir(&response_directory)
        {
            return;
        }
        let (
            Ok(local_identity),
            Ok(outbox_identity),
            Ok(request_directory_identity),
            Ok(response_directory_identity),
        ) = (
            crate::path_identity::verify_directory(&local),
            crate::path_identity::verify_directory(&outbox),
            crate::path_identity::verify_directory(&request_directory),
            crate::path_identity::verify_directory(&response_directory),
        )
        else {
            return;
        };
        if !private_directory_mode(&request_directory)
            || !private_directory_mode(&response_directory)
            || !crate::path_identity::is_verified_descendant(&local_identity, &root_identity)
            || !crate::path_identity::is_verified_descendant(&outbox_identity, &local_identity)
            || !crate::path_identity::is_verified_descendant(
                &request_directory_identity,
                &outbox_identity,
            )
            || !crate::path_identity::is_verified_descendant(
                &response_directory_identity,
                &local_identity,
            )
        {
            return;
        }
        if self.cursors.len() >= CURSOR_CAP
            && !self
                .cursors
                .contains_key(&request_directory_identity.object_id)
        {
            return;
        }
        self.observed.insert(request_directory_identity.object_id);
        sweep_directory(&request_directory, &request_directory_identity, false);
        sweep_directory(&response_directory, &response_directory_identity, true);
        let cursor = self
            .cursors
            .get(&request_directory_identity.object_id)
            .cloned()
            .unwrap_or_default();
        let mut next = std::collections::BTreeSet::new();
        let mut wrapped = std::collections::BTreeSet::new();
        let Ok(entries) = terminal_snapshots::snapshot_entries(&request_directory) else {
            return;
        };
        for (index, entry) in entries.enumerate() {
            if index >= MAX_DIRECTORY_ENTRIES {
                log::warn!("[terminal-snapshot] stage=publish code=directory_capacity");
                return;
            }
            let Ok((_, name)) = entry else {
                return;
            };
            let Ok(name) = name.into_string() else {
                continue;
            };
            if protocol_request_name(&name).is_none() {
                continue;
            }
            let candidates = if name.as_str() > cursor.as_str() {
                &mut next
            } else {
                &mut wrapped
            };
            candidates.insert(name);
            if candidates.len() > MAX_PER_POLL {
                candidates.pop_last();
            }
        }
        let selected: Vec<String> = if next.is_empty() {
            wrapped.into_iter().collect()
        } else {
            next.into_iter().collect()
        };
        if let Some(last) = selected.last() {
            self.cursors
                .insert(request_directory_identity.object_id, last.clone());
        }

        let Some(snapshot_state) =
            app.try_state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
        else {
            return;
        };
        for name in selected {
            if self.shutdown_owner.is_sealed() {
                break;
            }
            let source_key = format!(
                "host:{:016x}:{:016x}",
                request_directory_identity.object_id.volume,
                request_directory_identity.object_id.file
            );
            let Ok(ingress) = snapshot_state.try_admit_ingress(source_key) else {
                break;
            };
            let request_id = protocol_request_name(&name).unwrap_or_default().to_string();
            let request_path = request_directory.join(&name);
            let processing = request_directory.join(format!(
                ".{}.{}.terminal-snapshot-processing",
                request_id,
                Uuid::new_v4()
            ));
            if !private_regular_file_mode(&request_path) {
                continue;
            }
            let Ok(original) = crate::path_identity::verify_regular_file(&request_path) else {
                continue;
            };
            let Ok(reservation) = snapshot_state.reserve_existing_artifact(
                &request_directory,
                &request_directory_identity,
                original.object_id,
            ) else {
                break;
            };
            if crate::path_identity::publish_new_file_atomic(&request_path, &processing).is_err() {
                continue;
            }
            let Ok(claimed) = crate::path_identity::verify_regular_file(&processing) else {
                continue;
            };
            if !crate::path_identity::same_object(&original, &claimed) {
                continue;
            }
            if reservation
                .commit(processing.clone(), claimed.clone())
                .is_err()
            {
                safe_remove(&processing, &claimed);
                continue;
            }
            let snapshot_state = snapshot_state.inner().clone();
            let cleanup_state = Arc::clone(&snapshot_state);
            let cleanup_processing = processing.clone();
            let cleanup_claimed = claimed.clone();
            let cleanup_request_directory = request_directory.clone();
            let cleanup_request_identity = request_directory_identity.clone();
            let cleanup_response_directory = response_directory.clone();
            let cleanup_response_identity = response_directory_identity.clone();
            let cleanup: ScannerTaskCleanup = std::sync::Arc::new(move || {
                cleanup_tracked_artifact(&cleanup_state, &cleanup_processing, &cleanup_claimed);
                sweep_directory(&cleanup_request_directory, &cleanup_request_identity, false);
                sweep_directory(
                    &cleanup_response_directory,
                    &cleanup_response_identity,
                    true,
                );
            });
            let app = app.clone();
            let response_directory = response_directory.clone();
            let expected_root = root_identity.clone();
            let ownership = ScannerTaskOwnership::new();
            let phase = ownership.phase.clone();
            let audit_request_id = request_id.clone();
            #[cfg(test)]
            let task_start_control = self.shutdown_owner.take_next_task_start_control();
            let future = async move {
                #[cfg(test)]
                if let Some(control) = task_start_control {
                    control.enter().await;
                }
                let processed = crate::logging::catch_payload_future(process_claimed(
                    ownership,
                    &app,
                    snapshot_state,
                    processing,
                    response_directory,
                    expected_root,
                    claimed,
                    request_id,
                    ingress,
                ))
                .await;
                if processed.is_err() {
                    log::error!("[terminal-snapshot] stage=host_task code=internal");
                }
            };
            if !self.shutdown_owner.spawn_tracked(
                Some(audit_request_id.clone()),
                phase,
                cleanup.clone(),
                future,
            ) {
                let reason = if self.shutdown_owner.is_sealed() {
                    TerminalSnapshotReasonCode::AuthorityChanged
                } else {
                    TerminalSnapshotReasonCode::RateLimited
                };
                record_host_ingress_failure(Some(audit_request_id), None, reason);
                cleanup();
                break;
            }
        }
    }
}

fn private_directory_mode(path: &Path) -> bool {
    terminal_snapshots::private_snapshot_directory_mode(path)
}

fn private_regular_file_mode(path: &Path) -> bool {
    terminal_snapshots::private_snapshot_file_mode(path)
}
