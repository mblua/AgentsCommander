use super::{
    assert_git_ignore_status, assert_git_success, contains_exact_line,
    ensure_instance_gitignore_at, escape_gitignore_path_segment, fresh_file_bytes, git,
    ignore_rows, missing_rule_indexes, push_pair, required_rules, ArtifactKind,
    TEST_AGENT_LOCAL_DIR,
};

#[test]
fn git_fixture_ignores_exactly_required_paths_without_untracking() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    assert_git_success(repo, &["init", "--quiet"]);

    let parent_gitignore = b"/.parent-ignore-sentinel\n";
    let info_exclude = b"/.exclude-sentinel\n";
    std::fs::write(repo.join(".gitignore"), parent_gitignore).expect("seed parent .gitignore");
    std::fs::write(repo.join(".git/info/exclude"), info_exclude).expect("seed info exclude");

    let config_dir = repo.join("instance");
    let required_paths = [
        "ac-root-agent/.agentscommander_amp-office/config.json",
        "ac-root-agent/config.json",
        ".settings.json.12345.tmp",
        // Depth independence of the atomic-write glob, inside a directory
        // this policy deliberately tracks: exactly the leftover class an
        // anchored rule would leave visible in `git status`.
        "coding-agents/.agents.10.default.json.4242.0.tmp",
        ".agentscommander-context-templates.json",
        ".agentscommander-injected-messages.json",
        ".api-clients-1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed.tmp",
        "activity.jsonl",
        "activity.jsonl.1",
        // The four agency template cache siblings, one sample per shape the
        // writers can produce. The three staging shapes are whole directory
        // trees and their rule carries no trailing slash, so the nested
        // samples are what prove a matched directory still ignores its
        // contents.
        "agency-agents_templates.lock",
        "agency-agents_templates.next-1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed/engineering/role.md",
        "agency-agents_templates.download-2c8e7dda-ccae-5c3e-ac6e-bc9eacce5df1/.git-clone-marker",
        "agency-agents_templates.prev-3d9f8eeb-ddbf-6d4f-bd7f-cdafbddf6ea2/design/role.md",
        "api-audit.log",
        "api-audit.log.1",
        "api-clients.json",
        "api-clients.lock",
        "api-message-bus.sqlite3",
        "api-message-bus.sqlite3-shm",
        "api-message-bus.sqlite3-wal",
        // The sample that makes the glob's reason for existing testable:
        // rollback-journal mode produces this sidecar and three literals
        // would not have covered it.
        "api-message-bus.sqlite3-journal",
        "app-outbox-path.txt",
        "app.log",
        // Both ends of the rotation range (`APP_LOG_KEEP = 5`), so the
        // sample set proves the glob spans every generation rather than only
        // its first. `app.log.1` used to be a control asserting it must NOT
        // be ignored; covering it is a declared reversal of that #1164
        // narrowness control, because it is a live rotated generation.
        "app.log.1",
        "app.log.5",
        "blocking-menus.state.no-git.json",
        "codex-home/agent-1/config.toml",
        // The nested sample proves the Dir row covers the `results/` subtree
        // in one rule.
        "coding-agent-requests/req-1.json",
        "coding-agent-requests/results/res-1.json",
        // #1968: the machine-local catalog children and every publication
        // temporary shape, including the doubled-dot journal temporary.
        "coding-agents/agents.50.personal.no-git.json",
        "coding-agents/agents.migration-v1.backup.json",
        "coding-agents/.agents.migration-v1.json",
        "coding-agents/.agents.10.default.json.lock",
        "coding-agents/.agents.50.personal.no-git.json.4242.0.tmp",
        // #2715: a catalog file the naming migration set aside.
        "coding-agents/agents.json.deprecated-1.no-git",
        // #2715: the retired catalog lock sidecar the migration holds.
        "coding-agents/.agents.json.lock",
        "coding-agents/.agents.migration-v1.backup.json.4242.0.tmp",
        "coding-agents/..agents.migration-v1.json.4242.0.tmp",
        "context-cache/ac-context-1.md",
        "coordinator_clocks.json",
        "coordinator_clocks.json.4242.7.tmp",
        "daemon.pid",
        "debug-logs.txt",
        "diag-raw.log",
        "diag-sent.log",
        "git-guard/git.cmd",
        "injected-messages.default.toml",
        "injected-messages.toml",
        "injected-messages.toml.bak-20260801T221533Z",
        "instances/0f0e/instance.json",
        "logs/harness.log",
        "master-token.txt",
        "orphaned-sessions.archive.json",
        // The `ORPHAN_ARCHIVE_KEEP` edge.
        "orphaned-sessions.archive.json.3",
        "project-refresh-requests/req-1.json",
        "pty-input-locks/operation-1.lock",
        "session-requests/create-1.json",
        "sessions.json",
        // #2058: the first and last rotation slots, the `SETTINGS_BACKUP_KEEP` edge.
        "settings.30.instance.no-git.backup.1.json",
        "settings.30.instance.no-git.backup.5.json",
        "settings.30.instance.no-git.json",
        "settings.30.instance.no-git.json.lock",
        // #2714: the settings writer's temporary, and the retired lock the
        // naming migration leaves on disk forever.
        "settings.30.instance.no-git.json.1.2.tmp",
        "settings.json.lock",
        // #2714: the naming-migration journal, its lock, and two set-aside
        // copies from different families.
        "naming-migration.state.no-git.json",
        ".naming-migration.state.no-git.json.lock",
        "settings.json.deprecated-1.no-git",
        "blocking-menus.json.deprecated-7.no-git",
        "blocking-menus.10.default.no-git.json",
        "blocking-menus.50.personal.no-git.json",
        "blocking-menus.20.remote.no-git.json",
        // #2133: the operator-owned per-agent help overlay.
        "agent-help.local.json",
        // #2133: the shipped per-agent help, rewritten by AC at every start.
        "agent-help.json",
        // #2133: the remote per-agent help cache and its throttle stamp.
        "agent-help-remote-check.json",
        "agent-help.remote.json",
        // #1737: the operator-owned overlay and the two managed context
        // template overrides. The settings row is an exact-name rule and does
        // not reach the personal overlay, which is why the row exists.
        "settings.50.personal.no-git.json",
        "Context.AgentsCommander.local.md",
        "Context.root-agent.local.md",
        "settings.30.instance.no-git.pre-384-v1.json",
        "settings.30.instance.no-git.pre-999-v9.json",
        "telegram-bridge.log",
        "ui-automation/session.json",
        "update-check.json",
        "update-check.json.tmp",
        "web-token.txt",
    ];
    for relative in required_paths {
        let path = config_dir.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture parent"))
            .expect("create fixture parent");
        std::fs::write(path, b"fixture").expect("write required fixture");
    }

    // `app.log` was already covered before this change; the database is a
    // newly covered artifact, which is the population a new rule actually
    // affects. Neither may be untracked by adding a rule.
    for tracked in ["instance/app.log", "instance/api-message-bus.sqlite3"] {
        assert_git_success(repo, &["add", "--", tracked]);
        assert_git_success(repo, &["ls-files", "--error-unmatch", "--", tracked]);
    }

    ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR)
        .expect("ensure fixture .gitignore");
    let generated = std::fs::read(config_dir.join(".gitignore")).expect("read generated file");

    for relative in required_paths {
        let repo_relative = format!("instance/{relative}");
        let output = git(
            repo,
            &[
                "check-ignore",
                "--no-index",
                "--quiet",
                "--",
                &repo_relative,
            ],
        );
        assert!(
            output.status.success(),
            "required path was not ignored: {repo_relative}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // #2714 E11: each of these is ignored by its own anchored rule, the two
    // set-aside copies by `SET_ASIDE_GLOB`.
    for (relative, rule) in [
        (
            "settings.30.instance.no-git.json",
            "/settings.30.instance.no-git.json",
        ),
        (
            "settings.30.instance.no-git.json.lock",
            "/settings.30.instance.no-git.json.lock",
        ),
        ("settings.json.lock", "/settings.json.lock"),
        (
            "settings.30.instance.no-git.json.1.2.tmp",
            "/settings.30.instance.no-git.json.*.tmp",
        ),
        (
            "naming-migration.state.no-git.json",
            "/naming-migration.state.no-git.json",
        ),
        (
            ".naming-migration.state.no-git.json.lock",
            "/.naming-migration.state.no-git.json.lock",
        ),
        (
            "settings.json.deprecated-1.no-git",
            "/*.deprecated-*.no-git",
        ),
        (
            "blocking-menus.json.deprecated-7.no-git",
            "/*.deprecated-*.no-git",
        ),
    ] {
        let repo_relative = format!("instance/{relative}");
        let output = assert_git_success(
            repo,
            &["check-ignore", "--no-index", "-v", "--", &repo_relative],
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let matched = stdout
            .split('\t')
            .next()
            .and_then(|source| source.rsplit(':').next())
            .unwrap_or_default();
        assert_eq!(matched, rule, "{repo_relative} matched by: {stdout}");
    }

    let control_paths = [
        "cache/entry.bin",
        "state.sqlite",
        "ac-root-agent/unrelated/config.json",
        "injected-messages.toml.bak",
        "injected-messages.json",
        "agentscommander-injected-messages.json",
        "sub/injected-messages.toml",
        // The Track set of product decision 6: these are user-editable and
        // must stay visible to git.
        "agent-templates/default-role.md",
        // Load-bearing: this is what proves the `agency-agents_templates.*`
        // glob does not reach the suffix-less tracked directory. The literal
        // dot is the whole mechanism.
        "agency-agents_templates/engineering/role.md",
        // Load-bearing: this is what proves the `coding-agent-requests/` row
        // does not reach its byte-order neighbour.
        "coding-agents/agents.10.default.json",
        // #1968: the tracked base and the tracked masters stay visible.
        "coding-agents/_seed/.claude/settings.json",
        // #1737: these two also prove the `*.local.md` glob does not reach the
        // tracked base templates whose overrides it covers.
        "Context.root-agent.md",
        // The standalone global context and both retirement-backup shapes:
        // all three hold the user's own bytes.
        "Context.AgentsCommander.md",
        "Context.AgentsCommander.md.retired-20260820-101112Z.bak",
        "Context.AgentsCommander.md.retired-20260820-101112Z.3.bak",
        "ac-root-agent/CLAUDE.md",
        // The atomic-write glob stays narrow, at the root and at depth.
        "foo.tmp",
        ".foo.tmp",
        "sub/foo.tmp",
        // The rotation rows are anchored globs, not a second any-depth rule:
        // they must not reach a subdirectory.
        "sub/app.log.1",
        "sub/activity.jsonl.1",
        // The literal dot in each rotation glob is load-bearing, not
        // decorative.
        "applog.1",
    ];
    for relative in control_paths {
        let path = config_dir.join(relative);
        std::fs::create_dir_all(path.parent().expect("control parent"))
            .expect("create control parent");
        std::fs::write(path, b"control").expect("write control fixture");
        let repo_relative = format!("instance/{relative}");
        let output = git(
            repo,
            &[
                "check-ignore",
                "--no-index",
                "--quiet",
                "--",
                &repo_relative,
            ],
        );
        assert_eq!(
            output.status.code(),
            Some(1),
            "unrequested control was ignored: {repo_relative}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // The generated file is asserted here, OUTSIDE the control loop above.
    // That loop writes `b"control"` into every path it checks, so a
    // `.gitignore` control would have overwritten the ruleset and left every
    // later control passing against an empty file. The byte comparison is
    // the assertion that would have caught that class.
    assert_git_ignore_status(repo, "instance/.gitignore", 1);
    assert_eq!(
        std::fs::read(config_dir.join(".gitignore")).expect("re-read generated file"),
        generated,
        "the fixture's own writes must not have touched the generated file"
    );

    for tracked in ["instance/app.log", "instance/api-message-bus.sqlite3"] {
        assert_git_success(repo, &["ls-files", "--error-unmatch", "--", tracked]);
    }
    assert_eq!(
        std::fs::read(repo.join(".gitignore")).expect("read parent .gitignore"),
        parent_gitignore
    );
    assert_eq!(
        std::fs::read(repo.join(".git/info/exclude")).expect("read info exclude"),
        info_exclude
    );
}

#[test]
fn dir_rows_require_a_real_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    assert_git_success(repo, &["init", "--quiet"]);
    let config_dir = repo.join("instance");
    std::fs::create_dir(&config_dir).expect("create config dir");
    ensure_instance_gitignore_at(&config_dir, TEST_AGENT_LOCAL_DIR)
        .expect("ensure dir-semantics fixture");

    let dir_rows: Vec<&str> = ignore_rows()
        .into_iter()
        .filter(|artifact| matches!(artifact.kind, ArtifactKind::Dir))
        .map(|artifact| artifact.name)
        .collect();
    assert!(
        !dir_rows.is_empty(),
        "the table has Dir rows to prove this on"
    );

    for name in dir_rows {
        // A plain file bearing the row's name is NOT ignored: that is the
        // whole reason `Dir` renders a trailing slash.
        let plain = config_dir.join(name);
        std::fs::write(&plain, b"fixture").expect("write plain-file sample");
        assert_git_ignore_status(repo, &format!("instance/{name}"), 1);
        std::fs::remove_file(&plain).expect("remove plain-file sample");

        // A file under a real directory of that name IS ignored.
        let nested = config_dir.join(name).join("nested-sample.txt");
        std::fs::create_dir_all(nested.parent().expect("nested parent"))
            .expect("create real directory");
        std::fs::write(&nested, b"fixture").expect("write nested sample");
        assert_git_ignore_status(repo, &format!("instance/{name}/nested-sample.txt"), 0);
    }
}

#[test]
fn literal_gitignore_segment_encoding_is_canonical() {
    for (input, expected) in [("\\", "\\\\"), ("*", "\\*"), ("?", "\\?"), ("[", "\\[")] {
        assert_eq!(
            escape_gitignore_path_segment(input).expect("encode metacharacter"),
            expected
        );
    }
    assert_eq!(
        escape_gitignore_path_segment(r"\*?[]-^#! spaced._(), café 東京")
            .expect("encode combined segment"),
        r"\\\*\?\[]-^#! spaced._(), café 東京"
    );

    for invalid in [
        ".agents\rinjected",
        ".agents\ninjected",
        ".agents/injected",
        ".agents\0injected",
    ] {
        let temp = tempfile::tempdir().expect("tempdir");
        let result = ensure_instance_gitignore_at(temp.path(), invalid);
        assert!(result.is_err(), "invalid segment was accepted: {invalid:?}");
        assert!(
            !temp.path().join(".gitignore").exists(),
            "invalid segment touched .gitignore: {invalid:?}"
        );
    }

    let agent_local_dir = r".agents\*?[]-^#! café";
    let escaped_agent_local_dir =
        escape_gitignore_path_segment(agent_local_dir).expect("encode agent local dir");
    let rules = required_rules(agent_local_dir).expect("construct canonical rules");
    assert_eq!(rules.len(), 2 + ignore_rows().len());
    assert_eq!(
        rules[0].pattern,
        format!(
            "/{}/{escaped_agent_local_dir}/config.json",
            super::super::super::ROOT_AGENT_DIR_NAME
        )
    );
    assert_eq!(
        rules[1].pattern,
        format!("/{}/config.json", super::super::super::ROOT_AGENT_DIR_NAME)
    );
    assert_eq!(
        rules
            .iter()
            .filter(|rule| rule.pattern.contains(&escaped_agent_local_dir))
            .count(),
        1
    );
    let raw_companion = format!(
        "/{}/{agent_local_dir}/config.json",
        super::super::super::ROOT_AGENT_DIR_NAME
    );
    assert!(!rules.iter().any(|rule| rule.pattern == raw_companion));
}

#[test]
fn git_fixture_treats_bracketed_agent_name_as_literal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    assert_git_success(repo, &["init", "--quiet"]);
    let config_dir = repo.join("instance");
    std::fs::create_dir(&config_dir).expect("create config dir");

    let agent_local_dir = ".agents[1]";
    ensure_instance_gitignore_at(&config_dir, agent_local_dir).expect("ensure bracketed fixture");
    let rules = required_rules(agent_local_dir).expect("canonical bracketed rules");
    assert_eq!(rules[0].pattern, r"/ac-root-agent/.agents\[1]/config.json");
    let generated = std::fs::read(config_dir.join(".gitignore")).expect("read .gitignore");
    assert_eq!(generated, fresh_file_bytes(&rules));
    assert_eq!(
        generated
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count(),
        rules.len() * 2
    );

    let literal = "instance/ac-root-agent/.agents[1]/config.json";
    let sibling = "instance/ac-root-agent/.agents1/config.json";
    for relative in [literal, sibling] {
        let path = repo.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture parent"))
            .expect("create fixture parent");
        std::fs::write(path, b"fixture").expect("write fixture");
    }
    assert_git_ignore_status(repo, literal, 0);
    assert_git_ignore_status(repo, sibling, 1);
}

#[cfg(unix)]
#[test]
fn git_fixture_treats_unix_metacharacter_agent_names_as_literal() {
    let cases = [
        (".agents*literal", ".agentsZZliteral"),
        (".agents?literal", ".agentsXliteral"),
        (".agents\\literal", ".agentsliteral"),
    ];

    for (agent_local_dir, unintended_sibling) in cases {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path();
        assert_git_success(repo, &["init", "--quiet"]);
        let config_dir = repo.join("instance");
        std::fs::create_dir(&config_dir).expect("create config dir");

        ensure_instance_gitignore_at(&config_dir, agent_local_dir)
            .expect("ensure metacharacter fixture");
        let rules = required_rules(agent_local_dir).expect("canonical metacharacter rules");
        let generated = std::fs::read(config_dir.join(".gitignore")).expect("read .gitignore");
        assert_eq!(generated, fresh_file_bytes(&rules));
        assert_eq!(
            generated
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .count(),
            rules.len() * 2
        );
        let raw_companion = format!(
            "/{}/{agent_local_dir}/config.json",
            super::super::super::ROOT_AGENT_DIR_NAME
        );
        assert!(!rules.iter().any(|rule| rule.pattern == raw_companion));

        let literal = format!(
            "instance/{}/{agent_local_dir}/config.json",
            super::super::super::ROOT_AGENT_DIR_NAME
        );
        let sibling = format!(
            "instance/{}/{unintended_sibling}/config.json",
            super::super::super::ROOT_AGENT_DIR_NAME
        );
        for relative in [&literal, &sibling] {
            let path = repo.join(relative);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            std::fs::write(path, b"fixture").expect("write fixture");
        }
        assert_git_ignore_status(repo, &literal, 0);
        assert_git_ignore_status(repo, &sibling, 1);
    }
}

#[test]
fn escaped_canonical_line_controls_detection_and_repair() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(".gitignore");
    let agent_local_dir = ".agents[1]";
    let rules = required_rules(agent_local_dir).expect("canonical rules");
    let canonical_first = &rules[0].pattern;
    let raw_first = format!(
        "/{}/{agent_local_dir}/config.json",
        super::super::super::ROOT_AGENT_DIR_NAME
    );

    assert!(!contains_exact_line(
        format!("{raw_first}\n").as_bytes(),
        canonical_first.as_bytes()
    ));
    assert!(contains_exact_line(
        format!("{canonical_first}\n").as_bytes(),
        canonical_first.as_bytes()
    ));

    let mut seed = Vec::new();
    seed.extend_from_slice(raw_first.as_bytes());
    seed.push(b'\n');
    for rule in &rules[1..] {
        seed.extend_from_slice(rule.pattern.as_bytes());
        seed.push(b'\n');
    }
    assert_eq!(missing_rule_indexes(&seed, &rules), vec![0]);
    std::fs::write(&path, &seed).expect("seed raw predecessor");

    ensure_instance_gitignore_at(temp.path(), agent_local_dir).expect("repair raw predecessor");
    let repaired = std::fs::read(&path).expect("read repaired file");
    let mut expected = seed.clone();
    push_pair(&mut expected, rules[0].comment, canonical_first);
    assert_eq!(repaired, expected);
    assert_eq!(missing_rule_indexes(&repaired, &rules), Vec::<usize>::new());

    ensure_instance_gitignore_at(temp.path(), agent_local_dir).expect("repeat repaired ensure");
    assert_eq!(std::fs::read(path).expect("read repeated result"), repaired);
}
