use super::*;

#[test]
fn upsert_and_same_millisecond_noop_use_atomic_writer_once() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let row = context_row(
        "Context.AgentsCommander.md",
        "context:agentscommander",
        "2026-07-16T19:40:07.123Z",
    )
    .unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .record_file(&activation, row.clone()),
        ManifestRecordOutcome::Recorded
    );
    let first = std::fs::read(canonical_path(&project)).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(&activation, row),
        ManifestRecordOutcome::Unchanged
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), first);

    let later = context_row(
        "Context.AgentsCommander.md",
        "context:agentscommander",
        "2026-07-16T19:40:07.124Z",
    )
    .unwrap();
    assert_eq!(
        guard.publication_permit().record_file(&activation, later),
        ManifestRecordOutcome::Recorded
    );
    guard.release();
    assert!(
        String::from_utf8(std::fs::read(canonical_path(&project)).unwrap())
            .unwrap()
            .contains("2026-07-16T19:40:07.124Z")
    );
}

#[test]
fn config_scope_replacement_is_canonical_noop_and_preserves_in_memory_semantics() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let scope = config_scope("wg-1-team", "alpha", ".claude");
    let first = PublishedScopeBatch::new(
        scope.clone(),
        ManifestSource::WorkspaceBase,
        vec![
            config_path("wg-1-team", "alpha", ".claude", "a"),
            config_path("wg-1-team", "alpha", ".claude", "b"),
        ],
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap();
    let second = PublishedScopeBatch::new(
        scope.clone(),
        ManifestSource::MatrixBase,
        vec![
            config_path("wg-1-team", "alpha", ".claude", "b"),
            config_path("wg-1-team", "alpha", ".claude", "c"),
        ],
        timestamp("2026-07-16T19:42:12.456Z"),
    )
    .unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    let canonical = std::fs::read(canonical_path(&project)).unwrap();
    assert_eq!(
        guard.publication_permit().replace_scope(&activation, first),
        ManifestRecordOutcome::Unchanged
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), canonical);
    let paths = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state
            .rows
            .values()
            .map(|row| row.path.serialized().to_string())
            .collect::<Vec<_>>(),
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(
        paths,
        vec![
            ".ac/Context.AgentsCommander.md".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/a".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/b".to_string(),
        ]
    );
    assert_eq!(
        guard
            .publication_permit()
            .replace_scope(&activation, second),
        ManifestRecordOutcome::Unchanged
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), canonical);
    let paths = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state
            .rows
            .values()
            .map(|row| row.path.serialized().to_string())
            .collect::<Vec<_>>(),
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(
        paths,
        vec![
            ".ac/Context.AgentsCommander.md".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/b".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/c".to_string(),
        ]
    );
    let empty = PublishedScopeBatch::new(
        scope,
        ManifestSource::MatrixBase,
        Vec::new(),
        timestamp("2026-07-16T19:43:12.456Z"),
    )
    .unwrap();
    assert_eq!(
        guard.publication_permit().replace_scope(&activation, empty),
        ManifestRecordOutcome::Unchanged
    );
    let final_paths = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state
            .rows
            .values()
            .map(|row| row.path.serialized().to_string())
            .collect::<Vec<_>>(),
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(
        final_paths,
        vec![".ac/Context.AgentsCommander.md".to_string()]
    );
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), canonical);
    assert_eq!(read_disk_state(&project).rows.len(), 1);
}

#[test]
fn empty_scope_against_absence_is_noop_without_header_file() {
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let batch = PublishedScopeBatch::new(
        config_scope("wg-1-team", "alpha", ".claude"),
        ManifestSource::WorkspaceBase,
        Vec::new(),
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .replace_scope(&ManifestActivationToken::for_test(), batch),
        ManifestRecordOutcome::Unchanged
    );
    guard.release();
    assert!(!canonical_path(&project).exists());
}

#[test]
fn overbound_removal_from_raw_legacy_scope_writes_canonical_without_partial_rows() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let scope = config_scope("wg-1-team", "alpha", ".claude");
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            scope.clone(),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    std::fs::write(
        canonical_path(&project),
        serialize_raw_legacy_fixture(&legacy),
    )
    .unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().remove_unrecordable_scope(
            &activation,
            scope,
            ResourceBoundKind::Rows,
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ResourceBound(
            ResourceBoundKind::Rows
        ))
    );
    guard.release();
    assert!(read_disk_state(&project).rows.is_empty());
}

#[test]
fn failed_overbound_scope_removal_reports_the_persistence_failure() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let scope = config_scope("wg-1-team", "alpha", ".claude");
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            scope.clone(),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    std::fs::write(
        canonical_path(&project),
        serialize_raw_legacy_fixture(&legacy),
    )
    .unwrap();

    let external = b"lock-unaware external edit".to_vec();
    let hook_bytes = external.clone();
    let hooks = TestFilesystemHooks {
        before_raw_conflict_check: Some(Arc::new(move |canonical| {
            std::fs::write(canonical, &hook_bytes).expect("write external edit");
        })),
        ..TestFilesystemHooks::default()
    };
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, DEFAULT_LOCK_TIMEOUT, hooks)
            .unwrap();
    assert_eq!(
        guard.publication_permit().remove_unrecordable_scope(
            &activation,
            scope,
            ResourceBoundKind::Rows,
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ExternalEdit)
    );
    let restored = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(restored.rows.len(), 1);
    assert!(restored.rows.values().any(|row| {
        row.path.serialized() == ".ac/wg-1-team/__agent_alpha/.claude/settings.json"
    }));
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), external);
}

#[test]
fn lifecycle_filters_are_component_wise_for_foreign_encoded_rows() {
    let mut bytes = b".ac/wg-1-team/__agent_alpha/.claude/foreign".to_vec();
    bytes.push(0xff);
    let foreign = ManifestPathIdentity::parse(
        ManifestPathEncoding::UnixBytesHex,
        encode_lower_hex_bytes(&bytes),
    )
    .unwrap();
    let mut state = ManifestState::default();
    let _ = state.upsert(
        PublishedManifestRow::replica_config(
            foreign,
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    let _ = state.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-2-team", "beta", ".claude", "keep"),
            config_scope("wg-2-team", "beta", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    let filter = ManifestLifecycleFilter::config_path_prefix(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/wg-1-team/__agent_alpha".to_string(),
        )
        .unwrap(),
    )
    .unwrap();
    let journal = state.remove_matching(|row| filter.matches(row));
    assert!(journal.changed());
    assert_eq!(state.rows.len(), 1);
    assert!(state
        .rows
        .values()
        .next()
        .unwrap()
        .path
        .serialized()
        .contains("wg-2-team"));
}

#[test]
fn lifecycle_filter_prunes_legacy_config_and_writes_canonical_supported_rows() {
    let (_temp, project) = setup_project();
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-2-team", "beta", ".claude", "settings.json"),
            config_scope("wg-2-team", "beta", ".claude"),
            ManifestSource::MatrixBase,
            timestamp("2026-07-16T19:42:12.456Z"),
        )
        .unwrap(),
    );
    let raw = serialize_raw_legacy_fixture(&legacy);
    std::fs::write(canonical_path(&project), &raw).unwrap();
    let activation = ManifestActivationToken::for_test();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let absent = ManifestLifecycleFilter::replica_component("__agent_absent".to_string()).unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .apply_lifecycle_filter(&activation, absent),
        ManifestRecordOutcome::Unchanged
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), raw);
    let unchanged = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(unchanged, &legacy);

    let alpha = ManifestLifecycleFilter::replica_component("__agent_alpha".to_string()).unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .apply_lifecycle_filter(&activation, alpha),
        ManifestRecordOutcome::Recorded
    );
    let held = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(held.rows.len(), 2);
    assert!(held
        .rows
        .values()
        .any(|row| row.path.serialized().contains("wg-2-team")));
    assert!(!held
        .rows
        .values()
        .any(|row| row.path.serialized().contains("wg-1-team")));
    let disk = std::fs::read(canonical_path(&project)).unwrap();
    assert!(!String::from_utf8_lossy(&disk).contains("replica_config_file"));
    guard.release();
    let parsed = parse_manifest_bytes(&disk).unwrap();
    assert_eq!(parsed.rows.len(), 1);
    let context = parsed.rows.values().next().unwrap();
    assert_eq!(context.kind, ManifestFileKind::ProjectContextTemplate);
    assert_eq!(context.path.serialized(), ".ac/Context.AgentsCommander.md");
    assert_eq!(context.scope, "context:agentscommander");
    assert_eq!(context.source, ManifestSource::Builtin);
    assert_eq!(context.published_at, timestamp("2026-07-16T19:40:07.123Z"));
    assert_eq!(serialize_state(&parsed).unwrap(), disk);
}

#[test]
fn lifecycle_filter_persistence_failure_rolls_back_and_preserves_external_bytes() {
    let (_temp, project) = setup_project();
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    std::fs::write(
        canonical_path(&project),
        serialize_raw_legacy_fixture(&legacy),
    )
    .unwrap();
    let external = b"lock-unaware lifecycle edit".to_vec();
    let hook_bytes = external.clone();
    let hooks = TestFilesystemHooks {
        before_raw_conflict_check: Some(Arc::new(move |canonical| {
            std::fs::write(canonical, &hook_bytes).expect("write external edit");
        })),
        ..TestFilesystemHooks::default()
    };
    let filter = ManifestLifecycleFilter::replica_component("__agent_alpha".to_string()).unwrap();
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, DEFAULT_LOCK_TIMEOUT, hooks)
            .unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .apply_lifecycle_filter(&ManifestActivationToken::for_test(), filter,),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ExternalEdit)
    );
    let restored = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(restored, &legacy);
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), external);
}

#[test]
fn degraded_permit_never_records_even_with_an_activation_token() {
    let permit =
        ProjectPublicationPermit::degraded_without_guard(ManifestDegradedReason::LockUnavailable);
    assert!(!permit.is_tracked());
    let outcome = permit.record_file(
        &ManifestActivationToken::production(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::LockUnavailable)
    );
}

#[test]
fn production_activation_token_records_through_a_tracked_permit() {
    // Stage F: the production constructor is a real, non-test activation path.
    // A tracked permit driven by it records exactly like the test token, so
    // production emission is genuinely wired and not a dormant no-op.
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let outcome = guard.publication_permit().record_file(
        &ManifestActivationToken::production(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    assert_eq!(outcome, ManifestRecordOutcome::Recorded);
    guard.release();
    let disk = read_disk_state(&project);
    assert_eq!(disk.rows.len(), 1);
}
