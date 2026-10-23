use super::*;

/// #1369 (C4) AC-4.5-C: population C, the edge case - a pristine v2 on disk
/// with NO `global` state entry. Failing-first proof for the v3 rename
/// (assert_ne), the #979 standalone recognizer and the version bump,
/// retargeted by #1748 at the scan.
#[test]
fn scan_replaces_pre_agent_repos_global_template() {
    assert_ne!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS,
        crate::config::session_context::get_default_agent_template(),
        "the v3 rename must actually change the template or the freeze is pointless"
    );
    assert!(
        is_known_generated_standalone_global_template(GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS),
        "standalone (retirement) recognizer must accept the frozen v2 bytes"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS,
    )
    .expect("write pristine v2 global");

    assert_eq!(
        scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
            .expect("scan pristine v2 global")
            .len(),
        1,
        "population C has no state entry, so the replacement is notified"
    );

    let content = std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
        .expect("read global");
    assert_eq!(
        content,
        crate::config::session_context::get_default_agent_template(),
        "pristine v2 Context.AgentsCommander.md must be repaired"
    );
    assert_eq!(backup_files(&ac_root).len(), 1);
    let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("read seeded state");
    let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
    assert_eq!(
        parsed["templates"]["global"]["currentVersion"],
        serde_json::Value::Null
    );
    assert!(
        scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan updates")
            .is_empty(),
        "a repaired template must not leave a pending update"
    );
}

/// #1369 (C4) AC-4.5-B: population B, the dominant one - a pristine v2 on
/// disk PLUS a `global` state entry whose `lastSeededSha256` is the v2 hash,
/// which is what any project seeded by a normal build carries. Without the
/// freeze this yields 1 pending update, i.e. the modal telling the user that
/// a file they never touched "appears customized".
#[test]
fn scan_replaces_pre_agent_repos_global_template_with_state_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS,
    )
    .expect("write pristine v2 global");
    let mut state = SeededContextTemplateState::default();
    state.templates.insert(
        "global".to_string(),
        SeededContextTemplateEntry {
            template_id: "global".to_string(),
            current_version: Some(2),
            last_seeded_sha256: Some(hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS)),
            last_observed_sha256: None,
            ignored_default_sha256: None,
            ignored_observed_sha256: None,
        },
    );
    persist_state(&ac_root, &state).expect("persist v2 state entry");

    assert!(
        scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
            .expect("scan the dominant population")
            .is_empty(),
        "the state names these exact bytes, so the repair is silent"
    );

    assert_eq!(
        std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read global"),
        crate::config::session_context::get_default_agent_template(),
        "a pristine v2 with a trusted state entry must be repaired silently"
    );
    assert_eq!(backup_files(&ac_root).len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state"),
    )
    .expect("parse seeded state");
    assert_eq!(
        parsed["templates"]["global"]["currentVersion"],
        serde_json::Value::Null
    );
    assert!(
        scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan updates")
            .is_empty(),
        "the dominant population must never be accused of having customized the file"
    );
}

#[test]
fn scan_existing_ac_does_not_create_missing_templates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");

    let updates =
        scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");

    assert!(updates.is_empty());
    assert!(!ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME).exists());
    assert!(!ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).exists());
}

#[test]
fn equal_default_observation_has_no_publication() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        crate::config::session_context::get_default_agent_template(),
    )
    .expect("write current global default");

    let publications = sync_for_read_at(
        &ac_root,
        GLOBAL_CONTEXT_TEMPLATE_FILENAME,
        fixed_publication_time(),
    );

    assert!(
        publications.is_empty(),
        "observing equal default bytes must not manufacture a publication"
    );
}

#[test]
fn generated_update_lost_race_has_no_publication_or_clock_sample() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let path = ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(
        &path,
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
    )
    .expect("write generated coordinator");
    let spec =
        project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).expect("coordinator spec");
    let mut clock_calls = 0_u32;
    let mut clock = || {
        clock_calls += 1;
        fixed_publication_time()
    };

    let execution =
        auto_update_generated_template(&path, spec, "different-observed-hash", &mut clock);

    assert_eq!(execution.published, None);
    assert_eq!(
        execution.completion.expect("lost race outcome"),
        TemplatePublication::ChangedUnderUs
    );
    assert_eq!(clock_calls, 0);
    assert_eq!(
        std::fs::read_to_string(path).expect("read preserved target"),
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
    );
}

#[test]
fn generated_update_publish_failure_has_no_publication() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(
        &path,
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
    )
    .expect("write generated coordinator");
    let expected_hash = hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION);
    let spec =
        project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).expect("coordinator spec");
    let mut clock = || fixed_publication_time();

    let execution =
        auto_update_generated_template_with(&path, spec, &expected_hash, &mut clock, |_, _, _| {
            Err("injected atomic publication failure".to_string())
        });

    assert_eq!(execution.published, None);
    assert_eq!(
        execution
            .completion
            .expect_err("publication failure must remain an error"),
        "injected atomic publication failure"
    );
    assert_eq!(
        std::fs::read_to_string(path).expect("read preserved target"),
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
    );
}

#[cfg(windows)]
#[test]
fn readonly_generated_update_has_no_publication_or_clock_sample() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(
        &path,
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
    )
    .expect("write generated coordinator");
    let expected_hash = hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION);
    let original_permissions = std::fs::metadata(&path)
        .expect("read target metadata")
        .permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).expect("make target read-only");
    let spec =
        project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).expect("coordinator spec");
    let mut clock_calls = 0_u32;
    let mut clock = || {
        clock_calls += 1;
        fixed_publication_time()
    };

    let execution = auto_update_generated_template(&path, spec, &expected_hash, &mut clock);

    std::fs::set_permissions(&path, original_permissions).expect("restore target permissions");
    assert_eq!(execution.published, None);
    assert!(execution.completion.is_err());
    assert_eq!(clock_calls, 0);
    assert_eq!(
        std::fs::read_to_string(path).expect("read preserved target"),
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
    );
}

#[test]
fn create_publication_survives_post_commit_validation_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let published_at = fixed_publication_time();
    let mut clock = || {
        std::fs::remove_file(&path).expect("remove published target at validation seam");
        std::fs::create_dir(&path).expect("replace published target with directory");
        published_at
    };

    let execution = create_missing_template(&path, "published bytes", &mut clock);

    assert_eq!(
        execution.published,
        Some(ContextPublication { published_at })
    );
    assert!(execution
        .completion
        .expect_err("post-commit validation must fail")
        .contains("not a regular file"));
}

#[test]
fn ensure_consumes_first_publication_before_the_next_target_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::create_dir(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
        .expect("block coordinator target with directory");
    let published_at = fixed_publication_time();
    let mut clock = || published_at;
    let mut publications = Vec::new();

    let error = ensure_project_context_templates_with_clock(
        &ac_root,
        &mut clock,
        &mut |filename, publication| publications.push((filename, publication)),
    )
    .expect_err("second target must fail");

    assert!(error.contains("not a regular file"), "{error}");
    assert_one_publication(
        &publications,
        GLOBAL_CONTEXT_TEMPLATE_FILENAME,
        published_at,
    );
    assert_eq!(
        std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read first published target"),
        crate::config::session_context::get_default_agent_template()
    );
}

/// #1605: a fresh `.ac` root seeds the three platform files byte-equal to
/// their embedded defaults with `platform.*` state entries carrying the
/// default sha; a pre-existing custom platform file is never overwritten
/// (absent-only) and is preserved silently.
#[test]
fn ensure_project_context_templates_seeds_platform_files_absent_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");

    let custom_windows = "## Host Platform Rules\n\nMY OWN WINDOWS RULES\n";
    std::fs::write(
        ac_root.join(crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS),
        custom_windows,
    )
    .expect("write pre-existing custom windows platform file");

    let published_at = fixed_publication_time();
    let mut clock = || published_at;
    let mut publications = Vec::new();
    ensure_project_context_templates_with_clock(
        &ac_root,
        &mut clock,
        &mut |filename, publication| publications.push((filename, publication)),
    )
    .expect("ensure project context templates");

    assert_eq!(
        std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read global"),
        crate::config::session_context::get_default_agent_template()
    );
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator"),
        get_default_coordinator_template()
    );
    for (filename, default) in [
        (
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
        ),
        (
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
        ),
    ] {
        assert_eq!(
            std::fs::read_to_string(ac_root.join(filename)).expect("read seeded platform file"),
            default,
            "{filename} must be seeded byte-equal to its embedded default"
        );
    }
    assert_eq!(
        std::fs::read_to_string(
            ac_root.join(crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS)
        )
        .expect("read preserved windows platform file"),
        custom_windows,
        "a pre-existing custom platform file must be preserved, never overwritten"
    );

    let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("read seeded state");
    let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
    for (id, default) in [
        (
            "platform.linux",
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
        ),
        (
            "platform.macos",
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
        ),
    ] {
        assert_eq!(
            parsed["templates"][id]["currentVersion"],
            if id == "platform.windows" { 2 } else { 1 },
            "{id} state entry must carry the current version"
        );
        assert_eq!(
            parsed["templates"][id]["lastSeededSha256"],
            hash_text(default),
            "{id} state entry must carry the default sha"
        );
    }
    assert_eq!(
        parsed["templates"]["platform.windows"],
        serde_json::Value::Null,
        "a stateless pre-existing custom platform file must stay unowned (same posture as the \
            coordinator template)"
    );
}

/// #1625 T-3: `ensure_platform_context_templates` seeds ONLY the missing
/// platform files, byte-equal to their embedded defaults, with `platform.*`
/// state entries at current versions carrying the default sha; global/coordinator templates
/// are never touched (scope is platform-only). A pre-existing custom
/// platform file is preserved and stays unowned (silent preservation via
/// `suppress_unknown_without_state`).
#[test]
fn ensure_platform_context_templates_seeds_only_missing_platform_files() {
    let platform_files = [
        (
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
        ),
        (
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
        ),
        (
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
        ),
    ];
    let assert_platform_only_scope = |ac_root: &Path| {
        assert!(
            !ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME).exists(),
            "global template is out of scope for the platform seeder"
        );
        assert!(
            !ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).exists(),
            "coordinator template is out of scope for the platform seeder"
        );
    };

    // Fresh `.ac`: all three platform files are created byte-equal to their
    // embedded defaults and the state records Windows v2 and Linux/macOS v1
    // with `lastSeededSha256` = hash of the default.
    let temp = tempfile::tempdir().expect("tempdir");
    let fresh = temp.path().join(".ac");
    std::fs::create_dir(&fresh).expect("create workspace");
    ensure_platform_context_templates(&fresh).expect("seed platform templates");
    for (filename, default) in platform_files {
        assert_eq!(
            std::fs::read_to_string(fresh.join(filename)).expect("read seeded platform file"),
            default,
            "{filename} must be seeded byte-equal to its embedded default"
        );
    }
    let state = std::fs::read_to_string(fresh.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("read seeded state");
    let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
    for (id, default) in [
        (
            "platform.windows",
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
        ),
        (
            "platform.linux",
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
        ),
        (
            "platform.macos",
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
        ),
    ] {
        assert_eq!(
            parsed["templates"][id]["currentVersion"],
            if id == "platform.windows" { 2 } else { 1 },
            "{id} state entry must carry the current version"
        );
        assert_eq!(
            parsed["templates"][id]["lastSeededSha256"],
            hash_text(default),
            "{id} state entry must carry the default sha"
        );
    }
    assert_platform_only_scope(&fresh);

    // Pre-existing custom file: preserved byte-for-byte and left unowned
    // (no state entry); the other two are still seeded absent-only.
    let temp2 = tempfile::tempdir().expect("tempdir");
    let custom_root = temp2.path().join(".ac");
    std::fs::create_dir(&custom_root).expect("create workspace");
    let custom_windows = "## Host Platform Rules\n\nMY OWN WINDOWS RULES\n";
    std::fs::write(
        custom_root.join(crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS),
        custom_windows,
    )
    .expect("write pre-existing custom windows platform file");
    ensure_platform_context_templates(&custom_root).expect("seed platform templates");
    assert_eq!(
        std::fs::read_to_string(
            custom_root.join(crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS)
        )
        .expect("read preserved windows platform file"),
        custom_windows,
        "a pre-existing custom platform file must be preserved, never overwritten"
    );
    for (filename, default) in platform_files.iter().filter(|(filename, _)| {
        *filename != crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS
    }) {
        assert_eq!(
            std::fs::read_to_string(custom_root.join(filename)).expect("read seeded platform file"),
            *default,
            "{filename} must be seeded byte-equal to its embedded default"
        );
    }
    let state2 = std::fs::read_to_string(custom_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("read seeded state");
    let parsed2: serde_json::Value = serde_json::from_str(&state2).expect("parse seeded state");
    assert_eq!(
        parsed2["templates"]["platform.windows"],
        serde_json::Value::Null,
        "a stateless pre-existing custom platform file must stay unowned"
    );
    assert_eq!(
        parsed2["templates"]["platform.linux"]["currentVersion"], 1,
        "platform.linux must be seeded in the mixed scenario"
    );
    assert_eq!(
        parsed2["templates"]["platform.macos"]["currentVersion"], 1,
        "platform.macos must be seeded in the mixed scenario"
    );
    assert_platform_only_scope(&custom_root);
}

/// #1605: after seeding, an edit to a platform file is preserved by the
/// sync path (content unchanged) and lands in the observed posture
/// (`lastObservedSha256` updated); a scan against the same default yields no
/// pending update, and the file is never silently overwritten.
#[test]
fn platform_file_edit_is_preserved_and_observed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    ensure_project_context_templates(&ac_root).expect("seed platform files");

    let filename = crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS;
    let edited = format!(
        "{}\n\nMY OWN WINDOWS RULES\n",
        crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS
    );
    std::fs::write(ac_root.join(filename), &edited).expect("edit windows platform file");

    let published_at = fixed_publication_time();
    let publications = sync_for_read_at(&ac_root, filename, published_at);
    assert!(
        publications.is_empty(),
        "an edited platform file must not publish"
    );
    assert_eq!(
        std::fs::read_to_string(ac_root.join(filename)).expect("read edited platform file"),
        edited,
        "the edit must be preserved, never overwritten"
    );

    let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("read seeded state");
    let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
    assert_eq!(
        parsed["templates"]["platform.windows"]["lastObservedSha256"],
        hash_text(&edited),
        "the edit must land in the observed posture"
    );

    let updates =
        scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");
    assert_eq!(
        updates.len(),
        1,
        "a customized platform file must be offered as a pending update"
    );
    assert_eq!(updates[0].filename, filename);
    assert_eq!(updates[0].current_file_sha256, hash_text(&edited));
    assert_eq!(
        updates[0].current_default_sha256,
        hash_text(crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS)
    );
    assert_eq!(updates[0].current_default_version, 2);
    assert_eq!(
        std::fs::read_to_string(ac_root.join(filename)).expect("re-read edited platform file"),
        edited,
        "the scan must never silently overwrite the edit"
    );
}

#[test]
fn read_sync_creates_missing_coordinator_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");

    let published_at = fixed_publication_time();
    let publications = sync_for_read_at(
        &ac_root,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        published_at,
    );
    assert_one_publication(
        &publications,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        published_at,
    );

    let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
        .expect("read coordinator");
    assert_eq!(content, get_default_coordinator_template());
}
