use super::{canonical_command_text, raw_merged_profile_env};
use crate::config::coding_agent_profiles::ProfileResolution;
use crate::config::settings::AgentConfig;

pub(crate) fn log_profile_stamp(
    agent: &AgentConfig,
    profile_resolution: &ProfileResolution,
    profile_hash: &str,
    effective_command: &str,
) {
    let merged_profile_env = raw_merged_profile_env(agent, &profile_resolution.cell.env);
    // #592/#597 - surface exactly what gets hashed at spawn so a later drift
    // mismatch can be traced. Kept at debug for support; off the hot path (fires
    // once per spawn). Env VALUES are intentionally omitted (they may hold
    // secrets); the hash already fingerprints them, and the key set + count
    // reveal whether an env row participated.
    log::debug!(
        concat!(
            "[profile-hash] spawn-stamp: agent={} profile={} hash={} ",
            "effective_command={:?} canonical_command={:?} env_keys=[{}] ({} entries)",
        ),
        agent.id,
        profile_resolution.effective_profile,
        profile_hash,
        effective_command,
        canonical_command_text(effective_command),
        merged_profile_env
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        merged_profile_env.len(),
    );
}
