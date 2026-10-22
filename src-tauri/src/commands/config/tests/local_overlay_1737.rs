use super::{
    persist_protected_settings_update_with_saver, persist_settings_draft_update_with_saver,
    state_for,
};
use crate::config::settings::{
    load_settings_from_path, save_settings_to_path_preserving_project_paths,
    validate_and_repair_settings, AppSettings,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn agent_json(id: &str) -> Value {
    json!({
        "id": id,
        "label": id,
        "command": id,
        "color": "#000000",
        "blockingMenus": [],
    })
}

/// A base `settings.json` with two agents and a watcher whose
/// `dedupeWindowMs` the overlay overrides, plus the local file. Returns the
/// settings path.
fn seed(dir: &Path, local: &Value) -> PathBuf {
    let base = json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "rootToken": "base-token",
        "agents": [agent_json("codex"), agent_json("claude")],
        "watchers": {
            "a": { "mode": "state", "pattern": "^ready", "dedupeWindowMs": 500 }
        },
        "codingAgentProfiles": {
            "schemaVersion": 2,
            "profileSlots": { "A": { "label": "" } },
            "profilesByAgent": {
                "codex": { "A": { "enabled": true, "command": "codex", "env": {}, "notes": "" } },
                "claude": { "A": { "enabled": true, "command": "claude", "env": {}, "notes": "" } }
            },
            "defaultProfileByAgent": {},
            "profileLabelsByAgent": {}
        }
    });
    let path = dir.join("settings.json");
    std::fs::write(&path, serde_json::to_string_pretty(&base).unwrap()).unwrap();
    std::fs::write(
        dir.join("settings.50.personal.no-git.json"),
        serde_json::to_string_pretty(local).unwrap(),
    )
    .unwrap();
    path
}

fn disk(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn dedupe_window_on_disk(path: &Path) -> Value {
    disk(path)["watchers"]["a"]["dedupeWindowMs"].clone()
}

/// The exact renderer round trip: serialize and deserialize, which is what
/// drops every `#[serde(skip)]` field.
fn renderer_round_trip(settings: &AppSettings) -> AppSettings {
    serde_json::from_value(serde_json::to_value(settings).unwrap()).unwrap()
}

// P4, asserted first because P1 to P3 depend on the behaviour it pins.
#[test]
fn the_renderer_round_trip_drops_the_overlay_state() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        &json!({"watchers": {"a": {"dedupeWindowMs": 50}}}),
    );
    let loaded = load_settings_from_path(&path);
    assert!(
        !loaded.local_overlay_state.is_empty(),
        "the fixture must carry an overlay"
    );
    let payload = renderer_round_trip(&loaded);
    assert!(
        payload.local_overlay_state.is_empty(),
        "this is the serde behaviour the D14 carry-overs compensate for"
    );
}

// P1 and P3
#[tokio::test]
async fn the_protected_writer_inherits_the_live_overlay_state() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        &json!({"watchers": {"a": {"dedupeWindowMs": 50}}}),
    );
    let loaded = load_settings_from_path(&path);
    let state = state_for(loaded.clone());
    let payload = renderer_round_trip(&loaded);

    let (saved, _) = {
        let path = path.clone();
        persist_protected_settings_update_with_saver(&state, payload, move |candidate| {
            save_settings_to_path_preserving_project_paths(candidate, &path)
        })
        .await
        .unwrap()
    };

    // P1: the overlay-owned leaf on disk still holds the base value.
    assert_eq!(dedupe_window_on_disk(&path), json!(500));
    assert!(!saved.local_overlay_state.is_empty());

    // P3: the value the state now holds is still protected, so a second
    // round trip through the same path is protected too.
    {
        let live = state.read().await;
        assert!(!live.local_overlay_state.is_empty());
    }
    let second_payload = {
        let live = state.read().await;
        renderer_round_trip(&live)
    };
    let path2 = path.clone();
    persist_protected_settings_update_with_saver(&state, second_payload, move |candidate| {
        save_settings_to_path_preserving_project_paths(candidate, &path2)
    })
    .await
    .unwrap();
    assert_eq!(dedupe_window_on_disk(&path), json!(500));
}

// P2
#[tokio::test]
async fn the_draft_writer_inherits_the_live_overlay_state() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        &json!({"watchers": {"a": {"dedupeWindowMs": 50}}}),
    );
    let loaded = load_settings_from_path(&path);
    let state = state_for(loaded.clone());
    let payload = renderer_round_trip(&loaded);

    let (saved, _events) = {
        let path = path.clone();
        persist_settings_draft_update_with_saver(&state, payload, move |candidate| {
            save_settings_to_path_preserving_project_paths(candidate, &path)
        })
        .await
        .unwrap()
    };

    assert_eq!(dedupe_window_on_disk(&path), json!(500));
    assert!(!saved.local_overlay_state.is_empty());
}

/// The `agents` overlay that introduces one scratch agent, which is what
/// makes D13's closure own `codingAgentProfiles.profilesByAgent.scratch-agent`.
fn scratch_agent_overlay() -> Value {
    json!({
        "agents": [
            agent_json("codex"),
            agent_json("claude"),
            agent_json("scratch-agent"),
        ]
    })
}

// P5
#[tokio::test]
async fn a_profile_edit_persists_except_for_the_overlay_introduced_agent() {
    for protected in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let overlay = if protected {
            scratch_agent_overlay()
        } else {
            json!({})
        };
        let path = seed(temp.path(), &overlay);
        let overlay_path = temp.path().join("settings.50.personal.no-git.json");
        let overlay_before = std::fs::read(&overlay_path).unwrap();
        let mut expected_raw = disk(&path)["codingAgentProfiles"].clone();
        // Independent #2450 preimages for these two fixed seed rows.
        expected_raw["identityByAgent"] = json!({
            "codex":"460361e30eb7c7a5", "claude":"57bf1d23533f9c40"
        });
        let loaded = load_settings_from_path(&path);
        assert_eq!(
            loaded
                .local_overlay_state
                .owns_top_level("codingAgentProfiles"),
            protected
        );
        assert_eq!(
            loaded
                .coding_agent_profiles
                .profiles_by_agent
                .contains_key("scratch-agent"),
            protected
        );

        let state = state_for(loaded.clone());

        let mut draft = renderer_round_trip(&loaded);
        draft
            .coding_agent_profiles
            .profiles_by_agent
            .get_mut("codex")
            .unwrap()
            .get_mut("A")
            .unwrap()
            .command = "codex --edited".to_string();
        draft.coding_agent_profiles.profile_slots.insert(
            "B".to_string(),
            crate::config::settings::ProfileSlotConfig {
                label: "Second".to_string(),
            },
        );
        draft
            .coding_agent_profiles
            .default_profile_by_agent
            .insert("codex".to_string(), "A".to_string());

        let path2 = path.clone();
        let (saved, _events) =
            persist_settings_draft_update_with_saver(&state, draft, move |candidate| {
                save_settings_to_path_preserving_project_paths(candidate, &path2)
            })
            .await
            .unwrap();

        let physical_path = if protected {
            path.clone()
        } else {
            path.with_file_name("agents.30.instance.no-git.json")
        };
        let profiles = disk(&physical_path)["codingAgentProfiles"].clone();
        if protected {
            assert_eq!(
                profiles, expected_raw,
                "whole protected group, only existing identity output excepted"
            );
        } else {
            let physical = disk(&physical_path);
            let codex: crate::config::settings::AgentConfig = serde_json::from_value(
                physical["agents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["id"] == "codex")
                    .unwrap()
                    .clone(),
            )
            .unwrap();
            assert_eq!(codex.command, "codex");
            assert!(codex.envs.is_empty());
            let hash = crate::config::settings::configuration_identity(&codex.command, &codex.envs);
            let cell = &profiles["profilesByBaseHash"][&hash]["A"];
            assert_eq!(cell["additionalArguments"], json!("codex --edited"));
            assert_eq!(cell["command"], json!("codex"));
            for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
                assert!(profiles.get(key).is_none());
            }
            assert_eq!(profiles["profileSlots"]["B"]["label"], json!("Second"));
            assert_eq!(profiles["defaultProfileByAgent"]["codex"], json!("A"));
        }
        if protected {
            assert_eq!(
                profiles["profilesByAgent"].get("scratch-agent"),
                expected_raw["profilesByAgent"].get("scratch-agent")
            );
            assert!(
                saved.coding_agent_profiles.profiles_by_agent["scratch-agent"].contains_key("A")
            );
        } else {
            let scratch_hash =
                crate::config::settings::configuration_identity("scratch-agent", &[]);
            for key in ["profilesByBaseHash", "profileLabelsByBaseHash"] {
                assert!(profiles[key].is_object());
                assert!(profiles[key].get(&scratch_hash).is_none());
            }
            for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
                assert!(profiles.get(key).is_none());
            }
        }
        assert_eq!(
            profiles
                .get("legacyProfileOrphans")
                .and_then(|rows| rows.get("scratch-agent")),
            expected_raw
                .get("legacyProfileOrphans")
                .and_then(|rows| rows.get("scratch-agent"))
        );
        assert_eq!(std::fs::read(&overlay_path).unwrap(), overlay_before);
        let expected_command = if protected { "codex" } else { "codex --edited" };
        let expected_hash = if protected {
            "cid1:439b584f39ee418c1c6e59bb9b06e7f04b83c2b338d48580f1fb8b7a1750703d"
        } else {
            "cid1:2d09a350fb33981f1d78f968766bba7ba46ae78531a2ee157653926809a03114"
        };
        assert_eq!(
            saved.coding_agent_profiles.profiles_by_agent["codex"]["A"].command,
            expected_command
        );
        assert_eq!(
            saved.coding_agent_profiles.profiles_by_agent["codex"]["A"]
                .id_hash
                .as_deref(),
            Some(expected_hash)
        );
        let reloaded = load_settings_from_path(&path);
        assert_eq!(saved.coding_agent_profiles, reloaded.coding_agent_profiles);
        assert_eq!(
            state.read().await.coding_agent_profiles,
            saved.coding_agent_profiles
        );
    }
}

// P6
#[test]
fn the_dedicated_profiles_command_shape_has_the_same_outcome() {
    for protected in [true, false] {
        p6_profiles_command_case(protected);
    }
}

fn p6_profiles_command_case(protected: bool) {
    let temp = tempfile::tempdir().unwrap();
    let overlay = if protected {
        scratch_agent_overlay()
    } else {
        json!({})
    };
    let path = seed(temp.path(), &overlay);
    let overlay_path = temp.path().join("settings.50.personal.no-git.json");
    let overlay_before = std::fs::read(&overlay_path).unwrap();
    let mut expected_raw = disk(&path)["codingAgentProfiles"].clone();
    // Independent #2450 preimages for these two fixed seed rows.
    expected_raw["identityByAgent"] = json!({
        "codex":"460361e30eb7c7a5", "claude":"57bf1d23533f9c40"
    });
    let loaded = load_settings_from_path(&path);
    assert_eq!(
        loaded
            .local_overlay_state
            .owns_top_level("codingAgentProfiles"),
        protected
    );
    assert_eq!(
        loaded
            .coding_agent_profiles
            .profiles_by_agent
            .contains_key("scratch-agent"),
        protected
    );

    // `persist_coding_agent_profiles_update` clones the live guard, assigns
    // the payload and saves; `save_settings` delegates to the path-taking
    // preserving writer for a resolved path. Reproduced here because that
    // helper takes no injectable saver and redirecting `save_settings`
    // would need a process-global env mutation.
    let mut candidate = loaded.clone();
    candidate
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("codex")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .command = "codex --edited".to_string();
    candidate.coding_agent_profiles.profile_slots.insert(
        "B".to_string(),
        crate::config::settings::ProfileSlotConfig {
            label: "Second".to_string(),
        },
    );
    candidate
        .coding_agent_profiles
        .default_profile_by_agent
        .insert("codex".to_string(), "A".to_string());
    validate_and_repair_settings(&mut candidate).unwrap();
    let saved = save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();

    let physical_path = if protected {
        path.clone()
    } else {
        path.with_file_name("agents.30.instance.no-git.json")
    };
    let profiles = disk(&physical_path)["codingAgentProfiles"].clone();
    if protected {
        assert_eq!(
            profiles, expected_raw,
            "whole protected group, only existing identity output excepted"
        );
    } else {
        let physical = disk(&physical_path);
        p6_assert_unprotected_profile_storage(&physical, &profiles);
    }
    if protected {
        assert_eq!(
            profiles["profilesByAgent"].get("scratch-agent"),
            expected_raw["profilesByAgent"].get("scratch-agent")
        );
        assert!(saved.coding_agent_profiles.profiles_by_agent["scratch-agent"].contains_key("A"));
    } else {
        let scratch_hash = crate::config::settings::configuration_identity("scratch-agent", &[]);
        for key in ["profilesByBaseHash", "profileLabelsByBaseHash"] {
            assert!(profiles[key].is_object());
            assert!(profiles[key].get(&scratch_hash).is_none());
        }
        for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
            assert!(profiles.get(key).is_none());
        }
    }
    assert_eq!(
        profiles
            .get("legacyProfileOrphans")
            .and_then(|rows| rows.get("scratch-agent")),
        expected_raw
            .get("legacyProfileOrphans")
            .and_then(|rows| rows.get("scratch-agent"))
    );
    assert_eq!(std::fs::read(&overlay_path).unwrap(), overlay_before);
    let expected_command = if protected { "codex" } else { "codex --edited" };
    let expected_hash = if protected {
        "cid1:439b584f39ee418c1c6e59bb9b06e7f04b83c2b338d48580f1fb8b7a1750703d"
    } else {
        "cid1:2d09a350fb33981f1d78f968766bba7ba46ae78531a2ee157653926809a03114"
    };
    assert_eq!(
        saved.coding_agent_profiles.profiles_by_agent["codex"]["A"].command,
        expected_command
    );
    assert_eq!(
        saved.coding_agent_profiles.profiles_by_agent["codex"]["A"]
            .id_hash
            .as_deref(),
        Some(expected_hash)
    );
    let reloaded = load_settings_from_path(&path);
    assert_eq!(saved.coding_agent_profiles, reloaded.coding_agent_profiles);
}

fn p6_assert_unprotected_profile_storage(physical: &Value, profiles: &Value) {
    let codex: crate::config::settings::AgentConfig = serde_json::from_value(
        physical["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "codex")
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert_eq!(codex.command, "codex");
    assert!(codex.envs.is_empty());
    let hash = crate::config::settings::configuration_identity(&codex.command, &codex.envs);
    let cell = &profiles["profilesByBaseHash"][&hash]["A"];
    assert_eq!(cell["additionalArguments"], json!("codex --edited"));
    assert_eq!(cell["command"], json!("codex"));
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(profiles.get(key).is_none());
    }
    assert_eq!(profiles["profileSlots"]["B"]["label"], json!("Second"));
    assert_eq!(profiles["defaultProfileByAgent"]["codex"], json!("A"));
}

// P7
#[test]
fn an_agent_delete_removes_its_default_profile_entry_from_the_base_file() {
    for protected in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let overlay = if protected {
            scratch_agent_overlay()
        } else {
            json!({})
        };
        let path = seed(temp.path(), &overlay);
        let overlay_path = temp.path().join("settings.50.personal.no-git.json");
        let overlay_before = std::fs::read(&overlay_path).unwrap();
        // Seed the physical default before protection. The negative
        // must prove an existing A survives, not that absent stays absent.
        let mut raw = disk(&path);
        raw["codingAgentProfiles"]["defaultProfileByAgent"]["claude"] = json!("A");
        let mut expected_raw = raw["codingAgentProfiles"].clone();
        expected_raw["identityByAgent"] = json!({
            "codex":"460361e30eb7c7a5", "claude":"57bf1d23533f9c40"
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
        let loaded = load_settings_from_path(&path);
        assert_eq!(
            loaded
                .local_overlay_state
                .owns_top_level("codingAgentProfiles"),
            protected
        );
        let physical_path = if protected {
            path.clone()
        } else {
            path.with_file_name("agents.30.instance.no-git.json")
        };
        assert_eq!(
            disk(&physical_path)["codingAgentProfiles"]["defaultProfileByAgent"]["claude"],
            json!("A")
        );
        let mut candidate = loaded.clone();
        assert_eq!(
            candidate
                .coding_agent_profiles
                .default_profile_by_agent
                .remove("claude"),
            Some("A".to_string())
        );
        let saved = save_settings_to_path_preserving_project_paths(&candidate, &path).unwrap();
        let profiles = disk(&physical_path)["codingAgentProfiles"].clone();
        if protected {
            assert_eq!(profiles, expected_raw);
            assert_eq!(profiles["defaultProfileByAgent"]["claude"], json!("A"));
            assert_eq!(
                saved
                    .coding_agent_profiles
                    .default_profile_by_agent
                    .get("claude"),
                Some(&"A".to_string())
            );
        } else {
            assert!(
                profiles["defaultProfileByAgent"].get("claude").is_none(),
                "the editable removal must persist"
            );
            assert!(!saved
                .coding_agent_profiles
                .default_profile_by_agent
                .contains_key("claude"));
        }
        if protected {
            assert_eq!(
                profiles["profilesByAgent"].get("scratch-agent"),
                expected_raw["profilesByAgent"].get("scratch-agent")
            );
            assert!(
                saved.coding_agent_profiles.profiles_by_agent["scratch-agent"].contains_key("A")
            );
        } else {
            let scratch_hash =
                crate::config::settings::configuration_identity("scratch-agent", &[]);
            for key in ["profilesByBaseHash", "profileLabelsByBaseHash"] {
                assert!(profiles[key].is_object());
                assert!(profiles[key].get(&scratch_hash).is_none());
            }
            for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
                assert!(profiles.get(key).is_none());
            }
        }
        assert_eq!(
            profiles
                .get("legacyProfileOrphans")
                .and_then(|rows| rows.get("scratch-agent")),
            expected_raw
                .get("legacyProfileOrphans")
                .and_then(|rows| rows.get("scratch-agent"))
        );
        assert_eq!(std::fs::read(&overlay_path).unwrap(), overlay_before);
        assert_eq!(
            saved.coding_agent_profiles,
            load_settings_from_path(&path).coding_agent_profiles
        );
    }
}
