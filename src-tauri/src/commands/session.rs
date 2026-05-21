use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::agent_config::{self, AgentLocalConfig};
use crate::config::sessions_persistence::persist_current_state;
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::{SessionInfo, SessionRepo};
use crate::session::profile::CodingAgentKind;
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;

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

    /// Single-pass expansion of `%NAME%` (cmd) and `$env:NAME` (PowerShell)
    /// environment-variable references against `std::env::var`. Unknown names
    /// are preserved literally, so a downstream `is_dir()` check returns
    /// false rather than silently mis-resolving. Names must be ASCII
    /// alphanumeric or `_`; anything else terminates the name.
    ///
    /// Limitations (acceptable for real-world wrappers):
    ///   - No nested expansion: `%A%` whose value contains `%B%` is not re-expanded.
    ///   - No escape syntax (cmd's `^%`, PowerShell's backtick) — wrappers don't use these.
    fn expand_env_vars(input: &str) -> String {
        // Pass 1: %NAME% (cmd-style).
        let mut buf = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find('%') {
            buf.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            match after.find('%') {
                Some(end) => {
                    let name = &after[..end];
                    let valid = !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
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
            let expanded = expand_env_vars(unquoted);
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

/// Decide whether to auto-inject `--continue` for a Claude session.
/// Pure function: no filesystem access. Caller is responsible for resolving
/// `claude_project_exists` (typically `~/.claude/projects/<mangled-cwd>/.is_dir()`).
///
/// Callers should compute `claude_project_exists` via `resolve_claude_projects_dir`
/// to honor wrapper-set `CLAUDE_CONFIG_DIR`. Note: a wrapper named exactly
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
/// (#137 introduced `brief-set-title` which creates its own atomic backup;
/// the backend no longer snapshots before injection.)
///
/// The caller is the post-spawn task in `create_session_inner`; it injects this
/// prompt by itself. Credentials are not part of this payload.
///
/// Gates layered (in order):
///   1. workgroup BRIEF.md path resolvable from `cwd` → else `Err`
///      (config issue, F7 preserved).
///   2. BRIEF.md exists and read succeeds → else `Err`.
///   3. BRIEF.md non-empty (after trim) → else `Ok(None)` (silent skip).
///   4. No `title:` field in existing frontmatter → else `Ok(None)` (silent
///      skip).
///   5. Build title prompt with the absolute, UNC-stripped path (F4
///      preserved). Return `Ok(Some(prompt))`.
fn build_title_prompt_appendage(cwd: &str) -> Result<Option<String>, String> {
    use crate::commands::entity_creation::parse_brief_title;
    use crate::session::session::find_workgroup_brief_path_for_cwd;

    // (1) Resolve workgroup BRIEF.md path. F7 preserved.
    let brief_path = find_workgroup_brief_path_for_cwd(cwd)
        .ok_or_else(|| format!("[auto-title:config] no wg- ancestor in cwd '{}'", cwd))?;

    // (2) Read BRIEF.md. Missing/unreadable → Err (warn-and-skip at caller).
    let content = std::fs::read_to_string(&brief_path)
        .map_err(|e| format!("read BRIEF.md at {:?}: {}", brief_path, e))?;

    // (3) Empty brief → silent skip.
    if content.trim().is_empty() {
        return Ok(None);
    }

    // (4) Title already present → silent skip.
    if parse_brief_title(&content).is_some() {
        return Ok(None);
    }

    // (5) F4 preserved — strip Windows \\?\ extended-length prefix.
    let raw = brief_path.to_string_lossy().to_string();
    let path_str = raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string();
    let prompt = crate::pty::title_prompt::build_title_prompt(&path_str);

    Ok(Some(prompt))
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
pub async fn create_session_inner(
    app: &AppHandle,
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
) -> Result<SessionInfo, String> {
    let (agent_id, agent_label) = {
        let settings_state = app.state::<SettingsState>();
        let cfg = settings_state.read().await;
        resolve_actual_agent(
            &shell,
            &shell_args,
            agent_id.as_deref(),
            agent_label.as_deref(),
            &cfg,
        )
    };

    // Recompute is_coordinator from the current team snapshot. One source of truth —
    // every caller of create_session_inner gets the same computation.
    let teams = crate::config::teams::discover_teams();
    let is_coordinator = crate::config::teams::is_coordinator_for_cwd(&cwd, &teams);

    let mgr = session_mgr.read().await;
    let mut session = mgr
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
        .map_err(|e| e.to_string())?;

    if let Some(name) = session_name {
        mgr.rename_session(session.id, name.clone())
            .await
            .map_err(|e| e.to_string())?;
        session.name = name;
    }

    let id = session.id;

    let mut shell_args = shell_args;
    let full_cmd = format!("{} {}", shell, shell_args.join(" "));
    // #260 — single detector (session/profile.rs). Replaces the old
    // starts_with triple; this is the SAME call `strip_auto_injected_args`
    // makes, so the persisted recipe and the runtime identity cannot disagree.
    let agent_kind = CodingAgentKind::detect(&shell, &shell_args);
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
    let claude_project_exists = resolve_claude_projects_dir(&shell, &shell_args, &cwd)
        .map(|p| p.is_dir())
        .unwrap_or(false);
    if should_inject_continue(
        agent_kind == Some(CodingAgentKind::Claude),
        skip_auto_resume,
        claude_project_exists,
        &full_cmd,
    ) {
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

    let materialized_context_path = if let Some(target) = context_target {
        match crate::config::session_context::materialize_agent_context_file(&cwd, target) {
            Ok(context) => context,
            Err(e) => {
                log::error!("Replica context validation failed: {}", e);
                use tauri_plugin_dialog::DialogExt;
                let dialog_msg = format!("Cannot launch session — context files missing:\n\n{}", e);
                app.dialog()
                    .message(&dialog_msg)
                    .title("Context File Error")
                    .show(|_| {});
                let mgr2 = session_mgr.read().await;
                if let Ok(Some(new_id)) = mgr2.destroy_session(id).await {
                    let _ = app.emit(
                        "session_switched",
                        serde_json::json!({ "id": new_id.to_string() }),
                    );
                }
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

    pty_mgr
        .lock()
        .unwrap()
        .spawn(
            id,
            &shell,
            &shell_args,
            &cwd,
            120,
            30,
            &extra_env,
            crate::session::profile::idle_tuning_for(agent_kind),
            app.clone(),
        )
        .map_err(|e| e.to_string())?;

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
        let auto_title_enabled = {
            let settings_state = app.state::<SettingsState>();
            let cfg = settings_state.read().await;
            cfg.auto_generate_brief_title
        };

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
        }
    }

    Ok(info)
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
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    shell: Option<String>,
    shell_args: Option<Vec<String>>,
    cwd: Option<String>,
    session_name: Option<String>,
    agent_id: Option<String>,
    git_repos: Option<Vec<SessionRepo>>,
) -> Result<SessionInfo, String> {
    let cfg = settings.read().await;

    let cwd = cwd.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\".to_string())
    });

    // If agentId provided and shell not explicitly set, use that agent's command
    let (shell, shell_args, agent_label) = match (&shell, &agent_id) {
        (None, Some(aid)) => resolve_agent_command(aid, &cfg),
        _ => {
            let s = shell.unwrap_or_else(|| cfg.default_shell.clone());
            let sa = shell_args.unwrap_or_else(|| cfg.default_shell_args.clone());
            let al = agent_id.as_ref().and_then(|aid| {
                cfg.agents
                    .iter()
                    .find(|a| a.id == *aid)
                    .map(|a| a.label.clone())
            });
            (s, sa, al)
        }
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
        true, // skip_auto_resume = true → fresh create, no `--continue` injection
    )
    .await?;

    // Persist after creation
    {
        let mgr = session_mgr.read().await;
        persist_current_state(&mgr).await;
    }

    // Auto-attach Telegram bot if repo has .agentscommander/config.json
    let id = Uuid::parse_str(&info.id).unwrap();
    let config_path = std::path::Path::new(&cwd)
        .join(crate::config::agent_local_dir_name())
        .join("config.json");
    if let Ok(contents) = tokio::fs::read_to_string(&config_path).await {
        if let Ok(local_config) = serde_json::from_str::<AgentLocalConfig>(&contents) {
            if let Some(bot_label) = local_config.tooling.telegram_bot {
                let cfg = settings.read().await;
                let bot = cfg
                    .telegram_bots
                    .iter()
                    .find(|b| b.label == bot_label)
                    .cloned();
                drop(cfg);

                if let Some(bot) = bot {
                    let pty_arc = pty_mgr.inner().clone();
                    let reader = match crate::commands::telegram::derive_reader(
                        &info.shell,
                        &info.shell_args,
                        &cwd,
                        info.agent_kind,
                    ) {
                        Ok(r) => r,
                        Err(reason) => {
                            let err_msg = format!(
                                "Telegram bridge: {} for session {} (shell={:?}). Bridge inactive.",
                                reason, id, info.shell
                            );
                            log::error!("{}", err_msg);
                            let _ = app.emit(
                                "telegram_bridge_error",
                                serde_json::json!({
                                    "sessionId": info.id,
                                    "error": err_msg,
                                }),
                            );
                            // Skip attach — do not silently fall back to PTY.
                            return Ok(info);
                        }
                    };
                    let mut tg = tg_mgr.lock().await;
                    if let Ok(bridge_info) = tg.attach(id, &bot, pty_arc, app.clone(), reader) {
                        let _ = app.emit("telegram_bridge_attached", bridge_info);
                    }
                }
            }
        }
    }

    Ok(info)
}

/// Core session destruction logic shared by the Tauri command and the MailboxPoller.
/// Kills PTY, detaches Telegram bridge, removes from SessionManager, persists, and emits events.
pub async fn destroy_session_inner(app: &AppHandle, uuid: Uuid) -> Result<(), String> {
    let id = uuid.to_string();

    // Remove from detached set
    {
        let detached = app.state::<DetachedSessionsState>();
        let mut detached_set = detached.lock().unwrap();
        detached_set.remove(&uuid);
    }

    // Auto-detach Telegram bridge if active
    {
        let tg_mgr = app.state::<TelegramBridgeState>();
        let mut tg = tg_mgr.lock().await;
        if tg.has_bridge(uuid) {
            let _ = tg.detach(uuid);
            let _ = app.emit(
                "telegram_bridge_detached",
                serde_json::json!({ "sessionId": id }),
            );
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

    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
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

/// Resolves the effective `skip_auto_resume` flag for `restart_session`.
/// Defaults to `true` (fresh conversation) to preserve existing restart-button semantics.
/// `Some(false)` is used by the deferred-wake path (ProjectPanel.handleReplicaClick)
/// to allow provider auto-resume and continue the prior conversation.
fn effective_restart_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
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
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn restart_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    id: String,
    agent_id: Option<String>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    // 1. Read config from existing session BEFORE destroying it
    let (shell, shell_args, cwd, name, stored_agent_id, stored_agent_label, git_repos) = {
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
        )
    };

    // 2. Strip auto-injected args before restart so the new session starts from the saved recipe.
    let clean_args =
        crate::config::sessions_persistence::strip_auto_injected_args(&shell, &shell_args);

    let requested_agent_id = agent_id;
    let (shell, shell_args, agent_label) = if let Some(ref aid) = requested_agent_id {
        let cfg = settings.read().await;
        let resolved = resolve_agent_command(aid, &cfg);
        drop(cfg);
        resolved
    } else {
        (shell, clean_args, stored_agent_label)
    };

    // 3. Destroy the old session (resolves all State<> internally from app)
    destroy_session_inner(&app, uuid).await?;

    // 4. Create new session with same config, or switch to the selected coding agent.
    let session_info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        cwd.clone(),
        Some(name),
        requested_agent_id.or(stored_agent_id),
        agent_label,
        false, // skip_tooling_save
        git_repos,
        effective_restart_skip_auto_resume(skip_auto_resume),
    )
    .await?;

    // 5. Explicitly activate the new session.
    //    destroy_session_inner may have auto-activated a sibling.
    //    create_session_inner only auto-activates if active.is_none().
    //    With multiple sessions, the new session would NOT be active without this.
    let new_uuid = Uuid::parse_str(&session_info.id).map_err(|e| e.to_string())?;
    {
        let mgr = session_mgr.read().await;
        let _ = mgr.switch_session(new_uuid).await;
    }
    let _ = app.emit(
        "session_switched",
        serde_json::json!({ "id": session_info.id, "userInitiated": true }),
    );

    // 6. Re-attach Telegram bridge if the repo config has one
    let config_path = std::path::Path::new(&cwd)
        .join(crate::config::agent_local_dir_name())
        .join("config.json");
    if let Ok(contents) = tokio::fs::read_to_string(&config_path).await {
        if let Ok(local_config) = serde_json::from_str::<AgentLocalConfig>(&contents) {
            if let Some(bot_label) = local_config.tooling.telegram_bot {
                let cfg = settings.read().await;
                let bot = cfg
                    .telegram_bots
                    .iter()
                    .find(|b| b.label == bot_label)
                    .cloned();
                drop(cfg);

                if let Some(bot) = bot {
                    let pty_arc = pty_mgr.inner().clone();
                    let reader = match crate::commands::telegram::derive_reader(
                        &session_info.shell,
                        &session_info.shell_args,
                        &cwd,
                        session_info.agent_kind,
                    ) {
                        Ok(r) => r,
                        Err(reason) => {
                            let err_msg = format!(
                                "Telegram bridge: {} for session {} (shell={:?}). Bridge inactive.",
                                reason, new_uuid, session_info.shell
                            );
                            log::error!("{}", err_msg);
                            let _ = app.emit(
                                "telegram_bridge_error",
                                serde_json::json!({
                                    "sessionId": session_info.id,
                                    "error": err_msg,
                                }),
                            );
                            // Skip attach — do not silently fall back to PTY.
                            // Persist + return the new session info so restart still succeeds
                            // semantically (the user explicitly restarted; only the bridge fails).
                            {
                                let mgr = session_mgr.read().await;
                                persist_current_state(&mgr).await;
                            }
                            return Ok(session_info);
                        }
                    };
                    let mut tg = tg_mgr.lock().await;
                    if let Ok(bridge_info) = tg.attach(new_uuid, &bot, pty_arc, app.clone(), reader)
                    {
                        let _ = app.emit("telegram_bridge_attached", bridge_info);
                    }
                }
            }
        }
    }

    // 7. Persist state — create_session_inner does NOT persist
    {
        let mgr = session_mgr.read().await;
        persist_current_state(&mgr).await;
    }

    Ok(session_info)
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
) -> Result<Vec<SessionInfo>, String> {
    let mgr = session_mgr.read().await;
    Ok(mgr.list_sessions().await)
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

fn resolve_agent_command(
    agent_id: &str,
    settings: &AppSettings,
) -> (String, Vec<String>, Option<String>) {
    if let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) {
        log::info!(
            "[session] Agent resolved: id={:?}, label={:?}, command={:?}",
            agent.id,
            agent.label,
            agent.command
        );
        let parts: Vec<String> = agent
            .command
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if let Some((cmd, args)) = parts.split_first() {
            (cmd.clone(), args.to_vec(), Some(agent.label.clone()))
        } else {
            (
                settings.default_shell.clone(),
                settings.default_shell_args.clone(),
                Some(agent.label.clone()),
            )
        }
    } else {
        log::warn!(
            "[session] Agent NOT found for aid={:?}. Falling back to default shell.",
            agent_id
        );
        (
            settings.default_shell.clone(),
            settings.default_shell_args.clone(),
            None,
        )
    }
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
    // Collect all tokens from shell + args, extract basenames for comparison
    let full_cmd = format!("{} {}", shell, shell_args.join(" "));
    let cmd_basenames: Vec<String> = full_cmd
        .split_whitespace()
        .map(executable_basename)
        .collect();

    for agent in &settings.agents {
        let agent_exec = agent.command.split_whitespace().next().unwrap_or("");
        let agent_basename = executable_basename(agent_exec);
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

/// Create or reuse a root agent session.
/// Derives the root agent path from the current binary name:
///   {exe_dir}/.{binary_name}/ac-root-agent
/// If a session already exists at that path, switches to it instead.
/// Uses the first configured coding agent from settings.
/// Starts the root agent with per-child credential env when a configured coding agent is launched.
#[tauri::command]
pub async fn create_root_agent_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
) -> Result<SessionInfo, String> {
    // Derive root agent path from binary name
    let exe_path =
        std::env::current_exe().map_err(|e| format!("Failed to get current exe path: {}", e))?;
    let binary_name = exe_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Failed to extract binary name")?
        .to_string();

    let exe_dir = exe_path
        .parent()
        .ok_or("Failed to get exe parent directory")?;
    let root_agent_path = exe_dir
        .join(format!(".{}", binary_name))
        .join("ac-root-agent")
        .to_string_lossy()
        .to_string();

    // Check if a session already exists at this path — reuse it
    {
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        if let Some(existing) = sessions
            .iter()
            .find(|s| s.working_directory == root_agent_path)
        {
            log::info!(
                "[root-agent] Reusing existing session {} at {}",
                existing.id,
                root_agent_path
            );
            return Ok(existing.clone());
        }
    }

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&root_agent_path)
        .map_err(|e| format!("Failed to create root agent directory: {}", e))?;

    // Get the first configured agent from settings
    let cfg = settings.read().await;
    let (agent_id, shell, shell_args, agent_label) = if let Some(agent) = cfg.agents.first() {
        let parts: Vec<String> = agent
            .command
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if let Some((cmd, args)) = parts.split_first() {
            (
                Some(agent.id.clone()),
                cmd.clone(),
                args.to_vec(),
                Some(agent.label.clone()),
            )
        } else {
            (
                None,
                cfg.default_shell.clone(),
                cfg.default_shell_args.clone(),
                None,
            )
        }
    } else {
        (
            None,
            cfg.default_shell.clone(),
            cfg.default_shell_args.clone(),
            None,
        )
    };
    drop(cfg);

    let info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        root_agent_path.clone(),
        Some("Root Agent".to_string()),
        agent_id,
        agent_label,
        false,
        Vec::new(),
        true, // skip_auto_resume = true → fresh create, no `--continue` injection
    )
    .await?;

    // Persist after creation
    {
        let mgr = session_mgr.read().await;
        persist_current_state(&mgr).await;
    }

    // Auto-attach Telegram bot if configured
    let id = Uuid::parse_str(&info.id).map_err(|e| format!("Invalid session UUID: {}", e))?;
    let config_path = std::path::Path::new(&root_agent_path)
        .join(crate::config::agent_local_dir_name())
        .join("config.json");
    if let Ok(contents) = tokio::fs::read_to_string(&config_path).await {
        if let Ok(local_config) = serde_json::from_str::<AgentLocalConfig>(&contents) {
            if let Some(bot_label) = local_config.tooling.telegram_bot {
                let cfg = settings.read().await;
                let bot = cfg
                    .telegram_bots
                    .iter()
                    .find(|b| b.label == bot_label)
                    .cloned();
                drop(cfg);
                if let Some(bot) = bot {
                    let pty_arc = pty_mgr.inner().clone();
                    let reader = match crate::commands::telegram::derive_reader(
                        &info.shell,
                        &info.shell_args,
                        &root_agent_path,
                        info.agent_kind,
                    ) {
                        Ok(r) => r,
                        Err(reason) => {
                            let err_msg = format!(
                                "Telegram bridge: {} for session {} (shell={:?}). Bridge inactive.",
                                reason, id, info.shell
                            );
                            log::error!("{}", err_msg);
                            let _ = app.emit(
                                "telegram_bridge_error",
                                serde_json::json!({
                                    "sessionId": info.id,
                                    "error": err_msg,
                                }),
                            );
                            // Skip attach — do not silently fall back to PTY.
                            return Ok(info);
                        }
                    };
                    let mut tg = tg_mgr.lock().await;
                    if let Ok(bridge_info) = tg.attach(id, &bot, pty_arc, app.clone(), reader) {
                        let _ = app.emit("telegram_bridge_attached", bridge_info);
                    }
                }
            }
        }
    }

    // Credentials for resolved agent sessions are provided through the child process environment in PtyManager::spawn; no visible credential write occurs here.

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::{inject_codex_resume, resolve_actual_agent, should_inject_continue};
    use crate::config::settings::{AgentConfig, AppSettings};
    use std::path::PathBuf;

    fn test_settings() -> AppSettings {
        AppSettings {
            agents: vec![
                AgentConfig {
                    id: "claude".to_string(),
                    label: "Claude Code".to_string(),
                    command: "claude".to_string(),
                    color: "#d97706".to_string(),
                    git_pull_before: false,
                    exclude_global_claude_md: false,
                },
                AgentConfig {
                    id: "codex".to_string(),
                    label: "Codex".to_string(),
                    command: "codex".to_string(),
                    color: "#10b981".to_string(),
                    git_pull_before: false,
                    exclude_global_claude_md: false,
                },
            ],
            ..AppSettings::default()
        }
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
    // Tempdir naming starts with `wg-` so `find_workgroup_brief_path_for_cwd`'s
    // ancestor walk finds the cwd itself as the wg ancestor. The three tests
    // pin gates (3), (4), and the happy path. Path-walk gate (1) failure is
    // exercised by the existing `find_workgroup_brief_path_for_cwd` tests in
    // `session/session.rs`. Read-failure gate (2) requires fault-injecting
    // `std::fs::read_to_string`, which is not worth the harness for a thin
    // orchestrator.

    #[test]
    fn build_title_prompt_appendage_returns_none_when_title_already_present() {
        use std::env;
        let dir = env::temp_dir().join(format!("wg-r5-idempotent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let brief = dir.join("BRIEF.md");
        std::fs::write(&brief, b"---\ntitle: 'Pre-existing'\n---\nBody.\n").unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let result = super::build_title_prompt_appendage(&cwd);
        assert!(
            matches!(result, Ok(None)),
            "expected Ok(None), got {:?}",
            result
        );
        let _ = std::fs::remove_file(&brief);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn build_title_prompt_appendage_returns_none_when_brief_empty() {
        use std::env;
        let dir = env::temp_dir().join(format!("wg-r5-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let brief = dir.join("BRIEF.md");
        std::fs::write(&brief, b"   \n\n\t\n").unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let result = super::build_title_prompt_appendage(&cwd);
        assert!(
            matches!(result, Ok(None)),
            "expected Ok(None), got {:?}",
            result
        );
        let _ = std::fs::remove_file(&brief);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn build_title_prompt_appendage_returns_some_when_brief_has_no_title() {
        use std::env;
        let dir = env::temp_dir().join(format!("wg-r5-some-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let brief = dir.join("BRIEF.md");
        std::fs::write(&brief, b"# A real brief with body content.\n").unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let result = super::build_title_prompt_appendage(&cwd);
        let prompt = match result {
            Ok(Some(p)) => p,
            other => panic!("expected Ok(Some(_)), got {:?}", other),
        };
        assert!(prompt.contains("brief-set-title"));
        assert!(prompt.contains("<AGENTSCOMMANDER_BINARY_PATH>"));
        // Production code strips `\\?\` UNC prefix before embedding the path
        // (F4 fold). Mirror the strip here so the assertion holds on Windows
        // setups where `temp_dir()` returns an extended-length path.
        let brief_raw = brief.to_string_lossy().to_string();
        let brief_str = brief_raw.strip_prefix(r"\\?\").unwrap_or(&brief_raw);
        assert!(prompt.contains(brief_str));
        let legacy_header = ["# === Session", "Credentials ==="].join(" ");
        assert!(prompt.contains("<AGENTSCOMMANDER_TOKEN>"));
        assert!(!prompt.contains(&legacy_header));
        assert!(!prompt.to_ascii_lowercase().contains("fallback"));
        assert!(!prompt.to_ascii_lowercase().contains("visible"));
        let _ = std::fs::remove_file(&brief);
        let _ = std::fs::remove_dir(&dir);
    }

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
