use super::*;

#[test]
fn golden_empty_manifest_is_exact() {
    let bytes = serialize_state(&ManifestState::default()).expect("serialize empty");
    assert_eq!(
        bytes,
        concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 1\n",
            "coverage_version = 2\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \
                \"coding_agent_catalog\"]\n",
            "files = []\n"
        )
        .as_bytes()
    );
    assert!(!bytes.windows(2).any(|window| window == b"\r\n"));
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.ends_with(b"\n\n"));
}

#[test]
fn golden_populated_manifest_is_exact_sorted_and_omits_legacy_config() {
    let state = populated_state();
    assert_eq!(state.rows.len(), 3);
    assert_eq!(state.canonical_rows().count(), 2);
    let bytes = serialize_state(&state).expect("serialize populated");
    let expected = concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 1\n",
        "coverage_version = 2\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\", \
            \"coding_agent_catalog\"]\n",
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
        "path = \".ac/coding-agents/agents.json\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"coding_agent_catalog\"\n",
        "scope = \"catalog:coding-agents\"\n",
        "source = \"builtin\"\n",
        "last_seeded_at = \"2026-07-16T19:42:00.789Z\"\n"
    );
    assert_eq!(bytes, expected.as_bytes());
    assert_eq!(exact_serialized_len(&state).unwrap(), bytes.len());
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.ends_with(b"\n\n"));
    assert!(!bytes
        .windows(b"replica_config_file".len())
        .any(|window| { window == b"replica_config_file" }));
    assert!(!String::from_utf8_lossy(&bytes)
        .contains(".ac/wg-14-dev-team/__agent_architect/.claude/settings.json"));
    let parsed = parse_manifest_bytes(&bytes).unwrap();
    assert_eq!(parsed.rows.len(), 2);
    assert!(parsed
        .rows
        .values()
        .all(|row| row.kind.is_canonical_output()));
    assert_eq!(serialize_state(&parsed).unwrap(), bytes);
}

#[test]
fn legacy_only_state_serializes_as_empty_canonical_manifest() {
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    assert_eq!(legacy.rows.len(), 1);
    assert_eq!(legacy.canonical_rows().count(), 0);
    let canonical = serialize_state(&legacy).expect("serialize canonical output");
    assert_eq!(
        canonical,
        serialize_state(&ManifestState::default()).unwrap()
    );
    assert_eq!(exact_serialized_len(&legacy).unwrap(), canonical.len());
    let parsed = parse_manifest_bytes(&canonical).unwrap();
    assert!(parsed.rows.is_empty());
    assert_eq!(serialize_state(&parsed).unwrap(), canonical);
}

#[test]
fn valid_legacy_replica_config_rows_remain_readable() {
    let scope = config_scope("wg-1-team", "alpha", ".claude");
    let source = ManifestSource::WorkspaceBase;
    let at = timestamp("2026-07-16T19:41:12.456Z");
    let mut legacy = ManifestState::default();
    for suffix in ["settings.json", "hooks.json"] {
        let _ = legacy.upsert(
            PublishedManifestRow::replica_config(
                config_path("wg-1-team", "alpha", ".claude", suffix),
                scope.clone(),
                source,
                at,
            )
            .unwrap(),
        );
    }
    let raw = serialize_raw_legacy_fixture(&legacy);
    let parsed = parse_manifest_bytes(&raw).expect("valid legacy rows remain readable");
    assert_eq!(parsed.rows.len(), 2);
    let paths = parsed
        .rows
        .values()
        .map(|row| row.path.serialized().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            ".ac/wg-1-team/__agent_alpha/.claude/hooks.json".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/settings.json".to_string(),
        ]
    );
    for row in parsed.rows.values() {
        assert_eq!(row.kind, ManifestFileKind::ReplicaConfigFile);
        assert_eq!(row.scope, scope);
        assert_eq!(row.source, source);
        assert_eq!(row.published_at, at);
    }
}

#[test]
fn insertion_order_does_not_change_bytes() {
    let first = populated_state();
    let mut second = ManifestState::default();
    for row in first.rows.values().rev() {
        let _ = second.upsert(row.clone());
    }
    assert_eq!(
        serialize_state(&first).unwrap(),
        serialize_state(&second).unwrap()
    );
}

#[test]
fn strict_wire_rejects_missing_unknown_duplicate_and_noncanonical_values() {
    let empty = String::from_utf8(serialize_state(&ManifestState::default()).unwrap()).unwrap();
    let without_files = empty.replace("files = []\n", "");
    assert!(matches!(
        parse_manifest_bytes(without_files.as_bytes()),
        Err(SeedManifestError::Parse(_))
    ));

    let unknown = empty.replace("schema_version = 1\n", "schema_version = 1\nextra = 1\n");
    assert!(matches!(
        parse_manifest_bytes(unknown.as_bytes()),
        Err(SeedManifestError::Parse(_))
    ));

    let duplicate = empty.replace(
        "schema_version = 1\n",
        "schema_version = 1\nschema_version = 1\n",
    );
    assert!(matches!(
        parse_manifest_bytes(duplicate.as_bytes()),
        Err(SeedManifestError::Parse(_))
    ));

    let wrong_coverage = empty.replace("coverage_version = 2", "coverage_version = 3");
    assert!(matches!(
        parse_manifest_bytes(wrong_coverage.as_bytes()),
        Err(SeedManifestError::Validation(_))
    ));

    let unknown_enum = String::from_utf8(serialize_state(&populated_state()).unwrap())
        .unwrap()
        .replace("source = \"builtin\"", "source = \"mystery\"");
    assert!(matches!(
        parse_manifest_bytes(unknown_enum.as_bytes()),
        Err(SeedManifestError::Parse(_))
    ));

    let noncanonical_time = String::from_utf8(serialize_state(&populated_state()).unwrap())
        .unwrap()
        .replace("2026-07-16T19:40:07.123Z", "2026-07-16T19:40:07.123+00:00");
    assert!(matches!(
        parse_manifest_bytes(noncanonical_time.as_bytes()),
        Err(SeedManifestError::Validation(_))
    ));
}

#[test]
fn strict_state_rejects_duplicate_identity_and_mixed_config_batches() {
    let row = PublishedManifestRow::replica_config(
        config_path("wg-1-team", "alpha", ".claude", "settings.json"),
        config_scope("wg-1-team", "alpha", ".claude"),
        ManifestSource::WorkspaceBase,
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap()
    .to_wire();
    let duplicate_wire = SeedManifestWire {
        schema_version: 1,
        coverage_version: 2,
        coverage: COVERAGE.map(str::to_string).to_vec(),
        files: vec![row.clone(), row.clone()],
    };
    assert!(matches!(
        ManifestState::from_wire(duplicate_wire),
        Err(SeedManifestError::Validation(_))
    ));

    let mut mixed = row;
    mixed.path = ".ac/wg-1-team/__agent_alpha/.claude/other.json".to_string();
    mixed.source = ManifestSource::MatrixBase;
    let mixed_wire = SeedManifestWire {
        schema_version: 1,
        coverage_version: 2,
        coverage: COVERAGE.map(str::to_string).to_vec(),
        files: vec![
            PublishedManifestRow::replica_config(
                config_path("wg-1-team", "alpha", ".claude", "settings.json"),
                config_scope("wg-1-team", "alpha", ".claude"),
                ManifestSource::WorkspaceBase,
                timestamp("2026-07-16T19:41:12.456Z"),
            )
            .unwrap()
            .to_wire(),
            mixed,
        ],
    };
    assert!(matches!(
        ManifestState::from_wire(mixed_wire),
        Err(SeedManifestError::Validation(_))
    ));

    let first = PublishedManifestRow::replica_config(
        config_path("wg-1-team", "alpha", ".claude", "settings.json"),
        config_scope("wg-1-team", "alpha", ".claude"),
        ManifestSource::WorkspaceBase,
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap()
    .to_wire();
    let mut different_time = first.clone();
    different_time.path = ".ac/wg-1-team/__agent_alpha/.claude/later.json".to_string();
    different_time.last_seeded_at = "2026-07-16T19:41:12.457Z".to_string();
    let mixed_time_wire = SeedManifestWire {
        schema_version: 1,
        coverage_version: 2,
        coverage: COVERAGE.map(str::to_string).to_vec(),
        files: vec![first, different_time],
    };
    assert!(matches!(
        ManifestState::from_wire(mixed_time_wire),
        Err(SeedManifestError::Validation(_))
    ));
}

#[test]
fn path_codecs_are_lossless_canonical_and_component_wise() {
    let readable = ManifestPathIdentity::parse(
        ManifestPathEncoding::Utf8,
        ".ac/wg-1-team/__agent_alpha/.claude/settings.json".to_string(),
    )
    .unwrap();
    assert_eq!(readable.encoding(), ManifestPathEncoding::Utf8);

    let mut unix_bytes = b".ac/wg-1-team/__agent_alpha/.claude/file".to_vec();
    unix_bytes.push(0xff);
    let unix = ManifestPathIdentity::parse(
        ManifestPathEncoding::UnixBytesHex,
        encode_lower_hex_bytes(&unix_bytes),
    )
    .unwrap();
    assert!(unix.starts_with_utf8_components(&[
        ".ac".to_string(),
        "wg-1-team".to_string(),
        "__agent_alpha".to_string(),
    ]));

    let mut windows_units = ".ac/wg-1-team/__agent_alpha/.claude/file"
        .encode_utf16()
        .collect::<Vec<_>>();
    windows_units.push(0xd800);
    let windows = ManifestPathIdentity::parse(
        ManifestPathEncoding::WindowsUtf16Hex,
        encode_lower_hex_u16(&windows_units),
    )
    .unwrap();
    assert!(windows.starts_with_utf8_components(&[
        ".ac".to_string(),
        "wg-1-team".to_string(),
        "__agent_alpha".to_string(),
    ]));
    assert_ne!(unix.key(), windows.key());

    let canonical_hex = encode_lower_hex_bytes(b".ac/readable");
    assert!(
        ManifestPathIdentity::parse(ManifestPathEncoding::UnixBytesHex, canonical_hex).is_err()
    );
    assert!(ManifestPathIdentity::parse(
        ManifestPathEncoding::UnixBytesHex,
        "2E6163ff".to_string()
    )
    .is_err());
    assert!(
        ManifestPathIdentity::parse(ManifestPathEncoding::Utf8, ".ac/../escape".to_string())
            .is_err()
    );
    assert!(
        ManifestPathIdentity::parse(ManifestPathEncoding::Utf8, "/.ac/absolute".to_string())
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn native_unix_invalid_byte_path_round_trips() {
    use std::os::unix::ffi::OsStringExt;

    let path = PathBuf::from(".ac").join(std::ffi::OsString::from_vec(vec![b'f', 0xff]));
    let identity = ManifestPathIdentity::from_relative_path(&path).unwrap();
    assert_eq!(identity.encoding(), ManifestPathEncoding::UnixBytesHex);
    let reparsed =
        ManifestPathIdentity::parse(identity.encoding(), identity.serialized().to_string())
            .unwrap();
    assert_eq!(identity, reparsed);
}

#[cfg(windows)]
#[test]
fn native_windows_unpaired_utf16_path_round_trips() {
    use std::os::windows::ffi::OsStringExt;

    let path = PathBuf::from(".ac").join(std::ffi::OsString::from_wide(&[u16::from(b'f'), 0xd800]));
    let identity = ManifestPathIdentity::from_relative_path(&path).unwrap();
    assert_eq!(identity.encoding(), ManifestPathEncoding::WindowsUtf16Hex);
    let reparsed =
        ManifestPathIdentity::parse(identity.encoding(), identity.serialized().to_string())
            .unwrap();
    assert_eq!(identity, reparsed);
}

#[test]
fn bounds_and_exact_outbound_counter_are_enforced() {
    let oversized = format!(".ac/{}", "x".repeat(MAX_FIELD_BYTES));
    assert!(matches!(
        ManifestPathIdentity::parse(ManifestPathEncoding::Utf8, oversized),
        Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::PathBytes,
            ..
        })
    ));

    let escaped = PublishedManifestRow::replica_config(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/wg-1-team/__agent_alpha/.claude/quote\"-slash\\-tab\t-del\u{7f}".to_string(),
        )
        .unwrap(),
        config_scope("wg-1-team", "alpha", ".claude"),
        ManifestSource::CatalogDefault,
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap();
    let mut state = ManifestState::default();
    let _ = state.upsert(escaped);
    let _ = state.upsert(
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    let serialized = serialize_state(&state).unwrap();
    assert!(!serialized.is_empty());
    assert_eq!(exact_serialized_len(&state).unwrap(), serialized.len());
    let output_limit = u64::try_from(serialized.len() - 1).unwrap();
    assert!(matches!(
        exact_serialized_len_with_limit(&state, output_limit),
        Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::OutputBytes,
            limit,
            ..
        }) if limit == output_limit
    ));

    let oversized_scope = format!("config:{}", "x".repeat(MAX_FIELD_BYTES));
    assert!(matches!(
        PublishedScopeBatch::new(
            oversized_scope,
            ManifestSource::WorkspaceBase,
            Vec::new(),
            timestamp("2026-07-16T19:41:12.456Z"),
        ),
        Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::ScopeBytes,
            ..
        })
    ));

    let mut total = usize::MAX;
    assert!(matches!(
        checked_add(&mut total, 1),
        Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::ArithmeticOverflow,
            ..
        })
    ));
}

#[test]
fn bounded_reader_rejects_the_first_extra_byte() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp.path(), b"12345").unwrap();
    let mut file = File::open(temp.path()).unwrap();
    let error = read_bounded_file(&mut file, 5, 4, temp.path()).unwrap_err();
    assert!(matches!(
        error,
        SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::InputBytes,
            limit: 4,
            observed_at_least: 5
        }
    ));
}
