use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::agent_command::AgentSpawnCommand;
use crate::config::agent_config::{self, AgentLocalConfig};
use crate::config::coordinator_clocks::CoordinatorClocksState;
use crate::config::sessions_persistence::persist_current_state;
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::manager::PtyManager;
use crate::resource_monitor::{
    AgentLaunchPermit, ResourceLaunchMetadata, ResourceLaunchRegistration, ResourceLimits,
    ResourceMonitorState,
};
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::session::{SessionInfo, SessionRepo, SessionStatus};
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;

static ROOT_AGENT_SESSION_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

pub(crate) fn root_agent_session_lock() -> &'static tokio::sync::Mutex<()> {
    ROOT_AGENT_SESSION_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingRootAction {
    ReuseLive,
    WakeDormant,
    DiscardMissingPty,
}

fn classify_existing_root(status: &SessionStatus, has_pty: bool) -> ExistingRootAction {
    if matches!(status, SessionStatus::Exited(_)) {
        ExistingRootAction::WakeDormant
    } else if has_pty {
        ExistingRootAction::ReuseLive
    } else {
        ExistingRootAction::DiscardMissingPty
    }
}

async fn rollback_pre_created_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    id: Uuid,
    reason: &str,
) {
    log::warn!(
        "[session] Rolling back pre-created session {} after setup failure: {}",
        id,
        reason
    );

    if let Err(e) = pty_mgr.lock().unwrap().kill(id) {
        log::warn!(
            "[session] Failed to clean PTY state while rolling back {}: {}",
            id,
            e
        );
    }

    let mgr = session_mgr.read().await;
    let was_active = mgr.get_active().await == Some(id);
    match mgr.destroy_session(id).await {
        Ok(Some(new_id)) => {
            let _ = app.emit(
                "session_switched",
                serde_json::json!({ "id": new_id.to_string() }),
            );
        }
        Ok(None) if was_active => {
            let _ = app.emit(
                "session_switched",
                serde_json::json!({ "id": serde_json::Value::Null }),
            );
        }
        Ok(None) => {}
        Err(e) => {
            log::warn!(
                "[session] Failed to remove pre-created session {} after setup failure: {}",
                id,
                e
            );
        }
    }
}

fn release_resource_launch_permit(
    monitor: &Arc<ResourceMonitorState>,
    permit: &mut Option<AgentLaunchPermit>,
) {
    if let Some(permit) = permit.take() {
        monitor.release_unregistered_permit(permit);
    }
}

fn token_has_unclosed_quote(token: &str, quote: char) -> bool {
    token.chars().filter(|c| *c == quote).count() % 2 == 1
}

fn advance_past_config_value(tokens: &[&str], start: usize) -> usize {
    if start >= tokens.len() {
        return start;
    }

    let mut idx = start;
    let mut in_single = false;
    let mut in_double = false;

    while idx < tokens.len() {
        let token = tokens[idx];
        if token_has_unclosed_quote(token, '\'') {
            in_single = !in_single;
        }
        if token_has_unclosed_quote(token, '"') {
            in_double = !in_double;
        }
        idx += 1;
        if !in_single && !in_double {
            break;
        }
    }

    idx
}

fn codex_option_takes_value(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "-c" | "--config"
            | "--enable"
            | "--disable"
            | "--remote"
            | "--remote-auth-token-env"
            | "-i"
            | "--image"
            | "-m"
            | "--model"
            | "--local-provider"
            | "-p"
            | "--profile"
            | "-s"
            | "--sandbox"
            | "-a"
            | "--ask-for-approval"
            | "--cd"
            | "--add-dir"
    )
}

fn codex_has_explicit_subcommand(tokens: &[&str], start: usize) -> bool {
    const CODEX_SUBCOMMANDS: &[&str] = &[
        "exec",
        "e",
        "review",
        "login",
        "logout",
        "mcp",
        "marketplace",
        "mcp-server",
        "app-server",
        "completion",
        "sandbox",
        "debug",
        "apply",
        "a",
        "resume",
        "fork",
        "cloud",
        "exec-server",
        "features",
        "help",
    ];

    let mut idx = start;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if codex_option_takes_value(token) {
            idx += 2;
            continue;
        }
        if token.starts_with('-') {
            idx += 1;
            continue;
        }
        return CODEX_SUBCOMMANDS
            .iter()
            .any(|subcommand| token.eq_ignore_ascii_case(subcommand));
    }

    false
}

fn codex_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
    let mut idx = start;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if token.eq_ignore_ascii_case("resume") || token.eq_ignore_ascii_case("--last") {
            return true;
        }
        idx += 1;
    }
    false
}

fn gemini_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
    let mut idx = start;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if token.eq_ignore_ascii_case("--resume") || token.to_lowercase().starts_with("--resume=") {
            return true;
        }
        idx += 1;
    }
    false
}

fn inject_gemini_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    // #260 — resume tokens sourced from the CodingAgentProfile (single source
    // of truth). G6: slice-pattern destructure, never index — a future
    // <2-element slice degrades gracefully instead of panicking in release.
    let &[resume_flag, resume_value] = CodingAgentKind::Gemini.profile().resume_tokens else {
        debug_assert!(false, "Gemini resume_tokens must have exactly 2 elements");
        return false;
    };
    match executable_basename(shell).as_str() {
        "gemini" => {
            let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
            if gemini_tokens_have_resume(&tokens, 0) {
                return false;
            }
            shell_args.insert(0, resume_flag.to_string());
            shell_args.insert(1, resume_value.to_string());
            true
        }
        "cmd" => {
            if let Some(idx) = shell_args
                .iter()
                .position(|arg| executable_basename(arg) == "gemini")
            {
                let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
                if gemini_tokens_have_resume(&tokens, idx + 1) {
                    return false;
                }
                shell_args.insert(idx + 1, resume_flag.to_string());
                shell_args.insert(idx + 2, resume_value.to_string());
                return true;
            }

            for arg in shell_args.iter_mut() {
                let mut tokens: Vec<String> = arg
                    .split_whitespace()
                    .map(|token| token.to_string())
                    .collect();
                if let Some(idx) = tokens
                    .iter()
                    .position(|token| executable_basename(token) == "gemini")
                {
                    let token_refs: Vec<&str> = tokens.iter().map(|token| token.as_str()).collect();
                    if gemini_tokens_have_resume(&token_refs, idx + 1) {
                        return false;
                    }
                    tokens.insert(idx + 1, resume_flag.to_string());
                    tokens.insert(idx + 2, resume_value.to_string());
                    *arg = tokens.join(" ");
                    return true;
                }
            }

            false
        }
        _ => false,
    }
}

fn inject_codex_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    // #260 — resume tokens sourced from the CodingAgentProfile (single source
    // of truth). G6: slice-pattern destructure, never index — a future
    // <2-element slice degrades gracefully instead of panicking in release.
    let &[resume_subcmd, resume_flag] = CodingAgentKind::Codex.profile().resume_tokens else {
        debug_assert!(false, "Codex resume_tokens must have exactly 2 elements");
        return false;
    };
    match executable_basename(shell).as_str() {
        "codex" => {
            let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
            if codex_tokens_have_resume(&tokens, 0) || codex_has_explicit_subcommand(&tokens, 0) {
                return false;
            }
            shell_args.insert(0, resume_subcmd.to_string());
            shell_args.insert(1, resume_flag.to_string());
            true
        }
        "cmd" => {
            if let Some(idx) = shell_args
                .iter()
                .position(|arg| executable_basename(arg) == "codex")
            {
                let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
                if codex_tokens_have_resume(&tokens, idx + 1)
                    || codex_has_explicit_subcommand(&tokens, idx + 1)
                {
                    return false;
                }
                shell_args.insert(idx + 1, resume_subcmd.to_string());
                shell_args.insert(idx + 2, resume_flag.to_string());
                return true;
            }

            for arg in shell_args.iter_mut() {
                let mut tokens: Vec<String> = arg
                    .split_whitespace()
                    .map(|token| token.to_string())
                    .collect();
                if let Some(idx) = tokens
                    .iter()
                    .position(|token| executable_basename(token) == "codex")
                {
                    let token_refs: Vec<&str> = tokens.iter().map(|token| token.as_str()).collect();
                    if codex_tokens_have_resume(&token_refs, idx + 1)
                        || codex_has_explicit_subcommand(&token_refs, idx + 1)
                    {
                        return false;
                    }
                    tokens.insert(idx + 1, resume_subcmd.to_string());
                    tokens.insert(idx + 2, resume_flag.to_string());
                    *arg = tokens.join(" ");
                    return true;
                }
            }

            false
        }
        _ => false,
    }
}

/// Single-pass expansion of `%NAME%` (cmd) and `$env:NAME` (PowerShell)
/// environment-variable references against `std::env::var`. Unknown names
/// are preserved literally, so a downstream `is_dir()` check returns
/// false rather than silently mis-resolving. Names must be ASCII
/// alphanumeric or `_`; anything else terminates the name.
///
/// Limitations (acceptable for real-world wrappers):
///   - No nested expansion: `%A%` whose value contains `%B%` is not re-expanded.
///   - No escape syntax (cmd's `^%`, PowerShell's backtick); wrappers don't use these.
///
/// (#599 R2) Lifted to module scope (was nested in `resolve_claude_projects_dir`)
/// so `claude_projects_dir_for_config_dir` can reuse it for the env-layer probe.
fn expand_env_var_refs(input: &str) -> String {
    // Pass 1: %NAME% (cmd-style).
    let mut buf = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        buf.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                let valid =
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if valid {
                    if let Ok(v) = std::env::var(name) {
                        buf.push_str(&v);
                    } else {
                        buf.push('%');
                        buf.push_str(name);
                        buf.push('%');
                    }
                } else {
                    // Not a valid var name (e.g. "100%" literal); preserve.
                    buf.push('%');
                    buf.push_str(name);
                    buf.push('%');
                }
                rest = &after[end + 1..];
            }
            None => {
                buf.push('%');
                buf.push_str(after);
                rest = "";
                break;
            }
        }
    }
    buf.push_str(rest);

    // Pass 2: $env:NAME (PowerShell-style). Case-insensitive prefix; name
    // terminates at the first byte that is not [A-Za-z0-9_].
    let mut out = String::with_capacity(buf.len());
    let bytes = buf.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let remaining = &buf[i..];
        if remaining.len() >= 5 && remaining.as_bytes()[..5].eq_ignore_ascii_case(b"$env:") {
            let name_start = i + 5;
            let mut name_end = name_start;
            while name_end < bytes.len() {
                let c = bytes[name_end];
                if c.is_ascii_alphanumeric() || c == b'_' {
                    name_end += 1;
                } else {
                    break;
                }
            }
            if name_end > name_start {
                let name = &buf[name_start..name_end];
                if let Ok(v) = std::env::var(name) {
                    out.push_str(&v);
                } else {
                    out.push_str(&buf[i..name_end]);
                }
                i = name_end;
                continue;
            }
        }
        // Default: copy one full UTF-8 char.
        let ch = buf[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

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
    use std::path::{Path, PathBuf};

    fn default_base() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".claude"))
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
        // Cap read at 64 KiB; real wrappers are < 1 KiB. Refusing larger
        // files protects against accidentally treating an exe-renamed-as-cmd
        // as a wrapper.
        const MAX: u64 = 64 * 1024;
        let metadata = std::fs::metadata(path).ok()?;
        if metadata.len() > MAX {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
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

    fn looks_like_wrapper_extension(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let lc = e.to_ascii_lowercase();
                matches!(lc.as_str(), "cmd" | "bat" | "ps1" | "sh")
            })
            .unwrap_or(false)
    }

    fn resolve_token_to_file(token: &str) -> Option<PathBuf> {
        let p = Path::new(token);
        // Direct path (absolute, or relative with separator) — use as-is if
        // it exists. Avoids consulting %PATH% when the user already gave us
        // a full location.
        let has_separator = token.contains('/') || token.contains('\\');
        if has_separator || p.is_absolute() {
            return if p.is_file() {
                Some(p.to_path_buf())
            } else {
                None
            };
        }
        // Bare basename — defer to %PATH% + PATHEXT (Windows) via `which`.
        which::which(token).ok()
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
    // executable_basename returns a lowercased stem; comparison is case-insensitive by construction.
    if executable_basename(&claude_token) == "claude" {
        return default_base().map(|base| base.join("projects").join(&mangled));
    }

    // Non-default name (e.g. `claude-mb`). Try to resolve to an actual file
    // and parse it for a CLAUDE_CONFIG_DIR override.
    if let Some(file) = resolve_token_to_file(&claude_token) {
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
fn claude_projects_dir_for_config_dir(config_dir: &str, cwd: &str) -> std::path::PathBuf {
    let base = std::path::PathBuf::from(expand_env_var_refs(config_dir));
    let mangled = crate::session::session::mangle_cwd_for_claude(cwd);
    base.join("projects").join(mangled)
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
fn should_inject_continue(
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
fn build_title_prompt_appendage(_cwd: &str) -> Result<Option<String>, String> {
    Ok(None)
}

/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects provider-specific
/// resume flags (`claude --continue`, `codex resume --last`, `gemini --resume latest`)
/// when appropriate.
/// If `skip_tooling_save` is true, skips writing to the repo's config.json (for temp sessions).
///
/// `skip_auto_resume` controls provider auto-resume injection:
/// - `true` — suppress all provider auto-resume. Use this for any "fresh
///   create" call site (UI/CLI/root-agent create, mailbox wake-from-cold
///   meaning no SessionManager record at this CWD, `restart_session` with
///   default semantics from `effective_restart_skip_auto_resume`).
/// - `false` — allow provider auto-resume. Use this only for paths restoring
///   a session AC already knows about (the startup-restore loop in `lib.rs`,
///   the wake-from-known-state branch in `mailbox::deliver_wake` — any
///   `RespawnExited` match, today driven exclusively by deferred-non-coord
///   `Exited(0)` records — and `restart_session` when its caller passes
///   `Some(false)`).
// Shared by Tauri command + restore path; collapsing args would force a context struct.
#[allow(clippy::too_many_arguments)]
pub async fn create_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
) -> Result<SessionInfo, String> {
    let (agent_id, agent_label) = {
        if let Some(spawn) = resolved_spawn.as_ref() {
            (
                Some(spawn.trusted_agent_id.clone()),
                Some(spawn.trusted_agent_label.clone()),
            )
        } else {
            let settings_state = app.state::<SettingsState>();
            let cfg = settings_state.read().await;
            resolve_actual_agent(
                &shell,
                &shell_args,
                agent_id.as_deref(),
                agent_label.as_deref(),
                &cfg,
            )
        }
    };

    // Recompute is_coordinator from the current team snapshot. One source of truth —
    // every caller of create_session_inner gets the same computation.
    let teams = tokio::task::spawn_blocking(crate::config::teams::discover_teams)
        .await
        .map_err(|e| e.to_string())?;
    let is_coordinator = crate::config::teams::is_coordinator_for_cwd(&cwd, &teams);
    let is_root_agent = crate::config::root_agent::is_root_agent_path(&cwd);

    // #552 seed the badge clock for a coordinator's first spawn so it shows 0m
    // immediately, and clear any "auto-closed" marker. An auto-closed coordinator
    // is DESTROYED, so its reopen flows through this create path (the "create
    // in-place" branch of handleReplicaClick), NOT restart_session_inner.
    // seed_if_absent never overwrites, so a respawn does NOT reset the badge.
    if is_coordinator {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            // agent_fqn_from_path returns String (teams.rs:80), not Option.
            let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
            let now = chrono::Utc::now();
            let (seeded, cleared_auto, cleared_manual) = {
                let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
                (
                    g.seed_if_absent(&fqn, now),
                    g.clear_auto_closed(&fqn),
                    g.clear_manually_closed(&fqn),
                )
            };
            if seeded {
                let _ = app.emit(
                    "coordinator_clock_updated",
                    serde_json::json!({ "replicaPath": cwd, "lastUserMessageAt": now.to_rfc3339() }),
                );
            }
            if cleared_auto {
                let _ = app.emit(
                    "coordinator_auto_close_changed",
                    serde_json::json!({ "replicaPath": cwd, "autoClosedAt": null }),
                );
            }
            if cleared_manual {
                let _ = app.emit(
                    "coordinator_manual_close_changed",
                    serde_json::json!({ "replicaPath": cwd, "manuallyClosedAt": null }),
                );
            }
        }
    }

    let agent_kind = CodingAgentKind::detect(&shell, &shell_args);
    let is_agent_owned_launch = agent_id.is_some() || agent_kind.is_some() || is_root_agent;
    let resource_monitor = app.state::<Arc<ResourceMonitorState>>().inner().clone();
    let mut resource_permit = if is_agent_owned_launch {
        let cfg = app.state::<SettingsState>().read().await.clone();
        resource_monitor.try_reserve_agent_slot(ResourceLimits::from(&cfg))?
    } else {
        None
    };

    let mgr = session_mgr.read().await;
    let mut session = match mgr
        .create_session(
            shell.clone(),
            shell_args.clone(),
            cwd.clone(),
            agent_id.clone(),
            agent_label.clone(),
            git_repos,
            is_coordinator,
        )
        .await
    {
        Ok(session) => session,
        Err(e) => {
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Err(e.to_string());
        }
    };

    if is_root_agent {
        mgr.set_is_root_agent(session.id, true).await;
        session.is_root_agent = true;
    }

    if let Some(name) = session_name {
        if let Err(e) = mgr.rename_session(session.id, name.clone()).await {
            let err = e.to_string();
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            drop(mgr);
            rollback_pre_created_session(app, session_mgr, pty_mgr, session.id, &err).await;
            return Err(err);
        }
        session.name = name;
    }

    let id = session.id;
    if let Some(spawn) = resolved_spawn.as_ref() {
        let effective_codex_home = spawn
            .effective_codex_home
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
        mgr.set_profile_metadata(
            id,
            Some(spawn.profile_resolution.requested_profile.clone()),
            Some(spawn.profile_resolution.effective_profile.clone()),
            spawn.profile_resolution.fallback_chain.clone(),
            spawn.profile_resolution.fallback_applied,
            effective_codex_home.clone(),
            Some(spawn.profile_content_hash.clone()),
        )
        .await;
        session.requested_profile = Some(spawn.profile_resolution.requested_profile.clone());
        session.effective_profile = Some(spawn.profile_resolution.effective_profile.clone());
        session.profile_fallback_chain = spawn.profile_resolution.fallback_chain.clone();
        session.profile_fallback_applied = spawn.profile_resolution.fallback_applied;
        session.effective_codex_home = effective_codex_home;
        session.profile_content_hash = Some(spawn.profile_content_hash.clone());
    }

    let mut shell_args = shell_args;
    let full_cmd = format!("{} {}", shell, shell_args.join(" "));
    // #260: single detector (session/profile.rs). Replaces the old
    // starts_with triple; this is the SAME call `strip_auto_injected_args`
    // makes, so the persisted recipe and the runtime identity cannot disagree.
    let context_target = match agent_kind {
        Some(CodingAgentKind::Claude) => {
            Some(crate::config::session_context::ManagedContextTarget::Claude)
        }
        Some(CodingAgentKind::Codex) => {
            Some(crate::config::session_context::ManagedContextTarget::Codex)
        }
        Some(CodingAgentKind::Gemini) => {
            Some(crate::config::session_context::ManagedContextTarget::Gemini)
        }
        None => None,
    };

    // Single source of truth — store the identity on the SessionManager record
    // AND the local clone (the latter feeds the imminent `session_created` emit).
    mgr.set_agent_kind(id, agent_kind).await;
    session.agent_kind = agent_kind;

    // Auto-inject --continue for Claude agents when AC has reason to believe a prior
    // conversation exists for this session (issue #82: `is_dir()` alone is unsound;
    // call sites pass `skip_auto_resume = true` for fresh creates).
    // Issue #186: honor `CLAUDE_CONFIG_DIR` overrides set by `.cmd`/`.bat`/`.ps1`/`.sh`
    // wrappers (e.g. `claude-mb`) so the probe locates the real transcript store.
    // #599 R2 - also honor the profile/agent env layer's CLAUDE_CONFIG_DIR. The env
    // approach launches a plain `claude`, so resolve_claude_projects_dir would
    // short-circuit to ~/.claude and miss env-relocated transcripts. When the
    // resolved spawn carries CLAUDE_CONFIG_DIR, probe <that>/projects/<mangled>;
    // otherwise keep the wrapper-aware default resolution.
    let claude_config_dir_override = resolved_spawn.as_ref().and_then(|s| {
        s.child_env
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
            .map(|(_, v)| v.clone())
    });
    let resolved_claude_projects_dir = match claude_config_dir_override {
        Some(ref dir) => Some(claude_projects_dir_for_config_dir(dir, &cwd)),
        None => resolve_claude_projects_dir(&shell, &shell_args, &cwd),
    };
    let claude_project_exists = resolved_claude_projects_dir
        .as_ref()
        .map(|p| p.is_dir())
        .unwrap_or(false);
    let is_claude = agent_kind == Some(CodingAgentKind::Claude);
    let will_inject_continue = should_inject_continue(
        is_claude,
        skip_auto_resume,
        claude_project_exists,
        &full_cmd,
    );
    // (#630/#631) Resume-decision trace. Widened beyond Claude: codex/gemini ride
    // the same `skip_auto_resume` axis (below) and were equally invisible. INFO
    // during the stabilization window; demote later.
    if agent_kind.is_some() {
        log::info!(
            "[session] resume-decision {} agent={:?} cwd={:?} projects_dir={:?} exists={} skip_auto_resume={} -> inject_continue={}",
            &id.to_string()[..8],
            agent_kind,
            cwd,
            resolved_claude_projects_dir,
            claude_project_exists,
            skip_auto_resume,
            will_inject_continue
        );
    }
    if will_inject_continue {
        // #260 — Claude's resume flag from the CodingAgentProfile. resume_tokens
        // is a 1-element const for Claude, so [0] is provably in bounds.
        let continue_flag = CodingAgentKind::Claude.profile().resume_tokens[0];
        if let Some(ref aid) = agent_id {
            if executable_basename(&shell) == "cmd" {
                if let Some(last) = shell_args.last_mut() {
                    if executable_basename(last) == "claude"
                        || last.to_lowercase().contains("claude")
                    {
                        *last = format!("{} {}", last, continue_flag);
                        log::info!("Auto-injected --continue for agent '{}' (prior conversation exists, cmd path)", aid);
                    }
                }
            } else {
                shell_args.push(continue_flag.to_string());
                log::info!(
                    "Auto-injected --continue for agent '{}' (prior conversation exists)",
                    aid
                );
            }
        }
    }

    if agent_kind == Some(CodingAgentKind::Codex) && !skip_auto_resume {
        if let Some(ref aid) = agent_id {
            if inject_codex_resume(&shell, &mut shell_args) {
                log::info!("Auto-injected `codex resume --last` for agent '{}'", aid);
            }
        }
    }

    if agent_kind == Some(CodingAgentKind::Gemini) && !skip_auto_resume {
        if let Some(ref aid) = agent_id {
            if inject_gemini_resume(&shell, &mut shell_args) {
                log::info!("Auto-injected `gemini --resume latest` for agent '{}'", aid);
            }
        }
    }

    // #529 - resolve the instructions filename from the configured coding agent
    // (falling back to detection for ad-hoc launches), plus the union of every
    // configured agent's filename for cleanup. Computed under a single settings
    // read guard that is dropped before any filesystem I/O (no guard across the
    // materialize call).
    let (target_filename, managed_filenames, auto_self_clear): (Option<String>, Vec<String>, bool) = {
        let settings_state = app.state::<SettingsState>();
        let cfg = settings_state.read().await;
        let managed = crate::config::agent_command::managed_instructions_filenames(&cfg);
        let target = crate::config::agent_command::resolve_target_filename(
            agent_id.as_deref(),
            &cfg,
            context_target,
        );
        // #640 resolve under the same guard; no second lock, dropped before I/O.
        // Class-aware default: ON for coordinator/Root, OFF for specialists,
        // unless a per-agent override is set. Gated to coding-agent sessions.
        let class_default_on =
            is_coordinator || crate::config::root_agent::is_root_agent_dir_name(&cwd);
        let auto_self_clear = agent_kind.is_some()
            && crate::config::settings::resolve_auto_self_clear(
                &cfg,
                &crate::config::coding_agent_profiles::agent_name_from_dir(std::path::Path::new(
                    &cwd,
                ))
                .unwrap_or_default(),
                class_default_on,
            );
        (target, managed, auto_self_clear)
    };

    let materialized_context_path = if let Some(ref target_filename) = target_filename {
        match crate::config::session_context::materialize_agent_context_file_with_filename(
            &cwd,
            target_filename,
            &managed_filenames,
            is_coordinator,
            auto_self_clear,
        ) {
            Ok(context) => context,
            Err(e) => {
                log::error!("Replica context validation failed: {}", e);
                use tauri_plugin_dialog::DialogExt;
                // #537 facet (b) - the old copy blamed "context files missing",
                // but the real cause is usually a transient config.json lock
                // during replica identity repair. State what actually failed;
                // the interpolated error carries the precise, retry-suggesting
                // detail from format_publish_error.
                let dialog_msg = format!(
                    "Cannot launch session - failed to update replica config:\n\n{}",
                    e
                );
                app.dialog()
                    .message(&dialog_msg)
                    .title("Session Launch Error")
                    .show(|_| {});
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                drop(mgr);
                rollback_pre_created_session(app, session_mgr, pty_mgr, id, &e).await;
                return Err(e);
            }
        }
    } else {
        None
    };

    // Claude consumes the materialized CLAUDE.md via --append-system-prompt-file.
    if agent_kind == Some(CodingAgentKind::Claude) {
        if let Some(context_path) = materialized_context_path.as_ref() {
            if executable_basename(&shell) == "cmd" {
                if let Some(last) = shell_args.last_mut() {
                    if last.to_lowercase().contains("claude") {
                        *last =
                            format!("{} --append-system-prompt-file \"{}\"", last, context_path);
                        log::info!("Injected --append-system-prompt-file for Claude (cmd path)");
                    }
                }
            } else {
                shell_args.push("--append-system-prompt-file".to_string());
                shell_args.push(context_path.to_string());
                log::info!("Injected --append-system-prompt-file for Claude session");
            }
        }
    }

    // Capture the effective arg vector BEFORE spawn so SessionInfo::from(&session)
    // (emitted at line ~439 as "session_created") carries the injected flags.
    // Bind once, broadcast to two consumers: the store write is for later
    // `mgr.get_session` callers; the local-clone write is for the imminent emit.
    //
    // DO NOT REMOVE OR GATE THIS CAPTURE. Issue #65 regression guard — removing
    // or wrapping in a condition reintroduces the exact bug this plan fixes.
    // See _plans/bug-statusbar-dynamic-launch-args.md §10 and §15 for rationale.
    let effective = shell_args.clone();
    mgr.set_effective_shell_args(id, effective.clone()).await;
    session.effective_shell_args = Some(effective);

    let extra_env = if agent_id.is_some() {
        crate::pty::credentials::build_credentials_env(&session.token, &cwd)
    } else {
        Vec::new()
    };
    let configured_env: Vec<(String, String)> = resolved_spawn
        .as_ref()
        .map(|spawn| spawn.child_env.clone())
        .unwrap_or_default();
    let env_remove_keys: Vec<String> = resolved_spawn
        .as_ref()
        .map(|spawn| spawn.env_remove_keys.clone())
        .unwrap_or_default();
    let resource_registration = resource_permit.take().map(|permit| {
        // #516 - human-readable WG + agent identity for the Resource Monitor row,
        // derived from the deterministic spawn cwd (not the user-renamable
        // session.name). Root agents carry no wg-/__agent_ segments, so label them
        // explicitly with the bare replica dir name.
        let (workgroup, agent) = {
            let (wg, ag) = crate::config::teams::workgroup_and_agent_from_path(&cwd);
            if wg.is_some() {
                (wg, ag)
            } else if is_root_agent {
                let bare = cwd
                    .replace('\\', "/")
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .map(|s| s.to_string());
                (Some("Root agent".to_string()), bare)
            } else {
                (None, None)
            }
        };
        // #566 - project folder name for the Resource Monitor row, derived from
        // the same deterministic spawn cwd as the (workgroup, agent) pair above.
        let project = crate::config::teams::project_from_path(&cwd);
        ResourceLaunchRegistration::new(
            resource_monitor.as_ref().clone(),
            permit,
            ResourceLaunchMetadata {
                session_id: id,
                name: session.name.clone(),
                agent_id: agent_id.clone(),
                agent_label: agent_label.clone(),
                workgroup,
                agent,
                project,
            },
        )
    });

    // #598 - seed the config folder before the PTY starts so the agent sees the
    // templated config. Best-effort: never aborts the spawn. This is the single
    // execution chokepoint every real spawn funnels through (create / replica /
    // other variants, delivery / mailbox / web wakes); prevalidation never hits
    // it because it discards the spawn. Per Q2 this runs on EVERY spawn,
    // including loop/scheduler resume-wakes (overwrite every spawn).
    if let Some(seed) = resolved_spawn.as_ref().and_then(|s| s.seed.as_ref()) {
        // grinch HIGH-1: serialize the seed swap for ALL dests (not just
        // `.claude`). Two same-replica spawns can run concurrently (e.g. a
        // delivery tick + a mailbox wake) holding only `session_mgr.read()`,
        // with no global spawn lock. Without serialization, spawn B's
        // prefix-sweep of stale scratch (`clear_stale_seed_scratch`) could
        // delete spawn A's in-flight temp/trash mid-swap and lose the config
        // (breaking H1's "dest always fully-old or fully-new" + M3 isolation).
        // Under this lock any other-id scratch is truly stale, so the
        // leak-reclaim sweep stays safe. The same lock also gives the `.claude`
        // re-apply its M2 serialization vs sweep_rtk_hook's settings.local.json
        // read-modify-write. Clone the Arc out of State first so the owned guard
        // does not borrow a State temporary (E0716).
        let _seed_guard = {
            let lock = app.state::<crate::RtkSweepLockState>().inner().clone();
            lock.lock_owned().await
        };

        let report = crate::config::config_seed::perform_config_seed(seed, &id.to_string());

        // M1: a `.claude` seed clean-replaces the dir, wiping the AC-managed
        // settings.local.json. Re-stamp the rtk hook (per the global toggle).
        // `cwd` is the replica root; the writer appends `.claude`, so this lands
        // in the seeded dir.
        if matches!(report, crate::config::config_seed::ConfigSeedReport::Seeded) {
            if let Some(re) = &seed.claude_settings_reapply {
                let dir = std::path::Path::new(&cwd);
                if let Err(e) =
                    crate::config::claude_settings::ensure_rtk_pretool_hook(dir, re.inject_rtk_hook)
                {
                    log::warn!("[config-seed] re-apply rtk hook failed: {}", e);
                }
            }
        }
    }

    let spawn_result = {
        pty_mgr.lock().unwrap().spawn(
            id,
            &shell,
            &shell_args,
            &cwd,
            120,
            30,
            &configured_env,
            &env_remove_keys,
            &extra_env,
            crate::session::profile::idle_tuning_for(agent_kind),
            app.clone(),
            resource_registration,
        )
    };
    if let Err(e) = spawn_result {
        let err = e.to_string();
        drop(mgr);
        rollback_pre_created_session(app, session_mgr, pty_mgr, id, &err).await;
        return Err(err);
    }

    // Auto-inject optional non-credential bootstrap text for agent sessions
    // after PTY spawn. Credentials are already present in child environment
    // variables; no credentials are written through PTY.
    //
    // Currently the only bootstrap payload is the Coordinator auto-title prompt.
    if agent_id.is_some() && !is_coordinator {
        log::debug!(
            "[session] No bootstrap injection for non-coordinator agent session {}",
            id
        );
    }
    if agent_id.is_some() && is_coordinator {
        let auto_title_enabled = false; /*
                                            let settings_state = app.state::<SettingsState>();
                                            let cfg = settings_state.read().await;
                                            cfg.auto_generate_task_title
                                        };*/

        if auto_title_enabled {
            let app_clone = app.clone();
            let session_id = id;
            let cwd_clone = cwd.clone();

            tokio::spawn(async move {
                let prompt = match build_title_prompt_appendage(&cwd_clone) {
                    Ok(Some(prompt)) => {
                        log::info!(
                            "[session] Auto-title prompt built for session {}",
                            session_id
                        );
                        prompt
                    }
                    Ok(None) => {
                        log::info!(
                            "[session] Auto-title prompt skipped (gate not passed) for session {}",
                            session_id
                        );
                        return;
                    }
                    Err(e) => {
                        log::warn!(
                            "[session] Auto-title prompt skipped for session {}: {}",
                            session_id,
                            e
                        );
                        return;
                    }
                };

                let max_wait = std::time::Duration::from_secs(30);
                let poll = std::time::Duration::from_millis(500);
                let start = std::time::Instant::now();

                loop {
                    if start.elapsed() >= max_wait {
                        log::warn!(
                            "[session] Timeout waiting for idle before auto-title prompt injection for session {}",
                            session_id
                        );
                        break;
                    }
                    tokio::time::sleep(poll).await;

                    let session_mgr = app_clone.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                    let mgr = session_mgr.read().await;
                    let sessions = mgr.list_sessions().await;
                    match sessions.iter().find(|s| s.id == session_id.to_string()) {
                        Some(s) if s.waiting_for_input => break,
                        Some(_) => {}
                        None => {
                            log::warn!(
                                "[session] Session {} gone before auto-title prompt injection",
                                session_id
                            );
                            return;
                        }
                    }
                }

                match crate::pty::inject::inject_text_into_session(&app_clone, session_id, &prompt)
                    .await
                {
                    Ok(()) => {
                        log::info!(
                            "[session] Auto-title prompt injected for session {}",
                            session_id
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "[session] Failed to inject auto-title prompt for {}: {}",
                            session_id,
                            e
                        );
                    }
                }
            });
        }
    }

    let info = SessionInfo::from(&session);
    let _ = app.emit("session_created", info.clone());

    // 0.8.0: removed the "Show the terminal window when a session is created" branch.
    // Under the unified-window model the main window is created up-front and stays
    // visible; session creation has no window-show responsibility.

    // Save lastCodingAgent + codingAgents (skip for temp sessions)
    if !skip_tooling_save {
        if let Some(ref aid) = agent_id {
            // Resolve label: use provided agent_label, or look up from settings by agent_id.
            // Without this fallback, callers that pass agent_id but no label (session-requests,
            // web remote) would write app: "Unknown" into the per-instance config.json.
            let resolved_label = match agent_label.as_deref() {
                Some(l) => l.to_string(),
                None => {
                    let settings = app.state::<SettingsState>();
                    let cfg = settings.read().await;
                    resolve_agent_label(aid, &cfg).unwrap_or_else(|| {
                        log::warn!(
                            "Could not resolve label for agent_id='{}' — defaulting to 'Unknown'",
                            aid
                        );
                        "Unknown".to_string()
                    })
                }
            };
            let session_id_str = id.to_string();
            if let Err(e) = agent_config::set_last_coding_agent(
                &cwd,
                aid,
                &resolved_label,
                Some(&session_id_str),
            ) {
                log::warn!("Failed to save lastCodingAgent: {}", e);
            }
            // #592 - persist the loaded profile content-hash so drift survives an
            // AC restart. Best-effort; never aborts the spawn. No-op off-replica.
            if let Some(hash) = session.profile_content_hash.as_deref() {
                log::debug!(
                    "[profile-hash] spawn-persist: session={} agent={} hash={} cwd={:?}",
                    id,
                    aid,
                    hash,
                    cwd,
                );
                if let Err(e) =
                    crate::config::coding_agent_profiles::set_replica_profile_content_hash(
                        std::path::Path::new(&cwd),
                        hash,
                    )
                {
                    log::warn!("Failed to persist profileContentHash: {}", e);
                }
            }
        }
    }

    Ok(info)
}

pub(crate) fn build_configured_agent_spawn_for_cwd(
    settings: &AppSettings,
    agent_id: &str,
    cwd: &str,
    requested_profile: Option<&str>,
) -> Result<Option<AgentSpawnCommand>, String> {
    if !settings.agents.iter().any(|agent| agent.id == agent_id) {
        return Ok(None);
    }
    crate::config::agent_command::build_agent_spawn_command(
        settings,
        agent_id,
        Some(std::path::Path::new(cwd)),
        requested_profile,
    )
    .map(Some)
}

/// #592 - true when a session's loaded profile cell no longer matches what a
/// Reload would load right now. Mirrors the restart-time resolution so the
/// "configured" side equals `restart_session`'s effective cell. Hashes RAW
/// cells (cell-only); swallows all "cannot determine" cases to false (never
/// false-alarms). Plain-shell sessions (no agent) never drift.
fn compute_profile_outdated(settings: &AppSettings, info: &SessionInfo) -> bool {
    if info.agent_id.is_none() {
        return false;
    }
    let cwd = info.working_directory.as_str();
    // Which coding agent + letter a Reload would launch (honors currentCodingAgent).
    let Some(agent_id) =
        resolve_restart_selected_agent_id(settings, cwd, None, info.agent_id.as_deref())
    else {
        return false;
    };
    let requested = effective_restart_requested_profile(None, info.requested_profile.clone());
    let resolution = crate::config::coding_agent_profiles::resolve_profile(
        settings,
        crate::config::coding_agent_profiles::ProfileResolutionRequest {
            coding_agent_id: &agent_id,
            launch_path: Some(std::path::Path::new(cwd)),
            agent_matrix_name: None,
            requested_profile: requested.as_deref(),
        },
    );
    // #597 - mirror the spawn-time composition exactly: hash the effective command
    // (agent base + cell params) and the raw merged env (agent + cell). Look up the
    // agent a Reload would launch; if it vanished from settings, do not false-flag.
    let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) else {
        return false;
    };
    let configured_command = crate::config::agent_command::compose_effective_command(
        &agent.command,
        &resolution.cell.command,
    );
    let configured_env =
        crate::config::agent_command::raw_merged_profile_env(agent, &resolution.cell.env);
    let configured =
        crate::config::agent_command::profile_content_hash(&configured_command, &configured_env);
    // Loaded hash: in-memory stamp, else the persisted replica copy (survives an
    // AC restart that cleared the in-memory stamp).
    let loaded = info.profile_content_hash.clone().or_else(|| {
        crate::config::coding_agent_profiles::read_replica_profile_content_hash(
            std::path::Path::new(cwd),
        )
    });
    match loaded {
        Some(loaded) => loaded != configured,
        None => false,
    }
}

/// Create a new session. Optionally override shell/args/cwd/name (for action buttons).
/// Falls back to settings defaults when not provided.
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    shell: Option<String>,
    shell_args: Option<Vec<String>>,
    cwd: Option<String>,
    session_name: Option<String>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    git_repos: Option<Vec<SessionRepo>>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
    let cfg = settings.read().await;

    let cwd = cwd.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\".to_string())
    });

    let resolved_spawn = if let Some(aid) = agent_id.as_deref() {
        build_configured_agent_spawn_for_cwd(&cfg, aid, &cwd, requested_profile.as_deref())?
    } else {
        None
    };

    let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
        (
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            Some(spawn.trusted_agent_label.clone()),
        )
    } else {
        let s = shell.unwrap_or_else(|| cfg.default_shell.clone());
        let sa = shell_args.unwrap_or_else(|| cfg.default_shell_args.clone());
        let al = agent_id.as_ref().and_then(|aid| {
            cfg.agents
                .iter()
                .find(|a| a.id == *aid)
                .map(|a| a.label.clone())
        });
        (s, sa, al)
    };

    log::info!(
        "[session] FINAL resolved: shell={:?}, args={:?}, label={:?}",
        shell,
        shell_args,
        agent_label
    );

    drop(cfg);

    let info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        cwd.clone(),
        session_name,
        agent_id,
        agent_label,
        false, // persist tooling
        git_repos.unwrap_or_default(),
        // #599 R1: default true (fresh create); reopen of a closed coordinator
        // passes Some(false) so the prior conversation resumes.
        effective_create_skip_auto_resume(skip_auto_resume),
        resolved_spawn,
    )
    .await?;

    // Persist after creation
    {
        let mgr = session_mgr.read().await;
        persist_current_state(&mgr).await;
    }

    // Auto-attach Telegram bot if repo has .agentscommander/config.json
    let id = Uuid::parse_str(&info.id).unwrap();
    attach_local_config_telegram_if_any(&app, id, &cwd).await;

    Ok(info)
}

pub(crate) async fn attach_persisted_telegram_if_configured<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    bot_id: Option<&str>,
) {
    let Some(bot_id) = bot_id else {
        return;
    };

    let settings = app.state::<SettingsState>();
    let exists = {
        let cfg = settings.read().await;
        cfg.telegram_bots.iter().any(|b| b.id == bot_id)
    };

    if !exists {
        log::warn!(
            "[telegram] Persisted bot '{}' for session {} no longer exists in settings; leaving bridge OFF",
            bot_id,
            session_id
        );
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_telegram_bot_id(session_id, None).await;
        persist_current_state(&mgr).await;
        return;
    }

    let already_attached = {
        let tg_mgr = app.state::<TelegramBridgeState>();
        let tg = tg_mgr.lock().await;
        tg.get_bridge(session_id)
    };
    if let Some(info) = already_attached {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        if info.bot_id == bot_id {
            mgr.set_telegram_bot_id(session_id, Some(bot_id.to_string()))
                .await;
        } else {
            log::warn!(
                "[telegram] Session {} already has Telegram bot '{}' attached; persisted bot '{}' will be cleared",
                session_id,
                info.bot_id,
                bot_id
            );
            mgr.set_telegram_bot_id(session_id, None).await;
        }
        persist_current_state(&mgr).await;
        return;
    }

    if let Err(e) =
        crate::commands::telegram::attach_telegram_bot_by_id(app, session_id, bot_id).await
    {
        log::warn!(
            "[telegram] Failed to restore persisted bot '{}' for session {}: {}",
            bot_id,
            session_id,
            e
        );
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_telegram_bot_id(session_id, None).await;
        persist_current_state(&mgr).await;
    }
}

pub(crate) async fn preserve_deferred_telegram_intent_if_valid(
    mgr: &SessionManager,
    settings: &SettingsState,
    session_id: Uuid,
    session_name: &str,
    bot_id: Option<&str>,
) {
    let Some(bot_id) = bot_id else {
        return;
    };

    let exists = {
        let cfg = settings.read().await;
        cfg.telegram_bots.iter().any(|b| b.id == bot_id)
    };

    if exists {
        mgr.set_telegram_bot_id(session_id, Some(bot_id.to_string()))
            .await;
    } else {
        log::warn!(
            "[telegram] Persisted bot '{}' for deferred session '{}' no longer exists in settings; leaving bridge OFF",
            bot_id,
            session_name
        );
        mgr.set_telegram_bot_id(session_id, None).await;
    }
}

pub(crate) async fn attach_local_config_telegram_if_any<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    cwd: &str,
) {
    let config_path = std::path::Path::new(cwd)
        .join(crate::config::agent_local_dir_name())
        .join("config.json");

    let Some(bot_label) = tokio::fs::read_to_string(&config_path)
        .await
        .ok()
        .and_then(|contents| serde_json::from_str::<AgentLocalConfig>(&contents).ok())
        .and_then(|local_config| local_config.tooling.telegram_bot)
    else {
        return;
    };

    let settings = app.state::<SettingsState>();
    let bot_id = {
        let cfg = settings.read().await;
        cfg.telegram_bots
            .iter()
            .find(|b| b.label == bot_label)
            .map(|b| b.id.clone())
    };

    match bot_id {
        Some(bot_id) => {
            if let Err(e) =
                crate::commands::telegram::attach_telegram_bot_by_id(app, session_id, &bot_id).await
            {
                log::warn!(
                    "[telegram] Failed to auto-attach configured bot '{}' for session {}: {}",
                    bot_label,
                    session_id,
                    e
                );
            }
        }
        None => {
            log::warn!(
                "[telegram] Configured bot label '{}' for session {} no longer exists in settings; leaving bridge OFF",
                bot_label,
                session_id
            );
        }
    }
}

/// Core session destruction logic shared by the Tauri command and the MailboxPoller.
/// Kills PTY, detaches Telegram bridge, removes from SessionManager, persists, and emits events.
pub async fn destroy_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
) -> Result<(), String> {
    destroy_session_inner_with_options(app, uuid, false).await
}

pub(crate) async fn force_destroy_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
) -> Result<(), String> {
    destroy_session_inner_with_options(app, uuid, true).await
}

async fn destroy_session_inner_with_options<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
    force_destroy_root: bool,
) -> Result<(), String> {
    let id = uuid.to_string();
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let existing = mgr
        .get_session(uuid)
        .await
        .ok_or_else(|| "Session not found".to_string())?;
    let is_root_agent = existing.is_root_agent
        || crate::config::root_agent::is_root_agent_path(&existing.working_directory);
    let was_active = matches!(existing.status, SessionStatus::Active);

    // Remove from detached set
    {
        let detached = app.state::<DetachedSessionsState>();
        let mut detached_set = detached.lock().unwrap();
        detached_set.remove(&uuid);
    }

    // Auto-detach Telegram bridge if active
    let mut bridge_shutdown = None;
    {
        let tg_mgr = app.state::<TelegramBridgeState>();
        let mut tg = tg_mgr.lock().await;
        if tg.has_bridge(uuid) {
            bridge_shutdown = tg.detach(uuid).ok();
            mgr.set_telegram_bot_id(uuid, None).await;
            let _ = app.emit(
                "telegram_bridge_detached",
                serde_json::json!({ "sessionId": id }),
            );
        }
    }
    if let Some(shutdown) = bridge_shutdown.take() {
        shutdown.spawn_wait_or_abort();
    }

    {
        let resource_monitor = app.state::<Arc<ResourceMonitorState>>().inner().clone();
        if resource_monitor.has_registered_group(uuid) {
            let cleanup_started = std::time::Instant::now();
            let monitor = Arc::clone(&resource_monitor);
            let result = tokio::task::spawn_blocking(move || {
                monitor.kill_group(
                    uuid,
                    crate::resource_monitor::ResourceKillReason::SessionDestroy,
                )
            })
            .await
            .map_err(|e| e.to_string())??;
            log::info!(
                "[session] destroy resource cleanup for {} took {}ms (quarantined={})",
                id,
                cleanup_started.elapsed().as_millis(),
                result.quarantined
            );
            if result.quarantined {
                log::warn!(
                    "[session] Resource cleanup for {} quarantined: {}",
                    id,
                    result.message
                );
            }
        }
    }

    // Kill the PTY first
    {
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        pty_mgr
            .lock()
            .unwrap()
            .kill(uuid)
            .map_err(|e| e.to_string())?;
    }

    if is_root_agent && !force_destroy_root {
        mgr.set_is_root_agent(uuid, true).await;
        let cleared_raise_hand = mgr.mark_exited(uuid, 0).await;
        if cleared_raise_hand {
            let _ = app.emit(
                "session_communication_changed",
                serde_json::json!({
                    "sessionId": uuid.to_string(),
                    "communication": null,
                }),
            );
        }
        mgr.clear_active_if(uuid).await;
        let dormant_info = mgr.get_session(uuid).await.map(|s| SessionInfo::from(&s));

        let _ = app.emit("session_destroyed", serde_json::json!({ "id": id }));
        if let Some(info) = dormant_info {
            let _ = app.emit("session_created", info);
        }

        let detached_label = format!("terminal-{}", id.replace('-', ""));
        if let Some(detached_win) = app.get_webview_window(&detached_label) {
            let _ = detached_win.destroy();
        }

        if was_active {
            let sessions = mgr.list_sessions().await;
            let fallback = {
                let detached = app.state::<DetachedSessionsState>();
                let set = detached.lock().unwrap();
                sessions.iter().find_map(|s| {
                    if s.id == id || matches!(s.status, SessionStatus::Exited(_)) {
                        return None;
                    }
                    Uuid::parse_str(&s.id).ok().filter(|u| !set.contains(u))
                })
            };
            if let Some(fb) = fallback {
                let _ = mgr.switch_session(fb).await;
                let _ = app.emit(
                    "session_switched",
                    serde_json::json!({ "id": fb.to_string() }),
                );
            } else {
                mgr.clear_active().await;
                let _ = app.emit(
                    "session_switched",
                    serde_json::json!({ "id": serde_json::Value::Null }),
                );
            }
        }

        persist_current_state(&mgr).await;

        return Ok(());
    }

    let new_active = mgr.destroy_session(uuid).await.map_err(|e| e.to_string())?;

    // Persist after destruction
    persist_current_state(&mgr).await;

    let _ = app.emit("session_destroyed", serde_json::json!({ "id": id }));

    // Close any detached terminal window for this session.
    // R.2: `destroy()` — not `close()` — so the Phase 2 `onCloseRequested` handler
    // on the detached window is bypassed. Triggering the handler here would call
    // `attach_terminal` on a session that's been destroyed (benign no-op per
    // A2.2.G5) but emits extra window-lifecycle noise for no gain.
    let detached_label = format!("terminal-{}", id.replace('-', ""));
    if let Some(detached_win) = app.get_webview_window(&detached_label) {
        let _ = detached_win.destroy();
    }

    // If a new session was auto-activated, emit switch event.
    // Plan §A2.2.G2: the manager's `order.first()` choice is unaware of
    // `DetachedSessionsState`; if the next-active is a detached session, emitting
    // its id to main would cause main + the detached window to both own an xterm
    // for the same session (duplicate display + keystroke routing ambiguity). Filter
    // here — if detached, walk the list for the first non-detached session instead.
    if let Some(new_id) = new_active {
        let is_detached = {
            let detached = app.state::<DetachedSessionsState>();
            let set = detached.lock().unwrap();
            set.contains(&new_id)
        };
        if is_detached {
            let sessions = mgr.list_sessions().await;
            let fallback = {
                let detached = app.state::<DetachedSessionsState>();
                let set = detached.lock().unwrap();
                sessions
                    .iter()
                    .find_map(|s| Uuid::parse_str(&s.id).ok().filter(|u| !set.contains(u)))
            };
            if let Some(fb) = fallback {
                let _ = mgr.switch_session(fb).await;
                let _ = app.emit(
                    "session_switched",
                    serde_json::json!({ "id": fb.to_string() }),
                );
            } else {
                mgr.clear_active().await;
                let _ = app.emit(
                    "session_switched",
                    serde_json::json!({ "id": serde_json::Value::Null }),
                );
            }
        } else {
            let _ = app.emit(
                "session_switched",
                serde_json::json!({ "id": new_id.to_string() }),
            );
        }
    }

    // 0.8.0: removed the "Hide the terminal window when no sessions remain" branch.
    // Under the unified-window model the main window stays visible (sidebar remains
    // usable for creating/opening sessions); the embedded terminal pane shows an
    // empty-state placeholder when no active session exists.

    Ok(())
}

#[tauri::command]
pub async fn destroy_session(
    app: AppHandle,
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    _pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    _detached: State<'_, DetachedSessionsState>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    destroy_session_inner(&app, uuid).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorCloseOutcome {
    /// true if the close was performed; false if confirmation is required first.
    pub closed: bool,
    /// number of live, working (not waiting_for_input) team members; only
    /// meaningful when `closed == false`.
    pub working_count: usize,
}

/// #588 Count LIVE team members that are working (not waiting_for_input). Pure so
/// the working-gate rule is unit-testable without a live app, mirroring how
/// `auto_close.rs` extracts `team_is_closeable`. A member counts only when it is
/// both live (PTY-backed) and not waiting for input.
fn count_working_members(members: &[(bool /*live*/, bool /*waiting_for_input*/)]) -> usize {
    members
        .iter()
        .filter(|(live, waiting)| *live && !*waiting)
        .count()
}

/// #588 Manually close a coordinator (and, when the cascade setting is on, its
/// team). Sets the MANUALLY-CLOSED marker on the coordinator regardless of the
/// cascade setting. When cascade is on and confirmation has not been given and at
/// least one team member is working, closes NOTHING and reports `closed:false` so
/// the FE can show the confirmation modal.
#[tauri::command]
pub async fn close_coordinator(
    app: AppHandle,
    id: String,
    confirmed: bool,
) -> Result<CoordinatorCloseOutcome, String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();

    // Snapshot coordinator facts BEFORE destroying (destroy removes the record).
    let (is_coordinator, cwd) = {
        let mgr = session_mgr.read().await;
        let s = mgr
            .get_session(uuid)
            .await
            .ok_or_else(|| "Session not found".to_string())?;
        (s.is_coordinator, s.working_directory.clone())
    };

    // Defensive: a non-coordinator target is a plain destroy (no marker/cascade).
    if !is_coordinator {
        destroy_session_inner(&app, uuid).await?;
        return Ok(CoordinatorCloseOutcome {
            closed: true,
            working_count: 0,
        });
    }

    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let team_key = match fqn.rsplit_once('/') {
        Some((team, _agent)) => team.to_string(),
        None => String::new(),
    };

    // E0716: bind the State to a local FIRST (matches auto_close.rs:115). The
    // guard-bound-then-used-next-line form does NOT compile (the temporary State is
    // dropped at the `;`, so the read guard dangles). `bool` is Copy.
    let settings = app.state::<SettingsState>();
    let cascade = settings.read().await.coordinator_cascade_close_enabled;
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

    // LIVE-PTY members of THIS team, excluding the coordinator (decision 1; mirrors
    // auto_close.rs:136-142). agent_team_members() includes EXITED/dormant records
    // (exited sessions persist until destroy), so we filter on has_session to a live
    // set ONCE and share it between the working-count gate and the destroy loop;
    // dormant rows are never reaped. The std Mutex guard is taken AFTER the tokio
    // read await and dropped at the block end, so no lock is held across an await.
    let live_ids: Vec<Uuid> = {
        let mgr = session_mgr.read().await;
        let members = mgr.agent_team_members().await;
        let pm = pty_mgr.lock().unwrap_or_else(|e| e.into_inner());
        members
            .into_iter()
            .filter(|(mid, tk)| {
                *mid != uuid && !team_key.is_empty() && *tk == team_key && pm.has_session(*mid)
            })
            .map(|(mid, _)| mid)
            .collect()
    };

    // Confirmation gate: cascade ON, not yet confirmed, >=1 LIVE member working.
    // Only the tokio read guard is held across the get_session awaits.
    if cascade && !confirmed {
        let mgr = session_mgr.read().await;
        let mut member_states: Vec<(bool, bool)> = Vec::with_capacity(live_ids.len());
        for mid in &live_ids {
            // live_ids is pre-filtered to live PTYs, so `live` is true; the waiting
            // flag is the authoritative session record (absent -> treat as idle so a
            // vanished member never forces a modal).
            let waiting = mgr
                .get_session(*mid)
                .await
                .map(|s| s.waiting_for_input)
                .unwrap_or(true);
            member_states.push((true, waiting));
        }
        let working_count = count_working_members(&member_states);
        if working_count > 0 {
            return Ok(CoordinatorCloseOutcome {
                closed: false,
                working_count,
            });
        }
    }

    // Perform the close. Live members first (avoids intermediate auto-activation
    // churn), coordinator last. Mirrors the auto-close destroy loop.
    if cascade {
        for mid in live_ids {
            if let Err(e) = destroy_session_inner(&app, mid).await {
                log::warn!(
                    "[manual-close] member destroy {} failed: {}",
                    &mid.to_string()[..8],
                    e
                );
            }
        }
    }
    destroy_session_inner(&app, uuid).await?;

    // Set the manual marker on the coordinator FQN and persist immediately (this
    // command is not on the auto-close tick, so flush_clocks won't run).
    if let Some(clocks) = app.try_state::<CoordinatorClocksState>() {
        let now = chrono::Utc::now();
        let newly = clocks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mark_manually_closed(&fqn, now);
        if newly {
            let _ = app.emit(
                "coordinator_manual_close_changed",
                serde_json::json!({ "replicaPath": cwd, "manuallyClosedAt": now.to_rfc3339() }),
            );
            // also clear any stale auto-closed pill on the FE (mark_* cleared it
            // server-side via the mutual-exclusion transition).
            let _ = app.emit(
                "coordinator_auto_close_changed",
                serde_json::json!({ "replicaPath": cwd, "autoClosedAt": null }),
            );
        }
        let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
            log::warn!("[manual-close] clocks save failed: {}", e);
        }
    }

    Ok(CoordinatorCloseOutcome {
        closed: true,
        working_count: 0,
    })
}

/// Resolves the effective `skip_auto_resume` flag for `restart_session`.
/// Defaults to `true` (fresh conversation) to preserve existing restart-button semantics.
/// `Some(false)` is used by the deferred-wake path (ProjectPanel.handleReplicaClick)
/// to allow provider auto-resume and continue the prior conversation.
fn effective_restart_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
}

/// (#630/#631) Compose the restart path's effective fresh intent: fresh on an
/// explicit "Restart Session" (`requested` defaults to `true`), OR if the session
/// still carries an unconsumed durable fresh intent (`stored_start_fresh`). A
/// deferred member reopened via Branch A passes `Some(false)`, but its persisted
/// intent wins, so it stays fresh; a normal member (`false || false`) resumes,
/// unchanged. Named so the composition is unit-tested; the call site is guarded
/// against a silent revert by the `dead_code` lint under `-D warnings` (CI runs
/// `cargo clippy --all-targets -- -D warnings`), which fails the build if
/// reverting to the inline expression leaves this function unused. Mirrors
/// `lib::skip_auto_resume_for_restore`.
fn restart_skip_auto_resume_with_intent(stored_start_fresh: bool, requested: Option<bool>) -> bool {
    stored_start_fresh || effective_restart_skip_auto_resume(requested)
}

/// (#599) Resolves the effective `skip_auto_resume` for the `create_session`
/// command. Defaults to `true` (fresh conversation) so every existing
/// create-in-place / new-agent / open-agent / CLI / web call site keeps its
/// intentional "no --continue on a fresh create" semantics. The
/// reopen-of-a-closed-coordinator path (ProjectPanel.handleReplicaClick) passes
/// `Some(false)` to resume the prior conversation. Mirror of
/// `effective_restart_skip_auto_resume` so both create and restart routes share
/// the same default.
fn effective_create_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
}

fn effective_restart_requested_profile(
    requested: Option<String>,
    stored: Option<String>,
) -> Option<String> {
    requested.or(stored)
}

/// #537 read-side: pick which coding agent a restart should launch.
///
/// Ranks an explicit request first, then the replica's Selection-UI assignment
/// (`tooling.currentCodingAgent` in `cwd/config.json`, validated against the
/// configured agents), then the agent the live session was launched with. This
/// mirrors the wake path (#534): a fresh user selection must override stale
/// launch history. The `currentCodingAgent` is validated so an unconfigured id
/// is never passed downstream (which would make `build_configured_agent_spawn_for_cwd`
/// return `Ok(None)` and silently keep the old recipe). Root agents and plain
/// terminals have no `currentCodingAgent`, so they fall straight through to
/// `stored_agent_id` and their behavior is unchanged.
fn resolve_restart_selected_agent_id(
    settings: &AppSettings,
    cwd: &str,
    requested_agent_id: Option<&str>,
    stored_agent_id: Option<&str>,
) -> Option<String> {
    if let Some(requested) = requested_agent_id {
        return Some(requested.to_string());
    }
    let current_selection =
        crate::config::coding_agent_profiles::read_replica_current_coding_agent(
            std::path::Path::new(cwd),
        )
        .filter(|id| settings.agents.iter().any(|agent| agent.id == *id));
    current_selection.or_else(|| stored_agent_id.map(str::to_string))
}

/// Restart a session: destroy the existing one and recreate it with the same
/// configuration but a fresh PTY. By default suppresses provider auto-resume
/// (true user-intent restart — fresh conversation).
///
/// Callers that are instead *waking* a previously-deferred session pass
/// `skip_auto_resume = Some(false)`. A session is "deferred" (PTY `Exited(0)`
/// at startup) for any of the following reasons:
///
///   1. `restoreCoordinatorWakeState = false` (the new-policy default per
///      issue #248) defers every persisted session regardless of agent type.
///   2. `restoreCoordinatorWakeState = true` defers all non-coordinator team
///      members (these are never auto-woken on startup under the new policy).
///   3. `restoreCoordinatorWakeState = true` defers coordinators whose PTY
///      status was `Exited(_)` at the prior shutdown (the state-sensitive
///      branch added by issue #248).
///   4. The user explicitly closed the session during the prior run.
///
/// In all four cases, `skip_auto_resume = Some(false)` allows the next PTY
/// spawn to inject `claude --continue`, `codex resume --last`, or
/// `gemini --resume latest` so the prior conversation continues.
///
/// The restarted session is automatically activated, Telegram bridges are
/// re-attached, and state is persisted.
#[allow(clippy::too_many_arguments)]
pub async fn restart_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
    restart_session_inner_with_activation(
        app,
        session_mgr,
        pty_mgr,
        settings,
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn restart_session_inner_with_activation<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
    activate_after: bool,
) -> Result<SessionInfo, String> {
    // 1. Read config from existing session BEFORE destroying it
    let (
        shell,
        shell_args,
        cwd,
        name,
        stored_agent_id,
        stored_agent_label,
        git_repos,
        is_root_agent,
        telegram_bot_id,
        stored_requested_profile,
        stored_start_fresh,
    ) = {
        let mgr = session_mgr.read().await;
        let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
        (
            session.shell.clone(),
            session.shell_args.clone(),
            session.working_directory.clone(),
            session.name.clone(),
            session.agent_id.clone(),
            session.agent_label.clone(),
            session.git_repos.clone(),
            session.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&session.working_directory),
            session.telegram_bot_id.clone(),
            session.requested_profile.clone(),
            session.start_fresh_on_restore, // (#630/#631) honor an unconsumed durable fresh intent
        )
    };

    let _root_guard = if is_root_agent {
        Some(root_agent_session_lock().lock().await)
    } else {
        None
    };
    let cwd = if is_root_agent {
        crate::config::root_agent::ensure_root_agent_dir()?
    } else {
        cwd
    };

    // 2. Strip auto-injected args before restart so the new session starts from the saved recipe.
    let clean_args =
        crate::config::sessions_persistence::strip_auto_injected_args(&shell, &shell_args);

    let requested_agent_id = agent_id;
    let selected_requested_profile =
        effective_restart_requested_profile(requested_profile, stored_requested_profile);
    // #537 read-side: resolve the launch agent (honoring currentCodingAgent) and
    // build its spawn under a single settings read guard. No await is held across
    // the guard; it is dropped at the end of this block.
    let (selected_agent_id, resolved_spawn) = {
        let cfg = settings.read().await;
        let selected_agent_id = resolve_restart_selected_agent_id(
            &cfg,
            &cwd,
            requested_agent_id.as_deref(),
            stored_agent_id.as_deref(),
        );
        let resolved_spawn = if let Some(ref aid) = selected_agent_id {
            build_configured_agent_spawn_for_cwd(
                &cfg,
                aid,
                &cwd,
                selected_requested_profile.as_deref(),
            )?
        } else {
            None
        };
        (selected_agent_id, resolved_spawn)
    };
    let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
        (
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            Some(spawn.trusted_agent_label.clone()),
        )
    } else {
        (shell, clean_args, stored_agent_label)
    };

    // 3. Destroy the old session (resolves all State<> internally from app)
    if is_root_agent {
        force_destroy_session_inner(app, uuid).await?;
    } else {
        destroy_session_inner(app, uuid).await?;
    }

    // (#630/#631) Fresh on an explicit "Restart Session" (None -> true), OR if the
    // session still carries an unconsumed durable fresh intent. Root agents are
    // scoped out (separate restore path ignores the marker, §8): they are never
    // stamped below, so `stored_start_fresh` is false for root and this reduces to
    // today's `effective_restart_skip_auto_resume`.
    let restart_start_fresh =
        restart_skip_auto_resume_with_intent(stored_start_fresh, skip_auto_resume);

    // 4. Create new session with same config, or switch to the selected coding agent.
    let session_info = create_session_inner(
        app,
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd.clone(),
        Some(name),
        selected_agent_id,
        agent_label,
        false, // skip_tooling_save
        git_repos,
        restart_start_fresh,
        resolved_spawn,
    )
    .await?;

    let new_uuid = Uuid::parse_str(&session_info.id).map_err(|e| e.to_string())?;

    // (#630/#631) Stamp the honored fresh intent onto the recreated session so it
    // survives the next app restart. Skipped for root agents (scoped out, §8): the
    // root restore path ignores the marker, so we do not persist a field we ignore.
    // The `persist_current_state` in step 7 below writes it durably.
    if !is_root_agent {
        let mgr = session_mgr.read().await;
        mgr.set_start_fresh_on_restore(new_uuid, restart_start_fresh)
            .await;
    }

    if activate_after {
        // 5. Explicitly activate the new session.
        //    destroy_session_inner may have auto-activated a sibling.
        //    create_session_inner only auto-activates if active.is_none().
        //    With multiple sessions, the new session would NOT be active without this.
        {
            let mgr = session_mgr.read().await;
            let _ = mgr.switch_session(new_uuid).await;
        }
        let _ = app.emit(
            "session_switched",
            serde_json::json!({ "id": session_info.id, "userInitiated": true }),
        );
    }

    // 6. Re-attach Telegram bridge from live persisted intent, or fall back to repo config.
    if telegram_bot_id.is_some() {
        attach_persisted_telegram_if_configured(app, new_uuid, telegram_bot_id.as_deref()).await;
    } else {
        attach_local_config_telegram_if_any(app, new_uuid, &cwd).await;
    }

    // 7. Persist state — create_session_inner does NOT persist
    {
        let mgr = session_mgr.read().await;
        persist_current_state(&mgr).await;
    }

    Ok(session_info)
}

// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn restart_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    id: String,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    restart_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        settings.inner(),
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
    )
    .await
}

#[tauri::command]
pub async fn switch_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    detached: State<'_, DetachedSessionsState>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    // If this session is detached, focus its window instead of switching the main terminal
    let is_detached = {
        let detached_set = detached.lock().unwrap();
        detached_set.contains(&uuid)
    };
    if is_detached {
        let mgr = session_mgr.read().await;
        mgr.clear_active_if(uuid).await;
        let label = format!("terminal-{}", id.replace('-', ""));
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.set_focus();
        }
        return Ok(());
    }

    let mgr = session_mgr.read().await;
    mgr.switch_session(uuid).await.map_err(|e| e.to_string())?;

    // Persist after switch (updates was_active)
    persist_current_state(&mgr).await;

    let _ = app.emit(
        "session_switched",
        serde_json::json!({ "id": id, "userInitiated": true }),
    );

    Ok(())
}

#[tauri::command]
pub async fn rename_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
    name: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    let mgr = session_mgr.read().await;
    mgr.rename_session(uuid, name.clone())
        .await
        .map_err(|e| e.to_string())?;

    // Persist after rename
    persist_current_state(&mgr).await;

    let _ = app.emit(
        "session_renamed",
        serde_json::json!({ "id": id, "name": name }),
    );

    Ok(())
}

#[tauri::command]
pub async fn list_sessions(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
) -> Result<Vec<SessionInfo>, String> {
    let mut infos = { session_mgr.read().await.list_sessions().await };
    // #592 - recompute drift per session against current settings (settings-aware,
    // unlike the `From<&Session>` path which always emits `profile_outdated=false`).
    let cfg = settings.read().await;
    for info in infos.iter_mut() {
        info.profile_outdated = compute_profile_outdated(&cfg, info);
    }
    Ok(infos)
}

#[tauri::command]
pub async fn set_last_prompt(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
    text: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let mgr = session_mgr.read().await;
    mgr.set_last_prompt(uuid, text.clone()).await;
    crate::config::sessions_persistence::persist_current_state(&mgr).await;
    let _ = app.emit(
        "last_prompt",
        serde_json::json!({ "sessionId": id, "text": text }),
    );
    Ok(())
}

/// Extract the basename (without extension) from a path or command token.
pub(crate) fn executable_basename(s: &str) -> String {
    std::path::Path::new(s)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(s)
        .to_lowercase()
}

type ResolvedRootAgentCommand = (String, Vec<String>, Option<String>, Option<String>);

#[cfg(test)]
fn resolve_agent_command(
    agent_id: &str,
    settings: &AppSettings,
) -> Result<(String, Vec<String>, Option<String>), String> {
    if let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) {
        log::info!(
            "[session] Agent resolved: id={:?}, label={:?}, command={:?}",
            agent.id,
            agent.label,
            agent.command
        );
        let source = format!("selected agent '{}'", agent_id);
        let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
        Ok((shell, shell_args, Some(agent.label.clone())))
    } else {
        log::warn!(
            "[session] Agent NOT found for aid={:?}. Falling back to default shell.",
            agent_id
        );
        Ok((
            settings.default_shell.clone(),
            settings.default_shell_args.clone(),
            None,
        ))
    }
}

fn normalize_agent_command_for_source(
    agent: &crate::config::settings::AgentConfig,
    source: &str,
) -> Result<(String, Vec<String>), String> {
    crate::config::agent_command::normalize_legacy_agent_command(&agent.command)
        .map(|cmd| (cmd.shell, cmd.shell_args))
        .map_err(|e| {
            format!(
                "Invalid agent command from {} (agent id '{}', label '{}'): {}. command={:?}",
                source, agent.id, agent.label, e, agent.command
            )
        })
}

pub(crate) fn resolve_root_agent_command(
    settings: &AppSettings,
    requested_agent_id: Option<&str>,
    last_coding_agent: Option<&str>,
) -> Result<ResolvedRootAgentCommand, String> {
    let resolve_configured =
        |agent_id: &str| settings.agents.iter().find(|agent| agent.id == agent_id);

    if let Some(agent_id) = requested_agent_id {
        if let Some(agent) = resolve_configured(agent_id) {
            let source = format!("requested root agent '{}'", agent_id);
            let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
            return Ok((
                shell,
                shell_args,
                Some(agent.id.clone()),
                Some(agent.label.clone()),
            ));
        } else {
            log::warn!(
                "[root-agent] Requested coding agent '{}' no longer exists; falling back",
                agent_id
            );
        }
    }

    if let Some(agent_id) = last_coding_agent {
        if let Some(agent) = resolve_configured(agent_id) {
            let source = format!("root lastCodingAgent '{}'", agent_id);
            let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
            return Ok((
                shell,
                shell_args,
                Some(agent.id.clone()),
                Some(agent.label.clone()),
            ));
        } else {
            log::warn!(
                "[root-agent] lastCodingAgent '{}' no longer exists; falling back",
                agent_id
            );
        }
    }

    if let Some(agent) = settings.agents.first() {
        let source = format!("first configured root agent '{}'", agent.id);
        let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
        return Ok((
            shell,
            shell_args,
            Some(agent.id.clone()),
            Some(agent.label.clone()),
        ));
    }

    Err("No resolvable coding agent is configured for the Root Agent. Configure a coding agent before launching the Root Agent.".to_string())
}

fn resolve_agent_label(agent_id: &str, settings: &AppSettings) -> Option<String> {
    settings
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .map(|a| a.label.clone())
}

fn resolve_actual_agent(
    shell: &str,
    shell_args: &[String],
    requested_agent_id: Option<&str>,
    requested_agent_label: Option<&str>,
    settings: &AppSettings,
) -> (Option<String>, Option<String>) {
    if let Some(agent_id) = requested_agent_id {
        if let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) {
            match crate::config::agent_command::normalize_legacy_agent_command(&agent.command) {
                Ok(normalized)
                    if normalized.shell == shell && normalized.shell_args == shell_args =>
                {
                    return (
                        Some(agent.id.clone()),
                        requested_agent_label
                            .map(ToString::to_string)
                            .or_else(|| Some(agent.label.clone())),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    log::debug!(
                        "[session] Requested agent_id='{}' command did not normalize during metadata resolution: {}",
                        agent_id,
                        e
                    );
                }
            }
        }
    }

    let detected = resolve_agent_from_shell(shell, shell_args, settings);

    if let Some(agent_id) = requested_agent_id {
        match detected.0.as_deref() {
            Some(detected_id) if detected_id == agent_id => {
                return (
                    detected.0,
                    requested_agent_label
                        .map(ToString::to_string)
                        .or(detected.1),
                )
            }
            Some(detected_id) => {
                log::warn!(
                    "[session] Requested agent_id='{}' did not match final shell-resolved agent '{}'; storing resolved agent instead",
                    agent_id,
                    detected_id
                );
                return detected;
            }
            None => {
                log::warn!(
                    "[session] Requested agent_id='{}' did not validate against final launched shell; clearing actual agent metadata",
                    agent_id
                );
                return (None, None);
            }
        }
    }

    detected
}

/// Try to match the shell command against configured agents in settings.
/// Returns (Some(agent_id), Some(label)) if a match is found, (None, None) otherwise.
fn resolve_agent_from_shell(
    shell: &str,
    shell_args: &[String],
    settings: &AppSettings,
) -> (Option<String>, Option<String>) {
    // Compare against already-normalized launch tokens without re-splitting shell.
    let mut cmd_basenames: Vec<String> = Vec::with_capacity(shell_args.len() + 1);
    cmd_basenames.push(executable_basename(shell));
    cmd_basenames.extend(shell_args.iter().map(|arg| executable_basename(arg)));

    for agent in &settings.agents {
        let normalized = match crate::config::agent_command::normalize_legacy_agent_command(
            &agent.command,
        ) {
            Ok(cmd) => cmd,
            Err(e) => {
                log::debug!(
                    "[session] Skipping agent '{}' during shell auto-detection because its command is invalid: {}",
                    agent.id,
                    e
                );
                continue;
            }
        };
        let agent_basename = executable_basename(&normalized.shell);
        if !agent_basename.is_empty() && cmd_basenames.contains(&agent_basename) {
            log::info!(
                "Auto-detected agent '{}' ({}) from shell command",
                agent.id,
                agent.label
            );
            return (Some(agent.id.clone()), Some(agent.label.clone()));
        }
    }
    (None, None)
}

#[tauri::command]
pub async fn get_active_session(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    detached: State<'_, DetachedSessionsState>,
) -> Result<Option<String>, String> {
    let mgr = session_mgr.read().await;
    let Some(active_id) = mgr.get_active().await else {
        return Ok(None);
    };
    let is_detached = {
        let set = detached.lock().unwrap();
        set.contains(&active_id)
    };
    if is_detached {
        mgr.clear_active_if(active_id).await;
        return Ok(None);
    }
    Ok(Some(active_id.to_string()))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_root_agent_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    _tg_mgr: &TelegramBridgeState,
    settings: &SettingsState,
    requested_agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume_for_new_session: bool,
) -> Result<SessionInfo, String> {
    let _guard = root_agent_session_lock().lock().await;
    let root_agent_path = crate::config::root_agent::ensure_root_agent_dir()?;

    let existing = {
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        sessions.into_iter().find(|s| {
            s.is_root_agent || crate::config::root_agent::is_root_agent_path(&s.working_directory)
        })
    };
    let mut waking_existing = false;
    let mut restored_telegram_bot_id: Option<String> = None;
    let last_coding_agent = crate::config::root_agent::read_last_coding_agent(&root_agent_path);
    let mut resolved_root_agent_command: Option<ResolvedRootAgentCommand> = None;

    if let Some(existing) = existing {
        let uuid = Uuid::parse_str(&existing.id).map_err(|e| e.to_string())?;
        {
            let mgr = session_mgr.read().await;
            mgr.set_is_root_agent(uuid, true).await;
        }

        let has_pty = pty_mgr.lock().unwrap().has_session(uuid);
        match classify_existing_root(&existing.status, has_pty) {
            ExistingRootAction::ReuseLive => {
                log::info!(
                    "[root-agent] Reusing existing live session {} at {}",
                    existing.id,
                    existing.working_directory
                );
                let mgr = session_mgr.read().await;
                if let Some(updated) = mgr.get_session(uuid).await {
                    persist_current_state(&mgr).await;
                    return Ok(SessionInfo::from(&updated));
                }
                return Ok(existing);
            }
            ExistingRootAction::WakeDormant => {
                resolved_root_agent_command = Some({
                    let cfg = settings.read().await;
                    resolve_root_agent_command(
                        &cfg,
                        requested_agent_id.as_deref(),
                        last_coding_agent.as_deref(),
                    )?
                });
                waking_existing = true;
                restored_telegram_bot_id = existing.telegram_bot_id.clone();
                log::info!(
                    "[root-agent] Waking dormant root session {} with provider resume",
                    existing.id
                );
                force_destroy_session_inner(app, uuid).await?;
            }
            ExistingRootAction::DiscardMissingPty => {
                resolved_root_agent_command = Some({
                    let cfg = settings.read().await;
                    resolve_root_agent_command(
                        &cfg,
                        requested_agent_id.as_deref(),
                        last_coding_agent.as_deref(),
                    )?
                });
                log::warn!(
                    "[root-agent] Discarding root session {} because it has status {:?} but no PTY",
                    existing.id,
                    existing.status
                );
                force_destroy_session_inner(app, uuid).await?;
            }
        }
    }

    let (shell, shell_args, agent_id, agent_label) =
        if let Some(resolved) = resolved_root_agent_command {
            resolved
        } else {
            let cfg = settings.read().await;
            resolve_root_agent_command(
                &cfg,
                requested_agent_id.as_deref(),
                last_coding_agent.as_deref(),
            )?
        };
    let resolved_spawn = if let Some(aid) = agent_id.as_deref() {
        let cfg = settings.read().await;
        build_configured_agent_spawn_for_cwd(
            &cfg,
            aid,
            &root_agent_path,
            requested_profile.as_deref(),
        )?
    } else {
        None
    };
    let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
        (
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            Some(spawn.trusted_agent_label.clone()),
        )
    } else {
        (shell, shell_args, agent_label)
    };

    let info = create_session_inner(
        app,
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        root_agent_path.clone(),
        Some(crate::config::root_agent::ROOT_AGENT_SESSION_NAME.to_string()),
        agent_id,
        agent_label,
        false,
        Vec::new(),
        if waking_existing {
            false
        } else {
            skip_auto_resume_for_new_session
        },
        resolved_spawn,
    )
    .await?;

    {
        let mgr = session_mgr.read().await;
        persist_current_state(&mgr).await;
    }

    let id = Uuid::parse_str(&info.id).map_err(|e| format!("Invalid session UUID: {}", e))?;
    if restored_telegram_bot_id.is_some() {
        attach_persisted_telegram_if_configured(app, id, restored_telegram_bot_id.as_deref()).await;
    } else {
        attach_local_config_telegram_if_any(app, id, &root_agent_path).await;
    }

    Ok(info)
}

/// Create, wake, or reuse a root agent session.
#[tauri::command]
pub async fn create_root_agent_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
) -> Result<SessionInfo, String> {
    create_root_agent_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        tg_mgr.inner(),
        settings.inner(),
        agent_id,
        requested_profile,
        true, // skip_auto_resume = true → fresh create, no `--continue` injection
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        classify_existing_root, compute_profile_outdated, count_working_members,
        effective_restart_requested_profile, inject_codex_resume, resolve_actual_agent,
        resolve_agent_command, resolve_agent_from_shell, resolve_restart_selected_agent_id,
        resolve_root_agent_command, should_inject_continue, ExistingRootAction,
    };
    use crate::config::settings::{AgentConfig, AppSettings, ProfileCellConfig};
    use crate::session::manager::SessionManager;
    use crate::session::session::{SessionInfo, SessionStatus};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn count_working_members_counts_live_and_busy_only() {
        // tuples are (live /*PTY-backed*/, waiting_for_input)
        assert_eq!(count_working_members(&[]), 0, "empty -> 0");
        assert_eq!(
            count_working_members(&[(true, true), (true, true)]),
            0,
            "all live but idle (waiting) -> 0"
        );
        assert_eq!(
            count_working_members(&[(true, false), (true, false), (true, false)]),
            3,
            "all live and busy -> N"
        );
        assert_eq!(
            count_working_members(&[(true, true), (true, false)]),
            1,
            "only the live+busy member counts"
        );
        assert_eq!(
            count_working_members(&[(false, false), (false, false)]),
            0,
            "busy but dead (no live PTY) -> 0"
        );
        assert_eq!(
            count_working_members(&[(true, false), (false, false), (true, true)]),
            1,
            "mixed: one live+busy, one dead+busy, one live+idle -> 1"
        );
    }

    fn test_settings() -> AppSettings {
        AppSettings {
            agents: vec![
                AgentConfig {
                    id: "claude".to_string(),
                    label: "Claude Code".to_string(),
                    command: "claude".to_string(),
                    color: "#d97706".to_string(),
                    envs: Vec::new(),
                    isolated_home: false,
                    instructions_filename: None,
                    config_seed: None,
                },
                AgentConfig {
                    id: "codex".to_string(),
                    label: "Codex".to_string(),
                    command: "codex".to_string(),
                    color: "#10b981".to_string(),
                    envs: Vec::new(),
                    isolated_home: false,
                    instructions_filename: None,
                    config_seed: None,
                },
            ],
            ..AppSettings::default()
        }
    }

    #[test]
    fn resolve_root_agent_command_prefers_valid_explicit_agent() {
        let settings = test_settings();

        let (shell, args, agent_id, label) =
            resolve_root_agent_command(&settings, Some("codex"), Some("claude")).unwrap();

        assert_eq!(shell, "codex");
        assert!(args.is_empty());
        assert_eq!(agent_id.as_deref(), Some("codex"));
        assert_eq!(label.as_deref(), Some("Codex"));
    }

    #[test]
    fn resolve_root_agent_command_uses_last_coding_agent_when_explicit_missing() {
        let settings = test_settings();

        let (shell, _args, agent_id, label) =
            resolve_root_agent_command(&settings, None, Some("codex")).unwrap();

        assert_eq!(shell, "codex");
        assert_eq!(agent_id.as_deref(), Some("codex"));
        assert_eq!(label.as_deref(), Some("Codex"));
    }

    #[test]
    fn resolve_root_agent_command_falls_back_to_first_configured_agent() {
        let settings = test_settings();

        let (shell, _args, agent_id, label) =
            resolve_root_agent_command(&settings, Some("stale"), Some("also-stale")).unwrap();

        assert_eq!(shell, "claude");
        assert_eq!(agent_id.as_deref(), Some("claude"));
        assert_eq!(label.as_deref(), Some("Claude Code"));
    }

    #[test]
    fn resolve_root_agent_command_rejects_default_shell_without_agents() {
        let mut settings = AppSettings {
            default_shell: "pwsh".to_string(),
            default_shell_args: vec!["-NoLogo".to_string()],
            ..AppSettings::default()
        };
        settings.agents.clear();

        let err =
            resolve_root_agent_command(&settings, Some("stale"), Some("also-stale")).unwrap_err();

        assert!(err.contains("No resolvable coding agent"));
    }

    fn write_current_coding_agent(dir: &std::path::Path, agent_id: &str) {
        std::fs::write(
            dir.join("config.json"),
            format!(r#"{{"tooling":{{"currentCodingAgent":"{}"}}}}"#, agent_id),
        )
        .unwrap();
    }

    // #537 read-side: a freshly assigned currentCodingAgent must win over the
    // agent the live session was launched with (stored_agent_id) when the restart
    // carries no explicit agent (the plain restart button passes None).
    #[test]
    fn restart_honors_current_coding_agent_when_no_explicit_request() {
        let settings = test_settings();
        let tmp = tempfile::tempdir().unwrap();
        write_current_coding_agent(tmp.path(), "codex");

        let selected = resolve_restart_selected_agent_id(
            &settings,
            &tmp.path().to_string_lossy(),
            None,
            Some("claude"),
        );

        assert_eq!(selected.as_deref(), Some("codex"));
    }

    #[test]
    fn restart_explicit_agent_overrides_current_coding_agent() {
        let settings = test_settings();
        let tmp = tempfile::tempdir().unwrap();
        write_current_coding_agent(tmp.path(), "codex");

        let selected = resolve_restart_selected_agent_id(
            &settings,
            &tmp.path().to_string_lossy(),
            Some("claude"),
            Some("codex"),
        );

        assert_eq!(selected.as_deref(), Some("claude"));
    }

    #[test]
    fn restart_ignores_unconfigured_current_coding_agent_and_keeps_stored() {
        let settings = test_settings();
        let tmp = tempfile::tempdir().unwrap();
        write_current_coding_agent(tmp.path(), "ghost");

        let selected = resolve_restart_selected_agent_id(
            &settings,
            &tmp.path().to_string_lossy(),
            None,
            Some("claude"),
        );

        assert_eq!(selected.as_deref(), Some("claude"));
    }

    // Root agents and plain terminals have no currentCodingAgent → behavior
    // unchanged: fall straight through to the stored launch agent.
    #[test]
    fn restart_falls_back_to_stored_when_no_current_coding_agent() {
        let settings = test_settings();
        let tmp = tempfile::tempdir().unwrap();

        let selected = resolve_restart_selected_agent_id(
            &settings,
            &tmp.path().to_string_lossy(),
            None,
            Some("claude"),
        );

        assert_eq!(selected.as_deref(), Some("claude"));
    }

    #[test]
    fn restart_requested_profile_prefers_explicit_then_stored() {
        assert_eq!(
            effective_restart_requested_profile(Some("C".to_string()), Some("B".to_string())),
            Some("C".to_string())
        );
        assert_eq!(
            effective_restart_requested_profile(None, Some("B".to_string())),
            Some("B".to_string())
        );
        assert_eq!(effective_restart_requested_profile(None, None), None);
    }

    #[test]
    fn build_configured_agent_spawn_for_cwd_honors_requested_profile() {
        let temp = tempfile::tempdir().unwrap();
        let mut settings = test_settings();
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "C".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    // #597 - the cell holds params only; they append to the agent
                    // base command (`codex`) to launch `codex --profile-c`.
                    command: "--profile-c".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );

        let cwd = temp.path().to_string_lossy().to_string();
        let spawn =
            super::build_configured_agent_spawn_for_cwd(&settings, "codex", &cwd, Some("C"))
                .unwrap()
                .unwrap();

        assert_eq!(spawn.profile_resolution.requested_profile, "C");
        assert_eq!(spawn.profile_resolution.effective_profile, "C");
        assert_eq!(spawn.shell_args, vec!["--profile-c".to_string()]);
    }

    // #592 - compute_profile_outdated integration: loaded hash vs current config.
    fn make_info(
        cwd: &str,
        agent_id: Option<&str>,
        requested_profile: Option<&str>,
        loaded_hash: Option<String>,
    ) -> SessionInfo {
        SessionInfo {
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            name: "s".to_string(),
            shell: "codex".to_string(),
            shell_args: Vec::new(),
            effective_shell_args: None,
            created_at: "2026-06-21T00:00:00Z".to_string(),
            working_directory: cwd.to_string(),
            status: SessionStatus::Running,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id: agent_id.map(str::to_string),
            agent_label: None,
            git_repos: Vec::new(),
            workgroup_task: None,
            is_coordinator: false,
            is_root_agent: false,
            token: "t".to_string(),
            agent_kind: None,
            requested_profile: requested_profile.map(str::to_string),
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            profile_content_hash: loaded_hash,
            profile_outdated: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
        }
    }

    #[test]
    fn compute_profile_outdated_flips_when_effective_cell_changes() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().to_string();

        let mut settings = test_settings();
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "--v1".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );

        // #597 - the hash the session launched with: agent base + cell A params,
        // composed via the SAME helpers the spawn path uses. The codex borrow ends
        // with this block, before the get_mut mutation below.
        let loaded = {
            let codex = settings.agents.iter().find(|a| a.id == "codex").unwrap();
            crate::config::agent_command::profile_content_hash(
                &crate::config::agent_command::compose_effective_command(&codex.command, "--v1"),
                &crate::config::agent_command::raw_merged_profile_env(codex, &BTreeMap::new()),
            )
        };
        let mut info = make_info(&cwd, Some("codex"), Some("A"), Some(loaded));

        // Config unchanged -> not outdated.
        assert!(!compute_profile_outdated(&settings, &info));

        // Edit the effective cell params -> outdated.
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .get_mut("codex")
            .unwrap()
            .get_mut("A")
            .unwrap()
            .command = "--v2".to_string();
        assert!(compute_profile_outdated(&settings, &info));

        // A plain-shell session (no agent) never drifts.
        info.agent_id = None;
        assert!(!compute_profile_outdated(&settings, &info));
    }

    #[test]
    fn compute_profile_outdated_flips_on_agent_base_command_edit() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().to_string();

        let mut settings = test_settings();
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "--v1".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );

        // #597 - stamp loaded from agent base `codex` + cell `--v1`.
        let loaded = {
            let codex = settings.agents.iter().find(|a| a.id == "codex").unwrap();
            crate::config::agent_command::profile_content_hash(
                &crate::config::agent_command::compose_effective_command(&codex.command, "--v1"),
                &crate::config::agent_command::raw_merged_profile_env(codex, &BTreeMap::new()),
            )
        };
        let info = make_info(&cwd, Some("codex"), Some("A"), Some(loaded));

        // Base unchanged -> not outdated.
        assert!(!compute_profile_outdated(&settings, &info));

        // Edit the agent base command -> outdated (the base is now in the hash).
        settings
            .agents
            .iter_mut()
            .find(|a| a.id == "codex")
            .unwrap()
            .command = "codex-next".into();
        assert!(compute_profile_outdated(&settings, &info));
    }

    #[test]
    fn resolve_agent_command_preserves_args() {
        let mut settings = test_settings();
        settings
            .agents
            .iter_mut()
            .find(|agent| agent.id == "codex")
            .unwrap()
            .command = "codex --yolo".to_string();

        let (shell, args, label) = resolve_agent_command("codex", &settings).unwrap();

        assert_eq!(shell, "codex");
        assert_eq!(args, vec!["--yolo".to_string()]);
        assert_eq!(label.as_deref(), Some("Codex"));
    }

    #[test]
    fn resolve_agent_command_rejects_invalid_selected_command() {
        let mut settings = test_settings();
        settings
            .agents
            .iter_mut()
            .find(|agent| agent.id == "codex")
            .unwrap()
            .command = "codex \"unterminated".to_string();

        let err = resolve_agent_command("codex", &settings).unwrap_err();

        assert!(err.contains("selected agent 'codex'"));
        assert!(err.contains("agent id 'codex'"));
        assert!(err.contains("unclosed double quote"));
    }

    #[test]
    fn resolve_root_agent_command_preserves_args() {
        let mut settings = test_settings();
        settings
            .agents
            .iter_mut()
            .find(|agent| agent.id == "codex")
            .unwrap()
            .command = "codex --yolo".to_string();

        let (shell, args, agent_id, label) =
            resolve_root_agent_command(&settings, Some("codex"), None).unwrap();

        assert_eq!(shell, "codex");
        assert_eq!(args, vec!["--yolo".to_string()]);
        assert_eq!(agent_id.as_deref(), Some("codex"));
        assert_eq!(label.as_deref(), Some("Codex"));
    }

    #[test]
    fn resolve_root_agent_command_rejects_invalid_last_coding_agent_command() {
        let mut settings = test_settings();
        settings
            .agents
            .iter_mut()
            .find(|agent| agent.id == "codex")
            .unwrap()
            .command = "codex \"unterminated".to_string();

        let err = resolve_root_agent_command(&settings, None, Some("codex")).unwrap_err();

        assert!(err.contains("root lastCodingAgent 'codex'"));
        assert!(err.contains("agent id 'codex'"));
        assert!(err.contains("unclosed double quote"));
    }

    #[test]
    fn existing_root_classifier_discards_live_record_without_pty() {
        assert_eq!(
            classify_existing_root(&SessionStatus::Running, false),
            ExistingRootAction::DiscardMissingPty
        );
        assert_eq!(
            classify_existing_root(&SessionStatus::Active, true),
            ExistingRootAction::ReuseLive
        );
        assert_eq!(
            classify_existing_root(&SessionStatus::Exited(0), false),
            ExistingRootAction::WakeDormant
        );
    }

    #[tokio::test]
    async fn dormant_root_command_resolution_failure_preserves_existing_session() {
        let session_mgr = SessionManager::new();
        let settings = AppSettings {
            agents: Vec::new(),
            ..AppSettings::default()
        };

        let dormant = {
            let session = session_mgr
                .create_session(
                    "codex".to_string(),
                    Vec::new(),
                    "C:\\test\\ac-root-agent".to_string(),
                    Some("codex".to_string()),
                    Some("Codex".to_string()),
                    Vec::new(),
                    false,
                )
                .await
                .expect("failed to create dormant root session");
            session_mgr.set_is_root_agent(session.id, true).await;
            session_mgr.mark_exited(session.id, 0).await;
            session
        };

        assert_eq!(
            classify_existing_root(&SessionStatus::Exited(0), false),
            ExistingRootAction::WakeDormant
        );

        let err = resolve_root_agent_command(&settings, Some("stale"), Some("also-stale"))
            .expect_err("replacement command should fail before destroying dormant root");

        assert!(err.contains("No resolvable coding agent"));
        let preserved = session_mgr.get_session(dormant.id).await;
        assert!(matches!(
            preserved.as_ref().map(|s| &s.status),
            Some(SessionStatus::Exited(0))
        ));
        assert!(preserved.is_some_and(|s| s.is_root_agent));
    }

    #[test]
    fn inject_gemini_resume_prefixes_direct_gemini_args() {
        let mut args = vec!["-m".to_string(), "gpt-5".to_string()];
        assert!(super::inject_gemini_resume("gemini", &mut args));
        assert_eq!(
            args,
            vec![
                "--resume".to_string(),
                "latest".to_string(),
                "-m".to_string(),
                "gpt-5".to_string()
            ]
        );
    }

    #[test]
    fn inject_gemini_resume_inserts_into_cmd_tokenized_wrapper() {
        let mut args = vec![
            "/C".to_string(),
            "gemini".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ];
        assert!(super::inject_gemini_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec![
                "/C".to_string(),
                "gemini".to_string(),
                "--resume".to_string(),
                "latest".to_string(),
                "-m".to_string(),
                "gpt-5".to_string()
            ]
        );
    }

    #[test]
    fn inject_gemini_resume_inserts_into_embedded_cmd_wrapper() {
        let mut args = vec!["/K".to_string(), "git pull && gemini -m gpt-5".to_string()];
        assert!(super::inject_gemini_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec![
                "/K".to_string(),
                "git pull && gemini --resume latest -m gpt-5".to_string()
            ]
        );
    }

    #[test]
    fn inject_gemini_resume_skips_existing_resume_tokens() {
        let mut args = vec![
            "--resume".to_string(),
            "latest".to_string(),
            "gpt-5".to_string(),
        ];
        assert!(!super::inject_gemini_resume("gemini", &mut args));
        assert_eq!(
            args,
            vec![
                "--resume".to_string(),
                "latest".to_string(),
                "gpt-5".to_string()
            ]
        );
    }

    #[test]
    fn inject_codex_resume_prefixes_direct_codex_args() {
        let mut args = vec![
            "-m".to_string(),
            "gpt-5".to_string(),
            "-c".to_string(),
            "model_reasoning_effort=\"high\"".to_string(),
        ];

        assert!(inject_codex_resume("codex", &mut args));
        assert_eq!(
            args,
            vec![
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
                "-c".to_string(),
                "model_reasoning_effort=\"high\"".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_inserts_into_cmd_tokenized_wrapper() {
        let mut args = vec![
            "/C".to_string(),
            "codex".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ];

        assert!(inject_codex_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec![
                "/C".to_string(),
                "codex".to_string(),
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_inserts_into_embedded_cmd_wrapper() {
        let mut args = vec!["/K".to_string(), "git pull && codex -m gpt-5".to_string()];

        assert!(inject_codex_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec![
                "/K".to_string(),
                "git pull && codex resume --last -m gpt-5".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_skips_existing_resume_tokens() {
        let mut args = vec![
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ];

        assert!(!inject_codex_resume("codex", &mut args));
        assert_eq!(
            args,
            vec![
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_skips_explicit_fork_subcommand() {
        let mut args = vec!["fork".to_string(), "--last".to_string()];

        assert!(!inject_codex_resume("codex", &mut args));
        assert_eq!(args, vec!["fork".to_string(), "--last".to_string()]);
    }

    #[test]
    fn inject_codex_resume_skips_explicit_exec_subcommand_after_options() {
        let mut args = vec![
            "-m".to_string(),
            "gpt-5".to_string(),
            "exec".to_string(),
            "--json".to_string(),
        ];

        assert!(!inject_codex_resume("codex", &mut args));
        assert_eq!(
            args,
            vec![
                "-m".to_string(),
                "gpt-5".to_string(),
                "exec".to_string(),
                "--json".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_skips_explicit_help_subcommand_in_cmd_wrapper() {
        let mut args = vec!["/C".to_string(), "codex".to_string(), "help".to_string()];

        assert!(!inject_codex_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec!["/C".to_string(), "codex".to_string(), "help".to_string()]
        );
    }

    #[test]
    fn inject_codex_resume_ignores_resume_text_inside_config_value() {
        let mut args = vec![
            "-c".to_string(),
            "instruction=\"resume later\"".to_string(),
            "--search".to_string(),
        ];

        assert!(inject_codex_resume("codex", &mut args));
        assert_eq!(
            args,
            vec![
                "resume".to_string(),
                "--last".to_string(),
                "-c".to_string(),
                "instruction=\"resume later\"".to_string(),
                "--search".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_actual_agent_keeps_requested_agent_when_shell_validates_it() {
        let settings = test_settings();

        let resolved = resolve_actual_agent(
            "codex",
            &["-m".to_string(), "gpt-5".to_string()],
            Some("codex"),
            Some("Codex Stable"),
            &settings,
        );

        assert_eq!(
            resolved,
            (Some("codex".to_string()), Some("Codex Stable".to_string()))
        );
    }

    #[test]
    fn resolve_actual_agent_keeps_requested_agent_when_normalized_command_matches() {
        let mut settings = test_settings();
        settings.agents.push(AgentConfig {
            id: "codex-yolo".to_string(),
            label: "Codex Yolo".to_string(),
            command: "codex --yolo".to_string(),
            color: "#10b981".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
        });

        let resolved = resolve_actual_agent(
            "codex",
            &["--yolo".to_string()],
            Some("codex-yolo"),
            Some("Codex Yolo"),
            &settings,
        );

        assert_eq!(
            resolved,
            (
                Some("codex-yolo".to_string()),
                Some("Codex Yolo".to_string())
            )
        );
    }

    #[test]
    fn resolve_actual_agent_falls_back_to_detected_label_when_validated_match_has_no_stored_label()
    {
        let settings = test_settings();

        let resolved = resolve_actual_agent(
            "codex",
            &["-m".to_string(), "gpt-5".to_string()],
            Some("codex"),
            None,
            &settings,
        );

        assert_eq!(
            resolved,
            (Some("codex".to_string()), Some("Codex".to_string()))
        );
    }

    #[test]
    fn resolve_actual_agent_clears_requested_agent_when_shell_is_unresolved() {
        let settings = test_settings();

        let resolved = resolve_actual_agent(
            "powershell.exe",
            &["-NoLogo".to_string()],
            Some("codex"),
            Some("Codex"),
            &settings,
        );

        assert_eq!(resolved, (None, None));
    }

    #[test]
    fn resolve_actual_agent_uses_shell_resolved_agent_on_mismatch() {
        let settings = test_settings();

        let resolved = resolve_actual_agent("claude", &[], Some("codex"), Some("Codex"), &settings);

        assert_eq!(
            resolved,
            (Some("claude".to_string()), Some("Claude Code".to_string()))
        );
    }

    #[test]
    fn resolve_agent_from_shell_skips_invalid_configured_command() {
        let mut settings = test_settings();
        settings.agents = vec![AgentConfig {
            id: "broken-codex".to_string(),
            label: "Broken Codex".to_string(),
            command: "codex \"unterminated".to_string(),
            color: "#10b981".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
        }];

        let resolved = resolve_agent_from_shell("codex", &[], &settings);

        assert_eq!(resolved, (None, None));
    }

    #[test]
    fn effective_restart_skip_auto_resume_defaults_to_true_for_none() {
        // No explicit value → preserve legacy "fresh conversation" semantics
        // used by SessionItem, ProjectPanel context menu, AcDiscoveryPanel.
        assert!(super::effective_restart_skip_auto_resume(None));
    }

    #[test]
    fn effective_restart_skip_auto_resume_respects_explicit_false() {
        // Deferred-wake path (ProjectPanel.handleReplicaClick) MUST be able
        // to opt in to provider auto-resume; otherwise gemini/codex/claude
        // sessions re-open with a blank slate instead of continuing.
        assert!(!super::effective_restart_skip_auto_resume(Some(false)));
    }

    #[test]
    fn effective_restart_skip_auto_resume_respects_explicit_true() {
        // Explicit true still works (future-proof against a caller that
        // wants to be explicit rather than rely on the default).
        assert!(super::effective_restart_skip_auto_resume(Some(true)));
    }

    // ── (#630/#631) restart_skip_auto_resume_with_intent: the call site uses this
    //    exact composition to decide resume-vs-fresh on a restart. ──

    #[test]
    fn restart_intent_explicit_restart_button_is_fresh() {
        // "Restart Session" (None) on a session with no stored intent => fresh.
        assert!(super::restart_skip_auto_resume_with_intent(false, None));
    }

    #[test]
    fn restart_intent_normal_member_reopen_resumes() {
        // Branch A reopen of a NORMAL member (no stored intent, Some(false))
        // => resume, unchanged from today.
        assert!(!super::restart_skip_auto_resume_with_intent(
            false,
            Some(false)
        ));
    }

    #[test]
    fn restart_intent_deferred_fresh_member_reopen_stays_fresh() {
        // #631 closure: a "Restart Session"-then-app-restart member defers, then
        // reopens via Branch A (Some(false)), but its persisted fresh intent wins.
        assert!(super::restart_skip_auto_resume_with_intent(
            true,
            Some(false)
        ));
    }

    #[test]
    fn restart_intent_stored_fresh_with_restart_button_is_fresh() {
        // Stored fresh AND an explicit restart => still fresh (no regression).
        assert!(super::restart_skip_auto_resume_with_intent(true, None));
    }

    // ── #599 R1 effective_create_skip_auto_resume tests ──

    #[test]
    fn effective_create_skip_auto_resume_defaults_to_true_for_none() {
        // No explicit value: legacy fresh-create default used by create-in-place,
        // new-agent, open-agent, CLI, and web call sites (no --continue).
        assert!(super::effective_create_skip_auto_resume(None));
    }

    #[test]
    fn effective_create_skip_auto_resume_respects_explicit_false() {
        // #599 R1: reopening a coordinator destroyed by auto-close (#552/#580)
        // or manual close (#588) opts in to resume; --continue is then injected
        // (still subject to the claude_project_exists disk gate downstream).
        assert!(!super::effective_create_skip_auto_resume(Some(false)));
    }

    #[test]
    fn effective_create_skip_auto_resume_respects_explicit_true() {
        // Explicit true still skips resume (parity with the restart helper).
        assert!(super::effective_create_skip_auto_resume(Some(true)));
    }

    // ── #599 R2 claude_projects_dir_for_config_dir tests ──

    #[test]
    fn claude_projects_dir_for_config_dir_joins_projects_and_mangled() {
        // An explicit CLAUDE_CONFIG_DIR (already AC-expanded) maps to
        // <base>/projects/<mangled-cwd>, mirroring the wrapper resolver output.
        let base = if cfg!(windows) {
            "C:\\Users\\maria\\.claude-env"
        } else {
            "/home/maria/.claude-env"
        };
        let cwd = if cfg!(windows) { "C:\\x" } else { "/home/x" };
        let resolved = super::claude_projects_dir_for_config_dir(base, cwd);
        let expected = std::path::PathBuf::from(base)
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd));
        assert_eq!(resolved, expected);
    }

    #[test]
    fn claude_projects_dir_for_config_dir_expands_percent_envvar_in_base() {
        // Directly covers the lifted expand_env_var_refs: a %VAR% ref in the
        // config dir resolves against the process env before the projects join.
        let var = "AC_TEST_599_CONFIG_DIR";
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-env");
        // SAFETY: env state is process-global; unique name avoids cross-test races.
        std::env::set_var(var, custom_base.to_str().unwrap());
        let resolved = super::claude_projects_dir_for_config_dir(&format!("%{}%", var), "C:\\x");
        std::env::remove_var(var);
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, expected);
    }

    // ── should_inject_continue tests (issue #82, plan §8.1) ──

    #[test]
    fn should_inject_continue_returns_false_when_not_claude() {
        assert!(!should_inject_continue(false, false, true, "codex"));
    }

    #[test]
    fn should_inject_continue_returns_false_when_skip_overrides_existing_dir() {
        // G4 strengthening: lock the predicate against future refactors that
        // re-order early-return clauses. Explicit fixture, not "all permissive".
        assert!(!should_inject_continue(true, true, true, "claude"));
    }

    #[test]
    fn should_inject_continue_returns_false_when_dir_missing() {
        assert!(!should_inject_continue(true, false, false, "claude"));
    }

    #[test]
    fn should_inject_continue_returns_false_when_continue_already_present() {
        assert!(!should_inject_continue(
            true,
            false,
            true,
            "claude --continue"
        ));
    }

    #[test]
    fn should_inject_continue_returns_true_for_canonical_resume_case() {
        assert!(should_inject_continue(true, false, true, "claude"));
    }

    #[test]
    fn should_inject_continue_returns_false_when_continue_with_value_present() {
        // R2.4 / G2: the GNU long-option-with-value form must also suppress
        // re-injection.
        assert!(!should_inject_continue(
            true,
            false,
            true,
            "claude --continue=somevalue"
        ));
    }

    #[test]
    fn should_inject_continue_returns_false_when_uppercase_continue_present() {
        // D4 #6: case-insensitivity regression fence.
        assert!(!should_inject_continue(
            true,
            false,
            true,
            "claude --CONTINUE"
        ));
    }

    #[test]
    fn should_inject_continue_returns_false_when_short_form_present() {
        // D4 #7: -c short-form regression fence.
        assert!(!should_inject_continue(true, false, true, "claude -c"));
    }

    #[test]
    fn should_inject_continue_returns_false_when_continue_in_cmd_wrapper() {
        // D4 #8: token-level scan, not arg-index scan.
        assert!(!should_inject_continue(
            true,
            false,
            true,
            "cmd /C claude --continue"
        ));
    }

    #[test]
    fn should_inject_continue_returns_true_when_unrelated_continue_substring() {
        // D4 #9: token-equality fence — `--continued-mode` is NOT `--continue`.
        // Guards against a future regression to substring matching.
        assert!(should_inject_continue(
            true,
            false,
            true,
            "claude --continued-mode something"
        ));
    }

    // ── Issue #107 Round 5 §R5.8.6 — build_title_prompt_appendage idempotence ──
    //
    // Tempdir naming starts with `wg-` so `find_workgroup_task_path_for_cwd`'s
    // ancestor walk finds the cwd itself as the wg ancestor. The three tests
    // pin gates (3), (4), and the happy path. Path-walk gate (1) failure is
    // exercised by the existing `find_workgroup_task_path_for_cwd` tests in
    // `session/session.rs`. Read-failure gate (2) requires fault-injecting
    // `std::fs::read_to_string`, which is not worth the harness for a thin
    // orchestrator.

    // ── Issue #186 — resolve_claude_projects_dir ──

    fn write_wrapper(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn resolve_claude_projects_dir_uses_home_for_bare_claude() {
        // Default install → fall back to <home>/.claude/projects/<mangled>.
        let cwd = "C:\\Users\\Test\\repo";
        let resolved = super::resolve_claude_projects_dir("claude", &[], cwd);
        // Skip if the test host has no home dir (CI sandboxes sometimes don't).
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_uses_home_for_direct_claude_exe_path() {
        // Direct executable path with file_stem == "claude" → still default base.
        let cwd = "C:\\Users\\Test\\repo";
        let resolved = super::resolve_claude_projects_dir("C:\\Tools\\claude.exe", &[], cwd);
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_returns_none_when_no_claude_token() {
        let resolved =
            super::resolve_claude_projects_dir("powershell.exe", &["-NoLogo".to_string()], "C:\\x");
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_claude_projects_dir_parses_wrapper_with_set_directive() {
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let cwd = "C:\\Users\\Test\\repo";
        let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], cwd);
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_strips_quotes_around_value() {
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join("Path With Spaces").join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR=\"{}\"\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_falls_back_when_wrapper_lacks_directive() {
        let tmp = tempfile::tempdir().unwrap();
        let wrapper = write_wrapper(tmp.path(), "claude-mb.cmd", "@echo off\r\nclaude %*\r\n");
        let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_falls_back_when_wrapper_missing() {
        let resolved = super::resolve_claude_projects_dir(
            "C:\\definitely\\not\\there\\claude-mb.cmd",
            &[],
            "C:\\x",
        );
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_finds_claude_token_in_cmd_wrapper_args() {
        // shell=cmd.exe, args=["/K", "<abs path to claude-mb.cmd>", "--effort", "max"]
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let resolved = super::resolve_claude_projects_dir(
            "cmd.exe",
            &[
                "/K".to_string(),
                wrapper.to_str().unwrap().to_string(),
                "--effort".to_string(),
                "max".to_string(),
            ],
            "C:\\x",
        );
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_finds_claude_token_in_embedded_cmd_string() {
        // Embedded form — per-arg whitespace split must surface claude-mb.cmd.
        // Skip on hosts where the temp dir contains spaces (split would
        // fragment the wrapper path); the cmd-wrapped-args test above
        // already covers spaced paths via direct token form.
        let tmp = tempfile::tempdir().unwrap();
        if tmp.path().to_string_lossy().contains(' ') {
            return;
        }
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let combined = format!("git pull && {} --effort max", wrapper.display());
        let resolved =
            super::resolve_claude_projects_dir("cmd.exe", &["/K".to_string(), combined], "C:\\x");
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_ignores_oversized_wrapper() {
        // Large file (> 64 KiB cap) → treated as not-a-wrapper, fall back.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("claude-mb.cmd");
        let mut body = String::with_capacity(80 * 1024);
        body.push_str("set CLAUDE_CONFIG_DIR=C:\\should-not-be-read\r\n");
        body.push_str(&"x".repeat(80 * 1024));
        std::fs::write(&path, body).unwrap();
        let resolved = super::resolve_claude_projects_dir(path.to_str().unwrap(), &[], "C:\\x");
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_parses_cmd_quoted_whole_assignment() {
        // `set "CLAUDE_CONFIG_DIR=<path with spaces>"` — canonical cmd idiom
        // when the value contains spaces or shell metacharacters.
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join("Path With Spaces").join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset \"CLAUDE_CONFIG_DIR={}\"\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_expands_percent_envvar_value() {
        let var = "AC_TEST_186_BASE_PCT";
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        // SAFETY: env state is process-global; unique name avoids cross-test races.
        std::env::set_var(var, custom_base.to_str().unwrap());
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR=%{}%\r\nclaude %*\r\n",
                var
            ),
        );
        let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
        std::env::remove_var(var);
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_expands_powershell_envvar_value() {
        let var = "AC_TEST_186_BASE_PS";
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        std::env::set_var(var, custom_base.to_str().unwrap());
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.ps1",
            &format!(
                "$env:CLAUDE_CONFIG_DIR = \"$env:{}\"\r\nclaude @args\r\n",
                var
            ),
        );
        let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
        std::env::remove_var(var);
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_preserves_unknown_envvar_literal() {
        // Unknown var → literal preserved → resulting path is_dir() will be
        // false at the call site, but parse must succeed (return Some).
        let tmp = tempfile::tempdir().unwrap();
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            "@echo off\r\nset CLAUDE_CONFIG_DIR=%AC_TEST_186_DEFINITELY_UNSET%\\\\.claude-mb\r\nclaude %*\r\n",
        );
        let resolved = super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
        let resolved = resolved.expect("parser must return Some even when var is unset");
        let s = resolved.to_string_lossy();
        assert!(
            s.contains("%AC_TEST_186_DEFINITELY_UNSET%"),
            "expected literal preservation, got {s}"
        );
    }

    #[test]
    fn resolve_claude_projects_dir_parses_wrapper_with_export_directive() {
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.sh",
            &format!(
                "#!/usr/bin/env bash\nexport CLAUDE_CONFIG_DIR={}\nexec claude \"$@\"\n",
                custom_base.display()
            ),
        );
        let resolved =
            super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "/home/test/repo");
        let expected =
            custom_base
                .join("projects")
                .join(crate::session::session::mangle_cwd_for_claude(
                    "/home/test/repo",
                ));
        assert_eq!(resolved, Some(expected));
    }
}
