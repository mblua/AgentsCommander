use std::collections::BTreeMap;

use super::normalize_legacy_agent_command;
use crate::config::settings::{command_token_basename, AgentConfig, ProfileCellConfig};
use sha2::{Digest, Sha256};

/// #597 - the effective launch command: the agent base command (the binary,
/// possibly with its own fixed args) followed by the profile cell command (extra
/// params), joined as `<base> <cell>`. Each side is trimmed; a single ASCII space
/// joins them when both are non-empty; an empty side contributes nothing (no
/// stray space). Both empty yields `""`, which `normalize_legacy_agent_command`
/// then rejects as "agent command is empty", the same failure a blank command has
/// always produced. This is the single source of truth for the concatenation rule
/// (build, drift recompute, and settings validation all call it).
pub fn compose_effective_command(agent_command: &str, cell_command: &str) -> String {
    let base = agent_command.trim();
    let cell = cell_command.trim();
    match (base.is_empty(), cell.is_empty()) {
        (true, true) => String::new(),
        (false, true) => base.to_string(),
        (true, false) => cell.to_string(),
        (false, false) => format!("{base} {cell}"),
    }
}

/// #597 - RAW (pre-expansion) merged env used for the content hash: the agent's
/// ENABLED env rows overlaid by the cell env (profile-wins), keys folded to
/// uppercase on every OS (#2431) so a case-only difference does not double-count.
/// Values verbatim.
/// Mirrors `merge_env_layers`' agent-then-profile precedence but stays raw and
/// excludes the generated layer (CODEX_HOME isolation etc.), which is derived
/// state, not user config (decision §0.2; see Notes for the accepted limitation).
pub fn raw_merged_profile_env(
    agent: &AgentConfig,
    cell_env: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    // #2431 - keys fold to uppercase on EVERY OS before the overlay, so the
    // profile-wins precedence below is platform-independent. Hash input only; the
    // real spawn env is built by `merge_env_layers` with the platform rule.
    for row in agent.envs.iter().filter(|row| row.enabled) {
        merged.insert(row.key.to_ascii_uppercase(), row.value.clone());
    }
    for (key, value) in cell_env {
        merged.insert(key.to_ascii_uppercase(), value.clone());
    }
    merged
}

/// #2431 - the identity of one profile cell: the single definition of what a
/// cell's digest is over. The spawn path and later producers/matchers call this.
/// #3516 - a configured preflight chain appends its frame; an empty chain keeps
/// the legacy `profile_content_hash` bytes.
pub fn cell_identity(agent: &AgentConfig, cell: &ProfileCellConfig) -> String {
    let command = compose_effective_command(&agent.command, &cell.command);
    let env = raw_merged_profile_env(agent, &cell.env);
    match super::preflight::preflight_identity_frame(agent, cell) {
        None => profile_content_hash(&command, &env),
        Some(frame) => {
            let mut buf = profile_hash_preimage(&command, &env);
            buf.push_str(&frame);
            hex16(&buf)
        }
    }
}

/// #2431 - the canonical command TEXT: executable reduced by
/// `command_token_basename`, arguments lowercased (never path-normalized), each
/// token rendered so `normalize_legacy_agent_command` reads back the same vector.
/// An untokenizable command falls back to its trimmed, whitespace-collapsed text.
pub fn canonical_command_text(command: &str) -> String {
    match canonical_command_tokens(command) {
        Ok(tokens) => tokens
            .iter()
            .map(|token| render_command_token(token))
            .collect::<Vec<_>>()
            .join(" "),
        Err(raw) => raw,
    }
}

/// `Ok(exe + lowercased args)`, or `Err(collapsed raw text)` when tokenizing fails.
fn canonical_command_tokens(command: &str) -> Result<Vec<String>, String> {
    match normalize_legacy_agent_command(command) {
        Ok(normalized) => {
            let mut tokens = Vec::with_capacity(normalized.shell_args.len() + 1);
            tokens.push(command_token_basename(&normalized.shell));
            tokens.extend(normalized.shell_args.iter().map(|arg| arg.to_lowercase()));
            Ok(tokens)
        }
        Err(_) => Err(command
            .split_ascii_whitespace()
            .collect::<Vec<_>>()
            .join(" ")),
    }
}

/// Inverse of `normalize_legacy_agent_command` for one token (#2431 rule 4b).
pub(super) fn render_command_token(token: &str) -> String {
    let bare = !token.is_empty()
        && !token
            .chars()
            .any(|c| c.is_ascii_whitespace() || c == '\'' || c == '"');
    if bare {
        return token.to_string();
    }
    let mut out = String::with_capacity(token.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in token.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', 2 * backslashes + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', 2 * backslashes));
    out.push('"');
    out
}

/// #2431 - the exact bytes `profile_content_hash` digests. Canonicalizes the RAW
/// inputs itself (callers never pre-canonicalize): env keys fold to uppercase,
/// the command becomes an argv vector (`exe`, `argc`, `a` per argument) so argv
/// boundaries survive, or `rawcmd` when it cannot be tokenized.
pub(crate) fn profile_hash_preimage(command: &str, env: &BTreeMap<String, String>) -> String {
    use std::fmt::Write as _;
    // Fold keys for dedup/compare; value stays verbatim (raw).
    let mut normalized: BTreeMap<String, &str> = BTreeMap::new();
    for (key, value) in env {
        normalized.insert(key.to_ascii_uppercase(), value.as_str());
    }
    // Versioned, NUL-tagged serialization. NUL cannot appear in commands/env we
    // accept, and the field tags stop a value from forging a record boundary.
    let mut buf = String::from("v2\u{0}");
    match canonical_command_tokens(command) {
        Ok(tokens) => {
            let (exe, args) = tokens.split_first().expect("tokenizer yields an exe");
            let _ = write!(buf, "exe\u{0}{}\u{0}argc\u{0}{}\u{0}", exe, args.len());
            for arg in args {
                let _ = write!(buf, "a\u{0}{}\u{0}", arg);
            }
        }
        Err(raw) => {
            let _ = write!(buf, "rawcmd\u{0}{}\u{0}", raw);
        }
    }
    let _ = write!(buf, "envc\u{0}{}\u{0}", normalized.len());
    for (key, value) in &normalized {
        let _ = write!(buf, "k\u{0}{}\u{0}v\u{0}{}\u{0}", key, value);
    }
    buf
}

/// #592 - stable 16-hex content fingerprint for profile drift detection
/// ("loaded != configured"). Hashes the RAW `command` + `env` it is given
/// (placeholders un-expanded); the primitive is agnostic to how those are built.
/// #597 - callers now pass the COMPOSED effective command (agent base + cell
/// params) via `compose_effective_command` and the merged env (agent enabled rows
/// overlaid by the cell, profile-wins) via `raw_merged_profile_env`, so an edit to
/// the base command, cell command, base env, or cell env all flip the hash
/// (SUPERSEDES the original #592 cell-only input). Env keys fold to uppercase on
/// every OS (#2431), then are ordered via `BTreeMap`, so a case-only key edit does
/// not false-flag and iteration order is irrelevant. SHA-256 (stable across Rust versions, unlike
/// DefaultHasher), truncated to the first 16 hex chars (matches the existing
/// `profile_assignment_fingerprint` 16-hex shape).
/// #2431 - canonicalized inside `profile_hash_preimage` (portable across OSes and
/// path shapes); tag `v2`.
pub fn profile_content_hash(command: &str, env: &BTreeMap<String, String>) -> String {
    hex16(&profile_hash_preimage(command, env))
}

/// First 16 hex chars of SHA-256: the single digest rule for cell identities.
fn hex16(buf: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(buf.as_bytes()));
    // Hex is ASCII single-byte; slicing the first 16 chars is char-boundary safe.
    digest[..16].to_string()
}
