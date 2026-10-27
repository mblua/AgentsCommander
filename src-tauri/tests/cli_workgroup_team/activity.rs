use super::{
    assert_no_side_effect_dirs, assert_same_path, command_for_binary, config_dir_for_bin,
    copy_binary_into, project_refresh_request_paths, project_with_agents, run_fail, run_json,
    run_json_machine, spawn_lock, write_settings, Path, PathBuf, PersistedSession, SessionStatus,
    Stdio, Tmp,
};

/// A live daemon PID for this process: every positive CI case needs one,
/// because a snapshot is only authoritative while the daemon that refreshes it
/// is live.
fn write_live_daemon_pid(config_dir: &Path) {
    std::fs::write(
        config_dir.join("daemon.pid"),
        std::process::id().to_string(),
    )
    .expect("write daemon.pid");
}

/// An RFC3339 UTC instant `offset_secs` away from now, seconds precision like
/// the producer's codec.
fn activity_rfc3339(offset_secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(offset_secs))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Write the on-disk snapshot DIRECTLY, pinning the schema the reader must
/// accept: version 1, `generatedAt`, `{path, ciState}` entries.
fn write_activity_snapshot(config_dir: &Path, generated_at: &str, entries: &[(&Path, &str)]) {
    let repos: Vec<serde_json::Value> = entries
        .iter()
        .map(|(path, state)| {
            serde_json::json!({
                "path": path.to_string_lossy(),
                "ciState": state,
            })
        })
        .collect();
    let snapshot = serde_json::json!({
        "schemaVersion": 1,
        "generatedAt": generated_at,
        "repos": repos,
    });
    std::fs::write(
        config_dir.join("remote-activity.json"),
        serde_json::to_string_pretty(&snapshot).expect("snapshot json"),
    )
    .expect("write remote-activity.json");
}

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
        created_at: Some("2026-06-13T00:00:00Z".to_string()),
        ..PersistedSession::default()
    }
}

fn write_activity_sessions(config_dir: &Path, rows: Vec<PersistedSession>) {
    std::fs::write(
        config_dir.join("sessions.json"),
        serde_json::to_string_pretty(&rows).expect("sessions json"),
    )
    .expect("write sessions.json");
}

/// Create a team and its first room via `team create` + `workgroup add`, so
/// every `__agent_<name>` replica exists exactly as production creates it.
fn create_activity_workgroup(
    tmp: &Path,
    bin: &Path,
    config_dir: &Path,
    agents: &[&str],
) -> (PathBuf, PathBuf) {
    write_settings(config_dir, tmp);
    let project = project_with_agents(tmp, agents);
    let mut team_args: Vec<&str> = vec![
        "team",
        "create",
        "--project",
        "ProjectAlpha",
        "--team",
        "Dev Team",
        "--coordinator",
        agents[0],
    ];
    for agent in &agents[1..] {
        team_args.push("--agent");
        team_args.push(agent);
    }
    run_json(bin, &team_args);
    let created = run_json(
        bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
        ],
    );
    let wg_dir = project.join(".ac").join("room-1-dev-team");
    assert_same_path(created["path"].as_str().expect("path"), &wg_dir);
    (project, wg_dir)
}

fn activity_json(bin: &Path, project: &str) -> Vec<serde_json::Value> {
    let value = run_json(bin, &["room", "activity", "--project", project]);
    value.as_array().cloned().expect("activity array")
}

fn activity_room<'a>(items: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    items
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| panic!("room {name} missing from {items:?}"))
}

#[test]
fn room_activity_lists_rooms_in_order_for_both_spellings() {
    let tmp = Tmp::new("cli-room-activity-shape");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    project_with_agents(tmp.path(), &["architect"]);
    run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
            "--coordinator",
            "architect",
        ],
    );
    run_json(
        &bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
        ],
    );

    let canonical = activity_json(&bin, "ProjectAlpha");
    let alias = run_json(
        &bin,
        &["workgroup", "activity", "--project", "ProjectAlpha"],
    );
    assert_eq!(
        serde_json::Value::Array(canonical.clone()),
        alias,
        "the deprecated spelling reaches the same behavior"
    );

    let names: Vec<&str> = canonical
        .iter()
        .map(|item| item["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        vec!["room-1-dev-team", "room-2-dev-team"],
        "existing room enumeration order"
    );
    for item in &canonical {
        let object = item.as_object().expect("room object");
        assert_eq!(object.len(), 8, "exactly the documented fields: {item}");
        assert_eq!(item["team"], "dev-team");
        assert_eq!(item["working"], false);
        assert_eq!(item["ciState"], "unknown", "no repo-* means unknown");
        assert_eq!(
            item["taskTitle"], "USER: Build",
            "the title decodes from the created TASK.md"
        );
    }
}

#[test]
fn room_activity_missing_project_errors_without_writes() {
    let tmp = Tmp::new("cli-room-activity-missing-project");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let config_sentinel = config_dir.join("sentinel.txt");
    std::fs::write(&config_sentinel, "keep").expect("write config sentinel");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let stderr = run_fail(&bin, &["room", "activity", "--project", "MissingProject"]);

    assert!(stderr.contains("MissingProject"), "stderr was:\n{stderr}");
    assert!(project_refresh_request_paths(&config_dir).is_empty());
    assert_eq!(
        std::fs::read_to_string(&config_sentinel).expect("read config sentinel"),
        "keep"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_no_side_effect_dirs(tmp.path());
}

#[test]
fn room_activity_idle_coordinator_is_not_a_proxy_and_a_member_flips_the_room() {
    let tmp = Tmp::new("cli-room-activity-working");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (_project, wg_dir) =
        create_activity_workgroup(tmp.path(), &bin, &config_dir, &["coord", "dev"]);
    let coord_dir = wg_dir.join("__agent_coord");
    let dev_dir = wg_dir.join("__agent_dev");

    write_activity_sessions(
        &config_dir,
        vec![activity_row(
            "room-1-dev-team/coord",
            &coord_dir,
            SessionStatus::Idle,
            true,
        )],
    );
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["working"],
        false,
        "an idle coordinator must not make its room work"
    );

    write_activity_sessions(
        &config_dir,
        vec![
            activity_row(
                "room-1-dev-team/coord",
                &coord_dir,
                SessionStatus::Idle,
                true,
            ),
            activity_row(
                "room-1-dev-team/dev",
                &dev_dir,
                SessionStatus::Running,
                false,
            ),
        ],
    );
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["working"],
        true,
        "a local non-coordinator working row flips the room true"
    );
}

#[test]
fn room_activity_rejects_foreign_and_boundary_row_aliases() {
    let tmp = Tmp::new("cli-room-activity-boundaries");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (_project, wg_dir) =
        create_activity_workgroup(tmp.path(), &bin, &config_dir, &["coord", "dev"]);
    let dev_dir = wg_dir.join("__agent_dev");

    // Another project with the SAME room and agent directory names: its working
    // session must not color this project's room.
    let foreign_dev = tmp
        .path()
        .join("ProjectBeta")
        .join(".ac")
        .join("room-1-dev-team")
        .join("__agent_dev");
    std::fs::create_dir_all(&foreign_dev).expect("foreign replica");

    write_activity_sessions(
        &config_dir,
        vec![
            // The raw directory name is not the persisted session name.
            activity_row(
                "room-1-dev-team/__agent_dev",
                &dev_dir,
                SessionStatus::Running,
                false,
            ),
            // Similarly prefixed room and agent names are not this room's.
            activity_row(
                "room-1-dev-teams/dev",
                &dev_dir,
                SessionStatus::Running,
                false,
            ),
            activity_row(
                "room-2-dev-team/dev",
                &dev_dir,
                SessionStatus::Running,
                false,
            ),
            activity_row(
                "room-1-dev-team/dev-2",
                &dev_dir,
                SessionStatus::Running,
                false,
            ),
            // Same room and agent names, different normalized working directory.
            activity_row(
                "room-1-dev-team/dev",
                &foreign_dev,
                SessionStatus::Running,
                false,
            ),
        ],
    );
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["working"],
        false,
        "aliases and a foreign working directory must not match"
    );

    write_activity_sessions(
        &config_dir,
        vec![activity_row(
            "room-1-dev-team/dev",
            &dev_dir,
            SessionStatus::Running,
            false,
        )],
    );
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["working"],
        true,
        "the exact local room/agent/path row matches"
    );
}

#[test]
fn room_activity_ci_tri_state_from_a_fresh_live_snapshot() {
    let tmp = Tmp::new("cli-room-activity-ci");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (_project, wg_dir) =
        create_activity_workgroup(tmp.path(), &bin, &config_dir, &["architect"]);
    let repo_a = wg_dir.join("repo-a");
    let repo_b = wg_dir.join("repo-b");
    std::fs::create_dir_all(&repo_a).expect("repo-a");
    std::fs::create_dir_all(&repo_b).expect("repo-b");
    write_live_daemon_pid(&config_dir);

    write_activity_snapshot(
        &config_dir,
        &activity_rfc3339(0),
        &[(&repo_a, "running"), (&repo_b, "idle")],
    );
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "running",
        "one running repo outranks a matched idle one"
    );

    write_activity_snapshot(
        &config_dir,
        &activity_rfc3339(0),
        &[(&repo_a, "idle"), (&repo_b, "idle")],
    );
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "idle",
        "every repo matched idle"
    );

    write_activity_snapshot(&config_dir, &activity_rfc3339(0), &[(&repo_a, "idle")]);
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "unknown",
        "a missing repo match is unknown"
    );

    write_activity_snapshot(
        &config_dir,
        &activity_rfc3339(0),
        &[(&repo_a, "idle"), (&repo_b, "unknown")],
    );
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "unknown",
        "an explicit unknown is unknown"
    );

    std::fs::remove_dir_all(&repo_a).expect("remove repo-a");
    std::fs::remove_dir_all(&repo_b).expect("remove repo-b");
    write_activity_snapshot(&config_dir, &activity_rfc3339(0), &[(&repo_a, "running")]);
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "unknown",
        "no repo-* directories is unknown"
    );
}

#[test]
fn room_activity_forces_unknown_without_a_live_daemon_or_current_bytes() {
    let tmp = Tmp::new("cli-room-activity-gates");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (_project, wg_dir) =
        create_activity_workgroup(tmp.path(), &bin, &config_dir, &["architect"]);
    let repo = wg_dir.join("repo-a");
    std::fs::create_dir_all(&repo).expect("repo-a");
    let running = [(repo.as_path(), "running")];

    // A fresh running snapshot with no daemon.pid: unknown, not running.
    write_activity_snapshot(&config_dir, &activity_rfc3339(0), &running);
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "unknown"
    );

    // A malformed daemon.pid: unknown, not running.
    std::fs::write(config_dir.join("daemon.pid"), "not-a-pid").expect("malformed pid");
    write_activity_snapshot(&config_dir, &activity_rfc3339(0), &running);
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "unknown"
    );

    write_live_daemon_pid(&config_dir);

    // Live daemon, stale bytes: still unknown. The margin is well beyond the
    // 30 s window so a slow test binary can only make the age larger.
    write_activity_snapshot(&config_dir, &activity_rfc3339(-120), &running);
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "unknown"
    );

    // Live daemon, far-future bytes: still unknown. The margin is well beyond
    // the window so scheduling delay cannot pull it back inside.
    write_activity_snapshot(&config_dir, &activity_rfc3339(120), &running);
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "unknown"
    );

    // Positive control: live daemon and current bytes reach the cache.
    write_activity_snapshot(&config_dir, &activity_rfc3339(0), &running);
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "running"
    );

    // Disabled CI dial: the same current, live bytes are ignored.
    let settings_path = config_dir.join("settings.30.instance.no-git.json");
    let mut settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read settings"))
            .expect("settings json");
    settings["ciActivityEnabled"] = serde_json::json!(false);
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).expect("settings json"),
    )
    .expect("write settings");
    write_activity_snapshot(&config_dir, &activity_rfc3339(0), &running);
    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(
        activity_room(&items, "room-1-dev-team")["ciState"],
        "unknown",
        "ciActivityEnabled=false forces unknown"
    );
}

#[test]
fn room_activity_task_title_variants_and_warning_routing() {
    let tmp = Tmp::new("cli-room-activity-task");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (_project, wg_dir) =
        create_activity_workgroup(tmp.path(), &bin, &config_dir, &["architect"]);
    let task_path = wg_dir.join("TASK.md");
    let app_log = config_dir.join("app.log");
    let read_app_log = || std::fs::read_to_string(&app_log).unwrap_or_default();

    // Missing TASK.md is null WITHOUT a warning: stdout parses as JSON and the
    // logger's file gains no room warning.
    std::fs::remove_file(&task_path).expect("remove task");
    let before = read_app_log();
    let items = run_json_machine(&bin, &["room", "activity", "--project", "ProjectAlpha"]);
    assert_eq!(
        activity_room(items.as_array().expect("array"), "room-1-dev-team")["taskTitle"],
        serde_json::Value::Null
    );
    let after_missing = read_app_log();
    assert!(
        !after_missing[before.len()..].contains("TASK.md"),
        "a missing TASK.md must not warn"
    );

    // Quoted and bare titles decode, and the fixture bytes survive untouched.
    for body in ["---\ntitle: 'Quoted'\n---\n", "---\ntitle: Bare\n---\n"] {
        std::fs::write(&task_path, body).expect("write task");
        let items = activity_json(&bin, "ProjectAlpha");
        let expected = if body.contains("Quoted") {
            "Quoted"
        } else {
            "Bare"
        };
        assert_eq!(
            activity_room(&items, "room-1-dev-team")["taskTitle"],
            expected
        );
        assert_eq!(
            std::fs::read_to_string(&task_path).expect("read task"),
            body
        );
    }

    // Invalid UTF-8 under AC_MACHINE_OUTPUT: stdout stays valid JSON and the
    // warning is routed through the logger (app.log), never to stdout.
    let invalid = [0xFF_u8, 0xFE, 0x00];
    std::fs::write(&task_path, invalid).expect("write invalid task");
    let before = read_app_log();
    let items = run_json_machine(&bin, &["room", "activity", "--project", "ProjectAlpha"]);
    assert_eq!(
        activity_room(items.as_array().expect("array"), "room-1-dev-team")["taskTitle"],
        serde_json::Value::Null
    );
    let after_machine = read_app_log();
    assert!(
        after_machine[before.len()..].contains("not valid UTF-8"),
        "AC_MACHINE_OUTPUT must route the warning through the logger"
    );
    assert_eq!(std::fs::read(&task_path).expect("read task"), invalid);

    // Without the machine flag the same warning goes to stderr directly and
    // adds nothing to app.log.
    let items = run_json(&bin, &["room", "activity", "--project", "ProjectAlpha"]);
    assert_eq!(
        activity_room(items.as_array().expect("array"), "room-1-dev-team")["taskTitle"],
        serde_json::Value::Null
    );
    let after_plain = read_app_log();
    assert!(
        !after_plain[after_machine.len()..].contains("not valid UTF-8"),
        "without AC_MACHINE_OUTPUT the warning stays on stderr"
    );
    assert_eq!(std::fs::read(&task_path).expect("read task"), invalid);
}

#[test]
fn room_activity_writes_nothing() {
    let tmp = Tmp::new("cli-room-activity-no-write");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (_project, wg_dir) =
        create_activity_workgroup(tmp.path(), &bin, &config_dir, &["architect"]);
    let repo = wg_dir.join("repo-a");
    std::fs::create_dir_all(&repo).expect("repo-a");
    write_live_daemon_pid(&config_dir);
    write_activity_snapshot(&config_dir, &activity_rfc3339(0), &[(&repo, "idle")]);
    write_activity_sessions(
        &config_dir,
        vec![activity_row(
            "room-1-dev-team/architect",
            &wg_dir.join("__agent_architect"),
            SessionStatus::Running,
            false,
        )],
    );

    let owned_files = [
        config_dir.join("sessions.json"),
        config_dir.join("remote-activity.json"),
        config_dir.join("daemon.pid"),
        wg_dir.join("TASK.md"),
    ];
    let before: Vec<Vec<u8>> = owned_files
        .iter()
        .map(|path| std::fs::read(path).expect("read before"))
        .collect();
    let refreshes_before = project_refresh_request_paths(&config_dir).len();

    let items = activity_json(&bin, "ProjectAlpha");
    assert_eq!(activity_room(&items, "room-1-dev-team")["working"], true);
    // #2473 B6: the filters add no write either.
    let filtered = run_json(
        &bin,
        &[
            "room",
            "activity",
            "--project",
            "ProjectAlpha",
            "--rooms",
            "1",
            "--team",
            "dev-team",
            "--working",
            "true",
            "--ci-state",
            "idle",
            "--title-regex",
            "Build",
            "--hide-clean",
        ],
    );
    assert_eq!(filtered.as_array().expect("array").len(), 1);

    for (path, bytes) in owned_files.iter().zip(&before) {
        assert_eq!(
            &std::fs::read(path).expect("read after"),
            bytes,
            "{} must be byte-identical after room activity",
            path.display()
        );
    }
    assert_eq!(
        project_refresh_request_paths(&config_dir).len(),
        refreshes_before,
        "room activity must not request a project refresh"
    );
}

// ---------------------------------------------------------------------------
// #2473 - `room activity` filters and CI detail fields.
// ---------------------------------------------------------------------------

/// Snapshot entries with the #2473 optional fields.
fn write_detailed_snapshot(config_dir: &Path, generated_at: &str, entries: &[serde_json::Value]) {
    let snapshot = serde_json::json!({
        "schemaVersion": 1,
        "generatedAt": generated_at,
        "repos": entries,
    });
    std::fs::write(
        config_dir.join("remote-activity.json"),
        serde_json::to_string_pretty(&snapshot).expect("snapshot json"),
    )
    .expect("write remote-activity.json");
}

fn write_task_title(room: &Path, title: Option<&str>) {
    let task = room.join("TASK.md");
    match title {
        Some(title) => std::fs::write(&task, format!("---\ntitle: {title}\n---\n")).expect("task"),
        None => {
            let _ = std::fs::remove_file(&task);
        }
    }
}

/// Four rooms over two teams:
/// - room-1-dev-team: `Clean`, idle session, repo-a running (runs 101/102, PR 12)
/// - room-2-dev-team: `Fix #12 parser`, working, repo-b idle
/// - room-3-ops-team: `Docs pass`, not working, repo-c not in the snapshot
/// - room-4-ops-team: no title, working, no repos
struct ActivityFixture {
    _tmp: Tmp,
    bin: PathBuf,
    config_dir: PathBuf,
    rooms: Vec<PathBuf>,
}

fn activity_filter_fixture(label: &str) -> ActivityFixture {
    let tmp = Tmp::new(label);
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let (project, room1) = create_activity_workgroup(tmp.path(), &bin, &config_dir, &["architect"]);
    let ac = project.join(".ac");
    let rooms = vec![
        room1,
        ac.join("room-2-dev-team"),
        ac.join("room-3-ops-team"),
        ac.join("room-4-ops-team"),
    ];
    for room in &rooms[1..] {
        std::fs::create_dir_all(room.join("__agent_dev")).expect("room");
    }
    let repo_a = rooms[0].join("repo-a");
    let repo_b = rooms[1].join("repo-b");
    let repo_c = rooms[2].join("repo-c");
    for repo in [&repo_a, &repo_b, &repo_c] {
        std::fs::create_dir_all(repo).expect("repo");
    }
    write_task_title(&rooms[0], Some("Clean"));
    write_task_title(&rooms[1], Some("Fix #12 parser"));
    write_task_title(&rooms[2], Some("Docs pass"));
    write_task_title(&rooms[3], None);
    write_activity_sessions(
        &config_dir,
        vec![
            activity_row(
                "room-1-dev-team/architect",
                &rooms[0].join("__agent_architect"),
                SessionStatus::Idle,
                true,
            ),
            activity_row(
                "room-2-dev-team/dev",
                &rooms[1].join("__agent_dev"),
                SessionStatus::Running,
                false,
            ),
            activity_row(
                "room-4-ops-team/dev",
                &rooms[3].join("__agent_dev"),
                SessionStatus::Running,
                false,
            ),
        ],
    );
    write_live_daemon_pid(&config_dir);
    let fixture = ActivityFixture {
        _tmp: tmp,
        bin,
        config_dir,
        rooms,
    };
    write_fixture_snapshot(&fixture, 0);
    fixture
}

fn write_fixture_snapshot(fixture: &ActivityFixture, offset_secs: i64) {
    write_detailed_snapshot(
        &fixture.config_dir,
        &activity_rfc3339(offset_secs),
        &[
            serde_json::json!({
                "path": fixture.rooms[0].join("repo-a").to_string_lossy(),
                "ciState": "running",
                "runIds": [102, 101],
                "pullRequests": [12],
            }),
            serde_json::json!({
                "path": fixture.rooms[1].join("repo-b").to_string_lossy(),
                "ciState": "idle",
            }),
        ],
    );
}

fn activity_with(fixture: &ActivityFixture, filters: &[&str]) -> Vec<serde_json::Value> {
    let mut args = vec!["room", "activity", "--project", "ProjectAlpha"];
    args.extend_from_slice(filters);
    run_json(&fixture.bin, &args)
        .as_array()
        .cloned()
        .expect("activity array")
}

fn names_of(items: &[serde_json::Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| item["name"].as_str().expect("name").to_string())
        .collect()
}

const R1: &str = "room-1-dev-team";
const R2: &str = "room-2-dev-team";
const R3: &str = "room-3-ops-team";
const R4: &str = "room-4-ops-team";

#[test]
fn room_activity_each_filter_alone_drops_exactly_the_non_matching_rooms() {
    let fixture = activity_filter_fixture("cli-room-activity-filters");
    let unfiltered = activity_with(&fixture, &[]);
    assert_eq!(names_of(&unfiltered), vec![R1, R2, R3, R4]);
    let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (vec!["--rooms", "3,1,3"], vec![R1, R3]),
        (vec!["--rooms", " 4 "], vec![R4]),
        (vec!["--team", "ops-team"], vec![R3, R4]),
        (vec!["--team", "no-such-team"], vec![]),
        (vec!["--working", "true"], vec![R2, R4]),
        (vec!["--working", "false"], vec![R1, R3]),
        (vec!["--ci-state", "running"], vec![R1]),
        (vec!["--ci-state", "idle"], vec![R2]),
        (vec!["--ci-state", "unknown"], vec![R3, R4]),
        (vec!["--title-regex", "#1\\d"], vec![R2]),
        (vec!["--title-regex", "(?i)^DOCS"], vec![R3]),
        (vec!["--hide-clean"], vec![R2, R3]),
    ];
    for (filters, expected) in cases {
        let filtered = activity_with(&fixture, &filters);
        assert_eq!(names_of(&filtered), expected, "filters {filters:?}");
        // B3 negative control: every filter drops at least one fixture room.
        assert!(
            filtered.len() < unfiltered.len(),
            "filters {filters:?} must drop a room"
        );
    }
}

#[test]
fn room_activity_filters_combine_with_and() {
    let fixture = activity_filter_fixture("cli-room-activity-filters-and");
    let all = activity_with(
        &fixture,
        &[
            "--rooms",
            "1,2,3,4",
            "--team",
            "dev-team",
            "--working",
            "true",
            "--ci-state",
            "idle",
            "--title-regex",
            "Fix",
            "--hide-clean",
        ],
    );
    assert_eq!(names_of(&all), vec![R2]);
    let none = activity_with(&fixture, &["--team", "ops-team", "--ci-state", "running"]);
    assert!(none.is_empty(), "{none:?}");
}

fn run_activity_raw(bin: &Path, filters: &[&str]) -> (Option<i32>, String, String) {
    let mut command = command_for_binary(bin);
    command
        .args(["room", "activity", "--project", "ProjectAlpha"])
        .args(filters)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = child.wait_with_output().expect("collect output");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn room_activity_invalid_filter_values_exit_one_with_empty_stdout() {
    let fixture = activity_filter_fixture("cli-room-activity-filter-errors");
    let rooms = |v: &str| {
        format!(
            "Invalid --rooms '{v}': expected comma-separated positive room numbers, e.g. 5,12,17"
        )
    };
    let cases: Vec<(Vec<&str>, String)> = vec![
        (vec!["--rooms", "-1"], rooms("-1")),
        (vec!["--rooms=-1"], rooms("-1")),
        (vec!["--rooms", "5,,7"], rooms("5,,7")),
        (vec!["--rooms", "0"], rooms("0")),
        (vec!["--rooms", "abc"], rooms("abc")),
        (
            vec!["--team", " "],
            "Invalid --team ' ': expected a team name".to_string(),
        ),
        (
            vec!["--working", "TRUE"],
            "Invalid --working 'TRUE': expected true or false".to_string(),
        ),
        (
            vec!["--ci-state", "Running"],
            "Invalid --ci-state 'Running': expected running, idle or unknown".to_string(),
        ),
        (
            vec!["--title-regex", "("],
            "Invalid --title-regex '(': ".to_string(),
        ),
    ];
    for (filters, expected) in cases {
        let (code, stdout, stderr) = run_activity_raw(&fixture.bin, &filters);
        assert_eq!(code, Some(1), "filters {filters:?}; stderr {stderr}");
        assert!(stdout.is_empty(), "filters {filters:?}; stdout {stdout}");
        assert!(
            stderr.contains(&format!("Error: {expected}")),
            "filters {filters:?}; stderr was:\n{stderr}"
        );
    }
}

#[test]
fn room_activity_keeps_hyphen_leading_valid_values() {
    let fixture = activity_filter_fixture("cli-room-activity-filter-hyphen");
    write_task_title(&fixture.rooms[2], Some("a -x b"));
    assert_eq!(
        names_of(&activity_with(&fixture, &["--title-regex", "-x"])),
        vec![R3]
    );
    assert!(activity_with(&fixture, &["--team", "-t"]).is_empty());
}

#[test]
fn room_activity_no_filter_output_adds_the_three_ci_detail_fields() {
    let fixture = activity_filter_fixture("cli-room-activity-detail");
    let items = activity_with(&fixture, &[]);
    for item in &items {
        let mut keys: Vec<&str> = item
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "ciPullRequests",
                "ciRunIds",
                "ciState",
                "ciUnknownReason",
                "name",
                "taskTitle",
                "team",
                "working",
            ]
        );
    }
    let expect = |name: &str, value: serde_json::Value| {
        assert_eq!(activity_room(&items, name), &value, "{name}");
    };
    expect(
        R1,
        serde_json::json!({
            "name": R1, "team": "dev-team", "working": false, "ciState": "running",
            "taskTitle": "Clean", "ciRunIds": [101, 102], "ciPullRequests": [12],
            "ciUnknownReason": null,
        }),
    );
    expect(
        R2,
        serde_json::json!({
            "name": R2, "team": "dev-team", "working": true, "ciState": "idle",
            "taskTitle": "Fix #12 parser", "ciRunIds": [], "ciPullRequests": [],
            "ciUnknownReason": null,
        }),
    );
    expect(
        R3,
        serde_json::json!({
            "name": R3, "team": "ops-team", "working": false, "ciState": "unknown",
            "taskTitle": "Docs pass", "ciRunIds": [], "ciPullRequests": [],
            "ciUnknownReason": "repo-not-in-snapshot",
        }),
    );
    expect(
        R4,
        serde_json::json!({
            "name": R4, "team": "ops-team", "working": true, "ciState": "unknown",
            "taskTitle": null, "ciRunIds": [], "ciPullRequests": [],
            "ciUnknownReason": "no-repos",
        }),
    );

    write_fixture_snapshot(&fixture, -120);
    let stale = activity_with(&fixture, &[]);
    for item in &stale {
        assert_eq!(item["ciState"], "unknown");
        assert_eq!(item["ciUnknownReason"], "snapshot-unavailable");
    }

    write_fixture_snapshot(&fixture, 0);
    std::fs::remove_file(fixture.config_dir.join("daemon.pid")).expect("remove pid");
    let no_daemon = activity_with(&fixture, &[]);
    for item in &no_daemon {
        assert_eq!(item["ciUnknownReason"], "daemon-not-live");
        assert_eq!(item["ciRunIds"], serde_json::json!([]));
    }
}
