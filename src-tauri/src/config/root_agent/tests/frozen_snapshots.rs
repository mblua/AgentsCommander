use super::*;

#[test]
fn issue_2832_root_recognition_is_exact_while_role_normalization_is_unchanged() {
    for source in frozen_root_context_snapshots()
        .into_iter()
        .chain([default_root_context_template()])
    {
        assert!(is_known_generated_root_context_template(source));
        for edit in [
            format!(" {source}"),
            format!("{source} "),
            format!("{source}\n"),
            source.replace('\n', "\r\n"),
        ] {
            assert!(!is_known_generated_root_context_template(&edit));
        }
    }
    assert_eq!(
        normalize_role_text(&format!(" {} ", MINIMAL_ROOT_ROLE_MD.replace('\n', "\r\n"))),
        normalize_role_text(MINIMAL_ROOT_ROLE_MD)
    );
    assert!(!is_known_generated_root_context_template(""));
    assert!(!is_known_generated_root_context_template("\n"));
}

#[test]
fn issue_2832_default_contains_literal_policy_and_frozen_v8_supplement_body() {
    let source = default_root_context_template();
    let body = ROOT_ROLE_MD
        .splitn(3, "---")
        .nth(2)
        .unwrap()
        .trim_start_matches('\n');
    // Markdown checkout line endings may differ by host. Check the complete
    // literal supplement by lines; production recognition remains byte-exact.
    let source_lines: Vec<_> = source.lines().collect();
    let body_lines: Vec<_> = body.lines().collect();
    assert!(source_lines
        .windows(body_lines.len())
        .any(|lines| lines == body_lines));
    for heading in [
        "## GOLDEN RULE",
        "## Root Agent Authority",
        "## Skills",
        "## Inter-Agent Messaging",
        "## Privileged PTY Input",
        "## Self-Maintenance",
        "## Host Platform Rules",
    ] {
        assert!(source.contains(heading));
    }
    for coarse in [
        "{{WRITE_RESTRICTIONS}}",
        "{{ROOT_AUTHORITY}}",
        "{{CLI_CONTEXT}}",
        "{{INTER_AGENT_MESSAGING}}",
        "{{HOST_PLATFORM_RULES}}",
        "{{SELF_MAINTENANCE}}",
    ] {
        assert!(!source.contains(coarse));
    }
    assert!(!source.starts_with("---"));
}

#[test]
#[cfg(windows)]
fn display_path_converts_verbatim_unc() {
    assert_eq!(
        display_path(Path::new(r"\\?\UNC\server\share\ac-root-agent")),
        r"\\server\share\ac-root-agent"
    );
}

#[test]
fn ensure_root_agent_dir_at_creates_layout_role_and_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);

    ensure_root_agent_dir_at(&root).expect("ensure root");

    for sub in ["memory", "plans", "skills", "inbox", "outbox", "messaging"] {
        assert!(root.join(sub).is_dir(), "missing {}", sub);
    }
    assert!(root.join("Role.md").is_file());
    assert!(ROOT_ROLE_MD.contains("You are the AgentsCommander Root Agent"));
    assert!(!ROOT_ROLE_MD.contains("verified workgroup coordinator replicas only"));
    assert!(!ROOT_ROLE_MD.contains("list-peers-lean"));
    assert!(!ROOT_ROLE_MD.contains("AGENTSCOMMANDER_TOKEN"));
    // #648: the Agency guidance moved into skills/agency-agents-roles; the
    // role now carries only a trigger pointer, not the inline CLI block.
    assert!(!ROOT_ROLE_MD.contains("agency-templates update"));
    assert!(!ROOT_ROLE_MD.contains("Do not invent Agency template IDs"));
    assert!(ROOT_ROLE_MD.contains("skills/agency-agents-roles/SKILL.md"));
    assert!(ROOT_ROLE_MD.contains("create-agent-matrix"));
    assert!(ROOT_ROLE_MD.contains("team create"));
    assert!(ROOT_ROLE_MD.contains("room add"));
    assert!(ROOT_ROLE_MD.contains("Agents must exist before team creation"));
    assert!(!ROOT_ROLE_MD.contains("room add --coordinator"));
    assert!(ROOT_ROLE_MD.contains("role-skill-boundary-audit"));
    assert!(ROOT_ROLE_MD.contains("`Role.md` files"));
    assert!(ROOT_ROLE_MD.contains("skills"));
    assert!(ROOT_ROLE_MD.contains("Agent Matrix structure"));
    let skill_path = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("role-skill-boundary-audit")
        .join(SKILL_MD_FILENAME);
    assert!(skill_path.is_file());
    assert_eq!(
        std::fs::read_to_string(&skill_path).expect("read default skill"),
        DEFAULT_ROOT_SKILLS[0].content
    );
    let agency_skill_path = root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("agency-agents-roles")
        .join(SKILL_MD_FILENAME);
    assert!(agency_skill_path.is_file());
    assert_eq!(
        std::fs::read_to_string(&agency_skill_path).expect("read agency skill"),
        DEFAULT_ROOT_SKILLS[1].content
    );
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let global_template_path = temp
        .path()
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let coordinator_template_path = temp
        .path()
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    // #979: Root provisioning seeds ONLY the Root supplement. It never creates a
    // standalone global or coordinator template; the one editable global is
    // project-scoped and lives in `<project>/.ac`.
    assert!(template_path.is_file());
    assert!(
        !global_template_path.exists(),
        "Root provisioning must not create a standalone global context template"
    );
    assert!(
        !coordinator_template_path.exists(),
        "Root provisioning must not create a coordinator context template"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD
    );
    assert_eq!(
        std::fs::read_to_string(template_path).expect("read template"),
        default_root_context_template()
    );
    let config_raw = std::fs::read_to_string(root.join("config.json")).expect("read config");
    let config: Value = serde_json::from_str(&config_raw).expect("parse config");
    assert_eq!(config["tooling"], serde_json::json!({}));
    assert_eq!(
        config["context"],
        serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)
    );
    assert!(
        !config_raw.contains(crate::config::session_context::CONTEXT_TOKEN_GLOBAL),
        "the canonical Root config must carry no global sentinel"
    );
}

#[test]
fn ensure_root_agent_dir_at_preserves_existing_custom_template_and_seeds_minimal_role() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    let global_template_path = temp
        .path()
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let coordinator_template_path = temp
        .path()
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    let custom_template = "# Custom Root Template\n\nUse this exact seed.\n";
    let custom_global = "# Custom Global Template\n\nKeep global.\n";
    let custom_coordinator = "# Custom Coordinator Template\n\nKeep coordinator.\n";
    std::fs::write(&template_path, custom_template).expect("write template");
    std::fs::write(&global_template_path, custom_global).expect("write global template");
    std::fs::write(&coordinator_template_path, custom_coordinator)
        .expect("write coordinator template");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD
    );
    assert_eq!(
        std::fs::read_to_string(template_path).expect("read template"),
        custom_template
    );
    // #979: an unused custom coordinator file is outside this issue and is left
    // exactly as it is.
    assert_eq!(
        std::fs::read_to_string(coordinator_template_path).expect("read coordinator template"),
        custom_coordinator
    );
    // ...while the standalone global is retired: the ACTIVE name is gone, and the
    // custom bytes survive in exactly one inert timestamped backup.
    assert!(
        !global_template_path.exists(),
        "the active standalone global name must be retired"
    );
    let backups = retired_global_backups(temp.path());
    assert_eq!(backups.len(), 1, "expected exactly one inert backup");
    assert_eq!(
        std::fs::read_to_string(&backups[0]).expect("read retired backup"),
        custom_global,
        "custom global bytes must survive byte-for-byte"
    );
}

/// #979: the inert `Context.AgentsCommander.md.retired-<ts>[.<n>].bak` shapes.
fn retired_global_backups(config_dir: &Path) -> Vec<PathBuf> {
    let prefix = format!(
        "{}.retired-",
        crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME
    );
    let mut found: Vec<PathBuf> = std::fs::read_dir(config_dir)
        .expect("read config dir")
        .filter_map(|entry| {
            let path = entry.expect("dir entry").path();
            let name = path.file_name()?.to_str()?.to_string();
            (name.starts_with(&prefix) && name.ends_with(".bak")).then_some(path)
        })
        .collect();
    found.sort();
    found
}

#[test]
fn ensure_root_agent_dir_at_still_provisions_when_retirement_fails() {
    // #979 G2: retirement is BEST-EFFORT. `ensure_root_agent_dir` is fatal at two
    // of its four callers (commands/session.rs:2741 and :3276), so a retirement
    // error must never make the Root Agent uncreatable or unrestartable. A
    // directory at the live global path is the portable way to force that error.
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let global_template_path = temp
        .path()
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::create_dir_all(&global_template_path).expect("create dir at the live global path");
    std::fs::write(global_template_path.join("inner.md"), "KEEP_ME\n").expect("write inner");

    ensure_root_agent_dir_at(&root).expect("provisioning must still succeed");

    for sub in ["memory", "plans", "skills", "inbox", "outbox", "messaging"] {
        assert!(root.join(sub).is_dir(), "missing {}", sub);
    }
    assert!(root.join("Role.md").is_file());
    assert!(root.join("config.json").is_file());
    assert!(root
        .join(ROOT_AGENT_SKILLS_DIR)
        .join("role-skill-boundary-audit")
        .join(SKILL_MD_FILENAME)
        .is_file());
    assert!(temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)
        .is_file());

    // The offending entry is untouched: never followed, moved, or deleted.
    assert!(global_template_path.is_dir());
    assert_eq!(
        std::fs::read_to_string(global_template_path.join("inner.md")).expect("read inner"),
        "KEEP_ME\n"
    );
    assert!(retired_global_backups(temp.path()).is_empty());
}

/// The frozen v8 supplement retains the #1370 vocabulary contract. The v9
/// resource also includes filesystem policy that describes workspace roots.
#[test]
fn frozen_v8_root_supplement_has_no_workspace_prose() {
    assert!(
        !ROOT_ROLE_MD.to_lowercase().contains("workspace"),
        "the frozen v8 supplement must not use the deprecated word"
    );
}

/// #1370 C7 / AC-5: the frozen v5 root snapshot must stay byte-identical to
/// what the #1005 S5..cfde2968 builds shipped. Expected values captured by a
/// one-off run of `default_root_context_template()` AT base commit cfde2968,
/// never from this const.
#[test]
fn root_context_before_workspace_prose_snapshot_is_byte_exact() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD.len(),
        2469,
        "frozen v5 root snapshot must be the cfde2968 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD.as_bytes())
        ),
        "dab3ed05eeb1927542ab5e35e9828e8992f0e7c9537ffdfea928fe39e6079dfb",
        "frozen v5 root snapshot changed; it must stay byte-identical to what shipped"
    );
    assert_ne!(
        ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD,
        default_root_context_template(),
        "the #1370 wording change must actually change the template or the freeze is pointless"
    );
}

/// #1614 AC7.9 and AC7.10. ONE fixture (the frozen pre-Room-rename bytes)
/// drives BOTH root paths, modelled on
/// `frozen_v5_root_context_is_recognized_and_migrated_on_both_paths`, whose
/// doc comment says a list edited in only one place cannot pass silently.
/// The recognizer list and `migrate_root_role_file`'s independent pristine
/// list are separate, and D8a wires the snapshot into both.
#[test]
fn frozen_pre_room_rename_root_context_migrates_on_the_role_path_too() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("Role.md"), ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD)
        .expect("write pristine pre-Room-rename role");
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD)
        .expect("write pristine pre-Room-rename template");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD,
        "a pristine pre-Room-rename Role.md must reduce to the minimal role; this is \
         migrate_root_role_file's OWN list, not old_generated"
    );
    assert_eq!(
        std::fs::read_to_string(&template_path).expect("read template"),
        default_root_context_template(),
        "a pristine pre-Room-rename Context.root-agent.md must auto-upgrade (recognizer list)"
    );
    assert!(is_known_generated_root_context_template(
        ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD
    ));
}

/// #1614 AC7.9 for the root spec.
#[test]
fn frozen_pre_room_rename_root_context_is_recognized() {
    assert!(is_known_generated_root_context_template(
        ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD
    ));
    assert_ne!(
        ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD, ROOT_ROLE_MD,
        "the Room rename must actually change ROOT_ROLE_MD or the freeze is pointless"
    );
    assert!(ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD.contains("workgroup add"));
    assert!(!ROOT_ROLE_MD.to_lowercase().contains("workgroup"));
}

/// #1614 AC7.3 and AC7.5. Expected values taken from the frozen base and
/// written into the plan (section 3.12 Table B), never read back from the
/// constants they check.
#[test]
fn frozen_snapshots_are_byte_exact_at_d7008b34() {
    use sha2::{Digest, Sha256};

    assert_eq!(
        ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD.len(),
        2467,
        "frozen pre-Room-rename root context must be the d7008b34 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD.as_bytes())
        ),
        "7f82f28c70221c8476bb957f5978433173f60e388a9f18db729e5c2bf014c52d",
        "frozen root snapshot changed; every pristine pre-rename root Role.md would be orphaned"
    );

    assert_eq!(
        ROOT_COORDINATION_MESSAGING_PARAGRAPH_BEFORE_ROOM_RENAME.len(),
        897,
        "frozen pre-Room-rename coordination paragraph must be the d7008b34 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(ROOT_COORDINATION_MESSAGING_PARAGRAPH_BEFORE_ROOM_RENAME.as_bytes())
        ),
        "fc2164a2a56957e481debca460f9df3cc681a634edda58f5270939c85668f207",
        "the frozen half of the D8c split changed; old_generated[1] is no longer the shipped bytes"
    );
}

/// #1614 AC7.6 / D8d. `OLD_DEFERRED_MESSAGING_PARAGRAPH` is NOT compared for
/// equality; it is searched for INSIDE the user's live Role.md, and on a hit
/// the migration substitutes the coordination paragraph for it. Editing it
/// would silently disable that migration for every installation still on the
/// deferred-messaging generation: the `contains` stops matching, the branch
/// never fires, and nothing reports it. It has no live twin, so there is
/// nothing to split and it takes NO edit -- which is exactly why it gets a
/// byte pin rather than a code comment.
#[test]
fn old_deferred_messaging_paragraph_is_frozen() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        OLD_DEFERRED_MESSAGING_PARAGRAPH.len(),
        293,
        "OLD_DEFERRED_MESSAGING_PARAGRAPH must stay the d7008b34 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(OLD_DEFERRED_MESSAGING_PARAGRAPH.as_bytes())
        ),
        "6e12e68e51c3c6df2386728dfd0ed98bfe06a8a0c3f6383bfaf8fd4463c7a463",
        "editing this silently and permanently disables the deferred-messaging migration"
    );
}

/// #1005 S5 / G3: the frozen v4 root snapshot must stay byte-identical to what
/// the #698..409b7f90 builds shipped. Expected values captured by a one-off run
/// of `default_root_context_template()` AT base commit 409b7f90, never from
/// this const.
#[test]
fn root_context_before_token_minimization_snapshot_is_byte_exact() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD.len(),
        2516,
        "frozen v4 root snapshot must be the 409b7f90 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD.as_bytes())
        ),
        "f100cfcf4df40c0ce1e81b6bebc89f7eca79eb1d4cfef9298e8abd3da53c1e73",
        "frozen v4 root snapshot changed; it must stay byte-identical to what shipped"
    );
}

/// #1005 S5 failing-first proof for ALL of: the v5 rewrite (assert_ne), the
/// recognizer list (E5.4 path 1), the `migrate_root_role` pristine list
/// (E5.4 path 2), and the root_spec version bump. ONE fixture (the frozen v4
/// bytes) drives both migration paths so a missed list cannot pass silently:
/// a pristine v4 Role.md must reduce to MINIMAL, and a pristine v4
/// Context.root-agent.md must auto-upgrade to the current default.
#[test]
fn frozen_v4_root_context_is_recognized_and_migrated_on_both_paths() {
    assert_ne!(
        ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD,
        default_root_context_template(),
        "v5 rewrite must actually change the template or the freeze is pointless"
    );
    assert!(is_known_generated_root_context_template(
        ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD
    ));

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(
        root.join("Role.md"),
        ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD,
    )
    .expect("write pristine v4 role");
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD)
        .expect("write pristine v4 template");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD,
        "pristine v4 Role.md must reduce to the minimal role (migrate_root_role list)"
    );
    assert_eq!(
        std::fs::read_to_string(&template_path).expect("read template"),
        default_root_context_template(),
        "pristine v4 Context.root-agent.md must auto-upgrade (recognizer list)"
    );
    let state = std::fs::read_to_string(
        temp.path()
            .join(crate::config::seeded_context_templates::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
    )
    .expect("read seeded state");
    let parsed: Value = serde_json::from_str(&state).expect("parse seeded state");
    assert_eq!(
        parsed["templates"]["rootAgent"]["currentVersion"], 10,
        "root_spec current_version must be bumped to 10 by #3024"
    );
}

/// #1571 T3: the frozen pre-rename Root snapshot must stay byte-identical to
/// what shipped at base commit ecc6527b. Expected values captured by a one-off
/// run of the shipped constant AT ecc6527b (plan 3.6), never from this const.
#[test]
fn root_context_pre_orchestrator_rename_snapshot_is_byte_exact() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD.len(),
        2464,
        "frozen pre-rename root snapshot must be the ecc6527b bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD.as_bytes())
        ),
        "e244249ccd2fa832918d1f830939ca4dae37a3594461334fa6d7004ba430e74f",
        "frozen pre-rename root snapshot changed; it must stay byte-identical to what shipped"
    );
}

/// #3024: the frozen v9 Root snapshot must stay byte-identical to what
/// shipped at base commit f6e5d08. Expected values captured externally from
/// `git show f6e5d08:...Context.root-agent.md`, never from this const.
#[test]
fn root_context_before_reporting_scope_snapshot_is_byte_exact() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        ROOT_CONTEXT_BEFORE_REPORTING_SCOPE_MD.len(),
        19240,
        "frozen v9 root snapshot must be the f6e5d08 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(ROOT_CONTEXT_BEFORE_REPORTING_SCOPE_MD.as_bytes())
        ),
        "3b323448ab8ed018b9111aae969e0bf6a46186772b469e6cf8958390d824a6d9",
        "frozen v9 root snapshot changed; it must stay byte-identical to what shipped"
    );
}

/// #1370 C7 / AC-6: failing-first proof for the recognizer list, the
/// `migrate_root_role` pristine list and the root_spec version bump. ONE
/// fixture (the frozen v5 bytes) drives both migration paths so a list edited
/// in only one place cannot pass silently: a pristine v5 Role.md must reduce
/// to MINIMAL, and a pristine v5 Context.root-agent.md must auto-upgrade to
/// the current default. Without it, skipping B2/B3 fails silently and
/// permanently.
#[test]
fn frozen_v5_root_context_is_recognized_and_migrated_on_both_paths() {
    assert!(is_known_generated_root_context_template(
        ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD
    ));

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("Role.md"), ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD)
        .expect("write pristine v5 role");
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD)
        .expect("write pristine v5 template");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD,
        "pristine v5 Role.md must reduce to the minimal role (migrate_root_role list)"
    );
    assert_eq!(
        std::fs::read_to_string(&template_path).expect("read template"),
        default_root_context_template(),
        "pristine v5 Context.root-agent.md must auto-upgrade (recognizer list)"
    );
    let state = std::fs::read_to_string(
        temp.path()
            .join(crate::config::seeded_context_templates::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
    )
    .expect("read seeded state");
    let parsed: Value = serde_json::from_str(&state).expect("parse seeded state");
    assert_eq!(
        parsed["templates"]["rootAgent"]["currentVersion"], 10,
        "root_spec current_version must be bumped to 10 by #3024"
    );
}

/// #1571 T4: the same both-paths proof for the frozen pre-rename Root bytes.
/// ONE fixture drives `migrate_root_role` and the recognizer array, so wiring
/// one list and forgetting the other cannot pass silently.
///
/// Two deliberate deviations from the `frozen_v5_...` model above. The direct
/// recognizer assertion is LAST rather than first: with either new entry
/// deleted, a first-position direct assert panics before
/// `ensure_root_agent_dir_at` runs, so a mutation probe would prove only that a
/// predicate whose one matching entry was just deleted returns false, not that
/// the migration path consumes it. And there is no `currentVersion` assertion:
/// the root bump is a later step, so a `== 7` assertion here would make this
/// step's gate unreachable by construction. The bump is pinned twice elsewhere.
///
/// This test can only discriminate after the templates are rewritten: until
/// then the frozen constant is byte-identical to `ROOT_ROLE_MD` and the
/// pre-existing `ROOT_ROLE_MD` entries satisfy it on their own.
#[test]
fn frozen_v6_root_context_is_recognized_and_migrated_on_both_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(
        root.join("Role.md"),
        ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD,
    )
    .expect("write pristine pre-rename role");
    let template_path = temp
        .path()
        .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD)
        .expect("write pristine pre-rename template");

    ensure_root_agent_dir_at(&root).expect("ensure root");

    assert_eq!(
        std::fs::read_to_string(root.join("Role.md")).expect("read role"),
        MINIMAL_ROOT_ROLE_MD,
        "pristine pre-rename Role.md must reduce to the minimal role (migrate_root_role list)"
    );
    assert_eq!(
        std::fs::read_to_string(&template_path).expect("read template"),
        default_root_context_template(),
        "pristine pre-rename Context.root-agent.md must auto-upgrade (recognizer list)"
    );

    assert!(is_known_generated_root_context_template(
        ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD
    ));
}

#[test]
fn ensure_root_agent_dir_at_migrates_old_root_template_defaults() {
    for old_default in [
        OLD_ROOT_ROLE_MD.to_string(),
        OLD_ROOT_CONTEXT_WITH_COORDINATION_MD.to_string(),
        ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD.to_string(),
        ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD.to_string(),
        ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD.to_string(),
        ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD.to_string(),
    ] {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(&template_path, old_default).expect("write old template");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(template_path).expect("read template"),
            default_root_context_template()
        );
    }
}
