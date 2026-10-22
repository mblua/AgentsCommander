use super::super::*;
use super::local_overlay_1737::{base_fixture, disk_object, folder_trust_entry, seed};
use serde_json::json;
use std::time::Duration;

fn valid_entry(pattern: &str, notification: &str) -> BlockingMenuEntry {
    BlockingMenuEntry::Valid(BlockingMenuConfig {
        pattern: pattern.to_string(),
        notification: notification.to_string(),
        enabled: true,
        captured_against: None,
    })
}

fn settings_path_in(temp: &tempfile::TempDir) -> PathBuf {
    temp.path().join("settings.json")
}

// T1
#[test]
fn embedded_content_matches_the_rust_literals() {
    let shipped = shipped_blocking_menus();
    assert_eq!(shipped.schema_version, 1);
    assert!(shipped.note.is_some());
    assert!(shipped.by_agent.is_empty());
    let keys: Vec<&str> = shipped.by_command.keys().map(String::as_str).collect();
    assert_eq!(keys, ["codex", "pi"]);

    let pi = &shipped.by_command["pi"];
    assert_eq!(pi.len(), 1);
    let pi_entry = pi[0].valid().expect("the pi entry is Valid");
    assert_eq!(pi_entry.pattern, r"^\s*Trust project folder\?");
    assert!(pi_entry.enabled);
    assert_eq!(
        pi_entry.captured_against.as_deref(),
        Some("pi 0.52 / Windows")
    );

    let codex = &shipped.by_command["codex"];
    assert_eq!(codex.len(), 2);
    let trust = codex[0].valid().expect("codex[0] is Valid");
    assert_eq!(
        trust.pattern,
        r"^\s*Do you trust the contents of this directory\?"
    );
    assert_eq!(trust.captured_against.as_deref(), Some("codex 0.x / Linux"));
    assert_eq!(
        codex[1],
        BlockingMenuEntry::Valid(codex_hooks_review_menu())
    );
}

// T2
#[test]
fn resolve_precedence_is_legacy_then_local_agent_then_local_command_then_shipped() {
    let x = valid_entry("^X", "x");
    let y = valid_entry("^Y", "y");
    let z = valid_entry("^Z", "z");
    let mut local = BlockingMenusFile::default();
    local
        .by_agent
        .insert("codex-b".to_string(), vec![x.clone()]);
    local.by_agent.insert("pi-off".to_string(), vec![]);
    local
        .by_command
        .insert("codex".to_string(), vec![y.clone()]);
    let store = BlockingMenusStore::with_local(local);

    assert_eq!(store.resolve("codex-b", "codex"), vec![x.clone()]);
    assert_eq!(store.resolve("codex-a", "codex"), vec![y.clone()]);
    // Directory-free on purpose: `file_stem` of a backslash path differs on Unix.
    assert_eq!(
        store.resolve("codex-a", "Codex.exe --search"),
        vec![y.clone()]
    );
    assert!(store.resolve("pi-off", "pi").is_empty());
    assert!(store.resolve("claude-1", "claude").is_empty());
    assert_eq!(
        store.resolve("pi-1", "pi"),
        default_blocking_menus_for_command("pi")
    );

    // Layer 0: an array still on the agent wins over every file layer.
    assert_eq!(
        store.resolve_for(&super::agent_1757(
            "codex-b",
            "codex",
            Some(vec![z.clone()])
        )),
        vec![z]
    );
    assert!(store
        .resolve_for(&super::agent_1757("pi-1", "pi", Some(vec![])))
        .is_empty());
    assert_eq!(
        store.resolve_for(&super::agent_1757("codex-b", "codex", None)),
        vec![x]
    );
}

// T3
#[test]
fn shipped_file_is_written_when_absent_or_stale_and_left_alone_when_equal() {
    let temp = tempfile::tempdir().unwrap();
    let settings = settings_path_in(&temp);
    let shipped = blocking_menus_shipped_path(&settings);
    let canonical = pretty_json_bytes(shipped_blocking_menus()).unwrap();

    assert!(
        refresh_shipped_blocking_menus_file(&settings),
        "absent: written"
    );
    assert_eq!(std::fs::read(&shipped).unwrap(), canonical);

    assert!(
        !refresh_shipped_blocking_menus_file(&settings),
        "equal: left alone"
    );
    assert_eq!(std::fs::read(&shipped).unwrap(), canonical);

    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&shipped)
            .unwrap();
        file.write_all(b"x").unwrap();
    }
    assert_ne!(std::fs::read(&shipped).unwrap(), canonical);

    assert!(
        refresh_shipped_blocking_menus_file(&settings),
        "stale: rewritten"
    );
    assert_eq!(std::fs::read(&shipped).unwrap(), canonical);
}

// T4
#[test]
fn local_file_rejections_yield_an_empty_layer_and_match_the_parser() {
    let temp = tempfile::tempdir().unwrap();
    let settings = settings_path_in(&temp);
    let local = blocking_menus_local_path(&settings);

    // Absent is the empty layer.
    assert_eq!(
        load_local_blocking_menus_file(&settings),
        BlockingMenusFile::default()
    );

    let rejected = [
        "{ not json",
        "[1]",
        "[]",
        "null",
        r#"{"schemaVersion": 2}"#,
        r#"{"schemaVersion":1,"byCommand":42,"byAgent":{}}"#,
        r#"{"byAgent":{"codex":42}}"#,
        r#"{"note":42,"byAgent":{}}"#,
    ];
    for text in rejected {
        assert!(
            parse_blocking_menus_file(text).is_err(),
            "{text:?} must be rejected by the parser"
        );
        std::fs::write(&local, text).unwrap();
        assert_eq!(
            load_local_blocking_menus_file(&settings),
            BlockingMenusFile::default(),
            "{text:?} must load as the empty layer"
        );
    }

    // A valid file with one `byAgent` entry and an unknown top-level key.
    let valid_text = json!({
        "schemaVersion": 1,
        "extra": true,
        "byAgent": {"codex-b": [{"pattern": "^X", "notification": "n"}]}
    })
    .to_string();
    let parsed = parse_blocking_menus_file(&valid_text).unwrap();
    assert_eq!(parsed.by_agent["codex-b"], vec![valid_entry("^X", "n")]);
    std::fs::write(&local, &valid_text).unwrap();
    assert_eq!(load_local_blocking_menus_file(&settings), parsed);

    // A malformed entry is kept verbatim as `Invalid`, as today.
    let kept = parse_blocking_menus_file(r#"{"byAgent":{"codex":[42]}}"#).unwrap();
    assert_eq!(
        kept.by_agent["codex"],
        vec![BlockingMenuEntry::Invalid(json!(42))]
    );
}

// ─────────────────────────────────────────────────────────────────────────
// T6-T14 - the #1905 one-shot export out of settings.json.
// Fixture policy: every fixture entry carries all four fields, because the
// export writes a `Valid` entry through the typed round-trip, which adds
// `"enabled": true` when absent, so an equality assertion against a
// three-field fixture fails.
// ─────────────────────────────────────────────────────────────────────────

/// `{id, label: id, command, color}` plus `"blockingMenus": menus` when `Some`.
fn agent_json(id: &str, command: &str, menus: Option<Value>) -> Value {
    let mut agent = json!({
        "id": id,
        "label": id,
        "command": command,
        "color": "#000000",
    });
    if let Some(menus) = menus {
        agent["blockingMenus"] = menus;
    }
    agent
}

/// `base_fixture()` with its `agents` replaced and a `codingAgentProfiles`
/// object holding an `A` cell for every distinct id, so that
/// `repair_coding_agent_profiles_config` is inert and the export is the
/// only possible `needs_save` source in this module.
fn fixture_with(agents: &[Value]) -> Value {
    let mut base = base_fixture();
    base["agents"] = Value::Array(agents.to_vec());
    let mut profiles_by_agent = Map::new();
    for agent in agents {
        profiles_by_agent
            .entry(agent["id"].as_str().expect("agent id").to_string())
            .or_insert_with(|| {
                json!({
                    "A": { "enabled": true, "command": "", "env": {}, "notes": "" }
                })
            });
    }
    base["codingAgentProfiles"] = json!({
        "schemaVersion": 2,
        "profileSlots": { "A": { "label": "" } },
        "defaultProfileByAgent": {},
    });
    base["codingAgentProfiles"]["profilesByAgent"] = Value::Object(profiles_by_agent);
    base
}

/// The shipped folder-trust entry as JSON (four fields).
fn ft() -> Value {
    folder_trust_entry()
}

/// `ft` with `enabled: false`.
fn ft_disabled() -> Value {
    let mut entry = ft();
    entry["enabled"] = json!(false);
    entry
}

/// The shipped codex hooks-review entry as canonical JSON.
fn hooks() -> Value {
    serde_json::to_value(codex_hooks_review_menu()).unwrap()
}

/// A user-customized entry (all four fields).
fn custom_entry(pattern: &str) -> Value {
    json!({
        "pattern": pattern,
        "notification": "custom 1905 notification",
        "enabled": true,
        "capturedAgainst": "test",
    })
}

fn agent_by_id<'a>(settings: &'a AppSettings, id: &str) -> &'a AgentConfig {
    settings
        .agents
        .iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("no agent {id}"))
}

/// The blocking-menus `.local` file parsed back as JSON.
fn local_disk_object(settings_path: &Path) -> Value {
    let raw = std::fs::read_to_string(blocking_menus_local_path(settings_path)).unwrap();
    serde_json::from_str(&raw).expect(".local file must parse as JSON")
}

fn resolve_json(store: &BlockingMenusStore, agent: &AgentConfig) -> Value {
    serde_json::to_value(store.resolve_for(agent)).unwrap()
}

fn assert_no_blocking_menus_on_disk(path: &Path) {
    let object = disk_object(&crate::config::settings::agents_instance_path(path));
    let agents = object["agents"].as_array().expect("agents array");
    for agent in agents {
        assert!(
            agent.get("blockingMenus").is_none(),
            "blockingMenus must be stripped from settings.json"
        );
    }
}

fn assert_blocking_menus_on_disk_unchanged(path: &Path, seeded: &[Value]) {
    let object = disk_object(&crate::config::settings::agents_instance_path(path));
    let agents = object["agents"].as_array().expect("agents array");
    assert_eq!(agents.len(), seeded.len());
    for (agent, expected) in agents.iter().zip(seeded) {
        // #2716 (B3) 6.1: the three legs of (b), not an exact equality.
        super::b3_assert_blocking_menus_legs(
            expected.get("blockingMenus"),
            agent.get("blockingMenus"),
            expected["command"].as_str().unwrap_or_default(),
            &expected["id"].to_string(),
        );
    }
}

// T6
#[test]
fn export_moves_customized_arrays_and_drops_pristine_ones_then_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let seeded = [
        agent_json("pi", "pi", Some(json!([]))),
        agent_json("codex", "codex", Some(json!([ft(), hooks()]))),
        agent_json("codex-old", "codex", Some(json!([ft()]))),
        agent_json("codex-off", "codex", Some(json!([]))),
        agent_json("claude", "claude", Some(json!([]))),
        agent_json(
            "mine",
            "claude",
            Some(json!([custom_entry("^custom-1905"), 12345])),
        ),
    ];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);

    let settings = load_settings_from_path(&path);

    // Every customized or explicit array moved; pristine ones were dropped.
    assert_no_blocking_menus_on_disk(&path);
    let local = local_disk_object(&path);
    assert_eq!(local["schemaVersion"], 1);
    let by_agent = local["byAgent"].as_object().expect("byAgent object");
    let mut keys: Vec<&str> = by_agent.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["codex-off", "mine", "pi"]);
    assert_eq!(by_agent["pi"], json!([]));
    assert_eq!(by_agent["codex-off"], json!([]));
    assert_eq!(
        by_agent["mine"],
        json!([custom_entry("^custom-1905"), 12345])
    );

    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "pi")),
        json!([])
    );
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "codex-off")),
        json!([])
    );
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "codex")),
        serde_json::to_value(default_blocking_menus_for_command("codex")).unwrap()
    );
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "codex-old")),
        serde_json::to_value(default_blocking_menus_for_command("codex")).unwrap()
    );
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "claude")),
        json!([])
    );
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "mine"))
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Idempotent: the second load changes neither file.
    let settings_bytes = std::fs::read(&path).unwrap();
    let local_bytes = std::fs::read(blocking_menus_local_path(&path)).unwrap();
    load_settings_from_path(&path);
    assert_eq!(std::fs::read(&path).unwrap(), settings_bytes);
    assert_eq!(
        std::fs::read(blocking_menus_local_path(&path)).unwrap(),
        local_bytes
    );
}

// T7
#[test]
fn export_keeps_an_existing_local_entry_and_preserves_unknown_keys() {
    let temp = tempfile::tempdir().unwrap();
    let seeded = [
        agent_json("codex", "codex", Some(json!([ft_disabled()]))),
        agent_json(
            "mine",
            "claude",
            Some(json!([custom_entry("^custom-1905-b")])),
        ),
    ];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
    std::fs::write(
                blocking_menus_local_path(&path),
                r#"{"extra": true, "byAgent": {"codex": [{"pattern": "^custom-1905", "notification": "custom 1905 notification", "enabled": true, "capturedAgainst": "test"}]}}"#,
            )
            .unwrap();

    let settings = load_settings_from_path(&path);

    let local = local_disk_object(&path);
    assert_eq!(
        local["byAgent"]["codex"],
        json!([custom_entry("^custom-1905")]),
        "the existing byAgent entry wins over the candidate"
    );
    assert_eq!(
        local["byAgent"]["mine"],
        json!([custom_entry("^custom-1905-b")])
    );
    assert_eq!(local["extra"], json!(true), "unknown keys survive");
    assert_eq!(local["schemaVersion"], 1);
    assert_no_blocking_menus_on_disk(&path);

    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "codex")),
        json!([custom_entry("^custom-1905")])
    );

    // The 1757 step ran before the byAgent comparison: on a copy of the
    // fixture settings it appends the hooks entry to codex's array.
    let mut copy: AppSettings = serde_json::from_value(fixture_with(&seeded)).unwrap();
    apply_issue_1757_migration(&mut copy);
    assert_eq!(
        serde_json::to_value(agent_by_id(&copy, "codex").blocking_menus.as_ref().unwrap()).unwrap(),
        json!([ft_disabled(), hooks()])
    );
}

// T8
#[test]
fn export_refuses_when_the_local_file_is_unparseable() {
    let temp = tempfile::tempdir().unwrap();
    let seeded = [
        agent_json("codex", "codex", Some(json!([ft()]))),
        agent_json(
            "mine",
            "claude",
            Some(json!([custom_entry("^custom-1905")])),
        ),
    ];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
    std::fs::write(blocking_menus_local_path(&path), "{ not json").unwrap();
    let seeded_bytes = std::fs::read(&path).unwrap();

    let settings = load_settings_from_path(&path);

    // Nothing moved, nothing was rewritten: the on-disk arrays are exactly
    // what was seeded (no 1757 entry is added on disk on an abort path).
    assert_blocking_menus_on_disk_unchanged(&path, &seeded);
    super::b3_assert_split_matches_seed(&path, &seeded_bytes, &[]);
    assert_eq!(
        std::fs::read(blocking_menus_local_path(&path)).unwrap(),
        b"{ not json"
    );

    // Layer 0 keeps serving both arrays; codex's is the in-memory 1757 step's
    // result, which is what the current binary serves.
    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "codex")),
        json!([ft(), hooks()])
    );
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "mine")),
        json!([custom_entry("^custom-1905")])
    );
}

// T9
#[test]
fn export_is_state_keyed() {
    let temp = tempfile::tempdir().unwrap();
    let seeded = [
        agent_json("codex", "codex", None),
        agent_json("mine", "claude", None),
    ];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
    let seeded_bytes = std::fs::read(&path).unwrap();

    let settings = load_settings_from_path(&path);

    // No agent had `Some`, so the export is a no-op: no save, no `.local`.
    assert!(!blocking_menus_local_path(&path).exists());
    super::b3_assert_split_matches_seed(&path, &seeded_bytes, &[]);

    // The migrated steady state is served by the files.
    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "codex")),
        serde_json::to_value(default_blocking_menus_for_command("codex")).unwrap()
    );
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "mine")),
        json!([])
    );
}

// T10
#[test]
fn export_aborts_when_the_local_file_cannot_be_written() {
    let temp = tempfile::tempdir().unwrap();
    let seeded = [agent_json(
        "mine",
        "claude",
        Some(json!([custom_entry("^custom-1905")])),
    )];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
    let tmp_dir = temp.path().join(format!(
        ".blocking-menus.50.personal.no-git.json.{}.tmp",
        std::process::id()
    ));
    std::fs::create_dir(&tmp_dir).unwrap();
    let seeded_bytes = std::fs::read(&path).unwrap();

    let settings = load_settings_from_path_with_blocking_menus_publish(
        &path,
        Some(&|tmp, target| {
            assert!(
                tmp.is_file(),
                "fresh staging acquired before injected refusal"
            );
            assert_eq!(target, blocking_menus_local_path(&path));
            Err("fixture temp config publish failure".into())
        }),
    );

    super::b3_assert_split_matches_seed(&path, &seeded_bytes, &[]);
    assert!(!blocking_menus_local_path(&path).exists());
    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "mine")),
        json!([custom_entry("^custom-1905")])
    );

    // The retry on the next start finishes the export.
    std::fs::remove_dir(&tmp_dir).unwrap();
    load_settings_from_path(&path);
    let local = local_disk_object(&path);
    assert_eq!(
        local["byAgent"]["mine"],
        json!([custom_entry("^custom-1905")])
    );
    assert_no_blocking_menus_on_disk(&path);
}

// T11 - takes about 2 seconds by design: the loader's own save waits its
// 2-second SettingsFileLock timeout while the test holds the lock.
#[test]
fn a_failed_settings_save_after_a_successful_export_is_completed_at_the_next_start() {
    let temp = tempfile::tempdir().unwrap();
    let seeded = [
        agent_json("codex", "codex", Some(json!([ft(), hooks()]))),
        agent_json(
            "mine",
            "claude",
            Some(json!([custom_entry("^custom-1905")])),
        ),
    ];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
    let seeded_bytes = std::fs::read(&path).unwrap();

    let (mut settings, _, _) = super::super::capture_project_settings(&path, false).unwrap();
    assert!(super::super::export_blocking_menus_to_local_file(
        &mut settings,
        &path
    ));
    let held = SettingsFileLock::acquire(&path, Duration::from_millis(50)).unwrap();
    assert!(
        super::super::save_settings_to_path_preserving_project_paths(&settings, &path).is_err()
    );

    // Export completed unlocked; the contended Preserve save refuses.
    let local = local_disk_object(&path);
    assert_eq!(
        local["byAgent"]["mine"],
        json!([custom_entry("^custom-1905")])
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        seeded_bytes,
        "the failed save must leave settings.json untouched"
    );
    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "mine")),
        json!([custom_entry("^custom-1905")]),
        "the file serves the entries; the in-memory array is None"
    );

    drop(held);
    let local_bytes = std::fs::read(blocking_menus_local_path(&path)).unwrap();
    let settings = load_settings_from_path(&path);

    assert_no_blocking_menus_on_disk(&path);
    assert_eq!(
        std::fs::read(blocking_menus_local_path(&path)).unwrap(),
        local_bytes,
        "the second start finds the ids present, inserts nothing, strips again"
    );
    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "mine")),
        json!([custom_entry("^custom-1905")])
    );
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "codex")),
        serde_json::to_value(default_blocking_menus_for_command("codex")).unwrap()
    );
}

// T12
#[test]
fn a_restart_between_the_two_writes_finishes_the_export() {
    let temp = tempfile::tempdir().unwrap();
    let seeded = [
        agent_json("codex", "codex", Some(json!([ft(), hooks()]))),
        agent_json(
            "mine",
            "claude",
            Some(json!([custom_entry("^custom-1905")])),
        ),
    ];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
    // The exact on-disk state T11 leaves after its first load.
    let local_text = r#"{"schemaVersion": 1, "byAgent": {"mine": [{"pattern": "^custom-1905", "notification": "custom 1905 notification", "enabled": true, "capturedAgainst": "test"}]}}"#;
    std::fs::write(blocking_menus_local_path(&path), local_text).unwrap();

    let settings = load_settings_from_path(&path);

    assert_no_blocking_menus_on_disk(&path);
    assert_eq!(
        std::fs::read(blocking_menus_local_path(&path)).unwrap(),
        local_text.as_bytes(),
        "nothing was inserted, so the .local file is untouched"
    );
    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, agent_by_id(&settings, "mine")),
        json!([custom_entry("^custom-1905")])
    );
}

// T13
#[test]
fn schema_parity_rejects_what_the_runtime_rejects() {
    let texts = [
        r#"{"schemaVersion":1,"byCommand":42,"byAgent":{}}"#,
        r#"{"byAgent":{"codex":42}}"#,
        r#"{"note":42,"byAgent":{}}"#,
        r#"{"schemaVersion":2}"#,
        "[1]",
    ];
    for text in texts {
        let temp = tempfile::tempdir().unwrap();
        let seeded = [
            agent_json("codex", "codex", Some(json!([ft()]))),
            agent_json(
                "mine",
                "claude",
                Some(json!([custom_entry("^custom-1905")])),
            ),
        ];
        let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
        std::fs::write(blocking_menus_local_path(&path), text).unwrap();
        let seeded_bytes = std::fs::read(&path).unwrap();

        let settings = load_settings_from_path(&path);

        // #2716 (B3) 6.1: an abort still strips nothing from the agents.
        super::b3_assert_split_matches_seed(&path, &seeded_bytes, &[]);
        assert_eq!(
            std::fs::read(blocking_menus_local_path(&path)).unwrap(),
            text.as_bytes(),
            "{text:?}: the .local file must stay untouched"
        );
        let store = BlockingMenusStore::load_from_settings_path(&path);
        assert_eq!(
            resolve_json(&store, agent_by_id(&settings, "mine")),
            json!([custom_entry("^custom-1905")]),
            "{text:?}: layer 0 keeps serving the in-memory array"
        );
        assert!(parse_blocking_menus_file(text).is_err(), "{text:?}");
    }
}

// T14
#[test]
fn duplicate_agent_ids_abort_unless_their_arrays_and_commands_are_equal() {
    // Different arrays under one command: abort.
    let temp = tempfile::tempdir().unwrap();
    let seeded = [
        agent_json("dup", "claude", Some(json!([custom_entry("^custom-1905")]))),
        agent_json(
            "dup",
            "claude",
            Some(json!([custom_entry("^custom-1905-b")])),
        ),
    ];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
    let seeded_bytes = std::fs::read(&path).unwrap();
    let settings = load_settings_from_path(&path);
    assert_blocking_menus_on_disk_unchanged(&path, &seeded);
    super::b3_assert_split_matches_seed(&path, &seeded_bytes, &[]);
    assert!(!blocking_menus_local_path(&path).exists());
    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, &settings.agents[0]),
        json!([custom_entry("^custom-1905")])
    );
    assert_eq!(
        resolve_json(&store, &settings.agents[1]),
        json!([custom_entry("^custom-1905-b")])
    );

    // Equal arrays under different commands (pristine for one, custom for the
    // other): abort, layer 0 keeps serving both.
    let temp = tempfile::tempdir().unwrap();
    let seeded = [
        agent_json("dup", "claude", Some(json!([ft(), hooks()]))),
        agent_json("dup", "codex", Some(json!([ft(), hooks()]))),
    ];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
    let seeded_bytes = std::fs::read(&path).unwrap();
    let settings = load_settings_from_path(&path);
    assert_blocking_menus_on_disk_unchanged(&path, &seeded);
    super::b3_assert_split_matches_seed(&path, &seeded_bytes, &[]);
    assert!(!blocking_menus_local_path(&path).exists());
    let store = BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        resolve_json(&store, &settings.agents[0]),
        json!([ft(), hooks()])
    );
    assert_eq!(
        resolve_json(&store, &settings.agents[1]),
        json!([ft(), hooks()])
    );

    // Equal arrays under one command: one candidate, exported.
    let temp = tempfile::tempdir().unwrap();
    let seeded = [
        agent_json("dup", "claude", Some(json!([custom_entry("^custom-1905")]))),
        agent_json("dup", "claude", Some(json!([custom_entry("^custom-1905")]))),
    ];
    let path = seed(temp.path(), Some(&fixture_with(&seeded)), None);
    let seeded_bytes = std::fs::read(&path).unwrap();
    load_settings_from_path(&path);
    // #2716 (B3) 6.1: this branch exports, so `blockingMenus` leaves the
    // agents file; the line below asserts that, the split asserts the rest.
    super::b3_assert_split_matches_seed(&path, &seeded_bytes, &["blockingMenus"]);
    assert_no_blocking_menus_on_disk(&path);
    let local = local_disk_object(&path);
    let by_agent = local["byAgent"].as_object().expect("byAgent object");
    let mut keys: Vec<&str> = by_agent.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["dup"]);
    assert_eq!(by_agent["dup"], json!([custom_entry("^custom-1905")]));
}
