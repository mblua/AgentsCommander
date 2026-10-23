use super::*;

fn seed_agency_skill(bytes: &[u8]) -> AgencyFixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    let skills_root = root.join(ROOT_AGENT_SKILLS_DIR);
    let skill_dir = skills_root.join("agency-agents-roles");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    let path = skill_dir.join(SKILL_MD_FILENAME);
    std::fs::write(&path, bytes).expect("seed skill file");
    AgencyFixture {
        _temp: temp,
        root,
        skills_root,
        path,
    }
}

fn agency_content() -> &'static str {
    default_root_skill("agency-agents-roles").content
}

fn migrate_agency(
    fixture: &AgencyFixture,
    legacy_snapshots: &[&str],
) -> Result<SkillMigration, String> {
    migrate_default_skill_file(
        &fixture.root,
        &fixture.skills_root,
        &fixture.path,
        agency_content(),
        legacy_snapshots,
    )
}

fn migrate_agency_with_snapshot(fixture: &AgencyFixture) -> Result<SkillMigration, String> {
    migrate_agency(fixture, &[AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX])
}

fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("read skill file")
}

/// `warn_once_for_path` inserts on both the WARN and the DEBUG branch, so membership means
/// exactly "we logged something about this path". This is how the no-WARN assertions are made
/// deterministic without installing a global logger under parallel tests.
fn warned_for_path(path: &Path) -> bool {
    SKILL_REPAIR_WARNED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(path)
}

fn stray_temp_files(skill_path: &Path) -> Vec<PathBuf> {
    let dir = skill_path.parent().expect("skill parent");
    std::fs::read_dir(dir)
        .expect("read skill dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".SKILL.md."))
        })
        .collect()
}

/// Reproduces `.agentscommander_ac2`, the one hand-repaired copy on disk, byte for byte.
///
/// Built positionally rather than with `replacen`. `"template cache."` occurs TWICE in the
/// file, on line 3 and again on line 21, so `str::replace` (which replaces all) would plant a
/// stray apostrophe in the body and produce a file that exists nowhere. Rewriting line index 2
/// cannot reach line 21 at all.
fn user_edited_agency_skill() -> String {
    let mut lines: Vec<String> = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX
        .lines()
        .map(str::to_string)
        .collect();
    let value = lines[2]
        .strip_prefix("description: ")
        .expect("snapshot line 3 is the description")
        .to_string();
    lines[2] = format!("description: '{}'", value);
    format!("{}\n", lines.join("\n"))
}

/// The test that made the enum necessary, written the only way that proves anything.
///
/// Asserting `Skipped(NoSnapshots)` against a junk fixture proves nothing, because junk also
/// yields `Skipped(UserEdit)` under an empty list. The only fixture that isolates the early
/// return is one that WOULD be repaired if the list were non-empty. Same fixture, same content,
/// same path; the snapshot list is the only difference.
#[test]
fn empty_snapshot_list_is_what_prevents_the_repair() {
    let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());

    assert_eq!(
        migrate_agency(&fixture, &[]).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::NoSnapshots)
    );
    assert_eq!(
        read_bytes(&fixture.path),
        AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes()
    );

    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Repaired
    );
    assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
}

#[test]
fn migrates_pristine_broken_agency_skill() {
    let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Repaired
    );
    assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
    assert!(stray_temp_files(&fixture.path).is_empty());
}

/// This is the case every broken install on disk is actually in, and the test that proves
/// normalization earns its keep.
#[test]
fn migrates_pristine_broken_agency_skill_with_crlf() {
    let crlf = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.replace('\n', "\r\n");
    let fixture = seed_agency_skill(crlf.as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Repaired
    );
    assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
}

#[test]
fn migrates_pristine_broken_agency_skill_with_trailing_whitespace() {
    let padded = format!("{}\n\n", AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX);
    let fixture = seed_agency_skill(padded.as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Repaired
    );
    assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
}

/// Pinned to the real file. If the fixture ever drifts away from `.agentscommander_ac2`, this
/// stops being a test about the real world and nobody would notice.
#[test]
fn preserves_user_modified_agency_skill() {
    use sha2::{Digest, Sha256};

    let edited = user_edited_agency_skill();
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(normalize_role_text(&edited).as_bytes())
        ),
        "0c01c6f5c45092191f693e5c50c4d05da48deb9111990fa648214b659214d4c5",
        "fixture must stay byte-identical to the hand-repaired copy on disk"
    );

    let fixture = seed_agency_skill(edited.as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::UserEdit)
    );
    assert_eq!(read_bytes(&fixture.path), edited.as_bytes());
}

#[test]
fn preserves_user_modified_agency_skill_with_appended_body() {
    let edited = format!("{}\nmy notes\n", AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX);
    let fixture = seed_agency_skill(edited.as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::UserEdit)
    );
    assert_eq!(read_bytes(&fixture.path), edited.as_bytes());
}

#[test]
fn preserves_already_migrated_agency_skill() {
    let fixture = seed_agency_skill(agency_content().as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::AlreadyCurrent)
    );
    assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
    assert!(stray_temp_files(&fixture.path).is_empty());
}

#[test]
fn migration_is_idempotent() {
    let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());

    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("first migration"),
        SkillMigration::Repaired
    );
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("second migration"),
        SkillMigration::Skipped(SkipReason::AlreadyCurrent)
    );
    assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
}

/// Falsifiable ONLY because the padding is whitespace: `trim` removes it, so without the
/// bounded read the padded file would normalize-equal the snapshot and be repaired. Padding
/// with junk would be skipped as a user edit either way and would prove nothing.
///
/// The size is taken from `max_migratable_len` at runtime. Hard-coding it fires on an LF build
/// and silently misses on a Windows `autocrlf` build, where the bound is 66 bytes larger.
#[test]
fn migration_skips_oversized_file() {
    let snapshots = [AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX];
    let bound = max_migratable_len(agency_content(), &snapshots);
    let target = bound as usize + 1;
    let padding = target
        .checked_sub(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.len())
        .expect("bound must exceed the snapshot length");
    assert!(padding > 0);

    let oversized = format!(
        "{}{}",
        AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX,
        " ".repeat(padding)
    );
    assert!(oversized.len() as u64 > bound);
    // Without the guard this fixture WOULD be repaired: it trims to exactly the snapshot.
    assert_eq!(
        normalize_role_text(&oversized),
        normalize_role_text(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX)
    );

    let fixture = seed_agency_skill(oversized.as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::TooLarge)
    );
    assert_eq!(read_bytes(&fixture.path), oversized.as_bytes());
}

/// The data-loss input, made executable. This is the file that `take(bound)` without the `+ 1`
/// and its rejection would have DESTROYED: a pristine snapshot, a long whitespace run, and then
/// the user's real bytes sitting past the cut.
///
/// Read to `bound + 1` and stopped there, the prefix is snapshot-plus-whitespace, which trims to
/// exactly the snapshot, matches, and gets overwritten with `content`. `MY NOTES` is gone and
/// nothing warns. The whole file, read honestly, is a user edit.
///
/// Two things this fixture has that `migration_skips_oversized_file` does not. It is **the only
/// fixture in the suite where `Take` stops strictly before EOF** (measured: every other call into
/// `read_bounded` reads a file of at most `bound + 1` bytes, so the take limit and EOF coincide),
/// which means the truncation path had no coverage at all until this test existed. And it is the
/// only place where the data-loss consequence is executable rather than prose: a real user's
/// bytes past the cut, destroyed, with the function reporting `Repaired`.
///
/// What it does **not** add is coverage of the missing `+ 1`. `migration_skips_oversized_file`
/// already catches that mutation: its fixture is exactly `bound + 1` bytes, so under `take(bound)`
/// the read returns `bound` bytes, `buf.len() > bound` can never fire, the prefix trims to the
/// snapshot, and it is `Repaired`. Both tests go red on it. The difference is the consequence:
/// the old fixture's padding is pure whitespace, so the repair destroys nothing a user would miss
/// and the test reports only a wrong classification. This one reports a destroyed file.
///
/// Sized from `max_migratable_len` at runtime. A constant fires on an LF build and silently
/// misses on a Windows `autocrlf` build, where the bound is 66 bytes larger.
#[test]
fn migration_skips_oversized_file_hiding_user_notes_past_the_cut() {
    let snapshots = [AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX];
    let bound = max_migratable_len(agency_content(), &snapshots);
    let cut = bound as usize + 1; // exactly what `read_bounded` will read

    // Enough whitespace that the cut lands strictly inside the run, so the truncated prefix is
    // snapshot + spaces and nothing else. The slack is what puts `MY NOTES` past the cut.
    let spaces = cut
        .checked_sub(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.len())
        .expect("bound must exceed the snapshot length")
        + 64;
    let notes = "MY NOTES\n";
    let hazard = format!(
        "{}{}{}",
        AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX,
        " ".repeat(spaces),
        notes
    );

    assert!(hazard.len() > cut, "the user's bytes must sit past the cut");
    assert!(hazard.is_char_boundary(cut));

    // The hazard, spelled out. Truncated at the cut, this file impersonates the snapshot.
    assert_eq!(
        normalize_role_text(&hazard[..cut]),
        normalize_role_text(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX),
        "the truncated prefix must be indistinguishable from a pristine stale default"
    );
    // Read honestly, it is nothing of the sort.
    assert_ne!(
        normalize_role_text(&hazard),
        normalize_role_text(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX)
    );
    assert!(normalize_role_text(&hazard).ends_with("MY NOTES"));

    let fixture = seed_agency_skill(hazard.as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::TooLarge)
    );
    assert_eq!(
        read_bytes(&fixture.path),
        hazard.as_bytes(),
        "the user's notes must survive, byte for byte"
    );
}

/// Classified, not an error. `Err` would mean a `log::warn!` on every root-agent context build
/// forever, for a condition that is sticky and provably unrepairable: `legacy_snapshots` are
/// `&'static str`, so no non-UTF-8 file can ever normalize-equal one.
///
/// The no-WARN half is asserted through `ensure_default_root_agent_skills_at`, not through a
/// direct call. `warn_once_for_path` lives at the caller's swallow point, so a direct call never
/// has it in its call graph and `!warned_for_path` would be near-vacuous: the `Ok` return alone
/// would carry it. Routed this way, the assertion means what its comment claims.
#[test]
fn migration_skips_non_utf8_file() {
    let invalid = [0xff_u8, 0xfe, 0x00, 0x41];
    let fixture = seed_agency_skill(&invalid);

    // The classification, asserted where the enum is visible.
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::NotUtf8)
    );

    // The absence of the warn, asserted where the warn would actually be emitted.
    ensure_default_root_agent_skills_at(&fixture.root)
        .expect("an unrepairable skill must never fail a session start");

    assert_eq!(read_bytes(&fixture.path), invalid);
    assert!(
        !warned_for_path(&fixture.path),
        "an unrepairable, sticky condition must not warn, on any build"
    );
}

/// `normalize_role_text` collapses `\r\n` only, and `str::trim` does not strip U+FEFF, which is
/// not `White_Space`. So a BOM-prefixed copy is a user edit and stays broken. Pinned rather
/// than asserted in prose.
#[test]
fn preserves_bom_prefixed_agency_skill() {
    let bom = format!("\u{FEFF}{}", AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX);
    let fixture = seed_agency_skill(bom.as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::UserEdit)
    );
    assert_eq!(read_bytes(&fixture.path), bom.as_bytes());
}

#[test]
fn preserves_lone_cr_agency_skill() {
    let old_mac = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.replace('\n', "\r");
    let fixture = seed_agency_skill(old_mac.as_bytes());
    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::UserEdit)
    );
    assert_eq!(read_bytes(&fixture.path), old_mac.as_bytes());
}

/// Differential, and the differential is the point. A read-only file that is ALREADY CURRENT
/// must classify as `AlreadyCurrent`, not `ReadOnly`: the read-only check sits below the
/// classification returns precisely so it never warns about healthy files.
#[cfg(windows)]
#[test]
fn migration_skips_read_only_destination() {
    fn set_readonly(path: &Path, readonly: bool) {
        let mut perms = std::fs::metadata(path).expect("metadata").permissions();
        perms.set_readonly(readonly);
        std::fs::set_permissions(path, perms).expect("set permissions");
    }

    // Healthy but read-only: classification wins, nothing is warned.
    let healthy = seed_agency_skill(agency_content().as_bytes());
    set_readonly(&healthy.path, true);
    assert_eq!(
        migrate_agency_with_snapshot(&healthy).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::AlreadyCurrent)
    );
    assert!(!warned_for_path(&healthy.path));
    set_readonly(&healthy.path, false);

    // Stale and read-only: `ReplaceFileW` would fail with os error 5, so we skip and warn once.
    let stale = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());
    set_readonly(&stale.path, true);
    // `SKILL_REPAIR_WARNED` is a process-global that is never cleared, so "a WARN was emitted"
    // below means the `first == true` branch ran only if this path was absent beforehand.
    // Assert that rather than inherit it from the tempdir being fresh.
    assert!(!warned_for_path(&stale.path));
    assert_eq!(
        migrate_agency_with_snapshot(&stale).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::ReadOnly)
    );
    assert_eq!(
        read_bytes(&stale.path),
        AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes()
    );
    assert!(warned_for_path(&stale.path));
    set_readonly(&stale.path, false);
}

/// The write side of the swallow. The read side is `migration_skips_non_utf8_file`; this is the
/// side that can actually strand a user mid-session. Fault-injected rather than chmod-ed: under
/// a root-uid CI container mode bits are ignored, the repair succeeds, and a chmod test goes red
/// for a reason unrelated to the code.
#[test]
fn migration_write_failure_is_not_fatal() {
    let crlf = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.replace('\n', "\r\n");
    let fixture = seed_agency_skill(crlf.as_bytes());

    let _armed = ArmedHook::new(
        &fixture.path,
        Box::new(|_| Err("injected publish failure".to_string())),
    );

    // See `migration_skips_read_only_destination`: the post-condition below means "a WARN was
    // emitted" only because this path was absent from the process-global memo beforehand.
    assert!(!warned_for_path(&fixture.path));

    ensure_default_root_agent_skills_at(&fixture.root)
        .expect("a failed repair must never fail a session start");

    assert_eq!(read_bytes(&fixture.path), crlf.as_bytes());
    assert!(warned_for_path(&fixture.path));
    assert!(stray_temp_files(&fixture.path).is_empty());
}

/// Another writer repaired the file inside our fsync window. We must observe that and publish
/// nothing.
///
/// The hook writes a byte-DIFFERENT but normalize-equal form, so "we skipped" and "we published"
/// are distinguishable. Writing `content` itself would make the two outcomes identical on disk
/// and the test would prove nothing.
#[test]
fn concurrent_repair_yields_already_current() {
    let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());
    let equivalent = format!("{}\n\n", normalize_role_text(agency_content()));
    assert_ne!(equivalent.as_bytes(), agency_content().as_bytes());

    let written = equivalent.clone();
    let _armed = ArmedHook::new(
        &fixture.path,
        Box::new(move |p: &Path| std::fs::write(p, written.as_bytes()).map_err(|e| e.to_string())),
    );

    assert_eq!(
        migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
        SkillMigration::Skipped(SkipReason::AlreadyCurrent)
    );
    assert_eq!(
        read_bytes(&fixture.path),
        equivalent.as_bytes(),
        "we must not have published over the other writer's bytes"
    );
    assert!(stray_temp_files(&fixture.path).is_empty());
}

/// THE test that proves the re-read earns its place. Delete the re-read block in
/// `atomic_write_default_skill` and this goes red: the user's bytes are overwritten with
/// `content`.
#[test]
fn destination_changed_under_us_is_not_written() {
    let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());
    let user_edit = user_edited_agency_skill();

    let written = user_edit.clone();
    let _armed = ArmedHook::new(
        &fixture.path,
        Box::new(move |p: &Path| std::fs::write(p, written.as_bytes()).map_err(|e| e.to_string())),
    );

    let result = migrate_agency_with_snapshot(&fixture);
    assert!(
        result.is_err(),
        "a destination that changed mid-repair must abort, got {:?}",
        result
    );
    assert_eq!(
        read_bytes(&fixture.path),
        user_edit.as_bytes(),
        "the user's bytes must survive"
    );
    assert!(stray_temp_files(&fixture.path).is_empty());
}

/// #2786 (C1) - no state file: an unrelated malformed field must not hide
/// `lastCodingAgent`, exactly as before the loader.
#[test]
fn read_last_coding_agent_ignores_an_unrelated_malformed_field() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        temp.path().join("config.json"),
        r#"{"tooling":{"lastCodingAgent":"claude","codingAgents":null}}"#,
    )
    .expect("seed config");
    assert_eq!(
        read_last_coding_agent(temp.path().to_str().expect("utf-8")).as_deref(),
        Some("claude")
    );
}
