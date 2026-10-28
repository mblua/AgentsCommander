use super::handoff::{
    dedup_outbox_dirs_by_object_id, is_permanent_delivery_error, outbox_message_send_time,
    prune_retry_tracker, wake_claim_has_settled_receipt, wake_claim_origin, wake_claim_path,
    WakeHandoff, WakeOutcomeReport, WAKE_CLAIM_EXTENSION,
};
use super::notice::{
    retain_unarchived_session_dirs, OutboxOrigin, RetryState, MAX_DELIVERY_ATTEMPTS,
};
use super::poller::{deliver_claimed_wake, read_text_bom_tolerant, MailboxPoller, ValidatedWake};
use super::pty_claim::classify_outbox_document;
use super::settle::OutboxClassification;
use crate::config::settings::SettingsState;
use crate::phone::types::OutboxMessage;
use crate::session::manager::SessionManager;
use crate::AppOutbox;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

impl MailboxPoller {
    /// (#1399) A delivery-only poller for a detached worker. `deliver_wake` and
    /// its whole `&self` callee chain read `app.state::<...>()` (see the
    /// #791 note at the top of `deliver_wake`), so a fresh instance sharing
    /// the same message warnings is equivalent to `&self` for the tail. Test hooks
    /// are carried across because `MailboxTestHooks` is `Arc`-based and `Clone`.
    pub(crate) fn delivery_poller(&self) -> Self {
        let mut poller = Self::delivery_only(Arc::clone(&self.messages));
        poller.response_sources = self.response_sources.clone();
        #[cfg(test)]
        let poller = Self {
            test_hooks: self.test_hooks.clone(),
            ..poller
        };
        poller
    }

    /// Start the poller as a background task.
    pub fn start(mut self, app: tauri::AppHandle, shutdown: crate::shutdown::ShutdownSignal) {
        tauri::async_runtime::spawn(async move {
            // Initial poll without delay (matches original behavior)
            if let Err(e) = self.poll(&app).await {
                log::warn!("MailboxPoller error: {}", e);
            }
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.token().cancelled() => {
                        log::info!("[MailboxPoller] Shutdown signal received, stopping");
                        // (#1399) Detach instead of cancelling: dropping a
                        // JoinSet cancels its tasks, which would cut a wake
                        // mid-inject. Detached workers run to completion
                        // best-effort, and any claim cut off by process exit
                        // is returned by reclamation at the next start.
                        self.wake_workers.detach_all();
                        break;
                    }
                    _ = tokio::time::sleep(self.poll_interval) => {
                        if let Err(e) = self.poll(&app).await {
                            log::warn!("MailboxPoller error: {}", e);
                        }
                    }
                }
            }
        });
    }

    fn scrub_stale_pty_input_temps(
        &self,
        outbox: &Path,
        store: &crate::api::message_store::MessageStore,
    ) {
        let Ok(entries) = std::fs::read_dir(outbox) else {
            return;
        };
        for entry in entries.filter_map(Result::ok).take(1_024) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.ends_with(".pty-input-request-tmp") || !name.starts_with('.') {
                continue;
            }
            let Some(injection_id) = name.split('.').nth(1) else {
                continue;
            };
            if crate::phone::types::parse_canonical_uuid_v4(injection_id).is_err()
                || self.verified_pty_outbox_owner(&path).is_err()
            {
                continue;
            }
            let old_enough = std::fs::symlink_metadata(&path)
                .ok()
                .filter(|metadata| metadata.is_file())
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= std::time::Duration::from_secs(10 * 60));
            if !old_enough || crate::path_identity::verify_regular_file(&path).is_err() {
                continue;
            }
            let Ok(Some(_operation_lock)) = store.try_operation_lock(injection_id) else {
                continue;
            };
            if let Err(error) = std::fs::remove_file(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::debug!(
                        "[pty-input] stale request temp cleanup failed id={} \
                        code=artifact_unclaimed error_kind={:?}",
                        injection_id,
                        error.kind()
                    );
                }
            }
        }
    }

    /// (#1399) Return every claim with no live owner to the outbox. A claim is
    /// unowned iff it is absent from `self.live_claims`: at the first cycle
    /// the set is empty, so every claim left by a previous process is
    /// returned; afterwards a claim is skipped while its worker is running or
    /// its outcome is undrained. The caller runs this only for a verified
    /// outbox (R2), so an unverifiable spelling defers reclamation instead of
    /// resurrecting a live claim. Assumes one daemon per outbox (different
    /// builds never share one, because `config::agent_local_dir_name()` derives
    /// the outbox from the binary stem).
    pub(crate) fn reclaim_unowned_wake_claims(&self, _outbox_dir: &Path, claims: &[PathBuf]) {
        for claim in claims {
            if self.live_claims.contains(claim) {
                continue;
            }
            let Some(origin) = wake_claim_origin(claim) else {
                continue;
            };
            // Sound because every outbox writer names the file `<msg.id>.json`
            // (cli send / close_session / self_switch / self_clear / purge_wg /
            // raise_hand); a writer breaking that convention breaks this
            // receipt check.
            let Some(id) = origin.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            // The receipt proves a terminal outcome, not successful delivery. Share
            // this predicate with the owned-outcome drain so both paths treat
            // a failed observation as pending rather than absence.
            let settled = match wake_claim_has_settled_receipt(&origin) {
                Ok(settled) => settled,
                Err(error) => {
                    log::warn!(
                        "[mailbox] #1399 could not inspect receipts for unowned claim {}: {}",
                        id,
                        error
                    );
                    continue;
                }
            };
            if settled {
                // Preserve pending if an origin remains or its observation fails.
                // Receipt recovery has no authenticated full-return delivery proof.
                let origin_absent = matches!(origin.try_exists(), Ok(false));
                // (#1399 R6) Keep the Result: `remove_file` fails under exactly
                // the locked-handle condition that is ordinary on Windows, and
                // a log line reporting an action that did not happen costs an
                // hour in an incident. Self-heals on a later cycle either way.
                match std::fs::remove_file(claim) {
                    Ok(()) => {
                        if origin_absent {
                            self.settle_response_source(&origin, false);
                        }
                        log::warn!(
                            "[mailbox] #1399 dropped unowned claim {} (receipt already present)",
                            id
                        );
                    }
                    Err(error) => log::warn!(
                        "[mailbox] #1399 could not drop unowned claim {}: {}",
                        id,
                        error
                    ),
                }
            } else if let Err(error) = std::fs::rename(claim, &origin) {
                // Retried on the next cycle. This is the only reason a locked
                // file cannot wedge the message.
                log::warn!(
                    "[mailbox] #1399 could not return unowned claim {}: {}",
                    id,
                    error
                );
            } else {
                log::warn!(
                    "[mailbox] #1399 returned unowned claim {} to the outbox",
                    id
                );
            }
        }
    }

    /// (#1399) Reserve the target's lane, claim the file on disk, and hand the
    /// delivery tail to a detached worker.
    ///
    /// MUST stay a synchronous `fn`. Between the `rename` and the
    /// `live_claims` insert the claim exists on disk and is not yet owned;
    /// because there is no await point here and `poll()` drives the drain, the
    /// reclamation and this function in sequence on one task, nothing can
    /// observe that window. An await inside it would let reclamation treat the
    /// claim as unowned and rename it back under a live worker.
    #[cfg(test)]
    pub(crate) fn claim_and_spawn_wake<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: OutboxMessage,
    ) -> WakeHandoff {
        self.claim_and_spawn_validated_wake(
            app,
            path,
            ValidatedWake {
                message: msg,
                response: None,
            },
        )
    }

    pub(crate) fn claim_and_spawn_validated_wake<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        validated: ValidatedWake,
    ) -> WakeHandoff {
        let msg = &validated.message;
        let response_delivery = validated
            .response
            .as_ref()
            .map(|work| work.state.begin_delivery(&work.target));
        let response_delivery = match response_delivery {
            Some(Err(crate::pty::manager::ResponseCloseError::ClosingDeferred)) => {
                return WakeHandoff::LaneBusy
            }
            Some(Err(crate::pty::manager::ResponseCloseError::Unavailable)) | None => None,
            Some(Err(error)) => {
                return WakeHandoff::ClaimFailed(format!("response close admission: {:?}", error))
            }
            Some(Ok(reservation)) => Some(reservation),
        };
        // A queued or requeued outcome can outlive both its worker and lane.
        // Cap `live_claims` separately so pending reports cannot exceed the
        // same global limit.
        if self.wake_workers.len() >= crate::phone::wake_lanes::WAKE_WORKER_LIMIT
            || self.live_claims.len() >= crate::phone::wake_lanes::WAKE_WORKER_LIMIT
        {
            return WakeHandoff::LaneBusy;
        }
        let Some(reservation) = self.wake_lanes.try_reserve(&msg.to) else {
            return WakeHandoff::LaneBusy;
        };
        let claim = wake_claim_path(path);
        // (#1399 G8) A worker that finished after this cycle's drain has
        // already renamed the file back and freed its lane, but its outcome is
        // still queued. Re-claiming now would race that outcome against a path
        // this claim is about to invalidate. Refuse until the outcome is
        // recorded; this is what makes "at most one attempt per cycle" true.
        if self.live_claims.contains(&claim) {
            return WakeHandoff::LaneBusy;
        }
        if let Err(error) = std::fs::rename(path, &claim) {
            // The reservation drops here, so the lane is free again.
            return if error.kind() == std::io::ErrorKind::NotFound {
                WakeHandoff::LaneBusy // the file vanished; nothing to do
            } else {
                WakeHandoff::ClaimFailed(format!("Failed to claim outbox message: {}", error))
            };
        }
        let worker_app = app.clone();
        let outcomes = self.wake_outcomes_tx.clone();
        let origin = path.to_path_buf();
        let delivery = self.delivery_poller();
        let worker_claim = claim.clone();
        self.wake_workers.spawn(async move {
            let _reservation = reservation;
            let _response_delivery = response_delivery;
            // (#1399 R1) RAII, for the same reason `_reservation` is: an
            // unwind must not be able to skip the report.
            let mut report = WakeOutcomeReport::new(outcomes, origin.clone());
            let outcome =
                deliver_claimed_wake(&delivery, &worker_app, &worker_claim, &origin, &validated)
                    .await;
            report.set(outcome);
        });
        // Inserted AFTER `spawn` returns: if `spawn` itself panicked, an
        // earlier insert would leave an entry no guard can rescue. Safe
        // because the drain is upstream in this same single-task sequence, so
        // the worker's outcome cannot be recorded before this insert.
        self.live_claims.insert(claim);
        WakeHandoff::Spawned
    }

    /// (#1399) Join every finished worker so `JoinSet::len()` means "in
    /// flight" when the cap is checked; runs at the top of the cycle,
    /// mirroring the dispatcher's reap-before-dispatch.
    fn reap_wake_workers(&mut self) {
        while let Some(result) = self.wake_workers.try_join_next() {
            if let Err(error) = result {
                // `error`, not `warn`: a worker that died without returning
                // may have stranded its claim until its guard's outcome
                // drains, and the condition must be greppable.
                log::error!(
                    "[mailbox] #1399 wake worker task failed (a claim may be stranded until its \
                    outcome drains): {}",
                    error
                );
            }
        }
    }

    /// (#1399) Settle one finite snapshot of worker reports. Ownership ends only
    /// after the corresponding on-disk state is settled and the outcome can be
    /// recorded without racing reclamation or an absent terminal-rejection input.
    pub(crate) async fn drain_wake_outcomes(&mut self) {
        let mut reports = Vec::new();
        while let Ok(report) = self.wake_outcomes_rx.try_recv() {
            reports.push(report);
        }

        for (path, outcome) in reports {
            let claim = wake_claim_path(&path);
            // Observation is part of settlement. `Path::exists()` would turn every
            // metadata/access failure into `false`, release the ledger, and either
            // count against an absent origin or resurrect an unreadable receipt.
            let settlement = (|| -> Result<(bool, bool, bool), String> {
                let claim_present = claim.try_exists().map_err(|error| {
                    format!("could not inspect owned claim {:?}: {}", claim, error)
                })?;
                let origin_present = path.try_exists().map_err(|error| {
                    format!("could not inspect owned origin {:?}: {}", path, error)
                })?;
                let payload_receipt_present = wake_claim_has_settled_receipt(&path)?;
                let claim_has_receipt = claim_present && payload_receipt_present;

                if claim_present {
                    let (action, disk_result) = if claim_has_receipt {
                        // Claim plus receipt is authoritative. Never resurrect it.
                        ("delete", std::fs::remove_file(&claim))
                    } else {
                        // `record_message_outcome` requires the origin to exist on
                        // every retry/reject path, including panic attempt 10.
                        ("restore", std::fs::rename(&claim, &path))
                    };
                    disk_result.map_err(|error| {
                        format!("could not {} owned claim {:?}: {}", action, claim, error)
                    })?;
                }

                Ok((claim_present, origin_present, payload_receipt_present))
            })();

            let (claim_present, origin_present, payload_receipt_present) = match settlement {
                Ok(state) => state,
                Err(error) => {
                    log::warn!(
                        "[mailbox] #1399 could not settle owned claim {:?}: {}; outcome remains \
                        pending",
                        claim,
                        error
                    );
                    // The receiver snapshot is already finite, so this report is
                    // not observed again until a later drain and cannot spin here.
                    if self.wake_outcomes_tx.send((path, outcome)).is_err() {
                        // Structurally unreachable while `self` owns the receiver.
                        // Keep ledger ownership; durable disk state is the restart fallback.
                        log::error!(
                            "[mailbox] #1399 could not requeue outcome for owned claim {:?}",
                            claim
                        );
                    }
                    continue;
                }
            };
            // Receipt-wins recovery applies only while the claimed payload still
            // exists. A receipt plus a restored origin is the existing
            // `move_to_delivered` cleanup-failure state and must keep its Err.
            let claim_has_receipt = claim_present && payload_receipt_present;

            if !claim_present && !origin_present && !payload_receipt_present {
                // No in-scope worker path produces this state. Do not retain an
                // owner/report pair with no durable payload, but make external
                // deletion or corruption visible before preserving today's
                // outcome-recording behavior.
                log::error!(
                    "[mailbox] #1399 outcome has no claim, origin, or payload receipt: {:?}",
                    path
                );
            }

            self.live_claims.remove(&claim);
            if claim_has_receipt {
                // The claim was removed successfully after receipt observation.
                // Never turn delivered-or-rejected into return success authority,
                // or retire work while a restored origin remains actionable.
                if !origin_present {
                    self.settle_response_source(&path, false);
                }
                // Receipt-wins recovery is already the contract of general
                // reclamation. Clear retry state and do not redeliver.
                self.record_message_outcome(path, Ok(())).await;
            } else {
                self.record_message_outcome(path, outcome).await;
            }
        }
    }

    /// (#1399) Deterministic replacement for "the delivery finished inside
    /// poll()". Takes no `AppHandle`: joining and draining need none, and the
    /// drain is the SHARED `drain_wake_outcomes` rather than a second copy.
    #[cfg(test)]
    pub(crate) async fn settle_wake_workers(&mut self) {
        while let Some(result) = self.wake_workers.join_next().await {
            if let Err(error) = result {
                log::warn!("[mailbox] wake worker failed: {}", error);
            }
        }
        self.drain_wake_outcomes().await;
    }

    /// One poll cycle: scan all repo outbox dirs, process each message.
    pub(crate) async fn poll<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
    ) -> Result<(), String> {
        self.snapshot_scanner.begin_cycle();
        // (#1399) The two timing lines the issue authorizes: one at the top
        // of the cycle and one at the bottom, each reporting elapsed ms,
        // outboxes scanned, and messages processed. Together they prove the
        // scanner returns in milliseconds instead of holding the loop for
        // delivery time.
        let cycle_started = std::time::Instant::now();
        let mut messages_processed = 0usize;
        log::info!(
            "[mailbox] poll cycle started elapsed_ms={} outboxes=0 messages=0",
            cycle_started.elapsed().as_millis()
        );
        // (#1399) Reap, then drain, before any outbox work: joined workers
        // free cap slots, and an outcome's path can belong to any outbox, so
        // the drain is per-cycle, not per-outbox. Draining before the scans
        // also means a rename-back the drain rejects cannot be re-claimed in
        // the same cycle.
        self.reap_wake_workers();
        self.drain_wake_outcomes().await;
        if let Some(state) = app.try_state::<crate::api::message_store::MessageStoreState>() {
            if let Ok(store) = &state.store {
                let active = state.active_operations.snapshot();
                if store
                    .recover_pty_input_runtime_offloaded(active, chrono::Utc::now())
                    .await
                    .is_err()
                {
                    log::warn!("[pty-input] runtime recovery failed code=store_transient");
                }
                if store
                    .compact_pty_terminal_maintenance_offloaded(chrono::Utc::now(), 64)
                    .await
                    .is_err()
                {
                    log::warn!("[pty-input] compaction failed code=store_transient");
                }
            }
        }
        let settings = app.state::<SettingsState>();
        let (repo_paths, archived) = {
            let cfg = settings.read().await;
            (
                cfg.project_paths.clone(),
                cfg.archived_project_paths.clone(),
            )
        };

        // Also scan CWDs of active sessions for repos not in settings
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let session_dirs = {
            let mgr = session_mgr.read().await;
            mgr.get_sessions_working_dirs().await
        };
        let snapshot_session_dirs = session_dirs.clone();
        let archived_for_snapshot = archived.clone();
        let session_dirs = if archived.is_empty() {
            session_dirs
        } else {
            tokio::task::spawn_blocking(move || {
                let roots = crate::config::sessions_persistence::normalize_project_roots(&archived);
                retain_unarchived_session_dirs(session_dirs, &roots)
            })
            .await
            .map_err(|e| format!("archived mailbox-session filter task failed: {}", e))?
        };

        let mut startup_project_paths = repo_paths.clone();
        for path in &archived_for_snapshot {
            if !startup_project_paths.contains(path) {
                startup_project_paths.push(path.clone());
            }
        }
        let mut all_paths: Vec<String> = repo_paths;
        for (_, dir) in &session_dirs {
            if !all_paths.contains(dir) {
                all_paths.push(dir.clone());
            }
        }

        // One bounded startup sweep covers canonical Root plus verified WG
        // replicas under active and archived projects. Fresh artifacts are
        // registered so the monotonic cleanup task continues after this poll.
        if self.snapshot_scanner.startup_sweep_pending() {
            if let Some(state) =
                app.try_state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
            {
                let mut startup_objects = std::collections::HashSet::new();
                if let Ok(root) = crate::config::root_agent::root_agent_dir() {
                    let root = PathBuf::from(root);
                    if let Ok(identity) = crate::path_identity::verify_directory(&root) {
                        if startup_objects.insert(identity.object_id) {
                            self.snapshot_scanner.startup_sweep_root(&state, &root);
                        }
                    }
                }
                let mut target_count = 0usize;
                for project in &startup_project_paths {
                    let Ok(targets) =
                        crate::config::teams::discover_verified_terminal_snapshot_targets(
                            std::slice::from_ref(project),
                        )
                    else {
                        continue;
                    };
                    for target in targets {
                        target_count = target_count.saturating_add(1);
                        if target_count > 4_096 {
                            break;
                        }
                        let Ok(identity) =
                            crate::path_identity::verify_directory(&target.replica_root)
                        else {
                            continue;
                        };
                        if startup_objects.insert(identity.object_id) {
                            self.snapshot_scanner
                                .startup_sweep_root(&state, &target.replica_root);
                        }
                    }
                    if target_count > 4_096 {
                        break;
                    }
                }
            }
            self.snapshot_scanner.finish_startup_sweep();
        }

        // Snapshot ingress derives exact replica roots before checking ordinary
        // outbox spellings, so a live session CWD below its replica cannot make
        // the dedicated protocol invisible. Physical roots are scanned once.
        let mut snapshot_root_objects = std::collections::HashSet::new();
        for (_, discovered) in &snapshot_session_dirs {
            let Some(requester_root) =
                crate::phone::terminal_snapshot::verified_requester_root_from_discovered_path(
                    Path::new(discovered),
                )
            else {
                continue;
            };
            let Ok(identity) = crate::path_identity::verify_directory(&requester_root) else {
                continue;
            };
            if snapshot_root_objects.insert(identity.object_id) {
                self.snapshot_scanner.scan_root(app, &requester_root);
            }
        }

        // Include the instance-private app outbox
        let app_outbox = app.state::<AppOutbox>();
        let app_outbox_path = app_outbox.path().to_string();

        // Collect all outbox directories to scan
        let mut outbox_dirs: Vec<PathBuf> = all_paths
            .iter()
            .map(|p| {
                Path::new(p)
                    .join(crate::config::agent_local_dir_name())
                    .join("outbox")
            })
            .collect();
        // (#1399 R9) Dedup BEFORE the app outbox is pushed: `retain` keeps the
        // first occurrence, so running it over the full list could drop the
        // app entry in favour of an aliasing project spelling and silently
        // turn `is_app_outbox` false. This ordering makes that structurally
        // impossible.
        dedup_outbox_dirs_by_object_id(&mut outbox_dirs);
        outbox_dirs.push(PathBuf::from(&app_outbox_path));

        // #2232 phase 7: the Co-managed provenance queues of every room an
        // active session lives in. Each is its own scanned directory with its
        // own origin, never an outbox subdirectory (the sweep skips those).
        let mut queue_dirs: Vec<PathBuf> = Vec::new();
        for path in &all_paths {
            let Some(room_root) = crate::config::co_managed::room_root_for_path(Path::new(path))
            else {
                continue;
            };
            let queue = crate::config::co_managed::queue_dir(&room_root);
            if !queue_dirs.contains(&queue) {
                queue_dirs.push(queue);
            }
        }

        let mut scan_dirs: Vec<(PathBuf, OutboxOrigin)> = outbox_dirs
            .into_iter()
            .map(|dir| {
                let origin = if dir.as_path() == Path::new(&app_outbox_path) {
                    OutboxOrigin::Master
                } else {
                    OutboxOrigin::Replica
                };
                (dir, origin)
            })
            .collect();
        scan_dirs.extend(
            queue_dirs
                .into_iter()
                .map(|dir| (dir, OutboxOrigin::CoManaged)),
        );

        for (outbox_dir, origin) in &scan_dirs {
            if !outbox_dir.is_dir() {
                continue;
            }
            let is_app_outbox = *origin == OutboxOrigin::Master;
            if let Some(state) = app.try_state::<crate::api::message_store::MessageStoreState>() {
                if let Ok(store) = &state.store {
                    self.scrub_stale_pty_input_temps(outbox_dir, store);
                }
            }

            // (#1399) One walk partitions the outbox root by extension:
            // messages to scan and claim markers to reclaim. Deliberately
            // unbounded, exactly as today's scan is.
            let mut entries: Vec<PathBuf> = Vec::new();
            let mut claims: Vec<PathBuf> = Vec::new();
            match std::fs::read_dir(outbox_dir) {
                Ok(rd) => {
                    for path in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
                        // Skip files in subdirectories (delivered/, rejected/)
                        if path.parent() != Some(outbox_dir.as_path()) {
                            continue;
                        }
                        match path.extension().and_then(|s| s.to_str()) {
                            Some("json") => entries.push(path),
                            Some(WAKE_CLAIM_EXTENSION) => claims.push(path),
                            _ => {}
                        }
                    }
                }
                Err(_) => continue,
            }
            // (#1399 R2) Reclamation renames files a live worker may own, and
            // the ownership test is path-keyed, so it runs only where the
            // outbox identity is established: an unverifiable spelling of an
            // aliased outbox can never resurrect a live claim. An outbox that
            // fails verification cannot deliver anyway (the hardened reader
            // rejects its messages for the same reason), so nothing that works
            // today is lost. Reclaimed files are re-scanned on the NEXT cycle;
            // `entries` for this one is already collected.
            if crate::path_identity::verify_directory(outbox_dir).is_ok() {
                self.reclaim_unowned_wake_claims(outbox_dir, &claims);
            }

            // (#1399) Process a batch in send order. `read_dir` yields NTFS
            // filename order over random UUID names, so today's order is
            // arbitrary; once the per-target lane defers the rest of a
            // target's queue, that arbitrary order becomes the ADMISSION
            // decision, re-rolled every cycle, and an already-queued message
            // could be passed over indefinitely by newer arrivals whose names
            // sort lower. `sort_by_cached_key` reads each file at most once;
            // `None` sorts first so an unreadable document still reaches the
            // existing rejection path promptly; the path tie-breaker makes
            // equal timestamps deterministic. The length guard keeps the
            // steady state (zero or one message) at exactly zero extra reads.
            if entries.len() > 1 {
                entries.sort_by_cached_key(|path| (outbox_message_send_time(path), path.clone()));
            }

            for path in entries {
                messages_processed += 1;
                let standard_content = match classify_outbox_document(&path) {
                    OutboxClassification::PrivilegedCandidate { bytes, identity } => {
                        if *origin == OutboxOrigin::CoManaged {
                            // #2232 phase 7: the Co-managed origin may express
                            // exactly one thing, a wake carrying a pointer. A
                            // privileged PTY-input envelope is rejected before
                            // any authority resolution; the queue grants no
                            // other capability.
                            let _ = Self::reject_raw_file(
                                &path,
                                "Co-managed origin may carry only a wake; privileged PTY input \
                                    is rejected",
                            );
                            self.retry_tracker.remove(&path);
                            continue;
                        }
                        self.process_pty_input_file(app, &path, is_app_outbox, &bytes, &identity)
                            .await;
                        self.retry_tracker.remove(&path);
                        continue;
                    }
                    OutboxClassification::InvalidDocument => {
                        self.reject_malformed_pty_candidate(&path);
                        self.retry_tracker.remove(&path);
                        continue;
                    }
                    OutboxClassification::Standard(content) => content,
                };
                let mut deferred: Option<ValidatedWake> = None;
                let outcome = self
                    .process_message_content(
                        app,
                        &path,
                        *origin,
                        &standard_content,
                        Some(&mut deferred),
                    )
                    .await;
                match outcome {
                    Ok(()) => match deferred {
                        // Validated and handed off (or the lane was busy): the
                        // retry state for `path` must survive until the worker
                        // reports.
                        Some(msg) => match self.claim_and_spawn_validated_wake(app, &path, msg) {
                            WakeHandoff::Spawned => {}
                            WakeHandoff::LaneBusy => {}
                            WakeHandoff::ClaimFailed(e) => {
                                self.record_message_outcome(path, Err(e)).await
                            }
                        },
                        // Settled inline: delivered, rejected, or handled by an
                        // action branch.
                        None => self.record_message_outcome(path, Ok(())).await,
                    },
                    Err(e) => self.record_message_outcome(path, Err(e)).await,
                }
            }
        }

        log::info!(
            "[mailbox] poll cycle done elapsed_ms={} outboxes={} messages={}",
            cycle_started.elapsed().as_millis(),
            scan_dirs.len(),
            messages_processed
        );

        self.snapshot_scanner.finish_cycle();

        // (#1399) A live claim owns its retry state. Otherwise prune only after
        // fallible probes prove both durable spellings absent, so counted state
        // remains able to reach MAX_DELIVERY_ATTEMPTS.
        prune_retry_tracker(
            &mut self.retry_tracker,
            &self.live_claims,
            #[cfg(test)]
            None,
        );

        // Poll project-refresh-requests directory from create-agent-matrix CLI.
        self.poll_project_refresh_requests(app).await;

        // Poll session-requests directory (from create-agent CLI)
        self.poll_session_requests(app).await;

        // #786: poll coding-agent-requests (from `coding-agent` CLI mutations).
        self.poll_coding_agent_requests(app).await;

        Ok(())
    }

    /// (#1399) The retry/reject bookkeeping for one outbox message: `Ok`
    /// clears retry state, `Err` counts an attempt and rejects at the attempt
    /// ceiling. Called inline for a message the scanner settled itself, and
    /// from the outcome drain for a message a worker settled. Takes `path`
    /// owned so every expression in the block stays textually identical.
    pub(crate) async fn record_message_outcome(
        &mut self,
        path: PathBuf,
        outcome: Result<(), String>,
    ) {
        match outcome {
            Ok(()) => {
                self.retry_tracker.remove(&path);
            }
            Err(e) => {
                // #2336 - the typing hold shares the menu guard's exemption: a
                // deferred wake is "not yet", never an attempt, and the file is
                // already back at its original path (the claim restore ran
                // before this bookkeeping).
                let held_by = if crate::pty::menu_guard::is_menu_guard_deferred_error(&e) {
                    Some("menu guard")
                } else if crate::pty::menu_guard::is_typing_hold_deferred_error(&e) {
                    Some("typing hold")
                } else if crate::api::message_store::is_response_closing_deferred(&e) {
                    Some("response close")
                } else {
                    None
                };
                if let Some(held_by) = held_by {
                    log::debug!(
                        "[mailbox] message delivery deferred by {} {:?}: {}",
                        held_by,
                        path,
                        e
                    );
                    return;
                }
                let is_permanent = is_permanent_delivery_error(&e);
                let should_reject = is_permanent || {
                    let state = self
                        .retry_tracker
                        .entry(path.clone())
                        .or_insert(RetryState {
                            attempt_count: 0,
                            logged: false,
                        });
                    state.attempt_count += 1;

                    if !state.logged {
                        log::warn!(
                            "Failed to process outbox message {:?} (attempt {}): {}",
                            path,
                            state.attempt_count,
                            e
                        );
                        state.logged = true;
                    } else {
                        log::debug!(
                            "Retry {} for outbox message {:?}: {}",
                            state.attempt_count,
                            path,
                            e
                        );
                    }

                    state.attempt_count >= MAX_DELIVERY_ATTEMPTS
                };

                if should_reject {
                    let reason = if is_permanent {
                        e.clone()
                    } else {
                        let attempts = self
                            .retry_tracker
                            .get(&path)
                            .map(|s| s.attempt_count)
                            .unwrap_or(0);
                        format!(
                            "Undeliverable after {} attempts. Last error: {}",
                            attempts, e
                        )
                    };

                    // §130-stuck-file: on read failure (e.g. non-UTF-8 non-BOM file),
                    // fall back to `reject_raw_file` so the file is moved to `rejected/`
                    // instead of looping forever with `attempt_count >= MAX`.
                    let rejected = match read_text_bom_tolerant(&path) {
                        Ok(content) => {
                            if let Ok(msg) = serde_json::from_str::<OutboxMessage>(&content) {
                                self.reject_message(&path, &msg, &reason).await.is_ok()
                            } else {
                                Self::reject_raw_file(&path, &reason).is_ok()
                            }
                        }
                        Err(_) => Self::reject_raw_file(&path, &reason).is_ok(),
                    };

                    if rejected {
                        self.retry_tracker.remove(&path);
                    } else {
                        log::error!("Failed to reject outbox message {:?}; will retry", path);
                    }
                }
            }
        }
    }
}
