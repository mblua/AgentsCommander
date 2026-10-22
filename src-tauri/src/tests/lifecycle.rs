use super::*;

// Stage E (#1064) shutdown-decision conformance (plan section 10.4 items
// 14/15/34). Stage E extends only lib.rs `#[cfg(test)]` coverage; the exit
// wiring order itself is exercised by the selection/alert/scraper shutdown
// tests. Persistence is allowed only when BOTH the selection tracker and the
// container cleanup are terminal, and retained-owner diagnostics from both
// shutdown halves are merged.
#[test]
fn shutdown_persistence_is_allowed_only_when_selection_and_container_are_terminal() {
    assert!(super::shutdown_persistence_allowed(true, true));
    assert!(!super::shutdown_persistence_allowed(false, true));
    assert!(!super::shutdown_persistence_allowed(true, false));
    assert!(!super::shutdown_persistence_allowed(false, false));
}

#[test]
fn combined_shutdown_diagnostics_merge_selection_and_container_owners() {
    assert!(
        super::combined_shutdown_retained_diagnostics(Vec::new(), Vec::new()).is_empty(),
        "no retained owners yields no diagnostics"
    );
    let combined = super::combined_shutdown_retained_diagnostics(
        vec!["reason=blocking-seed-transaction-await state=retained".to_string()],
        vec!["reason=container-stop state=retained".to_string()],
    );
    assert_eq!(
        combined.len(),
        2,
        "each retained shutdown owner is represented once, got {combined:?}"
    );
    assert!(
        combined.iter().all(|entry| !entry.is_empty()),
        "retained diagnostics are non-empty"
    );
}

#[test]
fn scanner_retained_owner_is_merged_without_request_content_or_paths() {
    let combined = super::combined_shutdown_retained_diagnostics_with_scanner(
        vec!["reason=terminal-snapshot-finalizer state=retained".to_string()],
        vec!["reason=selection-worker state=retained".to_string()],
        vec!["reason=container-stop state=retained".to_string()],
    );
    assert_eq!(combined.len(), 3);
    assert!(combined
        .iter()
        .any(|entry| entry.contains("owner=terminalSnapshotScanner")));
    assert!(combined.iter().all(|entry| !entry.contains('\\')));
    assert!(combined.iter().all(|entry| !entry.contains('/')));
}

async fn p51_open_owner(settings: AppSettings) -> (tempfile::TempDir, SettingsState) {
    tokio::task::spawn_blocking(move || {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let owner =
            crate::config::settings::SettingsOwner::open_active(&path, Duration::from_secs(5))
                .unwrap();
        (dir, owner)
    })
    .await
    .unwrap()
}

async fn p51_invalidate_owner(owner: &SettingsState) {
    use crate::config::settings::{OperationOutcome, SettingsAvailability, SettingsPublisher};
    let report = owner
        .with_operation("p51-invalid", |_, guard| {
            guard.publish(SettingsPublisher::SettingsSave, |path| {
                std::fs::write(path, b"{").map_err(|error| error.to_string())?;
                Err::<(), String>("ReDecode".into())
            })
        })
        .await
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
    assert_eq!(report.availability, SettingsAvailability::Invalid);
    assert_eq!(owner.availability(), SettingsAvailability::Invalid);
    assert!(owner.current_read().await.is_err());
}

async fn p51_invalid_owner(settings: AppSettings) -> (tempfile::TempDir, SettingsState) {
    let (dir, owner) = p51_open_owner(settings).await;
    p51_invalidate_owner(&owner).await;
    (dir, owner)
}

async fn p51_close_owner(owner: SettingsState) {
    tokio::task::spawn_blocking(move || {
        owner.drain_blocking(Duration::from_secs(5));
        assert_eq!(
            owner.availability(),
            crate::config::settings::SettingsAvailability::Closed
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn e2992_p51_adapters_owner_denied() {
    use super::{QuotaSourceProvider, QuotaSources, WatcherPatternSource, WatcherPatterns};
    let (dir, denied) = p51_invalid_owner(settings_with_context_regex("context (.*)")).await;
    let scraper = ScraperPatterns {
        settings: Arc::clone(&denied),
    };
    let quota = QuotaSources {
        settings: Arc::clone(&denied),
    };
    let watchers = WatcherPatterns {
        settings: Arc::clone(&denied),
        log: Default::default(),
    };
    for _ in 0..2 {
        assert!(scraper.patterns().await.is_none());
        assert!(quota.sources().await.is_none());
        assert!(watchers.resolve().await.is_none());
    }
    assert_eq!(
        std::fs::read(
            dir.path()
                .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME)
        )
        .unwrap(),
        b"{"
    );
    for settings in [
        AppSettings {
            agents: Vec::new(),
            ..AppSettings::default()
        },
        settings_with_context_regex("context (.*)"),
    ] {
        let populated = !settings.agents.is_empty();
        let owner = crate::config::settings::new_settings_state(settings);
        assert_eq!(
            ScraperPatterns {
                settings: Arc::clone(&owner)
            }
            .patterns()
            .await
            .unwrap()
            .len(),
            usize::from(populated)
        );
        assert!(QuotaSources {
            settings: Arc::clone(&owner)
        }
        .sources()
        .await
        .unwrap()
        .is_empty());
        assert!(WatcherPatterns {
            settings: owner,
            log: Default::default()
        }
        .resolve()
        .await
        .unwrap()
        .is_empty());
    }
    let mut settings = settings_with_context_regex("context (.*)");
    settings.quota_sources.insert(
        "codex".into(),
        crate::config::settings::QuotaSourceEntry::Valid(
            crate::config::settings::QuotaSourceConfig::ScreenRegex {
                pattern: "Weekly (.*)".into(),
                enabled: true,
            },
        ),
    );
    settings.watchers.insert(
        "watch".into(),
        serde_json::from_value(serde_json::json!({
            "enabled": true, "mode": "state", "pattern": "ready"
        }))
        .unwrap(),
    );
    let owner = crate::config::settings::new_settings_state(settings);
    let quota = QuotaSources {
        settings: Arc::clone(&owner),
    }
    .sources()
    .await
    .unwrap();
    assert_eq!(
        quota.get("codex").unwrap().spec,
        crate::pty::agent_quota::source::SourceSpec::ScreenRegex {
            pattern: "Weekly (.*)".into()
        }
    );
    let watchers = WatcherPatterns {
        settings: owner,
        log: Default::default(),
    }
    .resolve()
    .await
    .unwrap();
    assert_eq!(watchers.get("codex").unwrap().running[0].pattern, "ready");
    p51_close_owner(denied).await;
}

#[tokio::test]
async fn e2992_p51_invalid_no_co_managed_budget() {
    use crate::commands::session::co_managed_tests::{configure_room, room_fixture};
    use crate::commands::session::reader_demand_tests::harness;
    use crate::commands::telegram::{holds_room_reader_demand, room_demand_epoch};
    use crate::telegram::manager::TelegramBridgeState;
    use tauri::Manager;

    let fixture = make_co_managed_fixture();
    let (endpoint, hits) = spawn_jev_listener(vec![("to-user", 0.9)]).await;
    let (dir, denied) = p51_invalid_owner(AppSettings {
        jev_endpoint: endpoint.clone(),
        jev_api_key: "test-key".to_string(),
        ..AppSettings::default()
    })
    .await;
    // Deliberately omit network and manager state: denied settings must return
    // before touching either service, the detector, or any commit path.
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&denied))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let (handle, _rx) = CoManagedSupervisorHandle::new();
    let id = uuid::Uuid::new_v4();
    let record = co_managed_candidate(&id.to_string(), "leak AKIAIOSFODNN7EXAMPLE");
    let slot = CaptureSlot::new();
    slot.offer(Arc::clone(&record), 0);
    let seq = slot.snapshot().seq;
    let budget = crate::capture::state::budget(&fixture.room_root);
    for _ in 0..2 {
        let outcome = super::co_managed_cycle(
            app.handle(),
            &handle,
            id,
            &fixture.room_root,
            &slot,
            seq,
            &record,
        )
        .await;
        assert!(
            matches!(
                outcome,
                super::CoManagedOutcome::Done(reason) if reason == "settings_unavailable"
            )
        );
        assert!(!super::release_room_reader_demands_when_globally_off(app.handle()).await);
    }
    assert_eq!(slot.snapshot().seq, seq);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(crate::capture::state::budget(&fixture.room_root), budget);
    assert!(messaging_files(&fixture.room_root).is_empty());
    assert!(queue_files(&fixture.room_root).is_empty());
    assert_eq!(
        std::fs::read(
            dir.path()
                .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME)
        )
        .unwrap(),
        b"{"
    );
    // A held demand makes the unavailable/global-off distinction observable.
    let demand_fixture = room_fixture();
    configure_room(demand_fixture.room_path(), true);
    let demand_harness = harness(&demand_fixture);
    let demand_id = demand_harness
        .session_in(demand_fixture.coordinator_path())
        .await;
    super::reader_demand_pass(demand_harness.app.handle()).await;
    assert!(holds_room_reader_demand(demand_harness.app.handle(), demand_id).await);
    let epoch = room_demand_epoch(demand_harness.app.handle(), demand_id).await;
    let demand_app = tauri::test::mock_builder()
        .manage(Arc::clone(&denied))
        .manage(Arc::clone(
            demand_harness
                .app
                .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                .inner(),
        ))
        .manage(Arc::clone(
            demand_harness.app.state::<TelegramBridgeState>().inner(),
        ))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    for _ in 0..2 {
        assert!(!super::release_room_reader_demands_when_globally_off(demand_app.handle()).await);
        assert!(holds_room_reader_demand(demand_app.handle(), demand_id).await);
        assert_eq!(
            room_demand_epoch(demand_app.handle(), demand_id).await,
            epoch
        );
    }
    // Positive control: the same session and bridge really release when Ready/off.
    demand_harness
        .set_global_co_managed(false, "test-key")
        .await;
    assert!(
        super::release_room_reader_demands_when_globally_off(demand_harness.app.handle()).await
    );
    assert!(!holds_room_reader_demand(demand_harness.app.handle(), demand_id).await);
    assert_ne!(
        room_demand_epoch(demand_harness.app.handle(), demand_id).await,
        epoch
    );
    crate::commands::session::reader_demand_tests::assert_no_room_reader_state_for(
        &demand_harness,
        demand_id,
    )
    .await;
    p51_close_owner(denied).await;

    // Ready control: a secret is handled before any network request.
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    let (slot, record) = install_candidate(&registry, id, "leak AKIAIOSFODNN7EXAMPLE");
    let hooks = Arc::new(CoManagedTestHooks::default());
    let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));
    let outcome = super::co_managed_cycle(
        app.handle(),
        &handle,
        id,
        &fixture.room_root,
        &slot,
        slot.snapshot().seq,
        &record,
    )
    .await;
    assert!(matches!(outcome, super::CoManagedOutcome::Done(_)));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert!(hooks.commit_calls() > 0);
    assert!(slot.snapshot().value.record().is_none());
}

fn p51_assert_restore_body_counts(
    mode: &str,
    body_entries: &std::sync::atomic::AtomicUsize,
    body_completions: &std::sync::atomic::AtomicUsize,
) {
    assert_eq!(
        body_entries.load(Ordering::SeqCst),
        usize::from(mode != "invalid")
    );
    assert_eq!(
        body_completions.load(Ordering::SeqCst),
        usize::from(matches!(mode, "ready" | "tail-panic"))
    );
}

async fn p51_assert_restore_state(
    mode: &str,
    owner: &SettingsState,
    manager: &Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>,
    saved_path: &std::path::Path,
    saved_bytes: &[u8],
) {
    assert!(manager.read().await.list_sessions().await.is_empty());
    assert_eq!(std::fs::read(saved_path).unwrap(), saved_bytes);
    if mode == "invalid" {
        assert_eq!(
            owner.availability(),
            crate::config::settings::SettingsAvailability::Invalid
        );
        assert!(owner.current_read().await.is_err());
    }
}

// Production admission/lifecycle boundary; the concrete Wry body is not executed.
async fn p51_restore_lifecycle_case(mode: &'static str) {
    use crate::session::manager::SessionManager;
    use crate::session::selection::SelectionCoordinator;
    let (dir, owner) = p51_open_owner(settings_with_agent()).await;
    let snapshot = owner.current_read().await.unwrap().clone();
    let rows = Ok::<_, String>(vec![persisted_row("saved-p51", true)]);
    let saved_path = dir.path().join("sessions.json");
    let saved_bytes = serde_json::to_vec(rows.as_ref().unwrap()).unwrap();
    std::fs::write(&saved_path, &saved_bytes).unwrap();
    if mode == "invalid" {
        p51_invalidate_owner(&owner).await;
    }
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(
        Arc::clone(&manager),
        tokio_util::sync::CancellationToken::new(),
    );
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    coordinator.start(app.handle().clone()).unwrap();
    let barrier = coordinator.submit_restore_first().await.unwrap();
    let observer = Arc::new(RestoreObserverStartBarrier::default());
    observer.mark_restore_admitted().unwrap();
    let flag = Arc::new(super::RestoreInProgress(
        std::sync::atomic::AtomicBool::new(true),
    ));
    let (started, handshake) = tokio::sync::oneshot::channel();
    let mut queued = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move {
            started.send(()).unwrap();
            coordinator.snapshot().await
        })
    };
    handshake.await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut queued)
        .await
        .is_err());
    let body_entries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tail_entries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let future_entries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let body_completions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tail_completions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let body = {
        let body_entries = Arc::clone(&body_entries);
        let future_entries = Arc::clone(&future_entries);
        let body_completions = Arc::clone(&body_completions);
        let manager = Arc::clone(&manager);
        Box::new(move || -> futures::future::BoxFuture<'static, ()> {
            body_entries.fetch_add(1, Ordering::SeqCst);
            assert_ne!(
                mode, "invalid",
                "Invalid owner admitted saved restore input"
            );
            assert_ne!(mode, "sync-panic", "synchronous body panic control");
            Box::pin(async move {
                future_entries.fetch_add(1, Ordering::SeqCst);
                assert!(matches!(rows, Ok(ref rows) if !rows.is_empty()));
                assert!(!snapshot.agents.is_empty());
                assert!(manager.read().await.list_sessions().await.is_empty());
                assert_ne!(mode, "future-panic", "body future panic control");
                body_completions.fetch_add(1, Ordering::SeqCst);
            })
        })
    };
    let continuation = {
        let body_completions = Arc::clone(&body_completions);
        let tail_completions = Arc::clone(&tail_completions);
        let (body_entries, tail_entries, flag, observer) = (
            Arc::clone(&body_entries),
            Arc::clone(&tail_entries),
            Arc::clone(&flag),
            Arc::clone(&observer),
        );
        Box::new(move || -> futures::future::BoxFuture<'static, ()> {
            Box::pin(async move {
                assert!(!flag.0.load(Ordering::SeqCst));
                observer
                    .start("p51-independent", || {
                        assert_eq!(tail_entries.fetch_add(1, Ordering::SeqCst), 0);
                    })
                    .unwrap();
                p51_assert_restore_body_counts(mode, &body_entries, &body_completions);
                assert_ne!(mode, "tail-panic", "tail panic control");
                tail_completions.fetch_add(1, Ordering::SeqCst);
            })
        })
    };
    let restore = super::spawn_restore_startup(
        barrier,
        Arc::clone(&observer),
        Arc::clone(&flag),
        Arc::clone(&owner),
        body,
        continuation,
    );
    tokio::time::timeout(Duration::from_secs(2), restore)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        body_entries.load(Ordering::SeqCst),
        usize::from(mode != "invalid")
    );
    assert_eq!(tail_entries.load(Ordering::SeqCst), 1);
    assert_eq!(
        future_entries.load(Ordering::SeqCst),
        usize::from(!matches!(mode, "invalid" | "sync-panic"))
    );
    assert_eq!(
        body_completions.load(Ordering::SeqCst),
        usize::from(matches!(mode, "ready" | "tail-panic"))
    );
    assert_eq!(
        tail_completions.load(Ordering::SeqCst),
        usize::from(mode != "tail-panic")
    );
    assert!(!flag.0.load(Ordering::SeqCst));
    observer.start("p51-after-join", || {}).unwrap();
    tokio::time::timeout(Duration::from_secs(2), queued)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    p51_assert_restore_state(mode, &owner, &manager, &saved_path, &saved_bytes).await;
    tokio::time::timeout(Duration::from_secs(2), coordinator.close_and_join())
        .await
        .unwrap();
    p51_close_owner(owner).await;
    drop(dir);
}

#[tokio::test]
async fn e2992_p51_restore_unavailable_completes() {
    p51_restore_lifecycle_case("invalid").await;
}

#[tokio::test]
async fn e2992_p51_restore_ready_runs_body() {
    p51_restore_lifecycle_case("ready").await;
}

#[tokio::test]
async fn e2992_p51_restore_body_future_panic_completes() {
    p51_restore_lifecycle_case("future-panic").await;
}

#[tokio::test]
async fn e2992_p51_restore_body_sync_panic_completes() {
    p51_restore_lifecycle_case("sync-panic").await;
}

#[tokio::test]
async fn e2992_p51_restore_tail_panic_keeps_completion() {
    p51_restore_lifecycle_case("tail-panic").await;
}

fn settings_with_context_regex(regex: &str) -> AppSettings {
    let mut settings = settings_with_agent();
    settings.agents[0].context_regex = Some(regex.to_string());
    settings
}

fn resolved(settings: AppSettings) -> std::collections::HashMap<String, String> {
    let source = ScraperPatterns {
        settings: crate::config::settings::new_settings_state(settings) as SettingsState,
    };
    futures::executor::block_on(source.patterns()).expect("Ready context fixture")
}

#[tokio::test]
async fn sample_adapter_is_nonblocking_and_recovers_only_after_quarter_capacity() {
    let capacity = crate::session::context_alerts::CONTEXT_SAMPLE_QUEUE_CAPACITY;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(capacity);
    let adapter = ScraperSamples {
        sender,
        closed_logged: AtomicBool::new(false),
        saturated: AtomicBool::new(false),
        dropped: AtomicU64::new(0),
    };
    let sample = || ContextSample::Unavailable {
        session_id: uuid::Uuid::nil(),
    };

    for _ in 0..capacity {
        adapter.observe(sample());
    }
    adapter.observe(sample());
    assert!(adapter.saturated.load(Ordering::Relaxed));
    assert_eq!(adapter.dropped.load(Ordering::Relaxed), 1);

    receiver.recv().await.expect("one queued sample");
    adapter.observe(sample());
    assert!(
        adapter.saturated.load(Ordering::Relaxed),
        "a one-slot drain must not end the saturation episode"
    );

    for _ in 0..257 {
        receiver.recv().await.expect("queued sample to drain");
    }
    adapter.observe(sample());
    assert!(!adapter.saturated.load(Ordering::Relaxed));
    assert_eq!(adapter.dropped.load(Ordering::Relaxed), 0);
    assert!(adapter.sender.capacity() >= capacity / 4);

    drop(receiver);
    adapter.observe(sample());
    adapter.observe(sample());
    assert!(adapter.closed_logged.load(Ordering::Relaxed));
}

/// #1032 - the adapter must hand `compile` the string the user wrote, byte for byte.
///
/// The pattern is the ONLY defence this feature has: the engine deliberately ships no
/// anchoring rules of its own, so every rule that makes a reading trustworthy lives in
/// the user's text. An engine that edits that text can only weaken it, and it does so
/// silently, in the one place nobody thinks to look.
///
/// The concrete loss this pins: `  Context [\u{2591}\u{2588}]+ (\d{1,3})%` is the natural
/// transcription of a row the plan says ALWAYS starts at column 2 - copy the row, swap
/// the bar and the number for classes. Trimming it deletes the column-2 anchor, and with
/// the statusline suppressed the pattern then matches input-box prose and reports a
/// confident 99% that is a lie. Failing open, in a design where everything else fails
/// closed.
#[test]
fn the_adapter_hands_over_the_users_pattern_verbatim() {
    let user_wrote = "  Context [\u{2591}\u{2588}]+ (\\d{1,3})%";
    let patterns = resolved(settings_with_context_regex(user_wrote));

    assert_eq!(
        patterns.get("codex").map(String::as_str),
        Some(user_wrote),
        "the engine must not rewrite the user's regex, and leading spaces ARE the anchor"
    );
}

/// The consequence, end to end through the real adapter: what the user configured is
/// what gets read, and it reports NO number rather than a wrong one.
///
/// The grid here is the statusline-suppressed case (`/help`, autocomplete, a hook turned
/// off), where the only `Context ... %` on screen is prose the user typed themselves.
/// The column-2 anchor is the single thing that rejects it. Resolve the pattern through
/// the adapter, compile it, run it: `None`. With the adapter trimming, this same row
/// read `Some(99)` - a confident lie - which is why the string identity above matters.
#[test]
fn a_pattern_resolved_through_the_adapter_still_rejects_input_box_prose() {
    let user_wrote = "  Context [\u{2591}\u{2588}]+ (\\d{1,3})%";
    let patterns = resolved(settings_with_context_regex(user_wrote));
    let resolved_source = patterns.get("codex").expect("configured");

    let pattern = crate::pty::context_scrape::pattern::compile(resolved_source)
        .expect("the user's pattern compiles");
    let grid = vec![
        "\u{276f} The row says Context \u{2588}\u{2588}\u{2588}\u{2588}\u{2588} 99% right now"
            .to_string(),
    ];

    assert_eq!(
        crate::pty::context_scrape::rows::extract(&pattern, &grid),
        None,
        "no number beats a wrong number: the engine must not edit the only defence there is"
    );
}

/// Trailing whitespace is just as much the user's business: `%` then a space is a
/// pattern that requires a space, and only the user knows whether their row has one.
#[test]
fn trailing_whitespace_in_a_pattern_is_the_users_business_too() {
    let user_wrote = "Context (\\d{1,3})% ";
    let patterns = resolved(settings_with_context_regex(user_wrote));

    assert_eq!(patterns.get("codex").map(String::as_str), Some(user_wrote));
}

/// A blank field is the field being blank, not a pattern. Skipped for log hygiene ONLY:
/// `pattern::compile` already refuses "" and "   " (no capture group 1), so this cannot
/// become "a pattern that matches everything" - it would merely warn on every change.
#[test]
fn a_blank_context_regex_is_treated_as_unconfigured() {
    assert!(resolved(settings_with_context_regex("")).is_empty());
    assert!(resolved(settings_with_context_regex("   ")).is_empty());
    assert!(
        resolved(settings_with_agent()).is_empty(),
        "None is unconfigured"
    );
}

#[test]
fn web_and_api_server_handles_can_be_managed_together() {
    let _app = crate::test_support::test_builder()
        .manage(WebServerHandle::default())
        .manage(ApiServerHandle::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("web and api server handles must be distinct managed types");
}

#[test]
fn restore_loop_normalizes_archived_roots_before_persisted_session_loop() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("read lib.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production lib source");
    let hoist = production
        .find("let archived_roots = sessions_persistence::normalize_project_roots")
        .expect("archived root normalization");
    let loop_start = production
        .find("for ps in &persisted")
        .expect("persisted session loop");

    assert!(
        hoist < loop_start,
        "startup restore must normalize archived roots once before the session loop"
    );
}

fn persisted_row(name: &str, was_active: bool) -> PersistedSession {
    PersistedSession {
        name: name.to_string(),
        working_directory: format!("C:/restore/{name}"),
        was_active,
        ..PersistedSession::default()
    }
}

#[test]
fn restore_active_flag_normalization_accepts_zero_flags() {
    let mut sessions = vec![
        persisted_row("first", false),
        persisted_row("second", false),
    ];
    assert_eq!(
        normalize_persisted_active_flags(&mut sessions),
        PersistedActiveFlagNormalization::Zero
    );
    assert!(sessions.iter().all(|session| !session.was_active));
}

#[test]
fn restore_active_flag_normalization_keeps_exactly_one_target() {
    let mut sessions = vec![persisted_row("first", false), persisted_row("second", true)];
    assert_eq!(
        normalize_persisted_active_flags(&mut sessions),
        PersistedActiveFlagNormalization::One { index: 1 }
    );
    assert!(!sessions[0].was_active);
    assert!(sessions[1].was_active);
}

#[test]
fn restore_active_flag_normalization_clears_all_conflicting_targets_stably() {
    let mut sessions = vec![persisted_row("first", true), persisted_row("second", true)];
    assert_eq!(
        normalize_persisted_active_flags(&mut sessions),
        PersistedActiveFlagNormalization::Multiple {
            identities: vec![
                "0:first@C:/restore/first".to_string(),
                "1:second@C:/restore/second".to_string(),
            ],
        }
    );
    assert!(sessions.iter().all(|session| !session.was_active));
}

#[test]
fn idle_git_and_discovery_are_held_by_the_real_restore_completion_barrier() {
    let barrier = RestoreObserverStartBarrier::default();
    let starts = std::sync::Mutex::new(Vec::new());
    for producer in ["idle", "git", "discovery"] {
        assert!(barrier
            .start(producer, || starts.lock().unwrap().push(producer))
            .is_err());
    }
    assert!(starts.lock().unwrap().is_empty());

    barrier.mark_restore_admitted().unwrap();
    for producer in ["idle", "git", "discovery"] {
        assert!(barrier
            .start(producer, || starts.lock().unwrap().push(producer))
            .is_err());
    }
    assert!(starts.lock().unwrap().is_empty());

    barrier.mark_restore_complete().unwrap();
    for producer in ["idle", "git", "discovery"] {
        barrier
            .start(producer, || starts.lock().unwrap().push(producer))
            .unwrap();
    }
    assert_eq!(*starts.lock().unwrap(), ["idle", "git", "discovery"]);
}
