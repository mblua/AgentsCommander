use super::*;

// ── #2455 - Co-managed logging, read through the test tee ───────────────

/// The lines the tee saw after `before`, filtered by `needle`. The tee
/// only appends, so skipping the first `before` entries is the difference
/// between two snapshots.
fn p4_lines_since(before: usize, needle: &str) -> Vec<String> {
    crate::logging::test_tee_snapshot()
        .into_iter()
        .skip(before)
        .filter(|line| line.contains(needle))
        .collect()
}

fn p4_tee_len() -> usize {
    crate::logging::test_tee_snapshot().len()
}

fn p4_short(session_id: uuid::Uuid) -> String {
    session_id.to_string()[..8].to_string()
}

/// `spawn_jev_listener` with the status line taken from an argument.
async fn spawn_jev_listener_with_status(
    status_line: &'static str,
    scores: Vec<(&'static str, f32)>,
) -> (String, Arc<AtomicUsize>) {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            read_http_request(&mut stream).await;
            let body = jev_response_body(&scores);
            let response = format!(
                "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\ncontent-length: \
                    {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    (format!("http://127.0.0.1:{port}/v1/systemone"), hits)
}

fn p4_all_scores() -> Vec<(&'static str, f32)> {
    vec![
        ("to-user", 0.9),
        ("to-peer", 0.1),
        ("to-root", 0.0),
        ("to-reply", 0.0),
    ]
}

/// #2455 test 1 (E): no candidate text and no API key reach the log.
#[tokio::test]
async fn p4_co_managed_logs_never_carry_candidate_text_or_the_api_key() {
    crate::logging::test_install_logger();
    const CANDIDATE_MARKER: &str = "CANDMARK-p4-7c1e";
    const KEY_MARKER: &str = "KEYMARK-p4-9f3a";
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(p4_all_scores()).await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    tauri::Manager::state::<SettingsState>(&app)
        .write()
        .await
        .jev_api_key = format!("key-{KEY_MARKER}");
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    let mut big = "x".repeat(50 * 1024);
    big.replace_range(100..100 + CANDIDATE_MARKER.len(), CANDIDATE_MARKER);
    install_candidate(&registry, session_id, &big);
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());

    let before = p4_tee_len();
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;

    let mine = p4_lines_since(before, &p4_short(session_id));
    assert!(
        mine.iter().any(|line| line.contains("cycle start")),
        "positive control: this run's cycle-start line must be captured: {mine:?}"
    );
    assert!(
        mine.iter().any(|line| line.contains("jev classify")),
        "positive control: this run's classify line must be captured: {mine:?}"
    );
    let all = crate::logging::test_tee_snapshot();
    assert!(
        !all.iter().any(|line| line.contains(CANDIDATE_MARKER)),
        "candidate text leaked into the log"
    );
    assert!(
        !all.iter().any(|line| line.contains(KEY_MARKER)),
        "the API key leaked into the log"
    );

    // E2 marker 1: a catalog whose `categories` key holds `A"MARK1"A`,
    // which serde escapes. Readiness rejects such a catalog before a
    // cycle, so `classify` is called directly: it is the only site that
    // logs this reason.
    const MARK1: &str = "MARK1-p4-51d0";
    let catalog = crate::capture::catalog::Catalog::from_json_str(&format!(
        "{{\"categories\": \"A\\\"{MARK1}\\\"A\"}}"
    ));
    let network = crate::network::OutboundNetwork::new().expect("network");
    let settings = crate::capture::jev::JevSettings {
        api_key: "k".to_string(),
        model: "m".to_string(),
        endpoint: "http://127.0.0.1:9/".to_string(),
        timeout_secs: 1,
        threshold: 0.7,
        margin: 0.15,
    };
    let catalog_tag = format!("e2cat-{}", p4_short(session_id));
    let before = p4_tee_len();
    let returned =
        crate::capture::jev::classify(&network, &settings, &catalog, "text", &catalog_tag).await;
    let catalog_lines = p4_lines_since(before, &catalog_tag);
    assert!(
        matches!(
            &returned,
            crate::capture::jev::ClassifyOutcome::Abstained { reason } if reason.contains(MARK1)
        ),
        "the RETURNED reason is unchanged: {returned:?}"
    );
    assert!(
        catalog_lines
            .iter()
            .any(|line| line
                .contains("catalog invalid: catalog is not valid JSON: <redacted len=")),
        "the class label and length must survive: {catalog_lines:?}"
    );

    // E2 markers 2 and 3: a raw `"MARK2"` response id (the round-7
    // bypass) and an id mimicking an allowlisted prefix.
    const MARK2: &str = "MARK2-p4-e3b8";
    const MARK3: &str = "MARK3-p4-9a17";
    let id2 = format!("\"{MARK2}\"");
    let id3 = format!("catalog invalid: {MARK3}");
    for hostile_id in [id2.as_str(), id3.as_str()] {
        let id_fixture = make_co_managed_fixture();
        let hostile: &'static str = Box::leak(hostile_id.to_string().into_boxed_str());
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-user", 0.9),
            ("to-peer", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
            (hostile, 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&id_fixture, endpoint, true);
        let id_session = add_claude_session(
            &manager,
            &id_fixture.coordinator_cwd,
            SessionStatus::Running,
            &id_fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, id_session, "candidate text");
        let (handle, _rx) =
            CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());
        let before = p4_tee_len();
        let trigger = CoManagedTrigger::IdleEdge(id_session);
        super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;
        let id_lines = p4_lines_since(before, &p4_short(id_session));
        assert!(
            id_lines.iter().any(|line| line.contains(
                "abstained after response: malformed response: unknown id <redacted len="
            )),
            "the classify class label and length must survive: {id_lines:?}"
        );
        assert!(
            id_lines.iter().any(|line| line
                .contains("Done(abstained: malformed response: unknown id <redacted len=")),
            "the cycle-end class label and length must survive: {id_lines:?}"
        );
    }

    let all = crate::logging::test_tee_snapshot();
    for marker in [MARK1, MARK2, MARK3] {
        assert!(
            !all.iter().any(|line| line.contains(marker)),
            "untrusted bytes {marker} leaked into the log"
        );
    }
}

/// #2455 test 8 (D, round 7): the request-build and malformed-body legs
/// of `send_once` each log a warn line of their own.
#[tokio::test]
async fn p4_send_once_logs_request_build_and_malformed_body_failures() {
    use tokio::io::AsyncWriteExt;
    crate::logging::test_install_logger();
    let network = crate::network::OutboundNetwork::new().expect("network");
    let fixture = make_co_managed_fixture();
    write_co_managed_catalog(&fixture.room_root);
    let catalog = crate::capture::catalog::Catalog::load(&fixture.room_root.join("catalog.json"));
    let tag = format!("d8-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let mut settings = crate::capture::jev::JevSettings {
        api_key: "k".to_string(),
        model: "m".to_string(),
        endpoint: "not a url".to_string(),
        timeout_secs: 5,
        threshold: 0.7,
        margin: 0.15,
    };

    let before = p4_tee_len();
    crate::capture::jev::classify(&network, &settings, &catalog, "text", &tag).await;
    let build_lines = p4_lines_since(before, &format!("jev send [{tag}]"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            read_http_request(&mut stream).await;
            let body = r#"{"results":[{"id":"to-user"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: \
                    {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    settings.endpoint = format!("http://127.0.0.1:{port}/v1/systemone");
    let before = p4_tee_len();
    crate::capture::jev::classify(&network, &settings, &catalog, "text", &tag).await;
    let body_lines = p4_lines_since(before, &format!("[{tag}]"));

    assert!(
        build_lines.iter().any(
            |line| line.starts_with("WARN ") && line.contains("attempt=1 request build failed")
        ),
        "{build_lines:?}"
    );
    assert!(
        body_lines.iter().any(|line| line.starts_with("WARN ")
            && line.contains("malformed body after status=200 OK: <unclassified reason len=")),
        "{body_lines:?}"
    );
    assert!(
        body_lines
            .last()
            .is_some_and(|line| line.starts_with("WARN ") && line.contains("permanent error")),
        "a failed call must not end on an INFO line: {body_lines:?}"
    );
}

/// #2455 test 2 (F): the discovery tick adds no line for a steady state.
#[tokio::test]
async fn p4_discovery_tick_logs_nothing_for_a_steady_state() {
    crate::logging::test_install_logger();
    let fixture = make_co_managed_fixture();
    let (app, manager, registry) =
        co_managed_app(&fixture, "http://127.0.0.1:9/".to_string(), true);
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());

    let before = p4_tee_len();
    let mut ticks = 0_usize;
    for _ in 0..12 {
        super::watch_capture_slots(app.handle(), &handle).await;
        ticks += 1;
    }

    assert_eq!(ticks, 12, "every tick must have run");
    assert!(
        handle.is_watching(&session_id.to_string()),
        "the ticks saw the session"
    );
    let grown = p4_lines_since(before, &p4_short(session_id));
    assert!(grown.is_empty(), "the tick must stay silent: {grown:?}");
}

/// One trigger that must end in exactly one drop line; returns its reason.
async fn p4_drop_reason<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    trigger: CoManagedTrigger,
) -> (String, String) {
    let id = trigger.session_id().to_string();
    let before = p4_tee_len();
    super::handle_co_managed_trigger(app, handle, trigger).await;
    let lines: Vec<String> = p4_lines_since(before, &id)
        .into_iter()
        .filter(|line| line.contains("trigger dropped"))
        .collect();
    assert_eq!(lines.len(), 1, "exactly one drop line for {id}: {lines:?}");
    let reason = lines[0]
        .split_once("]: ")
        .map(|(_, reason)| reason.to_string())
        .expect("reason after the session id");
    (lines[0].clone(), reason)
}

/// #2455 test 3 (A): the eight trigger drops carry eight distinct reasons.
#[tokio::test]
async fn p4_eight_trigger_drops_log_eight_distinct_reasons() {
    crate::logging::test_install_logger();
    let fixture = make_co_managed_fixture();
    let endpoint = "http://127.0.0.1:9/".to_string();
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());
    let mut reasons = Vec::new();

    // 1. No capture registry in state.
    let bare = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build bare app");
    let trigger = CoManagedTrigger::IdleEdge(uuid::Uuid::new_v4());
    reasons.push(p4_drop_reason(bare.handle(), &handle, trigger).await.1);

    let (app, manager, registry) = co_managed_app(&fixture, endpoint.clone(), true);
    // 2. No slot for the session.
    let trigger = CoManagedTrigger::IdleEdge(uuid::Uuid::new_v4());
    reasons.push(p4_drop_reason(app.handle(), &handle, trigger).await.1);
    // 3. A slot with no candidate record.
    let empty_id = uuid::Uuid::new_v4();
    let (_empty_capture, _empty_rx) = registry.open(&empty_id.to_string());
    let trigger = CoManagedTrigger::IdleEdge(empty_id);
    reasons.push(p4_drop_reason(app.handle(), &handle, trigger).await.1);
    // 4. A candidate for a session that is gone.
    let gone_id = uuid::Uuid::new_v4();
    install_candidate(&registry, gone_id, "candidate text");
    let trigger = CoManagedTrigger::IdleEdge(gone_id);
    reasons.push(p4_drop_reason(app.handle(), &handle, trigger).await.1);
    // 5. A session outside any room.
    let roomless = add_claude_session(
        &manager,
        &fixture.project,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, roomless, "candidate text");
    let trigger = CoManagedTrigger::IdleEdge(roomless);
    reasons.push(p4_drop_reason(app.handle(), &handle, trigger).await.1);
    // 6. Readiness is not Ready (room flag off), logged at warn.
    let off = make_co_managed_fixture();
    let (off_app, off_manager, off_registry) = co_managed_app(&off, endpoint, false);
    let off_id = add_claude_session(
        &off_manager,
        &off.coordinator_cwd,
        SessionStatus::Running,
        &off.projects_dir,
    )
    .await;
    install_candidate(&off_registry, off_id, "candidate text");
    let trigger = CoManagedTrigger::IdleEdge(off_id);
    let (off_line, off_reason) = p4_drop_reason(off_app.handle(), &handle, trigger).await;
    assert!(
        off_line.starts_with("WARN ") && off_line.contains("RoomFlagOff"),
        "readiness loss is a warn naming the reason: {off_line}"
    );
    reasons.push(off_reason);
    // 7. Contended for the same sequence, in a ready room.
    let contended = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    let (slot, _) = install_candidate(&registry, contended, "candidate text");
    handle.mark_contended(&contended.to_string(), slot.seq());
    let trigger = CoManagedTrigger::IdleEdge(contended);
    reasons.push(p4_drop_reason(app.handle(), &handle, trigger).await.1);
    // 8. Not an idle edge, and the session is busy.
    let busy = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, busy, "candidate text");
    let trigger = CoManagedTrigger::SlotChanged(busy);
    reasons.push(p4_drop_reason(app.handle(), &handle, trigger).await.1);

    let distinct: std::collections::HashSet<&String> = reasons.iter().collect();
    assert_eq!(
        distinct.len(),
        8,
        "eight distinct drop reasons: {reasons:#?}"
    );
}

/// #2455 test 4 (D4-b): classify lines carry the cycle's session tag, and
/// the dry run carries `dry-run`.
#[tokio::test]
async fn p4_classify_lines_carry_the_session_tag_and_the_dry_run_tag() {
    crate::logging::test_install_logger();
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(p4_all_scores()).await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let session_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, session_id, "candidate text");
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());
    let before = p4_tee_len();
    super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::IdleEdge(session_id),
    )
    .await;
    let tag = format!("jev classify [{}]", p4_short(session_id));
    let tagged = p4_lines_since(before, &tag);

    let settings = crate::capture::jev::JevSettings {
        api_key: "k".to_string(),
        model: "m".to_string(),
        endpoint: "http://127.0.0.1:9/".to_string(),
        timeout_secs: 1,
        threshold: 0.7,
        margin: 0.15,
    };
    let before = p4_tee_len();
    crate::commands::co_managed::classify_dry_run(
        &crate::capture::catalog::Catalog::missing(),
        "dry text",
        &settings,
        &fixture.owners.capture.secret_rules,
    )
    .await
    .unwrap();
    let dry = p4_lines_since(before, "jev classify [dry-run]");

    assert!(!dry.is_empty(), "the dry run must log with its own tag");
    assert!(!tagged.is_empty(), "classify must log with the cycle's tag");
}

/// #2455 test 5 (D): the success status reaches the log; a retried 503
/// logs two warn lines, attempt 1 and attempt 2.
#[tokio::test]
async fn p4_send_once_logs_the_status_attempt_and_latency() {
    crate::logging::test_install_logger();
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener_with_status("200 OK", p4_all_scores()).await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let ok_id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Running,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, ok_id, "candidate text");
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());
    let before = p4_tee_len();
    let trigger = CoManagedTrigger::IdleEdge(ok_id);
    super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;
    let ok_lines = p4_lines_since(before, &format!("jev send [{}]", p4_short(ok_id)));

    let failing = make_co_managed_fixture();
    let (endpoint, hits) =
        spawn_jev_listener_with_status("503 Service Unavailable", p4_all_scores()).await;
    let (app, manager, registry) = co_managed_app(&failing, endpoint, true);
    let bad_id = add_claude_session(
        &manager,
        &failing.coordinator_cwd,
        SessionStatus::Running,
        &failing.projects_dir,
    )
    .await;
    install_candidate(&registry, bad_id, "candidate text");
    let (handle, _rx) =
        CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());
    let before = p4_tee_len();
    let trigger = CoManagedTrigger::IdleEdge(bad_id);
    super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;
    let bad_lines = p4_lines_since(before, &format!("jev send [{}]", p4_short(bad_id)));

    assert_eq!(hits.load(Ordering::SeqCst), 2, "503 is retried once");
    assert_eq!(bad_lines.len(), 2, "{bad_lines:?}");
    for (index, line) in bad_lines.iter().enumerate() {
        assert!(line.starts_with("WARN "), "{line}");
        assert!(line.contains("status=503"), "{line}");
        assert!(line.contains(&format!("attempt={}", index + 1)), "{line}");
    }
    assert_eq!(ok_lines.len(), 1, "{ok_lines:?}");
    assert!(ok_lines[0].starts_with("INFO "), "{ok_lines:?}");
    assert!(ok_lines[0].contains("attempt=1"), "{ok_lines:?}");
    assert!(ok_lines[0].contains("status=200 OK"), "{ok_lines:?}");
    assert!(ok_lines[0].contains("latency_ms="), "{ok_lines:?}");
}

/// #2455 test 6 (B): the cycle-start line names the trigger kind.
#[tokio::test]
async fn p4_cycle_start_names_the_trigger_kind() {
    crate::logging::test_install_logger();
    let mut kinds = Vec::new();
    for idle_edge in [true, false] {
        let status = if idle_edge {
            SessionStatus::Running
        } else {
            SessionStatus::Idle
        };
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(p4_all_scores()).await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            status,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let (handle, _rx) =
            CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());
        let before = p4_tee_len();
        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            if idle_edge {
                CoManagedTrigger::IdleEdge(session_id)
            } else {
                CoManagedTrigger::SlotChanged(session_id)
            },
        )
        .await;
        let start: Vec<String> = p4_lines_since(before, &session_id.to_string())
            .into_iter()
            .filter(|line| line.contains("cycle start"))
            .collect();
        assert_eq!(start.len(), 1, "{start:?}");
        let kind = start[0]
            .split("trigger=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .expect("trigger kind")
            .to_string();
        kinds.push(kind);
    }
    assert_ne!(
        kinds[0], kinds[1],
        "two trigger kinds must differ: {kinds:?}"
    );
    assert_eq!(
        kinds,
        vec!["IdleEdge".to_string(), "SlotChanged".to_string()]
    );
}

/// #2455 test 7 (B, round 6): the route line names the resolved variant
/// and its effect kind, from inside `co_managed_cycle`.
#[tokio::test]
async fn p4_route_line_names_the_variant_and_kind() {
    crate::logging::test_install_logger();
    let mut routes = Vec::new();
    for winner in ["to-user", "to-peer"] {
        let fixture = make_co_managed_fixture();
        let scores: Vec<(&'static str, f32)> = ["to-user", "to-peer", "to-root", "to-reply"]
            .into_iter()
            .map(|id| (id, if id == winner { 0.9 } else { 0.0 }))
            .collect();
        let (endpoint, _hits) = spawn_jev_listener(scores).await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let (handle, _rx) =
            CoManagedSupervisorHandle::with_capture_state(fixture.owners.capture.clone());
        let before = p4_tee_len();
        let trigger = CoManagedTrigger::IdleEdge(session_id);
        super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;
        let lines = p4_lines_since(before, &format!("route [{session_id}]"));
        assert_eq!(lines.len(), 1, "{lines:?}");
        let route = lines[0]
            .split_once("]: ")
            .map(|(_, rest)| rest.to_string())
            .expect("route fields");
        routes.push(route);
        let outcome: Vec<String> = p4_lines_since(before, &session_id.to_string())
            .into_iter()
            .filter(|line| line.contains("cycle end"))
            .collect();
        assert_eq!(outcome.len(), 1, "{outcome:?}");
        assert!(
            !outcome[0].contains("route none") && !outcome[0].contains("route done"),
            "the outcome arm must not be called a route: {outcome:?}"
        );
    }
    assert_eq!(
        routes,
        vec![
            "variant=User kind=TextToUser".to_string(),
            "variant=Wake kind=Automatic".to_string(),
        ]
    );
}
