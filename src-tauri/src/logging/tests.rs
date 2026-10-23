use super::*;
// #612 the gate's enabled()/log() are `log::Log` trait methods; bring the
// trait into scope so the tests can call them on a `LevelGateLogger` value.
use log::Log;

/// Build an `ErrorLogEntry` carrying `message`; the other fields are fixed.
fn entry(message: &str) -> ErrorLogEntry {
    ErrorLogEntry {
        timestamp: "2026-05-21 12:00:00.000".to_string(),
        level: "ERROR".to_string(),
        target: "agentscommander_lib::test".to_string(),
        message: message.to_string(),
        type_id: "v1|25:agentscommander_lib::test|n|n".to_string(),
    }
}

/// #3036 — an entry of emission-site type `type_id` carrying `message`.
fn typed(type_id: &str, message: &str) -> ErrorLogEntry {
    ErrorLogEntry {
        type_id: type_id.to_string(),
        ..entry(message)
    }
}

/// #3036 — `typeId` of a synthetic record at `(target, file, line)`.
fn type_of(target: &str, file: Option<&str>, line: Option<u32>, msg: &str) -> String {
    ErrorLogEntry::from_record(
        String::new(),
        &log::Record::builder()
            .level(log::Level::Error)
            .target(target)
            .file(file)
            .line(line)
            .args(format_args!("{msg}"))
            .build(),
    )
    .type_id
}

fn messages(entries: Vec<ErrorLogEntry>) -> Vec<String> {
    entries.into_iter().map(|e| e.message).collect()
}

/// Run `should_capture` against a synthetic record. The `Record` (and the
/// `format_args!` temporary it borrows) is built and consumed inside this
/// single expression — see §7.1's lifetime note.
fn captures(level: log::Level, target: &str) -> bool {
    should_capture(
        &log::Record::builder()
            .level(level)
            .target(target)
            .args(format_args!("synthetic message"))
            .build(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn payload_panic_and_join_failures_collapse_without_payload_formatting() {
    const UNWIND_CANARY: &str = "PANIC_UNWIND_1173_C6R9";
    const FUTURE_CANARY: &str = "PANIC_FUTURE_1173_N4H7";
    const JOIN_CANARY: &str = "PANIC_JOIN_1173_W8K3";

    let unwind: Result<(), PayloadPanic> =
        catch_payload_unwind(|| std::panic::panic_any(UNWIND_CANARY));
    let future: Result<(), PayloadPanic> = catch_payload_future(async {
        std::panic::panic_any(FUTURE_CANARY);
    })
    .await;

    let handle: tokio::task::JoinHandle<()> = tokio::spawn(async {
        let _guard = PayloadWorkerGuard::enter();
        std::panic::panic_any(JOIN_CANARY);
    });
    let joined = handle.await.map(Ok::<(), PayloadPanic>);
    let collapsed = collapse_payload_task(joined);

    assert_eq!(unwind, Err(PayloadPanic));
    assert_eq!(future, Err(PayloadPanic));
    assert_eq!(collapsed, Err(PayloadPanic));
    let diagnostics = format!("{unwind:?}\n{future:?}\n{collapsed:?}");
    for canary in [UNWIND_CANARY, FUTURE_CANARY, JOIN_CANARY] {
        assert!(!diagnostics.contains(canary));
    }
    assert_eq!(diagnostics.matches("PayloadPanic").count(), 3);
}

#[test]
fn should_capture_applies_level_and_target_guard() {
    // ERROR from AgentsCommander's own targets (lib + bin) → captured.
    assert!(captures(
        log::Level::Error,
        "agentscommander_lib::commands::entity_creation"
    ));
    assert!(captures(log::Level::Error, "agentscommander"));
    // ERROR from a third-party crate → not captured (target guard).
    assert!(!captures(log::Level::Error, "hyper::client"));
    // Below ERROR, even from our own targets → not captured (level guard).
    assert!(!captures(log::Level::Warn, "agentscommander_lib::foo"));
    assert!(!captures(log::Level::Info, "agentscommander_lib::foo"));
}

#[test]
fn from_record_copies_all_fields_and_keeps_newlines() {
    let built = ErrorLogEntry::from_record(
        "2026-05-21 12:00:00.000".to_string(),
        &log::Record::builder()
            .level(log::Level::Error)
            .target("agentscommander_lib::commands::entity_creation")
            .args(format_args!("line one\nline two"))
            .build(),
    );
    assert_eq!(built.timestamp, "2026-05-21 12:00:00.000");
    assert_eq!(built.level, "ERROR");
    assert_eq!(
        built.target,
        "agentscommander_lib::commands::entity_creation"
    );
    // The embedded newline survives verbatim (multi-line git errors etc.).
    assert_eq!(built.message, "line one\nline two");
}

/// #280 LOW-3 — companion to `from_record_redacts_telegram_token_in_message`
/// for the Gemini `?key=` shape. The voice.rs source-side fix moves
/// the API key out of the URL entirely, but this defense-in-depth
/// scrub catches any future caller that re-introduces the query
/// param (or a third-party crate that prints request URLs).
#[test]
fn from_record_redacts_gemini_key_in_message() {
    let fake_url = "error sending request for url \
        (https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:\
        generateContent?key=AIzaSyFakeKeyForTesting1234567890)";
    let built = ErrorLogEntry::from_record(
        "2026-05-21 12:00:00.000".to_string(),
        &log::Record::builder()
            .level(log::Level::Error)
            .target("agentscommander_lib::commands::voice")
            .args(format_args!("Gemini API request failed: {}", fake_url))
            .build(),
    );
    assert!(
        built.message.contains("?key=***"),
        "expected redacted shape; got: {}",
        built.message
    );
    assert!(
        !built.message.contains("AIzaSyFakeKeyForTesting"),
        "key leaked into message: {}",
        built.message
    );
}

/// #280 — the error modal payload reaches the UI; secrets must be
/// scrubbed at `from_record` time so even a future caller that bypassed
/// the source-side redaction in api.rs cannot leak a token into the
/// frontend.
#[test]
fn from_record_redacts_telegram_token_in_message() {
    let fake_url = "error sending request for url \
        (https://api.telegram.org/\
        bot987654321:FAKE_TOKEN_FOR_TESTING_xxxxxxxxxxxxxxx/getUpdates?offset=0)";
    let built = ErrorLogEntry::from_record(
        "2026-05-21 12:00:00.000".to_string(),
        &log::Record::builder()
            .level(log::Level::Error)
            .target("agentscommander_lib::telegram::bridge")
            .args(format_args!("Telegram poll error: {}", fake_url))
            .build(),
    );
    assert!(
        built.message.contains("/bot***/getUpdates"),
        "expected redacted shape; got: {}",
        built.message
    );
    assert!(
        !built.message.contains("FAKE_TOKEN_FOR_TESTING"),
        "token leaked into message: {}",
        built.message
    );
}

/// `ErrorLogEntry` crosses the Tauri IPC boundary, so the
/// `#[serde(rename_all = "camelCase")]` rename is part of the contract with
/// `src/shared/types.ts`. The four field names are single-word today, so the
/// rename is a no-op — this test guards the contract against a future
/// field rename.
#[test]
fn error_log_entry_serializes_camel_case() {
    let json = serde_json::to_string(&entry("boom")).expect("serialize");
    assert!(
        json.contains("\"timestamp\""),
        "missing timestamp: {}",
        json
    );
    assert!(json.contains("\"level\""), "missing level: {}", json);
    assert!(json.contains("\"target\""), "missing target: {}", json);
    assert!(json.contains("\"message\""), "missing message: {}", json);
    assert!(json.contains("\"typeId\""), "missing typeId: {}", json);
    assert!(!json.contains("type_id"), "snake_case leaked: {}", json);
}

// ── #3036 error-popup mute tests ────────────────────────────────

#[test]
fn type_id_groups_by_emission_site_only() {
    let t = "agentscommander_lib::commands::x";
    let a1 = type_of(t, Some("src/x.rs"), Some(10), "failed C:/a 1111-uuid");
    let a2 = type_of(t, Some("src/x.rs"), Some(10), "other D:/b 2222-uuid");
    assert_eq!(a1, a2);
    assert_eq!(a1, "v1|32:agentscommander_lib::commands::x|s8:src/x.rs|u10");
    // Same target, another site => distinct type.
    assert_ne!(a1, type_of(t, Some("src/x.rs"), Some(11), "failed"));
    // None vs Some("") and line None vs 0 never collide.
    assert_ne!(type_of(t, None, None, "m"), type_of(t, Some(""), None, "m"));
    assert_ne!(type_of(t, None, None, "m"), type_of(t, None, Some(0), "m"));
    assert_eq!(
        type_of(t, None, None, "m"),
        format!("v1|{}:{t}|n|n", t.len())
    );
    // Length prefixes keep separator-bearing parts unambiguous.
    assert_ne!(
        type_of("a|s1:b", None, None, "m"),
        type_of("a", Some("b|n"), None, "m")
    );
    // An aggregator site emitting causes A and B shares one type by design.
    let agg = |cause: &str| type_of(t, Some("src/agg.rs"), Some(7), cause);
    assert_eq!(agg("cause A"), agg("cause B"));
}

#[test]
fn mute_skips_only_new_inserts_and_keeps_queued() {
    let sink = ErrorEventSink::new();
    sink.capture(typed("A", "a-old"));
    assert_eq!(sink.set_muted("A", true), Ok(vec!["A".to_string()]));
    sink.capture(typed("A", "a-new"));
    sink.capture(typed("B", "b"));
    assert_eq!(messages(sink.drain()), ["a-old", "b"]);
}

#[test]
fn mute_is_idempotent_and_unmute_has_no_replay() {
    let sink = ErrorEventSink::new();
    sink.capture(typed("A", "a0"));
    sink.capture(typed("B", "b0"));
    let _ = sink.drain();
    assert_eq!(sink.set_muted("A", true), Ok(vec!["A".to_string()]));
    assert_eq!(sink.set_muted("A", true), Ok(vec!["A".to_string()]));
    sink.capture(typed("A", "skipped"));
    assert_eq!(sink.muted_snapshot(), ["A"]);
    assert_eq!(sink.set_muted("A", false), Ok(vec![]));
    assert_eq!(sink.set_muted("A", false), Ok(vec![]));
    sink.capture(typed("A", "a-future"));
    assert_eq!(messages(sink.drain()), ["a-future"]);
    assert!(sink.muted_snapshot().is_empty());
}

#[test]
fn new_sink_starts_unmuted_and_keeps_cap_and_poison_recovery() {
    let sink = ErrorEventSink::new();
    assert!(sink.muted_snapshot().is_empty());
    for i in 0..ERROR_BUFFER_CAP + 3 {
        sink.capture(typed("A", &i.to_string()));
    }
    let drained = sink.drain();
    assert_eq!(drained.len(), ERROR_BUFFER_CAP);
    assert_eq!(drained[0].message, "3");
    assert!(sink.drain().is_empty());
    // Poison the lock; capture/drain/set still recover.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = sink.state.lock().unwrap();
        panic!("poison");
    }));
    assert!(sink.state.is_poisoned());
    sink.capture(typed("A", "after-poison"));
    assert_eq!(messages(sink.drain()), ["after-poison"]);
    assert_eq!(sink.set_muted("A", true), Ok(vec!["A".to_string()]));
}

#[test]
fn unknown_type_is_rejected_without_mutation() {
    let sink = ErrorEventSink::new();
    sink.capture(typed("A", "a"));
    assert_eq!(sink.set_muted("A", true), Ok(vec!["A".to_string()]));
    assert_eq!(
        sink.set_muted("Z", true),
        Err("Unknown error popup type".to_string())
    );
    assert_eq!(
        sink.set_muted("Z", false),
        Err("Unknown error popup type".to_string())
    );
    assert_eq!(sink.muted_snapshot(), ["A"]);
    assert_eq!(messages(sink.drain()), ["a"]);
}

/// Captures racing the mute on other threads: barriers/channels order them
/// strictly before or after the set; the late drain shows the exact
/// insertion boundary (no sleeps).
#[test]
fn concurrent_captures_respect_set_boundary() {
    let sink = ErrorEventSink::new();
    sink.capture(typed("A", "seed"));
    let _ = sink.drain();
    let (before_tx, before_rx) = std::sync::mpsc::channel::<()>();
    let set_done = std::sync::Barrier::new(3);
    std::thread::scope(|s| {
        for name in ["t1", "t2"] {
            let sink = &sink;
            let before_tx = before_tx.clone();
            let set_done = &set_done;
            s.spawn(move || {
                sink.capture(typed("A", &format!("{name}-a-before")));
                before_tx.send(()).unwrap();
                set_done.wait();
                sink.capture(typed("A", &format!("{name}-a-after")));
                sink.capture(typed("B", &format!("{name}-b-after")));
            });
        }
        before_rx.recv().unwrap();
        before_rx.recv().unwrap();
        assert_eq!(sink.set_muted("A", true), Ok(vec!["A".to_string()]));
        set_done.wait();
    });
    let mut got = messages(sink.drain());
    got.sort();
    assert_eq!(
        got,
        ["t1-a-before", "t1-b-after", "t2-a-before", "t2-b-after"]
    );
}

/// Unmutes the global type on drop (also during unwind); log-free.
struct UnmuteGuard(String);

impl Drop for UnmuteGuard {
    fn drop(&mut self) {
        let _ = error_sink().set_muted(&self.0, false);
    }
}

fn issue_3083_record(seq: u64) -> serde_json::Value {
    serde_json::json!({"version":1,"windowId":"fedcba98-7654-4321-8abc-012345678901",
        "seq":seq,"atMs":0,"surface":"embedded","event":"pointerdown"})
}

#[test]
fn issue_3083_real_file_gate_format_append_and_private_rejection() {
    for level in [log::Level::Info, log::Level::Debug, log::Level::Trace] {
        let tmp = tempfile::tempdir().expect("temp log directory");
        {
            let mut builder = env_logger::Builder::new();
            builder.parse_filters(GATE_INNER_FILTER);
            install_format(&mut builder, Arc::new(Some(build_app_log(tmp.path(), 0))));
            let gate = LevelGateLogger {
                inner: builder.build(),
                level: Arc::new(AtomicU8::new(level as u8)),
            };
            for seq in [1, 2] {
                assert_eq!(
                    crate::commands::config::append_rail_diagnostic_to(
                        Some(issue_3083_record(seq)),
                        &gate
                    ),
                    Ok(())
                );
            }
            assert_eq!(
                crate::commands::config::append_rail_diagnostic_to(
                    Some(serde_json::json!({"secret":"PRIVATE_3083"})),
                    &gate
                ),
                Err("invalid rail diagnostic".into())
            );
        }
        let bytes = std::fs::read_to_string(tmp.path().join("app.log")).expect("real bytes");
        assert!(!bytes.contains("PRIVATE_3083"));
        if level == log::Level::Info {
            assert!(bytes.is_empty(), "Info admitted diagnostic: {bytes}");
            continue;
        }
        assert_eq!(bytes.matches("[rail-diag]").count(), 2, "{bytes}");
        assert_eq!(bytes.lines().count(), 2, "{bytes}");
        for (line, seq) in bytes.lines().zip([1, 2]) {
            assert!(line.contains("[DEBUG] agentscommander_lib::commands::config"));
            let (_, json) = line.split_once("[rail-diag]").expect("diagnostic prefix");
            let value: serde_json::Value = serde_json::from_str(json).expect("compact JSON");
            assert_eq!(value["windowId"], "fedcba98-7654-4321-8abc-012345678901");
            assert_eq!(value["seq"], seq);
            assert_eq!(value["event"], "pointerdown");
            assert_eq!(serde_json::to_string(&value).unwrap().len(), json.len());
        }
    }
}

#[test]
fn issue_3083_real_file_rotation_and_continuity() {
    let tmp = tempfile::tempdir().expect("temp log directory");
    {
        let mut builder = env_logger::Builder::new();
        builder.parse_filters(GATE_INNER_FILTER);
        install_format(
            &mut builder,
            Arc::new(Some(build_app_log(tmp.path(), APP_LOG_MAX_BYTES - 1))),
        );
        let gate = LevelGateLogger {
            inner: builder.build(),
            level: Arc::new(AtomicU8::new(log::Level::Debug as u8)),
        };
        for seq in [3083, 3084] {
            crate::commands::config::append_rail_diagnostic_to(Some(issue_3083_record(seq)), &gate)
                .expect("dispatch");
        }
    }
    let active = std::fs::read_to_string(tmp.path().join("app.log")).expect("active file");
    let rotated = std::fs::read_to_string(tmp.path().join("app.log.1")).expect("rotated file");
    assert!(rotated.contains("\"seq\":3083"), "{rotated}");
    assert!(active.contains("\"seq\":3084"), "{active}");
    let mut all = active;
    for n in 1..=5 {
        all.push_str(
            &std::fs::read_to_string(tmp.path().join(format!("app.log.{n}"))).unwrap_or_default(),
        );
    }
    for seq in [3083, 3084] {
        assert_eq!(all.matches(&format!("\"seq\":{seq}")).count(), 1, "{all}");
    }
    assert_eq!(all.matches("[rail-diag]").count(), 2);
}

#[test]
fn muted_type_is_skipped_by_real_logger_but_still_written_and_redacted() {
    let _drain_guard = ERROR_SINK_TEST_DRAIN_LOCK.blocking_lock();
    const TARGET: &str = "agentscommander_lib::tests::mute_3034_logger";
    const FILE: &str = "mute_3034_logger_fixture";
    let id = error_type_id(TARGET, Some(FILE), Some(3034));
    let tmp = tempfile::tempdir().expect("tmp");
    {
        let mut builder = env_logger::Builder::new();
        builder.parse_filters(GATE_INNER_FILTER);
        install_format(&mut builder, Arc::new(Some(build_app_log(tmp.path(), 0))));
        let gate = LevelGateLogger {
            inner: builder.build(),
            level: Arc::new(AtomicU8::new(log::Level::Error as u8)),
        };
        let emit = |msg: &str| {
            gate.log(
                &log::Record::builder()
                    .level(log::Level::Error)
                    .target(TARGET)
                    .file(Some(FILE))
                    .line(Some(3034))
                    .args(format_args!("{msg}"))
                    .build(),
            );
        };
        // Register A as known, then drain that entry before measuring.
        emit("mute-3034 prior capture");
        let prior = error_sink().drain();
        assert!(prior.iter().any(|e| e.type_id == id), "prior={prior:?}");

        let unmute = UnmuteGuard(id.clone());
        assert!(error_sink().set_muted(&id, true).is_ok());
        emit(
            "MUTE_3034_MARKER https://api.telegram.org/\
             bot123456789:MUTE_3034_SECRET_xxxxxxxxxxxxxx/getMe",
        );
        let after = error_sink().drain();
        assert!(!after.iter().any(|e| e.type_id == id), "after={after:?}");
        drop(unmute);
        drop(gate);
    }
    let log = std::fs::read_to_string(tmp.path().join("app.log")).expect("read app.log");
    assert!(log.contains("MUTE_3034_MARKER"), "log={log}");
    assert!(!log.contains("MUTE_3034_SECRET"), "secret leaked: {log}");
    assert!(log.contains("/bot***/getMe"), "log={log}");
}

#[test]
fn sink_capture_then_drain_is_read_and_clear() {
    let sink = ErrorEventSink::new();
    sink.capture(entry("only"));
    let first = sink.drain();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].message, "only");
    // A second drain sees nothing — drain is read-and-clear.
    assert!(sink.drain().is_empty());
}

#[test]
fn sink_drain_preserves_fifo_order() {
    let sink = ErrorEventSink::new();
    sink.capture(entry("first"));
    sink.capture(entry("second"));
    sink.capture(entry("third"));
    let messages: Vec<String> = sink.drain().into_iter().map(|e| e.message).collect();
    assert_eq!(messages, ["first", "second", "third"]);
}

#[test]
fn sink_drops_oldest_entries_past_the_cap() {
    let sink = ErrorEventSink::new();
    let overflow = 5usize;
    for i in 0..ERROR_BUFFER_CAP + overflow {
        sink.capture(entry(&i.to_string()));
    }
    let drained = sink.drain();
    // The buffer never grows past the cap.
    assert_eq!(drained.len(), ERROR_BUFFER_CAP);
    // The oldest `overflow` entries (0..5) were evicted; the surviving
    // window starts at `overflow` and stays FIFO-ordered.
    assert_eq!(drained[0].message, overflow.to_string());
    assert_eq!(
        drained[ERROR_BUFFER_CAP - 1].message,
        (ERROR_BUFFER_CAP + overflow - 1).to_string()
    );
}

// ── #280 §2 — app.log rotation tests ──────────────────────────────

/// Build a synthetic `AppLogFile` rooted at `dir`. `bytes` is preloaded
/// so the tests can drive `rotate()` directly without needing to write
/// `APP_LOG_MAX_BYTES` worth of data first.
fn build_app_log(dir: &std::path::Path, bytes: u64) -> Arc<AppLogFile> {
    let path = dir.join("app.log");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("open active log");
    Arc::new(AppLogFile {
        file: Mutex::new(f),
        bytes: AtomicU64::new(bytes),
        path,
    })
}

/// Seed `app.log.<i>` with the given marker content so a successful
/// rotation can be observed by reading back which marker now lives
/// where.
fn seed_numbered(dir: &std::path::Path, i: u32, marker: &str) {
    let p = dir.join(format!("app.log.{i}"));
    std::fs::write(p, marker).expect("seed numbered");
}

fn read_marker(dir: &std::path::Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok()
}

#[test]
fn rotate_renames_active_to_dot_1_and_creates_fresh() {
    let tmp = tempfile::tempdir().expect("tmp");
    let state = build_app_log(tmp.path(), APP_LOG_MAX_BYTES);
    // Put a marker in the active file so we can verify it moved to .1.
    std::fs::write(&state.path, b"ACTIVE").expect("seed active");

    rotate(&state);

    assert_eq!(
        read_marker(tmp.path(), "app.log.1").as_deref(),
        Some("ACTIVE")
    );
    // Fresh active file exists and is empty.
    let active = std::fs::read(tmp.path().join("app.log")).expect("read active");
    assert!(
        active.is_empty(),
        "active file should be empty after rotate"
    );
    // Counter reset.
    assert_eq!(state.bytes.load(Ordering::Relaxed), 0);
}

#[test]
fn rotate_shifts_existing_numbered_files() {
    let tmp = tempfile::tempdir().expect("tmp");
    let state = build_app_log(tmp.path(), APP_LOG_MAX_BYTES);
    std::fs::write(&state.path, b"ACTIVE").expect("seed active");
    seed_numbered(tmp.path(), 1, "ONE");
    seed_numbered(tmp.path(), 2, "TWO");

    rotate(&state);

    assert_eq!(
        read_marker(tmp.path(), "app.log.1").as_deref(),
        Some("ACTIVE")
    );
    assert_eq!(read_marker(tmp.path(), "app.log.2").as_deref(), Some("ONE"));
    assert_eq!(read_marker(tmp.path(), "app.log.3").as_deref(), Some("TWO"));
}

/// G-HIGH-1 — the rotation walks `(KEEP - 1) ..= 1`, so the highest
/// rename is `.<KEEP - 1> → .<KEEP>`. The old `.<KEEP>` is evicted
/// atomically. No `.<KEEP + 1>` is ever created.
#[test]
fn rotate_caps_total_file_count_at_keep_plus_one() {
    let tmp = tempfile::tempdir().expect("tmp");
    let state = build_app_log(tmp.path(), APP_LOG_MAX_BYTES);
    std::fs::write(&state.path, b"ACTIVE").expect("seed active");
    // Pre-seed every retention slot.
    for i in 1..=APP_LOG_KEEP {
        seed_numbered(tmp.path(), i, &format!("DOT_{i}"));
    }

    rotate(&state);

    // After rotation: active + KEEP numbered files = KEEP + 1.
    let count = std::fs::read_dir(tmp.path())
        .expect("read dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name == "app.log" || name.starts_with("app.log.")
        })
        .count();
    assert_eq!(count, (APP_LOG_KEEP + 1) as usize);

    // `.<KEEP + 1>` must NOT exist.
    let overflow = tmp.path().join(format!("app.log.{}", APP_LOG_KEEP + 1));
    assert!(
        !overflow.exists(),
        "overflow file should not exist: {:?}",
        overflow
    );

    // Oldest retained slot (.<KEEP>) is the former .<KEEP - 1> content
    // (because we walked descending and renamed .<KEEP - 1> over the
    // prior .<KEEP>, evicting it).
    let oldest = read_marker(tmp.path(), &format!("app.log.{}", APP_LOG_KEEP));
    assert_eq!(
        oldest.as_deref(),
        Some(format!("DOT_{}", APP_LOG_KEEP - 1).as_str())
    );
}

/// G-MED-1 — under concurrent invocation, `rotate()` re-checks the
/// byte counter inside the lock. After the first rotation resets the
/// counter to 0, the second call must return early without shifting
/// the just-rotated files.
#[test]
fn rotate_is_idempotent_under_concurrent_invocations() {
    let tmp = tempfile::tempdir().expect("tmp");
    let state = build_app_log(tmp.path(), APP_LOG_MAX_BYTES);
    std::fs::write(&state.path, b"ACTIVE").expect("seed active");

    // First rotation: shifts ACTIVE → .1.
    rotate(&state);
    assert_eq!(
        read_marker(tmp.path(), "app.log.1").as_deref(),
        Some("ACTIVE")
    );
    assert_eq!(state.bytes.load(Ordering::Relaxed), 0);

    // Second rotation with bytes already reset: must be a no-op. If it
    // wasn't idempotent, the (now-empty) active file would shift to
    // .1, evicting the real backup.
    rotate(&state);
    assert_eq!(
        read_marker(tmp.path(), "app.log.1").as_deref(),
        Some("ACTIVE")
    );
    assert!(
        read_marker(tmp.path(), "app.log.2").is_none(),
        "no second-slot file should exist after idempotent second rotation"
    );
}

// ── #612 runtime log-level gate tests ───────────────────────────

/// Build the fixed inner `env_logger::Logger` for the gate WITHOUT reading
/// RUST_LOG (hermetic): `Builder::new()` + the exact production filter, so a
/// dev with RUST_LOG exported in their shell cannot perturb these tests.
fn gate_inner() -> env_logger::Logger {
    let mut builder = env_logger::Builder::new();
    builder.parse_filters(GATE_INNER_FILTER);
    builder.build()
}

/// A gate at `level` over a fresh hermetic inner logger.
fn gate_at(level: log::Level) -> LevelGateLogger {
    LevelGateLogger {
        inner: gate_inner(),
        level: Arc::new(AtomicU8::new(level as u8)),
    }
}

fn admits(gate: &LevelGateLogger, level: log::Level, target: &str) -> bool {
    gate.enabled(&log::Metadata::builder().level(level).target(target).build())
}

#[test]
fn level_from_str_maps_five_levels_and_falls_back_to_info() {
    assert_eq!(level_from_str("error"), log::Level::Error);
    assert_eq!(level_from_str("warn"), log::Level::Warn);
    assert_eq!(level_from_str("info"), log::Level::Info);
    assert_eq!(level_from_str("debug"), log::Level::Debug);
    assert_eq!(level_from_str("trace"), log::Level::Trace);
    // Case-insensitive + trimmed.
    assert_eq!(level_from_str("  INFO  "), log::Level::Info);
    assert_eq!(level_from_str("Debug"), log::Level::Debug);
    // None-equivalent / empty / legacy full-filter string / typo -> Info.
    assert_eq!(level_from_str(""), log::Level::Info);
    assert_eq!(
        level_from_str("info,agentscommander_lib::config::teams=debug"),
        log::Level::Info
    );
    assert_eq!(level_from_str("verbose"), log::Level::Info);
}

#[test]
fn normalize_log_level_accepts_valid_rejects_junk() {
    assert_eq!(normalize_log_level("error"), Some("error"));
    assert_eq!(normalize_log_level("WARN"), Some("warn"));
    assert_eq!(normalize_log_level(" info "), Some("info"));
    assert_eq!(normalize_log_level("debug"), Some("debug"));
    assert_eq!(normalize_log_level("trace"), Some("trace"));
    // Junk / empty / legacy full-filter string -> None (command rejects it).
    assert_eq!(normalize_log_level(""), None);
    assert_eq!(normalize_log_level("verbose"), None);
    assert_eq!(
        normalize_log_level("info,agentscommander_lib::config::teams=debug"),
        None
    );
}

#[test]
fn max_filter_for_clamps_to_at_least_warn() {
    // Error/Warn both floor at Warn so third-party warn! stays visible even
    // when the user selects Error for our own targets.
    assert_eq!(max_filter_for(log::Level::Error), log::LevelFilter::Warn);
    assert_eq!(max_filter_for(log::Level::Warn), log::LevelFilter::Warn);
    // Info/Debug/Trace map through unchanged.
    assert_eq!(max_filter_for(log::Level::Info), log::LevelFilter::Info);
    assert_eq!(max_filter_for(log::Level::Debug), log::LevelFilter::Debug);
    assert_eq!(max_filter_for(log::Level::Trace), log::LevelFilter::Trace);
}

#[test]
fn level_gate_at_info_suppresses_our_debug_keeps_thirdparty_warn() {
    let gate = gate_at(log::Level::Info);
    // Our own targets: trace/debug suppressed; info/warn/error admitted.
    assert!(!admits(&gate, log::Level::Trace, "agentscommander_lib::x"));
    assert!(!admits(&gate, log::Level::Debug, "agentscommander_lib::x"));
    assert!(admits(&gate, log::Level::Info, "agentscommander_lib::x"));
    assert!(admits(&gate, log::Level::Warn, "agentscommander_lib::x"));
    assert!(admits(&gate, log::Level::Error, "agentscommander_lib::x"));
    // Third-party: info suppressed, warn admitted (fixed warn filter).
    assert!(!admits(&gate, log::Level::Info, "hyper::client"));
    assert!(admits(&gate, log::Level::Warn, "hyper::client"));
}

#[test]
fn level_gate_at_error_admits_only_error_for_our_targets() {
    let gate = gate_at(log::Level::Error);
    assert!(admits(&gate, log::Level::Error, "agentscommander_lib::foo"));
    assert!(!admits(&gate, log::Level::Warn, "agentscommander_lib::foo"));
    assert!(!admits(&gate, log::Level::Info, "agentscommander_lib::foo"));
    // Third-party warn is independent of the atomic: still admitted at Error.
    assert!(admits(&gate, log::Level::Warn, "hyper::client"));
}

#[test]
fn level_gate_at_trace_admits_all_ours_but_not_thirdparty_trace() {
    let gate = gate_at(log::Level::Trace);
    for lvl in [
        log::Level::Error,
        log::Level::Warn,
        log::Level::Info,
        log::Level::Debug,
        log::Level::Trace,
    ] {
        assert!(
            admits(&gate, lvl, "agentscommander_lib::x"),
            "{lvl} should be admitted for our targets at Trace"
        );
    }
    // Third-party stays pinned at warn even when WE are at Trace.
    assert!(!admits(&gate, log::Level::Trace, "hyper::client"));
    assert!(!admits(&gate, log::Level::Debug, "hyper::client"));
    assert!(admits(&gate, log::Level::Warn, "hyper::client"));
}

#[test]
fn level_gate_atomic_flip_changes_verdict_at_runtime() {
    let atomic = Arc::new(AtomicU8::new(log::Level::Info as u8));
    let gate = LevelGateLogger {
        inner: gate_inner(),
        level: Arc::clone(&atomic),
    };
    // At Info, our debug is suppressed.
    assert!(!admits(&gate, log::Level::Debug, "agentscommander_lib::x"));
    // Flip the atomic (what set_runtime_log_level does) -> now admitted.
    atomic.store(log::Level::Debug as u8, Ordering::Relaxed);
    assert!(admits(&gate, log::Level::Debug, "agentscommander_lib::x"));
    // Flip back down to Error -> debug suppressed again.
    atomic.store(log::Level::Error as u8, Ordering::Relaxed);
    assert!(!admits(&gate, log::Level::Debug, "agentscommander_lib::x"));
}

/// R2 (grinch): the #264 error-sink now sits BEHIND the new gate plumbing.
/// Prove an ERROR record is NOT dropped by the gate at the STRICTEST
/// selectable level (Error) and still reaches `error_sink()` through the
/// reused format closure (`install_format`'s #264 tee). `error_sink()` is a
/// process-wide global, so assert our unique message was captured (a delta
/// check) rather than an absolute count, to stay robust under the parallel
/// test runner. This crate's only unit-test global-sink drainer is this
/// test, so the entry cannot be stolen between log() and drain().
#[test]
fn error_record_survives_gate_at_strictest_level_and_reaches_264_sink() {
    // #1883 — hold the shared sink-drain lock for the whole window: the
    // async mailbox test also read-and-clears this process-wide sink, so
    // drains must serialize. `blocking_lock()` is safe here because this
    // `#[test]` runs outside Tokio.
    let _drain_guard = ERROR_SINK_TEST_DRAIN_LOCK.blocking_lock();
    let mut builder = env_logger::Builder::new();
    builder.parse_filters(GATE_INNER_FILTER);
    // No app.log file: an empty log_state makes the closure's file branch a
    // no-op; the #264 tee path is independent of the file sink.
    install_format(&mut builder, Arc::new(None::<Arc<AppLogFile>>));
    let gate = LevelGateLogger {
        inner: builder.build(),
        level: Arc::new(AtomicU8::new(log::Level::Error as u8)),
    };

    // Clear pre-existing entries so the delta is clean, then push an ERROR
    // for one of our own targets through the gate at the strictest level.
    let _ = error_sink().drain();
    let needle = "synthetic #612 gate-264 probe";
    gate.log(
        &log::Record::builder()
            .level(log::Level::Error)
            .target("agentscommander_lib::commands::probe_612")
            .args(format_args!("{needle}"))
            .build(),
    );
    let captured = error_sink().drain();
    assert!(
        captured.iter().any(|e| e.message.contains(needle)),
        "ERROR did not reach the #264 sink through the gate at Error; captured={captured:?}"
    );
}
