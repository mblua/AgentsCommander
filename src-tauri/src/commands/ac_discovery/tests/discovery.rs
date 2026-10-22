use super::*;

/// #1472 - the RTK runtime-artifact rules must be narrow: they ignore the
/// three artifact families under `_agent_*` roots, and nothing else.
#[test]
fn ensure_ac_root_gitignore_ignores_rtk_runtime_artifacts_narrowly() {
    // Part A: fresh-file path plus real git semantics.
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let fixtures = [
        ".ac/_agent_probe/rtk-matrix-history.db",
        ".ac/_agent_probe/rtk-matrix-history-claude.db",
        ".ac/_agent_probe/rtk-matrix-history-pi.db-shm",
        ".ac/_agent_probe/rtk-matrix-history-pi.db-wal",
        ".ac/_agent_probe/rtk_ignored_tools.md",
        ".ac/_agent_probe/rtk-ignored-tools-pi.md",
        concat!(
            ".ac/_agent_probe/skills/rtk-telemetry-forensics/scripts/",
            "__pycache__/rtk_patterns.cpython-314.pyc"
        ),
        ".ac/_agent_probe/skills/rtk-telemetry-forensics/scripts/__pycache__/non_pyc.bin",
        ".ac/_agent_probe/rtk-matrix-history.sqbpro",
        ".ac/_agent_probe/rtk-ignored-tools.md",
        ".ac/_agent_probe/skills/rtk-telemetry-forensics/SKILL.md",
        ".ac/wg-1/__agent_replica/rtk_ignored_tools.md",
        ".ac/__agent_replica/rtk_ignored_tools.md",
    ];
    for relative in fixtures {
        let path = project.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture has a parent"))
            .expect("create fixture dir");
        std::fs::write(&path, b"fixture").expect("write Git fixture");
    }

    let init = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .output()
        .expect("git init");
    assert!(
        init.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let ignored = |relative: &str| {
        std::process::Command::new("git")
            .args(["check-ignore", "--quiet", "--no-index", "--", relative])
            .current_dir(&project)
            .status()
            .expect("git check-ignore")
            .success()
    };
    for relative in [
        ".ac/_agent_probe/rtk-matrix-history.db",
        ".ac/_agent_probe/rtk-matrix-history-claude.db",
        ".ac/_agent_probe/rtk-matrix-history-pi.db-shm",
        ".ac/_agent_probe/rtk-matrix-history-pi.db-wal",
        ".ac/_agent_probe/rtk_ignored_tools.md",
        ".ac/_agent_probe/rtk-ignored-tools-pi.md",
        concat!(
            ".ac/_agent_probe/skills/rtk-telemetry-forensics/scripts/",
            "__pycache__/rtk_patterns.cpython-314.pyc"
        ),
        ".ac/_agent_probe/skills/rtk-telemetry-forensics/scripts/__pycache__/non_pyc.bin",
        // This path is ignored only because the pre-existing `wg-*/` first
        // entry excludes the whole `wg-1/` subtree, not because any new
        // pattern matches it.
        ".ac/wg-1/__agent_replica/rtk_ignored_tools.md",
    ] {
        assert!(ignored(relative), "{relative} must be ignored");
    }
    for relative in [
        ".ac/_agent_probe/rtk-matrix-history.sqbpro",
        ".ac/_agent_probe/rtk-ignored-tools.md",
        ".ac/_agent_probe/skills/rtk-telemetry-forensics/SKILL.md",
        // Replica-folder shape directly under `.ac/`, outside `wg-*`: pins
        // that no new pattern matches `__agent_*` folder names.
        ".ac/__agent_replica/rtk_ignored_tools.md",
    ] {
        assert!(!ignored(relative), "{relative} must not be ignored");
    }

    // Part B: append path keeps the prefix and appends each new pattern once.
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    std::fs::write(
        ac_root.join(".gitignore"),
        "wg-*/
",
    )
    .expect("write .gitignore");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
    assert_eq!(
        content
            .lines()
            .filter(|line| line.trim() == "wg-*/")
            .count(),
        1,
        "the pre-existing wg-*/ line must survive exactly once"
    );
    for pattern in [
        "_agent_*/rtk-matrix-history*.db",
        "_agent_*/rtk-matrix-history*.db-shm",
        "_agent_*/rtk-matrix-history*.db-wal",
        "_agent_*/rtk_ignored_tools*.md",
        "_agent_*/rtk-ignored-tools-*.md",
        "_agent_*/**/__pycache__/",
        "_agent_*/**/*.pyc",
    ] {
        assert_eq!(
            content
                .lines()
                .filter(|line| line.trim() == pattern)
                .count(),
            1,
            "pattern {pattern} must be appended exactly once"
        );
    }

    // Part C: 7-pattern-only fixture, replicating the Section 4.2 probe.
    // None of the 7 patterns alone matches replica-shaped content, so a
    // path under `wg-1/__agent_replica/` must stay not ignored.
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    let seven_patterns = [
        "_agent_*/rtk-matrix-history*.db",
        "_agent_*/rtk-matrix-history*.db-shm",
        "_agent_*/rtk-matrix-history*.db-wal",
        "_agent_*/rtk_ignored_tools*.md",
        "_agent_*/rtk-ignored-tools-*.md",
        "_agent_*/**/__pycache__/",
        "_agent_*/**/*.pyc",
    ];
    std::fs::write(ac_root.join(".gitignore"), seven_patterns.join("\n") + "\n")
        .expect("write .gitignore");
    let replica = ".ac/wg-1/__agent_replica/rtk_ignored_tools.md";
    let path = project.join(replica);
    std::fs::create_dir_all(path.parent().expect("fixture has a parent"))
        .expect("create fixture dir");
    std::fs::write(&path, b"fixture").expect("write Git fixture");

    let init = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .output()
        .expect("git init");
    assert!(
        init.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let ignored = std::process::Command::new("git")
        .args(["check-ignore", "--quiet", "--no-index", "--", replica])
        .current_dir(&project)
        .status()
        .expect("git check-ignore")
        .success();
    assert!(
        !ignored,
        "{replica} must not be ignored by the 7 patterns alone"
    );
}

// ── #1941 strict selection state in both discovery constructors ─────

struct DiscoverySelectionFixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    app: tauri::App,
    settings: SettingsState,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    branch_watcher: Arc<DiscoveryBranchWatcher>,
    clocks: crate::config::coordinator_clocks::CoordinatorClocksState,
}

fn discovery_selection_fixture() -> DiscoverySelectionFixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let room = ac_root.join("room-1-team");
    let replicas: [(&str, Value); 4] = [
        (
            "locked",
            json!({"profile": "A", "currentCodingAgent": "codex", "selectionLocked": true}),
        ),
        (
            "unlocked",
            json!({"profile": "B", "currentCodingAgent": "codex"}),
        ),
        (
            "fallback",
            json!({"currentCodingAgent": "codex", "instanceProfileOverride": "C"}),
        ),
        ("invalid", json!({"selectionLocked": true})),
    ];
    for (name, tooling) in replicas {
        let matrix = ac_root.join(format!("_agent_{name}"));
        let replica = room.join(format!("__agent_{name}"));
        std::fs::create_dir_all(&matrix).expect("create matrix");
        std::fs::create_dir_all(&replica).expect("create replica");
        std::fs::write(matrix.join("Role.md"), "# Role\n").expect("write Role.md");
        std::fs::write(
            replica.join("config.json"),
            serde_json::to_vec(&json!({
                "identity": format!("../../_agent_{name}"),
                "tooling": tooling,
            }))
            .expect("serialize replica config"),
        )
        .expect("write replica config");
    }
    let settings: SettingsState = crate::config::settings::new_settings_state(AppSettings {
        agents: vec![crate::config::settings::AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "codex".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Codex".to_string(),
            command: "codex".to_string(),
            color: "#000000".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        project_paths: vec![project.to_string_lossy().to_string()],
        ..AppSettings::default()
    });
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let clocks: crate::config::coordinator_clocks::CoordinatorClocksState = Arc::new(
        std::sync::Mutex::new(crate::config::coordinator_clocks::CoordinatorClocks::default()),
    );
    let app = crate::test_support::test_builder()
        .manage(settings.clone())
        .manage(Arc::clone(&session_mgr))
        .manage(clocks.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build discovery test app");
    let branch_watcher =
        DiscoveryBranchWatcher::new(app.handle().clone(), Arc::clone(&session_mgr));
    assert!(app.manage(Arc::clone(&branch_watcher)));
    DiscoverySelectionFixture {
        _temp: temp,
        project,
        app,
        settings,
        session_mgr,
        branch_watcher,
        clocks,
    }
}

fn assert_discovery_selection_wire(result: &AcDiscoveryResult) {
    let room = result
        .workgroups
        .iter()
        .find(|wg| wg.name == "room-1-team")
        .expect("room discovered");
    let agent = |name: &str| {
        room.agents
            .iter()
            .find(|agent| agent.name == name)
            .unwrap_or_else(|| panic!("replica {name} discovered"))
    };

    let locked = agent("locked");
    assert_eq!(locked.selection_state.as_deref(), Some("locked"));
    let pair = locked.saved_pair.as_ref().expect("locked pair");
    assert_eq!(pair.coding_agent_id.as_deref(), Some("codex"));
    assert_eq!(pair.requested_profile.as_deref(), Some("A"));
    assert!(locked.selection_error.is_none());
    assert_eq!(locked.current_coding_agent_id.as_deref(), Some("codex"));
    assert_eq!(locked.current_profile.as_deref(), Some("A"));

    let unlocked = agent("unlocked");
    assert_eq!(unlocked.selection_state.as_deref(), Some("unlocked"));
    assert_eq!(
        unlocked
            .saved_pair
            .as_ref()
            .expect("unlocked pair")
            .requested_profile
            .as_deref(),
        Some("B")
    );
    assert!(unlocked.selection_error.is_none());

    let invalid = agent("invalid");
    assert_eq!(invalid.selection_state.as_deref(), Some("invalid"));
    assert!(invalid.saved_pair.is_none());
    assert!(
        invalid.selection_error.is_some(),
        "an invalid protection state carries its diagnostic"
    );

    // The existing resolver behavior is preserved: the legacy
    // instanceProfileOverride still resolves as the current profile, and
    // the strict pair reports the same letter.
    let fallback = agent("fallback");
    assert_eq!(fallback.current_profile.as_deref(), Some("C"));
    assert_eq!(fallback.selection_state.as_deref(), Some("unlocked"));
    assert_eq!(
        fallback
            .saved_pair
            .as_ref()
            .expect("fallback pair")
            .requested_profile
            .as_deref(),
        Some("C")
    );
}

#[tokio::test]
async fn issue_1937_selection_api_discovery_constructors_report_strict_selection_state() {
    let fixture = discovery_selection_fixture();
    let app_handle = fixture.app.handle().clone();
    let result = discover_ac_agents(
        app_handle.clone(),
        fixture
            .app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>(),
        fixture.app.state::<SettingsState>(),
        fixture.app.state::<Arc<DiscoveryBranchWatcher>>(),
        fixture
            .app
            .state::<crate::config::coordinator_clocks::CoordinatorClocksState>(),
    )
    .await
    .expect("discover_ac_agents");
    assert_discovery_selection_wire(&result);

    let project = fixture.project.to_string_lossy().to_string();
    let direct = discover_project_inner(
        &app_handle,
        &fixture.session_mgr,
        &project,
        &fixture.settings,
        &fixture.branch_watcher,
        &fixture.clocks,
    )
    .await
    .expect("discover_project_inner");
    assert_discovery_selection_wire(&direct);
}

// #2434 - instructions-filename rows and the discovery matcher.

fn agent_rows(content: &str) -> Vec<&str> {
    content
        .lines()
        .filter(|line| line.starts_with("**/__agent_*/"))
        .collect()
}

#[test]
fn gitignore_rows_cover_a_custom_instructions_filename() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let names = vec!["CLAUDE.md".into(), "AGENTS.md".into(), "Squad.md".into()];
    ensure_ac_root_gitignore_with_names(tmp.path(), &names).expect("ensure");
    let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    for row in [
        "**/__agent_*/Squad.md",
        "**/__agent_*/CLAUDE.md",
        "**/__agent_*/AGENTS.md",
    ] {
        assert!(content.lines().any(|l| l == row), "missing {row}");
    }

    // An existing floor-only file gains the row; nothing is removed.
    let existing = tempfile::tempdir().expect("tempdir");
    ensure_ac_root_gitignore_with_names(existing.path(), &floor_names()).expect("floor");
    let before = std::fs::read_to_string(existing.path().join(".gitignore")).unwrap();
    assert!(!before.contains("Squad.md"));
    ensure_ac_root_gitignore_with_names(existing.path(), &["Squad.md".into()]).expect("add");
    let after = std::fs::read_to_string(existing.path().join(".gitignore")).unwrap();
    assert!(after.starts_with(&before), "the writer is purely additive");
    assert!(after.lines().any(|l| l == "**/__agent_*/Squad.md"));
}

#[test]
fn gitignore_escapes_glob_metacharacters() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    let replica = ac_root.join("replicas").join("__agent_dev");
    std::fs::create_dir_all(&replica).expect("replica dir");
    ensure_ac_root_gitignore_with_names(&ac_root, &["[Squad].md".into()]).expect("ensure");
    let content = std::fs::read_to_string(ac_root.join(".gitignore")).unwrap();
    assert!(
        content.lines().any(|l| l == r"**/__agent_*/\[Squad\].md"),
        "{content}"
    );

    let init_status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .status()
        .expect("git init must execute");
    assert!(init_status.success(), "git init must succeed");
    let empty_excludes = project.join("empty-global-excludes");
    std::fs::write(&empty_excludes, []).expect("empty excludes");
    std::fs::write(replica.join("[Squad].md"), "x").expect("literal file");
    // A file the raw class `[Squad].md` WOULD match must stay unignored.
    std::fs::write(replica.join("S.md"), "x").expect("class-match file");
    let excludes_override = format!(
        "core.excludesFile={}",
        empty_excludes.to_string_lossy().replace('\\', "/")
    );
    let check = |target: &str| {
        std::process::Command::new("git")
            .arg("-c")
            .arg(&excludes_override)
            .args(["check-ignore", "-v", "--no-index", "--", target])
            .current_dir(&project)
            .output()
            .expect("git check-ignore must execute")
    };
    let literal = check(".ac/replicas/__agent_dev/[Squad].md");
    assert!(
        literal.status.success(),
        "the literal file must be ignored: {}",
        String::from_utf8_lossy(&literal.stderr)
    );
    let stdout = String::from_utf8(literal.stdout).expect("utf-8");
    assert!(
        stdout.contains(r":**/__agent_*/\[Squad\].md"),
        "matched by the escaped row: {stdout}"
    );
    let class = check(".ac/replicas/__agent_dev/S.md");
    assert_eq!(class.status.code(), Some(1), "S.md must not be ignored");
}

#[test]
fn gitignore_rows_are_idempotent_for_custom_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let names = vec!["Squad.md".into(), "[Squad].md".into(), "Squad.md".into()];
    ensure_ac_root_gitignore_with_names(tmp.path(), &names).expect("first");
    let first = std::fs::read(tmp.path().join(".gitignore")).unwrap();
    ensure_ac_root_gitignore_with_names(tmp.path(), &names).expect("second");
    let second = std::fs::read(tmp.path().join(".gitignore")).unwrap();
    assert_eq!(first, second);
    let content = String::from_utf8(first).unwrap();
    assert_eq!(
        content
            .lines()
            .filter(|l| *l == "**/__agent_*/Squad.md")
            .count(),
        1
    );
}

#[test]
fn gitignore_drops_an_unsafe_name() {
    let tmp = tempfile::tempdir().expect("tempdir");
    ensure_ac_root_gitignore_with_names(tmp.path(), &["../escape.md".into(), "Squad.md".into()])
        .expect("ensure");
    let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(content.lines().any(|l| l == "**/__agent_*/Squad.md"));
    assert!(!content.contains("escape"), "{content}");
    for row in agent_rows(&content) {
        let name = row.strip_prefix("**/__agent_*/").unwrap();
        assert!(
            !name.contains('/') && !name.contains('\\') && !name.contains(".."),
            "{row}"
        );
    }
}

#[test]
fn resolved_names_never_reach_the_writer_unsafe() {
    let mut unsafe_agent = matcher_agent("codex", "Codex", "codex");
    unsafe_agent.instructions_filename = Some("../escape.md".to_string());
    let settings = AppSettings {
        agents: vec![unsafe_agent],
        ..AppSettings::default()
    };
    let names = crate::config::agent_command::managed_instructions_filenames(&settings);
    assert_eq!(names, vec!["AGENTS.md".to_string()]);
    let tmp = tempfile::tempdir().expect("tempdir");
    ensure_ac_root_gitignore_with_names(tmp.path(), &names).expect("ensure");
    let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(!content.contains("escape"));
}

fn matcher_agent(id: &str, label: &str, command: &str) -> crate::config::settings::AgentConfig {
    crate::config::settings::AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: label.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

#[test]
fn discovery_returns_none_and_warns_on_no_match() {
    let settings = AppSettings {
        agents: vec![
            matcher_agent("local-claude", "Claude Code", "claude"),
            matcher_agent("local-codex", "Codex", "codex"),
        ],
        ..AppSettings::default()
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let write = |name: &str, config: Value| {
        let dir = tmp.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
        dir
    };
    let unknown = write(
        "_agent_unknown",
        json!({"tooling": {"lastCodingAgent": "foreign",
            "codingAgents": {"foreign": {"app": "Gemini", "command": "gemini"}}}}),
    );
    let lowercase = write(
        "_agent_lower",
        json!({"tooling": {"lastCodingAgent": "their-codex",
            "codingAgents": {"their-codex": {"app": "codex"}}}}),
    );

    let mut warnings = Vec::new();
    assert_eq!(
        read_preferred_agent_id_logged(&unknown, &settings, &mut warnings),
        None,
        "no other agent is substituted"
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("'foreign'"), "{warnings:?}");

    // The next directory still resolves: discovery keeps listing, and the
    // label now matches case-insensitively.
    let mut warnings = Vec::new();
    assert_eq!(
        read_preferred_agent_id_logged(&lowercase, &settings, &mut warnings).as_deref(),
        Some("local-codex")
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// #2717 E10: the renamed project settings, both lock sidecars and a set-aside
/// file are each ignored by their own anchored rule; the retired row is gone,
/// and a user-authored bare pattern survives.
#[test]
fn ac_root_gitignore_covers_the_renamed_project_settings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    std::fs::write(
        ac_root.join(".gitignore"),
        "# User rules\nproject-settings.json\n\n# AgentsCommander: exclude generated project-local \
            settings.\n/project-settings.json\n",
    )
    .expect("seed .gitignore");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read");
    assert!(
        !content.lines().any(|line| line == "/project-settings.json"),
        "the retired row is still there"
    );
    assert!(content
        .lines()
        .any(|line| line == "/.project-settings.json.lock"));
    assert!(
        content.lines().any(|line| line == "project-settings.json"),
        "a user-authored bare pattern was eaten"
    );

    let init = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .status()
        .expect("git init must execute");
    assert!(init.success());
    let empty_excludes = project.join("empty-global-excludes");
    std::fs::write(&empty_excludes, []).expect("empty excludes");
    let excludes_override = format!(
        "core.excludesFile={}",
        empty_excludes.to_string_lossy().replace('\\', "/")
    );
    for (relative, rule) in [
        (
            ".ac/settings.50.personal.no-git.json",
            "/settings.50.personal.no-git.json",
        ),
        (
            ".ac/.project-settings.json.lock",
            "/.project-settings.json.lock",
        ),
        (
            ".ac/.settings.50.personal.no-git.json.lock",
            "/.settings.50.personal.no-git.json.lock",
        ),
        (
            ".ac/project-settings.json.deprecated-1.no-git",
            "/*.deprecated-*.no-git",
        ),
    ] {
        std::fs::write(project.join(relative), []).expect("fixture file");
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg(&excludes_override)
            .args(["check-ignore", "-v", "--no-index", "--", relative])
            .current_dir(&project)
            .output()
            .expect("git check-ignore must execute");
        assert!(output.status.success(), "{relative} is not ignored");
        let stdout = String::from_utf8(output.stdout).expect("UTF-8");
        let (source_and_pattern, _) = stdout
            .trim_end()
            .split_once('\t')
            .expect("verbose output has a tab");
        let pattern = source_and_pattern.rsplit(':').next().expect("pattern");
        assert_eq!(pattern, rule, "{relative} matched the wrong rule");
        assert!(
            source_and_pattern.starts_with(".ac/.gitignore:"),
            "{relative}: {source_and_pattern}"
        );
    }
}

/// #2718 E14: the renamed Loop state, its write temporary and a set-aside
/// state file are each ignored by their own rule; the retired row is gone
/// and a user-authored bare pattern survives.
#[test]
fn ac_root_gitignore_covers_the_renamed_loop_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(ac_root.join("_loop_a")).expect("create loop dir");
    std::fs::write(
        ac_root.join(".gitignore"),
        "# User rules\n_loop_*/state.json\n\n# AgentsCommander: exclude Loop scheduler runtime \
            state.\n_loop_*/state.json\n",
    )
    .expect("seed .gitignore");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read");
    assert!(
        !content.contains(
            "# AgentsCommander: exclude Loop scheduler runtime state.\n_loop_*/state.json"
        ),
        "the retired row is still there"
    );
    assert_eq!(
        content
            .lines()
            .filter(|line| *line == "_loop_*/state.json")
            .count(),
        1,
        "a user-authored bare pattern was eaten"
    );

    let init = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .status()
        .expect("git init must execute");
    assert!(init.success());
    for (relative, rule) in [
        (
            ".ac/_loop_a/loop.state.no-git.json",
            "_loop_*/loop.state.no-git.json",
        ),
        (
            ".ac/_loop_a/loop.state.no-git.json.1.tmp",
            "_loop_*/loop.state.no-git.json.*.tmp",
        ),
        (
            ".ac/_loop_a/state.json.deprecated-1.no-git",
            "_loop_*/*.deprecated-*.no-git",
        ),
    ] {
        std::fs::write(project.join(relative), []).expect("fixture file");
        let output = std::process::Command::new("git")
            .args(["check-ignore", "-v", "--no-index", "--", relative])
            .current_dir(&project)
            .output()
            .expect("git check-ignore must execute");
        assert!(output.status.success(), "{relative} is not ignored");
        let stdout = String::from_utf8(output.stdout).expect("UTF-8");
        let (source_and_pattern, _) = stdout
            .trim_end()
            .split_once('\t')
            .expect("verbose output has a tab");
        let pattern = source_and_pattern.rsplit(':').next().expect("pattern");
        assert_eq!(pattern, rule, "{relative} matched the wrong rule");
    }
}
// ---------------------------------------------------------------- #2786 C1
// E14: the id and the descriptor come from one merged snapshot. The state
// file's descriptor (`new`) must win over the tracked one (`old`).
#[test]
fn the_id_and_the_descriptor_come_from_one_snapshot() {
    use crate::config::agent_config::load_probe_harness as h;
    let tmp = tempfile::tempdir().expect("tempdir");
    h::write_snapshot_fixture(tmp.path(), "lastCodingAgent", "new");
    let mut warnings = Vec::new();
    let got = read_preferred_agent_id_logged(tmp.path(), &h::snapshot_settings(), &mut warnings);
    assert_eq!(got.as_deref(), Some("via-new"), "{warnings:?}");
}

// E15: the parent row is ordinary and unarmed; the inner row runs alone,
// single-threaded, in a dedicated ARMED process and calls the REAL caller.
// Base at fd32c052 was RED, n=2 (a second load inside `from_config`); the
// single-snapshot change makes it one load of the caller's own directory.
#[test]
fn read_preferred_agent_id_logged_loads_once_from_its_own_dir() {
    let out = crate::config::agent_config::load_probe_harness::expect_child_pass(
        "commands::ac_discovery::tests::discovery::inner_read_preferred_agent_id_logged_loads_once",
    );
    assert!(out.contains("MEASURED-LOADS n=1"), "{out}");
}

#[test]
#[ignore = "#2786 C1: runs only in the dedicated armed process"]
fn inner_read_preferred_agent_id_logged_loads_once() {
    use crate::config::agent_config::load_probe::Observation;
    use crate::config::agent_config::load_probe_harness as h;
    let settings = h::snapshot_settings();
    let call = |dir: &std::path::Path, want: &str| {
        let obs = Observation::open();
        let mut warnings = Vec::new();
        let got = read_preferred_agent_id_logged(dir, &settings, &mut warnings);
        println!("REACHED-CALLER got={got:?} warnings={warnings:?}");
        let loads = obs.loads();
        println!("MEASURED-LOADS n={} {:?}", loads.len(), loads);
        let expected = vec![dir.to_path_buf()];
        assert_eq!(
            loads, expected,
            "recorded {loads:?}, expected exactly {expected:?}"
        );
        assert_eq!(got.as_deref(), Some(want), "{warnings:?}");
    };

    // E14's fixture: exactly one load, of this directory.

    println!("LEG main");
    let tmp = tempfile::tempdir().expect("tempdir");
    h::write_snapshot_fixture(tmp.path(), "lastCodingAgent", "new");
    call(tmp.path(), "via-new");

    // Unkeyed cache leg: two fresh directories with different values.

    println!("LEG unkeyed");
    for command in ["new-a", "new-b"] {
        let tmp = tempfile::tempdir().expect("tempdir");
        h::write_snapshot_fixture(tmp.path(), "lastCodingAgent", command);
        call(tmp.path(), &format!("via-{command}"));
    }

    // Keyed cache leg: the SAME directory, state rewritten between calls.

    println!("LEG keyed");
    let tmp = tempfile::tempdir().expect("tempdir");
    h::write_snapshot_fixture(tmp.path(), "lastCodingAgent", "new-a");
    call(tmp.path(), "via-new-a");
    h::write_snapshot_fixture(tmp.path(), "lastCodingAgent", "new-b");
    call(tmp.path(), "via-new-b");
}

/// C4 (#2470): the seeded state file of every row that seeds one. All four
/// state keys, plus the exact D7 marker `stamp_split_marker` writes.
#[rustfmt::skip]
const C4_SEEDED_STATE: &str =
    concat!(
        r#"{"tooling":{"lastCodingAgent":"codex","codingAgents":{},"#,
        r#""lastAgentMessageAt":"2026-01-01T00:00:00Z","profileContentHash":"h"},"#,
        r#""split":{"v":1,"keys":["lastCodingAgent","codingAgents","#,
        r#""lastAgentMessageAt","profileContentHash"]}}"#
    );

/// C4 (#2470): the state file beside a tracked `config.json`.
fn c4_state_path(tracked: &std::path::Path) -> std::path::PathBuf {
    tracked.with_file_name(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
}

/// C4 (#2470): after one writer, the tracked file carries none of the four
/// state keys. This says nothing at all about the state file: `c4_state_is`
/// is the only helper that reads it, and the row calls them in that order.
fn c4_tracked_is_clean(tracked: &std::path::Path, site: &str) {
    let text = std::fs::read_to_string(tracked).expect("tracked file");
    let value: serde_json::Value = serde_json::from_str(&text).expect("tracked json");
    let tooling = value.get("tooling");
    c4_absent(tooling, "lastCodingAgent", site, &value);
    c4_absent(tooling, "codingAgents", site, &value);
    c4_absent(tooling, "lastAgentMessageAt", site, &value);
    c4_absent(tooling, "profileContentHash", site, &value);
}

fn c4_absent(t: Option<&serde_json::Value>, key: &str, site: &str, all: &serde_json::Value) {
    assert!(
        t.and_then(|t| t.get(key)).is_none(),
        "{site} left {key}: {all}"
    );
}

/// C4 (#2470): the state file EXISTS and is exactly `expected`, compared
/// as parsed JSON so the pretty-printing of `publish_pair_side` is never
/// the subject. `expected` is always a literal written in the row. Absence
/// is NOT admitted here and no longer reads as JSON `null`:
/// `c4_no_state_file` is the only helper that may conclude absence.
fn c4_state_is(state: &std::path::Path, expected: &str, site: &str) {
    let want: serde_json::Value = serde_json::from_str(expected).expect("expected json");
    let raw = std::fs::read_to_string(state).expect("state file");
    let got: serde_json::Value = serde_json::from_str(&raw).expect("state json");
    assert_eq!(got, want, "{site} state file");
}

/// C4 (#2470) E2, site 4: the `context` write and its removal leave the
/// tracked file without a state key and the state file untouched.
#[tokio::test]
async fn c4_site_4_writes_context_and_leaves_no_state_key() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let journal = tempfile::tempdir().expect("journal dir");
    let _journal = crate::config::agent_config::journal_redirect::set(journal.path());
    let dir = tmp.path();
    let tracked = dir.join("config.json");
    std::fs::write(&tracked, r#"{"tooling": {}}"#).expect("seed tracked");
    std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).expect("seed state");
    let read = |path: &Path| -> Value {
        let raw = std::fs::read_to_string(path).expect("read config");
        serde_json::from_str(&raw).expect("parse config")
    };

    let result = set_replica_context_files(
        dir.to_string_lossy().to_string(),
        vec!["Role.md".to_string()],
    )
    .await;
    assert_eq!(result, Ok(()));
    c4_tracked_is_clean(&tracked, "site 4 insert");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 4 insert");
    assert_eq!(read(&tracked)["context"], json!(["Role.md"]));

    let result = set_replica_context_files(dir.to_string_lossy().to_string(), Vec::new()).await;
    assert_eq!(result, Ok(()));
    c4_tracked_is_clean(&tracked, "site 4 remove");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 4 remove");
    assert!(read(&tracked).get("context").is_none());
}
