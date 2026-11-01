use super::*;
use std::path::PathBuf;

fn shell_program() -> &'static str {
    if cfg!(windows) {
        "cmd.exe"
    } else {
        "sh"
    }
}

#[test]
fn parse_version_token_fixtures() {
    assert_eq!(
        parse_version_token("2.1.245 (Claude Code)").as_deref(),
        Some("2.1.245")
    );
    assert_eq!(
        parse_version_token("codex-cli 0.149.1").as_deref(),
        Some("0.149.1")
    );
    assert_eq!(parse_version_token("0.84.3").as_deref(), Some("0.84.3"));
    assert_eq!(
        parse_version_token("Hermes Agent v0.17.0 (2026.6.19) \u{b7} upstream").as_deref(),
        Some("0.17.0")
    );
    assert_eq!(parse_version_token("1.1.20").as_deref(), Some("1.1.20"));
    assert_eq!(
        parse_version_token("\n\n  1.18.23\n").as_deref(),
        Some("1.18.23")
    );
    assert_eq!(
        parse_version_token("\r\n\r\n1.2.3\r\n").as_deref(),
        Some("1.2.3")
    );
    assert_eq!(
        parse_version_token("\n\x1b[?25l\n2.0.1\n").as_deref(),
        Some("2.0.1")
    );
    assert_eq!(
        parse_version_token("\x1b[32mv1.2.3\x1b[0m\nsecond").as_deref(),
        Some("1.2.3")
    );
    assert_eq!(parse_version_token("hello\n1.2.3"), None);
    assert_eq!(parse_version_token("hello world"), None);
    assert_eq!(parse_version_token(""), None);
    assert_eq!(parse_version_token("\x1b[?25l\n"), None);
}

#[test]
fn first_text_line_skips_blank_and_ansi_only_lines() {
    assert_eq!(
        first_text_line("\n\x1b[?25l\n2.0.1\n").as_deref(),
        Some("2.0.1")
    );
    assert_eq!(first_text_line("\r\n  x  \r\n").as_deref(), Some("x"));
    assert_eq!(first_text_line("\x1b[?25l\n"), None);
    assert_eq!(first_text_line(""), None);
}

/// #1551 - an OSC sequence ends ONLY at BEL or at the exact pair `ESC \` (ST).
/// An `ESC` followed by anything else is OSC content: the parser stays inside
/// the sequence and keeps discarding, so malformed or adversarial CLI output
/// cannot leak sequence bytes into the version token or the diagnostic.
#[test]
fn osc_ends_only_at_bel_or_st() {
    // ESC ] foo ESC X bar BEL 1.2.3 -> the whole OSC is dropped.
    let esc_not_st = "\x1b]foo\x1bXbar\x071.2.3";
    assert_eq!(strip_ansi_and_controls(esc_not_st), "1.2.3");
    assert_eq!(first_text_line(esc_not_st).as_deref(), Some("1.2.3"));
    assert_eq!(parse_version_token(esc_not_st).as_deref(), Some("1.2.3"));
    assert_eq!(sanitize_detail(esc_not_st.as_bytes()), "1.2.3");

    // ST terminates it, and the text after ST survives.
    let st = "\x1b]0;window title\x1b\\2.0.1";
    assert_eq!(strip_ansi_and_controls(st), "2.0.1");
    assert_eq!(parse_version_token(st).as_deref(), Some("2.0.1"));

    // A non-ST ESC inside OSC never consumes the ESC of a later ST pair.
    assert_eq!(strip_ansi_and_controls("\x1b]a\x1b\x1b\\3.1.4"), "3.1.4");

    // BEL still terminates, as before.
    assert_eq!(strip_ansi_and_controls("\x1b]0;title\x074.5.6"), "4.5.6");

    // An OSC that never terminates still swallows the rest of the line.
    let unterminated = "\x1b]never ends 9.9.9";
    assert_eq!(strip_ansi_and_controls(unterminated), "");
    assert_eq!(first_text_line(unterminated), None);
    assert_eq!(parse_version_token(unterminated), None);
    assert_eq!(sanitize_detail(unterminated.as_bytes()), "");
}

#[test]
fn sanitize_detail_strips_ansi_controls_and_truncates() {
    assert_eq!(sanitize_detail(b"\x1b[32mv1.2.3\x1b[0m\nsecond"), "v1.2.3");
    assert_eq!(sanitize_detail(b"\n\x1b[?25l\n2.0.1\n"), "2.0.1");
    let long = "a".repeat(400);
    let truncated = sanitize_detail(long.as_bytes());
    assert_eq!(truncated.chars().count(), DETAIL_MAX_CHARS + 3);
    assert!(truncated.ends_with("..."));
    assert_eq!(&truncated[..DETAIL_MAX_CHARS], "a".repeat(DETAIL_MAX_CHARS));
    assert_eq!(sanitize_detail(b""), "");
}

#[tokio::test]
async fn probe_version_parses_echoed_version() {
    let args: Vec<&str> = if cfg!(windows) {
        vec!["/C", "echo 1.2.3"]
    } else {
        vec!["-c", "echo 1.2.3"]
    };
    let outcome = probe_version(Path::new(shell_program()), &args, PROBE_TIMEOUT).await;
    assert_eq!(outcome, ProbeOutcome::Version("1.2.3".to_string()));
}

#[tokio::test]
async fn probe_version_skips_leading_blank_lines() {
    let args: Vec<&str> = if cfg!(windows) {
        vec!["/C", "echo.& echo 1.2.3"]
    } else {
        vec!["-c", "printf '\\n\\n1.2.3\\n'"]
    };
    let outcome = probe_version(Path::new(shell_program()), &args, PROBE_TIMEOUT).await;
    assert_eq!(outcome, ProbeOutcome::Version("1.2.3".to_string()));
}

#[tokio::test]
async fn probe_version_skips_an_ansi_only_first_line() {
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("cmd.exe", vec!["/C", "echo \x1b[?25l& echo 2.0.1"])
    } else {
        ("sh", vec!["-c", "printf '\\033[?25l\\n2.0.1\\n'"])
    };
    let outcome = probe_version(Path::new(program), &args, PROBE_TIMEOUT).await;
    assert_eq!(outcome, ProbeOutcome::Version("2.0.1".to_string()));
}

#[tokio::test]
async fn probe_version_nonzero_exit_fails_with_code() {
    let args: Vec<&str> = if cfg!(windows) {
        vec!["/C", "exit 3"]
    } else {
        vec!["-c", "exit 3"]
    };
    let outcome = probe_version(Path::new(shell_program()), &args, PROBE_TIMEOUT).await;
    match outcome {
        ProbeOutcome::Failed(detail) => {
            assert!(detail.starts_with("exit code 3"), "unexpected: {detail}")
        }
        other @ ProbeOutcome::Version(_) => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn probe_version_no_token_fails() {
    let args: Vec<&str> = if cfg!(windows) {
        vec!["/C", "echo hello"]
    } else {
        vec!["-c", "echo hello"]
    };
    let outcome = probe_version(Path::new(shell_program()), &args, PROBE_TIMEOUT).await;
    match outcome {
        ProbeOutcome::Failed(detail) => assert!(
            detail.starts_with("no version in output: hello"),
            "unexpected: {detail}"
        ),
        other @ ProbeOutcome::Version(_) => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn probe_version_timeout_kills_tree() {
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("cmd.exe", vec!["/C", "ping", "-n", "30", "127.0.0.1"])
    } else {
        ("sh", vec!["-c", "sleep 30"])
    };
    let started = Instant::now();
    let outcome = probe_version(Path::new(program), &args, Duration::from_millis(200)).await;
    match outcome {
        ProbeOutcome::Failed(detail) => {
            assert!(detail.contains("timed out"), "unexpected: {detail}")
        }
        other @ ProbeOutcome::Version(_) => panic!("unexpected: {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "probe timeout must kill the tree promptly"
    );
}

async fn cancel_blocked_probe(program: &str, args: Vec<&str>) {
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let probe = tokio::spawn({
        let program = PathBuf::from(program);
        let args: Vec<String> = args.into_iter().map(str::to_string).collect();
        async move {
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            probe_version_cancellable(
                &program,
                &borrowed,
                Duration::from_secs(30),
                &std::env::var_os("PATH").unwrap_or_default(),
                cancel_rx,
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel_tx.send(true).expect("request cancellation");
    let outcome = tokio::time::timeout(Duration::from_secs(15), probe)
        .await
        .expect("probe cancellation settled")
        .expect("probe task");
    assert_eq!(outcome, CancellableProbeOutcome::Cancelled);
}

#[cfg(any(windows, unix))]
async fn cancel_during_probe_settlement(
    program: &str,
    args: Vec<&str>,
    native_observations: std::collections::VecDeque<ProbeNativeObservation>,
) -> CancellableProbeOutcome {
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let (proof_ready_tx, proof_ready_rx) = tokio::sync::oneshot::channel();
    let (release_proof_tx, release_proof_rx) = tokio::sync::oneshot::channel();
    let probe = tokio::spawn({
        let program = PathBuf::from(program);
        let args: Vec<String> = args.into_iter().map(str::to_string).collect();
        async move {
            with_probe_settlement_hook(
                ProbeSettlementHook {
                    before_native_proof: Some(proof_ready_tx),
                    release_native_proof: Some(release_proof_rx),
                    settlement_window: Some(Duration::from_millis(60)),
                    native_observations,
                    panic_after_native_proof_release: false,
                    panic_after_native_query_defect: false,
                    stdout_reader: None,
                },
                async move {
                    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                    probe_version_cancellable(
                        &program,
                        &borrowed,
                        Duration::from_secs(10),
                        &std::env::var_os("PATH").unwrap_or_default(),
                        cancel_rx,
                    )
                    .await
                },
            )
            .await
        }
    });
    proof_ready_rx
        .await
        .expect("probe direct handle dropped before native proof");
    cancel_tx
        .send(true)
        .expect("request cancellation during settlement");
    tokio::task::yield_now().await;
    assert!(
        !probe.is_finished(),
        "probe terminal outcome must remain behind native proof"
    );
    release_proof_tx
        .send(())
        .expect("release probe native proof");
    tokio::time::timeout(Duration::from_secs(15), probe)
        .await
        .expect("probe settlement completed")
        .expect("probe task")
}

#[cfg(windows)]
#[tokio::test]
async fn cancelled_contained_probe() {
    cancel_blocked_probe("cmd.exe", vec!["/C", "cmd /C ping -n 30 127.0.0.1 >NUL"]).await;
    let outcome = cancel_during_probe_settlement(
        "cmd.exe",
        vec!["/D", "/C", "exit 0"],
        std::collections::VecDeque::from([
            ProbeNativeObservation::Active,
            ProbeNativeObservation::Active,
            ProbeNativeObservation::Active,
            ProbeNativeObservation::Error("injected query error"),
            ProbeNativeObservation::Empty,
        ]),
    )
    .await;
    let CancellableProbeOutcome::CleanupFailed(detail) = outcome else {
        panic!("expected later-settled cleanup failure, got {outcome:?}");
    };
    assert!(detail.contains("settlement attempt deadline exceeded during job accounting proof"));
    assert!(detail.contains("job accounting failed: injected query error"));
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_probe_kills_unix_process_group_and_drains_readers() {
    cancel_blocked_probe("sh", vec!["-c", "sh -c 'sleep 30' & wait"]).await;
    let outcome = cancel_during_probe_settlement(
        "sh",
        vec!["-c", "exit 0"],
        std::collections::VecDeque::from([
            ProbeNativeObservation::Active,
            ProbeNativeObservation::Active,
            ProbeNativeObservation::Active,
            ProbeNativeObservation::Error("EPERM"),
            ProbeNativeObservation::Empty,
        ]),
    )
    .await;
    let CancellableProbeOutcome::CleanupFailed(detail) = outcome else {
        panic!("expected later-settled cleanup failure, got {outcome:?}");
    };
    assert!(detail
        .contains("settlement attempt deadline exceeded during process-group accounting proof"));
    assert!(detail.contains("process-group accounting failed: EPERM"));
}

#[test]
fn cache_lookup_ttl_and_single_flight() {
    let cache = Arc::new(AgentInstallCache::new());
    let now = Instant::now();
    assert!(matches!(
        cache.lookup("bob", now, INSTALL_CACHE_TTL),
        CacheLookup::Absent
    ));
    let ticket = cache.try_begin("bob").expect("ticket");
    assert!(matches!(
        cache.lookup("bob", now, INSTALL_CACHE_TTL),
        CacheLookup::InFlight
    ));
    let committed = ticket.complete(InstallState::missing("gone".to_string()));
    match committed {
        Completion::Committed(state) => assert_eq!(state.seq, 1),
        Completion::Stale(_) => panic!("unexpected stale completion"),
    }
    match cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL) {
        CacheLookup::Fresh(state) => {
            assert_eq!(state.status, InstallStatus::Missing);
            assert_eq!(state.seq, 1);
        }
        other => panic!("unexpected: {other:?}"),
    }
    // Expired by a 0s TTL (an explicitly later `now` keeps this deterministic).
    let later = Instant::now() + Duration::from_secs(1);
    assert!(matches!(
        cache.lookup("bob", later, Duration::ZERO),
        CacheLookup::Absent
    ));
}

#[test]
fn cache_commit_closes_the_ticket_once() {
    let cache = Arc::new(AgentInstallCache::new());
    let ticket = cache.try_begin("bob").expect("ticket");
    assert!(matches!(
        ticket.complete(InstallState::missing("gone".to_string())),
        Completion::Committed(_)
    ));
    let second = cache.try_begin("bob");
    assert!(second.is_some(), "the slot is free after a commit");
    assert!(
        cache.try_begin("bob").is_none(),
        "a second ticket for the same command must be refused"
    );
}

#[test]
fn cache_lookup_or_begin_opens_exactly_one_ticket_under_contention() {
    let cache = Arc::new(AgentInstallCache::new());
    let barrier = std::sync::Barrier::new(8);
    let began = std::sync::Mutex::new(Vec::new());
    let in_flight_reports = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                barrier.wait();
                match cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true) {
                    Scheduling::Began(ticket) => began.lock().expect("began lock").push(ticket),
                    Scheduling::InFlight => {
                        in_flight_reports.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    Scheduling::Fresh(_) => panic!("nothing was committed yet"),
                    Scheduling::Deferred => panic!("may_begin was true"),
                }
            });
        }
    });
    let mut began = began.into_inner().expect("began lock");
    assert_eq!(began.len(), 1);
    assert_eq!(
        in_flight_reports.load(std::sync::atomic::Ordering::SeqCst),
        7
    );
    assert_eq!(cache.in_flight_len(), 1);
    let ticket = began.pop().expect("the one ticket");
    assert!(matches!(
        ticket.complete(InstallState::missing("gone".to_string())),
        Completion::Committed(_)
    ));
    match cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true) {
        Scheduling::Fresh(state) => assert_eq!(state.seq, 1),
        _ => panic!("a committed entry must be served fresh"),
    }
}

#[test]
fn cache_lookup_or_begin_serves_a_commit_that_landed_before_the_second_call() {
    let cache = Arc::new(AgentInstallCache::new());
    let ticket = match cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true) {
        Scheduling::Began(ticket) => ticket,
        _ => panic!("first call must begin"),
    };
    let state = match ticket.complete(InstallState::missing("gone".to_string())) {
        Completion::Committed(state) => state,
        Completion::Stale(_) => panic!("unexpected stale completion"),
    };
    match cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true) {
        Scheduling::Fresh(served) => assert_eq!(served, state),
        _ => panic!("the second call must be served, never begin"),
    }
    assert_eq!(cache.in_flight_len(), 0);
}

#[test]
fn cache_lookup_or_begin_defers_when_scheduling_is_not_allowed() {
    let cache = Arc::new(AgentInstallCache::new());
    assert!(matches!(
        cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, false),
        Scheduling::Deferred
    ));
    assert_eq!(cache.in_flight_len(), 0);
    let ticket = cache.try_begin("bob").expect("ticket");
    assert!(matches!(
        cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, false),
        Scheduling::InFlight
    ));
    assert!(matches!(
        ticket.complete(InstallState::missing("gone".to_string())),
        Completion::Committed(_)
    ));
    assert!(matches!(
        cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, false),
        Scheduling::Fresh(_)
    ));
}

#[test]
fn cache_lookup_or_begin_replaces_an_expired_entry() {
    let cache = Arc::new(AgentInstallCache::new());
    let ticket = cache.try_begin("bob").expect("ticket");
    assert!(matches!(
        ticket.complete(InstallState::missing("gone".to_string())),
        Completion::Committed(_)
    ));
    let later = Instant::now() + Duration::from_secs(1);
    let renewed = match cache.lookup_or_begin("bob", later, Duration::ZERO, true) {
        Scheduling::Began(ticket) => ticket,
        _ => panic!("an expired entry must be replaced"),
    };
    assert!(matches!(
        cache.lookup("bob", later, Duration::ZERO),
        CacheLookup::InFlight
    ));
    drop(renewed);
}

#[test]
fn cache_stale_completion_renews_ticket_and_blocks_competitors() {
    let cache = Arc::new(AgentInstallCache::new());
    let ticket = cache.try_begin("bob").expect("ticket");
    cache.invalidate_all();
    let renewed = match ticket.complete(InstallState::missing("first".to_string())) {
        Completion::Stale(renewed) => renewed,
        Completion::Committed(_) => panic!("an invalidated generation must be rejected"),
    };
    assert!(matches!(
        cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true),
        Scheduling::InFlight
    ));
    let second = InstallState::missing("second".to_string());
    let committed = match renewed.complete(second.clone()) {
        Completion::Committed(state) => state,
        Completion::Stale(_) => panic!("the renewed ticket must commit"),
    };
    assert_eq!(committed.seq, 1);
    assert_eq!(committed.detail.as_deref(), Some("second"));
    match cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL) {
        CacheLookup::Fresh(state) => assert_eq!(state, committed),
        other => panic!("unexpected: {other:?}"),
    }
    assert!(matches!(
        cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true),
        Scheduling::Fresh(_)
    ));
}

#[test]
fn cache_repeated_invalidation_rejects_twice_then_frees_the_slot() {
    let cache = Arc::new(AgentInstallCache::new());
    let ticket = cache.try_begin("bob").expect("ticket");
    cache.invalidate_all();
    let second = match ticket.complete(InstallState::missing("one".to_string())) {
        Completion::Stale(renewed) => renewed,
        Completion::Committed(_) => panic!("expected a stale completion"),
    };
    cache.invalidate_all();
    let third = match second.complete(InstallState::missing("two".to_string())) {
        Completion::Stale(renewed) => renewed,
        Completion::Committed(_) => panic!("expected a second stale completion"),
    };
    assert!(matches!(
        cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
        CacheLookup::InFlight
    ));
    assert!(matches!(
        cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true),
        Scheduling::InFlight
    ));
    drop(third);
    assert!(matches!(
        cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
        CacheLookup::Absent
    ));
    assert!(matches!(
        cache.lookup_or_begin("bob", Instant::now(), INSTALL_CACHE_TTL, true),
        Scheduling::Began(_)
    ));
    assert_eq!(cache.seq(), 0);
    assert_eq!(cache.generation(), 2);
}

#[test]
fn cache_seq_is_monotonic_across_invalidation() {
    let cache = Arc::new(AgentInstallCache::new());
    let ticket = cache.try_begin("bob").expect("ticket");
    match ticket.complete(InstallState::missing("one".to_string())) {
        Completion::Committed(state) => assert_eq!(state.seq, 1),
        Completion::Stale(_) => panic!("unexpected stale completion"),
    }
    cache.invalidate_all();
    let ticket = cache.try_begin("bob").expect("ticket");
    match ticket.complete(InstallState::missing("two".to_string())) {
        Completion::Committed(state) => assert_eq!(state.seq, 2),
        Completion::Stale(_) => panic!("unexpected stale completion"),
    }
    assert_eq!(cache.generation(), 1);
}

#[test]
fn dropped_ticket_frees_in_flight() {
    let cache = Arc::new(AgentInstallCache::new());
    let ticket = cache.try_begin("bob").expect("ticket");
    assert_eq!(cache.in_flight_len(), 1);
    drop(ticket);
    assert_eq!(cache.in_flight_len(), 0);
    assert!(cache.try_begin("bob").is_some());
}
