use super::*;
use crate::config::settings::{AppSettings, SettingsState};
use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use crate::pty::watchers::{FrameStamp, ScreenFrame, ScreenRowsSince};

/// One watcher row of the Settings draft. `preview_watcher_reach` takes the whole draft,
/// so every reach test below builds a set and never a single row.
fn draft(id: &str, enabled: bool, commands: Option<&[&str]>) -> WatcherDraftEntry {
    WatcherDraftEntry {
        id: id.to_string(),
        enabled,
        commands: commands.map(|list| list.iter().map(|s| s.to_string()).collect()),
    }
}

fn draft_agent(id: &str, label: &str, command: &str) -> WatcherAgentDraftEntry {
    WatcherAgentDraftEntry {
        id: id.to_string(),
        label: label.to_string(),
        command: command.to_string(),
    }
}

/// The row of `id` in a reach response, which is addressed by id and never by position.
fn row<'a>(rows: &'a [WatcherReachRow], id: &str) -> &'a WatcherReachRow {
    rows.iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("no reach row for '{id}'"))
}

fn allocated_on(rows: &[WatcherReachRow], id: &str, agent_id: &str) -> bool {
    row(rows, id)
        .entries
        .iter()
        .find(|entry| entry.agent_id == agent_id)
        .unwrap_or_else(|| panic!("watcher '{id}' does not reach agent '{agent_id}'"))
        .allocated
}

fn reached_agents(rows: &[WatcherReachRow], id: &str) -> Vec<String> {
    row(rows, id)
        .entries
        .iter()
        .map(|entry| entry.agent_id.clone())
        .collect()
}

fn settings_app(settings: AppSettings) -> tauri::App<tauri::test::MockRuntime> {
    let state: SettingsState = crate::config::settings::new_settings_state(settings);
    tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a watcher preview test app")
}

/// A backend that paints one fixed screen and refuses everything else.
struct FixedScreenBackend {
    rows: Vec<String>,
}

impl PtyBackend for FixedScreenBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(
        &self,
        _spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), AppError> {
        unreachable!("a preview must never write to a PTY")
    }
    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
        unreachable!("a preview must never resize a PTY")
    }
    fn kill(&self, _id: Uuid) -> Result<(), AppError> {
        unreachable!("a preview must never kill a session")
    }
    fn has_session(&self, _id: Uuid) -> bool {
        true
    }
    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }
    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }
    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::Unavailable
    }
    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: std::path::PathBuf,
    ) {
    }
    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
    fn screen_rows_since(&self, _id: Uuid, _seen: Option<FrameStamp>) -> ScreenRowsSince {
        ScreenRowsSince::Frame(ScreenFrame {
            rows: self.rows.clone(),
            wrapped: vec![false; self.rows.len()],
            cursor_row: 0,
            stamp: Some(FrameStamp {
                sequence: 1,
                rows: self.rows.len() as u16,
                cols: 120,
            }),
        })
    }
}

fn pty_app(rows: &[&str]) -> (tauri::App<tauri::test::MockRuntime>, Uuid) {
    let backend = Arc::new(FixedScreenBackend {
        rows: rows.iter().map(|r| r.to_string()).collect(),
    });
    let manager = PtyManager::new_for_test(backend);
    let id = Uuid::new_v4();
    manager.record_route(id, SessionBackendKind::LocalProcess);
    let app = tauri::test::mock_builder()
        .manage(Arc::new(Mutex::new(manager)))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a pty preview test app");
    (app, id)
}

/// 9.5.65 - a `session_id` that is not a UUID is an error, exactly like
/// `get_session_context`. Everything else about this command answers with values.
#[test]
fn a_session_id_that_is_not_a_uuid_is_rejected() {
    let app = settings_app(AppSettings::default());

    assert!(get_watcher_activity(app.handle().clone(), "nope".into(), None).is_err());
}

/// #2566 - no engine managed is the feature being off: an empty map, never a panic.
#[test]
fn get_agent_quota_readings_answers_an_empty_map_when_the_engine_is_unmanaged() {
    let app = settings_app(AppSettings::default());

    assert_eq!(
        get_agent_quota_readings(app.handle().clone()),
        Ok(std::collections::HashMap::new())
    );
}

/// #2566 - one session on `a1` fills every agent on its command, `a2` included.
#[test]
fn get_agent_quota_readings_returns_every_agent_on_the_command() {
    use crate::pty::agent_quota::source::SourceSpec;
    use crate::pty::agent_quota::test_support::QuotaHarness;
    use crate::pty::context_scrape::ScreenRowsRead;

    let spec = || SourceSpec::ScreenRegex {
        pattern: r"Weekly (\d{1,3})% used".to_string(),
    };
    let harness = QuotaHarness::new();
    harness.sources.configure_on("a1", "claude", spec());
    harness.sources.configure_on("a2", "claude", spec());
    let id = Uuid::new_v4();
    harness.engine.register_session(id, "a1".to_string());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&harness.engine))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");

    harness.rows.push(
        id,
        ScreenRowsRead::Rows(vec!["Weekly 42% used".to_string()]),
    );
    futures::executor::block_on(harness.engine.tick());

    let readings = get_agent_quota_readings(app.handle().clone()).expect("readings");
    assert_eq!(readings.get("a1"), Some(&Some(42)));
    assert_eq!(readings.get("a2"), Some(&Some(42)));
}

/// 9.5.64 - with no history managed at all - a test app, a build without the engine - the
/// command answers with the EMPTY snapshot rather than an error. `warmedUp: false` is what
/// tells the window it is looking at a session the engine has not reached, so it shows a
/// neutral starting state instead of "no watcher reaches this agent".
#[test]
fn an_unmanaged_history_answers_with_the_empty_snapshot() {
    let app = settings_app(AppSettings::default());

    let snapshot = get_watcher_activity(app.handle().clone(), Uuid::new_v4().to_string(), None)
        .expect("never an error");

    assert!(snapshot.matches.is_empty());
    assert!(!snapshot.warmed_up);
    assert!(!snapshot.truncated);
    assert_eq!(snapshot.last_seq, 0);
    assert_eq!(snapshot.possibly_missed_frames, 0);
    assert!(snapshot.active_watchers.is_empty());
}

/// 9.5.63 - the command hands `limit` straight to the ring, which trims from the NEW end
/// and keeps the order.
#[test]
fn the_command_reads_the_ring_and_honours_the_limit() {
    use crate::pty::watchers::history::{SessionStatus, WatcherHistory, WatcherHistoryState};
    use crate::pty::watchers::WatcherMatchPayload;

    let id = Uuid::new_v4();
    let history: WatcherHistoryState = Arc::new(WatcherHistory::default());
    history.publish(id, SessionStatus::default());
    for seq in 1..=5u64 {
        history.record(
            id,
            &[WatcherMatchPayload {
                session_id: id.to_string(),
                seq,
                watcher_id: "w".to_string(),
                mode: crate::pty::watchers::WatcherMode::Occurrence,
                at: chrono::Utc::now(),
                captures: Vec::new(),
                row: format!("row {seq}"),
                row_truncated: false,
            }],
        );
    }
    let app = tauri::test::mock_builder()
        .manage(history)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a watcher activity test app");

    let all = get_watcher_activity(app.handle().clone(), id.to_string(), None).expect("all");
    assert_eq!(all.matches.len(), 5);
    assert!(all.warmed_up);

    let recent =
        get_watcher_activity(app.handle().clone(), id.to_string(), Some(2)).expect("recent");
    assert_eq!(
        recent.matches.iter().map(|m| m.seq).collect::<Vec<_>>(),
        vec![4, 5]
    );
    assert_eq!(recent.last_seq, 5);
}

/// 9.6.86 (first case) - **the common case**: a user writes a regex in Settings with no
/// agent session running. It compiles and says so, and says explicitly that it did not
/// look at anything - which is not the same as having looked and found nothing.
#[tokio::test]
async fn a_compile_only_preview_reports_compiles_without_sampling() {
    let app = settings_app(AppSettings::default());

    let preview = preview_watcher_pattern(app.handle().clone(), None, r"Read \((.+)\)".into())
        .await
        .expect("compile-only preview never fails");

    assert!(preview.compiles);
    assert!(preview.error.is_none());
    assert!(!preview.sampled);
    assert_eq!((preview.matched_rows, preview.total_rows), (0, 0));
    assert!(preview.samples.is_empty());
    assert!(!preview.captures_volatile);
}

/// 9.6.86 (fourth case) - a pattern that does not compile is a RESULT, not a command
/// failure. The Settings control needs the message to show the user.
#[tokio::test]
async fn an_uncompilable_pattern_returns_a_result_rather_than_an_error() {
    let app = settings_app(AppSettings::default());

    let preview = preview_watcher_pattern(app.handle().clone(), None, r"Read \((.+".into())
        .await
        .expect("a bad pattern must not fail the command");

    assert!(!preview.compiles);
    assert!(preview.error.is_some());
    assert!(!preview.sampled);
}

/// 9.6.86 (second case) - with a live session, the preview reports matched rows against
/// total LOGICAL rows and shows at most three of them.
#[tokio::test]
async fn a_preview_against_a_live_session_reports_matches_against_total_rows() {
    let (app, id) = pty_app(&[
        "Read (a.rs)",
        "idle",
        "Read (b.rs)",
        "Read (c.rs)",
        "Read (d.rs)",
    ]);

    let preview = preview_watcher_pattern(
        app.handle().clone(),
        Some(id.to_string()),
        r"Read \((.+)\)".into(),
    )
    .await
    .expect("preview");

    assert!(preview.sampled);
    assert_eq!(preview.total_rows, 5);
    assert_eq!(preview.matched_rows, 4);
    assert_eq!(preview.samples.len(), WATCHER_PREVIEW_SAMPLES);
    assert_eq!(preview.samples[0], "Read (a.rs)");
    assert!(
        !preview.captures_volatile,
        "the same screen twice cannot be volatile"
    );
}

/// 9.6.87 - **the pattern that looks fine and is not.** A regex capturing a clock matches
/// one row of thirty, so `matchedRows` says nothing is wrong, and in `state` mode it emits
/// five events a second forever. The two samples a second apart are what catch it.
///
/// Takes about a second in real time by construction: the interval between the samples IS
/// the measurement.
#[tokio::test]
async fn a_pattern_capturing_a_clock_is_reported_as_volatile() {
    struct TickingClockBackend {
        reads: std::sync::atomic::AtomicUsize,
    }

    impl PtyBackend for TickingClockBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn spawn(
            &self,
            _spec: BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
            Box::pin(async { Ok(()) })
        }
        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            _id: Uuid,
            _data: &[u8],
        ) -> Result<(), AppError> {
            unreachable!("a preview must never write to a PTY")
        }
        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
            unreachable!("a preview must never resize a PTY")
        }
        fn kill(&self, _id: Uuid) -> Result<(), AppError> {
            unreachable!("a preview must never kill a session")
        }
        fn has_session(&self, _id: Uuid) -> bool {
            true
        }
        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }
        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }
        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::Unavailable
        }
        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }
        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }
        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
        fn screen_rows_since(&self, _id: Uuid, _seen: Option<FrameStamp>) -> ScreenRowsSince {
            let tick = self
                .reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let rows = vec!["idle".to_string(), format!("elapsed 00:0{tick}")];
            ScreenRowsSince::Frame(ScreenFrame {
                wrapped: vec![false; rows.len()],
                cursor_row: 0,
                stamp: Some(FrameStamp {
                    sequence: tick as u64 + 1,
                    rows: rows.len() as u16,
                    cols: 120,
                }),
                rows,
            })
        }
    }

    let backend = Arc::new(TickingClockBackend {
        reads: std::sync::atomic::AtomicUsize::new(0),
    });
    let manager = PtyManager::new_for_test(backend);
    let id = Uuid::new_v4();
    manager.record_route(id, SessionBackendKind::LocalProcess);
    let app = tauri::test::mock_builder()
        .manage(Arc::new(Mutex::new(manager)))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a clock preview test app");

    let preview = preview_watcher_pattern(
        app.handle().clone(),
        Some(id.to_string()),
        r"elapsed (\d\d:\d\d)".into(),
    )
    .await
    .expect("preview");

    assert!(preview.sampled);
    assert_eq!(
        preview.matched_rows, 1,
        "one row of two: nothing looks wrong"
    );
    assert!(
        preview.captures_volatile,
        "...and yet in state mode this would emit five events a second, forever"
    );
}

/// 9.6.86 (third case) - a session that cannot be read reports `sampled: false` and KEEPS
/// the compile result. "Could not look" must never read as "looked and found nothing".
#[tokio::test]
async fn a_session_that_cannot_be_read_keeps_the_compile_result_and_does_not_claim_a_sample() {
    let app = settings_app(AppSettings::default());

    let preview = preview_watcher_pattern(
        app.handle().clone(),
        Some(Uuid::new_v4().to_string()),
        "Read".into(),
    )
    .await
    .expect("preview");

    assert!(preview.compiles);
    assert!(!preview.sampled);
}

#[tokio::test]
async fn a_session_id_that_is_not_a_uuid_is_an_error() {
    let app = settings_app(AppSettings::default());

    assert!(
        preview_watcher_pattern(app.handle().clone(), Some("not-a-uuid".into()), "x".into())
            .await
            .is_err()
    );
}

/// 9.4.58a - nine enabled selectorless rows against one agent: the first eight in ID order
/// hold a slot and the ninth does not, with nothing on disk contributing.
///
/// The rows are sent in an order deliberately different from lexicographic, so an
/// implementation that honours request order instead of `BTreeMap` key order fails here.
/// Its failure mode is the UI telling the user that a watcher holds a slot it does not.
#[tokio::test]
async fn a_draft_of_nine_rows_allocates_the_first_eight_in_id_order() {
    let ids = ["w3", "w9", "w1", "w7", "w5", "w2", "w8", "w4", "w6"];
    let rows = preview_watcher_reach(
        ids.iter().map(|id| draft(id, true, None)).collect(),
        vec![draft_agent("a1", "Claude", "claude")],
    )
    .await
    .expect("reach");

    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ids,
        "one response row per request row, in request order"
    );
    for id in ["w1", "w2", "w3", "w4", "w5", "w6", "w7", "w8"] {
        assert!(allocated_on(&rows, id, "a1"), "{id} is within the budget");
    }
    assert!(
        !allocated_on(&rows, "w9", "a1"),
        "w9 sorts ninth and the agent has eight slots"
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.entries.iter().any(|entry| entry.allocated))
            .count(),
        crate::pty::watchers::WATCHERS_PER_AGENT_BUDGET,
        "no draft yields more allocated rows on one agent than that agent has slots"
    );
}

/// 9.4.58b - the draft is the whole input. A row the user deleted contributes nothing to
/// any agent's budget, however many rows the saved map still holds.
#[tokio::test]
async fn rows_absent_from_the_draft_consume_no_budget() {
    let rows = preview_watcher_reach(
        ["w1", "w2", "w3", "w4"]
            .iter()
            .map(|id| draft(id, true, None))
            .collect(),
        vec![draft_agent("a1", "Claude", "claude")],
    )
    .await
    .expect("reach");

    assert_eq!(rows.len(), 4);
    for id in ["w1", "w2", "w3", "w4"] {
        assert!(allocated_on(&rows, id, "a1"));
    }
}

/// 9.4.58c - the displacement fixture: one disabled row plus eight enabled ones.
///
/// This is the case a per-row forced-enabled pass gets wrong. There, `a`'s own pass
/// displaced `i` while every other row's pass displaced nobody, so all nine reported
/// themselves in budget on an agent with eight slots.
#[tokio::test]
async fn a_disabled_row_reaches_everything_and_allocates_nothing() {
    let mut watchers = vec![draft("a", false, None)];
    for id in ["b", "c", "d", "e", "f", "g", "h", "i"] {
        watchers.push(draft(id, true, None));
    }
    let rows = preview_watcher_reach(watchers, vec![draft_agent("a1", "Claude", "claude")])
        .await
        .expect("reach");

    assert_eq!(
        reached_agents(&rows, "a"),
        vec!["a1"],
        "a disabled row still reports the agents its selector reaches"
    );
    assert!(
        !allocated_on(&rows, "a", "a1"),
        "a disabled row holds no slot, and the editor must call that 'disabled', not 'budget'"
    );
    for id in ["b", "c", "d", "e", "f", "g", "h", "i"] {
        assert!(
            allocated_on(&rows, id, "a1"),
            "{id} is one of the eight enabled rows and the disabled row displaces none of them"
        );
    }
    assert_eq!(
        rows.iter()
            .filter(|row| row.entries.iter().any(|entry| entry.allocated))
            .count(),
        crate::pty::watchers::WATCHERS_PER_AGENT_BUDGET
    );
}

/// 9.4.58d - reach does not depend on enablement. The `entries` of a row are identical
/// whether it is enabled or disabled, all else equal; only `allocated` moves.
#[tokio::test]
async fn reach_does_not_depend_on_enablement() {
    let agents = vec![
        draft_agent("a1", "Claude", "claude"),
        draft_agent("a2", "Codex", "codex"),
    ];
    let others = || {
        vec![
            draft("w1", true, Some(&["claude"])),
            draft("w2", true, None),
        ]
    };

    let mut enabled = others();
    enabled.push(draft("w3", true, Some(&["claude", "codex"])));
    let enabled = preview_watcher_reach(enabled, agents.clone())
        .await
        .expect("reach");

    let mut disabled = others();
    disabled.push(draft("w3", false, Some(&["claude", "codex"])));
    let disabled = preview_watcher_reach(disabled, agents)
        .await
        .expect("reach");

    assert_eq!(reached_agents(&enabled, "w3"), vec!["a1", "a2"]);
    assert_eq!(
        reached_agents(&disabled, "w3"),
        reached_agents(&enabled, "w3")
    );
    assert!(allocated_on(&enabled, "w3", "a1"));
    assert!(!allocated_on(&disabled, "w3", "a1"));
    assert_eq!(
        reached_agents(&disabled, "w1"),
        reached_agents(&enabled, "w1"),
        "and no other row's reach moved either"
    );
}

/// 9.4.58e - the selector rules, unchanged: absent reaches every agent, `[]` reaches none,
/// a selector that does not tokenize skips the whole watcher, and the reported stem is the
/// AGENT's. In both reach-nobody cases the response row is still present, with no entries.
#[tokio::test]
async fn the_selector_rules_survive_the_draft_shape() {
    let rows = preview_watcher_reach(
        vec![
            draft("all", true, None),
            draft("none", true, Some(&[])),
            draft("broken", true, Some(&["claude", "   "])),
            draft("claude-only", true, Some(&["claude"])),
            draft("typo", true, Some(&["gemni"])),
        ],
        vec![
            draft_agent("a1", "Claude", "claude"),
            draft_agent("a2", "Claude Sandbox", r"C:\rt\claude.cmd"),
            draft_agent("a3", "Codex", "codex"),
            draft_agent("a4", "Pi via Claude", "pi --provider claude"),
        ],
    )
    .await
    .expect("reach");

    assert_eq!(reached_agents(&rows, "all"), vec!["a1", "a2", "a3", "a4"]);
    assert!(
        row(&rows, "none").entries.is_empty(),
        "an empty selector reaches nobody and is the opposite of an absent one"
    );
    assert!(
        row(&rows, "broken").entries.is_empty(),
        "one unreadable selector entry skips the WHOLE watcher, never 'reaches everybody'"
    );
    assert!(row(&rows, "typo").entries.is_empty());
    assert_eq!(reached_agents(&rows, "claude-only"), vec!["a1", "a2"]);

    let claude_only = &row(&rows, "claude-only").entries;
    assert_eq!(claude_only[0].agent_label, "Claude");
    assert_eq!(
        claude_only[1].command_stem, "claude",
        "the stem reported is the agent's, resolved through the one rule in the tree"
    );
}

/// 9.4.58f - the agents come from the draft too. Each of these three is a case where
/// resolving against the saved agent list would have answered about a state the user had
/// already left, and two of the three would have over-reported.
#[tokio::test]
async fn the_agent_half_of_the_draft_decides_the_reach() {
    let watchers = || {
        vec![
            draft("on-claude", true, Some(&["claude"])),
            draft("on-codex", true, Some(&["codex"])),
        ]
    };

    let before = preview_watcher_reach(
        watchers(),
        vec![
            draft_agent("a1", "Claude", "claude"),
            draft_agent("a2", "Second", "claude"),
        ],
    )
    .await
    .expect("reach");
    assert_eq!(reached_agents(&before, "on-claude"), vec!["a1", "a2"]);
    assert!(row(&before, "on-codex").entries.is_empty());

    // The command changed in the draft: the agent leaves one watcher and joins the other.
    let retargeted = preview_watcher_reach(
        watchers(),
        vec![
            draft_agent("a1", "Claude", "claude"),
            draft_agent("a2", "Second", "codex"),
        ],
    )
    .await
    .expect("reach");
    assert_eq!(reached_agents(&retargeted, "on-claude"), vec!["a1"]);
    assert_eq!(reached_agents(&retargeted, "on-codex"), vec!["a2"]);

    // The agent was deleted in the draft: it is named by nobody.
    let removed = preview_watcher_reach(watchers(), vec![draft_agent("a1", "Claude", "claude")])
        .await
        .expect("reach");
    assert_eq!(reached_agents(&removed, "on-claude"), vec!["a1"]);

    // The agent was added in the draft: it is reached before it is ever saved.
    let added = preview_watcher_reach(
        watchers(),
        vec![
            draft_agent("a1", "Claude", "claude"),
            draft_agent("a2", "Second", "claude"),
            draft_agent("a3", "Third", "codex"),
        ],
    )
    .await
    .expect("reach");
    assert_eq!(reached_agents(&added, "on-claude"), vec!["a1", "a2"]);
    assert_eq!(reached_agents(&added, "on-codex"), vec!["a3"]);
}

/// 9.4.58j - the fixed points of the contract that the editor cannot produce but the
/// command still has to define: duplicate ids, an empty id, response order, entry order.
#[tokio::test]
async fn the_defined_behavior_for_drafts_the_editor_cannot_produce() {
    let rows = preview_watcher_reach(
        vec![
            draft("dup", true, Some(&["codex"])),
            draft("", true, None),
            draft("dup", true, Some(&["claude"])),
        ],
        vec![
            // Labels out of alphabetical order and one pair sharing a label, so the
            // entry sort is exercised on both keys.
            draft_agent("a2", "Zed", "claude"),
            draft_agent("a3", "Alpha", "claude"),
            draft_agent("a1", "Alpha", "claude"),
        ],
    )
    .await
    .expect("reach");

    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        vec!["dup", "", "dup"],
        "one response row per request row, in request order, id carried back"
    );
    assert_eq!(
        reached_agents(&rows, "dup"),
        vec!["a1", "a3", "a2"],
        "entries sort by label with the agent id as tie-break"
    );
    assert_eq!(
        rows[0].entries.len(),
        rows[2].entries.len(),
        "a duplicate id is later-wins in the map and BOTH response rows report that one \
         resolution"
    );
    assert_eq!(reached_agents(&rows, ""), vec!["a1", "a3", "a2"]);
    assert!(
        allocated_on(&rows, "", "a1"),
        "an empty id is a legal key that sorts first, and is not special-cased"
    );
}

/// 9.4.58l - allocation is slot assignment and not a promise of output.
///
/// The pattern does not travel to the reach command at all, so a row whose regex does not
/// compile is allocated a slot and is inert. The two dimensions are asserted together, so
/// nobody later reads `allocated` as a promise that the watcher will emit.
#[tokio::test]
async fn an_uncompilable_pattern_is_allocated_and_inert() {
    let app = settings_app(AppSettings::default());

    let rows = preview_watcher_reach(
        vec![draft("broken-regex", true, None)],
        vec![draft_agent("a1", "Claude", "claude")],
    )
    .await
    .expect("reach");
    assert!(
        allocated_on(&rows, "broken-regex", "a1"),
        "an enabled row within budget holds its slot whatever its pattern is"
    );

    let compile = preview_watcher_pattern(app.handle().clone(), None, "Read (".to_string())
        .await
        .expect("preview");
    assert!(
        !compile.compiles,
        "and the other dimension is answered next to it, by the row's own pattern preview"
    );
    assert!(compile.error.is_some());
}
