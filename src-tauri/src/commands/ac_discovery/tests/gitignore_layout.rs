use super::*;

/// #1614 AC8, requirement (E). Creation moves to `room-`, so the Project AC
/// Root ignore file needs a `room-*/` entry or every Room created inside a
/// git-tracked project becomes tracked content of the PARENT repository and
/// the next parent `git checkout` or `git reset --hard` corrupts the child
/// clones. That is a data-loss defect, not a cosmetic one.
#[test]
fn ensure_ac_root_gitignore_creates_both_patterns() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
    assert!(
        content.lines().any(|line| line.trim() == "room-*/"),
        "a fresh Project AC Root .gitignore must exclude room-*/ from parent git tracking"
    );
    assert!(
        content.lines().any(|line| line.trim() == "wg-*/"),
        "the legacy wg-*/ exclusion must survive; existing Rooms are still supported"
    );
}

/// #1968 - the project `.ac/.gitignore` carries the narrow coding-agent
/// sidecar rules. The patterns are literals here (an independent oracle;
/// production composes them from the registry), so this fixture pins the intended
/// policy: every local/migration/temp sidecar is excluded, while the tracked
/// base `agents.json` and the `_seed/` masters stay visible, and no broad
/// `coding-agents/` rule exists.
#[test]
fn managed_catalog_ac_root_gitignore_carries_sidecar_rules() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
    for pattern in [
        "/coding-agents/agents.50.personal.no-git.json",
        "/coding-agents/agents.migration-v1.backup.json",
        "/coding-agents/.agents.migration-v1.json",
        "/coding-agents/.agents.10.default.json.lock",
        "/coding-agents/.agents.10.default.json.*.tmp",
        "/coding-agents/.agents.50.personal.no-git.json.*.tmp",
        "/coding-agents/.agents.migration-v1.backup.json.*.tmp",
        "/coding-agents/..agents.migration-v1.json.*.tmp",
        "/coding-agents/.agents.json.lock",
        "/coding-agents/*.deprecated-*.no-git",
    ] {
        assert_eq!(
            content
                .lines()
                .filter(|line| line.trim() == pattern)
                .count(),
            1,
            "workspace .gitignore must carry {pattern} exactly once"
        );
    }
    for broad in [
        "/coding-agents/*",
        "/coding-agents",
        "coding-agents/",
        "/coding-agents/agents.json",
    ] {
        assert!(
            !content.lines().any(|line| line.trim() == broad),
            "workspace .gitignore must not carry the broad rule {broad}"
        );
    }

    // Idempotent: a second ensure appends nothing.
    let before = content;
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names()).expect("second ensure");
    assert_eq!(
        std::fs::read_to_string(ac_root.join(".gitignore")).expect("re-read"),
        before,
        "a second call must be a no-op"
    );
}

/// #1614 AC8, the case that matters for EXISTING installations. The presence
/// test compares only the pattern line, so a `.gitignore` that already
/// carries `wg-*/` gains `room-*/` on the next call and keeps whatever
/// comment it already had, byte for byte. That asymmetry is intended and is
/// residual R2: rewriting the existing comment would mean editing a
/// user-owned file for cosmetics.
#[test]
fn ensure_ac_root_gitignore_appends_room_to_a_legacy_only_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");

    // Exactly what a pre-#1614 installation has on disk.
    const LEGACY: &str = "# AgentsCommander: exclude room cloned repos from parent git tracking.\n# Without this, parent repo operations (checkout, reset) corrupt child clones.\nwg-*/\n";
    std::fs::write(ac_root.join(".gitignore"), LEGACY).expect("seed legacy .gitignore");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
    assert!(
        content.starts_with(LEGACY),
        "the user's existing wg-*/ line AND its original comment must survive byte for byte"
    );
    assert!(
        content.lines().any(|line| line.trim() == "room-*/"),
        "room-*/ must be appended to a Project AC Root that only knew wg-*/"
    );
    assert_eq!(
        content.lines().filter(|l| l.trim() == "wg-*/").count(),
        1,
        "the already-present wg-*/ pattern must not be duplicated"
    );

    // Idempotent: a second call adds nothing.
    let before = content;
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore again");
    assert_eq!(
        std::fs::read_to_string(ac_root.join(".gitignore")).expect("re-read"),
        before,
        "a second call must be a no-op"
    );
}

#[test]
fn ensure_ac_root_gitignore_includes_delete_sentinels_on_create() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
    assert!(
        content.lines().any(|line| line.trim() == ".deleting-*/"),
        "workspace .gitignore must ignore workgroup delete sentinel directories"
    );
    assert!(
        content
            .lines()
            .any(|line| line.trim() == "_loop_*/loop.state.no-git.json"),
        "workspace .gitignore must ignore Loop state files"
    );
    assert!(
        content
            .lines()
            .any(|line| line.trim() == "_loop_*/loop.state.no-git.json.*.tmp"),
        "workspace .gitignore must ignore Loop state write temporaries"
    );
    assert!(
        content
            .lines()
            .any(|line| line.trim() == "_loop_*/*.deprecated-*.no-git"),
        "workspace .gitignore must ignore Loop files the naming migration set aside"
    );
    assert!(
        content
            .lines()
            .any(|line| line.trim() == "_loop_*/audit.jsonl"),
        "workspace .gitignore must ignore Loop audit files"
    );
    assert!(
        !content
            .lines()
            .any(|line| line.trim() == "_loop_*/config.toml"),
        "workspace .gitignore must not ignore Loop config files"
    );
    assert!(
        content
            .lines()
            .any(|line| line.trim() == "_agent_*/**/config.state.no-git.json"),
        "workspace .gitignore must ignore agent state files"
    );
    assert!(
        content
            .lines()
            .any(|line| line.trim() == "**/.config.state.no-git.json.lock"),
        "workspace .gitignore must ignore the agent state write-lock sidecar"
    );
    assert!(
        content
            .lines()
            .any(|line| line.trim() == "_agent_*/**/.config.state.no-git.json.*.tmp"),
        "workspace .gitignore must ignore agent state write temporaries"
    );
    assert!(content.contains(concat!(
        "# AgentsCommander: exclude seed-manifest coordination files.\n",
        "/.seed-manifest.lock\n",
        "/.seed-manifest.*.tmp\n",
        "\n",
        "# AgentsCommander: exclude the seed publication manifest from Git tracking.\n",
        "/seed-manifest.toml\n"
    )));
}

/// #2807 (C2) E6 - each state path is ignored by its own anchored rule,
/// and the decisions file stays tracked.
#[test]
fn ac_root_gitignore_covers_the_config_state_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    let instance = crate::config::agent_local_dir_name();
    std::fs::create_dir_all(ac_root.join("_agent_a").join(instance.as_str()))
        .expect("create .ac tree");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let init_status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .status()
        .expect("git init must execute");
    assert!(init_status.success(), "git init must succeed");
    let empty_excludes = project.join("empty-global-excludes");
    std::fs::write(&empty_excludes, []).expect("create empty global excludes file");
    let excludes_override = format!(
        "core.excludesFile={}",
        empty_excludes.to_string_lossy().replace('\\', "/")
    );

    let state_rule = "_agent_*/**/config.state.no-git.json";
    let lock_rule = "**/.config.state.no-git.json.lock";
    let tmp_rule = "_agent_*/**/.config.state.no-git.json.*.tmp";
    let cases = [
        (
            ".ac/_agent_a/config.state.no-git.json".to_string(),
            Some(state_rule),
        ),
        (
            format!(".ac/_agent_a/{instance}/config.state.no-git.json"),
            Some(state_rule),
        ),
        (
            ".ac/_agent_a/.config.state.no-git.json.lock".to_string(),
            Some(lock_rule),
        ),
        (
            ".ac/_agent_a/.config.state.no-git.json.4242.tmp".to_string(),
            Some(tmp_rule),
        ),
        (
            format!(".ac/_agent_a/{instance}/.config.state.no-git.json.4242.tmp"),
            Some(tmp_rule),
        ),
        (".ac/_agent_a/config.json".to_string(), None),
    ];
    for (target, rule) in &cases {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg(&excludes_override)
            .args(["check-ignore", "-v", "--no-index", "--", target])
            .current_dir(&project)
            .output()
            .expect("git check-ignore must execute");
        let stdout = String::from_utf8(output.stdout).expect("output is UTF-8");
        let Some(rule) = rule else {
            assert_eq!(
                output.status.code(),
                Some(1),
                "{target} must stay tracked: {stdout}"
            );
            assert!(stdout.is_empty(), "{target} must match no rule: {stdout}");
            continue;
        };
        assert!(
            output.status.success(),
            "{target} must be ignored: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let line = stdout.trim_end_matches(&['\r', '\n'][..]);
        let (source_and_pattern, matched_target) = line
            .split_once('\t')
            .expect("verbose git check-ignore output contains a tab");
        let mut source_fields = source_and_pattern.rsplitn(3, ':');
        let pattern = source_fields.next().expect("matched pattern");
        let _line_number = source_fields.next().expect("matched line number");
        let source = source_fields.next().expect("matched source");
        assert_eq!(source, ".ac/.gitignore", "{target}: match source");
        assert_eq!(pattern, *rule, "{target}: matched by its own rule");
        assert_eq!(matched_target, target, "{target}: match target");
    }
}

#[test]
fn ensure_ac_root_gitignore_writes_team_config_lock_block_on_create() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
    let block =
        "# AgentsCommander: exclude team-config coordination files.\n/.team-config-write.lock";
    assert_eq!(
        content.matches(block).count(),
        1,
        "workspace .gitignore must contain the exact team-config lock block once"
    );
    assert_eq!(
        content
            .lines()
            .filter(|line| *line == "/.team-config-write.lock")
            .count(),
        1,
        "workspace .gitignore must contain the anchored team-config lock pattern once"
    );
}

#[test]
fn ensure_ac_root_gitignore_appends_delete_sentinels_to_existing_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    std::fs::write(ac_root.join(".gitignore"), "wg-*/\n").expect("write .gitignore");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
    let count = content
        .lines()
        .filter(|line| line.trim() == ".deleting-*/")
        .count();
    assert_eq!(
        count, 1,
        "workspace .gitignore should append the delete sentinel pattern exactly once"
    );
}

#[test]
fn ensure_ac_root_gitignore_writes_project_settings_block_on_create() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(ac_root.join(".gitignore")).expect("read .gitignore");
    let block =
        "# AgentsCommander: exclude generated project-local settings.\n/settings.50.personal.no-git.json";
    assert_eq!(
        content.matches(block).count(),
        1,
        "workspace .gitignore must contain the exact project-settings block once"
    );
    assert_eq!(
        content
            .lines()
            .filter(|line| *line == "/settings.50.personal.no-git.json")
            .count(),
        1,
        "workspace .gitignore must contain the anchored project-settings pattern once"
    );
    assert!(
        !content.lines().any(|line| matches!(
            line,
            "settings.50.personal.no-git.json" | ".ac/settings.50.personal.no-git.json"
        )),
        "workspace .gitignore must not contain a broader or incorrectly based alternative"
    );
}

#[test]
fn ensure_ac_root_gitignore_appends_project_settings_block_preserving_bytes_and_is_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    let gitignore_path = ac_root.join(".gitignore");
    let original = b"# User-authored rules\r\n!important.txt\r\n\r\n".to_vec();
    std::fs::write(&gitignore_path, &original).expect("write .gitignore");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let updated = std::fs::read(&gitignore_path).expect("read updated .gitignore");
    assert!(
        updated.starts_with(&original),
        "workspace .gitignore must preserve the original bytes as an exact prefix"
    );
    let updated_text = std::str::from_utf8(&updated).expect("updated .gitignore is UTF-8");
    let block =
        "# AgentsCommander: exclude generated project-local settings.\n/settings.50.personal.no-git.json";
    assert_eq!(
        updated_text.matches(block).count(),
        1,
        "workspace .gitignore must append the exact project-settings block once"
    );
    assert_eq!(
        updated_text
            .lines()
            .filter(|line| *line == "/settings.50.personal.no-git.json")
            .count(),
        1,
        "workspace .gitignore must contain the anchored project-settings pattern once"
    );

    let once_updated = updated;
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore again");
    let twice_updated = std::fs::read(&gitignore_path).expect("read .gitignore again");
    assert_eq!(
        twice_updated, once_updated,
        "a repeated ensure must leave .gitignore byte-identical"
    );
}

#[test]
fn ensure_ac_root_gitignore_project_settings_rule_is_root_anchored() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(ac_root.join("nested")).expect("create .ac tree");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let init_status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&project)
        .status()
        .expect("git init must execute");
    assert!(init_status.success(), "git init must succeed");

    let empty_excludes = project.join("empty-global-excludes");
    std::fs::write(&empty_excludes, []).expect("create empty global excludes file");
    std::fs::write(ac_root.join("settings.50.personal.no-git.json"), [])
        .expect("create root project settings");
    std::fs::write(
        ac_root
            .join("nested")
            .join("settings.50.personal.no-git.json"),
        [],
    )
    .expect("create nested project settings");

    let excludes_override = format!(
        "core.excludesFile={}",
        empty_excludes.to_string_lossy().replace('\\', "/")
    );
    let root_output = std::process::Command::new("git")
        .arg("-c")
        .arg(&excludes_override)
        .args([
            "check-ignore",
            "-v",
            "--no-index",
            "--",
            ".ac/settings.50.personal.no-git.json",
        ])
        .current_dir(&project)
        .output()
        .expect("root git check-ignore must execute");
    assert!(
        root_output.status.success(),
        "root git check-ignore must match: {}",
        String::from_utf8_lossy(&root_output.stderr)
    );

    let root_stdout = String::from_utf8(root_output.stdout).expect("root output is UTF-8");
    let root_line = root_stdout.trim_end_matches(&['\r', '\n'][..]);
    let (source_and_pattern, target) = root_line
        .split_once('\t')
        .expect("verbose git check-ignore output contains a tab");
    let mut source_fields = source_and_pattern.rsplitn(3, ':');
    let pattern = source_fields.next().expect("matched pattern");
    let line_number = source_fields.next().expect("matched line number");
    let source = source_fields.next().expect("matched source");
    assert_eq!(
        source, ".ac/.gitignore",
        "match source must be .ac/.gitignore"
    );
    assert!(
        line_number.parse::<usize>().is_ok(),
        "match line number must be numeric"
    );
    assert_eq!(
        pattern, "/settings.50.personal.no-git.json",
        "match pattern must be the anchored project-settings rule"
    );
    assert_eq!(
        target, ".ac/settings.50.personal.no-git.json",
        "match target must be the root project settings"
    );

    let nested_output = std::process::Command::new("git")
        .arg("-c")
        .arg(&excludes_override)
        .args([
            "check-ignore",
            "-v",
            "--no-index",
            "--",
            ".ac/nested/settings.50.personal.no-git.json",
        ])
        .current_dir(&project)
        .output()
        .expect("nested git check-ignore must execute");
    assert_eq!(
        nested_output.status.code(),
        Some(1),
        "nested git check-ignore must report no match: {}",
        String::from_utf8_lossy(&nested_output.stderr)
    );
    assert!(
        nested_output.stdout.is_empty(),
        "nested git check-ignore must emit no stdout"
    );
}

#[test]
fn ensure_ac_root_gitignore_project_settings_rule_requires_an_exact_line() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    let gitignore_path = ac_root.join(".gitignore");
    let original = b" /settings.50.personal.no-git.json\r\n wg-*/\r\n".to_vec();
    std::fs::write(&gitignore_path, &original).expect("write .gitignore");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let updated = std::fs::read(&gitignore_path).expect("read updated .gitignore");
    assert!(
        updated.starts_with(&original),
        "reconciliation must preserve whitespace-prefixed user rules byte-for-byte"
    );
    let updated_text = std::str::from_utf8(&updated).expect("updated .gitignore is UTF-8");
    assert_eq!(
        updated_text
            .lines()
            .filter(|line| *line == "/settings.50.personal.no-git.json")
            .count(),
        1,
        "a whitespace-prefixed decoy must not suppress the canonical rule"
    );
    assert_eq!(
        updated_text
            .lines()
            .filter(|line| line.trim() == "wg-*/")
            .count(),
        1,
        "legacy whitespace-tolerant presence detection must remain unchanged"
    );
    assert_eq!(
        updated_text.lines().filter(|line| *line == "wg-*/").count(),
        0,
        "legacy whitespace-tolerant presence detection must not append a canonical duplicate"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    let gitignore_path = ac_root.join(".gitignore");
    std::fs::write(&gitignore_path, b"/settings.50.personal.no-git.json\r\n")
        .expect("write exact project-settings rule");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    let content = std::fs::read_to_string(&gitignore_path).expect("read reconciled .gitignore");
    assert_eq!(
        content
            .lines()
            .filter(|line| *line == "/settings.50.personal.no-git.json")
            .count(),
        1,
        "an exact CRLF-terminated rule must be considered present"
    );
    assert_eq!(
        content
            .matches("# AgentsCommander: exclude generated project-local settings.")
            .count(),
        0,
        "an existing exact rule must not trigger comment repair"
    );
}

#[test]
fn seed_manifest_gitignore_rules_are_anchored_to_ac_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(ac_root.join("nested")).expect("create .ac tree");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("ensure workspace .gitignore");

    for relative in [
        ".ac/.seed-manifest.lock",
        ".ac/.seed-manifest.00000000-0000-0000-0000-000000000000.tmp",
        ".ac/seed-manifest.toml",
        ".ac/nested/.seed-manifest.lock",
        ".ac/nested/.seed-manifest.00000000-0000-0000-0000-000000000000.tmp",
        ".ac/nested/seed-manifest.toml",
    ] {
        std::fs::write(project.join(relative), b"fixture").expect("write Git fixture");
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
    assert!(ignored(".ac/.seed-manifest.lock"));
    assert!(ignored(
        ".ac/.seed-manifest.00000000-0000-0000-0000-000000000000.tmp"
    ));
    assert!(ignored(".ac/seed-manifest.toml"));
    assert!(!ignored(".ac/nested/.seed-manifest.lock"));
    assert!(!ignored(
        ".ac/nested/.seed-manifest.00000000-0000-0000-0000-000000000000.tmp"
    ));
    assert!(!ignored(".ac/nested/seed-manifest.toml"));
}

/// #2090 - an existing `.ac/.gitignore` carrying the retired
/// `!/seed-manifest.toml` pair must lose the pair, keep every other byte and
/// gain the ignore rule exactly once. A bare user negation is not removed
/// (covered separately).
#[test]
fn ensure_ac_root_gitignore_migrates_the_legacy_seed_manifest_block() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    let gitignore_path = ac_root.join(".gitignore");
    const USER_RULE: &str = "# user rule\n!important.txt\n";
    let legacy = format!(
        "{USER_RULE}{}",
        concat!(
            "# AgentsCommander: exclude seed-manifest coordination files.\n",
            "/.seed-manifest.lock\n",
            "/.seed-manifest.*.tmp\n",
            "\n",
            "# AgentsCommander: keep the seed publication manifest reviewable.\n",
            "!/seed-manifest.toml\n"
        )
    );
    std::fs::write(&gitignore_path, &legacy).expect("seed legacy .gitignore");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("migrate legacy .gitignore");

    let content = std::fs::read_to_string(&gitignore_path).expect("read .gitignore");
    assert!(
        content.starts_with(USER_RULE),
        "user content must survive byte for byte, got {content:?}"
    );
    assert!(
        !content.contains("!/seed-manifest.toml"),
        "the retired negation must be removed"
    );
    assert!(
        !content.contains("# AgentsCommander: keep the seed publication manifest reviewable."),
        "the retired comment must be removed"
    );
    for pattern in [
        "/.seed-manifest.lock",
        "/.seed-manifest.*.tmp",
        "/seed-manifest.toml",
    ] {
        assert_eq!(
            content
                .lines()
                .filter(|line| line.trim() == pattern)
                .count(),
            1,
            "the migrated .gitignore must carry {pattern} exactly once"
        );
    }

    // Idempotent: a second ensure appends nothing.
    let before = content;
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names()).expect("second ensure");
    assert_eq!(
        std::fs::read_to_string(&gitignore_path).expect("re-read"),
        before,
        "a second call must be a no-op"
    );
}

/// #2090 - the exact bytes an older build wrote must migrate to exactly the
/// bytes a fresh root gets, with no duplicated coordination rules and no
/// leftover blank line.
#[test]
fn ensure_ac_root_gitignore_migrated_root_matches_a_fresh_root() {
    const NEW_MANIFEST_BLOCK: &str = concat!(
        "# AgentsCommander: exclude the seed publication manifest from Git tracking.\n",
        "/seed-manifest.toml\n"
    );
    const LEGACY_MANIFEST_BLOCK: &str = concat!(
        "# AgentsCommander: keep the seed publication manifest reviewable.\n",
        "!/seed-manifest.toml\n"
    );

    let fresh = tempfile::tempdir().expect("tempdir");
    let fresh_ac = fresh.path().join(".ac");
    std::fs::create_dir(&fresh_ac).expect("create .ac");
    ensure_ac_root_gitignore_with_names(&fresh_ac, &floor_names())
        .expect("ensure fresh .gitignore");
    let expected =
        std::fs::read_to_string(fresh_ac.join(".gitignore")).expect("read fresh .gitignore");

    let old = tempfile::tempdir().expect("tempdir");
    let old_ac = old.path().join(".ac");
    std::fs::create_dir(&old_ac).expect("create .ac");
    let legacy_content = expected.replace(NEW_MANIFEST_BLOCK, LEGACY_MANIFEST_BLOCK);
    assert!(
        legacy_content.contains("!/seed-manifest.toml"),
        "the fixture must carry the retired negation"
    );
    std::fs::write(old_ac.join(".gitignore"), &legacy_content).expect("seed legacy .gitignore");

    ensure_ac_root_gitignore_with_names(&old_ac, &floor_names())
        .expect("migrate legacy .gitignore");

    assert_eq!(
        std::fs::read_to_string(old_ac.join(".gitignore")).expect("read migrated .gitignore"),
        expected,
        "migrating a prior-build .gitignore must reproduce a fresh root byte for byte"
    );
}

/// #2090 - a `!/seed-manifest.toml` line without the retired AC comment is
/// user intent; reconciliation must not remove it.
#[test]
fn ensure_ac_root_gitignore_preserves_a_bare_user_seed_manifest_negation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    let gitignore_path = ac_root.join(".gitignore");
    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names()).expect("ensure .gitignore");
    let mut content = std::fs::read_to_string(&gitignore_path).expect("read .gitignore");
    content.push_str("\n# user rule\n!/seed-manifest.toml\n");
    std::fs::write(&gitignore_path, &content).expect("append user negation");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names()).expect("ensure again");

    assert_eq!(
        std::fs::read_to_string(&gitignore_path).expect("re-read"),
        content,
        "a bare user negation must survive reconciliation byte for byte"
    );
}

/// #2090 - the retired pair is matched on trimmed lines, so a `.gitignore`
/// converted to CRLF by Git or an editor is migrated too.
#[test]
fn ensure_ac_root_gitignore_migrates_a_crlf_legacy_seed_manifest_block() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");
    let gitignore_path = ac_root.join(".gitignore");
    const USER_RULE: &str = "# user rule\r\n";
    let legacy = concat!(
        "# user rule\r\n",
        "# AgentsCommander: keep the seed publication manifest reviewable.\r\n",
        "!/seed-manifest.toml\r\n"
    );
    std::fs::write(&gitignore_path, legacy).expect("seed CRLF legacy .gitignore");

    ensure_ac_root_gitignore_with_names(&ac_root, &floor_names())
        .expect("migrate legacy .gitignore");

    let content = std::fs::read_to_string(&gitignore_path).expect("read .gitignore");
    assert!(
        content.starts_with(USER_RULE),
        "the user rule must survive byte for byte, got {content:?}"
    );
    assert!(
        !content.contains("!/seed-manifest.toml"),
        "the CRLF negation must be removed"
    );
    assert_eq!(
        content
            .lines()
            .filter(|line| line.trim() == "/seed-manifest.toml")
            .count(),
        1,
        "the CRLF file must gain the manifest ignore rule exactly once"
    );
}
