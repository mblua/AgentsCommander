use super::*;

pub(super) async fn assert_retained_pty_preboundary_rejections(
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    route: &crate::config::teams::VerifiedPtyInputRoute,
    sender: Uuid,
    previous: Uuid,
) {
    use crate::phone::types::{PtyInputReasonCode as C, PtyInputSourcePlane as Plane};
    use crate::session::selection::{
        OwnedReplacementWork, ResolvedReplacement, SourceExecutorGuard,
    };
    let app = fixture.app.handle();
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let state = app.state::<crate::api::message_store::MessageStoreState>();
    let store = state.store.as_ref().unwrap().clone();
    let gate = state.target_gate.as_ref().unwrap().clone();
    let original_settings = app.state::<SettingsState>().read().await.clone();
    let original_name = manager.get_session(previous).await.unwrap().name;
    let directory = crate::config::config_dir().unwrap();
    let publication = std::fs::read(directory.join("sessions.json")).unwrap();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let backend = pty
        .lock()
        .unwrap()
        .backend_for_kind(SessionBackendKind::LocalProcess);
    let backend = backend
        .as_any()
        .downcast_ref::<MailboxMockPtyBackend>()
        .unwrap();
    let kills = backend.kill_attempts.load(Ordering::SeqCst);
    let hooks = MailboxTestHooks::default();
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    // These are primitive checks in the existing positive PTY cell. Every
    // input has a genuine store claim and the SAME truly retired A.
    for case in ["backend", "absent", "temporary", "different", "prepared"] {
        tokio::time::timeout(Duration::from_secs(20), async {
            let injection = enqueue_shared_pty_engine_operation(
                &store,
                route,
                sender,
                b"preboundary-rejection",
            );
            let initial = store
                .query_pty_input_by_injection(&injection)
                .unwrap()
                .unwrap();
            assert!(!initial.terminal);
            let claimed = store
                .claim_pty_input(
                    Plane::HostCli,
                    Some(&injection),
                    &Uuid::new_v4().to_string(),
                    chrono::Utc::now(),
                )
                .unwrap()
                .unwrap();
            let mut heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
                store.clone(),
                injection.clone(),
                claimed.lease_owner.clone(),
                crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at).unwrap(),
            );
            let stripe = gate
                .try_target_lock(&route.target.canonical_fqn)
                .unwrap()
                .unwrap();
            let exact = gate.acquire_exact(&route.target.canonical_fqn).await;
            let ownership = gate
                .target_ownership(&route.target.canonical_fqn, &stripe, &exact)
                .unwrap();
            let mut prepared = None;
            if case == "prepared" {
                let cwd = crate::path_utils::normalize_windows_verbatim_path(
                    &route.target.replica_root.to_string_lossy(),
                );
                let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
                    &original_settings,
                    "codex",
                    &cwd,
                    None,
                )
                .unwrap()
                .unwrap();
                let request = crate::commands::session::OwnedReplacementCreateRequest {
                    shell: spawn.shell.clone(),
                    shell_args: spawn.shell_args.clone(),
                    cwd: cwd.clone(),
                    name: Some(
                        crate::config::teams::split_project_prefix(&claimed.target_fqn)
                            .1
                            .to_string(),
                    ),
                    agent_id: Some("codex".into()),
                    agent_label: Some(spawn.trusted_agent_label.clone()),
                    skip_tooling_save: false,
                    git_repos: Vec::new(),
                    resolved_spawn: Some(spawn),
                    host_shell: Some(crate::pty::backend::ResolvedAgentHostShell {
                        program: original_settings.default_shell.clone(),
                        args: original_settings.default_shell_args.clone(),
                    }),
                };
                let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
                let (carrier, row) = manager
                    .with_current_carrier_snapshot(&permit, |current| {
                        (
                            crate::config::sessions_persistence::retained_carrier_for_replacement(
                                &permit,
                                &directory,
                                &route.target.canonical_fqn,
                                previous,
                                current,
                            )
                            .unwrap(),
                            current
                                .rows
                                .iter()
                                .find(|row| row.id == previous)
                                .unwrap()
                                .clone(),
                        )
                    })
                    .await;
                drop(permit);
                prepared = Some(
                    coordinator.prepare_owned_replacement(
                        ResolvedReplacement {
                            target: route.target.canonical_fqn.clone(),
                            directory: directory.clone(),
                            previous: row,
                            carrier,
                            requested_agent_id: request.agent_id.clone(),
                            requested_profile: None,
                            shell: request.shell.clone(),
                            shell_args: request.shell_args.clone(),
                            launch_recipe: owned_replacement_launch_recipe(&request),
                            cwd: cwd.clone(),
                            backend: SessionBackendKind::LocalProcess,
                            work: OwnedReplacementWork::PtyDurableReference {
                                version: initial.version,
                                injection_id: claimed.injection_id.clone(),
                                op_id: claimed.op_id.clone(),
                                sender: claimed.sender_fqn.clone(),
                                target: claimed.target_fqn.clone(),
                                source_plane: "host_cli".into(),
                                payload_sha256: initial.payload_sha256.clone().unwrap(),
                                payload_bytes: initial.payload_bytes.unwrap(),
                                issued_at: initial.issued_at.clone().unwrap(),
                                expires_at: claimed.expires_at.clone(),
                                queued_at: initial.queued_at.clone().unwrap(),
                                lease_owner: claimed.lease_owner.clone(),
                                attempt: claimed.attempt,
                                authority_session_id: claimed.authority_session_id.clone(),
                                authority_client_id: claimed.authority_client_id.clone(),
                                authority_client_generation: claimed
                                    .authority_client_generation
                                    .clone(),
                                sender_identity_fingerprint: claimed
                                    .sender_identity_fingerprint
                                    .clone(),
                                target_identity_fingerprint: claimed
                                    .target_identity_fingerprint
                                    .clone(),
                                requested_agent_id: claimed.requested_agent_id.clone(),
                            },
                            cwd_identity: crate::path_identity::verify_directory(Path::new(&cwd))
                                .unwrap()
                                .into(),
                            replica_identity: Some(route.target.replica_identity.clone().into()),
                        },
                        manager.reserve_carrier_create_binding(),
                    ),
                );
                assert!(
                    prepared.as_ref().unwrap().matches_launch(
                        &request.shell,
                        &request.shell_args,
                        &request.cwd,
                        &request.agent_id,
                        &owned_replacement_launch_recipe(&request)
                    ),
                    "original actual prepared recipe matches"
                );
                app.state::<SettingsState>()
                    .write()
                    .await
                    .agents
                    .iter_mut()
                    .find(|agent| agent.id == "codex")
                    .unwrap()
                    .command
                    .push_str(" --changed-before-admission");
            }
            if case == "backend" {
                let settings_state = app.state::<SettingsState>();
                let mut settings = settings_state.write().await;
                let agent = settings
                    .agents
                    .iter_mut()
                    .find(|agent| agent.id == "codex")
                    .unwrap();
                agent.backend.kind = SessionBackendKind::ContainerTransport;
                agent.backend.image = Some("agentscommander/ac-codex:latest".into());
            }
            if case == "temporary" {
                manager
                    .rename_session(
                        previous,
                        format!(
                            "{}preboundary",
                            crate::session::session::TEMP_SESSION_PREFIX
                        ),
                    )
                    .await
                    .unwrap();
            }
            let other = if case == "different" {
                Some(
                    add_mailbox_session(
                        app,
                        &fixture.target_cwd,
                        "other real exited row",
                        SessionStatus::Exited(0),
                        None,
                    )
                    .await,
                )
            } else {
                None
            };
            let before = serde_json::to_value(SessionInfo::from(
                &manager.get_session(previous).await.unwrap(),
            ))
            .unwrap();
            let mut before_ids: Vec<_> = manager
                .list_sessions()
                .await
                .into_iter()
                .map(|row| row.id)
                .collect();
            before_ids.sort();
            let before_live = backend.live.lock().unwrap().clone();
            let mut matching: Vec<_> = manager
                .list_sessions()
                .await
                .into_iter()
                .filter(|row| {
                    crate::config::teams::agent_fqn_from_path(&row.working_directory)
                        == route.target.canonical_fqn
                })
                .collect();
            if case == "absent" || case == "backend" {
                matching.clear();
            }
            if let Some(other) = other {
                matching.retain(|row| row.id == other.to_string());
            }
            let (_, ready_a) = observe_pty_spawn_scope(app, &route.target).await;
            assert_eq!(
                ready_a,
                Some(previous),
                "test needs actual strict retained A: {case}"
            );
            assert!(pty_spawn_scope_may_resume(app, &route.target).await);
            let mut source = SourceExecutorGuard::unbound();
            let actual = poller
                .spawn_pty_input_target(
                    app,
                    &claimed,
                    &matching,
                    &route.sender,
                    &route.target,
                    Plane::HostCli,
                    None,
                    &ownership,
                    &initial,
                    prepared,
                    &mut source,
                    None,
                )
                .await;
            let expected = if case == "backend" || case == "prepared" {
                C::AuthorityChanged
            } else {
                C::InconsistentSession
            };
            assert!(
                matches!(actual, Err(code) if code == expected),
                "actual rejection: {case}"
            );
            source.returned(false);
            assert!(
                finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    &injection,
                    &claimed.lease_owner,
                    expected
                )
                .await
            );
            drop(exact);
            drop(stripe);
            let result = store
                .query_pty_input_by_injection(&injection)
                .unwrap()
                .unwrap();
            assert!(result.terminal);
            assert_eq!(
                result.status,
                crate::phone::types::PtyInputPublicStatus::Rejected
            );
            assert_eq!(result.reason.map(|reason| reason.code), Some(expected));
            let connection =
                rusqlite::Connection::open(fixture._temp.path().join("retained-respawn.sqlite3"))
                    .unwrap();
            let counts: (i64, i64, i64) = connection.query_row(
                "SELECT attempt,(SELECT count(*) FROM pty_input_audit WHERE injection_id=?1 AND \
                status='retry'),(SELECT count(*) FROM pty_input_audit WHERE injection_id=?1 AND \
                status='rejected') FROM pty_input_operations WHERE injection_id=?1",
                [&injection], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
            assert_eq!(counts, (claimed.attempt, 0, 1));
            assert_eq!(coordinator.fixture_root_count(), Some(0));
            let mut after_ids: Vec<_> = manager
                .list_sessions()
                .await
                .into_iter()
                .map(|row| row.id)
                .collect();
            after_ids.sort();
            assert_eq!(after_ids, before_ids, "no B inserted or A removed: {case}");
            assert_eq!(
                *backend.live.lock().unwrap(),
                before_live,
                "no native runtime created: {case}"
            );
            assert_eq!(
                serde_json::to_value(SessionInfo::from(
                    &manager.get_session(previous).await.unwrap()
                ))
                .unwrap(),
                before
            );
            assert_eq!(
                std::fs::read(directory.join("sessions.json")).unwrap(),
                publication
            );
            assert_eq!(backend.kill_attempts.load(Ordering::SeqCst), kills);
            assert!(backend.writes.lock().unwrap().is_empty());
            assert!(hooks.spawn_calls.lock().unwrap().is_empty());
            assert!(hooks.destroy_calls.lock().unwrap().is_empty());
            assert!(gate
                .try_target_lock(&route.target.canonical_fqn)
                .unwrap()
                .is_some());
            if let Some(other) = other {
                assert_eq!(manager.remove_exited_sessions(&[other]).await, 1);
            }
            manager
                .rename_session(previous, original_name.clone())
                .await
                .unwrap();
            *app.state::<SettingsState>().write().await = original_settings.clone();
        })
        .await
        .expect("entire preboundary operation must finish before its 20s deadline");
    }
    assert_eq!(
        observe_pty_spawn_scope(app, &route.target).await.1,
        Some(previous)
    );
}

#[cfg(test)]
pub(super) async fn retained_root_reclaim_operation(
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    manager: &SessionManager,
    request: &crate::commands::session::RootJobRequest,
    id: uuid::Uuid,
    operation: usize,
) -> uuid::Uuid {
    let app = fixture.app.handle();
    let mut id = id;
    let actual = tokio::time::timeout(
        Duration::from_secs(20),
        coordinator.root_lifecycle(app.clone(), request.clone()),
    )
    .await
    .expect("actual same-history Root entry deadline")
    .expect("actual legacy live-root reuse result");
    assert_eq!(
        actual.id,
        id.to_string(),
        "reuse returns the actual root incarnation"
    );
    assert!(manager.get_session(id).await.unwrap().is_root_agent);
    assert!(app
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap()
        .has_session(id));
    assert_eq!(
        coordinator.fixture_root_count(),
        Some(0),
        "actual EntryTerminal/effects/lease release reclaims capacity before next entry"
    );
    if operation == 0 || operation == 33 {
        let previous = id;
        app.state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap()
            .kill_for_kind(previous, SessionBackendKind::LocalProcess)
            .unwrap();
        if operation == 33 {
            // Real legacy exit mutation selects WakeDormant; the first case selects
            // DiscardMissingPty.
            manager.mark_exited(previous, 0).await;
            assert_eq!(
                manager.get_session(previous).await.unwrap().status,
                SessionStatus::Exited(0)
            );
        }
        let restarted = tokio::time::timeout(
            Duration::from_secs(20),
            coordinator.root_lifecycle(app.clone(), request.clone()),
        )
        .await
        .expect("same-entry Root direct-restart deadline")
        .expect("actual legacy root direct restart");
        id = Uuid::parse_str(&restarted.id).unwrap();
        assert_ne!(id, previous);
        assert!(manager.get_session(previous).await.is_none());
        assert!(manager.get_session(id).await.unwrap().is_root_agent);
        assert!(app
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap()
            .has_session(id));
        assert_eq!(
            coordinator.fixture_root_count(),
            Some(0),
            "SAME Root entry covers actual direct restart and releases once"
        );
    }
    id
}

#[cfg(test)]
pub(super) async fn observe_retained_native_source_unwind(
    seed: bool,
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    control: &crate::session::selection::ReplacementFixtureControl,
    entered: crate::session::selection::ReplacementFixtureCut,
    panic_source: tokio::sync::oneshot::Sender<()>,
) -> crate::session::manager::PendingCreateBinding {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let receipt = retained_source_wait_cut(control, entered).await;
    assert_eq!(
        coordinator.fixture_native_work_counts(),
        Some((1, 1)),
        "actual tracker Started with installed completion guard"
    );
    assert!(control.observe_owner(coordinator).unwrap().source_active);
    if seed {
        assert!(control
            .native_results
            .lock()
            .unwrap()
            .iter()
            .any(|(cut, result)| *cut == Cut::ContextNativeReturned && result.starts_with("Ok(")));
        assert!(
            std::fs::read_to_string(fixture.target_cwd.join("AGENTS.md"))
                .unwrap()
                .contains("Actual native context content."),
            "actual resolved context completed before selected seed native begins"
        );
    }
    panic_source.send(()).unwrap();
    loop {
        if control.observe_owner(coordinator).unwrap().source_unwound {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        control
            .observe_owner(coordinator)
            .unwrap()
            .source_returned_failed,
        None,
        "actual source unwind stays Stopped, never Returned"
    );
    assert_eq!(coordinator.fixture_native_work_counts(), Some((1, 1)));
    receipt.binding.unwrap()
}

#[cfg(test)]
pub(super) fn assert_retained_native_result(fixture: &MailboxFixture, seed: bool, actual: &str) {
    if seed {
        assert!(
            actual.starts_with("Published("),
            "actual selected seed result: {actual}"
        );
        assert_eq!(
            std::fs::read(fixture.target_cwd.join(".p05-seed/fixture.txt")).unwrap(),
            b"actual seed fixture content"
        );
    } else {
        assert!(
            actual.starts_with("Ok("),
            "actual resolved context result: {actual}"
        );
        assert!(
            std::fs::read_to_string(fixture.target_cwd.join("AGENTS.md"))
                .unwrap()
                .contains("Actual native context content.")
        );
    }
}

#[cfg(test)]
pub(super) fn run_retained_source_child_scenario<'a>(
    pty: bool,
    inline: bool,
    scenario: &'a str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
    match scenario {
        "native-waiter-context" | "native-waiter-seed" => Box::pin(
            retained_exited_respawn_native_waiter_cell(pty, scenario == "native-waiter-seed"),
        ),
        "lifetime-source-panic" => Box::pin(retained_exited_respawn_lifetime_cell(pty, inline)),
        "root-full" => Box::pin(retained_exited_respawn_root_full_cell(pty, inline)),
        "root-poison" | "root-close-first" => Box::pin(retained_exited_respawn_root_refusal_cell(
            pty,
            inline,
            scenario == "root-poison",
        )),
        "queued-drain"
        | "cleanup-claim-first"
        | "cleanup-finalize-first"
        | "cleanup-runtime-changed"
        | "cleanup-unknown-veto" => {
            Box::pin(retained_exited_respawn_queued_cell(pty, inline, scenario))
        }
        "root-reservation-refusal" => Box::pin(retained_exited_respawn_reservation_refusal_cell()),
        "entry-root-reclaim" => Box::pin(retained_exited_respawn_entry_root_reclaim_cell()),
        "entry-restart-reclaim" => Box::pin(retained_exited_respawn_entry_restart_reclaim_cell()),
        "call-before-accept" | "call-after-accept" => Box::pin(
            retained_exited_respawn_caller_cell(pty, inline, scenario == "call-before-accept"),
        ),
        "submit-refused" | "submit-channel-close" => Box::pin(
            retained_exited_respawn_submit_refused_cell(pty, scenario == "submit-channel-close"),
        ),
        "recv-after-release" => Box::pin(retained_exited_respawn_receive_after_release_cell(pty)),
        "pty-held-stop" | "pty-held-rollback" | "pty-cancel-held" => {
            Box::pin(retained_exited_respawn_pty_held_cell(scenario))
        }
        "positive"
        | "reclaim-66"
        | "walk-terminal"
        | "seal-insert-first"
        | "shutdown-executing"
        | "root-duplicate"
        | "newer-telegram"
        | "newer-full"
        | "under-commit-telegram"
        | "under-commit-full"
        | "seed-positive"
        | "seed-staging-file" => {
            Box::pin(retained_exited_respawn_source_cell(pty, inline, scenario))
        }
        _ => Box::pin(retained_exited_respawn_failure_cell(pty, inline, scenario)),
    }
}

#[cfg(test)]
pub(super) fn retained_failure_action(
    scenario: &str,
    inline: bool,
    control: &crate::session::selection::ReplacementFixtureControl,
    backend: &MailboxMockPtyBackend,
    physical_cut: Option<crate::session::selection::ReplacementFixtureCut>,
    physical_gate: &mut Option<tokio::sync::oneshot::Receiver<()>>,
    context_gate: &mut Option<tokio::sync::oneshot::Receiver<()>>,
) -> Option<crate::session::selection::ReplacementFixtureAction> {
    use crate::session::selection::{
        ReplacementFixtureAction as Action, ReplacementFixtureCut as Cut,
    };
    match scenario {
        "pre-finalize" | "post-carry-runtime" | "post-rename-physical" | "post-retire-runtime" => {
            Some(Action::Pause {
                cut: physical_cut.unwrap(),
                resume: physical_gate.take().expect("selected physical gate"),
            })
        }
        "pre-context" => Some(Action::Pause {
            cut: Cut::ContextNativeEnter,
            resume: context_gate.take().expect("selected context gate"),
        }),
        "native-context-panic" => Some(Action::PanicOnce {
            cut: Cut::ContextNativeEnter,
        }),
        "native-seed-panic" => Some(Action::PanicOnce {
            cut: Cut::SeedNativeEnter,
        }),
        "pre-backend" => {
            backend.fail_spawn.store(true, Ordering::SeqCst);
            None
        }
        "pre-binder" => Some(Action::BindPreparedTwice),
        "pre-rollback-error" => {
            backend.fail_kill_once.store(true, Ordering::SeqCst);
            Some(Action::SimulatedPrecommitFailure)
        }
        "base-missing" => Some(Action::FixtureStopExactB),
        "panic-spawn" => Some(Action::PanicOnce {
            cut: Cut::BeforeBackendSpawn,
        }),
        "panic-before-commit" => Some(Action::PanicOnce {
            cut: Cut::BeforeCommit,
        }),
        "panic-commit-before-disarm" => Some(Action::CommitBeforeDisarmPanicOnce),
        "panic-baseline" => Some(Action::PanicOnce {
            cut: Cut::AfterDisarm,
        }),
        "panic-carry" => Some(Action::PanicOnce {
            cut: Cut::BeforeCarry,
        }),
        "panic-rename" => Some(Action::PanicOnce {
            cut: Cut::BeforePublication,
        }),
        "panic-readback" => Some(Action::PublicationLeaf(
            crate::config::sessions_persistence::FixturePublicationActionKind::PanicOnce,
        )),
        "post-readback-fault" => {
            use crate::config::sessions_persistence::FixturePublicationActionKind as Kind;
            Some(Action::PublicationLeaf(Kind::CorruptFixtureLeafOnce))
        }
        "panic-retire" => Some(Action::PanicOnce {
            cut: Cut::BeforeRetirement,
        }),
        "base-continue-loss" => {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            *control.baseline_gate.lock().unwrap() = Some(receiver);
            drop(sender);
            None
        }
        "release-lost" if inline => {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            *control.before_release_gate.lock().unwrap() = Some(receiver);
            drop(sender);
            None
        }
        "recv-live-source" if inline => {
            control.drop_receiver.store(true, Ordering::Release);
            None
        }
        _ => panic!("unknown finite failure scenario {scenario}"),
    }
}

#[cfg(test)]
pub(super) async fn retained_failure_physical_controller(
    scenario: &str,
    physical_cut: Option<crate::session::selection::ReplacementFixtureCut>,
    control: &crate::session::selection::ReplacementFixtureControl,
    coordinator: &crate::session::selection::SelectionCoordinator,
    pty_manager: &Arc<Mutex<PtyManager>>,
    resume_physical: tokio::sync::oneshot::Sender<()>,
) {
    if let Some(cut) = physical_cut {
        let reached = retained_source_wait_cut(control, cut).await;
        let id = reached.binding.unwrap().session_id();
        if scenario == "post-rename-physical" {
            let directory = crate::config::config_dir().unwrap();
            let leaf = directory.join("sessions.json");
            let backup = directory.join(format!("p05-rename-backup-{}.json", reached.operation));
            assert!(leaf.is_file());
            assert!(!backup.exists());
            std::fs::rename(&leaf, &backup).unwrap();
            std::fs::create_dir(&leaf).unwrap();
            std::fs::write(leaf.join("nonempty-fixture"), b"real rename blocker").unwrap();
        } else {
            let owner = control.observe_owner(coordinator).unwrap();
            if scenario == "post-retire-runtime" {
                assert!(
                    owner.checkpoint_confirmed,
                    "actual first Confirmed checkpoint precedes retirement cut"
                );
            }
            if scenario == "post-carry-runtime" {
                assert!(owner.initial_version.is_some());
            }
            let actual = pty_manager.lock().unwrap();
            assert!(actual.has_session(id));
            actual
                .kill_for_kind(id, SessionBackendKind::LocalProcess)
                .unwrap();
            assert!(!actual.has_session(id));
        }
        resume_physical.send(()).unwrap();
    } else {
        drop(resume_physical);
    }
}

#[cfg(test)]
pub(super) async fn retained_failure_context_controller(
    scenario: &str,
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    control: &crate::session::selection::ReplacementFixtureControl,
    manager: &SessionManager,
    pty_manager: &Arc<Mutex<PtyManager>>,
    resume_context: tokio::sync::oneshot::Sender<()>,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    if scenario == "pre-context" {
        let entered = retained_source_wait_cut(control, Cut::ContextNativeEnter).await;
        assert!(control.observe_owner(coordinator).unwrap().source_active);
        assert!(
            !control
                .cleanup_observed_stopped(coordinator, manager, pty_manager)
                .await,
            "actual native context closure is Running; no stopped-source cleanup"
        );
        assert!(manager
            .get_pending_session(entered.binding.unwrap())
            .await
            .is_some());
        let target = fixture.target_cwd.join("AGENTS.md");
        if target.exists() {
            assert!(std::fs::symlink_metadata(&target).unwrap().is_file());
            std::fs::remove_file(&target).unwrap();
        }
        std::fs::create_dir(&target).unwrap();
        resume_context.send(()).unwrap();
        retained_source_wait_cut(control, Cut::ContextNativeReturned).await;
        let actual = control
            .native_results
            .lock()
            .unwrap()
            .iter()
            .find(|(cut, _)| *cut == Cut::ContextNativeReturned)
            .map(|(_, result)| result.clone())
            .expect("real native context result");
        assert!(
            actual.starts_with("Err("),
            "real content/materializer Err: {actual}"
        );
        assert!(
            actual.contains("is a real directory, not a file"),
            "resolved content Some reached actual refusal: {actual}"
        );
        assert!(target.is_dir(), "real managed directory was not deleted");
    } else {
        drop(resume_context);
    }
}
