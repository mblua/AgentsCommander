use super::*;
use crate::pty::backend::SessionBackendKind;
use crate::session::session::SessionStatus;
use serde_json::json;
use std::cell::Cell;
use tempfile::TempDir;

mod reducer;
mod writer;

// ---------------------------------------------------------------------
// Probes. `read_rotated_records` is the single place that opens the
// rotated file, so a per-thread counter proves whether the scan touched
// it. Each `#[test]` owns its thread, so the count needs no
// synchronization and cannot be perturbed by a parallel test.
// ---------------------------------------------------------------------

thread_local! {
    static ROTATED_READS: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn note_rotated_read() {
    ROTATED_READS.with(|count| count.set(count.get() + 1));
}

fn reset_rotated_reads() {
    ROTATED_READS.with(|count| count.set(0));
}

fn rotated_reads() -> usize {
    ROTATED_READS.with(Cell::get)
}

/// Insert straight into the coalescer, bypassing the pruning that
/// [`locked_coalescer`] applies, so a test can plant an entry the emitter
/// would otherwise have dropped, and report what the map holds for `id`
/// without releasing the lock. Observing under the same guard is what makes
/// an already-stale seed checkable: a parallel test that takes the lock
/// between a write and a separate read would prune the entry first (#2212).
fn seed_coalescer(
    id: Uuid,
    stamped: Instant,
    wall: DateTime<Utc>,
    c: &ActivityCoalescer,
) -> Option<(Instant, DateTime<Utc>)> {
    let mut guard = coalescer(c)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    guard.insert(id, (stamped, wall));
    guard.get(&id).copied()
}

fn coalescer_entry(id: Uuid, c: &ActivityCoalescer) -> Option<(Instant, DateTime<Utc>)> {
    coalescer(c)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(&id)
        .copied()
}

/// Pure form: `None` when the requested instant is not representable —
/// on Windows when `seconds` exceeds host uptime, on Linux only on
/// integer overflow of the monotonic timespec.
fn checked_ago(now: Instant, seconds: u64) -> Option<Instant> {
    now.checked_sub(Duration::from_secs(seconds))
}

fn ago(seconds: u64) -> Instant {
    let now = Instant::now();
    checked_ago(now, seconds).unwrap_or_else(|| {
        panic!(
            "ago({seconds}) cannot be represented on this platform, so the seed \
             would land inside the coalescer window instead of outside it"
        )
    })
}

#[test]
fn ago_panics_instead_of_saturating_when_the_instant_cannot_exist() {
    // u64::MAX is the one input that is None on every platform, so the
    // None branch is reached deterministically regardless of host uptime.
    assert!(
        checked_ago(Instant::now(), u64::MAX).is_none(),
        "the unrepresentable case must be None, not a saturated Instant"
    );
    let panicked = std::panic::catch_unwind(|| ago(u64::MAX)).is_err();
    assert!(panicked, "ago must panic rather than saturate to now");
}

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

fn sample_session(id: Uuid, agent_kind: Option<CodingAgentKind>) -> Session {
    Session {
        id,
        name: "wg-14-dev-team/dev-rust".to_string(),
        shell: "claude".to_string(),
        shell_args: vec!["--dangerously-skip-permissions".to_string()],
        backend_kind: SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: Utc::now(),
        working_directory: "C:\\repos\\ac\\.ac\\wg-14-dev-team\\__agent_dev-rust".to_string(),
        status: SessionStatus::Running,
        waiting_for_input: false,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: None,
        agent_label: None,
        git_repos: Vec::new(),
        is_coordinator: false,
        is_root_agent: false,
        git_repos_gen: 0,
        agent_turn_armed: false,
        token: Uuid::new_v4(),
        agent_kind,
        requested_profile: None,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        resolved_claude_projects_dir: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

fn sample_snapshot(id: Uuid) -> WorkingSessionSnapshot {
    WorkingSessionSnapshot {
        id,
        name: "wg-14-dev-team/tech-lead".to_string(),
        cwd: "C:\\repos\\ac\\.ac\\wg-14-dev-team\\__agent_tech-lead".to_string(),
        agent_kind: Some(CodingAgentKind::Claude),
    }
}

fn json_of(record: &ActivityRecord) -> Value {
    serde_json::to_value(record).expect("record serializes")
}

fn lines_of(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn parsed_lines(path: &Path) -> Vec<Value> {
    lines_of(path)
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).expect("line parses as JSON"))
        .collect()
}

/// A file at exactly [`ACTIVITY_MAX_BYTES`], made of valid records plus a
/// trailing fragment so the byte count lands on the cap.
fn write_file_at_cap(path: &Path, run_id: &str) {
    let record = json!({
        "v": 1,
        "at": at_offset(0),
        "runId": run_id,
        "event": "app_alive",
        "workingSessionIds": [],
    });
    let line = format!("{record}\n");
    let cap = ACTIVITY_MAX_BYTES as usize;
    let mut content = String::with_capacity(cap + line.len());
    while content.len() + line.len() <= cap {
        content.push_str(&line);
    }
    while content.len() < cap {
        content.push('x');
    }
    std::fs::write(path, &content).expect("write a file at the cap");
    assert_eq!(
        std::fs::metadata(path).expect("metadata").len(),
        ACTIVITY_MAX_BYTES
    );
}

const BASE_AT: &str = "2026-07-26T00:00:00.000Z";

fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(BASE_AT)
        .expect("base timestamp")
        .with_timezone(&Utc)
}

fn at_offset(millis: i64) -> String {
    stamp(base_time() + chrono::Duration::milliseconds(millis))
}

// ---------------------------------------------------------------------
// §10.1 - schema, sink, rotation
// ---------------------------------------------------------------------

#[test]
fn append_at_writes_exactly_one_line_per_record() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(FILE_NAME);
    let s = sample_session(Uuid::new_v4(), None);
    append_at(&path, &[build_busy(s.id, &s, BusyReason::MarkBusy, &c)], &c);
    append_at(&path, &[build_idle(s.id, &s, IdleReason::MarkIdle, &c)], &c);
    assert_eq!(lines_of(&path).len(), 2);
}

#[test]
fn append_batch_writes_all_lines_in_one_open() {
    let c = ActivityCoalescer::new();
    // `append_batch` delegates to `append_at`, which opens the file once
    // before its write loop; the observable property is that every record
    // of the batch lands, in order.
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(FILE_NAME);
    let first = sample_session(Uuid::new_v4(), None);
    let second = sample_session(Uuid::new_v4(), None);
    let batch = vec![
        build_idle(first.id, &first, IdleReason::AppStop, &c),
        build_idle(second.id, &second, IdleReason::AppStop, &c),
        build_app_stop(true, 2),
    ];
    append_at(&path, &batch, &c);
    let events: Vec<String> = parsed_lines(&path)
        .iter()
        .map(|value| value["event"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(events, vec!["idle", "idle", "app_stop"]);
}

#[test]
fn every_written_line_parses_as_json_and_carries_v_at_run_id_event() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(FILE_NAME);
    let s = sample_session(Uuid::new_v4(), Some(CodingAgentKind::Claude));
    append_at(
        &path,
        &[
            build_busy(s.id, &s, BusyReason::SessionStart, &c),
            build_idle(s.id, &s, IdleReason::MarkIdle, &c),
            build_app_alive(vec![s.id]),
            build_app_stop(false, 0),
        ],
        &c,
    );
    for line in lines_of(&path) {
        let record = parse_record(&line);
        assert!(record.is_some(), "line lacks a required field: {line}");
    }
}

#[test]
fn timestamp_format_is_byte_identical_to_canonical_pty_timestamp() {
    for offset in [0_i64, 1, 999, 1_000, 86_399_999] {
        let now = base_time() + chrono::Duration::milliseconds(offset);
        assert_eq!(
            stamp(now),
            crate::phone::types::canonical_pty_timestamp(now)
        );
    }
    assert_eq!(
        stamp(base_time()),
        crate::phone::types::canonical_pty_timestamp(base_time())
    );
}

#[test]
fn rotation_shifts_suffixes_and_evicts_the_oldest() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(FILE_NAME);
    write_file_at_cap(&path, "live-run");
    for index in 1..=ACTIVITY_KEEP {
        std::fs::write(
            dir.path().join(format!("{FILE_NAME}.{index}")),
            format!("generation-{index}\n"),
        )
        .expect("seed a rotated generation");
    }

    assert!(rotate_if_needed(&path), "a file at the cap must rotate");

    let generation =
        |index: u32| std::fs::read_to_string(dir.path().join(format!("{FILE_NAME}.{index}"))).ok();
    assert!(!path.exists(), "the live file moved into the .1 slot");
    assert_eq!(generation(2).as_deref(), Some("generation-1\n"));
    assert_eq!(generation(3).as_deref(), Some("generation-2\n"));
    assert_eq!(generation(4).as_deref(), Some("generation-3\n"));
    assert_eq!(
        generation(ACTIVITY_KEEP + 1),
        None,
        "retention must not grow past ACTIVITY_KEEP"
    );
    let rotated = generation(1).expect(".1 holds the former live file");
    assert!(
        !rotated.starts_with("generation-"),
        ".1 must hold the former live file, not a marker"
    );
}

#[test]
fn rotation_is_skipped_below_the_cap() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(FILE_NAME);
    std::fs::write(&path, "well below the cap\n").expect("seed a small file");
    assert!(!rotate_if_needed(&path));
    assert!(path.exists());
    assert!(!dir.path().join(format!("{FILE_NAME}.1")).exists());
}

#[test]
fn metrics_is_re_emitted_as_the_first_record_of_a_freshly_rotated_file() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(FILE_NAME);
    write_file_at_cap(&path, "previous-run");
    let s = sample_session(Uuid::new_v4(), None);
    append_at(&path, &[build_busy(s.id, &s, BusyReason::MarkBusy, &c)], &c);

    let records = parsed_lines(&path);
    assert_eq!(
        records.len(),
        2,
        "the fresh file holds metrics plus the record"
    );
    assert_eq!(records[0]["event"], "metrics");
    assert_eq!(
        records[0]["metrics"]["unsupported"],
        json!(["intervalCount", "blockCount"])
    );
    assert_eq!(records[0]["v"], json!(SCHEMA_VERSION));
    assert_eq!(records[1]["event"], "busy");
}

// ---------------------------------------------------------------------
// §10.1 - the coalescing annotation
// ---------------------------------------------------------------------

#[test]
fn first_busy_for_a_session_has_continues_block_false_and_no_gap_ms() {
    let c = ActivityCoalescer::new();
    assert!(c.last_idle_at.get().is_none());
    let s = sample_session(Uuid::new_v4(), None);
    let record = json_of(&build_busy(s.id, &s, BusyReason::SessionStart, &c));
    assert_eq!(record["continuesBlock"], json!(false));
    assert!(
        record.get("gapMs").is_none(),
        "gapMs is present exactly when continuesBlock is true"
    );
}

#[test]
fn busy_within_the_window_has_continues_block_true_and_reports_gap_ms() {
    let c = ActivityCoalescer::new();
    let s = sample_session(Uuid::new_v4(), None);
    seed_coalescer(s.id, ago(8), Utc::now() - chrono::Duration::seconds(8), &c);
    let record = json_of(&build_busy(s.id, &s, BusyReason::MarkBusy, &c));
    assert_eq!(record["continuesBlock"], json!(true));
    let gap = record["gapMs"]
        .as_u64()
        .expect("gapMs accompanies a continuation");
    assert!(
        (7_500..=9_000).contains(&gap),
        "gapMs must report the Instant delta, got {gap}"
    );
}

#[test]
fn busy_after_the_window_has_continues_block_false_and_no_gap_ms() {
    let c = ActivityCoalescer::new();
    let s = sample_session(Uuid::new_v4(), None);
    seed_coalescer(
        s.id,
        ago(45),
        Utc::now() - chrono::Duration::seconds(45),
        &c,
    );
    let record = json_of(&build_busy(s.id, &s, BusyReason::MarkBusy, &c));
    assert_eq!(record["continuesBlock"], json!(false));
    assert!(record.get("gapMs").is_none());
}

#[test]
fn a_backwards_utc_jump_suppresses_the_continuation_even_when_instant_is_under_the_window() {
    let c = ActivityCoalescer::new();
    let s = sample_session(Uuid::new_v4(), None);
    // The Instant delta is a few seconds, but the wall clock moved
    // backwards after the idle was stamped, so the Utc delta is negative.
    seed_coalescer(
        s.id,
        ago(5),
        Utc::now() + chrono::Duration::seconds(3_600),
        &c,
    );
    let record = json_of(&build_busy(s.id, &s, BusyReason::MarkBusy, &c));
    assert_eq!(
        record["continuesBlock"],
        json!(false),
        "a clock jump may only suppress a continuation"
    );
    assert!(record.get("gapMs").is_none());
}

#[test]
fn coalescer_state_prunes_entries_older_than_the_window() {
    let c = ActivityCoalescer::new();
    let stale = Uuid::new_v4();
    assert!(
        seed_coalescer(
            stale,
            ago(120),
            Utc::now() - chrono::Duration::seconds(120),
            &c
        )
        .is_some(),
        "seeded"
    );
    // Positive control: an in-window entry planted the same way must survive
    // the same write, so a prune that dropped everything could not pass.
    let fresh = Uuid::new_v4();
    let fresh_entry = (ago(1), Utc::now() - chrono::Duration::seconds(1));
    assert!(
        seed_coalescer(fresh, fresh_entry.0, fresh_entry.1, &c).is_some(),
        "seeded"
    );

    let other = sample_session(Uuid::new_v4(), None);
    let _ = build_idle(other.id, &other, IdleReason::MarkIdle, &c);

    assert!(
        coalescer_entry(stale, &c).is_none(),
        "an entry past the window is dropped on the next write"
    );
    assert_eq!(
        coalescer_entry(fresh, &c),
        Some(fresh_entry),
        "an entry inside the window survives the same write"
    );
}

#[test]
fn build_idle_from_snapshot_does_not_read_or_write_the_coalescer() {
    let c = ActivityCoalescer::new();
    let id = Uuid::new_v4();
    let stamped = ago(3);
    let wall = Utc::now() - chrono::Duration::seconds(3);
    seed_coalescer(id, stamped, wall, &c);

    let record = json_of(&build_idle_from_snapshot(
        &sample_snapshot(id),
        IdleReason::AppStop,
    ));

    assert_eq!(
        coalescer_entry(id, &c),
        Some((stamped, wall)),
        "the shutdown constructor must not write last_idle_at"
    );
    assert!(record.get("continuesBlock").is_none());
    assert!(record.get("gapMs").is_none());
}

#[test]
fn build_idle_from_snapshot_omits_idle_threshold_ms() {
    let record = json_of(&build_idle_from_snapshot(
        &sample_snapshot(Uuid::new_v4()),
        IdleReason::AppStop,
    ));
    assert_eq!(record["reason"], "app_stop");
    assert!(
        record.get("idleThresholdMs").is_none(),
        "nothing waited out a threshold on the app_stop path"
    );
}

// ---------------------------------------------------------------------
// §10.1 - fail-soft behavior
// ---------------------------------------------------------------------

#[test]
fn append_with_no_sink_configured_is_a_no_op() {
    let c = ActivityCoalescer::new();
    // No test ever calls `init_run`, so the sink is never configured in a
    // test process and this holds for every unit test in the crate.
    let dir = TempDir::new().expect("temp dir");
    let s = sample_session(Uuid::new_v4(), None);
    append(build_busy(s.id, &s, BusyReason::MarkBusy, &c), &c);
    append_batch(&[build_idle(s.id, &s, IdleReason::MarkIdle, &c)], &c);
    assert!(
        !dir.path().join(FILE_NAME).exists(),
        "an unconfigured sink must write nothing"
    );
}

#[test]
fn init_run_disabled_stores_no_sink_and_writes_nothing() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    init_run(dir.path(), "run-disabled", false, &c);
    init_run(dir.path(), "run-disabled", false, &c);
    assert!(sink_path().is_none());
    assert!(run_id().is_empty());
    assert!(!dir.path().join(FILE_NAME).exists());
    append(build_app_alive(vec![]), &c);
    append_batch(&[build_app_stop(true, 0)], &c);
    assert!(
        !dir.path().join(FILE_NAME).exists(),
        "a disabled run must never create the activity file"
    );
}

#[test]
fn append_at_an_unwritable_path_does_not_panic_and_returns() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("does-not-exist").join(FILE_NAME);
    let s = sample_session(Uuid::new_v4(), None);
    append_at(&path, &[build_busy(s.id, &s, BusyReason::MarkBusy, &c)], &c);
    assert!(!path.exists());
}

#[test]
fn a_poisoned_coalescer_mutex_does_not_panic_a_later_build() {
    let c = ActivityCoalescer::new();
    let handle = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let _guard = coalescer(&c)
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                panic!("poison the coalescer mutex on purpose");
            })
            .join()
    });
    assert!(handle.is_err(), "the helper thread must have panicked");

    let s = sample_session(Uuid::new_v4(), None);
    let idle = json_of(&build_idle(s.id, &s, IdleReason::MarkIdle, &c));
    assert_eq!(idle["event"], "idle");
    let busy = json_of(&build_busy(s.id, &s, BusyReason::MarkBusy, &c));
    assert_eq!(busy["continuesBlock"], json!(true));
}

// ---------------------------------------------------------------------
// §10.1 - the startup scan
// ---------------------------------------------------------------------

fn record_line(run_id: &str, event: &str, millis: i64) -> String {
    format!(
        "{}\n",
        json!({ "v": 1, "at": at_offset(millis), "runId": run_id, "event": event })
    )
}

fn seed_live(dir: &TempDir, content: &str) {
    std::fs::write(dir.path().join(FILE_NAME), content).expect("seed the live file");
}

fn seed_rotated(dir: &TempDir, content: &str) {
    std::fs::write(dir.path().join(format!("{FILE_NAME}.1")), content)
        .expect("seed the rotated file");
}

fn scan(dir: &TempDir) -> PreviousRunReport {
    reset_rotated_reads();
    scan_previous_run_at(dir.path())
}

#[test]
fn scan_returns_clean_when_the_last_record_is_app_stop() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(
        &dir,
        &format!(
            "{}{}",
            record_line("run-a", "app_start", 0),
            record_line("run-a", "app_stop", 5_000)
        ),
    );
    let report = scan(&dir);
    assert_eq!(report.scan, PreviousRunScan::Ok);
    let previous = report.previous_run.expect("a previous run");
    assert_eq!(previous.run_id, "run-a");
    assert!(previous.clean);
    assert!(previous.anchor_seen);
    assert_eq!(previous.last_event, "app_stop");
    assert_eq!(previous.last_record_at, at_offset(5_000));
}

#[test]
fn scan_returns_clean_when_app_stop_is_followed_by_a_stray_busy() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(
        &dir,
        &format!(
            "{}{}{}",
            record_line("run-a", "app_start", 0),
            record_line("run-a", "app_stop", 5_000),
            record_line("run-a", "busy", 5_200)
        ),
    );
    let previous = scan(&dir).previous_run.expect("a previous run");
    assert!(
        previous.clean,
        "an app_stop anywhere in the region means clean"
    );
    assert_eq!(previous.last_event, "busy");
}

#[test]
fn scan_returns_clean_when_app_stop_is_followed_by_app_alive() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(
        &dir,
        &format!(
            "{}{}{}",
            record_line("run-a", "app_start", 0),
            record_line("run-a", "app_stop", 5_000),
            record_line("run-a", "app_alive", 5_400)
        ),
    );
    let previous = scan(&dir).previous_run.expect("a previous run");
    assert!(previous.clean);
    assert_eq!(previous.last_event, "app_alive");
}

#[test]
fn scan_returns_unclean_when_no_app_stop_appears_for_that_run() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(
        &dir,
        &format!(
            "{}{}",
            record_line("run-a", "app_start", 0),
            record_line("run-a", "app_alive", 60_000)
        ),
    );
    let report = scan(&dir);
    assert_eq!(report.scan, PreviousRunScan::Ok);
    let previous = report.previous_run.expect("a previous run");
    assert!(!previous.clean);
    assert!(previous.anchor_seen, "the app_start is the anchor");
    assert_eq!(previous.last_record_at, at_offset(60_000));
}

#[test]
fn scan_sets_anchor_seen_false_when_neither_file_holds_an_anchor() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(&dir, &record_line("run-a", "app_alive", 60_000));
    seed_rotated(&dir, &record_line("run-a", "busy", 30_000));
    let report = scan(&dir);
    let previous = report.previous_run.expect("a previous run");
    assert!(!previous.anchor_seen);
    assert!(!previous.clean);
    assert_eq!(
        report.scan,
        PreviousRunScan::Ok,
        "the rotated file supplied no answer"
    );
}

#[test]
fn scan_keeps_the_first_line_when_the_file_is_smaller_than_the_tail_window() {
    let dir = TempDir::new().expect("temp dir");
    // A single record: discarding the leading line unconditionally would
    // leave nothing to find.
    seed_live(&dir, &record_line("run-a", "app_stop", 0));
    let previous = scan(&dir).previous_run.expect("a previous run");
    assert_eq!(previous.run_id, "run-a");
    assert!(previous.clean);
}

#[test]
fn scan_survives_a_multibyte_character_at_the_tail_boundary() {
    let dir = TempDir::new().expect("temp dir");
    let tail = format!(
        "{}{}",
        record_line("run-a", "app_start", 0),
        record_line("run-a", "app_stop", 1_000)
    );
    // Grow a padding line of multibyte characters until the tail window
    // starts inside one of them.
    let mut padding = String::new();
    let content = loop {
        let filler = format!("{{\"pad\":\"{}\"}}\n", "é".repeat(2_000));
        while padding.len() < TAIL_READ_BYTES as usize {
            padding.push_str(&filler);
        }
        let content = format!("{padding}{tail}");
        let offset = content.len() - TAIL_READ_BYTES as usize;
        if content.as_bytes()[offset] & 0xC0 == 0x80 {
            break content;
        }
        padding.push('x');
    };
    assert!(content.len() > TAIL_READ_BYTES as usize);
    seed_live(&dir, &content);

    let previous = scan(&dir).previous_run.expect("a previous run");
    assert_eq!(previous.run_id, "run-a");
    assert!(previous.clean, "a split codepoint must not break the scan");
}

#[test]
fn scan_takes_the_last_parseable_record_not_the_last_line() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(
        &dir,
        &format!(
            "{}{}{{\"v\":1,\"at\":\"2026-07-26T00:00:0",
            record_line("run-a", "app_start", 0),
            record_line("run-a", "app_alive", 2_000)
        ),
    );
    let previous = scan(&dir).previous_run.expect("a previous run");
    assert_eq!(previous.last_event, "app_alive");
    assert_eq!(previous.last_record_at, at_offset(2_000));
}

#[test]
fn scan_falls_back_to_the_rotated_file_when_the_live_tail_has_no_parseable_record() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(&dir, "{\"v\":1,\"at\":\"2026-07-26T00");
    seed_rotated(
        &dir,
        &format!(
            "{}{}",
            record_line("run-a", "app_start", 0),
            record_line("run-a", "app_alive", 9_000)
        ),
    );
    let report = scan(&dir);
    assert_eq!(report.scan, PreviousRunScan::RecoveredFromRotated);
    assert_eq!(rotated_reads(), 1);
    let previous = report.previous_run.expect("a previous run");
    assert_eq!(previous.last_event, "app_alive");
    assert!(!previous.clean);
    assert!(previous.anchor_seen);
}

#[test]
fn scan_falls_back_to_the_rotated_file_when_the_live_file_is_empty() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(&dir, "");
    seed_rotated(&dir, &record_line("run-a", "app_stop", 0));
    let report = scan(&dir);
    assert_eq!(report.scan, PreviousRunScan::RecoveredFromRotated);
    assert_eq!(rotated_reads(), 1);
    assert!(report.previous_run.expect("a previous run").clean);
}

#[test]
fn scan_falls_back_to_rotated_when_the_live_region_has_records_but_no_anchor_for_that_run() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(&dir, &record_line("run-a", "busy", 9_000));
    seed_rotated(&dir, &record_line("run-a", "app_start", 0));
    let report = scan(&dir);
    assert_eq!(rotated_reads(), 1, "the anchor clause must consult .1");
    assert_eq!(report.scan, PreviousRunScan::RecoveredFromRotated);
    assert!(report.previous_run.expect("a previous run").anchor_seen);
}

#[test]
fn scan_reports_clean_when_app_stop_is_in_the_rotated_file_and_a_teardown_busy_is_in_the_live_file()
{
    let dir = TempDir::new().expect("temp dir");
    seed_live(&dir, &record_line("run-a", "busy", 5_100));
    seed_rotated(
        &dir,
        &format!(
            "{}{}",
            record_line("run-a", "app_start", 0),
            record_line("run-a", "app_stop", 5_000)
        ),
    );
    let previous = scan(&dir).previous_run.expect("a previous run");
    assert!(
        previous.clean,
        "a rotation boundary must not turn a clean exit unclean"
    );
    assert_eq!(previous.last_event, "busy");
}

#[test]
fn scan_does_not_read_the_rotated_file_when_the_live_region_holds_the_app_stop() {
    let dir = TempDir::new().expect("temp dir");
    seed_live(
        &dir,
        &format!(
            "{}{}{}",
            record_line("run-a", "app_start", 0),
            record_line("run-a", "app_stop", 5_000),
            record_line("run-a", "app_alive", 5_500)
        ),
    );
    seed_rotated(&dir, &record_line("run-a", "app_start", -60_000));
    let report = scan(&dir);
    assert_eq!(
        rotated_reads(),
        0,
        "the clean case must short-circuit before opening .1"
    );
    assert!(report.previous_run.expect("a previous run").clean);
}

#[test]
fn scan_on_two_empty_files_reports_empty_and_null_previous_run() {
    let dir = TempDir::new().expect("temp dir");
    let report = scan(&dir);
    assert_eq!(report.scan, PreviousRunScan::Empty);
    assert_eq!(report.previous_run, None);

    seed_live(&dir, "");
    seed_rotated(&dir, "");
    let report = scan(&dir);
    assert_eq!(report.scan, PreviousRunScan::Empty);
    assert_eq!(report.previous_run, None);
}

// ---------------------------------------------------------------------
// §10.1 - init_run_at
// ---------------------------------------------------------------------

fn app_start_of(dir: &TempDir) -> Value {
    parsed_lines(&dir.path().join(FILE_NAME))
        .into_iter()
        .find(|value| value["event"] == "app_start")
        .expect("an app_start record")
}

#[test]
fn init_run_at_on_a_file_smaller_than_the_tail_window() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    seed_live(&dir, &record_line("run-a", "app_stop", 0));
    init_run_at(dir.path(), "run-b", &c);

    let start = app_start_of(&dir);
    assert_eq!(start["runId"], "run-b");
    assert_eq!(start["previousRunScan"], "ok");
    assert_eq!(start["previousRun"]["runId"], "run-a");
    assert_eq!(start["previousRun"]["clean"], json!(true));
    assert_eq!(start["appVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(start["pid"], json!(std::process::id()));
}

#[test]
fn init_run_at_on_a_tail_split_mid_utf8_character() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    let mut content = String::new();
    let filler = format!("{{\"pad\":\"{}\"}}\n", "ü".repeat(2_000));
    while content.len() < TAIL_READ_BYTES as usize + 1_024 {
        content.push_str(&filler);
    }
    content.push_str(&record_line("run-a", "app_stop", 0));
    seed_live(&dir, &content);

    init_run_at(dir.path(), "run-b", &c);
    let start = app_start_of(&dir);
    assert_eq!(start["previousRun"]["runId"], "run-a");
    assert_eq!(start["previousRun"]["clean"], json!(true));
}

#[test]
fn init_run_at_rotates_when_the_live_file_is_exactly_at_the_cap() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    write_file_at_cap(&dir.path().join(FILE_NAME), "run-a");

    init_run_at(dir.path(), "run-b", &c);

    assert!(dir.path().join(format!("{FILE_NAME}.1")).exists());
    let records = parsed_lines(&dir.path().join(FILE_NAME));
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["event"], "metrics");
    assert_eq!(records[1]["event"], "app_start");
    assert_eq!(
        records[1]["previousRun"]["runId"], "run-a",
        "the scan runs before the append, so the rotation cannot invalidate it"
    );
}

#[test]
fn init_run_at_on_a_read_only_directory_does_not_panic() {
    let c = ActivityCoalescer::new();
    // Windows ignores the read-only attribute on directories, so the
    // portable stand-ins for "this sink cannot be written" are a directory
    // that does not exist and a sink path that is itself a directory.
    let dir = TempDir::new().expect("temp dir");
    init_run_at(&dir.path().join("absent"), "run-b", &c);
    assert!(!dir.path().join("absent").exists());

    std::fs::create_dir(dir.path().join(FILE_NAME)).expect("occupy the sink path");
    init_run_at(dir.path(), "run-c", &c);
    assert!(dir.path().join(FILE_NAME).is_dir());
}

#[test]
fn init_run_at_reads_daemon_pid_from_its_argument_directory_not_the_live_config_dir() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    // PID 4 is the Windows System process: always present, and reported
    // alive even when `OpenProcess` is denied. The developer's real
    // `daemon.pid` never holds it, so observing it proves the argument
    // directory was the one read.
    std::fs::write(dir.path().join("daemon.pid"), "4").expect("seed daemon.pid");
    init_run_at(dir.path(), "run-b", &c);
    assert_eq!(app_start_of(&dir)["concurrentInstancePid"], json!(4));

    let own = TempDir::new().expect("temp dir");
    std::fs::write(
        own.path().join("daemon.pid"),
        std::process::id().to_string(),
    )
    .expect("seed daemon.pid");
    init_run_at(own.path(), "run-c", &c);
    assert!(
        app_start_of(&own).get("concurrentInstancePid").is_none(),
        "our own PID is not a concurrent instance"
    );
}

#[test]
fn app_start_always_declares_interval_count_unsupported() {
    let c = ActivityCoalescer::new();
    let dir = TempDir::new().expect("temp dir");
    init_run_at(dir.path(), "run-a", &c);
    let metrics = &app_start_of(&dir)["metrics"];
    assert_eq!(
        metrics["supported"],
        json!(["totalBusyMs", "totalBusyRawMs"])
    );
    assert_eq!(
        metrics["unsupported"],
        json!(["intervalCount", "blockCount"])
    );
}
