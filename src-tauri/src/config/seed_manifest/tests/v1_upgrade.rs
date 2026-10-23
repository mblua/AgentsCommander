use super::*;

// ---- #1318 v1 -> v2 coverage migration + catalog row -------------------

/// A byte-exact v1 manifest (context row + replica-config row with distinct
/// timestamps), exactly what an old build wrote.
fn v1_manifest_fixture() -> String {
    concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 1\n",
        "coverage_version = 1\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/Context.AgentsCommander.md\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"project_context_template\"\n",
        "scope = \"context:agentscommander\"\n",
        "source = \"builtin\"\n",
        "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/wg-1-dev-team/__agent_alpha/.claude/settings.json\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"replica_config_file\"\n",
        "scope = \"config:.ac/wg-1-dev-team/__agent_alpha/.claude\"\n",
        "source = \"workspace_base\"\n",
        "last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n"
    )
    .to_string()
}

#[test]
fn v1_manifest_upgrades_to_v2_preserving_supported_rows_and_omitting_legacy_config() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    let v1 = v1_manifest_fixture();
    std::fs::write(&path, &v1).unwrap();

    // First acquire runs the one-shot upgrade under the held lock.
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        disk.contains("coverage_version = 2"),
        "upgraded manifest must declare coverage v2: {disk}"
    );
    assert!(
        disk.contains(
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \
                \"coding_agent_catalog\"]"
        ),
        "upgraded manifest must carry the 3-item v2 coverage: {disk}"
    );
    // Supported rows and timestamps remain on disk; the legacy config row is
    // retained only in the held state.
    let state = read_disk_state(&project);
    assert_eq!(state.rows.len(), 1);
    assert!(disk.contains("2026-07-16T19:40:07.123Z"));
    assert!(!disk.contains("replica_config_file"));
    assert!(!disk.contains(".ac/wg-1-dev-team/__agent_alpha/.claude/settings.json"));
    assert!(!disk.contains("2026-07-16T19:41:12.456Z"));
    let held = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(held.rows.len(), 2);
    assert!(held
        .rows
        .values()
        .any(|row| row.kind == ManifestFileKind::ReplicaConfigFile));
    // The guard is a fully writable tracked snapshot: publications record.
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            PublishedManifestRow::coding_agent_catalog(
                ManifestPathIdentity::parse(
                    ManifestPathEncoding::Utf8,
                    ".ac/coding-agents/agents.json".to_string(),
                )
                .unwrap(),
                timestamp("2026-07-16T19:42:00.000Z"),
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    let held_after_record = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(held_after_record.rows.len(), 3);
    assert!(held_after_record
        .rows
        .values()
        .any(|row| row.kind == ManifestFileKind::ReplicaConfigFile));
    guard.release();

    // Runs exactly once: a second acquire parses v2 strictly and never
    // re-enters the migration.
    let after_first = std::fs::read(&path).unwrap();
    let guard2 = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let reacquired = match &guard2.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(reacquired.rows.len(), 2);
    assert!(reacquired
        .rows
        .values()
        .all(|row| row.kind.is_canonical_output()));
    guard2.release();
    assert_eq!(std::fs::read(&path).unwrap(), after_first);
    let state = read_disk_state(&project);
    assert_eq!(state.rows.len(), 2);
    let disk = String::from_utf8(after_first).unwrap();
    assert!(disk.contains("coding_agent_catalog"));
    assert!(!disk.contains("replica_config_file"));
}

#[test]
fn v1_with_different_coverage_list_stays_strictly_invalid() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    // v1 header with a DIFFERENT (3-item, wrong order) coverage list.
    let v1 = v1_manifest_fixture().replace(
        "coverage = [\"project_context_templates\", \"replica_config_folders\"]",
        "coverage = [\"replica_config_folders\", \"project_context_templates\"]",
    );
    std::fs::write(&path, &v1).unwrap();

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
    );
    guard.release();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        v1,
        "non-exact v1 shape stays byte-preserved"
    );
}

#[test]
fn coverage_version_three_stays_invalid_and_non_migratable() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    // v1 -> v2 -> the WRONG value for this build (3): the strict parse
    // rejects it and the migration must not touch it (not the exact v1
    // shape: coverage_version != 1).
    let wrong = v1_manifest_fixture().replace("coverage_version = 1", "coverage_version = 3");
    std::fs::write(&path, &wrong).unwrap();

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
    );
    guard.release();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        wrong,
        "wrong coverage version stays byte-preserved"
    );
}

#[test]
fn bound_violating_v1_file_stays_preserved_read_only() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    // An over-long path field: the wire deserializers enforce MAX_FIELD_BYTES
    // on the strict parse (ResourceBound, never migratable) AND on the
    // migration's lenient pre-parse, so the file stays preserved either way.
    let overlong = format!("x{}\n", "a".repeat(MAX_FIELD_BYTES + 1));
    let v1 = format!(
        concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 1\n",
            "coverage_version = 1\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
            "\n",
            "[[files]]\n",
            "path = \"{}\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"project_context_template\"\n",
            "scope = \"context:agentscommander\"\n",
            "source = \"builtin\"\n",
            "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n"
        ),
        overlong
    );
    std::fs::write(&path, &v1).unwrap();

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    // The strict parse classifies the over-long path as a parse failure on
    // this TOML surface (the lenient pre-parse fails the same way), and the
    // migration is never entered; either way the contract is preservation
    // with the writer disabled.
    let outcome = guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    assert!(matches!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(
            ManifestDegradedReason::InvalidCanonical | ManifestDegradedReason::ResourceBound(_)
        )
    ));
    guard.release();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        v1.as_bytes(),
        "bound-violating v1 file stays byte-preserved"
    );
}

#[test]
fn future_schema_snapshot_never_migrated() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    let future = concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 2\n",
        "coverage_version = 1\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
        "files = []\n"
    );
    std::fs::write(&path, future).unwrap();

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::FutureSchema)
    );
    guard.release();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        future.as_bytes(),
        "future-schema manifest stays byte-preserved"
    );
}

#[test]
fn external_edit_conflict_during_upgrade_preserves_v1_bytes() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    std::fs::write(&path, v1_manifest_fixture()).unwrap();

    // A lock-unaware external writer replaces the file between the
    // migration's re-read and its atomic write: `write_canonical`'s
    // raw-conflict check must fail and preserve the external bytes.
    let external = b"external lock-unaware edit\n".to_vec();
    let hooks = TestFilesystemHooks {
        before_raw_conflict_check: Some(Arc::new({
            let external = external.clone();
            move |canonical| std::fs::write(canonical, &external).unwrap()
        })),
        ..TestFilesystemHooks::default()
    };
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
            .unwrap();
    // The migration's own write fails the raw-conflict check and restores the
    // pre-migration degraded snapshot (InvalidCanonical), so the next
    // publication reports the restored reason; the load-bearing assertion is
    // that the external bytes win and no partial v2 upgrade ever lands.
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
    );
    guard.release();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        external,
        "the external edit wins; v1 bytes never partially upgraded"
    );
}

#[test]
fn coding_agent_catalog_row_requires_exact_path_scope_source_encoding() {
    let at = timestamp("2026-07-16T19:43:00.000Z");
    let ok = PublishedManifestRow::coding_agent_catalog(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/coding-agents/agents.json".to_string(),
        )
        .unwrap(),
        at,
    )
    .expect("exact catalog row is accepted");
    assert_eq!(ok.kind, ManifestFileKind::CodingAgentCatalog);
    assert_eq!(ok.scope, "catalog:coding-agents");
    assert_eq!(ok.source, ManifestSource::Builtin);
    assert_eq!(ok.path.serialized, ".ac/coding-agents/agents.json");
    assert_eq!(ok.path.encoding, ManifestPathEncoding::Utf8);

    // Wrong path -> rejected.
    assert!(PublishedManifestRow::coding_agent_catalog(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/coding-agents/other.json".to_string(),
        )
        .unwrap(),
        at,
    )
    .is_err());
    // Non-utf8 encoding -> rejected. A WindowsUtf16Hex wire shape only
    // parses for paths that are NOT valid Unicode (e.g. an unpaired
    // surrogate), exactly like the existing round-trip fixtures.
    let mut windows_units: Vec<u16> = ".ac/coding-agents/agents.json".encode_utf16().collect();
    windows_units.push(0xd800);
    let windows_hex = windows_units
        .iter()
        .map(|unit| format!("{unit:04x}"))
        .collect::<String>();
    assert!(PublishedManifestRow::coding_agent_catalog(
        ManifestPathIdentity::parse(ManifestPathEncoding::WindowsUtf16Hex, windows_hex).unwrap(),
        at,
    )
    .is_err());

    // A catalog row records under a held gate, alongside context rows.
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            PublishedManifestRow::coding_agent_catalog(
                ManifestPathIdentity::parse(
                    ManifestPathEncoding::Utf8,
                    ".ac/coding-agents/agents.json".to_string(),
                )
                .unwrap(),
                at,
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    guard.release();
    let disk = std::fs::read_to_string(canonical_path(&project)).unwrap();
    assert!(disk.contains("kind = \"coding_agent_catalog\""));
    assert!(disk.contains("scope = \"catalog:coding-agents\""));
    assert!(disk.contains("path = \".ac/coding-agents/agents.json\""));
    assert!(disk.contains("source = \"builtin\""));
}

#[test]
fn lock_identity_replacement_is_detected() {
    let (_temp, project) = setup_project();
    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let lock_path = project.join(".ac").join(SEED_MANIFEST_LOCK_FILENAME);
    std::fs::remove_file(&lock_path).unwrap();
    std::fs::write(&lock_path, b"replacement").unwrap();
    assert!(matches!(
        guard.revalidate_owner(),
        Err(SeedManifestError::UnsafePath { .. })
    ));
    guard.release();
}

#[test]
fn pinned_owner_chain_detects_replacement() {
    let (_temp, project) = setup_project();
    let ac = project.join(".ac");
    let owner = ac.join("wg-1-team").join("__agent_alpha");
    std::fs::create_dir_all(&owner).unwrap();
    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let chain = guard
        .pin_owner_chain_from_ac(Path::new("wg-1-team/__agent_alpha"))
        .unwrap();
    #[cfg(not(windows))]
    {
        std::fs::rename(ac.join("wg-1-team"), ac.join("wg-old")).unwrap();
        std::fs::create_dir_all(&owner).unwrap();
    }
    #[cfg(windows)]
    let chain = {
        let mut chain = chain;
        chain.directories[0].identity.file ^= 1;
        chain
    };
    assert!(matches!(
        chain.revalidate(),
        Err(SeedManifestError::UnsafePath { .. })
    ));
    guard.release();
}
