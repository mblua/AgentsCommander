use super::{
    assert_same_path, command_for_binary, config_dir_for_bin, copy_binary_into,
    project_with_agents, run_json, run_stdout, spawn_lock, write_settings, Path, PathBuf, Stdio,
    Tmp,
};

/// AC3 and requirement (A): creation produces a Room.
#[test]
fn room_add_creates_a_room_directory() {
    let tmp = Tmp::new("cli-room-add");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);

    // The canonical spelling, not the deprecated alias.
    let created = run_json(
        &bin,
        &[
            "room",
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

    let room_dir = project.join(".ac").join("room-1-dev-team");
    assert_same_path(created["path"].as_str().expect("path"), &room_dir);
    assert!(room_dir.is_dir(), "room-1-dev-team must exist on disk");

    // Requirement (A) is unqualified: no production path may produce a wg-*
    // directory, so the legacy name must NOT also appear.
    assert!(
        !project.join(".ac").join("wg-1-dev-team").exists(),
        "creation must never produce a wg-* directory"
    );

    // (E): the parent-repo exclusion must already carry room-*/, and it is
    // written BEFORE the directory is created, so it is present on the very
    // first Room.
    let ignore = std::fs::read_to_string(project.join(".ac").join(".gitignore"))
        .expect("read .ac/.gitignore");
    assert!(
        ignore.lines().any(|l| l.trim() == "room-*/"),
        ".ac/.gitignore must carry room-*/:\n{ignore}"
    );
    assert!(
        ignore.lines().any(|l| l.trim() == "wg-*/"),
        ".ac/.gitignore must keep wg-*/:\n{ignore}"
    );
}

/// Requirement (F): the deprecated subcommand spelling is the same command.
/// Byte-identical stdout on the same root.
#[test]
fn room_list_and_workgroup_list_produce_identical_stdout() {
    let tmp = Tmp::new("cli-room-list-alias");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    project_with_agents(tmp.path(), &["architect"]);
    run_json(
        &bin,
        &[
            "room",
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

    let canonical = run_stdout(&bin, &["room", "list", "--project", "ProjectAlpha"]);
    let deprecated = run_stdout(&bin, &["workgroup", "list", "--project", "ProjectAlpha"]);

    assert_eq!(
        canonical, deprecated,
        "`room list` and `workgroup list` must produce byte-identical stdout"
    );
    assert!(
        canonical.contains("room-1-dev-team"),
        "stdout should name the Room:\n{canonical}"
    );
}

/// AC4: the mixed root. A legacy Workgroup and a Room, both at slot 1, are two
/// distinct entities distinguished by their full directory name (residual R1).
#[test]
fn room_list_reports_a_mixed_root() {
    let tmp = Tmp::new("cli-room-list-mixed");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_agents(tmp.path(), &["architect"]);

    // A pre-existing legacy Workgroup, seeded on disk exactly as an older
    // release left it. Rule P2: it stays wg-*, it is not converted.
    let legacy = project.join(".ac").join("wg-1-dev-team");
    std::fs::create_dir_all(legacy.join("__agent_architect")).expect("seed legacy wg");

    run_json(
        &bin,
        &[
            "room",
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

    // (B): the Room allocator does not count wg-*, so the new Room is slot 1
    // even though wg-1-dev-team already holds slot 1.
    assert!(
        project.join(".ac").join("room-1-dev-team").is_dir(),
        "the Room allocator must ignore legacy wg-* slots"
    );

    let listed = run_json(&bin, &["room", "list", "--project", "ProjectAlpha"]);
    let items = listed.as_array().expect("list returns an array");
    let mut names: Vec<&str> = items
        .iter()
        .map(|i| i["name"].as_str().expect("name"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["room-1-dev-team", "wg-1-dev-team"],
        "a mixed root must list both kinds"
    );

    // Both teams resolve, from the suffix, through the same parser.
    for item in items {
        assert_eq!(
            item["team"].as_str().expect("team"),
            "dev-team",
            "both entities derive the same team: {item}"
        );
    }

    // The legacy directory is never renamed, moved or removed (no migration).
    assert!(
        legacy.is_dir(),
        "the legacy Workgroup must survive untouched"
    );
    assert!(legacy.join("__agent_architect").is_dir());
}

/// Requirement (F)'s committed regression gate, and AC6's observable half.
///
/// The obstacle round 5 recorded is a WAITING problem, not an observability
/// one: `cli/purge_wg.rs` writes the outbox file and only THEN enters the
/// response wait. So the artifact is observable without a daemon by spawning
/// the child, polling for the file, and killing the child rather than sitting
/// out the wait (which is a fixed 90s on the non-graceful path, not
/// `--timeout`; the plan's note says `--timeout` and that holds only under
/// `--graceful`).
#[test]
fn purge_room_and_purge_wg_produce_identical_outbox_messages() {
    let tmp = Tmp::new("cli-purge-alias");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());

    // A session token is any UUID; nothing before the outbox write verifies an
    // orchestrator identity, which is what makes this observable offline.
    let token = "00000000-0000-0000-0000-000000001614";

    let mut payloads = Vec::new();
    for (sub, flag, slot) in [
        ("purge-wg", "--wg", "a"),
        ("purge-room", "--room", "b"),
        ("purge-wg", "--room", "c"),
        ("purge-room", "--wg", "d"),
    ] {
        // A root per invocation, so the four runs cannot race on one outbox.
        let root = tmp.path().join(format!("root-{}", slot));
        std::fs::create_dir_all(&root).expect("create agent root");

        let mut command = command_for_binary(&bin);
        command
            .args([
                sub,
                "--token",
                token,
                "--root",
                root.to_string_lossy().as_ref(),
                flag,
                "room-1-dev-team",
                "--dry-run",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = {
            let _guard = spawn_lock();
            command.spawn().expect("spawn purge")
        };

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut found = None;
        while std::time::Instant::now() < deadline {
            let files = outbox_files(&root);
            if !files.is_empty() {
                found = Some(files[0].clone());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();

        let path = found.unwrap_or_else(|| {
            panic!(
                "{sub} {flag}: no outbox message was written under {}",
                root.display()
            )
        });
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read outbox"))
                .expect("outbox json");

        // The wire value is Rule P0 (section 3.10, D13) and must survive the
        // rename verbatim, or an in-flight message breaks across versions.
        assert_eq!(
            v["action"].as_str(),
            Some("purge-wg"),
            "{sub} {flag}: the outbox action value is a wire value and must not move"
        );
        assert_eq!(
            v["target"].as_str(),
            Some("room-1-dev-team"),
            "{sub} {flag}: both flag spellings must bind the identical value"
        );

        // The three fields that are unique per invocation by construction.
        // OutboxMessage is `#[serde(rename_all = "camelCase")]`, so the wire
        // key is `requestId`; asserting the wire spelling is part of the point.
        for unique in ["id", "requestId", "timestamp"] {
            assert!(
                !v[unique].is_null(),
                "{sub} {flag}: {unique} should be present before it is erased"
            );
            v[unique] = serde_json::Value::Null;
        }
        // `from` is derived from --root, which differs per invocation.
        v["from"] = serde_json::Value::Null;
        payloads.push((format!("{sub} {flag}"), v));
    }

    let (first_label, first) = &payloads[0];
    for (label, v) in &payloads[1..] {
        assert_eq!(
            first, v,
            "`{label}` and `{first_label}` must produce identical outbox messages \
             once id, request_id, timestamp and the per-run root are erased"
        );
    }
}

/// Every `<root>/<agent local dir>/outbox/*.json`, sorted. The local dir name
/// is derived from the binary's own stem, so it is discovered rather than
/// hard-coded.
fn outbox_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let outbox = entry.path().join("outbox");
        let Ok(files) = std::fs::read_dir(&outbox) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// #2374 - `room activity` black-box fixtures and tests.
// ---------------------------------------------------------------------------
