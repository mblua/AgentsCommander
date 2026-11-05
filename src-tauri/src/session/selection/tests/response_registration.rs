#[cfg(windows)]
use super::*;

#[cfg(windows)]
async fn wait_registration_child(
    child: &mut std::process::Child,
) -> (Result<std::process::ExitStatus, String>, bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut reaped = false;
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                reaped = true;
                break Ok(status);
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            pending_or_error => {
                let mut reason = match pending_or_error {
                    Err(error) => format!("registration child wait failed: {error}"),
                    _ => "registration child exceeded 60 seconds".to_string(),
                };
                if let Err(error) = child.kill() {
                    reason.push_str(&format!("; kill failed: {error}"));
                }
                match child.wait() {
                    Ok(_) => reaped = true,
                    Err(error) => reason.push_str(&format!("; reap failed: {error}")),
                }
                break Err(reason);
            }
        }
    };
    (result, reaped)
}

#[cfg(windows)]
fn replay_registration_child_logs(
    stdout_path: &std::path::Path,
    stderr_path: &std::path::Path,
) -> Vec<String> {
    let mut log_errors = Vec::new();
    match std::fs::read_to_string(stdout_path) {
        Ok(log) => print!("{log}"),
        Err(error) => log_errors.push(format!("read child stdout failed: {error}")),
    }
    match std::fs::read_to_string(stderr_path) {
        Ok(log) => eprint!("{log}"),
        Err(error) => log_errors.push(format!("read child stderr failed: {error}")),
    }
    log_errors
}

#[cfg(windows)]
fn assert_registration_child_environment(marker: Option<std::ffi::OsString>) -> std::path::PathBuf {
    use std::path::PathBuf;
    let expected = std::path::PathBuf::from(marker.expect("registration child marker"));
    assert!(
        expected.is_absolute(),
        "child config directory must be absolute"
    );
    for override_name in [
        "AGENTSCOMMANDER_CONFIG_DIR",
        "AGENTSCOMMANDER_TEST_CONFIG_DIR",
    ] {
        assert_eq!(
            std::env::var_os(override_name).map(std::path::PathBuf::from),
            Some(expected.clone()),
            "child override must match its recursion marker: {override_name}"
        );
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
    configured
}

#[cfg(windows)]
async fn create_registration_session(
    manager: &SessionManager,
    path: &std::path::Path,
) -> crate::session::session::Session {
    use crate::pty::backend::SessionBackendKind;
    manager
        .create_session(
            "shell".into(),
            Vec::new(),
            path.to_string_lossy().into_owned(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
}

#[cfg(windows)]
fn registration_spawn_spec(row_id: Uuid, cwd: String) -> crate::pty::backend::BackendSpawnSpec {
    crate::pty::backend::BackendSpawnSpec {
        id: row_id,
        agent_id: None,
        coding_agent: None,
        cmd: "shell".to_string(),
        args: Vec::new(),
        resolved_agent_host_shell: None,
        cwd,
        selected_cwd: None,
        cols: 120,
        rows: 30,
        container_image: None,
        configured_env: Vec::new(),
        env_remove_keys: Vec::new(),
        env_unset: Vec::new(),
        extra_env: Vec::new(),
        idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
        output_target: crate::pty::output::PtyOutputTarget::noop(),
        resource_registration: None,
        logical_resource_slot: None,
        container_credential: None,
        container_repo_mounts: Vec::new(),
        launch_witness: Default::default(),
    }
}

#[cfg(windows)]
fn remove_registration_route(
    pty: &Arc<Mutex<PtyManager>>,
    backend: &LifecycleTestBackend,
    row_id: Uuid,
) {
    use crate::pty::backend::SessionBackendKind;
    pty.lock()
        .unwrap()
        .remove_route_if_kind(row_id, SessionBackendKind::LocalProcess);
    assert!(pty.lock().unwrap().backend_kind(row_id).is_none());
    assert!(pty
        .lock()
        .unwrap()
        .response_close_state()
        .generation(row_id)
        .is_none());
    backend.set_live(row_id, false);
}

#[cfg(windows)]
async fn respawn_registration_spellings(
    pty: &Arc<Mutex<PtyManager>>,
    backend: &LifecycleTestBackend,
    row: &crate::session::session::Session,
) -> (std::path::PathBuf, std::path::PathBuf, u64) {
    use crate::pty::backend::SessionBackendKind;
    let ordinary_path = crate::path_utils::normalize_windows_verbatim_path_buf(
        &std::fs::canonicalize(&row.working_directory).unwrap(),
    );
    let ordinary_spec =
        registration_spawn_spec(row.id, ordinary_path.to_string_lossy().into_owned());
    PtyManager::spawn(pty, SessionBackendKind::LocalProcess, ordinary_spec)
        .await
        .unwrap();
    let proof = PtyManager::snapshot_route_proof(pty, row.id).unwrap();
    assert!(proof.saved_replica().is_none());
    let ordinary_id = proof.saved_cwd().object_id;
    let canonical_path = proof.saved_cwd().canonical_path.clone();
    assert_ne!(ordinary_path, canonical_path);
    let first_generation = pty
        .lock()
        .unwrap()
        .response_close_state()
        .generation(row.id)
        .expect("ordinary route registered");
    drop(proof);
    remove_registration_route(pty, backend, row.id);
    let canonical_spec =
        registration_spawn_spec(row.id, canonical_path.to_string_lossy().into_owned());
    PtyManager::spawn(pty, SessionBackendKind::LocalProcess, canonical_spec)
        .await
        .unwrap();
    let proof = PtyManager::snapshot_route_proof(pty, row.id).unwrap();
    assert!(proof.saved_replica().is_none());
    let canonical_id = proof.saved_cwd().object_id;
    assert_eq!(ordinary_id, canonical_id, "independent spawn verifications");
    let final_generation = pty
        .lock()
        .unwrap()
        .response_close_state()
        .generation(row.id)
        .expect("canonical route registered");
    assert!(final_generation > first_generation);
    drop(proof);
    (ordinary_path, canonical_path, final_generation)
}

#[cfg(windows)]
struct RegistrationRowCase<'a> {
    pty: &'a Arc<Mutex<PtyManager>>,
    current: &'a crate::session::manager::CarrierSnapshot,
    snapshot: &'a mut crate::session::manager::CarrierSnapshot,
    row: &'a crate::session::session::Session,
    canonical: bool,
    spelling: &'a (std::path::PathBuf, std::path::PathBuf, u64),
    invalid_paths: &'a [std::path::PathBuf],
    member: &'a crate::session::session::Session,
    coordinator: &'a crate::session::session::Session,
}

#[cfg(windows)]
fn assert_registration_row_case(case: RegistrationRowCase<'_>) {
    use crate::pty::backend::SessionBackendKind;
    use crate::pty::manager::ResponseRouteIdentity;
    let RegistrationRowCase {
        pty,
        current,
        snapshot,
        row,
        canonical,
        spelling,
        invalid_paths,
        member,
        coordinator,
    } = case;
    let (ordinary_path, canonical_path, final_generation) = spelling;
    let identity = ResponseRouteIdentity {
        target: String::new(),
        session_id: row.id,
        token: row.token,
        generation: *final_generation,
        carrier_version: current.versions[&row.id],
        backend: row.backend_kind,
    };
    snapshot
        .rows
        .iter_mut()
        .find(|actual| actual.id == row.id)
        .unwrap()
        .working_directory = if canonical {
        canonical_path
    } else {
        ordinary_path
    }
    .to_string_lossy()
    .into_owned();
    let snapshot = &*snapshot;
    assert_eq!(
        pty.lock()
            .unwrap()
            .response_close_state()
            .generation(row.id),
        Some(*final_generation)
    );
    let accepted = response_registration_row(snapshot, &identity).unwrap();
    assert!(response_registration_route(pty, &identity, accepted));
    for invalid in invalid_paths {
        let mut changed = snapshot.clone();
        changed
            .rows
            .iter_mut()
            .find(|actual| actual.id == row.id)
            .unwrap()
            .working_directory = invalid.to_string_lossy().into_owned();
        let accepted = response_registration_row(&changed, &identity).unwrap();
        assert!(!response_registration_route(pty, &identity, accepted));
    }
    let mut container = snapshot.clone();
    container
        .rows
        .iter_mut()
        .find(|actual| actual.id == row.id)
        .unwrap()
        .backend_kind = SessionBackendKind::ContainerTransport;
    let mut container_identity = identity.clone();
    container_identity.backend = SessionBackendKind::ContainerTransport;
    let accepted = response_registration_row(&container, &container_identity).unwrap();
    assert!(!response_registration_route(
        pty,
        &container_identity,
        accepted
    ));
    let mut wrong_token = identity.clone();
    wrong_token.token = if row.id == member.id {
        coordinator.token
    } else {
        member.token
    };
    assert!(response_registration_row(snapshot, &wrong_token).is_none());
    let mut wrong_version = identity.clone();
    wrong_version.carrier_version += 1;
    assert!(response_registration_row(snapshot, &wrong_version).is_none());
    let mut pending = snapshot.clone();
    pending.pending_ids.insert(row.id);
    assert!(response_registration_row(&pending, &identity).is_none());
    let mut exited = snapshot.clone();
    exited
        .rows
        .iter_mut()
        .find(|actual| actual.id == row.id)
        .unwrap()
        .status = SessionStatus::Exited(0);
    assert!(response_registration_row(&exited, &identity).is_none());
}

#[cfg(windows)]
async fn registration_case(
    configured: &std::path::Path,
    canonical_member: bool,
    canonical_coordinator: bool,
) {
    assert_eq!(crate::config::config_dir().unwrap(), configured);
    let temp = tempfile::TempDir::new().unwrap();
    let member_path = temp.path().join("member");
    let coordinator_path = temp.path().join("coordinator");
    std::fs::create_dir(&member_path).unwrap();
    std::fs::create_dir(&coordinator_path).unwrap();
    let manager = SessionManager::new();
    let member = create_registration_session(&manager, &member_path).await;
    let coordinator = create_registration_session(&manager, &coordinator_path).await;
    manager
        .rename_session(member.id, "member".to_string())
        .await
        .unwrap();
    manager
        .rename_session(coordinator.id, "coordinator".to_string())
        .await
        .unwrap();
    let backend = Arc::new(LifecycleTestBackend::default());
    let injected: Arc<dyn crate::pty::backend::PtyBackend> = backend.clone();
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(injected)));
    let mut spellings = Vec::new();
    for row in [&member, &coordinator] {
        spellings.push(respawn_registration_spellings(&pty, &backend, row).await);
    }
    let other = temp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let file = temp.path().join("file");
    std::fs::write(&file, b"not a directory").unwrap();
    let invalid_paths = [
        other.clone(),
        other.join("missing"),
        file,
        other.join("..").join("other"),
    ];
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            let mut snapshot = current.clone();
            for (index, (row, canonical)) in [
                (&member, canonical_member),
                (&coordinator, canonical_coordinator),
            ]
            .into_iter()
            .enumerate()
            {
                assert_registration_row_case(RegistrationRowCase {
                    pty: &pty,
                    current,
                    snapshot: &mut snapshot,
                    row,
                    canonical,
                    spelling: &spellings[index],
                    invalid_paths: &invalid_paths,
                    member: &member,
                    coordinator: &coordinator,
                });
            }
        })
        .await;
    drop(permit);
    for row in [&member, &coordinator] {
        remove_registration_route(&pty, &backend, row.id);
    }
}

#[cfg(windows)]
#[expect(
    clippy::mem_forget,
    reason = "Retain fixture directory while an unreaped child may still use it"
)]
#[tokio::test]
async fn response_registration_physical_cwd_boundary() {
    const CHILD_MARKER: &str = "AGENTSCOMMANDER_REGISTRATION_CWD_CHILD_CONFIG_DIR";
    let marker = std::env::var_os(CHILD_MARKER);
    if marker.is_none() {
        let temp = tempfile::tempdir().expect("parent-owned registration config directory");
        assert!(temp.path().is_absolute());
        let tmp = temp.path().join("tmp");
        std::fs::create_dir(&tmp).expect("isolated child temp directory");
        let stdout_path = temp.path().join("child-stdout.log");
        let stderr_path = temp.path().join("child-stderr.log");
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("compiled test executable"))
                .args([
                    "--exact",
                    "session::selection::tests::response_registration_physical_cwd_boundary",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("AGENTSCOMMANDER_CONFIG_DIR", temp.path())
                .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", temp.path())
                .env(CHILD_MARKER, temp.path())
                .env("TEMP", &tmp)
                .env("TMP", &tmp)
                .env("TMPDIR", &tmp)
                .stdin(std::process::Stdio::null())
                .stdout(std::fs::File::create(&stdout_path).expect("child stdout file"))
                .stderr(std::fs::File::create(&stderr_path).expect("child stderr file"))
                .spawn()
                .expect("spawn isolated registration child");
        let (result, reaped) = wait_registration_child(&mut child).await;
        let log_errors = replay_registration_child_logs(&stdout_path, &stderr_path);
        if !reaped {
            eprintln!(
                "unreaped registration child fixture retained: {}",
                temp.path().display()
            );
            std::mem::forget(temp);
        }
        assert!(
            matches!(&result, Ok(status) if status.success()) && log_errors.is_empty(),
            "isolated registration child failed: {result:?}; log errors: {log_errors:?}"
        );
        return;
    }
    let configured = assert_registration_child_environment(marker);
    for (canonical_member, canonical_coordinator) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        registration_case(&configured, canonical_member, canonical_coordinator).await;
    }
}
