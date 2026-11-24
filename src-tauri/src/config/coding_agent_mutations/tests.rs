use super::*;
use crate::config::settings::CodingAgentEnvSource;
use crate::pty::backend::SessionBackendKind;

mod agent_ops;
mod agent_order;
mod p02_patch_schema;
mod preflight;
mod queue_handler;
mod source_status;
mod source_transition_driver;

fn agent(id: &str, label: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: label.to_string(),
        command: command.to_string(),
        color: "#6366f1".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

fn add(agent: AgentConfig) -> CodingAgentOp {
    CodingAgentOp::Add { agent }
}

fn noop_save() -> impl FnMut(&AppSettings) -> Result<AppSettings, String> {
    |s: &AppSettings| Ok(s.clone())
}

fn write_request_file(requests: &Path, id: &str, created_at_ms: u64, op: CodingAgentOp) -> PathBuf {
    std::fs::create_dir_all(requests).unwrap();
    let req = CodingAgentRequest {
        request_id: id.to_string(),
        created_at_ms,
        op,
        context: None,
    };
    let path = requests.join(format!("{id}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&req).unwrap()).unwrap();
    path
}

// #4845 P04 - the optional `context` field is additive on the request wire.

#[test]
fn request_without_context_field_deserializes_as_none() {
    let op = serde_json::to_value(CodingAgentOp::Remove { id: "a".into() }).unwrap();
    let older = serde_json::json!({"requestId": "r", "createdAtMs": 1, "op": op});
    let req: CodingAgentRequest = serde_json::from_value(older).unwrap();
    assert_eq!(req.context, None);
}

#[test]
fn request_with_none_context_serializes_without_the_key() {
    let req = CodingAgentRequest {
        request_id: "r".into(),
        created_at_ms: 1,
        op: CodingAgentOp::Remove { id: "a".into() },
        context: None,
    };
    let value = serde_json::to_value(&req).unwrap();
    let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["createdAtMs", "op", "requestId"]);
}
