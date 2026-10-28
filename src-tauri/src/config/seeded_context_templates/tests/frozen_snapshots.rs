use super::*;

fn issue_2832_state(dir: &Path, content: &str, version: u32) {
    let state = serde_json::json!({"schemaVersion":1,"templates":{"rootAgent":{
        "templateId":"rootAgent","currentVersion":version,"lastSeededSha256":hash_text(content)
    }}});
    std::fs::write(
        dir.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
        state.to_string(),
    )
    .unwrap();
}

#[test]
fn issue_2832_every_exact_snapshot_migrates_despite_state_and_preserves_backup() {
    let snapshots = crate::config::root_agent::frozen_root_context_snapshots();
    let mut cases = 0;
    for old in snapshots {
        for state in 0..4 {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join(root_spec().filename);
            std::fs::write(&path, old).unwrap();
            let existing = temp.path().join(format!("{}.bak", root_spec().filename));
            std::fs::write(&existing, "PREEXISTING BACKUP").unwrap();
            match state {
                1 => issue_2832_state(temp.path(), "stale", 1),
                2 => issue_2832_state(temp.path(), old, 8),
                3 => issue_2832_state(temp.path(), "forged", 10),
                _ => {}
            }
            ensure_root_context_template(temp.path()).unwrap();
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                (root_spec().current_content)()
            );
            let backups = backup_files(temp.path());
            assert_eq!(backups.len(), 2);
            assert_eq!(
                std::fs::read_to_string(&existing).unwrap(),
                "PREEXISTING BACKUP"
            );
            let backup = backups.iter().find(|p| **p != existing).unwrap();
            assert_eq!(std::fs::read(backup).unwrap(), old.as_bytes());
            let state_bytes =
                std::fs::read(temp.path().join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)).unwrap();
            let state: serde_json::Value = serde_json::from_slice(&state_bytes).unwrap();
            assert_eq!(state["templates"]["rootAgent"]["currentVersion"], 10);
            assert_eq!(
                state["templates"]["rootAgent"]["lastSeededSha256"],
                hash_text((root_spec().current_content)())
            );
            ensure_root_context_template(temp.path()).unwrap();
            assert_eq!(backup_files(temp.path()), backups);
            assert_eq!(
                std::fs::read(temp.path().join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)).unwrap(),
                state_bytes
            );
            cases += 1;
        }
    }
    assert_eq!(cases, 48);
    println!("issue_2832 exact-snapshot/state cases: {cases}");
}

#[test]
fn issue_2832_custom_bytes_survive_absent_and_forged_state() {
    let mut snapshots = crate::config::root_agent::frozen_root_context_snapshots().to_vec();
    snapshots.push((root_spec().current_content)());
    let mut cases = 0;
    for source in snapshots {
        let edits = [
            format!("{source} "),
            format!(" {source}"),
            format!("{source}\n"),
            source.strip_suffix('\n').unwrap().to_string(),
            source.replacen("AgentsCommander", "AgentsCommandex", 1),
            source.replace('\n', "\r\n"),
            String::new(),
            "\n".to_string(),
        ];
        for custom in edits {
            assert!(!crate::config::root_agent::is_known_generated_root_context_template(&custom));
            for forged in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let path = temp.path().join(root_spec().filename);
                std::fs::write(&path, &custom).unwrap();
                if forged {
                    issue_2832_state(temp.path(), &custom, 8);
                }
                for _ in 0..2 {
                    ensure_root_context_template(temp.path()).unwrap();
                    assert_eq!(std::fs::read(&path).unwrap(), custom.as_bytes());
                    assert!(backup_files(temp.path()).is_empty());
                }
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 208);
    println!("issue_2832 custom-byte/state cases: {cases}");
}

#[test]
fn issue_2832_fresh_and_current_v10_do_not_backup_or_rewrite() {
    let temp = tempfile::tempdir().unwrap();
    ensure_root_context_template(temp.path()).unwrap();
    let path = temp.path().join(root_spec().filename);
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    ensure_root_context_template(temp.path()).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        (root_spec().current_content)()
    );
    assert!(backup_files(temp.path()).is_empty());
}

#[test]
fn issue_2832_backup_failures_prevent_publication_and_preserve_source_and_state() {
    let old = crate::config::root_agent::frozen_root_context_snapshots()[8];
    for operation in ["create", "write", "flush", "sync"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(root_spec().filename);
        std::fs::write(&path, old).unwrap();
        issue_2832_state(temp.path(), old, 8);
        let prior =
            std::fs::read(temp.path().join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)).unwrap();
        let mut clock = || fixed_publication_time();
        let execution = auto_update_generated_template_with_backup(
            &path,
            root_spec(),
            &hash_text(old),
            &mut clock,
            |path, bytes| {
                assert_eq!(bytes, old.as_bytes());
                Err(format!(
                    "Failed to {operation} backup context template {}.bak: injected",
                    path.display()
                ))
            },
            |_, _, _| panic!("publication must not run after failed {operation}"),
        );
        assert!(execution.completion.unwrap_err().contains(operation));
        assert!(execution.published.is_none());
        assert_eq!(std::fs::read(&path).unwrap(), old.as_bytes());
        assert_eq!(
            std::fs::read(temp.path().join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)).unwrap(),
            prior
        );
    }
}

#[test]
fn issue_2832_backup_precedes_publication_and_replace_failure_preserves_recovery() {
    let old = crate::config::root_agent::frozen_root_context_snapshots()[8];
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(root_spec().filename);
    std::fs::write(&path, old).unwrap();
    let mut clock = || fixed_publication_time();
    let execution = auto_update_generated_template_with(
        &path,
        root_spec(),
        &hash_text(old),
        &mut clock,
        |path, _, _| {
            let backups = backup_files(path.parent().unwrap());
            assert_eq!(backups.len(), 1);
            assert_eq!(std::fs::read(&backups[0]).unwrap(), old.as_bytes());
            Err("injected replacement failure".to_string())
        },
    );
    assert!(execution.completion.is_err());
    assert!(execution.published.is_none());
    assert_eq!(std::fs::read(path).unwrap(), old.as_bytes());
    assert_eq!(backup_files(temp.path()).len(), 1);
}

#[test]
fn issue_2832_edit_or_removal_after_backup_has_no_publication() {
    let old = crate::config::root_agent::frozen_root_context_snapshots()[8];
    for remove in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(root_spec().filename);
        std::fs::write(&path, old).unwrap();
        let mut clock = || fixed_publication_time();
        let execution = auto_update_generated_template_with_backup(
            &path,
            root_spec(),
            &hash_text(old),
            &mut clock,
            |path, bytes| {
                let backup = create_backup(path, bytes)?;
                if remove {
                    std::fs::remove_file(path).unwrap();
                } else {
                    std::fs::write(path, "OPERATOR EDIT").unwrap();
                }
                Ok(backup)
            },
            |_, _, _| panic!("changed source must not publish"),
        );
        assert_eq!(
            execution.completion.unwrap(),
            TemplatePublication::ChangedUnderUs
        );
        assert!(execution.published.is_none());
        assert_eq!(
            std::fs::read(&backup_files(temp.path())[0]).unwrap(),
            old.as_bytes()
        );
        if remove {
            assert!(!path.exists());
        } else {
            assert_eq!(std::fs::read_to_string(path).unwrap(), "OPERATOR EDIT");
        }
    }
}

#[test]
fn issue_2832_state_persistence_failure_keeps_published_v9_and_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(root_spec().filename);
    let old = crate::config::root_agent::frozen_root_context_snapshots()[8];
    std::fs::write(&path, old).unwrap();
    std::fs::create_dir(temp.path().join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)).unwrap();
    ensure_root_context_template(temp.path()).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        (root_spec().current_content)()
    );
    assert_eq!(
        std::fs::read(&backup_files(temp.path())[0]).unwrap(),
        old.as_bytes()
    );
    ensure_root_context_template(temp.path()).unwrap();
    assert_eq!(
        std::fs::metadata(path).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(backup_files(temp.path()).len(), 1);
}

/// #1614 AC7.9 and AC7.11. #1748 retired the PROJECT global recognizer, so
/// this constant's only recognizer is now the #979 standalone classifier; the
/// snapshot must still be accepted by it and be != the current default -- the
/// assert_ne is what proves the rename actually moved the default rather than
/// freezing a copy of something unchanged.
#[test]
fn frozen_pre_room_rename_global_template_is_recognized() {
    assert!(is_known_generated_standalone_global_template(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
    ));
    assert_ne!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
        crate::config::session_context::get_default_agent_template(),
        "the Room rename must actually change the global default or the freeze is pointless"
    );
    assert!(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.contains("**Workgroup**"));
    assert!(
        !crate::config::session_context::get_default_agent_template()
            .to_lowercase()
            .contains("workgroup")
    );
}

/// #1614 AC7.1b: the merge-resolution guard for the #1605 collision. Two
/// limbs, catching two different failures.
///
/// The `||`-chain limb catches a three-way merge over two adjacent
/// single-line additions to one recognizer chain silently dropping one of
/// them: main added `_BEFORE_HOST_PLATFORM_RULES` and this branch added
/// `_BEFORE_ROOM_RENAME` in the same place, and keeping only the newer one
/// would stop recognizing every pristine v4 file.
///
/// The `!=` limb is the one that catches the genuinely silent failure:
/// both recognizer lines kept but the frozen body NOT re-based, which
/// leaves two constants holding byte-identical bytes under two names with
/// v5 permanently unrecognized. No conflict, no compile error and no other
/// criterion in this plan surfaces that; before plan section 15.3's
/// re-base both constants really were 539 bytes and F4406596...316A.
#[test]
fn both_frozen_global_generations_are_standalone_recognized_and_distinct() {
    // The ||-chain limb: #1748 left one recognizer for these bytes, and neither
    // generation may be dropped from it.
    assert!(is_known_generated_standalone_global_template(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
    ));
    assert!(is_known_generated_standalone_global_template(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
    ));

    // The != limb: the v4 body (main's, #1605) and the v5 body (this
    // plan's, re-based in 15.3) are two different generations.
    assert_ne!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
        "the pre-Room-rename snapshot was not re-based onto main's v5 body; \
         two constants hold identical bytes and v5 is unrecognized forever"
    );
    assert_eq!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES.len(),
        539
    );
    assert_eq!(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len(), 564);
    assert!(
        !GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES.contains("{{HOST_PLATFORM_RULES}}"),
        "the v4 body predates the placeholder"
    );
    assert!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.contains("{{HOST_PLATFORM_RULES}}"),
        "the v5 body carries the placeholder #1605 introduced"
    );
}

#[test]
fn frozen_pre_room_rename_coordinator_template_is_recognized() {
    assert!(is_known_generated_coordinator_template(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
    ));
    assert_ne!(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
        get_default_coordinator_template(),
        "the Room rename must actually change the coordinator default"
    );
    assert!(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.contains("another workgroup"));
    assert!(!get_default_coordinator_template()
        .to_lowercase()
        .contains("workgroup"));
}

/// #1614 AC7.11 at the SPEC layer. The persisted-state layer is asserted by
/// the read_sync tests, so the bump is pinned on both sides.
#[test]
fn seeded_template_versions_were_bumped() {
    let [global, coordinator, ..] = project_specs();
    assert_eq!(global.current_version, 6, "global 5 -> 6");
    assert_eq!(coordinator.current_version, 6, "coordinator 5 -> 6");
    assert_eq!(root_spec().current_version, 10, "rootAgent 9 -> 10");
}

/// #1614 AC7.1 and AC7.2. The expected values come from the frozen base and
/// are written into the plan (section 3.12 Table B), NOT read back from the
/// constants they check. That is what makes this criterion non-self-
/// referential: a later coordinated rename that moved both the constant and
/// its test would change the bytes and these hard-coded values would fail.
/// Round 1's recognizer criteria all built their expected value by calling
/// the function they then classified, so they went green under any
/// internally consistent rename.
#[test]
fn frozen_snapshots_are_byte_exact_at_d7008b34() {
    use sha2::{Digest, Sha256};

    assert_eq!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len(),
        564,
        "frozen pre-Room-rename global template must be the df494bfa v5 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.as_bytes())
        ),
        "d094106b386172e714512dbe1d18cc30a82ff2b25df467f3a1be1c328d464f77",
        "frozen global snapshot changed; every pristine installation would stop auto-updating"
    );

    assert_eq!(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len(),
        2516,
        "frozen pre-Room-rename coordinator template must be the d7008b34 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.as_bytes())
        ),
        "0b89eb38608f6272f0d8087fc7df13ecc729fda716aba972673b15b734a2198e",
        "frozen coordinator snapshot changed; every pristine installation would stop auto-updating"
    );
}

// Stage E (#1064) recognized-predecessor exhaustiveness sentinel (plan
// section 10.2 item 17, 10.6, acceptance item 31): the coordinator recognizer
// must accept the current default AND every frozen generated predecessor
// (pre-cross-workgroup v3, pre-token-minimization v2, pre-raise-hand), but not
// custom content. A mutation that recognizes only one generation fails here.
// #1748 retired the PROJECT global recognizer, so the arms that asserted its
// narrowness are gone; the standalone arms below are what remains.
#[test]
fn stage_e_all_recognized_coordinator_predecessors_are_generated_and_custom_is_not() {
    assert!(is_known_generated_coordinator_template(
        get_default_coordinator_template()
    ));
    assert!(is_known_generated_coordinator_template(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
    ));
    assert!(is_known_generated_coordinator_template(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
    ));
    assert!(is_known_generated_coordinator_template(
        OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND
    ));
    assert!(!is_known_generated_coordinator_template(
        "custom operator-authored coordinator content"
    ));

    assert!(is_known_generated_standalone_global_template(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS
    ));
    assert!(is_known_generated_standalone_global_template(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION
    ));
    assert!(is_known_generated_standalone_global_template(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
    ));
}

#[test]
fn project_specs_bump_global_to_v6_and_add_platform_specs() {
    let [global, coordinator, windows, linux, macos] = project_specs();
    assert_eq!(global.id, "global");
    assert_eq!(global.current_version, 6);
    assert_eq!(
        (global.current_content)(),
        crate::config::session_context::get_default_agent_template()
    );
    assert!(
        global.is_known_generated.is_none(),
        "#1748: the global spec is distribution-owned and names no recognizer"
    );

    assert_eq!(coordinator.id, "coordinator");
    assert_eq!(coordinator.current_version, 6);
    assert_eq!(
        (coordinator.current_content)(),
        get_default_coordinator_template()
    );
    assert!(coordinator
        .is_known_generated
        .is_some_and(|matches| matches(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE)));
    assert!(!coordinator
        .is_known_generated
        .is_some_and(|matches| matches(GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION)));

    for (spec, id, filename, default) in [
        (
            windows,
            "platform.windows",
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
        ),
        (
            linux,
            "platform.linux",
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
        ),
        (
            macos,
            "platform.macos",
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
        ),
    ] {
        assert_eq!(spec.id, id);
        assert_eq!(spec.filename, filename);
        assert_eq!(
            spec.current_version,
            if id == "platform.windows" { 2 } else { 1 }
        );
        assert_eq!((spec.current_content)(), default);
        assert!(spec
            .is_known_generated
            .is_some_and(|matches| matches(default)));
        assert!(spec.project_actionable);
        assert!(spec.suppress_unknown_without_state);
    }
    assert!(!windows.is_known_generated.is_some_and(|matches| matches(
        crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX
    )));
}
