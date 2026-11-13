use super::poller::{collect_project_refresh_requests, read_text_bom_tolerant, MailboxPoller};
use crate::config::instance_artifacts::PROJECT_REFRESH_REQUESTS_DIR_NAME;
use crate::config::instance_artifacts::SESSION_REQUESTS_DIR_NAME;
use crate::filesystem::phone_mailbox as mailbox_disk;
use crate::phone::events::{MailboxDeliveryEvents, MailboxRequestEvents};
use crate::phone::mailbox::host::{MailboxHost, MailboxSessionCreate};
use crate::phone::mailbox::response_observation::ResponseCompletionState;
use crate::phone::types::OutboxMessage;
use crate::session::create_contracts::CreateSelectionIntent;
use std::path::Path;
use std::path::PathBuf;

fn archive_id_is_plain(id: &str) -> bool {
    let stem = id
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ');
    let device = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name));
    let port = stem.get(..3).is_some_and(|prefix| {
        (prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
            && matches!(
                stem.get(3..),
                Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
            )
    });
    let mut components = Path::new(id).components();
    let plain = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    // Reserve 11 suffix bytes for .reason.txt within a 255-byte component.
    !(id.len() > 244
        || id.is_empty()
        || id == "."
        || id == ".."
        || id.contains(['/', '\\', ':', '\0'])
        || id
            .chars()
            .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*' | '\u{1}'..='\u{1f}'))
        || !plain
        || device
        || port)
}

fn sweep_stale_session_results(requests_dir: &Path) {
    // #1163: sweep stale result sidecars. The CLI consumes the result it
    // reads and waits at most `SESSION_REQUEST_RESULT_TIMEOUT` (30 s); a
    // result older than 5 minutes is litter from a timed-out or crashed
    // CLI and is removed here (worst case a few KB in an already-ephemeral,
    // ignored instance-artifact directory).
    if let Ok(rd) = mailbox_disk::mailbox_directory_paths(requests_dir) {
        for path in rd.filter_map(std::result::Result::ok) {
            if !crate::cli::create_agent::is_session_request_result_path(&path) {
                continue;
            }
            let stale = mailbox_disk::mailbox_path_metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= std::time::Duration::from_mins(5));
            if stale {
                let _ = mailbox_disk::remove_mailbox_artifact(&path);
            }
        }
    }
}

// #1163: answer each request with a result sidecar on every terminal
// path (malformed, spawn-rebuild failure, create Ok/Err). Write the
// result BEFORE deleting the request: a crash in between leaves a
// re-processable request on disk (the pre-existing hazard), never a
// lost already-written outcome.
fn write_session_result(result: crate::cli::create_agent::SessionRequestResult) {
    if let Err(e) = crate::cli::create_agent::write_session_request_result(&result) {
        log::warn!(
            "[session-requests] Failed to write result for '{}': {}",
            result.id,
            e
        );
    }
}

fn read_session_request(path: &Path) -> Option<crate::cli::create_agent::SessionRequest> {
    let content = match read_text_bom_tolerant(path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[session-requests] Failed to read {path:?}: {e}");
            return None;
        }
    };

    let request = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[session-requests] Failed to parse {path:?}: {e}");
            // #1163: answer with a rejected result so the CLI can
            // distinguish rejection from a missing app. The request
            // filename IS `<id>.json`, so the id comes from the stem.
            write_session_result(crate::cli::create_agent::SessionRequestResult {
                id: path
                    .file_stem()
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or("unknown")
                    .to_string(),
                status: crate::cli::create_agent::SessionRequestResultStatus::Rejected,
                session_id: None,
                error: Some(format!("malformed session request: {e}")),
            });
            // Delete malformed file
            let _ = mailbox_disk::remove_mailbox_artifact(path);
            return None;
        }
    };
    Some(request)
}

fn complete_session_request(
    request: &crate::cli::create_agent::SessionRequest,
    path: &Path,
    create_result: Result<crate::session::session::SessionInfo, String>,
) {
    match &create_result {
        Ok(info) => {
            log::info!(
                "[session-requests] Created session '{}' (id={})",
                request.session_name,
                info.id
            );
        }
        Err(e) => {
            log::error!(
                "[session-requests] Failed to create session '{}': {}",
                request.session_name,
                e
            );
        }
    }

    // #1163: answer the request with the terminal outcome, then delete.
    let result = match create_result {
        Ok(info) => crate::cli::create_agent::SessionRequestResult {
            id: request.id.clone(),
            status: crate::cli::create_agent::SessionRequestResultStatus::Created,
            session_id: Some(info.id),
            error: None,
        },
        Err(e) => crate::cli::create_agent::SessionRequestResult {
            id: request.id.clone(),
            status: crate::cli::create_agent::SessionRequestResultStatus::Rejected,
            session_id: None,
            error: Some(e.clone()),
        },
    };
    write_session_result(result);

    // Delete processed request file regardless of success/failure
    let _ = mailbox_disk::remove_mailbox_artifact(path);
}

impl MailboxPoller {
    /// Move an outbox message to outbox/delivered/ with token stripped.
    pub(crate) async fn move_to_delivered<S>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
    ) -> Result<(), String>
    where
        S: MailboxDeliveryEvents + ResponseCompletionState + Clone + Send + Sync + 'static,
    {
        let outbox = path.parent().ok_or("No parent dir")?;
        let delivered_dir = outbox.join("delivered");
        mailbox_disk::ensure_mailbox_archive_directory(&delivered_dir)
            .map_err(|e| format!("Failed to create delivered dir: {e}"))?;

        // Strip token before storing
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = if archive_id_is_plain(&msg.id) {
            crate::phone::types::outbox_delivered_receipt(outbox, &msg.id)
        } else {
            let origin = super::handoff::wake_claim_origin(path);
            let name = origin
                .as_deref()
                .unwrap_or(path)
                .file_name()
                .ok_or("No source basename for delivered archive")?
                .to_os_string();
            delivered_dir.join(name)
        };
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {e}"))?;
        mailbox_disk::write_mailbox_document(&dest, json)
            .map_err(|e| format!("Failed to write delivered file: {e}"))?;

        // Remove original
        mailbox_disk::remove_mailbox_artifact(path)
            .map_err(|e| format!("Failed to remove outbox file: {e}"))?;
        self.complete_response_delivery(app, path).await;

        log::info!("[mailbox] Message {} moved to delivered/", msg.id);
        Ok(())
    }

    /// Reject a message: move to outbox/rejected/ with reason, and notify the sender.
    pub(crate) async fn reject_message(
        &self,
        path: &Path,
        msg: &OutboxMessage,
        reason: &str,
    ) -> Result<(), String> {
        if !archive_id_is_plain(&msg.id) {
            Self::reject_raw_file(path, reason)?;
            self.settle_response_source(path, false);
            return Ok(());
        }

        let outbox = path.parent().ok_or("No parent dir")?;
        let rejected_dir = outbox.join("rejected");
        mailbox_disk::ensure_mailbox_archive_directory(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {e}"))?;

        // Write reason file FIRST — the CLI polls for this file to detect rejection
        let reason_path = crate::phone::types::outbox_rejected_reason(outbox, &msg.id);
        mailbox_disk::write_mailbox_document(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        // Then write the stripped message JSON
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = crate::phone::types::outbox_rejected_receipt(outbox, &msg.id);
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {e}"))?;
        mailbox_disk::write_mailbox_document(&dest, json)
            .map_err(|e| format!("Failed to write rejected file: {e}"))?;

        // Remove original
        mailbox_disk::remove_mailbox_artifact(path)
            .map_err(|e| format!("Failed to remove outbox file: {e}"))?;
        self.settle_response_source(path, false);

        log::warn!(
            "[mailbox] Message {} moved to rejected/: {}",
            msg.id,
            reason
        );
        Ok(())
    }

    /// Reject a raw file that cannot be parsed as `OutboxMessage`.
    pub(crate) fn reject_raw_file(path: &Path, reason: &str) -> Result<(), String> {
        let rejected_dir = path.parent().ok_or("No parent dir")?.join("rejected");
        mailbox_disk::ensure_mailbox_archive_directory(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {e}"))?;

        let filename = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("unknown.json");

        let dest = rejected_dir.join(filename);
        mailbox_disk::rename_mailbox_document(path, &dest)
            .or_else(|_| {
                mailbox_disk::copy_mailbox_document(path, &dest)
                    .and_then(|_| mailbox_disk::remove_mailbox_artifact(path))
            })
            .map_err(|e| format!("Failed to move file to rejected: {e}"))?;

        let stem = Path::new(filename)
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("unknown");
        let reason_path = rejected_dir.join(format!("{stem}.reason.txt"));
        mailbox_disk::write_mailbox_document(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        log::warn!("Rejected raw file {path:?}: {reason}");
        Ok(())
    }

    /// Poll ~/.agentscommander/project-refresh-requests/ for sidebar refresh requests.
    pub(crate) async fn poll_project_refresh_requests<S: MailboxRequestEvents + ?Sized>(
        &self,
        app: &S,
    ) {
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let requests_dir = config_dir.join(PROJECT_REFRESH_REQUESTS_DIR_NAME);
        let batch = collect_project_refresh_requests(&requests_dir);

        for payload in &batch.payloads {
            log::info!(
                "[project-refresh-requests] Emitting refresh: dir='{}' project='{}' reason='{}'",
                requests_dir.display(),
                payload.project_path,
                payload.reason
            );
            let _ = app.project_refresh(payload);
        }

        for path in batch.processed_paths {
            let _ = mailbox_disk::remove_mailbox_artifact(&path);
        }
    }

    /// Poll ~/.agentscommander/session-requests/ for launch requests from the CLI.
    pub(crate) async fn poll_session_requests<S: MailboxHost>(&self, app: &S) {
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let requests_dir = config_dir.join(SESSION_REQUESTS_DIR_NAME);
        if !mailbox_disk::mailbox_path_is_dir(&requests_dir) {
            return;
        }

        sweep_stale_session_results(&requests_dir);

        let entries: Vec<PathBuf> = match mailbox_disk::mailbox_directory_paths(&requests_dir) {
            Ok(rd) => rd
                .filter_map(std::result::Result::ok)
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                // #1163: result sidecars (`<id>.result.json`) are ANSWERS, not
                // requests; without this exclusion a leftover result file would
                // be parsed as a `SessionRequest` (parse fails -> spurious
                // rejected result written for it).
                .filter(|p| !crate::cli::create_agent::is_session_request_result_path(p))
                .collect(),
            Err(_) => return,
        };

        for path in entries {
            let Some(request) = read_session_request(&path) else {
                continue;
            };

            log::info!(
                "[session-requests] Processing: name='{}' cwd='{}' agent='{}'",
                request.session_name,
                request.cwd,
                request.agent_id
            );

            let session_mgr = app.sessions();
            let pty_mgr = app.routes();
            let cwd = crate::path_utils::normalize_windows_verbatim_path(&request.cwd);
            let (resolved_spawn, resolved_agent_host_shell) = {
                let settings = app.pty_settings();
                let cfg = settings.read().await;
                match crate::config::agent_command::build_configured_agent_spawn_for_cwd(
                    &cfg,
                    &request.agent_id,
                    &cwd,
                    request.requested_profile.as_deref(),
                ) {
                    Ok(Some(spawn)) => (
                        Some(spawn),
                        // #1271 - same-guard host shell: copied from the same
                        // config snapshot that built the spawn, before any await.
                        Some(crate::pty::backend::ResolvedAgentHostShell {
                            program: cfg.default_shell.clone(),
                            args: cfg.default_shell_args.clone(),
                        }),
                    ),
                    Ok(None) => (None, None),
                    Err(e) => {
                        log::error!(
                            "[session-requests] Failed to rebuild configured agent command for \
                            '{}': {}",
                            request.session_name,
                            e
                        );
                        write_session_result(crate::cli::create_agent::SessionRequestResult {
                            id: request.id.clone(),
                            status: crate::cli::create_agent::SessionRequestResultStatus::Rejected,
                            session_id: None,
                            error: Some(e.clone()),
                        });
                        let _ = mailbox_disk::remove_mailbox_artifact(&path);
                        continue;
                    }
                }
            };
            let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
                (
                    spawn.shell.clone(),
                    spawn.shell_args.clone(),
                    Some(spawn.trusted_agent_label.clone()),
                )
            } else {
                (request.shell.clone(), request.shell_args.clone(), None)
            };

            let create_result = app
                .create_session_inner(MailboxSessionCreate {
                    session_mgr,
                    pty_mgr,
                    shell,
                    shell_args,
                    cwd,
                    session_name: Some(request.session_name.clone()),
                    agent_id: Some(request.agent_id.clone()),
                    agent_label, // No agent label for legacy custom-shell fallback
                    skip_tooling_save: false, // Persist tooling
                    git_repos: Vec::new(),
                    skip_auto_resume: true, // CLI session-request is a fresh create
                    resolved_spawn,
                    resolved_agent_host_shell,
                    // #973 - headless caller: no terminal to measure, keep 120x30.
                    viewport: None,
                    selection_intent: CreateSelectionIntent::Background,
                })
                .await;
            complete_session_request(&request, &path, create_result);
        }
    }

    /// #786: poll `<config_dir>/coding-agent-requests/` for CLI mutation requests
    /// while the GUI runs. Scans bare `.json` files only (so `.processing`,
    /// `.tmp`, and the `results/` subdir are excluded). For each request, R8:
    /// write-lock `SettingsState`, clone a candidate, run the pure per-request
    /// handler (claim -> parse -> expiry -> apply -> write result -> delete
    /// `.processing`) against it, and on Applied swap the in-memory state and emit
    /// `coding_agent_settings_updated` AFTER the guard drops. `save_settings` is
    /// synchronous, so the write guard is never held across an await.
    pub(crate) async fn poll_coding_agent_requests<S: MailboxHost>(&self, app: &S) {
        use crate::config::coding_agent_mutations as ca;
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let (requests_dir, results_dir) = ca::coding_agent_queue_directories(&config_dir);
        let entries = ca::pending_coding_agent_requests(&requests_dir);
        if entries.is_empty() {
            return;
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);

        let state = app.pty_settings();
        for path in entries {
            let applied = {
                let mut s = state.write().await;
                let mut candidate = s.clone();
                let mut written_settings = None;
                let disposition = {
                    let mut save = |st: &crate::config::settings::AppSettings| {
                        let written = crate::config::settings::save_settings(st)?;
                        written_settings = Some(written.clone());
                        Ok(written)
                    };
                    ca::process_coding_agent_request(
                        &path,
                        &results_dir,
                        now_ms,
                        &mut candidate,
                        &mut save,
                    )
                };
                if let ca::RequestDisposition::Applied { op, agent_id } = disposition {
                    debug_assert!(
                        written_settings.is_some(),
                        "Applied implies save() succeeded"
                    );
                    *s = written_settings.unwrap_or_else(|| {
                        log::error!("coding-agent op Applied without a save; publishing candidate");
                        candidate
                    });
                    Some((op, agent_id))
                } else {
                    None
                }
            }; // write guard dropped here, before the emit

            if let Some((op, agent_id)) = applied {
                let _ = app.coding_agent_settings_updated(
                    &serde_json::json!({ "op": op, "agentId": agent_id }),
                );
                log::info!("[coding-agent-requests] applied op={op} agentId={agent_id:?}");
            }
        }
    }
}
