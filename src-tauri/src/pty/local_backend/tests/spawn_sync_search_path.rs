use super::*;
use crate::config::agent_path::SearchPathOverrideGuard;
use crate::session::manager::SessionManager;
use std::os::unix::fs::PermissionsExt;

const PROBE: &str = "ac-2683-probe";

fn test_backend() -> LocalProcessBackend {
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let git_watcher = GitWatcher::new_headless_for_tests(session_mgr);
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    LocalProcessBackend::new(output_senders, idle_detector, git_watcher, None)
}

fn spawn_spec(cmd: &str, cwd: &std::path::Path) -> BackendSpawnSpec {
    BackendSpawnSpec {
        id: Uuid::new_v4(),
        agent_id: None,
        coding_agent: None,
        cmd: cmd.to_string(),
        args: Vec::new(),
        resolved_agent_host_shell: None,
        cwd: cwd.to_string_lossy().to_string(),
        selected_cwd: None,
        cols: 80,
        rows: 24,
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

struct LaunchCleanup<'a> {
    backend: &'a LocalProcessBackend,
    id: Uuid,
}
impl Drop for LaunchCleanup<'_> {
    fn drop(&mut self) {
        let result = self.backend.kill(self.id);
        if std::thread::panicking() {
            eprintln!("I3029 cleanup: {result:?}");
        } else {
            result.expect("cleanup own PTY");
        }
    }
}

fn await_launch(backend: &LocalProcessBackend, spec: BackendSpawnSpec, marker: &str) {
    let id = spec.id;
    backend.spawn_sync(spec).expect("Unix contract spawn");
    let _cleanup = LaunchCleanup { backend, id };
    let pid = backend.ptys.lock().unwrap()[&id]
        .child
        .as_ref()
        .unwrap()
        .process_id()
        .unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let painted = backend
            .fanout
            .get_screen_snapshot(id)
            .is_some_and(|s| String::from_utf8_lossy(&s.data).contains(marker));
        if painted
            && matches!(
                backend.probe_child(id),
                ChildLiveness::Exited {
                    code: 0,
                    success: true
                }
            )
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "marker {marker} / exit0 missing for PID {pid}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    backend.kill(id).expect("kill after exit");
    assert!(!backend.ptys.lock().unwrap().contains_key(&id));
    assert!(backend.fanout.registration_token_for_session(id).is_none());
    assert_eq!(
        unsafe { libc::kill(pid as libc::pid_t, 0) },
        -1,
        "own child leaked"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

fn script(path: &std::path::Path, text: &str, executable: bool) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
    )
    .unwrap();
}

#[test]
fn issue_3029_unix_launch_contracts() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("cwd");
    let bin = cwd.join("bin");
    let child = cwd.join("nested");
    let decoy = root.path().join("ambient");
    for dir in [&bin, &child, &decoy] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let probe = "ac3029-probe";
    script(&decoy.join(probe), "#!/bin/sh\necho WRONG_AMBIENT\n", true);
    let _search_path = SearchPathOverrideGuard::new(decoy.as_os_str());
    let backend = test_backend();
    for (index, program, working_dir, path) in [
        (0, "./bin/ac3029-probe", &cwd, "/usr/bin:/bin"),
        (1, "../bin/ac3029-probe", &child, "/usr/bin:/bin"),
        (2, "ac3029-probe", &cwd, "bin"),
    ] {
        let marker = format!("AC3029_RELATIVE_{index}");
        script(
            &bin.join(probe),
            &format!("#!/bin/sh\necho {marker}\n"),
            true,
        );
        let mut spec = spawn_spec(program, working_dir);
        spec.configured_env.push(("PATH".into(), path.into()));
        await_launch(&backend, spec, &marker);
    }
    let mut shell = spawn_spec("/bin/sh", &cwd);
    shell.args = vec!["-c".into(), "printf '%s' \"$SHELL\"".into()];
    shell
        .configured_env
        .push(("SHELL".into(), "AC3029_SHELL_SENTINEL".into()));
    await_launch(&backend, shell, "AC3029_SHELL_SENTINEL");
    script(
        &bin.join("not-executable"),
        "#!/bin/sh\necho UNEXPECTED\n",
        false,
    );
    for target in ["./bin", "./bin/not-executable"] {
        let spec = spawn_spec(target, &cwd);
        let id = spec.id;
        let witness = spec.launch_witness.clone();
        let error = backend
            .spawn_sync(spec)
            .expect_err("invalid executable must fail");
        assert!(matches!(error, AppError::PtyError(_)), "{error}");
        assert!(!error.to_string().is_empty());
        assert!(!witness.launched());
        assert!(!backend.ptys.lock().unwrap().contains_key(&id));
        assert!(backend.fanout.registration_token_for_session(id).is_none());
    }
}

struct SignalMaskGuard(libc::sigset_t);
impl SignalMaskGuard {
    fn block_usr1() -> Self {
        unsafe {
            let mut block = std::mem::zeroed();
            let mut original = std::mem::zeroed();
            assert_eq!(libc::sigemptyset(&mut block), 0);
            assert_eq!(libc::sigaddset(&mut block, libc::SIGUSR1), 0);
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut original),
                0
            );
            Self(original)
        }
    }
}
impl Drop for SignalMaskGuard {
    fn drop(&mut self) {
        assert_eq!(
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut()) },
            0
        );
    }
}

#[test]
fn pty_3029_child_fixture() {
    if std::env::var("AC3029_CHILD_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let blocked = unsafe {
        let mut current = std::mem::zeroed();
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut current),
            0
        );
        libc::sigismember(&current, libc::SIGUSR1)
    };
    assert_eq!(blocked, 0);
    println!(
        "AC3029_SHELL={} SIGUSR1_BLOCKED={blocked}",
        std::env::var("SHELL").unwrap()
    );
}

#[test]
fn issue_3029_unix_child_inherits_shell_and_unblocked_signals() {
    let cwd = tempfile::tempdir().unwrap();
    let backend = test_backend();
    let executable = std::env::current_exe().unwrap();
    let mut spec = spawn_spec(executable.to_str().unwrap(), cwd.path());
    spec.args = vec![
        "--exact".into(),
        "pty::local_backend::tests::spawn_sync_search_path::pty_3029_child_fixture".into(),
        "--nocapture".into(),
        "--test-threads=1".into(),
    ];
    spec.configured_env = vec![
        ("AC3029_CHILD_FIXTURE".into(), "1".into()),
        ("SHELL".into(), "AC3029_SIGNAL_SENTINEL".into()),
    ];
    let id = spec.id;
    let guard = SignalMaskGuard::block_usr1();
    let spawned = backend.spawn_sync(spec);
    drop(guard); // Restore the calling thread before assertions, even on launch failure.
    spawned.expect("spawn direct test child");
    let _cleanup = LaunchCleanup {
        backend: &backend,
        id,
    };
    let pid = backend.ptys.lock().unwrap()[&id]
        .child
        .as_ref()
        .unwrap()
        .process_id()
        .unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let painted = backend.fanout.get_screen_snapshot(id).is_some_and(|s| {
            String::from_utf8_lossy(&s.data)
                .contains("AC3029_SHELL=AC3029_SIGNAL_SENTINEL SIGUSR1_BLOCKED=0")
        });
        if painted
            && matches!(
                backend.probe_child(id),
                ChildLiveness::Exited {
                    code: 0,
                    success: true
                }
            )
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "direct fixture marker/exit missing"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    backend.kill(id).unwrap();
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

fn write_probe(dir: &std::path::Path) {
    let probe = dir.join(PROBE);
    std::fs::write(&probe, "#!/bin/sh\nexit 0\n").expect("write probe");
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).expect("chmod probe");
    let mode = std::fs::metadata(&probe)
        .expect("stat probe")
        .permissions()
        .mode();
    assert!(probe.is_file(), "probe must exist before spawning");
    assert_eq!(mode & 0o777, 0o755, "probe must be executable");
}

fn assert_ambient_path_cannot_resolve_probe() {
    let ambient = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&ambient) {
        assert!(
            !dir.join(PROBE).exists(),
            "ambient PATH entry {} resolves the probe; the test would be vacuous",
            dir.display()
        );
    }
}

#[test]
fn issue_2683_spawn_sync_resolves_a_program_only_in_the_injected_search_path() {
    let probe_dir = tempfile::tempdir().expect("probe dir");
    let cwd = tempfile::tempdir().expect("cwd");
    write_probe(probe_dir.path());
    assert_ambient_path_cannot_resolve_probe();
    let _guard = SearchPathOverrideGuard::new(probe_dir.path().as_os_str());

    let backend = test_backend();
    let spec = spawn_spec(PROBE, cwd.path());
    let id = spec.id;
    backend
        .spawn_sync(spec)
        .expect("the probe exists only in the injected search path and must spawn");
    assert!(backend.ptys.lock().unwrap().contains_key(&id));

    backend.kill(id).expect("kill spawned probe");
    assert!(!backend.ptys.lock().unwrap().contains_key(&id));
}

#[test]
fn issue_2683_spawn_sync_reproduces_the_vm200_failure_without_the_probe_dir() {
    let probe_dir = tempfile::tempdir().expect("probe dir");
    let empty_dir = tempfile::tempdir().expect("empty dir");
    let cwd = tempfile::tempdir().expect("cwd");
    write_probe(probe_dir.path());
    assert_ambient_path_cannot_resolve_probe();
    let _guard = SearchPathOverrideGuard::new(empty_dir.path().as_os_str());

    let backend = test_backend();
    let spec = spawn_spec(PROBE, cwd.path());
    let error = backend
        .spawn_sync(spec)
        .expect_err("the probe is absent from the injected search path");
    assert!(
        matches!(&error, AppError::PtyError(_))
            && error.to_string().contains(PROBE)
            && error.to_string().contains("PATH"),
        "unexpected error: {error}"
    );
    assert!(backend.ptys.lock().unwrap().is_empty());
}
