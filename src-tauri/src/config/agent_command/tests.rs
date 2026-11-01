use super::*;
use crate::config::coding_agent_profiles::ProfileResolution;
use crate::config::settings::{
    empty_profile_cell, AgentBackendConfig, AgentConfig, AppSettings, CodingAgentEnv,
    CodingAgentEnvSource, ConfigSeedConfig, ProfileCellConfig,
};
use std::collections::BTreeMap;

pub(super) fn agent(id: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
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

fn spawn_with_env(
    child_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
) -> AgentSpawnCommand {
    AgentSpawnCommand {
        shell: "codex".to_string(),
        shell_args: Vec::new(),
        agent_env: BTreeMap::new(),
        profile_env: BTreeMap::new(),
        generated_env: BTreeMap::new(),
        child_env,
        env_remove_keys,
        effective_codex_home: None,
        profile_resolution: ProfileResolution {
            requested_profile: "A".to_string(),
            effective_profile: "A".to_string(),
            fallback_chain: vec!["A".to_string()],
            fallback_applied: false,
            cell: empty_profile_cell(),
            warnings: Vec::new(),
        },
        profile_content_hash: "0000000000000000".to_string(),
        trusted_agent_id: "codex".to_string(),
        trusted_agent_label: "codex".to_string(),
        backend: AgentBackendConfig::default(),
        isolated_codex_home_to_prepare: None,
        seed: None,
        match_tier: None,
        original_profile_letter: None,
    }
}

fn opencode_env(value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("OPENCODE_CONFIG_DIR".to_string(), value.to_string())])
}

fn seed_replica(temp: &std::path::Path) -> std::path::PathBuf {
    let replica = temp
        .join(".ac")
        .join("wg-7-dev-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();
    replica
}

/// Mirror the builder's canonicalize + verbatim-prefix strip.
fn canonical_replica(replica: &std::path::Path) -> std::path::PathBuf {
    let canonical = std::fs::canonicalize(replica).unwrap();
    canonical
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .map(std::path::PathBuf::from)
        .unwrap_or(canonical)
}

fn cell(command: &str, env: BTreeMap<String, String>) -> ProfileCellConfig {
    ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        preflight: None,
        preflight_timeout_seconds: None,
        enabled: true,
        command: command.to_string(),
        env,
        notes: String::new(),
    }
}

fn settings_with_cell(agent_command: &str, letter: &str, cell: ProfileCellConfig) -> AppSettings {
    let mut settings = AppSettings {
        agents: vec![agent("codex", agent_command)],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("codex".to_string())
        .or_default()
        .insert(letter.to_string(), cell);
    settings
}

fn env_row(key: &str, value: &str, enabled: bool) -> CodingAgentEnv {
    CodingAgentEnv {
        key: key.to_string(),
        value: value.to_string(),
        source: CodingAgentEnvSource::User,
        enabled,
    }
}

fn match_settings(agents: Vec<AgentConfig>, cells: &[(&str, &str, &str, bool)]) -> AppSettings {
    let mut settings = AppSettings {
        agents,
        ..AppSettings::default()
    };
    for (agent_id, letter, command, enabled) in cells {
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry(agent_id.to_string())
            .or_default()
            .insert(
                letter.to_string(),
                ProfileCellConfig {
                    id_hash: None,
                    id_hash_debug: None,
                    preflight: None,
                    preflight_timeout_seconds: None,
                    enabled: *enabled,
                    command: command.to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
    }
    settings
}

fn labelled(id: &str, label: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        label: label.to_string(),
        ..agent(id, command)
    }
}

/// The digest #2433 would have written for `agent_id`'s `letter` cell.
fn digest_of(settings: &AppSettings, agent_id: &str, letter: &str) -> String {
    let agent = settings.agents.iter().find(|a| a.id == agent_id).unwrap();
    let cell = &settings.coding_agent_profiles.profiles_by_agent[agent_id][letter];
    super::cell_identity(agent, cell)
}

fn reference(id: &str, source: Option<&str>) -> super::StoredReference {
    super::StoredReference {
        id: id.to_string(),
        source_letter: source.map(str::to_string),
        ..Default::default()
    }
}

fn identity(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn resolve_with_warnings(
    settings: &AppSettings,
    reference: &super::StoredReference,
) -> (super::MatchOutcome, Vec<String>) {
    let mut warnings = Vec::new();
    let outcome = super::resolve_portable_reference_logged(settings, reference, &mut warnings);
    (outcome, warnings)
}

fn matched(outcome: super::MatchOutcome) -> super::AgentMatch {
    match outcome {
        super::MatchOutcome::Matched(found) => found,
        other @ super::MatchOutcome::NoMatch { .. } => panic!("expected a match, got {other:?}"),
    }
}

fn resolve(settings: &AppSettings, reference: &super::StoredReference) -> super::AgentMatch {
    matched(super::resolve_portable_reference(settings, reference))
}

mod derived_agent_id_home;
mod instructions;
mod matching;
mod opencode_seed;
mod p02_schema_launch;
mod parse_env;
mod profile_hash;
