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
    };
    let path = requests.join(format!("{id}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&req).unwrap()).unwrap();
    path
}
