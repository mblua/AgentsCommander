use super::super::*;
use serde_json::json;

mod source_slots;

#[test]
fn p02_source_candidate_retains_locked_legacy_without_editing_untouched_groups() {
    let fixture = Fixture::new();
    let snapshot = fixture.snapshot();
    let (_, origin) = fixture.origin();
    let mut candidate = fixture.registered.clone();
    candidate["agents"][1]["baseCommand"] = json!("other-new");
    candidate["agents"][1]["command"] = json!("stale-caller");
    let prepared = prepare_source_candidate(
        &snapshot,
        &snapshot.snapshot.revision,
        &candidate,
        Some(&origin),
    )
    .unwrap();
    let saved: Value = serde_json::from_slice(&prepared.serialized_after).unwrap();
    assert_eq!(saved["agents"][1]["baseCommand"], "other-new");
    assert_eq!(saved["agents"][1]["command"], "other-base");
    assert_eq!(
        saved["agents"][0]["command"],
        fixture.registered["agents"][0]["command"]
    );
    assert_eq!(
        saved["agents"][0]["id"],
        fixture.registered["agents"][0]["id"]
    );
    assert_eq!(
        project_physical_profiles(&saved, true).unwrap()["codingAgentProfiles"]["profilesByAgent"],
        project_physical_profiles(&fixture.registered, true).unwrap()["codingAgentProfiles"]
            ["profilesByAgent"]
    );
    assert_eq!(saved["foreign"], fixture.registered["foreign"]);
    assert_eq!(
        std::fs::read(fixture.root.path().join(AGENTS_INSTANCE_FILE_NAME)).unwrap(),
        serde_json::to_vec(&fixture.registered).unwrap()
    );
}

struct Fixture {
    root: tempfile::TempDir,
    context: SourceReadContext,
    registered: Value,
    settings: Value,
}

impl Fixture {
    fn general_candidate(level: &str) -> Value {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        for key in SOURCE_GENERAL_PROTECTED {
            value.as_object_mut().unwrap().remove(*key);
        }
        value["logLevel"] = json!(level);
        value
    }
    fn new() -> Self {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/i2893");
        std::fs::create_dir_all(&scratch).unwrap();
        let root = tempfile::Builder::new()
            .prefix("source-")
            .tempdir_in(scratch)
            .unwrap();
        let settings = serde_json::to_value(AppSettings::default()).unwrap();
        let mut registered = json!({"agents":[
                {"id":"K", "label":"Own", "color":"#123456", "command":"own-base"},
                {"id":"J", "label":"Other", "color":"#123456", "command":"other-base"}
            ], "codingAgentProfiles":{"schemaVersion":2,"profilesByAgent":{"K":{
                "A":{"command":"--alpha", "env":{"MODE":"alpha"}},
                "B":{"command":"--beta", "env":{"MODE":"beta"}}
            }, "J":{"A":{"command":"--other"}}}}, "foreign":{"null":null,"array":[false,1,{"deep":"\
                keep"}]}});
        registered["codingAgentProfiles"]["profileLabelsByAgent"] = json!({"K":{"B":"Original"}});
        let mut settings = settings;
        settings["codingAgentProfiles"]["defaultProfileByAgent"] = json!({"N":"B","N-prime":"A"});
        // Disjunction requires existing map containers in the actual
        // pre-overlay view, not just leaf keys in the overlay payload.
        registered["codingAgentProfiles"]["defaultProfileByAgent"] = json!({"N":"B","N-prime":"A"});
        registered["codingAgentProfiles"]["identityByAgent"] =
            json!({"K":"legacy-k","J":"legacy-j"});
        std::fs::write(
            root.path().join(SETTINGS_FILE_NAME),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.path().join(AGENTS_INSTANCE_FILE_NAME),
            serde_json::to_vec(&registered).unwrap(),
        )
        .unwrap();
        let target = root.path().join("_agent_N");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(
            target.join("config.json"),
            serde_json::to_vec(&json!({"tooling":{
                "currentCodingAgent":"K", "profile":"B", "keep":"legacy"
            }}))
            .unwrap(),
        )
        .unwrap();
        let context = SourceReadContext::from_settings(
            &AppSettings::default(),
            &root.path().join(SETTINGS_FILE_NAME),
        )
        .unwrap();
        Self {
            root,
            context,
            registered,
            settings,
        }
    }

    fn overlay(&self, value: &Value) {
        std::fs::write(
            self.root.path().join(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }

    fn origin(&self) -> (OriginInputs, LegacyCompatibilityOrigin) {
        let inputs = OriginInputs::capture(&self.context).unwrap();
        let origin = LegacyCompatibilityOrigin::classify(&inputs);
        (inputs, origin)
    }

    fn snapshot(&self) -> RawSourceSnapshot {
        read_source_snapshot(
            &self.context,
            &self.context.source(SourceKind::RegisteredInstance),
        )
        .unwrap()
    }

    // Test-only instrumentation of the settings-base default consumer. The
    // counter records an actual default lookup handed to the existing pure
    // profile resolver, not a launch or a new production source consumer.
    fn consume_legacy_default(
        &self,
        origin: &LegacyCompatibilityOrigin,
        snapshot: &RawSourceSnapshot,
        expected_target: &[u8],
        consumed: &mut usize,
    ) -> Result<crate::config::coding_agent_profiles::ProfileResolution, SourcePreparationError>
    {
        use crate::config::coding_agent_profiles::{
            agent_name_from_dir, resolve_profile, ProfileResolutionRequest,
        };
        let target = std::fs::canonicalize(self.root.path().join("_agent_N/config.json"))
            .map_err(|_| SourcePreparationError::OriginUnavailable)?;
        let root = std::fs::canonicalize(self.root.path())
            .map_err(|_| SourcePreparationError::OriginUnavailable)?;
        if !target.starts_with(root)
            || std::fs::read(&target).ok().as_deref() != Some(expected_target)
        {
            return Err(SourcePreparationError::StaleRevision);
        }
        let matrix = agent_name_from_dir(target.parent().unwrap())
            .ok_or(SourcePreparationError::InvalidCandidate)?;
        let pair: Value = serde_json::from_slice(expected_target)
            .map_err(|_| SourcePreparationError::InvalidCandidate)?;
        let key = pair
            .pointer("/tooling/currentCodingAgent")
            .and_then(Value::as_str)
            .ok_or(SourcePreparationError::InvalidCandidate)?;
        if origin.binding.context != snapshot.context
            || origin.binding.registered.as_ref() != Some(&snapshot.snapshot.revision)
        {
            return Err(SourcePreparationError::StaleRevision);
        }
        origin.require_legacy_units(
            [
                LegacyOriginUnit::Entry(key.into()),
                LegacyOriginUnit::Default(matrix.clone()),
            ],
            None,
        )?;
        let default = origin.base["codingAgentProfiles"]["defaultProfileByAgent"]
            .get(&matrix)
            .and_then(Value::as_str)
            .and_then(normalize_profile_letter);
        let letter = match default {
            Some(letter) => {
                *consumed += 1; // Only a real valid Default(N) read is counted.
                letter
            }
            None => "A".into(),
        };
        origin.require_legacy_units(
            [LegacyOriginUnit::Profile(key.into(), letter.clone())],
            None,
        )?;
        let mut settings: AppSettings = serde_json::from_value(origin.base.clone())
            .map_err(|_| SourcePreparationError::InvalidCandidate)?;
        settings.agents = serde_json::from_value(json!(snapshot.snapshot.agents))
            .map_err(|_| SourcePreparationError::InvalidCandidate)?;
        settings.coding_agent_profiles.profiles_by_agent = snapshot.snapshot.profiles.clone();
        Ok(resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: key,
                launch_path: None,
                agent_matrix_name: Some(&matrix),
                requested_profile: Some(&letter),
                requested_profile_authoritative: false,
            },
        ))
    }

    fn assert_positive_default(&self) {
        let (inputs, origin) = self.origin();
        let snapshot = self.snapshot();
        let target = self.root.path().join("_agent_N/config.json");
        let before_target = std::fs::read(&target).unwrap();
        inputs.revalidate(&self.context).unwrap();
        let mut consumed_default = 0;
        let resolution = self
            .consume_legacy_default(&origin, &snapshot, &before_target, &mut consumed_default)
            .unwrap();
        assert!(is_positive_default_b(&resolution));
        assert_eq!(consumed_default, 1);
        let mut all_unresolved = origin.clone();
        all_unresolved.applied_paths.push(Vec::new());
        let mut rejected_consumptions = 0;
        assert!(self
            .consume_legacy_default(
                &all_unresolved,
                &snapshot,
                &before_target,
                &mut rejected_consumptions
            )
            .is_err());
        assert_eq!(rejected_consumptions, 0);
        assert_ne!(
            resolution.cell.command,
            snapshot.snapshot.profiles["K"]["A"].command
        );
        assert_eq!(snapshot.snapshot.agents[0]["command"], "own-base");
        let after_target = std::fs::read(&target).unwrap();
        assert_eq!(after_target, before_target);
        let pair: Value = serde_json::from_slice(&after_target).unwrap();
        assert_eq!(pair["tooling"]["currentCodingAgent"], "K");
        assert_eq!(pair["tooling"]["profile"], "B");
        assert!(pair["tooling"].get("configurationRef").is_none());
        assert!(pair.get("configurationRef").is_none());
        assert_eq!(
            snapshot.snapshot.source.kind,
            SourceKind::RegisteredInstance
        );
        assert!(!snapshot.snapshot.diagnostics.iter().any(|d| matches!(
            d.code.as_str(),
            "legacyOriginUnresolved" | "originUnavailable"
        )));
        inputs.revalidate(&self.context).unwrap();
    }
}

fn is_positive_default_b(
    resolution: &crate::config::coding_agent_profiles::ProfileResolution,
) -> bool {
    resolution.requested_profile == "B"
        && resolution.effective_profile == "B"
        && !resolution.fallback_applied
        && resolution.cell.command == "--beta"
        && resolution.cell.env == BTreeMap::from([("MODE".into(), "beta".into())])
        && resolution.warnings.is_empty()
}

#[test]
fn legacy_origin_contract_positive_oracle_detects_fallback_and_missing_default() {
    let fixture = Fixture::new();
    let target = std::fs::read(fixture.root.path().join("_agent_N/config.json")).unwrap();
    let mut raw = fixture.registered.clone();
    raw["codingAgentProfiles"]["profilesByAgent"]["K"]["B"]["enabled"] = json!(false);
    std::fs::write(
        fixture.root.path().join(AGENTS_INSTANCE_FILE_NAME),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    let mut consumed = 0;
    let fallback = fixture
        .consume_legacy_default(
            &fixture.origin().1,
            &fixture.snapshot(),
            &target,
            &mut consumed,
        )
        .unwrap();
    assert_eq!(consumed, 1);
    assert_eq!(fallback.requested_profile, "B");
    assert_eq!(fallback.effective_profile, "A");
    assert!(fallback.fallback_applied);
    assert!(!is_positive_default_b(&fallback));
    let mut base = fixture.settings.clone();
    base["codingAgentProfiles"]["defaultProfileByAgent"]
        .as_object_mut()
        .unwrap()
        .remove("N");
    std::fs::write(
        &fixture.context.settings_path,
        serde_json::to_vec(&base).unwrap(),
    )
    .unwrap();
    consumed = 0;
    let missing = fixture
        .consume_legacy_default(
            &fixture.origin().1,
            &fixture.snapshot(),
            &target,
            &mut consumed,
        )
        .unwrap();
    assert_eq!(consumed, 0);
    assert_eq!(missing.requested_profile, "A");
    assert!(!is_positive_default_b(&missing));
    crate::config::local_config_io::update_config_json_object(
        &fixture.root.path().join("_agent_N/config.json"),
        false,
        |object| {
            object.clear();
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        fixture.consume_legacy_default(
            &fixture.origin().1,
            &fixture.snapshot(),
            &target,
            &mut consumed
        ),
        Err(SourcePreparationError::StaleRevision)
    );
}

#[test]
fn legacy_origin_contract_base_default_no_overlay() {
    Fixture::new().assert_positive_default();
}

#[test]
fn legacy_origin_contract_base_default_other_matrix_overlay() {
    let fixture = Fixture::new();
    fixture.overlay(&json!({"codingAgentProfiles":{"defaultProfileByAgent":{"N-prime":"B"}}}));
    fixture.assert_positive_default();
    assert_eq!(
        fixture
            .origin()
            .1
            .require_disjoint([LegacyOriginUnit::Default("N-prime".into())]),
        Err(SourcePreparationError::LegacyOriginUnresolved)
    );
}

#[test]
fn legacy_origin_contract_labels_only_runtime_unchanged() {
    let fixture = Fixture::new();
    fixture
        .overlay(&json!({"codingAgentProfiles":{"profileLabelsByAgent":{"K":{"B":"Personal"}}}}));
    fixture.assert_positive_default();
    assert_eq!(
        fixture
            .origin()
            .1
            .require_disjoint([LegacyOriginUnit::Presentation("K".into(), "B".into())]),
        Err(SourcePreparationError::LegacyOriginUnresolved)
    );
}

#[test]
fn legacy_origin_contract_leaf_sibling_disjoint_and_base_delete_expansion() {
    let fixture = Fixture::new();
    fixture.overlay(
        &json!({"codingAgentProfiles":{"profilesByAgent":{"K":{"B":{"command":"--overlay"}}}}}),
    );
    let (_, origin) = fixture.origin();
    origin
        .require_disjoint([
            LegacyOriginUnit::Profile("K".into(), "A".into()),
            LegacyOriginUnit::Entry("J".into()),
        ])
        .unwrap();
    assert_eq!(
        origin.require_disjoint([LegacyOriginUnit::Profile("K".into(), "B".into())]),
        Err(SourcePreparationError::LegacyOriginUnresolved)
    );
    let snapshot = fixture.snapshot();
    let mut candidate = fixture.registered.clone();
    candidate["agents"][1]["command"] = json!("other-new");
    let plan = prepare_source_candidate(
        &snapshot,
        &snapshot.snapshot.revision,
        &candidate,
        Some(&origin),
    )
    .unwrap();
    let saved: Value = serde_json::from_slice(&plan.serialized_after).unwrap();
    assert_eq!(saved["foreign"], fixture.registered["foreign"]);
    assert_eq!(
        project_physical_profiles(&saved, true).unwrap()["codingAgentProfiles"]["profilesByAgent"]
            ["K"],
        project_physical_profiles(&fixture.registered, true).unwrap()["codingAgentProfiles"]
            ["profilesByAgent"]["K"]
    );
    candidate = fixture.registered.clone();
    candidate["agents"][0]["command"] = json!("own-new");
    assert!(matches!(
        prepare_source_candidate(
            &snapshot,
            &snapshot.snapshot.revision,
            &candidate,
            Some(&origin)
        ),
        Err(SourcePreparationError::LegacyOriginUnresolved)
    ));
    candidate["agents"].as_array_mut().unwrap().remove(0);
    assert!(matches!(
        prepare_source_candidate(
            &snapshot,
            &snapshot.snapshot.revision,
            &candidate,
            Some(&origin)
        ),
        Err(SourcePreparationError::LegacyOriginUnresolved)
    ));
}

#[test]
fn p02_source_aware_duplicate_candidate_refuses_without_publication() {
    let fixture = Fixture::new();
    let (_, origin) = fixture.origin();
    let snapshot = fixture.snapshot();
    let mut candidate = fixture.registered.clone();
    let duplicate = candidate["agents"][0].clone();
    candidate["agents"].as_array_mut().unwrap().push(duplicate);
    let path = fixture.root.path().join(AGENTS_INSTANCE_FILE_NAME);
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        prepare_source_candidate(
            &snapshot,
            &snapshot.snapshot.revision,
            &candidate,
            Some(&origin),
        ),
        Err(SourcePreparationError::InvalidCandidate)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn legacy_origin_contract_agents_equal_replacement_and_omitted_new_union() {
    let fixture = Fixture::new();
    for agents in [
        fixture.registered["agents"].clone(),
        json!([]),
        json!([{"id":"new", "label":"New", "color":"red", "command":"new"}]),
    ] {
        fixture.overlay(&json!({"agents":agents}));
        let (_, origin) = fixture.origin();
        for key in ["K", "J"] {
            assert_eq!(
                origin.require_disjoint([LegacyOriginUnit::Entry(key.into())]),
                Err(SourcePreparationError::LegacyOriginUnresolved)
            );
            assert_eq!(
                origin.require_disjoint([LegacyOriginUnit::Profile(key.into(), "A".into())]),
                Err(SourcePreparationError::LegacyOriginUnresolved)
            );
        }
    }
}

#[test]
fn legacy_origin_contract_missing_duplicate_nonstring_ids_unknown() {
    let fixture = Fixture::new();
    for agents in [
        json!([{"label":"Missing", "command":"own", "color":"red"}]),
        json!([{"id":1}]),
        json!([{"id":"K"},{"id":"K"}]),
    ] {
        let mut inputs = fixture.origin().0;
        inputs.registered =
            OriginPhysicalInput::Bytes(serde_json::to_vec(&json!({"agents":agents})).unwrap());
        let origin = LegacyCompatibilityOrigin::classify(&inputs);
        assert_eq!(
            origin.require_disjoint([LegacyOriginUnit::Entry("J".into())]),
            Err(SourcePreparationError::OriginUnavailable)
        );
    }
}

#[test]
fn legacy_origin_contract_identity_orphan_and_ancestor_nonobject() {
    let fixture = Fixture::new();
    fixture.overlay(
        &json!({"codingAgentProfiles":{"identityByAgent":{"K":"not-a-proof"},"profilesB\
        yAgent":{"orphan":{"B":{"command":"x"}}}}}),
    );
    let (_, origin) = fixture.origin();
    assert_eq!(
        origin.require_disjoint([LegacyOriginUnit::Profile("K".into(), "A".into())]),
        Err(SourcePreparationError::LegacyOriginUnresolved)
    );
    assert_eq!(
        origin.require_disjoint([LegacyOriginUnit::Profile("orphan".into(), "B".into())]),
        Err(SourcePreparationError::LegacyOriginUnresolved)
    );
    origin
        .require_disjoint([LegacyOriginUnit::Profile("J".into(), "A".into())])
        .unwrap();
    let mut inputs = fixture.origin().0;
    inputs.registered = OriginPhysicalInput::Absent;
    let mut base = fixture.settings.clone();
    base["codingAgentProfiles"] = Value::Null;
    inputs.settings = OriginPhysicalInput::Bytes(serde_json::to_vec(&base).unwrap());
    inputs.overlay = OriginPhysicalInput::Bytes(
        serde_json::to_vec(
            &json!({"codingAgentProfiles":{"profilesByAgent":{"K":{"A":{"command":"x"}}}}}),
        )
        .unwrap(),
    );
    let origin = LegacyCompatibilityOrigin::classify(&inputs);
    assert!(origin
        .applied_paths
        .iter()
        .any(|p| p == &vec!["codingAgentProfiles".to_string()]));
}

#[test]
fn legacy_origin_contract_absent_map_ancestor_is_not_disjoint() {
    let fixture = Fixture::new();
    let mut raw = fixture.registered.clone();
    raw["codingAgentProfiles"]
        .as_object_mut()
        .unwrap()
        .remove("defaultProfileByAgent");
    raw["codingAgentProfiles"]
        .as_object_mut()
        .unwrap()
        .remove("identityByAgent");
    std::fs::write(
        fixture.root.path().join(AGENTS_INSTANCE_FILE_NAME),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    fixture.overlay(
        &json!({"codingAgentProfiles":{"defaultProfileByAgent":{"N-prime":"B"}, "identi\
        tyByAgent":{"K":"personal"}}}),
    );
    let (_, origin) = fixture.origin();
    assert!(origin.applied_paths.contains(&vec![
        "codingAgentProfiles".into(),
        "defaultProfileByAgent".into()
    ]));
    assert!(origin.applied_paths.contains(&vec![
        "codingAgentProfiles".into(),
        "identityByAgent".into()
    ]));
    assert_eq!(
        origin.require_disjoint([LegacyOriginUnit::Default("N".into())]),
        Err(SourcePreparationError::LegacyOriginUnresolved)
    );
    assert_eq!(
        origin.require_disjoint([LegacyOriginUnit::Profile("J".into(), "A".into())]),
        Err(SourcePreparationError::LegacyOriginUnresolved)
    );
}

#[test]
fn legacy_origin_contract_absent_empty_rejected_known_and_unknown() {
    let fixture = Fixture::new();
    for bytes in [b"{}".as_slice(), b"not JSON", b"[]", b"{\"agents\":null}"] {
        std::fs::write(
            fixture.root.path().join(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
            bytes,
        )
        .unwrap();
        fixture.assert_positive_default();
        let (_, origin) = fixture.origin();
        assert!(origin.applied_paths.is_empty());
    }
    let mut inputs = fixture.origin().0;
    inputs.overlay = OriginPhysicalInput::Unknown;
    inputs.binding.overlay = None;
    assert_eq!(
        LegacyCompatibilityOrigin::classify(&inputs)
            .require_disjoint([LegacyOriginUnit::Default("N".into())]),
        Err(SourcePreparationError::OriginUnavailable)
    );
}

#[test]
fn legacy_origin_contract_shared_units_preserve_distinct_default_groups_and_order() {
    let fixture = Fixture::new();
    fixture.overlay(&json!({"codingAgentProfiles":{"defaultProfileByAgent":{"N":"B","M":"A"}}}));
    let (_, origin) = fixture.origin();
    let snapshot = fixture.snapshot();
    let (agents, profiles) = decode_registered_raw(&fixture.registered).unwrap();
    let mut proofs = Vec::new();
    for (matrix, letter) in [("N", "B"), ("M", "A")] {
        let selection = ConfigurationRef {
            version: 1,
            source: snapshot.snapshot.source.clone(),
            calculated_id: configuration_identity(&agents[0].command, &agents[0].envs),
            requested_profile: Some(letter.into()),
            profile_calculated_id: Some(profile_identity(&profiles.profiles_by_agent["K"][letter])),
            legacy_entry_hint: Some("K".into()),
        };
        let bytes = serde_json::to_vec(
            &json!({"tooling":{"replicaSelectionDefault":{"configurationRef":selection}}}),
        )
        .unwrap();
        proofs.push(
            origin
                .bind_original_selection(
                    &snapshot,
                    &selection,
                    &snapshot.snapshot.revision,
                    &snapshot.snapshot.context_revision,
                    &bytes,
                    &bytes,
                    Some(matrix),
                )
                .unwrap(),
        );
    }
    let units = [LegacyOriginUnit::Entry("K".into())].into_iter().collect();
    let aggregate = SourceOriginalProofAggregate {
        originals: proofs.clone(),
    };
    require_source_original_units(&origin, &units, Some(&aggregate)).unwrap();
    proofs.reverse();
    require_source_original_units(
        &origin,
        &units,
        Some(&SourceOriginalProofAggregate {
            originals: proofs.clone(),
        }),
    )
    .unwrap();
    // Keep the real M group covered. Add only a negative clone claiming N
    // with M's different profile, so N coherence is the sole violated rule.
    let mut conflicting_n = proofs[0].clone();
    conflicting_n
        .units
        .remove(&LegacyOriginUnit::Default("M".into()));
    conflicting_n
        .units
        .insert(LegacyOriginUnit::Default("N".into()));
    proofs.push(conflicting_n);
    let m = LegacyOriginUnit::Default("M".into());
    assert!(proofs.iter().any(|proof| proof.units.contains(&m)));
    let n = LegacyOriginUnit::Default("N".into());
    let n_groups: Vec<_> = proofs
        .iter()
        .filter(|proof| proof.units.contains(&n))
        .collect();
    assert_eq!(n_groups.len(), 2);
    assert_ne!(n_groups[0].units, n_groups[1].units);
    origin
        .require_legacy_units(
            [m.clone()],
            proofs.iter().find(|proof| proof.units.contains(&m)),
        )
        .unwrap();
    for reverse in [false, true] {
        if reverse {
            proofs.reverse();
        }
        assert_eq!(
            require_source_original_units(
                &origin,
                &units,
                Some(&SourceOriginalProofAggregate {
                    originals: proofs.clone()
                })
            ),
            Err(SourcePreparationError::LegacyOriginUnresolved)
        );
    }
}

#[test]
fn legacy_origin_contract_default_matrix_domain_requires_original_link() {
    let fixture = Fixture::new();
    for matrix in ["N", "K"] {
        fixture.overlay(&json!({"codingAgentProfiles":{"defaultProfileByAgent":{matrix:"B"}}}));
        let (_, origin) = fixture.origin();
        assert_eq!(
            origin.require_legacy_units([LegacyOriginUnit::Default(matrix.into())], None),
            Err(SourcePreparationError::LegacyOriginUnresolved)
        );
        let snapshot = fixture.snapshot();
        let (agents, profiles) = decode_registered_raw(&fixture.registered).unwrap();
        let selection = ConfigurationRef {
            version: 1,
            source: snapshot.snapshot.source.clone(),
            calculated_id: configuration_identity(&agents[0].command, &agents[0].envs),
            requested_profile: Some("B".into()),
            profile_calculated_id: Some(profile_identity(&profiles.profiles_by_agent["K"]["B"])),
            legacy_entry_hint: Some("K".into()),
        };
        let bytes = serde_json::to_vec(
            &json!({"tooling":{"replicaSelectionDefault":{"configurationRef":selection}}}),
        )
        .unwrap();
        let proof = origin
            .bind_original_selection(
                &snapshot,
                &selection,
                &snapshot.snapshot.revision,
                &snapshot.snapshot.context_revision,
                &bytes,
                &bytes,
                Some(matrix),
            )
            .unwrap();
        origin
            .require_legacy_units([LegacyOriginUnit::Default(matrix.into())], Some(&proof))
            .unwrap();
        assert!(origin
            .bind_original_selection(
                &snapshot,
                &selection,
                &snapshot.snapshot.revision,
                &snapshot.snapshot.context_revision,
                b"{}",
                &bytes,
                Some(matrix)
            )
            .is_err());
    }
}

#[test]
fn legacy_origin_contract_complete_binding_drift_with_stable_raw30() {
    let fixture = Fixture::new();
    let (inputs, _) = fixture.origin();
    inputs.revalidate(&fixture.context).unwrap();
    fixture.overlay(&json!({"logLevel":"debug"}));
    assert_eq!(
        inputs.revalidate(&fixture.context),
        Err(SourcePreparationError::StaleRevision)
    );
    let (inputs, _) = fixture.origin();
    let mut base = fixture.settings.clone();
    base["logLevel"] = json!("trace");
    std::fs::write(
        fixture.root.path().join(SETTINGS_FILE_NAME),
        serde_json::to_vec(&base).unwrap(),
    )
    .unwrap();
    assert_eq!(
        inputs.revalidate(&fixture.context),
        Err(SourcePreparationError::StaleRevision)
    );
    let mut changed = fixture.context.clone();
    changed.revision.push('x');
    assert_eq!(
        inputs.revalidate(&changed),
        Err(SourcePreparationError::ContextChanged)
    );
}

#[test]
fn legacy_origin_contract_general_only_preserves_absent_null_value_and_source() {
    let fixture = Fixture::new();
    fixture.overlay(
        &json!({"codingAgentProfiles":{"profilesByAgent":{"K":{"B":{"command":"personal"}}}}}),
    );
    for placement in [
        None,
        Some(Value::Null),
        Some(json!({"x":1,"y":2,"width":800,"height":600})),
    ] {
        let mut base = fixture.settings.clone();
        base.as_object_mut().unwrap().remove("mainGeometry");
        if let Some(value) = &placement {
            base["mainGeometry"] = value.clone();
        }
        base["foreign"] = json!({"absentSibling":null, "value":[1,2]});
        std::fs::write(
            fixture.root.path().join(SETTINGS_FILE_NAME),
            serde_json::to_vec(&base).unwrap(),
        )
        .unwrap();
        let before = std::fs::read(fixture.root.path().join(AGENTS_INSTANCE_FILE_NAME)).unwrap();
        let inputs = fixture.origin().0;
        let plan =
            prepare_settings_without_source_write(&inputs, &Fixture::general_candidate("debug"))
                .unwrap();
        let after: Value = serde_json::from_slice(&plan.serialized_after).unwrap();
        assert_eq!(after.get("mainGeometry"), placement.as_ref());
        assert_eq!(after["foreign"], base["foreign"]);
        assert_eq!(after["codingAgentProfiles"], base["codingAgentProfiles"]);
        assert_eq!(plan.redecoded_settings.agents[0].command, "own-base");
        assert_eq!(
            std::fs::read(fixture.root.path().join(AGENTS_INSTANCE_FILE_NAME)).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read(fixture.root.path().join(SETTINGS_FILE_NAME)).unwrap(),
            serde_json::to_vec(&base).unwrap()
        );
    }
}

fn assert_general_overlay_keeps_disk_value(disk_value: Option<Value>) {
    let fixture = Fixture::new();
    let mut base = fixture.settings.clone();
    base.as_object_mut().unwrap().remove("logLevel");
    if let Some(value) = &disk_value {
        base["logLevel"] = value.clone();
    }
    std::fs::write(
        fixture.context.settings_path.clone(),
        serde_json::to_vec(&base).unwrap(),
    )
    .unwrap();
    fixture.overlay(&json!({"logLevel":"debug"}));
    let inputs = fixture.origin().0;
    let plan = prepare_settings_without_source_write(&inputs, &Fixture::general_candidate("debug"))
        .unwrap();
    let after: Value = serde_json::from_slice(&plan.serialized_after).unwrap();
    assert_eq!(after.get("logLevel"), disk_value.as_ref());
    assert_eq!(plan.redecoded_settings.log_level.as_deref(), Some("debug"));
    assert_eq!(after["codingAgentProfiles"], base["codingAgentProfiles"]);
    // All three physical inputs, including the source, remain byte-identical.
    inputs.revalidate(&fixture.context).unwrap();
}

#[test]
fn legacy_origin_contract_general_overlay_base_present() {
    assert_general_overlay_keeps_disk_value(Some(json!("info")));
}

#[test]
fn legacy_origin_contract_general_overlay_base_absent() {
    assert_general_overlay_keeps_disk_value(None);
}

#[test]
fn legacy_origin_contract_general_overlay_base_null() {
    assert_general_overlay_keeps_disk_value(Some(Value::Null));
}
