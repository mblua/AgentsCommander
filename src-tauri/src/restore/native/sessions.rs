use super::{
    attach_persisted_telegram_if_configured, collect_restart_resume_target, commands,
    restore_session_should_become_active, session_spawn_recipe, session_wake_decision,
    sessions_persistence, skip_auto_resume_for_restore, RestoreContext, RestoreProgress,
};

impl RestoreContext {
    pub(super) async fn restore_session(
        &self,
        ps: &sessions_persistence::PersistedSession,
        archived_roots: &[String],
        progress: &mut RestoreProgress,
    ) {
        // Skip sessions whose CWD no longer exists (permanent failure)
        if !crate::filesystem::directories::restore_cwd_exists(std::path::Path::new(
            &ps.working_directory,
        )) {
            log::warn!(
                "Skipping restore of '{}': CWD '{}' no longer exists",
                ps.name,
                ps.working_directory
            );
            // §1295 site C (restore-skip): append ONE archive
            // record BEFORE the `continue`. The restore task is
            // async and holds no `sessions_save_lock` here, so we
            // use the locking public variant (S4). The record is
            // written before the continue unchanged; it does not
            // touch sessions.json (site C leaves the row's disk
            // fate to the next persist, §224 G5).
            let config_dir = crate::config::config_dir();
            if let Some(config_dir) = config_dir {
                sessions_persistence::append_orphan_archive_record(
                    &config_dir,
                    "restoreCwdMissing",
                    "archived",
                    ps,
                )
                .await;
            }
            return;
        }

        // #248 — decide wake vs defer for this session.
        // §DR2: use `agent_fqn_from_path` so WG replicas get project-precise
        // team membership and coordinator checks. Strict `is_coordinator`
        // (§AR2-strict) requires the FQN to avoid cross-project flag leaks.
        let (agent_name, live_is_coord, is_coord, archived_session, persisted_working, wake) =
            session_wake_decision(
                ps,
                &self.teams,
                archived_roots,
                self.setting_on,
                &self.settings_snapshot,
            );

        if !wake {
            self.restore_deferred_session(ps, &agent_name, is_coord, archived_session, progress)
                .await;
            return;
        }

        self.restore_live_session(ps, is_coord, live_is_coord, persisted_working, progress)
            .await;
    }

    async fn restore_deferred_session(
        &self,
        ps: &sessions_persistence::PersistedSession,
        agent_name: &str,
        is_coord: bool,
        archived_session: bool,
        progress: &mut RestoreProgress,
    ) {
        // Defer: create a dormant Session record (no PTY, status = Exited(0)).
        match self
            .restore_transaction_for_task
            .restore_dormant_inline(crate::session::selection::DormantRestoreRequest {
                persisted: ps.clone(),
                working_directory: ps.working_directory.clone(),
                is_coordinator: is_coord,
                is_root_agent: false,
            })
            .await
        {
            Ok(info) => {
                // Grinch Z5 — debug, not info: under the new default every
                // persisted session lands here, and an info-level line per
                // session creates a "mass defer" wall in startup logs that
                // looks like an alarm. The end-of-loop info summary below
                // carries the load-bearing signal.
                log::debug!(
                    "Deferred session '{}' on startup (agent: {}, is_coord: {}, \
                        setting: {}, persisted_status: {:?}, was_detached: {})",
                    ps.name,
                    agent_name,
                    is_coord,
                    self.setting_on,
                    ps.status,
                    ps.was_detached
                );
                progress.n_deferred += 1;
                // Preserve `was_active` for the post-loop active-switch:
                // a deferred session can still be the persisted-active one.
                // The post-loop branching (Fix A) ensures `set_active_only`
                // is used (not `switch_session`), so the dormant status
                // survives selection.
                if restore_session_should_become_active(ps.was_active, archived_session) {
                    progress.active_id = Some(info.id);
                }
            }
            Err(e) => {
                log::error!("Failed to create deferred session '{}': {}", ps.name, e);
                progress.failed_recoverable.push(ps.clone());
            }
        }
    }

    async fn restore_live_session(
        &self,
        ps: &sessions_persistence::PersistedSession,
        is_coord: bool,
        live_is_coord: bool,
        persisted_working: bool,
        progress: &mut RestoreProgress,
    ) {
        // Wake: rebuild configured-agent sessions from the persisted recipe,
        // while custom-shell records keep their materialized shell args.
        let resolved_spawn = match session_spawn_recipe(
            ps,
            &self.settings_snapshot,
            &mut progress.failed_recoverable,
        ) {
            Ok(spawn) => spawn,
            Err(()) => return,
        };
        // #1271 - rebuilt at restore time from the SAME settings
        // snapshot that re-resolved the agent command (no
        // persistence, same rationale as the root-agent restore).
        let resolved_agent_host_shell = if resolved_spawn.is_some() {
            Some(crate::pty::backend::ResolvedAgentHostShell {
                program: self.settings_snapshot.default_shell.clone(),
                args: self.settings_snapshot.default_shell_args.clone(),
            })
        } else {
            None
        };
        let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
            (
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                Some(spawn.trusted_agent_label.clone()),
            )
        } else {
            (
                ps.shell.clone(),
                ps.shell_args.clone(),
                ps.agent_label.clone(),
            )
        };

        // Wake: full PTY restore inside the restore transaction.
        match commands::session::create_session_inner_for_restore(
            &self.restore_transaction_for_task,
            &self.session_mgr_clone,
            &self.pty_mgr_clone,
            shell,
            shell_args,
            ps.working_directory.clone(),
            Some(ps.name.clone()),
            ps.agent_id.clone(),
            agent_label,
            false, // Persist tooling on restore
            ps.git_repos.clone(),
            // (#630/#631) resume unless restarted fresh
            skip_auto_resume_for_restore(ps.start_fresh_on_restore),
            resolved_spawn,
            resolved_agent_host_shell,
            // #973 - headless caller: no terminal to measure, keep 120x30.
            None,
            Some(ps.start_fresh_on_restore),
            is_coord
                .then(|| {
                    crate::commands::session::carry_communication_for_restart(
                        ps.communication.clone(),
                        ps.start_fresh_on_restore,
                    )
                })
                .flatten(),
            ps.binding_metadata()
                .expect("restore binding validated before spawn"),
            Some(ps),
        )
        .await
        {
            Ok(info) => {
                self.finish_live_session(
                    ps,
                    info,
                    is_coord,
                    live_is_coord,
                    persisted_working,
                    progress,
                )
                .await;
            }
            Err(e) => {
                log::error!("Failed to restore session '{}': {}", ps.name, e);
                // Preserve for next startup attempt (CWD exists, transient failure)
                progress.failed_recoverable.push(ps.clone());
            }
        }
    }

    async fn finish_live_session(
        &self,
        ps: &sessions_persistence::PersistedSession,
        info: crate::session::session::SessionInfo,
        is_coord: bool,
        live_is_coord: bool,
        persisted_working: bool,
        progress: &mut RestoreProgress,
    ) {
        if ps.was_active {
            progress.active_id = Some(info.id.clone());
        }
        progress.n_woken += 1;
        // (#630/#631) Restore-decision trace (INFO during stabilization).
        log::info!(
            "[restore] woke '{}' (is_coord={}, live_is_coord={}, \
                    start_fresh_on_restore={})",
            ps.name,
            is_coord,
            live_is_coord,
            ps.start_fresh_on_restore
        );
        if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
            attach_persisted_telegram_if_configured(
                &self.app_handle,
                uuid,
                ps.telegram_bot_id.as_deref(),
            )
            .await;
        }

        // (#1793) Pair the row with the session it just became;
        // both are in hand, so no correlation key is needed.
        collect_restart_resume_target(
            &info.id,
            ps,
            &self.settings_snapshot,
            is_coord,
            persisted_working,
            &self.restart_resume_targets_for_body,
        );

        // Phase 3 restore: reconstruct detach state for the live session.
        // Deferred sessions (handled above with a `continue`) never reach
        // this branch, so R.9's "skip detached-window spawn for deferred"
        // guard is enforced structurally by this code path.
        self.restore_detached_session(ps, &info.id, false).await;
    }

    pub(super) async fn restore_detached_session(
        &self,
        ps: &sessions_persistence::PersistedSession,
        session_id: &str,
        root: bool,
    ) {
        if ps.was_detached {
            if let Ok(uuid) = uuid::Uuid::parse_str(session_id) {
                // Restore geometry independently. The detach transaction
                // commits persisted intent only after the window and PTY
                // rechecks pass.
                {
                    let mgr = self.session_mgr_clone.read().await;
                    if let Some(ref geo) = ps.detached_geometry {
                        mgr.set_detached_geometry(uuid, geo.clone()).await;
                    }
                }

                let detached_result = commands::window::execute_detach_transaction(
                    &self.restore_transaction_for_task,
                    uuid,
                    ps.detached_geometry.clone(),
                    true,
                )
                .await;
                if let Err(e) = detached_result {
                    if root {
                        log::warn!(
                            concat!(
                                "[restore] detach_terminal_inner failed for root agent '{}': {} — ",
                                "session stays live (attached)"
                            ),
                            ps.name,
                            e
                        );
                    } else {
                        log::warn!(
                            "[restore] detach_terminal_inner failed for '{}': {} — \
                                session stays live (attached)",
                            ps.name,
                            e
                        );
                    }
                }
            }
        }
    }
}
