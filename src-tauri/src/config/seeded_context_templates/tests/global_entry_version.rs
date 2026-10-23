use super::*;

/// #1748 new test 6. The other half of this contract lives in
/// `config::local_overlay`, whose PRIVATE markdown local suffix must stay
/// `.local.md`. This pins the VALUE only; it cannot detect the forbidden
/// module arc, because it never calls into the overlay. AC-01-8 detects that.
#[test]
fn local_override_filename_is_the_overlay_sibling() {
    assert_eq!(
        local_override_filename(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        "Context.AgentsCommander.local.md"
    );
    // `strip_suffix`, not `trim_end_matches`: the latter strips a REPEATED
    // suffix and would turn `a.md.md` into `a`.
    assert_eq!(local_override_filename("a.md.md"), "a.md.local.md");
}

/// #1748 new test 7: the whole content of D2, and the replacement carrier for
/// the read-path tests that would otherwise have survived vacuously. One
/// fixture, driven through FOUR entry points; the flipped input is the entry
/// point throughout. Arms (a), (b) and (c) share one tree, and (c)'s
/// `repair: true` call is the LAST call on it because that call replaces the
/// bytes and writes a backup; arm (d) runs on a second tree built from the
/// same fixture, so "one `*.bak` appears" is a statement about the scan alone.
#[test]
fn the_read_path_no_longer_repairs_the_global_but_the_scan_does() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(&path, GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES)
        .expect("write pristine v4 global");

    // (a) the render-time read path.
    assert!(
        sync_for_read_at(
            &ac_root,
            GLOBAL_CONTEXT_TEMPLATE_FILENAME,
            fixed_publication_time(),
        )
        .is_empty(),
        "the read path cannot report a replacement, so it must not make one"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read global"),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
        "the read path must leave the bytes exactly as they were"
    );
    assert!(backup_files(&ac_root).is_empty());

    // (b) project registration, the OTHER `repair: false` caller that iterates
    // `project_specs()`. The `Ok(())` is what makes this arm gradeable: with
    // this caller's `repair` flipped to `true`, `project_dir` is `None`, so
    // control reaches the project-dir guard, which sits BEFORE `create_backup`
    // and returns `Err` having written nothing. Both filesystem assertions
    // below hold in either state. This arm therefore proves "this call site
    // does not pass `repair: true`", NOT "a repair on this path would leave
    // the bytes alone" -- the latter is unreachable here.
    assert_eq!(
        ensure_project_context_templates(&ac_root),
        Ok(()),
        "project registration must not attempt a repair it cannot report"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read global after registration"),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
        "project registration must leave the bytes exactly as they were"
    );
    assert!(backup_files(&ac_root).is_empty());

    // (c) `sync_one_template` directly, the only pair in this phase that can
    // distinguish the new skip reason from a silent fall-through: unchanged
    // bytes and no publication hold equally for `AmbiguousWithoutState`,
    // `CreationDisabled` or any other quiet return.
    let spec = project_spec_by_filename(GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .expect("the global project spec");
    let mut clock = || fixed_publication_time();
    let mut loaded = load_state(&ac_root, true).expect("load state for the deferred call");
    let deferred = sync_one_template(
        Some(temp.path()),
        &ac_root,
        spec,
        &mut loaded,
        false,
        true,
        false,
        &mut clock,
    )
    .completion
    .expect("classify the deferred repair");
    assert!(
        matches!(
            deferred.target_outcome,
            TemplatePublication::Skipped(ContextTemplateSkipReason::DistributionRepairDeferred)
        ),
        "expected Skipped(DistributionRepairDeferred), got {:?}",
        deferred.target_outcome
    );
    assert!(deferred.replacement.is_none());
    assert!(backup_files(&ac_root).is_empty());

    let mut loaded = load_state(&ac_root, true).expect("load state for the repairing call");
    let repaired = sync_one_template(
        Some(temp.path()),
        &ac_root,
        spec,
        &mut loaded,
        false,
        true,
        true,
        &mut clock,
    )
    .completion
    .expect("run the distribution repair");
    assert!(
        matches!(repaired.target_outcome, TemplatePublication::Published(..)),
        "expected Published(..), got {:?}",
        repaired.target_outcome
    );
    assert!(
        repaired.replacement.is_some(),
        "an installation with no state entry must be notified"
    );

    // (d) the scan, on a second tree built from the same fixture.
    let scan_temp = tempfile::tempdir().expect("tempdir");
    let scan_ac_root = scan_temp.path().join(".ac");
    std::fs::create_dir(&scan_ac_root).expect("create scan workspace");
    let scan_path = scan_ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    std::fs::write(
        &scan_path,
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
    )
    .expect("write pristine v4 global");
    let replacements =
        scan_project_context_template_replacements_for_test(scan_temp.path(), &scan_ac_root)
            .expect("scan the second tree");
    assert_eq!(replacements.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&scan_path).expect("read repaired global"),
        crate::config::session_context::get_default_agent_template()
    );
    let backups = backup_files(&scan_ac_root);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(
        std::fs::read_to_string(&backups[0]).expect("read backup"),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
    );
}

/// #1748 new test 8: distribution ownership and the recognizer slot are one
/// decision, across every spec. Written as ONE loop with a single assertion so
/// the field-access census stays derivable.
#[test]
fn distribution_ownership_and_the_recognizer_slot_never_drift() {
    let [global, coordinator, windows, linux, macos] = project_specs();
    let mut owned = 0usize;
    for spec in [global, coordinator, windows, linux, macos, root_spec()] {
        assert_eq!(
            spec.distribution_owned,
            spec.is_known_generated.is_none(),
            "#1748: {} is distribution-owned exactly when it names no recognizer",
            spec.id
        );
        owned += usize::from(spec.distribution_owned);
    }
    assert_eq!(owned, 1, "exactly one spec is distribution-owned");
}

/// #1748 phase 01b: the raw slice of one template entry in the persisted state
/// file. The assertions below read the serialized BYTES rather than a parsed
/// round trip, because a round trip reads `Null` both when a key holds null and
/// when the field was dropped from every entry (D4). No entry field is itself an
/// object, so the first `}` closes the entry.
fn raw_entry_slice<'a>(raw: &'a str, id: &str) -> &'a str {
    let key = format!("\"{id}\": {{");
    let start = raw
        .find(&key)
        .unwrap_or_else(|| panic!("the state file must carry a `{id}` entry:\n{raw}"));
    let rest = &raw[start + key.len()..];
    let end = rest
        .find('}')
        .unwrap_or_else(|| panic!("the `{id}` entry must be closed:\n{raw}"));
    &rest[..end]
}

/// #1748 phase 01b test 1: only the `global` entry loses its recorded version.
/// The `coordinator` and `unrelated` assertions ARE the control, and they are the
/// ones a DELETION of the field breaks: `SeededContextTemplateState.templates` is
/// a `BTreeMap` and `persist_state` re-serializes the whole map, so dropping the
/// field would drop the key from every entry, including five out-of-stage
/// templates and any unrecognized id.
#[test]
fn only_the_global_entry_loses_its_recorded_version() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    // A base file that DIFFERS from the current default, so the `global` entry
    // takes the repair path and is rewritten.
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
    )
    .expect("write pristine v3 global");
    let mut state = SeededContextTemplateState::default();
    state.templates.insert(
        "global".to_string(),
        SeededContextTemplateEntry {
            template_id: "global".to_string(),
            current_version: Some(1),
            last_seeded_sha256: Some(hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION)),
            last_observed_sha256: None,
            ignored_default_sha256: None,
            ignored_observed_sha256: None,
        },
    );
    state.templates.insert(
        "coordinator".to_string(),
        SeededContextTemplateEntry {
            template_id: "coordinator".to_string(),
            current_version: Some(6),
            last_seeded_sha256: None,
            last_observed_sha256: None,
            ignored_default_sha256: None,
            ignored_observed_sha256: None,
        },
    );
    state.templates.insert(
        "unrelated".to_string(),
        SeededContextTemplateEntry {
            template_id: "unrelated".to_string(),
            current_version: Some(7),
            last_seeded_sha256: None,
            last_observed_sha256: None,
            ignored_default_sha256: None,
            ignored_observed_sha256: None,
        },
    );
    persist_state(&ac_root, &state).expect("persist seeded state");

    scan_project_context_template_updates(temp.path(), &ac_root).expect("scan project");

    let raw = std::fs::read_to_string(state_path(&ac_root)).expect("read seeded state");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("parse seeded state");
    assert_eq!(
        parsed["templates"]["global"]["currentVersion"],
        serde_json::Value::Null,
        "#1748: the distribution-owned global entry records no version:\n{raw}"
    );
    assert!(
        raw_entry_slice(&raw, "coordinator").contains(r#""currentVersion": 6"#),
        "the out-of-stage coordinator entry keeps its recorded version:\n{raw}"
    );
    assert!(
        raw_entry_slice(&raw, "unrelated").contains(r#""currentVersion": 7"#),
        "an unrecognized id keeps its recorded version:\n{raw}"
    );
    assert!(
        raw_entry_slice(&raw, "global").contains(r#""currentVersion": null"#),
        "the key is still serialized on the global entry, as null:\n{raw}"
    );
}

/// #1748 phase 01b test 2: the equal-default path, which D2 exists for. On an
/// installation whose base file already matches the shipped default the scan takes
/// the early return in `sync_one_template`, every downstream guard compares hashes
/// and finds them equal, and without D2 nothing would ever mark the state dirty,
/// so the stale `"currentVersion": 6` would stay on disk forever.
///
/// `"probe": "sentinel"` is the two-sided detector for "was the state file
/// rewritten at all", which no byte comparison can give when a rewrite would
/// reproduce identical content: `SeededContextTemplateEntry` carries no
/// `deny_unknown_fields`, so the key parses in without costing the fixture its
/// trust and is DROPPED on the next serialize. The control arm flips the input,
/// not the expectation: with the recorded version already null, `already_recorded`
/// is true, nothing is persisted and the sentinel survives byte for byte. That arm
/// is also what forbids the lazy reading of D2, an unconditional
/// `self.dirty = true` in `entry_mut`.
#[test]
fn an_in_sync_installation_still_drops_its_recorded_global_version() {
    for (arm, recorded_version, sentinel_survives) in
        [("main", "6", false), ("control", "null", true)]
    {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let current = crate::config::session_context::get_default_agent_template();
        let current_hash = hash_text(current);
        // ONLY `Context.AgentsCommander.md`. The scan passes
        // `allow_create_missing = false`, so the four absent template files return
        // before touching state; writing one of them would mark the state dirty in
        // BOTH arms and the control would stop discriminating while still passing.
        let global_path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(&global_path, current).expect("write in-sync global");
        // Hand-written in the PRETTY shape `to_vec_pretty` produces, WITH the space
        // after each colon (D4a): the control arm asserts against these bytes
        // verbatim while the main arm asserts against bytes the code wrote, and if
        // the two shapes differed the control arm would silently stop testing the
        // same thing. `lastSeededSha256` is the current default's hash, so
        // `mark_seeded`'s own guard is FALSE, which is the whole point.
        let fixture = format!(
            r#"{{
  "schemaVersion": 1,
  "templates": {{
"coordinator": {{
  "templateId": "coordinator",
  "currentVersion": 6
}},
"global": {{
  "templateId": "global",
  "currentVersion": {recorded_version},
  "lastSeededSha256": "{current_hash}",
  "probe": "sentinel"
}}
  }}
}}"#
        );
        std::fs::write(state_path(&ac_root), &fixture).expect("write fixture state");

        let replacements =
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan in-sync workspace");

        assert!(
            replacements.is_empty(),
            "{arm}: an already-current file is never replaced: {replacements:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&global_path).expect("read global"),
            current,
            "{arm}: the template file the user sees is not touched"
        );
        let backups = backup_files(&ac_root);
        assert!(
            backups.is_empty(),
            "{arm}: no backup is created: {backups:?}"
        );

        let raw = std::fs::read_to_string(state_path(&ac_root)).expect("read seeded state");
        assert!(
            raw_entry_slice(&raw, "global").contains(r#""currentVersion": null"#),
            "{arm}: the global entry records no version:\n{raw}"
        );
        assert!(
            raw_entry_slice(&raw, "global")
                .contains(&format!(r#""lastSeededSha256": "{current_hash}""#)),
            "{arm}: the seeded hash is unchanged:\n{raw}"
        );
        assert!(
            raw_entry_slice(&raw, "coordinator").contains(r#""currentVersion": 6"#),
            "{arm}: the out-of-stage coordinator entry is untouched:\n{raw}"
        );
        assert_eq!(
            raw.contains(r#""probe": "sentinel""#),
            sentinel_survives,
            "{arm}: the sentinel survives exactly when no persist ran:\n{raw}"
        );
        if sentinel_survives {
            assert_eq!(raw, fixture, "{arm}: the state file is byte-identical");
        }
    }
}
