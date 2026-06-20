use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::coding_agent_profiles::{
    resolve_profile, ProfileResolution, ProfileResolutionRequest,
};
use crate::config::placeholders::{
    ac_placeholder_error, expand_placeholders, expand_placeholders_in_args,
    placeholder_context_for_launch_root, reject_unexpanded_markers, value_contains_ac_placeholder,
    PlaceholderContext,
};
use crate::config::session_context::ManagedContextTarget;
use crate::config::settings::{
    is_codex_home_key, is_opencode_config_dir_key, normalize_env_key_for_platform,
    validate_expanded_codex_home_value, validate_user_env_key, AgentConfig, AppSettings,
};
use crate::session::profile::CodingAgentKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAgentCommand {
    pub shell: String,
    pub shell_args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AgentSpawnCommand {
    pub shell: String,
    pub shell_args: Vec<String>,
    pub agent_env: BTreeMap<String, String>,
    pub profile_env: BTreeMap<String, String>,
    pub generated_env: BTreeMap<String, String>,
    pub child_env: Vec<(String, String)>,
    pub env_remove_keys: Vec<String>,
    pub effective_codex_home: Option<PathBuf>,
    pub profile_resolution: ProfileResolution,
    pub trusted_agent_id: String,
    pub trusted_agent_label: String,
}

pub fn normalize_legacy_agent_command(command: &str) -> Result<NormalizedAgentCommand, String> {
    let input = command.trim_matches(|c: char| c.is_ascii_whitespace());
    if input.is_empty() {
        return Err("agent command is empty".to_string());
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut token_started = false;

    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), '\\') => {
                let mut slash_count = 1;
                while matches!(chars.peek(), Some('\\')) {
                    slash_count += 1;
                    chars.next();
                }
                if chars.peek().copied() == Some(q) {
                    current.extend(std::iter::repeat_n('\\', slash_count / 2));
                    if slash_count % 2 == 1 {
                        current.push(q);
                        chars.next();
                    } else {
                        quote = None;
                        chars.next();
                    }
                } else {
                    current.extend(std::iter::repeat_n('\\', slash_count));
                }
                token_started = true;
            }
            (Some(q), c) if c == q => {
                quote = None;
                token_started = true;
            }
            (Some(_), c) => {
                current.push(c);
                token_started = true;
            }
            (None, '\'' | '"') => {
                quote = Some(ch);
                token_started = true;
            }
            (None, c) if c.is_ascii_whitespace() => {
                if token_started {
                    tokens.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            (None, c) => {
                current.push(c);
                token_started = true;
            }
        }
    }

    if let Some(q) = quote {
        return Err(format!(
            "unclosed {} quote",
            if q == '"' { "double" } else { "single" }
        ));
    }

    if token_started {
        tokens.push(current);
    }

    let Some((shell, args)) = tokens.split_first() else {
        return Err("agent command is empty".to_string());
    };
    if shell.is_empty() {
        return Err("agent executable is empty".to_string());
    }

    Ok(NormalizedAgentCommand {
        shell: shell.clone(),
        shell_args: args.to_vec(),
    })
}

// ---------------------------------------------------------------------------
// #529 - per-coding-agent instructions filename (written to the agent root at
// launch; content is the same AC context + Role.md AC already generates).
// ---------------------------------------------------------------------------

/// Per-command default when no explicit filename is configured. Derives the
/// filename from the command via the same `CodingAgentKind::detect` the launch
/// path uses, so a configured agent's default matches what detection would pick.
pub fn default_instructions_filename_for_command(command: &str) -> &'static str {
    match normalize_legacy_agent_command(command) {
        Ok(n) => match CodingAgentKind::detect(&n.shell, &n.shell_args) {
            Some(CodingAgentKind::Claude) => "CLAUDE.md",
            Some(CodingAgentKind::Gemini) => "GEMINI.md",
            // Codex, OpenCode, custom, and unknown all use AGENTS.md.
            _ => "AGENTS.md",
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
    // user-facing built-ins CLAUDE.md / GEMINI.md / AGENTS.md stay allowed.
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

pub fn stringify_agent_command_tokens(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|token| stringify_agent_command_token(token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn stringify_agent_command_token(token: &str) -> String {
    if token.is_empty() {
        return "\"\"".to_string();
    }
    if !token
        .chars()
        .any(|ch| ch.is_ascii_whitespace() || ch == '"' || ch == '\'')
    {
        return token.to_string();
    }
    let mut out = String::with_capacity(token.len() + 2);
    out.push('"');
    let mut slash_count = 0;
    for ch in token.chars() {
        if ch == '\\' {
            slash_count += 1;
            continue;
        }
        if ch == '"' {
            out.extend(std::iter::repeat_n('\\', slash_count * 2 + 1));
            out.push(ch);
            slash_count = 0;
            continue;
        }
        out.extend(std::iter::repeat_n('\\', slash_count));
        slash_count = 0;
        out.push(ch);
    }
    out.extend(std::iter::repeat_n('\\', slash_count * 2));
    out.push('"');
    out
}

fn executable_basename(s: &str) -> String {
    std::path::Path::new(s)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(s)
        .to_lowercase()
}

fn collect_agent_env(
    agent: &AgentConfig,
    placeholder_context: Option<&PlaceholderContext>,
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut seen = BTreeMap::new();
    for row in agent.envs.iter().filter(|row| row.enabled) {
        let context = format!("Agent '{}' env settings", agent.label);
        validate_user_env_key(&row.key, &context)?;
        let normalized = normalize_env_key_for_platform(&row.key);
        if let Some(previous) = seen.insert(normalized, row.key.clone()) {
            return Err(format!(
                "{}: duplicate env keys '{}' and '{}'",
                context, previous, row.key
            ));
        }
        let strict_path = is_codex_home_key(&row.key);
        let value = expand_runtime_value(&row.value, placeholder_context, &context, strict_path)?;
        if strict_path {
            validate_expanded_codex_home_value(&value, &context)?;
        }
        out.insert(row.key.clone(), value);
    }
    Ok(out)
}

fn collect_profile_env(
    agent_id: &str,
    profile: &str,
    env: &BTreeMap<String, String>,
    placeholder_context: Option<&PlaceholderContext>,
) -> Result<BTreeMap<String, String>, String> {
    let context = format!("Profile '{}:{}' env settings", agent_id, profile);
    let mut out = BTreeMap::new();
    let mut seen = BTreeMap::new();
    for (key, value) in env {
        validate_user_env_key(key, &context)?;
        let normalized = normalize_env_key_for_platform(key);
        if let Some(previous) = seen.insert(normalized, key.clone()) {
            return Err(format!(
                "{}: duplicate env keys '{}' and '{}'",
                context, previous, key
            ));
        }
        let strict_path = is_codex_home_key(key);
        let value = expand_runtime_value(value, placeholder_context, &context, strict_path)?;
        if strict_path {
            validate_expanded_codex_home_value(&value, &context)?;
        }
        out.insert(key.clone(), value);
    }
    Ok(out)
}

fn expand_runtime_value(
    value: &str,
    placeholder_context: Option<&PlaceholderContext>,
    context: &str,
    strict_path_value: bool,
) -> Result<String, String> {
    match placeholder_context {
        Some(context_obj) => {
            let expanded = expand_placeholders(value, context_obj)?;
            reject_unexpanded_markers(&expanded, context, strict_path_value)?;
            Ok(expanded)
        }
        None => {
            if value_contains_ac_placeholder(value) {
                return Err(ac_placeholder_error().to_string());
            }
            reject_unexpanded_markers(value, context, strict_path_value)?;
            Ok(value.to_string())
        }
    }
}

fn find_env_value<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a String> {
    let wanted = normalize_env_key_for_platform(key);
    env.iter()
        .find(|(candidate, _)| normalize_env_key_for_platform(candidate) == wanted)
        .map(|(_, value)| value)
}

fn sanitize_codex_home_id(agent_id: &str) -> String {
    let sanitized: String = agent_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "codex".to_string()
    } else {
        trimmed.to_string()
    }
}

struct ComputedCodexHome {
    generated_env: BTreeMap<String, String>,
    effective_codex_home: Option<PathBuf>,
    env_remove_keys: Vec<String>,
}

fn compute_codex_home(
    agent: &AgentConfig,
    shell: &str,
    shell_args: &[String],
    agent_env: &BTreeMap<String, String>,
    profile_env: &BTreeMap<String, String>,
) -> Result<ComputedCodexHome, String> {
    let mut generated_env = BTreeMap::new();
    let mut env_remove_keys = Vec::new();
    let agent_kind = CodingAgentKind::detect(shell, shell_args);
    if agent_kind != Some(CodingAgentKind::Codex) && executable_basename(shell) != "codex" {
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: None,
            env_remove_keys,
        });
    }

    if agent.isolated_home {
        let config_dir = crate::config::config_dir()
            .ok_or_else(|| "Could not determine config directory for CODEX_HOME".to_string())?;
        let home = config_dir
            .join("codex-home")
            .join(sanitize_codex_home_id(&agent.id));
        std::fs::create_dir_all(&home).map_err(|e| {
            format!(
                "Failed to create isolated CODEX_HOME '{}': {}",
                home.display(),
                e
            )
        })?;
        generated_env.insert("CODEX_HOME".to_string(), home.to_string_lossy().to_string());
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: Some(home),
            env_remove_keys,
        });
    }

    if let Some(value) = find_env_value(profile_env, "CODEX_HOME") {
        let path = validate_expanded_codex_home_value(value, "Profile CODEX_HOME")?;
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: Some(path),
            env_remove_keys,
        });
    }
    if let Some(value) = find_env_value(agent_env, "CODEX_HOME") {
        let path = validate_expanded_codex_home_value(value, "Agent CODEX_HOME")?;
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: Some(path),
            env_remove_keys,
        });
    }
    if let Ok(value) = std::env::var("CODEX_HOME") {
        match validate_expanded_codex_home_value(&value, "Inherited CODEX_HOME") {
            Ok(path) => {
                return Ok(ComputedCodexHome {
                    generated_env,
                    effective_codex_home: Some(path),
                    env_remove_keys,
                })
            }
            Err(e) => {
                log::warn!(
                    "[codex-home] {}. Removing inherited CODEX_HOME for child.",
                    e
                );
                env_remove_keys.push("CODEX_HOME".to_string());
            }
        }
    }

    Ok(ComputedCodexHome {
        generated_env,
        effective_codex_home: None,
        env_remove_keys,
    })
}

/// Outcome of [`ensure_opencode_config_dir`]. Returned for unit testing; the
/// launch path ignores it (the operation is best-effort, see the fn docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpencodeConfigDirOutcome {
    /// Command does not run opencode, or no `OPENCODE_CONFIG_DIR` is set, or the
    /// value is not an absolute path: nothing was created.
    Skipped,
    /// `create_dir_all` succeeded (dir created, or already existed).
    Ensured,
    /// `create_dir_all` failed; the launch proceeds anyway (warning logged).
    Failed,
}

/// True when the launch command runs opencode, matched by executable basename.
/// There is no `CodingAgentKind::OpenCode` (the enum is Claude/Codex/Gemini and
/// `detect` does not know opencode), so this mirrors the
/// `executable_basename(shell) == "codex"` fallback in [`compute_codex_home`].
/// Args are scanned too, so a `cmd /c opencode` wrapper is still recognized.
/// Called BEFORE the `git_pull_before` wrap, so `shell` is the real command.
fn command_runs_opencode(shell: &str, shell_args: &[String]) -> bool {
    if executable_basename(shell) == "opencode" {
        return true;
    }
    shell_args
        .iter()
        .flat_map(|arg| arg.split_whitespace())
        .any(|token| executable_basename(token) == "opencode")
}

/// Resolved `OPENCODE_CONFIG_DIR` value with profile-over-agent precedence,
/// mirroring `merge_env_layers` (`generated_env` never carries this key).
fn find_opencode_config_dir<'a>(
    agent_env: &'a BTreeMap<String, String>,
    profile_env: &'a BTreeMap<String, String>,
) -> Option<&'a String> {
    profile_env
        .iter()
        .find(|(key, _)| is_opencode_config_dir_key(key))
        .or_else(|| agent_env.iter().find(|(key, _)| is_opencode_config_dir_key(key)))
        .map(|(_, value)| value)
}

/// #576 follow-up: opencode does not create its `OPENCODE_CONFIG_DIR`; at
/// startup it writes a managed `.gitignore` into that dir and exits 1 if the dir
/// is missing (ENOENT on the parent). When the launch runs opencode and the
/// resolved (post-expansion) `OPENCODE_CONFIG_DIR` is an absolute path, create
/// it up front so the child can start.
///
/// This mirrors the `create_dir_all` precedent in [`compute_codex_home`], with
/// one deliberate difference: it is BEST-EFFORT. A `create_dir_all` failure is
/// logged and the launch proceeds (opencode will surface its own error), so a
/// transient failure never blocks a spawn, unlike the AC-owned isolated codex
/// home which aborts. A non-absolute value (relative, empty, or otherwise not a
/// real path) is skipped, never created.
///
/// Scope: only the agent and profile env layers are inspected (the AC-managed,
/// config-driven sources, same precedence as `compute_codex_home`). An
/// `OPENCODE_CONFIG_DIR` that AC merely inherits from its own ambient
/// environment is intentionally NOT created here: AC does not own that value,
/// and the bug this targets comes from AC-configured replica-local paths.
fn ensure_opencode_config_dir(
    shell: &str,
    shell_args: &[String],
    agent_env: &BTreeMap<String, String>,
    profile_env: &BTreeMap<String, String>,
) -> OpencodeConfigDirOutcome {
    if !command_runs_opencode(shell, shell_args) {
        return OpencodeConfigDirOutcome::Skipped;
    }
    let Some(value) = find_opencode_config_dir(agent_env, profile_env) else {
        return OpencodeConfigDirOutcome::Skipped;
    };
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        log::debug!(
            "[opencode] OPENCODE_CONFIG_DIR '{}' is not an absolute path; not creating it.",
            value
        );
        return OpencodeConfigDirOutcome::Skipped;
    }
    match std::fs::create_dir_all(&path) {
        Ok(()) => {
            log::info!("[opencode] Ensured OPENCODE_CONFIG_DIR '{}'", path.display());
            OpencodeConfigDirOutcome::Ensured
        }
        Err(e) => {
            log::warn!(
                "[opencode] Failed to create OPENCODE_CONFIG_DIR '{}': {}. Launching anyway.",
                path.display(),
                e
            );
            OpencodeConfigDirOutcome::Failed
        }
    }
}

fn merge_env_layers(layers: &[&BTreeMap<String, String>]) -> Vec<(String, String)> {
    let mut by_normalized: BTreeMap<String, (String, String)> = BTreeMap::new();
    for layer in layers {
        for (key, value) in *layer {
            by_normalized.insert(
                normalize_env_key_for_platform(key),
                (key.clone(), value.clone()),
            );
        }
    }
    by_normalized.into_values().collect()
}

#[cfg(windows)]
fn cmd_quote_token(token: &str) -> Result<String, String> {
    if token.contains('\0') || token.contains('\n') || token.contains('\r') {
        return Err("gitPullBefore command tokens must not contain NUL or newline".to_string());
    }
    if token.contains('%') || token.contains('!') {
        return Err(
            "gitPullBefore command tokens must not contain percent or delayed-expansion markers"
                .to_string(),
        );
    }
    if token.contains('"') {
        return Err(
            "gitPullBefore command tokens with double quotes are not supported".to_string(),
        );
    }
    let escaped = token.replace('^', "^^");
    if escaped.is_empty() {
        return Ok("\"\"".to_string());
    }
    if escaped
        .chars()
        .any(|ch| ch.is_ascii_whitespace() || matches!(ch, '&' | '|' | '<' | '>' | '(' | ')'))
    {
        Ok(format!("\"{}\"", escaped))
    } else {
        Ok(escaped)
    }
}

#[cfg(windows)]
fn wrap_git_pull_before(
    shell: String,
    shell_args: Vec<String>,
) -> Result<(String, Vec<String>), String> {
    let mut parts = Vec::with_capacity(shell_args.len() + 1);
    parts.push(cmd_quote_token(&shell)?);
    for arg in &shell_args {
        parts.push(cmd_quote_token(arg)?);
    }
    Ok((
        "cmd.exe".to_string(),
        vec!["/K".to_string(), format!("git pull && {}", parts.join(" "))],
    ))
}

#[cfg(not(windows))]
fn wrap_git_pull_before(
    shell: String,
    shell_args: Vec<String>,
) -> Result<(String, Vec<String>), String> {
    Ok((shell, shell_args))
}

pub fn build_agent_spawn_command(
    settings: &AppSettings,
    agent_id: &str,
    launch_path: Option<&Path>,
    requested_profile: Option<&str>,
) -> Result<AgentSpawnCommand, String> {
    let agent = settings
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("Agent '{}' is not configured", agent_id))?;
    let profile_resolution = resolve_profile(
        settings,
        ProfileResolutionRequest {
            coding_agent_id: &agent.id,
            launch_path,
            agent_matrix_name: None,
            requested_profile,
        },
    );
    for warning in &profile_resolution.warnings {
        log::warn!("[profiles] {}", warning);
    }

    let selected_command = if profile_resolution.cell.command.trim().is_empty() {
        agent.command.as_str()
    } else {
        profile_resolution.cell.command.as_str()
    };
    let normalized = normalize_legacy_agent_command(selected_command).map_err(|e| {
        format!(
            "Invalid profile command for '{}:{}': {}. command={:?}",
            agent.id, profile_resolution.effective_profile, e, selected_command
        )
    })?;
    let mut command_tokens = Vec::with_capacity(normalized.shell_args.len() + 1);
    command_tokens.push(normalized.shell);
    command_tokens.extend(normalized.shell_args);
    let needs_placeholder_context = command_tokens
        .iter()
        .any(|value| value_contains_ac_placeholder(value))
        || agent
            .envs
            .iter()
            .filter(|row| row.enabled)
            .any(|row| value_contains_ac_placeholder(&row.value))
        || profile_resolution
            .cell
            .env
            .values()
            .any(|value| value_contains_ac_placeholder(value));
    let placeholder_context = if needs_placeholder_context {
        Some(
            launch_path
                .ok_or_else(|| ac_placeholder_error().to_string())
                .and_then(placeholder_context_for_launch_root)?,
        )
    } else {
        None
    };
    let expanded_command_tokens = if let Some(context) = placeholder_context.as_ref() {
        expand_placeholders_in_args(&command_tokens, context)?
    } else {
        for token in &command_tokens {
            reject_unexpanded_markers(token, "Agent command", false)?;
        }
        command_tokens
    };
    let Some((shell, shell_args)) = expanded_command_tokens.split_first() else {
        return Err("agent command is empty".to_string());
    };
    let mut shell = shell.clone();
    let mut shell_args = shell_args.to_vec();

    let agent_env = collect_agent_env(agent, placeholder_context.as_ref())?;
    let profile_env = collect_profile_env(
        &agent.id,
        &profile_resolution.effective_profile,
        &profile_resolution.cell.env,
        placeholder_context.as_ref(),
    )?;
    let computed_codex_home =
        compute_codex_home(agent, &shell, &shell_args, &agent_env, &profile_env)?;
    // #576 follow-up: opencode exits 1 if OPENCODE_CONFIG_DIR is missing, so
    // create it before spawn. Best-effort (never aborts the build); see fn docs.
    // Runs before the git_pull_before wrap so `shell` is still the real command.
    ensure_opencode_config_dir(&shell, &shell_args, &agent_env, &profile_env);
    let child_env =
        merge_env_layers(&[&agent_env, &profile_env, &computed_codex_home.generated_env]);

    if agent.git_pull_before {
        let wrapped = wrap_git_pull_before(shell, shell_args)?;
        shell = wrapped.0;
        shell_args = wrapped.1;
    }

    Ok(AgentSpawnCommand {
        shell,
        shell_args,
        agent_env,
        profile_env,
        generated_env: computed_codex_home.generated_env,
        child_env,
        env_remove_keys: computed_codex_home.env_remove_keys,
        effective_codex_home: computed_codex_home.effective_codex_home,
        profile_resolution,
        trusted_agent_id: agent.id.clone(),
        trusted_agent_label: agent.label.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_agent_spawn_command, command_runs_opencode, default_instructions_filename_for_command,
        ensure_opencode_config_dir, find_opencode_config_dir, is_safe_instructions_filename,
        managed_instructions_filenames, normalize_legacy_agent_command, resolve_instructions_filename,
        resolve_target_filename, OpencodeConfigDirOutcome,
    };
    use crate::config::settings::{
        AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource, ProfileCellConfig,
    };
    use std::collections::BTreeMap;

    #[test]
    fn normalizes_plain_command_with_args() {
        let got = normalize_legacy_agent_command("codex --yolo").unwrap();
        assert_eq!(got.shell, "codex");
        assert_eq!(got.shell_args, vec!["--yolo"]);
    }

    #[test]
    fn preserves_quoted_arg_with_spaces() {
        let got = normalize_legacy_agent_command("codex --model \"gpt 5\"").unwrap();
        assert_eq!(got.shell, "codex");
        assert_eq!(got.shell_args, vec!["--model", "gpt 5"]);
    }

    #[test]
    fn supports_quoted_windows_executable_path() {
        let got = normalize_legacy_agent_command("\"C:\\Program Files\\Codex\\codex.exe\" --yolo")
            .unwrap();
        assert_eq!(got.shell, "C:\\Program Files\\Codex\\codex.exe");
        assert_eq!(got.shell_args, vec!["--yolo"]);
    }

    #[test]
    fn preserves_empty_quoted_arg() {
        let got = normalize_legacy_agent_command("codex --config \"\" --flag").unwrap();
        assert_eq!(got.shell, "codex");
        assert_eq!(got.shell_args, vec!["--config", "", "--flag"]);
    }

    #[test]
    fn stringifies_migration_tokens_with_spaces_quotes_and_empty_args() {
        let tokens = vec![
            "C:\\Program Files\\Codex\\codex.exe".to_string(),
            "--config".to_string(),
            "".to_string(),
            "a\"b".to_string(),
        ];
        let text = super::stringify_agent_command_tokens(&tokens);
        let normalized = normalize_legacy_agent_command(&text).unwrap();
        let mut round_trip = vec![normalized.shell];
        round_trip.extend(normalized.shell_args);
        assert_eq!(round_trip, tokens);
    }

    #[test]
    fn stringifies_tokens_with_frontend_msvc_quoting_cases() {
        let cases = [
            (vec!["codex", "--model", "gpt 5"], "codex --model \"gpt 5\""),
            (
                vec!["codex", "--model", "gpt\"5"],
                "codex --model \"gpt\\\"5\"",
            ),
            (vec!["codex", "--model", "gpt'5"], "codex --model \"gpt'5\""),
            (vec!["codex", "--config", ""], "codex --config \"\""),
            (
                vec!["codex", r#"C:\tmp\"quoted"#],
                r##"codex "C:\tmp\\\"quoted""##,
            ),
            (
                vec!["codex", r#"C:\Program Files\Codex\codex.exe"#],
                r##"codex "C:\Program Files\Codex\codex.exe""##,
            ),
        ];

        for (tokens, expected) in cases {
            let tokens: Vec<String> = tokens.into_iter().map(str::to_string).collect();
            let text = super::stringify_agent_command_tokens(&tokens);
            assert_eq!(text, expected);

            let normalized = normalize_legacy_agent_command(&text).unwrap();
            assert_eq!(normalized.shell, tokens[0]);
            assert_eq!(normalized.shell_args, tokens[1..]);
        }
    }

    #[test]
    fn parses_quoted_backslashes_like_frontend_argv_parser() {
        let normalized = normalize_legacy_agent_command(
            r##"codex "C:\Program Files\Codex\codex.exe" "C:\tmp\\\"quoted""##,
        )
        .unwrap();

        assert_eq!(normalized.shell, "codex");
        assert_eq!(
            normalized.shell_args,
            vec![
                r#"C:\Program Files\Codex\codex.exe"#.to_string(),
                r#"C:\tmp\"quoted"#.to_string(),
            ]
        );
    }

    #[test]
    fn rejects_unclosed_quote() {
        let err = normalize_legacy_agent_command("codex \"unterminated").unwrap_err();
        assert!(err.contains("unclosed double quote"));
    }

    #[test]
    fn rejects_empty_quoted_executable() {
        let err = normalize_legacy_agent_command("\"\" --flag").unwrap_err();
        assert!(err.contains("agent executable is empty"));
    }

    fn agent(id: &str, command: &str) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            label: id.to_string(),
            command: command.to_string(),
            color: "#000000".to_string(),
            git_pull_before: false,
            exclude_global_claude_md: false,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
        }
    }

    #[test]
    fn build_spawn_uses_profile_command_as_complete_invocation() {
        let mut settings = AppSettings {
            agents: vec![agent("codex", "codex --base")],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "B".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "codex --profile fast".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("B")).unwrap();

        assert_eq!(spawn.shell, "codex");
        assert_eq!(spawn.shell_args, vec!["--profile", "fast"]);
        assert_eq!(spawn.profile_resolution.effective_profile, "B");
    }

    #[test]
    fn env_merge_precedence_is_agent_profile_generated() {
        let mut codex = agent("codex", "codex");
        codex.envs = vec![
            CodingAgentEnv {
                key: "OPENAI_API_BASE".to_string(),
                value: "agent".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "CODEX_HOME".to_string(),
                value: std::env::temp_dir()
                    .join("agent-codex")
                    .to_string_lossy()
                    .to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
        ];
        codex.isolated_home = true;
        let mut settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };
        let profile_home = std::env::temp_dir().join("profile-codex");
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: BTreeMap::from([
                        ("OPENAI_API_BASE".to_string(), "profile".to_string()),
                        (
                            "CODEX_HOME".to_string(),
                            profile_home.to_string_lossy().to_string(),
                        ),
                    ]),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();
        let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();

        assert_eq!(
            env.get("OPENAI_API_BASE").map(String::as_str),
            Some("profile")
        );
        assert_eq!(
            env.get("CODEX_HOME").map(String::as_str),
            spawn
                .effective_codex_home
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .as_deref()
        );
        assert!(spawn.generated_env.contains_key("CODEX_HOME"));
    }

    #[test]
    fn reserved_env_keys_are_rejected_case_insensitively_on_windows() {
        let mut codex = agent("codex", "codex");
        codex.envs = vec![CodingAgentEnv {
            key: if cfg!(windows) {
                "agentscommander_token".to_string()
            } else {
                "AGENTSCOMMANDER_TOKEN".to_string()
            },
            value: "bad".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();
        assert!(err.contains("reserved"), "{err}");
    }

    #[test]
    fn profile_codex_home_is_effective_when_isolation_is_off() {
        let profile_home = std::env::temp_dir().join("profile-codex-home");
        let mut settings = AppSettings {
            agents: vec![agent("codex", "codex")],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: BTreeMap::from([(
                        "CODEX_HOME".to_string(),
                        profile_home.to_string_lossy().to_string(),
                    )]),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();

        assert_eq!(
            spawn.effective_codex_home.as_deref(),
            Some(profile_home.as_path())
        );
        assert!(spawn.generated_env.is_empty());
    }

    #[test]
    fn ac_root_command_expands_after_parse_for_replica_launch_root() {
        let temp = tempfile::tempdir().unwrap();
        let replica = temp
            .path()
            .join("root with spaces")
            .join(".ac")
            .join("wg-7-dev-team")
            .join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();
        let mut settings = AppSettings {
            agents: vec![agent("codex", "codex")],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "%AC_REPLICA_ROOT%\\bin\\codex.exe --flag".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "codex", Some(&replica), Some("A"))
            .expect("replica launch root should expand");

        // build_agent_spawn_command canonicalizes the launch root before expanding
        // %AC_REPLICA_ROOT% (see placeholders.rs). On Windows, canonicalization resolves an
        // 8.3 short path component (e.g. a CI runner's RUNNER~1) back to its long
        // form (runneradmin), whereas the temp dir may be reported in 8.3 short form
        // when the username exceeds 8 chars. Normalize the expected value through the
        // same canonicalize + verbatim-prefix strip so the comparison does not depend
        // on which form the OS hands back.
        let canonical_root = std::fs::canonicalize(&replica).unwrap();
        let canonical_text = canonical_root.to_string_lossy();
        let expected_root = canonical_text
            .strip_prefix(r"\\?\")
            .map(std::path::PathBuf::from)
            .unwrap_or(canonical_root);

        assert_eq!(
            spawn.shell,
            expected_root.join("bin").join("codex.exe").to_string_lossy()
        );
        assert_eq!(spawn.shell_args, vec!["--flag"]);
    }

    #[test]
    fn ac_root_in_normal_repo_launch_returns_clear_error() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo-thing");
        std::fs::create_dir_all(&repo).unwrap();
        let settings = AppSettings {
            agents: vec![agent("codex", "%AC_REPLICA_ROOT%\\bin\\codex.exe")],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", Some(&repo), Some("A"))
            .expect_err("normal repo launch must reject AC_REPLICA_ROOT");

        assert!(err.contains("%AC_REPLICA_ROOT% requires an AC replica or root-agent launch root"));
    }

    #[test]
    fn workspace_and_matrix_placeholders_expand_in_profile_env_for_replica_launch() {
        let temp = tempfile::tempdir().unwrap();
        let replica = temp
            .path()
            .join("root with spaces")
            .join(".ac")
            .join("wg-7-dev-team")
            .join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();
        let mut settings = AppSettings {
            agents: vec![agent("claude", "claude")],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("claude".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: BTreeMap::from([
                        (
                            "CLAUDE_CONFIG_DIR".to_string(),
                            "%AC_WORKSPACE_ROOT%\\.claude".to_string(),
                        ),
                        (
                            "CLAUDE_MATRIX_DIR".to_string(),
                            "%AC_MATRIX_ROOT%\\.claude".to_string(),
                        ),
                    ]),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A"))
            .expect("replica launch root should expand workspace + matrix placeholders");
        let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();

        // Mirror the canonicalize + verbatim-prefix strip the builder applies, then
        // derive the .ac workspace ancestor and the matrix dir from it.
        let canonical_root = std::fs::canonicalize(&replica).unwrap();
        let canonical_text = canonical_root.to_string_lossy();
        let expected_replica = canonical_text
            .strip_prefix(r"\\?\")
            .map(std::path::PathBuf::from)
            .unwrap_or(canonical_root);
        let expected_workspace = expected_replica
            .parent()
            .and_then(|parent| parent.parent())
            .expect("replica has a .ac workspace ancestor");
        let expected_matrix = expected_workspace.join("_agent_dev-rust");

        let expected_config = format!("{}\\.claude", expected_workspace.to_string_lossy());
        let expected_matrix_claude = format!("{}\\.claude", expected_matrix.to_string_lossy());
        assert_eq!(env.get("CLAUDE_CONFIG_DIR"), Some(&expected_config));
        assert_eq!(env.get("CLAUDE_MATRIX_DIR"), Some(&expected_matrix_claude));
    }

    #[test]
    fn matrix_codex_home_expands_to_absolute_path_for_replica_launch() {
        let temp = tempfile::tempdir().unwrap();
        let replica = temp
            .path()
            .join("root with spaces")
            .join(".ac")
            .join("wg-7-dev-team")
            .join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();
        let mut settings = AppSettings {
            agents: vec![agent("codex", "codex")],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: BTreeMap::from([(
                        "CODEX_HOME".to_string(),
                        "%AC_MATRIX_ROOT%\\.codex".to_string(),
                    )]),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "codex", Some(&replica), Some("A"))
            .expect("matrix CODEX_HOME should expand to an absolute path");

        let canonical_root = std::fs::canonicalize(&replica).unwrap();
        let canonical_text = canonical_root.to_string_lossy();
        let expected_replica = canonical_text
            .strip_prefix(r"\\?\")
            .map(std::path::PathBuf::from)
            .unwrap_or(canonical_root);
        let expected_matrix = expected_replica
            .parent()
            .and_then(|parent| parent.parent())
            .expect("replica has a .ac workspace ancestor")
            .join("_agent_dev-rust");

        let home = spawn
            .effective_codex_home
            .expect("codex home resolves from the matrix placeholder");
        assert!(home.is_absolute(), "{}", home.display());
        // Mirror the builder's string concatenation (matrix + literal "\.codex"). Comparing
        // the whole path keeps the assertion exact on Windows and still correct on platforms
        // where the backslash is an ordinary character rather than a path separator, so the
        // test does not silently become Windows-only.
        let expected_home =
            std::path::PathBuf::from(format!("{}\\.codex", expected_matrix.to_string_lossy()));
        assert_eq!(home, expected_home);
    }

    #[test]
    fn non_path_env_allows_literal_dollar_but_rejects_unknown_percent_marker() {
        let mut codex = agent("codex", "codex");
        codex.envs = vec![
            CodingAgentEnv {
                key: "SECRET".to_string(),
                value: "s3cr$tP4ss".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "BAD_MARKER".to_string(),
                value: "%UNKNOWN%".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
        ];
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();

        assert!(err.contains("unknown placeholder"), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn duplicate_env_keys_are_rejected_case_insensitively_on_windows() {
        let mut codex = agent("codex", "codex");
        codex.envs = vec![
            CodingAgentEnv {
                key: "OPENAI_API_BASE".to_string(),
                value: "one".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "openai_api_base".to_string(),
                value: "two".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
        ];
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();

        assert!(err.contains("duplicate env keys"), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn git_pull_before_wraps_tokens_without_raw_concatenation() {
        let mut codex = agent("codex", "codex --config effort=low");
        codex.git_pull_before = true;
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();

        assert_eq!(spawn.shell, "cmd.exe");
        assert_eq!(spawn.shell_args[0], "/K");
        assert_eq!(spawn.shell_args[1], "git pull && codex --config effort=low");
    }

    #[cfg(windows)]
    #[test]
    fn git_pull_before_rejects_expansion_tokens() {
        let mut codex = agent("codex", "codex --config %USERPROFILE%");
        codex.git_pull_before = true;
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };

        let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();
        assert!(err.contains("unknown placeholder"), "{err}");
    }

    // #529 - instructions filename resolver + safety validation.

    #[test]
    fn default_instructions_filename_maps_by_detected_kind() {
        assert_eq!(
            default_instructions_filename_for_command("claude"),
            "CLAUDE.md"
        );
        assert_eq!(
            default_instructions_filename_for_command("claude-mb --effort max"),
            "CLAUDE.md"
        );
        assert_eq!(
            default_instructions_filename_for_command("cmd.exe /c claude"),
            "CLAUDE.md"
        );
        assert_eq!(
            default_instructions_filename_for_command("gemini -m gpt-5"),
            "GEMINI.md"
        );
        assert_eq!(default_instructions_filename_for_command("codex"), "AGENTS.md");
        assert_eq!(
            default_instructions_filename_for_command("opencode"),
            "AGENTS.md"
        );
        assert_eq!(
            default_instructions_filename_for_command("my-custom-agent"),
            "AGENTS.md"
        );
        // Unparseable (unclosed quote) and empty both fall back to AGENTS.md.
        assert_eq!(
            default_instructions_filename_for_command("codex \"unterminated"),
            "AGENTS.md"
        );
        assert_eq!(default_instructions_filename_for_command(""), "AGENTS.md");
    }

    #[test]
    fn resolve_instructions_filename_prefers_valid_explicit() {
        let mut a = agent("x", "codex");
        // No explicit value -> command-derived default.
        assert_eq!(resolve_instructions_filename(&a), "AGENTS.md");
        // Valid explicit value wins.
        a.instructions_filename = Some("Squad.md".to_string());
        assert_eq!(resolve_instructions_filename(&a), "Squad.md");
        // Surrounding whitespace is trimmed.
        a.instructions_filename = Some("  Squad.md  ".to_string());
        assert_eq!(resolve_instructions_filename(&a), "Squad.md");
        // Whitespace-only -> command default.
        a.instructions_filename = Some("   ".to_string());
        assert_eq!(resolve_instructions_filename(&a), "AGENTS.md");
        // Unsafe value (path escape) silently falls back to the default; it never
        // reaches the writer.
        a.instructions_filename = Some("../escape.md".to_string());
        assert_eq!(resolve_instructions_filename(&a), "AGENTS.md");
        // Claude command default is CLAUDE.md when no explicit value is set.
        let claude = agent("c", "claude");
        assert_eq!(resolve_instructions_filename(&claude), "CLAUDE.md");
    }

    #[test]
    fn managed_instructions_filenames_dedupes_across_agents() {
        let settings = AppSettings {
            agents: vec![
                agent("claude", "claude"),     // CLAUDE.md
                agent("codex", "codex"),       // AGENTS.md
                agent("gemini", "gemini"),     // GEMINI.md
                agent("opencode", "opencode"), // AGENTS.md (dup of codex)
            ],
            ..AppSettings::default()
        };
        let mut got = managed_instructions_filenames(&settings);
        got.sort();
        assert_eq!(
            got,
            vec![
                "AGENTS.md".to_string(),
                "CLAUDE.md".to_string(),
                "GEMINI.md".to_string()
            ]
        );
    }

    #[test]
    fn resolve_target_filename_covers_all_four_branches() {
        use crate::config::session_context::ManagedContextTarget;
        let mut configured = agent("opencode", "opencode");
        configured.instructions_filename = Some("Team.md".to_string());
        let settings = AppSettings {
            agents: vec![configured, agent("codex", "codex")],
            ..AppSettings::default()
        };

        // 1. configured agent with explicit valid filename wins.
        assert_eq!(
            resolve_target_filename(Some("opencode"), &settings, None).as_deref(),
            Some("Team.md")
        );
        // 2. configured agent without explicit -> command default (overrides detection).
        assert_eq!(
            resolve_target_filename(Some("codex"), &settings, Some(ManagedContextTarget::Claude))
                .as_deref(),
            Some("AGENTS.md")
        );
        // 3. unknown id -> detection fallback.
        assert_eq!(
            resolve_target_filename(Some("ghost"), &settings, Some(ManagedContextTarget::Gemini))
                .as_deref(),
            Some("GEMINI.md")
        );
        // 3b. no id -> detection fallback.
        assert_eq!(
            resolve_target_filename(None, &settings, Some(ManagedContextTarget::Codex)).as_deref(),
            Some("AGENTS.md")
        );
        // 4. neither configured agent nor detection -> None.
        assert_eq!(resolve_target_filename(None, &settings, None), None);
        assert_eq!(resolve_target_filename(Some("ghost"), &settings, None), None);
    }

    #[test]
    fn is_safe_instructions_filename_accepts_bare_md_names() {
        for ok in [
            "AGENTS.md",
            "CLAUDE.md",
            "GEMINI.md",
            "MyTeam.md",
            "AGENTS.MD",
            "a.md",
            "my-notes.md",
            // COM0/LPT0 are NOT reserved devices.
            "COM0.md",
            "LPT0.md",
        ] {
            assert!(is_safe_instructions_filename(ok), "should accept {ok:?}");
        }
    }

    #[test]
    fn is_safe_instructions_filename_rejects_unsafe_names() {
        let bad = [
            "",                 // empty
            "   ",              // whitespace only
            ".md",              // empty stem
            "AGENTS.txt",       // wrong extension
            "AGENTS",           // no extension
            "a/b.md",           // forward separator
            "a\\b.md",          // backslash separator
            "..\\x.md",         // traversal + separator
            "../x.md",          // traversal
            "a..md",            // contains ..
            "C:x.md",           // drive prefix (colon)
            "AGENTS.md:evil",   // NTFS Alternate Data Stream (colon)
            "AGENTS.md ",       // trailing space
            "AGENTS.md.",       // trailing dot
            "AGENTS .md",       // space immediately before extension
            "a\nb.md",          // control char
            "CON.md",           // reserved device
            "con.md",           // reserved device (case-insensitive)
            "PRN.md",
            "aux.md",
            "NUL.md",
            "nul.md",
            "COM1.md",
            "com9.md",
            "LPT1.md",
            "lpt9.md",
            "CON.foo.md", // device name resolved by the segment before the first dot
            "nul.x.md",   // device name (case-insensitive) with extra dot segment
            "last_ac_context.md", // G9 internal sentinel
            "LAST_AC_CONTEXT.md", // G9 (case-insensitive)
        ];
        for name in bad {
            assert!(!is_safe_instructions_filename(name), "should reject {name:?}");
        }
        // Length cap (> 128 chars).
        let long = format!("{}.md", "a".repeat(130));
        assert!(
            !is_safe_instructions_filename(&long),
            "should reject an over-long name"
        );
    }

    // #576 follow-up - auto-create OPENCODE_CONFIG_DIR before spawn.

    fn opencode_env(value: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("OPENCODE_CONFIG_DIR".to_string(), value.to_string())])
    }

    #[test]
    fn command_runs_opencode_matches_basename_and_wrapper() {
        assert!(command_runs_opencode("opencode", &[]));
        assert!(command_runs_opencode("opencode.exe", &[]));
        assert!(command_runs_opencode(
            r"C:\tools\opencode.exe",
            &["--flag".to_string()]
        ));
        // cmd /c opencode wrapper: opencode appears as an arg token.
        assert!(command_runs_opencode(
            "cmd.exe",
            &["/c".to_string(), "opencode".to_string()]
        ));
        // Non-opencode commands do not match.
        assert!(!command_runs_opencode("codex", &[]));
        assert!(!command_runs_opencode("claude", &["--resume".to_string()]));
    }

    #[test]
    fn find_opencode_config_dir_prefers_profile_over_agent() {
        let agent_env = opencode_env("agent-value");
        let profile_env = opencode_env("profile-value");
        assert_eq!(
            find_opencode_config_dir(&agent_env, &profile_env).map(String::as_str),
            Some("profile-value")
        );
        // Falls back to the agent layer when the profile lacks the key.
        let empty = BTreeMap::new();
        assert_eq!(
            find_opencode_config_dir(&agent_env, &empty).map(String::as_str),
            Some("agent-value")
        );
        // None when neither layer sets it.
        assert_eq!(find_opencode_config_dir(&empty, &empty), None);
    }

    #[test]
    fn ensure_opencode_config_dir_creates_missing_absolute_dir() {
        let temp = tempfile::tempdir().unwrap();
        // Nested + missing, to prove create_dir_all builds the whole chain.
        let target = temp.path().join("nested").join(".opencode");
        assert!(!target.exists());
        let profile_env = opencode_env(&target.to_string_lossy());

        let outcome = ensure_opencode_config_dir("opencode", &[], &BTreeMap::new(), &profile_env);

        assert_eq!(outcome, OpencodeConfigDirOutcome::Ensured);
        assert!(target.is_dir(), "config dir should have been created");
    }

    #[test]
    fn ensure_opencode_config_dir_existing_dir_is_noop_success() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join(".opencode");
        std::fs::create_dir_all(&target).unwrap();
        let agent_env = opencode_env(&target.to_string_lossy());

        let outcome = ensure_opencode_config_dir("opencode", &[], &agent_env, &BTreeMap::new());

        assert_eq!(outcome, OpencodeConfigDirOutcome::Ensured);
        assert!(target.is_dir());
    }

    #[test]
    fn ensure_opencode_config_dir_skips_relative_value() {
        // Not an absolute path -> never created (cannot trust where it would land).
        let profile_env = opencode_env(r"relative\.opencode");
        let outcome = ensure_opencode_config_dir("opencode", &[], &BTreeMap::new(), &profile_env);
        assert_eq!(outcome, OpencodeConfigDirOutcome::Skipped);
    }

    #[test]
    fn ensure_opencode_config_dir_skips_non_opencode_command() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join(".opencode");
        let profile_env = opencode_env(&target.to_string_lossy());

        let outcome = ensure_opencode_config_dir("codex", &[], &BTreeMap::new(), &profile_env);

        assert_eq!(outcome, OpencodeConfigDirOutcome::Skipped);
        assert!(!target.exists(), "must not create the dir for a non-opencode command");
    }

    #[test]
    fn ensure_opencode_config_dir_skips_when_unset() {
        let outcome =
            ensure_opencode_config_dir("opencode", &[], &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(outcome, OpencodeConfigDirOutcome::Skipped);
    }

    #[test]
    fn ensure_opencode_config_dir_warns_and_continues_on_create_failure() {
        // Make an ancestor a regular file so create_dir_all cannot succeed.
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").unwrap();
        let target = blocker.join(".opencode");
        let profile_env = opencode_env(&target.to_string_lossy());

        let outcome = ensure_opencode_config_dir("opencode", &[], &BTreeMap::new(), &profile_env);

        assert_eq!(outcome, OpencodeConfigDirOutcome::Failed);
        assert!(!target.exists());
    }

    #[test]
    fn build_spawn_creates_opencode_config_dir_for_literal_absolute_value() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("nested").join(".opencode");
        assert!(!target.exists());

        let mut settings = AppSettings {
            agents: vec![agent("opencode", "opencode")],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("opencode".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: opencode_env(&target.to_string_lossy()),
                    notes: String::new(),
                },
            );

        // launch_path None is fine: a literal absolute value needs no placeholder context.
        let spawn = build_agent_spawn_command(&settings, "opencode", None, Some("A")).unwrap();

        assert!(target.is_dir(), "build should have created OPENCODE_CONFIG_DIR");
        let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();
        assert_eq!(
            env.get("OPENCODE_CONFIG_DIR").map(String::as_str),
            Some(target.to_string_lossy().as_ref())
        );
    }
}
