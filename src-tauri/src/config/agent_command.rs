use std::collections::BTreeMap;
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
    is_codex_home_key, is_opencode_config_dir_key, normalize_env_key_for_platform,
    validate_expanded_codex_home_value, validate_user_env_key, AgentBackendConfig, AgentConfig,
    AppSettings,
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
/// PATH; bare name -> `which::which` (PATH, plus PATHEXT on Windows, so npm `.cmd`
/// shims resolve). The GUI process PATH is what is consulted (documented caveat).
pub fn resolve_program(token: &str) -> Option<PathBuf> {
    let p = Path::new(token);
    if !is_bare_program_token(token) {
        return if p.is_file() {
            Some(p.to_path_buf())
        } else {
            None
        };
    }
    which::which(token).ok()
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
            | Some(CodingAgentKind::Antigravity) => "AGENTS.md",
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
        let config_dir = crate::config::config_dir()
            .ok_or_else(|| "Could not determine config directory for CODEX_HOME".to_string())?;
        let home = config_dir
            .join(CODEX_HOME_DIR_NAME)
            .join(sanitize_codex_home_id(&agent.id));
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
/// ENABLED env rows overlaid by the cell env (profile-wins), keys normalized for
/// the platform so a case-only difference does not double-count. Values verbatim.
/// Mirrors `merge_env_layers`' agent-then-profile precedence but stays raw and
/// excludes the generated layer (CODEX_HOME isolation etc.), which is derived
/// state, not user config (decision §0.2; see Notes for the accepted limitation).
pub fn raw_merged_profile_env(
    agent: &AgentConfig,
    cell_env: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    for row in agent.envs.iter().filter(|row| row.enabled) {
        merged.insert(normalize_env_key_for_platform(&row.key), row.value.clone());
    }
    for (key, value) in cell_env {
        merged.insert(normalize_env_key_for_platform(key), value.clone());
    }
    merged
}

/// #592 - stable 16-hex content fingerprint for profile drift detection
/// ("loaded != configured"). Hashes the RAW `command` + `env` it is given
/// (placeholders un-expanded); the primitive is agnostic to how those are built.
/// #597 - callers now pass the COMPOSED effective command (agent base + cell
/// params) via `compose_effective_command` and the merged env (agent enabled rows
/// overlaid by the cell, profile-wins) via `raw_merged_profile_env`, so an edit to
/// the base command, cell command, base env, or cell env all flip the hash
/// (SUPERSEDES the original #592 cell-only input). Env keys are normalized with
/// the same platform rule `merge_env_layers` uses (Windows case-fold), then
/// ordered via `BTreeMap`, so a case-only key edit on Windows does not false-flag
/// and iteration order is irrelevant. SHA-256 (stable across Rust versions, unlike
/// DefaultHasher), truncated to the first 16 hex chars (matches the existing
/// `profile_assignment_fingerprint` 16-hex shape).
pub fn profile_content_hash(command: &str, env: &BTreeMap<String, String>) -> String {
    use std::fmt::Write as _;
    // Normalize keys for dedup/compare; value stays verbatim (raw).
    let mut normalized: BTreeMap<String, &str> = BTreeMap::new();
    for (key, value) in env {
        normalized.insert(normalize_env_key_for_platform(key), value.as_str());
    }
    // Versioned, NUL-tagged serialization. NUL cannot appear in commands/env we
    // accept, and the field tags stop a value from forging a record boundary.
    let mut buf = String::new();
    let _ = write!(
        buf,
        "v1\u{0}cmd\u{0}{}\u{0}envc\u{0}{}\u{0}",
        command,
        normalized.len()
    );
    for (key, value) in &normalized {
        let _ = write!(buf, "k\u{0}{}\u{0}v\u{0}{}\u{0}", key, value);
    }
    let digest = format!("{:x}", Sha256::digest(buf.as_bytes()));
    // Hex is ASCII single-byte; slicing the first 16 chars is char-boundary safe.
    digest[..16].to_string()
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
    let merged_profile_env = raw_merged_profile_env(agent, &profile_resolution.cell.env);
    let profile_hash = profile_content_hash(&effective_command, &merged_profile_env);
    // #592/#597 - surface exactly what gets hashed at spawn so a later drift
    // mismatch can be traced. Kept at debug for support; off the hot path (fires
    // once per spawn). Env VALUES are intentionally omitted (they may hold
    // secrets); the hash already fingerprints them, and the key set + count
    // reveal whether an env row participated.
    log::debug!(
        "[profile-hash] spawn-stamp: agent={} profile={} hash={} effective_command={:?} env_keys=[{}] ({} entries)",
        agent.id,
        profile_resolution.effective_profile,
        profile_hash,
        effective_command,
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
    let computed_codex_home =
        compute_codex_home(agent, &shell, &shell_args, &agent_env, &profile_env)?;
    let child_env =
        merge_env_layers(&[&agent_env, &profile_env, &computed_codex_home.generated_env]);

    // #598 - resolve the config-folder seed. Pure path math plus a log-only
    // warning string; NO filesystem is touched here. The destructive copy runs
    // later at the single session chokepoint (create_session_inner).
    let seed = match seed_choice {
        Some(cfg) => {
            let mut resolved = crate::config::config_seed::resolve_config_seed(
                cfg,
                &profile_resolution.effective_profile,
                placeholder_context.as_ref(),
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
                let master_dir = placeholder_context
                    .as_ref()
                    .and_then(|ctx| ctx.ac_root.as_ref())
                    .map(|ac_root| {
                        crate::config::coding_agents_catalog::master_dir_for_dest(ac_root, dest)
                    })
                    .or_else(|| {
                        crate::config::config_dir().map(|config_dir| {
                            crate::config::coding_agents_catalog::master_dir_for_dest(
                                &config_dir,
                                dest,
                            )
                        })
                    });
                if let Some(master_dir) = master_dir {
                    r.candidates.push((
                        crate::config::config_seed::ConfigSeedTier::CatalogDefault,
                        master_dir,
                    ));
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
mod tests {
    #[cfg(windows)]
    use super::normalize_launch_path_for_spawn;
    use super::{
        build_agent_spawn_command, command_runs_opencode,
        default_instructions_filename_for_command, ensure_opencode_config_dir,
        find_opencode_config_dir, is_bare_program_token, is_safe_instructions_filename,
        managed_instructions_filenames, normalize_legacy_agent_command,
        prepare_agent_spawn_command, profile_content_hash, resolve_agent_spawn_command,
        resolve_instructions_filename, resolve_program, resolve_target_filename, AgentSpawnCommand,
        OpencodeConfigDirOutcome,
    };
    use crate::config::coding_agent_profiles::ProfileResolution;
    use crate::config::settings::{
        empty_profile_cell, AgentBackendConfig, AgentConfig, AppSettings, CodingAgentEnv,
        CodingAgentEnvSource, ConfigSeedConfig, ProfileCellConfig,
    };
    use std::collections::BTreeMap;

    #[test]
    fn normalizes_plain_command_with_args() {
        let got = normalize_legacy_agent_command("codex --yolo").unwrap();
        assert_eq!(got.shell, "codex");
        assert_eq!(got.shell_args, vec!["--yolo"]);
    }

    #[test]
    #[cfg(windows)]
    fn normalize_launch_path_for_spawn_converts_verbatim_unc() {
        let path = normalize_launch_path_for_spawn(Some(std::path::Path::new(
            r"\\?\UNC\server\share\repo\.ac\wg-1\__agent_dev",
        )))
        .expect("normalized path");

        assert_eq!(
            path,
            std::path::PathBuf::from(r"\\server\share\repo\.ac\wg-1\__agent_dev")
        );
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
        }
    }

    #[test]
    fn effective_env_helpers_apply_removals_and_last_wins() {
        let spawn = spawn_with_env(
            vec![
                ("FOO".to_string(), "one".to_string()),
                ("BAR".to_string(), "remove".to_string()),
                ("FOO".to_string(), "last".to_string()),
            ],
            vec!["BAR".to_string()],
        );

        assert_eq!(
            spawn.effective_child_env(),
            vec![
                ("FOO".to_string(), "one".to_string()),
                ("FOO".to_string(), "last".to_string())
            ]
        );
        assert_eq!(spawn.effective_env_value("FOO"), Some("last"));
        assert_eq!(spawn.effective_env_value("BAR"), None);

        if cfg!(windows) {
            assert_eq!(spawn.effective_env_value("foo"), Some("last"));
        } else {
            assert_eq!(spawn.effective_env_value("foo"), None);
        }
    }

    #[test]
    fn build_spawn_concatenates_agent_base_and_cell_params() {
        // #597 - the cell holds params only; they append to the agent base command,
        // so base `codex` + cell `--profile fast` launches `codex --profile fast`.
        let settings = settings_with_cell("codex", "B", cell("--profile fast", BTreeMap::new()));

        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("B")).unwrap();

        assert_eq!(spawn.shell, "codex");
        assert_eq!(spawn.shell_args, vec!["--profile", "fast"]);
        assert_eq!(spawn.profile_resolution.effective_profile, "B");
    }

    #[test]
    fn pi_model_overlap_never_generates_codex_home() {
        for (id, command) in [
            ("pi", "pi --model codex-model"),
            ("pi-cmd", "cmd /C pi --model codex-model"),
        ] {
            let mut pi = agent(id, command);
            pi.isolated_home = true;
            let settings = AppSettings {
                agents: vec![pi],
                ..AppSettings::default()
            };

            let spawn = build_agent_spawn_command(&settings, id, None, Some("A")).unwrap();

            assert!(!spawn.generated_env.contains_key("CODEX_HOME"));
            assert!(spawn.effective_codex_home.is_none());
            assert!(!spawn
                .env_remove_keys
                .iter()
                .any(|key| key.eq_ignore_ascii_case("CODEX_HOME")));
        }
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
        // #597 - under concatenation the binary (placeholder included) lives in the
        // agent base command and the cell holds the param. The placeholder must
        // still expand on the composed token list so the launched shell is the
        // expanded binary path.
        let mut settings = AppSettings {
            agents: vec![agent("codex", "%AC_REPLICA_ROOT%\\bin\\codex.exe")],
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
                    command: "--flag".to_string(),
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
            expected_root
                .join("bin")
                .join("codex.exe")
                .to_string_lossy()
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
        let expected_ac_root = expected_replica
            .parent()
            .and_then(|parent| parent.parent())
            .expect("replica has a .ac workspace ancestor");
        let expected_matrix = expected_ac_root.join("_agent_dev-rust");

        let expected_config = format!("{}\\.claude", expected_ac_root.to_string_lossy());
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
            default_instructions_filename_for_command("agy -m gpt-5"),
            "AGENTS.md"
        );
        assert_eq!(
            default_instructions_filename_for_command("codex"),
            "AGENTS.md"
        );
        assert_eq!(
            default_instructions_filename_for_command("pi --model claude-sonnet"),
            "AGENTS.md"
        );
        assert_eq!(
            default_instructions_filename_for_command("pi --model codex-model"),
            "AGENTS.md"
        );
        assert_eq!(
            default_instructions_filename_for_command("pi --provider gemini"),
            "AGENTS.md"
        );
        assert_eq!(
            default_instructions_filename_for_command("cmd /C pi --model claude-sonnet"),
            "AGENTS.md"
        );
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
                agent("agy", "agy"),           // AGENTS.md
                agent("opencode", "opencode"), // AGENTS.md (dup of codex)
            ],
            ..AppSettings::default()
        };
        let mut got = managed_instructions_filenames(&settings);
        got.sort();
        assert_eq!(got, vec!["AGENTS.md".to_string(), "CLAUDE.md".to_string()]);
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
            resolve_target_filename(
                Some("ghost"),
                &settings,
                Some(ManagedContextTarget::Antigravity)
            )
            .as_deref(),
            Some("AGENTS.md")
        );
        // 3b. no id -> detection fallback.
        assert_eq!(
            resolve_target_filename(None, &settings, Some(ManagedContextTarget::Codex)).as_deref(),
            Some("AGENTS.md")
        );
        // 4. neither configured agent nor detection -> None.
        assert_eq!(resolve_target_filename(None, &settings, None), None);
        assert_eq!(
            resolve_target_filename(Some("ghost"), &settings, None),
            None
        );
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
            "",               // empty
            "   ",            // whitespace only
            ".md",            // empty stem
            "AGENTS.txt",     // wrong extension
            "AGENTS",         // no extension
            "a/b.md",         // forward separator
            "a\\b.md",        // backslash separator
            "..\\x.md",       // traversal + separator
            "../x.md",        // traversal
            "a..md",          // contains ..
            "C:x.md",         // drive prefix (colon)
            "AGENTS.md:evil", // NTFS Alternate Data Stream (colon)
            "AGENTS.md ",     // trailing space
            "AGENTS.md.",     // trailing dot
            "AGENTS .md",     // space immediately before extension
            "a\nb.md",        // control char
            "CON.md",         // reserved device
            "con.md",         // reserved device (case-insensitive)
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
            assert!(
                !is_safe_instructions_filename(name),
                "should reject {name:?}"
            );
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
        // Allowlisted executable forms still match as args (path + npm-style shim).
        assert!(command_runs_opencode(
            "cmd.exe",
            &["/c".to_string(), r"C:\tools\opencode.cmd".to_string()]
        ));
        assert!(command_runs_opencode(
            "cmd.exe",
            &["/c opencode.bat".to_string()]
        ));
        // Non-opencode commands do not match.
        assert!(!command_runs_opencode("codex", &[]));
        assert!(!command_runs_opencode("claude", &["--resume".to_string()]));
    }

    #[test]
    fn command_runs_opencode_ignores_opencode_doc_and_config_args() {
        // Regression (grinch Finding 1): a non-opencode agent whose command merely
        // references an `opencode.md`/`.json` file must NOT be read as an opencode
        // launch. `file_stem("opencode.md") == "opencode"`, so the prior arg-scan
        // false-matched; the file_name allowlist rejects these forms.
        assert!(!command_runs_opencode(
            "claude",
            &["--instructions".to_string(), "opencode.md".to_string()]
        ));
        assert!(!command_runs_opencode(
            "claude",
            &[r"C:\Users\me\opencode.md".to_string()]
        ));
        assert!(!command_runs_opencode(
            "claude",
            &["opencode.json".to_string()]
        ));
        assert!(!command_runs_opencode(
            "claude",
            &["opencode.txt".to_string()]
        ));
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
        assert!(
            !target.exists(),
            "must not create the dir for a non-opencode command"
        );
    }

    #[test]
    fn ensure_opencode_config_dir_skips_opencode_md_arg_with_absolute_value() {
        // Regression (grinch Finding 1): a non-opencode launch (here `claude`)
        // whose args reference an `opencode.md` file, even with an absolute
        // OPENCODE_CONFIG_DIR set, must NOT be read as an opencode launch, so the
        // dir is never created.
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("nested").join(".opencode");
        assert!(!target.exists());
        let profile_env = opencode_env(&target.to_string_lossy());

        let outcome = ensure_opencode_config_dir(
            "claude",
            &["--instructions".to_string(), "opencode.md".to_string()],
            &BTreeMap::new(),
            &profile_env,
        );

        assert_eq!(outcome, OpencodeConfigDirOutcome::Skipped);
        assert!(
            !target.exists(),
            "an opencode.md arg must not trigger OPENCODE_CONFIG_DIR creation"
        );
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
    fn resolve_is_read_only_and_prepare_is_idempotent_for_opencode() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("resolved").join(".opencode");
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

        let spawn =
            resolve_agent_spawn_command(&settings, "opencode", None, Some("A"), false).unwrap();
        assert!(
            !target.exists(),
            "read-only resolution must not create the directory"
        );

        prepare_agent_spawn_command(&spawn).unwrap();
        assert!(target.is_dir());
        prepare_agent_spawn_command(&spawn).unwrap();
        assert!(target.is_dir(), "preparation must be idempotent");
    }

    #[test]
    fn resolve_is_read_only_and_public_build_prepares_isolated_codex_home() {
        let id = format!("codex-resolve-{}", uuid::Uuid::new_v4());
        let mut codex = agent(&id, "codex");
        codex.isolated_home = true;
        let settings = AppSettings {
            agents: vec![codex],
            ..AppSettings::default()
        };
        let expected = crate::config::config_dir()
            .unwrap()
            .join(crate::config::instance_artifacts::CODEX_HOME_DIR_NAME)
            .join(super::sanitize_codex_home_id(&id));
        let _ = std::fs::remove_dir_all(&expected);

        let resolved = resolve_agent_spawn_command(&settings, &id, None, Some("A"), false).unwrap();
        assert_eq!(
            resolved.effective_codex_home.as_deref(),
            Some(expected.as_path())
        );
        assert!(
            !expected.exists(),
            "read-only resolution must not create CODEX_HOME"
        );

        prepare_agent_spawn_command(&resolved).unwrap();
        assert!(expected.is_dir());
        prepare_agent_spawn_command(&resolved).unwrap();
        assert!(expected.is_dir(), "preparation must be idempotent");
        std::fs::remove_dir_all(&expected).unwrap();

        let built = build_agent_spawn_command(&settings, &id, None, Some("A")).unwrap();
        assert_eq!(
            built.effective_codex_home.as_deref(),
            Some(expected.as_path())
        );
        assert!(
            expected.is_dir(),
            "public build must retain preparation behavior"
        );
        std::fs::remove_dir_all(expected).unwrap();
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

        assert!(
            target.is_dir(),
            "build should have created OPENCODE_CONFIG_DIR"
        );
        let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();
        assert_eq!(
            env.get("OPENCODE_CONFIG_DIR").map(String::as_str),
            Some(target.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn build_spawn_expands_and_creates_opencode_config_dir_for_replica_placeholder() {
        // Finding 2 (grinch): the literal-value test does not cover the real user
        // scenario, where OPENCODE_CONFIG_DIR holds %AC_REPLICA_ROOT%\.opencode and
        // is expanded against the replica launch root before the dir is created.
        // This drives that path end-to-end: expand -> absolute -> create.
        let temp = tempfile::tempdir().unwrap();
        let replica = temp
            .path()
            .join("root with spaces")
            .join(".ac")
            .join("wg-7-dev-team")
            .join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();

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
                    env: opencode_env(r"%AC_REPLICA_ROOT%\.opencode"),
                    notes: String::new(),
                },
            );

        let spawn = build_agent_spawn_command(&settings, "opencode", Some(&replica), Some("A"))
            .expect("replica launch should expand %AC_REPLICA_ROOT% and build the spawn");

        // Mirror the builder's canonicalize + verbatim-prefix strip, then append the
        // literal "\.opencode" the way placeholder expansion concatenates it (see the
        // matrix CODEX_HOME test for why the whole-path comparison stays cross-platform).
        let canonical_root = std::fs::canonicalize(&replica).unwrap();
        let canonical_text = canonical_root.to_string_lossy();
        let expected_replica = canonical_text
            .strip_prefix(r"\\?\")
            .map(std::path::PathBuf::from)
            .unwrap_or(canonical_root);
        let expected_config = format!("{}\\.opencode", expected_replica.to_string_lossy());

        // The child env carries the fully-expanded absolute path...
        let env: BTreeMap<_, _> = spawn.child_env.into_iter().collect();
        assert_eq!(
            env.get("OPENCODE_CONFIG_DIR").map(String::as_str),
            Some(expected_config.as_str())
        );
        // ...and the build created that exact directory end-to-end.
        assert!(
            std::path::Path::new(&expected_config).is_dir(),
            "build should have created the expanded OPENCODE_CONFIG_DIR at {expected_config}"
        );
    }

    // #598 - config-folder seed resolution wired into build_agent_spawn_command.

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

    #[test]
    fn build_spawn_resolves_active_seed_for_replica_without_touching_fs() {
        let temp = tempfile::tempdir().unwrap();
        let replica = seed_replica(temp.path());

        let mut claude = agent("claude", "claude");
        claude.config_seed = Some(ConfigSeedConfig {
            enabled: true,
            dest: ".claude".to_string(),
        });
        let settings = AppSettings {
            agents: vec![claude],
            ..AppSettings::default()
        };

        let spawn =
            build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
        let seed = spawn.seed.expect("active seed resolves to Some");

        let expected_replica = canonical_replica(&replica);
        let ac_root = expected_replica.parent().unwrap().parent().unwrap();
        let matrix = ac_root.join("_agent_dev-rust");
        let letter = spawn
            .profile_resolution
            .effective_profile
            .to_ascii_lowercase();

        use crate::config::config_seed::ConfigSeedTier;
        // Workspace tiers outrank matrix tiers; profile beats base in each. The
        // matrix's bare `.claude` (the agent's own live config) is never a source.
        // #769 P2 + #1318 appends the absent-only CatalogDefault master LAST
        // (still pure path math; perform_config_seed gates it on absent-dest +
        // non-empty), resolved from the SESSION WORKSPACE's
        // `.ac/coding-agents/_seed/<dest>`.
        let mut expected = vec![
            (
                ConfigSeedTier::WorkspaceProfile,
                ac_root.join(format!("default_profile_{}.claude", letter)),
            ),
            (
                ConfigSeedTier::WorkspaceBase,
                ac_root.join("default.claude"),
            ),
            (
                ConfigSeedTier::MatrixProfile,
                matrix.join(format!("default_profile_{}.claude", letter)),
            ),
            (ConfigSeedTier::MatrixBase, matrix.join("default.claude")),
        ];
        expected.push((
            ConfigSeedTier::CatalogDefault,
            ac_root.join("coding-agents").join("_seed").join(".claude"),
        ));
        assert_eq!(seed.candidates, expected);
        assert_eq!(seed.dest, expected_replica.join(".claude"));
        // Pure resolution: no template dirs were created.
        assert!(!ac_root.join("default.claude").exists());
    }

    #[test]
    fn build_spawn_catalog_default_absent_without_ac_root() {
        // A replica-shaped launch root with NO `.ac` workspace ancestor (the
        // only shape that could reach the fill's config-dir legacy fallback).
        // `resolve_config_seed` derives its candidates exclusively from the
        // workspace and matrix roots, both of which require a workspace, so the
        // seed does not resolve at all and no CatalogDefault tier is produced:
        // the `.or_else(config_dir)` legacy fallback in the fill is structurally
        // unreachable for a resolved seed and stays defense-in-depth for
        // pre-migration shapes.
        let temp = tempfile::tempdir().unwrap();
        let replica = temp.path().join("wg-1-dev-team").join("__agent_legacy");
        std::fs::create_dir_all(&replica).unwrap();

        let mut claude = agent("claude", "claude");
        claude.config_seed = Some(ConfigSeedConfig {
            enabled: true,
            dest: ".claude".to_string(),
        });
        let settings = AppSettings {
            agents: vec![claude],
            ..AppSettings::default()
        };

        let spawn =
            build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
        assert!(
            spawn.seed.is_none(),
            "a workspace-less replica resolves no seed and no CatalogDefault tier"
        );
    }

    #[test]
    fn build_spawn_computes_seed_config_dir_warning() {
        // A Claude agent with no CLAUDE_CONFIG_DIR gets the "not configured"
        // warning, computed at build time from the real launch command.
        let temp = tempfile::tempdir().unwrap();
        let replica = seed_replica(temp.path());

        let mut claude = agent("claude", "claude");
        claude.config_seed = Some(ConfigSeedConfig {
            enabled: true,
            dest: ".claude".to_string(),
        });
        let settings = AppSettings {
            agents: vec![claude],
            ..AppSettings::default()
        };

        let spawn =
            build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
        let warning = spawn
            .seed
            .expect("seed")
            .config_dir_warning
            .expect("warning computed for the launch command");
        assert!(warning.contains("CLAUDE_CONFIG_DIR"), "{warning}");
    }

    #[test]
    fn build_spawn_seed_none_for_normal_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo-thing");
        std::fs::create_dir_all(&repo).unwrap();

        let mut claude = agent("claude", "claude");
        claude.config_seed = Some(ConfigSeedConfig {
            enabled: true,
            dest: ".claude".to_string(),
        });
        let settings = AppSettings {
            agents: vec![claude],
            ..AppSettings::default()
        };

        let spawn = build_agent_spawn_command(&settings, "claude", Some(&repo), Some("A")).unwrap();
        assert!(
            spawn.seed.is_none(),
            "a normal (non-replica) cwd must not seed"
        );
    }

    #[test]
    fn build_spawn_seed_reapply_none_for_non_claude_dest() {
        let temp = tempfile::tempdir().unwrap();
        let replica = seed_replica(temp.path());

        let mut claude = agent("claude", "claude");
        claude.config_seed = Some(ConfigSeedConfig {
            enabled: true,
            dest: ".claude-amp".to_string(),
        });
        let settings = AppSettings {
            agents: vec![claude],
            ..AppSettings::default()
        };

        let spawn =
            build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
        let seed = spawn.seed.expect("seed");
        assert_eq!(seed.dest.file_name().unwrap(), ".claude-amp");
    }

    #[test]
    fn build_spawn_seed_none_when_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let replica = seed_replica(temp.path());

        let mut claude = agent("claude", "claude");
        claude.config_seed = Some(ConfigSeedConfig {
            enabled: false,
            dest: ".claude".to_string(),
        });
        let settings = AppSettings {
            agents: vec![claude],
            ..AppSettings::default()
        };

        let spawn =
            build_agent_spawn_command(&settings, "claude", Some(&replica), Some("A")).unwrap();
        assert!(spawn.seed.is_none(), "disabled seed must resolve to None");
    }

    // #592 - profile content-hash (drift detector) tests.

    fn cell(command: &str, env: BTreeMap<String, String>) -> ProfileCellConfig {
        ProfileCellConfig {
            enabled: true,
            command: command.to_string(),
            env,
            notes: String::new(),
        }
    }

    fn settings_with_cell(
        agent_command: &str,
        letter: &str,
        cell: ProfileCellConfig,
    ) -> AppSettings {
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

    #[test]
    fn profile_content_hash_is_deterministic_and_16_lowercase_hex() {
        let mut env = BTreeMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let a = profile_content_hash("claude --x", &env);
        let b = profile_content_hash("claude --x", &env);
        assert_eq!(a, b, "same inputs must hash identically");
        assert_eq!(a.len(), 16, "hash must be 16 chars");
        assert!(
            a.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "hash must be lowercase hex: {a}"
        );
    }

    #[test]
    fn profile_content_hash_is_raw_not_expanded() {
        let env = BTreeMap::new();
        let raw = profile_content_hash("%AC_REPLICA_ROOT%\\bin\\claude", &env);
        let expanded = profile_content_hash("C:\\replica\\bin\\claude", &env);
        assert_ne!(
            raw, expanded,
            "the function must hash the raw placeholder text, never an expansion"
        );
    }

    #[test]
    fn profile_content_hash_changes_on_agent_base_command_edit() {
        // #597 - the hash now fingerprints the EFFECTIVE command (base + cell), so
        // an edit to the agent base command flips it even with the cell unchanged.
        let s1 = settings_with_cell("codex --one", "A", cell("--p", BTreeMap::new()));
        let s2 = settings_with_cell("codex --two", "A", cell("--p", BTreeMap::new()));
        let h1 = build_agent_spawn_command(&s1, "codex", None, Some("A"))
            .unwrap()
            .profile_content_hash;
        let h2 = build_agent_spawn_command(&s2, "codex", None, Some("A"))
            .unwrap()
            .profile_content_hash;
        assert_ne!(
            h1, h2,
            "an agent-base edit must flip the effective-command hash"
        );
    }

    #[test]
    fn profile_content_hash_flips_on_cell_command_edit() {
        let s1 = settings_with_cell("codex", "A", cell("claude", BTreeMap::new()));
        let s2 = settings_with_cell("codex", "A", cell("claude --foo", BTreeMap::new()));
        let h1 = build_agent_spawn_command(&s1, "codex", None, Some("A"))
            .unwrap()
            .profile_content_hash;
        let h2 = build_agent_spawn_command(&s2, "codex", None, Some("A"))
            .unwrap()
            .profile_content_hash;
        assert_ne!(h1, h2, "a cell command edit must flip the hash (drift)");
    }

    #[test]
    fn profile_content_hash_uses_effective_cell_after_fallback() {
        let mut settings = AppSettings {
            agents: vec![agent("codex", "codex")],
            ..AppSettings::default()
        };
        {
            let cells = settings
                .coding_agent_profiles
                .profiles_by_agent
                .entry("codex".to_string())
                .or_default();
            cells.insert("A".to_string(), cell("codex --a", BTreeMap::new()));
            cells.insert("C".to_string(), cell("codex --c", BTreeMap::new()));
        }

        // Request D with only A/C present -> falls back to C.
        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("D")).unwrap();
        assert_eq!(spawn.profile_resolution.effective_profile, "C");
        assert_eq!(spawn.profile_resolution.cell.command, "codex --c");
        // The stamped hash is the EFFECTIVE (post-fallback C) cell composed with
        // the agent base command, not D. (#597)
        let agent_cfg = settings.agents.iter().find(|a| a.id == "codex").unwrap();
        let expected = profile_content_hash(
            &super::compose_effective_command(
                &agent_cfg.command,
                &spawn.profile_resolution.cell.command,
            ),
            &super::raw_merged_profile_env(agent_cfg, &spawn.profile_resolution.cell.env),
        );
        assert_eq!(spawn.profile_content_hash, expected);
    }

    #[test]
    fn compose_effective_command_joins_and_drops_empty_sides() {
        assert_eq!(
            super::compose_effective_command("claude", "--x"),
            "claude --x"
        );
        assert_eq!(super::compose_effective_command("claude", ""), "claude");
        assert_eq!(super::compose_effective_command("", "--x"), "--x");
        assert_eq!(super::compose_effective_command("", ""), "");
        // Each side is trimmed; no double space at the seam.
        assert_eq!(
            super::compose_effective_command("  claude  ", "  --x  "),
            "claude --x"
        );
    }

    #[test]
    fn build_spawn_empty_cell_uses_base_command_only() {
        let settings = settings_with_cell("codex --yolo", "A", cell("", BTreeMap::new()));
        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();
        assert_eq!(spawn.shell, "codex");
        assert_eq!(spawn.shell_args, vec!["--yolo"]);
    }

    #[test]
    fn build_spawn_concatenation_handles_base_with_args_and_quoted_params() {
        // The whole `<base> <cell>` line tokenizes once; quotes/embedded spaces survive.
        let settings = settings_with_cell(
            "cmd.exe /c claude",
            "A",
            cell("--model \"gpt 5\"", BTreeMap::new()),
        );
        let spawn = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap();
        assert_eq!(spawn.shell, "cmd.exe");
        assert_eq!(spawn.shell_args, vec!["/c", "claude", "--model", "gpt 5"]);
    }

    #[test]
    fn build_spawn_errors_when_base_and_cell_both_empty() {
        let settings = settings_with_cell("", "A", cell("", BTreeMap::new()));
        let err = build_agent_spawn_command(&settings, "codex", None, Some("A")).unwrap_err();
        assert!(
            err.contains("agent command is empty"),
            "both-empty must report an empty command: {err}"
        );
    }

    #[test]
    fn profile_content_hash_changes_on_agent_base_env_edit() {
        // The base env is now in the hash; a value edit on an enabled row flips it.
        // LOWERCASE key so the Windows case-fold in raw_merged_profile_env does not
        // skew the assertion.
        fn settings_with_env_value(value: &str) -> AppSettings {
            let mut ag = agent("codex", "codex");
            ag.envs = vec![CodingAgentEnv {
                key: "kk".to_string(),
                value: value.to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            }];
            let mut settings = AppSettings {
                agents: vec![ag],
                ..AppSettings::default()
            };
            settings
                .coding_agent_profiles
                .profiles_by_agent
                .entry("codex".to_string())
                .or_default()
                .insert("A".to_string(), cell("", BTreeMap::new()));
            settings
        }
        let h1 =
            build_agent_spawn_command(&settings_with_env_value("one"), "codex", None, Some("A"))
                .unwrap()
                .profile_content_hash;
        let h2 =
            build_agent_spawn_command(&settings_with_env_value("two"), "codex", None, Some("A"))
                .unwrap()
                .profile_content_hash;
        assert_ne!(h1, h2, "an agent base env edit must flip the hash");
    }

    #[test]
    fn raw_merged_profile_env_overlays_cell_over_agent_raw() {
        let mut ag = agent("codex", "codex");
        ag.envs = vec![
            CodingAgentEnv {
                key: "ka".to_string(),
                value: "agent".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "kb".to_string(),
                value: "agent".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "koff".to_string(),
                value: "agent".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: false,
            },
        ];
        let cell_env = BTreeMap::from([
            ("kb".to_string(), "cell".to_string()),
            ("kc".to_string(), "cell".to_string()),
        ]);
        let merged = super::raw_merged_profile_env(&ag, &cell_env);
        // Keys come back platform-normalized (uppercased on Windows), so look them
        // up through the same normalizer to stay cross-platform.
        let key = |k: &str| crate::config::settings::normalize_env_key_for_platform(k);
        assert_eq!(merged.get(&key("ka")).map(String::as_str), Some("agent"));
        assert_eq!(merged.get(&key("kb")).map(String::as_str), Some("cell")); // profile wins
        assert_eq!(merged.get(&key("kc")).map(String::as_str), Some("cell"));
        assert!(
            !merged.contains_key(&key("koff")),
            "disabled rows are excluded"
        );
    }

    #[cfg(windows)]
    #[test]
    fn profile_content_hash_normalizes_env_keys_case_insensitively_on_windows() {
        let mut lower = BTreeMap::new();
        lower.insert("Path".to_string(), "x".to_string());
        let mut upper = BTreeMap::new();
        upper.insert("PATH".to_string(), "x".to_string());
        assert_eq!(
            profile_content_hash("claude", &lower),
            profile_content_hash("claude", &upper),
            "a case-only env-key edit must not false-flag drift on Windows"
        );
    }

    // #1551 - the shared program resolver lifted out of `commands::session`.

    #[test]
    fn resolve_program_explicit_path_requires_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("tool.exe");
        std::fs::write(&file, b"x").expect("write");
        assert_eq!(
            resolve_program(&file.to_string_lossy()),
            Some(file.clone()),
            "an explicit path to a file resolves to itself"
        );
        let missing = dir.path().join("missing.exe");
        assert_eq!(
            resolve_program(&missing.to_string_lossy()),
            None,
            "an explicit path that does not exist never falls back to PATH"
        );
        assert_eq!(
            resolve_program(&dir.path().to_string_lossy()),
            None,
            "a directory is not a program"
        );
    }

    #[test]
    fn resolve_program_bare_name_uses_path() {
        let token = if cfg!(windows) { "cmd" } else { "sh" };
        let resolved = resolve_program(token).expect("a bare shell name resolves through PATH");
        let stem = resolved
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("file stem")
            .to_ascii_lowercase();
        assert_eq!(stem, token);
    }

    #[test]
    fn resolve_program_bare_unknown_is_none() {
        assert_eq!(resolve_program("definitely-not-a-program-1551"), None);
    }

    #[test]
    fn is_bare_program_token_cases() {
        assert!(is_bare_program_token("claude"));
        assert!(!is_bare_program_token("./claude"));
        assert!(!is_bare_program_token(r"C:\x\claude.exe"));
        assert!(!is_bare_program_token("/usr/bin/claude"));
        assert!(!is_bare_program_token(r"bin\claude"));
    }
}
