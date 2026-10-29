use super::*;

#[test]
#[cfg(windows)]
fn strip_extended_prefix_converts_verbatim_unc() {
    assert_eq!(
        strip_extended_prefix(PathBuf::from(r"\\?\UNC\server\share\repo")),
        PathBuf::from(r"\\server\share\repo")
    );
}

#[test]
fn transition_inventory_locator_real_matrix_and_replica_readonly() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    let bytes = br#"{"identity":"../../_agent_dev-rust"}"#;
    std::fs::write(replica.join("config.json"), bytes).unwrap();
    let settings = settings_with_project(&project);
    for path in [&matrix, &replica] {
        let target = read_source_selection_target(&settings, path).unwrap();
        assert_eq!(target.root, std::fs::canonicalize(path).unwrap());
        assert_eq!(target.decisions, target.root.join("config.json"));
        assert_eq!(
            target.state,
            target
                .root
                .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
        );
        assert_eq!(
            target.reservation,
            target
                .root
                .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
        );
        assert_eq!(
            target.physical_revision[1],
            crate::config::settings::IdentityReferencePhysicalState::Absent
        );
        assert!(!target.state.exists());
        assert!(!target.reservation.exists());
    }
    assert!(!matrix.join("config.json").exists());
    assert_eq!(std::fs::read(replica.join("config.json")).unwrap(), bytes);
}

#[test]
fn transition_inventory_locator_rejects_unregistered_matrix_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let foreign = temp.path().join("foreign/.ac/_agent_dev-rust");
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    std::fs::create_dir_all(&foreign).unwrap();
    let settings = settings_with_project(&project);
    assert!(read_source_selection_target(&settings, &foreign).is_err());
    assert_eq!(std::fs::read_dir(&foreign).unwrap().count(), 0);
}

#[test]
fn transition_strict_discovery_proves_empty_and_detects_new_candidate_without_writes() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let settings = settings_with_project(&project);
    let (before, targets) = discover_source_inventory(&settings).unwrap();
    assert!(targets.is_empty());
    assert!(before.slots.contains(&(project.join(".ac"), false)));
    assert_eq!(std::fs::read_dir(&project).unwrap().count(), 0);
    std::fs::create_dir(project.join(".ac")).unwrap();
    let (after, targets) = discover_source_inventory(&settings).unwrap();
    assert!(targets.is_empty());
    assert_ne!(before, after);
    assert_eq!(std::fs::read_dir(project.join(".ac")).unwrap().count(), 0);
}

#[test]
fn transition_strict_discovery_refuses_invalid_candidate_and_missing_registry_base() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let settings = settings_with_project(&project);
    let candidate = project.join(".ac");
    std::fs::write(&candidate, b"external-not-directory").unwrap();
    assert!(discover_source_inventory(&settings).is_err());
    assert_eq!(
        std::fs::read(&candidate).unwrap(),
        b"external-not-directory"
    );
    assert!(
        discover_source_inventory(&settings_with_project(&root.path().join("missing"))).is_err()
    );
    assert!(!root.path().join("missing").exists());
}

#[derive(Default)]
struct SourceDiscoveryTestControls {
    calls: std::collections::BTreeMap<&'static str, usize>,
    failure: Option<(&'static str, usize)>,
}

impl SourceDiscoveryTestControls {
    fn cut(&mut self, stage: &'static str) -> std::io::Result<()> {
        *self.calls.entry(stage).or_default() += 1;
        if let Some((failure_stage, remaining)) = self.failure {
            if failure_stage == stage {
                if remaining == 1 {
                    self.failure = None;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "source inventory fail-only cut",
                    ));
                }
                self.failure = Some((failure_stage, remaining - 1));
            }
        }
        Ok(())
    }
}

#[test]
fn source_discovery_actual_iterator_type_and_directory_failures_never_prove_empty() {
    let root = crate::config::settings::source_transition_test_root();
    let project = root.path().join("P");
    let matrix = project.join(".ac/_agent_N");
    let nested = project.join("child/.ac/_agent_nested");
    let replica = project.join(".ac/room-7-dev-team/__agent_N");
    for path in [&matrix, &nested, &replica] {
        std::fs::create_dir_all(path).unwrap();
    }
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    let target = matrix.join("config.json");
    let bytes = br#"{"unknown":null}"#;
    std::fs::write(&target, bytes).unwrap();
    let replica_bytes = br#"{"identity":"../../_agent_N"}"#;
    std::fs::write(replica.join("config.json"), replica_bytes).unwrap();
    let mut controls = SourceDiscoveryTestControls::default();
    let independent = SourceDiscoveryTestControls::default();
    controls.failure = Some(("readDir", 1));
    assert!(independent.calls.is_empty());
    assert!(independent.failure.is_none());
    controls.failure = None;
    let baseline =
        super::super::source_selection::discover_source_inventory_with_cut(&settings, |stage| {
            controls.cut(stage)
        })
        .unwrap();
    assert_eq!(baseline.1.len(), 3);
    let counts = controls.calls.clone();
    for stage in ["readDir", "entry", "fileType"] {
        assert!(
            counts[stage] > 1,
            "fixture must reach multiple discovery loops"
        );
        for ordinal in 1..=counts[stage] {
            controls.failure = Some((stage, ordinal));
            let calls_before = controls.calls[stage];
            assert!(
                super::super::source_selection::discover_source_inventory_with_cut(
                    &settings,
                    |stage| controls.cut(stage),
                )
                .is_err(),
                "{stage}/{ordinal} silently became empty"
            );
            assert!(controls.failure.is_none(), "cut not reached");
            assert_eq!(controls.calls[stage] - calls_before, ordinal);
            assert_eq!(std::fs::read(&target).unwrap(), bytes);
            assert_eq!(
                std::fs::read(replica.join("config.json")).unwrap(),
                replica_bytes
            );
            let recovered = super::super::source_selection::discover_source_inventory_with_cut(
                &settings,
                |stage| controls.cut(stage),
            )
            .unwrap();
            assert_eq!(recovered.0, baseline.0);
            assert_eq!(recovered.1.len(), baseline.1.len());
        }
    }
}

#[test]
fn falls_back_to_nearest_lower_available_profile() {
    let settings = settings_with_cells(&[("codex", vec!["A", "C"])]);
    let resolved = resolve_profile(
        &settings,
        ProfileResolutionRequest {
            coding_agent_id: "codex",
            launch_path: None,
            agent_matrix_name: Some("dev-rust"),
            requested_profile: Some("D"),
            requested_profile_authoritative: false,
        },
    );

    assert_eq!(resolved.requested_profile, "D");
    assert_eq!(resolved.effective_profile, "C");
    assert!(resolved.fallback_applied);
    assert_eq!(resolved.fallback_chain, vec!["D", "C"]);
    assert_eq!(resolved.cell.command, "codex --c");
}

#[test]
fn synthesizes_a_cell_when_missing() {
    let settings = settings_with_cells(&[("codex", vec![])]);
    let resolved = resolve_profile(
        &settings,
        ProfileResolutionRequest {
            coding_agent_id: "codex",
            launch_path: None,
            agent_matrix_name: None,
            requested_profile: Some("B"),
            requested_profile_authoritative: false,
        },
    );

    assert_eq!(resolved.effective_profile, "A");
    assert!(resolved.cell.command.is_empty());
}

#[test]
fn instance_override_wins_over_explicit_request() {
    let temp = tempfile::tempdir().unwrap();
    let agent_dir = temp.path().join(".ac").join("_agent_dev-rust");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("config.json"),
        r#"{"tooling":{"instanceProfileOverride":"C"}}"#,
    )
    .unwrap();

    let settings = settings_with_cells(&[("codex", vec!["A", "B", "C"])]);
    let resolved = resolve_profile(
        &settings,
        ProfileResolutionRequest {
            coding_agent_id: "codex",
            launch_path: Some(&agent_dir),
            agent_matrix_name: None,
            requested_profile: Some("B"),
            requested_profile_authoritative: false,
        },
    );

    assert_eq!(resolved.requested_profile, "C");
    assert_eq!(resolved.effective_profile, "C");
}

#[test]
fn dispatch_authoritative_request_outranks_instance_override() {
    let temp = tempfile::tempdir().unwrap();
    let agent_dir = temp.path().join(".ac").join("_agent_dev-rust");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("config.json"),
        r#"{"tooling":{"instanceProfileOverride":"C"}}"#,
    )
    .unwrap();

    let settings = settings_with_cells(&[("codex", vec!["A", "B", "C"])]);
    // Historical ranking (picker, self-switch, restart, drift): the replica
    // pin wins over the explicit request.
    let pinned = resolve_profile(
        &settings,
        ProfileResolutionRequest {
            coding_agent_id: "codex",
            launch_path: Some(&agent_dir),
            agent_matrix_name: None,
            requested_profile: Some("B"),
            requested_profile_authoritative: false,
        },
    );
    assert_eq!(pinned.requested_profile, "C");
    assert_eq!(pinned.effective_profile, "C");

    // Wake dispatch: the explicit request outranks the pin for this spawn.
    let dispatched = resolve_profile(
        &settings,
        ProfileResolutionRequest {
            coding_agent_id: "codex",
            launch_path: Some(&agent_dir),
            agent_matrix_name: None,
            requested_profile: Some("B"),
            requested_profile_authoritative: true,
        },
    );
    assert_eq!(dispatched.requested_profile, "B");
    assert_eq!(dispatched.effective_profile, "B");
}

#[test]
fn selection_write_dual_writes_profile_and_preserves_last_coding_agent() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","tooling":{"lastCodingAgent":"claude"}}"#,
    )
    .unwrap();
    let settings = settings_with_project(&project);

    set_replica_coding_agent_selection(&settings, &replica, "codex", "b").unwrap();

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(replica.join("config.json")).unwrap())
            .unwrap();
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(saved["tooling"]["profile"], "B");
    assert_eq!(saved["tooling"]["instanceProfileOverride"], "B");
    assert_eq!(
        state_value(&replica)["tooling"]["lastCodingAgent"],
        "claude"
    );
    assert_untracked(&replica, &["lastCodingAgent"]);
}

#[test]
fn divergent_profile_and_legacy_override_returns_warning_and_prefers_v2() {
    let temp = tempfile::tempdir().unwrap();
    let agent_dir = temp.path().join("__agent_dev-rust");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("config.json"),
        r#"{"tooling":{"profile":"B","instanceProfileOverride":"C"}}"#,
    )
    .unwrap();

    let read = read_replica_profile_result(&agent_dir);

    assert_eq!(read.profile.as_deref(), Some("B"));
    assert!(read.warning.unwrap().contains("differs"));
}

#[test]
fn replica_origin_default_is_followed_when_identity_is_valid() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join("project").join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(
        matrix.join("config.json"),
        r#"{"tooling":{"defaultProfile":"B"}}"#,
    )
    .unwrap();
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust"}"#,
    )
    .unwrap();

    let settings = settings_with_cells(&[("codex", vec!["A", "B"])]);
    let resolved = resolve_profile(
        &settings,
        ProfileResolutionRequest {
            coding_agent_id: "codex",
            launch_path: Some(&replica),
            agent_matrix_name: None,
            requested_profile: None,
            requested_profile_authoritative: false,
        },
    );

    assert_eq!(resolved.requested_profile, "B");
    assert_eq!(resolved.effective_profile, "B");
}

#[test]
fn profile_selection_rejects_arbitrary_agent_dir_outside_ac_roots() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    let fake = temp.path().join("_agent_fake");
    std::fs::create_dir_all(&fake).unwrap();
    let settings = settings_with_project(&project);

    let err = validate_profile_selection_agent_path(&settings, &fake).unwrap_err();

    assert!(
        err.contains("Project AC Root") || err.contains("configured AC project roots"),
        "{err}"
    );
    assert!(!fake.join("config.json").exists());
}

#[test]
fn profile_selection_accepts_real_matrix_and_replica_paths() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust"}"#,
    )
    .unwrap();
    let settings = settings_with_project(&project);

    let matrix_validated = validate_profile_selection_agent_path(&settings, &matrix).unwrap();
    let replica_validated = validate_profile_selection_agent_path(&settings, &replica).unwrap();
    let matrix_canonical = canonical_existing_dir(&matrix, "Agent Matrix").unwrap();

    assert!(same_canonical_path(
        &matrix_validated.origin_matrix_dir,
        &matrix_canonical
    ));
    assert!(same_canonical_path(
        &replica_validated.origin_matrix_dir,
        &matrix_canonical
    ));
}

#[test]
fn profile_selection_rejects_matrix_outside_configured_project_paths() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let other_project = temp.path().join("other");
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    let fake = other_project.join(".ac").join("_agent_fake");
    std::fs::create_dir_all(&fake).unwrap();
    let settings = settings_with_project(&project);

    let err = validate_profile_selection_agent_path(&settings, &fake).unwrap_err();

    assert!(err.contains("configured AC project roots"), "{err}");
    assert!(!fake.join("config.json").exists());
}

// #592 - profile content-hash replica persistence.

#[test]
fn profile_content_hash_round_trips_and_preserves_profile() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    let replica = ac_root.join("wg-7-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","tooling":{"lastCodingAgent":"claude"}}"#,
    )
    .unwrap();
    let settings = settings_with_project(&project);

    // Assign a profile first (writes tooling.profile = "B").
    set_replica_coding_agent_selection(&settings, &replica, "codex", "b").unwrap();
    // Persist a content-hash.
    set_replica_profile_content_hash(&replica, "deadbeefdeadbeef").unwrap();

    assert_eq!(
        read_replica_profile_content_hash(&replica).as_deref(),
        Some("deadbeefdeadbeef")
    );
    // The profile assignment is untouched by the hash write.
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(replica.join("config.json")).unwrap())
            .unwrap();
    assert_eq!(saved["tooling"]["profile"], "B");
    assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
    assert_eq!(
        state_value(&replica)["tooling"]["profileContentHash"],
        "deadbeefdeadbeef"
    );
    assert_untracked(&replica, &["profileContentHash", "lastCodingAgent"]);
}

#[test]
fn profile_content_hash_write_is_noop_off_replica() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo-thing");
    std::fs::create_dir_all(&repo).unwrap();

    // A normal repo dir is not a replica/matrix: the write is a silent no-op
    // and must NOT create a config.json.
    assert!(set_replica_profile_content_hash(&repo, "x").is_ok());
    assert!(!repo.join("config.json").exists());
    assert_eq!(read_replica_profile_content_hash(&repo), None);
}

#[test]
fn profile_content_hash_write_persists_for_root_agent_dir() {
    let temp = tempfile::tempdir().unwrap();
    // The Root Agent dir name does NOT start with `__agent_`/`_agent_`, but it
    // is a legitimate replica running a coding agent: the hash must persist
    // and round-trip so drift survives an AC restart (#592 Root Agent fix).
    let root_agent = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root_agent).unwrap();

    set_replica_profile_content_hash(&root_agent, "cafef00dcafef00d").unwrap();

    assert_eq!(
        read_replica_profile_content_hash(&root_agent).as_deref(),
        Some("cafef00dcafef00d")
    );
    assert_eq!(
        state_value(&root_agent)["tooling"]["profileContentHash"],
        "cafef00dcafef00d"
    );
    assert_untracked(&root_agent, &["profileContentHash"]);
}

// ------------------------------------------------------------------
// #1939 replica selection state, intents, defaults and guarded writes.
// ------------------------------------------------------------------
