mod issue_3047_schema;

// Each logger scenario runs in its own process. No test installs or changes
// the logger of the parent harness or shares runtime filters with siblings.
const SETTINGS_LOCK_DEBUG_HARNESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

struct SettingsLockDebugChild {
    process: std::process::Child,
    input: std::process::ChildStdin,
    receipts: std::sync::mpsc::Receiver<serde_json::Value>,
    output_thread: Option<std::thread::JoinHandle<()>>,
    config: tempfile::TempDir,
}

impl SettingsLockDebugChild {
    fn start(
        mode: &str,
        path: &std::path::Path,
        rust_log: Option<&str>,
        level: Option<&str>,
    ) -> Self {
        use std::io::BufRead;
        let config = tempfile::tempdir().unwrap();
        if let Some(level) = level {
            std::fs::write(
                config.path().join(super::SETTINGS_FILE_NAME),
                serde_json::json!({"logLevel": level}).to_string(),
            )
            .unwrap();
        }
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "config::settings::tests::settings_lock_debug_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("AC_SETTINGS_LOCK_DEBUG_CHILD", mode)
            .env("AC_SETTINGS_LOCK_DEBUG_FIXTURE", path)
            .env("AGENTSCOMMANDER_CONFIG_DIR", config.path())
            .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", config.path())
            .env_remove("RUST_LOG")
            .env("RUST_LOG_STYLE", "never")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        if let Some(rust_log) = rust_log {
            command.env("RUST_LOG", rust_log);
        }
        let mut process = command.spawn().unwrap();
        let input = process.stdin.take().unwrap();
        let output = process.stdout.take().unwrap();
        let (send, receipts) = std::sync::mpsc::channel();
        let output_thread = std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                let line = line.unwrap();
                // libtest may prefix the first line with its test label.
                if let Some((_, json)) = line.split_once("LOCK-RECEIPT ") {
                    send.send(serde_json::from_str(json).unwrap()).unwrap();
                }
            }
        });
        Self {
            process,
            input,
            receipts,
            output_thread: Some(output_thread),
            config,
        }
    }

    fn receipt(&self, phase: &str) -> serde_json::Value {
        let receipt = self
            .receipts
            .recv_timeout(SETTINGS_LOCK_DEBUG_HARNESS_TIMEOUT)
            .unwrap_or_else(|error| panic!("missing {phase} receipt: {error}"));
        assert_eq!(receipt["phase"], phase, "unexpected receipt: {receipt}");
        receipt
    }

    fn command(&mut self, command: &str) {
        use std::io::Write;
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
    }

    fn finish(mut self) {
        self.receipt("done");
        let deadline = std::time::Instant::now() + SETTINGS_LOCK_DEBUG_HARNESS_TIMEOUT;
        let status = loop {
            if let Some(status) = self.process.try_wait().unwrap() {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child did not exit after done"
            );
            // Poll only process termination; event ordering uses receipts.
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        self.output_thread.take().unwrap().join().unwrap();
        assert!(status.success());
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.config.path().join("app.log")).unwrap()
    }
}

impl Drop for SettingsLockDebugChild {
    fn drop(&mut self) {
        // Also reap a child after any harness assertion, never strand owners.
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn settings_lock_debug_receipt(value: &serde_json::Value) {
    use std::io::Write;
    println!("LOCK-RECEIPT {value}");
    std::io::stdout().flush().unwrap();
}

fn settings_lock_debug_processed(
    receiver: &std::sync::mpsc::Receiver<serde_json::Value>,
    phase: &str,
) -> serde_json::Value {
    let event = receiver
        .recv_timeout(SETTINGS_LOCK_DEBUG_HARNESS_TIMEOUT)
        .unwrap();
    assert_eq!(event["phase"], phase);
    for field in ["preparation_ms", "wait_ms", "hold_ms"] {
        if let Some(value) = event[field].as_f64() {
            assert!(value >= 0.0);
        }
    }
    event
}

fn settings_lock_debug_assert_pair(acquired: &serde_json::Value, release: &serde_json::Value) {
    for field in [
        "instance",
        "pid",
        "attempt",
        "operation",
        "target",
        "sidecar",
    ] {
        assert_eq!(acquired[field], release[field], "correlation field {field}");
    }
    assert!(release["hold_ms"].as_f64().unwrap() >= 0.0);
}

#[test]
fn settings_lock_debug_child() {
    let Ok(mode) = std::env::var("AC_SETTINGS_LOCK_DEBUG_CHILD") else {
        return;
    };
    let path =
        std::path::PathBuf::from(std::env::var_os("AC_SETTINGS_LOCK_DEBUG_FIXTURE").unwrap());
    let (processed_send, processed) = std::sync::mpsc::channel();
    let (gated_send, gated) = std::sync::mpsc::channel();
    let (resume, resume_recv) = std::sync::mpsc::channel();
    let hooks = super::SettingsLockDebugTestHooks {
        gate_phase: "acquired",
        gate_once: std::sync::atomic::AtomicBool::new(matches!(
            mode.as_str(),
            "suppress" | "saturated"
        )),
        gated: gated_send,
        resume: std::sync::Mutex::new(resume_recv),
        processed: processed_send,
    };
    assert!(super::SETTINGS_LOCK_DEBUG_TEST_HOOKS.set(hooks).is_ok());
    crate::logging::init_logger();
    let config =
        std::path::PathBuf::from(std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR").unwrap());
    let log = || std::fs::read_to_string(config.join("app.log")).unwrap();
    match mode.as_str() {
        "disabled" | "probe" => {
            let guard = super::SettingsFileLock::acquire(&path, std::time::Duration::ZERO)
                .unwrap_or_else(|_| panic!("probe lock unavailable"));
            drop(guard);
            assert!(super::SETTINGS_LOCK_DEBUG_TRANSPORT.get().is_none());
            assert!(processed.try_recv().is_err());
            assert!(!log().contains("settings-lock phase="));
            if mode == "probe" {
                settings_lock_debug_receipt(&serde_json::json!({"phase":"probe-ok"}));
            }
        }
        "waiter" => {
            let error =
                match super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(2)) {
                    Ok(_) => panic!("waiter unexpectedly acquired"),
                    Err(error) => error,
                };
            assert!(matches!(
                error.cause,
                super::SettingsSaveCause::Semantic(super::SettingsSaveReason::LockTimedOut)
            ));
            assert_eq!(
                error.legacy_outward.into_general_string(),
                super::SETTINGS_LOCK_UNAVAILABLE_CODE
            );
            settings_lock_debug_processed(&processed, "acquire-start");
            let timeout = settings_lock_debug_processed(&processed, "timeout");
            assert!(timeout["wait_ms"].as_f64().unwrap() >= 2000.0);
            assert!(processed.try_recv().is_err());
            settings_lock_debug_receipt(&timeout);
        }
        "owner" | "runtime" | "filter-on" => {
            settings_lock_debug_owner_scenario(&mode, &path, &processed, &log)
        }
        "suppress" => {
            settings_lock_debug_suppress_scenario(&path, &processed, &gated, &resume, &log)
        }
        "saturated" => {
            settings_lock_debug_saturated_scenario(&path, &processed, &gated, &resume, &log)
        }
        "failures" => settings_lock_debug_failure_scenarios(&path, &processed),
        _ => panic!("unknown child mode {mode}"),
    }
    settings_lock_debug_receipt(&serde_json::json!({"phase":"done"}));
}

fn settings_lock_debug_owner_scenario(
    mode: &str,
    path: &std::path::Path,
    processed: &std::sync::mpsc::Receiver<serde_json::Value>,
    log: &impl Fn() -> String,
) {
    if mode == "runtime" {
        let guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("initial INFO acquisition"));
        drop(guard);
        assert!(super::SETTINGS_LOCK_DEBUG_TRANSPORT.get().is_none());
        assert!(processed.try_recv().is_err());
        assert!(!log().contains("settings-lock phase="));
        assert!(crate::logging::set_runtime_log_level("debug"));
    }
    if std::env::var_os("RUST_LOG").is_some() {
        assert!(!crate::logging::set_runtime_log_level("error"));
    }
    let guard = super::empty_project_transaction(path, "capture_project_settings")
        .unwrap_or_else(|_| panic!("owner acquisition"));
    let start = settings_lock_debug_processed(processed, "acquire-start");
    let acquired = settings_lock_debug_processed(processed, "acquired");
    assert_eq!(start["attempt"], acquired["attempt"]);
    assert_eq!(acquired["operation"], "capture_project_settings");
    assert!(log().contains("settings-lock phase=acquired"));
    assert!(!log().contains("PRIVATE-SETTINGS-SENTINEL"));
    assert!(guard._lock.file.is_some());
    settings_lock_debug_receipt(&acquired);
    if mode != "filter-on" {
        settings_lock_debug_wait_command("release");
    }
    drop(guard);
    let release = settings_lock_debug_processed(processed, "release");
    settings_lock_debug_assert_pair(&acquired, &release);
    assert_eq!(release["reason"], "unlock_ok");
    assert!(log().contains("settings-lock phase=release"));
    settings_lock_debug_receipt(&release);
    if mode == "runtime" {
        let sender = super::SETTINGS_LOCK_DEBUG_TRANSPORT
            .get()
            .unwrap()
            .as_ref()
            .unwrap();
        assert!(crate::logging::set_runtime_log_level("info"));
        let guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("INFO re-acquisition"));
        assert!(guard.debug.is_none());
        drop(guard);
        assert!(crate::logging::set_runtime_log_level("debug"));
        let guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("DEBUG re-acquisition"));
        // The same OnceLock sender/worker serves the next acquisition.
        assert!(std::ptr::eq(
            sender,
            super::SETTINGS_LOCK_DEBUG_TRANSPORT
                .get()
                .unwrap()
                .as_ref()
                .unwrap()
        ));
        settings_lock_debug_processed(processed, "acquire-start");
        settings_lock_debug_processed(processed, "acquired");
        drop(guard);
        settings_lock_debug_processed(processed, "release");
    }
}

fn settings_lock_debug_suppress_scenario(
    path: &std::path::Path,
    processed: &std::sync::mpsc::Receiver<serde_json::Value>,
    gated: &std::sync::mpsc::Receiver<()>,
    resume: &std::sync::mpsc::Sender<()>,
    log: &impl Fn() -> String,
) {
    let guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
        .unwrap_or_else(|_| panic!("suppress owner"));
    settings_lock_debug_processed(processed, "acquire-start");
    gated
        .recv_timeout(SETTINGS_LOCK_DEBUG_HARNESS_TIMEOUT)
        .unwrap();
    assert!(crate::logging::set_runtime_log_level("info"));
    resume.send(()).unwrap();
    settings_lock_debug_processed(processed, "acquired");
    drop(guard);
    settings_lock_debug_processed(processed, "release");
    let logs = log();
    assert!(logs.contains("settings-lock phase=acquire-start"));
    assert!(!logs.contains("settings-lock phase=acquired"));
    assert!(!logs.contains("settings-lock phase=release"));
}

fn settings_lock_debug_saturated_scenario(
    path: &std::path::Path,
    processed: &std::sync::mpsc::Receiver<serde_json::Value>,
    gated: &std::sync::mpsc::Receiver<()>,
    resume: &std::sync::mpsc::Sender<()>,
    log: &impl Fn() -> String,
) {
    let guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
        .unwrap_or_else(|_| panic!("saturation owner"));
    settings_lock_debug_processed(processed, "acquire-start");
    gated
        .recv_timeout(SETTINGS_LOCK_DEBUG_HARNESS_TIMEOUT)
        .unwrap();
    let debug = guard.debug.as_ref().unwrap();
    for _ in 0..super::SETTINGS_LOCK_DEBUG_CAPACITY + 10 {
        debug.event("acquire-start", None, None);
    }
    assert_eq!(
        super::SETTINGS_LOCK_DEBUG_LOST.load(super::Ordering::Relaxed),
        10
    );
    drop(guard);
    assert_eq!(
        super::SETTINGS_LOCK_DEBUG_LOST.load(super::Ordering::Relaxed),
        11
    );
    settings_lock_debug_receipt(&serde_json::json!({"phase":"closed-sink-blocked"}));
    settings_lock_debug_wait_command("resume");
    resume.send(()).unwrap();
    let acquired = settings_lock_debug_processed(processed, "acquired");
    assert_eq!(acquired["lost_events"], 11);
    for _ in 0..super::SETTINGS_LOCK_DEBUG_CAPACITY {
        settings_lock_debug_processed(processed, "acquire-start");
    }
    assert!(log().contains("lost_events=11"));
}

fn settings_lock_debug_wait_command(expected: &str) {
    use std::io::BufRead;
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).unwrap();
        let _ = send.send(line);
    });
    assert_eq!(
        receive
            .recv_timeout(SETTINGS_LOCK_DEBUG_HARNESS_TIMEOUT)
            .unwrap()
            .trim(),
        expected
    );
}

fn settings_lock_debug_failure_scenarios(
    path: &std::path::Path,
    processed: &std::sync::mpsc::Receiver<serde_json::Value>,
) {
    // I/O failure during preparation has no acquired, and preserves outward.
    let bad_parent = path.with_file_name("not-a-directory");
    std::fs::write(&bad_parent, b"PRIVATE-SETTINGS-SENTINEL").unwrap();
    let error = match super::SettingsFileLock::acquire(
        &bad_parent.join("settings.json"),
        std::time::Duration::ZERO,
    ) {
        Ok(_) => panic!("invalid parent accepted"),
        Err(error) => error,
    };
    assert_eq!(
        error.legacy_outward.into_general_string(),
        super::SETTINGS_LOCK_UNAVAILABLE_CODE
    );
    settings_lock_debug_processed(processed, "acquire-start");
    let failure = settings_lock_debug_processed(processed, "acquire-error");
    assert_eq!(failure["reason"], "io_error");
    assert!(failure["wait_ms"].is_null());
    // An invalid sidecar is rejected with no acquired event.
    let directory = path.with_file_name("invalid-sidecar");
    std::fs::create_dir_all(directory.join(super::SETTINGS_LOCK_FILE_NAME)).unwrap();
    assert!(super::SettingsFileLock::acquire(
        &directory.join("settings.json"),
        std::time::Duration::ZERO
    )
    .is_err());
    settings_lock_debug_processed(processed, "acquire-start");
    settings_lock_debug_processed(processed, "acquire-error");
    std::fs::remove_dir_all(&directory).unwrap();
    // Deadline overflow happens after opening. Windows rename additionally
    // proves the no-share-delete handle was closed before the failure receipt.
    assert!(super::SettingsFileLock::acquire(path, std::time::Duration::MAX).is_err());
    settings_lock_debug_processed(processed, "acquire-start");
    let failure = settings_lock_debug_processed(processed, "acquire-error");
    assert_eq!(failure["reason"], "lock_deadline_overflow");
    let sidecar = path.parent().unwrap().join(super::SETTINGS_LOCK_FILE_NAME);
    let renamed = sidecar.with_extension("test-closed");
    std::fs::rename(&sidecar, &renamed).unwrap();
    std::fs::rename(&renamed, &sidecar).unwrap();
    // Early return and unwinding both release before an immediate new owner.
    let early = || -> Result<(), ()> {
        let _guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("early owner"));
        Err(())
    };
    assert!(early().is_err());
    settings_lock_debug_processed(processed, "acquire-start");
    let acquired = settings_lock_debug_processed(processed, "acquired");
    let release = settings_lock_debug_processed(processed, "release");
    settings_lock_debug_assert_pair(&acquired, &release);
    assert!(std::panic::catch_unwind(|| {
        let _guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("unwind owner"));
        panic!("test unwind");
    })
    .is_err());
    settings_lock_debug_processed(processed, "acquire-start");
    let acquired = settings_lock_debug_processed(processed, "acquired");
    let release = settings_lock_debug_processed(processed, "release");
    settings_lock_debug_assert_pair(&acquired, &release);
    let guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
        .unwrap_or_else(|_| panic!("unlock-error owner"));
    settings_lock_debug_processed(processed, "acquire-start");
    let acquired = settings_lock_debug_processed(processed, "acquired");
    super::SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR.with(|flag| flag.set(true));
    drop(guard);
    let release = settings_lock_debug_processed(processed, "release");
    settings_lock_debug_assert_pair(&acquired, &release);
    assert_eq!(release["reason"], "unlock_error");
    assert_eq!(release["raw_os_error"], 5);
    let guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
        .unwrap_or_else(|_| panic!("closed failed unlock handle"));
    settings_lock_debug_processed(processed, "acquire-start");
    let acquired = settings_lock_debug_processed(processed, "acquired");
    // Moving the guard preserves the original acquisition thread/context.
    let dropper = std::thread::spawn(move || drop(guard));
    dropper.join().unwrap();
    let release = settings_lock_debug_processed(processed, "release");
    settings_lock_debug_assert_pair(&acquired, &release);
    assert_eq!(
        release["acquisition_thread"],
        acquired["acquisition_thread"]
    );
    assert_ne!(release["event_thread"], acquired["event_thread"]);
    // The wrapper propagates each operation and the caller location.
    for operation in ["capture_project_settings", "read_project_paths_from_disk"] {
        let origin_line = line!() + 1;
        let transaction = super::empty_project_transaction(path, operation)
            .unwrap_or_else(|_| panic!("wrapper owner"));
        assert_eq!(
            transaction
                ._lock
                .debug
                .as_ref()
                .unwrap()
                .context
                .caller
                .line(),
            origin_line
        );
        settings_lock_debug_processed(processed, "acquire-start");
        let acquired = settings_lock_debug_processed(processed, "acquired");
        assert_eq!(acquired["operation"], operation);
        drop(transaction);
        settings_lock_debug_processed(processed, "release");
    }
    super::SETTINGS_LOCK_DEBUG_ATTEMPT.store(u64::MAX, super::Ordering::Relaxed);
    let guard = super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
        .unwrap_or_else(|_| panic!("counter overflow changed lock result"));
    assert!(guard.debug.is_none());
    drop(guard);
    assert!(processed.try_recv().is_err());
}

#[test]
fn settings_lock_debug_active_owner_and_runtime_info_to_debug() {
    for mode in ["owner", "runtime"] {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("settings.json");
        std::fs::write(&path, "PRIVATE-SETTINGS-SENTINEL").unwrap();
        let mut owner = SettingsLockDebugChild::start(
            mode,
            &path,
            (mode == "owner").then_some("debug"),
            Some("info"),
        );
        let acquired = owner.receipt("acquired");
        assert_eq!(acquired["pid"], owner.process.id());
        assert!(uuid::Uuid::parse_str(acquired["instance"].as_str().unwrap()).is_ok());
        assert_eq!(
            acquired["sidecar"],
            super::settings_lock_debug_path(&fixture.path().join(super::SETTINGS_LOCK_FILE_NAME)).0
        );
        assert!(owner.log().contains("settings-lock phase=acquired"));
        // A different target in the same parent must still contend on sidecar.
        let waiter = SettingsLockDebugChild::start(
            "waiter",
            &fixture.path().join("other-settings.json"),
            Some("debug"),
            None,
        );
        let timeout = waiter.receipt("timeout");
        assert_eq!(timeout["sidecar"], acquired["sidecar"]);
        assert_ne!(timeout["instance"], acquired["instance"]);
        assert_ne!(timeout["pid"], acquired["pid"]);
        waiter.finish();
        // Release is authorized only after the active owner's real log receipt.
        owner.command("release");
        let release = owner.receipt("release");
        settings_lock_debug_assert_pair(&acquired, &release);
        owner.finish();
        let probe = SettingsLockDebugChild::start("probe", &path, Some("info"), None);
        probe.receipt("probe-ok");
        probe.finish();
    }
}

#[test]
fn settings_lock_debug_filters_and_pending_suppression() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("settings.json");
    for (rust_log, level) in [
        (Some("info"), Some("debug")),
        (Some("warn"), Some("debug")),
        (Some("error"), Some("debug")),
        (None, Some("info")),
        (None, Some("warn")),
        (None, Some("error")),
        (None, None),
    ] {
        SettingsLockDebugChild::start("disabled", &path, rust_log, level).finish();
    }
    for (rust_log, level) in [(Some("debug"), Some("error")), (None, Some("debug"))] {
        let child = SettingsLockDebugChild::start("filter-on", &path, rust_log, level);
        child.receipt("acquired");
        child.receipt("release");
        child.finish();
    }
    SettingsLockDebugChild::start("suppress", &path, None, Some("debug")).finish();
}

#[test]
fn settings_lock_debug_stalled_sink_does_not_retain_kernel_lock() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("settings.json");
    let mut owner = SettingsLockDebugChild::start("saturated", &path, Some("debug"), None);
    owner.receipt("closed-sink-blocked");
    let probe = SettingsLockDebugChild::start("probe", &path, Some("info"), None);
    probe.receipt("probe-ok");
    probe.finish();
    owner.command("resume");
    owner.finish();
}

#[test]
fn settings_lock_debug_failure_and_raii_events() {
    let fixture = tempfile::tempdir().unwrap();
    SettingsLockDebugChild::start(
        "failures",
        &fixture.path().join("settings.json"),
        Some("debug"),
        None,
    )
    .finish();
}

#[test]
fn settings_lock_debug_disabled_spawn_failure_and_disconnected_transport() {
    let slot = std::sync::OnceLock::new();
    assert!(
        super::settings_lock_debug_transport(&slot, false, |_| panic!("disabled spawn")).is_none()
    );
    assert!(slot.get().is_none());
    assert!(
        super::settings_lock_debug_transport(&slot, true, |_| Err(std::io::Error::other(
            "injected spawn failure"
        )))
        .is_none()
    );
    assert!(matches!(slot.get(), Some(None)));
    assert!(super::settings_lock_debug_transport(&slot, true, |_| panic!("spawn retry")).is_none());
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    drop(receiver);
    let context = std::sync::Arc::new(super::SettingsLockDebugContext {
        instance: uuid::Uuid::new_v4(),
        pid: std::process::id(),
        attempt: 1,
        operation: "test-lock",
        caller: std::panic::Location::caller(),
        thread: std::thread::current().id(),
        target: "test".into(),
        sidecar: "test".into(),
        path_truncated: false,
    });
    let debug = super::SettingsLockDebugAttempt {
        context,
        sender,
        started: std::time::Instant::now(),
        retry_started: None,
        acquired_at: None,
    };
    let before = super::SETTINGS_LOCK_DEBUG_LOST.load(super::Ordering::Relaxed);
    debug.event("acquire-start", None, None);
    assert!(super::SETTINGS_LOCK_DEBUG_LOST.load(super::Ordering::Relaxed) > before);
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("settings.json");
    let mut debug = Some(debug);
    let file = super::SettingsFileLock::acquire_file(
        &path,
        super::SETTINGS_LOCK_FILE_NAME,
        std::time::Duration::ZERO,
        &mut debug,
    )
    .unwrap_or_else(|_| panic!("disconnected diagnostics affected acquisition"));
    drop(super::SettingsFileLock {
        file: Some(file),
        debug,
    });
    assert!(super::SettingsFileLock::acquire(&path, std::time::Duration::ZERO).is_ok());
}

#[test]
fn settings_lock_debug_paths_are_escaped_and_bounded() {
    let (escaped, truncated) =
        super::settings_lock_debug_path(std::path::Path::new("hello\n\r\t\"\\world"));
    assert!(!truncated);
    assert!(!escaped.contains('\n'));
    assert!(escaped.contains("\\n"));
    assert_eq!(
        serde_json::from_str::<String>(&escaped).unwrap(),
        "hello\n\r\t\"\\world"
    );
    let (escaped, truncated) =
        super::settings_lock_debug_path(std::path::Path::new(&"🦀".repeat(4096)));
    assert!(truncated);
    assert!(escaped.len() <= super::SETTINGS_LOCK_DEBUG_PATH_BYTES);
    assert!(serde_json::from_str::<String>(&escaped).is_ok());
}

#[test]
fn id_hash_debug_exclusions_preserve_real_content_and_clear_orphans() {
    let agent: super::AgentConfig = serde_json::from_value(serde_json::json!({
        "id":"alpha", "label":"Alpha", "baseCommand":"tool", "color":"red"
    }))
    .unwrap();
    let mut supplied = agent.clone();
    supplied.id_hash_debug = Some(vec!["forged".into()]);
    assert!(super::known_agent_field("id_hash_debug"));
    assert!(!super::known_agent_field("idHashDebug"));
    assert_eq!(
        super::comparable_agent(&agent),
        super::comparable_agent(&supplied)
    );
    supplied.command.push_str(" --real");
    assert_ne!(
        super::comparable_agent(&agent),
        super::comparable_agent(&supplied)
    );

    let mut profiles = super::CodingAgentProfilesConfig::default();
    let mut cell = super::empty_profile_cell();
    cell.id_hash = Some("stale".into());
    cell.id_hash_debug = Some(vec!["stale".into()]);
    profiles
        .profiles_by_agent
        .insert("orphan".into(), BTreeMap::from([("A".into(), cell)]));
    assert!(super::is_profile_gap(&profiles, "orphan"));
    profiles
        .profiles_by_agent
        .get_mut("orphan")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "real".into();
    assert!(!super::is_profile_gap(&profiles, "orphan"));
    super::refresh_pair_id_hashes(&[agent], &mut profiles);
    let orphan = &profiles.profiles_by_agent["orphan"]["A"];
    assert_eq!(orphan.id_hash, None);
    assert_eq!(orphan.id_hash_debug, None);
    assert_eq!(orphan.notes, "real");
}

#[test]
fn id_hash_debug_schema_tolerates_metadata_and_preserves_real_errors() {
    let agent = serde_json::json!({
        "id": "custom-tool", "label": "Custom", "baseCommand": "tool", "color": "red"
    });
    let cell = serde_json::json!({"additionalArguments": "--flag"});
    for (is_agent, baseline) in [(true, agent), (false, cell)] {
        let decode = |raw: serde_json::Value| {
            if is_agent {
                serde_json::from_value::<super::AgentConfig>(raw).map(|value| {
                    (
                        value.id_hash_debug.clone(),
                        serde_json::to_value(value).unwrap(),
                    )
                })
            } else {
                serde_json::from_value::<super::ProfileCellConfig>(raw).map(|value| {
                    (
                        value.id_hash_debug.clone(),
                        serde_json::to_value(value).unwrap(),
                    )
                })
            }
        };
        let (debug, expected) = decode(baseline.clone()).unwrap();
        assert_eq!(debug, None);
        assert!(expected.get("id_hash_debug").is_none());
        for raw in [
            serde_json::json!(null),
            serde_json::json!(42),
            serde_json::json!(true),
            serde_json::json!("wrong"),
            serde_json::json!({}),
            serde_json::json!(["valid", 42]),
            serde_json::json!([null]),
            serde_json::json!([[]]),
        ] {
            let mut supplied = baseline.clone();
            supplied["id_hash_debug"] = raw;
            assert_eq!(decode(supplied).unwrap(), (None, expected.clone()));
        }
        let mut camel = baseline.clone();
        camel["idHashDebug"] = serde_json::json!(["ignored"]);
        assert_eq!(decode(camel).unwrap(), (None, expected.clone()));
        for leaves in [vec![], vec![""], vec![" Tool Ω ", "--Flag", "", "Tool"]] {
            let mut supplied = baseline.clone();
            supplied["id_hash_debug"] = serde_json::json!(leaves);
            let expected_leaves: Vec<String> = leaves.iter().map(|value| (*value).into()).collect();
            let mut expected_record = expected.clone();
            expected_record["id_hash_debug"] = serde_json::json!(leaves);
            let (debug, encoded) = decode(supplied).unwrap();
            assert_eq!(debug, Some(expected_leaves));
            assert_eq!(encoded, expected_record);
            assert!(encoded.get("idHashDebug").is_none());
            assert_eq!(decode(encoded.clone()).unwrap().1, encoded);
        }
        let command_key = if is_agent {
            "baseCommand"
        } else {
            "additionalArguments"
        };
        let env_key = if is_agent { "envs" } else { "env" };
        for key in [command_key, env_key] {
            let mut invalid = baseline.clone();
            invalid[key] = serde_json::json!(42);
            invalid["id_hash_debug"] = serde_json::json!(["ok", false]);
            assert!(decode(invalid).is_err(), "real field {key}");
        }
    }
}

#[test]
fn id_hash_schema_missing_null_and_nonstring_preserve_valid_settings() {
    let baseline = serde_json::json!({
        "defaultShell": "test-shell", "defaultShellArgs": [],
        "agents": [{
            "id": "custom-tool", "label": "Custom tool", "command": "tool --flag",
            "color": "#123456", "envs": [{"key": "MODE", "value": "safe", "enabled": true}]
        }]
    });
    let expected: super::AppSettings = serde_json::from_value(baseline.clone()).unwrap();
    assert_eq!(expected.agents[0].id_hash, None);
    for raw in [
        serde_json::json!(null),
        serde_json::json!(42),
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!(true),
        serde_json::json!(false),
        serde_json::json!([]),
        serde_json::json!(["hash"]),
        serde_json::json!({}),
        serde_json::json!({"hash": "value"}),
    ] {
        let mut supplied = baseline.clone();
        supplied["agents"][0]["id_hash"] = raw.clone();
        let decoded: super::AppSettings = serde_json::from_value(supplied).unwrap();
        assert_eq!(decoded.agents[0].id_hash, None, "metadata {raw}");
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(&expected).unwrap(),
            "metadata {raw} must not change valid settings"
        );
    }

    for malformed_agents in [
        serde_json::json!({}),
        serde_json::json!([null]),
        serde_json::json!([{"id": "a", "label": "A", "command": 42, "color": "red", "id_hash": false}]),
        serde_json::json!([{"id": "a", "label": "A", "command": "tool", "color": "red", "envs": {}, "id_hash": false}]),
    ] {
        let mut malformed = baseline.clone();
        malformed["agents"] = malformed_agents;
        assert!(serde_json::from_value::<super::AppSettings>(malformed).is_err());
    }
}

#[test]
fn id_hash_schema_serializes_literal_key_roundtrips_and_omits_none() {
    let mut agent: super::AgentConfig = serde_json::from_value(serde_json::json!({
        "id": "custom-tool", "label": "Custom tool", "command": "tool", "color": "red"
    }))
    .unwrap();
    agent.id_hash = Some("cid1:example".to_string());
    let encoded = serde_json::to_value(&agent).unwrap();
    assert_eq!(encoded["id_hash"], "cid1:example");
    assert!(encoded.get("idHash").is_none());
    let decoded: super::AgentConfig = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded.id, "custom-tool");
    assert_eq!(decoded.id_hash.as_deref(), Some("cid1:example"));
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);

    agent.id_hash = None;
    let encoded = serde_json::to_value(agent).unwrap();
    assert!(encoded.get("id_hash").is_none());
    assert!(encoded.get("idHash").is_none());
}

#[test]
fn id_hash_schema_ignores_camelcase_and_preserves_supplied_strings() {
    let baseline = serde_json::json!({
        "id": "custom-tool", "label": "Custom tool", "command": "tool", "color": "red",
        "idHash": "ignored"
    });
    let decoded: super::AgentConfig = serde_json::from_value(baseline.clone()).unwrap();
    assert_eq!(decoded.id_hash, None);
    assert!(serde_json::to_value(decoded)
        .unwrap()
        .get("idHash")
        .is_none());
    for raw in ["", "not-a-hash", "cid1:forged", " arbitrary Ω "] {
        let mut supplied = baseline.clone();
        supplied["id_hash"] = serde_json::json!(raw);
        let decoded: super::AgentConfig = serde_json::from_value(supplied).unwrap();
        assert_eq!(decoded.id, "custom-tool");
        assert_eq!(decoded.id_hash.as_deref(), Some(raw));
        let encoded = serde_json::to_value(decoded).unwrap();
        assert_eq!(encoded["id_hash"], raw);
        assert!(encoded.get("idHash").is_none());
    }
}

#[test]
fn p03_readers_select_sidecar_without_migration_and_preserve_unrelated_on_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let b = real_ac_project_path(temp.path(), "B");
    let legacy = serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"projectPaths":[a],"projectPath":a});
    std::fs::write(&path, legacy.to_string()).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert_eq!(
        super::load_settings_for_cli_strict_from_path(Some(&path))
            .unwrap()
            .project_paths,
        vec![a.clone()]
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!super::project_paths_path(&path).exists());
    std::fs::write(
        super::project_paths_path(&path),
        serde_json::json!({"projectPaths":[b]}).to_string(),
    )
    .unwrap();
    assert!(super::load_settings_for_cli_strict_from_path(Some(&path)).is_err());
    let failed = super::load_settings_for_cli_from_path(&path);
    assert_eq!(failed.default_shell, "sentinel");
    assert!(failed.project_paths.is_empty());
    assert!(failed.project_paths_persistence_error.is_some());
    let app = super::load_settings_from_path(&path);
    assert!(app.project_paths_persistence_error.is_some());
    assert!(app.root_token.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::write(
        &path,
        r#"{"defaultShell":"sentinel","defaultShellArgs":[],"agents":[]}"#,
    )
    .unwrap();
    assert_eq!(
        super::load_settings_for_cli_strict_from_path(Some(&path))
            .unwrap()
            .project_paths,
        vec![b]
    );
    std::fs::write(super::project_paths_path(&path), b"{}").unwrap();
    let mut stale = failed;
    stale.project_paths = vec![a];
    super::refresh_and_decode_project_paths_from_path(&mut stale, &path).unwrap();
    assert!(stale.project_paths.is_empty());
    assert!(stale.project_paths_persistence_error.is_none());
    std::fs::remove_file(&path).unwrap();
    assert!(super::load_settings_for_cli_strict_from_path(Some(&path))
        .unwrap()
        .project_paths
        .is_empty());
    assert!(!path.exists());
}

#[test]
fn p03_preserve_and_cas_strip_six_keys_and_do_not_write_project_drafts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let raw = serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"projectPaths":[a],"projectPathsRelativeToInstance":[null],"projectPath":a,"projectPathRelativeToInstance":null,"archivedProjectPaths":[],"archivedProjectPathsRelativeToInstance":[],"defaultShell":"sentinel"});
    std::fs::write(&path, raw.to_string()).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut settings = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    settings.project_paths = vec!["renderer draft".into()];
    let written = super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(written.project_paths, vec![a]);
    let destination = super::project_paths_path(&path);
    let sidecar = std::fs::read(&destination).unwrap();
    assert_eq!(
        std::fs::read(
            path.with_file_name(super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME)
        )
        .unwrap(),
        original
    );
    let managed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(super::raw_project_projection(managed.as_object().unwrap()).is_empty());
    let cas =
        super::compare_and_set_terminal_snapshots_enabled_at_path(&written, &path, true, false)
            .unwrap();
    assert!(!cas.terminal_snapshots_enabled);
    assert_eq!(std::fs::read(&destination).unwrap(), sidecar);
    let security = super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
    assert!(!security.terminal_snapshots_enabled);
    assert_eq!(security.project_paths, cas.project_paths);
    let managed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(super::raw_project_projection(managed.as_object().unwrap()).is_empty());
    std::fs::write(&destination, r#"{"projectPaths":"wrong","unknown":"keep"}"#).unwrap();
    let sidecar = std::fs::read(&destination).unwrap();
    super::save_settings_to_path_preserving_project_paths(&cas, &path).unwrap();
    assert_eq!(std::fs::read(destination).unwrap(), sidecar);
}

#[test]
fn p03_privileged_source_choice_and_disclosure_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = "C:/authorized-A";
    let b = "C:/legacy-B";
    std::fs::write(&path, r#"{"terminalSnapshotsEnabled":false}"#).unwrap();
    let destination = super::project_paths_path(&path);
    std::fs::write(
        &destination,
        serde_json::json!({"projectPaths":[a]}).to_string(),
    )
    .unwrap();
    assert_eq!(
        super::read_pty_input_project_paths_strict_from_path(&path).unwrap(),
        Some(vec![a.into()])
    );
    let security = super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
    assert!(!security.terminal_snapshots_enabled);
    assert_eq!(security.project_paths, vec![a]);
    std::fs::write(
        &path,
        serde_json::json!({"terminalSnapshotsEnabled":true,"projectPaths":[b]}).to_string(),
    )
    .unwrap();
    assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());
    assert!(super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err());
    std::fs::write(&path, serde_json::json!({"projectPaths":[a]}).to_string()).unwrap();
    assert_eq!(
        super::read_terminal_snapshot_security_settings_strict_from_path(&path)
            .unwrap()
            .project_paths,
        vec![a]
    );
    std::fs::write(&destination, b"{\"projectPaths\":[],\"projectPaths\":[]}").unwrap();
    assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());
    std::fs::write(&path, b"{}").unwrap();
    std::fs::write(&destination, b"{\"projectPaths\":null}").unwrap();
    assert_eq!(
        super::read_pty_input_project_paths_strict_from_path(&path).unwrap(),
        Some(vec![])
    );
    assert!(super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err());
}

#[test]
fn p03_loader_reader_first_holds_lock_until_capture_and_decode() {
    use std::sync::mpsc;
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    std::fs::write(
            &path,
            serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"projectPaths":[a]}).to_string(),
        )
        .unwrap();
    let (captured_tx, captured_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let reader_path = path.clone();
    let reader = std::thread::spawn(move || {
        super::PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| {
                captured_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }))
        });
        let result = super::load_settings_for_cli_strict_from_path(Some(&reader_path));
        super::PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| *hook.borrow_mut() = None);
        result.unwrap()
    });
    captured_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (owned_tx, owned_rx) = mpsc::channel();
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        let token = super::empty_project_transaction(&writer_path, "test-lock").unwrap();
        owned_tx.send(()).unwrap();
        super::ensure_project_paths_migrated_locked(&token).unwrap();
    });
    attempt_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(owned_rx.recv_timeout(Duration::from_millis(100)).is_err());
    assert!(!super::project_paths_path(&path).exists());
    release_tx.send(()).unwrap();
    let settings = reader.join().unwrap();
    assert_eq!(settings.project_paths, vec![a.clone()]);
    owned_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    writer.join().unwrap();
    assert_eq!(
        super::load_settings_for_cli_strict_from_path(Some(&path))
            .unwrap()
            .project_paths,
        vec![a]
    );
}

#[test]
fn p03_loader_migrator_first_timeout_never_authorizes_and_retry_recovers() {
    use std::sync::mpsc;
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    std::fs::write(
            &path,
            serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"projectPaths":[a]}).to_string(),
        )
        .unwrap();
    let (published_tx, published_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        let token = super::empty_project_transaction(&writer_path, "test-lock").unwrap();
        super::ensure_project_paths_migrated_with(&token, &mut |stage, _| {
            if stage == "destination_published" {
                published_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            Ok(())
        })
        .unwrap();
    });
    published_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(super::load_settings_for_cli_strict_from_path(Some(&path)).is_err());
    let failed = super::load_settings_for_cli_from_path(&path);
    assert!(failed.project_paths.is_empty());
    assert!(failed.project_paths_persistence_error.is_some());
    release_tx.send(()).unwrap();
    writer.join().unwrap();
    let recovered = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    assert_eq!(recovered.project_paths, vec![a]);
    assert!(recovered.project_paths_persistence_error.is_none());
}

#[test]
fn p03_setup_rollback_restores_exact_raw_projection_under_same_token() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let b = real_ac_project_path(temp.path(), "B");
    let raw = serde_json::json!({"defaultShell":"sentinel","defaultShellArgs":[],"agents":[],"projectPath":a,"projectPaths":[a,"C:/missing"],"projectPathsRelativeToInstance":[null,null]});
    std::fs::write(&path, raw.to_string()).unwrap();
    let mut token =
        super::begin_project_paths_transaction_at_path(&super::AppSettings::default(), &path)
            .unwrap();
    let before = token.raw_project_projection().unwrap();
    let mut candidate = token.fresh_settings().clone();
    token.refresh_raw_projects(&mut candidate).unwrap();
    candidate.project_paths.push(b);
    super::resync_project_state_from_runtime(&mut candidate);
    token.commit_project_groups(&candidate, true, true).unwrap();
    token.restore_raw_project_projection(&before).unwrap();
    assert_eq!(token.raw_project_projection().unwrap(), before);
    let managed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(super::raw_project_projection(managed.as_object().unwrap()).is_empty());
}

#[test]
fn project_migration_fault_reopen_and_backup_recovery() {
    let raw = serde_json::json!({"projectPaths":["C:/missing"],"projectPath":null,"unknown":{"keep":true}});
    for point in [
        "backup_create",
        "backup_prefix",
        "backup_write",
        "backup_sync",
        "backup_before_rename",
        "backup_published",
        "destination_before_write",
        "destination_published",
        "destination_readback",
        "source_cleanup",
    ] {
        let (temp, path, bytes) = migration_fixture(&raw);
        let transaction = migration_transaction(&path);
        let mut orphan = None;
        assert!(
            super::ensure_project_paths_migrated_with(&transaction, &mut |stage, target| {
                if stage == point {
                    if target
                        .extension()
                        .is_some_and(|extension| extension == "tmp")
                        && target.exists()
                    {
                        orphan = Some((target.to_path_buf(), std::fs::read(target).unwrap()));
                    }
                    return Err(super::project_migration_error(
                        target,
                        super::SettingsSaveStage::TempWrite,
                        "injected backup/publication failure",
                    ));
                }
                Ok(())
            })
            .is_err(),
            "{point}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{point}");
        let backup = temp
            .path()
            .join(super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
        if point.starts_with("backup_") && point != "backup_published" {
            assert!(!backup.exists());
        }
        drop(transaction);
        super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), bytes);
        if let Some((orphan, original)) = orphan {
            assert_eq!(std::fs::read(orphan).unwrap(), original);
        }
        assert_eq!(
            super::read_raw_project_source(&super::project_paths_path(&path))
                .unwrap()
                .unwrap()
                .object,
            super::raw_project_projection(raw.as_object().unwrap())
        );
    }
    let (temp, path, bytes) = migration_fixture(&raw);
    let backup = temp
        .path()
        .join(super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    std::fs::write(&backup, b"historical partial backup").unwrap();
    let transaction = migration_transaction(&path);
    let error = super::ensure_project_paths_migrated_locked(&transaction).unwrap_err();
    assert!(matches!(
        error.cause,
        super::SettingsSaveCause::Semantic(super::SettingsSaveReason::ProjectPathsBackupMismatch)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!super::project_paths_path(&path).exists());
    // Simulate the specified stopped-operator recovery, inside this fixture only.
    let recovered = temp
        .path()
        .join("project-paths.pre-migration.recovered-20261008-unique.no-git.json");
    assert!(!recovered.exists());
    std::fs::rename(&backup, &recovered).unwrap();
    super::ensure_project_paths_migrated_locked(&transaction).unwrap();
    assert_eq!(
        std::fs::read(recovered).unwrap(),
        b"historical partial backup"
    );
    assert_eq!(std::fs::read(backup).unwrap(), bytes);
}

#[test]
fn project_migration_atomic_failure_matrix() {
    for target_source in [false, true] {
        for stage in [
            super::SettingsSaveStage::TempCreate,
            super::SettingsSaveStage::TempWrite,
            super::SettingsSaveStage::TempSync,
            super::SettingsSaveStage::AtomicReplace,
            super::SettingsSaveStage::PostWriteRead,
        ] {
            let (_temp, path, bytes) =
                migration_fixture(&serde_json::json!({"projectPaths":["C:/gone"],"unknown":9}));
            let target = if target_source {
                path.clone()
            } else {
                super::project_paths_path(&path)
            };
            let transaction = migration_transaction(&path);
            super::PROJECT_MIGRATION_ATOMIC_FAULT
                .with(|slot| *slot.borrow_mut() = Some((target, stage)));
            let result = super::ensure_project_paths_migrated_locked(&transaction);
            super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
            assert!(result.is_err());
            if !target_source || stage != super::SettingsSaveStage::PostWriteRead {
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
            drop(transaction);
            super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
            assert_eq!(
                super::read_raw_project_source(&super::project_paths_path(&path))
                    .unwrap()
                    .unwrap()
                    .object["projectPaths"],
                serde_json::json!(["C:/gone"])
            );
            assert!(super::raw_project_projection(
                &super::read_raw_project_source(&path)
                    .unwrap()
                    .unwrap()
                    .object
            )
            .is_empty());
        }
    }
}

#[test]
fn project_migration_equal_retry_skips_backup_and_keeps_unrelated_changes() {
    let (temp, path, _) =
        migration_fixture(&serde_json::json!({"projectPaths":["C:/gone"],"unknown":1}));
    let transaction = migration_transaction(&path);
    assert!(
        super::ensure_project_paths_migrated_with(&transaction, &mut |stage, target| {
            if stage == "source_cleanup" {
                return Err(super::project_migration_error(
                    target,
                    super::SettingsSaveStage::AtomicReplace,
                    "cleanup failed",
                ));
            }
            Ok(())
        })
        .is_err()
    );
    drop(transaction);
    std::fs::write(&path, br#"{"projectPaths":["C:/gone"],"unknown":2}"#).unwrap();
    let backup = temp
        .path()
        .join(super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    let backup_bytes = std::fs::read(&backup).unwrap();
    super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
    assert_eq!(
        super::read_raw_project_source(&path)
            .unwrap()
            .unwrap()
            .object["unknown"],
        2
    );
    assert_eq!(std::fs::read(&backup).unwrap(), backup_bytes);
    std::fs::write(&path, br#"{"projectPaths":["different"],"unknown":3}"#).unwrap();
    assert!(super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).is_err());
    std::fs::write(&path, br#"{"projectPaths":["C:/gone"],"unknown":3}"#).unwrap();
    std::fs::remove_file(&backup).unwrap();
    std::fs::create_dir(&backup).unwrap();
    super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
    assert_eq!(
        super::read_raw_project_source(&path)
            .unwrap()
            .unwrap()
            .object["unknown"],
        3
    );
    assert!(
        backup.is_dir(),
        "equal remnant must never read historical backup"
    );
}

#[cfg(unix)]
#[test]
fn p03_final_retained_root_check_rejects_persistent_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("selected");
    std::fs::create_dir(&parent).unwrap();
    let path = parent.join("settings.json");
    let retired = temp.path().join("retired");
    super::PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |path| {
            let parent = path.parent().unwrap();
            std::fs::rename(parent, &retired).unwrap();
            std::fs::create_dir(parent).unwrap();
            let mut replacement = serde_json::to_value(super::AppSettings::default()).unwrap();
            replacement["roomNumberMask"] = serde_json::json!("###");
            std::fs::write(path, serde_json::to_vec(&replacement).unwrap()).unwrap();
        }));
    });
    let captured = super::capture_project_settings(&path, false);
    super::PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| *hook.borrow_mut() = None);
    let (settings, _, original) = captured.unwrap();
    assert_eq!(settings.room_number_mask, "###");
    assert_eq!(
        settings.project_paths_persistence_error.as_deref(),
        Some(super::SETTINGS_LOCK_UNAVAILABLE_CODE)
    );
    assert!(settings.project_paths.is_empty());
    assert!(original.is_none());
    assert!(!super::project_paths_path(&path).exists());
}

#[test]
fn p03_capture_creates_fresh_parent_without_readonly_data_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fresh/nested/settings.json");
    let (settings, _, _) = super::capture_project_settings(&path, false).unwrap();
    assert!(settings.project_paths_persistence_error.is_none());
    assert!(settings.project_paths.is_empty());
    assert!(path.parent().unwrap().is_dir());
    assert!(!path.exists());
    assert!(!super::project_paths_path(&path).exists());
}

#[test]
fn p03_physical_agents_io_preserves_logical_registry_and_messages() {
    let temp = tempfile::tempdir().unwrap();
    let io = temp.path().join("physical/settings.json");
    let label = temp.path().join("logical/settings.json");
    std::fs::create_dir_all(io.parent().unwrap()).unwrap();
    std::fs::write(super::agents_instance_path(&io), b"{broken").unwrap();
    let projection = serde_json::Map::new();
    let contents = serde_json::to_string(&super::AppSettings::default()).unwrap();
    let (mut settings, _) = super::parse_settings_json_with_projection_labeled(
        &contents,
        &label.to_string_lossy(),
        Some(&io),
        Some(&label),
        Some(&projection),
    )
    .unwrap();
    assert_eq!(
        settings.agents_layer.unreadable_path(),
        Some(super::agents_instance_path(&label).as_path())
    );
    assert!(super::agents_layer_unreadable_for_dir(
        label.parent().unwrap()
    ));
    assert!(!super::agents_layer_unreadable_for_dir(
        io.parent().unwrap()
    ));
    if let super::AgentsLayerState::Unreadable { reason, .. } = &settings.agents_layer {
        assert!(reason.contains(&super::agents_instance_path(&label).display().to_string()));
        assert!(!reason.contains(&super::agents_instance_path(&io).display().to_string()));
    } else {
        panic!("physical unreadable agents layer was lost");
    }
    super::set_agents_layer_state(&mut settings, &label, super::AgentsLayerState::Readable);
}

#[test]
fn p03_labeled_migration_keeps_raw_bytes_and_logical_conflict_paths() {
    let temp = tempfile::tempdir().unwrap();
    let io = temp.path().join("physical/settings.json");
    let label = temp.path().join("logical/settings.json");
    std::fs::create_dir_all(io.parent().unwrap()).unwrap();
    let original = br####"{"roomNumberMask":"###","projectPaths":["missing"],"projectPath":null,"projectPathsRelativeToInstance":[null],"archivedProjectPaths":["archived"]}"####;
    std::fs::write(&io, original).unwrap();
    let _transaction = super::empty_project_transaction(&io, "test-lock").unwrap();
    super::ensure_project_paths_migrated_at_labeled_path(&io, &label, &mut |_, _| Ok(())).unwrap();
    let backup =
        io.with_file_name(crate::config::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    assert_eq!(std::fs::read(backup).unwrap(), original);
    let cleaned: serde_json::Value = serde_json::from_slice(&std::fs::read(&io).unwrap()).unwrap();
    assert!(super::raw_project_projection(cleaned.as_object().unwrap()).is_empty());
    assert_eq!(cleaned["roomNumberMask"], "###");
    let selected =
        super::select_project_source_at_labeled_path_with_limit(&io, &label, 16 * 1024 * 1024)
            .unwrap();
    let raw: serde_json::Value = serde_json::from_slice(original).unwrap();
    assert_eq!(
        selected.projection,
        super::raw_project_projection(raw.as_object().unwrap())
    );
    let before = std::fs::read(super::project_paths_path(&io)).unwrap();
    super::ensure_project_paths_migrated_at_labeled_path(&io, &label, &mut |_, _| Ok(())).unwrap();
    assert_eq!(
        std::fs::read(super::project_paths_path(&io)).unwrap(),
        before
    );
    std::fs::write(&io, br#"{"projectPaths":["other"]}"#).unwrap();
    let error =
        super::select_project_source_at_labeled_path_with_limit(&io, &label, 16 * 1024 * 1024)
            .unwrap_err();
    let message = error.legacy_outward.into_general_string();
    assert!(message.contains(&label.display().to_string()));
    assert!(message.contains(&super::project_paths_path(&label).display().to_string()));
    assert!(!message.contains(&io.display().to_string()));
}

#[test]
fn e2432_project_capture_syntax_admission_all_participants() {
    for case in [
        "syntax",
        "overlay-syntax",
        "overlay-syntax-only",
        "overlay-io",
        "main-duplicate",
        "overlay-duplicate",
        "overlay-binding",
        "overlay-quarantine",
        "overlay-conflict",
        "sidecar-io",
    ] {
        project_capture_syntax_case(case);
    }
}

fn project_capture_syntax_case(case: &str) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    let mut main = serde_json::json!({
        "rootToken":"stable", "projectPaths":[project], "projectPath":project
    })
    .to_string();
    if case == "main-duplicate" {
        main.pop();
        main.push_str(",\"unknown\":1,\"unknown\":1}");
    }
    std::fs::write(&path, &main).unwrap();
    let sidecar = super::agents_instance_path(&path);
    if case == "sidecar-io" {
        std::fs::create_dir(&sidecar).unwrap();
    } else if case == "overlay-syntax-only" {
        std::fs::write(&sidecar, b"{}").unwrap();
    } else {
        std::fs::write(&sidecar, b"{broken agents").unwrap();
    }
    let overlay = path.with_file_name(super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
    if case == "overlay-io" {
        std::fs::create_dir(&overlay).unwrap();
    }
    let overlay_bytes = match case {
        "overlay-syntax" | "overlay-syntax-only" => Some("{broken overlay".to_owned()),
        "overlay-duplicate" => Some(r#"{"agents":[],"unknown":1,"unknown":1}"#.to_owned()),
        "overlay-binding" => {
            Some(r#"{"codingAgentProfiles":{"profileBindingVersion":9}}"#.to_owned())
        }
        "overlay-quarantine" => {
            Some(r#"{"codingAgentProfiles":{"legacyProfileOrphans":null}}"#.to_owned())
        }
        "overlay-conflict" => Some(
            serde_json::json!({
                "agents":[b3_agent("alpha", "A"), b3_agent("beta", "B")],
                "codingAgentProfiles":{"profilesByAgent":{
                    "alpha":{"A":{"notes":"first"}}, "beta":{"A":{"notes":"second"}}
                }}
            })
            .to_string(),
        ),
        _ => None,
    };
    if let Some(bytes) = &overlay_bytes {
        std::fs::write(&overlay, bytes).unwrap();
    }
    let capture = super::collect_registered_profile_gate(&path);
    let migrates = matches!(
        case,
        "syntax" | "overlay-syntax" | "overlay-syntax-only" | "overlay-io"
    );
    assert_eq!(capture.admits_project_migration(&path), migrates, "{case}");
    assert_eq!(
        capture.ordinary_error().is_some(),
        case != "overlay-syntax-only",
        "{case}"
    );
    if matches!(
        case,
        "overlay-syntax" | "overlay-syntax-only" | "overlay-io"
    ) {
        assert!(!capture
            .participant_documents()
            .iter()
            .any(|(document, _)| document == &overlay));
    }
    super::read_settings_for_startup(&path);
    assert_eq!(
        super::project_paths_path(&path).exists(),
        migrates,
        "{case}"
    );
    if !migrates {
        assert_eq!(std::fs::read_to_string(&path).unwrap(), main, "{case}");
        assert!(!path
            .with_file_name(super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME)
            .exists());
    }
    if case == "overlay-syntax-only" {
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"{}");
    } else if case != "sidecar-io" {
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"{broken agents");
    }
    if let Some(bytes) = overlay_bytes {
        assert_eq!(std::fs::read_to_string(&overlay).unwrap(), bytes);
    }
    if case == "overlay-io" {
        assert!(overlay.is_dir());
    }
}

#[test]
fn e2432_project_capture_rechecks_malformed_bytes_and_absence() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, b"{}").unwrap();
    let sidecar = super::agents_instance_path(&path);
    std::fs::write(&sidecar, b"{broken").unwrap();
    let captured = super::collect_registered_profile_gate(&path);
    assert!(captured.admits_project_migration(&path));
    assert!(super::recheck_registered_profile_gate(&captured.documents).is_ok());
    std::fs::write(&sidecar, b"{changed").unwrap();
    assert!(super::recheck_registered_profile_gate(&captured.documents).is_err());
    std::fs::write(&sidecar, b"{broken").unwrap();
    std::fs::write(
        path.with_file_name(super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
        b"{}",
    )
    .unwrap();
    assert!(super::recheck_registered_profile_gate(&captured.documents).is_err());
}

#[test]
fn e2432_editable_legacy_action_preserves_opaque_canonical_fixed_point() {
    let owner = serde_json::json!({
        "agents":[b3_agent("alpha", "A")],
        "codingAgentProfiles":{
            "schemaVersion":2,"letters":{"old":true},
            "profileSlots":{"A":{"name":"opaque", "extension":7}},
            "profilesByAgent":{"alpha":{"A":{"command":"--x", "argv":["opaque"], "extension":9}}}
        }
    });
    assert!(super::actionable_profile_legacy_work(&owner, true).unwrap());
    let projected = super::project_physical_profiles(&owner, true).unwrap();
    let mut canonical = super::canonical_profile_source(&owner, &projected, true).unwrap();
    canonical["codingAgentProfiles"]
        .as_object_mut()
        .unwrap()
        .remove("letters");
    assert!(super::legacy_profiles_shape_present(
        canonical.as_object().unwrap()
    ));
    assert!(!super::actionable_profile_legacy_work(&canonical, true).unwrap());
    assert_eq!(
        canonical["codingAgentProfiles"]["profileSlots"]["A"]["name"],
        "opaque"
    );
    let hash = super::configuration_identity("claude", &[]);
    assert_eq!(
        canonical["codingAgentProfiles"]["profilesByBaseHash"][&hash]["A"]["argv"],
        serde_json::json!(["opaque"])
    );
    assert_eq!(
        canonical["codingAgentProfiles"]["profilesByBaseHash"][&hash]["A"]["extension"],
        9
    );
    canonical["codingAgentProfiles"]["matrix"] = serde_json::json!({"old":true});
    assert!(super::actionable_profile_legacy_work(&canonical, true).unwrap());
}

#[test]
fn selected_project_decode_fallback_retains_overlay_and_agents_state() {
    for startup in [false, true] {
        for unreadable in [false, true] {
            assert_selected_project_decode_fallback_case(startup, unreadable);
        }
    }
}

fn n1_test_overlay(kind: &str) -> serde_json::Value {
    if kind == "agents" {
        serde_json::json!({"agents":[b3_agent("overlay-only", "Overlay")]})
    } else {
        serde_json::json!({"codingAgentProfiles":{
            "profileSlots":{"A":{"label":"overlay-slot"}},
            "profilesByAgent":{"overlay-only":{"A":{"additionalArguments":"--overlay"}}},
            "defaultProfileByAgent":{"overlay-matrix":"A"}
        }})
    }
}

#[test]
fn e2432_n1_absent_main_startup_and_ordinary_first_save() {
    for startup in [false, true] {
        for kind in ["agents", "profiles"] {
            let temp = tempfile::tempdir().unwrap();
            let path = b3_settings_path(temp.path());
            let overlay_path = b3_overlay_path(temp.path());
            b3_write(&overlay_path, &n1_test_overlay(kind));
            let overlay_bytes = std::fs::read(&overlay_path).unwrap();
            let before = hash_activation_files(temp.path());
            let mut draft = super::load_settings_for_cli_from_path(&path);
            assert!(draft.coding_agent_identity_physical.is_none());
            assert_eq!(hash_activation_files(temp.path()), before);
            assert!(!path.exists());
            assert!(!super::agents_instance_path(&path).exists());
            let written = if startup {
                super::load_settings_from_path(&path)
            } else {
                draft.root_token = Some("ordinary-first-save".into());
                let written =
                    super::save_settings_to_path_preserving_project_paths(&draft, &path).unwrap();
                assert!(
                    draft.coding_agent_identity_physical.is_none(),
                    "comparison reference must not become input identity"
                );
                written
            };
            assert!(path.exists(), "first save {kind}/{startup}");
            assert!(written.root_token.is_some());
            assert_eq!(std::fs::read(&overlay_path).unwrap(), overlay_bytes);
            assert!(!super::agents_instance_path(&path).exists());
            let physical = b3_object(&path);
            assert!(!physical
                .get("agents")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .any(|row| row["id"] == "overlay-only"));
            let profiles = physical.get("codingAgentProfiles");
            assert!(profiles
                .and_then(|p| p.get("profilesByAgent"))
                .and_then(|p| p.get("overlay-only"))
                .is_none());
            assert!(profiles
                .and_then(|p| p.get("defaultProfileByAgent"))
                .and_then(|p| p.get("overlay-matrix"))
                .is_none());
            assert_ne!(
                profiles
                    .and_then(|p| p.pointer("/profileSlots/A/label"))
                    .and_then(serde_json::Value::as_str),
                Some("overlay-slot")
            );
            let reloaded = super::load_settings_for_cli_from_path(&path);
            assert_eq!(
                serde_json::to_value(&written.agents).unwrap(),
                serde_json::to_value(&reloaded.agents).unwrap()
            );
            assert_eq!(
                written.coding_agent_profiles,
                reloaded.coding_agent_profiles
            );
            if kind == "agents" {
                assert_eq!(written.agents[0].id, "overlay-only");
            } else {
                assert_eq!(
                    written.coding_agent_profiles.profile_slots["A"].label,
                    "overlay-slot"
                );
                assert_eq!(
                    written.coding_agent_profiles.profiles_by_agent["overlay-only"]["A"].command,
                    "--overlay"
                );
            }
        }
    }
}

#[test]
fn e2432_n1_absent_main_real_owned_edits_refuse_before_publishers() {
    for kind in ["agents", "profiles", "default"] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&b3_overlay_path(temp.path()), &n1_test_overlay(kind));
        let mut draft = super::load_settings_for_cli_from_path(&path);
        assert!(draft.coding_agent_identity_physical.is_none());
        match kind {
            "agents" => draft.agents[0].command = "changed-owned-command".into(),
            "profiles" => {
                draft
                    .coding_agent_profiles
                    .profiles_by_agent
                    .get_mut("overlay-only")
                    .unwrap()
                    .get_mut("A")
                    .unwrap()
                    .command = "changed-owned-profile".into()
            }
            _ => {
                draft
                    .coding_agent_profiles
                    .default_profile_by_agent
                    .insert("overlay-matrix".into(), "B".into());
            }
        }
        let before = hash_activation_files(temp.path());
        let error =
            super::save_settings_to_path_preserving_project_paths(&draft, &path).unwrap_err();
        assert!(
            error.contains("overlayOwnedProfileGroup"),
            "{kind}: {error}"
        );
        assert_eq!(hash_activation_files(temp.path()), before);
        assert!(!path.exists());
        assert!(!super::project_paths_path(&path).exists());
    }
}

#[test]
fn e2432_n1_captured_sidecar_home_is_not_overlay_or_default_authority() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let defaults = serde_json::to_value(super::AppSettings::default()).unwrap();
    let sidecar_path = super::agents_instance_path(&path);
    let mut sidecar = serde_json::json!({"agents":defaults["agents"],"codingAgentProfiles":defaults["codingAgentProfiles"]});
    sidecar["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"overlay-only":"physical-home"});
    b3_write(&sidecar_path, &sidecar);
    let mut overlay = n1_test_overlay("agents");
    overlay["codingAgentProfiles"] =
        serde_json::json!({"codexHomeKeyByAgent":{"overlay-only":"forged-home"}});
    b3_write(&b3_overlay_path(temp.path()), &overlay);
    let raw_sidecar = std::fs::read(&sidecar_path).unwrap();
    let raw_overlay = std::fs::read(b3_overlay_path(temp.path())).unwrap();
    let mut draft = super::load_settings_for_cli_from_path(&path);
    assert!(!path.exists());
    assert!(draft.coding_agent_identity_physical.is_none());
    assert_eq!(
        draft.coding_agent_profiles.codex_home_key_by_agent["overlay-only"],
        "physical-home"
    );
    draft.root_token = Some("ordinary-first-save".into());
    let written = super::save_settings_to_path_preserving_project_paths(&draft, &path).unwrap();
    assert!(draft.coding_agent_identity_physical.is_none());
    assert_eq!(
        written.coding_agent_profiles.codex_home_key_by_agent["overlay-only"],
        "physical-home"
    );
    assert_eq!(std::fs::read(&sidecar_path).unwrap(), raw_sidecar);
    assert_eq!(
        std::fs::read(b3_overlay_path(temp.path())).unwrap(),
        raw_overlay
    );
    assert_eq!(written.agents[0].id, "overlay-only");
    assert_eq!(
        written.coding_agent_profiles,
        super::load_settings_for_cli_from_path(&path).coding_agent_profiles
    );
}

#[test]
fn e2432_n1_stale_overlay_ownership_and_present_main_stay_refused() {
    for case in [
        "changed-value",
        "changed-ownership",
        "typed-invalid",
        "main-present",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        let overlay = b3_overlay_path(temp.path());
        b3_write(&overlay, &n1_test_overlay("agents"));
        let draft = super::load_settings_for_cli_from_path(&path);
        assert!(draft.coding_agent_identity_physical.is_none());
        match case {
            "changed-value" => b3_write(
                &overlay,
                &serde_json::json!({"agents":[{"id":"overlay-only","label":"Overlay","color":"red","command":"changed"}]}),
            ),
            "changed-ownership" => b3_write(&overlay, &n1_test_overlay("profiles")),
            "typed-invalid" => b3_write(&overlay, &serde_json::json!({"agents":"not-an-array"})),
            _ => b3_write(
                &path,
                &serde_json::to_value(super::AppSettings::default()).unwrap(),
            ),
        }
        let before = hash_activation_files(temp.path());
        let error =
            super::save_settings_to_path_preserving_project_paths(&draft, &path).unwrap_err();
        assert!(
            error.contains("overlayProfileComparisonUnavailable")
                || (case == "changed-value" && error.contains("overlayOwnedProfileGroup")),
            "{case}: {error}"
        );
        assert_eq!(hash_activation_files(temp.path()), before);
    }
}

#[test]
fn e2432_dl1_membership_and_n1_absence_revision_barriers() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let overlay = b3_overlay_path(temp.path());
    b3_write(&path, &serde_json::json!({}));
    std::fs::write(&overlay, b"{rejected").unwrap();
    let captured = super::collect_registered_profile_gate(&path);
    assert!(captured.admits_project_migration(&path));
    assert!(!captured
        .participant_documents()
        .iter()
        .any(|(p, _)| p == &overlay));
    std::fs::write(&overlay, b"{different rejected").unwrap();
    assert!(super::recheck_registered_profile_gate(&captured.participant_documents()).is_ok());
    b3_write(&overlay, &n1_test_overlay("agents"));
    let participating = super::capture_registered_profile_gate(&path).unwrap();
    std::fs::remove_file(&overlay).unwrap();
    std::fs::create_dir(&overlay).unwrap();
    assert!(super::recheck_registered_profile_gate(&participating).is_err());
    std::fs::remove_dir(&overlay).unwrap();
    b3_write(&overlay, &n1_test_overlay("agents"));
    std::fs::remove_file(&path).unwrap();
    let absent = super::capture_registered_profile_gate(&path).unwrap();
    let draft = super::load_settings_for_cli_from_path(&path);
    assert!(super::absent_main_overlay_comparison(&draft, &path, &None, &None, &absent).is_ok());
    b3_write(&path, &serde_json::json!({}));
    assert!(super::recheck_registered_profile_gate(&absent).is_err());
    let present = super::capture_registered_profile_gate(&path).unwrap();
    assert!(super::absent_main_overlay_comparison(&draft, &path, &None, &None, &present).is_err());
    std::fs::remove_file(&path).unwrap();
    b3_write(&overlay, &n1_test_overlay("profiles"));
    assert!(super::recheck_registered_profile_gate(&absent).is_err());
}

fn assert_o2_stable_full_load(path: &std::path::Path) {
    let before = hash_activation_files(path.parent().unwrap());
    let (diagnostics, _capture) = capture_settings_save_diagnostics();
    // An attempted final main publication reports this existing fault even
    // when equal bytes would otherwise hide an unnecessary write.
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.to_path_buf(), super::SettingsSaveStage::TempCreate));
    });
    let loaded = super::load_settings_from_path(path);
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(loaded.project_paths_persistence_error.is_none());
    assert!(
        diagnostics.borrow().is_empty(),
        "unexpected automatic publication: {:?}",
        diagnostics.borrow()
    );
    assert_eq!(hash_activation_files(path.parent().unwrap()), before);
}

#[test]
fn e2432_round7_schema2_load_migrates_without_old_markers() {
    for in_sidecar in [false, true] {
        let (_temp, path) = hash_activation_seed(in_sidecar);
        let owner = if in_sidecar {
            super::agents_instance_path(&path)
        } else {
            path.clone()
        };
        let mut raw = serde_json::Value::Object(b3_object(&owner));
        raw["codingAgentProfiles"] = serde_json::json!({
            "schemaVersion":2,
            "profilesByAgent":{"alpha":{"A":{"command":"--model x","notes":"retained","opaque":{"null":null}}}},
            "profileLabelsByAgent":{"alpha":{"A":"Own"}},
            "profileSlots":{"A":{"label":"Slot","opaque":[null,3]}},
            "defaultProfileByAgent":{"matrix":"A"},
            "codexHomeKeyByAgent":{"alpha":"home-leaf"},
            "identityByAgent":{"alpha":"not-BASE","identity-only":null}
        });
        b3_write(&owner, &raw);
        assert!(!super::legacy_profiles_shape_present(
            raw.as_object().unwrap()
        ));
        let before = super::load_settings_for_cli_from_path(&path);
        assert!(before.root_token.is_some());
        let loaded = super::load_settings_from_path(&path);
        let stored = b3_object(&super::agents_instance_path(&path));
        let profiles = &stored["codingAgentProfiles"];
        let hash = super::configuration_identity("claude", &[]);
        assert_eq!(profiles["profileBindingVersion"], 1);
        assert_eq!(
            profiles["profilesByBaseHash"][&hash],
            super::normalized_profile_cells(
                raw.pointer("/codingAgentProfiles/profilesByAgent/alpha")
            )
            .unwrap()
        );
        for key in [
            "profileLabelsByBaseHash",
            "profileSlots",
            "defaultProfileByAgent",
            "codexHomeKeyByAgent",
        ] {
            let expected = if key == "profileLabelsByBaseHash" {
                serde_json::json!({(hash.clone()):{"A":"Own"}})
            } else {
                raw["codingAgentProfiles"][key].clone()
            };
            assert_eq!(profiles[key], expected, "{key}");
        }
        for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
            assert!(profiles.get(key).is_none());
        }
        assert_eq!(
            profiles["legacyProfileOrphans"]["identity-only"],
            serde_json::json!([{"identity":null}])
        );
        assert_eq!(
            serde_json::to_value(&loaded.agents).unwrap(),
            serde_json::to_value(&before.agents).unwrap()
        );
        assert_eq!(loaded.root_token, before.root_token);
        assert_eq!(
            loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
            "retained"
        );
        if !in_sidecar {
            let main = b3_object(&path);
            assert!(!main.contains_key("agents"));
            assert!(!main.contains_key("codingAgentProfiles"));
        }
        assert_o2_stable_full_load(&path);
    }
}

#[test]
fn e2432_round7_authoritative_split_routes_only_selected_raw_groups() {
    for sidecar_rows in [false, true] {
        let (_temp, path) = hash_activation_seed(true);
        let sidecar_path = super::agents_instance_path(&path);
        let mut main = b3_object(&path);
        let mut sidecar = b3_object(&sidecar_path);
        let mut rows = sidecar.remove("agents").unwrap();
        rows[0]["baseCommand"] = serde_json::json!("codex");
        rows[0].as_object_mut().unwrap().remove("command");
        rows[0]["opaqueRow"] = serde_json::json!({"keep":[null,7]});
        let mut profiles = sidecar.remove("codingAgentProfiles").unwrap();
        profiles["profilesByAgent"] =
            serde_json::json!({"alpha":{"A":{"command":"--selected","opaqueCell":[null]}}});
        profiles["profilesByAgent"]["beta"] =
            serde_json::json!({"A":{"notes":"distinct shadowed claude set","opaque":null}});
        profiles["profileLabelsByAgent"] = serde_json::json!({"alpha":{"A":"Selected"}});
        profiles["codexHomeKeyByAgent"] = serde_json::json!({"alpha":"selected-home"});
        if sidecar_rows {
            sidecar.insert("agents".into(), rows.clone());
            main.insert(
                "agents".into(),
                serde_json::json!([
                    b3_agent("alpha", "Shadowed claude"),
                    b3_agent("beta", "Conflicting shadowed claude")
                ]),
            );
            main.insert("codingAgentProfiles".into(), profiles.clone());
        } else {
            main.insert("agents".into(), rows.clone());
            main.insert(
                "codingAgentProfiles".into(),
                serde_json::json!({"profilesByAgent":{"alpha":{"A":{"notes":"shadowed"}}}}),
            );
            sidecar.insert("codingAgentProfiles".into(), profiles.clone());
        }
        b3_write(&path, &serde_json::Value::Object(main.clone()));
        b3_write(&sidecar_path, &serde_json::Value::Object(sidecar));
        let loaded = super::load_settings_from_path(&path);
        assert!(loaded.project_paths_persistence_error.is_none());
        let saved = b3_object(&sidecar_path);
        let hash = super::configuration_identity("codex", &[]);
        assert_eq!(
            saved["codingAgentProfiles"]["profilesByBaseHash"][&hash],
            super::normalized_profile_cells(profiles.pointer("/profilesByAgent/alpha")).unwrap()
        );
        assert_eq!(
            saved["codingAgentProfiles"]["profilesByBaseHash"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert!(saved["codingAgentProfiles"]["profilesByBaseHash"]
            .get(super::configuration_identity("claude", &[]))
            .is_none());
        assert_eq!(
            saved["codingAgentProfiles"]["codexHomeKeyByAgent"],
            profiles["codexHomeKeyByAgent"]
        );
        assert_eq!(
            saved["codingAgentProfiles"]["legacyProfileOrphans"]["beta"],
            serde_json::json!([{"cells":profiles["profilesByAgent"]["beta"].clone()}])
        );
        assert_eq!(saved["agents"][0]["opaqueRow"], rows[0]["opaqueRow"]);
        assert_eq!(saved["agents"][0].get("command"), rows[0].get("command"));
        assert_eq!(saved["agents"][0]["baseCommand"], rows[0]["baseCommand"]);
        let expected: Vec<super::AgentConfig> = serde_json::from_value(rows).unwrap();
        let mut expected = expected;
        super::finalize_agent_order(&mut expected);
        super::refresh_agent_id_hashes(&mut expected);
        assert_eq!(
            serde_json::to_value(&loaded.agents).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let saved_main = b3_object(&path);
        let transferred = if sidecar_rows {
            "codingAgentProfiles"
        } else {
            "agents"
        };
        let unselected = if sidecar_rows {
            "agents"
        } else {
            "codingAgentProfiles"
        };
        assert!(!saved_main.contains_key(transferred));
        assert_eq!(saved_main.get(unselected), main.get(unselected));
        assert_eq!(
            loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].command,
            "--selected"
        );
        assert_o2_stable_full_load(&path);
    }
}

#[test]
fn e2432_round7_empty_presence_blocks_main_donors() {
    for empty_rows in [false, true] {
        let (_temp, path) = hash_activation_seed(false);
        let sidecar = super::agents_instance_path(&path);
        let mut main = serde_json::Value::Object(b3_object(&path));
        main["codingAgentProfiles"]["profilesByAgent"] =
            serde_json::json!({"alpha":{"B":{"notes":"main"}}});
        b3_write(&path, &main);
        b3_write(
            &sidecar,
            &if empty_rows {
                serde_json::json!({"agents":[]})
            } else {
                serde_json::json!({"codingAgentProfiles":{}})
            },
        );
        let loaded = super::load_settings_from_path(&path);
        let saved = b3_object(&sidecar);
        let profiles = &saved["codingAgentProfiles"];
        if empty_rows {
            assert_eq!(saved["agents"], serde_json::json!([]));
            assert!(loaded.agents.is_empty());
            assert_eq!(profiles["profilesByBaseHash"], serde_json::json!({}));
            assert_eq!(
                profiles["legacyProfileOrphans"]["alpha"],
                serde_json::json!([{"cells":{"B":{"notes":"main"}}}])
            );
        } else {
            let hash = super::configuration_identity("claude", &[]);
            assert!(profiles["profilesByBaseHash"][&hash].get("B").is_none());
            assert_eq!(
                b3_object(&path)["codingAgentProfiles"],
                main["codingAgentProfiles"]
            );
        }
        assert_o2_stable_full_load(&path);
    }
}

#[test]
fn e2432_round7_complete_collision_matrix_leaves_every_id_and_publisher_untouched() {
    for difference in [
        "notes",
        "env",
        "enabled",
        "letter",
        "unknown",
        "label",
        "destination",
    ] {
        let (_temp, path) = hash_activation_seed(true);
        let sidecar = super::agents_instance_path(&path);
        let mut source = serde_json::Value::Object(b3_object(&sidecar));
        let mut first = b3_agent("a-compatible", "Earlier compatible row");
        first["command"] = serde_json::json!("safe-tool");
        first["blockingMenus"] =
            serde_json::json!([{"pattern":"^Ready?","notification":"choose","enabled":true}]);
        source["agents"].as_array_mut().unwrap().insert(0, first);
        source["agents"]
            .as_array_mut()
            .unwrap()
            .push(b3_agent("beta", "Second alias"));
        let cells = serde_json::json!({"A":{"notes":"same","opaque":{"keep":null}}});
        source["codingAgentProfiles"]["profilesByAgent"] =
            serde_json::json!({"alpha":cells.clone(),"beta":cells.clone()});
        source["codingAgentProfiles"]["profileLabelsByAgent"] =
            serde_json::json!({"alpha":{"A":"same"},"beta":{"A":"same"}});
        match difference {
            "label" => {
                source["codingAgentProfiles"]["profileLabelsByAgent"]["beta"]["A"] =
                    serde_json::json!("different")
            }
            "letter" => {
                source["codingAgentProfiles"]["profilesByAgent"]["beta"]["B"] =
                    serde_json::json!({})
            }
            "destination" => {
                let hash = super::configuration_identity("claude", &[]);
                source["codingAgentProfiles"]["profilesByBaseHash"] =
                    serde_json::json!({(hash):{"A":{"notes":"prospective destination"}}});
            }
            field => {
                let value = match field {
                    "enabled" => serde_json::json!(false),
                    "env" => serde_json::json!({"KEY":"different"}),
                    "unknown" => serde_json::json!([null, 7]),
                    _ => serde_json::json!("different"),
                };
                source["codingAgentProfiles"]["profilesByAgent"]["beta"]["A"][field] = value;
            }
        }
        b3_write(&sidecar, &source);
        let mut main = serde_json::Value::Object(b3_object(&path));
        main["projectPaths"] = serde_json::json!([]);
        b3_write(&path, &main);
        std::fs::remove_file(super::project_paths_path(&path)).unwrap();
        let before = hash_activation_files(path.parent().unwrap());
        let menus = std::cell::Cell::new(0);
        let publish = |_: &std::path::Path, _: &std::path::Path| {
            menus.set(menus.get() + 1);
            Err("unexpected menu publisher".to_owned())
        };
        let loaded =
            super::load_settings_from_path_with_blocking_menus_publish(&path, Some(&publish));
        assert!(
            loaded.project_paths_persistence_error.is_none(),
            "{difference}"
        );
        assert!(super::registered_profile_raw_gate(&path)
            .unwrap_err()
            .contains("sourceProfileConflict"));
        assert!(super::save_settings_to_path_preserving_project_paths(&loaded, &path).is_err());
        assert_eq!(menus.get(), 0);
        assert_eq!(
            hash_activation_files(path.parent().unwrap()),
            before,
            "{difference}"
        );
        let stored = b3_object(&sidecar);
        assert_eq!(
            stored["codingAgentProfiles"]["profilesByAgent"],
            source["codingAgentProfiles"]["profilesByAgent"]
        );
        // No denied flag is cached: correction permits the next load alone.
        source["codingAgentProfiles"]["profilesByAgent"]["beta"] = cells;
        source["codingAgentProfiles"]["profileLabelsByAgent"]["beta"]["A"] =
            serde_json::json!("same");
        source["codingAgentProfiles"]
            .as_object_mut()
            .unwrap()
            .remove("profilesByBaseHash");
        b3_write(&sidecar, &source);
        let retried = super::load_settings_from_path(&path);
        assert_eq!(
            retried.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
            "same"
        );
        assert!(b3_object(&sidecar)["codingAgentProfiles"]
            .get("profilesByAgent")
            .is_none());
        assert_o2_stable_full_load(&path);
    }
}

#[test]
fn e2432_round7_sidecar_failure_keeps_complete_legacy_packet_for_retry() {
    for stage in [
        super::SettingsSaveStage::TempCreate,
        super::SettingsSaveStage::AtomicReplace,
    ] {
        let (_temp, path) = hash_activation_seed(false);
        let sidecar = super::agents_instance_path(&path);
        let before = hash_activation_files(path.parent().unwrap());
        let before_revision = super::source_bytes_revision(&std::fs::read(&sidecar).unwrap());
        super::PROJECT_MIGRATION_ATOMIC_FAULT
            .with(|slot| *slot.borrow_mut() = Some((sidecar.clone(), stage)));
        let (records, capture) = capture_settings_save_diagnostics();
        let loaded = super::load_settings_from_path(&path);
        super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
        assert!(!records.borrow().is_empty());
        drop(capture);
        assert!(loaded.project_paths_persistence_error.is_none());
        assert_eq!(hash_activation_files(path.parent().unwrap()), before);
        assert_eq!(
            super::source_bytes_revision(&std::fs::read(&sidecar).unwrap()),
            before_revision
        );
        assert!(b3_object(&path)["codingAgentProfiles"]
            .get("profilesByAgent")
            .is_some());
        let retried = super::load_settings_from_path(&path);
        assert!(retried.project_paths_persistence_error.is_none());
        assert_eq!(
            b3_object(&sidecar)["codingAgentProfiles"]["profileBindingVersion"],
            1
        );
        assert_ne!(
            super::source_bytes_revision(&std::fs::read(&sidecar).unwrap()),
            before_revision
        );
        assert!(!b3_object(&path).contains_key("codingAgentProfiles"));
        assert_o2_stable_full_load(&path);
    }
}

#[test]
fn e2432_round7_sidecar_success_main_failure_restarts_canonical_first() {
    let (temp, path) = hash_activation_seed(false);
    let path = std::fs::canonicalize(temp.path())
        .unwrap()
        .join(path.file_name().unwrap());
    let main_before = std::fs::read(&path).unwrap();
    let owner =
        super::SettingsOwner::open_active(&path, std::time::Duration::from_secs(2)).unwrap();
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::SettingsSaveStage::AtomicReplace))
    });
    let report = owner
        .with_operation_blocking("e2432-round7-transfer", |current, guard| {
            super::registered_profile_raw_gate(guard.settings_path())?;
            guard.publish(super::SettingsPublisher::SettingsSave, |physical| {
                super::save_settings_value_locked(
                    current,
                    physical,
                    super::ProjectWriteMode::Preserve,
                    super::TerminalSnapshotGateWriteMode::Preserve,
                    super::MainWindowPlacementWriteMode::Preserve,
                )
                .map_err(|error| error.to_string())
            })
        })
        .unwrap();
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert_eq!(report.outcome, super::OperationOutcome::PartialOrUnknown);
    assert!(report.value.is_none());
    assert!(report.error.is_some());
    assert_eq!(std::fs::read(&path).unwrap(), main_before);
    let sidecar = super::agents_instance_path(&path);
    let committed = b3_object(&sidecar);
    let committed_revision = super::source_bytes_revision(&std::fs::read(&sidecar).unwrap());
    assert_eq!(committed["codingAgentProfiles"]["profileBindingVersion"], 1);
    assert!(committed["codingAgentProfiles"]
        .get("profilesByAgent")
        .is_none());
    owner.drain_blocking(std::time::Duration::from_secs(2));
    let before_restart = hash_activation_files(temp.path());
    let restarted = super::load_settings_from_path(&path);
    assert_eq!(hash_activation_files(temp.path()), before_restart);
    assert_eq!(
        super::source_bytes_revision(&std::fs::read(&sidecar).unwrap()),
        committed_revision
    );
    assert_eq!(
        serde_json::to_value(&restarted.agents).unwrap(),
        serde_json::to_value(super::load_settings_for_cli_from_path(&path).agents).unwrap()
    );
    let mut edited = restarted;
    edited
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "fresh canonical edit".into();
    let written = super::save_settings_to_path_preserving_project_paths(&edited, &path).unwrap();
    assert_eq!(
        written.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
        "fresh canonical edit"
    );
    let hash = super::configuration_identity("claude", &[]);
    assert_eq!(
        b3_object(&sidecar)["codingAgentProfiles"]["profilesByBaseHash"][&hash]["A"]["notes"],
        "fresh canonical edit"
    );
    assert_ne!(
        super::source_bytes_revision(&std::fs::read(&sidecar).unwrap()),
        committed_revision
    );
    assert_eq!(
        b3_object(&path).get("codingAgentProfiles"),
        serde_json::from_slice::<serde_json::Value>(&main_before)
            .unwrap()
            .get("codingAgentProfiles")
    );
    assert_o2_stable_full_load(&path);
}

#[test]
fn e2432_round7_shadowed_equal_strips_only_on_legitimate_publication() {
    let (_temp, path) = hash_activation_seed(true);
    super::load_settings_from_path(&path);
    let sidecar = b3_object(&super::agents_instance_path(&path));
    let mut main = b3_object(&path);
    for key in super::AGENTS_INSTANCE_KEYS {
        main.insert(key.into(), sidecar[key].clone());
    }
    b3_write(&path, &serde_json::Value::Object(main));
    assert_o2_stable_full_load(&path);
    let mut settings = super::load_settings_for_cli_from_path(&path);
    settings.log_level = Some("debug".into());
    super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let main = b3_object(&path);
    assert_eq!(main["logLevel"], "debug");
    for key in super::AGENTS_INSTANCE_KEYS {
        assert!(!main.contains_key(key));
    }
    assert_o2_stable_full_load(&path);
}

#[test]
fn e2432_round7_read_only_catalog_decoys_never_supply_migration_data() {
    let (_temp, path) = hash_activation_seed(true);
    let initial = super::load_settings_for_cli_from_path(&path);
    let mut context = super::SourceReadContext::from_settings(&initial, &path).unwrap();
    context.project = false;
    context.catalog_root = path.parent().unwrap().to_path_buf();
    let mut catalogs = Vec::new();
    for kind in [
        super::SourceKind::CatalogBase,
        super::SourceKind::CatalogPersonal,
    ] {
        let source = context.source(kind);
        let locator = context.locator(&source).unwrap();
        std::fs::create_dir_all(locator.parent().unwrap()).unwrap();
        let decoy = serde_json::json!({"schemaVersion":2,
                "agents":[{"key":"alpha","label":"Decoy","description":"Independent","color":"#112233","baseCommand":"claude"}],
                "codingAgentProfiles":{"alpha":{"A":{"notes":"independent decoy"}}}});
        b3_write(&locator, &decoy);
        catalogs.push((source, locator.clone(), std::fs::read(locator).unwrap()));
    }
    let main_before = std::fs::read(&path).unwrap();
    let sidecar = super::agents_instance_path(&path);
    let registered_before = std::fs::read(&sidecar).unwrap();
    super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    super::capture_project_settings(&path, false).unwrap();
    super::list_active_sources(&context);
    let registered_source = context.source(super::SourceKind::RegisteredInstance);
    for source in catalogs
        .iter()
        .map(|(source, _, _)| source)
        .chain(std::iter::once(&registered_source))
    {
        let snapshot = super::read_source_snapshot(&context, source).unwrap();
        assert_eq!(
            snapshot.snapshot.availability,
            super::SourceAvailability::Available
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), main_before);
    assert_eq!(std::fs::read(&sidecar).unwrap(), registered_before);
    let migrated = super::load_settings_from_path(&path);
    assert_ne!(
        migrated.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
        "independent decoy"
    );
    for (_, locator, bytes) in catalogs {
        assert_eq!(std::fs::read(locator).unwrap(), bytes);
    }
    assert_o2_stable_full_load(&path);
}

#[test]
fn e2432_round7_canonical_main_does_not_relocate_on_startup() {
    let (_temp, path) = hash_activation_seed(false);
    let main = serde_json::Value::Object(b3_object(&path));
    let projected = super::project_physical_profiles(&main, true).unwrap();
    let canonical = super::canonical_profile_source(&main, &projected, true).unwrap();
    b3_write(&path, &canonical);
    std::fs::remove_file(super::agents_instance_path(&path)).unwrap();
    assert_o2_stable_full_load(&path);
    assert!(!super::agents_instance_path(&path).exists());
    let mut settings = super::load_settings_for_cli_from_path(&path);
    settings.log_level = Some("debug".into());
    super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert!(!super::agents_instance_path(&path).exists());
    assert_eq!(
        b3_object(&path)["codingAgentProfiles"],
        canonical["codingAgentProfiles"]
    );
}

#[test]
fn e2432_o2_sidecar_full_load_converts_once_and_keeps_raw_roots() {
    let (temp, path) = hash_activation_seed(true);
    let sidecar = super::agents_instance_path(&path);
    let mut raw = serde_json::Value::Object(b3_object(&sidecar));
    raw["codingAgentProfiles"]["letters"] = serde_json::Value::Null;
    raw["codingAgentProfiles"]["matrix"] = serde_json::json!([{"raw":null}]);
    raw["codingAgentProfiles"]["agentDefaults"] = serde_json::json!({"unknown":[7]});
    raw["codingAgentProfiles"]["profileSlots"]["A"]["name"] = serde_json::json!("opaque");
    raw["codingAgentProfiles"]["profileSlots"]["A"]["extension"] = serde_json::json!([null, 9]);
    raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["additionalArguments"] =
        serde_json::json!("--model x");
    raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["argv"] =
        serde_json::json!(["opaque"]);
    raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["args"] =
        serde_json::json!({"raw":null});
    raw["codingAgentProfiles"]["legacyProfileOrphans"] =
        serde_json::json!({"old":[{"cells":null,"extension":[7]}]});
    b3_write(&sidecar, &raw);
    let backup = path
        .with_file_name(super::super::instance_artifacts::SETTINGS_MIGRATION_BACKUP_384_V1_NAME);
    std::fs::write(&backup, b"first main backup").unwrap();
    assert!(!super::legacy_profiles_shape_present(&b3_object(&path)));

    let loaded = super::load_settings_from_path(&path);
    assert_eq!(
        loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].command,
        "--model x"
    );
    let saved = b3_object(&sidecar);
    let profiles = &saved["codingAgentProfiles"];
    assert_eq!(profiles["profileBindingVersion"], 1);
    for key in ["letters", "matrix", "agentDefaults", "legacyProfileOrphans"] {
        assert_eq!(
            profiles.get(key),
            raw["codingAgentProfiles"].get(key),
            "{key}"
        );
    }
    assert_eq!(profiles["profileSlots"]["A"]["name"], "opaque");
    assert_eq!(
        profiles["profileSlots"]["A"]["extension"],
        serde_json::json!([null, 9])
    );
    let hash = super::configuration_identity("claude", &[]);
    assert_eq!(
        profiles["profilesByBaseHash"][&hash]["A"]["argv"],
        serde_json::json!(["opaque"])
    );
    assert_eq!(
        profiles["profilesByBaseHash"][&hash]["A"]["args"],
        serde_json::json!({"raw":null})
    );
    assert!(profiles.get("profilesByAgent").is_none());
    assert!(profiles.get("profileLabelsByAgent").is_none());
    assert!(profiles.get("identityByAgent").is_none());
    assert_eq!(std::fs::read(&backup).unwrap(), b"first main backup");
    assert_o2_stable_full_load(&path);
    assert_o2_stable_full_load(&path);
    assert_eq!(std::fs::read(&backup).unwrap(), b"first main backup");
    let backups: Vec<_> = hash_activation_files(temp.path())
        .into_keys()
        .filter(|name| name.contains("pre-384"))
        .collect();
    assert_eq!(
        backups,
        vec![backup.file_name().unwrap().to_string_lossy().into_owned()]
    );
}

#[test]
fn e2432_o2_sidecar_stale_maps_cleanup_remains_actionable_without_replay() {
    let (_temp, path) = hash_activation_seed(true);
    let sidecar = super::agents_instance_path(&path);
    let raw = serde_json::Value::Object(b3_object(&sidecar));
    let projected = super::project_physical_profiles(&raw, true).unwrap();
    let mut canonical = super::canonical_profile_source(&raw, &projected, true).unwrap();
    canonical["codingAgentProfiles"]["letters"] = serde_json::json!([null, {"opaque":7}]);
    canonical["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
        "alpha":{"A":{"notes":"stale cannot overwrite BASE"}}, "lost":null
    });
    canonical["codingAgentProfiles"]["profileLabelsByAgent"] =
        serde_json::json!({"labels-only":null});
    canonical["codingAgentProfiles"]["identityByAgent"] =
        serde_json::json!({"identity-only":"not BASE"});
    b3_write(&sidecar, &canonical);
    assert!(!super::legacy_profiles_shape_present(&b3_object(&path)));
    let loaded = super::load_settings_from_path(&path);
    assert_ne!(
        loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].notes,
        "stale cannot overwrite BASE"
    );
    let saved = b3_object(&sidecar);
    let profiles = &saved["codingAgentProfiles"];
    assert_eq!(
        profiles["letters"],
        canonical["codingAgentProfiles"]["letters"]
    );
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(profiles.get(key).is_none(), "{key}");
    }
    assert_eq!(
        profiles["legacyProfileOrphans"]["lost"],
        serde_json::json!([{"cells":null}])
    );
    assert_eq!(
        profiles["legacyProfileOrphans"]["labels-only"],
        serde_json::json!([{"labels":null}])
    );
    assert_eq!(
        profiles["legacyProfileOrphans"]["identity-only"],
        serde_json::json!([{"identity":"not BASE"}])
    );
    let backup = path
        .with_file_name(super::super::instance_artifacts::SETTINGS_MIGRATION_BACKUP_384_V1_NAME);
    assert!(!backup.exists());
    assert_o2_stable_full_load(&path);
    assert_eq!(b3_object(&sidecar)["codingAgentProfiles"], *profiles);
    assert!(!backup.exists());
}

#[test]
fn e2432_o2_protected_legacy_keeps_baseline_save_cause() {
    let (temp, path) = hash_activation_seed(true);
    let mut main = b3_object(&path);
    main.insert(
        "codingAgentProfiles".into(),
        serde_json::json!({"schemaVersion":2,"letters":{"old":true}}),
    );
    b3_write(&path, &serde_json::Value::Object(main));
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({"codingAgentProfiles":{"profileSlots":{"A":{"label":"overlay"}}}}),
    );
    let sidecar = super::agents_instance_path(&path);
    let raw_profiles = b3_object(&sidecar)["codingAgentProfiles"].clone();
    let (diagnostics, _capture) = capture_settings_save_diagnostics();
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::SettingsSaveStage::TempCreate));
    });
    let loaded = super::load_settings_from_path(&path);
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(loaded
        .local_overlay_state
        .owns_top_level("codingAgentProfiles"));
    assert!(
        !diagnostics.borrow().is_empty(),
        "baseline protected legacy save was suppressed"
    );
    assert_eq!(b3_object(&sidecar)["codingAgentProfiles"], raw_profiles);
}

#[test]
fn e2432_o2_ordinary_repair_keeps_baseline_save_cause() {
    let (_temp, path) = hash_activation_seed(true);
    let sidecar = super::agents_instance_path(&path);
    let raw = serde_json::Value::Object(b3_object(&sidecar));
    let projected = super::project_physical_profiles(&raw, true).unwrap();
    let mut canonical = super::canonical_profile_source(&raw, &projected, true).unwrap();
    canonical["codingAgentProfiles"]["profileSlots"] = serde_json::json!({});
    b3_write(&sidecar, &canonical);
    assert!(!super::actionable_profile_legacy_work(&canonical, false).unwrap());
    let (diagnostics, _capture) = capture_settings_save_diagnostics();
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::SettingsSaveStage::TempCreate));
    });
    let loaded = super::load_settings_from_path(&path);
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(loaded.coding_agent_profiles.profile_slots.contains_key("A"));
    assert!(
        !diagnostics.borrow().is_empty(),
        "ordinary repair publication was suppressed"
    );
}

#[test]
fn e2432_o2_sidecar_rejections_keep_every_source_unchanged() {
    for case in ["conflict", "quarantine", "duplicate"] {
        let (temp, path) = hash_activation_seed(true);
        let sidecar = super::agents_instance_path(&path);
        let mut raw = serde_json::Value::Object(b3_object(&sidecar));
        raw["codingAgentProfiles"]["letters"] = serde_json::json!([null]);
        if case == "conflict" {
            raw["agents"]
                .as_array_mut()
                .unwrap()
                .push(b3_agent("beta", "B"));
            raw["codingAgentProfiles"]["profilesByAgent"]["beta"] =
                serde_json::json!({"A":{"notes":"different complete set"}});
        } else if case == "quarantine" {
            raw["codingAgentProfiles"]["legacyProfileOrphans"] = serde_json::Value::Null;
        }
        let mut bytes = raw.to_string();
        if case == "duplicate" {
            bytes.pop();
            bytes.push_str(",\"unknown\":1,\"unknown\":1}");
        }
        std::fs::write(&sidecar, bytes).unwrap();
        let before = hash_activation_files(temp.path());
        let loaded = super::load_settings_from_path(&path);
        assert_eq!(hash_activation_files(temp.path()), before, "startup {case}");
        assert!(
            super::save_settings_to_path_preserving_project_paths(&loaded, &path).is_err(),
            "save {case}"
        );
        assert_eq!(hash_activation_files(temp.path()), before, "save {case}");
    }
}

fn assert_selected_project_decode_fallback_case(startup: bool, unreadable: bool) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let project = temp.path().join("A");
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    let a = project.to_string_lossy().replace('\\', "/");
    // Valid source object, unrelated typed field cannot decode.
    let source = serde_json::to_vec(&serde_json::json!({
        "rootToken":42,"projectPaths":[a],"projectPath":a
    }))
    .unwrap();
    std::fs::write(&path, &source).unwrap();
    let overlay_path = b3_overlay_path(temp.path());
    let overlay = br####"{"roomNumberMask":"###"}"####;
    std::fs::write(&overlay_path, overlay).unwrap();
    let agents_path = super::agents_instance_path(&path);
    let defaults = serde_json::to_value(super::AppSettings::default()).unwrap();
    let agents = if unreadable {
        b"{broken agents".to_vec()
    } else {
        serde_json::to_vec(&serde_json::json!({
            "agents":defaults["agents"],
            "codingAgentProfiles":defaults["codingAgentProfiles"]
        }))
        .unwrap()
    };
    std::fs::write(&agents_path, &agents).unwrap();
    let (settings, legacy_profiles, original) = if startup {
        super::read_settings_for_startup(&path)
    } else {
        super::capture_project_settings(&path, false).unwrap()
    };
    let expected = crate::config::projects::display_canonical(
        std::fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .as_ref(),
    );
    assert_eq!(settings.project_paths, vec![expected.clone()]);
    assert_eq!(settings.project_path, Some(expected));
    assert!(settings.project_paths_persistence_error.is_none());
    assert_eq!(settings.room_number_mask, "###");
    assert_eq!(settings.root_token, None);
    assert!(!legacy_profiles);
    assert!(original.is_none());
    assert_eq!(
        settings.agents_layer.unreadable_path().is_some(),
        unreadable
    );
    assert_eq!(
        super::agents_layer_unreadable_for_dir(temp.path()),
        unreadable
    );
    assert_eq!(
        super::try_with_agents_layer_publication_state(temp.path(), |flag| flag).unwrap(),
        unreadable
    );
    assert_eq!(std::fs::read(&agents_path).unwrap(), agents);
    assert_eq!(std::fs::read(&overlay_path).unwrap(), overlay);
    let sidecar = super::project_paths_path(&path);
    if startup {
        assert!(super::raw_project_projection(&b3_object(&path)).is_empty());
    } else {
        assert_eq!(std::fs::read(&path).unwrap(), source);
        assert!(!sidecar.exists());
        assert!(!temp
            .path()
            .join(super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME)
            .exists());
    }
    assert_selected_project_decode_fallback_preserve(
        &settings,
        &path,
        &agents_path,
        &agents,
        &overlay_path,
        overlay,
    );
}

fn assert_selected_project_decode_fallback_preserve(
    settings: &super::AppSettings,
    path: &std::path::Path,
    agents_path: &std::path::Path,
    agents: &[u8],
    overlay_path: &std::path::Path,
    overlay: &[u8],
) {
    let sidecar = super::project_paths_path(path);
    let before_sidecar = std::fs::read(&sidecar).ok();
    match super::save_settings_to_path_preserving_project_paths_typed(settings, path) {
        Ok(_) => {
            assert_eq!(std::fs::read(agents_path).unwrap(), agents);
            assert!(super::raw_project_projection(&b3_object(path)).is_empty());
        }
        Err(error) => {
            eprintln!("P1 Preserve explicitly refused: {error:?}");
            assert_eq!(std::fs::read(agents_path).unwrap(), agents);
        }
    }
    if let Some(bytes) = before_sidecar {
        assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
    }
    assert_eq!(std::fs::read(overlay_path).unwrap(), overlay);
}

#[test]
fn unavailable_project_fallback_retains_agents_publication_mark() {
    for unreadable in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let source = br#"{"rootToken":42}"#;
        let destination = b"{broken project source";
        let agents = if unreadable {
            b"{broken".as_slice()
        } else {
            b"{}".as_slice()
        };
        std::fs::write(&path, source).unwrap();
        std::fs::write(super::project_paths_path(&path), destination).unwrap();
        std::fs::write(super::agents_instance_path(&path), agents).unwrap();
        for startup in [false, true] {
            let (settings, _, _) = if startup {
                super::read_settings_for_startup(&path)
            } else {
                super::capture_project_settings(&path, false).unwrap()
            };
            assert!(settings.project_paths_persistence_error.is_some());
            assert!(settings.project_paths.is_empty());
            assert!(settings.project_path.is_none());
            assert_eq!(
                settings.agents_layer.unreadable_path().is_some(),
                unreadable
            );
            assert_eq!(
                super::try_with_agents_layer_publication_state(temp.path(), |flag| flag).unwrap(),
                unreadable
            );
            assert_eq!(std::fs::read(&path).unwrap(), source);
            assert_eq!(
                std::fs::read(super::project_paths_path(&path)).unwrap(),
                destination
            );
            assert_eq!(
                std::fs::read(super::agents_instance_path(&path)).unwrap(),
                agents
            );
            assert!(settings.root_token.is_none());
        }
    }
}

fn migration_wait_marker(root: &std::path::Path, name: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join(name).is_file() {
        assert!(
            std::time::Instant::now() < deadline,
            "barrier timed out: {name}; diagnostics {}",
            root.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

struct MigrationChild {
    child: std::process::Child,
    root: std::path::PathBuf,
}

impl MigrationChild {
    fn finish(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                std::fs::write(self.root.join("child-exit.txt"), status.to_string()).unwrap();
                assert!(
                    status.success(),
                    "child failed; diagnostics {}",
                    self.root.display()
                );
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child exit timed out; diagnostics {}",
                self.root.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for MigrationChild {
    fn drop(&mut self) {
        // Cleanup cannot replace a primary assertion/error with a second panic.
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            _ => {
                let killed = self.child.kill();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    let reaped = self.child.try_wait();
                    let done = matches!(&reaped, Ok(Some(_)));
                    if done || std::time::Instant::now() >= deadline {
                        let _ = std::fs::write(
                            self.root.join("child-cleanup.txt"),
                            format!(
                                "kill={killed:?}; reap={reaped:?}; deadline_expired={}",
                                !done
                            ),
                        );
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }
}

fn launch_migration_child(root: &std::path::Path, role: &str) -> MigrationChild {
    launch_migration_child_with_path(root, role, &root.join("settings.json"))
}

fn launch_migration_child_with_path(
    root: &std::path::Path,
    role: &str,
    path: &std::path::Path,
) -> MigrationChild {
    let log = std::fs::File::create(root.join("child.log")).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "config::settings::tests::project_migration_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("AC_P02_CHILD_ROOT", root)
        .env("AC_P02_CHILD_ROLE", role)
        .env("AC_P03_DATA_PATH", path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(log.try_clone().unwrap()))
        .stdout(std::process::Stdio::from(log))
        .spawn()
        .unwrap();
    MigrationChild {
        child,
        root: root.to_path_buf(),
    }
}

#[test]
fn project_migration_child() {
    let Some(root) = std::env::var_os("AC_P02_CHILD_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    assert!(root.is_absolute());
    assert_eq!(
        std::fs::read(root.join("isolated-fixture.marker")).unwrap(),
        b"P02 isolated process fixture"
    );
    let role = std::env::var("AC_P02_CHILD_ROLE").unwrap();
    if role.starts_with("race-") {
        project_archive_preserve_child(&root, &role);
        return;
    }
    assert!(matches!(
        role.as_str(),
        "hold" | "crash-backup-sync" | "crash-backup-published" | "crash-destination"
    ));
    std::fs::write(root.join("ready"), std::process::id().to_string()).unwrap();
    migration_wait_marker(&root, "begin");
    let transaction = migration_transaction(&root.join("settings.json"));
    std::fs::write(root.join("acquired"), b"lock owned").unwrap();
    super::ensure_project_paths_migrated_with(&transaction, &mut |stage, _| {
        let stop = match role.as_str() {
            "hold" | "crash-destination" => stage == "destination_published",
            "crash-backup-sync" => stage == "backup_sync",
            "crash-backup-published" => stage == "backup_published",
            _ => unreachable!(),
        };
        if stop {
            std::fs::write(root.join("publication"), stage.as_bytes()).unwrap();
            migration_wait_marker(&root, "attempt");
            migration_wait_marker(&root, "release-cleanup");
        }
        Ok(())
    })
    .unwrap();
    std::fs::write(
        root.join("cleanup"),
        b"source cleaned with lock still owned",
    )
    .unwrap();
    migration_wait_marker(&root, "release-lock");
    drop(transaction);
}

// Separate control roots share one data path and actual native settings lock.
fn project_archive_preserve_child(root: &std::path::Path, role: &str) {
    let path = std::path::PathBuf::from(std::env::var_os("AC_P03_DATA_PATH").unwrap());
    assert!(path.is_absolute());
    let data = path.parent().unwrap();
    let draft: super::AppSettings =
        serde_json::from_slice(&std::fs::read(data.join("stale-draft.json")).unwrap()).unwrap();
    let target = std::fs::read_to_string(data.join("target.txt")).unwrap();
    let holder = role.ends_with("-holder");
    let preserve = role.starts_with("race-preserve-");
    let unarchive = role.starts_with("race-unarchive-");
    assert!(preserve || unarchive || role.starts_with("race-archive-"));
    std::fs::write(root.join("ready"), b"ready").unwrap();
    migration_wait_marker(root, "begin");
    if preserve && holder {
        let control = root.to_path_buf();
        super::SETTINGS_PRESERVE_LOCK_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| {
                std::fs::write(control.join("acquired"), b"Preserve lock owned").unwrap();
                migration_wait_marker(&control, "release");
            }));
        });
    }
    let operation = || -> Result<(), String> {
        if preserve {
            super::save_settings_to_path_preserving_project_paths_typed(&draft, &path)
                .map(|_| ())
                .map_err(|error| {
                    super::report_settings_save_error(
                        error,
                        super::SettingsSaveReportSurface::GeneralSettings,
                    )
                })
        } else {
            let mut transaction = super::begin_project_paths_transaction_at_path(&draft, &path)?;
            if holder {
                std::fs::write(root.join("acquired"), b"project token owned").unwrap();
                migration_wait_marker(root, "release");
            }
            let mut candidate = transaction.fresh_settings().clone();
            transaction.refresh_raw_projects(&mut candidate)?;
            if unarchive {
                assert!(super::projects::unarchive_project_path(
                    &mut candidate,
                    &target
                ));
            } else {
                assert!(super::projects::archive_project_path(
                    &mut candidate,
                    &target
                ));
            }
            super::resync_project_state_from_runtime(&mut candidate);
            transaction.commit_project_groups(&candidate, true, true)?;
            Ok(())
        }
    };
    if !holder {
        let source = std::fs::read(&path).unwrap();
        let sidecar = std::fs::read(super::project_paths_path(&path)).unwrap();
        let started = std::time::Instant::now();
        let refused = operation().expect_err("held native lock must refuse competing writer");
        assert!(started.elapsed() >= std::time::Duration::from_secs(2));
        assert_eq!(std::fs::read(&path).unwrap(), source);
        assert_eq!(
            std::fs::read(super::project_paths_path(&path)).unwrap(),
            sidecar
        );
        std::fs::write(root.join("refused"), refused).unwrap();
        migration_wait_marker(root, "retry");
    }
    let before = std::fs::read(super::project_paths_path(&path)).unwrap();
    operation().unwrap();
    if preserve {
        assert_eq!(
            std::fs::read(super::project_paths_path(&path)).unwrap(),
            before
        );
    }
    std::fs::write(root.join("done"), b"writer succeeded").unwrap();
}

#[test]
fn project_archive_unarchive_processes_race_typed_preserve_both_orders() {
    for unarchive in [false, true] {
        for preserve_first in [false, true] {
            project_archive_preserve_case(unarchive, preserve_first);
        }
    }
}

fn project_archive_preserve_case(unarchive: bool, preserve_first: bool) {
    let root = tempfile::tempdir().unwrap().keep();
    let path = root.join("settings.json");
    let a = root.join("A").to_string_lossy().replace('\\', "/");
    let b = root.join("B").to_string_lossy().replace('\\', "/");
    // Missing raw registrations exercise retained companion pairs.
    assert!(!root.join("A").exists() && !root.join("B").exists());
    let (active, archived, active_rel, archived_rel) = if unarchive {
        (
            serde_json::json!([b]),
            serde_json::json!([a]),
            serde_json::json!(["missing-companion-B"]),
            serde_json::json!(["missing-companion-A"]),
        )
    } else {
        (
            serde_json::json!([a, b]),
            serde_json::json!([]),
            serde_json::json!(["missing-companion-A", "missing-companion-B"]),
            serde_json::json!([]),
        )
    };
    let mut managed = serde_json::to_value(super::AppSettings {
        room_number_mask: "##".into(),
        root_token: Some("fixture".into()),
        ..super::AppSettings::default()
    })
    .unwrap();
    super::replace_project_projection(managed.as_object_mut().unwrap(), &serde_json::Map::new());
    std::fs::write(&path, serde_json::to_vec(&managed).unwrap()).unwrap();
    std::fs::write(
        super::project_paths_path(&path),
        serde_json::to_vec(&serde_json::json!({
            "projectPaths":active,"projectPathsRelativeToInstance":active_rel,
            "archivedProjectPaths":archived,
            "archivedProjectPathsRelativeToInstance":archived_rel,
            "projectPath": if unarchive { &b } else { &a },
            "projectPathRelativeToInstance":null,"unknownSidecar":{"sentinel":29}
        }))
        .unwrap(),
    )
    .unwrap();
    let draft = super::AppSettings {
        project_paths: vec![a.clone(), b.clone()],
        project_path: Some(a.clone()),
        archived_project_paths: vec![b.clone()],
        room_number_mask: "##".into(),
        root_token: Some("fixture".into()),
        ..super::AppSettings::default()
    };
    std::fs::write(
        root.join("stale-draft.json"),
        serde_json::to_vec(&draft).unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("target.txt"), &a).unwrap();
    let holder_root = root.join("holder");
    let contender_root = root.join("contender");
    for control in [&holder_root, &contender_root] {
        std::fs::create_dir(control).unwrap();
        std::fs::write(
            control.join("isolated-fixture.marker"),
            b"P02 isolated process fixture",
        )
        .unwrap();
    }
    let mutation = if unarchive {
        "race-unarchive"
    } else {
        "race-archive"
    };
    let (holder_role, contender_role) = if preserve_first {
        (
            "race-preserve-holder".to_string(),
            format!("{mutation}-contender"),
        )
    } else {
        (
            format!("{mutation}-holder"),
            "race-preserve-contender".to_string(),
        )
    };
    let mut holder = launch_migration_child_with_path(&holder_root, &holder_role, &path);
    let mut contender = launch_migration_child_with_path(&contender_root, &contender_role, &path);
    migration_wait_marker(&holder_root, "ready");
    migration_wait_marker(&contender_root, "ready");
    std::fs::write(holder_root.join("begin"), b"begin").unwrap();
    migration_wait_marker(&holder_root, "acquired");
    let before_settings = std::fs::read(&path).unwrap();
    let before_sidecar = std::fs::read(super::project_paths_path(&path)).unwrap();
    std::fs::write(contender_root.join("begin"), b"begin").unwrap();
    migration_wait_marker(&contender_root, "refused");
    assert_eq!(std::fs::read(&path).unwrap(), before_settings);
    assert_eq!(
        std::fs::read(super::project_paths_path(&path)).unwrap(),
        before_sidecar
    );
    std::fs::write(holder_root.join("release"), b"release").unwrap();
    holder.finish();
    std::fs::write(contender_root.join("retry"), b"retry").unwrap();
    contender.finish();
    let raw = b3_object(&super::project_paths_path(&path));
    assert_eq!(
        raw["projectPaths"],
        if unarchive {
            serde_json::json!([b, a])
        } else {
            serde_json::json!([b])
        }
    );
    assert_eq!(
        raw["archivedProjectPaths"],
        if unarchive {
            serde_json::json!([])
        } else {
            serde_json::json!([a])
        }
    );
    assert_eq!(raw["projectPath"], serde_json::json!(b));
    assert_eq!(raw["projectPathRelativeToInstance"], "missing-companion-B");
    assert_eq!(
        raw["projectPathsRelativeToInstance"],
        if unarchive {
            serde_json::json!(["missing-companion-B", null])
        } else {
            serde_json::json!(["missing-companion-B"])
        }
    );
    assert_eq!(
        raw["archivedProjectPathsRelativeToInstance"],
        if unarchive {
            serde_json::json!([])
        } else {
            serde_json::json!([null])
        }
    );
    assert_eq!(raw["unknownSidecar"], serde_json::json!({"sentinel":29}));
    let managed = b3_object(&path);
    assert_eq!(managed["roomNumberMask"], "##");
    assert!(super::raw_project_projection(&managed).is_empty());
    eprintln!(
        "P03 unarchive={unarchive}/Preserve-first={preserve_first} diagnostics {}",
        root.display()
    );
}

// Dormant writer control: same P01 token, migration before the requested
// mutation. Production entry points remain deliberately legacy in P02.
fn migration_fixture_writer(
    path: &std::path::Path,
    variant: &str,
) -> Result<(), super::SettingsSaveError> {
    let lock = super::SettingsFileLock::acquire(path, std::time::Duration::from_secs(2))?;
    let transaction = super::ProjectPathsWriteTransaction {
        _lock: lock,
        path: path.to_path_buf(),
        fresh: super::AppSettings::default(),
        revalidated_after_failure: false,
    };
    super::ensure_project_paths_migrated_locked(&transaction)?;
    let target = if variant == "Reconcile" {
        super::project_paths_path(path)
    } else {
        path.to_path_buf()
    };
    let mut object = super::read_raw_project_source(&target)?.unwrap().object;
    match variant {
        "Preserve" => {
            object.insert("requestedPreserve".into(), serde_json::json!(true));
        }
        "Reconcile" => {
            object.insert(
                "projectPaths".into(),
                serde_json::json!(["C:/requested-registration"]),
            );
        }
        "CAS" => {
            assert_eq!(object["terminalSnapshotsEnabled"], false);
            object.insert("terminalSnapshotsEnabled".into(), serde_json::json!(true));
        }
        _ => panic!("unknown fixture writer"),
    }
    super::write_value_atomic(&serde_json::Value::Object(object), &target)?;
    Ok(())
}

#[test]
fn project_migration_process_timeout_retry_preserve_reconcile_cas() {
    use sha2::{Digest, Sha256};
    for variant in ["Preserve", "Reconcile", "CAS"] {
        let (temp, path, source) = migration_fixture(
            &serde_json::json!({"projectPaths":[],"terminalSnapshotsEnabled":false,"unrelated":9}),
        );
        let root = temp.keep(); // Retain child diagnostics, including failure/crash evidence.
        std::fs::write(
            root.join("isolated-fixture.marker"),
            b"P02 isolated process fixture",
        )
        .unwrap();
        std::fs::write(
            root.join("source-sha256.txt"),
            format!("{:x}", Sha256::digest(&source)),
        )
        .unwrap();
        let mut child = launch_migration_child(&root, "hold");
        migration_wait_marker(&root, "ready");
        std::fs::write(root.join("begin"), b"begin").unwrap();
        migration_wait_marker(&root, "acquired");
        migration_wait_marker(&root, "publication");
        let destination = std::fs::read(super::project_paths_path(&path)).unwrap();
        std::fs::write(root.join("attempt"), variant).unwrap();
        let current = super::AppSettings::default();
        // Real production lock entry points must also reject the request.
        match variant {
            "Preserve" => assert!(super::save_settings_value(
                &current,
                &path,
                super::ProjectWriteMode::Preserve
            )
            .is_err()),
            "Reconcile" => {
                assert!(super::begin_project_paths_transaction_at_path(&current, &path).is_err())
            }
            "CAS" => assert!(super::compare_and_set_terminal_snapshots_enabled_at_path(
                &current, &path, false, true
            )
            .is_err()),
            _ => unreachable!(),
        }
        let start = std::time::Instant::now();
        let error = migration_fixture_writer(&path, variant).unwrap_err();
        assert!(matches!(
            error.cause,
            super::SettingsSaveCause::Semantic(super::SettingsSaveReason::LockTimedOut)
        ));
        assert!(start.elapsed() >= std::time::Duration::from_secs(2));
        std::fs::write(
            root.join("timeout-ms.txt"),
            start.elapsed().as_millis().to_string(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), source);
        assert_eq!(
            std::fs::read(super::project_paths_path(&path)).unwrap(),
            destination
        );
        assert!(!root.join("cleanup").exists());
        std::fs::write(root.join("release-cleanup"), b"release").unwrap();
        migration_wait_marker(&root, "cleanup");
        assert!(super::raw_project_projection(
            &super::read_raw_project_source(&path)
                .unwrap()
                .unwrap()
                .object
        )
        .is_empty());
        // Cleanup completion does not release ownership early.
        let early = super::SettingsFileLock::acquire(&path, std::time::Duration::from_millis(30));
        assert!(early.is_err());
        std::fs::write(root.join("release-lock"), b"release").unwrap();
        child.finish();
        migration_fixture_writer(&path, variant).unwrap();
        let settings = super::read_raw_project_source(&path)
            .unwrap()
            .unwrap()
            .object;
        assert_eq!(settings["unrelated"], 9);
        assert!(super::raw_project_projection(&settings).is_empty());
        match variant {
            "Preserve" => assert_eq!(settings["requestedPreserve"], true),
            "CAS" => assert_eq!(settings["terminalSnapshotsEnabled"], true),
            "Reconcile" => assert_eq!(
                super::read_raw_project_source(&super::project_paths_path(&path))
                    .unwrap()
                    .unwrap()
                    .object["projectPaths"],
                serde_json::json!(["C:/requested-registration"])
            ),
            _ => unreachable!(),
        }
        eprintln!("P02 {variant} process diagnostics {}", root.display());
    }
}

#[test]
fn project_migration_process_crash_reopen_between_publications() {
    for role in [
        "crash-backup-sync",
        "crash-backup-published",
        "crash-destination",
    ] {
        let (temp, path, bytes) =
            migration_fixture(&serde_json::json!({"projectPaths":["C:/gone"],"unknown":9}));
        let root = temp.keep();
        std::fs::write(
            root.join("isolated-fixture.marker"),
            b"P02 isolated process fixture",
        )
        .unwrap();
        let child = launch_migration_child(&root, role);
        migration_wait_marker(&root, "ready");
        std::fs::write(root.join("begin"), b"begin").unwrap();
        migration_wait_marker(&root, "publication");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let orphans: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| {
                let p = entry.unwrap().path();
                p.extension()
                    .is_some_and(|extension| extension == "tmp")
                    .then(|| (p.clone(), std::fs::read(p).unwrap()))
            })
            .collect();
        let backup = root.join(super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
        if role == "crash-backup-sync" {
            assert!(!backup.exists());
            assert_eq!(orphans.len(), 1);
        }
        // Drop kills and reaps the child while it holds the actual OS lock.
        drop(child);
        let cleanup = std::fs::read_to_string(root.join("child-cleanup.txt")).unwrap();
        assert!(cleanup.contains("deadline_expired=false"), "{cleanup}");
        super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), bytes);
        for (orphan, original) in orphans {
            assert_eq!(std::fs::read(orphan).unwrap(), original);
        }
        assert!(super::raw_project_projection(
            &super::read_raw_project_source(&path)
                .unwrap()
                .unwrap()
                .object
        )
        .is_empty());
        eprintln!("P02 {role} crash diagnostics {}", root.display());
    }
}

#[test]
fn project_migration_readback_and_cleanup_order_oracle() {
    for unsafe_destination in [false, true] {
        let (_temp, path, bytes) =
            migration_fixture(&serde_json::json!({"projectPaths":["C:/gone"],"unknown":9}));
        let destination = super::project_paths_path(&path);
        let transaction = migration_transaction(&path);
        let result =
            super::ensure_project_paths_migrated_with(&transaction, &mut |stage, target| {
                if stage == "destination_published" {
                    if unsafe_destination {
                        std::fs::remove_file(target).unwrap();
                        std::fs::create_dir(target).unwrap();
                    } else {
                        std::fs::write(target, b"{\"projectPaths\":[\"wrong\"]}").unwrap();
                    }
                }
                Ok(())
            });
        assert!(
            result.is_err(),
            "unsafe publication must never authorize cleanup"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "readback/order oracle: source loss"
        );
        if unsafe_destination {
            std::fs::remove_dir(&destination).unwrap();
        }
        std::fs::write(
            &destination,
            b"{\"projectPaths\":[\"C:/gone\"],\"unknownTarget\":17}",
        )
        .unwrap();
        super::ensure_project_paths_migrated_locked(&transaction).unwrap();
        assert_eq!(
            super::read_raw_project_source(&destination)
                .unwrap()
                .unwrap()
                .object["unknownTarget"],
            17
        );
        assert_eq!(
            super::read_raw_project_source(&path)
                .unwrap()
                .unwrap()
                .object["unknown"],
            9
        );
    }
}

fn migration_transaction(path: &std::path::Path) -> super::ProjectPathsWriteTransaction {
    super::ProjectPathsWriteTransaction {
        _lock: super::SettingsFileLock::acquire(path, std::time::Duration::from_secs(2)).unwrap(),
        path: path.to_path_buf(),
        fresh: super::AppSettings::default(),
        revalidated_after_failure: false,
    }
}

fn migration_fixture(raw: &serde_json::Value) -> (tempfile::TempDir, std::path::PathBuf, Vec<u8>) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let bytes = serde_json::to_vec_pretty(raw).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    (temp, path, bytes)
}

#[test]
fn project_migration_raw_matrix_and_noop_reopen() {
    let cases = [
        serde_json::json!({}),
        serde_json::json!({"projectPath":"C:/singular-only"}),
        serde_json::json!({"projectPaths":null,"projectPath":null,"archivedProjectPaths":[]}),
        serde_json::json!({"projectPaths":["C:/gone","D:/missing","//server/share/missing"],"projectPathsRelativeToInstance":["../gone",null,"../conflict"],"projectPath":"C:/gone","projectPathRelativeToInstance":"../gone","archivedProjectPaths":["C:/archived-missing"],"archivedProjectPathsRelativeToInstance":["../archived-missing"]}),
        serde_json::json!({"projectPaths":42,"projectPathsRelativeToInstance":{},"archivedProjectPaths":[null,7],"projectPath":true}),
    ];
    for mut raw in cases {
        raw["unrelatedUnknown"] = serde_json::json!({"nested":[null,42,"unchanged"]});
        let (temp, path, bytes) = migration_fixture(&raw);
        let expected = super::raw_project_projection(raw.as_object().unwrap());
        assert_eq!(
            super::select_project_source_at_path(&path).unwrap(),
            expected
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(
            !super::project_paths_path(&path).exists(),
            "read-only must not migrate"
        );
        let transaction = migration_transaction(&path);
        super::ensure_project_paths_migrated_locked(&transaction).unwrap();
        let destination = super::read_raw_project_source(&super::project_paths_path(&path))
            .unwrap()
            .unwrap();
        assert_eq!(destination.object, expected);
        let cleaned = super::read_raw_project_source(&path).unwrap().unwrap();
        assert!(super::raw_project_projection(&cleaned.object).is_empty());
        assert_eq!(cleaned.object["unrelatedUnknown"], raw["unrelatedUnknown"]);
        let state = super::decode_project_state(
            &destination.object,
            Some(temp.path()),
            &super::projects::FsCandidateResolver,
        );
        if raw["projectPaths"] == 42 {
            assert!(state.has_structural());
        }
        // Windows rejects reads through another handle while the lock is
        // held. Snapshot every file only between migration transactions.
        drop(transaction);
        let before: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| {
                let p = entry.unwrap().path();
                (
                    p.clone(),
                    std::fs::read(&p).unwrap(),
                    std::fs::metadata(&p).unwrap().modified().unwrap(),
                )
            })
            .collect();
        let transaction = migration_transaction(&path);
        super::ensure_project_paths_migrated_locked(&transaction).unwrap();
        drop(transaction);
        assert_eq!(
            std::fs::read_dir(temp.path()).unwrap().count(),
            before.len()
        );
        for (p, bytes, modified) in before {
            assert_eq!(std::fs::read(&p).unwrap(), bytes);
            assert_eq!(std::fs::metadata(p).unwrap().modified().unwrap(), modified);
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    assert!(super::select_project_source_at_path(&path)
        .unwrap()
        .is_empty());
    super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
    assert_eq!(
        std::fs::read(super::project_paths_path(&path)).unwrap(),
        b"{}"
    );
    assert!(!path.exists());
}

#[test]
fn project_migration_rejects_invalid_sources_and_conflicts() {
    for bad in [
        b"[1]".as_slice(),
        b"{",
        b"{\"projectPaths\":[],\"projectPaths\":[1]}",
    ] {
        for bad_destination in [false, true] {
            let (_temp, path, bytes) = migration_fixture(&serde_json::json!({"projectPaths":[]}));
            let destination = super::project_paths_path(&path);
            let bad_path = if bad_destination { &destination } else { &path };
            std::fs::write(bad_path, bad).unwrap();
            let transaction = migration_transaction(&path);
            assert!(super::ensure_project_paths_migrated_locked(&transaction).is_err());
            assert_eq!(std::fs::read(bad_path).unwrap(), bad);
            if bad_destination {
                let error = super::select_project_source_locked(&transaction).unwrap_err();
                assert!(!error
                    .legacy_outward
                    .into_general_string()
                    .contains("settings.json exists but could not be parsed"));
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            } else {
                assert!(!destination.exists());
                // A present valid destination cannot hide an invalid source:
                // conflict/remnant evaluation requires both readable objects.
                std::fs::write(&destination, b"{\"projectPaths\":[]}").unwrap();
                assert!(super::ensure_project_paths_migrated_locked(&transaction).is_err());
                assert_eq!(std::fs::read(&path).unwrap(), bad);
                assert_eq!(
                    std::fs::read(&destination).unwrap(),
                    b"{\"projectPaths\":[]}"
                );
            }
        }
    }
    let (_temp, path, bytes) = migration_fixture(&serde_json::json!({"projectPath":null}));
    let destination = super::project_paths_path(&path);
    std::fs::write(&destination, b"{}").unwrap();
    let transaction = migration_transaction(&path);
    let error = super::ensure_project_paths_migrated_locked(&transaction).unwrap_err();
    assert!(super::report_settings_save_error(
        error,
        super::SettingsSaveReportSurface::GeneralSettings
    )
    .contains("project_paths_conflict"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(std::fs::read(&destination).unwrap(), b"{}");
    std::fs::remove_file(&destination).unwrap();
    std::fs::create_dir(&destination).unwrap();
    assert!(super::ensure_project_paths_migrated_locked(&transaction).is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn project_transaction_refreshes_after_barrier_controlled_second_writer() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = temp.path().join("A");
    let b = temp.path().join("B");
    for project in [&a, &b] {
        std::fs::create_dir_all(project.join(".ac")).unwrap();
    }
    let raw = serde_json::json!({"defaultShell": "test-shell", "defaultShellArgs": [], "agents": [], "projectPaths": [a], "projectPath": a});
    std::fs::write(&path, raw.to_string()).unwrap();
    let mut stale = super::AppSettings::default();
    super::refresh_and_decode_project_paths_from_path(&mut stale, &path).unwrap();
    // Also exercise a cached object that claims no pending repair.
    stale.project_path_state = std::sync::Arc::default();
    let lock = super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let worker_barrier = barrier.clone();
    let worker_path = path.clone();
    let worker = std::thread::spawn(move || {
        worker_barrier.wait();
        super::repair_fresh_project_paths_at_path(&mut stale, &worker_path).unwrap();
        stale
    });
    barrier.wait();
    // This writer owns the file lock before the repair can acquire it.
    let fresh = serde_json::json!({"defaultShell": "test-shell", "defaultShellArgs": [], "agents": [], "projectPaths": [a, b], "projectPath": a});
    super::write_value_atomic(&fresh, &path).unwrap();
    drop(lock);
    let repaired = worker.join().unwrap();
    assert_eq!(repaired.project_paths.len(), 2);
    let disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(super::project_paths_path(&path)).unwrap()).unwrap();
    assert_eq!(disk["projectPaths"].as_array().unwrap().len(), 2);
    assert!(repaired.project_paths_persistence_error.is_none());
}

#[test]
fn project_transaction_timeout_preserves_bytes_and_live_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let bytes = b"{\"defaultShell\":\"test-shell\",\"defaultShellArgs\":[],\"agents\":[],\"projectPaths\":[]}";
    std::fs::write(&path, bytes).unwrap();
    let _lock = super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
    let mut current = super::AppSettings {
        project_paths: vec!["cached".into()],
        ..super::AppSettings::default()
    };
    let result = super::repair_fresh_project_paths_at_path(&mut current, &path);
    assert!(matches!(
        result,
        Err((super::ProjectPathsRepairStage::Read, _))
    ));
    assert_eq!(current.project_paths, vec!["cached"]);
    assert!(super::project_state_has_structural(&current));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn project_fresh_repair_retains_missing_raw_registration() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let active = temp.path().join("active");
    let missing = temp.path().join("missing").to_string_lossy().into_owned();
    std::fs::create_dir_all(active.join(".ac")).unwrap();
    let raw = serde_json::json!({"defaultShell": "test-shell", "defaultShellArgs": [], "agents": [], "projectPaths": [active, missing], "projectPath": active});
    std::fs::write(&path, raw.to_string()).unwrap();
    let mut current = super::AppSettings::default();
    super::repair_fresh_project_paths_at_path(&mut current, &path).unwrap();
    let disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(super::project_paths_path(&path)).unwrap()).unwrap();
    assert_eq!(disk["projectPaths"].as_array().unwrap().len(), 2);
    assert_eq!(disk["projectPaths"][1], missing);
    assert_eq!(current.project_paths.len(), 1);
}

#[test]
fn project_transaction_keeps_candidate_separate_on_precommit_failure() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, b"{\"defaultShell\":\"test-shell\",\"defaultShellArgs\":[],\"agents\":[],\"projectPaths\":[]}").unwrap();
    let current = super::AppSettings::default();
    let mut transaction = super::begin_project_paths_transaction_at_path(&current, &path).unwrap();
    let mut candidate = transaction.fresh_settings().clone();
    candidate.project_paths = vec!["candidate".into()];
    super::resync_project_state_from_runtime(&mut candidate);
    // Simulate a noncooperating fault, not a second cooperating writer.
    std::fs::write(&path, b"invalid").unwrap();
    assert!(transaction
        .commit_project_groups(&candidate, true, true)
        .is_err());
    assert!(transaction.fresh_settings().project_paths.is_empty());
    assert!(current.project_paths.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), b"invalid");
}

#[test]
fn project_transaction_uncertain_commit_adopts_only_revalidated_disk() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, b"{\"defaultShell\":\"test-shell\",\"defaultShellArgs\":[],\"agents\":[],\"projectPaths\":[]}").unwrap();
    let mut transaction =
        super::begin_project_paths_transaction_at_path(&super::AppSettings::default(), &path)
            .unwrap();
    let error = || {
        super::SettingsSaveError::semantic(
            super::SettingsSaveStage::PostWriteRead,
            &path,
            None,
            super::SettingsSaveReason::PostWriteReadRejected,
            super::SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    };
    // Model a completed publication followed by uncertain readback.
    let published = serde_json::json!({"defaultShell": "test-shell", "defaultShellArgs": [], "agents": [], "projectPaths": [], "sidebarStyle": "published"});
    super::write_value_atomic(&published, &path).unwrap();
    assert!(transaction.handle_commit_result(Err(error())).is_err());
    assert_eq!(
        transaction.fresh_settings().sidebar_style,
        super::AppSettings::default().sidebar_style
    );
    assert!(transaction
        .fresh_settings()
        .project_paths_persistence_error
        .is_none());
    assert!(transaction.revalidated_after_failure);
    // Failure to revalidate must block rather than publish guessed state.
    std::fs::write(&path, b"invalid").unwrap();
    assert!(transaction.handle_commit_result(Err(error())).is_err());
    assert!(transaction
        .fresh_settings()
        .project_paths_persistence_error
        .is_some());
    assert_eq!(std::fs::read(&path).unwrap(), b"invalid");
}

#[test]
fn project_fresh_repair_clears_unavailable_only_after_valid_read() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut current = super::AppSettings::default();
    std::fs::write(&path, b"invalid").unwrap();
    assert!(super::repair_fresh_project_paths_at_path(&mut current, &path).is_err());
    assert!(current.project_paths_persistence_error.is_some());
    std::fs::write(
            &path,
            b"{\"defaultShell\":\"test-shell\",\"defaultShellArgs\":[],\"agents\":[],\"projectPaths\":[],\"archivedProjectPaths\":[]}",
        )
        .unwrap();
    super::repair_fresh_project_paths_at_path(&mut current, &path).unwrap();
    assert!(current.project_paths_persistence_error.is_none());
    assert!(current.project_paths.is_empty());
    assert!(!super::project_state_has_structural(&current));
    let mut hidden = current.clone();
    hidden.project_paths_persistence_error = Some("unavailable".into());
    assert_eq!(
        serde_json::to_value(&hidden).unwrap(),
        serde_json::to_value(&current).unwrap()
    );
}

fn assert_no_issue_1330_temp_files(directory: &std::path::Path) {
    let entries = std::fs::read_dir(directory).unwrap();
    for entry in entries {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(
            !(name.starts_with("settings.30.instance.no-git.json.") && name.ends_with(".tmp")),
            "unexpected settings temp file: {name}"
        );
    }
}

/// #2058 - put `settings` on disk in exactly the bytes a production save
/// produces, and return them. The file is absent beforehand, so D3's
/// `disk_read == None` arm skips rotation and no slot exists on return.
fn seed_settings_with_production_bytes(
    path: &std::path::Path,
    settings: &super::AppSettings,
) -> Vec<u8> {
    super::save_settings_to_path_preserving_project_paths(settings, path).unwrap();
    let bytes = std::fs::read(path).unwrap();
    assert!(
        !path
            .with_file_name("settings.30.instance.no-git.backup.1.json")
            .exists(),
        "seeding must not rotate"
    );
    bytes
}

fn assert_no_backup_slots(path: &std::path::Path) {
    for slot in 1..=super::SETTINGS_BACKUP_KEEP {
        assert!(
            !path
                .with_file_name(format!("settings.30.instance.no-git.backup.{slot}.json"))
                .exists(),
            "settings.30.instance.no-git.backup.{slot}.json must not exist"
        );
    }
}

#[test]
fn issue_2058_save_archives_the_replaced_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::AppSettings::default()
    };
    let seed = seed_settings_with_production_bytes(&path, &a);
    let b = super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::AppSettings::default()
    };
    super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        seed,
        "slot 1 holds the replaced bytes A"
    );
    assert_ne!(
        std::fs::read(&path).unwrap(),
        seed,
        "settings.json holds the new bytes"
    );
    assert!(
        !path
            .with_file_name("settings.30.instance.no-git.backup.2.json")
            .exists(),
        "slot 2 must not exist after a single rotation"
    );
}

#[test]
fn issue_2058_first_save_with_no_existing_file_writes_no_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::AppSettings::default()
    };
    super::save_settings_to_path_preserving_project_paths(&a, &path).unwrap();
    assert_no_backup_slots(&path);
    // Positive control: a second save with a different value creates slot 1.
    let first_bytes = std::fs::read(&path).unwrap();
    let b = super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::AppSettings::default()
    };
    super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        first_bytes,
        "positive control: slot 1 holds the first save's bytes"
    );
}

#[test]
fn issue_2058_identical_save_does_not_rotate() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::AppSettings::default()
    };
    let seed = seed_settings_with_production_bytes(&path, &a);
    // Save the same settings value again: bytes equal, D3 skips rotation.
    super::save_settings_to_path_preserving_project_paths(&a, &path).unwrap();
    assert_no_backup_slots(&path);
    // Positive control: a third save with a changed value creates slot 1.
    let c = super::AppSettings {
        root_token: Some("c".to_string()),
        ..super::AppSettings::default()
    };
    super::save_settings_to_path_preserving_project_paths(&c, &path).unwrap();
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        seed,
        "positive control: slot 1 holds exactly A"
    );
}

#[test]
fn issue_2058_keeps_five_generations_and_drops_the_oldest() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut written: Vec<Vec<u8>> = Vec::new();
    for n in 0..7u32 {
        let settings = super::AppSettings {
            root_token: Some(format!("v{n}")),
            ..super::AppSettings::default()
        };
        super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
        written.push(std::fs::read(&path).unwrap());
    }
    // Slots 1..=5 hold the written bytes of v5, v4, v3, v2, v1.
    for (slot, n) in (1..=5u32).zip((1..=5u32).rev()) {
        assert_eq!(
            std::fs::read(super::settings_backup_path(&path, slot)).unwrap(),
            written[n as usize],
            "slot {slot} must hold the written bytes of v{n}"
        );
    }
    assert!(
        !path
            .with_file_name("settings.30.instance.no-git.backup.6.json")
            .exists(),
        "slot 6 must not exist"
    );
    for slot in 1..=5u32 {
        assert_ne!(
            std::fs::read(super::settings_backup_path(&path, slot)).unwrap(),
            written[0],
            "no slot may hold v0's bytes"
        );
    }
}

#[test]
fn issue_2058_non_regular_slot_aborts_rotation() {
    // A non-regular slot 1 (a directory) aborts rotation (D8) without
    // fataling the save (D7).
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::AppSettings::default()
    };
    let seed = seed_settings_with_production_bytes(&path, &a);
    std::fs::create_dir(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap();
    let b = super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::AppSettings::default()
    };
    super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    let slot_one = path.with_file_name("settings.30.instance.no-git.backup.1.json");
    assert!(slot_one.is_dir(), "slot 1 is still a directory");
    assert!(
        slot_one.read_dir().unwrap().next().is_none(),
        "slot 1 is empty"
    );
    assert_ne!(
        std::fs::read(&path).unwrap(),
        seed,
        "settings.json holds B's bytes"
    );
    assert!(
        !path
            .with_file_name("settings.30.instance.no-git.backup.2.json")
            .exists(),
        "slot 2 must not exist"
    );
    // Positive control: the same sequence without the directory creates slot 1.
    let ctrl_temp = tempfile::tempdir().unwrap();
    let ctrl_path = ctrl_temp.path().join("settings.json");
    let ctrl_seed = seed_settings_with_production_bytes(&ctrl_path, &a);
    super::save_settings_to_path_preserving_project_paths(&b, &ctrl_path).unwrap();
    assert_eq!(
        std::fs::read(ctrl_path.with_file_name("settings.30.instance.no-git.backup.1.json"))
            .unwrap(),
        ctrl_seed,
        "positive control: slot 1 holds A"
    );
}

#[test]
fn issue_2058_rotation_write_failure_is_not_fatal() {
    let temp = tempfile::tempdir().unwrap();
    let absent = temp.path().join("absent-dir").join("settings.json");
    super::rotate_settings_backups(&absent, b"previous");
    assert!(
        !temp.path().join("absent-dir").exists(),
        "the absent parent directory was never created"
    );
}

#[test]
fn issue_2058_rotation_leaves_the_pre_384_backup_alone() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let legacy = LEGACY_PROFILES_SETTINGS_FIXTURE;
    std::fs::write(&path, legacy).unwrap();
    let b = super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::AppSettings::default()
    };
    super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    let b_bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(path.with_file_name("settings.30.instance.no-git.pre-384-v1.json"))
            .unwrap(),
        legacy,
        "pre-384 backup holds the legacy bytes"
    );
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        legacy.as_bytes(),
        "slot 1 holds the legacy bytes"
    );
    assert_ne!(
        std::fs::read_to_string(&path).unwrap(),
        legacy,
        "settings.json holds B's bytes, not the legacy"
    );
    // Third save with a different value C.
    let c = super::AppSettings {
        root_token: Some("c".to_string()),
        ..super::AppSettings::default()
    };
    super::save_settings_to_path_preserving_project_paths(&c, &path).unwrap();
    assert_eq!(
        std::fs::read_to_string(path.with_file_name("settings.30.instance.no-git.pre-384-v1.json"))
            .unwrap(),
        legacy,
        "pre-384 backup is still byte-identical to legacy"
    );
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        b_bytes,
        "slot 1 now holds B's bytes"
    );
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.2.json")).unwrap(),
        legacy.as_bytes(),
        "slot 2 holds the legacy bytes"
    );
}

#[test]
fn issue_2058_rotation_creates_exactly_one_new_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::AppSettings::default()
    };
    seed_settings_with_production_bytes(&path, &a);
    let before: std::collections::HashSet<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let b = super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::AppSettings::default()
    };
    super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    let after: std::collections::HashSet<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let added: Vec<&String> = after.difference(&before).collect();
    let removed: Vec<&String> = before.difference(&after).collect();
    assert_eq!(added.len(), 1, "exactly one file added: {added:?}");
    assert_eq!(
        added[0].as_str(),
        "settings.30.instance.no-git.backup.1.json"
    );
    assert!(removed.is_empty(), "no file removed: {removed:?}");
    assert_no_issue_1330_temp_files(temp.path());
}

struct SettingsSaveDiagnosticCaptureGuard {
    previous: Option<super::SettingsSaveDiagnosticCapture>,
}

impl SettingsSaveDiagnosticCaptureGuard {
    fn install(capture: impl Fn(&str) + 'static) -> Self {
        let previous = super::SETTINGS_SAVE_DIAGNOSTIC_CAPTURE
            .with(|slot| slot.replace(Some(Box::new(capture))));
        Self { previous }
    }
}

impl Drop for SettingsSaveDiagnosticCaptureGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        super::SETTINGS_SAVE_DIAGNOSTIC_CAPTURE.with(|slot| {
            slot.replace(previous);
        });
    }
}

fn capture_settings_save_diagnostics() -> (
    std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    SettingsSaveDiagnosticCaptureGuard,
) {
    let records = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let captured = std::rc::Rc::clone(&records);
    let guard = SettingsSaveDiagnosticCaptureGuard::install(move |payload| {
        captured.borrow_mut().push(payload.to_string());
    });
    (records, guard)
}

trait CapturedRecordsSnapshot {
    fn snapshot(&self) -> Vec<String>;
}

impl CapturedRecordsSnapshot for std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    fn snapshot(&self) -> Vec<String> {
        self.borrow().clone()
    }
}

impl CapturedRecordsSnapshot for std::cell::Ref<'_, Vec<String>> {
    fn snapshot(&self) -> Vec<String> {
        self.to_vec()
    }
}

impl CapturedRecordsSnapshot for Vec<String> {
    fn snapshot(&self) -> Vec<String> {
        self.clone()
    }
}

fn parse_settings_save_diagnostic(payload: &str) -> std::collections::BTreeMap<String, String> {
    payload
        .split_whitespace()
        .skip(1)
        .map(|field| {
            let (key, value) = field.split_once('=').unwrap();
            (key.to_string(), value.to_string())
        })
        .collect()
}

#[cfg(windows)]
fn expected_rendered_diagnostic_path(path: &std::path::Path) -> String {
    let path = path.to_string_lossy();
    let mut rendered = String::with_capacity(path.len() + 2);
    rendered.push('"');
    for character in path.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/' | ':') {
            rendered.push(character);
        } else {
            rendered.extend(character.escape_unicode());
        }
    }
    rendered.push('"');
    rendered
}

fn expected_io_kind_from_literal_table(kind: std::io::ErrorKind) -> &'static str {
    match kind {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::ConnectionRefused => "connection_refused",
        std::io::ErrorKind::ConnectionReset => "connection_reset",
        std::io::ErrorKind::HostUnreachable => "host_unreachable",
        std::io::ErrorKind::NetworkUnreachable => "network_unreachable",
        std::io::ErrorKind::ConnectionAborted => "connection_aborted",
        std::io::ErrorKind::NotConnected => "not_connected",
        std::io::ErrorKind::AddrInUse => "addr_in_use",
        std::io::ErrorKind::AddrNotAvailable => "addr_not_available",
        std::io::ErrorKind::NetworkDown => "network_down",
        std::io::ErrorKind::BrokenPipe => "broken_pipe",
        std::io::ErrorKind::AlreadyExists => "already_exists",
        std::io::ErrorKind::WouldBlock => "would_block",
        std::io::ErrorKind::NotADirectory => "not_a_directory",
        std::io::ErrorKind::IsADirectory => "is_a_directory",
        std::io::ErrorKind::DirectoryNotEmpty => "directory_not_empty",
        std::io::ErrorKind::ReadOnlyFilesystem => "read_only_filesystem",
        std::io::ErrorKind::StaleNetworkFileHandle => "stale_network_file_handle",
        std::io::ErrorKind::InvalidInput => "invalid_input",
        std::io::ErrorKind::InvalidData => "invalid_data",
        std::io::ErrorKind::TimedOut => "timed_out",
        std::io::ErrorKind::WriteZero => "write_zero",
        std::io::ErrorKind::StorageFull => "storage_full",
        std::io::ErrorKind::NotSeekable => "not_seekable",
        std::io::ErrorKind::QuotaExceeded => "quota_exceeded",
        std::io::ErrorKind::FileTooLarge => "file_too_large",
        std::io::ErrorKind::ResourceBusy => "resource_busy",
        std::io::ErrorKind::ExecutableFileBusy => "executable_file_busy",
        std::io::ErrorKind::Deadlock => "deadlock",
        std::io::ErrorKind::CrossesDevices => "crosses_devices",
        std::io::ErrorKind::TooManyLinks => "too_many_links",
        std::io::ErrorKind::InvalidFilename => "invalid_filename",
        std::io::ErrorKind::ArgumentListTooLong => "argument_list_too_long",
        std::io::ErrorKind::Interrupted => "interrupted",
        std::io::ErrorKind::Unsupported => "unsupported",
        std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
        std::io::ErrorKind::OutOfMemory => "out_of_memory",
        std::io::ErrorKind::Other => "other",
        _ => "other",
    }
}

#[test]
fn settings_save_error_preserves_sources_and_separates_diagnostic_from_outward() {
    for raw in [5, 32] {
        let target = std::path::PathBuf::from("C:/issue-1330/target.json");
        let temporary = std::path::PathBuf::from("C:/issue-1330/settings.json.1.2.tmp");
        let legacy = format!("legacy-only-sentinel-{raw}");
        let error = super::SettingsSaveError::io(
            super::SettingsSaveStage::PreserveDiskGate,
            &target,
            Some(&temporary),
            std::io::Error::from_raw_os_error(raw),
            super::SettingsSaveLegacyOutward::DiskRead(legacy.clone()),
        );
        let (source_raw, source_kind, source_display) = match &error.cause {
            super::SettingsSaveCause::Io(source) => {
                (source.raw_os_error(), source.kind(), source.to_string())
            }
            _ => panic!("expected retained I/O source"),
        };
        assert_eq!(source_raw, Some(raw));
        if let super::SettingsSaveCause::Io(retained_source) = &error.cause {
            let exposed_source = std::error::Error::source(&error).unwrap();
            let exposed_io = exposed_source.downcast_ref::<std::io::Error>().unwrap();
            assert!(std::ptr::eq(exposed_io, retained_source));
        }
        assert_eq!(error.stage, super::SettingsSaveStage::PreserveDiskGate);
        assert_eq!(error.target_path.as_path(), target.as_path());
        assert_eq!(
            error.temp_path.as_deref().map(|path| path.as_path()),
            Some(temporary.as_path())
        );
        assert_eq!(error.pid, std::process::id());
        assert_eq!(
            error.general_settings_diagnostic_code(),
            "settings_save_failed"
        );
        assert_eq!(error.to_string(), "settings_save_failed");

        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::report_settings_save_error(
            error,
            super::SettingsSaveReportSurface::GeneralSettings,
        );
        assert_eq!(outward, legacy);
        let records = CapturedRecordsSnapshot::snapshot(&records);
        assert_eq!(records.len(), 1);
        assert!(!records[0].contains("legacy-only-sentinel"));
        assert!(!records[0].contains(&source_display));
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], "settings_save_failed");
        assert_eq!(fields["source_kind"], "io");
        assert_eq!(fields["reason"], "none");
        assert_eq!(fields["raw_os_error"], raw.to_string());
        assert_eq!(
            fields["io_kind"],
            expected_io_kind_from_literal_table(source_kind)
        );
        assert_eq!(fields["pid"], std::process::id().to_string());
        assert_eq!(fields["target_path"], "\"C:/issue-1330/target.json\"");
        assert_eq!(
            fields["temp_path"],
            "\"C:/issue-1330/settings.json.1.2.tmp\""
        );
    }

    let lock_error = super::SettingsSaveError::io(
        super::SettingsSaveStage::LockAcquire,
        std::path::Path::new("C:/issue-1330/lock.json"),
        None,
        std::io::Error::from_raw_os_error(5),
        super::SettingsSaveLegacyOutward::SettingsLockUnavailable,
    );
    assert!(std::error::Error::source(&lock_error).is_some());
    assert_eq!(lock_error.to_string(), "settings_lock_unavailable");
    let (records, _capture_guard) = capture_settings_save_diagnostics();
    let outward = super::report_settings_save_error(
        lock_error,
        super::SettingsSaveReportSurface::GeneralSettings,
    );
    assert_eq!(outward, "settings_lock_unavailable");
    assert_eq!(
        parse_settings_save_diagnostic(&records.borrow()[0])["code"],
        "settings_lock_unavailable"
    );

    for stage in [
        super::SettingsSaveStage::LockAcquire,
        super::SettingsSaveStage::AtomicReplace,
    ] {
        let error = super::SettingsSaveError::semantic(
            stage,
            std::path::Path::new("C:/issue-1330/terminal.json"),
            None,
            super::SettingsSaveReason::LockTimedOut,
            super::SettingsSaveLegacyOutward::DiskValidation("general-legacy-sentinel".to_string()),
        );
        let expected_display = if stage == super::SettingsSaveStage::LockAcquire {
            "settings_lock_unavailable"
        } else {
            "settings_save_failed"
        };
        assert_eq!(error.to_string(), expected_display);
        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::report_settings_save_error(
            error,
            super::SettingsSaveReportSurface::TerminalSnapshotCompareAndSet,
        );
        assert_eq!(outward, "terminal_snapshot_setting_save_failed");
        let fields = parse_settings_save_diagnostic(&records.borrow()[0]);
        assert_eq!(fields["code"], "terminal_snapshot_setting_save_failed");
        assert!(!records.borrow()[0].contains("general-legacy-sentinel"));
    }

    let json_source = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    let json_error = super::SettingsSaveError::json(
        super::SettingsSaveStage::Serialize,
        std::path::Path::new("C:/issue-1330/json.json"),
        None,
        json_source,
        super::SettingsSaveLegacyOutward::Serialize("json legacy".to_string()),
    );
    assert!(std::error::Error::source(&json_error).is_some());
    let semantic_error = super::SettingsSaveError::semantic(
        super::SettingsSaveStage::Serialize,
        std::path::Path::new("C:/issue-1330/semantic.json"),
        None,
        super::SettingsSaveReason::SerializedJsonNotObject,
        super::SettingsSaveLegacyOutward::SerializedJsonNotObject("semantic legacy".to_string()),
    );
    assert!(std::error::Error::source(&semantic_error).is_none());
}

#[test]
fn settings_save_io_kind_allowlist_matches_stable_1_97_1() {
    let rows = [
        (std::io::ErrorKind::NotFound, "not_found"),
        (std::io::ErrorKind::PermissionDenied, "permission_denied"),
        (std::io::ErrorKind::ConnectionRefused, "connection_refused"),
        (std::io::ErrorKind::ConnectionReset, "connection_reset"),
        (std::io::ErrorKind::HostUnreachable, "host_unreachable"),
        (
            std::io::ErrorKind::NetworkUnreachable,
            "network_unreachable",
        ),
        (std::io::ErrorKind::ConnectionAborted, "connection_aborted"),
        (std::io::ErrorKind::NotConnected, "not_connected"),
        (std::io::ErrorKind::AddrInUse, "addr_in_use"),
        (std::io::ErrorKind::AddrNotAvailable, "addr_not_available"),
        (std::io::ErrorKind::NetworkDown, "network_down"),
        (std::io::ErrorKind::BrokenPipe, "broken_pipe"),
        (std::io::ErrorKind::AlreadyExists, "already_exists"),
        (std::io::ErrorKind::WouldBlock, "would_block"),
        (std::io::ErrorKind::NotADirectory, "not_a_directory"),
        (std::io::ErrorKind::IsADirectory, "is_a_directory"),
        (std::io::ErrorKind::DirectoryNotEmpty, "directory_not_empty"),
        (
            std::io::ErrorKind::ReadOnlyFilesystem,
            "read_only_filesystem",
        ),
        (
            std::io::ErrorKind::StaleNetworkFileHandle,
            "stale_network_file_handle",
        ),
        (std::io::ErrorKind::InvalidInput, "invalid_input"),
        (std::io::ErrorKind::InvalidData, "invalid_data"),
        (std::io::ErrorKind::TimedOut, "timed_out"),
        (std::io::ErrorKind::WriteZero, "write_zero"),
        (std::io::ErrorKind::StorageFull, "storage_full"),
        (std::io::ErrorKind::NotSeekable, "not_seekable"),
        (std::io::ErrorKind::QuotaExceeded, "quota_exceeded"),
        (std::io::ErrorKind::FileTooLarge, "file_too_large"),
        (std::io::ErrorKind::ResourceBusy, "resource_busy"),
        (
            std::io::ErrorKind::ExecutableFileBusy,
            "executable_file_busy",
        ),
        (std::io::ErrorKind::Deadlock, "deadlock"),
        (std::io::ErrorKind::CrossesDevices, "crosses_devices"),
        (std::io::ErrorKind::TooManyLinks, "too_many_links"),
        (std::io::ErrorKind::InvalidFilename, "invalid_filename"),
        (
            std::io::ErrorKind::ArgumentListTooLong,
            "argument_list_too_long",
        ),
        (std::io::ErrorKind::Interrupted, "interrupted"),
        (std::io::ErrorKind::Unsupported, "unsupported"),
        (std::io::ErrorKind::UnexpectedEof, "unexpected_eof"),
        (std::io::ErrorKind::OutOfMemory, "out_of_memory"),
        (std::io::ErrorKind::Other, "other"),
    ];
    assert_eq!(rows.len(), 39);
    for (kind, expected) in rows {
        assert_eq!(super::io_error_kind_as_str(kind), expected);
        assert_eq!(expected_io_kind_from_literal_table(kind), expected);
    }
}

#[test]
fn terminal_snapshot_setting_invalid_defensive_projection_is_closed() {
    for stage in [
        super::SettingsSaveStage::PreserveDiskGate,
        super::SettingsSaveStage::ReconcileDiskGate,
    ] {
        let error = super::SettingsSaveError::semantic(
            stage,
            std::path::Path::new("C:/issue-1330/settings.json"),
            None,
            super::SettingsSaveReason::TerminalSnapshotSettingInvalid,
            super::SettingsSaveLegacyOutward::TerminalSnapshotSettingInvalid(
                "terminal_snapshot_setting_invalid".to_string(),
            ),
        );
        let diagnostic = error.safe_diagnostic(super::SettingsSaveReportSurface::GeneralSettings);
        assert_eq!(diagnostic.reason, "terminal_snapshot_setting_invalid");
        assert_eq!(diagnostic.code, "settings_save_failed");
        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::report_settings_save_error(
            error,
            super::SettingsSaveReportSurface::GeneralSettings,
        );
        assert_eq!(outward, "terminal_snapshot_setting_invalid");
        assert_eq!(records.borrow().len(), 1);
    }
}

#[test]
fn general_settings_outward_compatibility_matrix() {
    fn assert_general_outward(error: super::SettingsSaveError, expected: &str) {
        let diagnostic_code = error.general_settings_diagnostic_code();
        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::report_settings_save_error(
            error,
            super::SettingsSaveReportSurface::GeneralSettings,
        );
        assert_eq!(outward, expected);
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        assert_eq!(
            parse_settings_save_diagnostic(&records[0])["code"],
            diagnostic_code,
        );
    }

    let path = std::path::Path::new("C:/issue-1330/settings.json");
    assert_general_outward(
        super::SettingsSaveError::semantic(
            super::SettingsSaveStage::LockAcquire,
            path,
            None,
            super::SettingsSaveReason::LockTimedOut,
            super::SettingsSaveLegacyOutward::SettingsLockUnavailable,
        ),
        "settings_lock_unavailable",
    );
    assert_general_outward(
        super::SettingsSaveError::semantic(
            super::SettingsSaveStage::PrepareTarget,
            path,
            None,
            super::SettingsSaveReason::TargetNotRegularFile,
            super::SettingsSaveLegacyOutward::SettingsSaveFailed,
        ),
        "settings_save_failed",
    );
    assert_general_outward(
            super::SettingsSaveError::io(
                super::SettingsSaveStage::PreserveDiskGate,
                path,
                None,
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, "reader source"),
                super::SettingsSaveLegacyOutward::DiskRead(
                    "Failed to read C:/issue-1330/settings.json for a project-preserving save (aborting to avoid dropping a project): reader source".to_string(),
                ),
            ),
            "Failed to read C:/issue-1330/settings.json for a project-preserving save (aborting to avoid dropping a project): reader source",
        );
    assert_general_outward(
            super::SettingsSaveError::json(
                super::SettingsSaveStage::PreserveDiskGate,
                path,
                None,
                serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
                super::SettingsSaveLegacyOutward::DiskJson(
                    "Refusing to overwrite C:/issue-1330/settings.json: the existing settings file is not valid JSON (EOF while parsing an object at line 1 column 1)".to_string(),
                ),
            ),
            "Refusing to overwrite C:/issue-1330/settings.json: the existing settings file is not valid JSON (EOF while parsing an object at line 1 column 1)",
        );
    assert_general_outward(
            super::SettingsSaveError::semantic(
                super::SettingsSaveStage::PreserveDiskGate,
                path,
                None,
                super::SettingsSaveReason::DiskJsonNotObject,
                super::SettingsSaveLegacyOutward::DiskJsonNotObject(
                    "Refusing to overwrite C:/issue-1330/settings.json: the existing settings root is not a JSON object".to_string(),
                ),
            ),
            "Refusing to overwrite C:/issue-1330/settings.json: the existing settings root is not a JSON object",
        );
    assert_general_outward(
            super::SettingsSaveError::semantic(
                super::SettingsSaveStage::ReconcileDiskGate,
                path,
                None,
                super::SettingsSaveReason::DiskSettingsValidationRejected,
                super::SettingsSaveLegacyOutward::DiskValidation(
                    "Refusing to overwrite present settings whose non-project fields are invalid: invalid type: string \"invalid\", expected a boolean".to_string(),
                ),
            ),
            "Refusing to overwrite present settings whose non-project fields are invalid: invalid type: string \"invalid\", expected a boolean",
        );
    let serialize_source = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    assert_general_outward(
        super::SettingsSaveError::json(
            super::SettingsSaveStage::Serialize,
            path,
            None,
            serialize_source,
            super::SettingsSaveLegacyOutward::Serialize(
                "Failed to serialize settings: independent source".to_string(),
            ),
        ),
        "Failed to serialize settings: independent source",
    );
    assert_general_outward(
        super::SettingsSaveError::semantic(
            super::SettingsSaveStage::Serialize,
            path,
            None,
            super::SettingsSaveReason::SerializedJsonNotObject,
            super::SettingsSaveLegacyOutward::SerializedJsonNotObject(
                "settings did not serialize to a JSON object".to_string(),
            ),
        ),
        "settings did not serialize to a JSON object",
    );
    assert_general_outward(
        super::SettingsSaveError::json(
            super::SettingsSaveStage::ReDecode,
            path,
            None,
            serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
            super::SettingsSaveLegacyOutward::ReDecode(
                "Failed to re-decode written settings: independent source".to_string(),
            ),
        ),
        "Failed to re-decode written settings: independent source",
    );
}

#[test]
fn settings_save_diagnostic_redacts_values_and_encodes_untrusted_paths() {
    let target =
        std::path::PathBuf::from("C:\\target path\t\r\n\0\"=source_kind=json injected=true-ñ");
    let temporary = std::path::PathBuf::from("C:\\temp path\t\r\n\0\"=code=injected");
    let error = super::SettingsSaveError::io(
        super::SettingsSaveStage::TempWrite,
        &target,
        Some(&temporary),
        std::io::Error::other(
            "source-root-secret api-secret provider-secret serialized-settings-secret",
        ),
        super::SettingsSaveLegacyOutward::DiskRead(
            "legacy-root-secret api-secret provider-secret rootToken apiKey providerCredential"
                .to_string(),
        ),
    );
    let payload = super::render_settings_save_diagnostic(
        &error.safe_diagnostic(super::SettingsSaveReportSurface::GeneralSettings),
    );
    assert_eq!(payload.matches("target_path=").count(), 1);
    assert_eq!(payload.matches("temp_path=").count(), 1);
    for forbidden in [
        "source-root-secret",
        "api-secret",
        "provider-secret",
        "serialized-settings-secret",
        "legacy-root-secret",
        "rootToken",
        "apiKey",
        "providerCredential",
        "injected=true",
        "code=injected",
    ] {
        assert!(
            !payload.contains(forbidden),
            "leaked {forbidden}: {payload}"
        );
    }
    for required in [
        r"\u{20}", r"\u{9}", r"\u{d}", r"\u{a}", r"\u{0}", r"\u{22}", r"\u{3d}", r"\u{5c}",
        r"\u{f1}",
    ] {
        assert!(payload.contains(required), "missing {required}: {payload}");
    }
    let fields = parse_settings_save_diagnostic(&payload);
    for key in ["target_path", "temp_path"] {
        let value = &fields[key];
        assert!(!value.chars().any(char::is_whitespace));
        assert!(!value[1..value.len() - 1].contains('='));
        assert!(!value[1..value.len() - 1].contains('"'));
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        assert!(super::render_diagnostic_path(&path).contains(r"\u{fffd}"));
    }
}

#[cfg(windows)]
#[test]
fn windows_atomic_replace_failure_retains_os_metadata() {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let baseline = super::AppSettings::default();
    super::save_settings_to_path(&baseline, &path).unwrap();
    let original = std::fs::read(&path).unwrap();

    let lock_guard =
        super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
    let retained_handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .unwrap();

    let mut candidate_value = serde_json::to_value(&baseline).unwrap();
    candidate_value.as_object_mut().unwrap().insert(
        "rootToken".to_string(),
        serde_json::Value::String("issue-1330-candidate-root".to_string()),
    );
    let candidate: super::AppSettings = serde_json::from_value(candidate_value).unwrap();
    let error = super::save_settings_value_locked(
        &candidate,
        &path,
        super::ProjectWriteMode::Preserve,
        super::TerminalSnapshotGateWriteMode::Preserve,
        super::MainWindowPlacementWriteMode::Preserve,
    )
    .unwrap_err();

    assert_eq!(error.stage, super::SettingsSaveStage::AtomicReplace);
    assert!(matches!(
        &error.legacy_outward,
        super::SettingsSaveLegacyOutward::SettingsSaveFailed
    ));
    let recorded_target = error.target_path.clone();
    let recorded_temp = error.temp_path.clone().unwrap();
    let recorded_pid = error.pid;
    let (source_raw, source_kind) = match &error.cause {
        super::SettingsSaveCause::Io(source) => (source.raw_os_error(), source.kind()),
        _ => panic!("expected retained atomic replacement I/O source"),
    };
    assert!(source_raw.is_some());
    assert_eq!(recorded_target.as_path(), path.as_path());
    assert_eq!(recorded_pid, std::process::id());
    assert_eq!(recorded_temp.parent(), Some(temp.path()));
    let temp_name = recorded_temp.file_name().unwrap().to_string_lossy();
    assert!(temp_name.starts_with(&format!(
        "settings.30.instance.no-git.json.{}.",
        recorded_pid
    )));
    assert!(temp_name.ends_with(".tmp"));

    let (records, _capture_guard) = capture_settings_save_diagnostics();
    let outward =
        super::report_settings_save_error(error, super::SettingsSaveReportSurface::GeneralSettings);
    assert_eq!(outward, "settings_save_failed");
    let records = CapturedRecordsSnapshot::snapshot(&records);
    assert_eq!(records.len(), 1);
    let fields = parse_settings_save_diagnostic(&records[0]);
    assert_eq!(fields["code"], "settings_save_failed");
    assert_eq!(fields["stage"], "atomic_replace");
    assert_eq!(fields["reason"], "none");
    assert_eq!(fields["source_kind"], "io");
    assert_eq!(fields["raw_os_error"], source_raw.unwrap().to_string(),);
    assert_eq!(
        fields["io_kind"],
        expected_io_kind_from_literal_table(source_kind),
    );
    assert_eq!(fields["pid"], recorded_pid.to_string());
    assert_eq!(
        fields["target_path"],
        expected_rendered_diagnostic_path(&recorded_target),
    );
    assert_eq!(
        fields["temp_path"],
        expected_rendered_diagnostic_path(&recorded_temp),
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!recorded_temp.exists());
    drop(retained_handle);
    drop(lock_guard);
}

#[cfg(windows)]
#[test]
fn windows_settings_replace_retries_access_denied() {
    let mut calls = 0;
    let mut sleeps = Vec::new();

    super::replace_settings_file_atomic_with_retry(
        || {
            calls += 1;
            if calls == 1 {
                Err(std::io::Error::from_raw_os_error(5))
            } else {
                Ok(())
            }
        },
        |duration| sleeps.push(duration.as_millis() as u64),
    )
    .unwrap();

    assert_eq!(calls, 2);
    assert_eq!(sleeps, vec![15]);
}

#[cfg(windows)]
#[test]
fn windows_settings_replace_retries_sharing_violation() {
    let mut calls = 0;
    let mut sleeps = Vec::new();

    super::replace_settings_file_atomic_with_retry(
        || {
            calls += 1;
            if calls == 1 {
                Err(std::io::Error::from_raw_os_error(32))
            } else {
                Ok(())
            }
        },
        |duration| sleeps.push(duration.as_millis() as u64),
    )
    .unwrap();

    assert_eq!(calls, 2);
    assert_eq!(sleeps, vec![15]);
}

#[cfg(windows)]
#[test]
fn windows_settings_replace_non_retryable_fails_immediately() {
    let mut calls = 0;
    let mut sleeps = Vec::new();

    let error = super::replace_settings_file_atomic_with_retry(
        || {
            calls += 1;
            Err(std::io::Error::from_raw_os_error(87))
        },
        |duration| sleeps.push(duration.as_millis() as u64),
    )
    .unwrap_err();

    assert_eq!(error.raw_os_error(), Some(87));
    assert_eq!(calls, 1);
    assert!(sleeps.is_empty());
}

#[cfg(windows)]
#[test]
fn windows_settings_replace_exhaustion_returns_final_error() {
    let mut calls = 0;
    let mut sleeps = Vec::new();

    let error = super::replace_settings_file_atomic_with_retry(
        || {
            calls += 1;
            let raw_os_error = if calls < 6 { 5 } else { 32 };
            Err(std::io::Error::from_raw_os_error(raw_os_error))
        },
        |duration| sleeps.push(duration.as_millis() as u64),
    )
    .unwrap_err();

    assert_eq!(calls, 6);
    assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
    assert_eq!(error.raw_os_error(), Some(32));
}

#[cfg(any(unix, windows))]
#[test]
fn startup_source_lock_failure_blocks_all_save_triggers() {
    for case in [
        "missing_root_token",
        "issue_248_migration",
        "coding_agent_profile_v1_to_v2",
        "profile_repair",
    ] {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        let mut value = serde_json::to_value(super::AppSettings::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        if case != "missing_root_token" {
            object.insert(
                "rootToken".to_string(),
                serde_json::Value::String(format!("issue-1330-root-{case}")),
            );
        }
        match case {
            "missing_root_token" => {
                object.remove("rootToken");
            }
            "issue_248_migration" => {
                object.remove("restoreCoordinatorWakeState");
                object.insert(
                    "startOnlyCoordinators".to_string(),
                    serde_json::Value::Bool(true),
                );
            }
            "coding_agent_profile_v1_to_v2" => {
                object["codingAgentProfiles"]
                    .as_object_mut()
                    .unwrap()
                    .insert(
                        "schemaVersion".to_string(),
                        serde_json::Value::Number(1.into()),
                    );
            }
            "profile_repair" => {
                object["codingAgentProfiles"]
                    .as_object_mut()
                    .unwrap()
                    .insert(
                        "profileSlots".to_string(),
                        serde_json::Value::Object(serde_json::Map::new()),
                    );
            }
            _ => unreachable!(),
        }
        let original = serde_json::to_vec_pretty(&value).unwrap();
        std::fs::write(&path, &original).unwrap();
        let held_lock =
            super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
        let (records, _capture_guard) = capture_settings_save_diagnostics();

        let loaded = super::load_settings_from_path(&path);

        assert_eq!(std::fs::read(&path).unwrap(), original, "case {case}");
        let records = records.borrow();
        assert_eq!(records.len(), 1, "case {case}");
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], "settings_lock_unavailable", "case {case}");
        assert_eq!(fields["stage"], "lock_acquire", "case {case}");
        assert_eq!(fields["reason"], "lock_timed_out", "case {case}");
        assert_eq!(fields["temp_path"], "none", "case {case}");
        assert!(!records[0].contains(&format!("issue-1330-root-{case}")));
        // The source cannot be captured while another process owns the lock.
        // Startup must expose no projects or generated privilege from defaults.
        assert!(loaded.project_paths.is_empty());
        assert!(loaded.project_paths_persistence_error.is_some());
        assert!(loaded.root_token.is_none());
        drop(held_lock);
    }
}

#[test]
fn preserve_disk_gate_failures_are_typed_and_do_not_overwrite() {
    let rows = [
        ("{not json", "malformed"),
        ("[]", "non_object"),
        (
            r#"{"terminalSnapshotsEnabled":"invalid"}"#,
            "invalid_terminal_snapshot_setting",
        ),
    ];

    for (contents, case) in rows {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, contents).unwrap();
        let original = std::fs::read(&path).unwrap();
        let expected = match case {
                "malformed" => {
                    let source =
                        serde_json::from_str::<serde_json::Value>(contents).unwrap_err();
                    format!(
                        "Refusing to overwrite {}: the existing settings file is not valid JSON ({})",
                        path.display(),
                        source
                    )
                }
                "non_object" => format!(
                    "Refusing to overwrite {}: the existing settings root is not a JSON object",
                    path.display()
                ),
                "invalid_terminal_snapshot_setting" => "Refusing to overwrite present settings whose non-project fields are invalid: invalid type: string \"invalid\", expected a boolean".to_string(),
                _ => unreachable!(),
            };

        let error = super::save_settings_value(
            &super::AppSettings::default(),
            &path,
            super::ProjectWriteMode::Preserve,
        )
        .unwrap_err();

        assert_eq!(error.stage, super::SettingsSaveStage::PreserveDiskGate);
        match case {
            "malformed" => {
                assert!(matches!(&error.cause, super::SettingsSaveCause::Json(_)));
                assert!(matches!(
                    &error.legacy_outward,
                    super::SettingsSaveLegacyOutward::DiskJson(_)
                ));
            }
            "non_object" => {
                assert!(matches!(
                    &error.cause,
                    super::SettingsSaveCause::Semantic(
                        super::SettingsSaveReason::DiskJsonNotObject
                    )
                ));
                assert!(matches!(
                    &error.legacy_outward,
                    super::SettingsSaveLegacyOutward::DiskJsonNotObject(_)
                ));
            }
            "invalid_terminal_snapshot_setting" => {
                assert!(matches!(
                    &error.cause,
                    super::SettingsSaveCause::Semantic(
                        super::SettingsSaveReason::DiskSettingsValidationRejected
                    )
                ));
                assert!(matches!(
                    &error.legacy_outward,
                    super::SettingsSaveLegacyOutward::DiskValidation(_)
                ));
                assert_ne!(expected, "terminal_snapshot_setting_invalid",);
            }
            _ => unreachable!(),
        }
        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::report_settings_save_error(
            error,
            super::SettingsSaveReportSurface::GeneralSettings,
        );
        assert_eq!(outward, expected);
        let captured_records = records.borrow();
        assert_eq!(captured_records.len(), 1);
        let fields = parse_settings_save_diagnostic(&captured_records[0]);
        assert_eq!(fields["code"], "settings_save_failed");
        assert_eq!(fields["stage"], "preserve_disk_gate");
        let expected_reason = match case {
            "malformed" => "none",
            "non_object" => "disk_json_not_object",
            "invalid_terminal_snapshot_setting" => "disk_settings_validation_rejected",
            _ => unreachable!(),
        };
        assert_eq!(fields["reason"], expected_reason);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_no_issue_1330_temp_files(temp.path());
    }
}

#[test]
fn reconcile_disk_gate_failure_uses_reconcile_stage_and_legacy_outward() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"terminalSnapshotsEnabled":"invalid"}"#).unwrap();
    let original = std::fs::read(&path).unwrap();
    let error = super::save_settings_value(
        &super::AppSettings::default(),
        &path,
        super::ProjectWriteMode::Reconcile {
            active: false,
            archived: false,
        },
    )
    .unwrap_err();

    assert_eq!(error.stage, super::SettingsSaveStage::ReconcileDiskGate);
    assert!(matches!(
        &error.cause,
        super::SettingsSaveCause::Semantic(
            super::SettingsSaveReason::DiskSettingsValidationRejected
        )
    ));
    assert!(matches!(
        &error.legacy_outward,
        super::SettingsSaveLegacyOutward::DiskValidation(_)
    ));
    let (records, _capture_guard) = capture_settings_save_diagnostics();
    let outward =
        super::report_settings_save_error(error, super::SettingsSaveReportSurface::GeneralSettings);
    assert_eq!(
            outward,
            "Refusing to overwrite present settings whose non-project fields are invalid: invalid type: string \"invalid\", expected a boolean"
        );
    assert_ne!(outward, "terminal_snapshot_setting_invalid");
    let records = CapturedRecordsSnapshot::snapshot(&records);
    assert_eq!(records.len(), 1);
    let fields = parse_settings_save_diagnostic(&records[0]);
    assert_eq!(fields["code"], "settings_save_failed");
    assert_eq!(fields["stage"], "reconcile_disk_gate");
    assert_eq!(fields["reason"], "disk_settings_validation_rejected");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_no_issue_1330_temp_files(temp.path());
}

#[test]
fn project_refresh_compatibility_adapter_maps_typed_failures_once() {
    let rows = [
        ("{not json", "malformed"),
        ("[]", "non_object"),
        (
            r#"{"terminalSnapshotsEnabled":"invalid"}"#,
            "invalid_terminal_snapshot_setting",
        ),
    ];

    for (contents, case) in rows {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, contents).unwrap();
        let original = std::fs::read(&path).unwrap();
        let expected = match case {
                "malformed" => {
                    let source =
                        serde_json::from_str::<serde_json::Value>(contents).unwrap_err();
                    format!(
                        "Refusing to overwrite {}: the existing settings file is not valid JSON ({})",
                        path.display(),
                        source
                    )
                }
                "non_object" => format!(
                    "Refusing to overwrite {}: the existing settings root is not a JSON object",
                    path.display()
                ),
                "invalid_terminal_snapshot_setting" => "Refusing to overwrite present settings whose non-project fields are invalid: invalid type: string \"invalid\", expected a boolean".to_string(),
                _ => unreachable!(),
            };
        let mut settings = super::AppSettings::default();
        let before = serde_json::to_value(&settings).unwrap();

        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let error =
            super::refresh_and_decode_project_paths_from_path(&mut settings, &path).unwrap_err();

        assert_eq!(error, expected);
        let records = CapturedRecordsSnapshot::snapshot(&records);
        assert_eq!(records.len(), 1);
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], "settings_save_failed");
        assert_eq!(fields["stage"], "project_paths_refresh_disk_gate");
        let records = CapturedRecordsSnapshot::snapshot(&records);
        assert_eq!(records.len(), 1);
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], "settings_save_failed");
        assert_eq!(fields["stage"], "project_paths_refresh_disk_gate");
        assert_eq!(serde_json::to_value(&settings).unwrap(), before);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_no_issue_1330_temp_files(temp.path());
    }

    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::create_dir(&path).unwrap();
    let source = std::fs::read_to_string(&path).unwrap_err();
    let expected = format!(
            "Failed to read {} for a project-preserving save (aborting to avoid dropping a project): {}",
            path.display(),
            source
        );
    let mut settings = super::AppSettings::default();
    let before = serde_json::to_value(&settings).unwrap();

    let error =
        super::refresh_and_decode_project_paths_from_path(&mut settings, &path).unwrap_err();

    assert_eq!(error, expected);
    assert_eq!(serde_json::to_value(&settings).unwrap(), before);
}

#[test]
fn lock_and_writer_failures_keep_static_legacy_outward_strings() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let first_lock =
        super::SettingsFileLock::acquire(&settings_path, std::time::Duration::from_secs(1))
            .unwrap();
    let second = super::SettingsFileLock::acquire(&settings_path, std::time::Duration::ZERO);
    let lock_error = match second {
        Ok(_) => panic!("second lock unexpectedly succeeded"),
        Err(error) => error,
    };
    assert_eq!(lock_error.stage, super::SettingsSaveStage::LockAcquire);
    assert!(matches!(
        &lock_error.cause,
        super::SettingsSaveCause::Semantic(super::SettingsSaveReason::LockTimedOut)
    ));
    assert!(matches!(
        &lock_error.legacy_outward,
        super::SettingsSaveLegacyOutward::SettingsLockUnavailable
    ));
    let (lock_records, _lock_capture_guard) = capture_settings_save_diagnostics();
    let lock_outward = super::report_settings_save_error(
        lock_error,
        super::SettingsSaveReportSurface::GeneralSettings,
    );
    assert_eq!(lock_outward, "settings_lock_unavailable");
    let lock_records = lock_records.borrow();
    assert_eq!(lock_records.len(), 1);
    let lock_fields = parse_settings_save_diagnostic(&lock_records[0]);
    assert_eq!(lock_fields["code"], "settings_lock_unavailable");
    assert_eq!(lock_fields["stage"], "lock_acquire");
    assert_eq!(lock_fields["reason"], "lock_timed_out");
    drop(first_lock);

    let writer_temp = tempfile::TempDir::new().unwrap();
    let writer_path = writer_temp.path().join("settings.json");
    std::fs::create_dir(&writer_path).unwrap();
    let writer_error =
        super::save_settings_to_path(&super::AppSettings::default(), &writer_path).unwrap_err();
    assert_eq!(writer_error.stage, super::SettingsSaveStage::PrepareTarget);
    assert!(matches!(
        &writer_error.cause,
        super::SettingsSaveCause::Semantic(super::SettingsSaveReason::TargetNotRegularFile)
    ));
    assert!(matches!(
        &writer_error.legacy_outward,
        super::SettingsSaveLegacyOutward::SettingsSaveFailed
    ));
    let (writer_records, _writer_capture_guard) = capture_settings_save_diagnostics();
    let writer_outward = super::report_settings_save_error(
        writer_error,
        super::SettingsSaveReportSurface::GeneralSettings,
    );
    assert_eq!(writer_outward, "settings_save_failed");
    let writer_records = writer_records.borrow();
    assert_eq!(writer_records.len(), 1);
    let writer_fields = parse_settings_save_diagnostic(&writer_records[0]);
    assert_eq!(writer_fields["code"], "settings_save_failed");
    assert_eq!(writer_fields["stage"], "prepare_target");
    assert_eq!(writer_fields["reason"], "target_not_regular_file");
    assert_no_issue_1330_temp_files(writer_temp.path());
}

#[cfg(any(unix, windows))]
#[test]
fn terminal_compare_and_set_reports_typed_reader_failure_after_dropping_lock() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, "{not json").unwrap();
    let original = std::fs::read(&path).unwrap();
    let records = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let reacquired = std::rc::Rc::new(std::cell::RefCell::new(None));
    let captured_records = std::rc::Rc::clone(&records);
    let captured_reacquired = std::rc::Rc::clone(&reacquired);
    let probe_path = path.clone();
    let _capture_guard = SettingsSaveDiagnosticCaptureGuard::install(move |payload| {
        captured_records.borrow_mut().push(payload.to_string());
        let probe = super::SettingsFileLock::acquire(&probe_path, std::time::Duration::ZERO);
        let succeeded = probe.is_ok();
        if let Ok(probe_guard) = probe {
            drop(probe_guard);
        }
        *captured_reacquired.borrow_mut() = Some(succeeded);
    });

    let outward = super::compare_and_set_terminal_snapshots_enabled_at_path(
        &super::AppSettings::default(),
        &path,
        false,
        true,
    )
    .unwrap_err();

    assert_eq!(outward, "terminal_snapshot_setting_save_failed");
    assert_eq!(*reacquired.borrow(), Some(true));
    let records = CapturedRecordsSnapshot::snapshot(&records);
    assert_eq!(records.len(), 1);
    let fields = parse_settings_save_diagnostic(&records[0]);
    assert_eq!(fields["code"], "terminal_snapshot_setting_save_failed");
    assert_eq!(fields["stage"], "preserve_disk_gate");
    assert_eq!(fields["source_kind"], "json");
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[cfg(any(unix, windows))]
#[test]
fn string_facades_and_compatibility_adapters_map_typed_failures_once() {
    fn assert_held_lock_mapping(
        call: impl FnOnce(&super::AppSettings, &std::path::Path) -> Result<(), String>,
        expected_code: &str,
    ) {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        let settings = super::AppSettings::default();
        super::save_settings_to_path(&settings, &path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let held_lock =
            super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
        let (records, _capture_guard) = capture_settings_save_diagnostics();

        let outward = call(&settings, &path).unwrap_err();

        assert_eq!(outward, expected_code);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], expected_code);
        assert_eq!(fields["stage"], "lock_acquire");
        assert_eq!(fields["reason"], "lock_timed_out");
        drop(held_lock);
    }

    assert_held_lock_mapping(
        |settings, path| {
            super::reconcile_project_state_to_path(settings, path, false, false).map(|_| ())
        },
        "settings_lock_unavailable",
    );
    assert_held_lock_mapping(
        super::save_settings_with_project_paths_to_path,
        "settings_lock_unavailable",
    );
    assert_held_lock_mapping(
        |settings, path| {
            super::save_settings_to_path_preserving_project_paths(settings, path).map(|_| ())
        },
        "settings_lock_unavailable",
    );
    assert_held_lock_mapping(
        |settings, path| {
            super::compare_and_set_terminal_snapshots_enabled_at_path(settings, path, false, true)
                .map(|_| ())
        },
        "terminal_snapshot_setting_save_failed",
    );
}

#[test]
fn terminal_snapshot_gate_defaults_true_and_strict_reader_fails_closed() {
    assert!(super::AppSettings::default().terminal_snapshots_enabled);
    let mut legacy = serde_json::to_value(super::AppSettings::default()).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("terminalSnapshotsEnabled");
    let decoded: super::AppSettings = serde_json::from_value(legacy).unwrap();
    assert!(decoded.terminal_snapshots_enabled);

    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":[]}"#).unwrap();
    let absent = super::read_terminal_snapshot_security_settings_strict_from_path(&path)
        .expect("an absent legacy gate key is a valid default-on object");
    assert!(absent.terminal_snapshots_enabled);
    for malformed in [
        r#"{"terminalSnapshotsEnabled":null,"projectPaths":[]}"#,
        r#"{"terminalSnapshotsEnabled":"invalid","projectPaths":[]}"#,
    ] {
        std::fs::write(&path, malformed).unwrap();
        assert!(
            super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err(),
            "{malformed} must fail closed"
        );
    }
    std::fs::write(
        &path,
        r#"{"terminalSnapshotsEnabled":true,"terminalSnapshotsEnabled":false,"projectPaths":[]}"#,
    )
    .unwrap();
    assert!(super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err());
    std::fs::write(
        &path,
        r#"{"terminalSnapshotsEnabled":true,"projectPaths":["one"]}"#,
    )
    .unwrap();
    let strict = super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
    assert!(strict.terminal_snapshots_enabled);
    assert_eq!(strict.project_paths, vec!["one"]);

    // Empty active authority is valid only with all four active keys absent.
    std::fs::write(&path, r#"{"terminalSnapshotsEnabled":true}"#).unwrap();
    let destination = super::project_paths_path(&path);
    for projection in [
        r#"{}"#,
        r#"{"archivedProjectPaths":["archived"]}"#,
        r#"{"projectPaths":[]}"#,
    ] {
        std::fs::write(&destination, projection).unwrap();
        let strict =
            super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
        assert!(strict.terminal_snapshots_enabled);
        assert!(strict.project_paths.is_empty());
    }
    for projection in [
        r#"{"projectPath":"singular"}"#,
        r#"{"projectPath":null}"#,
        r#"{"projectPathRelativeToInstance":null}"#,
        r#"{"projectPathsRelativeToInstance":[]}"#,
        r#"{"projectPaths":null}"#,
        r#"{"projectPaths":"scalar"}"#,
        r#"{"projectPaths":[null]}"#,
    ] {
        std::fs::write(&destination, projection).unwrap();
        assert!(
            super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err(),
            "{projection}"
        );
    }
    std::fs::write(&destination, b"{}").unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(super::read_terminal_snapshot_security_settings_strict_from_path(&path).is_err());

    const PATH_CANARY: &str = r"C:\PATH_1173_SETTINGS_N3X6\project";
    let structural = super::TerminalSnapshotSecuritySettings {
        terminal_snapshots_enabled: true,
        project_paths: vec![PATH_CANARY.to_string()],
    };
    let diagnostic = format!("{structural:?}");
    assert!(!diagnostic.contains(PATH_CANARY));
    assert!(diagnostic.contains("terminal_snapshots_enabled: true"));
    assert!(diagnostic.contains("project_paths: 1"));
    assert!(diagnostic.contains(&format!("project_path_bytes: {}", PATH_CANARY.len())));
}

#[test]
fn terminal_snapshot_gate_cas_initializes_an_absent_file_before_success() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    assert!(!path.exists());
    let current = super::AppSettings {
        project_paths: vec![temp.path().to_string_lossy().to_string()],
        ..super::AppSettings::default()
    };

    // The absent file compares as the default-on value: an explicit false
    // request is stale against a missing file and must not write anything.
    assert_eq!(
        super::compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, false,)
            .unwrap_err(),
        "terminal_snapshot_setting_conflict"
    );
    assert!(!path.exists());

    // A matching default-on request may not report idempotent success while
    // no valid settings file exists: it must materialize the file first.
    let written =
        super::compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, true, true)
            .unwrap();
    assert!(written.terminal_snapshots_enabled);
    assert!(
        path.exists(),
        "an absent file must be written before the strict reader can authorize"
    );
    let strict = super::read_terminal_snapshot_security_settings_strict_from_path(&path).unwrap();
    assert!(strict.terminal_snapshots_enabled);
}

#[test]
fn terminal_snapshot_gate_cas_is_idempotent_and_rejects_stale_expected_value() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let current = super::AppSettings::default();
    super::save_settings_to_path(&current, &path).unwrap();

    let enabled =
        super::compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, true)
            .unwrap();
    assert!(enabled.terminal_snapshots_enabled);
    let idempotent =
        super::compare_and_set_terminal_snapshots_enabled_at_path(&enabled, &path, false, true)
            .unwrap();
    assert!(idempotent.terminal_snapshots_enabled);
    assert_eq!(
        super::compare_and_set_terminal_snapshots_enabled_at_path(&enabled, &path, false, false,)
            .unwrap_err(),
        "terminal_snapshot_setting_conflict"
    );
    let disabled =
        super::compare_and_set_terminal_snapshots_enabled_at_path(&enabled, &path, true, false)
            .unwrap();
    assert!(!disabled.terminal_snapshots_enabled);
}

#[test]
fn whole_settings_writer_preserves_disk_terminal_snapshot_gate() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let enabled = super::AppSettings {
        terminal_snapshots_enabled: true,
        ..super::AppSettings::default()
    };
    super::save_settings_to_path(&enabled, &path).unwrap();

    let stale = super::AppSettings::default();
    let written = super::save_settings_to_path_preserving_project_paths(&stale, &path).unwrap();
    assert!(written.terminal_snapshots_enabled);

    let false_path = temp.path().join("settings-false.json");
    let mut false_object = serde_json::to_value(super::AppSettings::default()).unwrap();
    false_object.as_object_mut().unwrap().insert(
        "terminalSnapshotsEnabled".to_string(),
        serde_json::json!(false),
    );
    std::fs::write(&false_path, serde_json::to_vec(&false_object).unwrap()).unwrap();
    let written = super::save_settings_to_path_preserving_project_paths(
        &super::AppSettings::default(),
        &false_path,
    )
    .unwrap();
    assert!(!written.terminal_snapshots_enabled);
    let object: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&false_path).unwrap()).unwrap();
    assert_eq!(object["terminalSnapshotsEnabled"], serde_json::json!(false));
    let strict =
        super::read_terminal_snapshot_security_settings_strict_from_path(&false_path).unwrap();
    assert!(!strict.terminal_snapshots_enabled);
}

#[test]
fn whole_settings_writer_materializes_true_for_an_absent_legacy_gate_key() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let mut legacy_object = serde_json::to_value(super::AppSettings::default()).unwrap();
    legacy_object
        .as_object_mut()
        .unwrap()
        .remove("terminalSnapshotsEnabled");
    std::fs::write(&path, serde_json::to_vec(&legacy_object).unwrap()).unwrap();

    let written = super::save_settings_to_path_preserving_project_paths(
        &super::AppSettings::default(),
        &path,
    )
    .unwrap();
    assert!(written.terminal_snapshots_enabled);
    let object: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(object["terminalSnapshotsEnabled"], serde_json::json!(true));
}

#[test]
fn privileged_project_paths_reader_rejects_duplicates_and_non_strings() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":["one"],"projectPaths":["two"]}"#).unwrap();
    assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());

    std::fs::write(&path, r#"{"projectPaths":["one",7]}"#).unwrap();
    assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());
}

#[test]
fn privileged_project_paths_reader_rejects_a_dangling_link_when_supported() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    let missing = temp.path().join("missing.json");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&missing, &path).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&missing, &path).is_ok();
    #[cfg(not(any(unix, windows)))]
    let linked = false;
    if linked {
        assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());
    }
}

#[test]
fn privileged_project_paths_reader_is_bounded_and_exact() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":["one","two"]}"#).unwrap();
    assert_eq!(
        super::read_pty_input_project_paths_strict_from_path(&path).unwrap(),
        Some(vec!["one".to_string(), "two".to_string()])
    );

    std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert!(super::read_pty_input_project_paths_strict_from_path(&path).is_err());
}

#[test]
fn validate_agent_commands_allows_plain_antigravity() {
    let settings = settings_with_agents(&[("Antigravity", "agy")]);
    assert!(super::validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_rejects_antigravity_continue() {
    for command in ["agy --continue", "agy -c"] {
        let settings = settings_with_agents(&[("Antigravity", command)]);
        let err = super::validate_agent_commands(&settings).unwrap_err();
        assert!(
            err.contains("must not include --continue or -c"),
            "command={command:?} err={err}"
        );
    }
}

#[test]
fn validate_agent_commands_allows_antigravity_conversation() {
    // `--conversation <ID>` is a user-authored resume-by-ID form and stays
    // allowed (analog of Claude `--resume <id>`; the injector skip honors it).
    let settings = settings_with_agents(&[("Antigravity", "agy --conversation abc123")]);
    assert!(super::validate_agent_commands(&settings).is_ok());
}

#[test]
fn codex_home_template_accepts_each_token_alone_and_as_leading_segment() {
    for token in [
        "%AC_REPLICA_ROOT%",
        "%AC_WORKSPACE_ROOT%",
        "%AC_MATRIX_ROOT%",
    ] {
        super::validate_codex_home_template_value(token, "ctx")
            .unwrap_or_else(|e| panic!("{token} alone should be accepted: {e}"));
        super::validate_codex_home_template_value(&format!("{token}\\.codex"), "ctx")
            .unwrap_or_else(|e| panic!("{token}\\.codex should be accepted: {e}"));
        super::validate_codex_home_template_value(&format!("{token}/.codex"), "ctx")
            .unwrap_or_else(|e| panic!("{token}/.codex should be accepted: {e}"));
    }
}

#[test]
fn codex_home_template_rejects_token_not_at_leading_segment() {
    let err =
        super::validate_codex_home_template_value(r"prefix%AC_MATRIX_ROOT%\x", "ctx").unwrap_err();
    assert!(
        err.contains("complete path segment") && err.contains("%AC_MATRIX_ROOT%"),
        "{err}"
    );
}

#[test]
fn codex_home_template_rejects_legacy_ac_root_as_unknown_marker() {
    // Save-time break: the old token is now an unknown placeholder, named in the error.
    let err = super::validate_codex_home_template_value(r"%AC_ROOT%\x", "ctx").unwrap_err();
    assert!(err.contains("%AC_ROOT%"), "{err}");
}

#[test]
fn codex_home_template_keys_on_leading_token_not_list_order() {
    // F3: the leading segment is a valid token; another token appearing later must
    // NOT trigger a rejection that names the wrong token.
    super::validate_codex_home_template_value(r"%AC_MATRIX_ROOT%\sub", "ctx")
        .expect("matrix leading segment is valid");
    super::validate_codex_home_template_value(r"%AC_MATRIX_ROOT%\%AC_REPLICA_ROOT%\x", "ctx")
        .expect("leading token valid; remainder tokens are known at shape level");
}

use super::{
    config_seed_dest_os_token_stem, merge_protected_coding_agent_settings,
    repair_coding_agent_profiles_config, validate_agent_commands, validate_api_server_settings,
    validate_resource_settings, AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource,
    MainSidebarSide, ProfileCellConfig, ProfileSlotConfig, ResourceWatchdogAction,
    TelegramNetworkPollErrorLogging, TelegramPollFailureLogLevel, TelegramPollRecoveryLogLevel,
};
use std::collections::BTreeMap;

mod response_close_2850;

#[test]
fn issue_2859_optional_preflight_serde_preserves_legacy_and_raw_values() {
    let legacy = serde_json::json!({
        "id": "row", "label": "Row", "command": "codex", "color": "#000000"
    });
    for null_fields in [false, true] {
        let mut row = legacy.clone();
        let mut cell = serde_json::json!({});
        if null_fields {
            for value in [&mut row, &mut cell] {
                value["preflight"] = serde_json::Value::Null;
                value["preflightTimeoutSeconds"] = serde_json::Value::Null;
            }
        }
        let row: AgentConfig = serde_json::from_value(row).unwrap();
        let cell: ProfileCellConfig = serde_json::from_value(cell).unwrap();
        assert!(row.preflight.is_none() && row.preflight_timeout_seconds.is_none());
        assert!(cell.preflight.is_none() && cell.preflight_timeout_seconds.is_none());
        for value in [
            serde_json::to_value(row).unwrap(),
            serde_json::to_value(cell).unwrap(),
        ] {
            assert!(value.get("preflight").is_none());
            assert!(value.get("preflightTimeoutSeconds").is_none());
        }
    }
    for timeout in [1, 60, 123, u32::MAX] {
        for command in ["", "  ", " echo  raw command "] {
            let mut row = legacy.clone();
            row["preflight"] = serde_json::json!(command);
            row["preflightTimeoutSeconds"] = serde_json::json!(timeout);
            let row: AgentConfig = serde_json::from_value(row).unwrap();
            let cell: ProfileCellConfig = serde_json::from_value(serde_json::json!({
                "enabled": false, "preflight": command, "preflightTimeoutSeconds": timeout
            }))
            .unwrap();
            for value in [
                serde_json::to_value(row).unwrap(),
                serde_json::to_value(cell).unwrap(),
            ] {
                assert_eq!(value["preflight"], command);
                assert_eq!(value["preflightTimeoutSeconds"], timeout);
            }
        }
    }
}

#[test]
fn issue_2859_preflight_timeout_rejects_malformed_wire_values_at_both_levels() {
    for raw in [
        "-1",
        "1.5",
        "\"60\"",
        "4294967296",
        "NaN",
        "Infinity",
        "1e999",
    ] {
        let row = format!(
            r##"{{"id":"row","label":"Row","command":"codex","color":"#000000","preflightTimeoutSeconds":{raw}}}"##
        );
        let cell = format!(r#"{{"preflightTimeoutSeconds":{raw}}}"#);
        assert!(serde_json::from_str::<AgentConfig>(&row).is_err(), "{raw}");
        assert!(
            serde_json::from_str::<ProfileCellConfig>(&cell).is_err(),
            "{raw}"
        );
    }
    assert!(super::validate_preflight_timeout_seconds(Some(0)).is_err());
    for value in [None, Some(1), Some(60), Some(u32::MAX)] {
        super::validate_preflight_timeout_seconds(value).unwrap();
    }
}

#[test]
fn issue_2859_preflight_validation_checks_disabled_and_invalid_letter_before_repair() {
    let mut settings = settings_with_agents(&[("Row", "codex")]);
    settings.agents[0].preflight = Some("secret row command".into());
    settings.agents[0].preflight_timeout_seconds = Some(0);
    let error = super::validate_and_repair_settings(&mut settings).unwrap_err();
    assert!(error.contains("agent-0") && error.contains("preflightTimeoutSeconds"));
    assert!(!error.contains("secret"));
    settings.agents[0].preflight_timeout_seconds = None;
    let mut cell = super::empty_profile_cell();
    cell.enabled = false;
    cell.preflight = Some("secret cell command".into());
    cell.preflight_timeout_seconds = Some(0);
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("orphan".into())
        .or_default()
        .insert("invalid-letter".into(), cell);
    let error = super::validate_and_repair_settings(&mut settings).unwrap_err();
    assert!(
        error.contains("orphan")
            && error.contains("invalid-letter")
            && error.contains("preflightTimeoutSeconds")
    );
    assert!(!error.contains("secret"));
    assert!(
        settings.coding_agent_profiles.profiles_by_agent["orphan"].contains_key("invalid-letter")
    );
}

#[test]
fn issue_2859_preflight_copy_repair_and_protected_merge_keep_row_cell_identity() {
    let mut settings = settings_with_agents(&[("First", "codex"), ("Second", "codex")]);
    for (index, row) in settings.agents.iter_mut().enumerate() {
        row.preflight = Some(format!(" raw  row {index} "));
        row.preflight_timeout_seconds = Some(60 + index as u32);
        let mut cell = super::empty_profile_cell();
        cell.enabled = false;
        cell.preflight_timeout_seconds = Some(100 + index as u32);
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry(row.id.clone())
            .or_default()
            .insert("B".into(), cell);
    }
    super::validate_and_repair_settings(&mut settings).unwrap();
    let copied = settings.clone();
    let merged = super::merge_protected_coding_agent_settings(&settings, copied);
    let restored: AppSettings =
        serde_json::from_value(serde_json::to_value(&merged).unwrap()).unwrap();
    for (index, row) in restored.agents.iter().enumerate() {
        assert_eq!(
            row.preflight.as_deref(),
            Some(format!(" raw  row {index} ").as_str())
        );
        assert_eq!(row.preflight_timeout_seconds, Some(60 + index as u32));
        let cell = &restored.coding_agent_profiles.profiles_by_agent[&row.id]["B"];
        assert_eq!(cell.preflight_timeout_seconds, Some(100 + index as u32));
        assert!(cell.preflight.is_none() && !cell.enabled);
    }
}

#[test]
fn issue_2859_preflight_overlay_save_retains_base_and_effective_whole_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::SETTINGS_FILE_NAME);
    let mut base = settings_with_agents(&[("Row", "codex")]);
    base.root_token = Some("test-token".into());
    base.agents[0].preflight = Some(" base  command ".into());
    base.agents[0].preflight_timeout_seconds = Some(60);
    super::validate_and_repair_settings(&mut base).unwrap();
    let mut cell = super::empty_profile_cell();
    cell.preflight_timeout_seconds = Some(123);
    base.coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .insert("B".into(), cell);
    let original = serde_json::to_string(&base).unwrap();
    std::fs::write(&path, &original).unwrap();
    let mut overlay = serde_json::to_value(&base).unwrap();
    overlay["agents"][0]["preflight"] = serde_json::json!(" overlay  command ");
    overlay["agents"][0]["preflightTimeoutSeconds"] = serde_json::json!(456);
    overlay["codingAgentProfiles"]["profilesByAgent"]["agent-0"]["B"]["preflightTimeoutSeconds"] =
        serde_json::json!(789);
    std::fs::write(
        dir.path().join(super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
        serde_json::to_vec(&serde_json::json!({
            "agents": overlay["agents"], "codingAgentProfiles": overlay["codingAgentProfiles"]
        }))
        .unwrap(),
    )
    .unwrap();
    let (mut effective, _) = super::parse_settings_json(&original, "test", Some(&path)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original,
        "load must not eagerly rewrite fields"
    );
    super::validate_and_repair_settings(&mut effective).unwrap();
    let written = super::save_settings_to_path_preserving_project_paths(&effective, &path).unwrap();
    assert_eq!(
        written.agents[0].preflight.as_deref(),
        Some(" overlay  command ")
    );
    assert_eq!(written.agents[0].preflight_timeout_seconds, Some(456));
    assert_eq!(
        written.coding_agent_profiles.profiles_by_agent["agent-0"]["B"].preflight_timeout_seconds,
        Some(789)
    );
    std::fs::remove_file(dir.path().join(super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME)).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let (base_again, _) = super::parse_settings_json(&contents, "test", Some(&path)).unwrap();
    assert_eq!(
        base_again.agents[0].preflight.as_deref(),
        Some(" base  command ")
    );
    assert_eq!(base_again.agents[0].preflight_timeout_seconds, Some(60));
    assert_eq!(
        base_again.coding_agent_profiles.profiles_by_agent["agent-0"]["B"]
            .preflight_timeout_seconds,
        Some(123)
    );
}

#[test]
fn issue_2859_legacy_load_does_not_persist_preflight_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::SETTINGS_FILE_NAME);
    let mut settings = settings_with_agents(&[("Row", "codex")]);
    settings.root_token = Some("test-token".into());
    super::validate_and_repair_settings(&mut settings).unwrap();
    super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let agents_path = dir.path().join(super::AGENTS_INSTANCE_FILE_NAME);
    let before_settings = std::fs::read(&path).unwrap();
    let before_agents = std::fs::read(&agents_path).unwrap();
    let loaded = super::load_settings_from_path(&path);
    assert_eq!(loaded.agents.len(), 1);
    assert!(loaded.agents[0].preflight.is_none());
    assert!(loaded.agents[0].preflight_timeout_seconds.is_none());
    let cell = &loaded.coding_agent_profiles.profiles_by_agent["agent-0"]["A"];
    assert!(cell.preflight.is_none() && cell.preflight_timeout_seconds.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), before_settings);
    assert_eq!(std::fs::read(&agents_path).unwrap(), before_agents);
    assert!(!String::from_utf8(before_agents)
        .unwrap()
        .contains("preflight"));
}

fn settings_with_agents(commands: &[(&str, &str)]) -> AppSettings {
    AppSettings {
        agents: commands
            .iter()
            .enumerate()
            .map(|(idx, (label, command))| AgentConfig {
                preflight: None,
                preflight_timeout_seconds: None,
                id: format!("agent-{idx}"),
                id_hash: None,
                id_hash_debug: None,
                label: (*label).to_string(),
                command: (*command).to_string(),
                color: "#000000".to_string(),
                order: None,
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                blocking_menus: None,
                backend: Default::default(),
            })
            .collect(),
        ..AppSettings::default()
    }
}

/// #2162 test 10: the save-time collision predicate. Decision logic only --
/// the `log::warn!` emission itself is declared not verified by test.
#[test]
fn dest_os_token_stem_detects_collisions() {
    for (dest, expected) in [
        (".claude.linux", Some(".claude")),
        (".claude.WINDOWS", Some(".claude")),
        (".claude.macos", Some(".claude")),
        (".claude", None),
        (".claudelinux", None),
        (".linux", Some("")),
    ] {
        assert_eq!(
            config_seed_dest_os_token_stem(dest),
            expected.map(|e: &str| e.to_string()),
            "dest {dest}"
        );
    }
}

#[test]
fn validate_config_seed_dest_accepts_dotfile_names() {
    for ok in [".claude", ".claude-amp", ".codex", ".opencode", "config"] {
        assert!(
            super::validate_config_seed_dest(ok).is_ok(),
            "should accept {ok:?}"
        );
    }
    // Leading/trailing whitespace is trimmed before validation.
    assert!(super::validate_config_seed_dest("  .claude  ").is_ok());
}

#[test]
fn validate_config_seed_dest_rejects_unsafe_names() {
    let bad = [
        "",                  // empty
        "   ",               // whitespace only
        ".config/opencode",  // forward separator (nested)
        ".config\\x",        // backslash separator
        "..",                // parent
        "a..b",              // contains ..
        "C:foo",             // drive-relative colon
        "x:y",               // ADS colon
        "%AC_REPLICA_ROOT%", // placeholder marker
        "$HOME",             // shell marker
        ".claude.",          // trailing dot (trim does NOT strip dots)
        "CON",               // reserved device
        "nul",               // reserved device (case-insensitive)
        "COM1",              // reserved device
        "lpt9.claude",       // reserved device before first dot
    ];
    for name in bad {
        assert!(
            super::validate_config_seed_dest(name).is_err(),
            "should reject {name:?}"
        );
    }
}

#[test]
fn config_seed_is_active_requires_enabled_and_nonempty_dest() {
    use super::ConfigSeedConfig;
    assert!(ConfigSeedConfig {
        enabled: true,
        dest: ".claude".to_string()
    }
    .is_active());
    assert!(!ConfigSeedConfig {
        enabled: false,
        dest: ".claude".to_string()
    }
    .is_active());
    assert!(!ConfigSeedConfig {
        enabled: true,
        dest: "   ".to_string()
    }
    .is_active());
}

#[test]
fn config_seed_serde_round_trips_camel_case_and_omits_when_absent() {
    use super::AgentConfig;
    // Present -> serializes as nested `configSeed { enabled, dest }`.
    let mut agent = AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "claude".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "Claude".to_string(),
        command: "claude".to_string(),
        color: "#fff".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: Some(super::ConfigSeedConfig {
            enabled: true,
            dest: ".claude".to_string(),
        }),
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    };
    let json = serde_json::to_string(&agent).unwrap();
    assert!(json.contains("\"configSeed\""), "{json}");
    assert!(json.contains("\"dest\":\".claude\""), "{json}");
    let back: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.config_seed.as_ref().map(|c| c.dest.as_str()),
        Some(".claude")
    );

    // Absent -> key omitted (skip_serializing_if), and old files deserialize to None.
    agent.config_seed = None;
    let json = serde_json::to_string(&agent).unwrap();
    assert!(!json.contains("configSeed"), "{json}");
    let back: AgentConfig =
        serde_json::from_str(r##"{"id":"x","label":"X","command":"claude","color":"#000"}"##)
            .unwrap();
    assert!(back.config_seed.is_none());
}

#[test]
fn agent_backend_config_defaults_to_local_process_and_omits_default() {
    use super::AgentConfig;

    let agent: AgentConfig =
        serde_json::from_str(r##"{"id":"x","label":"X","command":"codex","color":"#000"}"##)
            .unwrap();

    assert_eq!(
        crate::pty::backend::SessionBackendKind::from(&agent.backend),
        crate::pty::backend::SessionBackendKind::LocalProcess
    );
    let json = serde_json::to_string(&agent).unwrap();
    assert!(!json.contains("backend"), "{json}");

    let mut local_with_image = agent;
    local_with_image.backend.image = Some("hand-edited:latest".to_string());
    let json = serde_json::to_string(&local_with_image).unwrap();
    assert!(!json.contains("backend"), "{json}");
}

#[test]
fn agent_backend_config_serializes_container_image() {
    use super::AgentConfig;

    let mut settings = settings_with_agents(&[("Claude", "claude")]);
    settings.agents[0].backend.kind = crate::pty::backend::SessionBackendKind::ContainerTransport;
    settings.agents[0].backend.image = Some("agentscommander/ac-claude:latest".to_string());
    super::validate_and_repair_settings(&mut settings).unwrap();

    let json = serde_json::to_string(&settings.agents[0]).unwrap();
    assert!(json.contains("\"backend\""), "{json}");
    assert!(json.contains("\"kind\":\"containerTransport\""), "{json}");
    assert!(
        json.contains("\"image\":\"agentscommander/ac-claude:latest\""),
        "{json}"
    );
    let back: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.backend.image.as_deref(),
        Some("agentscommander/ac-claude:latest")
    );
}

#[test]
fn validate_and_repair_normalizes_backend_image_edges() {
    let mut blank_container = settings_with_agents(&[("Claude", "claude")]);
    blank_container.agents[0].backend.kind =
        crate::pty::backend::SessionBackendKind::ContainerTransport;
    blank_container.agents[0].backend.image = Some("   ".to_string());
    super::validate_and_repair_settings(&mut blank_container).unwrap();
    assert!(blank_container.agents[0].backend.image.is_none());

    let mut local_with_image = settings_with_agents(&[("Codex", "codex")]);
    local_with_image.agents[0].backend.image = Some(" custom:latest ".to_string());
    super::validate_and_repair_settings(&mut local_with_image).unwrap();
    assert!(local_with_image.agents[0].backend.image.is_none());
    let json = serde_json::to_string(&local_with_image.agents[0]).unwrap();
    assert!(!json.contains("backend"), "{json}");
}

#[test]
fn validate_and_repair_rejects_leading_dash_container_image() {
    let mut settings = settings_with_agents(&[("Claude", "claude")]);
    settings.agents[0].backend.kind = crate::pty::backend::SessionBackendKind::ContainerTransport;
    settings.agents[0].backend.image = Some(" --privileged".to_string());

    let err = super::validate_and_repair_settings(&mut settings).unwrap_err();
    assert!(err.contains("must not start with"), "{err}");
}

#[test]
fn screenshot_hotkey_defaults_when_absent() {
    // #714 an old settings file without the key deserializes to "Ctrl+Q".
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("screenshotCaptureHotkey");
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.screenshot_capture_hotkey, "Ctrl+Q");
}

#[test]
fn screenshot_hotkey_round_trips_camel_case() {
    let s = AppSettings {
        screenshot_capture_hotkey: "Control+P".to_string(),
        ..AppSettings::default()
    };
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        json.contains("\"screenshotCaptureHotkey\":\"Control+P\""),
        "{json}"
    );
    let back: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.screenshot_capture_hotkey, "Control+P");
}

#[test]
fn validate_and_repair_rejects_invalid_screenshot_hotkey() {
    let mut s = AppSettings {
        screenshot_capture_hotkey: "Ctrl+Shift+Q".to_string(),
        ..AppSettings::default()
    };
    assert!(super::validate_and_repair_settings(&mut s).is_err());

    s.screenshot_capture_hotkey = "Ctrl+Q".to_string();
    assert!(super::validate_and_repair_settings(&mut s).is_ok());
}

/// #2281 Cross-language parity contract (plan section "Cross-language
/// validator parity"): the 22 accepted letters `A-Z` minus `W/R/C/V` in
/// upper case, plus four mixed-case/padded spellings, so 26 entries.
/// `src/shared/app-hotkey.test.ts` (phase 5) re-asserts this list verbatim.
const PARITY_ACCEPTED: [&str; 26] = [
    "Ctrl+Shift+A",
    "Ctrl+Shift+B",
    "Ctrl+Shift+D",
    "Ctrl+Shift+E",
    "Ctrl+Shift+F",
    "Ctrl+Shift+G",
    "Ctrl+Shift+H",
    "Ctrl+Shift+I",
    "Ctrl+Shift+J",
    "Ctrl+Shift+K",
    "Ctrl+Shift+L",
    "Ctrl+Shift+M",
    "Ctrl+Shift+N",
    "Ctrl+Shift+O",
    "Ctrl+Shift+P",
    "Ctrl+Shift+Q",
    "Ctrl+Shift+S",
    "Ctrl+Shift+T",
    "Ctrl+Shift+U",
    "Ctrl+Shift+X",
    "Ctrl+Shift+Y",
    "Ctrl+Shift+Z",
    "ctrl+shift+e",
    "CTRL+SHIFT+E",
    "Control+Shift+Z",
    " Ctrl + Shift + E ",
];

/// #2281 The 13 rejected literals in the plan's numbered order. Entry 6 is
/// the digit leak, entry 5 proves the reserved check is case-insensitive and
/// entry 13 is the Kelvin sign U+212A, which a `to_uppercase()` mirror would
/// wrongly accept as `K`.
const PARITY_REJECTED: [&str; 13] = [
    "Ctrl+Shift+W",
    "Ctrl+Shift+R",
    "Ctrl+Shift+C",
    "Ctrl+Shift+V",
    "ctrl+shift+w",
    "Ctrl+Shift+2",
    "Ctrl+E",
    "Shift+E",
    "Alt+Shift+E",
    "Ctrl+Shift+EE",
    "Ctrl+Shift+",
    "",
    "Ctrl+Shift+\u{212A}",
];

#[test]
fn sidebar_compact_hotkey_defaults_when_absent() {
    // #2281 an old settings file without the key deserializes to "Ctrl+Shift+E".
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("sidebarCompactHotkey");
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.sidebar_compact_hotkey, "Ctrl+Shift+E");
}

#[test]
fn sidebar_compact_hotkey_round_trips_camel_case() {
    let s = AppSettings {
        sidebar_compact_hotkey: "Control+Shift+B".to_string(),
        ..AppSettings::default()
    };
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        json.contains("\"sidebarCompactHotkey\":\"Control+Shift+B\""),
        "{json}"
    );
    let back: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.sidebar_compact_hotkey, "Control+Shift+B");
}

#[test]
fn sidebar_compact_hotkey_accepts_the_parity_table() {
    assert_eq!(PARITY_ACCEPTED.len(), 26);
    for value in PARITY_ACCEPTED {
        assert!(
            super::validate_sidebar_compact_hotkey(value).is_ok(),
            "expected Ok for {value:?}"
        );
    }
}

#[test]
fn sidebar_compact_hotkey_rejects_the_parity_table() {
    assert_eq!(PARITY_REJECTED.len(), 13);
    for value in PARITY_REJECTED {
        let err = super::validate_sidebar_compact_hotkey(value)
            .expect_err(&format!("expected Err for {value:?}"));
        assert!(
            err.starts_with("Sidebar compact hotkey: "),
            "unexpected error for {value:?}: {err}"
        );
    }
}

#[test]
fn sidebar_compact_hotkey_call_site_preserves_a_valid_value() {
    // Positive control: a validator that rejected everything would pass the
    // reject table, so prove a valid non-default value survives the call site.
    let mut s = AppSettings {
        sidebar_compact_hotkey: "Ctrl+Shift+B".to_string(),
        ..AppSettings::default()
    };
    assert!(super::validate_and_repair_settings(&mut s).is_ok());
    assert_eq!(s.sidebar_compact_hotkey, "Ctrl+Shift+B");
}

#[test]
fn sidebar_compact_hotkey_call_site_rejects_an_invalid_value() {
    // Negative control: proves the validator is wired into
    // `validate_and_repair_settings` and not merely defined.
    let mut s = AppSettings {
        sidebar_compact_hotkey: "Ctrl+Shift+W".to_string(),
        ..AppSettings::default()
    };
    let err = super::validate_and_repair_settings(&mut s).unwrap_err();
    assert!(err.contains("Sidebar compact hotkey"), "{err}");
}

#[test]
fn room_number_mask_defaults_when_absent() {
    // #2507 an old settings file without the key deserializes to "#".
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    assert!(value
        .as_object_mut()
        .unwrap()
        .remove("roomNumberMask")
        .is_some());
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.room_number_mask, "#");
}

#[test]
fn room_number_mask_round_trips_camel_case() {
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value["roomNumberMask"] = serde_json::json!("####");
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    let back = serde_json::to_value(&parsed).unwrap();
    assert_eq!(back["roomNumberMask"], serde_json::json!("####"));
}

#[test]
fn room_number_mask_accepts_and_rejects_the_parity_table() {
    for (value, width) in [("#", 1), ("##", 2), ("#########", 9)] {
        assert_eq!(super::validate_room_number_mask(value), Ok(width));
    }
    for value in ["", " ##", "##1", "2", "#-#", "##########"] {
        let err = super::validate_room_number_mask(value)
            .expect_err(&format!("expected Err for {value:?}"));
        assert!(
            err.starts_with("Room number mask: "),
            "unexpected error for {value:?}: {err}"
        );
    }
}

#[test]
fn room_number_mask_call_site_blocks_an_invalid_save() {
    let mut bad = AppSettings {
        room_number_mask: "2".to_string(),
        ..AppSettings::default()
    };
    let err = super::validate_and_repair_settings(&mut bad).unwrap_err();
    assert!(err.starts_with("Room number mask: "), "{err}");
    // Anti-vacuous control: the same value with a valid mask passes.
    let mut good = AppSettings {
        room_number_mask: "###".to_string(),
        ..AppSettings::default()
    };
    assert!(super::validate_and_repair_settings(&mut good).is_ok());
    assert_eq!(good.room_number_mask, "###");
}

#[test]
fn screenshot_and_sidebar_compact_hotkeys_are_independent() {
    let mut s = AppSettings {
        sidebar_compact_hotkey: "Ctrl+Shift+B".to_string(),
        ..AppSettings::default()
    };
    assert_eq!(s.screenshot_capture_hotkey, "Ctrl+Q");
    s.screenshot_capture_hotkey = "Ctrl+P".to_string();
    assert_eq!(s.sidebar_compact_hotkey, "Ctrl+Shift+B");
    assert!(super::validate_and_repair_settings(&mut s).is_ok());
    assert_eq!(s.sidebar_compact_hotkey, "Ctrl+Shift+B");
    assert_eq!(s.screenshot_capture_hotkey, "Ctrl+P");
}

#[test]
fn typing_hold_seconds_defaults_to_30_when_absent() {
    // #2336 an old settings file without the key deserializes to 30.
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value.as_object_mut().unwrap().remove("typingHoldSeconds");
    let parsed: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.typing_hold_seconds, 30);

    let json = serde_json::to_string(&AppSettings::default()).unwrap();
    assert!(json.contains("\"typingHoldSeconds\":30"), "{json}");
}

#[test]
fn validate_and_repair_accepts_typing_hold_bounds_and_rejects_out_of_range() {
    for seconds in [
        super::TYPING_HOLD_SECONDS_MIN,
        30,
        super::TYPING_HOLD_SECONDS_MAX,
    ] {
        let mut s = AppSettings {
            typing_hold_seconds: seconds,
            ..AppSettings::default()
        };
        assert!(
            super::validate_typing_hold_settings(&s).is_ok(),
            "{seconds}"
        );
        assert!(
            super::validate_and_repair_settings(&mut s).is_ok(),
            "{seconds}"
        );
    }
    for seconds in [0, super::TYPING_HOLD_SECONDS_MAX + 1] {
        let s = AppSettings {
            typing_hold_seconds: seconds,
            ..AppSettings::default()
        };
        let err = super::validate_typing_hold_settings(&s).unwrap_err();
        assert!(err.contains("typingHoldSeconds"), "{err}");
        let mut repaired = s.clone();
        assert!(
            super::validate_and_repair_settings(&mut repaired).is_err(),
            "{seconds}"
        );
    }
}

/// #2336 - the Settings update path rejects an out-of-range typing-hold
/// window and leaves the live value unchanged: the error returns before the
/// save, so the persisted (and live) window stays at the old value. The
/// valid 1/3600 boundaries are covered by the validation test above.
#[tokio::test]
async fn invalid_typing_hold_settings_update_leaves_live_settings_unchanged() {
    let original = AppSettings::default();
    assert_eq!(original.typing_hold_seconds, 30);
    let state: super::SettingsState = crate::config::settings::new_settings_state(original.clone());

    let mut invalid = original;
    invalid.typing_hold_seconds = 0;
    let err = crate::commands::config::persist_protected_settings_update(&state, invalid)
        .await
        .expect_err("out-of-range typing hold must be rejected");
    assert!(err.contains("typingHoldSeconds"), "{err}");
    assert_eq!(state.read().await.typing_hold_seconds, 30);
}

/// #2015: shared pre-v2 fixture: `letters.A.name` plus
/// `matrix.codex.A.argv` legacy data under `codingAgentProfiles`.
const LEGACY_PROFILES_SETTINGS_FIXTURE: &str = r##"{
        "defaultShell": "bash",
        "defaultShellArgs": [],
        "rootToken": "existing-token",
        "agents": [{
            "id": "codex",
            "label": "Codex",
            "command": "codex",
            "color": "#000000"
        }],
        "codingAgentProfiles": {
            "schemaVersion": 1,
            "letters": { "A": { "name": "Baseline" } },
            "matrix": {
                "codex": {
                    "A": {
                        "enabled": true,
                        "argv": ["--model", "gpt'5"],
                        "env": {},
                        "notes": "legacy"
                    }
                }
            }
        }
    }"##;

#[test]
fn load_settings_backs_up_and_drops_legacy_profiles_shape() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, LEGACY_PROFILES_SETTINGS_FIXTURE).unwrap();

    let settings = super::load_settings_from_path(&path);

    assert_eq!(settings.root_token.as_deref(), Some("existing-token"));
    assert_eq!(settings.coding_agent_profiles.schema_version, 2);
    assert!(!settings
        .coding_agent_profiles
        .profiles_by_agent
        .get("codex")
        .is_some_and(|cells| cells.values().any(|cell| cell.command.contains("--model"))));

    let backup_path = temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json");
    assert_eq!(
        std::fs::read_to_string(backup_path).unwrap(),
        LEGACY_PROFILES_SETTINGS_FIXTURE
    );

    let saved_raw = std::fs::read_to_string(super::agents_instance_path(&path)).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved_raw).unwrap();
    assert_eq!(saved["codingAgentProfiles"]["schemaVersion"], 2);
    assert!(saved["codingAgentProfiles"].get("profileSlots").is_some());
    assert!(saved["codingAgentProfiles"]
        .get("profilesByBaseHash")
        .is_some());
    assert!(saved["codingAgentProfiles"]
        .get("profileLabelsByBaseHash")
        .is_some());
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(saved["codingAgentProfiles"].get(key).is_none());
    }
    assert_eq!(saved["codingAgentProfiles"]["profileBindingVersion"], 1);
    assert!(saved["codingAgentProfiles"].get("matrix").is_none());
    assert!(saved["codingAgentProfiles"].get("letters").is_none());
}

#[test]
fn preserve_save_backs_up_legacy_disk_before_dropping_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, LEGACY_PROFILES_SETTINGS_FIXTURE).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, detected) = super::parse_settings_json(&contents, "test", None).unwrap();
    assert!(detected);

    super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let backup_path = temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json");
    assert_eq!(
        std::fs::read_to_string(&backup_path).unwrap(),
        LEGACY_PROFILES_SETTINGS_FIXTURE
    );
    let saved_raw = std::fs::read_to_string(&path).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved_raw).unwrap();
    assert!(saved["codingAgentProfiles"].get("letters").is_none());
    assert!(saved["codingAgentProfiles"].get("matrix").is_none());

    // A later legacy-shaped disk file must not overwrite the first backup.
    std::fs::write(
        &path,
        LEGACY_PROFILES_SETTINGS_FIXTURE.replace("existing-token", "second-token"),
    )
    .unwrap();
    super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&backup_path).unwrap(),
        LEGACY_PROFILES_SETTINGS_FIXTURE
    );
}

#[test]
fn reconcile_save_keeps_legacy_keys_and_writes_no_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, LEGACY_PROFILES_SETTINGS_FIXTURE).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, _) = super::parse_settings_json(&contents, "test", None).unwrap();

    super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();

    let saved_raw = std::fs::read_to_string(super::agents_instance_path(&path)).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved_raw).unwrap();
    assert!(saved["codingAgentProfiles"].get("letters").is_some());
    assert!(!temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json")
        .exists());
}

#[test]
fn legacy_profiles_shape_detection_is_exact() {
    let rows: [(serde_json::Value, bool); 16] = [
        (serde_json::json!({}), false),
        (serde_json::json!({"codingAgentProfiles": []}), false),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": 2}}),
            false,
        ),
        (serde_json::json!({"codingAgentProfiles": {}}), false),
        (
            serde_json::json!({"codingAgentProfiles": {
                "schemaVersion": 2,
                "profileSlots": {"A": {"label": "x"}},
                "profilesByAgent": {"claude": {"A": {"enabled": true, "command": "--m", "env": {}, "notes": ""}}},
                "profileLabelsByAgent": {"claude": {"A": "L"}}
            }}),
            false,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": "1"}}),
            false,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"profileSlots": {"A": "name"}}}),
            false,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"profilesByAgent": {"claude": {"A": ["argv"]}}}}),
            false,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": 1}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"letters": {}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"matrix": {}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"agentDefaults": {}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": 2, "profileSlots": {"A": {"name": "Baseline"}}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"profileSlots": {"A": {"label": "x", "name": "y"}}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"schemaVersion": 2, "profilesByAgent": {"claude": {"A": {"argv": ["--model", "x"]}}}}}),
            true,
        ),
        (
            serde_json::json!({"codingAgentProfiles": {"profilesByAgent": {"claude": {"B": {"args": []}}}}}),
            true,
        ),
    ];

    for (value, expected) in rows {
        let before = value.clone();
        assert_eq!(
            super::legacy_profiles_shape_present(value.as_object().unwrap()),
            expected,
            "{value}"
        );
        assert_eq!(value, before, "the predicate must not mutate the value");
    }
}

#[test]
fn legacy_profiles_shape_warns_backs_up_and_keeps_v2_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let original = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "rootToken": "existing-token",
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex",
                "color": "#000000"
            }],
            "codingAgentProfiles": {
                "schemaVersion": 2,
                "profileSlots": { "A": { "name": "Baseline" } },
                "profileLabelsByAgent": { "codex": { "A": "Kept" } }
            }
        }"##;
    std::fs::write(&path, original).unwrap();

    let settings = super::load_settings_from_path(&path);

    let backup_path = temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json");
    assert_eq!(std::fs::read_to_string(backup_path).unwrap(), original);
    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["codex"]["A"],
        "Kept"
    );
    assert_eq!(settings.coding_agent_profiles.profile_slots["A"].label, "");
}

#[test]
fn profile_labels_survive_the_real_loader_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let original = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "rootToken": "round-trip-token",
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex",
                "color": "#000000"
            }],
            "codingAgentProfiles": {
                "schemaVersion": 2,
                "profileSlots": { "A": { "label": "" } },
                "defaultProfileByAgent": {},
                "profilesByAgent": {
                    "codex": {
                        "A": { "enabled": true, "command": "", "env": {}, "notes": "" }
                    }
                },
                "profileLabelsByAgent": { "codex": { "A": "First" } }
            }
        }"##;
    std::fs::write(&path, original).unwrap();

    let mut settings = super::load_settings_from_path(&path);
    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["codex"]["A"],
        "First"
    );

    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert("B".to_string(), "Second".to_string());
    super::save_settings_to_path_preserving_project_paths_typed(&settings, &path).unwrap();

    let reloaded = super::load_settings_from_path(&path);
    assert_eq!(
        reloaded.coding_agent_profiles.profile_labels_by_agent["codex"]["A"],
        "First"
    );
    assert_eq!(
        reloaded.coding_agent_profiles.profile_labels_by_agent["codex"]["B"],
        "Second"
    );
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(super::agents_instance_path(&path)).unwrap())
            .unwrap();
    assert_eq!(
        raw["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::configuration_identity("codex", &[])]["A"],
        "First"
    );
    assert_eq!(
        raw["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::configuration_identity("codex", &[])]["B"],
        "Second"
    );
    assert!(!temp
        .path()
        .join("settings.30.instance.no-git.pre-384-v1.json")
        .exists());
}

#[test]
fn legacy_env_source_and_isolated_home_alias_serialize_as_v2_names() {
    let json = r##"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex",
                "color": "#000000",
                "isolateCodexHome": true,
                "envs": [{
                    "key": "OPENAI_API_BASE",
                    "value": "https://example.test",
                    "source": "agentsCommander",
                    "enabled": true
                }]
            }]
        }"##;

    let (settings, _migrated) = super::parse_settings_json(json, "test", None).unwrap();
    let out = serde_json::to_string(&settings).unwrap();

    assert!(settings.agents[0].isolated_home);
    assert_eq!(
        settings.agents[0].envs[0].source,
        CodingAgentEnvSource::System
    );
    assert!(out.contains("\"isolatedHome\":true"));
    assert!(out.contains("\"source\":\"system\""));
    assert!(!out.contains("isolateCodexHome"));
    assert!(!out.contains("agentsCommander"));
}

#[test]
fn validate_agent_commands_allows_plain_claude() {
    let settings = settings_with_agents(&[("Claude", "claude")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn repair_profiles_adds_a_letter_and_a_cells_for_agents() {
    let mut settings = settings_with_agents(&[("Codex", "codex")]);
    settings.coding_agent_profiles.profile_slots.clear();
    settings.coding_agent_profiles.profile_slots.insert(
        "AA".to_string(),
        ProfileSlotConfig {
            label: "bad".into(),
        },
    );

    let changed =
        repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    assert!(changed);
    assert!(settings
        .coding_agent_profiles
        .profile_slots
        .contains_key("A"));
    assert!(!settings
        .coding_agent_profiles
        .profile_slots
        .contains_key("AA"));
    assert!(settings.coding_agent_profiles.profiles_by_agent["agent-0"].contains_key("A"));
}

#[test]
fn stale_full_settings_update_preserves_profiles_envs_and_isolation() {
    let mut current = settings_with_agents(&[("Codex", "codex")]);
    current.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    current.agents[0].isolated_home = true;
    current
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "B".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "codex --current".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );

    let mut stale = current.clone();
    stale.agents[0].envs.clear();
    stale.agents[0].isolated_home = false;
    stale.coding_agent_profiles.profiles_by_agent.clear();
    stale.sidebar_style = "command-center".to_string();

    let merged = merge_protected_coding_agent_settings(&current, stale);

    assert_eq!(merged.sidebar_style, "command-center");
    assert_eq!(merged.agents[0].envs[0].value, "current");
    assert!(merged.agents[0].isolated_home);
    assert!(merged.coding_agent_profiles.profiles_by_agent["agent-0"].contains_key("B"));
}

#[test]
fn validate_agent_commands_rejects_relative_codex_home_env() {
    let mut settings = settings_with_agents(&[("Codex", "codex")]);
    settings.agents[0].envs = vec![CodingAgentEnv {
        key: "CODEX_HOME".to_string(),
        value: "relative/path".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];

    let err = validate_agent_commands(&settings).unwrap_err();

    assert!(err.contains("CODEX_HOME must be an absolute path"), "{err}");
}

#[test]
fn validate_agent_commands_rejects_claude_continue() {
    let settings = settings_with_agents(&[("Claude", "claude --continue")]);
    let err = validate_agent_commands(&settings).unwrap_err();
    assert!(err.contains("Claude commands must not include --continue or -c"));
}

#[test]
fn validate_rejects_banned_continue_flag_in_cell_params() {
    // #597 - the cell holds params only; the provider token (claude) lives in
    // the base. The ban must still catch `--continue` typed in the cell, which
    // only works if validation runs on the COMPOSED command.
    let mut settings = settings_with_agents(&[("Claude", "claude")]);
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--continue".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    let err = validate_agent_commands(&settings).unwrap_err();
    assert!(
        err.contains("must not include --continue"),
        "composed claude + --continue cell must be rejected: {err}"
    );
}

#[test]
fn validate_allows_continue_flag_when_base_is_not_claude() {
    // #597 - the --continue ban is claude-specific. With a codex base the
    // composed `codex --continue` is allowed, proving validation keys off the
    // composed provider token, not the raw cell text.
    let mut settings = settings_with_agents(&[("Codex", "codex")]);
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--continue".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_allows_all_explicit_pi_session_controls_with_provider_overlap() {
    let selectors = [
        "-c",
        "-r",
        "--continue",
        "--continue=true",
        "--resume",
        "--resume=id",
        "--session",
        "--session=id",
        "--session-id",
        "--session-id=id",
        "--fork",
        "--fork=id",
        "--no-session",
        "--no-session=true",
    ];
    for provider in ["claude", "codex", "gemini"] {
        for selector in selectors {
            let command = format!("pi --provider {provider} {selector}");
            let settings = settings_with_agents(&[("Pi", command.as_str())]);
            assert!(
                validate_agent_commands(&settings).is_ok(),
                "provider={provider:?} selector={selector:?}"
            );
        }
    }
}

#[test]
fn validate_agent_commands_allows_explicit_pi_controls_in_composed_profile_cells() {
    let selectors = [
        "-c",
        "-r",
        "--continue",
        "--resume",
        "--session",
        "--session-id",
        "--fork",
        "--no-session",
    ];
    for provider in ["claude", "codex", "gemini"] {
        for selector in selectors {
            let base = format!("pi --provider {provider}");
            let mut settings = settings_with_agents(&[("Pi", base.as_str())]);
            settings
                .coding_agent_profiles
                .profiles_by_agent
                .entry("agent-0".to_string())
                .or_default()
                .insert(
                    "A".to_string(),
                    ProfileCellConfig {
                        id_hash: None,
                        id_hash_debug: None,
                        preflight: None,
                        preflight_timeout_seconds: None,
                        enabled: true,
                        command: format!("--model {provider} {selector}"),
                        env: BTreeMap::new(),
                        notes: String::new(),
                    },
                );
            assert!(
                validate_agent_commands(&settings).is_ok(),
                "provider={provider:?} selector={selector:?}"
            );
        }
    }
}

/// #1873 - a directly configured Muse recipe is user-authored: a Codex-looking
/// value plus manual `resume --last` passes, and the legacy Codex scan never
/// runs. Base recipe and composed profile cell behave the same.
#[test]
fn validate_agent_commands_allows_direct_muse_arguments_with_legacy_provider_collision() {
    for command in [
        "muse",
        "muse --workspace /tmp/codex resume --last",
        "/opt/muse/bin/muse --workspace /tmp/codex resume --last",
        "muse resume 0f3d2a5c-9c1e-4b7e-8f6a-2b1c3d4e5f60",
        "muse --no-session-log --continue -c",
        "muse --model claude-sonnet --provider agy --continue",
    ] {
        let settings = settings_with_agents(&[("Muse", command)]);
        assert!(
            validate_agent_commands(&settings).is_ok(),
            "command={command:?}"
        );
    }
    let mut settings = settings_with_agents(&[("Muse", "muse")]);
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "--workspace /tmp/codex resume --last".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    assert!(validate_agent_commands(&settings).is_ok());
}

/// #1873 - the same token sequence behind an `env` wrapper is not Muse, so
/// the pre-existing Codex manual-resume rejection still fires before spawn.
#[test]
fn validate_agent_commands_rejects_wrapped_muse_collision_before_spawn() {
    for command in [
        "env muse --workspace /tmp/codex resume --last",
        "cmd /C muse --workspace /tmp/codex resume --last",
    ] {
        let settings = settings_with_agents(&[("Muse", command)]);
        let err = validate_agent_commands(&settings).unwrap_err();
        assert_eq!(
                err,
                "Agent \"Muse\": Codex commands must not include resume or --last; AgentsCommander injects codex resume --last automatically",
                "command={command:?}"
            );
    }
}

#[test]
fn validate_agent_commands_allows_plain_codex() {
    let settings = settings_with_agents(&[("Codex", "codex")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_allows_codex_search() {
    let settings = settings_with_agents(&[("Codex", "codex --search")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_allows_explicit_codex_help() {
    let settings = settings_with_agents(&[("Codex", "codex help")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn validate_agent_commands_rejects_codex_resume_last() {
    let settings = settings_with_agents(&[("Codex", "codex resume --last")]);
    let err = validate_agent_commands(&settings).unwrap_err();
    assert!(err.contains("Codex commands must not include resume or --last"));
}

#[test]
fn validate_agent_commands_rejects_cmd_wrapper_codex_resume_last() {
    let settings = settings_with_agents(&[("Codex", "cmd /C codex resume --last")]);
    let err = validate_agent_commands(&settings).unwrap_err();
    assert!(err.contains("Codex commands must not include resume or --last"));
}

#[test]
fn validate_agent_commands_allows_codex_config_value_with_resume_text() {
    let settings =
        settings_with_agents(&[("Codex", "codex -c instruction=\"resume later\" --search")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

// #529 - instructions filename validation surfaces at settings save (G8/Q6).

#[test]
fn validate_agent_commands_rejects_unsafe_instructions_filename() {
    for bad in [
        "../x.md",
        "a/b.md",
        "a\\b.md",
        "C:x.md",
        "CON.md",
        "last_ac_context.md",
    ] {
        let mut settings = settings_with_agents(&[("Codex", "codex")]);
        settings.agents[0].instructions_filename = Some(bad.to_string());
        let err = validate_agent_commands(&settings).unwrap_err();
        assert!(
            err.contains("instructions filename is invalid"),
            "{bad:?} should be rejected, got: {err}"
        );
    }
}

#[test]
fn validate_agent_commands_allows_safe_or_empty_instructions_filename() {
    // Valid names plus empty/whitespace (which means "use the default").
    for ok in ["AGENTS.md", "CLAUDE.md", "MyTeam.md", "  ", ""] {
        let mut settings = settings_with_agents(&[("Codex", "codex")]);
        settings.agents[0].instructions_filename = Some(ok.to_string());
        assert!(
            validate_agent_commands(&settings).is_ok(),
            "{ok:?} should be allowed"
        );
    }
    // Absent (None) is allowed too.
    let settings = settings_with_agents(&[("Codex", "codex")]);
    assert!(validate_agent_commands(&settings).is_ok());
}

#[test]
fn instructions_filename_serde_round_trips_and_omits_when_absent() {
    // Absent -> omitted (skip_serializing_if) and deserializes back to None.
    let mut settings = settings_with_agents(&[("Codex", "codex")]);
    let json = serde_json::to_string(&settings).unwrap();
    assert!(!json.contains("instructionsFilename"));
    let back: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.agents[0].instructions_filename, None);

    // Present -> serialized under the camelCase key and round-trips.
    settings.agents[0].instructions_filename = Some("Squad.md".to_string());
    let json = serde_json::to_string(&settings).unwrap();
    assert!(json.contains("\"instructionsFilename\":\"Squad.md\""));
    let back: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.agents[0].instructions_filename.as_deref(),
        Some("Squad.md")
    );
}

#[test]
fn team_idle_beep_enabled_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.team_idle_beep_enabled);
    s.team_idle_beep_enabled = false;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"teamIdleBeepEnabled\":false"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(!back.team_idle_beep_enabled);
}

#[test]
fn team_idle_beep_enabled_defaults_true_when_missing_from_json() {
    // Old settings.json without the new field must deserialize to true (default_true).
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(s.team_idle_beep_enabled);
}

#[test]
fn telegram_network_poll_error_logging_round_trips_through_serde() {
    let s = AppSettings {
        telegram_network_poll_error_logging: TelegramNetworkPollErrorLogging {
            first_failure_level: TelegramPollFailureLogLevel::Debug,
            transient_repeat_level: TelegramPollFailureLogLevel::Warn,
            sustained_level: TelegramPollFailureLogLevel::Error,
            sustained_after_seconds: 12,
            sustained_repeat_seconds: 34,
            recovery_level: TelegramPollRecoveryLogLevel::Warn,
        },
        ..AppSettings::default()
    };

    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"telegramNetworkPollErrorLogging\""));
    assert!(json.contains("\"sustainedAfterSeconds\":12"));

    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        back.telegram_network_poll_error_logging
            .sustained_after_seconds,
        12
    );
    assert_eq!(
        back.telegram_network_poll_error_logging.recovery_level,
        TelegramPollRecoveryLogLevel::Warn
    );
}

#[test]
fn telegram_network_poll_error_logging_defaults_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert_eq!(
        s.telegram_network_poll_error_logging,
        TelegramNetworkPollErrorLogging::default()
    );
}

#[test]
fn telegram_network_poll_error_logging_field_defaults_are_backfilled() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "telegramNetworkPollErrorLogging": {
                "sustainedAfterSeconds": 5
            }
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize partial policy");
    assert_eq!(
        s.telegram_network_poll_error_logging
            .sustained_after_seconds,
        5
    );
    assert_eq!(
        s.telegram_network_poll_error_logging.first_failure_level,
        TelegramPollFailureLogLevel::Warn
    );
    assert_eq!(
        s.telegram_network_poll_error_logging.transient_repeat_level,
        TelegramPollFailureLogLevel::Debug
    );
    assert_eq!(
        s.telegram_network_poll_error_logging.recovery_level,
        TelegramPollRecoveryLogLevel::Info
    );
}

/// Test 8 (#2265): a settings file written before this change carries no
/// `jev*` key and must load with every documented default.
#[test]
fn jev_fields_default_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert_eq!(s.jev_api_key, "");
    assert_eq!(s.jev_model, "jev-1.13.0");
    assert_eq!(s.jev_endpoint, "https://api.typesafe.ai/v1/systemone");
    assert_eq!(s.jev_timeout_secs, 20);
    assert_eq!(s.jev_threshold, 0.70);
    assert_eq!(s.jev_margin, 0.15);
}

/// Test 9 (#2265): the key default is empty (not a placeholder) and the
/// model default is the pinned literal, so a later floating tag is a
/// deliberate, visible change.
#[test]
fn jev_api_key_defaults_empty_and_model_is_pinned() {
    let s = AppSettings::default();
    assert_eq!(s.jev_api_key, "");
    assert_eq!(s.jev_model, "jev-1.13.0");
    assert_ne!(s.jev_model, "jev-latest");
}

#[test]
fn sounds_enabled_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.sounds_enabled);
    s.sounds_enabled = false;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"soundsEnabled\":false"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(!back.sounds_enabled);
}

#[test]
fn sounds_enabled_defaults_true_when_missing_from_json() {
    // Old settings.json with only team_idle_beep_enabled (and no soundsEnabled
    // field) must deserialize with sounds_enabled = true so existing users
    // remain audible until they explicitly mute.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "teamIdleBeepEnabled": false,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(s.sounds_enabled);
    // Existing per-feature toggle is honored as-is.
    assert!(!s.team_idle_beep_enabled);
}

#[test]
fn rail_collapse_fields_round_trip_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.rail_collapsed_projects.is_empty());
    assert!(!s.rail_favorites_collapsed);

    s.rail_collapsed_projects = vec!["c:/foo/bar".to_string(), "d:/baz".to_string()];
    s.rail_favorites_collapsed = true;

    let json = serde_json::to_string(&s).expect("serialize");
    // (#965) camelCase guard. A broken `rename_all` would otherwise surface only as a
    // silently dead frontend, since the TS side reads `railCollapsedProjects` /
    // `railFavoritesCollapsed`.
    assert!(
        json.contains("\"railCollapsedProjects\":[\"c:/foo/bar\",\"d:/baz\"]"),
        "{json}"
    );
    assert!(json.contains("\"railFavoritesCollapsed\":true"), "{json}");

    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.rail_collapsed_projects, s.rail_collapsed_projects);
    assert!(back.rail_favorites_collapsed);
}

#[test]
fn rail_collapse_fields_default_when_missing_from_json() {
    // A legacy settings.json carries neither key. Build one by round-tripping a
    // default and deleting them, so every other field stays present and the test
    // isolates exactly the `#[serde(default)]` behavior.
    let mut value = serde_json::to_value(AppSettings::default()).expect("to_value");
    let obj = value.as_object_mut().expect("settings object");
    obj.remove("railCollapsedProjects");
    obj.remove("railFavoritesCollapsed");

    let s: AppSettings = serde_json::from_value(value).expect("deserialize legacy");

    assert!(s.rail_collapsed_projects.is_empty());
    assert!(!s.rail_favorites_collapsed);
}

#[test]
fn theme_light_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(!s.theme_light);
    s.theme_light = true;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"themeLight\":true"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.theme_light);
}

#[test]
fn theme_light_defaults_false_when_missing_from_json() {
    // Settings without themeLight now open dark by default.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.theme_light);
}

#[test]
fn theme_light_explicit_false_survives_round_trip() {
    // Once a user explicitly disables light mode, the value must survive
    // serialize/deserialize without being altered by the serde default.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "themeLight": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize");
    assert!(!s.theme_light);
    let out = serde_json::to_string(&s).expect("serialize");
    let back: AppSettings = serde_json::from_str(&out).expect("re-deserialize");
    assert!(!back.theme_light);
}

#[test]
fn spec_board_enabled_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(!s.spec_board_enabled);
    let default_json = serde_json::to_string(&s).expect("serialize default");
    assert!(default_json.contains("\"specBoardEnabled\":false"));

    s.spec_board_enabled = true;

    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"specBoardEnabled\":true"));

    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.spec_board_enabled);
}

#[test]
fn resource_monitor_settings_round_trip_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.resource_monitor_enabled);
    assert_eq!(s.max_concurrent_agent_processes, 32);
    assert_eq!(s.resource_watchdog_action, ResourceWatchdogAction::Warn);
    assert_eq!(s.agent_group_warn_private_bytes, 8 * 1024 * 1024 * 1024);
    assert_eq!(s.agent_group_kill_private_bytes, 12 * 1024 * 1024 * 1024);
    assert_eq!(s.agent_process_kill_private_bytes, 12 * 1024 * 1024 * 1024);
    assert!(s.resource_keep_last_snapshot);
    assert!(s.resource_backoff_polling);

    s.resource_monitor_enabled = false;
    s.max_concurrent_agent_processes = 5;
    s.resource_watchdog_action = ResourceWatchdogAction::KillGroup;
    s.agent_group_warn_private_bytes = 128;
    s.agent_group_kill_private_bytes = 256;
    s.agent_process_kill_private_bytes = 512;
    s.resource_keep_last_snapshot = false;
    s.resource_backoff_polling = false;

    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"resourceMonitorEnabled\":false"));
    assert!(json.contains("\"resourceWatchdogAction\":\"killGroup\""));

    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(!back.resource_monitor_enabled);
    assert_eq!(back.max_concurrent_agent_processes, 5);
    assert_eq!(
        back.resource_watchdog_action,
        ResourceWatchdogAction::KillGroup
    );
    assert_eq!(back.agent_group_warn_private_bytes, 128);
    assert_eq!(back.agent_group_kill_private_bytes, 256);
    assert_eq!(back.agent_process_kill_private_bytes, 512);
    assert!(!back.resource_keep_last_snapshot);
    assert!(!back.resource_backoff_polling);
}

#[test]
fn coordinator_clock_settings_default_when_keys_absent() {
    // #552: an old settings.json (no coordinator-* keys) must deserialize
    // cleanly to the documented defaults, no migration.
    // Serialize a default, strip ONLY these coordinator keys, deserialize back.
    let mut value =
        serde_json::to_value(AppSettings::default()).expect("serialize default to value");
    let obj = value
        .as_object_mut()
        .expect("settings serializes to an object");
    obj.remove("coordinatorIdleBadgeYellowMinutes");
    obj.remove("coordinatorIdleBadgeRedMinutes");
    obj.remove("coordinatorAutoCloseEnabled");
    obj.remove("coordinatorAutoCloseMinutes");
    obj.remove("coordinatorAutoCloseSkipTelegramAssigned");

    let back: AppSettings = serde_json::from_value(value).expect("deserialize without keys");
    assert!(back.coordinator_auto_close_enabled);
    assert_eq!(back.coordinator_auto_close_minutes, 60);
    assert!(!back.coordinator_auto_close_skip_telegram_assigned);
    assert_eq!(back.coordinator_idle_badge_yellow_minutes, 30);
    assert_eq!(back.coordinator_idle_badge_red_minutes, 60);
}

#[test]
fn coordinator_auto_close_skip_telegram_assigned_round_trips() {
    let s = AppSettings {
        coordinator_auto_close_skip_telegram_assigned: true,
        ..AppSettings::default()
    };

    let json = serde_json::to_value(&s).expect("serialize settings");
    assert_eq!(
        json.get("coordinatorAutoCloseSkipTelegramAssigned"),
        Some(&serde_json::Value::Bool(true))
    );

    let back: AppSettings = serde_json::from_value(json).expect("deserialize settings");
    assert!(back.coordinator_auto_close_skip_telegram_assigned);
}

#[test]
fn resolve_auto_self_clear_precedence_table() {
    // #640 §5 resolution table. Master-first kill switch, then per-agent
    // override, then the class-aware default computed at the call site.
    let mut s = AppSettings::default();

    // master ON, no override, class default ON (coordinator/Root) -> ON.
    assert!(super::resolve_auto_self_clear(&s, "tech-lead", true));
    // master ON, no override, class default OFF (specialist) -> OFF.
    assert!(!super::resolve_auto_self_clear(&s, "dev-rust", false));

    // explicit per-agent opt-in overrides a specialist's OFF class default.
    s.auto_self_clear_by_agent
        .insert("dev-rust".to_string(), true);
    assert!(super::resolve_auto_self_clear(&s, "dev-rust", false));
    // explicit per-agent opt-out overrides a coordinator's ON class default.
    s.auto_self_clear_by_agent
        .insert("tech-lead".to_string(), false);
    assert!(!super::resolve_auto_self_clear(&s, "tech-lead", true));

    // master OFF is an absolute kill switch: off for everyone, and it wins
    // even over a per-agent opt-in.
    s.auto_self_clear_enabled = false;
    assert!(!super::resolve_auto_self_clear(&s, "tech-lead", true));
    assert!(!super::resolve_auto_self_clear(&s, "dev-rust", true));
}

#[test]
fn resolve_auto_self_clear_empty_name_uses_class_default() {
    // #640 L1: the Root derives to an empty name (no `_agent_`/`__agent_`
    // prefix); with no "" key it falls through to the class default. Same
    // path covers any name-derivation failure.
    let s = AppSettings::default();
    assert!(super::resolve_auto_self_clear(&s, "", true));
    assert!(!super::resolve_auto_self_clear(&s, "", false));
}

#[test]
fn auto_self_clear_defaults_when_keys_absent() {
    // #640: an old settings.json (no auto-self-clear keys) must deserialize
    // to the documented defaults: master ON + empty per-agent map.
    let mut value =
        serde_json::to_value(AppSettings::default()).expect("serialize default to value");
    let obj = value
        .as_object_mut()
        .expect("settings serializes to an object");
    obj.remove("autoSelfClearEnabled");
    obj.remove("autoSelfClearByAgent");

    let back: AppSettings = serde_json::from_value(value).expect("deserialize without keys");
    assert!(back.auto_self_clear_enabled);
    assert!(back.auto_self_clear_by_agent.is_empty());
}

#[test]
fn auto_self_clear_round_trips_through_serde() {
    let mut s = AppSettings {
        auto_self_clear_enabled: false,
        ..AppSettings::default()
    };
    s.auto_self_clear_by_agent
        .insert("dev-rust".to_string(), true);
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"autoSelfClearEnabled\":false"));
    assert!(json.contains("\"autoSelfClearByAgent\":{\"dev-rust\":true}"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(!back.auto_self_clear_enabled);
    assert_eq!(back.auto_self_clear_by_agent.get("dev-rust"), Some(&true));
}

#[test]
fn agent_auto_update_by_command_default_empty_and_round_trip_camel_case() {
    // #1327: the per-command override map defaults to empty on a fresh
    // settings object AND survives the serde round trip with camelCase keys.
    let defaults = AppSettings::default();
    assert!(defaults.agent_auto_update_by_command.is_empty());

    let mut s = AppSettings::default();
    s.agent_auto_update_by_command
        .insert("claude".to_string(), true);
    s.agent_auto_update_by_command
        .insert("codex".to_string(), false);
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"agentAutoUpdateByCommand\":{\"claude\":true,\"codex\":false}"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.agent_auto_update_by_command.get("claude"), Some(&true));
    assert_eq!(back.agent_auto_update_by_command.get("codex"), Some(&false));
}

#[test]
fn agent_auto_update_by_command_absent_key_deserializes_empty() {
    // An old settings.json without the #1327 key deserializes to an empty
    // map (serde default), never an error.
    let mut value =
        serde_json::to_value(AppSettings::default()).expect("serialize default to value");
    let obj = value.as_object_mut().expect("settings object");
    obj.remove("agentAutoUpdateByCommand");
    let back: AppSettings = serde_json::from_value(value).expect("deserialize without keys");
    assert!(back.agent_auto_update_by_command.is_empty());
}

#[test]
fn validate_resource_settings_rejects_invalid_limits() {
    let s = AppSettings {
        max_concurrent_agent_processes: 0,
        ..AppSettings::default()
    };
    assert!(validate_resource_settings(&s)
        .unwrap_err()
        .contains("maxConcurrentAgentProcesses"));

    let s = AppSettings {
        agent_group_warn_private_bytes: 10,
        agent_group_kill_private_bytes: 9,
        ..AppSettings::default()
    };
    assert!(validate_resource_settings(&s)
        .unwrap_err()
        .contains("agentGroupWarnPrivateBytes"));

    let s = AppSettings {
        agent_group_warn_private_bytes: 1,
        agent_group_kill_private_bytes: 0,
        ..AppSettings::default()
    };
    assert!(validate_resource_settings(&s)
        .unwrap_err()
        .contains("agentGroupKillPrivateBytes"));

    let s = AppSettings {
        agent_group_warn_private_bytes: 1,
        agent_group_kill_private_bytes: 10,
        agent_process_kill_private_bytes: 0,
        ..AppSettings::default()
    };
    assert!(validate_resource_settings(&s)
        .unwrap_err()
        .contains("agentProcessKillPrivateBytes"));
}

#[test]
fn validate_resource_settings_accepts_max_concurrent_above_legacy_cap() {
    // #565: the old 1..=16 upper ceiling is gone. Any value >= 1 is accepted;
    // the user is responsible for sizing it to their machine.
    for value in [17u32, 32, 64, 256, 10_000] {
        let s = AppSettings {
            max_concurrent_agent_processes: value,
            ..AppSettings::default()
        };
        assert!(
            validate_resource_settings(&s).is_ok(),
            "expected max_concurrent_agent_processes={value} to validate"
        );
    }
}

#[test]
fn validate_api_server_settings_rejects_invalid_bind_and_port() {
    let mut empty_bind = AppSettings {
        api_server_bind: "   ".to_string(),
        ..AppSettings::default()
    };
    assert!(validate_api_server_settings(&mut empty_bind)
        .unwrap_err()
        .contains("apiServerBind"));

    let mut zero_port = AppSettings {
        api_server_port: 0,
        ..AppSettings::default()
    };
    assert!(validate_api_server_settings(&mut zero_port)
        .unwrap_err()
        .contains("apiServerPort"));

    let mut hostname = AppSettings {
        api_server_bind: "localhost".to_string(),
        ..AppSettings::default()
    };
    assert!(validate_api_server_settings(&mut hostname)
        .unwrap_err()
        .contains("apiServerBind"));
}

#[test]
fn validate_api_server_settings_accepts_ip_binds_and_trims() {
    for bind in ["127.0.0.1", "0.0.0.0", "::1", "::"] {
        let mut settings = AppSettings {
            api_server_bind: format!(" {bind} "),
            ..AppSettings::default()
        };
        validate_api_server_settings(&mut settings).unwrap();
        assert_eq!(settings.api_server_bind, bind);
    }
}

#[test]
fn coord_sort_by_activity_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(!s.coord_sort_by_activity);
    s.coord_sort_by_activity = true;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"coordSortByActivity\":true"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.coord_sort_by_activity);
}

#[test]
fn activity_log_enabled_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(!s.activity_log_enabled);
    s.activity_log_enabled = true;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"activityLogEnabled\":true"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.activity_log_enabled);
}

#[test]
fn log_level_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.log_level.is_none());
    s.log_level = Some("info,agentscommander_lib::config::teams=debug".to_string());
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"logLevel\":\"info,agentscommander_lib::config::teams=debug\""));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        back.log_level,
        Some("info,agentscommander_lib::config::teams=debug".to_string())
    );
}

#[test]
fn log_level_defaults_to_none_when_missing_from_json() {
    // Old settings.json without the new field must deserialize to None.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(s.log_level.is_none());
}

#[test]
fn read_log_level_only_returns_value_when_present() {
    let dir = std::env::temp_dir().join(format!("rlol-present-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(
        &path,
        r#"{"logLevel":"info,agentscommander_lib::config::teams=debug","other":"x"}"#,
    )
    .unwrap();
    assert_eq!(
        super::read_log_level_from_path(&path),
        Some("info,agentscommander_lib::config::teams=debug".to_string())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_log_level_only_returns_none_when_log_level_missing() {
    let dir = std::env::temp_dir().join(format!("rlol-missing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"other":"value"}"#).unwrap();
    assert_eq!(super::read_log_level_from_path(&path), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_log_level_only_returns_none_when_settings_missing() {
    let path = std::env::temp_dir().join(format!("rlol-no-such-file-{}.json", std::process::id()));
    // Intentionally do not create the file.
    assert_eq!(super::read_log_level_from_path(&path), None);
}

#[test]
fn read_log_level_only_returns_none_when_json_malformed() {
    let dir = std::env::temp_dir().join(format!("rlol-malformed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, "{ invalid json no closing brace").unwrap();
    assert_eq!(super::read_log_level_from_path(&path), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_log_level_only_returns_some_empty_string_when_log_level_is_empty() {
    // Asserts read_log_level_only returns Some("") (not None) when logLevel is the
    // empty string — the helper preserves the user's intent (the field is set, just
    // empty). Downstream filter machinery handles the rest, with semantics distinct
    // from the malformed-string case: empty-string → parse_filters("") produces 0
    // directives → env_filter's hidden {None, Error} default applies → Error-only logs
    // flow globally; malformed-string → 1 non-matching directive → all
    // agentscommander* logs suppressed. The helper is symmetric on both inputs
    // (returns Some(value)); the observable difference is at env_filter::Builder::build.
    let dir = std::env::temp_dir().join(format!("rlol-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"logLevel":"","other":"value"}"#).unwrap();
    assert_eq!(super::read_log_level_from_path(&path), Some(String::new()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_from_path_returns_true_when_true() {
    let dir = std::env::temp_dir().join(format!("rale-present-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"activityLogEnabled":true,"other":"x"}"#).unwrap();
    assert!(super::read_activity_log_enabled_from_path(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_from_path_defaults_false_when_key_missing() {
    let dir = std::env::temp_dir().join(format!("rale-missing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"other":"value"}"#).unwrap();
    assert!(!super::read_activity_log_enabled_from_path(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_from_path_defaults_false_when_null() {
    let dir = std::env::temp_dir().join(format!("rale-null-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"activityLogEnabled":null}"#).unwrap();
    assert!(!super::read_activity_log_enabled_from_path(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_from_path_defaults_false_when_json_malformed() {
    let dir = std::env::temp_dir().join(format!("rale-malformed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, "{ invalid json no closing brace").unwrap();
    assert!(!super::read_activity_log_enabled_from_path(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_only_defaults_false_when_settings_missing() {
    let path = std::env::temp_dir().join(format!("rale-no-such-file-{}.json", std::process::id()));
    assert!(!super::read_activity_log_enabled_from_path(&path));
}

// ── #778: disk-authoritative project_paths (Design S) ────────────────────

fn settings_with_project_paths(paths: &[&str]) -> AppSettings {
    AppSettings {
        project_paths: paths.iter().map(|p| p.to_string()).collect(),
        project_path: paths.first().map(|p| p.to_string()),
        ..AppSettings::default()
    }
}

fn settings_with_project_and_archived_paths(paths: &[&str], archived: &[&str]) -> AppSettings {
    AppSettings {
        project_paths: paths.iter().map(|p| p.to_string()).collect(),
        project_path: paths.first().map(|p| p.to_string()),
        archived_project_paths: archived.iter().map(|p| p.to_string()).collect(),
        ..AppSettings::default()
    }
}

/// #1077: create a real AC project dir (`<parent>/<name>/.ac`) and return its
/// display-canonical path, so the six-field decoder validates it and the
/// SELECTED runtime path equals the stored string. Used by preserve/reconcile
/// tests that assert the returned settings mirror the preserved disk list.
fn real_ac_project_path(parent: &std::path::Path, name: &str) -> String {
    let dir = parent.join(name);
    std::fs::create_dir_all(dir.join(".ac")).unwrap();
    let canon = std::fs::canonicalize(&dir).unwrap();
    crate::config::projects::display_canonical(&canon.to_string_lossy())
}

#[test]
fn read_project_paths_from_disk_returns_list_and_head() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::save_settings_to_path(&settings_with_project_paths(&["C:/a", "C:/b"]), &path).unwrap();
    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
    assert_eq!(
        lists.project_paths,
        vec!["C:/a".to_string(), "C:/b".to_string()]
    );
    assert_eq!(lists.project_path.as_deref(), Some("C:/a"));
}

#[test]
fn read_project_paths_from_disk_returns_none_when_file_absent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("does-not-exist.json");
    // NotFound means there is no disk truth to substitute.
    assert_eq!(super::read_project_paths_from_disk(&path).unwrap(), None);
}

#[test]
fn read_project_paths_from_disk_returns_empty_when_key_absent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"themeLight":true}"#).unwrap();
    assert!(super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap()
        .project_paths
        .is_empty());
}

#[test]
fn read_project_paths_from_disk_returns_empty_when_key_null() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":null}"#).unwrap();
    assert!(super::read_project_paths_from_disk(&path)
        .unwrap()
        .unwrap()
        .project_paths
        .is_empty());
}

#[test]
fn read_project_paths_from_disk_aborts_when_key_wrong_type() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":"C:/a"}"#).unwrap();
    assert!(super::read_project_paths_from_disk(&path).is_err());
}

#[test]
fn read_project_paths_from_disk_accepts_legit_empty_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":[]}"#).unwrap();
    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
    assert!(lists.project_paths.is_empty());
    assert_eq!(lists.project_path, None);
}

#[test]
fn read_project_paths_from_disk_returns_archived_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::save_settings_to_path(
        &settings_with_project_and_archived_paths(&["C:/a"], &["C:/old"]),
        &path,
    )
    .unwrap();

    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

    assert_eq!(
        lists.archived_project_paths,
        Some(vec!["C:/old".to_string()])
    );
}

#[test]
fn read_project_paths_from_disk_returns_archived_none_when_key_absent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":["C:/a"],"projectPath":"C:/a"}"#).unwrap();

    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

    assert_eq!(lists.project_paths, vec!["C:/a".to_string()]);
    assert_eq!(lists.project_path.as_deref(), Some("C:/a"));
    assert_eq!(lists.archived_project_paths, None);
}

#[test]
fn read_project_paths_from_disk_returns_archived_none_when_key_null() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a"],"archivedProjectPaths":null}"#,
    )
    .unwrap();

    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

    assert_eq!(lists.archived_project_paths, None);
}

#[test]
fn read_project_paths_from_disk_returns_archived_some_empty_when_key_is_empty_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a"],"archivedProjectPaths":[]}"#,
    )
    .unwrap();

    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

    assert_eq!(lists.archived_project_paths, Some(vec![]));
}

#[test]
fn read_project_paths_from_disk_aborts_when_archived_key_wrong_type() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a"],"archivedProjectPaths":"C:/old"}"#,
    )
    .unwrap();

    assert!(super::read_project_paths_from_disk(&path).is_err());
}

#[test]
fn read_project_paths_from_disk_aborts_when_archived_list_has_non_string_element() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a"],"archivedProjectPaths":["C:/old",123]}"#,
    )
    .unwrap();

    assert!(super::read_project_paths_from_disk(&path).is_err());
}

#[test]
fn read_project_paths_from_disk_drops_non_string_project_path_elements() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["C:/a",123],"archivedProjectPaths":["C:/old"]}"#,
    )
    .unwrap();

    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

    assert_eq!(lists.project_paths, vec!["C:/a".to_string()]);
    assert_eq!(
        lists.archived_project_paths,
        Some(vec!["C:/old".to_string()])
    );
}

#[test]
fn read_project_paths_from_disk_aborts_on_unparseable_json() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, "{ not valid json").unwrap();
    // G2: unparseable is NOT NotFound, so the reader aborts (Err), never disk=[].
    assert!(super::read_project_paths_from_disk(&path).is_err());
}

#[test]
fn read_project_paths_from_disk_aborts_on_non_notfound_read_error() {
    // G2: a directory at the path yields a non-NotFound read error; the reader
    // must abort (Err), not degrade to empty (which would silently drop a project).
    let temp = tempfile::tempdir().unwrap();
    let dir_as_path = temp.path().join("iam-a-dir");
    std::fs::create_dir_all(&dir_as_path).unwrap();
    assert!(super::read_project_paths_from_disk(&dir_as_path).is_err());
}

#[test]
fn save_settings_preserve_keeps_disk_project_paths_on_passthrough() {
    // Marquee preserve: disk has [A, X] (X = an append this in-memory candidate
    // never saw); a whole-object save that changed only an unrelated field must
    // leave project_paths = [A, X] on disk (fail-safe) AND persist the field.
    // #1077: real AC dirs so the selected runtime paths equal the preserved list.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let x = real_ac_project_path(temp.path(), "X");
    super::save_settings_to_path(&settings_with_project_paths(&[&a, &x]), &path).unwrap();

    let mut candidate = settings_with_project_paths(&[&a]); // stale, missing X
    candidate.sidebar_style = "deep-space".to_string(); // unrelated GUI field
    let written = super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

    let reloaded: AppSettings = super::load_settings_from_path(&path);
    assert_eq!(reloaded.project_paths, vec![a.clone(), x.clone()]); // X preserved, not clobbered
    assert_eq!(reloaded.project_path.as_deref(), Some(a.as_str()));
    assert_eq!(reloaded.sidebar_style, "deep-space"); // unrelated field persisted
                                                      // #1077: the returned settings carry the SELECTED (validated) projection,
                                                      // which equals the preserved disk list because both dirs are real.
    assert_eq!(written.project_paths, vec![a.clone(), x.clone()]);
    assert_eq!(written.project_path.as_deref(), Some(a.as_str()));
}

#[test]
fn refresh_project_paths_from_path_clears_stale_archived_when_authority_omits_key() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, r#"{"projectPaths":["A"],"projectPath":"A"}"#).unwrap();
    let mut settings = settings_with_project_and_archived_paths(&["stale"], &["Archived"]);

    super::refresh_project_paths_from_path(&mut settings, &path).unwrap();

    assert_eq!(settings.project_paths, vec!["A".to_string()]);
    assert_eq!(settings.project_path.as_deref(), Some("A"));
    assert!(settings.archived_project_paths.is_empty());
}

#[test]
fn refresh_project_paths_from_path_clears_archived_list_when_disk_says_empty() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"projectPaths":["A"],"projectPath":"A","archivedProjectPaths":[]}"#,
    )
    .unwrap();
    let mut settings = settings_with_project_and_archived_paths(&["stale"], &["Archived"]);

    super::refresh_project_paths_from_path(&mut settings, &path).unwrap();

    assert_eq!(settings.project_paths, vec!["A".to_string()]);
    assert_eq!(settings.project_path.as_deref(), Some("A"));
    assert!(settings.archived_project_paths.is_empty());
}

#[test]
fn save_settings_preserves_disk_archived_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let archived_a = real_ac_project_path(temp.path(), "ArchivedA");
    super::save_settings_to_path(
        &settings_with_project_and_archived_paths(&[&a], &[&archived_a]),
        &path,
    )
    .unwrap();
    let candidate = settings_with_project_and_archived_paths(&[&a], &["ArchivedB"]);

    let written = super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

    let reloaded: AppSettings =
        crate::config::settings::load_settings_from_path(std::path::Path::new(&path));
    assert_eq!(reloaded.archived_project_paths, vec![archived_a.clone()]);
    assert_eq!(written.archived_project_paths, vec![archived_a.clone()]);
}

#[test]
fn save_settings_discards_archived_draft_when_authority_omits_key() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let archived = real_ac_project_path(temp.path(), "Archived");
    // Seed a whole, valid AppSettings object but with NO archivedProjectPaths
    // key (the preserve writer's disk gate requires a valid whole object).
    let mut seed = serde_json::to_value(settings_with_project_paths(&[&a])).unwrap();
    seed.as_object_mut().unwrap().remove("archivedProjectPaths");
    std::fs::write(&path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();

    let candidate = settings_with_project_and_archived_paths(&[&a], &[&archived]);
    let written = super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

    let reloaded: AppSettings =
        crate::config::settings::load_settings_from_path(std::path::Path::new(&path));
    assert_eq!(written.project_paths, vec![a.clone()]);
    assert_eq!(written.project_path.as_deref(), Some(a.as_str()));
    // An absent authoritative group cannot be replaced by a caller draft.
    assert!(written.archived_project_paths.is_empty());
    assert!(reloaded.archived_project_paths.is_empty());
}

#[test]
fn save_settings_with_project_paths_writes_verbatim() {
    // The explicit writer persists the in-memory list as-is (deliberate mutation).
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::save_settings_to_path(&settings_with_project_paths(&["A"]), &path).unwrap();
    super::save_settings_to_path(&settings_with_project_paths(&["A", "Y"]), &path).unwrap();
    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
    assert_eq!(lists.project_paths, vec!["A".to_string(), "Y".to_string()]);
}

#[test]
fn save_settings_with_project_paths_writes_archived_verbatim() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::save_settings_with_project_paths_to_path(
        &settings_with_project_and_archived_paths(&["A"], &["Archived"]),
        &path,
    )
    .unwrap();

    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();

    assert_eq!(
        lists.archived_project_paths,
        Some(vec!["Archived".to_string()])
    );
}

#[test]
fn resync_reconciles_missing_and_conflict_without_duplication() {
    // Grinch Defect 1: a decoded state carrying a MISSING and a CONFLICT
    // record, mutated across register/remove, must never duplicate/resurrect
    // those records, and remove must actually remove.
    use crate::config::projects::{
        IssueKind, ProjectPathPersistenceState, ProjectSource, RawStringField, RepairKind,
        ResolvedPair,
    };
    let pair =
        |idx: usize, raw: &str, selected: Option<&str>, issue: Option<IssueKind>| ResolvedPair {
            source: ProjectSource::ProjectPaths,
            index: Some(idx),
            raw_absolute: RawStringField::string(raw),
            raw_relative: RawStringField::absent(),
            absolute_side: super::absent_side(),
            relative_side: super::absent_side(),
            selected: selected.map(String::from),
            selected_canonical_raw: selected.map(String::from),
            selected_identity: None,
            issue,
            repair: RepairKind::None,
        };
    let state = ProjectPathPersistenceState {
        pairs: vec![
            pair(0, "A", Some("A"), None),
            pair(1, "M", None, Some(IssueKind::Missing)),
            pair(2, "C", None, Some(IssueKind::Conflict)),
        ],
        selected_head: Some("A".to_string()),
        active_registration_count: 3,
        archived_registration_count: 0,
        active_companion_present: true,
        archived_companion_present: false,
        has_genuine_singular: false,
        active_reconcile_eligible: false,
        archived_reconcile_eligible: false,
        structural_issues: Vec::new(),
        runtime_authoritative: false,
    };
    let mut s = AppSettings {
        project_path_state: std::sync::Arc::new(state),
        ..AppSettings::default()
    };
    // Raw runtime (3-field refresh) carries all three stored absolutes.
    s.project_paths = vec!["A".to_string(), "M".to_string(), "C".to_string()];

    // Register D.
    s.project_paths.push("D".to_string());
    super::resync_project_state_from_runtime(&mut s);
    {
        let st = &s.project_path_state;
        assert_eq!(
            st.active_registration_count, 4,
            "no duplication on register"
        );
        let keys: Vec<String> = st
            .active_pairs()
            .iter()
            .map(|p| p.raw_absolute.value.clone().unwrap())
            .collect();
        assert_eq!(
            keys,
            vec!["A", "M", "C", "D"],
            "order preserved, no duplicates"
        );
        assert_eq!(st.active_pairs()[1].issue, Some(IssueKind::Missing));
        assert_eq!(st.active_pairs()[2].issue, Some(IssueKind::Conflict));
        assert!(st.active_pairs()[3].issue.is_none());
    }

    // Remove M ("Remove from list" on the missing entry).
    s.project_paths = vec!["A".to_string(), "C".to_string(), "D".to_string()];
    super::resync_project_state_from_runtime(&mut s);
    {
        let st = &s.project_path_state;
        assert_eq!(
            st.active_registration_count, 3,
            "the missing record must actually be removed, not resurrected"
        );
        let keys: Vec<String> = st
            .active_pairs()
            .iter()
            .map(|p| p.raw_absolute.value.clone().unwrap())
            .collect();
        assert_eq!(keys, vec!["A", "C", "D"]);
        assert_eq!(
            st.active_pairs()[1].issue,
            Some(IssueKind::Conflict),
            "unrelated conflict preserved through the removal"
        );
    }
}

#[test]
fn resync_persists_mutation_against_decoded_state() {
    // Regression: a mutator that changes only the runtime lists against a
    // DECODED (non-synthesized) hidden state must resync so the reconcile
    // write records the new project instead of copying the stale disk list.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = real_ac_project_path(temp.path(), "A");
    let b = real_ac_project_path(temp.path(), "B");
    super::save_settings_to_path(&settings_with_project_paths(&[&a]), &path).unwrap();

    // A decoded hidden state as the loader produces (runtime_authoritative=false).
    let value = serde_json::json!({ "projectPaths": [a.clone()], "projectPath": a.clone() });
    let state = super::decode_project_state(
        value.as_object().unwrap(),
        None,
        &crate::config::projects::FsCandidateResolver,
    );
    assert!(!state.runtime_authoritative);
    let mut settings = settings_with_project_paths(&[&a]);
    settings.project_path_state = std::sync::Arc::new(state);

    // Register B into the runtime list, then resync + reconcile-write.
    settings.project_paths.push(b.clone());
    super::resync_project_state_from_runtime(&mut settings);
    super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();

    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
    assert_eq!(lists.project_paths, vec![a, b]);
}

/// #1171, 9.2.17 - **the regression a plain `BTreeMap<String, WatcherConfig>` would let
/// through, and whose failure mode is the whole settings file.**
///
/// Three hand-written mistakes, one per entry: a capitalized enum variant, a scalar where
/// a list belongs, and a quoted number. Without the per-entry wrapper, ONE of them makes
/// `serde_json::from_value::<AppSettings>` fail, `load_settings_from_path` replaces
/// everything with `AppSettings::default()`, and AgentsCommander starts with no agents
/// configured and one log line - after which every save is refused by the #1077 gate.
#[test]
fn one_malformed_watcher_does_not_take_the_settings_file_down() {
    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": ["-NoLogo"],
        "agents": [
            { "id": "a1", "label": "Claude", "command": "claude", "color": "#fff" },
            { "id": "a2", "label": "Codex", "command": "codex", "color": "#000" }
        ],
        "watchers": {
            "bad-mode": { "mode": "State", "pattern": "x" },
            "bad-commands": { "mode": "occurrence", "pattern": "x", "commands": "claude" },
            "bad-window": { "mode": "occurrence", "pattern": "x", "dedupeWindowMs": "2000" },
            "good": { "mode": "state", "pattern": "Permission required" }
        }
    })
    .to_string();

    let (settings, _) = super::parse_settings_json(&contents, "test", None).expect(
        "a malformed watcher must never fail the whole file: that is what the wrapper is for",
    );

    // Every OTHER setting survived.
    assert_eq!(settings.agents.len(), 2);
    assert_eq!(settings.default_shell, "powershell.exe");

    // Every entry is still there, and exactly one of them resolved.
    assert_eq!(settings.watchers.len(), 4);
    let valid: Vec<&str> = settings
        .watchers
        .iter()
        .filter(|(_, entry)| entry.valid().is_some())
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(valid, vec!["good"]);

    // ...and the three bad ones are kept VERBATIM, so a save writes back the user's bytes
    // instead of deleting what it could not read.
    let round_tripped = serde_json::to_value(&settings.watchers).expect("serializes");
    assert_eq!(
        round_tripped["bad-mode"],
        serde_json::json!({ "mode": "State", "pattern": "x" })
    );
    assert_eq!(
        round_tripped["bad-commands"],
        serde_json::json!({ "mode": "occurrence", "pattern": "x", "commands": "claude" })
    );
    assert_eq!(
        round_tripped["bad-window"],
        serde_json::json!({ "mode": "occurrence", "pattern": "x", "dedupeWindowMs": "2000" })
    );
}

/// #1171, 9.2.18 - a user who configures nothing never sees either new key appear.
#[test]
fn a_settings_file_with_no_watchers_round_trips_without_the_new_keys() {
    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": []
    })
    .to_string();

    let (settings, _) = super::parse_settings_json(&contents, "test", None).expect("parses");
    assert!(settings.watchers.is_empty());
    assert!(settings.watchers_geometry.is_none());

    let written = serde_json::to_value(&settings).expect("serializes");
    let root = written.as_object().expect("object");
    assert!(
        !root.contains_key("watchers"),
        "an empty map must not appear"
    );
    assert!(
        !root.contains_key("watchersGeometry"),
        "this is why watchersGeometry carries skip_serializing_if and mainGeometry does not"
    );
}

/// #1171, 9.2.19 - a configured watcher round-trips value for value, INCLUDING the
/// absent-against-`[]` distinction for `commands`, which is the one place where the two
/// are opposites: absent reaches every agent, `[]` reaches none.
#[test]
fn watchers_round_trip_through_save_and_load_including_absent_versus_empty() {
    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "watchers": {
            "all-agents": { "mode": "state", "pattern": "  Permission" },
            "nobody": { "mode": "occurrence", "pattern": "Read", "commands": [] },
            "claude-only": {
                "enabled": false,
                "mode": "occurrence",
                "pattern": "Read \\((.+)\\)",
                "commands": ["claude"],
                "dedupe": "capture",
                "dedupeWindowMs": 5000,
                "capturedAgainst": "claude 2.1.212"
            }
        }
    })
    .to_string();

    let (settings, _) = super::parse_settings_json(&contents, "test", None).expect("parses");

    let all = settings.watchers["all-agents"].valid().expect("valid");
    assert!(all.enabled, "enabled defaults to true");
    assert_eq!(all.mode, super::WatcherMode::State);
    assert_eq!(all.pattern, "  Permission");
    assert!(all.commands.is_none(), "absent means every agent");
    assert_eq!(all.dedupe, super::WatcherDedupe::Row);
    assert_eq!(all.dedupe_window_ms, 2000);

    let nobody = settings.watchers["nobody"].valid().expect("valid");
    assert_eq!(
        nobody.commands.as_deref(),
        Some(&[] as &[String]),
        "`[]` must survive as `[]` and never collapse into absent"
    );

    let claude = settings.watchers["claude-only"].valid().expect("valid");
    assert!(!claude.enabled);
    assert_eq!(claude.mode, super::WatcherMode::Occurrence);
    assert_eq!(claude.dedupe, super::WatcherDedupe::Capture);
    assert_eq!(claude.dedupe_window_ms, 5000);
    assert_eq!(claude.captured_against.as_deref(), Some("claude 2.1.212"));

    // Byte-stable: what comes back out is what went in, key for key.
    let written = serde_json::to_value(&settings.watchers).expect("serializes");
    let original: serde_json::Value = serde_json::from_str(&contents).expect("parses");
    let mut expected = original["watchers"].clone();
    // The two defaulted fields the input omitted are written explicitly, since neither
    // carries `skip_serializing_if`: they are settings with a value, not absent ones.
    expected["all-agents"]["enabled"] = serde_json::json!(true);
    expected["all-agents"]["dedupe"] = serde_json::json!("row");
    expected["all-agents"]["dedupeWindowMs"] = serde_json::json!(2000);
    expected["nobody"]["enabled"] = serde_json::json!(true);
    expected["nobody"]["dedupe"] = serde_json::json!("row");
    expected["nobody"]["dedupeWindowMs"] = serde_json::json!(2000);
    assert_eq!(written, expected);
}

/// #2482 - absent `quotaSources` stays absent on save; an unknown `kind` costs one
/// skipped entry and round-trips verbatim.
#[test]
fn quota_sources_round_trip_and_an_unknown_kind_is_kept_verbatim() {
    let bare = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": []
    })
    .to_string();
    let (settings, _) = super::parse_settings_json(&bare, "test", None).expect("parses");
    assert!(settings.quota_sources.is_empty());
    let written = serde_json::to_value(&settings).expect("serializes");
    assert!(!written.as_object().unwrap().contains_key("quotaSources"));

    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "quotaSources": {
            "claude": { "kind": "screenRegex", "pattern": r"Weekly (\d+)%" },
            "codex": { "kind": "fromTheFuture", "endpoint": "x" }
        }
    })
    .to_string();
    let (settings, _) = super::parse_settings_json(&contents, "test", None).expect("parses");

    assert_eq!(
        settings.quota_sources["claude"].valid(),
        Some(&super::QuotaSourceConfig::ScreenRegex {
            pattern: r"Weekly (\d+)%".to_string(),
            enabled: true,
        })
    );
    assert!(settings.quota_sources["codex"].valid().is_none());

    let written = serde_json::to_value(&settings.quota_sources).expect("serializes");
    assert_eq!(
        written["codex"],
        serde_json::json!({ "kind": "fromTheFuture", "endpoint": "x" })
    );
    assert_eq!(
        written["claude"],
        serde_json::json!({ "kind": "screenRegex", "pattern": r"Weekly (\d+)%", "enabled": true })
    );
}

/// #2686 - the remaining kind round-trips, defaults `enabled` to true, and the tag
/// addition does not capture a `screenRegex` entry.
#[test]
fn a_remaining_kind_round_trips_and_defaults_enabled_true() {
    let contents = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "quotaSources": {
            "codex": { "kind": "screenRegexRemaining", "pattern": r"Weekly (\d+)% left" },
            "claude": { "kind": "screenRegex", "pattern": r"Weekly (\d+)%" }
        }
    })
    .to_string();
    let (settings, _) = super::parse_settings_json(&contents, "test", None).expect("parses");

    assert_eq!(
        settings.quota_sources["codex"].valid(),
        Some(&super::QuotaSourceConfig::ScreenRegexRemaining {
            pattern: r"Weekly (\d+)% left".to_string(),
            enabled: true,
        })
    );
    assert_eq!(
        settings.quota_sources["claude"].valid(),
        Some(&super::QuotaSourceConfig::ScreenRegex {
            pattern: r"Weekly (\d+)%".to_string(),
            enabled: true,
        })
    );

    let written = serde_json::to_value(&settings.quota_sources).expect("serializes");
    assert_eq!(
        written["codex"]["kind"],
        serde_json::json!("screenRegexRemaining")
    );
    assert_eq!(written["codex"]["enabled"], serde_json::json!(true));
    assert_eq!(written["claude"]["kind"], serde_json::json!("screenRegex"));
}

#[test]
fn save_settings_preserve_twice_succeeds_no_sharing_violation() {
    // G4a: the preserve-read closes its handle before the #774 rename, so two
    // preserve-saves in a row both succeed on Windows (a lingering read handle
    // would trip MoveFileEx with os 32).
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::save_settings_to_path(&settings_with_project_paths(&["A"]), &path).unwrap();
    super::save_settings_to_path_preserving_project_paths(
        &settings_with_project_paths(&["A"]),
        &path,
    )
    .unwrap();
    super::save_settings_to_path_preserving_project_paths(
        &settings_with_project_paths(&["A"]),
        &path,
    )
    .unwrap();
    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
    assert_eq!(lists.project_paths, vec!["A".to_string()]);
}

#[test]
fn remove_project_disk_composition_preserves_unseen_entry() {
    // remove_project command body against a tempdir: disk [A, X] (X unseen by the
    // sidebar), reconcile from disk, remove A, write verbatim => disk ends [X].
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::save_settings_to_path(&settings_with_project_paths(&["A", "X"]), &path).unwrap();

    let mut s = settings_with_project_paths(&["A"]); // stale in-memory
    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
    s.project_paths = lists.project_paths;
    s.project_path = lists.project_path; // reconciled to [A, X]
    assert!(crate::config::projects::remove_project_path(&mut s, "A"));
    super::save_settings_to_path(&s, &path).unwrap(); // verbatim

    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
    assert_eq!(lists.project_paths, vec!["X".to_string()]); // A gone, X preserved
    assert_eq!(lists.project_path.as_deref(), Some("X"));
}

#[test]
fn add_reconciles_from_disk_before_upsert() {
    // Add command body: in-memory stale [A], disk [A, X]; reconcile then append Y
    // and write verbatim => disk ends [A, X, Y] (the CLI append X is not lost).
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    super::save_settings_to_path(&settings_with_project_paths(&["A", "X"]), &path).unwrap();

    let mut s = settings_with_project_paths(&["A"]); // stale
    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
    s.project_paths = lists.project_paths;
    s.project_path = lists.project_path; // [A, X]
    s.project_paths.push("Y".to_string()); // stand-in for register upsert
    s.project_path = s.project_paths.first().cloned();
    super::save_settings_to_path(&s, &path).unwrap();

    let lists = super::read_project_paths_from_disk(&path).unwrap().unwrap();
    assert_eq!(
        lists.project_paths,
        vec!["A".to_string(), "X".to_string(), "Y".to_string()]
    );
}

#[test]
fn main_sidebar_side_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert_eq!(s.main_sidebar_side, MainSidebarSide::Right);
    s.main_sidebar_side = MainSidebarSide::Left;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"mainSidebarSide\":\"left\""));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.main_sidebar_side, MainSidebarSide::Left);
}

#[test]
fn main_sidebar_side_defaults_to_right_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert_eq!(s.main_sidebar_side, MainSidebarSide::Right);
}

#[test]
fn main_resource_monitor_attached_round_trips_and_defaults_false() {
    // #587 - round-trips through serde as camelCase.
    let mut s = AppSettings::default();
    assert!(!s.main_resource_monitor_attached);
    s.main_resource_monitor_attached = true;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"mainResourceMonitorAttached\":true"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.main_resource_monitor_attached);

    // Absent from an older settings.json => serde default false.
    let json_without = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let from_old: AppSettings = serde_json::from_str(json_without).expect("deserialize old json");
    assert!(!from_old.main_resource_monitor_attached);
}

// ── Issue #248 — legacy startOnlyCoordinators → restoreCoordinatorWakeState ──
//
// The minimal JSON below carries the three fields without serde defaults
// (`defaultShell`, `defaultShellArgs`, `agents`) plus whichever issue-#248
// field is under test. All other AppSettings fields use their serde defaults.

#[test]
fn issue_248_migration_legacy_true_translates_to_new_true() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "startOnlyCoordinators": true
        }"#;
    let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
    // Pre-migration: legacy field is parsed, new field is at its default.
    assert_eq!(s.legacy_start_only_coordinators, Some(true));
    assert!(!s.restore_coordinator_wake_state);

    super::apply_issue_248_migration(&mut s);

    assert!(s.restore_coordinator_wake_state);
    assert!(s.legacy_start_only_coordinators.is_none());

    // Round-trip — the legacy field must NOT reappear on next save.
    let out = serde_json::to_string(&s).expect("serialize");
    assert!(!out.contains("startOnlyCoordinators"));
    assert!(out.contains("\"restoreCoordinatorWakeState\":true"));
}

#[test]
fn issue_248_migration_legacy_false_translates_to_new_false() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "startOnlyCoordinators": false
        }"#;
    let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
    super::apply_issue_248_migration(&mut s);
    assert!(!s.restore_coordinator_wake_state);
    assert!(s.legacy_start_only_coordinators.is_none());
}

#[test]
fn issue_248_no_legacy_field_keeps_new_field_value() {
    // Fresh install or post-migrated settings.json — no legacy field.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "restoreCoordinatorWakeState": true
        }"#;
    let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
    super::apply_issue_248_migration(&mut s);
    assert!(s.restore_coordinator_wake_state); // untouched
    assert!(s.legacy_start_only_coordinators.is_none());
}

#[test]
fn issue_248_migration_conflict_keeps_new_value_and_drops_legacy() {
    // Grinch Z4 — both keys present, conflicting values. The user (or a
    // third-party tool) wrote restoreCoordinatorWakeState=true AFTER an
    // older startOnlyCoordinators=false was already on disk. The new value
    // wins; the legacy key is silently dropped from the next save. The
    // helper emits a `warn!` log line for triage — not asserted here (log
    // capture is not wired in the existing test suite), just exercised.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "startOnlyCoordinators": false,
            "restoreCoordinatorWakeState": true
        }"#;
    let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
    assert_eq!(s.legacy_start_only_coordinators, Some(false));
    assert!(s.restore_coordinator_wake_state);

    super::apply_issue_248_migration(&mut s);

    assert!(s.restore_coordinator_wake_state); // preserved
    assert!(s.legacy_start_only_coordinators.is_none()); // dropped
}

// ── Issue #1793 — restart-resume settings contract ─────────────────────

#[test]
fn restart_resume_defaults_are_the_decided_strings() {
    let s = super::AppSettings::default();
    assert_eq!(
        s.restart_resume_orchestrator_prompt,
        "AgentsCommander was restarted. Continue with the work that was in flight."
    );
    assert_eq!(s.restart_resume_agent_prompt, ".");
    // #2190: on a fresh install the struct default is now on.
    assert!(s.restart_resume_wake_working_agents);
}

#[test]
fn restart_resume_keys_absent_from_disk_load_as_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    // No restartResume* key of any kind. `sidebarAlwaysOnTop: true` is a
    // tripwire: it is not the default, so if this fixture failed to parse
    // and the loader fell back to defaults, the assertions below would be
    // comparing defaults with defaults and would prove nothing.
    std::fs::write(
        &path,
        r#"{
                "defaultShell": "bash",
                "defaultShellArgs": [],
                "agents": [],
                "sidebarAlwaysOnTop": true
            }"#,
    )
    .unwrap();

    let s = super::load_settings_from_path(&path);
    assert!(
        s.sidebar_always_on_top,
        "the fixture must have parsed; a fallback to defaults proves nothing"
    );
    assert_eq!(
        s.restart_resume_orchestrator_prompt,
        "AgentsCommander was restarted. Continue with the work that was in flight."
    );
    assert_eq!(s.restart_resume_agent_prompt, ".");
    // #2190 changed the struct default to true, but the field carries a bare
    // `#[serde(default)]`, so an existing settings.json without the key keeps
    // loading as false — existing user configs are deliberately untouched.
    assert!(!s.restart_resume_wake_working_agents);
}

#[test]
fn restart_resume_empty_prompt_survives_a_round_trip() {
    // Load-bearing: `#[serde(default = "...")]` fires only on ABSENCE, so an
    // explicitly empty prompt must not be re-defaulted on the way back in.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");

    let s = super::AppSettings {
        restart_resume_orchestrator_prompt: String::new(),
        restart_resume_agent_prompt: String::new(),
        ..super::AppSettings::default()
    };
    super::save_settings_to_path_preserving_project_paths(&s, &path).unwrap();

    let loaded = super::load_settings_from_path(&path);
    assert_eq!(loaded.restart_resume_orchestrator_prompt, "");
    assert_eq!(loaded.restart_resume_agent_prompt, "");
}

#[test]
fn restart_resume_serde_keys_are_camel_case() {
    // A default round trip cannot detect a wrong or renamed serde key, so the
    // raw key text is asserted here. Do not drop or merge these assertions.
    let json = serde_json::to_string(&super::AppSettings::default()).unwrap();
    assert!(
        json.contains("\"restartResumeOrchestratorPrompt\""),
        "the orchestrator prompt must serialize under its decided camelCase key"
    );
    assert!(
        json.contains("\"restartResumeAgentPrompt\""),
        "the agent prompt must serialize under its decided camelCase key"
    );
    assert!(
        json.contains("\"restartResumeWakeWorkingAgents\""),
        "the wake toggle must serialize under its decided camelCase key"
    );
}

#[test]
fn coord_sort_by_activity_defaults_when_missing_from_json() {
    // Old settings.json without the new field must deserialize to false.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.coord_sort_by_activity);
}

#[test]
fn activity_log_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.activity_log_enabled);
}

#[test]
fn spec_board_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.spec_board_enabled);
}

#[test]
fn coding_agent_install_enabled_default_impl_is_false() {
    assert!(!AppSettings::default().coding_agent_install_enabled);
}

#[test]
fn coding_agent_install_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;
    let settings: AppSettings = serde_json::from_str(json).expect("deserialize legacy settings");
    assert!(!settings.coding_agent_install_enabled);
}

#[test]
fn coding_agent_install_enabled_explicit_false_round_trips() {
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value["codingAgentInstallEnabled"] = serde_json::Value::Bool(false);
    let settings: AppSettings = serde_json::from_value(value).unwrap();
    assert!(!settings.coding_agent_install_enabled);
    assert_eq!(
        serde_json::to_value(settings).unwrap()["codingAgentInstallEnabled"],
        serde_json::Value::Bool(false)
    );
}

#[test]
fn coding_agent_install_enabled_true_round_trips_under_camel_case_key() {
    let settings = AppSettings {
        coding_agent_install_enabled: true,
        ..AppSettings::default()
    };
    let value = serde_json::to_value(settings).unwrap();
    assert_eq!(
        value["codingAgentInstallEnabled"],
        serde_json::Value::Bool(true)
    );
    assert!(value.get("coding_agent_install_enabled").is_none());
    let back: AppSettings = serde_json::from_value(value).unwrap();
    assert!(back.coding_agent_install_enabled);
    assert_eq!(
        serde_json::to_value(back).unwrap()["codingAgentInstallEnabled"],
        serde_json::Value::Bool(true)
    );
}

#[test]
fn container_options_enabled_default_impl_is_false() {
    assert!(!AppSettings::default().container_options_enabled);
}

#[test]
fn container_options_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;
    let settings: AppSettings = serde_json::from_str(json).expect("deserialize legacy settings");
    assert!(!settings.container_options_enabled);
}

#[test]
fn container_options_enabled_explicit_false_round_trips() {
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value["containerOptionsEnabled"] = serde_json::Value::Bool(false);
    let settings: AppSettings = serde_json::from_value(value).unwrap();
    assert!(!settings.container_options_enabled);
    assert_eq!(
        serde_json::to_value(settings).unwrap()["containerOptionsEnabled"],
        serde_json::Value::Bool(false)
    );
}

#[test]
fn container_options_enabled_true_round_trips_under_camel_case_key() {
    let settings = AppSettings {
        container_options_enabled: true,
        ..AppSettings::default()
    };
    let value = serde_json::to_value(settings).unwrap();
    assert_eq!(
        value["containerOptionsEnabled"],
        serde_json::Value::Bool(true)
    );
    assert!(value.get("container_options_enabled").is_none());
    let back: AppSettings = serde_json::from_value(value).unwrap();
    assert!(back.container_options_enabled);
    assert_eq!(
        serde_json::to_value(back).unwrap()["containerOptionsEnabled"],
        serde_json::Value::Bool(true)
    );
}

#[test]
fn co_managed_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.co_managed_enabled);
}

#[test]
fn co_managed_enabled_default_impl_is_false() {
    assert!(!AppSettings::default().co_managed_enabled);
}

#[test]
fn co_managed_enabled_round_trips_under_its_camel_case_key() {
    let settings = AppSettings {
        co_managed_enabled: true,
        ..AppSettings::default()
    };
    let value = serde_json::to_value(&settings).unwrap();
    assert_eq!(value["coManagedEnabled"], serde_json::Value::Bool(true));
    let back: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(
        serde_json::to_value(&back).unwrap()["coManagedEnabled"],
        serde_json::Value::Bool(true)
    );
}

// ---- #548: per-(agent, letter) profile label overrides ----

#[test]
fn profile_labels_override_survives_serde_round_trip() {
    let mut settings = AppSettings::default();
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "codex".to_string(),
            BTreeMap::from([("B".to_string(), "turbo".to_string())]),
        );

    let json = serde_json::to_string(&settings).unwrap();
    let round_tripped: AppSettings = serde_json::from_str(&json).unwrap();

    assert_eq!(
        round_tripped.coding_agent_profiles.profile_labels_by_agent["codex"]["B"],
        "turbo"
    );
}

#[test]
fn profile_labels_default_to_empty_and_always_serialize() {
    // A pre-#548 v2 codingAgentProfiles object with NO profileLabelsByAgent key
    // must load as an empty map (proves #[serde(default)]).
    let json = r##"{
            "schemaVersion": 2,
            "profileSlots": { "A": { "label": "" } },
            "defaultProfileByAgent": {},
            "profilesByAgent": {}
        }"##;

    let profiles: super::CodingAgentProfilesConfig = serde_json::from_str(json).unwrap();
    assert!(profiles.profile_labels_by_agent.is_empty());

    // Re-serializing always emits the key (no skip_serializing_if), parity with
    // profilesByAgent, so the on-disk key is stable.
    let out = serde_json::to_string(&profiles).unwrap();
    assert!(out.contains("profileLabelsByAgent"));
}

#[test]
fn repair_prunes_invalid_letter_label_overrides_and_keeps_valid() {
    let mut settings = settings_with_agents(&[("Codex", "codex")]); // id = agent-0
                                                                    // Pre-seed the A cell so the config is otherwise repair-clean; the only
                                                                    // change repair makes is dropping the invalid-letter overrides.
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "agent-0".to_string(),
        BTreeMap::from([("A".to_string(), super::empty_profile_cell())]),
    );
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "agent-0".to_string(),
            BTreeMap::from([
                ("B".to_string(), "turbo".to_string()),
                ("1".to_string(), "bad-digit".to_string()),
                ("ab".to_string(), "bad-two-char".to_string()),
                ("b".to_string(), "bad-lowercase".to_string()),
            ]),
        );

    let changed =
        repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    assert!(changed);
    let labels = &settings.coding_agent_profiles.profile_labels_by_agent["agent-0"];
    assert_eq!(labels.get("B").map(String::as_str), Some("turbo"));
    assert!(!labels.contains_key("1"));
    assert!(!labels.contains_key("ab"));
    assert!(!labels.contains_key("b"));
}

#[test]
fn repair_keeps_orphan_agent_id_label_override() {
    // An override under an agent id NOT present in settings.agents must survive
    // repair: no agent-id prune (plan 3.7); it is inert dead storage the
    // resolver never reads.
    let mut settings = settings_with_agents(&[("Codex", "codex")]); // id = agent-0
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "ghost-agent".to_string(),
            BTreeMap::from([("B".to_string(), "turbo".to_string())]),
        );

    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["ghost-agent"]["B"],
        "turbo"
    );
}

#[test]
fn repair_with_valid_label_map_does_not_flip_changed() {
    let mut settings = settings_with_agents(&[("Codex", "codex")]); // id = agent-0
                                                                    // Make the config otherwise repair-clean: agent-0 already holds its A cell.
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "agent-0".to_string(),
        BTreeMap::from([("A".to_string(), super::empty_profile_cell())]),
    );
    // A label map holding only valid letters must not trigger a disk rewrite.
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "agent-0".to_string(),
            BTreeMap::from([("B".to_string(), "turbo".to_string())]),
        );

    let changed =
        repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    assert!(!changed);
    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["agent-0"]["B"],
        "turbo"
    );
}

#[test]
fn resource_monitor_settings_default_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(s.resource_monitor_enabled);
    assert_eq!(s.max_concurrent_agent_processes, 32);
    assert_eq!(s.resource_watchdog_action, ResourceWatchdogAction::Warn);
    assert!(validate_resource_settings(&s).is_ok());
}

// ---- #1032: the per-agent context regex ---------------------------------------

/// Criterion 6. Every settings file that exists today predates this field, so the one
/// thing this field must never do is make an existing config fail to load.
#[test]
fn settings_without_context_regex_deserialize_unchanged() {
    let json = r##"{ "id": "claude", "label": "Claude", "command": "claude",
                         "color": "#10b981" }"##;
    let agent: super::AgentConfig = serde_json::from_str(json).expect("deserializes");
    assert_eq!(agent.label, "Claude");
    assert_eq!(agent.context_regex, None, "absent must mean off, not empty");
}

/// The field is an agent-config field, where an absent key is the correct way to say
/// "off" - unlike the IPC payload, where `percent` must serialize as an explicit null.
#[test]
fn context_regex_round_trips_as_camel_case() {
    let json = r##"{ "id": "claude", "label": "Claude", "command": "claude",
                         "color": "#10b981",
                         "contextRegex": "^ {2}Context [\u2591\u2588]+ (\\d{1,3})%" }"##;
    let agent: super::AgentConfig = serde_json::from_str(json).expect("deserializes");
    assert_eq!(
        agent.context_regex.as_deref(),
        Some("^ {2}Context [\u{2591}\u{2588}]+ (\\d{1,3})%")
    );

    let back = serde_json::to_string(&agent).expect("serializes");
    assert!(
        back.contains("\"contextRegex\""),
        "the frontend contract is camelCase: {back}"
    );

    let cleared = super::AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        context_regex: None,
        ..agent
    };
    let back = serde_json::to_string(&cleared).expect("serializes");
    assert!(
        !back.contains("contextRegex"),
        "None must omit the key here, so an untouched config stays untouched: {back}"
    );
}

// ---- #1646 / #1647: blocking menus and menu guard -----------------------------

#[test]
fn shipped_defaults_serve_pi_and_codex_only() {
    // The shipped patterns, not any per-agent materialization: the #1905 export
    // drops pristine arrays and the store serves these defaults for `None`.

    // Pi
    let pi_menus = super::default_blocking_menus_for_command("pi");
    assert_eq!(pi_menus.len(), 1);
    let pi_cfg = pi_menus[0].valid().unwrap();
    assert_eq!(pi_cfg.pattern, r"^\s*Trust project folder\?");
    assert_eq!(
        pi_cfg.notification,
        "pi is waiting for you to answer the folder-trust menu in this terminal"
    );
    assert!(pi_cfg.enabled);
    assert_eq!(
        pi_cfg.captured_against.as_deref(),
        Some("pi 0.52 / Windows")
    );

    // Codex
    let codex_menus = super::default_blocking_menus_for_command("codex");
    assert_eq!(codex_menus.len(), 2);
    assert_eq!(
        codex_menus[1],
        super::BlockingMenuEntry::Valid(super::codex_hooks_review_menu())
    );
    let codex_cfg = codex_menus[0].valid().unwrap();
    assert_eq!(
        codex_cfg.pattern,
        r"^\s*Do you trust the contents of this directory\?"
    );
    assert_eq!(
        codex_cfg.notification,
        "codex is waiting for you to answer the folder-trust menu in this terminal"
    );
    assert!(codex_cfg.enabled);
    assert_eq!(
        codex_cfg.captured_against.as_deref(),
        Some("codex 0.x / Linux")
    );

    // #1757 - the hooks-review entry, appended AFTER folder-trust (D13).
    let codex_hooks_cfg = codex_menus[1].valid().unwrap();
    assert_eq!(codex_hooks_cfg.pattern, super::CODEX_HOOKS_REVIEW_PATTERN);
    assert_eq!(
        codex_hooks_cfg.notification,
        "codex is waiting for you to answer the hooks-review menu in this terminal"
    );
    assert!(codex_hooks_cfg.enabled);
    assert_eq!(
        codex_hooks_cfg.captured_against.as_deref(),
        Some("codex 0.153.2 / Windows")
    );

    // Claude -> empty array: no shipped set, nothing materialized anymore.
    let claude_menus = super::default_blocking_menus_for_command("claude");
    assert!(claude_menus.is_empty());
}

#[test]
fn test_blocking_menus_tolerant_parsing() {
    let json = r##"{
            "id": "pi",
            "label": "Pi",
            "command": "pi",
            "color": "#10b981",
            "blockingMenus": [
                {
                    "pattern": "^\\s*Trust",
                    "notification": "prompt msg",
                    "enabled": true
                },
                12345,
                {
                    "invalidFieldOnly": true
                }
            ]
        }"##;

    let agent: super::AgentConfig =
        serde_json::from_str(json).expect("deserializes with invalid entries");
    let menus = agent
        .blocking_menus
        .as_ref()
        .expect("blocking_menus present");
    assert_eq!(menus.len(), 3);
    assert!(menus[0].valid().is_some());
    assert!(menus[1].valid().is_none());
    assert!(menus[2].valid().is_none());

    let serialized = serde_json::to_string(&agent).expect("serializes back");
    assert!(serialized.contains("12345"));
    assert!(serialized.contains("invalidFieldOnly"));

    let round_trip: super::AgentConfig =
        serde_json::from_str(&serialized).expect("round trip deserializes");
    assert_eq!(round_trip.blocking_menus, agent.blocking_menus);
}

#[test]
fn test_blocking_menus_explicit_empty_array() {
    let json = r##"{
            "id": "pi",
            "label": "Pi",
            "command": "pi",
            "color": "#10b981",
            "blockingMenus": []
        }"##;

    let agent: super::AgentConfig =
        serde_json::from_str(json).expect("deserializes explicit empty array");
    assert_eq!(agent.blocking_menus, Some(vec![]));

    let serialized = serde_json::to_string(&agent).expect("serializes");
    assert!(
        serialized.contains(r#""blockingMenus":[]"#),
        "an explicit empty array round-trips: {serialized}"
    );
}

#[test]
fn test_menu_guard_master_switch() {
    let def = AppSettings::default();
    assert!(def.menu_guard_enabled);

    let json = r#"{
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserializes");
    assert!(s.menu_guard_enabled, "defaults to true when missing");

    let json_disabled = r#"{
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "menuGuardEnabled": false
        }"#;
    let s_disabled: AppSettings =
        serde_json::from_str(json_disabled).expect("deserializes disabled");
    assert!(!s_disabled.menu_guard_enabled);

    let serialized = serde_json::to_string(&s_disabled).expect("serializes");
    assert!(serialized.contains("\"menuGuardEnabled\":false"));
}

// ---- #1757: the Codex hooks-review blocking menu -----------------------

/// One agent carrying exactly the `blocking_menus` state under test.
fn agent_1757(
    id: &str,
    command: &str,
    blocking_menus: Option<Vec<super::BlockingMenuEntry>>,
) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus,
        backend: Default::default(),
    }
}

fn settings_1757(agents: Vec<AgentConfig>) -> AppSettings {
    AppSettings {
        agents,
        ..AppSettings::default()
    }
}

/// The pre-#1757 materialized Codex array's only entry.
fn folder_trust_entry_1757() -> super::BlockingMenuEntry {
    super::BlockingMenuEntry::Valid(super::BlockingMenuConfig {
        pattern: r"^\s*Do you trust the contents of this directory\?".to_string(),
        notification: "codex is waiting for you to answer the folder-trust menu in this terminal"
            .to_string(),
        enabled: true,
        captured_against: Some("codex 0.x / Linux".to_string()),
    })
}

fn hooks_review_entry_1757(enabled: bool) -> super::BlockingMenuEntry {
    super::BlockingMenuEntry::Valid(super::BlockingMenuConfig {
        pattern: super::CODEX_HOOKS_REVIEW_PATTERN.to_string(),
        notification: "codex is waiting for you to answer the hooks-review menu in this terminal"
            .to_string(),
        enabled,
        captured_against: Some("codex 0.153.2 / Windows".to_string()),
    })
}

fn assert_folder_trust_entry_1757(entry: &super::BlockingMenuEntry) {
    let cfg = entry.valid().expect("folder-trust entry is valid");
    assert_eq!(
        cfg.pattern,
        r"^\s*Do you trust the contents of this directory\?"
    );
    assert_eq!(
        cfg.notification,
        "codex is waiting for you to answer the folder-trust menu in this terminal"
    );
    assert!(cfg.enabled);
    assert_eq!(cfg.captured_against.as_deref(), Some("codex 0.x / Linux"));
}

fn assert_hooks_review_entry_1757(entry: &super::BlockingMenuEntry) {
    let cfg = entry.valid().expect("hooks-review entry is valid");
    assert_eq!(cfg.pattern, super::CODEX_HOOKS_REVIEW_PATTERN);
    assert_eq!(
        cfg.notification,
        "codex is waiting for you to answer the hooks-review menu in this terminal"
    );
    assert!(cfg.enabled);
    assert_eq!(
        cfg.captured_against.as_deref(),
        Some("codex 0.153.2 / Windows")
    );
}

// T4
#[test]
fn issue_1757_appends_to_an_already_materialized_codex_array() {
    let mut settings = settings_1757(vec![agent_1757(
        "codex",
        "codex",
        Some(vec![folder_trust_entry_1757()]),
    )]);

    assert!(
        super::apply_issue_1757_migration(&mut settings),
        "an already-materialized codex array must be extended"
    );

    let menus = settings.agents[0]
        .blocking_menus
        .as_ref()
        .expect("blocking_menus present");
    assert_eq!(menus.len(), 2);
    assert_folder_trust_entry_1757(&menus[0]);
    assert_hooks_review_entry_1757(&menus[1]);
}

// T5
#[test]
fn issue_1757_is_idempotent() {
    let mut settings = settings_1757(vec![agent_1757(
        "codex",
        "codex",
        Some(vec![folder_trust_entry_1757()]),
    )]);

    assert!(super::apply_issue_1757_migration(&mut settings));
    assert!(
        !super::apply_issue_1757_migration(&mut settings),
        "the second call must be a no-op: the pattern is already present"
    );
    assert_eq!(
        settings.agents[0]
            .blocking_menus
            .as_ref()
            .expect("blocking_menus present")
            .len(),
        2
    );
}

// T6
#[test]
fn issue_1757_never_activates_an_explicitly_empty_array() {
    let mut settings = settings_1757(vec![agent_1757("codex", "codex", Some(vec![]))]);

    assert!(
        !super::apply_issue_1757_migration(&mut settings),
        "Some(vec![]) is the documented explicitly-disabled state (D8)"
    );
    assert_eq!(settings.agents[0].blocking_menus, Some(vec![]));
}

// T7
#[test]
fn issue_1757_respects_a_disabled_hooks_entry() {
    let disabled = hooks_review_entry_1757(false);
    let mut settings = settings_1757(vec![agent_1757(
        "codex",
        "codex",
        Some(vec![disabled.clone()]),
    )]);

    assert!(
            !super::apply_issue_1757_migration(&mut settings),
            "the presence test ignores the enabled flag, so enabled: false is a durable off switch (D9)"
        );
    assert_eq!(settings.agents[0].blocking_menus, Some(vec![disabled]));
}

// T8
#[test]
fn issue_1757_ignores_non_codex_agents() {
    let pi_menus = vec![super::BlockingMenuEntry::Valid(super::BlockingMenuConfig {
        pattern: r"^\s*Trust project folder\?".to_string(),
        notification: "pi is waiting for you to answer the folder-trust menu in this terminal"
            .to_string(),
        enabled: true,
        captured_against: Some("pi 0.52 / Windows".to_string()),
    })];
    let claude_menus = vec![super::BlockingMenuEntry::Valid(super::BlockingMenuConfig {
        pattern: "^Something else entirely".to_string(),
        notification: "claude is waiting".to_string(),
        enabled: true,
        captured_against: None,
    })];
    let mut settings = settings_1757(vec![
        agent_1757("pi", "pi", Some(pi_menus.clone())),
        agent_1757("claude", "claude", Some(claude_menus.clone())),
    ]);

    assert!(
        !super::apply_issue_1757_migration(&mut settings),
        "the stem gate must never target an agent the default would not"
    );
    assert_eq!(settings.agents[0].blocking_menus, Some(pi_menus));
    assert_eq!(settings.agents[1].blocking_menus, Some(claude_menus));
}

// T9 - the only new test that observes the real load chain. Every other
// needs_save source is deliberately inert on this fixture, so the export's
// own flag is the only thing that can drive the write.
#[test]
fn a_pre_1757_pristine_codex_array_is_dropped_by_the_export() {
    const FIXTURE: &str = r##"{
  "defaultShell": "test-shell",
  "defaultShellArgs": [],
  "rootToken": "issue-1757-fixture-token",
  "agents": [
    {
      "id": "codex",
      "label": "Codex",
      "command": "codex",
      "color": "#000000",
      "blockingMenus": [
        {
          "pattern": "^\\s*Do you trust the contents of this directory\\?",
          "notification": "codex is waiting for you to answer the folder-trust menu in this terminal",
          "enabled": true,
          "capturedAgainst": "codex 0.x / Linux"
        }
      ]
    }
  ],
  "codingAgentProfiles": {
    "schemaVersion": 2,
    "profileSlots": { "A": { "label": "" } },
    "defaultProfileByAgent": {},
    "profilesByAgent": {
      "codex": { "A": { "enabled": true, "command": "", "env": {}, "notes": "" } }
    }
  }
}"##;

    // Tripwire: a single-backslash escape here is not legal JSON, the load would
    // fall back to default_settings_with_overlay with zero agents, and this test
    // would silently stop testing anything.
    serde_json::from_str::<serde_json::Value>(FIXTURE).expect("the fixture must be valid JSON");

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, FIXTURE).unwrap();

    let settings = super::load_settings_from_path(&path);
    assert_eq!(
        settings.agents.len(),
        1,
        "the fixture must parse; zero agents means the loader fell back to defaults"
    );

    // The in-memory 1757 step ran (fixture had only folder-trust) and the array
    // then matched the shipped codex set, so the export dropped it as pristine:
    // the key is gone from settings.json and no `.local` file was created.
    let raw = std::fs::read_to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(
        value["agents"][0].get("blockingMenus").is_none(),
        "the pristine array must be stripped from settings.json"
    );
    assert!(
        !super::blocking_menus_local_path(&path).exists(),
        "pristine arrays must not create the .local file"
    );

    // The stripped agent is served by the shipped file from now on.
    let store = super::BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        store.resolve_for(&settings.agents[0]),
        super::default_blocking_menus_for_command("codex"),
        "the shipped codex set (two entries) must serve the agent"
    );
    assert!(
        super::blocking_menus_shipped_path(&path).exists(),
        "the store must have materialized the shipped file"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// #2450 - orphaned profiles are re-bound by a recorded agent identity.
// ─────────────────────────────────────────────────────────────────────────
mod profile_identity_2450;

// ─────────────────────────────────────────────────────────────────────────
// #1737 - the `.local` alter-ego override layer. Plan section 11, S1 to S34.
// ─────────────────────────────────────────────────────────────────────────
mod local_overlay_1737;

// ─────────────────────────────────────────────────────────────────────────
// #1905 - the blocking-menus store: shipped file, `.local` overlay, precedence.
// ─────────────────────────────────────────────────────────────────────────
mod blocking_menus_1905;

// ─────────────────────────────────────────────────────────────────────────
// #1925 - the downloaded layer: the published bytes, the whole-file validator,
// per-read cache validation, and precedence between the layers.
// ─────────────────────────────────────────────────────────────────────────
mod remote_blocking_menus_1925;

/// #2064 - the six remote-activity dials are additive: an older settings.json
/// without them must deserialize to the documented defaults, and a serialized
/// default must carry all six keys (guarding against an accidental
/// `skip_serializing_if`).
#[test]
fn remote_activity_dials_round_trip_through_serde() {
    const KEYS: [&str; 6] = [
        "ciActivityEnabled",
        "ciActivityNotifyOrchestrator",
        "branchStalenessEnabled",
        "branchStalenessNotifyOrchestrator",
        "ciSweepMinIntervalSecs",
        "branchStalenessIntervalSecs",
    ];

    let defaults = AppSettings::default();
    let value = serde_json::to_value(&defaults).expect("serialize default AppSettings");
    for key in KEYS {
        assert!(
            value.get(key).is_some(),
            "{key} must be serialized, never skipped"
        );
    }

    let mut stripped = value.clone();
    let object = stripped.as_object_mut().expect("settings object");
    for key in KEYS {
        object.remove(key);
    }
    let back: AppSettings =
        serde_json::from_value(stripped).expect("deserialize without the six keys");
    assert!(back.ci_activity_enabled);
    assert!(back.ci_activity_notify_orchestrator);
    assert!(back.branch_staleness_enabled);
    assert!(back.branch_staleness_notify_orchestrator);
    assert_eq!(back.ci_sweep_min_interval_secs, 30);
    assert_eq!(back.branch_staleness_interval_secs, 260);

    let round_trip: AppSettings = serde_json::from_value(value).expect("round trip");
    assert_eq!(round_trip.ci_activity_enabled, defaults.ci_activity_enabled);
    assert_eq!(
        round_trip.ci_activity_notify_orchestrator,
        defaults.ci_activity_notify_orchestrator
    );
    assert_eq!(
        round_trip.branch_staleness_enabled,
        defaults.branch_staleness_enabled
    );
    assert_eq!(
        round_trip.branch_staleness_notify_orchestrator,
        defaults.branch_staleness_notify_orchestrator
    );
    assert_eq!(
        round_trip.ci_sweep_min_interval_secs,
        defaults.ci_sweep_min_interval_secs
    );
    assert_eq!(
        round_trip.branch_staleness_interval_secs,
        defaults.branch_staleness_interval_secs
    );
}

// ─────────────────────────────────────────────────────────────────────────
// #2306 P1 - registered-agent explicit order model.
// ─────────────────────────────────────────────────────────────────────────

fn order_2306_decode_settings(agents: serde_json::Value) -> AppSettings {
    // `defaultShell`, `defaultShellArgs` and `agents` are the three fields
    // `AppSettings` has no serde default for.
    serde_json::from_value(serde_json::json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "agents": agents,
    }))
    .expect("settings decode")
}

fn order_2306_agent(id: &str, order: Option<serde_json::Value>) -> serde_json::Value {
    let mut agent = serde_json::json!({
        "id": id,
        "label": id.to_uppercase(),
        "command": "claude",
        "color": "#000000",
    });
    if let Some(order) = order {
        agent["order"] = order;
    }
    agent
}

fn order_2306_ids(agents: &[AgentConfig]) -> Vec<&str> {
    agents.iter().map(|agent| agent.id.as_str()).collect()
}

fn order_2306_positions(agents: &[AgentConfig]) -> Vec<Option<u32>> {
    agents.iter().map(|agent| agent.order).collect()
}

#[test]
fn agent_order_mixed_missing_duplicate_and_out_of_range_is_deterministic() {
    // Keys: a=(2,0), b=(1,1) [absent uses its index], c=(0,2), d=(0,3),
    // e=(99,4). Sorted by (key, original index): c,d,b,a,e.
    let mut settings = order_2306_decode_settings(serde_json::json!([
        order_2306_agent("a", Some(serde_json::json!(2))),
        order_2306_agent("b", None),
        order_2306_agent("c", Some(serde_json::json!(0))),
        order_2306_agent("d", Some(serde_json::json!(0))),
        order_2306_agent("e", Some(serde_json::json!(99))),
    ]));
    super::finalize_agent_order(&mut settings.agents);
    assert_eq!(order_2306_ids(&settings.agents), ["c", "d", "b", "a", "e"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );

    // Idempotent: a second finalize cannot re-sort the now-valid ordinals.
    super::finalize_agent_order(&mut settings.agents);
    assert_eq!(order_2306_ids(&settings.agents), ["c", "d", "b", "a", "e"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );
}

#[test]
fn agent_order_absent_keeps_legacy_vector_order() {
    let mut settings = order_2306_decode_settings(serde_json::json!([
        order_2306_agent("a", None),
        order_2306_agent("b", None),
        order_2306_agent("c", None),
    ]));
    super::finalize_agent_order(&mut settings.agents);
    assert_eq!(order_2306_ids(&settings.agents), ["a", "b", "c"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1), Some(2)]
    );
}

#[test]
fn agent_order_invalid_values_decode_as_absent_without_failing_the_file() {
    for raw in [
        serde_json::json!(null),
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!("3"),
        serde_json::json!(true),
        serde_json::json!(u64::from(u32::MAX) + 1),
    ] {
        let mut settings = order_2306_decode_settings(serde_json::json!([order_2306_agent(
            "a",
            Some(raw.clone())
        ),]));
        assert_eq!(settings.agents[0].order, None, "raw order {raw}");
        super::finalize_agent_order(&mut settings.agents);
        assert_eq!(settings.agents[0].order, Some(0), "raw order {raw}");
    }

    // The boundary value still decodes as a real ordinal.
    let settings = order_2306_decode_settings(serde_json::json!([order_2306_agent("a", None)]));
    assert_eq!(settings.agents[0].order, None);
    let settings = order_2306_decode_settings(serde_json::json!([{
        "id": "a", "label": "A", "command": "claude", "color": "#000000",
        "order": u32::MAX,
    }]));
    assert_eq!(settings.agents[0].order, Some(u32::MAX));
}

#[test]
fn agent_order_legacy_load_writes_nothing_and_next_save_persists_positions() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    // `order: -1` is invalid and stays on disk until an authorized save; `b`
    // has no ordinal at all. Complete A-profile cells keep the loader from
    // taking any unrelated repair/root-token save path, so a write here can
    // only come from the order code under test.
    std::fs::write(
        &path,
        r##"{
                "defaultShell": "test-shell",
                "defaultShellArgs": [],
                "rootToken": "base-token",
                "agents": [
                    {"id":"a","label":"A","command":"claude","color":"#000000","order":-1},
                    {"id":"b","label":"B","command":"codex","color":"#000000"}
                ],
                "codingAgentProfiles": {
                    "schemaVersion": 2,
                    "profileSlots": {"A": {"label": ""}},
                    "defaultProfileByAgent": {},
                    "profilesByAgent": {
                        "a": {"A": {"enabled": true, "command": "", "env": {}, "notes": "cell-a"}},
                        "b": {"A": {"enabled": true, "command": "", "env": {}, "notes": "cell-b"}}
                    },
                    "profileLabelsByAgent": {"a": {"A": "Alpha"}}
                }
            }"##,
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();

    let settings = super::load_settings_from_path(&path);
    assert_eq!(order_2306_ids(&settings.agents), ["a", "b"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1)],
        "absent and invalid ordinals normalize to the array position"
    );
    // A/B profile cells and labels survive reordering.
    assert_eq!(
        settings.coding_agent_profiles.profiles_by_agent["a"]["A"].notes,
        "cell-a"
    );
    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["a"]["A"],
        "Alpha"
    );
    // Loading writes nothing beyond the one-shot export of #2716 (B3) 4.3:
    // both groups move, every other byte stays, and a second load writes none.
    b3_assert_split_matches_seed(&path, &before, &["order"]);
    let sidecar = b3_object(&super::agents_instance_path(&path));
    assert_eq!(sidecar["agents"][0]["order"], 0);
    assert_eq!(sidecar["agents"][1]["order"], 1);
    let exported = std::fs::read(&path).unwrap();
    let exported_sidecar = std::fs::read(super::agents_instance_path(&path)).unwrap();
    let _ = super::load_settings_from_path(&path);
    assert_eq!(std::fs::read(&path).unwrap(), exported);
    assert_eq!(
        std::fs::read(super::agents_instance_path(&path)).unwrap(),
        exported_sidecar
    );
    // #2716 (B3) r12l: the one-shot export rotates exactly once, and slot 1
    // keeps the legacy file byte for byte.
    assert_eq!(
        std::fs::read(super::settings_backup_path(&path, 1)).expect("slot 1"),
        before,
        "slot 1 must hold the seeded bytes"
    );
    assert!(
        !super::settings_backup_path(&path, 2).exists(),
        "a second save rotated the settings backup again"
    );

    let written = super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(super::agents_instance_path(&path)).unwrap())
            .unwrap();
    assert_eq!(disk["agents"][0]["id"], "a");
    assert_eq!(disk["agents"][0]["order"], 0);
    assert_eq!(disk["agents"][1]["id"], "b");
    assert_eq!(disk["agents"][1]["order"], 1);
    assert_eq!(order_2306_positions(&written.agents), [Some(0), Some(1)]);

    // Second reload preserves the written explicit positions.
    let reloaded = super::load_settings_from_path(&path);
    assert_eq!(order_2306_ids(&reloaded.agents), ["a", "b"]);
    assert_eq!(order_2306_positions(&reloaded.agents), [Some(0), Some(1)]);
}

#[test]
fn agent_order_unknown_fields_follow_existing_writer_behavior() {
    let mut settings = order_2306_decode_settings(serde_json::json!([{
        "id": "a", "label": "A", "command": "claude", "color": "#000000",
        "futureField": 7,
    }]));
    assert_eq!(settings.agents.len(), 1);
    super::finalize_agent_order(&mut settings.agents);
    let object = serde_json::to_value(&settings).unwrap();
    assert!(
        object["agents"][0].get("futureField").is_none(),
        "the typed writer drops unknown fields as before: {object}"
    );
    assert_eq!(object["agents"][0]["order"], 0);
}

#[test]
fn agent_order_terminal_snapshot_cas_returns_and_installs_normalized_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r##"{
                "defaultShell": "test-shell",
                "defaultShellArgs": [],
                "rootToken": "base-token",
                "terminalSnapshotsEnabled": false,
                "agents": [
                    {"id":"a","label":"A","command":"claude","color":"#000000","order":1},
                    {"id":"b","label":"B","command":"claude","color":"#000000"},
                    {"id":"c","label":"C","command":"claude","color":"#000000","order":0},
                    {"id":"d","label":"D","command":"claude","color":"#000000","order":0}
                ]
            }"##,
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    let current = AppSettings::default();

    // disk gate == enabled: the no-op CAS returns the finalized, normalized view
    // with no write and no backup rotation.
    let no_op =
        super::compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, false)
            .unwrap();
    assert_eq!(order_2306_ids(&no_op.agents), ["c", "d", "a", "b"]);
    assert_eq!(
        order_2306_positions(&no_op.agents),
        [Some(0), Some(1), Some(2), Some(3)]
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the no-op CAS writes nothing"
    );

    // Direct CAS write: the file itself carries contiguous explicit ordinals.
    super::compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, true)
        .unwrap();
    let disk: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk["terminalSnapshotsEnabled"], true);
    let sidecar = b3_object(&super::agents_instance_path(&path));
    let agents = sidecar["agents"].as_array().unwrap();
    let ids: Vec<&str> = agents.iter().map(|a| a["id"].as_str().unwrap()).collect();
    let orders: Vec<u64> = agents
        .iter()
        .map(|a| a["order"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, ["c", "d", "a", "b"]);
    assert_eq!(orders, [0, 1, 2, 3]);
}

#[test]
fn agent_order_overlay_owns_agents_in_memory_and_leaves_both_files_base_owned() {
    use local_overlay_1737::{base_fixture, disk_object, seed};
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = serde_json::json!([
        {"id":"base-a","label":"Base A","command":"claude","color":"#111111","order":0},
        {"id":"base-b","label":"Base B","command":"claude","color":"#222222","order":1}
    ]);
    let local = serde_json::json!({
        "agents": [
            {"id":"ov-a","label":"Overlay A","command":"claude","color":"#aaaaaa","order":1},
            {"id":"ov-b","label":"Overlay B","command":"claude","color":"#bbbbbb"},
            {"id":"ov-c","label":"Overlay C","command":"claude","color":"#cccccc","order":0}
        ]
    });
    let path = seed(temp.path(), Some(&base), Some(&local));

    let mut settings = super::load_settings_from_path(&path);
    assert!(settings.local_overlay_state.owns_top_level("agents"));
    // Overlay numbers win; the missing value falls back to its overlay index.
    assert_eq!(order_2306_ids(&settings.agents), ["ov-c", "ov-a", "ov-b"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1), Some(2)]
    );

    // The CLI loaders share `parse_settings_json` with the GUI loader.
    let contents = std::fs::read_to_string(&path).unwrap();
    let (cli_view, _) = super::parse_settings_json(&contents, "test", Some(&path)).unwrap();
    assert_eq!(order_2306_ids(&cli_view.agents), ["ov-c", "ov-a", "ov-b"]);

    // A non-order save restores the base `agents` array to disk byte-for-byte,
    // keeps the effective overlay order only in memory, and never writes the
    // local file.
    let local_path = temp.path().join("settings.50.personal.no-git.json");
    let overlay_before = std::fs::read(&local_path).unwrap();
    settings.gemini_api_key = "unrelated".to_string();
    let written = super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let disk = disk_object(&path);
    assert_eq!(disk["agents"], base["agents"]);
    assert_eq!(std::fs::read(&local_path).unwrap(), overlay_before);
    assert_eq!(order_2306_ids(&written.agents), ["ov-c", "ov-a", "ov-b"]);
    assert_eq!(
        order_2306_positions(&written.agents),
        [Some(0), Some(1), Some(2)]
    );
}

#[test]
fn agent_order_overlay_only_install_honors_numeric_order_without_leaking_to_base() {
    use local_overlay_1737::{disk_object, seed};
    let temp = tempfile::tempdir().unwrap();
    let local = serde_json::json!({
        "agents": [
            {"id":"only-a","label":"A","command":"claude","color":"#111111","order":1},
            {"id":"only-b","label":"B","command":"claude","color":"#222222","order":0}
        ]
    });
    let path = seed(temp.path(), None, Some(&local));
    let local_path = temp.path().join("settings.50.personal.no-git.json");
    let overlay_before = std::fs::read(&local_path).unwrap();

    let settings = super::load_settings_from_path(&path);
    assert_eq!(order_2306_ids(&settings.agents), ["only-b", "only-a"]);
    assert_eq!(order_2306_positions(&settings.agents), [Some(0), Some(1)]);
    // Whole-group protection preserves physical absence. The effective
    // overlay order is independent of materializing a base agents array.
    let disk = disk_object(&path);
    assert!(
        !disk.contains_key("agents"),
        "the absent base agents key must stay absent: {disk:?}"
    );
    let sidecar = super::agents_instance_path(&path);
    assert!(!sidecar.exists() || !disk_object(&sidecar).contains_key("agents"));
    assert_eq!(std::fs::read(&local_path).unwrap(), overlay_before);
}

#[test]
fn agent_order_unparseable_base_with_overlay_still_finalizes_the_effective_view() {
    use local_overlay_1737::seed;
    let temp = tempfile::tempdir().unwrap();
    let local = serde_json::json!({
        "agents": [
            {"id":"ov-a","label":"A","command":"claude","color":"#111111","order":1},
            {"id":"ov-b","label":"B","command":"claude","color":"#222222","order":0}
        ]
    });
    let path = seed(temp.path(), None, Some(&local));
    std::fs::write(&path, "{not json").unwrap();
    let corrupt = std::fs::read(&path).unwrap();

    let settings = super::load_settings_from_path(&path);
    assert_eq!(order_2306_ids(&settings.agents), ["ov-b", "ov-a"]);
    assert_eq!(order_2306_positions(&settings.agents), [Some(0), Some(1)]);
    // The preserve writer refuses to overwrite a present-but-invalid base.
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
}
/// #2348 - main-window placement persistence: serde, the central present-only
/// `Preserve` table, the legacy-migration one-save regression, and overlay
/// base-preservation for the new key.
mod issue_2348_main_window_placement;

/// #2133 (P2) - the agent-help data model, parser and local overlay. Every test drives a
/// `tempfile::tempdir()`; none reads the test config dir.
mod agent_help_2133;

/// #2430: the worked table from the phase plan. Every row is what
/// `executableTokenBasename` (`src/shared/profile-utils.ts`) returns.
const STEM_TWIN_TABLE: &[(&str, &str)] = &[
    (r"C:\tools\claude.exe", "claude"),
    ("/usr/bin/claude", "claude"),
    ("claude", "claude"),
    ("C:/tools/CLAUDE.CMD", "claude"),
    ("claude-3.5", "claude-3"),
    ("./bin/agy", "agy"),
    ("", ""),
    (".bashrc", ""),
    ("claude.", "claude."),
    (r"C:\tools\", "c:/tools/"),
];

#[test]
fn command_token_basename_strips_windows_separators_on_every_os() {
    assert_eq!(
        super::command_token_basename(r"C:\tools\claude.exe"),
        "claude"
    );
}

#[test]
fn command_token_basename_matches_the_typescript_twin() {
    for (input, expected) in STEM_TWIN_TABLE {
        assert_eq!(
            super::command_token_basename(input),
            *expected,
            "input {input:?}"
        );
    }
}

#[test]
fn command_token_basename_is_idempotent() {
    for (input, _) in STEM_TWIN_TABLE {
        let once = super::command_token_basename(input);
        assert_eq!(
            super::command_token_basename(&once),
            once,
            "input {input:?}"
        );
    }
}

#[test]
fn derived_agent_id_home_legacy_roundtrip_omits_empty_carrier() {
    let value = b3_legacy_fixture("Legacy", "Profile");
    let settings: super::AppSettings = serde_json::from_value(value).unwrap();
    assert!(settings
        .coding_agent_profiles
        .codex_home_key_by_agent
        .is_empty());
    let encoded = serde_json::to_value(&settings).unwrap();
    assert!(encoded["codingAgentProfiles"]
        .get("codexHomeKeyByAgent")
        .is_none());
    let roundtrip: super::AppSettings = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        serde_json::to_value(&roundtrip.agents).unwrap(),
        serde_json::to_value(&settings.agents).unwrap()
    );
    assert_eq!(
        roundtrip.coding_agent_profiles,
        settings.coding_agent_profiles
    );
}

#[test]
fn derived_agent_id_home_whole_save_restores_sidecar_and_written() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let mut legacy = b3_legacy_fixture("Legacy", "Legacy P");
    legacy["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"alpha": "wrong-legacy"});
    b3_write(&path, &legacy);
    let mut sidecar = b3_legacy_fixture("Sidecar", "Sidecar P");
    let expected = serde_json::json!({"alpha": "original-home", "orphan": "retained"});
    sidecar["codingAgentProfiles"]["codexHomeKeyByAgent"] = expected.clone();
    b3_write(&b3_sidecar_path(temp.path()), &sidecar);
    let mut request = super::load_settings_for_cli_from_path(&path);
    request.coding_agent_profiles.codex_home_key_by_agent =
        std::collections::BTreeMap::from([("alpha".to_string(), "forged".to_string())]);
    // Negative control: the ordinary request serializer really carries the
    // forgery. Removing backend restoration makes the assertions below fail.
    assert_ne!(
        serde_json::to_value(&request).unwrap()["codingAgentProfiles"]["codexHomeKeyByAgent"],
        expected
    );
    request.agents[0].label = "Edited label".to_string();
    let written = super::save_settings_to_path_preserving_project_paths(&request, &path).unwrap();
    assert_eq!(
        b3_object(&b3_sidecar_path(temp.path()))["codingAgentProfiles"]["codexHomeKeyByAgent"],
        expected
    );
    assert_eq!(
        serde_json::to_value(&written.coding_agent_profiles).unwrap()["codexHomeKeyByAgent"],
        expected
    );
    assert_eq!(written.agents[0].label, "Edited label");
    assert_eq!(b3_marker(&written).as_deref(), Some("Sidecar P"));
}

#[test]
fn derived_agent_id_home_absent_base_rejects_new_carrier() {
    for present in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        if present {
            b3_write(&path, &b3_legacy_fixture("Legacy", "Profile"));
        }
        let mut request = super::AppSettings::default();
        request
            .coding_agent_profiles
            .codex_home_key_by_agent
            .insert("alpha".to_string(), "forged".to_string());
        let written =
            super::save_settings_to_path_preserving_project_paths(&request, &path).unwrap();
        assert!(written
            .coding_agent_profiles
            .codex_home_key_by_agent
            .is_empty());
        assert!(
            b3_object(&b3_sidecar_path(temp.path()))["codingAgentProfiles"]
                .get("codexHomeKeyByAgent")
                .is_none()
        );
    }
}

#[test]
fn derived_agent_id_home_serializers_preserve_legacy_carrier() {
    for reconcile in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        let mut legacy = b3_legacy_fixture("Legacy", "Profile");
        legacy["codingAgentProfiles"]["codexHomeKeyByAgent"] =
            serde_json::json!({"alpha": "legacy-home"});
        b3_write(&path, &legacy);
        let mut request = super::load_settings_for_cli_from_path(&path);
        request
            .coding_agent_profiles
            .codex_home_key_by_agent
            .clear();
        if reconcile {
            super::save_settings_with_project_paths_to_path(&request, &path).unwrap();
        } else {
            super::save_settings_to_path_preserving_project_paths(&request, &path).unwrap();
        }
        assert_eq!(
            b3_object(&b3_sidecar_path(temp.path()))["codingAgentProfiles"]["codexHomeKeyByAgent"],
            legacy["codingAgentProfiles"]["codexHomeKeyByAgent"]
        );
        assert_eq!(
            super::load_settings_for_cli_from_path(&path)
                .coding_agent_profiles
                .codex_home_key_by_agent
                .get("alpha")
                .map(String::as_str),
            Some("legacy-home")
        );
    }
}

#[test]
fn derived_agent_id_home_narrow_and_overlay_cannot_forge_written() {
    for terminal in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        let mut sidecar = b3_legacy_fixture("Sidecar", "Profile");
        sidecar["codingAgentProfiles"]["codexHomeKeyByAgent"] =
            serde_json::json!({"alpha": "physical-home"});
        b3_write(&b3_sidecar_path(temp.path()), &sidecar);
        b3_write(
            &b3_overlay_path(temp.path()),
            &serde_json::json!({
                "codingAgentProfiles": {"codexHomeKeyByAgent": {"alpha": "overlay-forged"}}
            }),
        );
        let mut request = super::load_settings_for_cli_from_path(&path);
        assert_eq!(
            request.coding_agent_profiles.codex_home_key_by_agent["alpha"],
            "physical-home"
        );
        request
            .coding_agent_profiles
            .codex_home_key_by_agent
            .insert("alpha".to_string(), "client-forged".to_string());
        let written = if terminal {
            super::compare_and_set_terminal_snapshots_enabled_at_path(&request, &path, true, false)
                .unwrap()
        } else {
            let geometry = super::WindowGeometry {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            };
            super::set_main_window_placement_at_path(
                &request,
                &path,
                &geometry,
                super::MainWindowDisplayState::Normal,
            )
            .unwrap()
        };
        assert_eq!(
            written.coding_agent_profiles.codex_home_key_by_agent["alpha"],
            "physical-home"
        );
        assert_eq!(
            b3_object(&b3_sidecar_path(temp.path()))["codingAgentProfiles"]["codexHomeKeyByAgent"]
                ["alpha"],
            "physical-home"
        );
        // Terminal CAS also has an idempotent return without serialization.
        let unchanged = super::compare_and_set_terminal_snapshots_enabled_at_path(
            &request,
            &path,
            written.terminal_snapshots_enabled,
            written.terminal_snapshots_enabled,
        )
        .unwrap();
        assert_eq!(
            unchanged.coding_agent_profiles.codex_home_key_by_agent["alpha"],
            "physical-home"
        );
    }
}

#[test]
fn derived_agent_id_home_load_uses_physical_origin_under_overlay() {
    let physical = tempfile::tempdir().unwrap();
    let logical = tempfile::tempdir().unwrap();
    let io = b3_settings_path(physical.path());
    let label = b3_settings_path(logical.path());
    let mut source = b3_legacy_fixture("Physical", "Profile");
    source["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"alpha": "physical-home"});
    b3_write(&b3_sidecar_path(physical.path()), &source);
    source["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"alpha": "logical-forged"});
    b3_write(&b3_sidecar_path(logical.path()), &source);
    b3_write(
        &b3_overlay_path(physical.path()),
        &serde_json::json!({"codingAgentProfiles": {"codexHomeKeyByAgent": {"alpha": "overlay-forged"}}}),
    );
    let contents = serde_json::to_string(&b3_migrated_fixture()).unwrap();
    let (loaded, _) = super::parse_settings_json_with_projection_labeled(
        &contents,
        "physical-test",
        Some(&io),
        Some(&label),
        None,
    )
    .unwrap();
    assert_eq!(
        loaded.coding_agent_profiles.codex_home_key_by_agent["alpha"],
        "physical-home"
    );
    let default_loaded = super::default_settings_with_overlay_captured(
        &io,
        "physical-test",
        &serde_json::Map::new(),
    );
    assert_eq!(
        default_loaded.coding_agent_profiles.codex_home_key_by_agent["alpha"],
        "physical-home"
    );
}

#[test]
fn derived_agent_id_home_invalid_durable_leaf_survives_save_and_fails_use() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let mut source = b3_legacy_fixture("Codex", "Profile");
    source["agents"][0]["command"] = serde_json::json!("codex");
    source["agents"][0]["isolatedHome"] = serde_json::json!(true);
    source["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"alpha": "../invalid"});
    b3_write(&path, &source);
    let mut request = super::load_settings_for_cli_from_path(&path);
    request
        .coding_agent_profiles
        .codex_home_key_by_agent
        .clear();
    let written = super::save_settings_to_path_preserving_project_paths(&request, &path).unwrap();
    assert_eq!(
        written.coding_agent_profiles.codex_home_key_by_agent["alpha"],
        "../invalid"
    );
    assert!(crate::config::agent_command::resolve_agent_spawn_command(
        &written,
        "alpha",
        None,
        Some("A"),
        false
    )
    .unwrap_err()
    .contains("Invalid isolated Codex HOME key"));
}

// #2716 (B3) - the agents file. Rows E4 to E20b of the phase plan.

fn b3_agent(id: &str, label: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "label": label,
        "command": "claude",
        "color": "#112233",
    })
}

fn b3_profiles(marker: &str) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": 2,
        "profileLabelsByAgent": { "alpha": { "A": marker } },
    })
}

/// The E4 fixture: a settings file still carrying both legacy groups.
fn b3_legacy_fixture(label: &str, marker: &str) -> serde_json::Value {
    serde_json::json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "rootToken": "base-token",
        "agents": [b3_agent("alpha", label)],
        "codingAgentProfiles": b3_profiles(marker),
    })
}

/// A settings file that no longer carries either group.
fn b3_migrated_fixture() -> serde_json::Value {
    serde_json::json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "rootToken": "base-token",
    })
}

fn b3_write(path: &std::path::Path, value: &serde_json::Value) {
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

fn b3_settings_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(super::SETTINGS_FILE_NAME)
}

fn b3_sidecar_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(super::AGENTS_INSTANCE_FILE_NAME)
}

fn b3_overlay_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME)
}

fn b3_object(path: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap() {
        serde_json::Value::Object(object) => object,
        other => panic!("{} is not an object: {other}", path.display()),
    }
}

fn b3_labels(settings: &super::AppSettings) -> Vec<String> {
    settings.agents.iter().map(|a| a.label.clone()).collect()
}

fn b3_marker(settings: &super::AppSettings) -> Option<String> {
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .get("alpha")
        .and_then(|cells| cells.get("A"))
        .cloned()
}

/// An agents file in the exact bytes the save writes for `label` and `marker`,
/// so a steady-state save that changes nothing leaves it byte-identical.
fn b3_seed_sidecar(dir: &std::path::Path, label: &str, marker: &str) -> Vec<u8> {
    let source = tempfile::tempdir().unwrap();
    let source_path = b3_settings_path(source.path());
    b3_write(&source_path, &b3_legacy_fixture(label, marker));
    let settings = super::load_settings_for_cli_from_path(&source_path);
    let mut groups = serde_json::Map::new();
    groups.insert(
        "agents".to_string(),
        serde_json::to_value(&settings.agents).unwrap(),
    );
    groups.insert(
        "codingAgentProfiles".to_string(),
        serde_json::to_value(&settings.coding_agent_profiles).unwrap(),
    );
    let bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(groups)).unwrap();
    std::fs::write(b3_sidecar_path(dir), &bytes).unwrap();
    bytes
}

fn hash_activation_seed(sidecar: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let mut settings = super::AppSettings {
        root_token: Some("hash-test-token".into()),
        agents: vec![serde_json::from_value(b3_agent("alpha", "Incumbent")).unwrap()],
        ..super::AppSettings::default()
    };
    super::validate_and_repair_settings(&mut settings).unwrap();
    super::normalize_agent_order(&mut settings.agents);
    let mut value = serde_json::to_value(settings).unwrap();
    super::replace_project_projection(value.as_object_mut().unwrap(), &serde_json::Map::new());
    value["agents"][0]
        .as_object_mut()
        .unwrap()
        .remove("id_hash");
    let groups = if sidecar {
        let object = value.as_object_mut().unwrap();
        serde_json::json!({
            "agents": object.remove("agents").unwrap(),
            "codingAgentProfiles": object.remove("codingAgentProfiles").unwrap()
        })
    } else {
        // Avoid the independent startup export; exercise missing-group legacy fallback.
        serde_json::json!({})
    };
    b3_write(&path, &value);
    b3_write(&b3_sidecar_path(temp.path()), &groups);
    // The existing project migration publishes even an empty projection.
    // Seed its destination before measuring hash-only read/rejection effects.
    b3_write(&super::project_paths_path(&path), &serde_json::json!({}));
    (temp, path)
}

fn hash_activation_files(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.extension().is_none_or(|ext| ext != "lock"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(path).unwrap(),
            )
        })
        .collect()
}

#[test]
fn issue_3047_pair_table_and_independent_preimages() {
    use sha2::{Digest, Sha256};
    // Independently reproduce both normalization stages of a pair.
    let normalize = |parts: &[&str]| {
        let mut chars: Vec<char> = parts
            .concat()
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|c| !c.is_whitespace())
            .collect();
        chars.sort_unstable();
        chars.iter().collect::<String>()
    };
    let expected = |base_parts: &[&str], profile_parts: &[&str]| {
        let base = normalize(base_parts);
        let profile = normalize(profile_parts);
        format!("cid1:{:x}", Sha256::digest(normalize(&[&base, &profile])))
    };
    let mut agents: Vec<super::AgentConfig> = serde_json::from_value(serde_json::json!([
        {"id":"a","label":"Alpha","color":"red","baseCommand":"Codex",
         "envs":[{"key":"K","value":"V","enabled":false}]},
        {"id":"b","label":"Beta","color":"blue","baseCommand":"Claude"}
    ]))
    .unwrap();
    let mut profiles: super::CodingAgentProfilesConfig =
        serde_json::from_value(serde_json::json!({"profilesByAgent": {
            "a":{"A":{"additionalArguments":"--model x","env":{"P":"Q"}},
                 "B":{"additionalArguments":"--model y","enabled":false}},
            "b":{"A":{"additionalArguments":"--effort low"},
                 "B":{"additionalArguments":"--effort high"}}
        }}))
        .unwrap();
    super::refresh_agent_id_hashes(&mut agents);
    let base = agents[0].id_hash.clone();
    super::refresh_pair_id_hashes(&agents, &mut profiles);
    let original = profiles.clone();
    for (id, letter, leaves) in [
        ("a", "A", vec!["Codex", "K", "V", "--model x", "P", "Q"]),
        ("a", "B", vec!["Codex", "K", "V", "--model y"]),
        ("b", "A", vec!["Claude", "--effort low"]),
        ("b", "B", vec!["Claude", "--effort high"]),
    ] {
        assert_eq!(
            profiles.profiles_by_agent[id][letter].id_hash_debug,
            Some(leaves.into_iter().map(String::from).collect())
        );
    }
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected(&["Codex", "K", "V"], &["--model x", "P", "Q"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"].id_hash,
        Some(expected(&["Codex", "K", "V"], &["--model y"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["b"]["A"].id_hash,
        Some(expected(&["Claude"], &["--effort low"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["b"]["B"].id_hash,
        Some(expected(&["Claude"], &["--effort high"]))
    );
    assert_ne!(
        profiles.profiles_by_agent["b"]["A"].id_hash,
        profiles.profiles_by_agent["b"]["B"].id_hash
    );
    profiles
        .profiles_by_agent
        .get_mut("a")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .command = "--model z".into();
    super::refresh_pair_id_hashes(&agents, &mut profiles);
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected(&["Codex", "K", "V"], &["--model z", "P", "Q"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"],
        original.profiles_by_agent["a"]["B"]
    );
    assert_eq!(
        profiles.profiles_by_agent["b"],
        original.profiles_by_agent["b"]
    );
    assert_eq!(agents[0].id_hash, base);
    agents[0].command = "Other".into();
    agents[0].envs[0].value = "W".into();
    super::refresh_pair_id_hashes(&agents, &mut profiles);
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected(&["Other", "K", "W"], &["--model z", "P", "Q"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"].id_hash,
        Some(expected(&["Other", "K", "W"], &["--model y"]))
    );
    assert_eq!(
        profiles.profiles_by_agent["b"],
        original.profiles_by_agent["b"]
    );
    profiles
        .profiles_by_agent
        .get_mut("a")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .env
        .insert("P".into(), "R".into());
    super::refresh_pair_id_hashes(&agents, &mut profiles);
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected(&["Other", "K", "W"], &["--model z", "P", "R"]))
    );
    assert_eq!(
        agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    // Original leaves retain vector order, disabled/duplicate/empty rows,
    // whitespace and Unicode. Expected hashes use two separate stages.
    agents[0].command = "İΣΣ\u{2003} oo --".into();
    agents[0].envs = serde_json::from_value(serde_json::json!([
        {"key":" K ","value":"İ","enabled":false},
        {"key":"","value":""},
        {"key":" K ","value":"İ","enabled":true}
    ]))
    .unwrap();
    let cell = profiles
        .profiles_by_agent
        .get_mut("a")
        .unwrap()
        .get_mut("A")
        .unwrap();
    cell.command = "Σ İ\u{a0}oo".into();
    cell.env = BTreeMap::from([("Z".into(), "Last".into()), ("A".into(), "".into())]);
    cell.id_hash_debug = Some(vec!["forged-pair".into()]);
    super::refresh_agent_id_hashes(&mut agents);
    let leaves = vec!["İΣΣ\u{2003} oo --", " K ", "İ", "", "", " K ", "İ"];
    assert_eq!(
        agents[0].id_hash_debug,
        Some(leaves.iter().map(|s| (*s).into()).collect())
    );
    let base_hash = agents[0].id_hash.clone();
    agents[0].id_hash_debug = Some(vec!["forged-base".into()]);
    super::refresh_pair_id_hashes(&agents, &mut profiles);
    let mut pair = leaves.clone();
    pair.extend(["Σ İ\u{a0}oo", "A", "", "Z", "Last"]);
    let base_preimage = normalize(&leaves);
    let profile_preimage = normalize(&pair[leaves.len()..]);
    let expected_hash = format!(
        "cid1:{:x}",
        Sha256::digest(normalize(&[&base_preimage, &profile_preimage]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        Some(expected_hash)
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash_debug,
        Some(pair.into_iter().map(String::from).collect())
    );
    assert_eq!(agents[0].id_hash, base_hash);
}

#[test]
fn issue_3047_pair_equivalence_exclusions_empty_orphan_and_first_match() {
    let mut agents: Vec<super::AgentConfig> = serde_json::from_value(serde_json::json!([
        {"id":"a","label":"First","color":"red","command":"İ A\u{2003}b"},
        {"id":"a","label":"Second","color":"blue","command":"wrong"}
    ]))
    .unwrap();
    let mut profiles: super::CodingAgentProfilesConfig =
        serde_json::from_value(serde_json::json!({"profilesByAgent": {
            "a":{"A":{"command":" XY"},"B":{}},
            "orphan":{"A":{"id_hash":"forged","id_hash_debug":["forged"]}}
        }}))
        .unwrap();
    super::refresh_pair_id_hashes(&agents, &mut profiles);
    let original = profiles.profiles_by_agent["a"]["A"].clone();
    assert_eq!(
        original.id_hash,
        Some(super::configuration_identity("i\u{307}abxy", &[]))
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"].id_hash,
        Some(super::configuration_identity(&agents[0].command, &[]))
    );
    assert!(!profiles.profiles_by_agent["a"].contains_key("C"));
    assert_eq!(profiles.profiles_by_agent["orphan"]["A"].id_hash, None);
    assert_eq!(
        profiles.profiles_by_agent["orphan"]["A"].id_hash_debug,
        None
    );
    assert_eq!(
        original.id_hash_debug,
        Some(vec!["İ A\u{2003}b".into(), " XY".into()])
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["B"].id_hash_debug,
        Some(vec!["İ A\u{2003}b".into(), "".into()])
    );
    agents[0].label = "Renamed".into();
    agents[0].command = "b aİ".into();
    profiles
        .default_profile_by_agent
        .insert("a".into(), "B".into());
    profiles.profile_labels_by_agent.insert(
        "a".into(),
        std::collections::BTreeMap::from([("A".into(), "Renamed profile".into())]),
    );
    let cell = profiles
        .profiles_by_agent
        .get_mut("a")
        .unwrap()
        .get_mut("A")
        .unwrap();
    cell.command = "y\u{a0}x".into();
    cell.enabled = false;
    cell.notes = "ignored".into();
    cell.preflight = Some("ignored too".into());
    super::refresh_pair_id_hashes(&agents, &mut profiles);
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash,
        original.id_hash
    );
    assert_eq!(
        profiles.profiles_by_agent["a"]["A"].id_hash_debug,
        Some(vec!["b aİ".into(), "y\u{a0}x".into()])
    );
    assert_ne!(
        profiles.profiles_by_agent["a"]["A"].id_hash_debug,
        original.id_hash_debug
    );
    let cell = &profiles.profiles_by_agent["a"]["A"];
    let pid = super::profile_identity(cell);
    let mut forged = cell.clone();
    forged.id_hash = Some("client".into());
    assert_eq!(super::profile_identity(&forged), pid);
    assert_ne!(
        super::pair_identity("ab", &[], cell),
        super::pair_identity("aab", &[], cell)
    );
}

fn assert_issue_3047_pair_adoption_view(
    settings: &super::AppSettings,
    orphan: &str,
    sidecar: bool,
) {
    let cells = &settings.coding_agent_profiles.profiles_by_agent["alpha"];
    let mut expected_a = super::empty_profile_cell();
    expected_a.id_hash = Some(super::configuration_identity("claude", &[]));
    expected_a.id_hash_debug = Some(vec!["claude".into(), "".into()]);
    assert_eq!(cells["A"], expected_a, "{sidecar}/{orphan}");
    assert_eq!(cells.len(), 1, "quarantine never supplies active cells");
    assert!(!settings
        .coding_agent_profiles
        .profiles_by_agent
        .contains_key("o"));
}

fn check_issue_3047_pair_adoption_case(sidecar: bool, orphan: &str, fail_publish: bool) {
    let (temp, path) = hash_activation_seed(sidecar);
    let physical = if sidecar {
        b3_sidecar_path(temp.path())
    } else {
        path.clone()
    };
    let mut raw = serde_json::Value::Object(b3_object(&physical));
    let agent: super::AgentConfig = serde_json::from_value(raw["agents"][0].clone()).unwrap();
    raw["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({});
    raw["codingAgentProfiles"]["identityByAgent"] = serde_json::json!({});
    raw["codingAgentProfiles"]["profileLabelsByAgent"] = serde_json::json!({});
    if orphan != "missing" {
        let letter = if orphan == "B" { "B" } else { "A" };
        raw["codingAgentProfiles"]["profilesByAgent"]["o"] = serde_json::json!({
            letter:{"additionalArguments":"--special","notes":"payload"}
        });
        raw["codingAgentProfiles"]["identityByAgent"]["o"] =
            serde_json::json!(if orphan == "unmatched" {
                "1234567890abcdef".to_string()
            } else {
                super::agent_profile_identity(&agent)
            });
    }
    b3_write(&physical, &raw);
    let before = hash_activation_files(temp.path());
    let cli = super::load_settings_for_cli_from_path(&path);
    let strict = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    for settings in [&cli, &strict] {
        assert_issue_3047_pair_adoption_view(settings, orphan, sidecar);
    }
    assert_eq!(hash_activation_files(temp.path()), before);
    let main_before = std::fs::read(&path).unwrap();
    if fail_publish {
        super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
            *slot.borrow_mut() = Some((path.clone(), super::SettingsSaveStage::AtomicReplace))
        });
    }
    let (records, _capture_guard) = capture_settings_save_diagnostics();
    let gui = super::load_settings_from_path(&path);
    // Inspect the GUI diagnostic and the same typed failure boundary.
    if fail_publish {
        let captured = records.borrow();
        assert_eq!(captured.len(), 1);
        let fields = parse_settings_save_diagnostic(&captured[0]);
        assert_eq!(fields["stage"], "atomic_replace");
        assert_eq!(fields["target_path"], super::render_diagnostic_path(&path));
        drop(captured);
        let error =
            super::save_settings_to_path_preserving_project_paths_typed(&gui, &path).unwrap_err();
        assert_eq!(error.stage, super::SettingsSaveStage::AtomicReplace);
        assert_eq!(error.target_path.as_ref(), &path);
    }
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert_eq!(
        gui.coding_agent_profiles.profiles_by_agent,
        strict.coding_agent_profiles.profiles_by_agent
    );
    if fail_publish {
        assert_eq!(std::fs::read(&path).unwrap(), main_before);
        let stored = b3_object(&b3_sidecar_path(temp.path()));
        let profiles = &stored["codingAgentProfiles"];
        assert_eq!(profiles["profileBindingVersion"], 1);
        assert!(
            profiles["profilesByBaseHash"][&super::configuration_identity("claude", &[])]["A"]
                .is_object()
        );
        assert!(profiles.get("profilesByAgent").is_none());
        if orphan != "missing" {
            let mut expected = serde_json::json!({
                "cells":raw["codingAgentProfiles"]["profilesByAgent"]["o"],
                "identity":raw["codingAgentProfiles"]["identityByAgent"]["o"]
            });
            if let Some(labels) = raw["codingAgentProfiles"]["profileLabelsByAgent"].get("o") {
                expected["labels"] = labels.clone();
            }
            assert_eq!(
                profiles["legacyProfileOrphans"]["o"],
                serde_json::json!([expected])
            );
        }
    } else {
        let reload = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert_eq!(gui.coding_agent_profiles, reload.coding_agent_profiles);
    }
}

#[test]
fn issue_3047_pair_load_save_fixed_point_and_untrusted_metadata() {
    for sidecar in [false, true] {
        for supplied in [
            serde_json::json!(null),
            serde_json::json!(false),
            serde_json::json!(42),
            serde_json::json!(1.5),
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!(""),
            serde_json::json!("forged"),
            serde_json::json!(["forged", ""]),
            serde_json::json!(["mixed", 42]),
        ] {
            let (temp, path) = hash_activation_seed(sidecar);
            let physical = if sidecar {
                b3_sidecar_path(temp.path())
            } else {
                path.clone()
            };
            let mut raw = serde_json::Value::Object(b3_object(&physical));
            raw["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
                "alpha":{"A":{"additionalArguments":"--model x","id_hash":supplied,"id_hash_debug":supplied,
                    "sentinel":{"keep":true}}}
            });
            b3_write(&physical, &raw);
            // CLI views remain read-only before the admitted GUI conversion.
            let before_cli = hash_activation_files(temp.path());
            super::load_settings_for_cli_from_path(&path);
            super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
            assert_eq!(hash_activation_files(temp.path()), before_cli);
            let first_gui = super::load_settings_from_path(&path);
            hash_activation_assert(&first_gui);
            let stored = b3_object(&b3_sidecar_path(temp.path()));
            assert_eq!(stored["codingAgentProfiles"]["profileBindingVersion"], 1);
            for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
                assert!(stored["codingAgentProfiles"].get(key).is_none());
            }
            let before = hash_activation_files(temp.path());
            for (loader, loaded) in b3_all_loaders(&path) {
                let cell = &loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"];
                assert_eq!(
                    cell.id_hash_debug,
                    Some(vec!["claude".into(), "--model x".into()])
                );
                assert_eq!(
                    cell.id_hash,
                    Some(super::configuration_identity("claude--modelx", &[])),
                    "{loader}"
                );
                assert_eq!(hash_activation_files(temp.path()), before, "{loader}");
            }
            let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
            let written =
                super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
            let groups = b3_object(&b3_sidecar_path(temp.path()));
            let stored_cell = &groups["codingAgentProfiles"]["profilesByBaseHash"]
                [&super::configuration_identity("claude", &[])]["A"];
            assert_eq!(
                stored_cell,
                &serde_json::json!({"additionalArguments":"--model x","enabled":true,"env":{},"notes":"","sentinel":{"keep":true}})
            );
            assert_eq!(
                written.coding_agent_profiles.profiles_by_agent["alpha"]["A"].id_hash_debug,
                Some(vec!["claude".into(), "--model x".into()])
            );
            let stable = hash_activation_files(temp.path());
            let reloaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
            super::save_settings_to_path_preserving_project_paths(&reloaded, &path).unwrap();
            assert_eq!(hash_activation_files(temp.path()), stable);
            let mut stale = reloaded.clone();
            stale
                .coding_agent_profiles
                .profiles_by_agent
                .get_mut("alpha")
                .unwrap()
                .get_mut("A")
                .unwrap()
                .id_hash = Some("stale".into());
            stale
                .coding_agent_profiles
                .profiles_by_agent
                .get_mut("alpha")
                .unwrap()
                .get_mut("A")
                .unwrap()
                .id_hash_debug = Some(vec!["stale".into()]);
            let cas = super::compare_and_set_terminal_snapshots_enabled_at_path(
                &stale,
                &path,
                stale.terminal_snapshots_enabled,
                stale.terminal_snapshots_enabled,
            )
            .unwrap();
            assert_eq!(cas.coding_agent_profiles, reloaded.coding_agent_profiles);
        }
    }
    // Missing-A repair and matching orphan adoption happen after parse.
    // Both CLI loaders must return final hashes without publishing anything.
    for sidecar in [false, true] {
        for orphan in ["missing", "A", "B", "unmatched"] {
            for fail_publish in [false, true] {
                check_issue_3047_pair_adoption_case(sidecar, orphan, fail_publish);
            }
        }
    }
}

fn assert_issue_3047_pair_projection_rejection(
    error: super::SettingsSaveError,
    sidecar: bool,
    path: &std::path::Path,
    sidecar_path: &std::path::Path,
) {
    assert_eq!(error.stage, super::SettingsSaveStage::PreserveDiskGate);
    if sidecar {
        assert_eq!(error.target_path.as_path(), sidecar_path);
        assert!(matches!(&error.legacy_outward,
                super::SettingsSaveLegacyOutward::DiskRead(text)
                if text.contains("Refusing to overwrite") && text.contains("undecodable agents array")));
    } else {
        assert_eq!(error.target_path.as_path(), path);
        assert!(matches!(
            error.cause,
            super::SettingsSaveCause::Semantic(
                super::SettingsSaveReason::DiskSettingsValidationRejected
            )
        ));
        assert!(matches!(&error.legacy_outward,
                super::SettingsSaveLegacyOutward::DiskValidation(text)
                if text.contains("non-project fields are invalid")));
        assert!(!sidecar_path.exists());
    }
}

#[test]
fn issue_3047_pair_protected_base_projection_and_early_rejections() {
    for sidecar in [false, true] {
        for invalid in [false, true] {
            let (temp, path) = hash_activation_seed(sidecar);
            let physical = if sidecar {
                b3_sidecar_path(temp.path())
            } else {
                path.clone()
            };
            let mut raw = serde_json::Value::Object(b3_object(&physical));
            raw["agents"][0]["sentinel"] = serde_json::json!({"keep":19});
            raw["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
                "alpha":{"A":{"additionalArguments":"--model x","sentinel":"cell"}}
            });
            b3_write(&physical, &raw);
            let mut overlay_agent = b3_agent("alpha", "Overlay");
            overlay_agent["command"] = serde_json::json!("codex --overlay");
            b3_write(
                &b3_overlay_path(temp.path()),
                &serde_json::json!({"agents":[overlay_agent]}),
            );
            let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
            assert!(loaded.local_overlay_state.owns_top_level("agents"));
            if invalid {
                raw["agents"][0]["label"] = serde_json::json!(17);
                b3_write(&physical, &raw);
            }
            if !sidecar {
                std::fs::remove_file(b3_sidecar_path(temp.path())).unwrap();
            }
            let before = std::fs::read(&physical).unwrap();
            let mut out = serde_json::to_value(&loaded).unwrap();
            out["codingAgentProfiles"] = raw["codingAgentProfiles"].clone();
            let result = super::save_agents_instance_file(
                &loaded,
                &path,
                &super::ProjectWriteMode::Preserve,
                None,
                out.as_object().unwrap(),
                super::SettingsSaveStage::PreserveDiskGate,
                (None, None, None),
            );
            if invalid {
                let error = result.unwrap_err();
                assert_issue_3047_pair_projection_rejection(
                    error,
                    sidecar,
                    &path,
                    &b3_sidecar_path(temp.path()),
                );
                assert_eq!(std::fs::read(&physical).unwrap(), before);
            } else {
                let groups = result.unwrap();
                assert_eq!(
                    groups["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["id_hash"],
                    super::configuration_identity("claude--modelx", &[])
                );
                assert_eq!(
                    groups["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["sentinel"],
                    "cell"
                );
                if sidecar {
                    assert_eq!(groups["agents"], raw["agents"]);
                } else {
                    assert_eq!(b3_object(&path)["agents"], raw["agents"]);
                    assert!(!groups.contains_key("agents"));
                }
            }
        }
    }
}

#[test]
fn issue_3047_pair_unresolvable_first_row_and_empty_physical_base() {
    for first in [
        serde_json::json!({"id":"a","baseCommand":17}),
        serde_json::json!({"id":"a","baseCommand":"codex","envs":"bad"}),
    ] {
        let agents = serde_json::json!([first, {"id":"a","baseCommand":"valid"}]);
        let before = agents.clone();
        let mut profiles = serde_json::json!({"profilesByAgent":{
            "a":{"A":{"command":"--model x","id_hash":"stale","id_hash_debug":["stale"],"unknown":1}}
        }});
        super::refresh_physical_pair_id_hashes(Some(&agents), &mut profiles);
        assert!(profiles["profilesByAgent"]["a"]["A"]
            .get("id_hash_debug")
            .is_none());
        assert!(profiles["profilesByAgent"]["a"]["A"]
            .get("id_hash")
            .is_none());
        assert_eq!(profiles["profilesByAgent"]["a"]["A"]["unknown"], 1);
        assert_eq!(agents, before);
        let (temp, path) = hash_activation_seed(true);
        let mut groups = serde_json::Value::Object(b3_object(&b3_sidecar_path(temp.path())));
        groups["agents"] = agents;
        b3_write(&b3_sidecar_path(temp.path()), &groups);
        let bytes = std::fs::read(b3_sidecar_path(temp.path())).unwrap();
        assert!(super::save_agents_instance_file(
            &super::AppSettings::default(),
            &path,
            &super::ProjectWriteMode::Preserve,
            None,
            &serde_json::Map::new(),
            super::SettingsSaveStage::PreserveDiskGate,
            (None, None, None),
        )
        .is_err());
        assert_eq!(std::fs::read(b3_sidecar_path(temp.path())).unwrap(), bytes);
    }
    for agents in [None, Some(serde_json::json!([]))] {
        let mut profiles = serde_json::json!({"profilesByAgent":{
            "a":{"A":{"id_hash":"stale","id_hash_debug":["stale"]}}
        }});
        super::refresh_physical_pair_id_hashes(agents.as_ref(), &mut profiles);
        assert!(profiles["profilesByAgent"]["a"]["A"]
            .get("id_hash_debug")
            .is_none());
        assert!(profiles["profilesByAgent"]["a"]["A"]
            .get("id_hash")
            .is_none());
    }
    let agents = serde_json::json!([{"id":"a","command":"codex","label":17,
            "envs":[{"key":"K","value":"V","enabled":false}]},
            {"id":"a","baseCommand":"wrong"}]);
    let mut profiles = serde_json::json!({"profilesByAgent":{
        "a":{"A":{"command":"--model x","env":{"P":"Q"},"unknown":1}}
    }});
    let before = profiles.clone();
    super::refresh_physical_pair_id_hashes(Some(&agents), &mut profiles);
    assert_eq!(
        profiles["profilesByAgent"]["a"]["A"]["id_hash"],
        super::configuration_identity("codexKV--modelxPQ", &[])
    );
    let mut expected = before;
    expected["profilesByAgent"]["a"]["A"]["id_hash"] =
        serde_json::json!("cid1:87849ab8d8f223bc755ecbc34b1e772ca61469dd5651dcf52063949e23001036");
    expected["profilesByAgent"]["a"]["A"]["id_hash_debug"] =
        serde_json::json!(["codex", "K", "V", "--model x", "P", "Q"]);
    assert_eq!(profiles, expected);
    let mut profiles = serde_json::json!({"profilesByAgent":{
        "a":{"A":{},"B":{"additionalArguments":17,"id_hash":"stale",
            "id_hash_debug":["stale"],"unknown":7},"C":false}
    }});
    super::refresh_physical_pair_id_hashes(Some(&agents), &mut profiles);
    assert_eq!(
        profiles["profilesByAgent"]["a"]["A"]["id_hash_debug"],
        serde_json::json!(["codex", "K", "V", ""])
    );
    assert_eq!(
        profiles["profilesByAgent"]["a"]["B"],
        serde_json::json!({"additionalArguments":17,"unknown":7})
    );
    assert_eq!(profiles["profilesByAgent"]["a"]["C"], false);
}

fn check_issue_3047_pair_ownership_case(sidecar: bool, protected: &str) {
    let (temp, path) = hash_activation_seed(sidecar);
    let physical = if sidecar {
        b3_sidecar_path(temp.path())
    } else {
        path.clone()
    };
    let mut raw = serde_json::Value::Object(b3_object(&physical));
    raw["agents"][0]["sentinel"] = serde_json::json!({"agent": 19});
    raw["codingAgentProfiles"] = serde_json::json!({
        "schemaVersion": 2,
        "sentinel": {"sibling": 23},
        "profilesByAgent": {"alpha":{"A":{"command":"--model x","id_hash":"physical", "id_hash_debug":["forged-physical"], "sentinel":17}}}
    });
    b3_write(&physical, &raw);
    if protected != "neither" {
        let mut overlay = serde_json::Map::new();
        let mut value = raw[protected].clone();
        if protected == "agents" {
            value[0].as_object_mut().unwrap().remove("baseCommand");
            value[0]["command"] = serde_json::json!("codex");
        } else {
            value["profilesByAgent"]["alpha"]["A"]["command"] = serde_json::json!("--effort high");
        }
        overlay.insert(protected.into(), value);
        b3_write(
            &b3_overlay_path(temp.path()),
            &serde_json::Value::Object(overlay),
        );
    }
    let mut loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    if protected == "agents" {
        loaded
            .coding_agent_profiles
            .profiles_by_agent
            .get_mut("alpha")
            .unwrap()
            .get_mut("A")
            .unwrap()
            .command = "--model y".into();
    } else {
        loaded.agents[0].command = "other-base".into();
    }
    let written = super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    let cell = &written.coding_agent_profiles.profiles_by_agent["alpha"]["A"];
    let effective_base = if protected == "agents" {
        "codex"
    } else {
        "other-base"
    };
    let effective_args = if protected == "codingAgentProfiles" {
        "--effort high"
    } else if protected == "agents" {
        "--model y"
    } else {
        "--model x"
    };
    assert_eq!(
        cell.id_hash_debug,
        Some(vec![effective_base.into(), effective_args.into()])
    );
    assert_eq!(
        cell.id_hash,
        Some(super::configuration_identity(
            &format!("{effective_base}{effective_args}"),
            &[]
        ))
    );
    let groups = b3_object(&b3_sidecar_path(temp.path()));
    if protected == "codingAgentProfiles" {
        let retained = groups
            .get(protected)
            .cloned()
            .unwrap_or_else(|| b3_object(&path)[protected].clone());
        let mut expected = raw[protected].clone();
        if !sidecar {
            expected["identityByAgent"] = serde_json::json!({
                "alpha": super::agent_profile_identity(&written.agents[0])
            });
        }
        assert_eq!(retained, expected);
    } else {
        let expected_base = if protected == "agents" {
            "claude"
        } else {
            "other-base"
        };
        let expected_args = if protected == "agents" {
            "--model y"
        } else {
            "--model x"
        };
        let hash = super::configuration_identity(expected_base, &[]);
        let main = b3_object(&path);
        let profiles = groups
            .get("codingAgentProfiles")
            .or_else(|| main.get("codingAgentProfiles"))
            .expect("physical profile owner");
        assert_eq!(profiles["profileBindingVersion"], 1);
        assert_eq!(
            profiles["profilesByBaseHash"][&hash]["A"],
            serde_json::json!({
                "command":"--model x", "additionalArguments":expected_args,
                "enabled":true, "env":{}, "notes":"", "sentinel":17
            })
        );
        assert_eq!(profiles["sentinel"], raw["codingAgentProfiles"]["sentinel"]);
        for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
            assert!(profiles.get(key).is_none());
        }
    }
    if protected == "agents" {
        let retained = groups
            .get("agents")
            .cloned()
            .unwrap_or_else(|| b3_object(&path)["agents"].clone());
        assert_eq!(retained, raw["agents"]);
    }
    let reloaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    assert_eq!(
        reloaded.coding_agent_profiles,
        written.coding_agent_profiles
    );
}

#[test]
fn issue_3047_pair_ownership_matrix_and_base_or_profile_only_edits() {
    for sidecar in [false, true] {
        for protected in ["agents", "codingAgentProfiles", "neither"] {
            check_issue_3047_pair_ownership_case(sidecar, protected);
        }
    }
    // Presence is independent of emptiness. Exercise the real writer with
    // sparse legacy groups, including D13's partially protected group.
    for case in [
        "present-absent",
        "absent-present",
        "absent-absent",
        "empty-present",
        "sidecar-empty",
    ] {
        let (temp, path) = hash_activation_seed(false);
        let mut raw = serde_json::Value::Object(b3_object(&path));
        raw["agents"] = serde_json::json!([]);
        let sparse =
            serde_json::json!({"schemaVersion":2,"sentinel":{"raw":29},"profilesByAgent":{}});
        if case.starts_with("absent-") {
            raw.as_object_mut().unwrap().remove("codingAgentProfiles");
        } else {
            raw["codingAgentProfiles"] = sparse.clone();
        }
        let overlay = match case {
            "present-absent" => {
                serde_json::json!({"codingAgentProfiles":{"identityByAgent":{"o":"1234567890abcdef"}}})
            }
            "absent-present" => {
                serde_json::json!({"agents":[b3_agent("alpha", "Effective only")]})
            }
            _ => {
                serde_json::json!({"codingAgentProfiles":{"profilesByAgent":{"o":{"A":{"command":"--model x"}}}}})
            }
        };
        if case == "sidecar-empty" {
            b3_write(
                &b3_sidecar_path(temp.path()),
                &serde_json::json!({"agents":[],"codingAgentProfiles":{}}),
            );
        }
        b3_write(&path, &raw);
        b3_write(&b3_overlay_path(temp.path()), &overlay);
        let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert!(
            loaded
                .local_overlay_state
                .owns_top_level("codingAgentProfiles"),
            "{case}"
        );
        super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
        let main = b3_object(&path);
        let groups = b3_object(&b3_sidecar_path(temp.path()));
        match case {
            "present-absent" => assert_eq!(main["codingAgentProfiles"], sparse),
            "absent-present" => assert_eq!(
                main["codingAgentProfiles"],
                serde_json::json!({"identityByAgent":{}})
            ),
            "absent-absent" => assert!(!main.contains_key("codingAgentProfiles")),
            "empty-present" => {
                let mut expected = sparse.clone();
                expected["identityByAgent"] = serde_json::json!({});
                assert_eq!(main["codingAgentProfiles"], expected);
            }
            "sidecar-empty" => {
                assert_eq!(main["codingAgentProfiles"], sparse);
                assert_eq!(groups["codingAgentProfiles"], serde_json::json!({}));
            }
            _ => unreachable!(),
        }
        if case != "sidecar-empty" {
            assert!(!groups.contains_key("codingAgentProfiles"), "{case}");
        }
    }
}

#[test]
fn issue_3047_pair_effective_base_without_physical_base_and_rejected_overlay() {
    let (temp, path) = hash_activation_seed(true);
    let sidecar = b3_sidecar_path(temp.path());
    let mut groups = serde_json::Value::Object(b3_object(&sidecar));
    groups.as_object_mut().unwrap().remove("agents");
    groups["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
        "alpha":{"A":{"additionalArguments":"--model x","id_hash":"stale"}}
    });
    b3_write(&sidecar, &groups);
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({
            "agents":[b3_agent("alpha", "Effective only")]
        }),
    );
    let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    let written = super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    assert_eq!(
        written.coding_agent_profiles.profiles_by_agent["alpha"]["A"].id_hash,
        Some(super::configuration_identity("claude--modelx", &[]))
    );
    let raw = b3_object(&sidecar);
    assert!(!raw.contains_key("agents"));
    assert!(loaded
        .local_overlay_state
        .owns_top_level("codingAgentProfiles"));
    assert!(!groups.as_object().unwrap().contains_key("agents"));
    assert_eq!(raw["codingAgentProfiles"], groups["codingAgentProfiles"]);
    assert_eq!(
        raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["id_hash"],
        "stale"
    );

    // Rejected overlays still derive the final accepted physical view.
    let (temp, path) = hash_activation_seed(true);
    let sidecar = b3_sidecar_path(temp.path());
    let mut groups = serde_json::Value::Object(b3_object(&sidecar));
    groups["codingAgentProfiles"]["profilesByAgent"] = serde_json::json!({
        "alpha":{"A":{"additionalArguments":"--model x","id_hash":"stale"}}
    });
    b3_write(&sidecar, &groups);
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({"defaultShellArgs":"bad"}),
    );
    let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    assert!(loaded.local_overlay_state.rejection().is_some());
    assert_eq!(
        loaded.coding_agent_profiles.profiles_by_agent["alpha"]["A"].id_hash,
        Some(super::configuration_identity("claude--modelx", &[]))
    );
    let previous = std::fs::read(&path).unwrap();
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::SettingsSaveStage::AtomicReplace))
    });
    let result = super::save_settings_to_path_preserving_project_paths(&loaded, &path);
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    let saved = b3_object(&sidecar);
    assert_eq!(saved["codingAgentProfiles"]["profileBindingVersion"], 1);
    assert!(saved["codingAgentProfiles"]
        .get("profilesByAgent")
        .is_none());
    assert_eq!(
        saved["codingAgentProfiles"]["profilesByBaseHash"]
            [&super::configuration_identity("claude", &[])]["A"],
        serde_json::json!({"additionalArguments":"--model x","enabled":true,"env":{},"notes":""})
    );
}

fn hash_activation_assert(settings: &super::AppSettings) {
    for agent in &settings.agents {
        let mut leaves = vec![agent.command.clone()];
        for env in &agent.envs {
            leaves.extend([env.key.clone(), env.value.clone()]);
        }
        assert_eq!(agent.id_hash_debug, Some(leaves.clone()));
        if let Some(cells) = settings
            .coding_agent_profiles
            .profiles_by_agent
            .get(&agent.id)
        {
            let first = settings
                .agents
                .iter()
                .find(|row| row.id == agent.id)
                .unwrap();
            for cell in cells.values() {
                let mut pair = vec![first.command.clone()];
                for env in &first.envs {
                    pair.extend([env.key.clone(), env.value.clone()]);
                }
                pair.push(cell.command.clone());
                for (key, value) in &cell.env {
                    pair.extend([key.clone(), value.clone()]);
                }
                assert_eq!(cell.id_hash_debug, Some(pair));
            }
        }
        assert_eq!(
            agent.id_hash.as_ref(),
            Some(&super::configuration_identity(&agent.command, &agent.envs))
        );
    }
}

#[test]
fn id_hash_activation_three_loaders_derive_without_writes_or_backups() {
    for sidecar in [false, true] {
        let (temp, path) = hash_activation_seed(sidecar);
        let physical = if sidecar {
            b3_sidecar_path(temp.path())
        } else {
            path.clone()
        };
        let mut raw = serde_json::Value::Object(b3_object(&physical));
        raw["agents"][0]["id_hash_debug"] = serde_json::json!(["old-base"]);
        raw["codingAgentProfiles"]["profilesByAgent"]["alpha"]["A"]["id_hash_debug"] =
            serde_json::json!(["old-pair"]);
        b3_write(&physical, &raw);
        // CLI views remain read-only before the admitted GUI conversion.
        let before_cli = hash_activation_files(temp.path());
        super::load_settings_for_cli_from_path(&path);
        super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert_eq!(hash_activation_files(temp.path()), before_cli);
        let first_gui = super::load_settings_from_path(&path);
        hash_activation_assert(&first_gui);
        let stored = b3_object(&b3_sidecar_path(temp.path()));
        assert_eq!(stored["codingAgentProfiles"]["profileBindingVersion"], 1);
        for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
            assert!(stored["codingAgentProfiles"].get(key).is_none());
        }
        let before = hash_activation_files(temp.path());
        for (loader, settings) in b3_all_loaders(&path) {
            assert_eq!(settings.agents[0].id, "alpha", "{loader}");
            hash_activation_assert(&settings);
            assert_eq!(
                settings.agents[0].id_hash_debug,
                Some(vec!["claude".into()]),
                "{loader}"
            );
            assert_eq!(
                settings.coding_agent_profiles.profiles_by_agent["alpha"]["A"].id_hash_debug,
                Some(vec!["claude".into(), "".into()]),
                "{loader}"
            );
            assert_eq!(hash_activation_files(temp.path()), before, "{loader}");
        }
    }
}

#[test]
fn id_hash_activation_direct_save_backfills_literal_and_reaches_fixed_point() {
    for supplied in [None, Some("forged".to_string())] {
        let (temp, path) = hash_activation_seed(false);
        let mut settings = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        settings.agents[0].id_hash = supplied;
        let written =
            super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
        let raw = b3_object(&b3_sidecar_path(temp.path()));
        assert_eq!(
            raw["agents"][0]["id_hash"],
            written.agents[0].id_hash.clone().unwrap()
        );
        assert!(raw["agents"][0].get("idHash").is_none());
        assert_eq!(written.agents[0].id, "alpha");
        let first = hash_activation_files(temp.path());
        let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
        assert_eq!(hash_activation_files(temp.path()), first);
    }
}

#[test]
fn id_hash_activation_all_untrusted_metadata_is_replaced() {
    for supplied in [
        serde_json::Value::Null,
        serde_json::json!(false),
        serde_json::json!(42),
        serde_json::json!(1.5),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!(""),
        serde_json::json!("cid1:stale"),
        serde_json::json!("forged"),
    ] {
        let (temp, path) = hash_activation_seed(true);
        let sidecar = b3_sidecar_path(temp.path());
        let mut raw = serde_json::Value::Object(b3_object(&sidecar));
        raw["agents"][0]["id_hash"] = supplied.clone();
        raw["agents"][0]["id_hash_debug"] = supplied;
        b3_write(&sidecar, &raw);
        let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        hash_activation_assert(&loaded);
        let written =
            super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
        assert_eq!(
            b3_object(&sidecar)["agents"][0]["id_hash"],
            written.agents[0].id_hash.clone().unwrap()
        );
        assert_eq!(
            b3_object(&sidecar)["agents"][0]["id_hash_debug"],
            serde_json::json!(["claude"])
        );
    }
}

#[test]
fn id_hash_activation_malformed_real_fields_refuse_overwrite() {
    for bad in [
        serde_json::json!("not-array"),
        serde_json::json!([{"id":"alpha","label":"A","color":"red","command":7}]),
        serde_json::json!([{"id":"alpha","label":"A","color":"red","command":"tool","envs":{}}]),
    ] {
        let (temp, path) = hash_activation_seed(true);
        let sidecar = b3_sidecar_path(temp.path());
        let mut raw = serde_json::Value::Object(b3_object(&sidecar));
        raw["agents"] = bad.clone();
        b3_write(&sidecar, &raw);
        let before = hash_activation_files(temp.path());
        let error = super::read_agents_instance_file(&sidecar).unwrap_err();
        assert!(error.contains("undecodable agents array"), "{bad}: {error}");
        // Existing strict loading exposes an unreadable layer in a valid
        // unrelated-settings snapshot; it does not always return Err.
        let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert!(
            matches!(&loaded.agents_layer, super::AgentsLayerState::Unreadable { path: failed, .. } if failed == &sidecar),
            "{bad}"
        );
        assert!(loaded.agents.is_empty(), "{bad}");
        assert!(super::save_settings_to_path_preserving_project_paths(
            &super::AppSettings::default(),
            &path
        )
        .is_err());
        assert_eq!(hash_activation_files(temp.path()), before);
    }
}

#[test]
fn id_hash_activation_overlay_owned_physical_and_effective_remain_distinct() {
    for (sidecar, field) in [
        (false, "baseCommand"),
        (true, "baseCommand"),
        (false, "command"),
        (true, "command"),
    ] {
        let (temp, path) = hash_activation_seed(sidecar);
        let physical_path = if sidecar {
            b3_sidecar_path(temp.path())
        } else {
            path.clone()
        };
        let mut physical = serde_json::Value::Object(b3_object(&physical_path));
        let physical_hash = if sidecar {
            super::configuration_identity("claude", &[])
        } else {
            "retained-physical-metadata".into()
        };
        physical["agents"][0]["id_hash"] = serde_json::json!(physical_hash);
        physical["agents"][0]["id_hash_debug"] = serde_json::json!(["retained-forged"]);
        physical["agents"][0]["unknownRowField"] = serde_json::json!({"keep":9});
        b3_write(&physical_path, &physical);
        let raw_before = physical["agents"].clone();
        let mut overlay = raw_before.clone();
        overlay[0].as_object_mut().unwrap().remove("baseCommand");
        overlay[0].as_object_mut().unwrap().remove("command");
        overlay[0][field] = serde_json::json!("codex --different");
        overlay[0]["envs"] =
            serde_json::json!([{"key":"MODE","value":"effective","enabled":false}]);
        overlay[0]["id_hash"] = serde_json::json!("forged-effective");
        b3_write(
            &b3_overlay_path(temp.path()),
            &serde_json::json!({"agents":overlay}),
        );
        let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        let written =
            super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
        hash_activation_assert(&written);
        assert_eq!(written.agents[0].command, "codex --different");
        assert_eq!(b3_object(&physical_path)["agents"], raw_before);
        assert_eq!(written.agents[0].id, "alpha");
        let reload = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert_eq!(
            serde_json::to_value(&reload.agents).unwrap(),
            serde_json::to_value(&written.agents).unwrap()
        );
        assert_ne!(
            written.agents[0].id_hash.as_deref(),
            Some(physical_hash.as_str())
        );
    }
}

#[test]
fn id_hash_activation_rejected_overlay_and_terminal_cas_derive_final_view() {
    let (temp, path) = hash_activation_seed(true);
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({"defaultShellArgs":"bad"}),
    );
    let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    hash_activation_assert(&loaded);
    assert!(loaded.local_overlay_state.rejection().is_some());
    let mut stale = loaded.clone();
    stale.agents[0].id_hash = Some("forged".into());
    let unchanged = super::compare_and_set_terminal_snapshots_enabled_at_path(
        &stale,
        &path,
        stale.terminal_snapshots_enabled,
        stale.terminal_snapshots_enabled,
    )
    .unwrap();
    hash_activation_assert(&unchanged);
    let written = super::save_settings_to_path_preserving_project_paths(&stale, &path).unwrap();
    hash_activation_assert(&written);
    assert_eq!(
        b3_object(&b3_sidecar_path(temp.path()))["agents"][0]["id_hash"],
        written.agents[0].id_hash.clone().unwrap()
    );
}

#[test]
fn id_hash_activation_main_publisher_failure_keeps_diagnostic_and_no_success() {
    let (temp, path) = hash_activation_seed(true);
    let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    let previous = std::fs::read(&path).unwrap();
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        *slot.borrow_mut() = Some((path.clone(), super::SettingsSaveStage::AtomicReplace))
    });
    let result = super::save_settings_to_path_preserving_project_paths(&loaded, &path);
    super::PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    // Preserve the existing partial-publication behavior and report failure.
    assert!(b3_object(&b3_sidecar_path(temp.path()))["agents"][0]["id_hash"].is_string());
    hash_activation_assert(&super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap());
}

#[test]
fn id_hash_activation_legacy_duplicates_preserve_rows_and_refuse_ambiguous_changes() {
    let (temp, path) = hash_activation_seed(true);
    let sidecar = b3_sidecar_path(temp.path());
    let mut raw = serde_json::Value::Object(b3_object(&sidecar));
    let row = raw["agents"][0].clone();
    raw["agents"].as_array_mut().unwrap().push(row.clone());
    let mut third = row.clone();
    third["id"] = serde_json::json!("beta");
    third["command"] = serde_json::json!("unique-command");
    raw["agents"].as_array_mut().unwrap().push(third);
    b3_write(&sidecar, &raw);
    let mut settings = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    settings.agents.reverse();
    super::normalize_agent_order(&mut settings.agents);
    settings.log_level = Some("debug".into());
    let written = super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(written.agents.iter().filter(|a| a.id == "alpha").count(), 2);
    hash_activation_assert(&written);
    let mut increased = written.clone();
    increased.agents.push(serde_json::from_value(row).unwrap());
    let before_identity = b3_object(&sidecar);
    let mut after_identity = before_identity.clone();
    after_identity.insert(
        "agents".into(),
        serde_json::to_value(&increased.agents).unwrap(),
    );
    assert!(super::validate_physical_identity_delta(
        &before_identity,
        &after_identity,
        true,
        false
    )
    .unwrap_err()
    .contains("DuplicateConfigurationIdentity"));
    let mut edited = written.clone();
    edited.agents[1].command = "repair-extra-character".into();
    let mut partially_deleted = written.clone();
    partially_deleted.agents.remove(1);
    for candidate in [increased, edited, partially_deleted] {
        let before = hash_activation_files(temp.path());
        let error =
            super::save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap_err();
        assert!(error.contains("duplicate agent ID 'alpha'"));
        assert!(error.contains("unprovable row correspondence"));
        assert_eq!(hash_activation_files(temp.path()), before);
        let reloaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
        assert_eq!(
            serde_json::to_value(&reloaded.agents).unwrap(),
            serde_json::to_value(&written.agents).unwrap()
        );
        assert_eq!(reloaded.log_level, written.log_level);
        hash_activation_assert(&reloaded);
    }
}

#[test]
fn e2432_duplicate_rows_keep_existing_general_save_control() {
    id_hash_activation_legacy_duplicates_preserve_rows_and_refuse_ambiguous_changes();
}

#[test]
fn e2432_id_hash_activation_anagram_alias_is_source_local() {
    let (_temp, path) = hash_activation_seed(true);
    let mut settings = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    let mut alias = settings.agents[0].clone();
    alias.id = "beta".into();
    alias.label = "Alias".into();
    alias.command = "eduacl".into();
    settings.agents.push(alias);
    let written = super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(written.agents[0].id_hash, written.agents[1].id_hash);
    // Sharing compares payloads, while debug metadata keeps each physical
    // row's original command/env strings. Pair hashes still compare equal.
    let payload = |id: &str| {
        let agent = written.agents.iter().find(|agent| agent.id == id).unwrap();
        let mut cells = written.coding_agent_profiles.profiles_by_agent[id].clone();
        for cell in cells.values_mut() {
            assert_eq!(
                cell.id_hash,
                Some(super::pair_identity(&agent.command, &agent.envs, cell))
            );
            assert_eq!(
                cell.id_hash_debug,
                Some(super::pair_identity_debug(
                    &agent.command,
                    &agent.envs,
                    cell
                ))
            );
            cell.id_hash_debug = None;
        }
        cells
    };
    assert_eq!(payload("alpha"), payload("beta"));
    let (other_temp, other_path) = hash_activation_seed(true);
    let other_before = hash_activation_files(other_temp.path());
    let other = super::load_settings_for_cli_strict_from_path(Some(&other_path)).unwrap();
    assert_eq!(other.agents.len(), 1);
    let mut edited = written;
    edited
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "same-source edit".into();
    let edited = super::save_settings_to_path_preserving_project_paths(&edited, &path).unwrap();
    assert_eq!(
        edited.coding_agent_profiles.profiles_by_agent["beta"]["A"].notes,
        "same-source edit"
    );
    assert_eq!(hash_activation_files(other_temp.path()), other_before);
}

#[test]
fn id_hash_activation_profile_delta_preserves_history_and_rejects_new_pair() {
    let (temp, path) = hash_activation_seed(true);
    let sidecar = b3_sidecar_path(temp.path());
    let mut raw = serde_json::Value::Object(b3_object(&sidecar));
    raw["codingAgentProfiles"]["profilesByAgent"]["alpha"] = serde_json::json!({
        "A":{"command":"--one"},"B":{"command":"--one"},"C":{"command":"--two"}
    });
    let mut other = raw["agents"][0].clone();
    other["id"] = serde_json::json!("beta");
    other["command"] = serde_json::json!("distinct-command");
    other["baseCommand"] = serde_json::json!("distinct-command");
    raw["agents"].as_array_mut().unwrap().push(other);
    raw["codingAgentProfiles"]["profilesByAgent"]["beta"] =
        serde_json::json!({"A":{"command":"--one"}});
    b3_write(&sidecar, &raw);
    let mut loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("beta")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .command = "--unrelated-edit".into();
    loaded.agents[0].label = "Renamed".into();
    super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .get_mut("C")
        .unwrap()
        .command = "--unique".into();
    super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .get_mut("C")
        .unwrap()
        .command = "--one".into();
    let before = hash_activation_files(temp.path());
    let error = super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap_err();
    assert!(error.contains("DuplicateProfileIdentity") && error.contains("Renamed"));
    assert_eq!(hash_activation_files(temp.path()), before);
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .remove("B");
    loaded
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("alpha")
        .unwrap()
        .remove("C");
    assert!(super::save_settings_to_path_preserving_project_paths(&loaded, &path).is_ok());
}

#[test]
fn id_hash_activation_legacy_before_is_read_when_only_profiles_are_externalized() {
    let (temp, path) = hash_activation_seed(false);
    let mut base = serde_json::Value::Object(b3_object(&path));
    let mut duplicate = base["agents"][0].clone();
    duplicate["id"] = serde_json::json!("beta");
    base["agents"].as_array_mut().unwrap().push(duplicate);
    b3_write(
        &b3_sidecar_path(temp.path()),
        &serde_json::json!({
            "codingAgentProfiles": base.as_object_mut().unwrap().remove("codingAgentProfiles").unwrap()
        }),
    );
    b3_write(&path, &base);
    let loaded = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    let written = super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    assert_eq!(written.agents.len(), 2);
    hash_activation_assert(&written);
}

fn b3_all_loaders(path: &std::path::Path) -> Vec<(&'static str, super::AppSettings)> {
    vec![
        (
            "load_settings_from_path",
            super::load_settings_from_path(path),
        ),
        (
            "load_settings_for_cli",
            super::load_settings_for_cli_from_path(path),
        ),
        (
            "load_settings_for_cli_strict",
            super::load_settings_for_cli_strict_from_path(Some(path)).unwrap(),
        ),
    ]
}

#[test]
fn the_export_moves_both_groups_and_leaves_memory_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    let before = super::load_settings_for_cli_from_path(&path);
    assert!(!b3_sidecar_path(temp.path()).exists());

    let loaded = super::load_settings_from_path(&path);

    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Alpha Recognisable");
    assert_eq!(
        sidecar["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::configuration_identity("claude", &[])]["A"],
        "Recognisable P"
    );
    let disk = b3_object(&path);
    assert!(!disk.contains_key("agents"), "{disk:?}");
    assert!(!disk.contains_key("codingAgentProfiles"), "{disk:?}");
    assert_eq!(
        serde_json::to_value(&loaded.agents).unwrap(),
        serde_json::to_value(&before.agents).unwrap()
    );
    // The export's save adds the #2450 identity rows, as any save does; every
    // user-authored value is unchanged.
    let mut loaded_profiles = loaded.coding_agent_profiles.clone();
    loaded_profiles.identity_by_agent.clear();
    assert_eq!(loaded_profiles, before.coding_agent_profiles);
    assert_eq!(b3_labels(&loaded), vec!["Alpha Recognisable".to_string()]);
    assert_eq!(b3_marker(&loaded).as_deref(), Some("Recognisable P"));
}

#[test]
fn all_three_loaders_see_the_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    super::load_settings_from_path(&path);
    assert!(!b3_object(&path).contains_key("agents"));

    for (loader, settings) in b3_all_loaders(&path) {
        assert_eq!(
            b3_labels(&settings),
            vec!["Alpha Recognisable".to_string()],
            "{loader} did not see the agents file"
        );
        assert_eq!(
            b3_marker(&settings).as_deref(),
            Some("Recognisable P"),
            "{loader} did not see the agents file"
        );
    }
}

#[test]
fn a_cli_strict_load_then_save_does_not_write_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    super::load_settings_from_path(&path);

    let settings = super::load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Alpha Recognisable");
    assert_eq!(sidecar["agents"].as_array().unwrap().len(), 1);
    assert_eq!(
        sidecar["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::configuration_identity("claude", &[])]["A"],
        "Recognisable P"
    );
}

#[test]
fn reconcile_mode_drops_the_legacy_keys_from_the_disk_object() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    let settings = super::load_settings_for_cli_from_path(&path);

    super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();

    let disk = b3_object(&path);
    assert!(!disk.contains_key("agents"), "{disk:?}");
    assert!(!disk.contains_key("codingAgentProfiles"), "{disk:?}");
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Alpha Recognisable");
}

#[test]
fn preserve_mode_drops_the_legacy_keys_from_the_typed_object() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    let settings = super::load_settings_for_cli_from_path(&path);

    super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let disk = b3_object(&path);
    assert!(!disk.contains_key("agents"), "{disk:?}");
    assert!(!disk.contains_key("codingAgentProfiles"), "{disk:?}");
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Alpha Recognisable");
}

/// E9 and E10: a directory at the agents file's name.
fn b3_assert_a_failed_sidecar_write_strips_nothing(reconcile: bool) {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    std::fs::create_dir(b3_sidecar_path(temp.path())).unwrap();
    let settings = super::load_settings_for_cli_from_path(&path);
    let before = std::fs::read(&path).unwrap();

    let result = if reconcile {
        super::save_settings_with_project_paths_to_path(&settings, &path).map(|_| ())
    } else {
        super::save_settings_to_path_preserving_project_paths(&settings, &path).map(|_| ())
    };

    assert!(result.is_err(), "the save must fail");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let disk = b3_object(&path);
    assert!(disk.contains_key("agents") && disk.contains_key("codingAgentProfiles"));
    assert!(b3_sidecar_path(temp.path()).is_dir());
}

#[test]
fn reconcile_mode_keeps_both_keys_when_the_sidecar_write_fails() {
    b3_assert_a_failed_sidecar_write_strips_nothing(true);
}

#[test]
fn preserve_mode_keeps_both_keys_when_the_sidecar_write_fails() {
    b3_assert_a_failed_sidecar_write_strips_nothing(false);
}

/// E10b. The overlay reuses the base agent's id, so the D13 derived-id closure
/// adds no `codingAgentProfiles` path and the overlay owns `agents` only.
#[test]
fn a_preserve_save_keeps_an_overlay_owned_key_when_no_sidecar_holds_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_legacy_fixture("Base A", "Recognisable P"));
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({ "agents": [b3_agent("alpha", "Overlay B")] }),
    );
    let settings = super::load_settings_for_cli_from_path(&path);
    assert!(settings
        .local_overlay_state
        .owns_top_level(super::OVERLAY_KEY_AGENTS));
    assert!(!settings
        .local_overlay_state
        .owns_top_level("codingAgentProfiles"));

    super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert!(!sidecar.contains_key("agents"), "{sidecar:?}");
    assert!(sidecar.contains_key("codingAgentProfiles"), "{sidecar:?}");
    let disk = b3_object(&path);
    assert!(!disk.contains_key("codingAgentProfiles"), "{disk:?}");
    assert_eq!(disk["agents"][0]["label"], "Base A", "{disk:?}");
    std::fs::remove_file(b3_overlay_path(temp.path())).unwrap();
    let reloaded = super::load_settings_for_cli_from_path(&path);
    assert_eq!(b3_labels(&reloaded), vec!["Base A".to_string()]);
}

#[test]
fn the_export_aborts_without_stripping() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    std::fs::create_dir(b3_sidecar_path(temp.path())).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut settings = super::load_settings_for_cli_from_path(&path);
    let agents = serde_json::to_value(&settings.agents).unwrap();
    let profiles = settings.coding_agent_profiles.clone();

    assert!(!super::export_agents_to_instance_file(&mut settings, &path));
    let loaded = super::load_settings_from_path(&path);

    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(serde_json::to_value(&settings.agents).unwrap(), agents);
    assert_eq!(settings.coding_agent_profiles, profiles);
    assert_eq!(serde_json::to_value(&loaded.agents).unwrap(), agents);
    assert!(b3_sidecar_path(temp.path()).is_dir());
}

#[test]
fn an_overlay_that_owns_agents_blocks_the_export() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_legacy_fixture("Base A", "Recognisable P"));
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({ "agents": [b3_agent("alpha", "Overlay B")] }),
    );
    let mut settings = super::load_settings_for_cli_from_path(&path);
    assert!(!super::export_agents_to_instance_file(&mut settings, &path));

    let loaded = super::load_settings_from_path(&path);

    assert!(
        !b3_sidecar_path(temp.path()).exists()
            || !b3_object(&b3_sidecar_path(temp.path())).contains_key("agents")
    );
    assert!(
        b3_object(&path).contains_key("agents"),
        "E12: the settings file keeps agents"
    );
    assert_eq!(b3_labels(&loaded), vec!["Overlay B".to_string()]);
}

#[test]
fn a_sidecar_that_disagrees_with_the_legacy_keys_is_not_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let sidecar_bytes = b3_seed_sidecar(temp.path(), "Sidecar", "Sidecar P");
    b3_write(&path, &b3_legacy_fixture("Legacy", "Legacy P"));
    let mut settings = super::load_settings_for_cli_from_path(&path);
    assert!(!super::export_agents_to_instance_file(&mut settings, &path));

    let legacy = b3_object(&path);
    let loaded = super::load_settings_from_path(&path);

    assert_eq!(b3_labels(&loaded), vec!["Sidecar".to_string()]);
    assert_eq!(b3_marker(&loaded).as_deref(), Some("Sidecar P"));
    assert_ne!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        sidecar_bytes
    );
    let stable = std::fs::read(b3_sidecar_path(temp.path())).unwrap();
    super::load_settings_from_path(&path);
    assert_eq!(std::fs::read(b3_sidecar_path(temp.path())).unwrap(), stable);
    let disk = b3_object(&path);
    assert!(disk.contains_key("agents") && disk.contains_key("codingAgentProfiles"));

    super::save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();

    let disk = b3_object(&path);
    assert_eq!(disk["agents"], legacy["agents"]);
    assert_eq!(disk["codingAgentProfiles"], legacy["codingAgentProfiles"]);
    // The save may add the #2450 identity rows; the sidecar's values still win.
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Sidecar");
    assert_eq!(
        sidecar["codingAgentProfiles"]["profileLabelsByBaseHash"]
            [&super::configuration_identity("claude", &[])]["A"],
        "Sidecar P"
    );
}

#[test]
fn a_corrupt_sidecar_empties_nothing_and_blocks_the_strip() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_legacy_fixture("Legacy", "Legacy P"));
    std::fs::write(b3_sidecar_path(temp.path()), b"{ not json").unwrap();
    let before = std::fs::read(&path).unwrap();

    let loaded = super::load_settings_from_path(&path);

    assert_eq!(b3_labels(&loaded), vec!["Legacy".to_string()]);
    assert_eq!(b3_marker(&loaded).as_deref(), Some("Legacy P"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        b"{ not json"
    );
    assert!(super::save_settings_to_path_preserving_project_paths(&loaded, &path).is_err());
    let disk = b3_object(&path);
    assert!(disk.contains_key("agents") && disk.contains_key("codingAgentProfiles"));
    assert_eq!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        b"{ not json"
    );
}

#[test]
fn load_save_load_save_is_byte_stable() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &b3_legacy_fixture("Alpha Recognisable", "Recognisable P"),
    );
    let first = super::load_settings_from_path(&path);
    super::save_settings_to_path_preserving_project_paths(&first, &path).unwrap();
    let settings_bytes = std::fs::read(&path).unwrap();
    let sidecar_bytes = std::fs::read(b3_sidecar_path(temp.path())).unwrap();

    let mut second = super::load_settings_from_path(&path);
    assert!(!super::export_agents_to_instance_file(&mut second, &path));
    super::save_settings_to_path_preserving_project_paths(&second, &path).unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), settings_bytes);
    assert_eq!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        sidecar_bytes
    );
}

/// E16 and E17: an agents file holding A under an overlay owning `agents` with B.
fn b3_seed_overlay_over_sidecar(dir: &std::path::Path) -> (std::path::PathBuf, Vec<u8>) {
    let path = b3_settings_path(dir);
    let sidecar_bytes = b3_seed_sidecar(dir, "Sidecar A", "Sidecar P");
    b3_write(&path, &b3_migrated_fixture());
    b3_write(
        &b3_overlay_path(dir),
        &serde_json::json!({ "agents": [b3_agent("alpha", "Overlay B")] }),
    );
    (path, sidecar_bytes)
}

#[test]
fn the_overlay_still_beats_the_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let (path, _) = b3_seed_overlay_over_sidecar(temp.path());

    for (loader, settings) in b3_all_loaders(&path) {
        assert_eq!(
            b3_labels(&settings),
            vec!["Overlay B".to_string()],
            "{loader} let the agents file override the overlay"
        );
    }
}

#[test]
fn a_preserve_save_does_not_freeze_the_overlay_into_the_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let (path, _) = b3_seed_overlay_over_sidecar(temp.path());
    let settings = super::load_settings_for_cli_from_path(&path);
    assert_eq!(b3_labels(&settings), vec!["Overlay B".to_string()]);

    super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Sidecar A");
    let reloaded = super::load_settings_for_cli_from_path(&path);
    assert_eq!(b3_labels(&reloaded), vec!["Overlay B".to_string()]);
}

#[test]
fn reconcile_never_writes_the_callers_groups() {
    // An existing agents file is left byte for byte.
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let sidecar_bytes = b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    b3_write(&path, &b3_migrated_fixture());
    let mut settings = super::load_settings_for_cli_from_path(&path);
    settings.agents[0].label = "Stale B".to_string();
    super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();
    assert_eq!(
        std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
        sidecar_bytes
    );

    // No agents file: it is created from the disk object, not from the caller.
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_legacy_fixture("Disk A", "Disk P"));
    let mut settings = super::load_settings_for_cli_from_path(&path);
    settings.agents[0].label = "Stale B".to_string();
    super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(sidecar["agents"][0]["label"], "Disk A");
    assert_eq!(
        sidecar["codingAgentProfiles"]["profileLabelsByAgent"]["alpha"]["A"],
        "Disk P"
    );
}

#[test]
fn an_unparseable_sidecar_blocks_the_direct_save_in_both_modes() {
    for reconcile in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_legacy_fixture("Legacy", "Legacy P"));
        std::fs::write(b3_sidecar_path(temp.path()), b"[1, 2").unwrap();
        let settings = super::load_settings_for_cli_from_path(&path);

        let result = if reconcile {
            super::save_settings_with_project_paths_to_path(&settings, &path).map(|_| ())
        } else {
            super::save_settings_to_path_preserving_project_paths(&settings, &path).map(|_| ())
        };

        assert!(result.is_err(), "reconcile={reconcile}");
        assert_eq!(
            std::fs::read(b3_sidecar_path(temp.path())).unwrap(),
            b"[1, 2",
            "reconcile={reconcile}"
        );
        let disk = b3_object(&path);
        assert!(
            disk.contains_key("agents") && disk.contains_key("codingAgentProfiles"),
            "reconcile={reconcile}"
        );
    }
}

#[test]
fn the_sidecar_is_merged_into_the_base_before_the_overlay() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    b3_write(&path, &b3_migrated_fixture());
    for (loader, settings) in b3_all_loaders(&path) {
        assert_eq!(
            b3_labels(&settings),
            vec!["Sidecar A".to_string()],
            "{loader}"
        );
    }

    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({ "logLevel": "debug" }),
    );
    for (loader, settings) in b3_all_loaders(&path) {
        assert!(!settings.local_overlay_state.is_empty(), "{loader}");
        assert_eq!(
            b3_labels(&settings),
            vec!["Sidecar A".to_string()],
            "{loader}"
        );
    }
}

#[test]
fn the_merge_point_is_between_the_detection_and_the_pre_merge_clone() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    // A legacy-shape key inside the agents file must not flip the flag.
    let mut sidecar = b3_object(&b3_sidecar_path(temp.path()));
    sidecar["codingAgentProfiles"]
        .as_object_mut()
        .unwrap()
        .insert("letters".to_string(), serde_json::json!({}));
    b3_write(
        &b3_sidecar_path(temp.path()),
        &serde_json::Value::Object(sidecar),
    );
    let mut base = b3_migrated_fixture();
    base["agents"] = serde_json::json!("not an agents array");
    b3_write(&path, &base);
    // An overlay leaf that fails the decode, so the base fallback runs.
    b3_write(
        &b3_overlay_path(temp.path()),
        &serde_json::json!({ "defaultShellArgs": "not an array" }),
    );

    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, legacy_profiles) = super::parse_settings_json(&contents, "test", Some(&path))
        .expect("the undecodable fallback must carry the agents file's agents");

    assert!(!legacy_profiles);
    assert_eq!(b3_labels(&settings), vec!["Sidecar A".to_string()]);
    assert!(
        matches!(
            settings.local_overlay_state.rejection(),
            Some(crate::config::local_overlay::OverlayRejection::MergedValueUndecodable(_))
        ),
        "the fallback did not run: {:?}",
        settings.local_overlay_state.rejection()
    );
}

/// #2716 (B3) 6.1 (b), `blockingMenus` alone: every seeded entry survives in
/// order and field for field; an extra entry is one of the command's shipped
/// defaults; an array that is not all defaults matches the seed exactly.
pub(super) fn b3_assert_blocking_menus_legs(
    seeded: Option<&serde_json::Value>,
    held: Option<&serde_json::Value>,
    command: &str,
    label: &str,
) {
    let Some(seeded) = seeded else {
        assert!(
            held.is_none(),
            "{label}: an omitted blockingMenus was written"
        );
        return;
    };
    let seeded = seeded.as_array().expect("seeded blockingMenus array");
    let held = held
        .unwrap_or_else(|| panic!("{label}: blockingMenus dropped"))
        .as_array()
        .unwrap_or_else(|| panic!("{label}: blockingMenus is not an array"));
    let defaults: Vec<serde_json::Value> = super::default_blocking_menus_for_command(command)
        .iter()
        .map(|entry| serde_json::to_value(entry).unwrap())
        .collect();
    if !seeded.iter().all(|entry| defaults.contains(entry)) {
        assert_eq!(held, seeded, "{label}: a customized array changed");
        return;
    }
    let mut next = 0;
    for entry in held {
        if next < seeded.len() && *entry == seeded[next] {
            next += 1;
        } else {
            assert!(
                defaults.contains(entry),
                "{label}: {entry} is neither seeded nor a shipped default"
            );
        }
    }
    assert_eq!(
        next,
        seeded.len(),
        "{label}: a seeded entry was dropped or edited"
    );
}

/// #2716 (B3) 6.1 - the split form of a whole-file comparison. (a) Containment:
/// every seeded key but the two groups keeps its seeded value, and a key the
/// save adds equals a default `AppSettings`, save the seven environment keys the
/// S6 normalizer names. (b) The agents file holds every seeded field of both
/// groups; `blockingMenus` takes the three legs; `skip` names agent fields the
/// caller asserts separately.
fn b3_assert_split_matches_seed(path: &std::path::Path, seeded: &[u8], skip: &[&str]) {
    let seed: serde_json::Value = serde_json::from_slice(seeded).unwrap();
    let mut seed = seed.as_object().unwrap().clone();
    let physical_agents = seed.get("agents").cloned();
    let ambiguous_rows = super::physical_profile_rows(
            &serde_json::json!({"agents":physical_agents.clone().unwrap_or_else(|| serde_json::json!([]))}), true,
        ).is_err();
    let mut disk = b3_object(path);
    let sidecar = b3_object(&super::agents_instance_path(path));
    for key in super::AGENTS_INSTANCE_KEYS {
        assert!(
            !disk.contains_key(key),
            "{key} is still in the settings file"
        );
        disk.remove(key);
        let Some(seeded_group) = seed.remove(key) else {
            continue;
        };
        let held = sidecar
            .get(key)
            .unwrap_or_else(|| panic!("the agents file does not hold {key}"));
        match (&seeded_group, held) {
            (serde_json::Value::Array(seeded_rows), serde_json::Value::Array(held_rows)) => {
                b3_assert_seeded_rows(key, seeded_rows, held_rows, skip);
            }
            (serde_json::Value::Object(seeded_fields), serde_json::Value::Object(held)) => {
                if held.get("profileBindingVersion") == Some(&serde_json::json!(1)) {
                    b3_assert_canonical_seeded_profiles(
                        seeded_fields,
                        held,
                        physical_agents.as_ref(),
                    );
                    continue;
                }
                b3_assert_legacy_seeded_profiles(
                    key,
                    seeded_fields,
                    held,
                    physical_agents.as_ref(),
                    ambiguous_rows,
                );
            }
            other => panic!("{key}: unexpected shapes {other:?}"),
        }
    }
    let exempt = [
        super::FIELD_PROJECT_PATH,
        super::FIELD_PROJECT_PATH_REL,
        super::FIELD_PROJECT_PATHS,
        super::FIELD_PROJECT_PATHS_REL,
        super::FIELD_ARCHIVED,
        super::FIELD_ARCHIVED_REL,
        "rootToken",
    ];
    // r12k: the reference is the seed decoded by the loader's typed path and
    // serialized again, compared key by key, both directions.
    let typed: super::AppSettings = serde_json::from_slice(seeded).unwrap();
    let mut reference = match serde_json::to_value(&typed).unwrap() {
        serde_json::Value::Object(object) => object,
        other => panic!("AppSettings serialized to {other}"),
    };
    for key in super::AGENTS_INSTANCE_KEYS.iter().chain(exempt.iter()) {
        reference.remove(*key);
        disk.remove(*key);
    }
    let mut differing: Vec<&String> = reference
        .keys()
        .chain(disk.keys())
        .filter(|key| reference.get(*key) != disk.get(*key))
        .collect();
    differing.sort();
    differing.dedup();
    assert!(
        differing.is_empty(),
        "keys differ from the typed seed: {differing:?}"
    );
}

fn b3_assert_seeded_rows(
    key: &str,
    seeded_rows: &[serde_json::Value],
    held_rows: &[serde_json::Value],
    skip: &[&str],
) {
    assert_eq!(seeded_rows.len(), held_rows.len(), "{key} length");
    for (seeded_row, held_row) in seeded_rows.iter().zip(held_rows) {
        let command = seeded_row
            .get("baseCommand")
            .or_else(|| seeded_row.get("command"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let mut leaves = vec![command.to_string()];
        for env in seeded_row
            .get("envs")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            leaves.push(env["key"].as_str().unwrap().to_string());
            leaves.push(env["value"].as_str().unwrap().to_string());
        }
        assert_eq!(held_row["id_hash_debug"], serde_json::json!(leaves));
        if seeded_row.get("blockingMenus").is_none() {
            b3_assert_blocking_menus_legs(
                None,
                held_row.get("blockingMenus"),
                "",
                &format!("{key}.{}", seeded_row["id"]),
            );
        }
        for (field, value) in seeded_row.as_object().unwrap() {
            if skip.contains(&field.as_str()) {
                continue;
            }
            if field == "blockingMenus" {
                b3_assert_blocking_menus_legs(
                    Some(value),
                    held_row.get(field),
                    seeded_row["command"].as_str().unwrap_or_default(),
                    &format!("{key}.{}", seeded_row["id"]),
                );
                continue;
            }
            assert_eq!(held_row.get(field), Some(value), "{key}.{field}");
        }
    }
}

fn b3_assert_canonical_seeded_profiles(
    seeded_fields: &serde_json::Map<String, serde_json::Value>,
    held: &serde_json::Map<String, serde_json::Value>,
    physical_agents: Option<&serde_json::Value>,
) {
    // Compare storage directly against the seeded physical rows,
    // including unknown cells and sibling fields, never an ID view.
    let mut expected_cells = serde_json::Map::new();
    let mut expected_labels = serde_json::Map::new();
    let mut resolved = std::collections::HashSet::new();
    for row in physical_agents
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let agent: super::AgentConfig = serde_json::from_value(row.clone()).unwrap();
        assert!(
            resolved.insert(agent.id.clone()),
            "ambiguous rows cannot convert"
        );
        let hash = super::configuration_identity(&agent.command, &agent.envs);
        let mut cells = seeded_fields
            .get("profilesByAgent")
            .and_then(|map| map.get(&agent.id))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        for cell in cells.as_object_mut().unwrap().values_mut() {
            let typed: super::ProfileCellConfig = serde_json::from_value(cell.clone()).unwrap();
            let mut defaults = serde_json::to_value(typed).unwrap();
            defaults.as_object_mut().unwrap().remove("id_hash");
            defaults.as_object_mut().unwrap().remove("id_hash_debug");
            let raw = cell.as_object_mut().unwrap();
            raw.remove("id_hash");
            raw.remove("id_hash_debug");
            raw.extend(defaults.as_object().unwrap().clone());
            for key in ["preflight", "preflightTimeoutSeconds"] {
                if raw.get(key).is_some_and(serde_json::Value::is_null) {
                    raw.remove(key);
                }
            }
        }
        cells
            .as_object_mut()
            .unwrap()
            .entry("A".to_string())
            .or_insert_with(|| serde_json::to_value(super::empty_profile_cell()).unwrap());
        let labels = seeded_fields
            .get("profileLabelsByAgent")
            .and_then(|map| map.get(&agent.id))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(previous) = expected_cells.insert(hash.clone(), cells.clone()) {
            assert_eq!(previous, cells, "complete alias cells");
        }
        if let Some(previous) = expected_labels.insert(hash, labels.clone()) {
            assert_eq!(previous, labels, "complete alias labels");
        }
    }
    assert_eq!(
        held.get("profilesByBaseHash"),
        Some(&serde_json::Value::Object(expected_cells))
    );
    assert_eq!(
        held.get("profileLabelsByBaseHash"),
        Some(&serde_json::Value::Object(expected_labels))
    );
    let mut orphan_records = serde_json::Map::new();
    for (map_key, record_key) in [
        ("profilesByAgent", "cells"),
        ("profileLabelsByAgent", "labels"),
        ("identityByAgent", "identity"),
    ] {
        for (id, raw) in seeded_fields
            .get(map_key)
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
        {
            if !resolved.contains(id) {
                let record = orphan_records
                    .entry(id.clone())
                    .or_insert_with(|| serde_json::json!([{}]));
                record[0][record_key] = raw.clone();
            }
        }
        assert!(!held.contains_key(map_key));
    }
    if !orphan_records.is_empty() {
        assert_eq!(
            held.get("legacyProfileOrphans"),
            Some(&serde_json::Value::Object(orphan_records))
        );
    }
    for (field, value) in seeded_fields {
        if !["profilesByAgent", "profileLabelsByAgent", "identityByAgent"].contains(&field.as_str())
        {
            assert_eq!(held.get(field), Some(value), "raw sibling {field}");
        }
    }
}

fn b3_assert_legacy_seeded_profiles(
    key: &str,
    seeded_fields: &serde_json::Map<String, serde_json::Value>,
    held: &serde_json::Map<String, serde_json::Value>,
    physical_agents: Option<&serde_json::Value>,
    ambiguous_rows: bool,
) {
    for (field, value) in seeded_fields {
        let mut expected = value.clone();
        if field == "profilesByAgent" && !ambiguous_rows {
            // Compare the complete original cells, including old
            // command values, plus only the new physical leaf.
            for (id, cells) in expected.as_object_mut().unwrap().iter_mut() {
                // First raw ID wins before decoding, including an
                // unresolvable first row. Never use effective/held.
                let base = physical_agents
                    .and_then(serde_json::Value::as_array)
                    .and_then(|rows| {
                        rows.iter().find(|row| {
                            row.get("id").and_then(serde_json::Value::as_str) == Some(id.as_str())
                        })
                    })
                    .and_then(|row| serde_json::from_value::<super::AgentConfig>(row.clone()).ok());
                for cell in cells.as_object_mut().unwrap().values_mut() {
                    assert!(cell.get("additionalArguments").is_none());
                    cell["additionalArguments"] = cell["command"].clone();
                    let profile: super::ProfileCellConfig =
                        serde_json::from_value(cell.clone()).unwrap();
                    let hash = base.as_ref().map(|base| {
                        // Independent raw-field preimage, every base
                        // env row (disabled included), then cell env.
                        let parts = std::iter::once(base.command.as_str())
                            .chain(
                                base.envs
                                    .iter()
                                    .flat_map(|env| [env.key.as_str(), env.value.as_str()]),
                            )
                            .chain(std::iter::once(profile.command.as_str()))
                            .chain(
                                profile
                                    .env
                                    .iter()
                                    .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
                            );
                        let mut chars: Vec<char> = parts
                            .flat_map(str::chars)
                            .flat_map(char::to_lowercase)
                            .filter(|c| !c.is_whitespace())
                            .collect();
                        chars.sort_unstable();
                        serde_json::Value::String(format!(
                            "cid1:{:x}",
                            <sha2::Sha256 as sha2::Digest>::digest(
                                chars.iter().collect::<String>()
                            )
                        ))
                    });
                    // Replace only expected derived metadata. None
                    // asserts concrete absence; observed stays raw.
                    let object = cell.as_object_mut().unwrap();
                    object.remove("id_hash");
                    object.extend(hash.map(|hash| ("id_hash".to_string(), hash)));
                    object.remove("id_hash_debug");
                    if let Some(base) = &base {
                        let mut leaves = vec![base.command.clone()];
                        for env in &base.envs {
                            leaves.extend([env.key.clone(), env.value.clone()]);
                        }
                        leaves.push(profile.command.clone());
                        for (key, value) in &profile.env {
                            leaves.extend([key.clone(), value.clone()]);
                        }
                        object.insert("id_hash_debug".into(), serde_json::json!(leaves));
                    }
                }
            }
        }
        assert_eq!(held.get(field), Some(&expected), "{key}.{field}");
    }
}

// #2716 (B3) - rows E25 to E31 and E40 of the phase plan.

fn b3_seed_corrupt_sidecar(dir: &std::path::Path) {
    std::fs::write(b3_sidecar_path(dir), "{ this is not json").unwrap();
}

fn b3_marked(settings: &super::AppSettings) -> Option<std::path::PathBuf> {
    settings
        .agents_layer
        .unreadable_path()
        .map(|p| p.to_path_buf())
}

#[test]
fn the_terminal_snapshot_cas_reads_the_sidecar_and_never_writes_an_empty_agents_list() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_migrated_fixture());
    b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    let current = super::load_settings_for_cli_from_path(&path);
    let gate = current.terminal_snapshots_enabled;

    let returned =
        super::compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, gate, !gate)
            .expect("the CAS write must succeed");

    assert_eq!(
        returned.terminal_snapshots_enabled, !gate,
        "the CAS did not write"
    );

    assert_eq!(b3_labels(&returned), vec!["Sidecar A".to_string()]);
    let sidecar = b3_object(&b3_sidecar_path(temp.path()));
    assert_eq!(
        sidecar["agents"][0]["label"], "Sidecar A",
        "the CAS wrote {:?} into the agents file",
        sidecar["agents"]
    );
    assert!(!b3_object(&path).contains_key("agents"));
}

#[test]
fn a_deleted_sidecar_loses_only_the_agents_and_keeps_every_other_setting() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(&path, &b3_migrated_fixture());
    assert!(!b3_sidecar_path(temp.path()).exists());

    let loaded = super::load_settings_from_path(&path);

    assert_eq!(loaded.default_shell, "test-shell");
    assert_eq!(loaded.root_token.as_deref(), Some("base-token"));
    assert!(loaded.agents.is_empty());
}

#[test]
fn a_migrated_settings_file_passes_the_non_project_validation() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let migrated = b3_migrated_fixture();
    super::validate_non_project_settings(migrated.as_object().unwrap())
        .expect("a settings file without agents must pass the gate");
    b3_write(&path, &migrated);
    b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
    let loaded = super::load_settings_for_cli_from_path(&path);

    super::save_settings_to_path_preserving_project_paths(&loaded, &path)
        .expect("a Preserve save over a migrated file must succeed");
}

#[test]
fn the_mark_is_set_only_by_an_unreadable_sidecar() {
    crate::logging::test_install_logger();
    // Corrupt: the mark is set, every other setting survives, notice 1 is raised.
    {
        let _drain_guard = crate::logging::ERROR_SINK_TEST_DRAIN_LOCK.blocking_lock();
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        b3_seed_corrupt_sidecar(temp.path());
        let _ = crate::logging::error_sink().drain();

        let loaded = super::load_settings_from_path(&path);

        let sidecar = b3_sidecar_path(temp.path());
        assert_eq!(b3_marked(&loaded), Some(sidecar.clone()), "corrupt leg");
        assert_eq!(loaded.default_shell, "test-shell");
        assert_eq!(loaded.root_token.as_deref(), Some("base-token"));
        assert!(loaded.agents.is_empty());
        let notice = super::agents_unreadable_startup_notice(&sidecar);
        assert!(notice
            .ends_with("While this file is unreadable, changes to your sessions are not saved."));
        let captured = crate::logging::error_sink().drain();
        assert!(
            captured.iter().any(|e| e.message.contains(&notice)),
            "notice 1 was not captured: {:?}",
            captured.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
    // Absent: clear.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        let loaded = super::load_settings_from_path(&path);
        assert_eq!(b3_marked(&loaded), None, "absent leg");
    }
    // Parseable: clear.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
        let loaded = super::load_settings_from_path(&path);
        assert_eq!(b3_marked(&loaded), None, "parseable leg");
    }
}

#[test]
fn the_mark_survives_the_defaults_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    b3_write(
        &path,
        &serde_json::json!({ "defaultShell": 42, "rootToken": "base-token" }),
    );
    b3_seed_corrupt_sidecar(temp.path());

    let loaded = super::load_settings_from_path(&path);

    assert_ne!(
        loaded.root_token.as_deref(),
        Some("base-token"),
        "the settings file decoded, so the fallback did not run"
    );
    assert_eq!(
        b3_marked(&loaded),
        Some(b3_sidecar_path(temp.path())),
        "the defaults fallback lost the mark"
    );
    assert!(super::agents_layer_unreadable_for_dir(temp.path()));
}

#[test]
fn the_mark_is_never_serialized_and_cannot_be_forged() {
    // Outward: a save with the mark set writes the same bytes as one without it.
    for reconcile in [false, true] {
        let mut bytes = Vec::new();
        for marked in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = b3_settings_path(temp.path());
            b3_write(&path, &b3_migrated_fixture());
            b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
            let mut settings = super::load_settings_for_cli_from_path(&path);
            if marked {
                settings.agents_layer = super::AgentsLayerState::Unreadable {
                    path: b3_sidecar_path(temp.path()),
                    reason: "forged for the test".to_string(),
                };
            }
            if reconcile {
                super::save_settings_with_project_paths_to_path(&settings, &path).unwrap();
            } else {
                super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
            }
            let settings_bytes = std::fs::read(&path).unwrap();
            let sidecar_bytes = std::fs::read(b3_sidecar_path(temp.path())).unwrap();
            for raw in [&settings_bytes, &sidecar_bytes] {
                let text = String::from_utf8_lossy(raw);
                assert!(!text.contains("agentsLayer") && !text.contains("agents_layer"));
            }
            bytes.push((settings_bytes, sidecar_bytes));
        }
        assert_eq!(
            bytes[0], bytes[1],
            "reconcile={reconcile}: the mark changed the bytes"
        );
    }
    // Inward: a renderer-shaped object carrying the key decodes with the mark clear.
    let mut value = serde_json::to_value(super::AppSettings::default()).unwrap();
    let object = value.as_object_mut().unwrap();
    let forged = serde_json::json!({ "Unreadable": { "path": "C:/x", "reason": "forged" } });
    object.insert("agentsLayer".to_string(), forged.clone());
    object.insert("agents_layer".to_string(), forged);
    let decoded: super::AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.agents_layer, super::AgentsLayerState::Readable);
}

#[test]
fn both_spawn_builders_refuse_under_the_mark() {
    let temp = tempfile::tempdir().unwrap();
    let sidecar = b3_sidecar_path(temp.path());
    let mut settings = super::AppSettings {
        agents: vec![serde_json::from_value(b3_agent("alpha", "Alpha")).unwrap()],
        agents_layer: super::AgentsLayerState::Unreadable {
            path: sidecar.clone(),
            reason: "test".to_string(),
        },
        ..Default::default()
    };
    let cwd = temp.path().to_string_lossy().into_owned();
    let expected = super::agents_unreadable_session_error(&sidecar);

    // Leg one: build.
    let built = crate::commands::session::build_configured_agent_spawn_for_cwd(
        &settings, "alpha", &cwd, None,
    );
    assert_eq!(built.err().as_deref(), Some(expected.as_str()), "build leg");
    // Leg two: resolve.
    let resolved = crate::commands::session::resolve_configured_agent_spawn_for_cwd(
        &settings, "alpha", &cwd, None, false,
    );
    assert_eq!(
        resolved.err().as_deref(),
        Some(expected.as_str()),
        "resolve leg"
    );

    // Mark clear, unknown id: both answer Ok(None).
    settings.agents_layer = super::AgentsLayerState::Readable;
    assert!(matches!(
        crate::commands::session::build_configured_agent_spawn_for_cwd(
            &settings, "unknown", &cwd, None
        ),
        Ok(None)
    ));
    assert!(matches!(
        crate::commands::session::resolve_configured_agent_spawn_for_cwd(
            &settings, "unknown", &cwd, None, false
        ),
        Ok(None)
    ));
}

#[test]
fn the_cli_loaders_also_set_the_mark() {
    type Loader = fn(&std::path::Path) -> super::AppSettings;
    let loaders: [(&str, Loader); 2] = [
        ("load_settings_for_cli", |p| {
            super::load_settings_for_cli_from_path(p)
        }),
        ("load_settings_for_cli_strict", |p| {
            super::load_settings_for_cli_strict_from_path(Some(p)).unwrap()
        }),
    ];
    for (name, load) in loaders {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        b3_seed_corrupt_sidecar(temp.path());
        let key = path.parent().unwrap().to_path_buf();

        let loaded = load(&path);
        assert_eq!(
            b3_marked(&loaded),
            Some(b3_sidecar_path(temp.path())),
            "{name}"
        );
        assert!(
            super::agents_layer_unreadable_for_dir(&key),
            "{name}: key not inserted"
        );

        b3_seed_sidecar(temp.path(), "Sidecar A", "Sidecar P");
        let loaded = load(&path);
        assert_eq!(b3_marked(&loaded), None, "{name}");
        assert!(
            !super::agents_layer_unreadable_for_dir(&key),
            "{name}: key not removed"
        );
    }
}

/// #2716 (B3) E43, r12o: with no agents file on disk, only the export's lock
/// can stop its write. Held by this thread, the export on another thread
/// cannot acquire it, writes nothing and strips nothing; the three-agent file
/// written under the guard is what a later load returns.
#[test]
fn the_export_cannot_write_a_sidecar_while_the_lock_is_held() {
    let two_agents = |dir: &std::path::Path| {
        let path = b3_settings_path(dir);
        let mut legacy = b3_legacy_fixture("Alpha", "P");
        legacy["agents"] =
            serde_json::json!([b3_agent("alpha", "Alpha"), b3_agent("beta", "Beta")]);
        legacy["codingAgentProfiles"]["profileLabelsByAgent"]["beta"] =
            legacy["codingAgentProfiles"]["profileLabelsByAgent"]["alpha"].clone();
        b3_write(&path, &legacy);
        path
    };

    // Positive control: the same fixture with no lock held exports two agents.
    let control = tempfile::tempdir().unwrap();
    let control_path = two_agents(control.path());
    let mut settings = super::load_settings_for_cli_from_path(&control_path);
    assert!(super::export_agents_to_instance_file(
        &mut settings,
        &control_path
    ));
    let exported = b3_object(&b3_sidecar_path(control.path()));
    assert_eq!(exported["agents"].as_array().map(Vec::len), Some(2));

    // Under the lock.
    let temp = tempfile::tempdir().unwrap();
    let path = two_agents(temp.path());
    let seeded = std::fs::read(&path).unwrap();
    let guard = super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
    let export_path = path.clone();
    let exported = std::thread::spawn(move || {
        let mut settings = super::load_settings_for_cli_from_path(&export_path);
        super::export_agents_to_instance_file(&mut settings, &export_path)
    })
    .join()
    .unwrap();
    assert!(
        !b3_sidecar_path(temp.path()).exists(),
        "the export wrote the agents file while the lock was held"
    );
    assert!(!exported, "the export ran while the lock was held");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        seeded,
        "the legacy keys changed"
    );

    // Still under the guard: another writer's three agents.
    b3_write(
        &b3_sidecar_path(temp.path()),
        &serde_json::json!({
            "agents": [
                b3_agent("alpha", "Alpha"),
                b3_agent("beta", "Beta"),
                b3_agent("gamma", "Gamma"),
            ],
        }),
    );
    drop(guard);

    let loaded = super::load_settings_from_path(&path);
    assert_eq!(
        b3_labels(&loaded),
        vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()]
    );
}

mod independent_source_tests;

mod source_identity_tests;

mod source_inventory_projection_tests;

mod derived_agent_binding_projection_tests;

mod p02_configuration_schema_tests;

mod p02_physical_schema_tests;

mod p02_duplicate_row_tests;

mod e2992_p41_owner_tests;

mod e2432_profile_storage_tests;

mod e2432_p02_physical_file_tests;

mod e2432_p02_closed_owner_tests;
