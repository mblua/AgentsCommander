use super::normalize_legacy_agent_command;
use crate::config::session_context::ManagedContextTarget;
use crate::config::settings::{AgentConfig, AppSettings};
use crate::session::profile::CodingAgentKind;

/// Per-command default when no explicit filename is configured. Derives the
/// filename from the command via the same `CodingAgentKind::detect` the launch
/// path uses, so a configured agent's default matches what detection would pick.
pub fn default_instructions_filename_for_command(command: &str) -> &'static str {
    // Shared AGENTS.md filename; Codex is one of its existing providers.
    let agents_md_filename = ManagedContextTarget::Codex.filename();
    match normalize_legacy_agent_command(command) {
        Ok(n) => match CodingAgentKind::detect(&n.shell, &n.shell_args) {
            Some(CodingAgentKind::Claude) => {
                crate::config::instructions_filename::CLAUDE_MD_FILE_NAME
            }
            Some(
                CodingAgentKind::Codex
                | CodingAgentKind::Pi
                | CodingAgentKind::Antigravity
                | CodingAgentKind::Muse,
            ) => agents_md_filename,
            // OpenCode, custom, and unknown commands also use AGENTS.md.
            None => agents_md_filename,
        },
        Err(_) => agents_md_filename,
    }
}

/// Resolved instructions filename for a configured coding agent: the explicit
/// (trimmed, validated) value when set, else the command-derived default. An
/// empty/whitespace/unsafe stored value silently falls back to the default, so
/// a hand-edited `settings.json` with a bad value never reaches the writer.
pub fn resolve_instructions_filename(agent: &AgentConfig) -> String {
    match agent.instructions_filename.as_deref().map(str::trim) {
        Some(f) if !f.is_empty() && is_safe_instructions_filename(f) => f.to_string(),
        _ => default_instructions_filename_for_command(&agent.command).to_string(),
    }
}

/// Union of every configured agent's resolved filename, used as the writer's
/// extra-cleanup set so launching one agent removes another's stale file. The
/// built-in managed names are always added by the writer itself; this only
/// contributes the configured + custom names.
pub fn managed_instructions_filenames(settings: &AppSettings) -> Vec<String> {
    let mut set: Vec<String> = Vec::new();
    for agent in &settings.agents {
        let f = resolve_instructions_filename(agent);
        if !set.contains(&f) {
            set.push(f);
        }
    }
    set
}

/// Pure launch-time resolver (R1.9): pick the instructions filename for a
/// launch given the post-resolution `agent_id`, the settings, and the
/// detection-derived target. Four branches:
///   1. configured agent with explicit valid filename -> that filename;
///   2. configured agent without one -> command-derived default;
///   3. no configured agent but a detected kind -> the detected filename;
///   4. neither -> `None` (no instructions file is materialized).
pub fn resolve_target_filename(
    agent_id: Option<&str>,
    settings: &AppSettings,
    detected: Option<ManagedContextTarget>,
) -> Option<String> {
    agent_id
        .and_then(|aid| settings.agents.iter().find(|a| a.id == aid))
        .map(resolve_instructions_filename)
        .or_else(|| detected.map(|t| t.filename().to_string()))
}

pub use crate::config::instructions_filename::is_safe_instructions_filename;
