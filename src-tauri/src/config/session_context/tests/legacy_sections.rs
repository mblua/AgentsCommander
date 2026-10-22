use super::*;

/// Seeds a temp `ac-root-agent` root and returns it. The directory name is not load-bearing
/// (`validate_root_agent_root_path` never inspects it, and neither does `discover_skill_index`),
/// but it matches the existing `root_agent.rs` tests.
fn temp_root_agent_dir(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let root = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root agent dir");
    root
}

/// Asserts that every shipped default skill survives the real indexer with no warning at all,
/// and that it actually reaches the rendered context. Shared by the two gates below.
fn assert_defaults_index_and_render_cleanly(root: &Path) {
    // Resolve the owner root the way production does, rather than handing `discover_skill_index`
    // a path we already know is right. `resolve_skill_owner_root` was long believed untestable
    // in-process because of `config_dir()`'s `OnceLock`; it is not. That `OnceLock` is reached by
    // `ensure_session_context_with_config` (`:27-28`), NOT by this function: it calls only
    // `is_canonical_agent_matrix_dir` and `is_root_agent_dir_name`, and neither touches it.
    //
    // For a temp `ac-root-agent` directory the first is false, so it falls through to the second
    // and canonicalizes, which is exactly the production path for a real root agent.
    //
    // This closes a real hole. `discover_skill_index(None)` returns an empty index with ZERO
    // warnings, so a regression in owner resolution would leave the root agent silently indexing
    // nothing while the `warnings.is_empty()` assertion below stayed green. The `skills.len()`
    // assertion is what catches it.
    let owner = resolve_skill_owner_root(&path_string(root), None);
    let index = discover_skill_index(owner.as_deref());

    // MUST be first. On the unfixed seed the YAML error is pushed at `:512` and the loop
    // `continue`s, so `skills.len() == 1`; a length check first would report `1 != 2` and say
    // nothing about why.
    assert!(
        index.warnings.is_empty(),
        "indexer warnings: {:?}",
        index.warnings
    );

    // Not optional: `discover_skill_index` returns early with NO warning when `skills/` is
    // absent (`:408-410`), so a silently-no-op seed would pass the assertion above.
    let expected = crate::config::root_agent::default_root_skill_dir_names();
    assert_eq!(
        index.skills.len(),
        expected.len(),
        "indexed skills: {:?}",
        index.skills.iter().map(|s| &s.name).collect::<Vec<_>>()
    );

    for skill in &index.skills {
        assert!(
            skill.metadata_warnings.is_empty(),
            "{}: {:?}",
            skill.folder_name,
            skill.metadata_warnings
        );
        let description = skill.description.as_deref().unwrap_or("");
        assert!(
            !description.trim().is_empty(),
            "{} has an empty description",
            skill.folder_name
        );
    }

    // A skill can sit in `index.skills`, warning-free, and still never reach the model:
    // `push_with_budget` drops entries past the budget and `skill_trigger_text` truncates.
    // The assertion #909 actually wants is "the shipped defaults reach the agent".
    let rendered = render_skills_section(&index);
    assert!(rendered.contains("### Available Skills"));

    for dir_name in expected {
        let skill = index
            .skills
            .iter()
            .find(|s| s.folder_name == dir_name)
            .unwrap_or_else(|| panic!("default skill `{}` missing from the index", dir_name));

        // Assert the whole `full_entry` line, not just the name. Two reasons a name-only check
        // is weaker than it looks: `render_skills_section` prints `skill.name`, the YAML `name:`
        // field, which only coincides with the directory name for today's two defaults; and the
        // budget-exhausted `minimal_entry` fallback (`:747-750`) ALSO emits "- `{name}`", so a
        // name check stays green for a skill whose metadata was dropped on the way to the model.
        // The trigger text is emitted by `full_entry` alone.
        let name = sanitize_skill_metadata_for_context(&skill.name);
        let trigger = sanitize_skill_metadata_for_context(&skill_trigger_text(skill));
        assert!(
            !trigger.trim().is_empty(),
            "{} rendered an empty trigger",
            dir_name
        );
        assert!(
            rendered.contains(&format!("- `{}` - {}", name, trigger)),
            "default skill `{}` did not reach the rendered section with its metadata intact",
            dir_name
        );
    }
}

/// THE GATE. Models a real, already-seeded install: the pre-fix snapshot on disk, written with
/// `\r\n`, which is the state every broken install is in. It exercises the **exists arm** and
/// therefore the repair.
///
/// A fresh-tempdir test cannot do this. `root_agent.rs:1159` and `:1168` asserted
/// `seeded == constant` against a fresh temp root and stayed green for eleven days while
/// eighteen installs on disk were broken. That blind spot is #909.
#[test]
fn broken_agency_skill_install_is_repaired_and_reaches_the_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp_root_agent_dir(&temp);
    let skill_dir = root.join(SKILLS_DIR_NAME).join("agency-agents-roles");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");

    let broken =
        crate::config::root_agent::agency_pre_yaml_fix_snapshot().replace('\n', "\r\n");
    std::fs::write(skill_dir.join("SKILL.md"), &broken).expect("seed the broken skill");

    crate::config::root_agent::ensure_default_root_agent_skills_at(&root)
        .expect("seeding must not fail");

    assert_defaults_index_and_render_cleanly(&root);
}

/// The constant check, not the gate. A fresh tempdir exercises only the NotFound arm, so this
/// proves `DEFAULT_ROOT_SKILLS[..].content` parses. It cannot prove a real install recovers.
#[test]
fn shipped_default_root_skills_parse_through_the_real_indexer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp_root_agent_dir(&temp);

    crate::config::root_agent::ensure_default_root_agent_skills_at(&root)
        .expect("seeding must not fail");

    assert_defaults_index_and_render_cleanly(&root);
}

/// #1005 S1 / G3 provenance pin: the frozen legacy intro must stay
/// byte-identical to the const that shipped through base commit 08897ef.
/// Expected values were captured by a one-off run of the shipped const AT
/// that commit (never from this const), per the plan's freeze-provenance
/// rule.
#[test]
fn legacy_generated_skills_section_intro_is_byte_exact() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        LEGACY_GENERATED_SKILLS_SECTION_INTRO.len(),
        560,
        "frozen legacy intro must be the 08897ef bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(LEGACY_GENERATED_SKILLS_SECTION_INTRO.as_bytes())
        ),
        "25a42fe4685b3700156331bce53351a54deca0cd53278e55a00a7dccb3def3c9",
        "frozen legacy intro changed; it must stay byte-identical to what shipped"
    );
}

/// #1005 S1 / plan 6.6: a REAL old-generation legacy rendered default whose
/// embedded skills section carries the pre-rewrite intro still classifies
/// StaleGenerated and heals on disk. The embedded section is the frozen OLD
/// intro (independently byte-pinned against 08897ef) plus the current
/// non-intro tail, which equals the old renderer's output byte-for-byte
/// because every non-intro literal in `render_skills_section` is frozen for
/// this project (G2 scope rule). Includes a Skill Discovery Warnings
/// subsection so the swap covers the warnings path.
#[test]
fn legacy_intro_skills_section_still_classifies_stale_generated_and_heals() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let old_matrix = ac_root.join("_agent_dev-rust");
    let new_matrix = ac_root.join("_agent_tech-lead");
    let old_replica = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    let new_replica = ac_root.join("wg-19-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&new_matrix).expect("create new matrix");
    std::fs::create_dir_all(&old_replica).expect("create old replica");
    std::fs::create_dir_all(&new_replica).expect("create new replica");

    // Real skills on disk for the OLD matrix: one valid (entry line), one
    // with broken frontmatter (warnings subsection).
    let valid_skill = old_matrix.join(SKILLS_DIR_NAME).join("gamma-skill");
    std::fs::create_dir_all(&valid_skill).expect("create valid skill");
    std::fs::write(
        valid_skill.join(SKILL_MD_FILENAME),
        "---\nname: gamma-skill\ndescription: Fixture skill for the legacy intro guard.\n---\n\nbody\n",
    )
    .expect("write valid skill");
    let broken_skill = old_matrix.join(SKILLS_DIR_NAME).join("delta-skill");
    std::fs::create_dir_all(&broken_skill).expect("create broken skill");
    std::fs::write(
        broken_skill.join(SKILL_MD_FILENAME),
        "---\nname: [unclosed\n---\n\nbody\n",
    )
    .expect("write broken skill");

    // The old-generation embedded section: frozen OLD intro + current tail.
    let current_render =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let tail = current_render
        .strip_prefix(GENERATED_SKILLS_SECTION_INTRO)
        .expect("render_skills_section starts with the current intro");
    assert!(
        current_render.contains("### Skill Discovery Warnings"),
        "fixture must exercise the warnings subsection"
    );
    let old_skills_section = format!("{}{}", LEGACY_GENERATED_SKILLS_SECTION_INTRO, tail);

    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills_section,
    );
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &legacy).expect("write stale generated default");

    // Direct classification: the two-sided compare recognizes the old intro.
    assert!(matches!(
        classify_legacy_rendered_default_context(
            &legacy,
            &path_string(&old_replica),
            Some(&path_string(&old_matrix)),
            &current_render,
        ),
        LegacyRenderedDefaultContext::StaleGenerated
    ));

    // End to end: resolve returns the current render and heals the on-disk
    // template to the current tokenized default.
    let rendered = resolve_agent_context(
        &path_string(&new_replica),
        Some(&path_string(&new_matrix)),
        &no_skill_section(),
        &new_replica,
        None,
        None,
    )
    .expect("resolve context");
    assert_mandatory_sections_once(&rendered);
    assert_no_raw_template_placeholders(&rendered);
    let healed = std::fs::read_to_string(&template_path).expect("read healed template");
    assert_eq!(healed, get_default_agent_template());
}

/// Negative control for the two-sided compare: one mutated byte inside the
/// embedded OLD intro means the section is no longer provably generated, so
/// the file is preserved as custom (NotLegacy), never healed.
#[test]
fn edited_legacy_intro_skills_section_is_preserved_not_healed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let old_matrix = ac_root.join("_agent_dev-rust");
    let old_replica = ac_root.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&old_matrix).expect("create old matrix");
    std::fs::create_dir_all(&old_replica).expect("create old replica");

    let current_render =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    let tail = current_render
        .strip_prefix(GENERATED_SKILLS_SECTION_INTRO)
        .expect("render_skills_section starts with the current intro");
    let edited_intro =
        LEGACY_GENERATED_SKILLS_SECTION_INTRO.replace("indexes skills", "indexes skillz");
    assert_ne!(edited_intro, LEGACY_GENERATED_SKILLS_SECTION_INTRO);
    let edited_section = format!("{}{}", edited_intro, tail);

    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &edited_section,
    );
    assert!(matches!(
        classify_legacy_rendered_default_context(
            &legacy,
            &path_string(&old_replica),
            Some(&path_string(&old_matrix)),
            &current_render,
        ),
        LegacyRenderedDefaultContext::NotLegacy
    ));

    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &legacy).expect("write edited legacy");
    let rendered = resolve_agent_context(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &no_skill_section(),
        &old_replica,
        None,
        None,
    )
    .expect("resolve context");
    assert!(!rendered.is_empty());
    let on_disk = std::fs::read_to_string(&template_path).expect("read template");
    assert_eq!(
        on_disk, legacy,
        "edited legacy file must be preserved, never healed"
    );
}

// ── #1614 step 6: the live halves and the D8f behavioural pins ──────────

/// #1614 D8b, the LIVE half of the git-scope split (the frozen half is
/// `workgroup_git_scope_frozen_half_is_byte_exact`). The `contains` pair is
/// the tripwire: a `contains` on ONE prefix is exactly what would pass a
/// half-done split. The two `assert!` CEILINGS below in
/// `git_scope_copy_is_location_correct_and_compact` are a compactness budget
/// on injected context, not a pin on this rename, and are NOT moved: 223 of
/// 236 characters and 34 of 34 words. That is ZERO word headroom, which is
/// residual R10 -- any later edit that adds a word turns that test red at
/// edit time.
#[test]
fn workgroup_git_scope_split_is_correct() {
    assert!(WORKGROUP_GIT_SCOPE.contains("room-*/"));
    assert!(WORKGROUP_GIT_SCOPE.contains("wg-*/"));
    assert_eq!(WORKGROUP_GIT_SCOPE.chars().count(), 223);
    assert_eq!(WORKGROUP_GIT_SCOPE.split_whitespace().count(), 34);
    assert_ne!(
        WORKGROUP_GIT_SCOPE, WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME,
        "the split is pointless if the two halves are the same bytes"
    );
}

/// #1614 AC7.14, the external anchor for D8f. The frozen half's length and
/// sha256 come from the plan (section 3.12 Table B), not from the constant.
/// AC7.15's two behavioural tests build their fixtures FROM this constant,
/// so it is the PAIR that is non-self-referential, not either half alone.
///
/// The trailing-newline assertion is explicit because dropping it is the
/// single most likely copy error and it breaks the compare silently: the
/// newline is inside the digest and inside the swap.
#[test]
fn skills_section_replica_line_split_is_correct() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME.len(),
        131,
        "the frozen pre-Room-rename replica line must be the d7008b34 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME.as_bytes())
        ),
        "a5c74fd65f2c2562d4c651f1c6e972684de9f2dbe1924e6153b26d4cb9c57ec9",
        "the frozen replica line changed; every pre-rename generated context stops healing"
    );

    assert!(GENERATED_SKILLS_SECTION_REPLICA_LINE.contains("room replica"));
    assert!(
        !GENERATED_SKILLS_SECTION_REPLICA_LINE
            .to_lowercase()
            .contains("workgroup"),
        "the LIVE replica line must not still say Workgroup"
    );
    assert!(GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME.ends_with('\n'));
    assert!(GENERATED_SKILLS_SECTION_REPLICA_LINE.ends_with('\n'));
    assert_ne!(
        GENERATED_SKILLS_SECTION_REPLICA_LINE,
        GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME
    );
}

/// #1614 AC7.15, the behavioural pin for D8f, modelled on #1005's twin
/// `legacy_intro_skills_section_still_classifies_stale_generated_and_heals`.
///
/// A `Context.AgentsCommander.md` whose EMBEDDED skills section carries the
/// pre-rename replica line must still classify StaleGenerated and heal after
/// the whole change. Without D8f's swap this file would flip to NotLegacy
/// permanently -- and that is every replica that has a `skills/` directory,
/// which is the common case.
#[test]
fn pre_room_rename_skills_section_still_classifies_stale_generated_and_heals() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let old_matrix = ac_root.join("_agent_dev-rust");
    let new_matrix = ac_root.join("_agent_tech-lead");
    let old_replica = ac_root.join("room-19-dev-team").join("__agent_dev-rust");
    let new_replica = ac_root.join("room-19-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&new_matrix).expect("create new matrix");
    std::fs::create_dir_all(&old_replica).expect("create old replica");
    std::fs::create_dir_all(&new_replica).expect("create new replica");

    // Real skills on disk: one valid (entry line), one with broken
    // frontmatter, so the warnings subsection is exercised too.
    let valid_skill = old_matrix.join(SKILLS_DIR_NAME).join("gamma-skill");
    std::fs::create_dir_all(&valid_skill).expect("create valid skill");
    std::fs::write(
        valid_skill.join(SKILL_MD_FILENAME),
        "---\nname: gamma-skill\ndescription: Fixture skill for the Room rename guard.\n---\n\nbody\n",
    )
    .expect("write valid skill");
    let broken_skill = old_matrix.join(SKILLS_DIR_NAME).join("delta-skill");
    std::fs::create_dir_all(&broken_skill).expect("create broken skill");
    std::fs::write(
        broken_skill.join(SKILL_MD_FILENAME),
        "---\nname: [unclosed\n---\n\nbody\n",
    )
    .expect("write broken skill");

    let current_render =
        render_skills_section(&discover_skill_index(Some(&path_string(&old_matrix))));
    assert!(
        current_render.contains("### Skill Discovery Warnings"),
        "fixture must exercise the warnings subsection"
    );
    assert!(
        current_render.contains(GENERATED_SKILLS_SECTION_REPLICA_LINE),
        "the fixture must actually render the line D8f moves; without a skills \
         directory the other match arm renders and this criterion is vacuous"
    );
    // The pre-rename generation: the current render with ONLY the replica
    // line swapped back to its frozen pre-rename bytes.
    let old_skills_section = current_render.replace(
        GENERATED_SKILLS_SECTION_REPLICA_LINE,
        GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME,
    );
    assert_ne!(old_skills_section, current_render);

    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&old_replica),
        Some(&path_string(&old_matrix)),
        &old_skills_section,
    );
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &legacy).expect("write stale generated default");

    assert!(matches!(
        classify_legacy_rendered_default_context(
            &legacy,
            &path_string(&old_replica),
            Some(&path_string(&old_matrix)),
            &current_render,
        ),
        LegacyRenderedDefaultContext::StaleGenerated
    ));

    let rendered = resolve_agent_context(
        &path_string(&new_replica),
        Some(&path_string(&new_matrix)),
        &no_skill_section(),
        &new_replica,
        None,
        None,
    )
    .expect("resolve context");
    assert_mandatory_sections_once(&rendered);
    assert_no_raw_template_placeholders(&rendered);
    let healed = std::fs::read_to_string(&template_path).expect("read healed template");
    assert_eq!(healed, get_default_agent_template());
}

/// #1614 AC7.15's negative control, modelled on
/// `edited_legacy_intro_skills_section_is_preserved_not_healed`. One mutated
/// byte inside the frozen pre-rename line means the section is no longer
/// provably generated, so the file classifies NotLegacy and is preserved
/// byte for byte, never healed. Without this the swap could be made
/// vacuously true by a substring match and nothing would notice.
#[test]
fn edited_pre_room_rename_skills_line_is_preserved_not_healed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("room-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).expect("create replica");
    let skill = matrix.join(SKILLS_DIR_NAME).join("gamma-skill");
    std::fs::create_dir_all(&skill).expect("create skill");
    std::fs::write(
        skill.join(SKILL_MD_FILENAME),
        "---\nname: gamma-skill\ndescription: Fixture skill for the Room rename guard.\n---\n\nbody\n",
    )
    .expect("write skill");

    let current_render =
        render_skills_section(&discover_skill_index(Some(&path_string(&matrix))));
    // ONE mutated byte inside the frozen line: "skills/..." -> "skills/.._"
    let mutated = GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME
        .replace("resolve skills/...", "resolve skills/..!");
    assert_ne!(
        mutated,
        GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME
    );
    let edited_section =
        current_render.replace(GENERATED_SKILLS_SECTION_REPLICA_LINE, &mutated);
    assert_ne!(edited_section, current_render);

    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&replica),
        Some(&path_string(&matrix)),
        &edited_section,
    );
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &legacy).expect("write edited legacy");

    assert!(matches!(
        classify_legacy_rendered_default_context(
            &legacy,
            &path_string(&replica),
            Some(&path_string(&matrix)),
            &current_render,
        ),
        LegacyRenderedDefaultContext::NotLegacy
    ));

    let on_disk = std::fs::read_to_string(&template_path).expect("read template");
    assert_eq!(
        on_disk, legacy,
        "an edited legacy file must be preserved, never healed"
    );
}

/// #1614 section 7 item 10, the OTHER outcome. A file whose bytes are
/// `legacy_rendered_default_context_for_compat` over the same inputs the
/// classifier is given returns `Current`, not `StaleGenerated`: there is
/// nothing stale about it, so it is returned as-is and left on disk
/// unmodified. Round 2 asserted StaleGenerated over exactly this fixture,
/// which the classifier cannot produce for it.
#[test]
fn current_generation_legacy_context_classifies_current() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("room-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).expect("create replica");
    std::fs::create_dir_all(&matrix).expect("create matrix");

    let skills = render_skills_section(&discover_skill_index(Some(&path_string(&matrix))));
    let legacy = legacy_rendered_default_context_for_compat(
        &path_string(&replica),
        Some(&path_string(&matrix)),
        &skills,
    );
    let template_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&template_path, &legacy).expect("write current-generation legacy");

    assert!(matches!(
        classify_legacy_rendered_default_context(
            &legacy,
            &path_string(&replica),
            Some(&path_string(&matrix)),
            &skills,
        ),
        LegacyRenderedDefaultContext::Current
    ));
    assert_eq!(
        std::fs::read_to_string(&template_path).expect("read template"),
        legacy,
        "a Current-generation legacy file is returned as-is and never rewritten"
    );
}

/// #1614 AC7.13. The rendered LIVE context for a Room replica says Room.
/// The matrix root must have a real `skills/` directory, or
/// `render_skills_section` takes the no-skills arm and the criterion passes
/// without ever exercising the line D8f moves.
#[test]
fn default_context_for_a_room_replica_says_room() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("room-1-t").join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).expect("create replica");
    let skill = matrix.join(SKILLS_DIR_NAME).join("gamma-skill");
    std::fs::create_dir_all(&skill).expect("create skill");
    std::fs::write(
        skill.join(SKILL_MD_FILENAME),
        "---\nname: gamma-skill\ndescription: Fixture skill.\n---\n\nbody\n",
    )
    .expect("write skill");

    let skills = render_skills_section(&discover_skill_index(Some(&path_string(&matrix))));
    assert!(
        skills.contains(GENERATED_SKILLS_SECTION_REPLICA_LINE),
        "the fixture must render the replica line, or this criterion is vacuous"
    );

    let rendered = resolve_agent_context(
        &path_string(&replica),
        Some(&path_string(&matrix)),
        &skills,
        &replica,
        None,
        None,
    )
    .expect("resolve context");

    assert!(rendered.contains("room-<N>-*"), "{rendered}");
    assert!(rendered.contains("<roomN>"), "{rendered}");
    let offenders: Vec<&str> = rendered
        .lines()
        .filter(|l| l.to_lowercase().contains("workgroup"))
        .collect();
    assert!(
        offenders.is_empty(),
        "the live context for a Room replica must not say Workgroup anywhere: {offenders:#?}"
    );
}

/// #1614 AC7.4, the frozen half of the D8b split. `WORKGROUP_GIT_SCOPE` is
/// read by the LIVE renderer and by the FROZEN legacy recognizer. Rule R
/// forces the first to change; Rule P3 forces the second not to. The live
/// half's assertions live in `workgroup_git_scope_split_is_correct`.
#[test]
fn workgroup_git_scope_frozen_half_is_byte_exact() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME.len(),
        220,
        "the frozen git-scope half must be the d7008b34 bytes"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME.as_bytes())
        ),
        "a386b52da8246826689215a8f07abf3cb58d01ebcc18afc530730157aa12566d",
        "the frozen git-scope half changed; every pre-#1369 context file stops self-healing"
    );
}

/// #1614 AC7.8, the source-side freeze, and the criterion round 1 most
/// needed and did not have: a byte comparison against a value taken from the
/// frozen base and written into the plan, so it cannot move with the code.
/// The rendered-output digests (AC7.7) prove nothing in the function's
/// closure moved; this proves no byte of the function itself moved.
///
/// The ONE difference this criterion tolerates is D8b's identifier switch at
/// the git-scope read, which changes an identifier and not a byte of prose.
#[test]
fn legacy_rendered_default_context_for_generation_source_is_frozen() {
    use sha2::{Digest, Sha256};

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("config")
        .join("session_context.rs");
    let source = std::fs::read_to_string(&path).expect("read session_context.rs");
    // The plan's digest is over the git blob, and .gitattributes pins *.rs
    // to `eol=lf`, so normalize before hashing rather than depending on how
    // this particular working tree was checked out.
    let source = source.replace("\r\n", "\n");

    let head = "fn legacy_rendered_default_context_for_generation(";
    let start = source
        .lines()
        .position(|line| line == head)
        .expect("frozen function head not found");
    let end = source
        .lines()
        .skip(start)
        .position(|line| line == "}")
        .expect("frozen function terminator not found")
        + start;

    let body: String = source
        .lines()
        .skip(start)
        .take(end - start + 1)
        .map(|line| format!("{line}\n"))
        .collect();
    let normalized = body.replace(
        "WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME",
        "WORKGROUP_GIT_SCOPE",
    );

    assert_eq!(
        normalized.len(),
        15027,
        "the frozen legacy reconstruction changed size; it reconstructs a user's pre-#1369 \
         context file for byte comparison and must not move"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized.as_bytes())),
        "940fa35733c78cdf513391e5aed64438afd50fe6472a26e4d317270e5ee716c2",
        "a byte of legacy_rendered_default_context_for_generation moved; every pre-#1369 \
         Context.AgentsCommander.md falls from StaleGenerated to NotLegacy, never self-heals \
         again, and keeps the pre-#1369 write restrictions permanently"
    );
}

/// #1614 (plan section 3.12 Table C, section 12 step 0): the two legacy
/// reconstructions the Room rename must not move, pinned by values captured
/// with a one-off run of the shipped functions AT base commit
/// d7008b34e155a8bd6481be5feecfc7d96575328f, read out of a deliberately
/// failing assertion and never read back from the functions themselves.
/// That is what makes this criterion non-self-referential: a later
/// coordinated rename that moved both the functions and this test would
/// change their output and these hard-coded values would fail.
///
/// The inputs are fixed synthetic constants rather than a `tempfile` path,
/// because a digest taken over a real temporary directory is not
/// reproducible: the path varies per run and is interpolated into the
/// output. Neither directory needs to exist. `workgroup_root` is a pure
/// path walk with no filesystem touch (`phone/messaging.rs`), so the agent
/// root below reaches the Workgroup messaging mode regardless, and
/// `root_agency_cache_guidance` returns "" for a non-root root.
///
/// A reviewer checks this by checking out d7008b34, applying only this test
/// and running it: both values must be green on the base tree.
#[test]
fn legacy_rendered_default_context_is_frozen() {
    use sha2::{Digest, Sha256};

    const AGENT_ROOT: &str = "C:/fake/.ac/wg-7-dev-team/__agent_architect";
    const MATRIX_ROOT: &str = "C:/fake/.ac/_agent_architect";

    // C1: the current-generation reconstruction, the `Current` arm's input.
    let c1 = legacy_rendered_default_context_for_compat(AGENT_ROOT, Some(MATRIX_ROOT), "");
    assert_eq!(
        c1.len(),
        8718,
        "C1 `legacy_rendered_default_context_for_compat` moved; the frozen \
         legacy recognizer no longer reproduces the d7008b34 bytes"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(c1.as_bytes())),
        "daf07cbc24e46988747385f1d622c0b3309d29fa1fda77964afb2892ef85275c",
        "C1 `legacy_rendered_default_context_for_compat` moved; the frozen \
         legacy recognizer no longer reproduces the d7008b34 bytes"
    );

    // C2: the pre-#1072 reconstruction, the candidate that actually carries
    // the `StaleGenerated` outcome.
    let c2 =
        pre_1072_legacy_rendered_default_context_for_compat(AGENT_ROOT, Some(MATRIX_ROOT), "");
    assert_eq!(
        c2.len(),
        9096,
        "C2 `pre_1072_legacy_rendered_default_context_for_compat` moved; \
         every pre-#1072 context file stops self-healing, permanently"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(c2.as_bytes())),
        "24b2c24ddef33c307fbb64734eee05c5baf60c2b6616e21ef1e56fc0f33999ef",
        "C2 `pre_1072_legacy_rendered_default_context_for_compat` moved; \
         every pre-#1072 context file stops self-healing, permanently"
    );
}
