use super::*;
use crate::session::session::SessionStatus;

fn activity_row(
    name: &str,
    working_directory: &Path,
    status: SessionStatus,
    waiting_for_input: bool,
) -> PersistedSession {
    PersistedSession {
        name: name.to_string(),
        shell: "powershell.exe".to_string(),
        shell_args: Vec::new(),
        working_directory: working_directory.to_string_lossy().to_string(),
        id: Some(uuid::Uuid::new_v4().to_string()),
        status: Some(status),
        waiting_for_input: Some(waiting_for_input),
        ..PersistedSession::default()
    }
}

fn activity_instant(text: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(text)
        .expect("instant")
        .with_timezone(&chrono::Utc)
}

#[test]
fn activity_path_key_normalizes_all_four_verbatim_forms_on_every_host() {
    // Host-independent by construction: these paths do not exist, so
    // `canonicalize` fails and the raw bytes go through the four-arm
    // normalizer on Linux and Windows alike.
    let plain_drive = canonical_activity_path_key(Path::new(r"C:\Repo\Rooms\A"));
    assert_eq!(
        canonical_activity_path_key(Path::new(r"\\?\C:\Repo\Rooms\A")),
        plain_drive
    );
    assert_eq!(
        canonical_activity_path_key(Path::new(r"\??\C:\Repo\Rooms\A")),
        plain_drive
    );
    assert_eq!(
        canonical_activity_path_key(Path::new("c:/repo/rooms/a/")),
        plain_drive
    );

    let plain_unc = canonical_activity_path_key(Path::new(r"\\server\share\repo"));
    assert_eq!(
        canonical_activity_path_key(Path::new(r"\\?\UNC\server\share\repo")),
        plain_unc
    );
    assert_eq!(
        canonical_activity_path_key(Path::new(r"\??\UNC\server\share\repo")),
        plain_unc
    );
    assert_eq!(
        canonical_activity_path_key(Path::new("//SERVER/SHARE/REPO/")),
        plain_unc
    );
    assert_ne!(plain_drive, plain_unc);
}

#[test]
fn activity_path_key_resolves_dot_segments_for_existing_paths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("create repo");
    let detoured = tmp.path().join("repo").join(".").join("..").join("repo");
    assert_eq!(
        canonical_activity_path_key(&repo),
        canonical_activity_path_key(&detoured)
    );
}

fn indexed(state: PersistedCiState) -> IndexedRepoCi {
    indexed_with(state, &[], &[], None)
}

fn indexed_with(
    state: PersistedCiState,
    run_ids: &[u64],
    pull_requests: &[u64],
    unknown_reason: Option<&str>,
) -> IndexedRepoCi {
    IndexedRepoCi {
        ci: repo_ci(state, run_ids, pull_requests, unknown_reason),
        conflicting: false,
    }
}

fn repo_ci(
    state: PersistedCiState,
    run_ids: &[u64],
    pull_requests: &[u64],
    unknown_reason: Option<&str>,
) -> PersistedRepoCi {
    PersistedRepoCi {
        state,
        run_ids: run_ids.to_vec(),
        pull_requests: pull_requests.to_vec(),
        unknown_reason: unknown_reason.map(str::to_string),
    }
}

fn state_of(repo_keys: &[String], entries: &HashMap<String, IndexedRepoCi>) -> &'static str {
    aggregate_room_ci(repo_keys, entries, None).state
}

#[test]
fn activity_ci_aggregation_is_the_documented_tri_state() {
    let entries = HashMap::from([
        ("a".to_string(), indexed(PersistedCiState::Running)),
        ("b".to_string(), indexed(PersistedCiState::Idle)),
    ]);
    assert_eq!(state_of(&[], &entries), "unknown");
    assert_eq!(state_of(&["a".to_string()], &entries), "running");
    assert_eq!(state_of(&["b".to_string()], &entries), "idle");
    assert_eq!(
        state_of(&["b".to_string(), "missing".to_string()], &entries),
        "unknown"
    );
    assert_eq!(
        state_of(
            &["b".to_string(), "c".to_string()],
            &HashMap::from([
                ("b".to_string(), indexed(PersistedCiState::Idle)),
                ("c".to_string(), indexed(PersistedCiState::Unknown)),
            ])
        ),
        "unknown"
    );
    assert_eq!(
        state_of(&["a".to_string(), "c".to_string()], &entries),
        "running",
        "a running repo outranks an unmatched one"
    );
}

#[test]
fn activity_ci_index_matches_verbatim_case_separator_and_trailing_variants() {
    let snapshot = RemoteActivitySnapshot {
        generated_at: chrono::Utc::now(),
        repos: vec![(
            r"\\?\C:\Repo\Rooms\A\".to_string(),
            repo_ci(PersistedCiState::Idle, &[], &[], None),
        )],
    };
    let entries = index_ci_entries(&snapshot);
    let room_repo_keys = vec![canonical_activity_path_key(Path::new("c:/repo/rooms/a"))];
    assert_eq!(state_of(&room_repo_keys, &entries), "idle");
}

#[test]
fn activity_ci_index_collapses_conflicting_duplicates_to_unknown() {
    let snapshot = RemoteActivitySnapshot {
        generated_at: chrono::Utc::now(),
        repos: vec![
            (
                "A:/repo".to_string(),
                repo_ci(PersistedCiState::Running, &[], &[], None),
            ),
            (
                "a:/repo/".to_string(),
                repo_ci(PersistedCiState::Idle, &[], &[], None),
            ),
        ],
    };
    let entries = index_ci_entries(&snapshot);
    assert_eq!(state_of(&["a:/repo".to_string()], &entries), "unknown");
}

// --- #2473 filters and CI detail ---

fn filter_args() -> WorkgroupActivityArgs {
    WorkgroupActivityArgs {
        project: "p".to_string(),
        rooms: None,
        team: None,
        working: None,
        ci_state: None,
        title_regex: None,
        hide_clean: false,
    }
}

#[test]
fn u1_parse_activity_filter_rejects_each_bad_value_with_the_exact_message() {
    let rooms = |v: &str| {
        format!(
            "Invalid --rooms '{v}': expected comma-separated positive room numbers, e.g. 5,12,17"
        )
    };
    let regex_error = regex::Regex::new(&String::from("("))
        .expect_err("bad regex")
        .to_string();
    let cases: Vec<(WorkgroupActivityArgs, String)> = vec![
        (
            WorkgroupActivityArgs {
                rooms: Some("5,,7".into()),
                ..filter_args()
            },
            rooms("5,,7"),
        ),
        (
            WorkgroupActivityArgs {
                rooms: Some("0".into()),
                ..filter_args()
            },
            rooms("0"),
        ),
        (
            WorkgroupActivityArgs {
                rooms: Some("-1".into()),
                ..filter_args()
            },
            rooms("-1"),
        ),
        (
            WorkgroupActivityArgs {
                rooms: Some("abc".into()),
                ..filter_args()
            },
            rooms("abc"),
        ),
        (
            WorkgroupActivityArgs {
                rooms: Some("".into()),
                ..filter_args()
            },
            rooms(""),
        ),
        (
            WorkgroupActivityArgs {
                rooms: Some("5,".into()),
                ..filter_args()
            },
            rooms("5,"),
        ),
        (
            WorkgroupActivityArgs {
                rooms: Some("4294967296".into()),
                ..filter_args()
            },
            rooms("4294967296"),
        ),
        (
            WorkgroupActivityArgs {
                team: Some(" ".into()),
                ..filter_args()
            },
            "Invalid --team ' ': expected a team name".to_string(),
        ),
        (
            WorkgroupActivityArgs {
                team: Some("".into()),
                ..filter_args()
            },
            "Invalid --team '': expected a team name".to_string(),
        ),
        (
            WorkgroupActivityArgs {
                working: Some("TRUE".into()),
                ..filter_args()
            },
            "Invalid --working 'TRUE': expected true or false".to_string(),
        ),
        (
            WorkgroupActivityArgs {
                working: Some("1".into()),
                ..filter_args()
            },
            "Invalid --working '1': expected true or false".to_string(),
        ),
        (
            WorkgroupActivityArgs {
                ci_state: Some("Running".into()),
                ..filter_args()
            },
            "Invalid --ci-state 'Running': expected running, idle or unknown".to_string(),
        ),
        (
            WorkgroupActivityArgs {
                ci_state: Some("done".into()),
                ..filter_args()
            },
            "Invalid --ci-state 'done': expected running, idle or unknown".to_string(),
        ),
        (
            WorkgroupActivityArgs {
                title_regex: Some("(".into()),
                ..filter_args()
            },
            format!("Invalid --title-regex '(': {regex_error}"),
        ),
        (
            // First failing flag in the documented order wins.
            WorkgroupActivityArgs {
                rooms: Some("x".into()),
                team: Some(" ".into()),
                working: Some("no".into()),
                ..filter_args()
            },
            rooms("x"),
        ),
        (
            WorkgroupActivityArgs {
                working: Some("no".into()),
                title_regex: Some("(".into()),
                ..filter_args()
            },
            "Invalid --working 'no': expected true or false".to_string(),
        ),
    ];
    assert!(cases.len() >= 12);
    for (args, expected) in cases {
        assert_eq!(parse_activity_filter(&args).err(), Some(expected));
    }
    let ok = parse_activity_filter(&WorkgroupActivityArgs {
        rooms: Some(" 5, 12,5 ".into()),
        team: Some("-t".into()),
        working: Some("false".into()),
        ci_state: Some("idle".into()),
        title_regex: Some("-x".into()),
        hide_clean: true,
        ..filter_args()
    })
    .expect("valid filter");
    assert_eq!(ok.rooms, Some(BTreeSet::from([5, 12])));
    assert_eq!(ok.team.as_deref(), Some("-t"));
    assert_eq!(ok.working, Some(false));
    assert_eq!(ok.ci_state, Some("idle"));
    assert!(ok.hide_clean);
}

fn item(
    name: &str,
    team: &str,
    working: bool,
    ci: &'static str,
    title: Option<&str>,
) -> WorkgroupActivityItem {
    WorkgroupActivityItem {
        name: name.to_string(),
        team: team.to_string(),
        working,
        ci_state: ci,
        task_title: title.map(str::to_string),
        ci_run_ids: Vec::new(),
        ci_pull_requests: Vec::new(),
        ci_unknown_reason: None,
    }
}

#[test]
fn u2_each_predicate_passes_one_item_and_rejects_another() {
    let base = item("room-5-dev", "dev", true, "running", Some("Fix #12 parser"));
    let cases: Vec<(ActivityFilter, WorkgroupActivityItem)> = vec![
        (
            ActivityFilter {
                rooms: Some(BTreeSet::from([5, 17])),
                ..ActivityFilter::default()
            },
            item("room-6-dev", "dev", true, "running", Some("Fix #12 parser")),
        ),
        (
            ActivityFilter {
                team: Some("dev".into()),
                ..ActivityFilter::default()
            },
            item("room-5-ops", "ops", true, "running", Some("Fix #12 parser")),
        ),
        (
            ActivityFilter {
                working: Some(true),
                ..ActivityFilter::default()
            },
            item(
                "room-5-dev",
                "dev",
                false,
                "running",
                Some("Fix #12 parser"),
            ),
        ),
        (
            ActivityFilter {
                ci_state: Some("running"),
                ..ActivityFilter::default()
            },
            item("room-5-dev", "dev", true, "idle", Some("Fix #12 parser")),
        ),
        (
            ActivityFilter {
                title_regex: Some(regex::Regex::new("#1\\d").expect("regex")),
                ..ActivityFilter::default()
            },
            item("room-5-dev", "dev", true, "running", None),
        ),
        (
            ActivityFilter {
                hide_clean: true,
                ..ActivityFilter::default()
            },
            item("room-5-dev", "dev", true, "running", Some("  CLEAN ")),
        ),
    ];
    for (filter, failing) in &cases {
        assert!(filter.matches(&base), "{filter:?} passes the base item");
        assert!(!filter.matches(failing), "{filter:?} rejects {failing:?}");
    }
    let regex = ActivityFilter {
        title_regex: Some(regex::Regex::new("fix").expect("regex")),
        ..ActivityFilter::default()
    };
    assert!(!regex.matches(&base), "the regex is case-sensitive");
    let hide = ActivityFilter {
        hide_clean: true,
        ..ActivityFilter::default()
    };
    assert!(!hide.matches(&item("room-5-dev", "dev", true, "running", None)));
    assert!(!hide.matches(&item("room-5-dev", "dev", true, "running", Some("  "))));
    assert!(hide.matches(&item(
        "room-5-dev",
        "dev",
        true,
        "running",
        Some("Clean up")
    )));
    let rooms = ActivityFilter {
        rooms: Some(BTreeSet::from([5])),
        ..ActivityFilter::default()
    };
    assert!(!rooms.matches(&item("room-x", "dev", true, "running", None)));
    assert!(ActivityFilter::default().matches(&base));
}

#[test]
fn u3_room_number_reads_both_prefixes() {
    assert_eq!(room_number("room-15-x"), Some(15));
    assert_eq!(room_number("wg-3-x"), Some(3));
    assert_eq!(room_number("room-x"), None);
    assert_eq!(room_number("other-3-x"), None);
}

#[test]
fn u4_aggregation_reports_every_reason_and_unions_runs() {
    let key = |k: &str| k.to_string();
    let entries = HashMap::from([
        (
            key("run1"),
            indexed_with(PersistedCiState::Running, &[30, 10], &[7], None),
        ),
        (
            key("run2"),
            indexed_with(PersistedCiState::Running, &[10, 20], &[7, 2], None),
        ),
        (key("idle"), indexed(PersistedCiState::Idle)),
        (key("unk"), indexed(PersistedCiState::Unknown)),
        (
            key("timeout"),
            indexed_with(
                PersistedCiState::Unknown,
                &[],
                &[],
                Some("ci-query-timeout"),
            ),
        ),
        (
            key("weird"),
            indexed_with(PersistedCiState::Unknown, &[], &[], Some("made-up")),
        ),
        (
            key("conflict"),
            IndexedRepoCi {
                ci: repo_ci(PersistedCiState::Unknown, &[], &[], None),
                conflicting: true,
            },
        ),
    ]);
    let reason = |keys: &[&str], gate: Option<&'static str>| {
        let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        aggregate_room_ci(&keys, &entries, gate)
    };
    let running = reason(&["run1", "run2", "unk", "idle"], None);
    assert_eq!(
        running,
        RoomCi {
            state: "running",
            run_ids: vec![10, 20, 30],
            pull_requests: vec![2, 7],
            unknown_reason: None,
        }
    );
    let idle = reason(&["idle"], None);
    assert_eq!(
        (
            idle.state,
            idle.run_ids,
            idle.pull_requests,
            idle.unknown_reason
        ),
        ("idle", vec![], vec![], None)
    );
    assert_eq!(reason(&[], None).unknown_reason, Some("no-repos"));
    assert_eq!(
        reason(&["idle", "missing"], None).unknown_reason,
        Some("repo-not-in-snapshot")
    );
    assert_eq!(
        reason(&["idle", "conflict"], None).unknown_reason,
        Some("conflicting-snapshot-entries")
    );
    assert_eq!(
        reason(&["timeout", "idle"], None).unknown_reason,
        Some("ci-query-timeout")
    );
    assert_eq!(
        reason(&["unk"], None).unknown_reason,
        Some("repo-ci-unknown")
    );
    assert_eq!(
        reason(&["weird"], None).unknown_reason,
        Some("repo-ci-unknown")
    );
    // First repo in sorted order decides: "timeout" < "unk".
    assert_eq!(
        reason(&["unk", "timeout"], None).unknown_reason,
        Some("ci-query-timeout")
    );
    for gate in ["ci-disabled", "daemon-not-live", "snapshot-unavailable"] {
        let empty = HashMap::new();
        let gated = aggregate_room_ci(&[], &empty, Some(gate));
        assert_eq!((gated.state, gated.unknown_reason), ("unknown", Some(gate)));
        let gated = aggregate_room_ci(&[key("a")], &empty, Some(gate));
        assert_eq!(
            gated.unknown_reason,
            Some(gate),
            "the gate precedes repo reasons"
        );
        assert!(gated.run_ids.is_empty() && gated.pull_requests.is_empty());
    }
    let unknown = reason(&["unk"], None);
    assert!(unknown.run_ids.is_empty() && unknown.pull_requests.is_empty());
}

#[test]
fn p4_duplicate_entries_union_metadata_but_conflicting_states_are_unknown() {
    let snapshot = RemoteActivitySnapshot {
        generated_at: chrono::Utc::now(),
        repos: vec![
            (
                "A:/repo".to_string(),
                repo_ci(PersistedCiState::Running, &[1, 3], &[9], None),
            ),
            (
                "a:/repo/".to_string(),
                repo_ci(PersistedCiState::Running, &[2, 3], &[8], None),
            ),
            (
                "b:/repo".to_string(),
                repo_ci(PersistedCiState::Running, &[5], &[], None),
            ),
            (
                "B:/repo/".to_string(),
                repo_ci(PersistedCiState::Idle, &[], &[], None),
            ),
        ],
    };
    let entries = index_ci_entries(&snapshot);
    let same = aggregate_room_ci(&["a:/repo".to_string()], &entries, None);
    assert_eq!(same.state, "running");
    assert_eq!(same.run_ids, vec![1, 2, 3]);
    assert_eq!(same.pull_requests, vec![8, 9]);
    let conflict = aggregate_room_ci(&["b:/repo".to_string()], &entries, None);
    assert_eq!(conflict.state, "unknown");
    assert_eq!(
        conflict.unknown_reason,
        Some("conflicting-snapshot-entries")
    );
    assert!(conflict.run_ids.is_empty());
}

#[test]
fn activity_ci_gate_prefers_disabled_then_daemon() {
    assert_eq!(ci_gate(true, true), CiGate::Usable);
    assert_eq!(ci_gate(false, true), CiGate::CiDisabled);
    assert_eq!(ci_gate(false, false), CiGate::CiDisabled);
    assert_eq!(ci_gate(true, false), CiGate::DaemonNotLive);
}

#[test]
fn activity_cache_read_rejects_missing_stale_future_and_malformed_and_accepts_fresh() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
    let now = activity_instant("2026-09-22T11:41:41Z");
    let entry = vec![(
        "a:/repo".to_string(),
        repo_ci(PersistedCiState::Running, &[], &[], None),
    )];

    assert!(
        read_fresh_ci_snapshot(Some(tmp.path()), now).is_err(),
        "a missing snapshot is an error"
    );
    assert!(
        read_fresh_ci_snapshot(None, now).is_err(),
        "an unavailable config directory is a missing snapshot"
    );

    crate::config::remote_activity_cache::write_snapshot(
        &path,
        now - chrono::Duration::seconds(30),
        &entry,
    )
    .expect("write fresh snapshot");
    let fresh = read_fresh_ci_snapshot(Some(tmp.path()), now).expect("fresh snapshot");
    assert_eq!(fresh.repos, entry);

    crate::config::remote_activity_cache::write_snapshot(
        &path,
        now - chrono::Duration::seconds(31),
        &entry,
    )
    .expect("write stale snapshot");
    assert!(read_fresh_ci_snapshot(Some(tmp.path()), now).is_err());

    crate::config::remote_activity_cache::write_snapshot(
        &path,
        now + chrono::Duration::seconds(31),
        &entry,
    )
    .expect("write future snapshot");
    assert!(read_fresh_ci_snapshot(Some(tmp.path()), now).is_err());

    std::fs::write(&path, b"{not json").expect("write malformed");
    assert!(read_fresh_ci_snapshot(Some(tmp.path()), now).is_err());
}

#[test]
fn activity_working_requires_exact_name_path_and_working_status() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let room = tmp.path().join("room-1-dev-team");
    let coord = room.join("__agent_coord");
    let dev = room.join("__agent_dev");
    std::fs::create_dir_all(&coord).expect("coord replica");
    std::fs::create_dir_all(&dev).expect("dev replica");
    let foreign = tmp.path().join("ForeignProject").join("__agent_dev");
    std::fs::create_dir_all(&foreign).expect("foreign replica");

    let idle_coordinator = activity_row("room-1-dev-team/coord", &coord, SessionStatus::Idle, true);
    assert!(
        !room_is_working(
            &room,
            "room-1-dev-team",
            std::slice::from_ref(&idle_coordinator)
        ),
        "an idle coordinator must not make its room work"
    );

    let mut no_id = activity_row("room-1-dev-team/dev", &dev, SessionStatus::Running, false);
    no_id.id = None;
    let rejected = vec![
        // The raw directory name is not the persisted session name.
        activity_row(
            "room-1-dev-team/__agent_dev",
            &dev,
            SessionStatus::Running,
            false,
        ),
        // Similarly prefixed room and agent names are not this room's.
        activity_row("room-1-dev-teams/dev", &dev, SessionStatus::Running, false),
        activity_row("room-2-dev-team/dev", &dev, SessionStatus::Running, false),
        activity_row("room-1-dev-team/dev-2", &dev, SessionStatus::Running, false),
        // Same name, different normalized working directory.
        activity_row(
            "room-1-dev-team/dev",
            &foreign,
            SessionStatus::Running,
            false,
        ),
        // Waiting/exited rows are not working.
        activity_row("room-1-dev-team/dev", &dev, SessionStatus::Running, true),
        activity_row("room-1-dev-team/dev", &dev, SessionStatus::Idle, false),
        no_id,
    ];
    for row in rejected {
        assert!(
            !room_is_working(&room, "room-1-dev-team", &[row]),
            "the row must not count as working"
        );
    }

    let running_dev = activity_row("room-1-dev-team/dev", &dev, SessionStatus::Running, false);
    assert!(
        room_is_working(&room, "room-1-dev-team", &[idle_coordinator, running_dev]),
        "a local non-coordinator working row flips the room true"
    );
}

#[test]
fn activity_repo_dirs_are_immediate_prefix_matches_only() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let room = tmp.path().join("room-1-dev-team");
    std::fs::create_dir_all(room.join("repo-b")).expect("repo-b");
    std::fs::create_dir_all(room.join("repo-a")).expect("repo-a");
    std::fs::create_dir_all(room.join("repo-a").join("nested")).expect("nested");
    std::fs::create_dir_all(room.join("not-a-repo")).expect("not-a-repo");
    std::fs::write(room.join("repo-file"), b"file").expect("repo-file");
    let names: Vec<String> = list_room_repo_dirs(&room)
        .iter()
        .map(|path| {
            path.file_name()
                .expect("file name")
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert_eq!(names, vec!["repo-a", "repo-b"]);
}

#[test]
fn activity_task_title_reads_quoted_and_bare_titles_and_ignores_missing_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert_eq!(read_task_title(tmp.path()), None, "missing TASK.md is null");

    std::fs::write(tmp.path().join("TASK.md"), "---\ntitle: 'Quoted'\n---\n")
        .expect("write quoted");
    assert_eq!(read_task_title(tmp.path()).as_deref(), Some("Quoted"));

    std::fs::write(tmp.path().join("TASK.md"), "---\ntitle: Bare\n---\n").expect("write bare");
    assert_eq!(read_task_title(tmp.path()).as_deref(), Some("Bare"));

    std::fs::write(tmp.path().join("TASK.md"), "---\ntitle: ''\n---\n").expect("write empty");
    assert_eq!(read_task_title(tmp.path()), None, "an empty title is null");

    std::fs::write(tmp.path().join("TASK.md"), [0xFF_u8, 0xFE]).expect("write invalid");
    assert_eq!(read_task_title(tmp.path()), None, "invalid UTF-8 is null");
}

#[test]
fn partial_delete_outcome_does_not_authorize_removed_refresh() {
    let outcome = WgDeleteOutcome::Partial {
        orphan_path: PathBuf::from(".deleting-wg-1-test-orphan"),
        error: std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "forced remove failure",
        ),
    };

    let err = cli_remove_refresh_decision(outcome)
        .expect_err("partial delete must not authorize workgroupRemoved refresh");
    assert!(err.contains("Failed to fully delete room directory"));
    assert!(err.contains(".deleting-wg-1-test-orphan"));
}

#[test]
fn clean_delete_outcome_authorizes_removed_refresh() {
    assert_eq!(
        cli_remove_refresh_decision(WgDeleteOutcome::Deleted)
            .expect("deleted outcome should refresh"),
        RemoveRefreshDecision::EmitWorkgroupRemoved
    );
}

#[test]
fn cli_team_builder_defaults_context_alerts_to_disabled() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir_all(ac_root.join("_agent_coordinator")).expect("coordinator matrix");
    std::fs::create_dir_all(ac_root.join("_agent_member")).expect("member matrix");

    let config = build_new_team_config(
        &ac_root,
        "coordinator",
        &["member".to_string()],
        &[],
        &[],
        &[],
    )
    .expect("build config");
    assert!(config.context_alert_percentages.is_empty());
}

#[tokio::test]
async fn legacy_workgroup_provisioning_never_upserts_concurrent_team_winner() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("Project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(ac_root.join("_agent_coordinator")).expect("coordinator matrix");
    let winner = TeamConfigResult {
        agents: vec!["_agent_coordinator".to_string()],
        coordinator: "_agent_coordinator".to_string(),
        repos: Vec::new(),
        context_alert_percentages: vec![75],
    };
    crate::commands::entity_creation::create_new_team_config_on_disk(&ac_root, "dev-team", &winner)
        .expect("concurrent winner");

    let err = create_workgroup_on_disk(WorkgroupDiskCreateArgs {
        project_path: project,
        team_name: "dev-team".to_string(),
        task_title: "Race test".to_string(),
        coordinator: Some("_agent_coordinator".to_string()),
        agents: vec!["_agent_coordinator".to_string()],
        repos: Vec::new(),
    })
    .await
    .expect_err("legacy loser must not upsert");

    assert!(err.contains("Team 'dev-team' already exists"), "{err}");
    assert_eq!(
        read_team_config(&ac_root, "dev-team")
            .expect("winner config")
            .context_alert_percentages,
        vec![75]
    );
    assert!(list_workgroup_dirs(&ac_root).is_empty());
}

// #1063 Stage D-owned exact ignored CHILD HELPER for the CLI workgroup
// cross-process lock-order inversion. Frozen fully-qualified name (a Stage E
// parent spawns this verbatim via `current_exe --exact <name> --ignored`):
//   crate::cli::workgroup::tests::cli_workgroup_lock_order_inversion_child
// No-ops (no guard, no mutation) unless the child-mode action + per-spawn nonce
// + control dir are all supplied; when driven, it calls the real private
// `remove_hooked` (project-only) and drives the `after_project_acquired` barrier.
#[test]
#[ignore]
fn cli_workgroup_lock_order_inversion_child() {
    use crate::cli::team::stage_d_lock_order_child as child;
    let Some(ctx) = child::child_context(child::WORKGROUP_ACTION) else {
        return;
    };
    let project = ctx.build_workgroup_fixture();
    let args = WorkgroupRemoveArgs {
        project,
        workgroup: "wg-1-dev-team".to_string(),
        force_dirty: false,
    };
    let result = remove_hooked(args, None, |_ac_root: &Path| ctx.report_and_wait());
    if let Err(error) = &result {
        println!("STAGE_D_LOCK_ORDER_ERROR {} workgroup {}", ctx.nonce, error);
    }
    println!(
        "STAGE_D_LOCK_ORDER_DONE {} workgroup ok={}",
        ctx.nonce,
        result.is_ok()
    );
}

// #1063: prove the driven project-only lock-order path in-process (no
// `current_exe` parent - that machinery is Stage E). Env-guarded and `#[ignore]`
// so the parallel `--lib` regression is untouched. Like the member driver it
// no-ops unless `DRIVE_VAR` is set, so a bare `cargo test --lib -- --ignored` run
// never drives it. Enable and isolate it deliberately (it pins the once-cached
// config dir, so it must own the process):
// `AC_STAGE_D_LOCK_ORDER_DRIVE=1 cargo test --lib -- --ignored --test-threads=1
// --exact cli::workgroup::tests::cli_workgroup_lock_order_inversion_driver`.
#[test]
#[ignore]
fn cli_workgroup_lock_order_inversion_driver() {
    use crate::cli::team::stage_d_lock_order_child::{
        driver_enabled, EnvGuard, ACTION_VAR, CONTROL_DIR_VAR, NONCE_VAR, WORKGROUP_ACTION,
    };
    if !driver_enabled() {
        return;
    }
    let control = tempfile::tempdir().expect("tempdir");
    let nonce = "driver-workgroup";
    let _guard = EnvGuard::capture(&[
        ACTION_VAR,
        NONCE_VAR,
        CONTROL_DIR_VAR,
        "AGENTSCOMMANDER_TEST_CONFIG_DIR",
    ]);
    std::env::set_var(ACTION_VAR, WORKGROUP_ACTION);
    std::env::set_var(NONCE_VAR, nonce);
    std::env::set_var(CONTROL_DIR_VAR, control.path());

    let reached = control.path().join(format!("reached-{nonce}"));
    let release = control.path().join(format!("release-{nonce}"));
    let releaser = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline && !reached.exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            reached.exists(),
            "child never reported reaching the barrier"
        );
        std::fs::write(&release, b"go").expect("write release");
    });

    cli_workgroup_lock_order_inversion_child();
    releaser.join().expect("join releaser");

    assert!(
        control.path().join(format!("reached-{nonce}")).exists(),
        "the barrier must have fired after the project gate was acquired"
    );
    let wg_dir = control
        .path()
        .join(format!("fixture-{nonce}"))
        .join("Project")
        .join(".ac")
        .join("wg-1-dev-team");
    assert!(
        !wg_dir.exists(),
        "the workgroup must be removed after the barrier releases"
    );
}

/// #1795 `T12`. Drives the real production call site, `create_workgroup_on_disk`,
/// end to end. A test that called `create_room_shared_dir` directly would stay
/// green with the production call deleted, which is exactly the hole control `C6`
/// probes.
#[tokio::test]
async fn create_workgroup_on_disk_creates_the_room_shared_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("Project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(ac_root.join("_agent_coordinator")).expect("coordinator matrix");

    create_workgroup_on_disk(WorkgroupDiskCreateArgs {
        project_path: project,
        team_name: "dev-team".to_string(),
        task_title: "Room shared directory".to_string(),
        coordinator: Some("_agent_coordinator".to_string()),
        agents: vec!["_agent_coordinator".to_string()],
        repos: Vec::new(),
    })
    .await
    .expect("create the room");

    let rooms = list_workgroup_dirs(&ac_root);
    assert_eq!(rooms.len(), 1, "exactly one room must have been created");
    let wg_dir = &rooms[0];

    assert!(
        wg_dir.join("room-shared").is_dir(),
        "`room-shared` must exist under {}",
        wg_dir.display()
    );
    assert!(
        wg_dir.join("messaging").is_dir(),
        "`messaging` must exist under {}",
        wg_dir.display()
    );
    assert!(
        wg_dir.join("TASK.md").is_file(),
        "`TASK.md` must exist under {}",
        wg_dir.display()
    );
}
