use super::*;
use std::sync::atomic::AtomicUsize;

/// Fast, deterministic, and above all NOT read from the process environment: a test
/// must not change its verdict because someone exported a tunable.
fn test_thresholds() -> Thresholds {
    Thresholds {
        stall_timeout: Duration::from_millis(120),
        paint_floor: DEFAULT_PAINT_FLOOR_BYTES,
        head_bytes: 512,
        concurrency_window: Duration::from_millis(10_000),
        stop_attribution: Duration::from_millis(500),
        teardown_suppression: Duration::from_millis(50),
        startup_poll: Duration::from_millis(5),
        steady_poll: Duration::from_millis(10),
    }
}

fn test_init(cli: Option<CodingAgentKind>) -> SpawnRecordInit {
    SpawnRecordInit {
        session_id: Uuid::new_v4(),
        pid: Some(4242),
        argv: vec!["codex".to_string(), "resume".to_string()],
        cwd: "C:/repo".to_string(),
        cli,
        agent_profile_id: cli.map(|_| "agent_1782513272568_0".to_string()),
        codex_home: None,
        configured_env_count: 0,
        removed_env_count: 0,
        redact: Vec::new(),
        window: SpawnWindow {
            window_ms: 10_000,
            total: 0,
            same_cli: 0,
        },
        started: Instant::now(),
        thresholds: test_thresholds(),
    }
}

fn test_record(cli: Option<CodingAgentKind>) -> SpawnRecord {
    SpawnRecord::new(test_init(cli))
}

/// A record whose startup deadline is already in the past.
fn aged_record(cli: Option<CodingAgentKind>) -> SpawnRecord {
    let thresholds = test_thresholds();
    SpawnRecord {
        started: Instant::now() - thresholds.stall_timeout - Duration::from_millis(1),
        ..test_record(cli)
    }
}

const CONPTY_PROLOGUE: &[u8] = b"\x1b[?9001h\x1b[?1004h";

/// The real thing, captured off a live `cmd.exe /C codex ...` spawn: ConPTY handshake,
/// ConPTY screen setup, and the npm shim's own `title %COMSPEC%`. 71 bytes, ~45ms, and
/// the agent has not written a thing.
const AC_SHELL_PROLOGUE: &[u8] = b"\x1b[?9001h\x1b[?1004h\x1b[?25l\x1b[2J\x1b[m\x1b[H\
      \x1b]0;C:\\WINDOWS\\system32\\cmd.exe \x07\x1b[?25h";

fn wait_until(deadline: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    done()
}

#[test]
fn head_capture_is_capped_and_stops_after_full() {
    let record = test_record(Some(CodingAgentKind::Codex));
    let cap = record.thresholds.head_bytes;
    record.note_output(&vec![b'a'; cap + 64]);
    record.note_output(b"ignored");

    let head = record.head.lock().unwrap();
    assert_eq!(head.len(), cap);
    assert!(head.iter().all(|b| *b == b'a'));
    assert!(record.head_full.load(Ordering::Relaxed));
    assert_eq!(
        record.bytes_read.load(Ordering::Relaxed),
        (cap + 64 + 7) as u64
    );
}

#[test]
fn first_output_stamp_is_recorded_once() {
    let record = test_record(None);
    assert!(!record.saw_output());

    record.note_output(b"hello");
    let first = record.first_output_us.load(Ordering::Relaxed);
    record.note_output(b" world");

    assert!(record.saw_output());
    assert_eq!(record.first_output_us.load(Ordering::Relaxed), first);
    assert_eq!(record.bytes_read.load(Ordering::Relaxed), 11);
}

#[test]
fn paint_is_stamped_once_the_output_passes_the_floor() {
    let record = test_record(Some(CodingAgentKind::Codex));

    // The ConPTY handshake alone is not a paint.
    record.note_output(CONPTY_PROLOGUE);
    assert!(record.saw_output());
    assert!(!record.painted());

    record.note_output(&vec![b'x'; record.thresholds.paint_floor as usize]);
    assert!(record.painted());
    let painted_at = record.first_paint_us.load(Ordering::Relaxed);

    record.note_output(b"more");
    assert_eq!(
        record.first_paint_us.load(Ordering::Relaxed),
        painted_at,
        "the paint stamp is taken once"
    );
}

#[test]
fn the_shell_prologue_alone_is_not_a_painted_screen() {
    // Measured, not assumed: 71 bytes of ConPTY + npm-shim noise land on the PTY of
    // EVERY codex spawn within ~45ms. With the old 64-byte floor they crossed it on
    // their own, so a codex that painted nothing at all read as "painted", the stall
    // detector never fired, and the deadline report called the hang healthy.
    assert_eq!(AC_SHELL_PROLOGUE.len(), 71);
    assert!(
        (AC_SHELL_PROLOGUE.len() as u64) < Thresholds::DEFAULT.paint_floor,
        "the paint floor must sit above the shell noise band"
    );

    let agent = aged_record(Some(CodingAgentKind::Codex));
    agent.note_output(AC_SHELL_PROLOGUE);

    assert!(agent.saw_output(), "the shell greeted us");
    assert!(!agent.painted(), "but the agent painted nothing");
    assert!(agent.is_stalled());
}

#[test]
fn a_coding_agent_that_never_paints_is_stalled_and_a_quiet_shell_is_not() {
    // Exactly what a hung Codex leaves behind on Windows: the ConPTY handshake and
    // not one byte more.
    let agent = aged_record(Some(CodingAgentKind::Codex));
    agent.note_output(CONPTY_PROLOGUE);
    assert!(
        agent.saw_output(),
        "ConPTY greets us even when the child never does"
    );
    assert!(agent.is_stalled());
    assert!(agent.startup_deadline_reached());

    let shell = aged_record(None);
    shell.note_output(b"C:\\repo>");
    assert!(
        !shell.is_stalled(),
        "a tiny shell prompt is a healthy start"
    );

    let painted = aged_record(Some(CodingAgentKind::Codex));
    painted.note_output(&vec![b'x'; painted.thresholds.paint_floor as usize]);
    assert!(
        !painted.is_stalled(),
        "a coding agent that painted its TUI is healthy"
    );
}

#[test]
fn the_startup_report_fires_once_for_every_session_whatever_it_painted() {
    // grinch F2: a hang that paints past the floor must still leave its bytes on
    // record, and so must a session with no coding-agent identity at all.
    let painted = aged_record(None);
    painted.note_output(&[b'x'; 200]);

    assert!(painted.startup_deadline_reached());
    assert!(painted.log_startup_report(&ChildLiveness::Alive));
    assert!(
        !painted.stall_reported.load(Ordering::Relaxed),
        "a painted session is a checkpoint, not a stall"
    );
    assert!(
        !painted.log_startup_report(&ChildLiveness::Alive),
        "the startup verdict is written once"
    );
    assert!(!painted.startup_deadline_reached());

    let stalled = aged_record(Some(CodingAgentKind::Codex));
    stalled.note_output(CONPTY_PROLOGUE);
    assert!(stalled.log_startup_report(&ChildLiveness::Alive));
    assert!(stalled.stall_reported.load(Ordering::Relaxed));
}

#[test]
fn a_disabled_stall_timeout_disables_the_startup_report() {
    let record = SpawnRecord {
        thresholds: Thresholds {
            stall_timeout: Duration::ZERO,
            ..test_thresholds()
        },
        ..aged_record(Some(CodingAgentKind::Codex))
    };
    assert!(!record.startup_deadline_reached());
}

#[test]
fn exit_is_reported_once_and_carries_its_cause() {
    let record = test_record(Some(CodingAgentKind::Codex));
    let liveness = ChildLiveness::Exited {
        code: 1,
        success: false,
    };

    assert!(record.log_child_exit(ExitCause::ChildInitiated, &liveness, "observed-by-monitor"));
    assert!(
        !record.log_child_exit(ExitCause::AcRequested, &liveness, "reaped-after-stop"),
        "a second exit report must be suppressed"
    );
    assert_eq!(
        *record.exit_cause.lock().unwrap(),
        Some(ExitCause::ChildInitiated)
    );
}

#[test]
fn output_keeps_flowing_after_a_stop_that_did_not_kill_the_child() {
    // grinch F5: a Quarantined resource-monitor kill leaves the agent running. The
    // record must not go blind, and a much later death is the child's own.
    let record = test_record(Some(CodingAgentKind::Codex));
    record.mark_ac_stop("watchdog", Some(ChildLiveness::Alive));

    record.note_output(&[b'x'; 128]);
    assert_eq!(record.bytes_read.load(Ordering::Relaxed), 128);
    assert_eq!(
        record.attribute_exit(record.stop_snapshot()),
        ExitCause::AcRequested,
        "inside the attribution window the exit is ours"
    );

    let stale = test_record(Some(CodingAgentKind::Codex));
    stale.mark_ac_stop("watchdog", Some(ChildLiveness::Alive));
    // Rewind the clock: the stop request is now older than the attribution window.
    let long_ago = stale.thresholds.stop_attribution + Duration::from_secs(1);
    let stale = SpawnRecord {
        started: Instant::now() - long_ago,
        ..stale
    };
    assert_eq!(
        stale.attribute_exit(stale.stop_snapshot()),
        ExitCause::ChildInitiated,
        "a kill that evidently failed does not own a death an hour later"
    );
}

#[test]
fn a_child_already_dead_at_the_stop_is_never_attributed_to_ac() {
    // grinch F6: the kill path probes before it touches anything, and that witness
    // outranks any flag the monitor reads afterwards.
    let record = test_record(Some(CodingAgentKind::Codex));
    record.mark_ac_stop(
        "session-kill",
        Some(ChildLiveness::Exited {
            code: 1,
            success: false,
        }),
    );

    assert_eq!(
        record.attribute_exit(record.stop_snapshot()),
        ExitCause::ChildInitiated,
        "it was already dead when AC asked"
    );
}

#[test]
fn an_exit_seen_before_the_stop_flag_is_the_child_s_own() {
    let record = test_record(Some(CodingAgentKind::Codex));
    let before = record.stop_snapshot(); // no stop yet

    // The stop lands after the snapshot, with no pre-stop witness (the
    // resource-monitor path). The exit we already saw cannot be ours.
    record.mark_ac_stop("watchdog", None);

    assert_eq!(record.attribute_exit(before), ExitCause::ChildInitiated);
}

#[test]
fn closing_a_coding_agent_tab_is_never_reported_as_a_spontaneous_death() {
    // grinch B1, the chain that ships on EVERY closed Codex tab:
    //   1. destroy_session publishes the witness: the child is Alive.
    //   2. kill_group TerminateProcess()es the process tree. Its root IS the PTY
    //      child, so the child is now dead with exit code 1.
    //   3. PtyBackend::kill probes that corpse and marks the stop again.
    // A "latest wins" witness takes the corpse as evidence that the child died on its
    // own, and log_child_exit then stamps WARN + argv + head on it, because
    // `exited(1)` is not a clean exit. That is requirement 3 inverted on the most
    // common path in the app, and `exited(1)` is literally the ExitStatus string from
    // the log trap this issue exists to kill.
    let record = test_record(Some(CodingAgentKind::Codex));
    record.note_output(&[b'x'; 300]); // it came up fine (past the paint floor)

    record.mark_ac_stop("session-destroy", Some(ChildLiveness::Alive));
    record.mark_ac_stop(
        "session-kill",
        Some(ChildLiveness::Exited {
            code: 1,
            success: false,
        }),
    );

    assert_eq!(
        record.attribute_exit(record.stop_snapshot()),
        ExitCause::AcRequested,
        "a probe taken after AC has begun killing is not a witness, \
        it is a photograph of our own kill"
    );
    assert!(
        matches!(record.pre_stop_liveness(), Some(ChildLiveness::Alive)),
        "the earliest probe of the stop episode is the witness"
    );
    assert_eq!(
        record.stop_source(),
        "session-kill",
        "source and timestamp stay last-wins; only the witness is first-wins"
    );
}

#[test]
fn ac_stop_takes_the_most_recent_source_and_timestamp() {
    // grinch D2: first-wins timestamps let a stale, failed stop own every later stop.
    let record = test_record(Some(CodingAgentKind::Codex));
    assert!(!record.stop_requested());

    record.mark_ac_stop("job-terminate", Some(ChildLiveness::Alive));
    let first_stamp = record.ac_stop_at_us.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(5));
    record.mark_ac_stop("session-kill", None);

    assert!(record.stop_requested());
    assert_eq!(record.stop_source(), "session-kill");
    assert!(
        record.ac_stop_at_us.load(Ordering::SeqCst) > first_stamp,
        "the attribution window is measured from the most recent stop"
    );
}

#[test]
fn a_new_stop_episode_takes_a_fresh_witness() {
    // The counterpart to B1: the witness is first-wins WITHIN an episode, not forever.
    // A stop that failed leaves the child running, so the next stop must look again.
    let record = SpawnRecord {
        thresholds: Thresholds {
            stop_attribution: Duration::from_millis(30),
            ..test_thresholds()
        },
        ..test_record(Some(CodingAgentKind::Codex))
    };

    // A blocked RM kill: the agent keeps running.
    record.mark_ac_stop("job-terminate", Some(ChildLiveness::Alive));
    std::thread::sleep(Duration::from_millis(45));

    // Much later the child dies on its own, and only then does the user close the tab.
    record.mark_ac_stop(
        "session-kill",
        Some(ChildLiveness::Exited {
            code: 0,
            success: true,
        }),
    );

    assert_eq!(
        record.attribute_exit(record.stop_snapshot()),
        ExitCause::ChildInitiated,
        "the stale stop did not kill it, and it was already dead when we asked again"
    );
}

#[test]
fn a_witness_that_saw_the_child_dead_can_never_be_downgraded() {
    // grinch B2: `LocalProcessBackend::kill` is reachable twice concurrently (the
    // blocking-spawn cancel guard fires it directly), and the second probe lands after
    // `ptys.remove` and returns `Gone`. A real death must not be blanked by it.
    let record = SpawnRecord {
        thresholds: Thresholds {
            stop_attribution: Duration::from_millis(1),
            ..test_thresholds()
        },
        ..test_record(Some(CodingAgentKind::Codex))
    };
    record.mark_ac_stop(
        "session-kill",
        Some(ChildLiveness::Exited {
            code: 1,
            success: false,
        }),
    );
    std::thread::sleep(Duration::from_millis(5)); // even once the episode has expired
    record.mark_ac_stop("session-kill", Some(ChildLiveness::Gone));
    record.mark_ac_stop(
        "session-kill",
        Some(ChildLiveness::Unqueryable("os error 5".to_string())),
    );

    assert!(
        matches!(
            record.pre_stop_liveness(),
            Some(ChildLiveness::Exited { .. })
        ),
        "a dead child does not come back"
    );
    assert_eq!(
        record.attribute_exit(record.stop_snapshot()),
        ExitCause::ChildInitiated
    );
}

#[test]
fn a_witness_less_stop_never_erases_a_witness_we_already_have() {
    let record = test_record(Some(CodingAgentKind::Codex));
    record.mark_ac_stop(
        "session-kill",
        Some(ChildLiveness::Exited {
            code: 1,
            success: false,
        }),
    );
    // The resource monitor publishes no witness; it must not blank the real one.
    record.mark_ac_stop("watchdog", None);

    assert_eq!(
        record.attribute_exit(record.stop_snapshot()),
        ExitCause::ChildInitiated,
        "it was already dead when AC asked, and a witness-less stop cannot rewrite that"
    );
}

#[test]
fn a_stale_failed_stop_does_not_poison_the_stop_that_finally_lands() {
    // grinch D2, the real run: RM Kill is blocked by AV/EDR (Quarantined leaves the
    // agent running), the user closes the session minutes later, we kill it. With a
    // first-wins anchor our own kill came out as `child-initiated` with a WARN and a
    // head dump: a fabricated smoking gun, the exact lie #942 exists to remove.
    let record = SpawnRecord {
        thresholds: Thresholds {
            stop_attribution: Duration::from_millis(30),
            ..test_thresholds()
        },
        ..test_record(Some(CodingAgentKind::Codex))
    };

    record.mark_ac_stop("job-terminate", Some(ChildLiveness::Alive));
    std::thread::sleep(Duration::from_millis(45));
    assert_eq!(
        record.attribute_exit(record.stop_snapshot()),
        ExitCause::ChildInitiated,
        "a stop that evidently failed does not own a death long after it"
    );

    record.mark_ac_stop("session-kill", Some(ChildLiveness::Alive));
    assert_eq!(
        record.attribute_exit(record.stop_snapshot()),
        ExitCause::AcRequested,
        "the window is measured from the stop that actually killed it"
    );
    assert_eq!(record.stop_source(), "session-kill");
}

#[test]
fn a_blocked_stop_cannot_fake_a_stall_on_a_session_that_keeps_painting() {
    // Same anchor, second consequence: the timing stamps are suppressed while a stop
    // is in flight, so the stall verdict must not depend on them.
    let record = SpawnRecord {
        thresholds: Thresholds {
            teardown_suppression: Duration::from_millis(10),
            ..test_thresholds()
        },
        ..aged_record(Some(CodingAgentKind::Codex))
    };
    record.mark_ac_stop("job-terminate", Some(ChildLiveness::Alive));
    record.note_output(&[b'x'; 300]);

    assert!(
        !record.painted(),
        "the paint stamp is suppressed while a stop is in flight"
    );
    assert!(
        !record.is_stalled(),
        "but the bytes are on record, so a blocked kill cannot fake a stall"
    );
}

#[test]
fn a_blank_session_killed_by_ac_still_dumps_its_evidence() {
    // The user who kills a blank terminal at t=3s, before the deadline report can
    // fire, is reporting THIS bug. An ac-requested exit must not answer with a
    // one-line shrug just because the kill was ours.
    let record = test_record(Some(CodingAgentKind::Codex));
    record.note_output(CONPTY_PROLOGUE);
    record.mark_ac_stop("session-kill", Some(ChildLiveness::Alive));

    assert!(
        record.is_stalled(),
        "16 bytes of ConPTY handshake is not a painted screen"
    );
    assert!(record.log_child_exit(
        ExitCause::AcRequested,
        &ChildLiveness::Exited {
            code: 1,
            success: false,
        },
        "observed-at-stop"
    ));
    assert_eq!(
        *record.exit_cause.lock().unwrap(),
        Some(ExitCause::AcRequested)
    );
}

#[test]
fn secrets_are_scrubbed_before_the_text_is_escaped() {
    // grinch D1: escaping first and matching second is a SILENT no-op for any secret
    // holding a backslash, a quote, a control character or non-ASCII, because the
    // haystack has been rewritten and the needle has not. A redaction that fails
    // quietly is worse than none: the feature invites people to paste the log.
    let windows_path = r"C:\Users\maria\.codex\auth.json";
    let quoted = "a\"b\\c-secret-value";
    let control = "tok\tsecret\r\n1234";
    let unicode = "p\u{e4}ssw\u{f6}rd-\u{fc}-9876";
    let record = SpawnRecord::new(SpawnRecordInit {
        argv: vec![
            "codex".to_string(),
            format!("--config={windows_path}"),
            format!("--auth={quoted}"),
        ],
        redact: vec![
            windows_path.to_string(),
            quoted.to_string(),
            control.to_string(),
            unicode.to_string(),
        ],
        ..test_init(Some(CodingAgentKind::Codex))
    });
    record.note_output(
        format!("loading {windows_path} with {quoted} {control} {unicode}\r\n").as_bytes(),
    );

    let head = record.head_log();
    let argv = record.argv_log();
    for secret in [windows_path, quoted, control, unicode] {
        assert!(
            !head.contains(secret),
            "raw secret survived in head: {head}"
        );
        assert!(
            !argv.contains(secret),
            "raw secret survived in argv: {argv}"
        );
    }
    // The escaped shapes must not survive either: escaping happens after the scrub.
    assert!(!head.contains("auth.json"), "escaped path survived: {head}");
    assert!(!argv.contains("auth.json"), "escaped path survived: {argv}");
    assert!(
        !head.contains("9876"),
        "escaped non-ASCII secret survived: {head}"
    );
    assert!(head.contains("<redacted>"));
    assert!(argv.contains("<redacted>"));
    assert!(
        head.contains("loading"),
        "the rest of the line still has to be readable: {head}"
    );
}

#[test]
fn a_key_smuggled_inside_a_url_in_the_child_output_is_redacted_too() {
    let record = test_record(Some(CodingAgentKind::Codex));
    record.note_output(
        b"error: GET https://generativelanguage.googleapis.com/v1beta/models?\
        key=AIzaSyFAKE_KEY_VALUE failed\n",
    );

    let head = record.head_log();
    assert!(
        !head.contains("AIzaSyFAKE_KEY_VALUE"),
        "url key survived: {head}"
    );
    assert!(
        head.contains("error: GET"),
        "the error text still has to be readable"
    );
}

#[test]
fn only_secret_keyed_env_rows_are_redacted() {
    // grinch D1 (second half): sweeping every configured env value blanked
    // `MODEL=gpt-5.6-sol` inside our own argv, the argv requirement 1 exists to
    // record, and could shred the Codex error text inside `head=`.
    assert!(is_secret_env_key("AGENTSCOMMANDER_TOKEN"));
    assert!(is_secret_env_key("OPENAI_API_KEY"));
    assert!(is_secret_env_key("DB_PASSWORD"));
    assert!(is_secret_env_key("gh_secret"));
    assert!(is_secret_env_key("SERVICE_CREDENTIAL"));
    assert!(is_secret_env_key("AUTHORIZATION"));
    assert!(is_secret_env_key("CODEX_AUTH"));
    assert!(is_secret_env_key("GH_PAT"));
    assert!(is_secret_env_key("pat"));

    assert!(!is_secret_env_key("MODEL"));
    assert!(!is_secret_env_key("CODEX_HOME"));
    assert!(!is_secret_env_key("AGENTSCOMMANDER_ROOT"));
    assert!(!is_secret_env_key("NODE_ENV"));
    // "pat" is a segment, never a substring: blanking a PATH across the log would
    // shred every path in the evidence.
    assert!(!is_secret_env_key("PATH"));
    assert!(!is_secret_env_key("PATHEXT"));
    assert!(!is_secret_env_key("AGENTSCOMMANDER_BINARY_PATH"));
}

#[test]
fn a_short_secret_from_a_secret_named_row_is_still_redacted() {
    // The 8-char floor used to gate rows whose NAME already declares them a secret.
    let record = SpawnRecord::new(SpawnRecordInit {
        redact: vec!["sk-ab".to_string()],
        ..test_init(Some(CodingAgentKind::Codex))
    });
    record.note_output(b"AUTHORIZATION=Bearer sk-ab\r\n");

    let head = record.head_log();
    assert!(!head.contains("sk-ab"), "short secret survived: {head}");
    assert!(head.contains("<redacted>"));
}

#[test]
fn a_numeric_value_from_a_secret_named_row_is_a_config_knob_not_a_secret() {
    // `MAX_TOKENS=8192` and `TOKEN_LIMIT=4096` are real rows whose NAME matches the
    // secret markers. Blanking `8192` everywhere would corrupt the evidence and
    // protect nothing.
    let record = SpawnRecord::new(SpawnRecordInit {
        argv: vec!["codex".to_string(), "--max-tokens=8192".to_string()],
        redact: vec!["8192".to_string()],
        ..test_init(Some(CodingAgentKind::Codex))
    });
    record.note_output(b"max_tokens=8192 loading\r\n");

    assert!(record.head_log().contains("8192"));
    assert!(record.argv_log().contains("8192"));
}

#[test]
fn credentials_are_scrubbed_from_echoed_output() {
    let token = "3f1c9d2e-7a4b-4c8d-9e1f-2b3c4d5e6f70";
    let record = SpawnRecord::new(SpawnRecordInit {
        argv: vec!["codex".to_string(), format!("--token={token}")],
        redact: vec![token.to_string()],
        ..test_init(Some(CodingAgentKind::Codex))
    });
    record.note_output(format!("AGENTSCOMMANDER_TOKEN={token}\r\n").as_bytes());

    assert!(record.head_log().contains("<redacted>"));
    assert!(!record.head_log().contains(token));
    assert!(record.argv_log().contains("<redacted>"));
    assert!(!record.argv_log().contains(token));
    assert!(
        record.head_log().contains("AGENTSCOMMANDER_TOKEN"),
        "the rest of the line still has to be readable"
    );
}

#[test]
fn thresholds_from_env_are_clamped() {
    assert_eq!(env_u64("AC_SPAWN_TEST_MISSING_KEY", 7, 100), 7);
    std::env::set_var("AC_SPAWN_TEST_CLAMP", "1000000000");
    assert_eq!(env_u64("AC_SPAWN_TEST_CLAMP", 512, 65_536), 65_536);
    std::env::set_var("AC_SPAWN_TEST_CLAMP", "not-a-number");
    assert_eq!(env_u64("AC_SPAWN_TEST_CLAMP", 512, 65_536), 512);
    std::env::remove_var("AC_SPAWN_TEST_CLAMP");
}

#[test]
fn spawn_window_counts_the_preceding_spawns_by_cli() {
    let owner = SpawnRegistry::new();
    let thresholds = test_thresholds();
    let first = note_spawn_attempt(Some(CodingAgentKind::Antigravity), thresholds, &owner);
    let second = note_spawn_attempt(Some(CodingAgentKind::Antigravity), thresholds, &owner);
    let third = note_spawn_attempt(Some(CodingAgentKind::Claude), thresholds, &owner);

    assert_eq!(first.same_cli, 0);
    assert_eq!(second.same_cli, 1, "two Antigravitys in the window");
    assert_eq!(third.same_cli, 0, "a Claude is not an Antigravity");
    assert!(second.total >= 1);
    assert!(third.total >= 2);
    assert_eq!(
        first.window_ms,
        thresholds.concurrency_window.as_millis() as u64
    );
}

#[test]
fn registry_forgets_a_session() {
    let owner = SpawnRegistry::new();
    let record = register(test_init(None), &owner);
    let id = record.session_id();

    assert!(mark_ac_stop(id, "session-kill", None, &owner).is_some());
    forget(id, &owner);
    assert!(mark_ac_stop(id, "session-kill", None, &owner).is_none());
}

// ---- the monitor thread (grinch N1) ----

struct FakeProbe {
    answers: Mutex<Vec<ChildLiveness>>,
    calls: AtomicUsize,
}

impl FakeProbe {
    /// Answers are consumed in order; the last one repeats forever.
    fn new(answers: Vec<ChildLiveness>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers),
            calls: AtomicUsize::new(0),
        })
    }

    fn probe(self: &Arc<Self>) -> impl Fn() -> ChildLiveness + Send + 'static {
        let probe = Arc::clone(self);
        move || {
            probe.calls.fetch_add(1, Ordering::SeqCst);
            let mut answers = probe.answers.lock().unwrap();
            if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers.first().cloned().unwrap_or(ChildLiveness::Gone)
            }
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[test]
fn monitor_reports_a_child_that_dies_on_its_own_and_then_ends() {
    let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
    let probe = FakeProbe::new(vec![
        ChildLiveness::Alive,
        ChildLiveness::Alive,
        ChildLiveness::Exited {
            code: 1,
            success: false,
        },
    ]);

    let handle = watch_child(Arc::clone(&record), probe.probe());
    assert!(
        wait_until(Duration::from_secs(2), || handle.is_finished()),
        "the monitor must end with the child"
    );
    handle.join().expect("monitor thread");

    assert_eq!(
        *record.exit_cause.lock().unwrap(),
        Some(ExitCause::ChildInitiated)
    );
    let calls = probe.calls();
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(calls, probe.calls(), "no polling after the exit");
}

#[test]
fn monitor_attributes_an_exit_after_an_ac_stop_to_ac() {
    let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
    record.mark_ac_stop("session-kill", Some(ChildLiveness::Alive));
    let probe = FakeProbe::new(vec![ChildLiveness::Exited {
        code: 1,
        success: false,
    }]);

    let handle = watch_child(Arc::clone(&record), probe.probe());
    handle.join().expect("monitor thread");

    assert_eq!(
        *record.exit_cause.lock().unwrap(),
        Some(ExitCause::AcRequested)
    );
}

#[test]
fn monitor_keeps_child_initiated_when_the_child_was_dead_before_the_stop() {
    let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
    record.mark_ac_stop(
        "session-kill",
        Some(ChildLiveness::Exited {
            code: 0,
            success: true,
        }),
    );
    let probe = FakeProbe::new(vec![ChildLiveness::Exited {
        code: 0,
        success: true,
    }]);

    let handle = watch_child(Arc::clone(&record), probe.probe());
    handle.join().expect("monitor thread");

    assert_eq!(
        *record.exit_cause.lock().unwrap(),
        Some(ExitCause::ChildInitiated),
        "AC asked, but it was already dead: the stop did not cause this"
    );
}

#[test]
fn monitor_ends_when_the_session_is_gone_without_reporting_an_exit() {
    let record = Arc::new(test_record(None));
    let probe = FakeProbe::new(vec![ChildLiveness::Gone]);

    let handle = watch_child(Arc::clone(&record), probe.probe());
    handle.join().expect("monitor thread");

    assert!(!record.exit_reported.load(Ordering::Relaxed));
    assert!(record.exit_cause.lock().unwrap().is_none());
}

#[test]
fn monitor_writes_the_startup_verdict_for_a_silent_child_and_keeps_watching() {
    let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
    record.note_output(CONPTY_PROLOGUE);
    let probe = FakeProbe::new(vec![ChildLiveness::Alive]);

    let handle = watch_child(Arc::clone(&record), probe.probe());
    assert!(
        wait_until(Duration::from_secs(2), || {
            record.startup_reported.load(Ordering::Relaxed)
                && record.stall_reported.load(Ordering::Relaxed)
        }),
        "the startup verdict must land at the deadline"
    );
    assert!(record.stall_reported.load(Ordering::Relaxed));
    assert!(
        !handle.is_finished(),
        "a stalled but living child stays under watch"
    );

    // The monitor still ends when the session goes away.
    *probe.answers.lock().unwrap() = vec![ChildLiveness::Gone];
    assert!(wait_until(Duration::from_secs(2), || handle.is_finished()));
    handle.join().expect("monitor thread");
}

#[test]
fn monitor_survives_a_panicking_probe() {
    let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
    let handle = watch_child(Arc::clone(&record), || panic!("poisoned child mutex"));

    assert!(
        wait_until(Duration::from_secs(2), || handle.is_finished()),
        "a panicking probe must end the monitor, not the app"
    );
    handle
        .join()
        .expect("the panic is caught inside the thread");
    assert!(!record.exit_reported.load(Ordering::Relaxed));
}

#[test]
fn monitor_reports_an_unqueryable_child_without_calling_it_alive() {
    let record = Arc::new(test_record(Some(CodingAgentKind::Codex)));
    record.note_output(CONPTY_PROLOGUE);
    let probe = FakeProbe::new(vec![ChildLiveness::Unqueryable("os error 5".to_string())]);

    let handle = watch_child(Arc::clone(&record), probe.probe());
    assert!(
        wait_until(Duration::from_secs(2), || record
            .startup_reported
            .load(Ordering::Relaxed)),
        "an unqueryable child still gets its startup verdict"
    );
    assert!(!record.exit_reported.load(Ordering::Relaxed));

    *probe.answers.lock().unwrap() = vec![ChildLiveness::Gone];
    assert!(wait_until(Duration::from_secs(2), || handle.is_finished()));
    handle.join().expect("monitor thread");
}
