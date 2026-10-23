use super::*;

#[test]
fn windows_platform_defaults_and_historical_recognition_are_byte_exact() {
    let approved = concat!(
        r#"## Host Platform Rules

Windows host session: invoke `C:\Program Files\Git\bin\bash.exe` directly for all shell work and "#,
        r#"every AgentsCommander CLI invocation; avoid PowerShell whenever Git Bash is "#,
        r#"available. Only if PowerShell is unavoidable, wrap with `& 'C:\Program "#,
        r#"Files\Git\bin\bash.exe' -lc '...'` and capture CLI output with `2>&1 | "#,
        r#"Out-String`."#
    );
    assert_eq!(
        crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
        approved
    );
    assert_eq!(
        HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT.len(),
        277
    );
    assert_eq!(
        hash_text(HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT),
        "5fd5dd5f7d3d097f90e58cee6e6a210e2b2a6070c24e4164a7ac06d3854286a7"
    );
    assert_eq!(
        crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
        "## Host Platform Rules\n\nThis session runs on a Linux host; no platform-specific shell \
            routing rules apply."
    );
    assert_eq!(
        crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
        "## Host Platform Rules\n\nThis session runs on a macOS host; no platform-specific shell \
            routing rules apply."
    );
    for content in [approved, HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT] {
        assert!(is_known_generated_platform_windows(content));
        assert!(!is_known_generated_platform_windows(
            &content.replacen("Windows", "windows", 1)
        ));
        assert!(!is_known_generated_platform_windows(
            &content.replace('\n', "\r\n")
        ));
    }
}

fn write_historical_windows_platform_state(ac_root: &Path) {
    let state = serde_json::json!({
        "schemaVersion": 1,
        "templates": {
            "platform.windows": {
                "templateId": "platform.windows",
                "currentVersion": 1,
                "lastSeededSha256": hash_text(HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT)
            }
        }
    });
    std::fs::write(
        ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
        state.to_string(),
    )
    .expect("write historical seeded state");
}

#[test]
fn historical_windows_platform_migrates_with_and_without_state() {
    for trusted in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).unwrap();
        let filename = crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS;
        std::fs::write(
            ac_root.join(filename),
            HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT,
        )
        .unwrap();
        if trusted {
            write_historical_windows_platform_state(&ac_root);
        }
        assert_eq!(
            sync_for_read_at(&ac_root, filename, fixed_publication_time()).len(),
            1
        );
        let current = crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS;
        assert_eq!(
            std::fs::read_to_string(ac_root.join(filename)).unwrap(),
            current
        );
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(state["templates"]["platform.windows"]["currentVersion"], 2);
        assert_eq!(
            state["templates"]["platform.windows"]["lastSeededSha256"],
            hash_text(current)
        );
        assert!(scan_project_context_template_updates(temp.path(), &ac_root)
            .unwrap()
            .is_empty());
        assert!(sync_for_read_at(&ac_root, filename, fixed_publication_time()).is_empty());
    }
}

#[test]
fn historical_windows_customization_is_preserved_with_and_without_state() {
    for trusted in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).unwrap();
        let filename = crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS;
        let custom = format!(
            "{}\nCustom rules",
            HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT
        );
        std::fs::write(ac_root.join(filename), &custom).unwrap();
        if trusted {
            write_historical_windows_platform_state(&ac_root);
        }
        assert!(sync_for_read_at(&ac_root, filename, fixed_publication_time()).is_empty());
        let updates = scan_project_context_template_updates(temp.path(), &ac_root).unwrap();
        assert_eq!(
            std::fs::read_to_string(ac_root.join(filename)).unwrap(),
            custom
        );
        if trusted {
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].filename, filename);
            assert_eq!(updates[0].current_default_version, 2);
        } else {
            assert!(updates.is_empty());
            let state_path = ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
            if state_path.exists() {
                let state: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(state_path).unwrap()).unwrap();
                assert!(state["templates"]["platform.windows"].is_null());
            }
        }
    }
}

// #1065 Stage F activation coverage. These tests drive the real context
// publication engine through a held `ProjectSeedManifestGuard` with a production
// recording closure, asserting the resulting manifest row. Removing the
// `record_project_context_publication` adapter call would leave no manifest and
// fail them (plan acceptance item 22).

#[test]
fn issue_2868_bootstrap_missing_ordinary_and_occupied_source_still_publishes() {
    for occupied in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let ac = temp.path().join(".ac");
        std::fs::create_dir(&ac).unwrap();
        let source = ac.join("project-skills");
        if occupied {
            std::fs::write(&source, "OCCUPIED_BOOTSTRAP").unwrap();
        }
        for _ in 0..2 {
            let mut clock = fixed_publication_time;
            let mut published = Vec::new();
            let mut on_publication = |filename: &'static str, _: ContextPublication| {
                published.push(filename);
            };
            ensure_project_context_templates_with_clock(&ac, &mut clock, &mut on_publication)
                .unwrap();
            for name in ["plans", "tools", "errors", "project-shared"] {
                assert!(ac.join(name).is_dir());
            }
            assert!(ac.join("Context.AgentsCommander.md").is_file());
            if occupied {
                assert_eq!(
                    std::fs::read_to_string(&source).unwrap(),
                    "OCCUPIED_BOOTSTRAP"
                );
            } else {
                assert!(source.is_dir());
            }
        }
    }
}

#[test]
fn issue_2868_four_directory_failure_still_blocks_before_project_skills() {
    let temp = tempfile::tempdir().unwrap();
    let ac = temp.path().join(".ac");
    std::fs::create_dir(&ac).unwrap();
    std::fs::write(ac.join("plans"), "BLOCK_SHARED").unwrap();
    let error = ensure_project_context_templates(&ac).unwrap_err();
    assert!(error.contains("failed to create project shared directories"));
    assert!(!ac.join("project-skills").exists());
    assert!(!ac.join("Context.AgentsCommander.md").exists());
    assert_eq!(
        std::fs::read_to_string(ac.join("plans")).unwrap(),
        "BLOCK_SHARED"
    );
}

#[test]
fn context_create_records_project_templates_under_the_gate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path();
    let ac_root = project.join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");

    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    let published_at = fixed_publication_time();
    let mut guard = crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(project)
        .expect("acquire project gate");
    {
        let mut clock = || published_at;
        let mut on_publication = |filename: &'static str, publication: ContextPublication| {
            crate::config::session_context::record_project_context_publication(
                &mut guard,
                &token,
                filename,
                publication.published_at,
            );
        };
        ensure_project_context_templates_with_clock(&ac_root, &mut clock, &mut on_publication)
            .expect("ensure project context templates");
    }
    guard.release();

    let manifest = std::fs::read_to_string(ac_root.join("seed-manifest.toml"))
        .expect("fresh project context creation records a seed manifest");
    assert!(
        manifest.contains("scope = \"context:agentscommander\""),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"context:coordinator\""),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"context:platform\""),
        "platform seed publications must record the context:platform scope: {manifest}"
    );
    assert!(
        manifest.contains("kind = \"project_context_template\""),
        "manifest: {manifest}"
    );
}

#[test]
fn coordinator_v3_to_v4_update_records_to_the_seed_manifest_under_the_gate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path();
    let ac_root = project.join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
    )
    .expect("write pristine v3 coordinator");

    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    let published_at = fixed_publication_time();
    let mut guard = crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(project)
        .expect("acquire project gate");
    {
        let mut clock = || published_at;
        let mut on_publication = |filename: &'static str, publication: ContextPublication| {
            crate::config::session_context::record_project_context_publication(
                &mut guard,
                &token,
                filename,
                publication.published_at,
            );
        };
        sync_project_context_template_for_read_with_clock(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            &mut clock,
            &mut on_publication,
        )
        .expect("sync coordinator for read");
    }
    guard.release();

    let manifest = std::fs::read_to_string(ac_root.join("seed-manifest.toml"))
        .expect("a recognized coordinator v3->v4 update records a seed manifest row");
    assert!(
        manifest.contains("scope = \"context:coordinator\""),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("2026-07-20T10:30:45.123Z"),
        "the commit-point publication time is recorded: {manifest}"
    );
}

#[test]
fn old_coordinator_default_is_known_generated_without_raise_hand() {
    assert!(!OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.contains("## Raising Your Hand"));
    assert!(OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.contains("## Sending Screenshots"));
    let misleading_names = "names can be misleading.";
    assert!(OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.contains(misleading_names));
    assert!(is_known_generated_coordinator_template(
        OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND
    ));
}

/// #1005 S4 / G3: the frozen v2 snapshot must stay byte-identical to what the
/// #684..1dd0b58 builds shipped. Expected values captured by a one-off run of
/// the shipped accessor AT base commit 1dd0b58, never from this const.
#[test]
fn coordinator_pre_token_minimization_snapshot_is_byte_exact() {
    assert_eq!(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION.len(),
        2403,
        "frozen v2 coordinator snapshot must be the 1dd0b58 bytes"
    );
    assert_eq!(
        hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION),
        "92f3abfc108147b07f1c4a49e7062c0f4d0d9aae570b7e5195852c31bb8b0d02",
        "frozen v2 coordinator snapshot changed; it must stay byte-identical to what shipped"
    );
}

/// #1030: the frozen v3 coordinator snapshot must stay byte-identical to
/// what shipped at base commit 4acadfe5. Expected values captured by a one-off
/// run of the shipped accessor AT 4acadfe5 (plan E2), never from this const.
#[test]
fn coordinator_pre_cross_workgroup_snapshot_is_byte_exact() {
    assert_eq!(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE.len(),
        2296,
        "frozen v3 coordinator snapshot must be the 4acadfe5 bytes"
    );
    assert_eq!(
        hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE),
        "9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84",
        "frozen v3 coordinator snapshot changed; it must stay byte-identical to what shipped"
    );
}

/// #1571 T1: the frozen v4 coordinator snapshot must stay byte-identical to
/// what shipped at base commit ecc6527b. Expected values captured by a one-off
/// run of the shipped accessor AT ecc6527b (plan 3.6), never from this const.
#[test]
fn coordinator_pre_orchestrator_rename_snapshot_is_byte_exact() {
    assert_eq!(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME.len(),
        2509,
        "frozen v4 coordinator snapshot must be the ecc6527b bytes"
    );
    assert_eq!(
        hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME),
        "f6ef7894b9f0f606e945c282d769144e96487fcc01ab435c9aab8019bb3ce1f6",
        "frozen v4 coordinator snapshot changed; it must stay byte-identical to what shipped"
    );
}

/// #1571 T5: the frozen v1 coordinator snapshot had no byte pin, and the
/// existing `old_coordinator_default_is_known_generated_without_raise_hand`
/// cannot detect an edit to it: its recognizer assertion is self-referential
/// and its three `contains()` assertions do not cover the first line, which is
/// the very sentence the #1571 rename rewrites elsewhere. Expected values
/// captured by a one-off run AT ecc6527b (plan 9.1), never from this const.
#[test]
fn old_coordinator_raise_hand_snapshot_is_byte_exact() {
    assert_eq!(
        OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.len(),
        2066,
        "frozen v1 coordinator snapshot must be the ecc6527b bytes"
    );
    assert_eq!(
        hash_text(OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND),
        "31d49d02c12fcc8cd2d5277455074dcae3fbc1a84f1f1a0cf0f37828e03f792f",
        "frozen v1 coordinator snapshot changed; it must stay byte-identical to what shipped"
    );
}

/// #1005 S4 failing-first migration proof: the assert_ne fails while the live
/// template still equals the frozen v2 bytes (pre-rewrite), and the sync half
/// proves a pristine v2 file on disk auto-upgrades to the current default.
#[test]
fn read_sync_updates_pre_token_minimization_coordinator_template() {
    assert_ne!(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
        get_default_coordinator_template(),
        "v3 rewrite must actually change the template or the freeze is pointless"
    );
    assert!(is_known_generated_coordinator_template(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
    ));

    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
    )
    .expect("write pristine v2 coordinator");

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
