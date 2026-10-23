use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScannerShutdownPhase {
    BeforeService,
    BlockingService,
    BeforeFinalizer,
    FinalizerOwned,
    CancellationRace,
    ReplacementCleanup,
}

impl ScannerShutdownPhase {
    fn label(self) -> &'static str {
        match self {
            Self::BeforeService => "before-service",
            Self::BlockingService => "blocking-service",
            Self::BeforeFinalizer => "before-finalizer",
            Self::FinalizerOwned => "finalizer-owned",
            Self::CancellationRace => "cancellation-race",
            Self::ReplacementCleanup => "replacement-cleanup",
        }
    }

    fn abortable(self) -> bool {
        matches!(self, Self::BeforeService | Self::ReplacementCleanup)
    }

    fn parse(value: &str) -> Option<Self> {
        [
            Self::BeforeService,
            Self::BlockingService,
            Self::BeforeFinalizer,
            Self::FinalizerOwned,
            Self::CancellationRace,
            Self::ReplacementCleanup,
        ]
        .into_iter()
        .find(|phase| phase.label() == value)
    }
}

fn scanner_processing_path(request_directory: &Path, request_id: &str) -> PathBuf {
    let matches = std::fs::read_dir(request_directory)
        .expect("scanner request directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!(".{request_id}."))
                        && name.ends_with(".terminal-snapshot-processing")
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "one scanner processing owner");
    matches.into_iter().next().expect("scanner processing path")
}

async fn run_composed_scanner_shutdown_phase(temporary_root: &Path, phase: ScannerShutdownPhase) {
    let temporary = tempfile::Builder::new()
        .prefix(&format!("scanner-shutdown-{}-", phase.label()))
        .tempdir_in(temporary_root)
        .expect("scanner shutdown temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("scanner shutdown config directory");
    let _env = ConfigEnvGuard::set(&config);
    let fixture = AcceptanceFixture::new(temporary).await;
    let target = format!("{PROJECT}:{WORKGROUP}/member-live");
    let requester = format!("{PROJECT}:{WORKGROUP}/coordinator");
    let nonce = match phase {
        ScannerShutdownPhase::BeforeService => "1".repeat(64),
        ScannerShutdownPhase::BlockingService => "2".repeat(64),
        ScannerShutdownPhase::BeforeFinalizer => "3".repeat(64),
        ScannerShutdownPhase::FinalizerOwned => "4".repeat(64),
        ScannerShutdownPhase::CancellationRace => "5".repeat(64),
        ScannerShutdownPhase::ReplacementCleanup => "6".repeat(64),
    };
    let request = retag_host_request(
        host_request(&fixture.host_coordinator, &requester, &target),
        &nonce,
    );
    let root = &fixture.paths.coordinator;
    let local = root.join(crate::config::agent_local_dir_name());
    let request_directory = local.join("outbox").join("terminal-snapshot-requests");
    let response_directory = local.join("terminal-snapshot-responses");
    let request_bytes = serde_json::to_vec(&request).expect("scanner shutdown request JSON");
    let (request_path, response_path) =
        write_host_request_bytes(root, &request.request_id, &request_bytes);
    let original_identity = crate::path_identity::verify_regular_file(&request_path)
        .expect("scanner shutdown request identity");

    let mut scanner = crate::phone::terminal_snapshot::SnapshotMailboxScanner::default();
    let owner = scanner.shutdown_owner_for_test();
    let start_control = phase
        .abortable()
        .then(crate::phone::terminal_snapshot::SnapshotScannerTaskStartControl::new);
    if let Some(control) = &start_control {
        owner.install_next_task_start_control(Arc::clone(control));
    }
    let blocking_control = (phase == ScannerShutdownPhase::BlockingService)
        .then(|| TerminalSnapshotBlockingControl::new(None));
    if let Some(control) = &blocking_control {
        fixture
            .snapshot_state
            .install_blocking_control(TerminalSnapshotBlockingStage::Capture, Arc::clone(control));
    }
    let cancellation_barrier = match phase {
        ScannerShutdownPhase::BeforeFinalizer => Some(install_host_cancellation_barrier(
            &fixture.snapshot_state,
            TerminalSnapshotHostCancellationStage::ResponseBytesReady,
        )),
        ScannerShutdownPhase::CancellationRace => Some(install_host_cancellation_barrier(
            &fixture.snapshot_state,
            TerminalSnapshotHostCancellationStage::Processing,
        )),
        _ => None,
    };
    let finalizer_control = (phase == ScannerShutdownPhase::FinalizerOwned).then(|| {
        TerminalSnapshotHostFinalizerControl::new(
            TerminalSnapshotHostFinalizerStage::RevalidationEntry,
            None,
        )
    });
    if let Some(control) = &finalizer_control {
        fixture
            .snapshot_state
            .install_next_host_finalizer_control(Arc::clone(control));
    }

    scanner.begin_cycle();
    scanner.scan_root(
        fixture.app.as_ref().expect("fixture app is alive").handle(),
        root,
    );
    scanner.finish_cycle();
    if let Some(control) = &start_control {
        control.wait_until_entered();
    }
    if let Some(control) = &blocking_control {
        control.wait_until_entered();
    }
    if let Some((entered, _)) = &cancellation_barrier {
        wait_for_host_cancellation_barrier(entered);
    }
    if let Some(control) = &finalizer_control {
        control.wait_until_entered();
        assert!(control.retained_response_bytes() > 0);
    }

    let processing = matches!(
        phase,
        ScannerShutdownPhase::CancellationRace | ScannerShutdownPhase::ReplacementCleanup
    )
    .then(|| scanner_processing_path(&request_directory, &request.request_id));
    let mut replacement = None;
    if phase == ScannerShutdownPhase::CancellationRace {
        assert!(!crate::cli::terminal_snapshot::cancel_request_for_test(
            &request_path,
            &request_directory,
            Uuid::parse_str(&request.request_id).expect("scanner cancellation request UUID"),
            &request.nonce,
            &original_identity,
        ));
        assert!(!host_cancellation_marker(&request_directory, &request).exists());
    }
    if phase == ScannerShutdownPhase::ReplacementCleanup {
        let processing = processing.expect("replacement processing path");
        let displaced = request_directory.join("scanner-owned-displaced");
        std::fs::rename(&processing, &displaced).expect("displace scanner-owned processing file");
        std::fs::write(&processing, b"foreign processing replacement")
            .expect("write processing replacement");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&processing, std::fs::Permissions::from_mode(0o600))
                .expect("private processing replacement");
        }
        replacement = Some((processing.clone(), displaced));
    }

    owner.seal();
    assert!(owner.is_sealed());
    fixture.snapshot_state.shutdown.trigger();
    if let Some(control) = &blocking_control {
        control.expire_deadline();
    }
    let first_budget = if phase.abortable() {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(75)
    };
    let first = owner
        .seal_and_drain_until(tokio::time::Instant::now() + first_budget)
        .await;
    if phase.abortable() {
        assert!(first.terminal, "abortable scanner owner must be joined");
        assert_eq!(first.aborted, 1);
        assert_eq!(first.joined, 1);
        assert!(first.retained.is_empty());
        assert_eq!(owner.task_count(), 0);
        assert_api_lifecycle_idle(&fixture.snapshot_state);
    } else {
        assert!(!first.terminal, "started scanner owner must be retained");
        assert_eq!(first.aborted, 0);
        assert_eq!(first.joined, 0);
        assert_eq!(first.retained.len(), 1);
        assert_eq!(owner.task_count(), 1);
        // A retained scanner is still reported in the shutdown diagnostics
        // below, but it must not suppress session persistence: its only
        // SessionManager access is a read-lock clone, so it cannot leave session
        // state inconsistent.
        assert!(
            crate::shutdown_persistence_allowed(true, true),
            "a retained snapshot scanner must not suppress session persistence"
        );
        let diagnostics = crate::combined_shutdown_retained_diagnostics_with_scanner(
            first.retained.clone(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("owner=terminalSnapshotScanner"));
        assert!(!diagnostics[0].contains(&request.request_id));
        assert!(!diagnostics[0].contains(SCREEN_SENTINEL));
        assert!(!diagnostics[0].contains('/'));
        assert!(!diagnostics[0].contains('\\'));
        if phase == ScannerShutdownPhase::FinalizerOwned {
            assert!(first.retained[0].contains("terminal-snapshot-finalizer"));
        } else {
            assert!(first.retained[0].contains("terminal-snapshot-service"));
        }
        assert!(fixture
            .app
            .as_ref()
            .expect("fixture app is alive")
            .try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .is_some());
        assert!(fixture
            .app
            .as_ref()
            .expect("fixture app is alive")
            .try_state::<Arc<std::sync::Mutex<PtyManager>>>()
            .is_some());
        match phase {
            ScannerShutdownPhase::CancellationRace => assert_eq!(
                api_lifecycle_counts(&fixture.snapshot_state),
                ApiLifecycleCounts {
                    ingress_available: SNAPSHOT_INGRESS_LIMIT - 1,
                    requester_in_flight: 0,
                    target_in_flight: 0,
                    global_in_flight: 0,
                }
            ),
            _ => assert_api_lifecycle_active(api_lifecycle_counts(&fixture.snapshot_state), true),
        }
    }

    if let Some(control) = &start_control {
        control.release();
    }
    if let Some(control) = &blocking_control {
        control.release();
        control.wait_until_completed();
    }
    if let Some((_, release)) = cancellation_barrier {
        release
            .send(())
            .expect("release scanner cancellation barrier");
    }
    if let Some(control) = &finalizer_control {
        control.release();
    }
    let terminal = owner
        .seal_and_drain_until(tokio::time::Instant::now() + Duration::from_secs(10))
        .await;
    assert!(terminal.terminal);
    assert_eq!(owner.task_count(), 0);
    assert!(crate::shutdown_persistence_allowed(true, true));
    assert_api_lifecycle_idle(&fixture.snapshot_state);

    let mut response_reason = None;
    if phase.abortable() {
        assert!(!response_path.exists());
    } else {
        let bytes = std::fs::read(&response_path).expect("shutdown metadata response");
        assert!(!contains_raw(&bytes, SCREEN_SENTINEL.as_bytes()));
        let decoded = decode_host_response(
            &bytes,
            &request.request_id,
            &request.confirmation_tag,
            &target,
            TerminalSnapshotFormat::Json,
        )
        .expect("strict shutdown response");
        assert!(decoded.result.is_none());
        response_reason = decoded.error;
        assert!(response_reason.is_some());
        #[cfg(not(unix))]
        let identity = crate::path_identity::verify_regular_file(&response_path)
            .expect("shutdown response identity");
        std::fs::remove_file(&response_path).expect("consume shutdown response");
        #[cfg(not(unix))]
        fixture.snapshot_state.untrack_artifact(&identity);
    }

    if let Some((replacement_path, displaced)) = replacement {
        assert_eq!(
            std::fs::read(&replacement_path).expect("read processing replacement"),
            b"foreign processing replacement"
        );
        assert!(displaced.exists());
        std::fs::remove_file(replacement_path).expect("remove processing replacement");
        std::fs::remove_file(displaced).expect("remove displaced scanner-owned file");
        #[cfg(not(unix))]
        fixture.snapshot_state.untrack_artifact(&original_identity);
    }
    assert!(!host_cancellation_marker(&request_directory, &request).exists());
    fixture.snapshot_state.sweep_artifacts_for_test(false);
    assert_eq!(fixture.snapshot_state.test_artifact_counts(), (0, 0, 0));
    assert_eq!(
        std::fs::read_dir(&request_directory)
            .expect("scanner request residue inventory")
            .count(),
        0
    );
    assert_eq!(
        std::fs::read_dir(&response_directory)
            .expect("scanner response residue inventory")
            .count(),
        0
    );

    let canaries = vec![
        SCREEN_SENTINEL.to_string(),
        OSC_TITLE_SENTINEL.to_string(),
        OSC_HYPERLINK_SENTINEL.to_string(),
        OSC_CLIPBOARD_SENTINEL.to_string(),
        fixture.host_coordinator.token.to_string(),
        request.nonce.clone(),
        request.confirmation_tag.clone(),
    ];
    let rows = snapshot_audit_rows(&config, &canaries);
    assert_eq!(rows.len(), 1, "one scanner shutdown audit event");
    let row = &rows[0];
    assert_eq!(
        row.get("requestId").and_then(serde_json::Value::as_str),
        Some(request.request_id.as_str())
    );
    assert_eq!(
        row.get("status").and_then(serde_json::Value::as_str),
        Some("failed")
    );
    if let Some(reason) = response_reason {
        assert_eq!(
            row.get("reasonCode").and_then(serde_json::Value::as_str),
            Some(reason.as_str())
        );
    }
    assert_eq!(fixture.local_backend.mutations(), 0);
}

#[test]
fn scanner_app_shutdown_owns_composed_host_phases() {
    let canaries = [
        SCREEN_SENTINEL,
        OSC_TITLE_SENTINEL,
        OSC_HYPERLINK_SENTINEL,
        OSC_CLIPBOARD_SENTINEL,
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let child_phase = std::env::var(SCANNER_SHUTDOWN_CHILD_ENV)
        .ok()
        .and_then(|value| ScannerShutdownPhase::parse(&value));
    if child_phase.is_none() {
        for phase in [
            ScannerShutdownPhase::BeforeService,
            ScannerShutdownPhase::BlockingService,
            ScannerShutdownPhase::BeforeFinalizer,
            ScannerShutdownPhase::FinalizerOwned,
            ScannerShutdownPhase::CancellationRace,
            ScannerShutdownPhase::ReplacementCleanup,
        ] {
            let output = std::process::Command::new(
                std::env::current_exe().expect("scanner shutdown test executable"),
            )
            .args([
                "--exact",
                SCANNER_SHUTDOWN_TEST_NAME,
                "--test-threads=1",
                "--nocapture",
            ])
            .env(SCANNER_SHUTDOWN_CHILD_ENV, phase.label())
            .output()
            .expect("spawn isolated scanner shutdown test");
            assert_canaries_absent_except(
                &output.stdout,
                &canaries,
                &[],
                "scanner shutdown child stdout",
            );
            assert_canaries_absent_except(
                &output.stderr,
                &canaries,
                &[],
                "scanner shutdown child stderr",
            );
            if !output.status.success() {
                let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
                for canary in &canaries {
                    stdout = stdout.replace(canary, "<redacted-canary>");
                    stderr = stderr.replace(canary, "<redacted-canary>");
                }
                panic!(
                    "isolated scanner shutdown phase={} failed; stdout={stdout:?}; \
                        stderr={stderr:?}",
                    phase.label()
                );
            }
        }
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("scanner shutdown current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("scanner shutdown temporary root");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("scanner shutdown runtime");
    runtime.block_on(run_composed_scanner_shutdown_phase(
        &temporary_root,
        child_phase.expect("validated scanner shutdown child phase"),
    ));
}
