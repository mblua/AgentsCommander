//! Protected carrier provisioning and retirement fixture.

use super::native_fixture::NativeFixture;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

pub(crate) struct ProtectedFixture {
    pub(crate) native: NativeFixture,
    pub(crate) route: crate::config::teams::VerifiedPtyInputRoute,
    pub(crate) carrier: crate::config::sessions_persistence::PublishedCarrier,
    pub(crate) sender: Uuid,
    pub(crate) parent_source: Option<Uuid>,
}
impl ProtectedFixture {
    pub(crate) async fn new(with_parent: bool) -> Self {
        let name = std::env::var("AC_P17_CARRIER_CHILD").unwrap();
        isolated_identity(&name);
        let mut f = NativeFixture::build(false).await;
        let tracker = f.pty.lock().unwrap().response_close_state();
        *f.manager.write().await =
            crate::session::manager::SessionManager::with_response_close(tracker.clone());
        assert!(f.app.manage(tracker));
        let project = f.cwd.path().join("proj-a");
        let ac = project.join(".ac");
        let room = ac.join("wg-1-dev-team");
        let sender = room.join("__agent_tech-lead");
        let target = room.join("__agent_dev-rust");
        let team = ac.join("_team_dev-team");
        for path in [
            &sender,
            &target,
            &team,
            &ac.join("_agent_tech-lead"),
            &ac.join("_agent_dev-rust"),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        let config = serde_json::json!({
            "agents": ["../_agent_dev-rust", "../_agent_tech-lead"],
            "coordinator": "../_agent_tech-lead"
        });
        std::fs::write(
            team.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        for (path, origin) in [(&sender, "tech-lead"), (&target, "dev-rust")] {
            let config = serde_json::json!({ "identity": format!("../../_agent_{origin}") });
            std::fs::write(
                path.join("config.json"),
                serde_json::to_vec(&config).unwrap(),
            )
            .unwrap();
        }
        f.coordinator.start(f.app.handle().clone()).unwrap();
        f.coordinator.submit_restore_first().await.unwrap().finish();
        f.coordinator.snapshot().await.unwrap();
        let sender_id = f.create_source_at(&sender).await;
        f.source = f.create_source_at(&target).await;
        let parent_source = if with_parent {
            Some(
                tokio::time::timeout(
                    std::time::Duration::from_secs(20),
                    f.create_source_at(target.parent().unwrap()),
                )
                .await
                .expect("unknown parent source creation must complete"),
            )
        } else {
            None
        };
        let project_paths = vec![f.cwd.path().to_string_lossy().into_owned()];
        let route = crate::config::teams::verify_pty_input_route(
            &sender,
            false,
            "proj-a:wg-1-dev-team/dev-rust",
            &project_paths,
        )
        .unwrap();
        let manager = f.manager.read().await.clone();
        manager
            .rename_session(sender_id, "coordinator".into())
            .await
            .unwrap();
        manager
            .rename_session(f.source, "member".into())
            .await
            .unwrap();
        manager.set_is_coordinator(sender_id, true).await;
        manager.mark_idle(f.source).await;
        manager
            .set_agent_kind(
                f.source,
                Some(crate::session::profile::CodingAgentKind::Codex),
            )
            .await;
        let settings = f
            .app
            .state::<crate::config::settings::SettingsState>()
            .read()
            .await
            .clone();
        crate::config::settings::save_settings_with_project_paths(&settings).unwrap();
        let written = crate::config::settings::save_settings(&settings).unwrap();
        *f.app
            .state::<crate::config::settings::SettingsState>()
            .write()
            .await = written;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let (token, version) = manager
            .with_current_carrier_snapshot(&permit, |current| {
                let row = current.rows.iter().find(|row| row.id == f.source).unwrap();
                (row.token, current.versions[&row.id])
            })
            .await;
        drop(permit);
        let carrier = crate::config::sessions_persistence::persist_initial_carrier(
            &manager,
            crate::config::config_dir().unwrap(),
            project_paths,
            crate::config::sessions_persistence::InitialCarrierIntent::new(
                route.target.canonical_fqn.clone(),
                f.source,
                token,
                version,
                route.target.replica_identity.canonical_path.clone(),
            ),
        )
        .await
        .unwrap()
        .expect("actual initial carrier");
        Self {
            native: f,
            route,
            carrier,
            sender: sender_id,
            parent_source,
        }
    }
}

impl ProtectedFixture {
    pub(crate) async fn retire(&self) {
        use crate::pty::manager::{ResponseClosePublisher as P, ResponseRouteIdentity};
        use std::time::Duration;
        let manager = self.native.manager.read().await.clone();
        let tracker = self.native.pty.lock().unwrap().response_close_state();
        let previous = self.native.source;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let (member, leader) = manager
            .with_current_carrier_snapshot(&permit, |current| {
                let identity = |id, target| {
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
                    identity(previous, self.route.target.canonical_fqn.clone()),
                    identity(self.sender, self.route.sender.canonical_fqn.clone()),
                )
            })
            .await;
        drop(permit);
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
                    &self.carrier,
                    current,
                    |_| {
                        tracker
                            .register_request(
                                crate::pty::manager::ResponseDeliveryProof {
                                    context,
                                    member: member.clone(),
                                    coordinator: leader.clone(),
                                    published_carrier_version: self.carrier.version,
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
                    carrier: self.carrier.clone()
                },
            )
        );
        crate::session::selection::poll_response_close(self.native.app.handle()).await;
        self.native.coordinator.snapshot().await.unwrap();
    }
}

fn isolated_identity(name: &str) -> (String, std::path::PathBuf) {
    assert_eq!(std::env::var("AC_P17_CARRIER_CHILD").unwrap(), name);
    let identity = std::env::var("AC_P17_CARRIER_IDENTITY").unwrap();
    let (id, exact) = identity.split_once('\n').unwrap();
    assert!(!Uuid::parse_str(id).unwrap().is_nil());
    assert_eq!(exact, name);
    let config =
        std::path::PathBuf::from(std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").unwrap());
    assert!(config.is_absolute());
    assert_eq!(
        std::path::PathBuf::from(std::env::var("AGENTSCOMMANDER_CONFIG_DIR").unwrap()),
        config
    );
    assert_eq!(crate::config::config_dir().unwrap(), config);
    (identity, config.parent().unwrap().join("completion.txt"))
}

pub(crate) fn complete_isolated(name: &str) {
    let (identity, receipt) = isolated_identity(name);
    assert!(!receipt.exists());
    std::fs::write(receipt, identity).unwrap();
}

struct CarrierChild {
    child: std::process::Child,
    directory: Option<tempfile::TempDir>,
    reaped: bool,
}

impl CarrierChild {
    fn retain_failure(&mut self, reason: &str) -> ! {
        let directory = self.directory.take().unwrap().keep();
        let stdout = std::fs::read_to_string(directory.join("stdout.txt")).unwrap_or_default();
        let stderr = std::fs::read_to_string(directory.join("stderr.txt")).unwrap_or_default();
        panic!(
            "{reason}; evidence={}\n{stdout}\n{stderr}",
            directory.display()
        );
    }
}

impl Drop for CarrierChild {
    fn drop(&mut self) {
        if !self.reaped {
            if let Err(error) = self.child.kill() {
                eprintln!("P17 child kill: {error}");
            }
            if let Err(error) = self.child.wait() {
                eprintln!("P17 child wait: {error}");
            }
        }
    }
}

pub(crate) async fn run_isolated(name: &str, body: impl std::future::Future<Output = ()>) {
    if let Ok(marker) = std::env::var("AC_P17_CARRIER_CHILD") {
        assert_eq!(marker, name);
        let (_, receipt) = isolated_identity(name);
        assert!(!receipt.exists());
        body.await;
        return;
    }
    assert!(std::env::var("AC_P17_CARRIER_IDENTITY").is_err());
    drop(body);
    let directory = tempfile::Builder::new()
        .prefix("p17-carrier-")
        .tempdir()
        .unwrap();
    let config = directory.path().join("config");
    let temporary = directory.path().join("tmp");
    std::fs::create_dir(&config).unwrap();
    std::fs::create_dir(&temporary).unwrap();
    let identity = format!("{}\n{name}", Uuid::new_v4());
    std::fs::write(directory.path().join("identity.txt"), &identity).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .current_dir(std::env::current_dir().unwrap())
        .stdin(std::process::Stdio::null())
        .stdout(std::fs::File::create(directory.path().join("stdout.txt")).unwrap())
        .stderr(std::fs::File::create(directory.path().join("stderr.txt")).unwrap())
        .env("AGENTSCOMMANDER_CONFIG_DIR", &config)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &config)
        .env("TEMP", &temporary)
        .env("TMP", &temporary)
        .env("TMPDIR", &temporary)
        .env("AC_P17_CARRIER_CHILD", name)
        .env("AC_P17_CARRIER_IDENTITY", &identity);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut owner = CarrierChild {
        child: command.spawn().unwrap(),
        directory: Some(directory),
        reaped: false,
    };
    let started = std::time::Instant::now();
    let status = loop {
        match owner.child.try_wait() {
            Ok(Some(status)) => {
                owner.reaped = true;
                break status;
            }
            Ok(None) if started.elapsed() < std::time::Duration::from_secs(120) => {
                tokio::task::yield_now().await;
            }
            Ok(None) => {
                let kill = owner.child.kill();
                let wait = owner.child.wait();
                owner.reaped = wait.is_ok();
                owner.retain_failure(&format!("child deadline; kill={kill:?}; wait={wait:?}"));
            }
            Err(error) => owner.retain_failure(&format!("child poll: {error}")),
        }
    };
    let directory = owner.directory.as_ref().unwrap().path();
    std::fs::write(
        directory.join("exit.txt"),
        format!("{status}; elapsed={:?}", started.elapsed()),
    )
    .unwrap();
    let receipt = std::fs::read_to_string(directory.join("completion.txt"));
    if !status.success() || !receipt.is_ok_and(|value| value == identity) {
        owner.retain_failure("child exit or completion receipt failed");
    }
    eprintln!(
        "P17 child {name}: config={} receipt={identity:?} exit={status}",
        config.display()
    );
}
