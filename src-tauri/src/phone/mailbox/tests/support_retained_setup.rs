use super::*;

pub(super) async fn retire_retained_fixture_member(
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    route: &crate::config::teams::VerifiedPtyInputRoute,
    sender: Uuid,
    previous: Uuid,
) {
    use crate::pty::manager::{ResponseClosePublisher as P, ResponseRouteIdentity};
    let app = fixture.app.handle();
    let tracker = app
        .state::<crate::pty::manager::ResponseCloseState>()
        .inner()
        .clone();
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    manager.set_is_coordinator(sender, true).await;
    manager
        .set_agent_kind(
            previous,
            Some(crate::session::profile::CodingAgentKind::Codex),
        )
        .await;
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let (member, leader) = manager
        .with_current_carrier_snapshot(&permit, |current| {
            let identity = |id, target: String| {
                let row = current.rows.iter().find(|row| row.id == id).unwrap();
                ResponseRouteIdentity {
                    target,
                    session_id: id,
                    token: row.token,
                    generation: tracker.generation(id).unwrap(),
                    carrier_version: current.versions[&id],
                    backend: row.backend_kind,
                }
            };
            (
                identity(previous, route.target.canonical_fqn.clone()),
                identity(sender, route.sender.canonical_fqn.clone()),
            )
        })
        .await;
    drop(permit);
    let directory = crate::config::config_dir().expect("designated isolated test config directory");
    let configured = std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR")
        .expect("retained-source fixtures require the designated isolated test configuration");
    assert_eq!(
        directory,
        PathBuf::from(configured),
        "never write a user configuration"
    );
    let settings = app.state::<SettingsState>().read().await.clone();
    crate::config::settings::save_settings_with_project_paths(&settings).unwrap();
    let written = crate::config::settings::save_settings(&settings).unwrap();
    *app.state::<SettingsState>().write().await = written;
    let carrier = crate::config::sessions_persistence::persist_initial_carrier(
        &manager,
        directory,
        vec![fixture._temp.path().to_string_lossy().into_owned()],
        crate::config::sessions_persistence::InitialCarrierIntent::new(
            member.target.clone(),
            previous,
            member.token,
            member.carrier_version,
            route.target.replica_identity.canonical_path.clone(),
        ),
    )
    .await
    .unwrap()
    .expect("real initial Confirmed A");
    let key = format!("prepare:{}", Uuid::new_v4());
    let mut admission = tracker.reserve_work(&member.target, &key).unwrap();
    admission.accept_durable("prepare-body").unwrap();
    drop(admission);
    let past = std::time::Instant::now() - Duration::from_secs(120);
    for publisher in [
        P::Manager,
        P::Pty,
        P::RawOutput,
        P::Detector,
        P::Menu,
        P::Hold,
    ] {
        tracker.bind_publisher(publisher);
    }
    tracker.mark_work_bootstrapped();
    tracker.try_enable_readiness().unwrap();
    tracker.apply_settings(true, 30, past);
    // Re-publish the real route identities on the fake Instant timeline
    // before admitting this test's first policy. No retirement is forged.
    for (identity, eligible) in [(&member, true), (&leader, false)] {
        tracker.register_route(identity.session_id, identity.generation, past);
        tracker.publish_manager_row(
            identity.session_id,
            identity.token,
            identity.generation,
            identity.carrier_version,
            eligible,
            false,
            past,
        );
        tracker.publish_manager_facts(identity.clone(), eligible, false, past);
    }
    tracker.observe_detector(previous, false, past);
    tracker.publish_menu(previous, Some(false), past);
    tracker.publish_hold(previous, None, true, past);
    let context = tracker
        .reserve_policy_candidate(
            member.clone(),
            leader.clone(),
            key,
            "prepare-body".into(),
            past,
        )
        .unwrap();
    tracker.mark_request_submitted(&context, true);
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            crate::config::sessions_persistence::register_published_carrier(
                &permit,
                &carrier,
                current,
                |_| {
                    tracker
                        .register_request(
                            crate::pty::manager::ResponseDeliveryProof {
                                context,
                                member: member.clone(),
                                coordinator: leader.clone(),
                                published_carrier_version: carrier.version,
                            },
                            past,
                        )
                        .ok()
                },
            )
        })
        .await
        .expect("real current published request registration");
    drop(permit);
    let context = tracker
        .admit_return(
            &member,
            &leader,
            format!("prepare-return-{}", Uuid::new_v4()),
            "prepare-return-body".into(),
            past,
        )
        .unwrap();
    tracker
        .observe_return_success(crate::pty::manager::ReturnDeliveryProof { context }, past)
        .unwrap();
    assert!(
        crate::session::selection::remember_response_close_registration(
            crate::session::selection::ResponseCloseRegistration {
                diagnostic_last_poll_outcome: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                tracker: tracker.clone(),
                member,
                coordinator: leader,
                carrier
            },
        )
    );
    crate::session::selection::poll_response_close(app).await;
    coordinator.snapshot().await.unwrap();
    let row = manager.get_session(previous).await.unwrap();
    assert_eq!(row.status, SessionStatus::Exited(0));
    assert!(
        tracker.generation(previous).is_none(),
        "actual finish removed route"
    );
    assert!(!app
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap()
        .has_session(previous));
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let retired = manager
        .with_current_carrier_snapshot(&permit, |current| {
            crate::config::sessions_persistence::retained_carrier_for_replacement(
                &permit,
                &crate::config::config_dir().unwrap(),
                &route.target.canonical_fqn,
                previous,
                current,
            )
            .is_some()
        })
        .await;
    drop(permit);
    assert!(retired, "genuine current retired Protected A prerequisite");
}

pub(super) fn provision_retained_fixture_member(
    fixture: &MailboxFixture,
    name: &str,
) -> (PathBuf, String) {
    let workspace = fixture.target_cwd.parent().unwrap();
    let ac_root = workspace.parent().unwrap();
    let origin = ac_root.join(format!("_agent_{name}"));
    let replica = workspace.join(format!("__agent_{name}"));
    std::fs::create_dir(&origin).unwrap();
    std::fs::create_dir(&replica).unwrap();
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_vec(&serde_json::json!({
            "identity": format!("../../_agent_{name}")
        }))
        .unwrap(),
    )
    .unwrap();
    let team_path = ac_root.join("_team_dev-team/config.json");
    let mut team: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&team_path).unwrap()).unwrap();
    team["agents"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::Value::String(format!("../_agent_{name}")));
    std::fs::write(team_path, serde_json::to_vec(&team).unwrap()).unwrap();
    (replica, format!("proj-a:wg-1-dev-team/{name}"))
}

pub(super) async fn prepare_additional_retained_member(
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    sender: Uuid,
    replica: &Path,
    target: &str,
) -> (crate::config::teams::VerifiedPtyInputRoute, Uuid) {
    let app = fixture.app.handle();
    let previous = add_mailbox_session(
        app,
        replica,
        "independent member",
        SessionStatus::Idle,
        None,
    )
    .await;
    let route = crate::config::teams::verify_pty_input_route(
        &fixture.sender_cwd,
        false,
        target,
        &[fixture._temp.path().to_string_lossy().into_owned()],
    )
    .unwrap();
    {
        let state = app.state::<Arc<Mutex<PtyManager>>>();
        let pty = state.lock().unwrap();
        pty.record_route_with_identities(
            previous,
            SessionBackendKind::LocalProcess,
            Some(crate::path_identity::verify_directory(replica).unwrap()),
            Some(route.target.replica_identity.clone()),
        )
        .unwrap();
        let backend = pty.backend_for_kind(SessionBackendKind::LocalProcess);
        backend
            .as_any()
            .downcast_ref::<MailboxMockPtyBackend>()
            .unwrap()
            .set_live(previous);
    }
    let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
    idle.register_session(previous, crate::session::profile::IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(previous);
    retire_retained_fixture_member(fixture, coordinator, &route, sender, previous).await;
    (route, previous)
}

pub(super) async fn set_retained_fixture_communication(
    manager: &SessionManager,
    id: Uuid,
    full: bool,
) -> Option<crate::session::session::SessionCommunication> {
    if !full {
        manager
            .set_telegram_bot_id(id, Some("newer-b".into()))
            .await;
        return None;
    }
    let (changed, tuple) = manager
        .set_blocked_menu(
            id,
            "newer-b-full".into(),
            chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        )
        .await
        .expect("real full communication setter");
    assert!(changed);
    Some(tuple)
}

pub(super) async fn observe_retained_commit_setter(
    control: &Arc<crate::session::selection::ReplacementFixtureControl>,
    manager: &SessionManager,
    resume_commit: tokio::sync::oneshot::Sender<()>,
    full: bool,
) -> Option<crate::session::session::SessionCommunication> {
    use std::future::Future;
    let receipt = retained_source_wait_cut(
        control,
        crate::session::selection::ReplacementFixtureCut::AfterDisarm,
    )
    .await;
    let id = receipt.binding.unwrap().session_id();
    let setter = set_retained_fixture_communication(manager, id, full);
    tokio::pin!(setter);
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(setter.as_mut().poll(cx).is_pending()))
            .await,
        "same setter must wait behind finalization U"
    );
    resume_commit.send(()).unwrap();
    setter.await
}

pub(super) async fn observe_retained_seed_fixture(
    fixture: &MailboxFixture,
    coordinator: &crate::session::selection::SelectionCoordinator,
    control: &Arc<crate::session::selection::ReplacementFixtureControl>,
    manager: &SessionManager,
    resume_native: tokio::sync::oneshot::Sender<()>,
    scenario: &str,
    seed_source: &Path,
) {
    use crate::session::selection::ReplacementFixtureCut as Cut;
    let app = fixture.app.handle();
    let entered = retained_source_wait_cut(control, Cut::SeedNativeEnter).await;
    let id = entered.binding.unwrap().session_id();
    let actual_pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    assert!(control.observe_owner(coordinator).unwrap().source_active);
    assert!(
        !control
            .cleanup_observed_stopped(coordinator, manager, &actual_pty)
            .await,
        "actual native seed closure is entered and Running, not cleanup authority"
    );
    assert!(manager
        .get_pending_session(entered.binding.unwrap())
        .await
        .is_some());
    let blocker = fixture
        .target_cwd
        .join(format!(".p05-seed.acseed-tmp-{id}"));
    let destination = fixture.target_cwd.join(".p05-seed");
    assert!(!destination.exists());
    if scenario == "seed-staging-file" {
        std::fs::write(&blocker, b"ordinary staging blocker").unwrap();
    }
    resume_native.send(()).unwrap();
    retained_source_wait_cut(control, Cut::SeedNativeReturned).await;
    let native = control
        .native_results
        .lock()
        .unwrap()
        .iter()
        .find(|(cut, _)| *cut == Cut::SeedNativeReturned)
        .map(|(_, result)| result.clone())
        .expect("actual seed report");
    if scenario == "seed-staging-file" {
        assert!(
            native.starts_with("Failed(Staging"),
            "actual staging failure: {native}"
        );
        assert!(
            native.contains(&format!("source: {seed_source:?}")),
            "selected readable source: {native}"
        );
        assert!(
            native.contains(&format!("staging_path: {blocker:?}")),
            "exact actual B suffix: {native}"
        );
        assert!(
            native.contains("create "),
            "actual staging create-path error: {native}"
        );
        assert!(std::fs::symlink_metadata(&blocker).unwrap().is_file());
        assert_eq!(
            std::fs::read(&blocker).unwrap(),
            b"ordinary staging blocker"
        );
        assert!(
            !destination.exists(),
            "best-effort staging failure did not partially publish"
        );
        std::fs::remove_file(&blocker).unwrap();
    } else {
        assert!(
            native.starts_with("Published("),
            "actual seed publication: {native}"
        );
        assert_eq!(
            std::fs::read(destination.join("fixture.txt")).unwrap(),
            b"actual seed fixture content"
        );
    }
}

pub(super) async fn configure_retained_seed_fixture(fixture: &MailboxFixture, seed_source: &Path) {
    let app = fixture.app.handle();
    let seed_lock: crate::ConfigSeedLockState = Arc::new(tokio::sync::Mutex::new(()));
    assert!(app.manage(seed_lock));
    std::fs::create_dir(seed_source).unwrap();
    std::fs::write(
        seed_source.join("fixture.txt"),
        b"actual seed fixture content",
    )
    .unwrap();
    let mut settings = app.state::<SettingsState>().read().await.clone();
    settings
        .agents
        .iter_mut()
        .find(|agent| agent.id == "codex")
        .unwrap()
        .config_seed = Some(crate::config::settings::ConfigSeedConfig {
        enabled: true,
        dest: ".p05-seed".into(),
    });
    crate::config::settings::validate_and_repair_settings(&mut settings)
        .expect("actual seed settings validator");
    let written = crate::config::settings::save_settings(&settings).unwrap();
    *app.state::<SettingsState>().write().await = written;
}
