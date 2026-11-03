use super::{
    attach_persisted_telegram_if_configured, commands, execute_destroy_transaction,
    root_spawn_recipe, sessions_persistence, should_auto_create_root_agent_on_first_restore,
    should_wake_root_agent_on_restore, DestroyRequest, DestructionSource, RestoreContext,
    RestoreProgress, TrustedCreateIntent,
};

impl RestoreContext {
    pub(super) async fn restore_root(
        &self,
        persisted: &[sessions_persistence::PersistedSession],
        progress: &mut RestoreProgress,
    ) -> bool {
        let root_agent_path = match crate::config::root_agent::ensure_root_agent_dir() {
            Ok(path) => Some(path),
            Err(e) => {
                log::error!(
                    "[root-agent] Failed to provision root agent during restore: {}",
                    e
                );
                None
            }
        };
        let root_ps = persisted
            .iter()
            .find(|ps| {
                ps.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&ps.working_directory)
            })
            .cloned();

        if let Some(root_path) = root_agent_path.clone() {
            if self.shutdown_for_body.token().is_cancelled() {
                // 14.4 - exit during restore: never spawn a root PTY
                // once shutdown is underway (it would be orphaned until
                // the next boot's container-orphan cleanup). Keep the
                // row for next boot via the progress.failed_recoverable merge.
                if let Some(ps) = root_ps.as_ref() {
                    progress.failed_recoverable.push(ps.clone());
                }
            } else {
                match root_ps.as_ref() {
                    None => {
                        if !self.restore_first_root(&root_path, progress).await {
                            return false;
                        }
                    }
                    Some(ps) if ps.binding_metadata().is_err() => {
                        log::error!("[restore] ambiguous Root binding; preserving recipe");
                        progress.failed_recoverable.push(ps.clone());
                    }
                    Some(ps) if should_wake_root_agent_on_restore(ps.status.as_ref()) => {
                        self.restore_awake_root(ps, &root_path, progress).await;
                    }
                    Some(ps) => {
                        self.restore_dormant_root(ps, root_path, progress).await;
                    }
                }
            }
        } else if let Some(ps) = root_ps.as_ref() {
            progress.failed_recoverable.push(ps.clone());
        }

        true
    }

    async fn restore_first_root(&self, root_path: &str, progress: &mut RestoreProgress) -> bool {
        let last_coding_agent = crate::config::root_agent::read_last_coding_agent(root_path);
        let should_auto_create = {
            let cfg = match self.settings_state_clone.current_read().await {
                Ok(cfg) => cfg,
                Err(error) => {
                    log::debug!("[restore] settings unavailable: {error}");
                    return false;
                }
            };
            should_auto_create_root_agent_on_first_restore(&cfg, last_coding_agent.as_deref())
        };

        if should_auto_create {
            match commands::session::execute_root_transaction(
                &self.restore_transaction_for_task,
                commands::session::RootJobRequest {
                    requested_agent_id: None,
                    requested_profile: None,
                    skip_auto_resume_for_new_session: true,
                    intent: TrustedCreateIntent::Background,
                    select_after: false,
                },
            )
            .await
            {
                Ok(_) => progress.n_woken += 1,
                Err(e) => log::error!("[root-agent] Failed to auto-create root session: {}", e),
            }
        } else {
            log::info!(
                "[root-agent] Skipping startup auto-create: no resolvable \
                            coding agent is configured"
            );
        }

        true
    }

    async fn restore_awake_root(
        &self,
        ps: &sessions_persistence::PersistedSession,
        root_path: &str,
        progress: &mut RestoreProgress,
    ) {
        let existing_root = {
            let mgr = self.session_mgr_clone.read().await;
            mgr.list_sessions().await.into_iter().find(|s| {
                s.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&s.working_directory)
            })
        };
        let mut should_create = true;
        if let Some(existing) = existing_root {
            if matches!(
                existing.status,
                crate::session::session::SessionStatus::Exited(_)
            ) {
                if let Ok(uuid) = uuid::Uuid::parse_str(&existing.id) {
                    let stale_destroy = execute_destroy_transaction(
                        &self.restore_transaction_for_task,
                        DestroyRequest {
                            ids: vec![uuid],
                            source: DestructionSource::BackgroundCleanup,
                            force_destroy_root: true,
                        },
                    )
                    .await
                    .and_then(|outcome| {
                        outcome
                            .succeeded(uuid)
                            .then_some(())
                            .ok_or_else(|| "stale dormant Root was not destroyed".to_string())
                    });
                    if let Err(e) = stale_destroy {
                        log::warn!(
                            "[root-agent] Failed to force-destroy stale \
                                        dormant root during restore: {}",
                            e
                        );
                    }
                }
            } else {
                if ps.was_active {
                    progress.active_id = Some(existing.id.clone());
                }
                progress.n_woken += 1;
                if let Ok(uuid) = uuid::Uuid::parse_str(&existing.id) {
                    attach_persisted_telegram_if_configured(
                        &self.app_handle,
                        uuid,
                        ps.telegram_bot_id.as_deref(),
                    )
                    .await;
                    commands::session::seed_root_last_prompt(
                        &self.app_handle,
                        &self.session_mgr_clone,
                        uuid,
                        ps,
                    )
                    .await;
                }
                should_create = false;
            }
        }
        if should_create {
            self.create_awake_root(ps, root_path, progress).await;
        }
    }

    async fn create_awake_root(
        &self,
        ps: &sessions_persistence::PersistedSession,
        root_path: &str,
        progress: &mut RestoreProgress,
    ) {
        let (rebuild_failed, resolved_spawn, resolved_agent_host_shell) = root_spawn_recipe(
            ps,
            &self.settings_snapshot,
            root_path,
            &mut progress.failed_recoverable,
        );
        if !rebuild_failed {
            match commands::session::create_persisted_root_for_restore(
                &self.restore_transaction_for_task,
                &self.session_mgr_clone,
                &self.pty_mgr_clone,
                ps,
                root_path.to_owned(),
                resolved_spawn,
                resolved_agent_host_shell,
            )
            .await
            {
                Ok(info) => {
                    if ps.was_active {
                        progress.active_id = Some(info.id.clone());
                    }
                    progress.n_woken += 1;
                    if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                        attach_persisted_telegram_if_configured(
                            &self.app_handle,
                            uuid,
                            ps.telegram_bot_id.as_deref(),
                        )
                        .await;
                    }

                    self.restore_detached_session(ps, &info.id, true).await;
                }
                Err(e) => {
                    log::error!(
                        "[root-agent] Failed to restore root session '{}': {}",
                        ps.name,
                        e
                    );
                    progress.failed_recoverable.push(ps.clone());
                }
            }
        }
    }

    async fn restore_dormant_root(
        &self,
        ps: &sessions_persistence::PersistedSession,
        root_path: String,
        progress: &mut RestoreProgress,
    ) {
        let existing_root = {
            let mgr = self.session_mgr_clone.read().await;
            mgr.list_sessions().await.into_iter().find(|s| {
                s.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&s.working_directory)
            })
        };
        if let Some(existing) = existing_root {
            if let Ok(uuid) = uuid::Uuid::parse_str(&existing.id) {
                let mgr = self.session_mgr_clone.read().await;
                commands::session::preserve_deferred_telegram_intent_if_valid(
                    &mgr,
                    &self.settings_state_clone,
                    uuid,
                    &ps.name,
                    ps.telegram_bot_id.as_deref(),
                )
                .await;
                drop(mgr);
                commands::session::seed_root_last_prompt(
                    &self.app_handle,
                    &self.session_mgr_clone,
                    uuid,
                    ps,
                )
                .await;
            }
            if ps.was_active {
                progress.active_id = Some(existing.id.clone());
            }
            progress.n_deferred += 1;
        } else {
            match self
                .restore_transaction_for_task
                .restore_dormant_inline(crate::session::selection::DormantRestoreRequest {
                    persisted: ps.clone(),
                    working_directory: root_path,
                    is_coordinator: false,
                    is_root_agent: true,
                })
                .await
            {
                Ok(info) => {
                    if ps.was_active {
                        progress.active_id = Some(info.id);
                    }
                    progress.n_deferred += 1;
                }
                Err(e) => {
                    log::error!(
                        "[root-agent] Failed to create dormant root session \
                                    '{}': {}",
                        ps.name,
                        e
                    );
                    progress.failed_recoverable.push(ps.clone());
                }
            }
        }
    }
}
