use super::*;

#[cfg(windows)]
fn physical_cwd_child_or_parent(wrapper: &str) -> bool {
    use std::os::windows::process::CommandExt;
    const MARKER: &str = "AC_P05_PHYSICAL_CWD_CHILD";
    const IDENTITY: &str = "AC_P05_PHYSICAL_CWD_IDENTITY";
    if let Some(marked_wrapper) = std::env::var_os(MARKER) {
        assert_eq!(
            marked_wrapper.to_str(),
            Some(wrapper),
            "wrong child wrapper"
        );
        let identity = std::env::var(IDENTITY).expect("child identity");
        let (run, suffix) = identity.split_once('\n').expect("run-bound identity");
        Uuid::parse_str(run).expect("parent run nonce");
        assert_eq!(suffix, wrapper);
        let config =
            PathBuf::from(std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR").expect("child config"));
        let test_config = PathBuf::from(
            std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR").expect("child test config"),
        );
        assert!(config.is_absolute());
        assert_eq!(config, test_config);
        assert_eq!(crate::config::config_dir().as_ref(), Some(&config));
        assert!(
            !config.join("completion.txt").exists(),
            "receipt must be fresh"
        );
        return false;
    }
    assert!(
        std::env::var_os(IDENTITY).is_none(),
        "orphan child identity"
    );
    let directory = tempfile::tempdir().expect("parent child directory");
    let path = directory
        .path()
        .canonicalize()
        .expect("absolute child directory");
    let temporary = path.join("tmp");
    std::fs::create_dir(&temporary).expect("child temporary directory");
    let identity = format!("{}\n{wrapper}", Uuid::new_v4());
    let stdout = std::fs::File::create(path.join("stdout.log")).expect("child stdout");
    let stderr = std::fs::File::create(path.join("stderr.log")).expect("child stderr");
    let child = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args([wrapper, "--exact", "--nocapture", "--test-threads=1"])
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .env(MARKER, wrapper)
        .env(IDENTITY, &identity)
        .env("AGENTSCOMMANDER_CONFIG_DIR", &path)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &path)
        .env("TEMP", &temporary)
        .env("TMP", &temporary)
        .env("TMPDIR", &temporary)
        .creation_flags(0x08000000)
        .spawn()
        .expect("spawn exact physical cwd child");
    let mut owner = RetainedSourceChild {
        child,
        directory: Some(directory),
        reaped: false,
    };
    owner.supervise(&identity, "physical-cwd");
    true
}

#[cfg(windows)]
fn complete_physical_cwd_child(wrapper: &str) {
    assert_eq!(
        std::env::var("AC_P05_PHYSICAL_CWD_CHILD").as_deref(),
        Ok(wrapper),
        "completion requires exact child marker"
    );
    assert!(
        !physical_cwd_child_or_parent(wrapper),
        "completion requires child"
    );
    let identity = std::env::var("AC_P05_PHYSICAL_CWD_IDENTITY").expect("child identity");
    let config =
        PathBuf::from(std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR").expect("child config"));
    std::fs::write(config.join("completion.txt"), identity).expect("child assertion receipt");
}

#[cfg(windows)]
#[tokio::test]
async fn response_request_physical_cwd_boundary() {
    if physical_cwd_child_or_parent(
        "phone::mailbox::tests::seed_shared::response_request_physical_cwd_boundary",
    ) {
        return;
    }
    use crate::pty::manager::{ResponseClosePublisher as P, ResponseRouteIdentity};
    let configured = PathBuf::from(
        std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").expect("isolated test config"),
    );
    assert!(configured.is_absolute());
    if let Some(public) = std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR") {
        assert_eq!(
            PathBuf::from(public),
            configured,
            "public override must match before config access"
        );
    }
    assert_eq!(crate::config::config_dir().unwrap(), configured);
    for name in ["TEMP", "TMP", "TMPDIR"] {
        assert!(
            PathBuf::from(std::env::var_os(name).expect("isolated absolute temp")).is_absolute()
        );
    }
    for (canonical_member, canonical_coordinator) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        assert_eq!(crate::config::config_dir().unwrap(), configured);
        let fixture = make_mailbox_fixture();
        let app = fixture.app.handle();
        let tracker = crate::pty::manager::ResponseCloseState::default();
        let managers = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        *managers.write().await = SessionManager::with_response_close(tracker.clone());
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        pty.lock().unwrap().bind_response_close(tracker.clone());
        app.manage(tracker.clone());
        let (route, sender, member_id) =
            seed_shared_pty_engine_sessions(&fixture, SessionStatus::Idle).await;
        let manager = managers.read().await.clone();
        manager.set_is_coordinator(sender, true).await;
        manager
            .set_agent_kind(
                member_id,
                Some(crate::session::profile::CodingAgentKind::Codex),
            )
            .await;
        let roots = {
            let state = app.state::<SettingsState>();
            let mut settings = state.write().await;
            settings.response_close_enabled = true;
            settings.response_close_idle_seconds = 30;
            settings.project_paths.clone()
        };
        let member_physical = crate::path_identity::verify_directory(&fixture.target_cwd).unwrap();
        let coordinator_physical =
            crate::path_identity::verify_directory(&fixture.sender_cwd).unwrap();
        let ordinary_member =
            crate::path_utils::normalize_windows_verbatim_path_buf(&member_physical.canonical_path);
        let ordinary_coordinator = crate::path_utils::normalize_windows_verbatim_path_buf(
            &coordinator_physical.canonical_path,
        );
        assert_ne!(ordinary_member, member_physical.canonical_path);
        assert_ne!(ordinary_coordinator, coordinator_physical.canonical_path);
        assert_eq!(
            crate::path_identity::verify_directory(&ordinary_member)
                .unwrap()
                .object_id,
            member_physical.object_id
        );
        assert_eq!(
            crate::path_identity::verify_directory(&ordinary_coordinator)
                .unwrap()
                .object_id,
            coordinator_physical.object_id
        );
        let other = fixture._temp.path().join("different-directory");
        std::fs::create_dir(&other).unwrap();
        let file = fixture._temp.path().join("regular-file");
        std::fs::write(&file, b"not a directory").unwrap();
        let invalid_paths = [
            other.clone(),
            other.join("missing"),
            file,
            other.join("..").join("different-directory"),
        ];
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
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
        let key = "physical-cwd-request".to_string();
        let digest = "physical-cwd-body".to_string();
        let mut admission = tracker
            .reserve_work(&route.target.canonical_fqn, &key)
            .unwrap();
        admission.accept_durable(&digest).unwrap();
        drop(admission);
        let work = ResponseWakeWork {
            diagnostic_message_id: None,
            state: tracker.clone(),
            target: route.target.canonical_fqn.clone(),
            key,
            digest,
            attempt: Arc::new(Mutex::new(None)),
            return_context: None,
            return_routes: None,
            delivery_context: None,
        };
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                let member_row = current.rows.iter().find(|row| row.id == member_id).unwrap();
                let coordinator_row = current.rows.iter().find(|row| row.id == sender).unwrap();
                let member = ResponseRouteIdentity {
                    target: route.target.canonical_fqn.clone(),
                    session_id: member_id,
                    token: member_row.token,
                    generation: tracker.generation(member_id).unwrap(),
                    carrier_version: current.versions[&member_id],
                    backend: member_row.backend_kind,
                };
                let coordinator = ResponseRouteIdentity {
                    target: route.sender.canonical_fqn.clone(),
                    session_id: sender,
                    token: coordinator_row.token,
                    generation: tracker.generation(sender).unwrap(),
                    carrier_version: current.versions[&sender],
                    backend: coordinator_row.backend_kind,
                };
                let mut positive = current.clone();
                positive
                    .rows
                    .iter_mut()
                    .find(|row| row.id == member_id)
                    .unwrap()
                    .working_directory = if canonical_member {
                    &member_physical.canonical_path
                } else {
                    &ordinary_member
                }
                .to_string_lossy()
                .into_owned();
                positive
                    .rows
                    .iter_mut()
                    .find(|row| row.id == sender)
                    .unwrap()
                    .working_directory = if canonical_coordinator {
                    &coordinator_physical.canonical_path
                } else {
                    &ordinary_coordinator
                }
                .to_string_lossy()
                .into_owned();
                for role in [member_id, sender] {
                    // The first case reaches the real write boundary; each later case changes one
                    // fact.
                    for failure in 0..7 {
                        let mut snapshot = positive.clone();
                        let mut prepared = PreparedResponseRequest {
                            member: member.clone(),
                            coordinator: coordinator.clone(),
                            roots: roots.clone(),
                        };
                        if (1..=4).contains(&failure) {
                            snapshot
                                .rows
                                .iter_mut()
                                .find(|row| row.id == role)
                                .unwrap()
                                .working_directory =
                                invalid_paths[failure - 1].to_string_lossy().into_owned();
                        }
                        if failure == 5 {
                            if role == member_id {
                                prepared.member.generation += 1;
                            } else {
                                prepared.coordinator.generation += 1;
                            }
                        }
                        if failure == 6 {
                            snapshot
                                .rows
                                .iter_mut()
                                .find(|row| row.id == role)
                                .unwrap()
                                .backend_kind = SessionBackendKind::ContainerTransport;
                            if role == member_id {
                                prepared.member.backend = SessionBackendKind::ContainerTransport;
                            } else {
                                prepared.coordinator.backend =
                                    SessionBackendKind::ContainerTransport;
                            }
                        }
                        let disposal = work.candidate_disposal();
                        let session = snapshot
                            .rows
                            .iter()
                            .find(|row| row.id == member_id)
                            .unwrap();
                        reserve_response_request_at_write(
                            app,
                            &snapshot,
                            session,
                            &prepared,
                            &work,
                            &disposal.0,
                        );
                        assert_eq!(
                            disposal.0.candidate.lock().unwrap().is_some(),
                            failure == 0,
                            "role={role} case={failure}"
                        );
                        drop(disposal);
                    }
                }
            })
            .await;
        drop(permit);
    }
    complete_physical_cwd_child(
        "phone::mailbox::tests::seed_shared::response_request_physical_cwd_boundary",
    );
}

#[cfg(windows)]
#[tokio::test]
async fn response_pair_physical_cwd_boundary() {
    if physical_cwd_child_or_parent(
        "phone::mailbox::tests::seed_shared::response_pair_physical_cwd_boundary",
    ) {
        return;
    }
    use crate::pty::manager::{ResponseClosePublisher as P, ResponseRouteIdentity};
    let configured = PathBuf::from(
        std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").expect("isolated test config"),
    );
    assert!(configured.is_absolute());
    if let Some(public) = std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR") {
        assert_eq!(
            PathBuf::from(public),
            configured,
            "public override must match before config access"
        );
    }
    assert_eq!(crate::config::config_dir().unwrap(), configured);
    for name in ["TEMP", "TMP", "TMPDIR"] {
        assert!(
            PathBuf::from(std::env::var_os(name).expect("isolated absolute temp")).is_absolute()
        );
    }
    for (canonical_member, canonical_coordinator) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        assert_eq!(crate::config::config_dir().unwrap(), configured);
        let fixture = make_mailbox_fixture();
        let app = fixture.app.handle();
        let tracker = crate::pty::manager::ResponseCloseState::default();
        let managers = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        *managers.write().await = SessionManager::with_response_close(tracker.clone());
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        pty.lock().unwrap().bind_response_close(tracker.clone());
        app.manage(tracker.clone());
        let (route, sender, member_id) =
            seed_shared_pty_engine_sessions(&fixture, SessionStatus::Idle).await;
        let manager = managers.read().await.clone();
        manager.set_is_coordinator(sender, true).await;
        manager
            .set_agent_kind(
                member_id,
                Some(crate::session::profile::CodingAgentKind::Codex),
            )
            .await;
        let roots = {
            let state = app.state::<SettingsState>();
            let mut settings = state.write().await;
            settings.response_close_enabled = true;
            settings.response_close_idle_seconds = 30;
            settings.project_paths.clone()
        };
        let member_physical = crate::path_identity::verify_directory(&fixture.target_cwd).unwrap();
        let coordinator_physical =
            crate::path_identity::verify_directory(&fixture.sender_cwd).unwrap();
        let ordinary_member =
            crate::path_utils::normalize_windows_verbatim_path_buf(&member_physical.canonical_path);
        let ordinary_coordinator = crate::path_utils::normalize_windows_verbatim_path_buf(
            &coordinator_physical.canonical_path,
        );
        assert_ne!(ordinary_member, member_physical.canonical_path);
        assert_ne!(ordinary_coordinator, coordinator_physical.canonical_path);
        assert_eq!(
            crate::path_identity::verify_directory(&ordinary_member)
                .unwrap()
                .object_id,
            member_physical.object_id
        );
        assert_eq!(
            crate::path_identity::verify_directory(&ordinary_coordinator)
                .unwrap()
                .object_id,
            coordinator_physical.object_id
        );
        let other = fixture._temp.path().join("different-directory");
        std::fs::create_dir(&other).unwrap();
        let file = fixture._temp.path().join("regular-file");
        std::fs::write(&file, b"not a directory").unwrap();
        let invalid_paths = [
            other.clone(),
            other.join("missing"),
            file,
            other.join("..").join("different-directory"),
        ];
        assert_eq!(
            roots,
            app.state::<SettingsState>().read().await.project_paths
        );
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
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                let member_row = current.rows.iter().find(|row| row.id == member_id).unwrap();
                let coordinator_row = current.rows.iter().find(|row| row.id == sender).unwrap();
                let member = ResponseRouteIdentity {
                    target: route.target.canonical_fqn.clone(),
                    session_id: member_id,
                    token: member_row.token,
                    generation: tracker.generation(member_id).unwrap(),
                    carrier_version: current.versions[&member_id],
                    backend: member_row.backend_kind,
                };
                let coordinator = ResponseRouteIdentity {
                    target: route.sender.canonical_fqn.clone(),
                    session_id: sender,
                    token: coordinator_row.token,
                    generation: tracker.generation(sender).unwrap(),
                    carrier_version: current.versions[&sender],
                    backend: coordinator_row.backend_kind,
                };
                let mut positive = current.clone();
                positive
                    .rows
                    .iter_mut()
                    .find(|row| row.id == member_id)
                    .unwrap()
                    .working_directory = if canonical_member {
                    &member_physical.canonical_path
                } else {
                    &ordinary_member
                }
                .to_string_lossy()
                .into_owned();
                positive
                    .rows
                    .iter_mut()
                    .find(|row| row.id == sender)
                    .unwrap()
                    .working_directory = if canonical_coordinator {
                    &coordinator_physical.canonical_path
                } else {
                    &ordinary_coordinator
                }
                .to_string_lossy()
                .into_owned();
                for role in [member_id, sender] {
                    for failure in 0..7 {
                        let mut snapshot = positive.clone();
                        let mut captured_member = member.clone();
                        let mut captured_coordinator = coordinator.clone();
                        if (1..=4).contains(&failure) {
                            snapshot
                                .rows
                                .iter_mut()
                                .find(|row| row.id == role)
                                .unwrap()
                                .working_directory =
                                invalid_paths[failure - 1].to_string_lossy().into_owned();
                        }
                        if failure == 5 {
                            if role == member_id {
                                captured_member.generation += 1;
                            } else {
                                captured_coordinator.generation += 1;
                            }
                        }
                        if failure == 6 {
                            snapshot
                                .rows
                                .iter_mut()
                                .find(|row| row.id == role)
                                .unwrap()
                                .backend_kind = SessionBackendKind::ContainerTransport;
                            if role == member_id {
                                captured_member.backend = SessionBackendKind::ContainerTransport;
                            } else {
                                captured_coordinator.backend =
                                    SessionBackendKind::ContainerTransport;
                            }
                        }
                        let mut calls = 0;
                        let result = with_verified_response_pair(
                            &snapshot,
                            &pty,
                            &tracker,
                            &captured_member,
                            &captured_coordinator,
                            |_, _| {
                                calls += 1;
                            },
                        );
                        assert_eq!(
                            calls,
                            usize::from(failure == 0),
                            "role={role} case={failure}"
                        );
                        assert_eq!(result.is_some(), failure == 0);
                    }
                }
            })
            .await;
        drop(permit);
    }
    complete_physical_cwd_child(
        "phone::mailbox::tests::seed_shared::response_pair_physical_cwd_boundary",
    );
}

#[cfg(windows)]
#[tokio::test]
async fn response_close_physical_cwd_reaches_dormant() {
    if physical_cwd_child_or_parent(
        "phone::mailbox::tests::seed_shared::response_close_physical_cwd_reaches_dormant",
    ) {
        return;
    }
    let configured = PathBuf::from(
        std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").expect("isolated test config"),
    );
    assert!(configured.is_absolute());
    if let Some(public) = std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR") {
        assert_eq!(
            PathBuf::from(public),
            configured,
            "public override must match before config access"
        );
    }
    assert_eq!(crate::config::config_dir().unwrap(), configured);
    for name in ["TEMP", "TMP", "TMPDIR"] {
        assert!(
            PathBuf::from(std::env::var_os(name).expect("isolated absolute temp")).is_absolute()
        );
    }
    for (canonical_member, canonical_coordinator) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        assert_eq!(crate::config::config_dir().unwrap(), configured);
        let mut fixture = make_mailbox_fixture();
        let member_physical = crate::path_identity::verify_directory(&fixture.target_cwd).unwrap();
        let coordinator_physical =
            crate::path_identity::verify_directory(&fixture.sender_cwd).unwrap();
        let ordinary_member =
            crate::path_utils::normalize_windows_verbatim_path_buf(&member_physical.canonical_path);
        let ordinary_coordinator = crate::path_utils::normalize_windows_verbatim_path_buf(
            &coordinator_physical.canonical_path,
        );
        assert_ne!(ordinary_member, member_physical.canonical_path);
        assert_ne!(ordinary_coordinator, coordinator_physical.canonical_path);
        assert_eq!(
            crate::path_identity::verify_directory(&ordinary_member)
                .unwrap()
                .object_id,
            member_physical.object_id
        );
        assert_eq!(
            crate::path_identity::verify_directory(&ordinary_coordinator)
                .unwrap()
                .object_id,
            coordinator_physical.object_id
        );
        fixture.target_cwd = if canonical_member {
            member_physical.canonical_path
        } else {
            ordinary_member
        };
        fixture.sender_cwd = if canonical_coordinator {
            coordinator_physical.canonical_path
        } else {
            ordinary_coordinator
        };
        let (coordinator, _, _, member) = prepare_retained_exited_respawn(&fixture).await;
        assert!(crate::session::selection::response_close_registration(member).is_none());
        let report = tokio::time::timeout(Duration::from_secs(5), coordinator.close_and_join())
            .await
            .expect("focused dormant fixture close");
        assert!(report.persistence_safe, "{:?}", report.retained);
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        assert!(
            crate::config::sessions_persistence::retire_carrier_for_discard(&permit, member),
            "finished focused fixture must release only its own retired carrier"
        );
        drop(permit);
    }
    complete_physical_cwd_child(
        "phone::mailbox::tests::seed_shared::response_close_physical_cwd_reaches_dormant",
    );
}
