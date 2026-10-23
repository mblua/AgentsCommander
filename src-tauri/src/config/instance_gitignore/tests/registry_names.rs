use super::{
    assert_git_ignore_status, assert_git_success, ensure_instance_gitignore_at, required_rules,
    AGENTS_INSTANCE_FILE_NAME, AGENTS_INSTANCE_TARGET_NAME, BLOCKING_MENUS_LOCAL_FILE_NAME,
    BLOCKING_MENUS_LOCAL_TARGET_NAME, BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME,
    BLOCKING_MENUS_REMOTE_CHECK_TARGET_NAME, BLOCKING_MENUS_REMOTE_FILE_NAME,
    BLOCKING_MENUS_REMOTE_TARGET_NAME, BLOCKING_MENUS_SHIPPED_FILE_NAME,
    BLOCKING_MENUS_SHIPPED_TARGET_NAME, CODING_AGENTS_BASE_FILENAME,
    CODING_AGENTS_BASE_TARGET_NAME, CODING_AGENTS_LOCAL_FILENAME, CODING_AGENTS_LOCAL_TARGET_NAME,
    CODING_AGENTS_LOCK_FILENAME, CODING_AGENTS_LOCK_TARGET_NAME,
    CODING_AGENTS_MIGRATION_BACKUP_FILENAME, CODING_AGENTS_MIGRATION_JOURNAL_FILENAME,
    CODING_AGENTS_PROJECT_TARGET_NAME, LAYERED_NAME_PROBES, LAYER_DEFAULT, LAYER_INSTANCE,
    LAYER_PERSONAL, LAYER_PROJECT, LAYER_REMOTE, NO_GIT_MARKER, RETIRED_PAIRS,
    SETTINGS_BACKUP_PREFIX, SETTINGS_BACKUP_ROTATION_GLOB, SETTINGS_BACKUP_ROTATION_TARGET_GLOB,
    SETTINGS_BACKUP_SUFFIX, SETTINGS_BACKUP_TARGET_PREFIX, SETTINGS_FILE_NAME,
    SETTINGS_LOCAL_OVERRIDE_FILE_NAME, SETTINGS_LOCAL_TARGET_NAME, SETTINGS_LOCK_FILE_NAME,
    SETTINGS_LOCK_TARGET_NAME, SETTINGS_MIGRATION_BACKUP_GLOB,
    SETTINGS_MIGRATION_BACKUP_TARGET_GLOB, SETTINGS_TARGET_NAME, STATE_MARKER,
    TEST_AGENT_LOCAL_DIR,
};

#[test]
fn instance_gitignore_covers_every_injected_messages_artifact() {
    use super::super::super::injected_messages::{
        INJECTED_MESSAGES_FILENAME, INJECTED_MESSAGES_REFERENCE_FILENAME,
        INJECTED_MESSAGES_STATE_FILENAME,
    };

    let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
    for expected in [
        format!("/{INJECTED_MESSAGES_FILENAME}"),
        format!("/{INJECTED_MESSAGES_REFERENCE_FILENAME}"),
        format!("/{INJECTED_MESSAGES_STATE_FILENAME}"),
        format!("/{INJECTED_MESSAGES_FILENAME}.bak-*"),
    ] {
        assert!(
            rules.iter().any(|rule| rule.pattern == expected),
            "injected-messages artifact is not covered by the instance policy: {expected}"
        );
    }
}

#[test]
fn instance_gitignore_ignores_injected_messages_artifacts_narrowly() {
    use super::super::super::injected_messages::{
        INJECTED_MESSAGES_FILENAME, INJECTED_MESSAGES_REFERENCE_FILENAME,
        INJECTED_MESSAGES_STATE_FILENAME,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    assert_git_success(repo, &["init", "--quiet"]);
    let config_dir = repo.join("instance");
    std::fs::create_dir(&config_dir).expect("create config dir");

    ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR)
        .expect("ensure injected-messages fixture");

    let stamped_backup = format!("{INJECTED_MESSAGES_FILENAME}.bak-20260801T221533Z");
    let unstamped_backup = format!("{INJECTED_MESSAGES_FILENAME}.bak");
    let nested = format!("sub/{INJECTED_MESSAGES_FILENAME}");
    let cases = [
        (INJECTED_MESSAGES_STATE_FILENAME, 0),
        (INJECTED_MESSAGES_REFERENCE_FILENAME, 0),
        (INJECTED_MESSAGES_FILENAME, 0),
        (stamped_backup.as_str(), 0),
        (unstamped_backup.as_str(), 1),
        ("injected-messages.json", 1),
        ("agentscommander-injected-messages.json", 1),
        (nested.as_str(), 1),
    ];

    for (relative, expected_code) in cases {
        let path = config_dir.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture parent"))
            .expect("create fixture parent");
        std::fs::write(path, b"fixture").expect("write fixture");
        assert_git_ignore_status(repo, &format!("instance/{relative}"), expected_code);
    }
}

/// #2470 E5 - every layered target constant equals its own row of the
/// target tables in `docs/reference/file-naming.md`, row by row. The row
/// count is pinned, so a deleted row cannot pass by shrinking both sides.
#[test]
fn target_names_match_the_documented_tables() {
    const DOC: &str = include_str!("../../../../../docs/reference/file-naming.md");
    let mut documented = Vec::new();
    for heading in [
        "### `settings.*`",
        "### `blocking-menus.*`",
        "### `agents.*`",
    ] {
        let start = DOC
            .find(heading)
            .unwrap_or_else(|| panic!("the document lost the heading {heading}"));
        let rows = DOC[start + heading.len()..]
            .lines()
            .skip_while(|line| !line.starts_with('|'))
            .take_while(|line| line.starts_with('|'))
            .skip(2); // header and separator
        for row in rows {
            let target = row
                .split('|')
                .nth(2)
                .unwrap_or_else(|| panic!("row without a target cell: {row}"));
            let value = target
                .split('`')
                .nth(1)
                .unwrap_or_else(|| panic!("target cell without a backticked span: {row}"));
            documented.push(value);
        }
    }
    // The backup row is a whole glob; the constant is only its prefix.
    let backup = format!("{SETTINGS_BACKUP_TARGET_PREFIX}*{SETTINGS_BACKUP_SUFFIX}");
    let expected: [(&str, &str); 14] = [
        ("SETTINGS_TARGET_NAME", SETTINGS_TARGET_NAME),
        ("SETTINGS_LOCAL_TARGET_NAME", SETTINGS_LOCAL_TARGET_NAME),
        ("SETTINGS_LOCK_TARGET_NAME", SETTINGS_LOCK_TARGET_NAME),
        ("SETTINGS_BACKUP_TARGET_PREFIX", &backup),
        (
            "SETTINGS_MIGRATION_BACKUP_TARGET_GLOB",
            SETTINGS_MIGRATION_BACKUP_TARGET_GLOB,
        ),
        (
            "BLOCKING_MENUS_SHIPPED_TARGET_NAME",
            BLOCKING_MENUS_SHIPPED_TARGET_NAME,
        ),
        (
            "BLOCKING_MENUS_REMOTE_TARGET_NAME",
            BLOCKING_MENUS_REMOTE_TARGET_NAME,
        ),
        (
            "BLOCKING_MENUS_LOCAL_TARGET_NAME",
            BLOCKING_MENUS_LOCAL_TARGET_NAME,
        ),
        (
            "BLOCKING_MENUS_REMOTE_CHECK_TARGET_NAME",
            BLOCKING_MENUS_REMOTE_CHECK_TARGET_NAME,
        ),
        (
            "CODING_AGENTS_BASE_TARGET_NAME",
            CODING_AGENTS_BASE_TARGET_NAME,
        ),
        (
            "CODING_AGENTS_LOCAL_TARGET_NAME",
            CODING_AGENTS_LOCAL_TARGET_NAME,
        ),
        (
            "CODING_AGENTS_PROJECT_TARGET_NAME",
            CODING_AGENTS_PROJECT_TARGET_NAME,
        ),
        ("AGENTS_INSTANCE_TARGET_NAME", AGENTS_INSTANCE_TARGET_NAME),
        (
            "CODING_AGENTS_LOCK_TARGET_NAME",
            CODING_AGENTS_LOCK_TARGET_NAME,
        ),
    ];
    assert_eq!(
        documented.len(),
        expected.len(),
        "the three target tables must hold exactly 14 rows: {documented:?}"
    );
    for ((name, value), row) in expected.iter().zip(&documented) {
        assert_eq!(value, row, "{name} disagrees with its documented row");
    }
}

/// #2470 E6 - each `layered_name!` arm types its layer token once; this ties
/// every arm to the constant that names the same token.
#[test]
fn layer_tokens_match_the_macro() {
    let expected = [
        format!("x.{LAYER_DEFAULT}.e"),
        format!("x.{LAYER_REMOTE}.e"),
        format!("x.{LAYER_INSTANCE}.e"),
        format!("x.{LAYER_PROJECT}.e"),
        format!("x.{LAYER_PERSONAL}.e"),
        format!("x.{STATE_MARKER}.e"),
        format!("x.{LAYER_INSTANCE}.{NO_GIT_MARKER}.e"),
    ];
    assert_eq!(LAYERED_NAME_PROBES.len(), expected.len());
    for ((composed, token), expected) in LAYERED_NAME_PROBES.iter().zip(&expected) {
        assert_eq!(composed, expected, "the macro arm for {token} drifted");
    }
}

/// #2714 E4 - B1b switches every settings and blocking-menus live name to
/// its Phase A target constant, never to a retyped literal. #2715 (B2)
/// switches the three coding-agent names; the two #1968 sidecars stay.
#[test]
fn live_names_are_the_target_names() {
    let switched = [
        (SETTINGS_FILE_NAME, SETTINGS_TARGET_NAME),
        (
            SETTINGS_LOCAL_OVERRIDE_FILE_NAME,
            SETTINGS_LOCAL_TARGET_NAME,
        ),
        (SETTINGS_LOCK_FILE_NAME, SETTINGS_LOCK_TARGET_NAME),
        (SETTINGS_BACKUP_PREFIX, SETTINGS_BACKUP_TARGET_PREFIX),
        (
            SETTINGS_BACKUP_ROTATION_GLOB,
            SETTINGS_BACKUP_ROTATION_TARGET_GLOB,
        ),
        (
            SETTINGS_MIGRATION_BACKUP_GLOB,
            SETTINGS_MIGRATION_BACKUP_TARGET_GLOB,
        ),
        (
            BLOCKING_MENUS_SHIPPED_FILE_NAME,
            BLOCKING_MENUS_SHIPPED_TARGET_NAME,
        ),
        (
            BLOCKING_MENUS_REMOTE_FILE_NAME,
            BLOCKING_MENUS_REMOTE_TARGET_NAME,
        ),
        (
            BLOCKING_MENUS_LOCAL_FILE_NAME,
            BLOCKING_MENUS_LOCAL_TARGET_NAME,
        ),
        (
            BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME,
            BLOCKING_MENUS_REMOTE_CHECK_TARGET_NAME,
        ),
        (CODING_AGENTS_BASE_FILENAME, CODING_AGENTS_BASE_TARGET_NAME),
        (
            CODING_AGENTS_LOCAL_FILENAME,
            CODING_AGENTS_LOCAL_TARGET_NAME,
        ),
        (CODING_AGENTS_LOCK_FILENAME, CODING_AGENTS_LOCK_TARGET_NAME),
        (AGENTS_INSTANCE_FILE_NAME, AGENTS_INSTANCE_TARGET_NAME),
    ];
    for (live, target) in switched {
        assert_eq!(live, target);
    }
    assert_eq!(SETTINGS_BACKUP_SUFFIX, ".json");
    let unchanged = [
        (
            CODING_AGENTS_MIGRATION_BACKUP_FILENAME,
            "agents.migration-v1.backup.json",
        ),
        (
            CODING_AGENTS_MIGRATION_JOURNAL_FILENAME,
            ".agents.migration-v1.json",
        ),
    ];
    for (constant, literal) in unchanged {
        assert_eq!(constant, literal);
    }
}

/// #2714 E10 - the nine retired pairs go, the retired lock keeps a row of
/// its own, this phase's three new rows arrive, and a user-authored bare
/// pattern with no AC comment above it survives byte for byte.
#[test]
fn the_nine_retired_pairs_go_and_the_lock_row_stays() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut seeded = String::from("# my own rules\n/settings.json\n");
    for (comment, pattern) in RETIRED_PAIRS {
        seeded.push_str(&format!("{comment}\n{pattern}\n"));
    }
    let path = temp.path().join(".gitignore");
    std::fs::write(&path, &seeded).expect("seed .gitignore");

    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("ensure");
    let body = std::fs::read_to_string(&path).expect("read .gitignore");
    let lines: Vec<&str> = body.lines().collect();

    assert!(
        body.starts_with("# my own rules\n/settings.json\n"),
        "{body}"
    );
    for (comment, pattern) in RETIRED_PAIRS {
        assert!(
            !lines.windows(2).any(|pair| pair == [comment, pattern]),
            "retired pair survived: {pattern}"
        );
    }
    for pattern in [
        "/settings.json.lock",
        "/settings.30.instance.no-git.json.*.tmp",
        "/*.deprecated-*.no-git",
    ] {
        assert_eq!(
            lines.iter().filter(|line| **line == pattern).count(),
            1,
            "{pattern} in {body}"
        );
    }

    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("ensure again");
    assert_eq!(std::fs::read_to_string(&path).expect("reread"), body);
}

/// #2470 - the lock is named after the live settings file, and until now
/// nothing said so.
#[test]
fn settings_lock_name_derives_from_the_settings_name() {
    assert_eq!(
        SETTINGS_LOCK_FILE_NAME,
        format!("{SETTINGS_FILE_NAME}.lock")
    );
}
