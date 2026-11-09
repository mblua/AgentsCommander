use std::collections::BTreeMap;
use std::path::Path;

use crate::session::profile::CodingAgentKind;

/// Heuristic, log-only warning when the seed dest does not line up with the
/// coding agent's configured config-dir env (so the tool would read its config
/// from elsewhere), OR when it EXACTLY matches it (L1: the seed overwrites the
/// tool's ACTIVE config dir every spawn). Never blocks; may warn spuriously if a
/// config-dir env is injected by a path this does not inspect.
///
/// Receives the real launch `shell`/`shell_args` (the agent command) so the
/// coding-agent kind is detected from the actual command.
pub fn compute_config_dir_warning(
    dest: &Path,
    shell: &str,
    shell_args: &[String],
    agent_env: &BTreeMap<String, String>,
    profile_env: &BTreeMap<String, String>,
    effective_codex_home: Option<&Path>,
) -> Option<String> {
    let key = match CodingAgentKind::detect(shell, shell_args) {
        Some(CodingAgentKind::Claude) => "CLAUDE_CONFIG_DIR",
        Some(CodingAgentKind::Codex) => "CODEX_HOME",
        // Antigravity, Pi and Muse (#1873) have no AC-managed config-dir env to
        // compare against.
        Some(CodingAgentKind::Antigravity | CodingAgentKind::Pi | CodingAgentKind::Muse) => {
            return None
        }
        None => {
            if super::command_runs_opencode(shell, shell_args) {
                "OPENCODE_CONFIG_DIR"
            } else {
                return None;
            }
        }
    };

    let dest_seg = dest.file_name().and_then(std::ffi::OsStr::to_str)?;

    // CODEX_HOME already went through the full resolver (isolation/profile/agent/
    // inherited); prefer its resolved value. Others: profile over agent layer.
    let configured: Option<String> = if key == "CODEX_HOME" {
        effective_codex_home
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| lookup_env_ci(profile_env, agent_env, key).cloned())
    } else {
        lookup_env_ci(profile_env, agent_env, key).cloned()
    };

    match configured {
        Some(value) => {
            let matches = final_segment(&value)
                .as_deref()
                .is_some_and(|seg| segments_eq(seg, dest_seg));
            if matches {
                // L1 case 2: seed overwrites the tool's ACTIVE config dir.
                Some(format!(
                    "config seed destination '{dest_seg}' has the same final folder name as the \
                    tool's active {key}='{value}'; this heuristic does not inspect filesystem \
                    identity. The seeded folder is overwritten on every spawn, so any \
                    credentials or session state living there are reset each launch (ensure the \
                    template is self-sufficient)"
                ))
            } else {
                // Case 1: dest does not match the configured config-dir env.
                Some(format!(
                    "config seed destination '{dest_seg}' does not match the final folder name of \
                    {key}='{value}'; this heuristic does not inspect filesystem identity, and the \
                    tool may read config from that path rather than the seeded folder"
                ))
            }
        }
        // Case 1 (unset but expected for this tool).
        None => Some(format!(
            "config seed destination '{dest_seg}' is set but {key} is not configured; the tool may \
            ignore the seeded folder"
        )),
    }
}

/// Case-insensitive env lookup (Windows env keys are case-insensitive, and the
/// stored key casing is user-controlled), profile layer over agent layer. The
/// plan's illustrative `.get(key)` would miss a lowercase-keyed value; this
/// mirrors `agent_command::find_env_value`.
fn lookup_env_ci<'a>(
    profile_env: &'a BTreeMap<String, String>,
    agent_env: &'a BTreeMap<String, String>,
    key: &str,
) -> Option<&'a String> {
    let want = key.to_ascii_uppercase();
    profile_env
        .iter()
        .find(|(k, _)| k.to_ascii_uppercase() == want)
        .or_else(|| {
            agent_env
                .iter()
                .find(|(k, _)| k.to_ascii_uppercase() == want)
        })
        .map(|(_, v)| v)
}

/// Final path segment of a configured value, trailing separators trimmed.
fn final_segment(value: &str) -> Option<String> {
    let trimmed = value.trim_end_matches(['/', '\\']);
    Path::new(trimmed)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map(ToString::to_string)
}

fn segments_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}
