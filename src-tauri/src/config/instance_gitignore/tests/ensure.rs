use super::{
    assert_git_ignore_status, assert_git_success, ensure_instance_gitignore_at,
    expected_fresh_bytes, ignore_rows, legacy_complete_bytes, missing_rule_indexes, push_pair,
    render, required_rules, track_rows, ArtifactKind, Duration, HashSet, Instant, OpenOptions,
    HISTORICAL_FIXED_RULES, TEST_AGENT_LOCAL_DIR,
};

#[test]
fn fresh_file_matches_the_registry_and_dynamic_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("ensure fresh file");

    let actual = std::fs::read(temp.path().join(".gitignore")).expect("read .gitignore");
    assert_eq!(actual, expected_fresh_bytes(TEST_AGENT_LOCAL_DIR));
    assert!(actual.ends_with(b"\n"));

    let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
    assert_eq!(rules.len(), 2 + ignore_rows().len());
    let lines: Vec<&[u8]> = actual[..actual.len() - 1]
        .split(|byte| *byte == b'\n')
        .collect();
    assert_eq!(lines.len(), rules.len() * 2);
    assert_eq!(
        rules
            .iter()
            .map(|rule| rule.pattern.as_str())
            .collect::<HashSet<_>>()
            .len(),
        rules.len()
    );

    assert_eq!(
        rules
            .iter()
            .filter(|rule| rule.pattern.contains(TEST_AGENT_LOCAL_DIR))
            .count(),
        1,
        "the running agent's local directory name belongs to the first rule only"
    );
    assert!(!actual
        .windows(b"<agent-local-dir>".len())
        .any(|window| window == b"<agent-local-dir>"));

    for artifact in ignore_rows() {
        let pattern = render(artifact).pattern;
        if matches!(artifact.kind, ArtifactKind::GlobAnyDepth) {
            assert!(
                !pattern.starts_with('/'),
                "{pattern} is depth-independent by design and must not be anchored"
            );
        } else {
            assert!(
                pattern.starts_with('/'),
                "{pattern} must be anchored to the instance root"
            );
        }
    }
    assert!(lines.iter().all(|line| !line.starts_with(b"!")));
    assert!(lines.iter().all(|line| *line != b"/*"));
}

#[test]
fn legacy_fourteen_rule_file_gains_exactly_the_new_entries() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(".gitignore");
    let legacy = legacy_complete_bytes(TEST_AGENT_LOCAL_DIR);
    std::fs::write(&path, &legacy).expect("seed legacy complete file");

    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("repair legacy file");
    let repaired = std::fs::read(&path).expect("read repaired legacy file");
    assert!(
        repaired.starts_with(&legacy),
        "reconciliation must never rewrite what the file already had"
    );

    let mut expected_appended = Vec::new();
    for artifact in ignore_rows() {
        let rendered = render(artifact);
        if HISTORICAL_FIXED_RULES.contains(&rendered.pattern.as_str()) {
            continue;
        }
        push_pair(&mut expected_appended, rendered.comment, &rendered.pattern);
    }
    assert!(
        !expected_appended.is_empty(),
        "this change adds rules, so the appended block cannot be empty"
    );
    assert_eq!(&repaired[legacy.len()..], expected_appended.as_slice());

    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("second ensure");
    assert_eq!(
        std::fs::read(&path).expect("read after second ensure"),
        repaired
    );
}

#[test]
fn read_only_legacy_complete_file_fails_without_modification() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(".gitignore");
    let legacy = legacy_complete_bytes(TEST_AGENT_LOCAL_DIR);
    std::fs::write(&path, &legacy).expect("seed legacy complete file");

    let original_permissions = std::fs::metadata(&path).expect("metadata").permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&path, read_only).expect("make legacy file read-only");
    let can_bypass_read_only = OpenOptions::new().append(true).open(&path).is_ok();
    let result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
    let after = std::fs::read(&path).expect("read read-only legacy file");
    std::fs::set_permissions(&path, original_permissions).expect("restore permissions");

    if can_bypass_read_only {
        eprintln!(
            "skipping permission-denial assertion because this process can write a read-only file"
        );
        return;
    }
    assert!(
        result.is_err(),
        "this change makes every pre-existing complete file partial, so a read-only \
         one takes the failing branch instead of the silent-OK one"
    );
    assert_eq!(after, legacy);
}

#[test]
fn root_agent_track_row_matches_the_root_agent_dir_constant() {
    let row = track_rows()
        .into_iter()
        .find(|artifact| artifact.name == super::super::super::ROOT_AGENT_DIR_NAME)
        .expect("root-agent Track row");
    assert!(matches!(row.kind, ArtifactKind::Dir));
}

#[test]
fn track_rows_are_exactly_the_declared_track_set() {
    let mut names: Vec<&str> = track_rows()
        .into_iter()
        .map(|artifact| artifact.name)
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "Context.AgentsCommander.md",
            "Context.AgentsCommander.md.retired-*.bak",
            "Context.root-agent.md",
            "ac-root-agent",
            "agency-agents_templates",
            "agent-templates",
            "coding-agents",
        ],
        "the tracked set is a product decision; a row leaving or joining it changes \
         what the generated file ignores and has to be argued, not edited"
    );
}

#[test]
fn injected_name_with_line_break_is_rejected_without_creating_a_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = ensure_instance_gitignore_at(temp.path(), ".valid\n/app.log");
    assert!(result.is_err());
    assert!(!temp.path().join(".gitignore").exists());
}

#[test]
fn partial_file_preserves_prefix_and_appends_only_missing_rules() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(".gitignore");
    let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
    let seed = format!(
        "# user comment\r\n/custom/**\r\n{}\r\n{}\r\n{}\r\n# /daemon.pid\r\n!/sessions.json\r\n{}\r\n!/update-check.json\r\n/web-token.txt # note\r\nlast-user-rule",
        rules[0].pattern, rules[3].pattern, rules[3].pattern, rules[8].pattern
    )
    .into_bytes();
    std::fs::write(&path, &seed).expect("seed partial file");

    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("repair partial file");
    let actual = std::fs::read(&path).expect("read repaired file");
    assert!(actual.starts_with(&seed));

    let mut expected = seed.clone();
    expected.push(b'\n');
    for (index, rule) in rules.iter().enumerate() {
        if !matches!(index, 0 | 3 | 8) {
            push_pair(&mut expected, rule.comment, &rule.pattern);
        }
    }
    assert_eq!(actual, expected);
}

#[test]
fn complete_file_is_byte_stable_across_repeated_ensure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(".gitignore");
    std::fs::write(&path, b"# retained user rule\n/custom\n").expect("seed file");

    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("first ensure");
    let after_first = std::fs::read(&path).expect("read first result");
    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("second ensure");
    let after_second = std::fs::read(&path).expect("read second result");

    assert_eq!(after_second, after_first);
}

#[test]
fn byte_scan_preserves_invalid_utf8_and_recognizes_crlf_rules() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(".gitignore");
    let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
    let mut seed = b"\xff\xfe user bytes\r\n".to_vec();
    for rule in &rules {
        seed.extend_from_slice(rule.pattern.as_bytes());
        seed.extend_from_slice(b"\r\n");
    }
    std::fs::write(&path, &seed).expect("seed invalid UTF-8 file");

    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("ensure CRLF file");
    assert_eq!(std::fs::read(path).expect("read file"), seed);
}

#[test]
fn directory_target_is_rejected_and_untouched() {
    let temp = tempfile::tempdir().expect("tempdir");
    let target = temp.path().join(".gitignore");
    std::fs::create_dir(&target).expect("create directory target");
    let sentinel = target.join("sentinel.txt");
    std::fs::write(&sentinel, b"untouched").expect("write sentinel");

    let result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
    assert!(result.is_err());
    assert!(target.is_dir());
    assert_eq!(
        std::fs::read(sentinel).expect("read sentinel"),
        b"untouched"
    );
}

#[test]
fn symlink_target_is_rejected_without_touching_referent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_dir = temp.path().join("config");
    std::fs::create_dir(&config_dir).expect("create config dir");
    let referent = temp.path().join("external-sentinel.txt");
    std::fs::write(&referent, b"external sentinel").expect("write referent");
    let link = config_dir.join(".gitignore");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&referent, &link).expect("create symlink");
    #[cfg(windows)]
    if std::os::windows::fs::symlink_file(&referent, &link).is_err() {
        return;
    }
    #[cfg(not(any(unix, windows)))]
    return;

    let result = ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR);
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(&referent).expect("read referent"),
        b"external sentinel"
    );
}

#[test]
fn read_only_complete_file_needs_no_write_but_partial_file_fails_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(".gitignore");
    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR).expect("create complete file");
    let complete = std::fs::read(&path).expect("read complete file");

    let original_permissions = std::fs::metadata(&path)
        .expect("complete metadata")
        .permissions();
    let mut read_only_permissions = original_permissions.clone();
    read_only_permissions.set_readonly(true);
    std::fs::set_permissions(&path, read_only_permissions).expect("make complete file read-only");
    let complete_result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
    let complete_after = std::fs::read(&path).expect("read complete read-only file");
    std::fs::set_permissions(&path, original_permissions).expect("restore complete permissions");
    assert!(complete_result.is_ok());
    assert_eq!(complete_after, complete);

    let partial = b"# partial user file\n".to_vec();
    std::fs::write(&path, &partial).expect("seed partial file");
    let partial_permissions = std::fs::metadata(&path)
        .expect("partial metadata")
        .permissions();
    let mut partial_read_only = partial_permissions.clone();
    partial_read_only.set_readonly(true);
    std::fs::set_permissions(&path, partial_read_only).expect("make partial file read-only");
    let can_bypass_read_only = OpenOptions::new().append(true).open(&path).is_ok();
    let partial_result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
    let partial_after = std::fs::read(&path).expect("read partial read-only file");
    std::fs::set_permissions(&path, partial_permissions).expect("restore partial permissions");

    if can_bypass_read_only {
        eprintln!(
            "skipping permission-denial assertion because this process can write a read-only file"
        );
        return;
    }
    assert!(partial_result.is_err());
    assert_eq!(partial_after, partial);
}

#[test]
fn locked_partial_file_fails_fast_and_remains_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(".gitignore");
    let partial = b"# partial\n".to_vec();
    std::fs::write(&path, &partial).expect("seed partial file");
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open lock holder");
    held.try_lock().expect("hold target lock");

    let started = Instant::now();
    let result = ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR);
    let elapsed = started.elapsed();
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.contains("lock contention")),
        "unexpected lock result: {result:?}"
    );
    assert!(elapsed < Duration::from_secs(5));

    drop(held);
    assert_eq!(
        std::fs::read(&path).expect("read file after releasing lock"),
        partial
    );
    ensure_instance_gitignore_at(temp.path(), TEST_AGENT_LOCAL_DIR)
        .expect("repair after releasing lock");
    let repaired = std::fs::read(path).expect("read repaired file");
    let rules = required_rules(TEST_AGENT_LOCAL_DIR).expect("rules");
    assert!(missing_rule_indexes(&repaired, &rules).is_empty());
}

#[test]
fn managed_catalog_journal_temp_name_exact() {
    // #1968: the journal publication temporary for the dot-prefixed journal
    // name has TWO leading dots. The rule is derived through the real
    // publication helper, and the SINGLE-dot near miss must not match THAT
    // rule. The near miss is still covered by the pre-existing any-depth
    // atomic-write glob, so this test isolates the derived rule in its own
    // one-rule .gitignore instead of asking the whole policy.
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    assert_git_success(repo, &["init", "--quiet"]);

    let temp_name = super::super::super::instance_artifacts::publication_temp_name_for_destination(
        ".agents.migration-v1.json",
        123,
        7,
    );
    assert_eq!(temp_name, "..agents.migration-v1.json.123.7.tmp");

    let row = ignore_rows()
        .into_iter()
        .find(|artifact| {
            artifact.name
                == super::super::super::instance_artifacts::CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT
        })
        .expect("journal temporary registry row");
    let pattern = render(row).pattern;
    std::fs::write(repo.join(".gitignore"), format!("{pattern}\n"))
        .expect("seed one-rule gitignore");

    let exact = format!("coding-agents/{temp_name}");
    let near_miss = "coding-agents/.agents.migration-v1.json.123.7.tmp";
    for relative in [exact.as_str(), near_miss] {
        let path = repo.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture parent"))
            .expect("create fixture parent");
        std::fs::write(path, b"fixture").expect("write fixture");
    }
    assert_git_ignore_status(repo, &exact, 0);
    assert_git_ignore_status(repo, near_miss, 1);
}
