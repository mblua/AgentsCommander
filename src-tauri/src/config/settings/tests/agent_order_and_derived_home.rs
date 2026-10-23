use super::*;

/// #2064 - the six remote-activity dials are additive: an older settings.json
/// without them must deserialize to the documented defaults, and a serialized
/// default must carry all six keys (guarding against an accidental
/// `skip_serializing_if`).
#[test]
fn remote_activity_dials_round_trip_through_serde() {
    const KEYS: [&str; 6] = [
        "ciActivityEnabled",
        "ciActivityNotifyOrchestrator",
        "branchStalenessEnabled",
        "branchStalenessNotifyOrchestrator",
        "ciSweepMinIntervalSecs",
        "branchStalenessIntervalSecs",
    ];

    let defaults = AppSettings::default();
    let value = serde_json::to_value(&defaults).expect("serialize default AppSettings");
    for key in KEYS {
        assert!(
            value.get(key).is_some(),
            "{key} must be serialized, never skipped"
        );
    }

    let mut stripped = value.clone();
    let object = stripped.as_object_mut().expect("settings object");
    for key in KEYS {
        object.remove(key);
    }
    let back: AppSettings =
        serde_json::from_value(stripped).expect("deserialize without the six keys");
    assert!(back.ci_activity_enabled);
    assert!(back.ci_activity_notify_orchestrator);
    assert!(back.branch_staleness_enabled);
    assert!(back.branch_staleness_notify_orchestrator);
    assert_eq!(back.ci_sweep_min_interval_secs, 30);
    assert_eq!(back.branch_staleness_interval_secs, 260);

    let round_trip: AppSettings = serde_json::from_value(value).expect("round trip");
    assert_eq!(round_trip.ci_activity_enabled, defaults.ci_activity_enabled);
    assert_eq!(
        round_trip.ci_activity_notify_orchestrator,
        defaults.ci_activity_notify_orchestrator
    );
    assert_eq!(
        round_trip.branch_staleness_enabled,
        defaults.branch_staleness_enabled
    );
    assert_eq!(
        round_trip.branch_staleness_notify_orchestrator,
        defaults.branch_staleness_notify_orchestrator
    );
    assert_eq!(
        round_trip.ci_sweep_min_interval_secs,
        defaults.ci_sweep_min_interval_secs
    );
    assert_eq!(
        round_trip.branch_staleness_interval_secs,
        defaults.branch_staleness_interval_secs
    );
}

// ─────────────────────────────────────────────────────────────────────────
// #2306 P1 - registered-agent explicit order model.
// ─────────────────────────────────────────────────────────────────────────

fn order_2306_decode_settings(agents: serde_json::Value) -> AppSettings {
    // `defaultShell`, `defaultShellArgs` and `agents` are the three fields
    // `AppSettings` has no serde default for.
    serde_json::from_value(serde_json::json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "agents": agents,
    }))
    .expect("settings decode")
}

fn order_2306_agent(id: &str, order: Option<serde_json::Value>) -> serde_json::Value {
    let mut agent = serde_json::json!({
        "id": id,
        "label": id.to_uppercase(),
        "command": "claude",
        "color": "#000000",
    });
    if let Some(order) = order {
        agent["order"] = order;
    }
    agent
}

fn order_2306_ids(agents: &[AgentConfig]) -> Vec<&str> {
    agents.iter().map(|agent| agent.id.as_str()).collect()
}

fn order_2306_positions(agents: &[AgentConfig]) -> Vec<Option<u32>> {
    agents.iter().map(|agent| agent.order).collect()
}

#[test]
fn agent_order_mixed_missing_duplicate_and_out_of_range_is_deterministic() {
    // Keys: a=(2,0), b=(1,1) [absent uses its index], c=(0,2), d=(0,3),
    // e=(99,4). Sorted by (key, original index): c,d,b,a,e.
    let mut settings = order_2306_decode_settings(serde_json::json!([
        order_2306_agent("a", Some(serde_json::json!(2))),
        order_2306_agent("b", None),
        order_2306_agent("c", Some(serde_json::json!(0))),
        order_2306_agent("d", Some(serde_json::json!(0))),
        order_2306_agent("e", Some(serde_json::json!(99))),
    ]));
    super::super::finalize_agent_order(&mut settings.agents);
    assert_eq!(order_2306_ids(&settings.agents), ["c", "d", "b", "a", "e"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );

    // Idempotent: a second finalize cannot re-sort the now-valid ordinals.
    super::super::finalize_agent_order(&mut settings.agents);
    assert_eq!(order_2306_ids(&settings.agents), ["c", "d", "b", "a", "e"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );
}

#[test]
fn agent_order_absent_keeps_legacy_vector_order() {
    let mut settings = order_2306_decode_settings(serde_json::json!([
        order_2306_agent("a", None),
        order_2306_agent("b", None),
        order_2306_agent("c", None),
    ]));
    super::super::finalize_agent_order(&mut settings.agents);
    assert_eq!(order_2306_ids(&settings.agents), ["a", "b", "c"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1), Some(2)]
    );
}

#[test]
fn agent_order_invalid_values_decode_as_absent_without_failing_the_file() {
    for raw in [
        serde_json::json!(null),
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!("3"),
        serde_json::json!(true),
        serde_json::json!(u64::from(u32::MAX) + 1),
    ] {
        let mut settings = order_2306_decode_settings(serde_json::json!([order_2306_agent(
            "a",
            Some(raw.clone())
        ),]));
        assert_eq!(settings.agents[0].order, None, "raw order {raw}");
        super::super::finalize_agent_order(&mut settings.agents);
        assert_eq!(settings.agents[0].order, Some(0), "raw order {raw}");
    }

    // The boundary value still decodes as a real ordinal.
    let settings = order_2306_decode_settings(serde_json::json!([order_2306_agent("a", None)]));
    assert_eq!(settings.agents[0].order, None);
    let settings = order_2306_decode_settings(serde_json::json!([{
        "id": "a", "label": "A", "command": "claude", "color": "#000000",
        "order": u32::MAX,
    }]));
    assert_eq!(settings.agents[0].order, Some(u32::MAX));
}

#[test]
fn agent_order_legacy_load_writes_nothing_and_next_save_persists_positions() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    // `order: -1` is invalid and stays on disk until an authorized save; `b`
    // has no ordinal at all. Complete A-profile cells keep the loader from
    // taking any unrelated repair/root-token save path, so a write here can
    // only come from the order code under test.
    std::fs::write(
        &path,
        r##"{
                "defaultShell": "test-shell",
                "defaultShellArgs": [],
                "rootToken": "base-token",
                "agents": [
                    {"id":"a","label":"A","command":"claude","color":"#000000","order":-1},
                    {"id":"b","label":"B","command":"codex","color":"#000000"}
                ],
                "codingAgentProfiles": {
                    "schemaVersion": 2,
                    "profileSlots": {"A": {"label": ""}},
                    "defaultProfileByAgent": {},
                    "profilesByAgent": {
                        "a": {"A": {"enabled": true, "command": "", "env": {}, "notes": "cell-a"}},
                        "b": {"A": {"enabled": true, "command": "", "env": {}, "notes": "cell-b"}}
                    },
                    "profileLabelsByAgent": {"a": {"A": "Alpha"}}
                }
            }"##,
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();

    let settings = super::super::load_settings_from_path(&path);
    assert_eq!(order_2306_ids(&settings.agents), ["a", "b"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1)],
        "absent and invalid ordinals normalize to the array position"
    );
    // A/B profile cells and labels survive reordering.
    assert_eq!(
        settings.coding_agent_profiles.profiles_by_agent["a"]["A"].notes,
        "cell-a"
    );
    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["a"]["A"],
        "Alpha"
    );
    // Loading writes nothing beyond the one-shot export of #2716 (B3) 4.3:
    // both groups move, every other byte stays, and a second load writes none.
    b3_assert_split_matches_seed(&path, &before, &["order"]);
    let sidecar = b3_object(&super::super::agents_instance_path(&path));
    assert_eq!(sidecar["agents"][0]["order"], 0);
    assert_eq!(sidecar["agents"][1]["order"], 1);
    let exported = std::fs::read(&path).unwrap();
    let exported_sidecar = std::fs::read(super::super::agents_instance_path(&path)).unwrap();
    let _ = super::super::load_settings_from_path(&path);
    assert_eq!(std::fs::read(&path).unwrap(), exported);
    assert_eq!(
        std::fs::read(super::super::agents_instance_path(&path)).unwrap(),
        exported_sidecar
    );
    // #2716 (B3) r12l: the one-shot export rotates exactly once, and slot 1
    // keeps the legacy file byte for byte.
    assert_eq!(
        std::fs::read(super::super::settings_backup_path(&path, 1)).expect("slot 1"),
        before,
        "slot 1 must hold the seeded bytes"
    );
    assert!(
        !super::super::settings_backup_path(&path, 2).exists(),
        "a second save rotated the settings backup again"
    );

    let written =
        super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(super::super::agents_instance_path(&path)).unwrap())
            .unwrap();
    assert_eq!(disk["agents"][0]["id"], "a");
    assert_eq!(disk["agents"][0]["order"], 0);
    assert_eq!(disk["agents"][1]["id"], "b");
    assert_eq!(disk["agents"][1]["order"], 1);
    assert_eq!(order_2306_positions(&written.agents), [Some(0), Some(1)]);

    // Second reload preserves the written explicit positions.
    let reloaded = super::super::load_settings_from_path(&path);
    assert_eq!(order_2306_ids(&reloaded.agents), ["a", "b"]);
    assert_eq!(order_2306_positions(&reloaded.agents), [Some(0), Some(1)]);
}

#[test]
fn agent_order_unknown_fields_follow_existing_writer_behavior() {
    let mut settings = order_2306_decode_settings(serde_json::json!([{
        "id": "a", "label": "A", "command": "claude", "color": "#000000",
        "futureField": 7,
    }]));
    assert_eq!(settings.agents.len(), 1);
    super::super::finalize_agent_order(&mut settings.agents);
    let object = serde_json::to_value(&settings).unwrap();
    assert!(
        object["agents"][0].get("futureField").is_none(),
        "the typed writer drops unknown fields as before: {object}"
    );
    assert_eq!(object["agents"][0]["order"], 0);
}

#[test]
fn agent_order_terminal_snapshot_cas_returns_and_installs_normalized_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        r##"{
                "defaultShell": "test-shell",
                "defaultShellArgs": [],
                "rootToken": "base-token",
                "terminalSnapshotsEnabled": false,
                "agents": [
                    {"id":"a","label":"A","command":"claude","color":"#000000","order":1},
                    {"id":"b","label":"B","command":"claude","color":"#000000"},
                    {"id":"c","label":"C","command":"claude","color":"#000000","order":0},
                    {"id":"d","label":"D","command":"claude","color":"#000000","order":0}
                ]
            }"##,
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    let current = AppSettings::default();

    // disk gate == enabled: the no-op CAS returns the finalized, normalized view
    // with no write and no backup rotation.
    let no_op = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
        &current, &path, false, false,
    )
    .unwrap();
    assert_eq!(order_2306_ids(&no_op.agents), ["c", "d", "a", "b"]);
    assert_eq!(
        order_2306_positions(&no_op.agents),
        [Some(0), Some(1), Some(2), Some(3)]
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the no-op CAS writes nothing"
    );

    // Direct CAS write: the file itself carries contiguous explicit ordinals.
    super::super::compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, true)
        .unwrap();
    let disk: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk["terminalSnapshotsEnabled"], true);
    let sidecar = b3_object(&super::super::agents_instance_path(&path));
    let agents = sidecar["agents"].as_array().unwrap();
    let ids: Vec<&str> = agents.iter().map(|a| a["id"].as_str().unwrap()).collect();
    let orders: Vec<u64> = agents
        .iter()
        .map(|a| a["order"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, ["c", "d", "a", "b"]);
    assert_eq!(orders, [0, 1, 2, 3]);
}

#[test]
fn agent_order_overlay_owns_agents_in_memory_and_leaves_both_files_base_owned() {
    use local_overlay_1737::{base_fixture, disk_object, seed};
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = serde_json::json!([
        {"id":"base-a","label":"Base A","command":"claude","color":"#111111","order":0},
        {"id":"base-b","label":"Base B","command":"claude","color":"#222222","order":1}
    ]);
    let local = serde_json::json!({
        "agents": [
            {"id":"ov-a","label":"Overlay A","command":"claude","color":"#aaaaaa","order":1},
            {"id":"ov-b","label":"Overlay B","command":"claude","color":"#bbbbbb"},
            {"id":"ov-c","label":"Overlay C","command":"claude","color":"#cccccc","order":0}
        ]
    });
    let path = seed(temp.path(), Some(&base), Some(&local));

    let mut settings = super::super::load_settings_from_path(&path);
    assert!(settings.local_overlay_state.owns_top_level("agents"));
    // Overlay numbers win; the missing value falls back to its overlay index.
    assert_eq!(order_2306_ids(&settings.agents), ["ov-c", "ov-a", "ov-b"]);
    assert_eq!(
        order_2306_positions(&settings.agents),
        [Some(0), Some(1), Some(2)]
    );

    // The CLI loaders share `parse_settings_json` with the GUI loader.
    let contents = std::fs::read_to_string(&path).unwrap();
    let (cli_view, _) = super::super::parse_settings_json(&contents, "test", Some(&path)).unwrap();
    assert_eq!(order_2306_ids(&cli_view.agents), ["ov-c", "ov-a", "ov-b"]);

    // A non-order save restores the base `agents` array to disk byte-for-byte,
    // keeps the effective overlay order only in memory, and never writes the
    // local file.
    let local_path = temp.path().join("settings.50.personal.no-git.json");
    let overlay_before = std::fs::read(&local_path).unwrap();
    settings.gemini_api_key = "unrelated".to_string();
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let disk = disk_object(&path);
    assert_eq!(disk["agents"], base["agents"]);
    assert_eq!(std::fs::read(&local_path).unwrap(), overlay_before);
    assert_eq!(order_2306_ids(&written.agents), ["ov-c", "ov-a", "ov-b"]);
    assert_eq!(
        order_2306_positions(&written.agents),
        [Some(0), Some(1), Some(2)]
    );
}

#[test]
fn agent_order_overlay_only_install_honors_numeric_order_without_leaking_to_base() {
    use local_overlay_1737::{disk_object, seed};
    let temp = tempfile::tempdir().unwrap();
    let local = serde_json::json!({
        "agents": [
            {"id":"only-a","label":"A","command":"claude","color":"#111111","order":1},
            {"id":"only-b","label":"B","command":"claude","color":"#222222","order":0}
        ]
    });
    let path = seed(temp.path(), None, Some(&local));
    let local_path = temp.path().join("settings.50.personal.no-git.json");
    let overlay_before = std::fs::read(&local_path).unwrap();

    let settings = super::super::load_settings_from_path(&path);
    assert_eq!(order_2306_ids(&settings.agents), ["only-b", "only-a"]);
    assert_eq!(order_2306_positions(&settings.agents), [Some(0), Some(1)]);
    // Whole-group protection preserves physical absence. The effective
    // overlay order is independent of materializing a base agents array.
    let disk = disk_object(&path);
    assert!(
        !disk.contains_key("agents"),
        "the absent base agents key must stay absent: {disk:?}"
    );
    let sidecar = super::super::agents_instance_path(&path);
    assert!(!sidecar.exists() || !disk_object(&sidecar).contains_key("agents"));
    assert_eq!(std::fs::read(&local_path).unwrap(), overlay_before);
}

#[test]
fn agent_order_unparseable_base_with_overlay_still_finalizes_the_effective_view() {
    use local_overlay_1737::seed;
    let temp = tempfile::tempdir().unwrap();
    let local = serde_json::json!({
        "agents": [
            {"id":"ov-a","label":"A","command":"claude","color":"#111111","order":1},
            {"id":"ov-b","label":"B","command":"claude","color":"#222222","order":0}
        ]
    });
    let path = seed(temp.path(), None, Some(&local));
    std::fs::write(&path, "{not json").unwrap();
    let corrupt = std::fs::read(&path).unwrap();

    let settings = super::super::load_settings_from_path(&path);
    assert_eq!(order_2306_ids(&settings.agents), ["ov-b", "ov-a"]);
    assert_eq!(order_2306_positions(&settings.agents), [Some(0), Some(1)]);
    // The preserve writer refuses to overwrite a present-but-invalid base.
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
}

/// #2430: the worked table from the phase plan. Every row is what
/// `executableTokenBasename` (`src/shared/profile-utils.ts`) returns.
const STEM_TWIN_TABLE: &[(&str, &str)] = &[
    (r"C:\tools\claude.exe", "claude"),
    ("/usr/bin/claude", "claude"),
    ("claude", "claude"),
    ("C:/tools/CLAUDE.CMD", "claude"),
    ("claude-3.5", "claude-3"),
    ("./bin/agy", "agy"),
    ("", ""),
    (".bashrc", ""),
    ("claude.", "claude."),
    (r"C:\tools\", "c:/tools/"),
];

#[test]
fn command_token_basename_strips_windows_separators_on_every_os() {
    assert_eq!(
        super::super::command_token_basename(r"C:\tools\claude.exe"),
        "claude"
    );
}

#[test]
fn command_token_basename_matches_the_typescript_twin() {
    for (input, expected) in STEM_TWIN_TABLE {
        assert_eq!(
            super::super::command_token_basename(input),
            *expected,
            "input {input:?}"
        );
    }
}

#[test]
fn command_token_basename_is_idempotent() {
    for (input, _) in STEM_TWIN_TABLE {
        let once = super::super::command_token_basename(input);
        assert_eq!(
            super::super::command_token_basename(&once),
            once,
            "input {input:?}"
        );
    }
}

#[test]
fn derived_agent_id_home_legacy_roundtrip_omits_empty_carrier() {
    let value = b3_legacy_fixture("Legacy", "Profile");
    let settings: super::super::AppSettings = serde_json::from_value(value).unwrap();
    assert!(settings
        .coding_agent_profiles
        .codex_home_key_by_agent
        .is_empty());
    let encoded = serde_json::to_value(&settings).unwrap();
    assert!(encoded["codingAgentProfiles"]
        .get("codexHomeKeyByAgent")
        .is_none());
    let roundtrip: super::super::AppSettings = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        serde_json::to_value(&roundtrip.agents).unwrap(),
        serde_json::to_value(&settings.agents).unwrap()
    );
    assert_eq!(
        roundtrip.coding_agent_profiles,
        settings.coding_agent_profiles
    );
}

#[test]
fn derived_agent_id_home_whole_save_restores_sidecar_and_written() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let mut legacy = b3_legacy_fixture("Legacy", "Legacy P");
    legacy["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"alpha": "wrong-legacy"});
    b3_write(&path, &legacy);
    let mut sidecar = b3_legacy_fixture("Sidecar", "Sidecar P");
    let expected = serde_json::json!({"alpha": "original-home", "orphan": "retained"});
    sidecar["codingAgentProfiles"]["codexHomeKeyByAgent"] = expected.clone();
    b3_write(&b3_sidecar_path(temp.path()), &sidecar);
    let mut request = super::super::load_settings_for_cli_from_path(&path);
    request.coding_agent_profiles.codex_home_key_by_agent =
        std::collections::BTreeMap::from([("alpha".to_string(), "forged".to_string())]);
    // Negative control: the ordinary request serializer really carries the
    // forgery. Removing backend restoration makes the assertions below fail.
    assert_ne!(
        serde_json::to_value(&request).unwrap()["codingAgentProfiles"]["codexHomeKeyByAgent"],
        expected
    );
    request.agents[0].label = "Edited label".to_string();
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&request, &path).unwrap();
    assert_eq!(
        b3_object(&b3_sidecar_path(temp.path()))["codingAgentProfiles"]["codexHomeKeyByAgent"],
        expected
    );
    assert_eq!(
        serde_json::to_value(&written.coding_agent_profiles).unwrap()["codexHomeKeyByAgent"],
        expected
    );
    assert_eq!(written.agents[0].label, "Edited label");
    assert_eq!(b3_marker(&written).as_deref(), Some("Sidecar P"));
}

#[test]
fn derived_agent_id_home_absent_base_rejects_new_carrier() {
    for present in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        if present {
            b3_write(&path, &b3_legacy_fixture("Legacy", "Profile"));
        }
        let mut request = super::super::AppSettings::default();
        request
            .coding_agent_profiles
            .codex_home_key_by_agent
            .insert("alpha".to_string(), "forged".to_string());
        let written =
            super::super::save_settings_to_path_preserving_project_paths(&request, &path).unwrap();
        assert!(written
            .coding_agent_profiles
            .codex_home_key_by_agent
            .is_empty());
        assert!(
            b3_object(&b3_sidecar_path(temp.path()))["codingAgentProfiles"]
                .get("codexHomeKeyByAgent")
                .is_none()
        );
    }
}

#[test]
fn derived_agent_id_home_serializers_preserve_legacy_carrier() {
    for reconcile in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        let mut legacy = b3_legacy_fixture("Legacy", "Profile");
        legacy["codingAgentProfiles"]["codexHomeKeyByAgent"] =
            serde_json::json!({"alpha": "legacy-home"});
        b3_write(&path, &legacy);
        let mut request = super::super::load_settings_for_cli_from_path(&path);
        request
            .coding_agent_profiles
            .codex_home_key_by_agent
            .clear();
        if reconcile {
            super::super::save_settings_with_project_paths_to_path(&request, &path).unwrap();
        } else {
            super::super::save_settings_to_path_preserving_project_paths(&request, &path).unwrap();
        }
        assert_eq!(
            b3_object(&b3_sidecar_path(temp.path()))["codingAgentProfiles"]["codexHomeKeyByAgent"],
            legacy["codingAgentProfiles"]["codexHomeKeyByAgent"]
        );
        assert_eq!(
            super::super::load_settings_for_cli_from_path(&path)
                .coding_agent_profiles
                .codex_home_key_by_agent
                .get("alpha")
                .map(String::as_str),
            Some("legacy-home")
        );
    }
}

#[test]
fn derived_agent_id_home_narrow_and_overlay_cannot_forge_written() {
    for terminal in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = b3_settings_path(temp.path());
        b3_write(&path, &b3_migrated_fixture());
        let mut sidecar = b3_legacy_fixture("Sidecar", "Profile");
        sidecar["codingAgentProfiles"]["codexHomeKeyByAgent"] =
            serde_json::json!({"alpha": "physical-home"});
        b3_write(&b3_sidecar_path(temp.path()), &sidecar);
        b3_write(
            &b3_overlay_path(temp.path()),
            &serde_json::json!({
                "codingAgentProfiles": {"codexHomeKeyByAgent": {"alpha": "overlay-forged"}}
            }),
        );
        let mut request = super::super::load_settings_for_cli_from_path(&path);
        assert_eq!(
            request.coding_agent_profiles.codex_home_key_by_agent["alpha"],
            "physical-home"
        );
        request
            .coding_agent_profiles
            .codex_home_key_by_agent
            .insert("alpha".to_string(), "client-forged".to_string());
        let written = if terminal {
            super::super::compare_and_set_terminal_snapshots_enabled_at_path(
                &request, &path, true, false,
            )
            .unwrap()
        } else {
            let geometry = super::super::WindowGeometry {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            };
            super::super::set_main_window_placement_at_path(
                &request,
                &path,
                &geometry,
                super::super::MainWindowDisplayState::Normal,
            )
            .unwrap()
        };
        assert_eq!(
            written.coding_agent_profiles.codex_home_key_by_agent["alpha"],
            "physical-home"
        );
        assert_eq!(
            b3_object(&b3_sidecar_path(temp.path()))["codingAgentProfiles"]["codexHomeKeyByAgent"]
                ["alpha"],
            "physical-home"
        );
        // Terminal CAS also has an idempotent return without serialization.
        let unchanged = super::super::compare_and_set_terminal_snapshots_enabled_at_path(
            &request,
            &path,
            written.terminal_snapshots_enabled,
            written.terminal_snapshots_enabled,
        )
        .unwrap();
        assert_eq!(
            unchanged.coding_agent_profiles.codex_home_key_by_agent["alpha"],
            "physical-home"
        );
    }
}

#[test]
fn derived_agent_id_home_load_uses_physical_origin_under_overlay() {
    let physical = tempfile::tempdir().unwrap();
    let logical = tempfile::tempdir().unwrap();
    let io = b3_settings_path(physical.path());
    let label = b3_settings_path(logical.path());
    let mut source = b3_legacy_fixture("Physical", "Profile");
    source["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"alpha": "physical-home"});
    b3_write(&b3_sidecar_path(physical.path()), &source);
    source["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"alpha": "logical-forged"});
    b3_write(&b3_sidecar_path(logical.path()), &source);
    b3_write(
        &b3_overlay_path(physical.path()),
        &serde_json::json!({"codingAgentProfiles": {"codexHomeKeyByAgent": {"alpha": "overlay-forge\
            d"}}}),
    );
    let contents = serde_json::to_string(&b3_migrated_fixture()).unwrap();
    let (loaded, _) = super::super::parse_settings_json_with_projection_labeled(
        &contents,
        "physical-test",
        Some(&io),
        Some(&label),
        None,
    )
    .unwrap();
    assert_eq!(
        loaded.coding_agent_profiles.codex_home_key_by_agent["alpha"],
        "physical-home"
    );
    let default_loaded = super::super::default_settings_with_overlay_captured(
        &io,
        "physical-test",
        &serde_json::Map::new(),
    );
    assert_eq!(
        default_loaded.coding_agent_profiles.codex_home_key_by_agent["alpha"],
        "physical-home"
    );
}

#[test]
fn derived_agent_id_home_invalid_durable_leaf_survives_save_and_fails_use() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let mut source = b3_legacy_fixture("Codex", "Profile");
    source["agents"][0]["command"] = serde_json::json!("codex");
    source["agents"][0]["isolatedHome"] = serde_json::json!(true);
    source["codingAgentProfiles"]["codexHomeKeyByAgent"] =
        serde_json::json!({"alpha": "../invalid"});
    b3_write(&path, &source);
    let mut request = super::super::load_settings_for_cli_from_path(&path);
    request
        .coding_agent_profiles
        .codex_home_key_by_agent
        .clear();
    let written =
        super::super::save_settings_to_path_preserving_project_paths(&request, &path).unwrap();
    assert_eq!(
        written.coding_agent_profiles.codex_home_key_by_agent["alpha"],
        "../invalid"
    );
    assert!(crate::config::agent_command::resolve_agent_spawn_command(
        &written,
        "alpha",
        None,
        Some("A"),
        false
    )
    .unwrap_err()
    .contains("Invalid isolated Codex HOME key"));
}
