use super::*;

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
                config.path().join(super::super::SETTINGS_FILE_NAME),
                serde_json::json!({"logLevel": level}).to_string(),
            )
            .unwrap();
        }
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "config::settings::tests::lock_debug_and_id_hash_schema::settings_lock_debug_child",
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
    let hooks = super::super::SettingsLockDebugTestHooks {
        gate_phase: "acquired",
        gate_once: std::sync::atomic::AtomicBool::new(matches!(
            mode.as_str(),
            "suppress" | "saturated"
        )),
        gated: gated_send,
        resume: std::sync::Mutex::new(resume_recv),
        processed: processed_send,
    };
    assert!(super::super::SETTINGS_LOCK_DEBUG_TEST_HOOKS
        .set(hooks)
        .is_ok());
    crate::logging::init_logger();
    let config =
        std::path::PathBuf::from(std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR").unwrap());
    let log = || std::fs::read_to_string(config.join("app.log")).unwrap();
    match mode.as_str() {
        "disabled" | "probe" => {
            let guard = super::super::SettingsFileLock::acquire(&path, std::time::Duration::ZERO)
                .unwrap_or_else(|_| panic!("probe lock unavailable"));
            drop(guard);
            assert!(super::super::SETTINGS_LOCK_DEBUG_TRANSPORT.get().is_none());
            assert!(processed.try_recv().is_err());
            assert!(!log().contains("settings-lock phase="));
            if mode == "probe" {
                settings_lock_debug_receipt(&serde_json::json!({"phase":"probe-ok"}));
            }
        }
        "waiter" => {
            let error = match super::super::SettingsFileLock::acquire(
                &path,
                std::time::Duration::from_secs(2),
            ) {
                Ok(_) => panic!("waiter unexpectedly acquired"),
                Err(error) => error,
            };
            assert!(matches!(
                error.cause,
                super::super::SettingsSaveCause::Semantic(
                    super::super::SettingsSaveReason::LockTimedOut
                )
            ));
            assert_eq!(
                error.legacy_outward.into_general_string(),
                super::super::SETTINGS_LOCK_UNAVAILABLE_CODE
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
        let guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("initial INFO acquisition"));
        drop(guard);
        assert!(super::super::SETTINGS_LOCK_DEBUG_TRANSPORT.get().is_none());
        assert!(processed.try_recv().is_err());
        assert!(!log().contains("settings-lock phase="));
        assert!(crate::logging::set_runtime_log_level("debug"));
    }
    if std::env::var_os("RUST_LOG").is_some() {
        assert!(!crate::logging::set_runtime_log_level("error"));
    }
    let guard = super::super::empty_project_transaction(path, "capture_project_settings")
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
        let sender = super::super::SETTINGS_LOCK_DEBUG_TRANSPORT
            .get()
            .unwrap()
            .as_ref()
            .unwrap();
        assert!(crate::logging::set_runtime_log_level("info"));
        let guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("INFO re-acquisition"));
        assert!(guard.debug.is_none());
        drop(guard);
        assert!(crate::logging::set_runtime_log_level("debug"));
        let guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("DEBUG re-acquisition"));
        // The same OnceLock sender/worker serves the next acquisition.
        assert!(std::ptr::eq(
            sender,
            super::super::SETTINGS_LOCK_DEBUG_TRANSPORT
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
    let guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
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
    let guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
        .unwrap_or_else(|_| panic!("saturation owner"));
    settings_lock_debug_processed(processed, "acquire-start");
    gated
        .recv_timeout(SETTINGS_LOCK_DEBUG_HARNESS_TIMEOUT)
        .unwrap();
    let debug = guard.debug.as_ref().unwrap();
    for _ in 0..super::super::SETTINGS_LOCK_DEBUG_CAPACITY + 10 {
        debug.event("acquire-start", None, None);
    }
    assert_eq!(
        super::super::SETTINGS_LOCK_DEBUG_LOST.load(super::super::Ordering::Relaxed),
        10
    );
    drop(guard);
    assert_eq!(
        super::super::SETTINGS_LOCK_DEBUG_LOST.load(super::super::Ordering::Relaxed),
        11
    );
    settings_lock_debug_receipt(&serde_json::json!({"phase":"closed-sink-blocked"}));
    settings_lock_debug_wait_command("resume");
    resume.send(()).unwrap();
    let acquired = settings_lock_debug_processed(processed, "acquired");
    assert_eq!(acquired["lost_events"], 11);
    for _ in 0..super::super::SETTINGS_LOCK_DEBUG_CAPACITY {
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
    let error = match super::super::SettingsFileLock::acquire(
        &bad_parent.join("settings.json"),
        std::time::Duration::ZERO,
    ) {
        Ok(_) => panic!("invalid parent accepted"),
        Err(error) => error,
    };
    assert_eq!(
        error.legacy_outward.into_general_string(),
        super::super::SETTINGS_LOCK_UNAVAILABLE_CODE
    );
    settings_lock_debug_processed(processed, "acquire-start");
    let failure = settings_lock_debug_processed(processed, "acquire-error");
    assert_eq!(failure["reason"], "io_error");
    assert!(failure["wait_ms"].is_null());
    // An invalid sidecar is rejected with no acquired event.
    let directory = path.with_file_name("invalid-sidecar");
    std::fs::create_dir_all(directory.join(super::super::SETTINGS_LOCK_FILE_NAME)).unwrap();
    assert!(super::super::SettingsFileLock::acquire(
        &directory.join("settings.json"),
        std::time::Duration::ZERO
    )
    .is_err());
    settings_lock_debug_processed(processed, "acquire-start");
    settings_lock_debug_processed(processed, "acquire-error");
    std::fs::remove_dir_all(&directory).unwrap();
    // Deadline overflow happens after opening. Windows rename additionally
    // proves the no-share-delete handle was closed before the failure receipt.
    assert!(super::super::SettingsFileLock::acquire(path, std::time::Duration::MAX).is_err());
    settings_lock_debug_processed(processed, "acquire-start");
    let failure = settings_lock_debug_processed(processed, "acquire-error");
    assert_eq!(failure["reason"], "lock_deadline_overflow");
    let sidecar = path
        .parent()
        .unwrap()
        .join(super::super::SETTINGS_LOCK_FILE_NAME);
    let renamed = sidecar.with_extension("test-closed");
    std::fs::rename(&sidecar, &renamed).unwrap();
    std::fs::rename(&renamed, &sidecar).unwrap();
    // Early return and unwinding both release before an immediate new owner.
    let early = || -> Result<(), ()> {
        let _guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("early owner"));
        Err(())
    };
    assert!(early().is_err());
    settings_lock_debug_processed(processed, "acquire-start");
    let acquired = settings_lock_debug_processed(processed, "acquired");
    let release = settings_lock_debug_processed(processed, "release");
    settings_lock_debug_assert_pair(&acquired, &release);
    assert!(std::panic::catch_unwind(|| {
        let _guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
            .unwrap_or_else(|_| panic!("unwind owner"));
        panic!("test unwind");
    })
    .is_err());
    settings_lock_debug_processed(processed, "acquire-start");
    let acquired = settings_lock_debug_processed(processed, "acquired");
    let release = settings_lock_debug_processed(processed, "release");
    settings_lock_debug_assert_pair(&acquired, &release);
    let guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
        .unwrap_or_else(|_| panic!("unlock-error owner"));
    settings_lock_debug_processed(processed, "acquire-start");
    let acquired = settings_lock_debug_processed(processed, "acquired");
    super::super::SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR.with(|flag| flag.set(true));
    drop(guard);
    let release = settings_lock_debug_processed(processed, "release");
    settings_lock_debug_assert_pair(&acquired, &release);
    assert_eq!(release["reason"], "unlock_error");
    assert_eq!(release["raw_os_error"], 5);
    let guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
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
        let transaction = super::super::empty_project_transaction(path, operation)
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
    super::super::SETTINGS_LOCK_DEBUG_ATTEMPT.store(u64::MAX, super::super::Ordering::Relaxed);
    let guard = super::super::SettingsFileLock::acquire(path, std::time::Duration::ZERO)
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
            super::super::settings_lock_debug_path(
                &fixture.path().join(super::super::SETTINGS_LOCK_FILE_NAME)
            )
            .0
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
        super::super::settings_lock_debug_transport(&slot, false, |_| panic!("disabled spawn"))
            .is_none()
    );
    assert!(slot.get().is_none());
    assert!(
        super::super::settings_lock_debug_transport(&slot, true, |_| Err(std::io::Error::other(
            "injected spawn failure"
        )))
        .is_none()
    );
    assert!(matches!(slot.get(), Some(None)));
    assert!(
        super::super::settings_lock_debug_transport(&slot, true, |_| panic!("spawn retry"))
            .is_none()
    );
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    drop(receiver);
    let context = std::sync::Arc::new(super::super::SettingsLockDebugContext {
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
    let debug = super::super::SettingsLockDebugAttempt {
        context,
        sender,
        started: std::time::Instant::now(),
        retry_started: None,
        acquired_at: None,
    };
    let before = super::super::SETTINGS_LOCK_DEBUG_LOST.load(super::super::Ordering::Relaxed);
    debug.event("acquire-start", None, None);
    assert!(super::super::SETTINGS_LOCK_DEBUG_LOST.load(super::super::Ordering::Relaxed) > before);
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("settings.json");
    let mut debug = Some(debug);
    let file = super::super::SettingsFileLock::acquire_file(
        &path,
        super::super::SETTINGS_LOCK_FILE_NAME,
        std::time::Duration::ZERO,
        &mut debug,
    )
    .unwrap_or_else(|_| panic!("disconnected diagnostics affected acquisition"));
    drop(super::super::SettingsFileLock {
        file: Some(file),
        debug,
    });
    assert!(super::super::SettingsFileLock::acquire(&path, std::time::Duration::ZERO).is_ok());
}

#[test]
fn settings_lock_debug_paths_are_escaped_and_bounded() {
    let (escaped, truncated) =
        super::super::settings_lock_debug_path(std::path::Path::new("hello\n\r\t\"\\world"));
    assert!(!truncated);
    assert!(!escaped.contains('\n'));
    assert!(escaped.contains("\\n"));
    assert_eq!(
        serde_json::from_str::<String>(&escaped).unwrap(),
        "hello\n\r\t\"\\world"
    );
    let (escaped, truncated) =
        super::super::settings_lock_debug_path(std::path::Path::new(&"🦀".repeat(4096)));
    assert!(truncated);
    assert!(escaped.len() <= super::super::SETTINGS_LOCK_DEBUG_PATH_BYTES);
    assert!(serde_json::from_str::<String>(&escaped).is_ok());
}

#[test]
fn id_hash_debug_exclusions_preserve_real_content_and_clear_orphans() {
    let agent: super::super::AgentConfig = serde_json::from_value(serde_json::json!({
        "id":"alpha", "label":"Alpha", "baseCommand":"tool", "color":"red"
    }))
    .unwrap();
    let mut supplied = agent.clone();
    supplied.id_hash_debug = Some(vec!["forged".into()]);
    assert!(super::super::known_agent_field("id_hash_debug"));
    assert!(!super::super::known_agent_field("idHashDebug"));
    assert_eq!(
        super::super::comparable_agent(&agent),
        super::super::comparable_agent(&supplied)
    );
    supplied.command.push_str(" --real");
    assert_ne!(
        super::super::comparable_agent(&agent),
        super::super::comparable_agent(&supplied)
    );

    let mut profiles = super::super::CodingAgentProfilesConfig::default();
    let mut cell = super::super::empty_profile_cell();
    cell.id_hash = Some("stale".into());
    cell.id_hash_debug = Some(vec!["stale".into()]);
    profiles
        .profiles_by_agent
        .insert("orphan".into(), BTreeMap::from([("A".into(), cell)]));
    assert!(super::super::is_profile_gap(&profiles, "orphan"));
    profiles
        .profiles_by_agent
        .get_mut("orphan")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .notes = "real".into();
    assert!(!super::super::is_profile_gap(&profiles, "orphan"));
    super::super::refresh_pair_id_hashes(&[agent], &mut profiles);
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
                serde_json::from_value::<super::super::AgentConfig>(raw).map(|value| {
                    (
                        value.id_hash_debug.clone(),
                        serde_json::to_value(value).unwrap(),
                    )
                })
            } else {
                serde_json::from_value::<super::super::ProfileCellConfig>(raw).map(|value| {
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
    let expected: super::super::AppSettings = serde_json::from_value(baseline.clone()).unwrap();
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
        let decoded: super::super::AppSettings = serde_json::from_value(supplied).unwrap();
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
        serde_json::json!([{"id": "a", "label": "A", "command": 42, "color": "red", "id_has\
            h": false}]),
        serde_json::json!([{"id": "a", "label": "A", "command": "tool", "color": "red", "env\
            s": {}, "id_hash": false}]),
    ] {
        let mut malformed = baseline.clone();
        malformed["agents"] = malformed_agents;
        assert!(serde_json::from_value::<super::super::AppSettings>(malformed).is_err());
    }
}

#[test]
fn id_hash_schema_serializes_literal_key_roundtrips_and_omits_none() {
    let mut agent: super::super::AgentConfig = serde_json::from_value(serde_json::json!({
        "id": "custom-tool", "label": "Custom tool", "command": "tool", "color": "red"
    }))
    .unwrap();
    agent.id_hash = Some("cid1:example".to_string());
    let encoded = serde_json::to_value(&agent).unwrap();
    assert_eq!(encoded["id_hash"], "cid1:example");
    assert!(encoded.get("idHash").is_none());
    let decoded: super::super::AgentConfig = serde_json::from_value(encoded.clone()).unwrap();
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
    let decoded: super::super::AgentConfig = serde_json::from_value(baseline.clone()).unwrap();
    assert_eq!(decoded.id_hash, None);
    assert!(serde_json::to_value(decoded)
        .unwrap()
        .get("idHash")
        .is_none());
    for raw in ["", "not-a-hash", "cid1:forged", " arbitrary Ω "] {
        let mut supplied = baseline.clone();
        supplied["id_hash"] = serde_json::json!(raw);
        let decoded: super::super::AgentConfig = serde_json::from_value(supplied).unwrap();
        assert_eq!(decoded.id, "custom-tool");
        assert_eq!(decoded.id_hash.as_deref(), Some(raw));
        let encoded = serde_json::to_value(decoded).unwrap();
        assert_eq!(encoded["id_hash"], raw);
        assert!(encoded.get("idHash").is_none());
    }
}
