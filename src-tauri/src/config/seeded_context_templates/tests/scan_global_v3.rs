use super::*;

/// #1571 T2, failing-first migration proof for the fifth recognizer arm: the
/// assert_ne fails while the live template still equals the frozen v4 bytes
/// (pre-rewrite), and the sync half proves a pristine v4 file on disk
/// auto-upgrades to the current default.
///
/// The direct recognizer assertion is deliberately LAST, unlike the sibling
/// above. With the fifth arm deleted, a first-position direct assert panics
/// before the sync runs, so a mutation probe would only prove that a predicate
/// whose one matching arm was just removed returns false. Asserting the
/// behavior first makes the probe prove that the sync path actually consumes
/// the recognizer, which is the silent-half-migration risk it exists to close.
#[test]
fn read_sync_updates_pre_orchestrator_rename_coordinator_template() {
    assert_ne!(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME,
        get_default_coordinator_template(),
        "the #1571 rename must actually change the template or the freeze is pointless"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME,
    )
    .expect("write pristine v4 coordinator");

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

    assert!(is_known_generated_coordinator_template(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME
    ));
}

/// #1005 S6 / G3: the frozen v1 global snapshot must stay byte-identical to
/// what the #658..ec660c17 builds shipped. Expected values captured by a
/// one-off run of the shipped accessor AT base commit ec660c17, never from
/// this const.
#[test]
fn global_pre_token_minimization_snapshot_is_byte_exact() {
    assert_eq!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION.len(),
        611,
        "frozen v1 global snapshot must be the ec660c17 bytes"
    );
    assert_eq!(
        hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION),
        "c9de5b80ad99a5743ad20c3344e7dd03888792f4da175943bee72e3d7d91fb88",
        "frozen v1 global snapshot changed; it must stay byte-identical to what shipped"
    );
}

/// #1005 S6 failing-first proof for the v2 rewrite (assert_ne), the #979
/// standalone recognizer and the version bump, retargeted by #1748 at the scan:
/// the distribution repair, not the read path, is what lands the current
/// default on an old pristine file.
#[test]
fn scan_replaces_pre_token_minimization_global_template() {
    assert_ne!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
        crate::config::session_context::get_default_agent_template(),
        "v2 rewrite must actually change the template or the freeze is pointless"
    );
    assert!(
        is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        ),
        "standalone (retirement) recognizer must accept the frozen v1 bytes"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
    )
    .expect("write pristine v1 global");

    assert!(
        scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan updates")
            .is_empty(),
        "a distribution-owned template never yields a pending update"
    );

    let content = std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
        .expect("read global");
    assert_eq!(
        content,
        crate::config::session_context::get_default_agent_template(),
        "pristine v1 Context.AgentsCommander.md must be repaired to the default"
    );
    let backups = backup_files(&ac_root);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(
        std::fs::read_to_string(&backups[0]).expect("read backup"),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
        "the backup must hold the pre-run bytes"
    );
    let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("read seeded state");
    let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
    assert_eq!(
        parsed["templates"]["global"]["currentVersion"],
        serde_json::Value::Null,
        "#1748: the distribution-owned global entry records no version"
    );
}

/// #1369 (C4) AC-4.3: the frozen v2 global snapshot must stay byte-identical
/// to what the #1005 S6..8f272a76 builds shipped. Expected values captured
/// externally from the raw literal AT base commit 8f272a76, never from this
/// const.
#[test]
fn global_pre_agent_repos_snapshot_is_byte_exact() {
    assert_eq!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS.len(),
        567,
        "frozen v2 global snapshot must be the 8f272a76 bytes"
    );
    assert_eq!(
        hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS),
        "e5861a9f011967e96e5515f858e1643f7fdf161511ad909fe86ddb4ce1a0cff7",
        "frozen v2 global snapshot changed; it must stay byte-identical to what shipped"
    );
}

#[test]
fn global_before_summarization_snapshot_is_byte_exact() {
    assert_eq!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION.len(),
        563,
        "frozen v3 global snapshot must be the 6aae531e bytes"
    );
    assert_eq!(
        hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION),
        "99a0aa4a15062d4b68b94597111ae268958cbeb4e3902aafe1b7361b63d34157",
        "frozen v3 global snapshot changed; it must stay byte-identical to what shipped"
    );
    assert_ne!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
        crate::config::session_context::get_default_agent_template(),
        "the v4 summarization must differ from its frozen v3 operand"
    );
}

/// #1605: the frozen v4 global snapshot must stay byte-identical to what
/// shipped at base commit 047248bc. Expected values captured by a one-off
/// run of the shipped accessor AT 047248bc (len 539, sha256
/// f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a),
/// never from this const.
#[test]
fn global_before_host_platform_rules_snapshot_is_byte_exact() {
    assert_eq!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES.len(),
        539,
        "frozen v4 global snapshot must be the 047248bc bytes"
    );
    assert_eq!(
        hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES),
        "f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a",
        "frozen v4 global snapshot changed; it must stay byte-identical to what shipped"
    );
    assert!(
        is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        ),
        "the standalone (retirement) recognizer must accept the frozen v4 bytes"
    );
    assert_ne!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
        crate::config::session_context::get_default_agent_template(),
        "the v5 placeholder insertion must differ from its frozen v4 operand"
    );
}

#[test]
fn global_before_answering_snapshot_is_byte_exact() {
    assert_eq!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION.len(),
        559,
        "frozen pre-Answering global snapshot must be the 329ab94e bytes"
    );
    assert_eq!(
        hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION),
        "ee456d60802157ecc8a5f5c5c97522442276524e1f4979481326a80cc6ec5f09",
        "frozen pre-Answering snapshot changed; it must stay byte-identical to what shipped"
    );
    assert!(
        !GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION.contains("## Answering"),
        "control: the frozen predecessor must not already carry the section"
    );
}

#[test]
fn frozen_pre_answering_global_template_is_recognized() {
    assert!(is_known_generated_standalone_global_template(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION
    ));
    assert_ne!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
        crate::config::session_context::get_default_agent_template(),
        "the Answering seed must actually change the global default or the freeze is pointless"
    );
    assert!(crate::config::session_context::get_default_agent_template().contains("## Answering"));
}

#[test]
fn scan_replaces_pre_answering_global_template_and_backs_it_up() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
    )
    .expect("write pristine pre-Answering global");

    let replacements = scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
        .expect("scan pre-Answering global");
    assert_eq!(
        replacements.len(),
        1,
        "with no state entry the replacement is notified"
    );
    let current = crate::config::session_context::get_default_agent_template();
    let content = std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
        .expect("read repaired global");
    assert_eq!(content, current);
    assert_eq!(content.matches("## Answering").count(), 1);
    let backups = backup_files(&ac_root);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(
        std::fs::read_to_string(&backups[0]).expect("read backup"),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
        "the backup must hold the pre-run bytes"
    );
    assert!(
        scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan updates")
            .is_empty(),
        "a distribution-owned template never yields a pending update"
    );
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state"),
    )
    .expect("parse seeded state");
    assert_eq!(
        state["templates"]["global"]["currentVersion"],
        serde_json::Value::Null
    );
    assert_eq!(
        state["templates"]["global"]["lastSeededSha256"],
        hash_text(current)
    );
    assert!(
        scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
            .expect("second scan")
            .is_empty()
    );
    assert_eq!(backup_files(&ac_root).len(), 1, "and no new backup");
}

#[test]
fn scan_replaces_pre_answering_global_template_silently_with_trusted_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
    )
    .expect("write pristine pre-Answering global");
    let mut state = SeededContextTemplateState::default();
    state.templates.insert(
        "global".to_string(),
        SeededContextTemplateEntry {
            template_id: "global".to_string(),
            current_version: None,
            last_seeded_sha256: Some(hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION)),
            last_observed_sha256: None,
            ignored_default_sha256: None,
            ignored_observed_sha256: None,
        },
    );
    persist_state(&ac_root, &state).expect("persist trusted pre-Answering state");

    let replacements = scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
        .expect("scan pre-Answering global");
    assert!(
        replacements.is_empty(),
        "a trusted entry naming these exact bytes makes the repair silent"
    );
    let current = crate::config::session_context::get_default_agent_template();
    assert_eq!(
        std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read repaired global"),
        current
    );
    assert_eq!(backup_files(&ac_root).len(), 1, "silent is still backed up");
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state"),
    )
    .expect("parse seeded state");
    assert_eq!(
        state["templates"]["global"]["lastSeededSha256"],
        hash_text(current)
    );
    assert!(
        scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
            .expect("second scan")
            .is_empty()
    );
    assert_eq!(backup_files(&ac_root).len(), 1, "and no new backup");
}

#[test]
fn global_before_summarization_is_an_exact_generated_operand() {
    let one_byte = format!("{GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION}X");
    let crlf = GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION.replace('\n', "\r\n");
    let recognizers = [(
        "standalone",
        is_known_generated_standalone_global_template as fn(&str) -> bool,
    )];

    for (label, recognizes) in recognizers {
        assert!(
            recognizes(GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION),
            "{label} recognizer must accept the exact v3 operand"
        );
        assert!(!recognizes(&one_byte), "{label} accepted v3 + X");
        assert!(!recognizes(&crlf), "{label} accepted CRLF v3");
    }
}

#[test]
fn scan_replaces_pristine_v3_global_template_without_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
    )
    .expect("write pristine v3 global");

    let replacements = scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
        .expect("scan pristine v3 global");
    assert_eq!(
        replacements.len(),
        1,
        "with no state entry the replacement is notified"
    );
    let current = crate::config::session_context::get_default_agent_template();
    let current_hash = hash_text(current);
    assert_eq!(
        std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read repaired global"),
        current
    );
    assert_eq!(backup_files(&ac_root).len(), 1);
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state"),
    )
    .expect("parse seeded state");
    assert_eq!(
        state["templates"]["global"]["currentVersion"],
        serde_json::Value::Null
    );
    assert_eq!(
        state["templates"]["global"]["lastSeededSha256"],
        current_hash
    );

    assert!(
        scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
            .expect("second scan")
            .is_empty(),
        "a second scan must produce no replacement"
    );
    assert_eq!(backup_files(&ac_root).len(), 1, "and no new backup");
    assert!(scan_project_context_template_updates(temp.path(), &ac_root)
        .expect("scan updates")
        .is_empty());
}

#[test]
fn scan_replaces_pristine_v3_global_template_with_trusted_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
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
            current_version: Some(3),
            last_seeded_sha256: Some(hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION)),
            last_observed_sha256: None,
            ignored_default_sha256: None,
            ignored_observed_sha256: None,
        },
    );
    persist_state(&ac_root, &state).expect("persist trusted v3 state");

    let replacements = scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
        .expect("scan pristine v3 global");
    assert!(
        replacements.is_empty(),
        "a trusted entry naming these exact bytes makes the repair silent"
    );
    let current = crate::config::session_context::get_default_agent_template();
    let current_hash = hash_text(current);
    assert_eq!(
        std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read repaired global"),
        current
    );
    assert_eq!(
        backup_files(&ac_root).len(),
        1,
        "silent does not mean unbacked-up"
    );
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state"),
    )
    .expect("parse seeded state");
    assert_eq!(
        state["templates"]["global"]["currentVersion"],
        serde_json::Value::Null
    );
    assert_eq!(
        state["templates"]["global"]["lastSeededSha256"],
        current_hash
    );

    assert!(
        scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
            .expect("second scan")
            .is_empty(),
        "a second scan must produce no replacement"
    );
    assert_eq!(backup_files(&ac_root).len(), 1, "and no new backup");
    assert!(scan_project_context_template_updates(temp.path(), &ac_root)
        .expect("scan updates")
        .is_empty());
}

/// #1748 inverted this: a near match is no longer a preserved category. It is
/// drift like any other and the scan replaces it, in both state shapes. The
/// read path still leaves it alone, which is D2.
#[test]
fn scan_replaces_v3_global_near_matches_in_both_project_state_shapes() {
    let variants = [
        (
            "one-byte",
            format!("{GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION}X").into_bytes(),
        ),
        (
            "crlf",
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION
                .replace('\n', "\r\n")
                .into_bytes(),
        ),
    ];

    for (label, bytes) in variants {
        for trusted_state in [false, true] {
            let temp = tempfile::tempdir().expect("tempdir");
            let ac_root = temp.path().join(".ac");
            std::fs::create_dir(&ac_root).expect("create workspace");
            std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), &bytes)
                .expect("write v3 near match");
            if trusted_state {
                let mut state = SeededContextTemplateState::default();
                state.templates.insert(
                    "global".to_string(),
                    SeededContextTemplateEntry {
                        template_id: "global".to_string(),
                        current_version: Some(3),
                        last_seeded_sha256: Some(hash_text(
                            GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
                        )),
                        last_observed_sha256: None,
                        ignored_default_sha256: None,
                        ignored_observed_sha256: None,
                    },
                );
                persist_state(&ac_root, &state).expect("persist trusted v3 state");
            }

            let published_at = fixed_publication_time();
            assert!(
                sync_for_read_at(&ac_root, GLOBAL_CONTEXT_TEMPLATE_FILENAME, published_at)
                    .is_empty(),
                "{label}/{trusted_state}: the read path must not publish a repair"
            );
            assert_eq!(
                std::fs::read(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                    .expect("read preserved near match"),
                bytes,
                "{label}/{trusted_state}: the read path changed bytes"
            );

            let replacements =
                scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                    .expect("scan v3 near match");
            assert_eq!(
                replacements.len(),
                1,
                "{label}/{trusted_state}: a near match is drift and is notified"
            );
            let current = crate::config::session_context::get_default_agent_template();
            assert_eq!(
                std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                    .expect("read repaired near match"),
                current,
                "{label}/{trusted_state}: the scan must repair a near match"
            );
            let backups = backup_files(&ac_root);
            assert_eq!(backups.len(), 1, "{label}/{trusted_state}: {backups:?}");
            assert_eq!(
                std::fs::read(&backups[0]).expect("read backup"),
                bytes,
                "{label}/{trusted_state}: the backup must hold the near-match bytes"
            );

            assert!(
                scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                    .expect("second scan v3 near match")
                    .is_empty(),
                "{label}/{trusted_state}: the second scan is idempotent"
            );
            assert_eq!(backup_files(&ac_root).len(), 1, "{label}/{trusted_state}");
            assert!(
                scan_project_context_template_updates(temp.path(), &ac_root)
                    .expect("second scan v3 near match")
                    .is_empty(),
                "{label}/{trusted_state}: never a pending update"
            );
        }
    }
}

/// #1748 inverted this the same way as the near-match test: a fine custom
/// global is replaced by the scan, in both state shapes.
#[test]
fn scan_replaces_the_compact_fine_token_global_in_both_state_shapes() {
    const FINE_TEMPLATE: &str = "# Fine custom context\n\
        {{AGENT_ROOT}}{{MATRIX_SECTION}}{{MESSAGING_EXCEPTION}}{{MATRIX_ALLOWED}}\
        {{MESSAGING_ALLOWED}}{{FORBIDDEN_SCOPE}}\
        {{GIT_SCOPE}}{{SKILLS_SECTION}}{{PEER_NAME_FORMAT}}{{SEND_MESSAGE_INSTRUCTIONS}}\n";
    assert!(!is_known_generated_standalone_global_template(
        FINE_TEMPLATE
    ));

    for trusted_state in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            FINE_TEMPLATE,
        )
        .expect("write fine custom global");
        if trusted_state {
            let mut state = SeededContextTemplateState::default();
            state.templates.insert(
                "global".to_string(),
                SeededContextTemplateEntry {
                    template_id: "global".to_string(),
                    current_version: Some(3),
                    last_seeded_sha256: Some(hash_text(
                        GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
                    )),
                    last_observed_sha256: None,
                    ignored_default_sha256: None,
                    ignored_observed_sha256: None,
                },
            );
            persist_state(&ac_root, &state).expect("persist trusted prior state");
        }

        assert!(
            sync_for_read_at(
                &ac_root,
                GLOBAL_CONTEXT_TEMPLATE_FILENAME,
                fixed_publication_time(),
            )
            .is_empty(),
            "the read path must not publish a repair"
        );
        assert_eq!(
            std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                .expect("read fine custom global"),
            FINE_TEMPLATE
        );

        let replacements =
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan fine custom global");
        assert_eq!(
            replacements.len(),
            1,
            "{trusted_state}: fine custom content is drift and is notified"
        );
        assert!(
            scan_project_context_template_updates(temp.path(), &ac_root)
                .expect("scan fine custom global")
                .is_empty(),
            "{trusted_state}: never a pending update"
        );
        assert_eq!(
            std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                .expect("re-read repaired global"),
            crate::config::session_context::get_default_agent_template()
        );
        let backups = backup_files(&ac_root);
        assert_eq!(backups.len(), 1, "{trusted_state}: {backups:?}");
        assert_eq!(
            std::fs::read_to_string(&backups[0]).expect("read backup"),
            FINE_TEMPLATE
        );
    }
}

/// #1605 failing-first proof for the v5 placeholder insertion (assert_ne), the
/// #979 standalone recognizer and the version bump, retargeted by #1748 at the
/// scan.
#[test]
fn scan_replaces_pre_host_platform_rules_global_template() {
    assert_ne!(
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
        crate::config::session_context::get_default_agent_template(),
        "the v5 rewrite must actually change the template or the freeze is pointless"
    );
    assert!(
        is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        ),
        "standalone (retirement) recognizer must accept the frozen v4 bytes"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
    )
    .expect("write pristine v4 global");

    assert_eq!(
        scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
            .expect("scan pristine v4 global")
            .len(),
        1
    );

    let content = std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
        .expect("read global");
    assert_eq!(
        content,
        crate::config::session_context::get_default_agent_template(),
        "pristine v4 Context.AgentsCommander.md must be repaired to v6"
    );
    let backups = backup_files(&ac_root);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(
        std::fs::read_to_string(&backups[0]).expect("read backup"),
        GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
    );
    let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("read seeded state");
    let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
    assert_eq!(
        parsed["templates"]["global"]["currentVersion"],
        serde_json::Value::Null,
        "#1748: the distribution-owned global entry records no version"
    );
}
