use super::*;

// Include directories as well as file bytes: absence, sidecars and mkdir are observable.
fn repair_tree(root: &Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(
        root: &Path,
        dir: &Path,
        entries: &mut std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>,
    ) {
        for entry in std::fs::read_dir(dir).expect("snapshot directory") {
            let path = entry.expect("snapshot entry").path();
            let bytes = if path.is_dir() {
                visit(root, &path, entries);
                None
            } else {
                Some(std::fs::read(&path).expect("snapshot bytes"))
            };
            entries.insert(
                path.strip_prefix(root).expect("fixture path").to_path_buf(),
                bytes,
            );
        }
    }
    let mut entries = std::collections::BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn reserve_repair_pair(replica: &Path) {
    use crate::config::local_config_io::{
        protected_config_pair_revision, read_config_pair_physical, PairReservationRequest,
    };
    let decisions = replica.join("config.json");
    let state = c4_state_path(&decisions);
    let before = [
        read_config_pair_physical(&decisions).expect("decisions image"),
        read_config_pair_physical(&state).expect("state image"),
    ];
    let request = PairReservationRequest {
        operation_id: "repair-authority".into(),
        source_physical_key: "repair-source".into(),
        mappings_digest: "a".repeat(64),
        before_protected: protected_config_pair_revision(&before).expect("protected revision"),
    };
    crate::config::agent_config::prepare_and_reserve_identity_reference_pair(
        &decisions,
        &request,
        |config, _| {
            config.insert("reservedChange".into(), Value::Bool(true));
            Ok(())
        },
    )
    .expect("reserve repair pair");
}

#[test]
fn repair_location_precedes_nonpending_cleanup_and_invalid_config() {
    for config in [None, Some("{invalid"), Some(C4_SEEDED_STATE)] {
        let temp = tempfile::tempdir().expect("fixture");
        let root = temp.path().join("workspace");
        let replica = root.join("room-2-dev-team").join("__agent_tech-lead");
        if let Some(config) = config {
            std::fs::create_dir_all(&replica).expect("replica");
            std::fs::write(replica.join("config.json"), config).expect("config");
            std::fs::write(root.join(".gitignore"), "# keep bytes").expect("ignore");
        }
        let before = repair_tree(temp.path());
        let error = read_and_repair_wg_replica_config(&replica, &[]).expect_err("location");
        assert_eq!(
            error,
            format!(
                "Room replica directory '{}' is not inside a Project AC Root",
                display_path(&replica)
            )
        );
        assert_eq!(repair_tree(temp.path()), before);
    }
}

#[test]
fn repair_missing_authoritative_root_does_not_create_it() {
    let temp = tempfile::tempdir().expect("fixture");
    let replica = temp.path().join(".ac/room-2-dev-team/__agent_tech-lead");
    let before = repair_tree(temp.path());
    let error = read_and_repair_wg_replica_config(&replica, &[]).expect_err("missing root");
    assert_eq!(
        error,
        format!(
            "project '{}' has no Project AC Root directory",
            temp.path().display()
        )
    );
    assert_eq!(repair_tree(temp.path()), before);
}

#[test]
fn repair_location_precedes_pending_binding_and_sidecars() {
    for existing_ignore in [false, true] {
        let temp = setup_replica(".ac");
        let project = temp.path().join("AgentsCommander_ac");
        let root = project.join(".ac");
        let relative = Path::new("wg-2-dev-team").join("__agent_tech-lead");
        let replica = root.join(&relative);
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_tech-lead"}"#,
        )
        .expect("config");
        reserve_repair_pair(&replica);
        // Retain the real packet, but remove setup sidecars to expose writer entry.
        for entry in std::fs::read_dir(&replica).expect("replica entries") {
            let path = entry.expect("entry").path();
            if path
                .extension()
                .is_some_and(|extension| extension == "lock")
            {
                std::fs::remove_file(path).expect("remove setup lock");
            }
        }
        std::fs::remove_file(root.join(".gitignore")).expect("remove generated ignore");
        if existing_ignore {
            std::fs::write(root.join(".gitignore"), "# exact user bytes").expect("ignore");
        }
        let invalid_root = project.join("workspace");
        std::fs::rename(&root, &invalid_root).expect("move own fixture root");
        let replica = invalid_root.join(relative);
        let before = repair_tree(temp.path());
        let error = read_and_repair_wg_replica_config(&replica, &[]).expect_err("location");
        assert_eq!(
            error,
            format!(
                "Room replica directory '{}' is not inside a Project AC Root",
                display_path(&replica)
            )
        );
        assert_eq!(repair_tree(temp.path()), before);
    }
}

#[test]
fn repair_valid_pending_pair_refuses_structural_repair_without_changing_bytes() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac/.ac/wg-2-dev-team/__agent_tech-lead");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../../../old/.ac/_agent_tech-lead","context":["notes.md"]}"#,
    )
    .expect("stale config");
    reserve_repair_pair(&replica);
    let before = repair_tree(temp.path());
    let error = read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
        .expect_err("pending structural repair");
    assert_eq!(error, "targetTransitionPending: PAIR is reserved");
    assert_eq!(repair_tree(temp.path()), before);
}

#[test]
#[cfg(windows)]
fn replica_identity_path_helpers_convert_verbatim_unc() {
    let path = PathBuf::from(r"\\?\UNC\server\share\repo");
    assert_eq!(strip_unc(path), PathBuf::from(r"\\server\share\repo"));
    assert_eq!(
        display_path(Path::new(r"\\?\UNC\server\share\repo")),
        r"\\server\share\repo"
    );
}

fn setup_replica(project_ac_root_name: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp
        .path()
        .join("AgentsCommander_ac")
        .join(project_ac_root_name);
    let matrix = ac_root.join("_agent_tech-lead");
    let replica = ac_root.join("wg-2-dev-team").join("__agent_tech-lead");
    std::fs::create_dir_all(&matrix).expect("create matrix");
    std::fs::create_dir_all(&replica).expect("create replica");
    std::fs::write(matrix.join(ROLE_MD_FILENAME), "# Tech Lead\n").expect("write Role.md");
    temp
}

#[test]
fn expected_identity_is_same_ac_root_for_canonical_ac() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    let stale_sibling = temp
        .path()
        .join("agentscommander-old")
        .join(".ac")
        .join("_agent_tech-lead");
    std::fs::create_dir_all(stale_sibling).expect("create stale sibling");

    let identity = expected_wg_replica_identity(&replica).expect("identity");

    assert_eq!(identity.identity, "../../_agent_tech-lead");
    assert!(identity.matrix_dir.ends_with(
        Path::new("AgentsCommander_ac")
            .join(".ac")
            .join("_agent_tech-lead")
    ));
}

#[test]
fn read_and_repair_rewrites_stale_identity_and_role_context() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    let stale = temp
        .path()
        .join("agentscommander-old")
        .join(".ac")
        .join("_agent_tech-lead")
        .to_string_lossy()
        .replace('\\', "/");
    std::fs::write(
        replica.join("config.json"),
        serde_json::json!({
            "identity": stale,
            "context": [
                "$AGENTSCOMMANDER_CONTEXT",
                "../../../../agentscommander-old/.ac/_agent_tech-lead/Role.md",
                "notes.md",
                "../../_agent_tech-lead/Role.md"
            ],
            "repos": []
        })
        .to_string(),
    )
    .expect("write config");

    let (config, identity) =
        read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
            .expect("repair config");

    assert_eq!(identity.identity, "../../_agent_tech-lead");
    assert_eq!(config["identity"], "../../_agent_tech-lead");
    assert_eq!(
        config["context"],
        serde_json::json!([
            "$AGENTSCOMMANDER_CONTEXT",
            "notes.md",
            "../../_agent_tech-lead/Role.md"
        ])
    );

    let persisted: Value = serde_json::from_str(
        &std::fs::read_to_string(replica.join("config.json")).expect("read config"),
    )
    .expect("parse config");
    assert_eq!(persisted["identity"], "../../_agent_tech-lead");
    assert!(!persisted.to_string().contains("agentscommander-old"));
}

#[test]
fn repair_rejects_identity_for_different_agent_basename() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../../../agentscommander-old/.ac/_agent_architect"}"#,
    )
    .expect("write config");

    let err = read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
        .expect_err("mismatched identity must reject");

    assert!(err.contains("_agent_architect"), "{err}");
    assert!(err.contains("_agent_tech-lead"), "{err}");
}

#[test]
fn repair_rejects_bare_persisted_identity_component() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    std::fs::write(replica.join("config.json"), r#"{"identity":"tech-lead"}"#)
        .expect("write config");

    let err = read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
        .expect_err("bare identity basename must reject");

    assert!(err.contains("must end in '_agent_<name>'"), "{err}");
    let persisted: Value = serde_json::from_str(
        &std::fs::read_to_string(replica.join("config.json")).expect("read config"),
    )
    .expect("parse config");
    assert_eq!(persisted["identity"], "tech-lead");
}

#[test]
fn repair_rejects_replica_persisted_identity_component() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../__agent_tech-lead"}"#,
    )
    .expect("write config");

    let err = read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
        .expect_err("replica identity basename must reject");

    assert!(err.contains("must end in '_agent_<name>'"), "{err}");
}

#[cfg(windows)]
#[test]
fn repair_accepts_windows_case_variant_persisted_identity_component() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../../../agentscommander-old/.ac/_Agent_Tech-Lead"}"#,
    )
    .expect("write config");

    let (config, identity) =
        read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT)
            .expect("Windows identity basename matching is case-insensitive");

    assert_eq!(identity.identity, "../../_agent_tech-lead");
    assert_eq!(config["identity"], "../../_agent_tech-lead");
}

/// #2557 - seven replicas, one per fault cause; two share ConfigUnreadable.
fn issue_2557_fault_cases(temp: &tempfile::TempDir) -> Vec<(PathBuf, ReplicaConfigFault)> {
    let room = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team");
    let replica = |dir: &str, config: Option<&[u8]>| {
        let path = room.join(dir);
        std::fs::create_dir_all(&path).expect("create replica");
        // Only the location case lacks a valid Matrix; the others need one
        // so the identity checks are reached.
        if let Some(agent) = dir.strip_prefix("__agent_") {
            let matrix = room
                .parent()
                .expect("ac root")
                .join(format!("_agent_{agent}"));
            std::fs::create_dir_all(&matrix).expect("create matrix");
            std::fs::write(matrix.join(ROLE_MD_FILENAME), "# Role\n").expect("write Role.md");
        }
        if let Some(bytes) = config {
            std::fs::write(path.join("config.json"), bytes).expect("write config");
        }
        path
    };
    let dir_config = replica("__agent_dir-config", None);
    std::fs::create_dir(dir_config.join("config.json")).expect("config.json as a dir");
    vec![
        (
            replica("__agent_no-config", None),
            ReplicaConfigFault::ConfigUnreadable,
        ),
        (dir_config, ReplicaConfigFault::ConfigUnreadable),
        (
            replica("__agent_not-json", Some(b"{not json")),
            ReplicaConfigFault::ConfigNotJson,
        ),
        (
            replica("__agent_array", Some(b"[]")),
            ReplicaConfigFault::ConfigNotObject,
        ),
        (
            replica("__agent_no-identity", Some(b"{}")),
            ReplicaConfigFault::IdentityMissing,
        ),
        (
            replica(
                "__agent_tech-lead",
                Some(br#"{"identity":"../../_agent_otro"}"#),
            ),
            ReplicaConfigFault::IdentityMismatch,
        ),
        (
            replica("plain", Some(br#"{"identity":"../../_agent_plain"}"#)),
            ReplicaConfigFault::LocationInvalid,
        ),
    ]
}

#[test]
fn issue_2557_each_fault_cause_gets_its_own_code() {
    let temp = setup_replica(".ac");
    let cases = issue_2557_fault_cases(&temp);
    let mut seen = HashSet::new();
    for (dir, expected) in &cases {
        let (fault, _) =
            read_wg_replica_config_read_only_classified(dir).expect_err("every case must fail");
        assert_eq!(fault, *expected, "{}", dir.display());
        seen.insert(format!("{fault:?}"));
    }
    assert_eq!(cases.len(), 7);
    assert_eq!(seen.len(), 6, "{seen:?}");
}

#[test]
fn issue_2557_wrapper_message_is_byte_identical() {
    let temp = setup_replica(".ac");
    for (dir, _) in issue_2557_fault_cases(&temp) {
        let (_, classified) =
            read_wg_replica_config_read_only_classified(&dir).expect_err("classified fails");
        let plain = read_wg_replica_config_read_only(&dir).expect_err("wrapper fails");
        assert_eq!(plain, classified, "{}", dir.display());
    }
}

#[test]
fn issue_2557_classified_reader_records_one_probe_call() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    std::fs::write(
        replica.join("config.json"),
        br#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .expect("write config");
    strict_read_probe::register(temp.path());
    let read = read_wg_replica_config_read_only(&replica);
    let count = strict_read_probe::unregister(temp.path());
    read.expect("valid replica reads");
    assert_eq!(count, 1);
}

/// #2816 (C3) E3b, site 12.
#[test]
fn an_absent_config_still_refuses_and_creates_nothing() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    let config = replica.join("config.json");

    let error = read_and_repair_wg_replica_config(&replica, &[]).expect_err("absent config");

    assert_eq!(
        error,
        format!("Local config {} does not exist", config.display())
    );
    assert!(!config.exists(), "site 12 created config.json");
    assert!(
        !replica
            .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
            .exists(),
        "site 12 created a state file"
    );
}

/// #2816 (C3) E3b, consequence 2 of 4.1: an orphan state file with a
/// malformed `tooling` is rejected by the cleanup before the refusal, and
/// still nothing is created.
#[test]
fn an_orphan_malformed_state_file_is_rejected_before_the_refusal() {
    let temp = setup_replica(".ac");
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    let state = replica.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME);
    std::fs::write(&state, r#"{"tooling":5}"#).expect("seed state");

    let error = read_and_repair_wg_replica_config(&replica, &[]).expect_err("orphan state");

    assert!(
        error.ends_with("cleanup failed: 'tooling' must be a JSON object in the state file"),
        "{error}"
    );
    assert!(!replica.join("config.json").exists());
    assert_eq!(
        std::fs::read_to_string(&state).expect("read state"),
        r#"{"tooling":5}"#
    );
}

/// C4 (#2470): the seeded state file of every row that seeds one. All four
/// state keys, plus the exact D7 marker `stamp_split_marker` writes.
const C4_SEEDED_STATE: &str = include_str!("tests/fixtures/c4-seeded-state.json");

/// C4 (#2470): the state file beside a tracked `config.json`.
fn c4_state_path(tracked: &std::path::Path) -> std::path::PathBuf {
    tracked.with_file_name(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
}

/// C4 (#2470): after one writer, the tracked file carries none of the four
/// state keys. This says nothing at all about the state file: `c4_state_is`
/// is the only helper that reads it, and the row calls them in that order.
fn c4_tracked_is_clean(tracked: &std::path::Path, site: &str) {
    let text = std::fs::read_to_string(tracked).expect("tracked file");
    let value: serde_json::Value = serde_json::from_str(&text).expect("tracked json");
    let tooling = value.get("tooling");
    c4_absent(tooling, "lastCodingAgent", site, &value);
    c4_absent(tooling, "codingAgents", site, &value);
    c4_absent(tooling, "lastAgentMessageAt", site, &value);
    c4_absent(tooling, "profileContentHash", site, &value);
}

fn c4_absent(t: Option<&serde_json::Value>, key: &str, site: &str, all: &serde_json::Value) {
    assert!(
        t.and_then(|t| t.get(key)).is_none(),
        "{site} left {key}: {all}"
    );
}

/// C4 (#2470): the state file EXISTS and is exactly `expected`, compared
/// as parsed JSON so the pretty-printing of `publish_pair_side` is never
/// the subject. `expected` is always a literal written in the row. Absence
/// is NOT admitted here and no longer reads as JSON `null`:
/// `c4_no_state_file` is the only helper that may conclude absence.
fn c4_state_is(state: &std::path::Path, expected: &str, site: &str) {
    let want: serde_json::Value = serde_json::from_str(expected).expect("expected json");
    let raw = std::fs::read_to_string(state).expect("state file");
    let got: serde_json::Value = serde_json::from_str(&raw).expect("state json");
    assert_eq!(got, want, "{site} state file");
}

/// C4 (#2470) E13, site 12: the repair rewrites the tracked file and leaves
/// the state file alone.
#[test]
fn c4_site_12_leaves_no_state_key_in_the_tracked_file() {
    let temp = setup_replica(".ac");
    let journal = tempfile::tempdir().expect("journal dir");
    let _journal = crate::config::agent_config::journal_redirect::set(journal.path());
    let replica = temp
        .path()
        .join("AgentsCommander_ac")
        .join(".ac")
        .join("wg-2-dev-team")
        .join("__agent_tech-lead");
    let stale = temp
        .path()
        .join("agentscommander-old")
        .join(".ac")
        .join("_agent_tech-lead")
        .to_string_lossy()
        .replace('\\', "/");
    let tracked = replica.join("config.json");
    std::fs::write(
        &tracked,
        serde_json::json!({
            "identity": stale,
            "context": [
                "$AGENTSCOMMANDER_CONTEXT",
                "../../../../agentscommander-old/.ac/_agent_tech-lead/Role.md",
                "notes.md",
                "../../_agent_tech-lead/Role.md"
            ],
            "repos": [],
            "tooling": {}
        })
        .to_string(),
    )
    .expect("write config");
    std::fs::write(c4_state_path(&tracked), C4_SEEDED_STATE).expect("seed state");

    let (config, identity) =
        read_and_repair_wg_replica_config(&replica, WG_REPLICA_REQUIRED_CONTEXT).expect("site 12");

    c4_tracked_is_clean(&tracked, "site 12");
    c4_state_is(&c4_state_path(&tracked), C4_SEEDED_STATE, "site 12");
    assert_eq!(identity.identity, "../../_agent_tech-lead");
    assert_eq!(config["identity"], "../../_agent_tech-lead");
    let raw = std::fs::read_to_string(&tracked).expect("read config");
    let on_disk: Value = serde_json::from_str(&raw).expect("parse config");
    assert_eq!(on_disk["identity"], "../../_agent_tech-lead");
}
