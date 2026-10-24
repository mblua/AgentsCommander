use super::*;

fn room_dir(temp: &tempfile::TempDir) -> PathBuf {
    let room = temp.path().join("room-1-dev-team");
    std::fs::create_dir_all(&room).expect("room dir");
    room
}

fn write_config(room: &Path, raw: &str) {
    let dir = co_managed_dir(room);
    std::fs::create_dir_all(&dir).expect("co-managed dir");
    std::fs::write(config_path(room), raw).expect("config.json");
}

fn write_catalog(room: &Path, name: &str, raw: &str) -> PathBuf {
    let path = room.join(name);
    std::fs::write(&path, raw).expect("catalog");
    path
}

/// Test 1: an absent `.co-managed/` directory is `RoomFlagOff`, creates nothing.
#[test]
fn absent_directory_is_flag_off_and_creates_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    let state = effective_state(&room, true, "key", true, true, "claude");
    assert_eq!(
        state,
        CoManagedState::Off {
            reason: OffReason::RoomFlagOff
        }
    );
    assert!(!co_managed_dir(&room).exists());
}

/// Test 2: enabled with an empty API key is `NoApiKey`.
#[test]
fn enabled_with_empty_key_is_no_api_key() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_config(&room, r#"{"enabled":true,"catalogPath":null}"#);
    assert_eq!(
        effective_state(&room, true, "", true, true, "claude"),
        CoManagedState::Off {
            reason: OffReason::NoApiKey
        }
    );
}

/// Test 3: enabled, key present, null catalog path is `NoCatalogFile`.
#[test]
fn enabled_without_catalog_path_is_no_catalog_file() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_config(&room, r#"{"enabled":true,"catalogPath":null}"#);
    assert_eq!(
        effective_state(&room, true, "key", true, true, "claude"),
        CoManagedState::Off {
            reason: OffReason::NoCatalogFile
        }
    );
}

/// Test 4: fully configured and parseable is `Ready`.
#[test]
fn fully_configured_room_is_ready() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_catalog(&room, "catalog.json", "{}");
    write_config(&room, r#"{"enabled":true,"catalogPath":"catalog.json"}"#);
    assert_eq!(
        effective_state(&room, true, "key", true, true, "claude"),
        CoManagedState::Ready
    );
}

/// A catalog that exists but does not parse is `CatalogUnreadable`.
#[test]
fn unparseable_catalog_is_catalog_unreadable() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_catalog(&room, "catalog.json", "not json at all");
    write_config(&room, r#"{"enabled":true,"catalogPath":"catalog.json"}"#);
    assert_eq!(
        effective_state(&room, true, "key", true, true, "claude"),
        CoManagedState::Off {
            reason: OffReason::CatalogUnreadable
        }
    );
}

/// A missing catalog is `NoCatalogFile`, not `CatalogUnreadable`.
#[test]
fn missing_catalog_is_no_catalog_file() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_config(
        &room,
        r#"{"enabled":true,"catalogPath":"missing-catalog.json"}"#,
    );
    assert_eq!(
        effective_state(&room, true, "key", true, true, "claude"),
        CoManagedState::Off {
            reason: OffReason::NoCatalogFile
        }
    );
}

/// Test 5: a non-orchestrator is rejected even when everything else is set.
#[test]
fn non_orchestrator_is_rejected_even_when_configured() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_catalog(&room, "catalog.json", "{}");
    write_config(&room, r#"{"enabled":true,"catalogPath":"catalog.json"}"#);
    assert_eq!(
        effective_state(&room, true, "key", false, true, "claude"),
        CoManagedState::Off {
            reason: OffReason::NotAnOrchestrator
        }
    );
}

/// Test 6: unknown keys survive `set_enabled`.
#[test]
fn set_enabled_preserves_unknown_keys() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_config(&room, r#"{"enabled":true,"futureKey":42}"#);
    let config = set_enabled(&room, false).expect("write succeeds");
    assert!(!config.enabled);

    let raw = std::fs::read_to_string(config_path(&room)).expect("config.json");
    let value: Value = serde_json::from_str(&raw).expect("valid json");
    assert_eq!(value[ENABLED_KEY], Value::Bool(false));
    assert_eq!(value["futureKey"], serde_json::json!(42));
}

/// Test 7: malformed JSON loads as defaults instead of an error.
#[test]
fn malformed_config_loads_as_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_config(&room, "not json at all");
    let config = load_config(&room);
    assert!(!config.enabled);
    assert_eq!(config.catalog_path, None);
}

/// Test 10: two concurrent writers serialise through the lock and never
/// leave a truncated file.
#[test]
fn concurrent_set_enabled_serialises_through_the_lock() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_config(&room, r#"{"enabled":false,"futureKey":42}"#);

    let first_room = room.clone();
    let second_room = room.clone();
    let first = std::thread::spawn(move || set_enabled(&first_room, true));
    let second = std::thread::spawn(move || set_enabled(&second_room, false));
    let first = first.join().expect("first thread").expect("first write");
    let second = second.join().expect("second thread").expect("second write");

    let raw = std::fs::read_to_string(config_path(&room)).expect("config.json");
    let value: Value = serde_json::from_str(&raw).expect("never a truncated file");
    let final_enabled = value[ENABLED_KEY].as_bool().expect("boolean");
    assert!(
        final_enabled == first.enabled || final_enabled == second.enabled,
        "the survivor must be exactly one of the two written values"
    );
    assert_eq!(value["futureKey"], serde_json::json!(42));
}

/// Test 11: an unsupported provider reports the offending agent label.
#[test]
fn unsupported_provider_carries_the_agent_label() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    write_catalog(&room, "catalog.json", "{}");
    write_config(&room, r#"{"enabled":true,"catalogPath":"catalog.json"}"#);
    assert_eq!(
        effective_state(&room, true, "key", true, false, "pi"),
        CoManagedState::Off {
            reason: OffReason::UnsupportedProvider {
                agent: "pi".to_string()
            }
        }
    );
}

/// Test 12: the fixed check order wins ties.
#[test]
fn non_orchestrator_wins_over_unsupported_provider() {
    let temp = tempfile::tempdir().unwrap();
    let room = room_dir(&temp);
    assert_eq!(
        effective_state(&room, true, "", false, false, "pi"),
        CoManagedState::Off {
            reason: OffReason::NotAnOrchestrator
        }
    );
}

/// Test 16: the wire shape is exactly the three literals phase 9 codes against.
#[test]
fn wire_shape_is_exactly_the_pinned_literals() {
    let ready = CoManagedState::Ready;
    let flag_off = CoManagedState::Off {
        reason: OffReason::RoomFlagOff,
    };
    let unsupported = CoManagedState::Off {
        reason: OffReason::UnsupportedProvider {
            agent: "pi".to_string(),
        },
    };

    assert_eq!(serde_json::to_string(&ready).unwrap(), "\"Ready\"");
    assert_eq!(
        serde_json::to_string(&flag_off).unwrap(),
        "{\"Off\":{\"reason\":\"RoomFlagOff\"}}"
    );
    assert_eq!(
        serde_json::to_string(&unsupported).unwrap(),
        "{\"Off\":{\"reason\":{\"UnsupportedProvider\":{\"agent\":\"pi\"}}}}"
    );

    for state in [ready, flag_off, unsupported] {
        let json = serde_json::to_string(&state).unwrap();
        let back: CoManagedState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
    }
}

/// `CoManagedConfig` keeps its camelCase wire names.
#[test]
fn config_wire_shape_is_camel_case() {
    let config = CoManagedConfig {
        enabled: true,
        catalog_path: Some("catalog.json".to_string()),
    };
    assert_eq!(
        serde_json::to_string(&config).unwrap(),
        "{\"enabled\":true,\"catalogPath\":\"catalog.json\"}"
    );
    let default = serde_json::to_string(&CoManagedConfig::default()).unwrap();
    assert_eq!(default, "{\"enabled\":false,\"catalogPath\":null}");
}

/// #2232 phase 7: the queue is a sibling of `config.json` and `state.json`,
/// directly under `.co-managed/`, never a subdirectory of an outbox.
#[test]
fn queue_dir_is_a_sibling_of_the_config_and_state_files() {
    let room = Path::new("/tmp/proj-a/.ac/room-1-dev-team");
    let queue = queue_dir(room);
    assert_eq!(queue, room.join(".co-managed").join("queue"));
    assert_eq!(queue.parent(), Some(co_managed_dir(room).as_path()));
    assert_eq!(queue.parent(), config_path(room).parent());
    assert_eq!(queue.parent(), lock_path(room).parent());
}

/// The room-root walk mirrors the messaging rule (`room-<digits>-*`).
#[test]
fn room_root_walk_matches_the_messaging_rule() {
    let temp = tempfile::tempdir().unwrap();
    let room = temp
        .path()
        .join("project-a")
        .join(".ac")
        .join("room-7-dev-team");
    let replica = room.join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();

    assert_eq!(room_root_for_path(&replica), Some(room.clone()));
    assert_eq!(room_root_for_path(&room), Some(room.clone()));
    assert_eq!(
        room_root_for_path(&temp.path().join("project-a")),
        None,
        "a project dir is not a room root"
    );
    assert_eq!(
        room_root_for_path(&temp.path().join("room-notes")),
        None,
        "the numeric run is required"
    );
}

fn ready_room(temp: &tempfile::TempDir) -> PathBuf {
    let room = room_dir(temp);
    write_catalog(&room, "catalog.json", "{}");
    write_config(&room, r#"{"enabled":true,"catalogPath":"catalog.json"}"#);
    room
}

/// The global switch masks a fully ready room; the same room with the
/// switch on is `Ready`, so the gate is proven to be the deciding input.
#[test]
fn global_switch_off_wins_over_every_other_reason() {
    let temp = tempfile::tempdir().unwrap();
    let room = ready_room(&temp);
    assert_eq!(
        effective_state(&room, false, "key", true, true, "claude"),
        CoManagedState::Off {
            reason: OffReason::GlobalSwitchOff
        }
    );
    assert_eq!(
        effective_state(&room, true, "key", true, true, "claude"),
        CoManagedState::Ready
    );
}

/// The global switch is checked before the orchestrator gate.
#[test]
fn global_switch_off_precedes_not_an_orchestrator() {
    let temp = tempfile::tempdir().unwrap();
    let room = ready_room(&temp);
    assert_eq!(
        effective_state(&room, false, "key", false, true, "claude"),
        CoManagedState::Off {
            reason: OffReason::GlobalSwitchOff
        }
    );
}

/// The switch masks; it never rewrites the room's `config.json`.
#[test]
fn global_switch_off_leaves_config_bytes_untouched() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let room = ready_room(&temp);
    let before = Sha256::digest(std::fs::read(config_path(&room)).unwrap());
    let _ = effective_state(&room, false, "key", true, true, "claude");
    let after = Sha256::digest(std::fs::read(config_path(&room)).unwrap());
    assert_eq!(before, after);
}

#[test]
fn globally_available_is_true_only_with_switch_on_and_a_key() {
    assert!(globally_available(true, "k"));
    assert!(!globally_available(false, "k"));
    assert!(!globally_available(true, ""));
}

#[test]
fn globally_available_treats_whitespace_as_absent() {
    assert!(!globally_available(true, "   "));
    assert!(!globally_available(true, "	"));
    assert!(!globally_available(
        true, "
"
    ));
}

#[test]
fn globally_available_accepts_a_padded_key() {
    assert!(globally_available(true, "  k  "));
}

/// #2525 D-d: `Ready` implies `globally_available`, so the sweep never
/// releases a demand an available room should hold.
#[test]
fn a_ready_room_is_never_globally_unavailable() {
    let temp = tempfile::tempdir().unwrap();
    let room = ready_room(&temp);
    assert_eq!(
        effective_state(&room, true, "test-key", true, true, "claude"),
        CoManagedState::Ready
    );
    for (enabled, key) in [(false, "test-key"), (true, ""), (true, "   ")] {
        assert_ne!(
            effective_state(&room, enabled, key, true, true, "claude"),
            CoManagedState::Ready,
            "({enabled}, {key:?})"
        );
        assert!(!globally_available(enabled, key), "({enabled}, {key:?})");
    }
}
