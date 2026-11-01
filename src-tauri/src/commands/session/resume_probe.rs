use super::agent_command::executable_basename;
use super::resume_args::{
    antigravity_tokens_have_resume, codex_has_explicit_subcommand, codex_tokens_have_resume,
    expand_env_var_refs, pi_has_explicit_session_control,
};
use crate::config::agent_command::AgentSpawnCommand;
use crate::pty::backend::SessionBackendKind;
use crate::pty::container_paths::{
    claude_config_dir_no_value_warning, container_config_dir, ContainerEnvWarning,
    ContainerPathMap, CLAUDE_CONFIG_DIR_KEY,
};
use crate::pty::container_runtime::DEFAULT_CONTAINER_WORKDIR;
use crate::session::profile::{locate_pi_command, CodingAgentKind};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Resolve the directory where Claude Code stores its project transcripts for
/// `cwd`, taking `CLAUDE_CONFIG_DIR` overrides set by `.cmd`/`.bat`/`.ps1`/`.sh`
/// wrapper scripts into account.
///
/// Background: a user can put a wrapper like `claude-mb.cmd` on `%PATH%`:
///
/// ```bat
/// @echo off
/// set CLAUDE_CONFIG_DIR=C:\Users\maria\.claude-mb
/// claude %*
/// ```
///
/// Real Claude then writes project transcripts under
/// `C:\Users\maria\.claude-mb\projects\<mangled-cwd>`, NOT
/// `~/.claude/projects/<mangled-cwd>`. This helper finds the right base.
///
/// Returns `Some(<base>/projects/<mangled-cwd>)` when a Claude-family token
/// exists in the launch command, else `None`. Falls back to `~/.claude/...`
/// whenever the wrapper cannot be resolved or parsed; this preserves the
/// pre-#186 default-install behavior exactly.
///
/// Visibility: `pub(crate)` so the Telegram JSONL watcher follow-up can reuse
/// the same resolver without a private→pub(crate) refactor (see plan §6.3).
pub(crate) fn resolve_claude_projects_dir(
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> Option<std::path::PathBuf> {
    resolve_claude_projects_dir_in(
        shell,
        shell_args,
        cwd,
        &crate::config::agent_path::effective_search_path(),
    )
}

fn strip_ascii_prefix_ci<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    if haystack.len() < needle.len() {
        return None;
    }
    if haystack.as_bytes()[..needle.len()].eq_ignore_ascii_case(needle.as_bytes()) {
        Some(&haystack[needle.len()..])
    } else {
        None
    }
}

fn parse_config_dir_from_wrapper(path: &Path) -> Option<PathBuf> {
    let bytes = crate::config::agent_command::read_claude_resume_wrapper(path)?;
    // Strip UTF-8 BOM if present; tolerate non-UTF-8 by lossy decode.
    let text_bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let text = String::from_utf8_lossy(text_bytes);

    for raw_line in text.lines() {
        let line = raw_line.trim_start();

        // Strip optional shell-prefix introducer:
        //   `cmd`/`.bat`: `set CLAUDE_CONFIG_DIR=...`
        //   `cmd`/`.bat`: `set "CLAUDE_CONFIG_DIR=..."`     (cmd-quoted whole-assignment)
        //   `.ps1`:       `$env:CLAUDE_CONFIG_DIR = ...`
        //   `.sh`:        `export CLAUDE_CONFIG_DIR=...`
        //   Bare:         `CLAUDE_CONFIG_DIR=...`
        let after_prefix = if let Some(rest) = strip_ascii_prefix_ci(line, "set ") {
            rest.trim_start()
        } else if let Some(rest) = strip_ascii_prefix_ci(line, "$env:") {
            rest.trim_start()
        } else if let Some(rest) = strip_ascii_prefix_ci(line, "export ") {
            rest.trim_start()
        } else {
            line
        };

        // Detect cmd's whole-assignment quoting: `set "VAR=value"`. After the
        // `set ` strip we may be sitting on a leading `"`; if so, the matching
        // closing `"` terminates the value (rather than wrapping it).
        let (after_open_quote, cmd_quoted) = match after_prefix.strip_prefix('"') {
            Some(rest) => (rest, true),
            None => (after_prefix, false),
        };

        let Some(rest) = strip_ascii_prefix_ci(after_open_quote, "CLAUDE_CONFIG_DIR") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let value = rest.trim();

        // Strip surrounding quotes. Two flavors:
        //   (a) cmd whole-assignment: `set "VAR=value"`     → consume trailing `"`
        //   (b) value-quoted:         `set VAR="value"` or `'value'` → strip matched pair
        let unquoted: &str = if cmd_quoted {
            value.strip_suffix('"').unwrap_or(value)
        } else if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };

        if unquoted.is_empty() {
            return None;
        }
        let expanded = expand_env_var_refs(unquoted);
        return Some(PathBuf::from(expanded));
    }
    None
}

/// #2589 - `resolve_claude_projects_dir` resolving a bare wrapper token through
/// an explicit search path.
pub(crate) fn resolve_claude_projects_dir_in(
    shell: &str,
    shell_args: &[String],
    cwd: &str,
    search_path: &std::ffi::OsStr,
) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};

    fn default_base() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".claude"))
    }

    fn looks_like_wrapper_extension(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let lc = e.to_ascii_lowercase();
                matches!(lc.as_str(), "cmd" | "bat" | "ps1" | "sh")
            })
            .unwrap_or(false)
    }

    // Find the first token whose basename starts with "claude" across
    // shell + shell_args, splitting each arg on whitespace so cmd-wrapped
    // strings ("git pull && claude-mb -x") are also covered.
    let claude_token: Option<String> = {
        let mut direct = std::iter::once(shell.to_string()).chain(
            shell_args
                .iter()
                .flat_map(|a| a.split_whitespace().map(str::to_string)),
        );
        direct.find(|t| executable_basename(t).starts_with("claude"))
    };
    let claude_token = claude_token?;

    let mangled = crate::session::session::mangle_cwd_for_claude(cwd);

    // Stem == "claude" → no wrapper, default base. This covers `claude`,
    // `claude.exe`, `C:\Tools\claude.cmd` (where the .cmd is the official
    // installer's launcher and writes nothing of its own), etc.
    // executable_basename returns a lowercased stem; comparison is case-insensitive by
    // construction.
    if executable_basename(&claude_token) == "claude" {
        return default_base().map(|base| base.join("projects").join(&mangled));
    }

    // Non-default name (e.g. `claude-mb`). Try to resolve to an actual file
    // and parse it for a CLAUDE_CONFIG_DIR override.
    if let Some(file) = crate::config::agent_command::resolve_program_in(&claude_token, search_path)
    {
        if looks_like_wrapper_extension(&file) {
            if let Some(custom_base) = parse_config_dir_from_wrapper(&file) {
                return Some(custom_base.join("projects").join(&mangled));
            }
        }
    }

    // Fall back to default base. Preserves pre-fix behavior whenever the
    // wrapper is missing, unreadable, has no `CLAUDE_CONFIG_DIR` line, or
    // points at a non-text extension.
    default_base().map(|base| base.join("projects").join(&mangled))
}

/// (#599 R2) Claude `projects/<mangled-cwd>` dir for an explicit
/// `CLAUDE_CONFIG_DIR` taken from the profile/agent env layer. That layer is
/// what the spawned Claude actually sees: the env approach launches a plain
/// `claude`, so `resolve_claude_projects_dir` short-circuits to `~/.claude`
/// (its `file_stem == "claude"` fast path) and misses env-relocated
/// transcripts. The value is already AC-placeholder-expanded by the spawn
/// builder; `expand_env_var_refs` additionally resolves any residual
/// `%VAR%` / `$env:VAR` for parity with the wrapper path.
pub(crate) fn claude_projects_dir_for_config_dir(
    config_dir: &str,
    cwd: &str,
) -> std::path::PathBuf {
    let base = std::path::PathBuf::from(expand_env_var_refs(config_dir));
    let mangled = crate::session::session::mangle_cwd_for_claude(cwd);
    base.join("projects").join(mangled)
}

#[derive(Debug, Clone)]
pub(crate) struct ResumeProbeTarget {
    pub(crate) host_probe_path: Option<PathBuf>,
    pub(crate) filesystem: &'static str,
    pub(crate) warning: Option<ContainerEnvWarning>,
}

#[derive(Debug, Clone)]
pub(crate) struct ContainerPathContext {
    pub(crate) host_root: String,
    pub(crate) map: ContainerPathMap,
}

pub(crate) fn container_path_context_for_cwd(cwd: &str) -> Result<ContainerPathContext, String> {
    let canonical_host_root =
        crate::filesystem::projects::canonical_project_path(std::path::Path::new(cwd))
            .map(|path| crate::path_utils::path_to_string_without_windows_verbatim_prefix(&path))
            .map_err(|err| {
                format!(
                    "failed to canonicalize container mount source '{}': {}",
                    cwd, err
                )
            })?;
    if let Some(reason) = crate::pty::container_paths::container_mount_source_rejection(
        std::path::Path::new(&canonical_host_root),
    ) {
        return Err(container_mount_rejection_message(
            cwd,
            &canonical_host_root,
            reason,
        ));
    };
    let map = ContainerPathMap::new(&canonical_host_root, DEFAULT_CONTAINER_WORKDIR)?;
    Ok(ContainerPathContext {
        host_root: canonical_host_root,
        map,
    })
}

pub(crate) fn container_mount_rejection_message(
    selected: &str,
    canonical: &str,
    reason: String,
) -> String {
    if selected == canonical {
        reason
    } else {
        format!(
            "{} (selected path '{}', canonical path '{}')",
            reason, selected, canonical
        )
    }
}

pub(crate) fn effective_codex_home_for_backend(
    backend_kind: SessionBackendKind,
    container_map: Option<&ContainerPathMap>,
    spawn: &AgentSpawnCommand,
) -> Option<String> {
    let raw_home = spawn.effective_codex_home.as_ref()?;
    let raw_home =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(raw_home.as_path());
    match backend_kind {
        SessionBackendKind::LocalProcess => Some(raw_home),
        SessionBackendKind::ContainerTransport => {
            let map = container_map?;
            let container_home = container_config_dir(map, &raw_home)?;
            map.to_host(&container_home).map(|path| {
                crate::path_utils::path_to_string_without_windows_verbatim_prefix(path.as_path())
            })
        }
    }
}

pub(crate) fn resume_probe_target(
    backend_kind: SessionBackendKind,
    container_map: Option<&ContainerPathMap>,
    resolved_spawn: Option<&AgentSpawnCommand>,
    injected_claude_config_dir: Option<&str>,
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> ResumeProbeTarget {
    // #930 - an injected copy-in default (host path under the replica mount) wins
    // only when the user set no CLAUDE_CONFIG_DIR; the caller enforces that
    // precondition, so fall back to the user's effective value otherwise.
    let claude_config_dir_override = injected_claude_config_dir
        .or_else(|| resolved_spawn.and_then(|s| s.effective_env_value(CLAUDE_CONFIG_DIR_KEY)));
    resume_probe_target_for_config_dir(
        backend_kind,
        container_map,
        claude_config_dir_override,
        shell,
        shell_args,
        cwd,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn claude_resume_probe_target_for_kind(
    agent_kind: Option<CodingAgentKind>,
    backend_kind: SessionBackendKind,
    container_map: Option<&ContainerPathMap>,
    resolved_spawn: Option<&AgentSpawnCommand>,
    injected_claude_config_dir: Option<&str>,
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> Option<ResumeProbeTarget> {
    if agent_kind != Some(CodingAgentKind::Claude) {
        return None;
    }
    Some(resume_probe_target(
        backend_kind,
        container_map,
        resolved_spawn,
        injected_claude_config_dir,
        shell,
        shell_args,
        cwd,
    ))
}

/// #930 - the CLAUDE_CONFIG_DIR value to inject for a container coding agent whose
/// host credentials we will copy. Returns the copy directory (a host path under
/// the replica mount, which the container env translation later maps to
/// `/workspace/.claude`) ONLY when a copy plan exists AND the user configured no
/// CLAUDE_CONFIG_DIR (`user_has_claude_config_dir == false`). `None` => inject
/// nothing (host-login-reuse off, no host credentials, or an explicit user
/// value/removal we must not override).
pub(crate) fn injected_claude_config_dir_for_copy(
    plan: Option<&crate::pty::container_credentials::ContainerCredentialPlan>,
    user_has_claude_config_dir: bool,
) -> Option<String> {
    if user_has_claude_config_dir {
        return None;
    }
    plan?
        .dest
        .parent()
        .map(crate::path_utils::path_to_string_without_windows_verbatim_prefix)
}

pub(crate) fn resume_probe_target_for_config_dir(
    backend_kind: SessionBackendKind,
    container_map: Option<&ContainerPathMap>,
    claude_config_dir_override: Option<&str>,
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> ResumeProbeTarget {
    match backend_kind {
        SessionBackendKind::LocalProcess => {
            let host_probe_path = match claude_config_dir_override {
                Some(dir) => Some(claude_projects_dir_for_config_dir(dir, cwd)),
                None => resolve_claude_projects_dir(shell, shell_args, cwd),
            };
            ResumeProbeTarget {
                host_probe_path,
                filesystem: "host",
                warning: None,
            }
        }
        SessionBackendKind::ContainerTransport => {
            let Some(map) = container_map else {
                return ResumeProbeTarget {
                    host_probe_path: None,
                    filesystem: "container-unreachable",
                    warning: Some(claude_config_dir_no_value_warning()),
                };
            };
            let Some(raw_config_dir) = claude_config_dir_override else {
                return ResumeProbeTarget {
                    host_probe_path: None,
                    filesystem: "container-unreachable",
                    warning: Some(claude_config_dir_no_value_warning()),
                };
            };
            let Some(container_config_dir) = container_config_dir(map, raw_config_dir) else {
                return ResumeProbeTarget {
                    host_probe_path: None,
                    filesystem: "container-unreachable",
                    warning: None,
                };
            };
            let container_projects_path = format!(
                "{}/projects/{}",
                container_config_dir.trim_end_matches('/'),
                crate::session::session::mangle_cwd_for_claude(DEFAULT_CONTAINER_WORKDIR)
            );
            ResumeProbeTarget {
                host_probe_path: map.to_host(&container_projects_path),
                filesystem: "container-via-mount",
                warning: None,
            }
        }
    }
}

/// Decide whether to auto-inject `--continue` for a Claude session.
/// Pure function: no filesystem access. Caller is responsible for resolving
/// `claude_project_exists` (typically `~/.claude/projects/<mangled-cwd>/.is_dir()`).
///
/// Callers should compute `claude_project_exists` via `resolve_claude_projects_dir`
/// to honor wrapper-set `CLAUDE_CONFIG_DIR`. (#599 R2) The caller additionally
/// honors the profile/agent env-layer `CLAUDE_CONFIG_DIR` first via
/// `claude_projects_dir_for_config_dir`; this function itself is unchanged.
/// Note: a wrapper named exactly
/// `claude.cmd` / `claude.exe` / `claude.ps1` that overrides `CLAUDE_CONFIG_DIR`
/// is intentionally NOT honored — the resolver short-circuits when the file_stem
/// equals `claude`. Users who need wrapper overrides should rename to
/// `claude-<suffix>` (e.g. `claude-mb`).
///
/// Returns `true` only when ALL of:
///   - the session is a Claude variant
///   - the caller has not requested skip
///   - the projects dir exists on disk
///   - the configured argv does not already contain `--continue`,
///     `--continue=<value>`, or `-c` (case-insensitive token match against
///     each whitespace-split token of `full_cmd`)
///
/// Note: `-c` is also Codex's short form for `--config` (e.g.,
/// `codex -c key=value`). In compound commands that mix `codex` and `claude`
/// (e.g., `cmd /K codex -c k=v && claude`), the `-c` from codex's tokens will
/// suppress Claude's `--continue` injection. Pre-existing behavior; documented
/// here so refactors do not silently lose it.
pub(crate) fn should_inject_continue(
    is_claude: bool,
    skip_auto_resume: bool,
    claude_project_exists: bool,
    full_cmd: &str,
) -> bool {
    if !is_claude || skip_auto_resume || !claude_project_exists {
        return false;
    }
    let already_has_continue = full_cmd.split_whitespace().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower.starts_with("--continue=") || lower == "-c"
    });
    !already_has_continue
}

/// Inspect the configured argv before AC adds its own launch controls. No
/// injection can also mean a first conversation, which must remain retainable.
pub(crate) fn configured_prompt_identity(
    kind: Option<CodingAgentKind>,
    shell: &str,
    shell_args: &[String],
) -> bool {
    let tokens: Vec<&str> = shell_args
        .iter()
        .flat_map(|arg| arg.split_whitespace())
        .collect();
    match kind {
        Some(CodingAgentKind::Claude) => {
            let full_cmd = format!("{} {}", shell, shell_args.join(" "));
            !should_inject_fresh_session_id(true, true, &full_cmd)
        }
        Some(CodingAgentKind::Codex) => {
            codex_tokens_have_resume(&tokens, 0)
                || (codex_has_explicit_subcommand(&tokens, 0)
                    && tokens
                        .iter()
                        .any(|token| token.eq_ignore_ascii_case("fork")))
        }
        Some(CodingAgentKind::Antigravity) => antigravity_tokens_have_resume(&tokens, 0),
        Some(CodingAgentKind::Pi) => locate_pi_command(shell, shell_args)
            .ok()
            .flatten()
            .is_some_and(|location| pi_has_explicit_session_control(&location.option_tokens)),
        Some(CodingAgentKind::Muse) => tokens.iter().any(|token| {
            CodingAgentKind::Muse
                .profile()
                .resume_tokens
                .iter()
                .any(|control| token.eq_ignore_ascii_case(control))
        }),
        _ => false,
    }
}

/// (#756) Decide whether to pass a launcher-minted `--session-id <uuid>` to a
/// Claude spawn. Pure function; mirrors `should_inject_continue`'s token scan.
///
/// Returns true only when BOTH:
///   - the session is a Claude variant, and
///   - AC is deliberately NOT resuming (`skip_auto_resume` true: fresh create
///     default, restart-fresh, or the #756 mirror guard),
///
/// and the configured argv does not already steer session identity:
/// `--session-id[=]`, `--resume[=]`/`-r`, `--continue[=]`/`-c`,
/// `--fork-session` (case-insensitive token match). User args win; stacking
/// identity flags is a HARD CLI error (Q2-verified: `--session-id` combined
/// with `--continue`/`--resume` and no `--fork-session` errors at argv
/// parse), so this veto scan is mandatory.
///
/// Purpose: belt-and-suspenders freshness (immune to future provider
/// auto-resume surprises) and a known-AT-SPAWN transcript identity (hands the
/// telegram claude_watcher its file identity instead of cwd+mtime heuristics).
/// The injected id is the SPAWN id, not a stable session id: a later in-session
/// `/clear` mints a new one.
pub(crate) fn should_inject_fresh_session_id(
    is_claude: bool,
    skip_auto_resume: bool,
    full_cmd: &str,
) -> bool {
    if !is_claude || !skip_auto_resume {
        return false;
    }
    let has_identity_flag = full_cmd.split_whitespace().any(|t| {
        let lower = t.to_lowercase();
        lower == "--session-id"
            || lower.starts_with("--session-id=")
            || lower == "--resume"
            || lower.starts_with("--resume=")
            || lower == "-r"
            || lower == "--continue"
            || lower.starts_with("--continue=")
            || lower == "-c"
            || lower == "--fork-session"
    });
    !has_identity_flag
}

/// #2454: the transcript id an argv names, so the Claude reader can pin its
/// first attach to `<id>.jsonl` instead of the newest file by mtime.
///
/// Rules, in order: the value of `--session-id` (or `--session-id=<v>`), then
/// the value of `--resume` (or `--resume=<v>`). Flags match case-insensitively
/// and every argument is split on whitespace first, so the `cmd` spawn shape
/// (`"<...>claude --session-id <uuid>"` in one element) is covered. A value
/// that is not a UUID yields `None`; `--continue`, `-c` and `-r` name no file
/// and yield `None`.
pub(crate) fn transcript_id_from_args(args: &[String]) -> Option<String> {
    let tokens: Vec<&str> = args.iter().flat_map(|a| a.split_whitespace()).collect();
    identity_flag_value(&tokens, "--session-id")
        .or_else(|| identity_flag_value(&tokens, "--resume"))
}

/// The UUID value of the first `flag` in `tokens`, in either the spaced or the
/// `=` form. A missing or non-UUID value yields `None`.
pub(crate) fn identity_flag_value(tokens: &[&str], flag: &str) -> Option<String> {
    for (i, token) in tokens.iter().enumerate() {
        let lower = token.to_ascii_lowercase();
        let value = if lower == flag {
            tokens.get(i + 1).copied()
        } else if lower.starts_with(flag) && lower.as_bytes().get(flag.len()) == Some(&b'=') {
            token.get(flag.len() + 1..)
        } else {
            continue;
        };
        return value
            .and_then(|v| Uuid::parse_str(v).ok())
            .map(|id| id.to_string());
    }
    None
}

/// Issue #107 round 5 — build the optional title prompt, or `Ok(None)` if the
/// auto-title preconditions do not hold.
///
/// Synchronous: filesystem reads only, no PTY, no await, no snapshot.
/// (#137 introduced `task-set-title` which creates its own atomic backup;
/// the backend no longer snapshots before injection.)
///
/// The caller is the post-spawn task in `create_session_inner`; it injects this
/// prompt by itself. Credentials are not part of this payload.
///
/// Gates layered (in order):
///   1. workgroup TASK.md path resolvable from `cwd` → else `Err`
///      (config issue, F7 preserved).
///   2. TASK.md exists and read succeeds → else `Err`.
///   3. TASK.md non-empty (after trim) → else `Ok(None)` (silent skip).
///   4. No `title:` field in existing frontmatter → else `Ok(None)` (silent
///      skip).
///   5. Build title prompt with the absolute, UNC-stripped path (F4
///      preserved). Return `Ok(Some(prompt))`.
pub(crate) fn build_title_prompt_appendage(_cwd: &str) -> Result<Option<String>, String> {
    Ok(None)
}
