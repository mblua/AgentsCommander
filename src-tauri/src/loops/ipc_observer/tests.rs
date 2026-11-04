use super::*;

/// An arbitrary but fixed wall-clock instant, standing in for the last
/// invoke the backend saw.
const L: u64 = 1_700_000_000_000;

/// Every intake test calls `ingest_with_marker`, never
/// `ingest_records`: in a cargo test binary `config_dir()` resolves under
/// `target/debug/deps`, so `ingest_records` would read, and could delete, a
/// real marker file there.
fn marker(silence_started_at_ms: u64, last_invoke_at_ms: u64) -> FreezeMarker {
    FreezeMarker {
        backend_started_at_ms: 1,
        silence_started_at_ms,
        last_invoke_at_ms,
        last_invoke_cmd: "switch_session".to_string(),
        probes_emitted: 3,
        silence_ended_at_ms: None,
        ended_by: None,
    }
}

fn record() -> BlackBoxRecord {
    BlackBoxRecord {
        v: 1,
        label: "main".to_string(),
        window_type: "main".to_string(),
        visible: true,
        ..Default::default()
    }
}

// 1
#[test]
fn classify_reports_healthy_for_a_live_record() {
    let mut r = record();
    r.started_at_ms = L - 600_000;
    r.written_at_ms = L;
    r.last_raf_at_ms = L - 1_000;
    assert_eq!(classify(&r, None), Verdict::Healthy);
}

// 2
#[test]
fn classify_reports_frame_stall_only_for_a_visible_window() {
    // `started_at_ms` must be at least 5 min before `written_at_ms`:
    // `no_episode` takes `raf_floor = last_raf_at_ms.max(started_at_ms)`, so
    // a fresh `started_at_ms` would swallow the stall and collapse both
    // assertions onto `Healthy`.
    let mut r = record();
    r.started_at_ms = L - 600_000;
    r.written_at_ms = L;
    r.last_raf_at_ms = L - 300_000;
    assert_eq!(classify(&r, None), Verdict::FrameStall);
    r.visible = false;
    assert_eq!(classify(&r, None), Verdict::Healthy);
}

// 3
#[test]
fn classify_reports_task_loop_stopped_when_the_record_died_with_the_channel() {
    let m = marker(L + 90_000, L);
    let mut r = record();
    r.started_at_ms = L - 600_000;
    r.written_at_ms = L;
    r.last_raf_at_ms = L;
    assert_eq!(classify(&r, Some(&m)), Verdict::TaskLoopStopped);
    r.written_at_ms = L + 4_000;
    assert_eq!(classify(&r, Some(&m)), Verdict::TaskLoopStopped);
    // The 20 s case moved with the constant (round 2: `Inconclusive`) and is
    // what phase 2's AC 6 actually produces.
    r.written_at_ms = L + 20_000;
    assert_eq!(classify(&r, Some(&m)), Verdict::TaskLoopStopped);
}

// 4
#[test]
fn classify_reports_send_path_broken_when_a_probe_arrived_during_the_episode() {
    let s = L + 90_000;
    let m = marker(s, L);
    let mut r = record();
    r.started_at_ms = L - 600_000;
    r.written_at_ms = L + 300_000;
    r.last_raf_at_ms = L + 300_000;
    r.probe_seq = Some(2);
    r.probe_at_ms = Some(s + 1_000);
    r.last_sent_at_ms = s + 2_000;
    assert_eq!(classify(&r, Some(&m)), Verdict::SendPathBroken);
}

// 5
#[test]
fn classify_reports_ipc_dead_both_ways_without_a_probe() {
    let s = L + 90_000;
    let m = marker(s, L);
    let mut r = record();
    r.started_at_ms = L - 600_000;
    r.written_at_ms = L + 300_000;
    r.last_raf_at_ms = L + 300_000;
    r.last_sent_at_ms = s + 2_000;
    r.probe_at_ms = None;
    assert_eq!(classify(&r, Some(&m)), Verdict::IpcDeadBothWays);
    // A stale probe from an earlier episode cannot forge a `SendPathBroken`.
    r.probe_at_ms = Some(s - 1_000);
    assert_eq!(classify(&r, Some(&m)), Verdict::IpcDeadBothWays);
}

// 6
#[test]
fn note_invoke_tracks_the_last_command_and_bounds_the_table() {
    let observer = IpcObserver::new();
    for i in 0..300 {
        observer.note_invoke(&format!("cmd_{i}"));
    }
    assert_eq!(observer.snapshot_for_tick(0).tracked, MAX_TRACKED_COMMANDS);
    assert_eq!(observer.last_invoke_snapshot().2, "cmd_299");
}

// 7
#[test]
fn ingest_skips_current_run_records_and_deletes_previous_run_records() {
    let observer = IpcObserver::new();
    let base = observer.backend_started_at_ms;
    let records = vec![
        StoredRecord {
            key: "ipcbb.old".to_string(),
            json: format!(
                r#"{{"v":1,"label":"main","startedAtMs":{}}}"#,
                base - 10_000
            ),
        },
        StoredRecord {
            key: "ipcbb.current".to_string(),
            json: format!(
                r#"{{"v":1,"label":"main","startedAtMs":{}}}"#,
                base + 10_000
            ),
        },
    ];
    let outcome = observer.ingest_with_marker(records, None);
    assert_eq!(outcome.delete_keys, vec!["ipcbb.old".to_string()]);
}

// 8
#[test]
fn ingest_deletes_unparseable_and_oversized_records() {
    let observer = IpcObserver::new();
    let records = vec![
        StoredRecord {
            key: "ipcbb.broken".to_string(),
            json: "{".to_string(),
        },
        StoredRecord {
            key: "ipcbb.huge".to_string(),
            json: "x".repeat(65 * 1024),
        },
    ];
    let outcome = observer.ingest_with_marker(records, None);
    assert!(outcome.delete_keys.contains(&"ipcbb.broken".to_string()));
    assert!(outcome.delete_keys.contains(&"ipcbb.huge".to_string()));
}

// 9
#[test]
fn black_box_record_parses_with_missing_and_unknown_fields() {
    let minimal: BlackBoxRecord = serde_json::from_str(r#"{"v":1,"label":"main"}"#).unwrap();
    assert_eq!(minimal.v, 1);
    assert_eq!(minimal.label, "main");
    // An older renderer sends no such key and must read as force-killed, not
    // as cleanly closed. Default and parse are separate assertions.
    assert!(!minimal.closed_cleanly);
    let future: BlackBoxRecord =
        serde_json::from_str(r#"{"v":1,"label":"main","futureField":42}"#).unwrap();
    assert_eq!(future.v, 1);
    let counted: BlackBoxRecord = serde_json::from_str(r#"{"overdueTotal":3}"#).unwrap();
    assert_eq!(counted.overdue_total, 3);
    let closed: BlackBoxRecord = serde_json::from_str(r#"{"closedCleanly":true}"#).unwrap();
    assert!(closed.closed_cleanly);
}

// 10
#[test]
fn freeze_marker_round_trips() {
    // A bare round-trip cannot see a renamed key; the key assertion can.
    let open = marker(L + 90_000, L);
    let text = serde_json::to_string(&open).unwrap();
    assert!(text.contains("silenceEndedAtMs"));
    assert_eq!(serde_json::from_str::<FreezeMarker>(&text).unwrap(), open);

    let mut closed = open.clone();
    closed.silence_ended_at_ms = Some(L + 100_000);
    closed.ended_by = Some("traffic".to_string());
    let text = serde_json::to_string(&closed).unwrap();
    assert!(text.contains("silenceEndedAtMs"));
    assert_eq!(serde_json::from_str::<FreezeMarker>(&text).unwrap(), closed);
}

// 11 (C2b)
#[test]
fn classify_prefers_the_probe_over_the_staleness_margin() {
    let s = L + 90_000;
    let m = marker(s, L);
    let mut r = record();
    r.started_at_ms = L - 600_000;
    // Inside round 1's 90-120 s band, where round 1 returned `TaskLoopStopped`.
    r.written_at_ms = L + 100_000;
    r.last_raf_at_ms = L + 100_000;
    r.probe_at_ms = Some(s + 1_000);
    r.last_sent_at_ms = s + 500;
    assert_eq!(classify(&r, Some(&m)), Verdict::SendPathBroken);
}

// 12 (C2a)
#[test]
fn classify_reports_ipc_dead_both_ways_inside_the_old_blind_band() {
    // The offset is now the (c) boundary, so it is pinned explicitly.
    let s = L + 90_000;
    let m = marker(s, L);
    let mut r = record();
    r.started_at_ms = L - 600_000;
    r.written_at_ms = L + 100_000;
    r.last_raf_at_ms = L + 100_000;
    r.probe_at_ms = None;
    r.last_sent_at_ms = s + 500;
    assert_eq!(classify(&r, Some(&m)), Verdict::IpcDeadBothWays);
}

// 13 (D3)
#[test]
fn classify_is_inconclusive_between_the_liveness_bound_and_the_silence_onset() {
    let s = L + 90_000;
    let m = marker(s, L);
    let mut r = record();
    r.started_at_ms = L - 600_000;
    r.written_at_ms = L + 50_000;
    r.last_raf_at_ms = L + 50_000;
    let verdict = classify(&r, Some(&m));
    assert_eq!(verdict, Verdict::Inconclusive);
    assert!(verdict.explanation().contains("30000"));
    // The runtime figure must still reach `app.log`.
    let block = record_block(&r, Some(&m), verdict).join("\n");
    assert!(block.contains("outlived_ms=50000"));

    let mut edge = r.clone();
    edge.written_at_ms = L + 29_000;
    assert_eq!(classify(&edge, Some(&m)), Verdict::TaskLoopStopped);
}

// 14 (C2c + D2 + E2 + F1 + H1)
#[test]
fn classify_keeps_a_recovered_episode_out_of_the_stale_arms_but_not_out_of_the_probe_arm() {
    let s = L + 90_000;
    // The end stamp must be pinned explicitly, not left unconstrained: the
    // corroborator reads it.
    let ended = s + 10_000;
    let mut m = marker(s, L);
    m.silence_ended_at_ms = Some(ended);
    m.ended_by = Some("traffic".to_string());

    // Every record below pins `written_at_ms` explicitly and carries a
    // `last_raf_at_ms` within `RAF_STALE_MS` of it, so no assertion can be
    // satisfied by a record that would have missed the arm anyway or that
    // lands on `FrameStall` instead of `Healthy`.
    let w = s + 60_000;
    let mut base = record();
    base.started_at_ms = L - 600_000;
    base.written_at_ms = w;
    base.last_raf_at_ms = w - 1_000;
    base.probe_seq = Some(2);
    base.probe_at_ms = Some(s + 1_000);
    base.last_sent_at_ms = s + 2_000;
    base.last_settled_at_ms = s - 1_000;
    base.overdue_total = 1;
    base.pending_total = 1;
    base.pending = vec![PendingEntry {
        id: 41_206,
        cmd: "get_active_session".to_string(),
        age_ms: 65_000,
        overdue: true,
    }];

    // Another window's traffic ended the process-wide silence; this window
    // received a probe inside the episode, and the call it still holds was
    // issued at `w - 65_000 = s - 5_000`, before the silence was even
    // declared, while nothing it sent had come back since the onset.
    assert_eq!(classify(&base, Some(&m)), Verdict::SendPathBroken);

    // With no outstanding call there is no per-window evidence left.
    let mut nothing_outstanding = base.clone();
    nothing_outstanding.overdue_total = 0;
    nothing_outstanding.pending_total = 0;
    nothing_outstanding.pending = Vec::new();
    assert_eq!(classify(&nothing_outstanding, Some(&m)), Verdict::Healthy);

    // (F1) The healthy window that merely held one slow, untimed call at
    // exit. The only assertion in the file that fails on the round-4
    // classifier and passes on the round-5 one.
    let mut kept_settling = base.clone();
    kept_settling.last_settled_at_ms = s + 1_000;
    assert_eq!(classify(&kept_settling, Some(&m)), Verdict::Healthy);

    // (H1) The only entry marked `overdue` was issued at
    // `w - 6_000 = s + 54_000`, 44 s AFTER traffic resumed. The leading
    // non-overdue dialog is the second guard: an implementation reading
    // `pending.first()` derives `s - 540_000`, clears the bound trivially
    // and returns `SendPathBroken`.
    let mut late_call = base.clone();
    late_call.pending_total = 2;
    late_call.pending = vec![
        PendingEntry {
            id: 5,
            cmd: "spec_board_pick_open".to_string(),
            age_ms: 600_000,
            overdue: false,
        },
        PendingEntry {
            id: 41_206,
            cmd: "get_active_session".to_string(),
            age_ms: 6_000,
            overdue: true,
        },
    ];
    assert_eq!(classify(&late_call, Some(&m)), Verdict::Healthy);

    // The window that froze SECOND: the derived instant `w - 55_000 =
    // s + 5_000` lands inside `(silence_started_at_ms, silence_ended_at_ms]`,
    // so this is what stops the bound from being tightened to the onset.
    let mut second_victim = base.clone();
    second_victim.pending[0].age_ms = 55_000;
    assert_eq!(classify(&second_victim, Some(&m)), Verdict::SendPathBroken);

    // Row (d) is still reachable while a closed marker sits on disk.
    let mut stalled = record();
    stalled.started_at_ms = w - 600_000;
    stalled.written_at_ms = w;
    stalled.last_raf_at_ms = w - 300_000;
    assert_eq!(classify(&stalled, Some(&m)), Verdict::FrameStall);

    // (D2) `ended_by: Some("shutdown")` is the opposite of `"traffic"`: the
    // silence never ended, so the episode is still an episode.
    let mut quit_while_silent = m.clone();
    quit_while_silent.ended_by = Some("shutdown".to_string());
    assert_eq!(
        classify(&base, Some(&quit_while_silent)),
        Verdict::SendPathBroken
    );
}

// 15 (C6b)
#[test]
fn ingest_reports_and_deletes_a_previous_run_marker_with_no_records() {
    let observer = IpcObserver::new();
    let mut previous = marker(L + 90_000, L);
    previous.backend_started_at_ms = observer.backend_started_at_ms - 1;
    let outcome = observer.ingest_with_marker(Vec::new(), Some(&previous));
    let text = outcome.lines.join("\n");
    assert!(text.contains("[ipc-blackbox] marker only:"));
    assert!(text.contains("NO VERDICT"));
    assert!(outcome.marker_reported);

    let mut current = previous.clone();
    current.backend_started_at_ms = observer.backend_started_at_ms;
    let outcome = observer.ingest_with_marker(Vec::new(), Some(&current));
    assert!(outcome.lines.is_empty());
    assert!(!outcome.marker_reported);
}

// 16
#[test]
fn summary_line_names_every_window_and_verdict() {
    let line = summary_line(&[
        ("main".to_string(), Verdict::SendPathBroken.tag()),
        ("spec-board".to_string(), Verdict::Healthy.tag()),
        ("watchers".to_string(), Verdict::FrameStall.tag()),
    ]);
    assert!(line.contains("SUMMARY 3 previous-run windows"));
    assert!(line.contains("main=b/send-path-broken"));
    assert!(line.contains("spec-board=healthy"));
    assert!(line.contains("watchers=d/frame-stall"));
}

// 17 (D1)
#[test]
fn classify_excludes_a_cleanly_closed_window_from_task_loop_stopped_and_inconclusive() {
    // The stale spec-board window: `last_invoke_at_ms` eight hours AFTER the
    // record's `written_at_ms`.
    let written = L;
    let last_invoke = written + 8 * 3_600_000;
    let s = last_invoke + 90_000;
    let m = marker(s, last_invoke);
    let mut r = record();
    r.started_at_ms = written - 600_000;
    r.written_at_ms = written;
    r.last_raf_at_ms = written;
    r.closed_cleanly = true;
    assert_eq!(classify(&r, Some(&m)), Verdict::Healthy);

    // The forged verdict reached by `saturating_sub` underflow.
    let mut killed = r.clone();
    killed.closed_cleanly = false;
    assert_eq!(classify(&killed, Some(&m)), Verdict::TaskLoopStopped);

    // The flag suppresses neither (b) nor (c). That third record's
    // `probe_at_ms` post-dates its own `written_at_ms` by about eight hours,
    // which is not a realizable state: it pins ARM ORDERING on a pure
    // function, it does not model a scenario.
    let mut probed = r.clone();
    probed.probe_at_ms = Some(s + 1_000);
    probed.last_sent_at_ms = s + 2_000;
    assert_eq!(classify(&probed, Some(&m)), Verdict::SendPathBroken);
}

// 18 (E1)
#[test]
fn classify_reports_bystander_when_a_probe_arrived_and_the_window_never_sent() {
    let s = L + 90_000;
    let m = marker(s, L);
    let mut r = record();
    r.started_at_ms = L - 600_000;
    r.written_at_ms = L + 50_000;
    r.last_raf_at_ms = L + 50_000;
    r.probe_at_ms = Some(s + 1_000);
    r.last_sent_at_ms = s - 60_000;
    assert_eq!(classify(&r, Some(&m)), Verdict::Bystander);

    // The send is the only thing separating the two.
    let mut sent = r.clone();
    sent.last_sent_at_ms = s + 1;
    assert_eq!(classify(&sent, Some(&m)), Verdict::SendPathBroken);

    // A recorded probe falsifies "recorded none" for that window, so it must
    // never fall into the (c) arm.
    let mut late = r.clone();
    late.written_at_ms = s + 60_000;
    late.last_raf_at_ms = s + 60_000;
    assert_eq!(classify(&late, Some(&m)), Verdict::Bystander);
}

struct MarkerRestore {
    path: std::path::PathBuf,
    original: Option<Vec<u8>>,
}
impl Drop for MarkerRestore {
    fn drop(&mut self) {
        let result = match &self.original {
            Some(bytes) => std::fs::write(&self.path, bytes),
            None => match std::fs::remove_file(&self.path) {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                result => result,
            },
        };
        if let Err(err) = result {
            eprintln!("failed to restore test marker: {err}");
            if !std::thread::panicking() {
                panic!("failed to restore test marker: {err}");
            }
        }
    }
}

fn isolated_marker_restore() -> MarkerRestore {
    let executable = std::env::current_exe().unwrap();
    let executable_tree = executable.parent().unwrap().canonicalize().unwrap();
    assert_eq!(executable_tree.file_name().unwrap(), "deps");
    let path = marker_path().expect("isolated test marker path");
    assert!(path.starts_with(executable.parent().unwrap()));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    assert!(path
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .starts_with(&executable_tree));
    let original = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => panic!("cannot save pre-existing marker: {err}"),
    };
    let restore = MarkerRestore { path, original };
    match std::fs::remove_file(&restore.path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("cannot clear isolated marker: {err}"),
    }
    restore
}

#[test]
fn issue_3260_marker_probe_shutdown_order() {
    type ProbeObservation = (u32, u64, FreezeMarker, Option<FreezeMarker>);

    struct ProbeRecording {
        observer: Arc<IpcObserver>,
        observations: Mutex<Vec<ProbeObservation>>,
    }
    impl IpcProbeEvents for ProbeRecording {
        fn probe(&self, seq: u32, backend_now_ms: u64) {
            let disk = read_marker().expect("marker exists at probe time");
            let open = self.observer.snapshot_for_tick(backend_now_ms).open_episode;
            self.observations
                .lock()
                .unwrap()
                .push((seq, backend_now_ms, disk, open));
        }
    }

    let _restore = isolated_marker_restore();
    let observer = IpcObserver::new();
    observer.total_invokes.store(1, Ordering::Relaxed);
    observer.last_invoke_at_ms.store(1, Ordering::Relaxed);
    let recording = ProbeRecording {
        observer: Arc::clone(&observer),
        observations: Mutex::new(Vec::new()),
    };
    let mut warned = false;
    tick(&recording, &observer, &mut warned);
    assert!(!warned);
    let first = recording.observations.lock().unwrap()[0].clone();
    assert_eq!(first.0, 1);
    assert_eq!(first.2.probes_emitted, 1);
    assert_eq!(first.2.silence_started_at_ms, first.1);
    assert!(first.3.is_none(), "Open must follow the first probe");
    assert_eq!(
        observer.snapshot_for_tick(first.1).open_episode,
        Some(first.2.clone())
    );
    assert_eq!(read_marker(), Some(first.2.clone()));

    observer.inner.lock().unwrap().next_probe_at_ms = 0;
    tick(&recording, &observer, &mut warned);
    assert!(!warned);
    let second = recording.observations.lock().unwrap()[1].clone();
    assert_eq!(second.0, 2);
    assert_eq!(
        second.2, first.2,
        "repeat marker write must follow the probe"
    );
    assert_eq!(
        second.3,
        Some(first.2),
        "Probe mutation must follow the probe"
    );
    let open = observer.snapshot_for_tick(second.1).open_episode.unwrap();
    assert_eq!(open.probes_emitted, 2);
    assert_eq!(read_marker(), Some(open.clone()));

    on_shutdown(&observer, &mut warned);
    assert!(!warned);
    let closed = read_marker().unwrap();
    assert_eq!(closed.probes_emitted, 2);
    assert_eq!(closed.ended_by.as_deref(), Some("shutdown"));
    assert!(closed.silence_ended_at_ms.is_some());
    let mut expected = open;
    expected.ended_by = Some("shutdown".into());
    expected.silence_ended_at_ms = closed.silence_ended_at_ms;
    assert_eq!(closed, expected);
    assert!(observer.snapshot_for_tick(now_ms()).open_episode.is_none());
    assert_eq!(recording.observations.lock().unwrap().len(), 2);
}
