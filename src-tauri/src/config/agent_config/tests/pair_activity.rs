use super::*;
use crate::config::local_config_io::{
    execute_reserved_config_pair, protected_config_pair_revision, read_config_pair_physical,
    PairReservationRequest,
};
use serde_json::json;

const T1: &str = "2026-10-08T01:00:00Z";
const T2: &str = "2026-10-08T02:00:00Z";
const T3: &str = "2026-10-08T03:00:00Z";

#[test]
fn pair_activity_nonpending_ignore_preflight_survives_read_error_and_blocks_on_failure() {
    use crate::config::instance_artifacts::{
        CONFIG_IDENTITY_RESERVATION_LOCK, CONFIG_IDENTITY_RESERVATION_NAME,
    };
    for ignore_failure in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let ac = root.path().join(".ac");
        let matrix = ac.join("_agent-test");
        std::fs::create_dir_all(&matrix).unwrap();
        let d = matrix.join("config.json");
        let s = state_file_path(&matrix);
        let r = matrix.join(CONFIG_IDENTITY_RESERVATION_NAME);
        std::fs::write(&d, b"{invalid").unwrap();
        let ignore = ac.join(".gitignore");
        if ignore_failure {
            std::fs::create_dir(&ignore).unwrap();
        }
        let error = update_agent_config_in(&d, None, |_, _| panic!("must fail before mutation"))
            .unwrap_err();
        let state_lock = matrix.join(format!(
            ".{}.lock",
            s.file_name().unwrap().to_str().unwrap()
        ));
        if ignore_failure {
            assert!(error.contains("failed to read"));
            assert!(!state_lock.exists());
            assert!(!matrix.join(CONFIG_IDENTITY_RESERVATION_LOCK).exists());
        } else {
            assert!(error.contains("blocked"));
            let content = std::fs::read_to_string(ignore).unwrap();
            for (row, _) in crate::config::naming_migration::config_state_ignore_rows() {
                assert!(content.lines().any(|line| line == row));
            }
            assert!(content
                .lines()
                .any(|line| line == format!("**/{CONFIG_IDENTITY_RESERVATION_LOCK}")));
            assert!(state_lock.is_file());
            assert!(matrix.join(CONFIG_IDENTITY_RESERVATION_LOCK).is_file());
        }
        assert_eq!(std::fs::read(d).unwrap(), b"{invalid");
        assert!(!s.exists());
        assert!(!r.exists());
    }
}

#[test]
fn pair_activity_pending_ignore_bytes_and_absence_are_preserved() {
    for existing_ignore in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let ac = root.path().join(".ac");
        let matrix = ac.join("_agent-test");
        std::fs::create_dir_all(&matrix).unwrap();
        let (d, _, _) = prepare(&matrix);
        let ignore = ac.join(".gitignore");
        if ignore.exists() {
            std::fs::remove_file(&ignore).unwrap();
        }
        let original = b"# unrelated bytes without trailing newline";
        if existing_ignore {
            std::fs::write(&ignore, original).unwrap();
        }
        assert_eq!(
            set_last_agent_message_at(matrix.to_str().unwrap(), T2),
            Ok(true)
        );
        update_agent_config_in(&d, None, |_, _| Ok(())).unwrap();
        let error = update_agent_config_in(&d, None, |d, _| {
            d.insert("history".into(), json!([]));
            Ok(())
        })
        .unwrap_err();
        assert!(error.contains("targetTransitionPending"));
        if existing_ignore {
            assert_eq!(std::fs::read(ignore).unwrap(), original);
        } else {
            assert!(!ignore.exists());
        }
    }
}

fn prepare(root: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let instance = root.join(crate::config::agent_local_dir_name());
    std::fs::create_dir_all(&instance).unwrap();
    let d = instance.join("config.json");
    let s = state_file_path(&instance);
    std::fs::write(
        &d,
        serde_json::to_vec(&json!({"name":"before","tooling":{"lastAgentMessageAt":T1}})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        set_last_agent_message_at(root.to_str().unwrap(), T1),
        Ok(false)
    );
    let before = [
        read_config_pair_physical(&d).unwrap(),
        read_config_pair_physical(&s).unwrap(),
    ];
    let request = PairReservationRequest {
        operation_id: "operation-1".into(),
        source_physical_key: "source-key".into(),
        mappings_digest: "a".repeat(64),
        before_protected: protected_config_pair_revision(&before).unwrap(),
    };
    prepare_and_reserve_identity_reference_pair(&d, &request, |d, _| {
        d.insert("name".into(), json!("after"));
        Ok(())
    })
    .unwrap();
    let r = instance.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME);
    (d, s, r)
}

#[test]
fn pair_activity_public_setter_history_selection_noop_and_root_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().to_str().unwrap();
    let root_config = root.path().join("config.json");
    std::fs::write(
        &root_config,
        br#"{"tooling":{"lastAgentMessageAt":"2026-10-08T00:00:00Z"},"unknown":null}"#,
    )
    .unwrap();
    let root_bytes = std::fs::read(&root_config).unwrap();
    let (d, s, r) = prepare(root.path());
    let d_bytes = std::fs::read(&d).unwrap();
    assert_eq!(set_last_agent_message_at(repo, T2), Ok(true));
    assert_eq!(set_last_agent_message_at(repo, T3), Ok(true));
    assert_eq!(set_last_agent_message_at(repo, T1), Ok(false));
    assert_eq!(read_last_agent_message_at(repo).as_deref(), Some(T3));
    let s_bytes = std::fs::read(&s).unwrap();
    let r_bytes = std::fs::read(&r).unwrap();
    update_agent_config(&d, |_, _| Ok(())).unwrap();
    let error =
        set_last_coding_agent(repo, "claude", "Claude Code", Some("sid"), None).unwrap_err();
    assert!(error.contains("targetTransitionPending"));
    let error = update_existing_agent_config(&d, |d, _| {
        d.get_mut("tooling")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("selectionLocked".into(), json!(true));
        Ok(())
    })
    .unwrap_err();
    assert!(error.contains("targetTransitionPending"));
    assert!(set_last_agent_message_at(repo, "invalid").is_err());
    assert_eq!(std::fs::read(&d).unwrap(), d_bytes);
    assert_eq!(std::fs::read(&s).unwrap(), s_bytes);
    assert_eq!(std::fs::read(&r).unwrap(), r_bytes);
    assert_eq!(std::fs::read(&root_config).unwrap(), root_bytes);
    execute_reserved_config_pair(&d, &s, &r, "operation-1").unwrap();
    assert_eq!(read_last_agent_message_at(repo).as_deref(), Some(T3));
    assert_eq!(std::fs::read(root_config).unwrap(), root_bytes);
}

#[test]
fn pair_activity_reader_landed_max_never_announces_intent_only_stamp() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().to_str().unwrap();
    let (d, s, r) = prepare(root.path());
    let mut packet: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&r).unwrap()).unwrap();
    packet["activityIntent"] = json!({"beforeTimestampPhysical":{"kind":"value","value":T1},
        "afterTimestamp":T3,"floorBefore":T1});
    std::fs::write(&r, serde_json::to_vec(&packet).unwrap()).unwrap();
    assert_eq!(read_last_agent_message_at(repo).as_deref(), Some(T1));
    assert_eq!(set_last_agent_message_at(repo, T2), Ok(false));
    assert_eq!(read_last_agent_message_at(repo).as_deref(), Some(T3));
    let landed: serde_json::Value = serde_json::from_slice(&std::fs::read(&s).unwrap()).unwrap();
    assert_eq!(landed["tooling"]["lastAgentMessageAt"], json!(T3));
    let d_before = std::fs::read(&d).unwrap();
    // The reader chooses max even when the legacy split marker would have
    // caused the old merged projection to prefer the tracked timestamp.
    std::fs::write(
        &d,
        serde_json::to_vec(&json!({"tooling":{"lastAgentMessageAt":T1}})).unwrap(),
    )
    .unwrap();
    assert_eq!(read_last_agent_message_at(repo).as_deref(), Some(T3));
    assert_ne!(std::fs::read(&d).unwrap(), d_before);
}
#[test]
fn p22_agent_guard_preflight_refuses_before_ignore_or_mutation() {
    let root = tempfile::tempdir().unwrap();
    let ac = root.path().join(".ac");
    let matrix = ac.join("_agent-test");
    std::fs::create_dir_all(&matrix).unwrap();
    let decisions = matrix.join("config.json");
    std::fs::write(&decisions, b"{}").unwrap();
    let before = [
        read_config_pair_physical(&decisions).unwrap(),
        read_config_pair_physical(&state_file_path(&matrix)).unwrap(),
    ];
    let request = PairReservationRequest {
        operation_id: "operation-guard".into(),
        source_physical_key: "source-key".into(),
        mappings_digest: "a".repeat(64),
        before_protected: protected_config_pair_revision(&before).unwrap(),
    };
    assert_eq!(
        prepare_and_reserve_identity_reference_pair_guarded(
            &decisions,
            &request,
            |_, _| panic!("no mutation after failed preflight"),
            &|| Err(crate::config::local_config_io::PreparedPairError::Conflict),
        ),
        Err(crate::config::local_config_io::PreparedPairError::Conflict)
    );
    assert!(!ac.join(".gitignore").exists());
    assert_eq!(std::fs::read(decisions).unwrap(), b"{}");
    assert!(!state_file_path(&matrix).exists());
}

#[test]
fn p22_agent_allowing_guard_reuses_policy_and_adapter_packet() {
    let root = tempfile::tempdir().unwrap();
    let ac = root.path().join(".ac");
    let matrix = ac.join("_agent-test");
    std::fs::create_dir_all(&matrix).unwrap();
    let decisions = matrix.join("config.json");
    let state = state_file_path(&matrix);
    let reservation =
        matrix.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME);
    std::fs::write(&decisions, b"{\"name\":\"before\"}").unwrap();
    let user_rows = "# user rows\nkeep-me/\n!keep-me/allowed\n";
    std::fs::write(ac.join(".gitignore"), user_rows).unwrap();
    assert_eq!(nearest_ac_root(&decisions), Some(ac.as_path()));
    let before = [
        read_config_pair_physical(&decisions).unwrap(),
        read_config_pair_physical(&state).unwrap(),
    ];
    let request = PairReservationRequest {
        operation_id: "operation-1".into(),
        source_physical_key: "source-key".into(),
        mappings_digest: "a".repeat(64),
        before_protected: protected_config_pair_revision(&before).unwrap(),
    };
    prepare_and_reserve_identity_reference_pair(&decisions, &request, |d, _| {
        d.insert("name".into(), json!("after"));
        Ok(())
    })
    .unwrap();
    let packet = std::fs::read(&reservation).unwrap();
    let ignore = std::fs::read_to_string(ac.join(".gitignore")).unwrap();
    let mut expected = user_rows.to_string();
    expected.push_str(&crate::config::naming_migration::missing_ignore_rows(
        user_rows,
        &crate::config::naming_migration::config_state_ignore_rows(),
    ));
    for name in [
        crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME,
        crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_LOCK,
        crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_TMP,
    ] {
        expected.push_str(&format!(
            "# AgentsCommander: private PAIR reservation state\n**/{name}\n"
        ));
    }
    assert_eq!(ignore, expected);
    assert!(ignore.starts_with(user_rows));
    prepare_and_reserve_identity_reference_pair_guarded(
        &decisions,
        &request,
        |d, _| {
            d.insert("name".into(), json!("after"));
            Ok(())
        },
        &|| Ok(()),
    )
    .unwrap();
    assert_eq!(std::fs::read(reservation).unwrap(), packet);
    assert_eq!(
        std::fs::read_to_string(ac.join(".gitignore")).unwrap(),
        ignore
    );
    assert_eq!(
        [
            read_config_pair_physical(&decisions).unwrap(),
            read_config_pair_physical(&state).unwrap()
        ],
        before
    );
}
