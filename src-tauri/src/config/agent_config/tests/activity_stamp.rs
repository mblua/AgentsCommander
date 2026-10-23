use super::*;

pub(super) const T1: &str = "2026-09-02T01:00:00+00:00";
pub(super) const T2: &str = "2026-09-02T02:00:00+00:00";
const T3: &str = "2026-09-02T03:00:00+00:00";

/// Path of the per-instance config the writer under test targets.
pub(super) fn instance_config(dir: &Path) -> PathBuf {
    dir.join(crate::config::agent_local_dir_name())
        .join("config.json")
}

/// Seed `dir`'s per-instance config with `value`, creating the instance dir.
pub(super) fn seed_instance_config(dir: &Path, value: &serde_json::Value) {
    let path = instance_config(dir);
    std::fs::create_dir_all(path.parent().expect("instance dir")).expect("create instance dir");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(value).expect("serialize"),
    )
    .expect("seed config");
}

/// Raw JSON currently stored in `dir`'s per-instance config.
pub(super) fn stored(dir: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(instance_config(dir)).expect("read stored config");
    serde_json::from_str(&raw).expect("stored config is JSON")
}

/// #2816 (C3) - raw JSON of the state file beside the tracked `config`.
pub(super) fn state_beside(config: &Path) -> serde_json::Value {
    let path = state_file_path(config.parent().expect("config dir"));
    let raw = std::fs::read_to_string(path).expect("read state file");
    serde_json::from_str(&raw).expect("state file is JSON")
}

/// #2816 (C3) - no state key may remain in the tracked `config`; an absent
/// tracked file holds none.
pub(super) fn assert_no_state_key_tracked(config: &Path) {
    let Ok(raw) = std::fs::read_to_string(config) else {
        return;
    };
    let tracked: serde_json::Value = serde_json::from_str(&raw).expect("tracked config is JSON");
    for key in STATE_KEYS {
        assert!(
            tracked["tooling"].get(key).is_none(),
            "{key} is still tracked in {}",
            config.display()
        );
    }
}

pub(super) fn set(dir: &Path, at: &str) -> Result<bool, String> {
    set_last_agent_message_at(dir.to_str().expect("utf-8 temp path"), at)
}

pub(super) fn read(dir: &Path) -> Option<String> {
    read_last_agent_message_at(dir.to_str().expect("utf-8 temp path"))
}

#[test]
fn set_last_agent_message_at_writes_only_the_instance_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    assert_eq!(set(dir, T2), Ok(true));

    assert_eq!(
        state_beside(&instance_config(dir))["tooling"]["lastAgentMessageAt"],
        serde_json::json!(T2)
    );
    assert_no_state_key_tracked(&instance_config(dir));
    // D2: the root copy is deliberately not a write target for this stamp.
    assert!(
        !dir.join("config.json").exists(),
        "the root config must not be written"
    );
}

#[test]
fn set_last_agent_message_at_preserves_existing_tooling_and_unknown_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    seed_instance_config(
        dir,
        &serde_json::json!({
            "tooling": {
                "lastCodingAgent": "claude",
                "codingAgents": {
                    "claude": { "app": "Claude Code", "lastUsed": T1 }
                }
            },
            "repos": ["repo-AgentsCommander"]
        }),
    );

    assert_eq!(set(dir, T2), Ok(true));

    let after = state_beside(&instance_config(dir));
    assert_no_state_key_tracked(&instance_config(dir));
    assert_eq!(
        after["tooling"]["lastCodingAgent"],
        serde_json::json!("claude")
    );
    assert_eq!(
        after["tooling"]["codingAgents"]["claude"],
        serde_json::json!({ "app": "Claude Code", "lastUsed": T1 })
    );
    assert_eq!(
        stored(dir)["repos"],
        serde_json::json!(["repo-AgentsCommander"])
    );
    assert_eq!(
        after["tooling"]["lastAgentMessageAt"],
        serde_json::json!(T2)
    );
}

#[test]
fn set_last_agent_message_at_is_monotonic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    assert_eq!(set(dir, T2), Ok(true));

    // Older instant: skipped, stored value untouched.
    assert_eq!(set(dir, T1), Ok(false));
    assert_eq!(read(dir), Some(T2.to_string()));

    // Newer instant: written.
    assert_eq!(set(dir, T3), Ok(true));
    assert_eq!(read(dir), Some(T3.to_string()));

    // An unparseable stored value is not a reason to skip.
    seed_instance_config(
        dir,
        &serde_json::json!({ "tooling": { "lastAgentMessageAt": "not-a-timestamp" } }),
    );
    assert_eq!(set(dir, T1), Ok(true));
    assert_eq!(read(dir), Some(T1.to_string()));
}

#[test]
fn a_session_restart_rewrite_preserves_the_stamp() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let repo = dir.to_str().expect("utf-8 temp path");

    assert_eq!(set(dir, T2), Ok(true));
    set_last_coding_agent(repo, "claude", "Claude Code", Some("sid"), None)
        .expect("restart rewrite");

    assert_eq!(read(dir), Some(T2.to_string()));
    assert_eq!(
        state_beside(&instance_config(dir))["tooling"]["lastCodingAgent"],
        serde_json::json!("claude")
    );
    assert_no_state_key_tracked(&instance_config(dir));
}

#[test]
fn read_last_agent_message_at_binds_present_and_absent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    // Absent file.
    assert_eq!(read(dir), None);

    // Invalid JSON.
    let path = instance_config(dir);
    std::fs::create_dir_all(path.parent().expect("instance dir")).expect("create instance dir");
    std::fs::write(&path, "{ not json").expect("write invalid json");
    assert_eq!(read(dir), None);

    // Tooling absent.
    seed_instance_config(dir, &serde_json::json!({}));
    assert_eq!(read(dir), None);

    // Tooling present without the key.
    seed_instance_config(
        dir,
        &serde_json::json!({ "tooling": { "lastCodingAgent": "claude" } }),
    );
    assert_eq!(read(dir), None);

    // Tooling set to a non-object.
    seed_instance_config(dir, &serde_json::json!({ "tooling": 5 }));
    assert_eq!(read(dir), None);

    // #2816 (C3) consequence 5: the write on that seed is rejected by the
    // pair's cleanup and the tracked file is left byte for byte.
    let tracked = instance_config(dir);
    assert!(
        !state_file_path(tracked.parent().expect("instance dir")).exists(),
        "no state file, so the tracked check is the one measured"
    );
    let before = std::fs::read(&tracked).expect("tracked bytes before");
    let error = set(dir, T2).expect_err("a malformed tracked tooling is rejected");
    assert!(
        error
            .contains("cleanup failed: 'tooling' must be a JSON object in the tracked config file"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(&tracked).expect("tracked bytes after"),
        before
    );
    assert_eq!(stored(dir)["tooling"], serde_json::json!(5));

    // Control: a well-formed value is read back.
    seed_instance_config(dir, &serde_json::json!({}));
    assert_eq!(set(dir, T2), Ok(true));
    assert_eq!(read(dir), Some(T2.to_string()));
}

#[test]
fn is_empty_tracks_the_stamp() {
    assert!(AgentTooling::default().is_empty());
    assert!(!AgentTooling {
        last_agent_message_at: Some(T2.to_string()),
        ..Default::default()
    }
    .is_empty());
}
