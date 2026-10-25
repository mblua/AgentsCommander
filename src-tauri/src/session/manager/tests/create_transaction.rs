use super::*;

async fn issue_3145_pending(
    manager: &SessionManager,
    cwd: &str,
    initial: Option<&str>,
    fresh: bool,
) -> (Session, PendingCreateBinding, PromptKey) {
    let (row, binding) = manager
        .create_transaction_pending_session(
            "codex".into(),
            Vec::new(),
            cwd.into(),
            Some("codex".into()),
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    manager
        .set_pending_profile_metadata(
            binding,
            Some("A".into()),
            Some("B".into()),
            Vec::new(),
            true,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let key = PromptKey {
        canonical_cwd: cwd.into(),
        agent_id: "codex".into(),
        requested_profile: Some("A".into()),
        effective_profile: "B".into(),
    };
    manager
        .initialize_pending_prompt(
            binding,
            Some(key.clone()),
            initial.map(str::to_string),
            !fresh,
            fresh,
            true,
        )
        .await
        .unwrap();
    (row, binding, key)
}

async fn issue_3145_publish(
    manager: &SessionManager,
    binding: PendingCreateBinding,
) -> SessionInfo {
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(binding, live_witness(binding.session_id));
    manager
        .commit_selection_transition(
            &CommitCapability::for_test(),
            CommitDecision::Keep,
            SelectionCause::Restore,
            mutations,
        )
        .await
        .unwrap()
        .finalized_rows
        .remove(0)
}

#[tokio::test]
async fn issue_3145_restart_commit_uses_latest_source_and_rejects_partial_failure() {
    let manager = SessionManager::new();
    manager.install_retained_prompts(Some(HashMap::new())).await;
    let (source, binding, _) = issue_3145_pending(&manager, "domain", Some("A"), false).await;
    issue_3145_publish(&manager, binding).await;
    let (target, pending, _) = issue_3145_pending(&manager, "domain", None, false).await;
    // An external input between pending preparation and commit must win.
    manager.set_last_prompt(source.id, "B".into()).await;
    let mut invalid = LifecycleMutations::default();
    invalid.finalize_live(pending, live_witness(target.id));
    invalid.transfer_prompt(source.id, pending); // missing removal
    assert!(manager
        .commit_selection_transition(
            &CommitCapability::for_test(),
            CommitDecision::Keep,
            SelectionCause::Restore,
            invalid
        )
        .await
        .is_err());
    assert_eq!(
        manager
            .get_pending_session(pending)
            .await
            .unwrap()
            .last_prompt,
        None
    );
    assert_eq!(
        manager
            .get_session(source.id)
            .await
            .unwrap()
            .last_prompt
            .as_deref(),
        Some("B")
    );
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(pending, live_witness(target.id));
    mutations.remove(source.id);
    mutations.transfer_prompt(source.id, pending);
    let result = manager
        .commit_selection_transition(
            &CommitCapability::for_test(),
            CommitDecision::Keep,
            SelectionCause::Restore,
            mutations,
        )
        .await
        .unwrap();
    assert_eq!(result.finalized_rows[0].last_prompt.as_deref(), Some("B"));
    assert!(manager.get_session(source.id).await.is_none());
    assert_eq!(
        manager.list_sessions().await[0].last_prompt.as_deref(),
        Some("B")
    );
    assert_eq!(
        crate::config::sessions_persistence::snapshot_sessions(&manager).await[0]
            .last_prompt
            .as_deref(),
        Some("B")
    );
}

#[tokio::test]
async fn issue_3145_pending_rollback_never_associates_or_accepts_public_input() {
    let manager = SessionManager::new();
    manager.install_retained_prompts(Some(HashMap::new())).await;
    let (row, binding, _) = issue_3145_pending(&manager, "domain", Some("A"), false).await;
    manager.set_last_prompt(row.id, "B".into()).await;
    assert_eq!(
        manager
            .get_pending_session(binding)
            .await
            .unwrap()
            .last_prompt
            .as_deref(),
        Some("A")
    );
    manager.rollback_pending_create(binding).await.unwrap();
    assert!(manager.retained_prompt_snapshot().await.unwrap().is_empty());
    assert!(manager.state.read().await.pending_prompts.is_empty());
}

#[tokio::test]
async fn issue_3145_failed_checkpoint_disables_lookup_but_direct_input_still_works() {
    let manager = SessionManager::new();
    let (row, binding, key) = issue_3145_pending(&manager, "domain", None, false).await;
    manager
        .install_retained_prompts(Some(HashMap::from([(
            key.clone(),
            RetainedPrompt {
                last_prompt: Some("A".into()),
                blocked: false,
            },
        )])))
        .await;
    manager.disable_prompt_reuse().await;
    assert_eq!(
        issue_3145_publish(&manager, binding).await.last_prompt,
        None
    );
    manager.set_last_prompt(row.id, "B".into()).await;
    assert_eq!(
        manager
            .get_session(row.id)
            .await
            .unwrap()
            .last_prompt
            .as_deref(),
        Some("B")
    );
    assert_eq!(
        manager.retained_prompt_snapshot().await.unwrap()[&key]
            .last_prompt
            .as_deref(),
        Some("B")
    );
}

#[tokio::test]
async fn issue_3145_ambiguity_is_sticky_and_fresh_exclusive_reopens_domain() {
    let manager = SessionManager::new();
    manager.install_retained_prompts(Some(HashMap::new())).await;
    let (first, binding, key) = issue_3145_pending(&manager, "domain", Some("A"), false).await;
    issue_3145_publish(&manager, binding).await;
    let (second, pending, _) = issue_3145_pending(&manager, "domain", None, false).await;
    let mut dormant = LifecycleMutations::default();
    dormant.finalize_dormant(pending, dormant_witness(second.id), 0);
    manager
        .commit_selection_transition(
            &CommitCapability::for_test(),
            CommitDecision::Keep,
            SelectionCause::Restore,
            dormant,
        )
        .await
        .unwrap();
    assert!(manager.retained_prompt_snapshot().await.unwrap()[&key].blocked);
    let mut remove = LifecycleMutations::default();
    remove.remove(second.id);
    manager
        .commit_selection_transition(
            &CommitCapability::for_test(),
            CommitDecision::Keep,
            SelectionCause::Restore,
            remove,
        )
        .await
        .unwrap();
    manager.set_last_prompt(first.id, "B".into()).await;
    assert!(manager.retained_prompt_snapshot().await.unwrap()[&key].blocked);
    let (fresh, pending, _) = issue_3145_pending(&manager, "domain", Some("old"), true).await;
    let mut replace = LifecycleMutations::default();
    replace.finalize_live(pending, live_witness(fresh.id));
    replace.remove(first.id);
    manager
        .commit_selection_transition(
            &CommitCapability::for_test(),
            CommitDecision::Keep,
            SelectionCause::Restart(crate::session::selection::TrustedRestartIntent::User),
            replace,
        )
        .await
        .unwrap();
    let snapshot = manager.retained_prompt_snapshot().await.unwrap();
    assert!(!snapshot[&key].blocked);
    assert!(snapshot[&key].last_prompt.is_none());
    manager.set_last_prompt(fresh.id, String::new()).await;
    assert_eq!(
        manager.retained_prompt_snapshot().await.unwrap()[&key]
            .last_prompt
            .as_deref(),
        Some("")
    );
    manager.set_start_fresh_on_restore(fresh.id, true).await;
    manager.clear_start_fresh_on_restore_if_set(fresh.id).await;
    assert_eq!(
        manager.retained_prompt_snapshot().await.unwrap()[&key].last_prompt,
        None
    );
    assert_eq!(
        manager.get_session(fresh.id).await.unwrap().last_prompt,
        None
    );
}

#[tokio::test]
async fn issue_3145_seed_and_setter_share_permit_and_empty_is_existing() {
    let manager = SessionManager::new();
    manager.install_retained_prompts(Some(HashMap::new())).await;
    let (row, binding, key) = issue_3145_pending(&manager, "domain", None, false).await;
    issue_3145_publish(&manager, binding).await;
    let permit = acquire_carrier_mutation_permit().await;
    assert!(
        manager
            .seed_if_absent_with_carrier_permit(&permit, row.id, &key, "A".into())
            .await
    );
    let mut setter = Box::pin(manager.set_last_prompt(row.id, "N".into()));
    use std::future::Future;
    let waker = futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    assert!(setter.as_mut().poll(&mut context).is_pending());
    assert_eq!(
        manager
            .get_session(row.id)
            .await
            .unwrap()
            .last_prompt
            .as_deref(),
        Some("A")
    );
    drop(permit);
    setter.await;
    let permit = acquire_carrier_mutation_permit().await;
    assert!(
        !manager
            .seed_if_absent_with_carrier_permit(&permit, row.id, &key, "old".into())
            .await
    );
    drop(permit);
    manager.set_last_prompt(row.id, String::new()).await;
    let permit = acquire_carrier_mutation_permit().await;
    assert!(
        !manager
            .seed_if_absent_with_carrier_permit(&permit, row.id, &key, "old".into())
            .await
    );
}

struct P44WriteBackend {
    entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    bytes: std::sync::Mutex<Vec<u8>>,
}

impl crate::pty::backend::PtyBackend for P44WriteBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(
        &self,
        _: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        panic!("unexpected spawn")
    }
    fn write(
        &self,
        _: &crate::pty::manager::BackendWriteAuthority,
        _: Uuid,
        bytes: &[u8],
    ) -> Result<(), AppError> {
        self.entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(())
    }
    fn resize(&self, _: Uuid, _: u16, _: u16) -> Result<(), AppError> {
        panic!("unexpected resize")
    }
    fn kill(&self, _: Uuid) -> Result<(), AppError> {
        panic!("unexpected kill")
    }
    fn has_session(&self, _: Uuid) -> bool {
        true
    }
    fn get_screen_snapshot(&self, _: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }
    fn get_pty_size(&self, _: Uuid) -> Option<(u16, u16)> {
        None
    }
    fn get_screen_rows(&self, _: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::Unavailable
    }
    fn register_response_watcher(&self, _: Uuid, _: String, _: PathBuf) {
        panic!("unexpected watcher")
    }
    fn terminate_job_for_session(&self, _: Uuid) -> bool {
        panic!("unexpected terminate")
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        panic!("unexpected kill all")
    }
}

#[tokio::test]
async fn e2992_p44_retained_settings_permit() {
    use crate::config::settings::{
        AppSettings, OperationOutcome, SettingsAvailability, SettingsOwner, SettingsOwnerError,
        SettingsPublisher,
    };
    use crate::pty::idle_detector::{IdleDetector, PtyInputBoundaryFailure};
    use crate::pty::manager::PtyManager;
    use std::sync::Mutex;
    use std::time::Duration;

    let evidence = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/2992-evidence/P44");
    std::fs::create_dir_all(&evidence).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("permit-")
        .tempdir_in(evidence)
        .unwrap();
    let collection = temp.path().join("projects");
    let ac = collection.join("p44-project/.ac");
    std::fs::create_dir_all(ac.join("_team_p44-team")).unwrap();
    std::fs::write(
        ac.join("_team_p44-team/config.json"),
        concat!(
            r#"{"agents":["../_agent_worker","../_agent_coordinator"],"#,
            r#""coordinator":"../_agent_coordinator"}"#
        ),
    )
    .unwrap();
    let manager = SessionManager::new();
    let mut sessions = Vec::new();
    let mut paths = Vec::new();
    for (index, name) in ["coordinator", "worker"].iter().enumerate() {
        std::fs::create_dir_all(ac.join(format!("_agent_{name}"))).unwrap();
        let root = ac.join(format!("room-01-p44-team/__agent_{name}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.json"),
            format!(r#"{{"identity":"../../_agent_{name}"}}"#),
        )
        .unwrap();
        sessions.push(
            manager
                .create_session(
                    "shell".into(),
                    Vec::new(),
                    root.to_string_lossy().into_owned(),
                    None,
                    None,
                    Vec::new(),
                    index == 0,
                    SessionBackendKind::LocalProcess,
                )
                .await
                .unwrap(),
        );
        paths.push(root);
    }
    let cfg = AppSettings {
        project_paths: vec![collection.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    let settings_path = temp.path().join("settings.30.instance.no-git.json");
    std::fs::write(&settings_path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    let owner = tokio::task::spawn_blocking(move || {
        SettingsOwner::open_active(&settings_path, Duration::from_secs(5)).unwrap()
    })
    .await
    .unwrap();
    let route = crate::config::teams::verify_pty_input_route(
        &paths[0],
        false,
        "p44-project:room-01-p44-team/worker",
        &cfg.project_paths,
    )
    .unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let backend = Arc::new(P44WriteBackend {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
        bytes: Mutex::new(Vec::new()),
    });
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    for (session, path) in sessions.iter().zip(&paths) {
        let identity = crate::path_identity::verify_directory(path).unwrap();
        pty.lock()
            .unwrap()
            .record_route_with_identities(
                session.id,
                SessionBackendKind::LocalProcess,
                Some(identity.clone()),
                Some(identity),
            )
            .unwrap();
    }
    let sender = sessions[0].id;
    let target = sessions[1].id;
    manager.mark_idle(target).await;
    let idle = IdleDetector::new(|_| {}, |_| {});
    idle.register_session(target, crate::session::profile::IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(target);
    macro_rules! boundary {
        ($authority:expr, $permit:expr) => {
            manager.prepare_pty_input_boundary(
                target,
                &route.target,
                SessionBackendKind::LocalProcess,
                sender,
                &route.sender,
                SessionBackendKind::LocalProcess,
                $authority,
                $permit,
                &idle,
                &owner,
                |_, _| true,
                || true,
            )
        };
    }
    let authority = PtyManager::authority_route_proof(&pty, sender).unwrap();
    let permit = PtyManager::acquire_input_writer(&pty, target)
        .await
        .unwrap();
    // Contention uses a legitimate owner operation, without a raw writer.
    let (builder_tx, builder_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel();
    let operation = {
        let owner = owner.clone();
        tokio::spawn(async move {
            owner
                .with_operation("p44-contention", move |_, _| {
                    builder_tx.send(()).unwrap();
                    finish_rx
                        .recv_timeout(Duration::from_secs(10))
                        .map_err(|e| e.to_string())?;
                    Ok(())
                })
                .await
                .unwrap()
        })
    };
    tokio::time::timeout(Duration::from_secs(5), builder_rx)
        .await
        .unwrap()
        .unwrap();
    let denied = boundary!(&authority, &permit).await;
    assert!(matches!(
        denied,
        Err(PtyInputBoundaryFailure::RouteUnavailable)
    ));
    drop(denied);
    assert!(backend.bytes.lock().unwrap().is_empty());
    finish_tx.send(()).unwrap();
    assert_eq!(
        operation.await.unwrap().outcome,
        OperationOutcome::Committed
    );
    drop(permit);
    drop(authority);

    // The bounded backend rendezvous is after validation, inside the real write.
    let worker = {
        let manager = manager.clone();
        let pty = pty.clone();
        let idle = idle.clone();
        let owner = owner.clone();
        let handle = tokio::runtime::Handle::current();
        let route = route.clone();
        tokio::task::spawn_blocking(move || {
            let authority = PtyManager::authority_route_proof(&pty, sender).unwrap();
            let permit = handle
                .block_on(PtyManager::acquire_input_writer(&pty, target))
                .unwrap();
            let guard = handle
                .block_on(manager.prepare_pty_input_boundary(
                    target,
                    &route.target,
                    SessionBackendKind::LocalProcess,
                    sender,
                    &route.sender,
                    SessionBackendKind::LocalProcess,
                    &authority,
                    &permit,
                    &idle,
                    &owner,
                    |_, _| true,
                    || true,
                ))
                .unwrap();
            guard.write(b"p44 exact bytes").unwrap();
            drop(guard);
        })
    };
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        pty.lock()
            .unwrap()
            .try_remove_route_if_kind(sender, SessionBackendKind::LocalProcess),
        Err(crate::pty::input_route::PtyRouteRemovalError::Busy)
    ));
    let (builder_tx, builder_rx) = std::sync::mpsc::channel();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let operation = {
        let owner = owner.clone();
        let backend = backend.clone();
        tokio::spawn(async move {
            started_tx.send(()).unwrap();
            owner
                .with_operation("p44-retained", move |_, _| {
                    assert_eq!(*backend.bytes.lock().unwrap(), b"p44 exact bytes");
                    builder_tx.send(()).unwrap();
                    Ok(())
                })
                .await
                .unwrap()
        })
    };
    started_rx.await.unwrap();
    let no_entry = tokio::task::spawn_blocking(move || {
        assert!(matches!(
            builder_rx.recv_timeout(Duration::from_millis(250)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        builder_rx
    })
    .await
    .unwrap();
    assert!(!operation.is_finished());
    assert!(backend.bytes.lock().unwrap().is_empty());
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), operation)
            .await
            .unwrap()
            .unwrap()
            .outcome,
        OperationOutcome::Committed
    );
    no_entry.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(*backend.bytes.lock().unwrap(), b"p44 exact bytes");

    pty.lock()
        .unwrap()
        .try_remove_route_if_kind(sender, SessionBackendKind::LocalProcess)
        .unwrap();
    assert!(PtyManager::authority_route_proof(&pty, sender).is_err());
    let sender_identity = crate::path_identity::verify_directory(&paths[0]).unwrap();
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            sender,
            SessionBackendKind::LocalProcess,
            Some(sender_identity.clone()),
            Some(sender_identity),
        )
        .unwrap();
    let authority = PtyManager::authority_route_proof(&pty, sender).unwrap();
    let stale_permit = PtyManager::acquire_input_writer(&pty, target)
        .await
        .unwrap();
    pty.lock()
        .unwrap()
        .try_remove_route_if_kind(target, SessionBackendKind::LocalProcess)
        .unwrap();
    let target_identity = crate::path_identity::verify_directory(&paths[1]).unwrap();
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            target,
            SessionBackendKind::LocalProcess,
            Some(target_identity.clone()),
            Some(target_identity),
        )
        .unwrap();
    manager.mark_idle(target).await;
    idle.set_pty_input_ready_for_test(target);
    let armed_before = manager.get_session(target).await.unwrap().agent_turn_armed;
    let denied = boundary!(&authority, &stale_permit).await;
    assert!(matches!(
        denied,
        Err(PtyInputBoundaryFailure::RouteUnavailable)
    ));
    drop(denied);
    let after_denial = manager.get_session(target).await.unwrap();
    assert!(after_denial.waiting_for_input);
    assert_eq!(after_denial.agent_turn_armed, armed_before);
    assert_eq!(*backend.bytes.lock().unwrap(), b"p44 exact bytes");
    drop(stale_permit);
    let permit = PtyManager::acquire_input_writer(&pty, target)
        .await
        .unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *backend.entered.lock().unwrap() = Some(entered_tx);
    *backend.release.lock().unwrap() = release_rx;
    let worker = {
        let manager = manager.clone();
        let idle = idle.clone();
        let owner = owner.clone();
        let route = route.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let guard = handle
                .block_on(manager.prepare_pty_input_boundary(
                    target,
                    &route.target,
                    SessionBackendKind::LocalProcess,
                    sender,
                    &route.sender,
                    SessionBackendKind::LocalProcess,
                    &authority,
                    &permit,
                    &idle,
                    &owner,
                    |_, _| true,
                    || true,
                ))
                .unwrap();
            guard.write(b"p44 replacement bytes").unwrap();
        })
    };
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*backend.bytes.lock().unwrap(), b"p44 exact bytes");
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *backend.bytes.lock().unwrap(),
        b"p44 exact bytesp44 replacement bytes"
    );

    let report = owner
        .with_operation("p44-permit-invalid", |_, guard| {
            guard.publish(SettingsPublisher::SettingsSave, |path| {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                value["defaultShell"] = serde_json::json!(42);
                std::fs::write(path, serde_json::to_vec(&value).unwrap()).map_err(|e| e.to_string())
            })
        })
        .await
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
    assert_eq!(report.availability, SettingsAvailability::Invalid);
    assert!(matches!(
        owner.current_read().await,
        Err(SettingsOwnerError::Unavailable(
            SettingsAvailability::Invalid
        ))
    ));
    manager.mark_idle(target).await;
    idle.set_pty_input_ready_for_test(target);
    let authority = PtyManager::authority_route_proof(&pty, sender).unwrap();
    let permit = PtyManager::acquire_input_writer(&pty, target)
        .await
        .unwrap();
    let denied = boundary!(&authority, &permit).await;
    assert!(matches!(
        denied,
        Err(PtyInputBoundaryFailure::RouteUnavailable)
    ));
    drop(denied);
    assert_eq!(
        *backend.bytes.lock().unwrap(),
        b"p44 exact bytesp44 replacement bytes"
    );
    drop(permit);
    drop(authority);
    tokio::task::spawn_blocking(move || {
        owner.drain_blocking(Duration::from_secs(5));
        assert_eq!(owner.availability(), SettingsAvailability::Closed);
    })
    .await
    .unwrap();
}
