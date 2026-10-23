use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::config::coding_agent_profiles::{
    resolve_profile, ProfileResolution, ProfileResolutionRequest,
};
use crate::config::instance_artifacts::CODEX_HOME_DIR_NAME;
use crate::config::placeholders::{
    ac_placeholder_error, expand_placeholders, expand_placeholders_in_args,
    placeholder_context_for_launch_root, reject_unexpanded_markers, value_contains_ac_placeholder,
    PlaceholderContext,
};
use crate::config::session_context::ManagedContextTarget;
use crate::config::settings::{
    command_token_basename, is_codex_home_key, is_opencode_config_dir_key,
    normalize_env_key_for_platform, validate_expanded_codex_home_value, validate_user_env_key,
    AgentBackendConfig, AgentConfig, AppSettings, ProfileCellConfig,
};
use crate::session::profile::CodingAgentKind;
use sha2::{Digest, Sha256};

/// #1551 - true when `token` is a bare program name (no path separator, not absolute):
/// the only form resolved through PATH, and the only form the version probe executes.
pub fn is_bare_program_token(token: &str) -> bool {
    !(token.contains('/') || token.contains('\\') || Path::new(token).is_absolute())
}

/// #1551 - resolve a program token to a file. Lifted byte-equivalently from the
/// `resolve_token_to_file` helper that `commands::session` used for the claude token:
/// explicit path (separator or absolute) -> Some iff it is a file, never consulting
/// PATH; bare name -> `which::which_in` (plus PATHEXT on Windows, so npm `.cmd`
/// shims resolve). #2589 - bare names are searched in
/// `agent_path::effective_search_path`: the inherited PATH first, then the user
/// bin dirs a GUI launch leaves out.
pub fn resolve_program(token: &str) -> Option<PathBuf> {
    resolve_program_in(token, &crate::config::agent_path::effective_search_path())
}

/// #2589 - `resolve_program` against an explicit search path.
pub fn resolve_program_in(token: &str, search_path: &OsStr) -> Option<PathBuf> {
    let p = Path::new(token);
    if !is_bare_program_token(token) {
        return if p.is_file() {
            Some(p.to_path_buf())
        } else {
            None
        };
    }
    which::which_in(
        token,
        Some(search_path),
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    )
    .ok()
}

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
    /// 16-hex SHA-256 of the effective resolved cell's RAW command + env
    /// (#592). Drift detector only; positional identity stays the locator.
    pub profile_content_hash: String,
    pub trusted_agent_id: String,
    pub trusted_agent_label: String,
    pub backend: AgentBackendConfig,
    /// Filesystem preparation deferred until immediately before spawn. This is
    /// private so callers cannot fabricate or mutate the resolution contract.
    isolated_codex_home_to_prepare: Option<PathBuf>,
    /// #598 - resolved config-folder seed (pure path math; executed at the
    /// session chokepoint, never here). `None` when the agent has no active seed
    /// or the launch root is not an AC replica/root-agent.
    pub seed: Option<crate::config::config_seed::ResolvedConfigSeed>,
    /// #2434 - the #2451 `matchTier` carrier value (`MatchTier::as_wire_str`).
    /// `None` unless a restart resolved a reference through the matcher.
    pub match_tier: Option<String>,
    /// #2434 - the #2451 `originalProfileLetter` carrier value.
    pub original_profile_letter: Option<String>,
}

impl AgentSpawnCommand {
    pub fn effective_child_env(&self) -> Vec<(String, String)> {
        self.child_env
            .iter()
            .filter(|(key, _)| {
                !self
                    .env_remove_keys
                    .iter()
                    .any(|remove| env_key_matches_platform(remove, key))
            })
            .cloned()
            .collect()
    }

    pub fn effective_env_value(&self, key: &str) -> Option<&str> {
        let wanted = normalize_env_key_for_platform(key);
        self.child_env
            .iter()
            .rev()
            .filter(|(candidate, _)| {
                !self
                    .env_remove_keys
                    .iter()
                    .any(|remove| env_key_matches_platform(remove, candidate))
            })
            .find(|(candidate, _)| normalize_env_key_for_platform(candidate) == wanted)
            .map(|(_, value)| value.as_str())
    }
}

fn env_key_matches_platform(left: &str, right: &str) -> bool {
    normalize_env_key_for_platform(left) == normalize_env_key_for_platform(right)
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

/// Lowercased final path component WITH its extension (unlike
/// [`executable_basename`], which strips the last extension via `file_stem`).
/// Used by the opencode arg-scan to match exact executable forms and reject
/// look-alikes such as `opencode.md`.
fn executable_filename(s: &str) -> String {
    std::path::Path::new(s)
        .file_name()
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
    isolated_home_to_prepare: Option<PathBuf>,
}

fn compute_codex_home(
    agent: &AgentConfig,
    home_key: Option<&str>,
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
            isolated_home_to_prepare: None,
        });
    }

    if agent.isolated_home {
        let leaf = match home_key {
            Some(key)
                if !key.is_empty()
                    && key.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || byte == b'-'
                            || byte == b'_'
                    }) =>
            {
                key.to_string()
            }
            Some(_) => {
                return Err(format!(
                    "Invalid isolated Codex HOME key for '{}'",
                    agent.id
                ))
            }
            None => sanitize_codex_home_id(&agent.id),
        };
        let config_dir = crate::config::config_dir()
            .ok_or_else(|| "Could not determine config directory for CODEX_HOME".to_string())?;
        let home = config_dir.join(CODEX_HOME_DIR_NAME).join(leaf);
        generated_env.insert("CODEX_HOME".to_string(), home.to_string_lossy().to_string());
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: Some(home.clone()),
            env_remove_keys,
            isolated_home_to_prepare: Some(home),
        });
    }

    if let Some(value) = find_env_value(profile_env, "CODEX_HOME") {
        let path = validate_expanded_codex_home_value(value, "Profile CODEX_HOME")?;
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: Some(path),
            env_remove_keys,
            isolated_home_to_prepare: None,
        });
    }
    if let Some(value) = find_env_value(agent_env, "CODEX_HOME") {
        let path = validate_expanded_codex_home_value(value, "Agent CODEX_HOME")?;
        return Ok(ComputedCodexHome {
            generated_env,
            effective_codex_home: Some(path),
            env_remove_keys,
            isolated_home_to_prepare: None,
        });
    }
    if let Ok(value) = std::env::var("CODEX_HOME") {
        match validate_expanded_codex_home_value(&value, "Inherited CODEX_HOME") {
            Ok(path) => {
                return Ok(ComputedCodexHome {
                    generated_env,
                    effective_codex_home: Some(path),
                    env_remove_keys,
                    isolated_home_to_prepare: None,
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
        isolated_home_to_prepare: None,
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

/// Executable file_name forms that count as opencode when found among the
/// command ARGS. The bare `opencode` covers the `cmd /c opencode` wrapper; the
/// `.exe`/`.cmd`/`.bat` forms cover a direct path or an npm-style shim.
const OPENCODE_ARG_FORMS: [&str; 4] = ["opencode", "opencode.exe", "opencode.cmd", "opencode.bat"];

/// True when the launch command runs opencode, matched by executable name.
/// There is no `CodingAgentKind::OpenCode` (the closed enum covers Claude,
/// Codex, Antigravity, and Pi, but `detect` does not know opencode), so this mirrors the
/// `executable_basename(shell) == "codex"` fallback in [`compute_codex_home`].
///
/// The `shell` (the program being launched) is matched on `file_stem`, like the
/// codex fallback: an `opencode.exe`/`.cmd` shell still resolves to `opencode`.
/// Args are scanned with a tighter rule: the token `file_name` (extension kept)
/// must be one of [`OPENCODE_ARG_FORMS`]. This still recognizes a
/// `cmd /c opencode` wrapper, but does NOT mistake a non-opencode agent whose
/// command merely references an `opencode.md` (or `.json`) file for an opencode
/// launch (a `file_stem` match would, since `file_stem("opencode.md")` is
/// `opencode`). An arg cannot be a bare-stem executable the way a shell can, so
/// the extension carries real signal here.
///
/// `pub(crate)` so `config_seed::compute_config_dir_warning` can reuse it (#598).
pub(crate) fn command_runs_opencode(shell: &str, shell_args: &[String]) -> bool {
    if executable_basename(shell) == "opencode" {
        return true;
    }
    shell_args
        .iter()
        .flat_map(|arg| arg.split_whitespace())
        .any(|token| OPENCODE_ARG_FORMS.contains(&executable_filename(token).as_str()))
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
        .or_else(|| {
            agent_env
                .iter()
                .find(|(key, _)| is_opencode_config_dir_key(key))
        })
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
            log::info!(
                "[opencode] Ensured OPENCODE_CONFIG_DIR '{}'",
                path.display()
            );
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
pub fn cell_identity(agent: &AgentConfig, cell: &ProfileCellConfig) -> String {
    profile_content_hash(
        &compose_effective_command(&agent.command, &cell.command),
        &raw_merged_profile_env(agent, &cell.env),
    )
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
fn render_command_token(token: &str) -> String {
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
    let buf = profile_hash_preimage(command, env);
    let digest = format!("{:x}", Sha256::digest(buf.as_bytes()));
    // Hex is ASCII single-byte; slicing the first 16 chars is char-boundary safe.
    digest[..16].to_string()
}

/// #2434 - which tier of `resolve_portable_reference` matched. A tier-0 local
/// id hit guesses nothing and is `None` wherever this is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchTier {
    Hash,
    LabelAndLetter,
    CommandAndLetter,
}

impl MatchTier {
    /// The wire string the #2451 `matchTier` carrier holds; equals the serde form.
    pub fn as_wire_str(self) -> &'static str {
        match self {
            MatchTier::Hash => "hash",
            MatchTier::LabelAndLetter => "labelAndLetter",
            MatchTier::CommandAndLetter => "commandAndLetter",
        }
    }
}

/// #2434 - a stored coding-agent reference, built by the caller from
/// `tooling.currentCodingAgent` / `lastCodingAgent` plus, when present, the
/// `tooling.codingAgents[<id>]` descriptor #2433 wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredReference {
    /// The stored id; it IS the reference.
    pub id: String,
    /// `descriptor.app`.
    pub app_label: Option<String>,
    /// `descriptor.command`, already canonical.
    pub command: Option<String>,
    /// Letter -> cell digest; empty when an older build wrote the descriptor.
    pub identity: BTreeMap<String, String>,
    /// The letter this launch asks for.
    pub source_letter: Option<String>,
}

impl StoredReference {
    /// Build the reference for `id` from a parsed agent `config.json`. A missing
    /// or non-object descriptor yields no descriptor fields; empty strings read
    /// as absent (an older build wrote them).
    pub fn from_config(config: Option<&serde_json::Value>, id: &str) -> Self {
        let descriptor = config
            .and_then(|value| value.get("tooling"))
            .and_then(|tooling| tooling.get("codingAgents"))
            .and_then(|agents| agents.get(id))
            .filter(|entry| entry.is_object());
        let text = |key: &str| {
            descriptor
                .and_then(|entry| entry.get(key))
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let identity = descriptor
            .and_then(|entry| entry.get("identity"))
            .and_then(|value| value.as_object())
            .map(|map| {
                map.iter()
                    .filter_map(|(letter, digest)| {
                        digest.as_str().map(|d| (letter.clone(), d.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        StoredReference {
            id: id.to_string(),
            app_label: text("app"),
            command: text("command"),
            identity,
            source_letter: None,
        }
    }
}

/// #2434 - a resolved reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMatch {
    pub agent_id: String,
    /// The LOCAL letter to launch, or `None` when the caller asked for no letter
    /// and none was matched. `Some` at tier 1, where the match IS a letter.
    pub profile_letter: Option<String>,
    /// `None` for a tier-0 local-id hit: nothing was guessed. Wire: `null`.
    pub tier: Option<MatchTier>,
    /// The key in `reference.identity` whose digest matched, when it differs
    /// from `profile_letter`; at tiers 2-3 the source letter when it differs.
    pub original_letter: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    Matched(AgentMatch),
    NoMatch { reference: String },
}

/// #2434 - true when `letter` launches for `agent_id`: an enabled cell, or the
/// implicit `A` that `resolve_profile` always resolves (to an empty cell when
/// missing or disabled). Mirrors `coding_agent_profiles::cell_for_letter`.
fn profile_letter_exists(settings: &AppSettings, agent_id: &str, letter: &str) -> bool {
    letter == "A"
        || settings
            .coding_agent_profiles
            .profiles_by_agent
            .get(agent_id)
            .and_then(|cells| cells.get(letter))
            .is_some_and(|cell| cell.enabled)
}

/// #2434 - resolve a stored reference to a local agent and letter. Pure: it
/// writes nothing, and callers must not write the result back. Tiers apply in
/// strict precedence: local id, cell digest, label + letter, command + letter.
/// Every tie takes the earliest entry in `settings.agents` order and logs both.
pub fn resolve_portable_reference(
    settings: &AppSettings,
    reference: &StoredReference,
) -> MatchOutcome {
    let mut warnings = Vec::new();
    let outcome = resolve_portable_reference_logged(settings, reference, &mut warnings);
    for warning in warnings {
        log::warn!("[agent-match] {}", warning);
    }
    outcome
}

fn resolve_portable_reference_logged(
    settings: &AppSettings,
    reference: &StoredReference,
    warnings: &mut Vec<String>,
) -> MatchOutcome {
    let source = reference.source_letter.as_deref();

    // Tier 0: the id is local. Same machine, unchanged behaviour.
    if settings.agents.iter().any(|agent| agent.id == reference.id) {
        return MatchOutcome::Matched(AgentMatch {
            agent_id: reference.id.clone(),
            profile_letter: reference.source_letter.clone(),
            tier: None,
            original_letter: None,
        });
    }

    // Tier 1: one target digest (the source letter's), else every descriptor digest.
    if !reference.identity.is_empty() {
        let targets: Vec<(&String, &String)> =
            match source.and_then(|letter| reference.identity.get_key_value(letter)) {
                Some(target) => vec![target],
                None => reference.identity.iter().collect(),
            };
        // (agent index, local letter, matched descriptor key), settings order.
        let mut hits: Vec<(usize, String, String)> = Vec::new();
        for (index, agent) in settings.agents.iter().enumerate() {
            let Some(cells) = settings
                .coding_agent_profiles
                .profiles_by_agent
                .get(&agent.id)
            else {
                continue;
            };
            for (letter, cell) in cells.iter().filter(|(_, cell)| cell.enabled) {
                let digest = cell_identity(agent, cell);
                let matched: Vec<&String> = targets
                    .iter()
                    .filter(|(_, target)| **target == digest)
                    .map(|(key, _)| *key)
                    .collect();
                let key = matched
                    .iter()
                    .find(|key| **key == letter)
                    .or_else(|| matched.first());
                if let Some(key) = key {
                    hits.push((index, letter.clone(), (*key).clone()));
                }
            }
        }
        if let Some((winner, _, _)) = hits.first() {
            let winner = *winner;
            let agent_id = &settings.agents[winner].id;
            if let Some((other, letter, _)) = hits.iter().find(|(index, _, _)| *index != winner) {
                warnings.push(format!(
                    "reference '{}' matched by digest in agents '{}' and '{}' (letter {}); using '{}'",
                    reference.id, agent_id, settings.agents[*other].id, letter, agent_id
                ));
            }
            let tied: Vec<&(usize, String, String)> = hits
                .iter()
                .filter(|(index, _, _)| *index == winner)
                .collect();
            let chosen = tied
                .iter()
                .find(|(_, letter, _)| Some(letter.as_str()) == source)
                .unwrap_or(&tied[0]);
            if tied.len() > 1 {
                let letters: Vec<&str> = tied.iter().map(|(_, l, _)| l.as_str()).collect();
                warnings.push(format!(
                    "reference '{}' matched letters {} of agent '{}' by digest; using {}",
                    reference.id,
                    letters.join(" and "),
                    agent_id,
                    chosen.1
                ));
            }
            let (_, letter, key) = (*chosen).clone();
            return MatchOutcome::Matched(AgentMatch {
                agent_id: agent_id.clone(),
                original_letter: (key != letter).then_some(key),
                profile_letter: Some(letter),
                tier: Some(MatchTier::Hash),
            });
        }
    }

    // Tiers 2-3 test the candidate against the source letter, or the implicit
    // `A`; that substitute never becomes `profile_letter`.
    let test_letter = source.unwrap_or("A");
    let tiers: [(MatchTier, Option<&str>); 2] = [
        (MatchTier::LabelAndLetter, reference.app_label.as_deref()),
        (MatchTier::CommandAndLetter, reference.command.as_deref()),
    ];
    for (tier, wanted) in tiers {
        let Some(wanted) = wanted.filter(|value| !value.is_empty()) else {
            continue;
        };
        let candidates: Vec<&AgentConfig> = settings
            .agents
            .iter()
            .filter(|agent| match tier {
                MatchTier::LabelAndLetter => agent.label.eq_ignore_ascii_case(wanted),
                _ => canonical_command_text(&agent.command) == wanted,
            })
            .filter(|agent| profile_letter_exists(settings, &agent.id, test_letter))
            .collect();
        let Some(first) = candidates.first() else {
            continue;
        };
        if let Some(second) = candidates.get(1) {
            warnings.push(format!(
                "reference '{}' matched agents '{}' and '{}' by {}; using '{}'",
                reference.id,
                first.id,
                second.id,
                tier.as_wire_str(),
                first.id
            ));
        }
        return MatchOutcome::Matched(AgentMatch {
            agent_id: first.id.clone(),
            profile_letter: reference.source_letter.clone(),
            tier: Some(tier),
            // The profile letter IS the source letter here, so it never differs.
            original_letter: None,
        });
    }

    MatchOutcome::NoMatch {
        reference: reference.id.clone(),
    }
}

fn normalize_launch_path_for_spawn(launch_path: Option<&Path>) -> Option<PathBuf> {
    launch_path.map(crate::path_utils::normalize_windows_verbatim_path_buf)
}

pub(crate) fn resolve_agent_spawn_command(
    settings: &AppSettings,
    agent_id: &str,
    launch_path: Option<&Path>,
    requested_profile: Option<&str>,
    requested_profile_authoritative: bool,
) -> Result<AgentSpawnCommand, String> {
    let normalized_launch_path = normalize_launch_path_for_spawn(launch_path);
    let launch_path = normalized_launch_path.as_deref();
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
            requested_profile_authoritative,
        },
    );
    for warning in &profile_resolution.warnings {
        log::warn!("[profiles] {}", warning);
    }

    // #598 - active config-folder seed for this agent (resolved below).
    // `is_active` = enabled AND non-empty dest.
    let seed_choice = agent.config_seed.as_ref().filter(|c| c.is_active());

    // #597 - the effective command CONCATENATES the agent base command (the
    // binary, possibly with its own fixed args) with the profile cell command
    // (extra params): `<base> <cell>`. An empty side drops cleanly; both empty
    // yields an empty string that the tokenizer rejects, same as a blank command
    // has always been rejected.
    let effective_command =
        compose_effective_command(&agent.command, &profile_resolution.cell.command);

    // #597 - fingerprint the RAW effective command (base+cell, pre-expansion) and
    // the RAW merged env (agent enabled rows + cell, profile-wins) so an edit to
    // the base command, cell command, base env, or cell env is detectable as
    // drift. SUPERSEDES the #592 cell-only hash input.
    // #2431 - via `cell_identity`, the single composition site in this module.
    let profile_hash = cell_identity(agent, &profile_resolution.cell);
    let merged_profile_env = raw_merged_profile_env(agent, &profile_resolution.cell.env);
    // #592/#597 - surface exactly what gets hashed at spawn so a later drift
    // mismatch can be traced. Kept at debug for support; off the hot path (fires
    // once per spawn). Env VALUES are intentionally omitted (they may hold
    // secrets); the hash already fingerprints them, and the key set + count
    // reveal whether an env row participated.
    log::debug!(
        "[profile-hash] spawn-stamp: agent={} profile={} hash={} effective_command={:?} canonical_command={:?} env_keys=[{}] ({} entries)",
        agent.id,
        profile_resolution.effective_profile,
        profile_hash,
        effective_command,
        canonical_command_text(&effective_command),
        merged_profile_env
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        merged_profile_env.len(),
    );

    let normalized = normalize_legacy_agent_command(&effective_command).map_err(|e| {
        format!(
            "Invalid profile command for '{}:{}': {}. command={:?}",
            agent.id, profile_resolution.effective_profile, e, effective_command
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
            .any(|value| value_contains_ac_placeholder(value))
        // #598: an active seed needs the placeholder context (for the dest, the
        // tier candidates, and content substitution). The `launch_path.is_some()`
        // guard keeps a configured seed from turning a no-path build (e.g.
        // prevalidation) into a hard error at the `?` below.
        || (seed_choice.is_some() && launch_path.is_some());
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
    let shell = shell.clone();
    let shell_args = shell_args.to_vec();

    let agent_env = collect_agent_env(agent, placeholder_context.as_ref())?;
    let profile_env = collect_profile_env(
        &agent.id,
        &profile_resolution.effective_profile,
        &profile_resolution.cell.env,
        placeholder_context.as_ref(),
    )?;
    let computed_codex_home = compute_codex_home(
        agent,
        settings
            .coding_agent_profiles
            .codex_home_key_by_agent
            .get(&agent.id)
            .map(String::as_str),
        &shell,
        &shell_args,
        &agent_env,
        &profile_env,
    )?;
    let child_env =
        merge_env_layers(&[&agent_env, &profile_env, &computed_codex_home.generated_env]);

    // #598 - resolve the config-folder seed. Pure path math plus a log-only
    // warning string; NO filesystem is touched here. The destructive copy runs
    // later at the single session chokepoint (create_session_inner).
    let seed = match seed_choice {
        Some(cfg) => {
            // #2162: the host OS token drives the `.<os>` refinement of every tier.
            let os_token = crate::config::config_seed::host_os_token();
            let mut resolved = crate::config::config_seed::resolve_config_seed(
                cfg,
                &profile_resolution.effective_profile,
                placeholder_context.as_ref(),
                os_token,
            );
            if let Some(r) = resolved.as_mut() {
                // #769 P2 + #1318: append the absent-only, non-empty CatalogDefault
                // tier LAST (lowest precedence). Pure path math here;
                // perform_config_seed gates it on absent-dest + non-empty master,
                // so it never overwrites an existing replica config. The master
                // lives in the session workspace's `.ac/coding-agents/_seed/<dest>`
                // (replicas always have a workspace); a session without a workspace
                // falls back to the legacy `<config_dir>/coding-agents/_seed/<dest>`
                // location. The master exists only for built-ins that ship one; for
                // any other dest the candidate path is absent and the tier is inert.
                let dest = cfg.dest.trim();
                let master_root = placeholder_context
                    .as_ref()
                    .and_then(|ctx| ctx.ac_root.as_ref())
                    .cloned()
                    .or_else(crate::config::config_dir);
                if let Some(master_root) = master_root {
                    // #2162: the OS variant of the master precedes the base one,
                    // inside the same (absent-only, non-empty-gated) tier.
                    if let Some(os) = os_token {
                        r.candidates
                            .push(crate::config::config_seed::ConfigSeedCandidate {
                                tier: crate::config::config_seed::ConfigSeedTier::CatalogDefault,
                                os_specific: true,
                                path: crate::config::coding_agents_catalog::master_dir_for_dest(
                                    &master_root,
                                    &format!("{dest}.{os}"),
                                ),
                            });
                    }
                    r.candidates
                        .push(crate::config::config_seed::ConfigSeedCandidate {
                            tier: crate::config::config_seed::ConfigSeedTier::CatalogDefault,
                            os_specific: false,
                            path: crate::config::coding_agents_catalog::master_dir_for_dest(
                                &master_root,
                                dest,
                            ),
                        });
                }
                r.config_dir_warning = crate::config::config_seed::compute_config_dir_warning(
                    &r.dest,
                    &shell,
                    &shell_args,
                    &agent_env,
                    &profile_env,
                    computed_codex_home.effective_codex_home.as_deref(),
                );
            }
            resolved
        }
        None => None,
    };

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
        profile_content_hash: profile_hash,
        trusted_agent_id: agent.id.clone(),
        trusted_agent_label: agent.label.clone(),
        backend: agent.backend.clone(),
        isolated_codex_home_to_prepare: computed_codex_home.isolated_home_to_prepare,
        seed,
        match_tier: None,
        original_profile_letter: None,
    })
}

/// Perform the filesystem preparation carried by a read-only spawn resolution.
/// Isolated CODEX_HOME creation is required; OpenCode configuration-directory
/// creation preserves its existing best-effort policy.
pub(crate) fn prepare_agent_spawn_command(command: &AgentSpawnCommand) -> Result<(), String> {
    if let Some(home) = command.isolated_codex_home_to_prepare.as_ref() {
        std::fs::create_dir_all(home).map_err(|e| {
            format!(
                "Failed to create isolated CODEX_HOME '{}': {}",
                home.display(),
                e
            )
        })?;
    }
    ensure_opencode_config_dir(
        &command.shell,
        &command.shell_args,
        &command.agent_env,
        &command.profile_env,
    );
    Ok(())
}

pub fn build_agent_spawn_command(
    settings: &AppSettings,
    agent_id: &str,
    launch_path: Option<&Path>,
    requested_profile: Option<&str>,
) -> Result<AgentSpawnCommand, String> {
    let command =
        resolve_agent_spawn_command(settings, agent_id, launch_path, requested_profile, false)?;
    prepare_agent_spawn_command(&command)?;
    Ok(command)
}

#[cfg(test)]
mod tests;
