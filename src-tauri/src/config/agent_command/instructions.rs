use super::normalize_legacy_agent_command;
use crate::config::session_context::ManagedContextTarget;
use crate::config::settings::{AgentConfig, AppSettings};
use crate::session::profile::CodingAgentKind;

/// Per-command default when no explicit filename is configured. Derives the
/// filename from the command via the same `CodingAgentKind::detect` the launch
/// path uses, so a configured agent's default matches what detection would pick.
pub fn default_instructions_filename_for_command(command: &str) -> &'static str {
    match normalize_legacy_agent_command(command) {
        Ok(n) => match CodingAgentKind::detect(&n.shell, &n.shell_args) {
            Some(CodingAgentKind::Claude) => "CLAUDE.md",
            Some(CodingAgentKind::Codex)
            | Some(CodingAgentKind::Pi)
            | Some(CodingAgentKind::Antigravity)
            | Some(CodingAgentKind::Muse) => "AGENTS.md",
            // OpenCode, custom, and unknown commands also use AGENTS.md.
            None => "AGENTS.md",
        },
        Err(_) => "AGENTS.md",
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

/// True only for a bare, safe `*.md` filename that can be `cwd.join`ed and
/// written without escaping the agent root (R1.5 + G9). This is a strict
/// predicate: callers trim before calling, so any surrounding whitespace here
/// is a genuine rejection (Windows silently strips trailing space/dot, which
/// would desync the validated, written, and cleanup names).
pub fn is_safe_instructions_filename(filename: &str) -> bool {
    if filename.is_empty() {
        return false;
    }
    // Length cap so `root + filename` stays well under MAX_PATH.
    if filename.chars().count() > 128 {
        return false;
    }
    // No path separators or parent-dir traversal.
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return false;
    }
    // No colon: rejects a drive prefix (`C:x.md`) AND an NTFS Alternate Data
    // Stream (`AGENTS.md:evil`).
    if filename.contains(':') {
        return false;
    }
    // No control chars.
    if filename.chars().any(|c| c.is_control()) {
        return false;
    }
    // No leading/trailing whitespace and no trailing dot (Windows strips them).
    if filename.starts_with(char::is_whitespace)
        || filename.ends_with(char::is_whitespace)
        || filename.ends_with('.')
    {
        return false;
    }
    // Require a `.md` extension, matched case-insensitively (accept `AGENTS.MD`);
    // the caller keeps the user's original spelling.
    let lower = filename.to_ascii_lowercase();
    let Some(stem) = lower.strip_suffix(".md") else {
        return false;
    };
    // Non-empty stem (reject bare `.md`) and no space immediately before `.md`.
    if stem.is_empty() || stem.ends_with(char::is_whitespace) {
        return false;
    }
    // G9: reject the exact AC-internal sentinel (case-insensitive). The
    // user-facing built-ins CLAUDE.md / AGENTS.md stay allowed.
    if filename.eq_ignore_ascii_case("last_ac_context.md") {
        return false;
    }
    // Windows reserved device names, checked case-insensitively on the segment
    // BEFORE THE FIRST DOT: Windows resolves a device by that base segment and
    // ignores the extension, so `CON.md` AND `CON.foo.md` both hit the CON
    // device. CON, PRN, AUX, NUL, COM1..COM9, LPT1..LPT9.
    let device_segment = stem.split('.').next().unwrap_or(stem);
    if is_reserved_device_segment(device_segment) {
        return false;
    }
    true
}

/// True iff `segment` (already lowercased) is a Windows reserved device name.
/// COM/LPT match digits 1-9 only (COM0/LPT0 are not devices).
fn is_reserved_device_segment(segment: &str) -> bool {
    if matches!(segment, "con" | "prn" | "aux" | "nul") {
        return true;
    }
    let bytes = segment.as_bytes();
    bytes.len() == 4
        && (segment.starts_with("com") || segment.starts_with("lpt"))
        && matches!(bytes[3], b'1'..=b'9')
}
