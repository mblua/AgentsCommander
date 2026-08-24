use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::agent_command::AgentSpawnCommand;
use crate::config::agent_config::{self, AgentLocalConfig};
use crate::config::coordinator_clocks::CoordinatorClocksState;
use crate::config::sessions_persistence::persist_current_state;
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::backend::{
    BackendSpawnSpec, PtyViewport, ResolvedAgentHostShell, SessionBackendKind,
};
use crate::pty::container_paths::{
    claude_config_dir_no_value_warning, container_config_dir, ContainerEnvWarning,
    ContainerPathMap, CLAUDE_CONFIG_DIR_KEY,
};
use crate::pty::container_runtime::DEFAULT_CONTAINER_WORKDIR;
use crate::pty::manager::PtyManager;
use crate::pty::output::PtyOutputTarget;
use crate::resource_monitor::{
    AgentLaunchPermit, ResourceLaunchMetadata, ResourceLaunchRegistration, ResourceLimits,
    ResourceLogicalAgentSlot, ResourceMonitorState,
};
use crate::session::manager::{
    CommitDecision, LifecycleMutations, PendingCreateBinding, SessionManager,
};
use crate::session::profile::{locate_pi_command, CodingAgentKind, PiInsertionPoint};
use crate::session::selection::{
    CriticalAdmissionOutcome, SelectionCause, SelectionCoordinator, SelectionRequest,
    SelectionSource, SelectionTransaction, TrustedCreateIntent, TrustedRestartIntent,
};
use crate::session::session::{
    SessionCommunication, SessionCommunicationKind, SessionInfo, SessionRepo, SessionStatus,
};
use crate::session::warnings::{emit_session_warning, SessionWarning};
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;

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

/// #1032 + #1171 - start sampling a freshly spawned session, with both engines.
///
/// **Sessions with no agent are never registered**, and that rule lives HERE, once, for both:
/// a plain shell costs neither engine anything, ever. `try_state` and not `state`, because
/// `state` panics when unmanaged and a test app manages neither engine - an absent engine is
/// simply the feature being off.
///
/// There is no race with the screen parser: `PtyManager::spawn` has already returned `Ok` at
/// the call site and the parser is registered during the spawn. The scraper's first sample is
/// 5 s away and the first watcher tick is 200 ms away, and neither can arrive before the
/// parser exists.
pub(crate) fn register_session_samplers<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: Uuid,
    agent_id: Option<String>,
) {
    let Some(agent_id) = agent_id else {
        return;
    };
    if let Some(scraper) = app.try_state::<Arc<crate::pty::context_scrape::ContextScraper>>() {
        scraper.register_session(id, agent_id.clone());
    }
    if let Some(watchers) = app.try_state::<Arc<crate::pty::watchers::WatcherEngine>>() {
        watchers.register_session(id, agent_id);
    }
}

/// #1171 - purge every per-session side structure a DESTROYED session leaves behind.
///
/// One helper with two call sites, because the per-session side-state purge is already
/// duplicated in this file: the destroy loop and `publish_restart_destroyed` are parallel
/// copies of each other, and adding a third thing to purge to only one of them is how a leak
/// gets written.
///
/// **The order is load-bearing.** The engine is retired FIRST, so no tick still in flight can
/// republish this session's status and recreate the entry step 2 removes. `WatcherHistory`
/// creates entries only from the engine's publish, and the engine publishes only while holding
/// its registration lock, so once `retire_session` returns nothing can bring the entry back.
///
/// **Destroyed sessions only.** A session that exits on its own is NOT destroyed: the engine
/// stops sampling it and its buffer stays, which is the case that matters - an API error or a
/// CLI crash is exactly when the evidence is worth keeping. Root-agent sessions retained as
/// `Exited` keep theirs too, because their row is still in the list.
pub(crate) fn purge_session_side_state<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    if let Some(watchers) = app.try_state::<Arc<crate::pty::watchers::WatcherEngine>>() {
        watchers.retire_session(session_id);
    }
    if let Some(history) = app.try_state::<crate::pty::watchers::history::WatcherHistoryState>() {
        history.purge(session_id);
    }
    reset_substantive_input(app, session_id);
}

/// #871 - clear a session's substantive-input marker.
///
/// Extracted by #1171 so it has ONE definition rather than the two inline copies the destroy
/// and restart cleanups each carried. It is a map removal, so calling it twice for the same
/// session is a no-op and not a second effect - which is what lets the destroy path keep
/// resetting every id it publishes, retained-exited ones included, while the destroyed-only
/// purge below also resets the ones it owns.
fn reset_substantive_input<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    if let Some(activity) = app.try_state::<crate::pty::input_activity::SubstantiveInputState>() {
        activity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .reset(session_id);
    }
}

async fn rollback_pre_created_session<R: tauri::Runtime>(
    _app: &AppHandle<R>,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    id: Uuid,
    reason: &str,
) {
    log::warn!(
        "[session] Rolling back pre-created session {} after setup failure: {}",
        id,
        reason
    );
    // The reserved create ticket or inline transaction owns the one common
    // backend-kind-aware rollback after this setup body returns. Killing here
    // would race that finalizer and perform the same teardown twice.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateSelectionIntent {
    User,
    Background,
    Suppress,
}

impl CreateSelectionIntent {
    fn trusted(self) -> Option<TrustedCreateIntent> {
        match self {
            Self::User => Some(TrustedCreateIntent::User),
            Self::Background => Some(TrustedCreateIntent::Background),
            Self::Suppress => None,
        }
    }
}

struct DeferredCreateOutput {
    info: SessionInfo,
    binding: PendingCreateBinding,
    warnings: Vec<SessionWarning>,
}

enum CreateCompletion {
    Finalized(SessionInfo),
    Deferred(DeferredCreateOutput),
}

impl CreateCompletion {
    fn into_finalized(self) -> Result<SessionInfo, String> {
        match self {
            Self::Finalized(info) => Ok(info),
            Self::Deferred(_) => Err("create unexpectedly deferred finalization".to_string()),
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

fn antigravity_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
    tokens[start..].iter().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower == "-c" || lower == "--conversation"
            || lower.starts_with("--conversation=")
    })
}

fn inject_antigravity_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    // #260/#1482 — resume token sourced from the CodingAgentProfile (single
    // source of truth). G6: slice-pattern destructure, never index — a future
    // <1-element slice degrades gracefully instead of panicking in release.
    let &[resume_token] = CodingAgentKind::Antigravity.profile().resume_tokens else {
        debug_assert!(false, "Antigravity resume_tokens must have exactly 1 element");
        return false;
    };
    match executable_basename(shell).as_str() {
        "agy" | "antigravity" => {
            let tokens: Vec<&str> = shell_args.iter().map(String::as_str).collect();
            if antigravity_tokens_have_resume(&tokens, 0) {
                return false;
            }
            shell_args.insert(0, resume_token.to_string());
            true
        }
        "cmd" => {
            // Tokenized and embedded forms, structurally mirroring the deleted
            // inject_gemini_resume cmd arm, but searching for a token whose
            // executable basename is exactly "agy" | "antigravity".
            let is_antigravity_executable = |arg: &str| {
                matches!(executable_basename(arg).as_str(), "agy" | "antigravity")
            };
            if let Some(idx) = shell_args.iter().position(|arg| is_antigravity_executable(arg)) {
                let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
                if antigravity_tokens_have_resume(&tokens, idx + 1) {
                    return false;
                }
                shell_args.insert(idx + 1, resume_token.to_string());
                return true;
            }

            for arg in shell_args.iter_mut() {
                let mut tokens: Vec<String> = arg
                    .split_whitespace()
                    .map(|token| token.to_string())
                    .collect();
                if let Some(idx) = tokens.iter().position(|token| is_antigravity_executable(token))
                {
                    let token_refs: Vec<&str> = tokens.iter().map(|token| token.as_str()).collect();
                    if antigravity_tokens_have_resume(&token_refs, idx + 1) {
                        return false;
                    }
                    tokens.insert(idx + 1, resume_token.to_string());
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

fn pi_has_explicit_session_control(option_tokens: &[String]) -> bool {
    const SHORT_CONTROLS: &[&str] = &["-c", "-r"];
    const LONG_CONTROLS: &[&str] = &[
        "--continue",
        "--resume",
        "--session",
        "--session-id",
        "--fork",
        "--no-session",
    ];

    option_tokens.iter().any(|token| {
        SHORT_CONTROLS.contains(&token.as_str())
            || LONG_CONTROLS.iter().any(|name| {
                token == *name
                    || token
                        .strip_prefix(*name)
                        .is_some_and(|suffix| suffix.starts_with('='))
            })
    })
}

fn pi_is_non_conversation_invocation(option_tokens: &[String]) -> bool {
    const MANAGEMENT_COMMANDS: &[&str] =
        &["install", "remove", "uninstall", "update", "list", "config"];
    const ONE_SHOT_FLAGS: &[&str] = &["--help", "-h", "--version", "-v"];
    const JOINABLE_ONE_SHOT_FLAGS: &[&str] = &["--export", "--list-models"];

    option_tokens
        .first()
        .is_some_and(|token| MANAGEMENT_COMMANDS.contains(&token.as_str()))
        || option_tokens.iter().any(|token| {
            ONE_SHOT_FLAGS.contains(&token.as_str())
                || JOINABLE_ONE_SHOT_FLAGS.iter().any(|name| {
                    token == *name
                        || token
                            .strip_prefix(*name)
                            .is_some_and(|suffix| suffix.starts_with('='))
                })
        })
}

fn inject_pi_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    let &[resume_token] = CodingAgentKind::Pi.profile().resume_tokens else {
        debug_assert!(false, "Pi resume_tokens must have exactly 1 element");
        return false;
    };
    let Ok(Some(location)) = locate_pi_command(shell, shell_args) else {
        return false;
    };
    if pi_has_explicit_session_control(&location.option_tokens)
        || pi_is_non_conversation_invocation(&location.option_tokens)
    {
        return false;
    }

    match location.insertion {
        PiInsertionPoint::Arg { index } => {
            if index > shell_args.len() {
                return false;
            }
            shell_args.insert(index, resume_token.to_string());
        }
        PiInsertionPoint::CmdText {
            arg_index,
            executable_range,
            segment_range,
        } => {
            let Some(text) = shell_args.get_mut(arg_index) else {
                return false;
            };
            if segment_range.start != executable_range.start
                || executable_range.start > executable_range.end
                || executable_range.end > segment_range.end
                || segment_range.end > text.len()
                || !text.is_char_boundary(executable_range.start)
                || !text.is_char_boundary(executable_range.end)
                || !text.is_char_boundary(segment_range.start)
                || !text.is_char_boundary(segment_range.end)
            {
                return false;
            }
            let insertion = format!(" {resume_token}");
            text.insert_str(executable_range.end, &insertion);
        }
    }
    true
}

fn maybe_inject_pi_resume(
    agent_kind: Option<CodingAgentKind>,
    resolved_spawn: Option<&AgentSpawnCommand>,
    skip_auto_resume: bool,
    shell: &str,
    shell_args: &mut Vec<String>,
) -> bool {
    if agent_kind != Some(CodingAgentKind::Pi) || resolved_spawn.is_none() || skip_auto_resume {
        return false;
    }
    inject_pi_resume(shell, shell_args)
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

#[derive(Debug, Clone)]
struct ResumeProbeTarget {
    host_probe_path: Option<PathBuf>,
    filesystem: &'static str,
    warning: Option<ContainerEnvWarning>,
}

#[derive(Debug, Clone)]
struct ContainerPathContext {
    host_root: String,
    map: ContainerPathMap,
}

fn container_path_context_for_cwd(cwd: &str) -> Result<ContainerPathContext, String> {
    let canonical_host_root = std::fs::canonicalize(cwd)
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

fn container_mount_rejection_message(selected: &str, canonical: &str, reason: String) -> String {
    if selected == canonical {
        reason
    } else {
        format!(
            "{} (selected path '{}', canonical path '{}')",
            reason, selected, canonical
        )
    }
}

fn effective_codex_home_for_backend(
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

fn resume_probe_target(
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
fn claude_resume_probe_target_for_kind(
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
fn injected_claude_config_dir_for_copy(
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

fn resume_probe_target_for_config_dir(
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
fn should_inject_fresh_session_id(is_claude: bool, skip_auto_resume: bool, full_cmd: &str) -> bool {
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

/// #1063: test-only finite barriers that pause a real session create at the two
/// points a deletion race needs: after the pending row is inserted and before the
/// config-seed project gate (`before_project_gate`), and after the seed transaction
/// and before PTY spawn/finalization (`after_seed_before_pty`). Barriers are keyed
/// by the create's normalized working directory, so concurrently running create
/// tests never take each other's barrier, and they exist only in `#[cfg(test)]`
/// builds, so production create is unchanged and stays non-emitting.
#[cfg(test)]
pub(crate) mod seed_race_barriers {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::sync::Notify;

    /// A one-shot rendezvous: the create signals `reached` when it hits the barrier,
    /// then awaits `release`. The controlling test awaits `reached`, interleaves its
    /// deletion, and finally signals `release`.
    #[derive(Default)]
    pub(crate) struct SeedRaceBarrier {
        pub(crate) reached: Notify,
        pub(crate) release: Notify,
    }

    type BarrierMap = Mutex<Option<HashMap<String, Arc<SeedRaceBarrier>>>>;

    static BEFORE_PROJECT_GATE: BarrierMap = Mutex::new(None);
    static AFTER_SEED_BEFORE_PTY: BarrierMap = Mutex::new(None);

    fn install(slot: &BarrierMap, key: &str) -> Arc<SeedRaceBarrier> {
        let barrier = Arc::new(SeedRaceBarrier::default());
        slot.lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(key.to_string(), barrier.clone());
        barrier
    }

    async fn hit(slot: &BarrierMap, key: &str) {
        let barrier = slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .and_then(|map| map.remove(key));
        if let Some(barrier) = barrier {
            barrier.reached.notify_one();
            barrier.release.notified().await;
        }
    }

    /// Arm the `before_project_gate` barrier for the create whose normalized cwd is
    /// `key`; the returned handle observes `reached` and drives `release`.
    pub(crate) fn install_before_project_gate(key: &str) -> Arc<SeedRaceBarrier> {
        install(&BEFORE_PROJECT_GATE, key)
    }

    /// Arm the `after_seed_before_pty` barrier for the create whose normalized cwd is `key`.
    pub(crate) fn install_after_seed_before_pty(key: &str) -> Arc<SeedRaceBarrier> {
        install(&AFTER_SEED_BEFORE_PTY, key)
    }

    pub(crate) async fn hit_before_project_gate(key: &str) {
        hit(&BEFORE_PROJECT_GATE, key).await;
    }

    pub(crate) async fn hit_after_seed_before_pty(key: &str) {
        hit(&AFTER_SEED_BEFORE_PTY, key).await;
    }
}

/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects provider-specific
/// resume flags (Claude/Pi/Antigravity `--continue`, Codex `resume --last`)
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
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    // #973 - the size the view has already fitted to, when the caller has a view at all.
    // `None` keeps AC's historical 120x30. See `PtyViewport`.
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
) -> Result<SessionInfo, String> {
    create_session_inner_impl(
        app,
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd,
        session_name,
        agent_id,
        agent_label,
        skip_tooling_save,
        git_repos,
        skip_auto_resume,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        selection_intent,
        None,
        None,
        None,
        None,
        false,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await?
    .into_finalized()
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session_inner_with_pty_target_ownership<R: tauri::Runtime>(
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
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
    target_ownership: &crate::api::message_store::PtyInputTargetOwnership<'_>,
) -> Result<SessionInfo, String> {
    create_session_inner_impl(
        app,
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd,
        session_name,
        agent_id,
        agent_label,
        skip_tooling_save,
        git_repos,
        skip_auto_resume,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        selection_intent,
        None,
        None,
        None,
        None,
        false,
        Some(target_ownership),
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await?
    .into_finalized()
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session_inner_for_restore<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
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
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    pending_start_fresh: Option<bool>,
    pending_communication: Option<SessionCommunication>,
) -> Result<SessionInfo, String> {
    create_session_inner_impl(
        transaction.app(),
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd,
        session_name,
        agent_id,
        agent_label,
        skip_tooling_save,
        git_repos,
        skip_auto_resume,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        CreateSelectionIntent::Suppress,
        pending_start_fresh,
        pending_communication,
        Some(transaction),
        Some(SelectionCause::Restore),
        false,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await?
    .into_finalized()
}

#[allow(clippy::too_many_arguments)]
async fn create_session_inner_impl<R: tauri::Runtime>(
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
    mut skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
    pending_start_fresh: Option<bool>,
    pending_communication: Option<SessionCommunication>,
    inline_transaction: Option<&SelectionTransaction<R>>,
    inline_cause: Option<SelectionCause>,
    defer_inline_finalization: bool,
    preheld_target: Option<&crate::api::message_store::PtyInputTargetOwnership<'_>>,
    enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
) -> Result<CreateCompletion, String> {
    // #1327 - startup coding-agent updates gate: no session may open (GUI,
    // restore, restart, root agent, web, phone) before the startup update run
    // finishes or times out. Absent in tests (mock app, nothing managed) and in
    // non-GUI contexts -> no-op.
    if let Some(gate) = app.try_state::<Arc<crate::agent_update::AgentUpdateGate>>() {
        gate.wait_until_done().await;
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);
    let cwd_path = std::path::Path::new(&cwd);
    let create_target_key = crate::config::teams::pty_input_create_gate_key_from_cwd(cwd_path)
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let create_target_replica_identity = if create_target_key.is_some() {
        let anchor = crate::config::teams::strict_wg_replica_anchor_from_cwd(cwd_path)
            .map_err(|_| "targetCreateGateUnavailable".to_string())?
            .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        Some(
            crate::path_identity::verify_directory(&anchor)
                .map_err(|_| "targetCreateGateUnavailable".to_string())?,
        )
    } else {
        None
    };
    if let Some(preheld) = preheld_target {
        let target_key = create_target_key
            .as_deref()
            .ok_or_else(|| "invalidTargetCreateOwnership".to_string())?;
        if !preheld.proves(target_key) {
            return Err("invalidTargetCreateOwnership".to_string());
        }
    }
    let _owned_target_gate = if preheld_target.is_none() {
        if let Some(target_key) = create_target_key.as_deref() {
            let gate = app
                .try_state::<crate::api::message_store::PtyInputTargetGateState>()
                .and_then(|state| state.gate.as_ref().ok().map(Arc::clone))
                .or_else(|| {
                    app.try_state::<crate::api::message_store::MessageStoreState>()
                        .and_then(|state| state.target_gate.as_ref().ok().map(Arc::clone))
                })
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            let stripe = gate
                .acquire_target_lock(target_key)
                .await
                .map_err(|_| "targetCreateGateUnavailable".to_string())?;
            let exact = gate.acquire_exact(target_key).await;
            Some((stripe, exact))
        } else {
            None
        }
    } else {
        None
    };
    let session_label = session_name.as_deref().unwrap_or(&shell).to_string();
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    let mut create_ticket = match inline_transaction {
        Some(_) => None,
        None => Some(match selection_intent.trusted() {
            Some(intent) => coordinator
                .reserve_create(intent)
                .await
                .map_err(|error| error.to_string())?,
            None => coordinator
                .reserve_suppressed_create()
                .await
                .map_err(|error| error.to_string())?,
        }),
    };
    let coordinator_shutdown = coordinator.shutdown_token();
    let inline_pending_binding = Arc::new(Mutex::new(None));
    let inline_pending_for_body = Arc::clone(&inline_pending_binding);
    let create_body = async {
        // §1295 5.1a creation gate: FIRST statement of create_body, BEFORE the
        // existing `enforce_unarchived_for_spawn` (which the archive-gate source
        // test :7438 counts separately). Reading the SAME normalized `cwd` string
        // the archive gate receives; no resource permit and no pending row exist
        // yet, so a rejection leaves zero RAM/disk residue and needs no
        // `rollback_pre_created_session`. (dev-rust E: the two call strings
        // at :1443/:1708 are untouched.)
        crate::config::sessions_persistence::enforce_creation_gate(app, &cwd, enforcement).await?;
        crate::config::archive_gate::enforce_unarchived_for_spawn(app, &cwd, &session_label)
            .await?;
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

        // Recompute is_coordinator from the current team snapshot. One source of truth:
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

        // (#756) Durable fresh-intent mirror, consumed at the create path: the
        // caller requested resume (#599 reopen passes skipAutoResume=false), but
        // this cwd's coordinator carries a pending fresh boundary (restart or
        // successful logical clear (/clear or Pi /new) whose record died with
        // an auto/manual close). Force a fresh spawn BEFORE any provider resume
        // injection below (Claude/Pi/Antigravity --continue, Codex resume --last);
        // provider-agnostic by construction. The mirror is deliberately NOT cleared
        // here: only post-boundary content (typed input or AC-injected content)
        // drops it, so repeated close/reopen cycles with no content stay fresh.
        let mut fresh_forced_by_mirror = false;
        if is_coordinator && !skip_auto_resume {
            if let Some(clocks) =
                app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
            {
                let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
                let pending = {
                    let guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
                    guard.start_fresh_at(&fqn)
                };
                if mirror_forces_fresh(skip_auto_resume, is_coordinator, pending.is_some()) {
                    log::info!(
                    "[session] fresh-intent mirror pending for '{}' (boundary {}): forcing skip_auto_resume (#756)",
                    fqn,
                    pending.map(|d| d.to_rfc3339()).unwrap_or_default()
                );
                    skip_auto_resume = true;
                    fresh_forced_by_mirror = true;
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

        // Clone the manager handle so the outer RwLock guard drops before spawn awaits.
        let mgr = session_mgr.read().await.clone();
        let backend_kind = resolved_spawn
            .as_ref()
            .map(|spawn| SessionBackendKind::from(&spawn.backend))
            .unwrap_or_default();
        if is_root_agent && backend_kind == SessionBackendKind::ContainerTransport {
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Err("root-agent cannot use container transport".to_string());
        }
        let container_path_context = if backend_kind == SessionBackendKind::ContainerTransport {
            match container_path_context_for_cwd(&cwd) {
                Ok(context) => Some(context),
                Err(err) => {
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    return Err(err);
                }
            }
        } else {
            None
        };
        // #935 - resolve the container's read-write repo bind mounts from its own
        // per-agent config.json repos[] (plan Sec 2/4), on the CANONICAL host root.
        // This runs BEFORE create_session below, so an inadmissible entry (an escape
        // signature: a repos[] rewrite reaching a sibling replica or messaging/) hard-
        // fails the spawn here with no session record, token, or container created
        // (plan Sec 4.3/4.4). None for local-process sessions.
        let container_repos = match container_path_context.as_ref() {
            Some(context) => {
                match crate::pty::container_repos::resolve_repo_mounts(std::path::Path::new(
                    &context.host_root,
                )) {
                    Ok(resolution) => Some(resolution),
                    Err(err) => {
                        release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                        return Err(err);
                    }
                }
            }
            None => None,
        };
        // #1101: the restart replacement create runs while `teardown_old_for_restart`
        // has released the old PTY but RETAINED its canonical manager row (removed only
        // at the atomic commit's `mutations.remove(uuid)`, which runs after this gate).
        // For a live-session restart that row is non-Exited, so `appeared_session` is a
        // false positive and would refuse every restart of a live WG replica session.
        // Restart passes `preheld_target = None` and acquires its own exact target lock
        // (`_owned_target_gate`), so it is already serialized and is the authoritative
        // replacement; this cold-spawn dedup heuristic is redundant and wrong for it.
        // Skip the gate only for the restart cause. The wake path (inline_cause = None)
        // is unchanged and keeps its dedup.
        if create_target_replica_identity.is_some()
            && matches!(
                selection_intent,
                CreateSelectionIntent::Background | CreateSelectionIntent::Suppress
            )
            && !matches!(inline_cause, Some(SelectionCause::Restart(_)))
        {
            let target = create_target_replica_identity
                .as_ref()
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            let appeared_session = mgr.list_sessions().await.into_iter().any(|session| {
                !matches!(session.status, SessionStatus::Exited(_))
                    && crate::path_identity::verify_directory(std::path::Path::new(
                        &session.working_directory,
                    ))
                    .is_ok_and(|cwd_identity| {
                        crate::path_identity::is_verified_descendant(&cwd_identity, target)
                    })
            });
            let appeared_spawn = pty_mgr
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .has_pending_spawn_for_replica(target);
            if appeared_session || appeared_spawn {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err("sessionRace".to_string());
            }
        }
        let spawn_mark = {
            let pty = pty_mgr.lock().unwrap_or_else(|error| error.into_inner());
            pty.mark_spawning(&cwd, &session_label)
        };
        let pending_result = if let Some(ticket) = create_ticket.as_mut() {
            mgr.create_pending_session(
                ticket,
                shell.clone(),
                shell_args.clone(),
                cwd.clone(),
                agent_id.clone(),
                agent_label.clone(),
                git_repos,
                is_coordinator,
                backend_kind,
            )
            .await
            .map(|session| {
                let binding = ticket
                    .binding()
                    .expect("manager bound the create ticket before returning");
                (session, binding)
            })
            .map_err(|error| error.to_string())
        } else if let Some(transaction) = inline_transaction {
            transaction
                .create_pending_session(
                    shell.clone(),
                    shell_args.clone(),
                    cwd.clone(),
                    agent_id.clone(),
                    agent_label.clone(),
                    git_repos,
                    is_coordinator,
                    backend_kind,
                )
                .await
        } else {
            Err("inline create transaction is unavailable".to_string())
        };
        let (mut session, pending_binding) = match pending_result {
            Ok(created) => created,
            Err(e) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(e);
            }
        };
        if inline_transaction.is_some() {
            *inline_pending_for_body
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(pending_binding);
        }
        if let Some(value) = pending_start_fresh {
            mgr.set_pending_start_fresh_on_restore(pending_binding, value)
                .await
                .map_err(|error| error.to_string())?;
            session.start_fresh_on_restore = value;
        }
        if let Some(communication) = pending_communication.clone() {
            mgr.set_pending_communication(pending_binding, communication.clone())
                .await
                .map_err(|error| error.to_string())?;
            session.communication = Some(communication);
        }

        // #1063 test-only barrier: the pending row now exists and is visible to a
        // deletion's pending-inclusive snapshot; pause here before any config-seed
        // project gate so a delete can interleave deterministically. No-op in production.
        #[cfg(test)]
        {
            seed_race_barriers::hit_before_project_gate(&cwd).await;
        }

        if let Err(e) =
            crate::config::archive_gate::enforce_unarchived_for_spawn(app, &cwd, &session_label)
                .await
        {
            let err = e.to_string();
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            drop(mgr);
            rollback_pre_created_session(app, session_mgr, pty_mgr, session.id, &err).await;
            return Err(err);
        }

        // (#756) Propagate the mirror-forced intent onto the NEW record: the
        // startup-restore path reads ONLY the record, so without this an app close
        // after the forced-fresh reopen would resume the pre-boundary conversation.
        // No persist call here: the command, restart and both restore callers
        // persist after inner returns; the wake/loop/web creates do not, which is
        // fine because the mirror is already persisted and re-consumed by the
        // guard above on every later create, and the record stamp reaches disk
        // with the next persist-any.
        if fresh_forced_by_mirror {
            mgr.set_pending_start_fresh_on_restore(pending_binding, true)
                .await
                .map_err(|error| error.to_string())?;
        }

        if is_root_agent {
            mgr.set_pending_is_root_agent(pending_binding, true)
                .await
                .map_err(|error| error.to_string())?;
            session.is_root_agent = true;
        }

        if let Some(name) = session_name {
            if let Err(e) = mgr
                .rename_pending_session(pending_binding, name.clone())
                .await
            {
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
            let effective_codex_home = effective_codex_home_for_backend(
                session.backend_kind,
                container_path_context.as_ref().map(|context| &context.map),
                spawn,
            );
            mgr.set_pending_profile_metadata(
                pending_binding,
                Some(spawn.profile_resolution.requested_profile.clone()),
                Some(spawn.profile_resolution.effective_profile.clone()),
                spawn.profile_resolution.fallback_chain.clone(),
                spawn.profile_resolution.fallback_applied,
                effective_codex_home.clone(),
                Some(spawn.profile_content_hash.clone()),
            )
            .await
            .map_err(|error| error.to_string())?;
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
            Some(CodingAgentKind::Antigravity) => {
                Some(crate::config::session_context::ManagedContextTarget::Antigravity)
            }
            Some(CodingAgentKind::Pi) => {
                Some(crate::config::session_context::ManagedContextTarget::Pi)
            }
            None => None,
        };

        // Single source of truth: store the identity on the SessionManager record
        // AND the local clone (the latter feeds the imminent `session_created` emit).
        mgr.set_pending_agent_kind(pending_binding, agent_kind)
            .await
            .map_err(|error| error.to_string())?;
        session.agent_kind = agent_kind;
        let trusted_configured_spawn = resolved_spawn.is_some();
        mgr.set_pending_trusted_configured_spawn(pending_binding, trusted_configured_spawn)
            .await
            .map_err(|error| error.to_string())?;
        session.trusted_configured_spawn = trusted_configured_spawn;

        // #930 - resolve the host-credential copy-in plan for container coding agents
        // BEFORE the resume probe and the container env translation, so both consumers
        // observe the CLAUDE_CONFIG_DIR we may inject below. Gated by the global setting
        // (default on) and the per-agent profile descriptor; None when off,
        // non-container, an unrecognized agent, or the host file is absent. The plan is
        // pure (env read + file-exists check); the actual copy runs later in the
        // container backend spawn (spawn_runtime_backed), preserving copy-after-seed.
        let spawn_cwd = container_path_context
            .as_ref()
            .map(|context| context.host_root.clone())
            .unwrap_or_else(|| cwd.clone());
        let container_credential = if session.backend_kind == SessionBackendKind::ContainerTransport
        {
            let copy_enabled = app
                .state::<SettingsState>()
                .read()
                .await
                .container_credentials_from_host;
            if copy_enabled {
                agent_kind
                    .and_then(|k| k.profile().container_credential)
                    .and_then(|src| {
                        crate::pty::container_credentials::resolve_plan(&src, &spawn_cwd)
                    })
            } else {
                None
            }
        } else {
            None
        };
        // #930 - when we WILL copy host creds and the user configured no
        // CLAUDE_CONFIG_DIR (respecting an explicit value OR an explicit removal),
        // default it to the copy directory so a default-on container authenticates with
        // zero env rows. Injected as a host path; the container env translation maps it
        // to /workspace/.claude and the resume probe then treats state as durable.
        let user_has_claude_config_dir = resolved_spawn
            .as_ref()
            .map(|spawn| {
                spawn.effective_env_value(CLAUDE_CONFIG_DIR_KEY).is_some()
                    || spawn.env_remove_keys.iter().any(|remove| {
                        crate::config::settings::normalize_env_key_for_platform(remove)
                            == crate::config::settings::normalize_env_key_for_platform(
                                CLAUDE_CONFIG_DIR_KEY,
                            )
                    })
            })
            .unwrap_or(false);
        let injected_claude_config_dir = injected_claude_config_dir_for_copy(
            container_credential.as_ref(),
            user_has_claude_config_dir,
        );

        // Resolve and inspect Claude transcript storage only for a canonically
        // detected Claude launch. Provider-looking Pi option values must not cause
        // any Claude path resolution, filesystem check, warning, or stored memo.
        let is_claude = agent_kind == Some(CodingAgentKind::Claude);
        let resume_probe = claude_resume_probe_target_for_kind(
            agent_kind,
            session.backend_kind,
            container_path_context.as_ref().map(|context| &context.map),
            resolved_spawn.as_ref(),
            injected_claude_config_dir.as_deref(),
            &shell,
            &shell_args,
            &cwd,
        );
        let resolved_claude_projects_dir = resume_probe
            .as_ref()
            .and_then(|probe| probe.host_probe_path.clone());
        mgr.set_pending_resolved_claude_projects_dir(
            pending_binding,
            resolved_claude_projects_dir.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;
        session.resolved_claude_projects_dir = resolved_claude_projects_dir.clone();
        let claude_project_exists = resume_probe
            .as_ref()
            .and_then(|probe| probe.host_probe_path.as_ref())
            .is_some_and(|path| path.is_dir());
        let mut session_env_warnings: Vec<ContainerEnvWarning> = Vec::new();
        if let Some(warning) = resume_probe
            .as_ref()
            .and_then(|probe| probe.warning.clone())
        {
            session_env_warnings.push(warning);
        }
        let will_inject_continue = should_inject_continue(
            is_claude,
            skip_auto_resume,
            claude_project_exists,
            &full_cmd,
        );
        if let Some(probe) = resume_probe.as_ref() {
            log::info!(
                "[session] claude-resume-decision {} cwd={:?} projects_dir={:?} filesystem={} exists={} skip_auto_resume={} -> inject_continue={}",
                &id.to_string()[..8],
                cwd,
                resolved_claude_projects_dir,
                probe.filesystem,
                claude_project_exists,
                skip_auto_resume,
                will_inject_continue
            );
        } else if let Some(kind) = agent_kind {
            log::info!(
                "[session] resume-decision {} agent={} cwd={:?} skip_auto_resume={}",
                &id.to_string()[..8],
                kind.as_str(),
                cwd,
                skip_auto_resume
            );
        }
        if will_inject_continue {
            // #260: Claude's resume flag from the CodingAgentProfile. resume_tokens
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

        // (#756) Rider: on Claude spawns where AC suppresses resume, mint the fresh
        // session's identity at spawn. Uuid::new_v4() satisfies the "unused UUID"
        // requirement (Claude errors on a colliding --session-id; v4 collision is
        // negligible and a fresh one is minted per spawn). Mutually exclusive with
        // the --continue block above by construction (will_inject_continue requires
        // skip_auto_resume=false). `full_cmd` predates both blocks' mutations, so
        // the veto scan sees user-configured identity flags, never AC's own.
        if should_inject_fresh_session_id(is_claude, skip_auto_resume, &full_cmd) {
            if let Some(ref aid) = agent_id {
                let fresh_session_id = Uuid::new_v4();
                if executable_basename(&shell) == "cmd" {
                    if let Some(last) = shell_args.last_mut() {
                        if executable_basename(last) == "claude"
                            || last.to_lowercase().contains("claude")
                        {
                            *last = format!("{} --session-id {}", last, fresh_session_id);
                            log::info!(
                            "Auto-injected --session-id {} for agent '{}' (fresh spawn, cmd path, #756)",
                            fresh_session_id,
                            aid
                        );
                        }
                    }
                } else {
                    shell_args.push("--session-id".to_string());
                    shell_args.push(fresh_session_id.to_string());
                    log::info!(
                        "Auto-injected --session-id {} for agent '{}' (fresh spawn, #756)",
                        fresh_session_id,
                        aid
                    );
                }
            }
        }

        if maybe_inject_pi_resume(
            agent_kind,
            resolved_spawn.as_ref(),
            skip_auto_resume,
            &shell,
            &mut shell_args,
        ) {
            if let Some(spawn) = resolved_spawn.as_ref() {
                log::info!(
                    "Auto-injected Pi `--continue` for trusted agent '{}'",
                    spawn.trusted_agent_id
                );
            }
        }

        if agent_kind == Some(CodingAgentKind::Codex) && !skip_auto_resume {
            if let Some(ref aid) = agent_id {
                if inject_codex_resume(&shell, &mut shell_args) {
                    log::info!("Auto-injected `codex resume --last` for agent '{}'", aid);
                }
            }
        }

        if agent_kind == Some(CodingAgentKind::Antigravity) && !skip_auto_resume {
            if let Some(ref aid) = agent_id {
                if inject_antigravity_resume(&shell, &mut shell_args) {
                    log::info!("Auto-injected `agy --continue` for agent '{}'", aid);
                }
            }
        }

        // #529 - resolve the instructions filename from the configured coding agent
        // (falling back to detection for ad-hoc launches), plus the union of every
        // configured agent's filename for cleanup. Computed under a single settings
        // read guard that is dropped before any filesystem I/O (no guard across the
        // materialize call).
        let (target_filename, managed_filenames, auto_self_clear): (
            Option<String>,
            Vec<String>,
            bool,
        ) = {
            let settings_state = app.state::<SettingsState>();
            let cfg = settings_state.read().await;
            let managed = crate::config::agent_command::managed_instructions_filenames(&cfg);
            let target = crate::config::agent_command::resolve_target_filename(
                agent_id.as_deref(),
                &cfg,
                context_target,
            );
            // Resolve under the same guard; no second lock is held across I/O.
            // Eligibility is an explicit direct-shell capability, independent of
            // CodingAgentKind provider tuning.
            let auto_self_clear =
                resolve_launch_auto_self_clear(&cfg, &shell, &cwd, is_coordinator);
            (target, managed, auto_self_clear)
        };

        if let Some(ref target_filename) = target_filename {
            let context_result = coordinator
                .run_blocking_seed_work({
                    let cwd = cwd.clone();
                    // #1172 D5: capture the FINAL fresh-versus-resume decision. `skip_auto_resume`
                    // is a `mut` parameter of this function whose only mutation is the #756 mirror
                    // at :1470, ~500 lines above, so the value read here is final. `true` means AC
                    // is deliberately NOT resuming: a fresh conversation begins.
                    let start_fresh = skip_auto_resume;
                    let target_filename = target_filename.clone();
                    let managed_filenames = managed_filenames.clone();
                    let container_repos = container_repos.clone();
                    move || {
                        // #1065 Stage F: production records the session-spawn context
                        // read/sync and self-heal under the per-project gate (acquired
                        // inside the blocking worker so the 10-second poll never blocks a
                        // Tokio worker); a `#[cfg(test)]` lib build stays non-emitting.
                        #[cfg(not(test))]
                        let activation = Some(
                            crate::config::seed_manifest::ManifestActivationToken::production(),
                        );
                        #[cfg(test)]
                        let activation: Option<
                            crate::config::seed_manifest::ManifestActivationToken,
                        > = None;
                        let context_result =
                            crate::config::session_context::materialize_agent_context_file_with_filename_activated(
                                &cwd,
                                &target_filename,
                                &managed_filenames,
                                is_coordinator,
                                auto_self_clear,
                                container_repos.as_ref(),
                                activation.as_ref(),
                            );
                        // #1172 - rotate the origin Agent Matrix's `memory/` for the fresh session
                        // about to start, so it begins with a clean write target and the previous
                        // session's memory is preserved under `memory_<ts>/`.
                        //
                        // Two gates, both load-bearing (D5):
                        //   - `start_fresh`: a RESUME never rotates. This chokepoint also serves the
                        //     app-startup restore path (`create_session_inner_for_restore`, :1239),
                        //     which continues an existing conversation; emptying `memory/` under a
                        //     resumed agent is the one outcome the user ruled out.
                        //   - `is_ok()`: a launch that is about to roll back (:2016-2036) leaves no
                        //     rotation behind.
                        //
                        // Never fails a launch: `rotate_origin_memory_at_spawn` returns `()` and every
                        // error path inside it warns and returns.
                        if start_fresh && context_result.is_ok() {
                            crate::config::agent_memory::rotate_origin_memory_at_spawn(&cwd);
                        }
                        context_result
                    }
                })
                .await;
            let context_result = match context_result {
                Ok(result) => result,
                Err(error) => {
                    let error = format!("replica context blocking preparation failed: {error}");
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            };
            match context_result {
                Ok(_) => {}
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
        }

        // Capture the effective arg vector BEFORE spawn so SessionInfo::from(&session)
        // (emitted at line ~439 as "session_created") carries provider resume flags.
        // Bind once, broadcast to two consumers: the store write is for later
        // `mgr.get_session` callers; the local-clone write is for the imminent emit.
        //
        // DO NOT REMOVE OR GATE THIS CAPTURE. Issue #65 regression guard: removing
        // or wrapping in a condition reintroduces the exact bug this plan fixes.
        // See _plans/bug-statusbar-dynamic-launch-args.md §10 and §15 for rationale.
        let effective = shell_args.clone();
        mgr.set_pending_effective_shell_args(pending_binding, effective.clone())
            .await
            .map_err(|error| error.to_string())?;
        session.effective_shell_args = Some(effective);

        let extra_env = if agent_id.is_some() {
            crate::pty::credentials::build_credentials_env(&session.token, &cwd)
        } else {
            Vec::new()
        };
        let mut configured_env: Vec<(String, String)> = resolved_spawn
            .as_ref()
            .map(|spawn| spawn.child_env.clone())
            .unwrap_or_default();
        // #930 - inject the copy directory as a host-path CLAUDE_CONFIG_DIR so the
        // container env translation below maps it to /workspace/.claude and the copied
        // token is actually read. Only present when we will copy and the user set none.
        if let Some(dir) = injected_claude_config_dir.as_ref() {
            configured_env.push((CLAUDE_CONFIG_DIR_KEY.to_string(), dir.clone()));
        }
        let env_remove_keys: Vec<String> = resolved_spawn
            .as_ref()
            .map(|spawn| spawn.env_remove_keys.clone())
            .unwrap_or_default();
        let mut env_unset: Vec<String> = Vec::new();
        if session.backend_kind == SessionBackendKind::ContainerTransport {
            if let Some(context) = container_path_context.as_ref() {
                let translated = crate::pty::container_backend::container_child_env(
                    configured_env,
                    env_remove_keys.clone(),
                    &context.map,
                );
                configured_env = translated.child_env;
                env_unset = translated.env_unset;
                session_env_warnings.extend(translated.warnings);
            }
        }
        let (resource_registration, logical_resource_slot): (
            Option<ResourceLaunchRegistration>,
            Option<ResourceLogicalAgentSlot>,
        ) = match session.backend_kind {
            SessionBackendKind::LocalProcess => resource_permit
                .take()
                .map(|permit| {
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
                })
                .map(|registration| (Some(registration), None))
                .unwrap_or((None, None)),
            SessionBackendKind::ContainerTransport => (
                None,
                resource_permit
                    .take()
                    .and_then(|permit| resource_monitor.hold_logical_agent_slot(permit)),
            ),
        };

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
            // leak-reclaim sweep stays safe. Clone the Arc out of State first so the
            // owned guard does not borrow a State temporary (E0716).
            let seed = seed.clone();
            let lock = app.state::<crate::ConfigSeedLockState>().inner().clone();
            let seed_result = coordinator
                .run_blocking_seed_work(move || {
                    let _seed_guard = lock.blocking_lock_owned();
                    // #1065 Stage F: production records the config-seed publication
                    // under the per-project gate, acquired AFTER this global lock
                    // (plan section 6.3 lock order) and dropped before returning; a
                    // `#[cfg(test)]` lib build stays non-emitting.
                    #[cfg(not(test))]
                    let activation =
                        Some(crate::config::seed_manifest::ManifestActivationToken::production());
                    #[cfg(test)]
                    let activation: Option<
                        crate::config::seed_manifest::ManifestActivationToken,
                    > = None;
                    crate::config::config_seed::perform_config_seed_recorded(
                        &seed,
                        &id.to_string(),
                        activation.as_ref(),
                    )
                })
                .await;
            match seed_result {
                Ok(report) => {
                    log::debug!("[config-seed] session {} outcome: {:?}", id, report);
                }
                Err(error) => {
                    let error = format!("config seed blocking preparation failed: {error}");
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            }
        }

        // #1063 test-only barrier: the seed transaction (if any) has acquired and
        // released the project gate, but the create is still pending and unfinalized,
        // so a deletion's pending-inclusive snapshot must still observe it. No-op in
        // production.
        #[cfg(test)]
        {
            seed_race_barriers::hit_after_seed_before_pty(&cwd).await;
        }

        let viewport = viewport.unwrap_or(PtyViewport::DEFAULT);
        let spawn_spec = BackendSpawnSpec {
            id,
            agent_id: agent_id.clone(),
            // #942 - the CLI identity from the canonical detector (`agent_kind`, resolved
            // at the top of this function). The profile id above cannot stand in for it:
            // it is opaque, several profiles can drive the same CLI, and a coding-agent
            // launch can carry no profile id at all.
            coding_agent: agent_kind,
            cmd: shell.clone(),
            args: shell_args.clone(),
            // #1271 - the configured host shell paired with a resolved agent
            // command, carried as one immutable snapshot (never as loose
            // program/argument fields, so the pairing invariant cannot drift).
            resolved_agent_host_shell: resolved_agent_host_shell.clone(),
            cwd: spawn_cwd.clone(),
            selected_cwd: if spawn_cwd != cwd {
                Some(cwd.clone())
            } else {
                None
            },
            // #973 - open the PTY at the size the terminal is actually going to be, so the
            // frontend never has to resize a child that is still starting up. Callers with no
            // view (restore loop, delivery loop, mailbox, CLI, tests) get PtyViewport::DEFAULT,
            // which is the 120x30 AC always used.
            cols: viewport.cols,
            rows: viewport.rows,
            container_image: resolved_spawn
                .as_ref()
                .and_then(|spawn| spawn.backend.image.clone()),
            configured_env,
            env_remove_keys,
            env_unset,
            extra_env,
            idle_tuning: crate::session::profile::idle_tuning_for(agent_kind),
            output_target: PtyOutputTarget::from_app_handle(app.clone()),
            resource_registration,
            logical_resource_slot,
            container_credential,
            container_repo_mounts: container_repos
                .as_ref()
                .map(|resolution| {
                    resolution
                        .mounts()
                        .map(
                            |(host, container)| crate::pty::container_repos::ContainerRepoMount {
                                host_path: host.to_path_buf(),
                                container_path: container.to_string(),
                            },
                        )
                        .collect()
                })
                .unwrap_or_default(),
        };
        let spawn_result = PtyManager::spawn(pty_mgr, session.backend_kind, spawn_spec).await;
        drop(spawn_mark);
        if let Err(e) = spawn_result {
            let err = e.to_string();
            drop(mgr);
            rollback_pre_created_session(app, session_mgr, pty_mgr, id, &err).await;
            return Err(err);
        }

        register_session_samplers(app, id, agent_id.clone());

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

                        let session_mgr =
                            app_clone.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
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

                    match crate::pty::inject::inject_text_into_session(
                        &app_clone, session_id, &prompt,
                    )
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

        let finalization_warnings = session_env_warnings
            .iter()
            .map(|warning| {
                SessionWarning::new(
                    id,
                    warning.key.clone(),
                    warning.kind,
                    warning.message.clone(),
                )
            })
            .collect::<Vec<_>>();

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

        if defer_inline_finalization {
            if create_ticket.is_some() || inline_transaction.is_none() {
                return Err(
                    "deferred create finalization requires an inline transaction".to_string(),
                );
            }
            let info = mgr
                .get_pending_session(pending_binding)
                .await
                .map(|record| SessionInfo::from(&record))
                .ok_or_else(|| "deferred create pending record disappeared".to_string())?;
            inline_pending_for_body
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            return Ok(CreateCompletion::Deferred(DeferredCreateOutput {
                info,
                binding: pending_binding,
                warnings: finalization_warnings,
            }));
        }

        let finalized = if let Some(ticket) = create_ticket.take() {
            ticket.finalize(finalization_warnings).await
        } else {
            let transaction = inline_transaction
                .ok_or_else(|| "inline create transaction is unavailable".to_string())?;
            let cause =
                inline_cause.ok_or_else(|| "inline create cause is unavailable".to_string())?;
            transaction
                .finalize_inline_create(pending_binding, cause, finalization_warnings)
                .await
        };
        if finalized.is_ok() && inline_transaction.is_some() {
            inline_pending_for_body
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
        }
        finalized.map(CreateCompletion::Finalized)
    };
    let result = tokio::select! {
        biased;
        _ = coordinator_shutdown.cancelled() => {
            Err("selectionCoordinatorUnavailable".to_string())
        }
        result = create_body => result,
    };
    if result.is_err() {
        let binding = {
            inline_pending_binding
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
        };
        if let (Some(transaction), Some(binding)) = (inline_transaction, binding) {
            transaction.rollback_inline_create(binding).await;
        }
    }
    result
}

fn resolve_launch_auto_self_clear(
    settings: &AppSettings,
    shell: &str,
    cwd: &str,
    is_coordinator: bool,
) -> bool {
    if !crate::pty::inject::supports_auto_self_maintenance(shell) {
        return false;
    }
    let class_default_on = is_coordinator || crate::config::root_agent::is_root_agent_dir_name(cwd);
    let agent_name =
        crate::config::coding_agent_profiles::agent_name_from_dir(std::path::Path::new(cwd))
            .unwrap_or_default();
    crate::config::settings::resolve_auto_self_clear(settings, &agent_name, class_default_on)
}

pub(crate) fn resolve_configured_agent_spawn_for_cwd(
    settings: &AppSettings,
    agent_id: &str,
    cwd: &str,
    requested_profile: Option<&str>,
) -> Result<Option<AgentSpawnCommand>, String> {
    if !settings.agents.iter().any(|agent| agent.id == agent_id) {
        return Ok(None);
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(cwd);
    crate::config::agent_command::resolve_agent_spawn_command(
        settings,
        agent_id,
        Some(std::path::Path::new(&cwd)),
        requested_profile,
    )
    .map(Some)
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
    let cwd = crate::path_utils::normalize_windows_verbatim_path(cwd);
    crate::config::agent_command::build_agent_spawn_command(
        settings,
        agent_id,
        Some(std::path::Path::new(&cwd)),
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

/// §1295 S5 — resolve the create-command working directory. The bare
/// `invoke("create_session", {})` default (cwd: None) falls back to the home
/// dir. In production the creation gate then REJECTS that default when home is
/// not a registered project root (an intentional, documented UX change, §5.8);
/// every shipped frontend call site passes an explicit registered cwd.
pub(crate) fn resolve_create_session_cwd(cwd: Option<String>) -> String {
    let cwd = cwd.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\".to_string())
    });
    crate::path_utils::normalize_windows_verbatim_path(&cwd)
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
    // #973 - the terminal size the view has already fitted to. Optional: an older frontend,
    // or any caller that has no tile to measure, simply omits it and gets AC's 120x30.
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<SessionInfo, String> {
    let cfg = settings.read().await;

    let cwd = resolve_create_session_cwd(cwd);

    let resolved_spawn = if let Some(aid) = agent_id.as_deref() {
        build_configured_agent_spawn_for_cwd(&cfg, aid, &cwd, requested_profile.as_deref())?
    } else {
        None
    };

    let (shell, shell_args, agent_label, resolved_agent_host_shell) =
        if let Some(spawn) = resolved_spawn.as_ref() {
            // #1271 - keep the resolved agent executable and its logical argv as
            // the command-to-run, and carry a copy of the configured default host
            // shell (program + args from the SAME immutable config snapshot, before
            // `drop(cfg)`) separately to the backend. The backend launches a
            // non-direct Windows agent command through this host shell instead of
            // the unconditional `cmd.exe /C` fallback.
            (
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                Some(spawn.trusted_agent_label.clone()),
                Some(ResolvedAgentHostShell {
                    program: cfg.default_shell.clone(),
                    args: cfg.default_shell_args.clone(),
                }),
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
            (s, sa, al, None)
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
        resolved_agent_host_shell,
        // #973 - the only caller that has a terminal to measure.
        match (cols, rows) {
            (Some(c), Some(r)) => Some(PtyViewport::from_fit(c, r)),
            _ => None,
        },
        CreateSelectionIntent::User,
    )
    .await?;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestructionSource {
    ManualClose,
    AutoClose,
    SpawnRollback,
    BackgroundCleanup,
}

impl DestructionSource {
    fn cause(self) -> SelectionCause {
        match self {
            Self::ManualClose => SelectionCause::ManualClose,
            Self::AutoClose => SelectionCause::AutoClose,
            Self::SpawnRollback => SelectionCause::SpawnRollback,
            Self::BackgroundCleanup => SelectionCause::BackgroundCleanup,
        }
    }

    fn selection_source(self) -> SelectionSource {
        match self {
            Self::ManualClose => SelectionSource::ManualClose,
            Self::AutoClose => SelectionSource::AutoClose,
            Self::SpawnRollback => SelectionSource::SpawnRollback,
            Self::BackgroundCleanup => SelectionSource::BackgroundCleanup,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DestroyRequest {
    pub ids: Vec<Uuid>,
    pub source: DestructionSource,
    pub force_destroy_root: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DestroyOutcome {
    pub destroyed_ids: Vec<Uuid>,
    pub retained_exited_ids: Vec<Uuid>,
    pub failed: Vec<(Uuid, String)>,
}

impl DestroyOutcome {
    pub fn succeeded(&self, id: Uuid) -> bool {
        self.destroyed_ids.contains(&id) || self.retained_exited_ids.contains(&id)
    }

    fn into_single_result(self, id: Uuid) -> Result<(), String> {
        if self.succeeded(id) {
            return Ok(());
        }
        let message = self
            .failed
            .into_iter()
            .find(|(failed_id, _)| *failed_id == id)
            .map(|(_, message)| message)
            .unwrap_or_else(|| "Session not found".to_string());
        Err(message)
    }
}

pub async fn destroy_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
) -> Result<(), String> {
    destroy_sessions_with_source(app, vec![uuid], DestructionSource::ManualClose, false)
        .await?
        .into_single_result(uuid)
}

pub(crate) async fn background_destroy_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
) -> Result<(), String> {
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    match coordinator.background_destroy(uuid).await? {
        CriticalAdmissionOutcome::Completed(outcome) => outcome.into_single_result(uuid),
        CriticalAdmissionOutcome::AlreadyPending => Ok(()),
    }
}

pub(crate) async fn destroy_sessions_with_source<R: tauri::Runtime>(
    app: &AppHandle<R>,
    ids: Vec<Uuid>,
    source: DestructionSource,
    force_destroy_root: bool,
) -> Result<DestroyOutcome, String> {
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    coordinator
        .destroy(DestroyRequest {
            ids,
            source,
            force_destroy_root,
        })
        .await
}

pub(crate) async fn execute_destroy_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: DestroyRequest,
) -> Result<DestroyOutcome, String> {
    let planned = request
        .ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let before = transaction.aggregate_snapshot().await;
    let selected_id = before.selection.id();
    let mut outcome = DestroyOutcome::default();
    let mut mutations = LifecycleMutations::default();
    let mut retained_marked = std::collections::HashSet::new();

    for session_id in request.ids.iter().copied() {
        let Some(existing) = before
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
        else {
            outcome
                .failed
                .push((session_id, "Session not found".to_string()));
            continue;
        };
        let is_root_agent = existing.is_root_agent
            || crate::config::root_agent::is_root_agent_path(&existing.working_directory);

        {
            let detached = transaction.app().state::<DetachedSessionsState>();
            detached
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&session_id);
        }

        let bridge_shutdown = {
            let telegram = transaction.app().state::<TelegramBridgeState>();
            let mut telegram = telegram.lock().await;
            if telegram.has_bridge(session_id) {
                telegram.detach(session_id).ok()
            } else {
                None
            }
        };
        if let Some(shutdown) = bridge_shutdown {
            transaction
                .manager()
                .await
                .set_telegram_bot_id(session_id, None)
                .await;
            if let Err(error) = transaction.app().emit(
                "telegram_bridge_detached",
                serde_json::json!({ "sessionId": session_id.to_string() }),
            ) {
                log::warn!(
                    "[session] telegram detach publication failed session={} source={:?}: {}",
                    session_id,
                    request.source,
                    error
                );
            }
            shutdown.spawn_wait_or_abort();
        }

        let resource_monitor = transaction
            .app()
            .state::<Arc<ResourceMonitorState>>()
            .inner()
            .clone();
        if resource_monitor.has_registered_group(session_id) {
            {
                let pty = transaction.app().state::<Arc<Mutex<PtyManager>>>();
                pty.lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .publish_stop_witness(session_id, "session-destroy");
            }
            let monitor = Arc::clone(&resource_monitor);
            match tokio::task::spawn_blocking(move || {
                monitor.kill_group(
                    session_id,
                    crate::resource_monitor::ResourceKillReason::SessionDestroy,
                )
            })
            .await
            {
                Ok(Ok(result)) => {
                    if result.quarantined {
                        log::warn!(
                            "[session] resource cleanup quarantined session={} source={:?}: {}",
                            session_id,
                            request.source,
                            result.message
                        );
                    }
                }
                Ok(Err(error)) => log::warn!(
                    "[session] resource cleanup failed session={} source={:?}: {}",
                    session_id,
                    request.source,
                    error
                ),
                Err(error) => log::warn!(
                    "[session] resource cleanup task failed session={} source={:?}: {}",
                    session_id,
                    request.source,
                    error
                ),
            }
        }

        // The std-Mutex guard from this `let` initializer is dropped at the `;`
        // before `runtime_snapshot` re-locks the same pty Mutex below. Moving this
        // into an `if let`/`match` scrutinee or an inner blocking block would hold
        // the guard across that re-lock and re-introduce the re-entrant std-Mutex
        // deadlock fixed in resource_monitor (§1295).
        let kill_result = transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .kill(session_id);
        let still_has_pty = transaction.runtime_snapshot(session_id).has_pty;
        if let Err(error) = kill_result {
            if still_has_pty {
                outcome.failed.push((session_id, error.to_string()));
                continue;
            }
            log::warn!(
                "[session] teardown reported an error after PTY loss session={} source={:?}: {}",
                session_id,
                request.source,
                error
            );
        }

        if is_root_agent && !request.force_destroy_root {
            mutations.set_detached_intent(session_id, false);
            if !matches!(existing.status, SessionStatus::Exited(_)) {
                mutations.mark_exited(session_id, 0);
                retained_marked.insert(session_id);
            }
            outcome.retained_exited_ids.push(session_id);
        } else {
            mutations.remove(session_id);
            outcome.destroyed_ids.push(session_id);
        }
    }

    let selected_was_torn_down = selected_id.is_some_and(|selected| outcome.succeeded(selected));
    let decision = if !selected_was_torn_down {
        CommitDecision::Keep
    } else {
        match request.source {
            DestructionSource::ManualClose => {
                let final_snapshot = transaction.aggregate_snapshot().await;
                let mut fallback = None;
                for candidate_id in &final_snapshot.order {
                    let Some(candidate) = final_snapshot
                        .sessions
                        .iter()
                        .find(|candidate| candidate.id == *candidate_id)
                    else {
                        continue;
                    };
                    if planned.contains(&candidate.id) {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty=false detached=false excluded=true reason=plannedForDestruction",
                            candidate.id,
                            candidate.status
                        );
                        continue;
                    }
                    if matches!(candidate.status, SessionStatus::Exited(_)) {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty=false detached=false excluded=false reason=exited",
                            candidate.id,
                            candidate.status
                        );
                        continue;
                    }
                    let runtime = transaction.runtime_snapshot(candidate.id);
                    if runtime.detached {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty={} detached=true excluded=false reason=detached",
                            candidate.id,
                            candidate.status,
                            runtime.has_pty
                        );
                        continue;
                    }
                    if !runtime.has_pty {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty=false detached=false excluded=false reason=missingPty",
                            candidate.id,
                            candidate.status
                        );
                        continue;
                    }
                    fallback = transaction.live_decision(candidate.id);
                    if fallback.is_some() {
                        break;
                    }
                }
                fallback.unwrap_or(CommitDecision::Clear)
            }
            DestructionSource::AutoClose
            | DestructionSource::SpawnRollback
            | DestructionSource::BackgroundCleanup => CommitDecision::Clear,
        }
    };

    let committed = transaction
        .commit(decision, request.source.cause(), mutations)
        .await?;
    if !outcome.destroyed_ids.is_empty() || !outcome.retained_exited_ids.is_empty() {
        transaction
            .persist(request.source.selection_source(), selected_id)
            .await;
    }

    for session_id in outcome
        .destroyed_ids
        .iter()
        .chain(outcome.retained_exited_ids.iter())
        .copied()
    {
        transaction.publish_destroyed(session_id);
        let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
        if let Some(window) = transaction.app().get_webview_window(&label) {
            if let Err(error) = window.destroy() {
                log::warn!(
                    "[session] detached window destroy failed session={} source={:?}: {}",
                    session_id,
                    request.source,
                    error
                );
            }
        }
        reset_substantive_input(transaction.app(), session_id);
    }

    // #1171 - a SEPARATE loop over `destroyed_ids` only, rather than a membership test inside
    // the loop above. That loop iterates `destroyed_ids.chain(retained_exited_ids)`, so a
    // `contains` check would be O(n squared) and, worse, would express a business rule as a
    // condition inside a loop that does something else. A separate loop makes "destroyed only"
    // structural.
    //
    // Root-agent sessions retained as `Exited` are deliberately NOT purged: they stay in the
    // manager and their row is still in the list, so their post-mortem view still has a place
    // to be shown.
    for session_id in outcome.destroyed_ids.iter().copied() {
        purge_session_side_state(transaction.app(), session_id);
    }

    for row in committed.changed_rows.iter().filter(|row| {
        Uuid::parse_str(&row.id)
            .ok()
            .is_some_and(|id| retained_marked.contains(&id))
    }) {
        transaction.publish_created(row);
    }
    for session_id in &committed.cleared_raise_hand_ids {
        transaction.publish_communication_cleared(*session_id);
    }
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }

    Ok(outcome)
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

async fn execute_manual_coordinator_destroy<R: tauri::Runtime>(
    app: &AppHandle<R>,
    coordinator_id: Uuid,
    member_ids: Vec<Uuid>,
    cascade: bool,
) -> Result<DestroyOutcome, String> {
    let mut planned_ids = if cascade { member_ids } else { Vec::new() };
    planned_ids.push(coordinator_id);
    let outcome =
        destroy_sessions_with_source(app, planned_ids, DestructionSource::ManualClose, false)
            .await?;
    for (failed_id, error) in &outcome.failed {
        if *failed_id != coordinator_id {
            log::warn!(
                "[manual-close] member destroy {} failed: {}",
                &failed_id.to_string()[..8],
                error
            );
        }
    }
    outcome.clone().into_single_result(coordinator_id)?;
    Ok(outcome)
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

    // Submit the complete cascade as one destruction transaction. The full
    // planned set is excluded from fallback ranking, member failures are known
    // before the one final manual selection, and no doomed sibling can become an
    // intermediate canonical target.
    execute_manual_coordinator_destroy(&app, uuid, live_ids, cascade).await?;

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

/// (#747) Decide whether a raised hand carries across the restart
/// destroy+create boundary. Carry ONLY a visible `RaiseHand` and ONLY when the
/// conversation resumes (`restart_start_fresh == false`, i.e. the Branch-A
/// reopen of a dormant session). A fresh restart starts a new conversation;
/// the raise belonged to the old one and is dropped, consistent with
/// "destroyed sessions leave no stale raised-hand state" (#747 AC 4).
///
/// Seam map: the explicit "Restart Session" command and the bulk
/// profile-assignment restart (commands/config.rs:627-637) pass fresh and drop
/// (user-initiated resets); the sidebar dormant reopen passes `Some(false)`
/// and carries; the startup wake arm reuses this helper with the persisted
/// intent (#747 plan change 3); the agent-initiated self-clear-and-handoff
/// restart carries via its own capture/re-apply (#747 plan change 10) because
/// it is not user input. `pub(crate)` so lib.rs's wake arm shares the exact
/// same decision.
pub(crate) fn carry_communication_for_restart(
    stored: Option<SessionCommunication>,
    restart_start_fresh: bool,
) -> Option<SessionCommunication> {
    if restart_start_fresh {
        return None;
    }
    stored.filter(|c| c.kind == SessionCommunicationKind::RaiseHand && c.visible)
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

/// (#756) Create-path mirror consume: force a fresh spawn when the caller
/// requested resume but the cwd's coordinator carries a pending fresh boundary.
/// Named + unit-tested per the #630 pattern (the call site is guarded against a
/// silent inline revert by the `dead_code` lint under CI's `-D warnings`).
fn mirror_forces_fresh(
    requested_skip_auto_resume: bool,
    is_coordinator: bool,
    mirror_pending: bool,
) -> bool {
    !requested_skip_auto_resume && is_coordinator && mirror_pending
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

#[derive(Debug, Clone)]
pub(crate) struct RestartJobRequest {
    pub session_id: Uuid,
    pub agent_id: Option<String>,
    pub requested_profile: Option<String>,
    pub skip_auto_resume: Option<bool>,
    pub activate_after: bool,
    pub intent: TrustedRestartIntent,
    pub communication_override: Option<SessionCommunication>,
    /// §1295 5.1b — creation-gate enforcement for the restart replacement.
    /// Root-agent and archived-root cwds are exempt by the gate itself, so
    /// callers pass the build default; the mailbox wake-restart passes
    /// `Enforce` explicitly (§1295 6.6 / mailbox.rs:9796).
    pub enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
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
/// `antigravity --continue` so the prior conversation continues.
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
    restart_session_inner_with_intent(
        app,
        session_mgr,
        pty_mgr,
        settings,
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        activate_after,
        TrustedRestartIntent::User,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn restart_session_inner_with_intent<R: tauri::Runtime>(
    app: &AppHandle<R>,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    _settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
    activate_after: bool,
    intent: TrustedRestartIntent,
    communication_override: Option<SessionCommunication>,
    enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
) -> Result<SessionInfo, String> {
    app.state::<SelectionCoordinator>()
        .restart_lifecycle(RestartJobRequest {
            session_id: uuid,
            agent_id,
            requested_profile,
            skip_auto_resume,
            activate_after,
            intent,
            communication_override,
            enforcement,
        })
        .await
}

struct RestartTeardownError {
    message: String,
    old_lost: bool,
}

async fn detach_restart_telegram<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    let bridge_shutdown = {
        let telegram = transaction.app().state::<TelegramBridgeState>();
        let mut telegram = telegram.lock().await;
        if telegram.has_bridge(session_id) {
            telegram.detach(session_id).ok()
        } else {
            None
        }
    };
    if let Some(shutdown) = bridge_shutdown {
        if let Err(error) = transaction.app().emit(
            "telegram_bridge_detached",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        ) {
            log::warn!(
                "[restart] telegram detach publication failed session={}: {}",
                session_id,
                error
            );
        }
        shutdown.spawn_wait_or_abort();
    }
}

async fn teardown_old_for_restart<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    old_was_dormant: bool,
) -> Result<bool, RestartTeardownError> {
    if !transaction.runtime_snapshot(session_id).has_pty {
        if old_was_dormant {
            return Ok(false);
        }
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        detach_restart_telegram(transaction, session_id).await;
        return Ok(true);
    }

    let resource_monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();
    if resource_monitor.has_registered_group(session_id) {
        transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .publish_stop_witness(session_id, "session-restart");
        let monitor = Arc::clone(&resource_monitor);
        match tokio::task::spawn_blocking(move || {
            monitor.kill_group(
                session_id,
                crate::resource_monitor::ResourceKillReason::SessionDestroy,
            )
        })
        .await
        {
            Ok(Ok(result)) if result.quarantined => log::warn!(
                "[restart] resource cleanup quarantined session={}: {}",
                session_id,
                result.message
            ),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => log::warn!(
                "[restart] resource cleanup failed session={}: {}",
                session_id,
                error
            ),
            Err(error) => log::warn!(
                "[restart] resource cleanup task failed session={}: {}",
                session_id,
                error
            ),
        }
    }

    // The std-Mutex guard from this `let` initializer is dropped at the `;` before
    // `runtime_snapshot` re-locks the same pty Mutex below. Moving this into an
    // `if let`/`match` scrutinee or an inner blocking block would hold the guard
    // across that re-lock and re-introduce the re-entrant std-Mutex deadlock fixed
    // in resource_monitor (§1295).
    let kill_result = transaction
        .app()
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .kill(session_id);
    let old_lost = !transaction.runtime_snapshot(session_id).has_pty;
    if !old_lost {
        let message = kill_result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "old PTY remained live after restart teardown".to_string());
        return Err(RestartTeardownError {
            message,
            old_lost: false,
        });
    }
    if let Err(error) = kill_result {
        log::warn!(
            "[restart] teardown reported an error after PTY loss session={}: {}",
            session_id,
            error
        );
    }
    transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&session_id);
    detach_restart_telegram(transaction, session_id).await;
    Ok(true)
}

/// #1171 - this covers BOTH restart paths: `execute_restart_transaction`'s success path and
/// `finalize_failed_restart`. A restart is a normal flow - it is in the invoke handler, the
/// mailbox calls it, and a bulk settings save can restart sessions - so without the purge here
/// every restart would orphan up to 500 ring entries under an id that is already gone from the
/// session list, unreachable from the UI.
fn publish_restart_destroyed<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    transaction.publish_destroyed(session_id);
    let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
    if let Some(window) = transaction.app().get_webview_window(&label) {
        if let Err(error) = window.destroy() {
            log::warn!(
                "[restart] detached window destroy failed session={}: {}",
                session_id,
                error
            );
        }
    }
    purge_session_side_state(transaction.app(), session_id);
}

async fn finalize_failed_restart<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    selected: bool,
    intent: TrustedRestartIntent,
) -> Result<(), String> {
    detach_restart_telegram(transaction, session_id).await;
    let mut mutations = LifecycleMutations::default();
    mutations.remove(session_id);
    let committed = transaction
        .commit(
            if selected {
                CommitDecision::Clear
            } else {
                CommitDecision::Keep
            },
            SelectionCause::Restart(intent),
            mutations,
        )
        .await?;
    transaction
        .persist(SelectionSource::Restart, Some(session_id))
        .await;
    publish_restart_destroyed(transaction, session_id);
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }
    Ok(())
}

pub(crate) async fn execute_restart_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RestartJobRequest,
) -> Result<SessionInfo, String> {
    let app = transaction.app();
    let session_mgr = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .inner()
        .clone();
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let settings = app.state::<SettingsState>().inner().clone();
    let RestartJobRequest {
        session_id: uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        activate_after,
        intent,
        communication_override,
        enforcement,
    } = request;
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
        stored_communication,
        old_status,
        old_selected,
    ) = {
        let mgr = session_mgr.read().await;
        let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
        let old_selected = mgr.selection_payload().await.id() == Some(uuid);
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
            session.communication.clone(),  // (#747) candidate raised-hand carry across the reopen
            session.status.clone(),
            old_selected,
        )
    };

    let cwd = if is_root_agent {
        crate::config::root_agent::ensure_root_agent_dir()?
    } else {
        cwd
    };
    let cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);
    crate::config::archive_gate::probe_spawn_refusal(app, &cwd).await?;
    // §1295 5.1b / B2: the creation gate runs AFTER `probe_spawn_refusal` but
    // WELL BEFORE `teardown_old_for_restart` (below). A rejected restart of a
    // live outside-roots session therefore DECLINES: the old session stays
    // running (live PTY untouched), `Err(sessionCreateBlocked: ...)` propagates,
    // and no `finalize_failed_restart` runs. Root-agent restarts already
    // normalized `cwd` to `ensure_root_agent_dir()` above, which the gate
    // exempts by rule 1.
    crate::config::sessions_persistence::enforce_creation_gate(app, &cwd, enforcement).await?;

    // 2. Strip auto-injected args before restart so the new session starts from the saved recipe.
    let clean_args =
        crate::config::sessions_persistence::strip_auto_injected_args(&shell, &shell_args);

    let requested_agent_id = agent_id;
    let selected_requested_profile =
        effective_restart_requested_profile(requested_profile, stored_requested_profile);
    // #537 read-side: resolve the launch agent (honoring currentCodingAgent) and
    // build its spawn under a single settings read guard. No await is held across
    // the guard; it is dropped at the end of this block. The #1271 host-shell
    // snapshot is copied from the SAME guard so program and args can never pair
    // across a configuration change.
    let (selected_agent_id, resolved_spawn, resolved_agent_host_shell) = {
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
        let resolved_agent_host_shell = if resolved_spawn.is_some() {
            Some(ResolvedAgentHostShell {
                program: cfg.default_shell.clone(),
                args: cfg.default_shell_args.clone(),
            })
        } else {
            None
        };
        (selected_agent_id, resolved_spawn, resolved_agent_host_shell)
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

    // (#630/#631) Fresh on an explicit "Restart Session" (None -> true), OR if the
    // session still carries an unconsumed durable fresh intent. Root agents are
    // scoped out (separate restore path ignores the marker, §8): they are never
    // stamped below, so `stored_start_fresh` is false for root and this reduces to
    // today's `effective_restart_skip_auto_resume`.
    let restart_start_fresh =
        restart_skip_auto_resume_with_intent(stored_start_fresh, skip_auto_resume);
    let carried_communication = communication_override
        .or_else(|| carry_communication_for_restart(stored_communication, restart_start_fresh));

    // 3. Release the old runtime slot while retaining its canonical manager row.
    // Dormant rows skip teardown and remain retryable until replacement success.
    let old_was_dormant = matches!(old_status, SessionStatus::Exited(_));
    let old_lost = match teardown_old_for_restart(transaction, uuid, old_was_dormant).await {
        Ok(old_lost) => old_lost,
        Err(error) => {
            if error.old_lost {
                if let Err(finalize_error) =
                    finalize_failed_restart(transaction, uuid, old_selected, intent).await
                {
                    return Err(format!(
                        "{}; restart failure finalization failed: {}",
                        error.message, finalize_error
                    ));
                }
            }
            return Err(error.message);
        }
    };

    // 4. Spawn the replacement as a pending, unannounced row.
    let completion = create_session_inner_impl(
        app,
        &session_mgr,
        &pty_mgr,
        shell,
        shell_args,
        cwd.clone(),
        Some(name),
        selected_agent_id,
        agent_label,
        false,
        git_repos,
        restart_start_fresh,
        resolved_spawn,
        resolved_agent_host_shell,
        None,
        CreateSelectionIntent::Suppress,
        (!is_root_agent).then_some(restart_start_fresh),
        carried_communication,
        Some(transaction),
        Some(SelectionCause::Restart(intent)),
        true,
        None,
        enforcement,
    )
    .await;
    let deferred = match completion {
        Ok(CreateCompletion::Deferred(deferred)) => deferred,
        Ok(CreateCompletion::Finalized(_)) => {
            let error = "restart replacement was published before atomic finalization".to_string();
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(error);
        }
        Err(error) => {
            if old_lost {
                if let Err(finalize_error) =
                    finalize_failed_restart(transaction, uuid, old_selected, intent).await
                {
                    return Err(format!(
                        "{}; restart failure finalization failed: {}",
                        error, finalize_error
                    ));
                }
            }
            return Err(error);
        }
    };
    let new_uuid = match Uuid::parse_str(&deferred.info.id) {
        Ok(id) => id,
        Err(error) => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(format!(
                "restart replacement returned an invalid id: {error}"
            ));
        }
    };
    let live = match transaction.live_decision(new_uuid) {
        Some(CommitDecision::Live(live)) => live,
        _ => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err("restart replacement is not displayable".to_string());
        }
    };

    // 5. Finalize the replacement, remove the old row, and decide selection in
    // one manager commit. Neither lifecycle row was public before this point.
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(deferred.binding, live);
    mutations.remove(uuid);
    let decision = if old_selected || activate_after {
        CommitDecision::Live(live)
    } else {
        CommitDecision::Keep
    };
    let committed = match transaction
        .commit(decision, SelectionCause::Restart(intent), mutations)
        .await
    {
        Ok(committed) => committed,
        Err(error) => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(error);
        }
    };
    let session_info = committed
        .finalized_rows
        .iter()
        .find(|row| row.id == new_uuid.to_string())
        .cloned()
        .ok_or_else(|| "restart finalization did not produce the replacement row".to_string())?;
    transaction
        .persist(SelectionSource::Restart, Some(new_uuid))
        .await;
    transaction.publish_created(&session_info);
    if !old_lost {
        detach_restart_telegram(transaction, uuid).await;
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&uuid);
    }
    publish_restart_destroyed(transaction, uuid);
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }
    for warning in deferred.warnings {
        emit_session_warning(app, warning);
    }

    // (#756) C3: mirror the honored fresh intent into the coordinator clocks so
    // it survives record destruction (idle auto-close, manual close destroy the
    // record that carries start_fresh_on_restore). Self-gates on coordinators
    // inside the helper; root agents are never coordinators, so this is a no-op
    // for them. Self-handoff-and-switch (#668) respawns through this function
    // and inherits the mirror automatically.
    if restart_start_fresh {
        crate::commands::pty::write_start_fresh_mirror_for_session(app, new_uuid, true).await;
    }

    // 6. Re-attach Telegram bridge from live persisted intent, or fall back to repo config.
    if telegram_bot_id.is_some() {
        attach_persisted_telegram_if_configured(app, new_uuid, telegram_bot_id.as_deref()).await;
    } else {
        attach_local_config_telegram_if_any(app, new_uuid, &cwd).await;
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
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
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
        let label = format!("terminal-{}", id.replace('-', ""));
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.set_focus();
        }
        return Ok(());
    }

    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    coordinator
        .transition(SelectionRequest::user_switch(uuid))
        .await?;

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
    coordinator: State<'_, SelectionCoordinator>,
) -> Result<crate::session::selection::SessionSelection, String> {
    coordinator.snapshot().await
}

#[derive(Debug, Clone)]
pub(crate) struct RootJobRequest {
    pub requested_agent_id: Option<String>,
    pub requested_profile: Option<String>,
    pub skip_auto_resume_for_new_session: bool,
    pub intent: TrustedCreateIntent,
    pub select_after: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_root_agent_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    tg_mgr: &TelegramBridgeState,
    settings: &SettingsState,
    requested_agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume_for_new_session: bool,
) -> Result<SessionInfo, String> {
    create_root_agent_inner_with_intent(
        app,
        session_mgr,
        pty_mgr,
        tg_mgr,
        settings,
        requested_agent_id,
        requested_profile,
        skip_auto_resume_for_new_session,
        TrustedCreateIntent::User,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_root_agent_inner_with_intent(
    app: &AppHandle,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    _tg_mgr: &TelegramBridgeState,
    _settings: &SettingsState,
    requested_agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume_for_new_session: bool,
    intent: TrustedCreateIntent,
    select_after: bool,
) -> Result<SessionInfo, String> {
    app.state::<SelectionCoordinator>()
        .root_lifecycle(RootJobRequest {
            requested_agent_id,
            requested_profile,
            skip_auto_resume_for_new_session,
            intent,
            select_after,
        })
        .await
}

pub(crate) async fn execute_root_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RootJobRequest,
) -> Result<SessionInfo, String> {
    let app = transaction.app();
    let session_mgr = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .inner()
        .clone();
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let settings = app.state::<SettingsState>().inner().clone();
    let root_agent_path = crate::config::root_agent::ensure_root_agent_dir()?;
    let existing = session_mgr
        .read()
        .await
        .list_sessions()
        .await
        .into_iter()
        .find(|session| {
            session.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&session.working_directory)
        });

    if let Some(existing) = existing {
        let session_id = Uuid::parse_str(&existing.id).map_err(|error| error.to_string())?;
        session_mgr
            .read()
            .await
            .set_is_root_agent(session_id, true)
            .await;
        let has_pty = pty_mgr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(session_id);
        match classify_existing_root(&existing.status, has_pty) {
            ExistingRootAction::ReuseLive => {
                if request.select_after {
                    let decision = transaction
                        .live_decision(session_id)
                        .ok_or_else(|| "Root session has no displayable PTY".to_string())?;
                    let committed = transaction
                        .commit(
                            decision,
                            SelectionCause::UserSwitch,
                            LifecycleMutations::default(),
                        )
                        .await?;
                    if let Some(selection) = committed.selection.as_ref() {
                        transaction
                            .persist(SelectionSource::UserSwitch, Some(session_id))
                            .await;
                        transaction.publish_selection(selection);
                    }
                }
                return session_mgr
                    .read()
                    .await
                    .get_session(session_id)
                    .await
                    .map(|session| SessionInfo::from(&session))
                    .ok_or_else(|| "Root session disappeared during reuse".to_string());
            }
            ExistingRootAction::WakeDormant | ExistingRootAction::DiscardMissingPty => {
                let wake_dormant = matches!(
                    classify_existing_root(&existing.status, has_pty),
                    ExistingRootAction::WakeDormant
                );
                let restart_intent = match request.intent {
                    TrustedCreateIntent::User => TrustedRestartIntent::User,
                    TrustedCreateIntent::Background => TrustedRestartIntent::Background,
                };
                return execute_restart_transaction(
                    transaction,
                    RestartJobRequest {
                        session_id,
                        agent_id: request.requested_agent_id,
                        requested_profile: request.requested_profile,
                        skip_auto_resume: Some(if wake_dormant {
                            false
                        } else {
                            request.skip_auto_resume_for_new_session
                        }),
                        activate_after: request.select_after,
                        intent: restart_intent,
                        communication_override: None,
                        enforcement: crate::config::sessions_persistence::default_creation_gate_enforcement(),
                    },
                )
                .await;
            }
        }
    }

    let last_coding_agent = crate::config::root_agent::read_last_coding_agent(&root_agent_path);
    let (shell, shell_args, agent_id, agent_label) = {
        let settings = settings.read().await;
        resolve_root_agent_command(
            &settings,
            request.requested_agent_id.as_deref(),
            last_coding_agent.as_deref(),
        )?
    };
    let (resolved_spawn, resolved_agent_host_shell) = if let Some(agent_id) = agent_id.as_deref() {
        // #1271 - build the spawn and copy the configured default host shell
        // from the SAME single guard, before any await, so the pair can never
        // mix across a configuration change (Phase 1 items 1-2; mirrors the
        // restart pattern).
        let settings = settings.read().await;
        let spawn = build_configured_agent_spawn_for_cwd(
            &settings,
            agent_id,
            &root_agent_path,
            request.requested_profile.as_deref(),
        )?;
        let host_shell = Some(ResolvedAgentHostShell {
            program: settings.default_shell.clone(),
            args: settings.default_shell_args.clone(),
        });
        (spawn, host_shell)
    } else {
        (None, None)
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
    let info = create_session_inner_impl(
        app,
        &session_mgr,
        &pty_mgr,
        shell,
        shell_args,
        root_agent_path.clone(),
        Some(crate::config::root_agent::ROOT_AGENT_SESSION_NAME.to_string()),
        agent_id,
        agent_label,
        false,
        Vec::new(),
        request.skip_auto_resume_for_new_session,
        resolved_spawn,
        resolved_agent_host_shell,
        None,
        CreateSelectionIntent::Suppress,
        None,
        None,
        Some(transaction),
        Some(SelectionCause::SessionCreated(request.intent)),
        false,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await?
    .into_finalized()?;
    let session_id = Uuid::parse_str(&info.id).map_err(|error| error.to_string())?;
    if request.select_after {
        let decision = transaction
            .live_decision(session_id)
            .ok_or_else(|| "new Root session has no displayable PTY".to_string())?;
        let committed = transaction
            .commit(
                decision,
                SelectionCause::SessionCreated(request.intent),
                LifecycleMutations::default(),
            )
            .await?;
        if let Some(selection) = committed.selection.as_ref() {
            transaction
                .persist(SelectionSource::SessionCreated, Some(session_id))
                .await;
            transaction.publish_selection(selection);
        }
    }
    attach_local_config_telegram_if_any(app, session_id, &root_agent_path).await;
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
        classify_existing_root, claude_projects_dir_for_config_dir,
        claude_resume_probe_target_for_kind, compute_profile_outdated,
        container_path_context_for_cwd, count_working_members, effective_restart_requested_profile,
        execute_manual_coordinator_destroy, inject_codex_resume, inject_pi_resume,
        injected_claude_config_dir_for_copy, maybe_inject_pi_resume,
        pi_has_explicit_session_control, pi_is_non_conversation_invocation, resolve_actual_agent,
        resolve_agent_command, resolve_agent_from_shell, resolve_claude_projects_dir,
        resolve_launch_auto_self_clear, resolve_restart_selected_agent_id,
        resolve_root_agent_command, resume_probe_target_for_config_dir, should_inject_continue,
        CreateSelectionIntent, ExistingRootAction,
    };
    use crate::config::settings::{AgentConfig, AppSettings, ProfileCellConfig};
    use crate::pty::backend::{PtyBackend, SessionBackendKind};
    use crate::pty::container_backend::container_child_env;
    use crate::pty::container_credentials::ContainerCredentialPlan;
    use crate::pty::container_paths::{
        ContainerPathMap, CLAUDE_CONFIG_DIR_KEY, WARNING_KIND_NO_VALUE,
    };
    use crate::session::manager::SessionManager;
    use crate::session::profile::CodingAgentKind;
    use crate::session::session::{
        SessionCommunication, SessionCommunicationKind, SessionInfo, SessionStatus,
    };
    use std::collections::{BTreeMap, HashMap, HashSet};
    #[cfg(windows)]
    use std::path::Path;
    use std::path::PathBuf;
    #[cfg(windows)]
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tauri::Manager;
    use uuid::Uuid;

    // Stage E (#1064) selection-intent mapping sentinel (plan section 10.4 item
    // 20, acceptance item 42, 10.6): the internal-alert background create keeps
    // `CreateSelectionIntent::Background`, which maps to the Background trusted
    // intent (never downgraded); Suppress finalizes nothing.
    #[test]
    fn stage_e_create_selection_intent_maps_background_to_trusted_background() {
        use crate::session::selection::TrustedCreateIntent;
        assert!(matches!(
            CreateSelectionIntent::Background.trusted(),
            Some(TrustedCreateIntent::Background)
        ));
        assert!(matches!(
            CreateSelectionIntent::User.trusted(),
            Some(TrustedCreateIntent::User)
        ));
        assert!(
            CreateSelectionIntent::Suppress.trusted().is_none(),
            "a suppressed create never becomes a trusted finalizing create"
        );
    }

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
                    context_regex: None,
                    backend: Default::default(),
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
                    context_regex: None,
                    backend: Default::default(),
                },
            ],
            ..AppSettings::default()
        }
    }

    #[test]
    fn resolve_launch_auto_self_clear_uses_capability_and_setting_precedence() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("__agent_dev-rust");
        std::fs::create_dir_all(&cwd).unwrap();
        let cwd = cwd.to_string_lossy().to_string();
        let mut settings = AppSettings::default();

        assert!(resolve_launch_auto_self_clear(&settings, "pi", &cwd, true));
        assert!(!resolve_launch_auto_self_clear(
            &settings, "pi", &cwd, false
        ));

        settings
            .auto_self_clear_by_agent
            .insert("dev-rust".to_string(), true);
        assert!(resolve_launch_auto_self_clear(&settings, "pi", &cwd, false));
        settings
            .auto_self_clear_by_agent
            .insert("dev-rust".to_string(), false);
        assert!(!resolve_launch_auto_self_clear(&settings, "pi", &cwd, true));

        settings.auto_self_clear_by_agent.clear();
        for shell in ["claude", "codex-wrapper", "agy"] {
            assert!(resolve_launch_auto_self_clear(&settings, shell, &cwd, true));
        }
        for shell in ["agent", "cmd.exe", "pwsh", "pip", "pi-agent"] {
            assert!(!resolve_launch_auto_self_clear(
                &settings, shell, &cwd, true
            ));
        }

        settings.auto_self_clear_enabled = false;
        assert!(!resolve_launch_auto_self_clear(&settings, "pi", &cwd, true));
        assert!(!resolve_launch_auto_self_clear(
            &settings, "claude", &cwd, true
        ));
    }

    #[test]
    fn configured_pi_materialization_wires_auto_self_clear_to_agents_md() {
        let temp = tempfile::tempdir().unwrap();
        let ac_root = temp.path().join(".ac");
        let matrix = ac_root.join("_agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        let cwd = matrix.to_string_lossy().to_string();
        let settings = AppSettings {
            agents: vec![AgentConfig {
                id: "pi".to_string(),
                label: "Pi".to_string(),
                command: "pi".to_string(),
                color: "#10b981".to_string(),
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: Some("AGENTS.md".to_string()),
                config_seed: None,
                context_regex: None,
                backend: Default::default(),
            }],
            ..AppSettings::default()
        };
        let target =
            crate::config::agent_command::resolve_target_filename(Some("pi"), &settings, None);
        assert_eq!(target.as_deref(), Some("AGENTS.md"));
        assert_eq!(
            crate::config::agent_command::resolve_target_filename(None, &settings, None),
            None,
            "an ad hoc Pi launch has no managed target filename"
        );
        let managed = crate::config::agent_command::managed_instructions_filenames(&settings);
        let enabled = resolve_launch_auto_self_clear(&settings, "pi", &cwd, true);
        assert!(enabled);

        crate::config::session_context::materialize_agent_context_file_with_filename(
            &cwd,
            target.as_deref().unwrap(),
            &managed,
            true,
            enabled,
            None,
        )
        .unwrap()
        .expect("configured Pi context should materialize");
        let on = std::fs::read_to_string(matrix.join("AGENTS.md")).unwrap();
        assert_eq!(on.matches("## Self-Maintenance").count(), 1);

        crate::config::session_context::materialize_agent_context_file_with_filename(
            &cwd,
            "AGENTS.md",
            &managed,
            true,
            false,
            None,
        )
        .unwrap()
        .expect("configured Pi context should rematerialize");
        let off = std::fs::read_to_string(matrix.join("AGENTS.md")).unwrap();
        assert!(!off.contains("## Self-Maintenance"));
    }

    fn inert_pi_spawn() -> crate::config::agent_command::AgentSpawnCommand {
        let settings = AppSettings {
            agents: vec![AgentConfig {
                id: "pi".to_string(),
                label: "Pi".to_string(),
                command: "pi".to_string(),
                color: "#10b981".to_string(),
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                backend: Default::default(),
            }],
            ..AppSettings::default()
        };
        crate::config::agent_command::resolve_agent_spawn_command(&settings, "pi", None, None)
            .expect("Pi test spawn should resolve without filesystem preparation")
    }

    fn probe_map() -> ContainerPathMap {
        ContainerPathMap::new(r"C:\Users\maria\repo\.ac\wg-1\__agent_x", "/workspace").unwrap()
    }

    fn norm(path: &std::path::Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn pi_provider_values_never_resolve_a_claude_probe() {
        for args in [
            vec![
                "--provider".to_string(),
                "claude".to_string(),
                "--continue".to_string(),
            ],
            vec!["--model".to_string(), "claude-sonnet".to_string()],
        ] {
            assert_eq!(
                CodingAgentKind::detect("pi", &args),
                Some(CodingAgentKind::Pi)
            );
            let probe = claude_resume_probe_target_for_kind(
                Some(CodingAgentKind::Pi),
                SessionBackendKind::LocalProcess,
                None,
                None,
                None,
                "pi",
                &args,
                r"Z:\path-that-must-not-be-probed",
            );
            assert!(probe.is_none());
        }
    }

    #[test]
    fn resume_probe_target_local_uses_effective_config_dir_on_host() {
        let cwd = r"C:\Users\maria\repo\.ac\wg-1\__agent_x";
        let config_dir = r"C:\Users\maria\.claude-work";
        let got = resume_probe_target_for_config_dir(
            SessionBackendKind::LocalProcess,
            None,
            Some(config_dir),
            "claude",
            &[],
            cwd,
        );

        assert_eq!(got.filesystem, "host");
        assert_eq!(
            got.host_probe_path,
            Some(claude_projects_dir_for_config_dir(config_dir, cwd))
        );
        assert!(got.warning.is_none());
    }

    #[test]
    fn resume_probe_target_local_bare_claude_uses_default_resolver() {
        let cwd = r"C:\Users\Test\repo";
        let got = resume_probe_target_for_config_dir(
            SessionBackendKind::LocalProcess,
            None,
            None,
            "claude",
            &[],
            cwd,
        );

        let expected = resolve_claude_projects_dir("claude", &[], cwd);
        let Some(expected) = expected else {
            return;
        };
        assert_eq!(got.filesystem, "host");
        assert_eq!(got.host_probe_path, Some(expected));
        assert!(got.warning.is_none());
    }

    #[test]
    fn resume_probe_target_local_wrapper_uses_wrapper_config_dir() {
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
        let cwd = r"C:\Users\Test\repo";

        let got = resume_probe_target_for_config_dir(
            SessionBackendKind::LocalProcess,
            None,
            None,
            wrapper.to_str().unwrap(),
            &[],
            cwd,
        );

        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd));
        assert_eq!(got.filesystem, "host");
        assert_eq!(got.host_probe_path, Some(expected));
        assert!(got.warning.is_none());
    }

    #[test]
    fn resume_probe_target_container_no_value_warns_and_skips_probe() {
        let map = probe_map();
        let got = resume_probe_target_for_config_dir(
            SessionBackendKind::ContainerTransport,
            Some(&map),
            None,
            "claude",
            &[],
            map.host_root(),
        );

        assert_eq!(got.filesystem, "container-unreachable");
        assert!(got.host_probe_path.is_none());
        assert_eq!(
            got.warning.as_ref().map(|w| w.kind),
            Some(WARNING_KIND_NO_VALUE)
        );
    }

    #[test]
    fn resume_probe_target_container_maps_bind_mounted_config_dir() {
        let map = probe_map();
        let got = resume_probe_target_for_config_dir(
            SessionBackendKind::ContainerTransport,
            Some(&map),
            Some(r"C:\Users\maria\repo\.ac\wg-1\__agent_x\.claude"),
            "claude",
            &[],
            map.host_root(),
        );

        assert_eq!(got.filesystem, "container-via-mount");
        let host_probe = got.host_probe_path.expect("host probe");
        assert!(norm(&host_probe).ends_with("/__agent_x/.claude/projects/-workspace"));
        assert!(got.warning.is_none());
    }

    #[test]
    fn resume_probe_target_container_unmappable_config_dir_skips_probe_without_duplicate_warning() {
        let map = probe_map();
        let got = resume_probe_target_for_config_dir(
            SessionBackendKind::ContainerTransport,
            Some(&map),
            Some("/workspace/.claude"),
            "claude",
            &[],
            map.host_root(),
        );

        assert_eq!(got.filesystem, "container-unreachable");
        assert!(got.host_probe_path.is_none());
        assert!(got.warning.is_none());
    }

    fn cred_host_root() -> &'static str {
        if cfg!(windows) {
            r"C:\Users\maria\repo\.ac\wg-1\__agent_x"
        } else {
            "/Users/maria/repo/.ac/wg-1/__agent_x"
        }
    }

    fn cred_plan_map() -> ContainerPathMap {
        ContainerPathMap::new(cred_host_root(), "/workspace").unwrap()
    }

    fn cred_plan() -> ContainerCredentialPlan {
        let dest = std::path::Path::new(cred_host_root())
            .join(".claude")
            .join(".credentials.json");
        ContainerCredentialPlan {
            source: PathBuf::from("unused-host-source"),
            dest,
            first_run: None,
        }
    }

    #[test]
    fn injected_config_dir_defaults_to_copy_dir_and_maps_into_container() {
        // #930 - host-login copy will happen (plan Some) and the user set no
        // CLAUDE_CONFIG_DIR -> inject the copy dir (host path), which the container
        // env translation maps to /workspace/.claude so the copied token is read.
        let plan = cred_plan();
        let injected = injected_claude_config_dir_for_copy(Some(&plan), false)
            .expect("copy without a user value must inject the copy dir");
        let expected_dir = format!("{}/.claude", cred_host_root().replace('\\', "/"));
        assert_eq!(injected.replace('\\', "/"), expected_dir);

        let translated = container_child_env(
            vec![(CLAUDE_CONFIG_DIR_KEY.to_string(), injected)],
            Vec::new(),
            &cred_plan_map(),
        );
        assert_eq!(
            translated.child_env,
            vec![(
                CLAUDE_CONFIG_DIR_KEY.to_string(),
                "/workspace/.claude".to_string()
            )]
        );
        assert!(translated.env_unset.is_empty());
        assert!(translated.warnings.is_empty());
    }

    #[test]
    fn injected_config_dir_respects_explicit_user_value() {
        // #930 - the user already configured CLAUDE_CONFIG_DIR; never overwrite it,
        // even though a copy plan exists.
        assert_eq!(
            injected_claude_config_dir_for_copy(Some(&cred_plan()), true),
            None
        );
    }

    #[test]
    fn injected_config_dir_none_without_copy_plan() {
        // #930 - host-login-reuse off or no host creds => no plan => inject nothing,
        // even when the user set no CLAUDE_CONFIG_DIR.
        assert_eq!(injected_claude_config_dir_for_copy(None, false), None);
    }

    #[test]
    fn container_path_context_uses_canonical_host_root_for_guard_and_map() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp
            .path()
            .join("project")
            .join(".ac")
            .join("wg-1-team")
            .join("repo-foo");
        std::fs::create_dir_all(&repo).unwrap();
        let raw = repo.join(".");
        let canonical = crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            &std::fs::canonicalize(&repo).unwrap(),
        );

        let got = container_path_context_for_cwd(raw.to_str().unwrap()).unwrap();

        assert_eq!(got.host_root, canonical);
        assert_eq!(got.map.host_root(), canonical);
    }

    #[cfg(windows)]
    fn create_junction(link: &Path, target: &Path) {
        let output = Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .output()
            .expect("failed to invoke mklink");
        assert!(
            output.status.success(),
            "failed to create junction link='{}' target='{}': stdout={} stderr={}",
            link.display(),
            target.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    fn container_path_context_refuses_junction_targeting_workgroup_root() {
        // B4 regression fence: before cwd canonicalization, the textual guard
        // saw only `link-to-wg` and permitted a bind mount of the workgroup.
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp
            .path()
            .join("project")
            .join(".ac")
            .join("wg-11-dev-v4-team");
        std::fs::create_dir_all(&target).unwrap();
        let link = tmp.path().join("link-to-wg");
        create_junction(&link, &target);

        let err = container_path_context_for_cwd(link.to_str().unwrap())
            .expect_err("junction to workgroup root must be refused");

        assert!(err.contains("workgroup root"), "{err}");
        assert!(err.contains("selected path"), "{err}");
        assert!(err.contains("canonical path"), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn container_path_context_refuses_junction_targeting_home_dir() {
        // Guard coverage, not the main B4 regression fence: the pre-B4 home
        // rule already canonicalized both sides. This test mainly preserves the
        // selected-vs-canonical rejection text for reparse-point inputs.
        //
        // Safety note: `%TEMP%` usually lives under the user's home directory,
        // so this junction points from a tempdir back to its own ancestor.
        // The test relies on std::fs::remove_dir_all treating the junction as a
        // reparse point and deleting only the link, not descending into home.
        let tmp = tempfile::tempdir().unwrap();
        let home = dirs::home_dir().expect("home dir required for junction guard test");
        let link = tmp.path().join("link-to-home");
        create_junction(&link, &home);

        let err = container_path_context_for_cwd(link.to_str().unwrap())
            .expect_err("junction to home dir must be refused");

        assert!(err.contains("home directory"), "{err}");
        assert!(err.contains("selected path"), "{err}");
        assert!(err.contains("canonical path"), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn container_path_context_refuses_cwd_that_cannot_be_canonicalized() {
        // Pins the fail-closed DD9 behavior from 8d0d3fd5. It is not the B4
        // junction bypass regression fence.
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing");

        let err = container_path_context_for_cwd(missing.to_str().unwrap())
            .expect_err("missing cwd must be refused");

        assert!(
            err.contains("failed to canonicalize container mount source"),
            "{err}"
        );
        assert!(err.contains(missing.to_str().unwrap()), "{err}");
    }

    #[derive(Default)]
    struct FailingSpawnBackend {
        spawned: Mutex<Vec<Uuid>>,
        killed: Mutex<Vec<Uuid>>,
        /// #973 - every (cols, rows) a spawn was asked for.
        sizes: Mutex<Vec<(u16, u16)>>,
    }

    impl FailingSpawnBackend {
        fn spawned(&self) -> Vec<Uuid> {
            self.spawned.lock().unwrap().clone()
        }

        fn killed(&self) -> Vec<Uuid> {
            self.killed.lock().unwrap().clone()
        }

        /// #973 - the sizes the PTY would have been opened at.
        fn sizes(&self) -> Vec<(u16, u16)> {
            self.sizes.lock().unwrap().clone()
        }
    }

    impl crate::pty::backend::PtyBackend for FailingSpawnBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async move {
                self.spawned.lock().unwrap().push(spec.id);
                // #973 - the size the ConPTY would have been opened at.
                self.sizes.lock().unwrap().push((spec.cols, spec.rows));
                Err(crate::errors::AppError::PtyError(
                    "synthetic spawn failure".to_string(),
                ))
            })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            _id: Uuid,
            _data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
            self.killed.lock().unwrap().push(id);
            Ok(())
        }

        fn has_session(&self, _id: Uuid) -> bool {
            false
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    /// A `PtyBackend` whose `spawn` parks on a oneshot so a test can observe the
    /// `create_session_inner` spawn mark while the PTY is still being spawned.
    /// `fail` decides whether the parked spawn ultimately errors (exercising the
    /// rollback path) or succeeds and records the session as live.
    struct GatedSpawnBackend {
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        live: Mutex<Vec<Uuid>>,
        fail: bool,
    }

    impl GatedSpawnBackend {
        fn new(
            started: tokio::sync::oneshot::Sender<()>,
            release: tokio::sync::oneshot::Receiver<()>,
            fail: bool,
        ) -> Self {
            Self {
                started: Mutex::new(Some(started)),
                release: Mutex::new(Some(release)),
                live: Mutex::new(Vec::new()),
                fail,
            }
        }
    }

    impl crate::pty::backend::PtyBackend for GatedSpawnBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            let started = self.started.lock().unwrap().take().expect("started sender");
            let release = self
                .release
                .lock()
                .unwrap()
                .take()
                .expect("release receiver");
            let fail = self.fail;
            Box::pin(async move {
                // Announce that PtyManager::spawn has released the outer manager
                // mutex and is now parked inside the backend, then wait for the
                // test to inspect the spawn mark before the spawn resolves.
                let _ = started.send(());
                let _ = release.await;
                if fail {
                    Err(crate::errors::AppError::PtyError(
                        "synthetic spawn failure".to_string(),
                    ))
                } else {
                    self.live.lock().unwrap().push(spec.id);
                    Ok(())
                }
            })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            _id: Uuid,
            _data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
            self.live.lock().unwrap().retain(|live| *live != id);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    #[derive(Default)]
    struct ScriptedSpawnBackend {
        live: Mutex<HashSet<Uuid>>,
        spawned: Mutex<Vec<Uuid>>,
        spawn_count: AtomicUsize,
        fail_spawn_number: AtomicUsize,
        gate_spawn_number: AtomicUsize,
        gate_started: Mutex<Option<tokio::sync::oneshot::Sender<Uuid>>>,
        gate_release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    struct BarrierDestroyBackend {
        live: Mutex<HashSet<Uuid>>,
        fail_kill: Mutex<HashSet<Uuid>>,
        kills: Mutex<Vec<Uuid>>,
        started: Mutex<Option<tokio::sync::oneshot::Sender<Uuid>>>,
        release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    impl BarrierDestroyBackend {
        fn new(
            started: tokio::sync::oneshot::Sender<Uuid>,
            release: std::sync::mpsc::Receiver<()>,
        ) -> Self {
            Self {
                live: Mutex::new(HashSet::new()),
                fail_kill: Mutex::new(HashSet::new()),
                kills: Mutex::new(Vec::new()),
                started: Mutex::new(Some(started)),
                release: Mutex::new(Some(release)),
            }
        }

        fn fail_kill_for(&self, session_id: Uuid) {
            self.fail_kill.lock().unwrap().insert(session_id);
        }
    }

    impl crate::pty::backend::PtyBackend for BarrierDestroyBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async move {
                self.live.lock().unwrap().insert(spec.id);
                Ok(())
            })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            id: Uuid,
            _data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            self.live
                .lock()
                .unwrap()
                .contains(&id)
                .then_some(())
                .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
        }

        fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            self.live
                .lock()
                .unwrap()
                .contains(&id)
                .then_some(())
                .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
        }

        fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
            self.kills.lock().unwrap().push(id);
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(id);
                if let Some(release) = self.release.lock().unwrap().take() {
                    release
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .map_err(|error| {
                            crate::errors::AppError::PtyError(format!(
                                "destroy barrier release failed: {error}"
                            ))
                        })?;
                }
            }
            if self.fail_kill.lock().unwrap().contains(&id) {
                return Err(crate::errors::AppError::PtyError(
                    "synthetic live teardown failure".to_string(),
                ));
            }
            self.live.lock().unwrap().remove(&id);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
            self.has_session(id).then_some((30, 120))
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    impl ScriptedSpawnBackend {
        fn fail_spawn(&self, number: usize) {
            self.fail_spawn_number.store(number, Ordering::SeqCst);
        }

        fn gate_spawn(
            &self,
            number: usize,
            started: tokio::sync::oneshot::Sender<Uuid>,
            release: tokio::sync::oneshot::Receiver<()>,
        ) {
            self.gate_spawn_number.store(number, Ordering::SeqCst);
            *self.gate_started.lock().unwrap() = Some(started);
            *self.gate_release.lock().unwrap() = Some(release);
        }

        fn lose_route(&self, session_id: Uuid) {
            self.live.lock().unwrap().remove(&session_id);
        }
    }

    impl crate::pty::backend::PtyBackend for ScriptedSpawnBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async move {
                let number = self.spawn_count.fetch_add(1, Ordering::SeqCst) + 1;
                self.spawned.lock().unwrap().push(spec.id);
                if self.gate_spawn_number.load(Ordering::SeqCst) == number {
                    if let Some(started) = self.gate_started.lock().unwrap().take() {
                        let _ = started.send(spec.id);
                    }
                    let release = self.gate_release.lock().unwrap().take();
                    if let Some(release) = release {
                        let _ = release.await;
                    }
                }
                if self.fail_spawn_number.load(Ordering::SeqCst) == number {
                    return Err(crate::errors::AppError::PtyError(
                        "synthetic scripted spawn failure".to_string(),
                    ));
                }
                self.live.lock().unwrap().insert(spec.id);
                Ok(())
            })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            id: Uuid,
            _data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            self.live
                .lock()
                .unwrap()
                .contains(&id)
                .then_some(())
                .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
        }

        fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            self.live
                .lock()
                .unwrap()
                .contains(&id)
                .then_some(())
                .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
        }

        fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
            self.live.lock().unwrap().remove(&id);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
            self.has_session(id).then_some((30, 120))
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    #[derive(Clone, Copy)]
    enum SessionTestStoreState {
        Ready,
        Error,
        Missing,
    }

    fn session_test_app_with_store(
        settings: AppSettings,
        session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: Arc<Mutex<crate::pty::manager::PtyManager>>,
        store_state: SessionTestStoreState,
    ) -> tauri::App<tauri::test::MockRuntime> {
        let shutdown = crate::shutdown::ShutdownSignal::new();
        let coordinator = crate::session::selection::SelectionCoordinator::new(
            Arc::clone(&session_mgr),
            shutdown.token().clone(),
        );
        let output_senders = Arc::new(Mutex::new(HashMap::new()));
        let telegram: crate::telegram::manager::TelegramBridgeState =
            Arc::new(tokio::sync::Mutex::new(
                crate::telegram::manager::TelegramBridgeManager::new(output_senders),
            ));
        let store_dir = tempfile::TempDir::new().expect("create target-gate store");
        let message_store = Arc::new(
            crate::api::message_store::MessageStore::open(
                store_dir
                    .path()
                    .join(crate::api::message_store::DB_FILENAME),
            )
            .expect("open target-gate store"),
        );
        let target_gate_state = crate::api::message_store::PtyInputTargetGateState::for_root(
            store_dir.path().to_path_buf(),
        );
        let mut builder = tauri::test::mock_builder()
            .manage(Arc::new(tokio::sync::RwLock::new(settings)))
            .manage(Arc::new(
                crate::resource_monitor::ResourceMonitorState::new(),
            ))
            .manage(session_mgr)
            .manage(pty_mgr)
            .manage(crate::DetachedSessionsState::default())
            .manage(telegram)
            .manage(crate::session::warnings::new_session_warning_state())
            .manage(coordinator.clone())
            .manage(target_gate_state.clone())
            .manage(shutdown);
        builder = match store_state {
            SessionTestStoreState::Ready => builder.manage(
                crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                    Ok(message_store),
                    target_gate_state.gate.clone(),
                ),
            ),
            SessionTestStoreState::Error => builder.manage(
                crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                    Err("store_unavailable".to_string()),
                    target_gate_state.gate.clone(),
                ),
            ),
            SessionTestStoreState::Missing => builder,
        };
        let app = builder
            .manage(store_dir)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build session test app");
        coordinator
            .start(app.handle().clone())
            .expect("start coordinator");
        let bootstrap = coordinator.clone();
        std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move {
                bootstrap
                    .submit_restore_first()
                    .await
                    .expect("open coordinator")
                    .finish();
            });
        })
        .join()
        .expect("join coordinator bootstrap");
        app
    }

    fn session_test_app(
        settings: AppSettings,
        session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: Arc<Mutex<crate::pty::manager::PtyManager>>,
    ) -> tauri::App<tauri::test::MockRuntime> {
        session_test_app_with_store(settings, session_mgr, pty_mgr, SessionTestStoreState::Ready)
    }

    fn strict_target_fixture() -> (tempfile::TempDir, String, String) {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("project");
        let ac_root = project.join(".ac");
        let team = ac_root.join("_team_team");
        let coordinator_matrix = ac_root.join("_agent_lead");
        let first_matrix = ac_root.join("_agent_dev-one");
        let second_matrix = ac_root.join("_agent_dev-two");
        let workgroup = ac_root.join("wg-1-team");
        let coordinator = workgroup.join("__agent_lead");
        let first = workgroup.join("__agent_dev-one");
        let second = workgroup.join("__agent_dev-two");
        for directory in [
            &team,
            &coordinator_matrix,
            &first_matrix,
            &second_matrix,
            &coordinator,
            &first,
            &second,
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(
            team.join("config.json"),
            r#"{"agents":["../_agent_dev-one","../_agent_dev-two","../_agent_lead"],"coordinator":"../_agent_lead"}"#,
        )
        .unwrap();
        for (replica, identity) in [
            (&coordinator, "../../_agent_lead"),
            (&first, "../../_agent_dev-one"),
            (&second, "../../_agent_dev-two"),
        ] {
            std::fs::write(
                replica.join("config.json"),
                format!(r#"{{"identity":"{identity}"}}"#),
            )
            .unwrap();
        }
        (
            temp,
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        )
    }

    /// #1175. The bytes every rotation fixture seeds into `memory/MEMORY.md`.
    /// Asserted verbatim on both sides of every differential, so a green test can
    /// never mean "an archive directory exists but is empty".
    const ROTATION_SENTINEL: &str = "remembered bytes";

    /// #1175. One replica and the origin Agent Matrix its `config.json` identity
    /// points at.
    struct RotationSide {
        replica_cwd: String,
        matrix: std::path::PathBuf,
    }

    /// #1175. `strict_target_fixture` (`:5896`) already builds two symmetric
    /// replicas under `<temp>/project/.ac/wg-1-team/` whose `config.json` identity
    /// names `<temp>/project/.ac/_agent_dev-one` and `_agent_dev-two`. Those two
    /// directories sit directly under a `.ac` workspace, so they satisfy
    /// `is_canonical_agent_matrix_dir`, which is what `resolve_rotatable_matrix_root`
    /// (`config/agent_memory.rs:83`) resolves a replica session to.
    ///
    /// Seed a non-empty `memory/` in each: #1172 D2 makes an EMPTY `memory/` a
    /// no-op, so the sentinel is what gives the fresh half of each differential
    /// something to rotate.
    fn rotation_fixture() -> (tempfile::TempDir, RotationSide, RotationSide) {
        let (temp, first_replica, second_replica) = strict_target_fixture();
        let ac_root = temp.path().join("project").join(".ac");
        let mut sides = Vec::new();
        for (replica_cwd, agent) in [
            (first_replica, "_agent_dev-one"),
            (second_replica, "_agent_dev-two"),
        ] {
            let matrix = ac_root.join(agent);
            std::fs::create_dir_all(matrix.join("memory")).expect("create origin memory/");
            std::fs::write(matrix.join("memory").join("MEMORY.md"), ROTATION_SENTINEL)
                .expect("seed origin memory/");
            sides.push(RotationSide {
                replica_cwd,
                matrix,
            });
        }
        let second = sides.pop().expect("second side");
        let first = sides.pop().expect("first side");
        (temp, first, second)
    }

    /// #1175. Every rotated sibling of `memory/`, sorted. Same shape as
    /// `agent_memory.rs:180-189`.
    fn rotated_memory_dirs(matrix: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(matrix)
            .expect("read origin matrix")
            .map(|entry| entry.expect("dir entry").file_name())
            .filter_map(|name| name.to_str().map(str::to_string))
            .filter(|name| name.starts_with("memory_"))
            .collect();
        names.sort();
        names
    }

    /// #1175. "Nothing has happened to this side." Used for the resume side AND
    /// for a side whose own launch has not run yet, which is what stops the second
    /// launch from laundering a side effect of the first (D3, hole 2).
    fn assert_memory_pristine(side: &RotationSide, case: &str) {
        assert_eq!(
            std::fs::read_to_string(side.matrix.join("memory").join("MEMORY.md")).ok(),
            Some(ROTATION_SENTINEL.to_string()),
            "#1175 ({case}): the origin matrix's live memory/ must be exactly as it was"
        );
        let rotated = rotated_memory_dirs(&side.matrix);
        assert!(
            rotated.is_empty(),
            "#1175 ({case}): nothing may have rotated, and this created {rotated:?} in {}",
            side.matrix.display()
        );
        let replica = std::path::Path::new(&side.replica_cwd);
        assert!(
            rotated_memory_dirs(replica).is_empty() && !replica.join("memory").exists(),
            "#1175 ({case}): the replica itself must gain no memory* entry"
        );
    }

    /// #1175. The RESUME assertion: pristine, PLUS a witness that the launch
    /// actually entered the context block.
    ///
    /// The witness is not decoration. `:2041` is an `if let`, and a launch that
    /// skips it still reaches the spawn, so `spawn_count` cannot see the
    /// difference; without this, "nothing rotated" is satisfiable by a resume that
    /// never reached the gate at all.
    /// `materialize_agent_context_file_with_filename_activated`
    /// (`config/session_context.rs:2215`) writes `cwd.join(target_filename)`, and
    /// the fixture seeds only `config.json` into a replica, so this file existing
    /// means the block ran. Measured in probe 14 and in 2.7.
    fn assert_resume_left_memory_alone(side: &RotationSide, case: &str) {
        assert!(
            std::path::Path::new(&side.replica_cwd)
                .join("AGENTS.md")
                .is_file(),
            "#1175 ({case}): the resume must have ENTERED the context block; without \
             this witness the no-rotation assertion below can pass vacuously"
        );
        assert_memory_pristine(side, case);
    }

    /// #1175. The FRESH assertion, which is also the POSITIVE CONTROL: it fails if
    /// the launch never reached the rotation chokepoint, which is what makes a green
    /// `assert_resume_left_memory_alone` on the resume side attributable to the gate
    /// rather than to an unreached call site.
    fn assert_memory_rotated_once(side: &RotationSide, case: &str) {
        let rotated = rotated_memory_dirs(&side.matrix);
        assert_eq!(
            rotated.len(),
            1,
            "#1175 ({case}): a fresh launch must rotate exactly once, found {rotated:?}"
        );
        assert_eq!(
            std::fs::read_to_string(side.matrix.join(&rotated[0]).join("MEMORY.md")).ok(),
            Some(ROTATION_SENTINEL.to_string()),
            "#1175 ({case}): the archive must carry the previous session's bytes"
        );
        let live = side.matrix.join("memory");
        assert!(
            live.is_dir(),
            "#1175 ({case}): the fresh session must get a live memory/ back"
        );
        assert_eq!(
            std::fs::read_dir(&live).expect("read live memory/").count(),
            0,
            "#1175 ({case}): the recreated memory/ starts empty"
        );
        let replica = std::path::Path::new(&side.replica_cwd);
        assert!(
            rotated_memory_dirs(replica).is_empty() && !replica.join("memory").exists(),
            "#1175 ({case}): the replica itself must gain no memory* entry"
        );
    }

    async fn create_target_for_test(
        app: &tauri::App<tauri::test::MockRuntime>,
        session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
        cwd: &str,
        intent: CreateSelectionIntent,
    ) -> Result<SessionInfo, String> {
        super::create_session_inner(
            app.handle(),
            session_mgr,
            pty_mgr,
            "codex".to_string(),
            Vec::new(),
            cwd.to_string(),
            Some("target gate fixture".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            intent,
        )
        .await
    }

    async fn create_scripted_session(
        app: &tauri::App<tauri::test::MockRuntime>,
        session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
        cwd: &str,
    ) -> SessionInfo {
        super::create_session_inner(
            app.handle(),
            session_mgr,
            pty_mgr,
            "test-shell".to_string(),
            Vec::new(),
            cwd.to_string(),
            Some("restart fixture".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .expect("create scripted session")
    }

    fn capture_session_lifecycle(
        app: &tauri::App<tauri::test::MockRuntime>,
    ) -> std::sync::mpsc::Receiver<(String, String)> {
        use tauri::Listener;

        let (sender, receiver) = std::sync::mpsc::channel();
        for event_name in [
            "session_created",
            "session_destroyed",
            "session_switched",
            "session_communication_changed",
        ] {
            let sender = sender.clone();
            app.listen_any(event_name, move |event| {
                let _ = sender.send((event_name.to_string(), event.payload().to_string()));
            });
        }
        receiver
    }

    async fn close_test_coordinator(app: &tauri::App<tauri::test::MockRuntime>) {
        use tauri::Manager;

        app.state::<crate::session::selection::SelectionCoordinator>()
            .close_and_join()
            .await;
    }

    // #1063: while a real create is paused at `before_project_gate` with its pending
    // row inserted, the deletion-only pending-inclusive snapshot must observe its
    // working directory even though every public read hides it. This is the exact
    // property the Agent Matrix delete recheck relies on.
    #[tokio::test]
    async fn before_project_gate_barrier_exposes_pending_create_to_deletion_snapshot_only() {
        let cwd = tempfile::tempdir().unwrap();
        let cwd_str = cwd.path().to_string_lossy().into_owned();
        // The create normalizes its cwd; key the barriers by the same value.
        let barrier_key = crate::path_utils::normalize_windows_verbatim_path(&cwd_str);
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let backend = Arc::new(GatedSpawnBackend::new(started_tx, release_rx, false));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend,
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );

        let before_gate = super::seed_race_barriers::install_before_project_gate(&barrier_key);
        let after_seed = super::seed_race_barriers::install_after_seed_before_pty(&barrier_key);

        let create = {
            let app = app.handle().clone();
            let session_mgr = Arc::clone(&session_mgr);
            let pty_mgr = Arc::clone(&pty_mgr);
            let cwd_str = cwd_str.clone();
            tokio::spawn(async move {
                super::create_session_inner(
                    &app,
                    &session_mgr,
                    &pty_mgr,
                    "codex".to_string(),
                    Vec::new(),
                    cwd_str,
                    Some("race fixture".to_string()),
                    None,
                    None,
                    true,
                    Vec::new(),
                    true,
                    None,
                    None,
                    None,
                    CreateSelectionIntent::User,
                )
                .await
            })
        };

        let assert_pending_visible_to_deletion_only = || {
            let session_mgr = Arc::clone(&session_mgr);
            let cwd_str = cwd_str.clone();
            async move {
                assert!(
                    session_mgr.read().await.list_sessions().await.is_empty(),
                    "the pending create must stay hidden from public reads"
                );
                let workdirs = tokio::task::spawn_blocking(move || {
                    session_mgr
                        .blocking_read()
                        .live_working_directories_for_deletion_blocking()
                })
                .await
                .unwrap();
                assert!(
                    workdirs.iter().any(|dir| dir == &cwd_str),
                    "the deletion-only snapshot must include the pending create workdir; got {workdirs:?}"
                );
            }
        };

        // Paused at before_project_gate, pending row inserted.
        before_gate.reached.notified().await;
        assert_pending_visible_to_deletion_only().await;
        before_gate.release.notify_one();

        // Paused at after_seed_before_pty, still pending and unfinalized.
        after_seed.reached.notified().await;
        assert_pending_visible_to_deletion_only().await;
        after_seed.release.notify_one();

        // Release the create and drive its PTY spawn to completion.
        started_rx.await.unwrap();
        release_tx.send(()).unwrap();
        create
            .await
            .expect("join create")
            .expect("create completes");
        close_test_coordinator(&app).await;
    }

    async fn create_manual_cascade_fixture(
        app: &tauri::App<tauri::test::MockRuntime>,
        session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
        root: &std::path::Path,
    ) -> [Uuid; 4] {
        let mut ids = Vec::new();
        for name in ["external", "member-a", "member-b", "coordinator"] {
            let cwd = root.join(name);
            std::fs::create_dir_all(&cwd).unwrap();
            let info =
                create_scripted_session(app, session_mgr, pty_mgr, &cwd.to_string_lossy()).await;
            ids.push(Uuid::parse_str(&info.id).unwrap());
        }
        ids.try_into().expect("four cascade fixture ids")
    }

    #[tokio::test]
    async fn manual_coordinator_cascade_barrier_publishes_only_one_final_selection() {
        use tauri::Manager;

        let cwd = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let backend = Arc::new(BarrierDestroyBackend::new(started_tx, release_rx));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend,
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let [external, member_a, member_b, coordinator_id] =
            create_manual_cascade_fixture(&app, &session_mgr, &pty_mgr, cwd.path()).await;
        app.state::<crate::session::selection::SelectionCoordinator>()
            .transition(crate::session::selection::SelectionRequest::user_switch(
                member_a,
            ))
            .await
            .unwrap();
        let events = capture_session_lifecycle(&app);
        let close = {
            let app = app.handle().clone();
            tokio::spawn(async move {
                execute_manual_coordinator_destroy(
                    &app,
                    coordinator_id,
                    vec![member_a, member_b],
                    true,
                )
                .await
            })
        };

        assert_eq!(started_rx.await.unwrap(), member_a);
        assert_eq!(
            session_mgr.read().await.selection_payload().await.id(),
            Some(member_a),
            "selection must remain stable while the cascade is barrier-held"
        );
        assert!(events.try_recv().is_err());
        release_tx.send(()).unwrap();
        let outcome = close
            .await
            .expect("join cascade close")
            .expect("cascade succeeds");
        assert_eq!(
            outcome.destroyed_ids,
            vec![member_a, member_b, coordinator_id]
        );
        let selection = session_mgr.read().await.selection_payload().await;
        assert_eq!(selection.id(), Some(external));
        assert_eq!(
            selection.source(),
            crate::session::selection::SelectionSource::ManualClose
        );
        let observed = (0..4)
            .map(|_| {
                events
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .unwrap()
                    .0
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![
                "session_destroyed",
                "session_destroyed",
                "session_destroyed",
                "session_switched",
            ]
        );
        assert!(events.try_recv().is_err());
        close_test_coordinator(&app).await;
    }

    #[tokio::test]
    async fn manual_coordinator_cascade_failed_selected_member_stays_selected_without_fallback() {
        use tauri::Manager;

        let cwd = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let backend = Arc::new(BarrierDestroyBackend::new(started_tx, release_rx));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let [_external, member_a, member_b, coordinator_id] =
            create_manual_cascade_fixture(&app, &session_mgr, &pty_mgr, cwd.path()).await;
        app.state::<crate::session::selection::SelectionCoordinator>()
            .transition(crate::session::selection::SelectionRequest::user_switch(
                member_a,
            ))
            .await
            .unwrap();
        backend.fail_kill_for(member_a);
        let revision = session_mgr
            .read()
            .await
            .selection_payload()
            .await
            .revision();
        let events = capture_session_lifecycle(&app);
        let close = {
            let app = app.handle().clone();
            tokio::spawn(async move {
                execute_manual_coordinator_destroy(
                    &app,
                    coordinator_id,
                    vec![member_a, member_b],
                    true,
                )
                .await
            })
        };
        assert_eq!(started_rx.await.unwrap(), member_a);
        release_tx.send(()).unwrap();
        let outcome = close
            .await
            .expect("join failed-member cascade")
            .expect("coordinator still closes");

        assert!(outcome
            .failed
            .iter()
            .any(|(session_id, error)| *session_id == member_a
                && error.contains("synthetic live teardown failure")));
        assert_eq!(outcome.destroyed_ids, vec![member_b, coordinator_id]);
        assert!(session_mgr
            .read()
            .await
            .get_session(member_a)
            .await
            .is_some());
        let selection = session_mgr.read().await.selection_payload().await;
        assert_eq!(selection.id(), Some(member_a));
        assert_eq!(selection.revision(), revision);
        let observed = (0..2)
            .map(|_| {
                events
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .unwrap()
                    .0
            })
            .collect::<Vec<_>>();
        assert_eq!(observed, vec!["session_destroyed", "session_destroyed"]);
        assert!(events.try_recv().is_err());
        close_test_coordinator(&app).await;
    }

    #[tokio::test]
    async fn restart_success_publishes_created_destroyed_selection_in_order() {
        use tauri::Manager;

        let cwd = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let old =
            create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy())
                .await;
        let old_id = Uuid::parse_str(&old.id).unwrap();
        let carried = SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: "2026-07-16T00:00:00Z".to_string(),
        };
        session_mgr
            .read()
            .await
            .set_communication_for_test(old_id, carried.clone())
            .await;
        let events = capture_session_lifecycle(&app);
        let settings = app.state::<crate::config::settings::SettingsState>();

        let replacement = super::restart_session_inner_with_activation(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            settings.inner(),
            old_id,
            None,
            None,
            Some(false),
            true,
        )
        .await
        .expect("restart succeeds");
        let replacement_id = Uuid::parse_str(&replacement.id).unwrap();
        assert_ne!(replacement_id, old_id);
        assert!(!backend.has_session(old_id));
        assert!(backend.has_session(replacement_id));
        let rows = session_mgr.read().await.list_sessions().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, replacement.id);
        assert_eq!(replacement.communication, Some(carried));
        let selection = session_mgr.read().await.selection_payload().await;
        assert_eq!(selection.id(), Some(replacement_id));
        assert_eq!(
            selection.source(),
            crate::session::selection::SelectionSource::Restart
        );

        let observed = (0..3)
            .map(|_| {
                events
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["session_created", "session_destroyed", "session_switched"]
        );
        assert!(events.try_recv().is_err());
        let created_payload: serde_json::Value = serde_json::from_str(&observed[0].1).unwrap();
        assert_eq!(created_payload["communication"]["kind"], "raiseHand");
        let selection_payload: serde_json::Value = serde_json::from_str(&observed[2].1).unwrap();
        assert_eq!(selection_payload["id"], replacement.id);
        assert_eq!(selection_payload["source"], "restart");
        assert_eq!(selection_payload["userInitiated"], true);
        close_test_coordinator(&app).await;
    }

    #[tokio::test]
    async fn restart_of_live_wg_replica_session_does_not_sessionrace() {
        // #1101: `teardown_old_for_restart` retains the old canonical row (non-Exited)
        // until the atomic commit's `mutations.remove(uuid)`, which runs AFTER the
        // create-gate. For a live WG replica restart (Suppress intent + replica cwd) the
        // gate saw `appeared_session == true` and returned Err("sessionRace"). The gate
        // must be skipped for the restart replacement create. The existing restart tests
        // use a plain tempdir (not a WG replica), so they never entered this gate.
        let (_fixture, first_cwd, _second) = strict_target_fixture();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );

        // Seed a LIVE session at the strict WG replica cwd. User intent bypasses the
        // create-gate on create and establishes a non-Exited canonical row.
        let old = create_target_for_test(
            &app,
            &session_mgr,
            &pty_mgr,
            &first_cwd,
            CreateSelectionIntent::User,
        )
        .await
        .expect("seed live WG replica session");
        let old_id = Uuid::parse_str(&old.id).unwrap();

        let settings = app.state::<crate::config::settings::SettingsState>();
        let replacement = super::restart_session_inner_with_activation(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            settings.inner(),
            old_id,
            None,        // agent_id
            None,        // requested_profile
            Some(false), // skip_auto_resume
            true,        // activate_after
        )
        .await
        .unwrap_or_else(|e| {
            panic!("restart of a live WG replica session must not fail; got Err({e})")
        });

        let replacement_id = Uuid::parse_str(&replacement.id).unwrap();
        assert_ne!(replacement_id, old_id, "restart must replace the old row");
        let rows = session_mgr.read().await.list_sessions().await;
        assert_eq!(rows.len(), 1, "old row replaced, not duplicated");
        assert_eq!(rows[0].id, replacement.id);
        close_test_coordinator(&app).await;
    }

    #[tokio::test]
    async fn restart_after_route_loss_cleans_old_external_state_once_before_replacement() {
        use tauri::Manager;

        let cwd = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let old =
            create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy())
                .await;
        let old_id = Uuid::parse_str(&old.id).unwrap();
        session_mgr
            .read()
            .await
            .set_telegram_bot_id(old_id, Some("old-bot".to_string()))
            .await;
        let telegram = app.state::<crate::telegram::manager::TelegramBridgeState>();
        telegram.lock().await.insert_test_bridge(old_id, "old-bot");
        app.state::<crate::DetachedSessionsState>()
            .lock()
            .unwrap()
            .insert(old_id);

        backend.lose_route(old_id);
        pty_mgr.lock().unwrap().remove_route_if_kind(
            old_id,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        );
        let (spawn_started_tx, spawn_started_rx) = tokio::sync::oneshot::channel();
        let (spawn_release_tx, spawn_release_rx) = tokio::sync::oneshot::channel();
        backend.gate_spawn(2, spawn_started_tx, spawn_release_rx);
        let events = capture_session_lifecycle(&app);
        let restart = {
            let app_handle = app.handle().clone();
            let session_mgr = Arc::clone(&session_mgr);
            let pty_mgr = Arc::clone(&pty_mgr);
            let settings = app
                .state::<crate::config::settings::SettingsState>()
                .inner()
                .clone();
            tokio::spawn(async move {
                super::restart_session_inner_with_activation(
                    &app_handle,
                    &session_mgr,
                    &pty_mgr,
                    &settings,
                    old_id,
                    None,
                    None,
                    Some(false),
                    true,
                )
                .await
            })
        };

        let replacement_id = spawn_started_rx.await.expect("replacement spawn starts");
        assert!(!telegram.lock().await.has_bridge(old_id));
        assert_eq!(telegram.lock().await.test_detach_count(old_id), 1);
        assert!(!app
            .state::<crate::DetachedSessionsState>()
            .lock()
            .unwrap()
            .contains(&old_id));
        let mut route_loss = {
            let sender = app
                .state::<crate::session::selection::SelectionCoordinator>()
                .container_lifecycle_sender();
            tokio::spawn(async move { sender.route_lost(old_id, 91).await })
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut route_loss)
                .await
                .is_err(),
            "late route-loss reconciliation must serialize behind restart"
        );
        spawn_release_tx.send(()).unwrap();
        let replacement = restart
            .await
            .expect("join restart")
            .expect("restart succeeds after old route loss");
        assert_eq!(replacement.id, replacement_id.to_string());
        assert_eq!(
            route_loss
                .await
                .expect("join route-loss callback")
                .expect("late route-loss callback"),
            crate::session::selection::CriticalAdmissionOutcome::Completed(())
        );
        assert_eq!(telegram.lock().await.test_detach_count(old_id), 1);
        assert!(session_mgr.read().await.get_session(old_id).await.is_none());
        assert_eq!(
            session_mgr.read().await.selection_payload().await.id(),
            Some(replacement_id)
        );
        let observed = (0..3)
            .map(|_| {
                events
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .unwrap()
                    .0
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec!["session_created", "session_destroyed", "session_switched"]
        );
        assert!(events.try_recv().is_err());
        close_test_coordinator(&app).await;
    }

    #[tokio::test]
    async fn restart_pre_teardown_failure_preserves_old_row_route_selection_and_events() {
        use tauri::Manager;

        let cwd = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let old =
            create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy())
                .await;
        let old_id = Uuid::parse_str(&old.id).unwrap();
        let events = capture_session_lifecycle(&app);
        let settings = app.state::<crate::config::settings::SettingsState>();
        settings.write().await.archived_project_paths =
            vec![cwd.path().to_string_lossy().to_string()];

        let error = super::restart_session_inner_with_activation(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            settings.inner(),
            old_id,
            None,
            None,
            Some(true),
            true,
        )
        .await
        .expect_err("restart teardown must fail");
        assert!(
            error.contains("Cannot start a session in archived project"),
            "{error}"
        );
        assert!(backend.has_session(old_id));
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
        assert_eq!(session_mgr.read().await.list_sessions().await.len(), 1);
        assert_eq!(
            session_mgr.read().await.selection_payload().await.id(),
            Some(old_id)
        );
        assert!(events.try_recv().is_err());
        close_test_coordinator(&app).await;
    }

    #[tokio::test]
    async fn restart_post_teardown_spawn_failure_destroys_old_and_publishes_one_null() {
        use tauri::Manager;

        let cwd = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let old =
            create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy())
                .await;
        let old_id = Uuid::parse_str(&old.id).unwrap();
        let events = capture_session_lifecycle(&app);
        backend.fail_spawn(2);
        let settings = app.state::<crate::config::settings::SettingsState>();

        let error = super::restart_session_inner_with_activation(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            settings.inner(),
            old_id,
            None,
            None,
            Some(true),
            true,
        )
        .await
        .expect_err("replacement spawn must fail");
        assert!(
            error.contains("synthetic scripted spawn failure"),
            "{error}"
        );
        assert!(session_mgr.read().await.list_sessions().await.is_empty());
        let selection = session_mgr.read().await.selection_payload().await;
        assert_eq!(selection.id(), None);
        assert_eq!(
            selection.source(),
            crate::session::selection::SelectionSource::Restart
        );
        assert!(!backend.has_session(old_id));

        let destroyed = events
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("old destroy event");
        let switched = events
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("restart null event");
        assert_eq!(destroyed.0, "session_destroyed");
        assert_eq!(switched.0, "session_switched");
        assert!(events.try_recv().is_err());
        let selection_payload: serde_json::Value = serde_json::from_str(&switched.1).unwrap();
        assert!(selection_payload["id"].is_null());
        assert_eq!(selection_payload["source"], "restart");
        close_test_coordinator(&app).await;
    }

    #[tokio::test]
    async fn dormant_restart_failure_retains_exact_exit_and_emits_no_lifecycle_event() {
        use tauri::Manager;

        let cwd = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let old =
            create_scripted_session(&app, &session_mgr, &pty_mgr, &cwd.path().to_string_lossy())
                .await;
        let old_id = Uuid::parse_str(&old.id).unwrap();
        backend.kill(old_id).unwrap();
        app.state::<crate::session::selection::SelectionCoordinator>()
            .container_lifecycle_sender()
            .route_lost(old_id, 23)
            .await
            .expect("reconcile dormant route");
        let events = capture_session_lifecycle(&app);
        backend.fail_spawn(2);
        let settings = app.state::<crate::config::settings::SettingsState>();

        super::restart_session_inner_with_activation(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            settings.inner(),
            old_id,
            None,
            None,
            Some(false),
            true,
        )
        .await
        .expect_err("dormant replacement spawn must fail");
        let retained = session_mgr
            .read()
            .await
            .get_session(old_id)
            .await
            .expect("dormant old row retained");
        assert_eq!(retained.status, SessionStatus::Exited(23));
        let selection = session_mgr.read().await.selection_payload().await;
        assert_eq!(selection.id(), Some(old_id));
        assert_eq!(selection.status(), Some(&SessionStatus::Exited(23)));
        assert!(events.try_recv().is_err());
        close_test_coordinator(&app).await;
    }

    #[tokio::test]
    async fn create_session_inner_rolls_back_pre_created_session_on_spawn_error() {
        let temp = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(FailingSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let app_handle = app.handle().clone();

        let err = super::create_session_inner(
            &app_handle,
            &session_mgr,
            &pty_mgr,
            "missing-ac-test-command".to_string(),
            Vec::new(),
            temp.path().to_string_lossy().to_string(),
            None,
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            // #973 - headless caller: no terminal to measure, keep 120x30.
            None,
            CreateSelectionIntent::User,
        )
        .await
        .expect_err("spawn should fail");

        assert!(err.contains("synthetic spawn failure"), "{err}");
        assert!(session_mgr.read().await.list_sessions().await.is_empty());
        tokio::time::timeout(Duration::from_secs(1), async {
            while backend.killed().len() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("reserved rollback kills the failed spawn exactly once");
        assert_eq!(backend.killed(), backend.spawned());
        assert_eq!(backend.killed().len(), 1);
    }

    /// #973 - THE REGRESSION. Red against main, which hardcodes `cols: 120, rows: 30`.
    ///
    /// AC used to open every ConPTY at 120x30 and let the frontend correct it 300-500 ms
    /// later. That correction lands inside a coding agent's TUI startup, and a resize there
    /// makes Codex redraw its still-empty viewport and lose the wakeup for the content that
    /// becomes ready straight after: a blank terminal, alive, until any key is pressed.
    ///
    /// Measured outside AC, on a bare ConPTY: opened at 120x30 and then resized in the
    /// window, Codex comes up blank 8 times in 10. Opened at the size the view actually
    /// wants, and never resized, 0 times in 10.
    ///
    /// This pins the plumbing: the size the caller supplies is the size the PTY is opened
    /// at. It does not, and cannot, prove Codex stops hanging - that needs a real child in a
    /// real ConPTY inside a ~100 ms race. The harness is the proof of that.
    #[tokio::test]
    async fn privileged_preheld_target_gate_does_not_reenter_and_rechecks_pending_or_live() {
        let (_fixture, first_cwd, _) = strict_target_fixture();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            AppSettings::default(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let target =
            crate::config::teams::verify_pty_input_replica_cwd(std::path::Path::new(&first_cwd))
                .unwrap();
        let state = app.state::<crate::api::message_store::MessageStoreState>();
        let gate = state.target_gate.as_ref().unwrap().clone();
        let stripe = gate
            .acquire_target_lock(&target.canonical_fqn)
            .await
            .unwrap();
        let exact = gate.acquire_exact(&target.canonical_fqn).await;
        let ownership = gate
            .target_ownership(&target.canonical_fqn, &stripe, &exact)
            .unwrap();

        let pending = pty_mgr
            .lock()
            .unwrap()
            .mark_spawning(&first_cwd, "pending race");
        let pending_error = super::create_session_inner_with_pty_target_ownership(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            first_cwd.clone(),
            Some("privileged target".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::Background,
            &ownership,
        )
        .await
        .unwrap_err();
        assert_eq!(pending_error, "sessionRace");
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 0);
        drop(pending);

        let created = super::create_session_inner_with_pty_target_ownership(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            first_cwd.clone(),
            Some("privileged target".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::Background,
            &ownership,
        )
        .await
        .unwrap();
        assert_eq!(created.working_directory, first_cwd);
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);

        let live_error = super::create_session_inner_with_pty_target_ownership(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            first_cwd,
            Some("stale background create".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::Background,
            &ownership,
        )
        .await
        .unwrap_err();
        assert_eq!(live_error, "sessionRace");
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ordinary_strict_create_is_fail_soft_when_pty_store_is_error_or_missing() {
        for store_state in [SessionTestStoreState::Error, SessionTestStoreState::Missing] {
            let (_fixture, first_cwd, second_cwd) = strict_target_fixture();
            let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
            let backend = Arc::new(ScriptedSpawnBackend::default());
            let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
                backend.clone(),
            )));
            let app = session_test_app_with_store(
                AppSettings::default(),
                Arc::clone(&session_mgr),
                Arc::clone(&pty_mgr),
                store_state,
            );

            let created = create_target_for_test(
                &app,
                &session_mgr,
                &pty_mgr,
                &first_cwd,
                CreateSelectionIntent::User,
            )
            .await;

            assert!(
                created.is_ok(),
                "ordinary create must not depend on the specialized PTY store: {created:?}"
            );
            assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 1);

            let restore =
                crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
            let restored = super::create_session_inner_for_restore(
                &restore,
                &session_mgr,
                &pty_mgr,
                "codex".to_string(),
                Vec::new(),
                second_cwd,
                Some("restore target".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None,
                None,
                None,
            )
            .await;
            assert!(
                restored.is_ok(),
                "startup restore must not depend on the specialized PTY store: {restored:?}"
            );
            assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
            close_test_coordinator(&app).await;
        }
    }

    #[tokio::test]
    async fn sequential_user_same_target_creates_remain_compatible() {
        let (_fixture, first_cwd, _) = strict_target_fixture();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            AppSettings::default(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        create_target_for_test(
            &app,
            &session_mgr,
            &pty_mgr,
            &first_cwd,
            CreateSelectionIntent::User,
        )
        .await
        .unwrap();
        create_target_for_test(
            &app,
            &session_mgr,
            &pty_mgr,
            &first_cwd,
            CreateSelectionIntent::User,
        )
        .await
        .unwrap();
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
        assert_eq!(session_mgr.read().await.list_sessions().await.len(), 2);
    }

    #[tokio::test]
    async fn different_strict_targets_create_concurrently() {
        let (_fixture, first_cwd, second_cwd) = strict_target_fixture();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        backend.gate_spawn(1, started_tx, release_rx);
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            AppSettings::default(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let first_handle = app.handle().clone();
        let first_sessions = Arc::clone(&session_mgr);
        let first_pty = Arc::clone(&pty_mgr);
        let first_task = tokio::spawn(async move {
            super::create_session_inner(
                &first_handle,
                &first_sessions,
                &first_pty,
                "codex".to_string(),
                Vec::new(),
                first_cwd,
                Some("first target".to_string()),
                None,
                None,
                true,
                Vec::new(),
                true,
                None,
                None,
                None,
                CreateSelectionIntent::User,
            )
            .await
        });
        started_rx.await.unwrap();
        let second = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            create_target_for_test(
                &app,
                &session_mgr,
                &pty_mgr,
                &second_cwd,
                CreateSelectionIntent::User,
            ),
        )
        .await
        .expect("a different target must not wait for the first exact gate")
        .unwrap();
        assert_eq!(second.working_directory, second_cwd);
        release_tx.send(()).unwrap();
        first_task.await.unwrap().unwrap();
        assert_eq!(backend.spawn_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_same_target_creates_serialize_on_the_exact_gate() {
        let (_fixture, first_cwd, _) = strict_target_fixture();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        // Gate the first spawn so the winning create holds the exact target gate.
        backend.gate_spawn(1, started_tx, release_rx);
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            AppSettings::default(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );

        // Task A wins the exact gate for the target and blocks inside its spawn.
        let a_task = {
            let handle = app.handle().clone();
            let sessions = Arc::clone(&session_mgr);
            let pty = Arc::clone(&pty_mgr);
            let cwd = first_cwd.clone();
            tokio::spawn(async move {
                super::create_session_inner(
                    &handle,
                    &sessions,
                    &pty,
                    "codex".to_string(),
                    Vec::new(),
                    cwd,
                    Some("first same-target".to_string()),
                    None,
                    None,
                    true,
                    Vec::new(),
                    true,
                    None,
                    None,
                    None,
                    CreateSelectionIntent::Background,
                )
                .await
            })
        };
        started_rx.await.unwrap();

        // Task B is a concurrent background create for the SAME target. It must
        // block on the exact gate that A holds, not race into the missing-target
        // window beside A.
        let b_task = {
            let handle = app.handle().clone();
            let sessions = Arc::clone(&session_mgr);
            let pty = Arc::clone(&pty_mgr);
            let cwd = first_cwd.clone();
            tokio::spawn(async move {
                super::create_session_inner(
                    &handle,
                    &sessions,
                    &pty,
                    "codex".to_string(),
                    Vec::new(),
                    cwd,
                    Some("second same-target".to_string()),
                    None,
                    None,
                    true,
                    Vec::new(),
                    true,
                    None,
                    None,
                    None,
                    CreateSelectionIntent::Background,
                )
                .await
            })
        };

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !b_task.is_finished(),
            "a second same-target create must block on the exact gate while the first holds it"
        );
        assert_eq!(
            backend.spawn_count.load(Ordering::SeqCst),
            1,
            "no second spawn may start while the exact target gate is held"
        );

        // Release A; it finalizes a live session and drops the gate. B then wins the
        // gate, rechecks live/pending state, and refuses to spawn a duplicate.
        release_tx.send(()).unwrap();
        let created = a_task.await.unwrap().unwrap();
        assert_eq!(created.working_directory, first_cwd);
        let raced = b_task.await.unwrap().unwrap_err();
        assert_eq!(
            raced, "sessionRace",
            "the serialized loser must observe the now-live target and not create beside it"
        );
        assert_eq!(
            backend.spawn_count.load(Ordering::SeqCst),
            1,
            "the serialized loser never spawned a duplicate"
        );
        assert_eq!(session_mgr.read().await.list_sessions().await.len(), 1);
    }

    #[tokio::test]
    async fn create_session_opens_the_pty_at_the_size_the_view_supplied() {
        let temp = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(FailingSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let app_handle = app.handle().clone();

        let _ = super::create_session_inner(
            &app_handle,
            &session_mgr,
            &pty_mgr,
            "missing-ac-test-command".to_string(),
            Vec::new(),
            temp.path().to_string_lossy().to_string(),
            None,
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            Some(crate::pty::backend::PtyViewport::from_fit(74, 23)),
            CreateSelectionIntent::User,
        )
        .await;

        assert_eq!(
            backend.sizes(),
            vec![(74, 23)],
            "the PTY must be opened at the size the view fitted to, not at 120x30: \
             opening at the wrong size is what forces the startup resize that loses \
             Codex's first render (#973)"
        );
    }

    /// #973 - backward compatibility. Every headless caller (startup restore, the delivery
    /// loop, the phone mailbox, the CLI, tests) has no terminal to measure and must keep
    /// AC's historical 120x30. Those sessions are never attached to a view, so nothing
    /// resizes them, and none of them ever hit this bug.
    #[tokio::test]
    async fn create_session_without_a_view_keeps_the_historical_120x30() {
        let temp = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(FailingSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let app_handle = app.handle().clone();

        let _ = super::create_session_inner(
            &app_handle,
            &session_mgr,
            &pty_mgr,
            "missing-ac-test-command".to_string(),
            Vec::new(),
            temp.path().to_string_lossy().to_string(),
            None,
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None, // no view
            CreateSelectionIntent::User,
        )
        .await;

        assert_eq!(
            backend.sizes(),
            vec![(120, 30)],
            "a caller with no view must behave exactly as it did before #973"
        );
    }

    /// #973 - opening a 0-column ConPTY would be worse than the bug we are fixing, so a
    /// degenerate fitted size must fall back rather than be honoured.
    ///
    /// It does not come from xterm: `fit()` clamps to MINIMUM_COLS = 2 / MINIMUM_ROWS = 1. It is
    /// guarded because this is a `u16` boundary that anything upstream can hand a 0 to, and
    /// because the cost of being wrong is a terminal with no screen at all.
    #[tokio::test]
    async fn a_degenerate_fitted_size_falls_back_instead_of_opening_a_zero_column_pty() {
        let temp = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(FailingSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let app_handle = app.handle().clone();

        let _ = super::create_session_inner(
            &app_handle,
            &session_mgr,
            &pty_mgr,
            "missing-ac-test-command".to_string(),
            Vec::new(),
            temp.path().to_string_lossy().to_string(),
            None,
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            Some(crate::pty::backend::PtyViewport::from_fit(0, 0)),
            CreateSelectionIntent::User,
        )
        .await;

        assert_eq!(
            backend.sizes(),
            vec![(120, 30)],
            "a 0x0 fit must fall back to the default, never open a 0-column ConPTY"
        );
    }

    #[test]
    fn create_session_inner_keeps_both_archive_activation_gates() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/session.rs"
        ))
        .expect("read session.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production session source");
        let normalized = production.split_whitespace().collect::<String>();
        let gate_call = "crate::config::archive_gate::enforce_unarchived_for_spawn(app,&cwd,&session_label).await";
        let count = normalized.matches(gate_call).count();

        assert_eq!(
            count, 2,
            "create_session_inner must keep both archive activation gates"
        );
    }

    // T16 (#1172 D5), rescoped by #1175. READ THIS BEFORE TRUSTING IT.
    //
    // This is a SOURCE-LEVEL INVENTORY over `session.rs` alone. It does NOT prove
    // that a resume never rotates, and it does not close the class of changes that
    // retarget the rotation. Three measured facts bound it:
    //
    //   - A cross-file alias (`pub(crate) use ... as <alias>;` in `config/mod.rs`
    //     plus an ungated call to the alias here) keeps assertions 1 to 3 intact
    //     while a resume rotates. Recorded in the issue.
    //   - A cross-file CALL added in `lib.rs` before the restore at `:2350` is
    //     invisible to every assertion here and to both behavioral tests below.
    //   - `skip_auto_resume |= is_coordinator;` inserted immediately above the
    //     binding leaves ALL FIVE assertions and BOTH behavioral tests green while
    //     a coordinator resume rotates. Measured; plan 2.8.
    //
    // What it DOES enforce, and what nothing else in this suite can:
    //   - assertions 1 to 3: `rotate_origin_memory_at_spawn` is REFERENCED exactly
    //     twice in this file, once in the comment and once in the gated call. A
    //     third occurrence is an UNBUDGETED REFERENCE; it need not be a call. This
    //     is what catches an ungated second call added on a launch branch the
    //     behavioral tests do not drive (`execute_restart_transaction` at `:3897`,
    //     the Root Agent launch at `:4703`).
    //   - assertions 4 and 5: the name `start_fresh` is bound exactly once, and its
    //     right-hand side is exactly the resume flag. That is a complete statement
    //     about ONE LINE. It says nothing about the value flowing into that line,
    //     which is the residual named above and in plan D5.
    //
    // The normative property is guarded behaviorally by
    // `a_production_shaped_startup_restore_never_rotates_while_a_fresh_launch_does`
    // and `create_session_inner_rotates_the_fresh_target_and_not_the_resumed_one`,
    // for the entry points and argument shapes those tests drive and no further.
    // Plan section 4.5 enumerates what neither side covers.
    //
    // One invisible dependency, already satisfied: the split needle below uses
    // `\n`. This repository sets `core.autocrlf=true` on Windows checkouts, and
    // `.gitattributes` `*.rs text eol=lf` is what keeps `session.rs` LF on disk
    // (measured: 0 CRLF, 9344 LF). If that attribute were ever dropped, this test
    // fails LOUD rather than silent: assertion 1 would count 2, because the needle
    // also appears as this test's own `let gated_call = "..."` literal.
    #[test]
    fn rotate_origin_memory_at_spawn_has_one_gated_call_in_session_rs() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/session.rs"
        ))
        .expect("read session.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production session source");
        let normalized = production.split_whitespace().collect::<String>();
        let gated_call = "ifstart_fresh&&context_result.is_ok(){crate::config::agent_memory::rotate_origin_memory_at_spawn(&cwd);}";

        assert_eq!(
            normalized.matches(gated_call).count(),
            1,
            "#1172 D5: the memory rotation must stay behind `start_fresh && context_result.is_ok()`. \
             This pins the gate's exact spelling at the single call site in this file; it does not \
             prove the resume property, and the behavioral tests named above prove it only for the \
             entry points they drive."
        );
        // What assertions 2 and 3 add to assertion 1, stated as the bounded
        // property they actually enforce.
        //
        // Assertion 1 pins one exact spelling of the gated call, so a second
        // call spelled differently would not match its needle: a different
        // argument, the path wrapped in parens so it reads `..._at_spawn)(`, or
        // a block comment between the identifier and the paren. Assertions 2
        // and 3 reach those by counting the BARE identifier rather than an
        // invocation shape, and by pinning the one legitimate occurrence that
        // is not a call, the comment 5.4 requires above the gate.
        //
        // The property they enforce, and nothing broader: within `session.rs`
        // the exact identifier `rotate_origin_memory_at_spawn` occurs exactly
        // twice, once in that comment and once in the call assertion 1 already
        // pinned. A third occurrence is unbudgeted.
        //
        // That is a count of one identifier in one file. It does NOT establish
        // that every route to the rotation is accounted for. A call reached
        // through a cross-file alias, and a call added in another file, both
        // leave this count at two; both are recorded in the header above, and
        // the second is plan 9.2's M9, measured green against all five
        // assertions here and both behavioral tests below.
        let comment_mention = "`rotate_origin_memory_at_spawn`returns`()`";
        assert_eq!(
            normalized.matches(comment_mention).count(),
            1,
            "#1172 D5: the comment above the gate must keep naming the rotation exactly once; \
             it is the one non-call mention the reference budget below accounts for."
        );
        assert_eq!(
            normalized.matches("rotate_origin_memory_at_spawn").count(),
            2,
            "#1172 D5: within THIS FILE the rotation entry point must be REFERENCED exactly twice - \
             once in the comment above the gate, once in the gated call itself. A third occurrence \
             is unbudgeted: it may be a second call site, an import, or any other reference, and \
             each of those is a change this inventory deliberately refuses to absorb silently. A \
             cross-file alias evades this count entirely; see the header comment."
        );
        // #1175 D2 part 4. The gate's INPUT, not just its shape. Bounded claim: this
        // pins ONE LINE. It cannot see how `skip_auto_resume` got its value; see the
        // header and plan D5 residual 3.
        assert_eq!(
            normalized
                .matches("letstart_fresh=skip_auto_resume;")
                .count(),
            1,
            "#1175: `start_fresh` must be bound to the resume flag ALONE. Appending `|| <extra>` \
             here makes a RESUME rotate on the production launches that supply the extra input. \
             Measured (plan 2.7): `|| pending_start_fresh.is_some()` reds this assertion AND the \
             production-shaped restore test; the `|| is_coordinator` variant reds only this one, \
             because no cwd in this test binary can be a coordinator."
        );
        // #1175 round 2. Assertion 4 alone is defeated by ordinary shadowing:
        // `let start_fresh = skip_auto_resume; let start_fresh = start_fresh || X;`
        // preserves its needle exactly. MEASURED (plan 2.8): this assertion is the
        // only thing in the suite that reds on that form.
        assert_eq!(
            normalized.matches("letstart_fresh=").count(),
            1,
            "#1175: `start_fresh` must be bound EXACTLY ONCE. A second binding shadows the first \
             and can compose any additional input into the rotation decision while assertion 4's \
             needle stays intact."
        );
    }

    /// #1175 B1. The behavioral guard on #1172 D5's normative property, within the
    /// scope D5 of this plan declares. This EXECUTES the launch path and looks at
    /// the filesystem, so unlike the reference inventory above it reds under the
    /// cross-file alias the issue built.
    ///
    /// The resume half is PRODUCTION-SHAPED, not merely nominal. Round 0 passed
    /// `pending_start_fresh = None`, `resolved_spawn = None`, `agent_id = None` and
    /// `skip_tooling_save = true`, none of which is what `lib.rs:2313-2372` passes
    /// on an ordinary startup restore. Grinch showed the cost: a one-line
    /// `let start_fresh = skip_auto_resume || pending_start_fresh.is_some();`
    /// left every round-0 guard green while a real restore rotated, because
    /// production passes `Some(false)` there and round 0 passed `None`. Each
    /// argument below is annotated with the production line it mirrors.
    ///
    /// The fresh half is the POSITIVE CONTROL and is not optional: without it, a
    /// green resume half would also be produced by a launch that never reached the
    /// rotation chokepoint, or by the feature having been deleted outright.
    ///
    /// Observation happens at every boundary, never only at the end. See D3.
    #[tokio::test]
    async fn a_production_shaped_startup_restore_never_rotates_while_a_fresh_launch_does() {
        let (_fixture, resumed, fresh) = rotation_fixture();
        // `lib.rs:2314` rebuilds the spawn recipe from settings before restoring.
        let settings = test_settings();
        let spawn = super::build_configured_agent_spawn_for_cwd(
            &settings,
            "codex",
            &resumed.replica_cwd,
            None,
        )
        .expect("resolve the configured codex spawn")
        .expect("codex is configured in test_settings");
        // `lib.rs:2334-2340` takes shell, args and label from the rebuilt spawn.
        let shell = spawn.shell.clone();
        let shell_args = spawn.shell_args.clone();
        let agent_label = Some(spawn.trusted_agent_label.clone());

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

        let restore =
            crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
        let restored = super::create_session_inner_for_restore(
            &restore,
            &session_mgr,
            &pty_mgr,
            shell,
            shell_args,
            resumed.replica_cwd.clone(),
            Some("startup restore".to_string()),
            Some("codex".to_string()), // lib.rs:2358, ps.agent_id
            agent_label,               // lib.rs:2339, spawn.trusted_agent_label
            false,                     // lib.rs:2360, "Persist tooling on restore"
            Vec::new(),
            false,       // lib.rs:2362, skip_auto_resume_for_restore(false): RESUME
            Some(spawn), // lib.rs:2363, the rebuilt recipe
            None,
            None,        // lib.rs:2365, headless caller keeps 120x30
            Some(false), // lib.rs:2366, Some(ps.start_fresh_on_restore). LOAD-BEARING.
            None,
        )
        .await;
        let restored = restored.expect("the production-shaped restore must launch");

        // The launch must be an ACTUAL provider resume, not just a `false` argument.
        // The Codex injection at `:1999` gates on `agent_kind`, and its body gates
        // again on `if let Some(ref aid) = agent_id` at `:2000`, so this only holds
        // because the rebuilt spawn supplies the agent id at `:1446-1450`.
        let effective = restored
            .effective_shell_args
            .clone()
            .unwrap_or_else(|| restored.shell_args.clone());
        assert!(
            effective.iter().any(|arg| arg == "resume"),
            "#1175: the restore must be an ACTUAL Codex provider resume, got {effective:?}"
        );

        // Boundary 1: observe BOTH sides before the second cause runs.
        assert_resume_left_memory_alone(&resumed, "production-shaped startup restore");
        assert_memory_pristine(&fresh, "fresh side, before its own launch");

        let created = super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            fresh.replica_cwd.clone(),
            Some("fresh launch".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true, // a fresh create
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await;
        assert!(created.is_ok(), "the fresh launch must launch: {created:?}");
        assert_eq!(
            backend.spawn_count.load(Ordering::SeqCst),
            2,
            "both launches must reach the PTY (this proves nothing about the context block)"
        );

        // Boundary 2: the fresh side rotated, and the resume side is STILL untouched.
        assert_memory_rotated_once(&fresh, "fresh create");
        assert_resume_left_memory_alone(&resumed, "restore, rechecked after the fresh launch");
        close_test_coordinator(&app).await;
    }

    /// #1175 B2. The same function twice with matched arguments and the same
    /// session label, one launch fresh and one launch resuming, in the OPPOSITE
    /// order from B1.
    ///
    /// It is deliberately NOT called a one-boolean experiment, and it never was
    /// one: the cwd differs, and the second launch runs against a `SessionManager`,
    /// selection coordinator and backend the first already mutated. Naming that
    /// "the flag alone" would grant exactly the false confidence #1175 exists to
    /// remove. What it does establish, together with B1, is that no discriminator
    /// based solely on first-versus-second launch explains the result: B1 runs
    /// resume then fresh, this runs fresh then resume.
    ///
    /// The cwd differs for a positive reason, not because it has to. Two
    /// same-target `CreateSelectionIntent::User` creates are supported and are
    /// already exercised by `sequential_user_same_target_creates_remain_compatible`
    /// (`:6958`); the dedup gate at `:1609` applies to Background and Suppress
    /// only. Separate replicas are used so each side has its own independently
    /// seeded, non-empty `memory/` to observe.
    ///
    /// Its resume half uses the same `false` polarity as the #599 reopen of a
    /// closed coordinator, but it is NOT that scenario: `is_coordinator` is
    /// deterministically false for every cwd in this binary, and the Tauri
    /// `create_session` producer is bypassed. Plan D5 records coordinator polarity
    /// and outer composition as untested.
    ///
    /// The rotating role is assigned to the OPPOSITE replica from B1's, so no
    /// single fixture directory is the one that always rotates.
    #[tokio::test]
    async fn create_session_inner_rotates_the_fresh_target_and_not_the_resumed_one() {
        let (_fixture, fresh, resumed) = rotation_fixture();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );

        let created = super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            fresh.replica_cwd.clone(),
            Some("matched differential".to_string()),
            Some("codex".to_string()),
            Some("Codex".to_string()),
            true,
            Vec::new(),
            true, // FRESH first: the opposite order from B1
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await;
        assert!(created.is_ok(), "the fresh create must launch: {created:?}");

        // Boundary 1.
        assert_memory_rotated_once(&fresh, "fresh create, observed immediately");
        assert_memory_pristine(&resumed, "resume side, before its own launch");

        let reopened = super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            resumed.replica_cwd.clone(),
            Some("matched differential".to_string()),
            Some("codex".to_string()),
            Some("Codex".to_string()),
            true,
            Vec::new(),
            false, // the #599 reopen value: this launch RESUMES
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await;
        assert!(reopened.is_ok(), "the reopen must launch: {reopened:?}");
        assert_eq!(
            backend.spawn_count.load(Ordering::SeqCst),
            2,
            "both launches must reach the PTY"
        );

        // Boundary 2.
        assert_resume_left_memory_alone(&resumed, "reopen that resumes");
        assert_memory_rotated_once(&fresh, "fresh side, rechecked after the resume");
        close_test_coordinator(&app).await;
    }

    #[test]
    fn create_session_inner_marks_spawning_until_spawn_returns() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/session.rs"
        ))
        .expect("read session.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production session source");
        let normalized = production.split_whitespace().collect::<String>();
        let first_gate = normalized
            .find("crate::config::archive_gate::enforce_unarchived_for_spawn(app,&cwd,&session_label).await?;")
            .expect("first archive gate");
        let mark = normalized
            .find("letspawn_mark={")
            .expect("spawn mark immediately before pending-session creation");
        let pending_create = normalized
            .find("letpending_result=ifletSome(ticket)=create_ticket.as_mut(){")
            .expect("pending-session creation");
        let spawn = normalized
            .find(
                "letspawn_result=PtyManager::spawn(pty_mgr,session.backend_kind,spawn_spec).await;",
            )
            .expect("spawn call");
        let drop_mark = normalized
            .find("drop(spawn_mark);")
            .expect("spawn mark drop");
        let spawn_error = normalized
            .find("ifletErr(e)=spawn_result{")
            .expect("spawn error handling");

        assert!(
            first_gate < mark,
            "spawn mark must be created only after pre-create validation"
        );
        assert!(
            mark < pending_create,
            "spawn mark must precede pending-session creation"
        );
        assert!(
            pending_create < spawn,
            "pending-session creation must run before PTY spawn"
        );
        assert!(
            spawn < drop_mark,
            "spawn mark must stay live while spawn awaits"
        );
        assert!(
            drop_mark < spawn_error,
            "spawn mark must drop immediately after spawn returns"
        );
    }

    // Runtime witness for plan section 12: drive create_session_inner with a
    // fake backend and assert archive_liveness sees the spawn mark WHILE the PTY
    // is spawning, then sees it retired once the PTY exists. Unlike the
    // source-scrape guards above, this executes the code, so it reds under a
    // string-preserving runtime mutation (e.g. mark_spawning inserting nothing,
    // or the mark dropped before PtyManager::spawn).
    #[tokio::test]
    async fn create_session_inner_holds_a_spawn_mark_until_the_pty_exists() {
        use crate::pty::backend::PtyBackend;

        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let expected_cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let backend = Arc::new(GatedSpawnBackend::new(started_tx, release_rx, false));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let app_handle = app.handle().clone();

        let task = {
            let app_handle = app_handle.clone();
            let session_mgr = session_mgr.clone();
            let pty_mgr = pty_mgr.clone();
            let cwd = cwd.clone();
            tokio::spawn(async move {
                super::create_session_inner(
                    &app_handle,
                    &session_mgr,
                    &pty_mgr,
                    "hold-mark-test-command".to_string(),
                    Vec::new(),
                    cwd,
                    Some("hold-mark".to_string()),
                    None,
                    None,
                    true,
                    Vec::new(),
                    true,
                    None,
                    None,
                    None, // #973 - no view in this test: 120x30
                    CreateSelectionIntent::User,
                )
                .await
            })
        };

        // Park inside the backend's spawn. PtyManager::spawn releases the outer
        // manager mutex before awaiting backend.spawn, so the mark is readable
        // here without racing the spawn task.
        started_rx.await.expect("spawn started");
        let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
        assert_eq!(
            pending,
            vec![crate::pty::manager::PendingSpawn {
                cwd: expected_cwd.clone(),
                label: "hold-mark".to_string(),
            }],
            "spawn mark must be live while the PTY is still being spawned"
        );

        let _ = release_tx.send(());
        let info = task
            .await
            .expect("join create_session_inner")
            .expect("create_session_inner should succeed");

        assert!(
            backend.has_session(Uuid::parse_str(&info.id).expect("session id is a uuid")),
            "the PTY must exist once create_session_inner returns Ok"
        );
        let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
        assert!(
            pending.is_empty(),
            "spawn mark must retire once the PTY exists"
        );
    }

    // Runtime witness for plan section 12: the mark is held across a FAILING
    // spawn exactly as across a succeeding one, and is retired on the rollback
    // path. Holding the mark across the in-flight spawn is what makes this red
    // under a string-preserving mutation (mark_spawning no-op / drop before
    // spawn); the empty-afterwards assertion pins retirement on the failure arm.
    #[tokio::test]
    async fn create_session_inner_retires_the_spawn_mark_on_spawn_failure() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let expected_cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let backend = Arc::new(GatedSpawnBackend::new(started_tx, release_rx, true));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let app_handle = app.handle().clone();

        let task = {
            let app_handle = app_handle.clone();
            let session_mgr = session_mgr.clone();
            let pty_mgr = pty_mgr.clone();
            let cwd = cwd.clone();
            tokio::spawn(async move {
                super::create_session_inner(
                    &app_handle,
                    &session_mgr,
                    &pty_mgr,
                    "retire-mark-test-command".to_string(),
                    Vec::new(),
                    cwd,
                    Some("retire-mark".to_string()),
                    None,
                    None,
                    true,
                    Vec::new(),
                    true,
                    None,
                    None,
                    None, // #973 - no view in this test: 120x30
                    CreateSelectionIntent::User,
                )
                .await
            })
        };

        started_rx.await.expect("spawn started");
        let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
        assert_eq!(
            pending,
            vec![crate::pty::manager::PendingSpawn {
                cwd: expected_cwd.clone(),
                label: "retire-mark".to_string(),
            }],
            "spawn mark must be live while the failing spawn is in flight"
        );

        let _ = release_tx.send(());
        let err = task
            .await
            .expect("join create_session_inner")
            .expect_err("create_session_inner should fail when the spawn fails");
        assert!(err.contains("synthetic spawn failure"), "{err}");

        let (pending, _) = pty_mgr.lock().unwrap().archive_liveness(&[]);
        assert!(
            pending.is_empty(),
            "spawn mark must retire after a failed spawn"
        );
        assert!(
            session_mgr.read().await.list_sessions().await.is_empty(),
            "rollback must remove the pre-created record on spawn failure"
        );
    }

    #[test]
    fn restart_session_inner_probes_archive_before_destroying_session() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/session.rs"
        ))
        .expect("read session.rs");
        let probe = source
            .find("crate::config::archive_gate::probe_spawn_refusal(app, &cwd).await?;")
            .expect("restart probe call");
        let replacement_spawn = source
            .find("// 3. Spawn and validate the replacement")
            .expect("replacement spawn marker");

        assert!(
            probe < replacement_spawn,
            "restart must probe archive refusal before spawning or destroying a session"
        );
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
            backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
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
            trusted_configured_spawn: false,
            profile_outdated: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
            context_percent: None,
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
                    crate::pty::backend::SessionBackendKind::LocalProcess,
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
    fn pi_explicit_session_control_matches_only_decided_spellings() {
        for token in [
            "-c",
            "-r",
            "--continue",
            "--continue=true",
            "--resume",
            "--resume=id",
            "--session",
            "--session=id",
            "--session-id",
            "--session-id=id",
            "--fork",
            "--fork=id",
            "--no-session",
            "--no-session=true",
        ] {
            assert!(
                pi_has_explicit_session_control(&[token.to_string()]),
                "token={token:?}"
            );
        }
        for token in [
            "-cr",
            "--Continue",
            "--session-dir",
            "--session-dir=custom",
            "--session-directory",
            "--forked",
            "--no-sessions",
        ] {
            assert!(
                !pi_has_explicit_session_control(&[token.to_string()]),
                "token={token:?}"
            );
        }
    }

    #[test]
    fn pi_non_conversation_policy_is_case_sensitive_and_position_aware() {
        for command in ["install", "remove", "uninstall", "update", "list", "config"] {
            assert!(pi_is_non_conversation_invocation(&[command.to_string()]));
            assert!(!pi_is_non_conversation_invocation(&[
                "--print".to_string(),
                command.to_string(),
            ]));
        }
        for flag in [
            "--help",
            "-h",
            "--version",
            "-v",
            "--export",
            "--export=file.jsonl",
            "--list-models",
            "--list-models=json",
        ] {
            assert!(pi_is_non_conversation_invocation(&[
                "--model".to_string(),
                "x".to_string(),
                flag.to_string(),
            ]));
        }
        for eligible in ["Install", "--Help", "--print", "-p", "--json", "--rpc"] {
            assert!(!pi_is_non_conversation_invocation(&[eligible.to_string()]));
        }
    }

    #[test]
    fn inject_pi_resume_handles_direct_tokenized_and_embedded_commands() {
        let mut direct = vec!["--model".to_string(), "claude-sonnet".to_string()];
        assert!(inject_pi_resume("pi", &mut direct));
        assert_eq!(direct, vec!["--continue", "--model", "claude-sonnet"]);

        let mut tokenized = vec![
            "/C".to_string(),
            "pi.cmd".to_string(),
            "--session-dir".to_string(),
            r"C:\Pi State".to_string(),
        ];
        assert!(inject_pi_resume("cmd.exe", &mut tokenized));
        assert_eq!(
            tokenized,
            vec![
                "/C",
                "pi.cmd",
                "--continue",
                "--session-dir",
                r"C:\Pi State"
            ]
        );

        let original =
            r#"  "C:\Program Files\Pi\pi.cmd"  --model "x&&y" x^&y > out %VAR%&&echo  done"#;
        let expected = r#"  "C:\Program Files\Pi\pi.cmd" --continue  --model "x&&y" x^&y > out %VAR%&&echo  done"#;
        let mut embedded = vec!["/K".to_string(), original.to_string()];
        assert!(inject_pi_resume("cmd", &mut embedded));
        assert_eq!(embedded, vec!["/K", expected]);
        assert!(!inject_pi_resume("cmd", &mut embedded));
        assert_eq!(embedded, vec!["/K", expected]);
    }

    #[test]
    fn inject_pi_resume_honors_every_selector_and_decoded_cmd_spelling() {
        for selector in [
            "-c",
            "-r",
            "--continue",
            "--continue=true",
            "--resume",
            "--resume=id",
            "--session",
            "--session=id",
            "--session-id",
            "--session-id=id",
            "--fork",
            "--fork=id",
            "--no-session",
            "--no-session=true",
        ] {
            let mut args = vec![selector.to_string(), "--model".to_string(), "x".to_string()];
            let original = args.clone();
            assert!(!inject_pi_resume("pi", &mut args), "selector={selector:?}");
            assert_eq!(args, original);
        }

        for text in [r#"pi "--resume" --model x"#, "pi --res^ume --model x"] {
            let mut args = vec!["/C".to_string(), text.to_string()];
            let original = args.clone();
            assert!(!inject_pi_resume("cmd.exe", &mut args), "text={text:?}");
            assert_eq!(args, original);
        }
    }

    #[test]
    fn inject_pi_resume_distinguishes_session_dir_and_prefix_names() {
        for initial in [
            vec!["--session-dir".to_string(), "custom".to_string()],
            vec!["--session-dir=custom".to_string()],
            vec!["--session-directory".to_string()],
        ] {
            let mut args = initial.clone();
            assert!(inject_pi_resume("pi", &mut args));
            assert_eq!(args.first().map(String::as_str), Some("--continue"));
            assert_eq!(&args[1..], initial.as_slice());
        }

        let mut explicit = vec!["--session".to_string(), "id".to_string()];
        let original = explicit.clone();
        assert!(!inject_pi_resume("pi", &mut explicit));
        assert_eq!(explicit, original);
    }

    #[test]
    fn inject_pi_resume_preserves_non_conversation_invocations() {
        for command in ["install", "remove", "uninstall", "update", "list", "config"] {
            let mut args = vec![command.to_string(), "package".to_string()];
            let original = args.clone();
            assert!(!inject_pi_resume("pi", &mut args), "command={command:?}");
            assert_eq!(args, original);
        }
        for flag in [
            "--help",
            "-h",
            "--version",
            "-v",
            "--export",
            "--export=file",
            "--list-models",
            "--list-models=json",
        ] {
            let mut args = vec!["--model".to_string(), "x".to_string(), flag.to_string()];
            let original = args.clone();
            assert!(!inject_pi_resume("pi", &mut args), "flag={flag:?}");
            assert_eq!(args, original);
        }

        for conversational in ["--print", "-p", "--json", "--rpc", "Install", "--Help"] {
            let mut args = vec![conversational.to_string()];
            assert!(inject_pi_resume("pi", &mut args));
            assert_eq!(args.first().map(String::as_str), Some("--continue"));
        }
    }

    #[test]
    fn inject_pi_resume_inspects_only_the_first_cmd_segment() {
        let original = "pi --model x&&echo --resume";
        let expected = "pi --continue --model x&&echo --resume";
        let mut args = vec!["/C".to_string(), original.to_string()];
        assert!(inject_pi_resume("cmd.exe", &mut args));
        assert_eq!(args, vec!["/C", expected]);
    }

    #[test]
    fn inject_pi_resume_leaves_malformed_unsupported_and_non_pi_inputs_unchanged() {
        let cases = [
            ("cmd.exe", vec!["/C".to_string(), "pi>out".to_string()]),
            (
                "cmd.exe",
                vec!["/C".to_string(), "pi \"unterminated".to_string()],
            ),
            (
                "cmd.exe",
                vec!["/C".to_string(), "npx".to_string(), "pi".to_string()],
            ),
            ("powershell.exe", vec!["pi".to_string()]),
        ];
        for (shell, mut args) in cases {
            let original = args.clone();
            assert!(!inject_pi_resume(shell, &mut args), "shell={shell:?}");
            assert_eq!(args, original);
        }
    }

    #[test]
    fn maybe_inject_pi_resume_requires_kind_trusted_spawn_and_known_state() {
        let spawn = inert_pi_spawn();
        let base = vec!["--model".to_string(), "claude-sonnet".to_string()];

        let mut eligible = base.clone();
        assert!(maybe_inject_pi_resume(
            Some(CodingAgentKind::Pi),
            Some(&spawn),
            false,
            "pi",
            &mut eligible,
        ));
        assert_eq!(eligible, vec!["--continue", "--model", "claude-sonnet"]);

        for (kind, trusted, fresh) in [
            (Some(CodingAgentKind::Pi), None, false),
            (Some(CodingAgentKind::Pi), Some(&spawn), true),
            (Some(CodingAgentKind::Claude), Some(&spawn), false),
            (None, Some(&spawn), false),
        ] {
            let mut args = base.clone();
            assert!(!maybe_inject_pi_resume(
                kind, trusted, fresh, "pi", &mut args,
            ));
            assert_eq!(args, base);
        }
    }

    #[test]
    fn pi_tuned_profile_flag_stays_false_while_direct_shell_capability_is_separate() {
        let mut settings = AppSettings::default();
        settings
            .auto_self_clear_by_agent
            .insert("dev-rust".to_string(), true);
        let requested =
            crate::config::settings::resolve_auto_self_clear(&settings, "dev-rust", false);
        assert!(requested);
        // The tuned profile retains #1069's value. Launch gating uses #1059's
        // independent exact-stem direct-shell capability instead.
        assert!(!CodingAgentKind::Pi.profile().auto_self_clear_supported && requested);
    }

    #[test]
    fn heuristic_agent_metadata_cannot_authorize_pi_mutation() {
        let settings = AppSettings {
            agents: vec![AgentConfig {
                id: "unrelated-cmd".to_string(),
                label: "Unrelated cmd recipe".to_string(),
                command: "cmd".to_string(),
                color: "#000000".to_string(),
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                backend: Default::default(),
            }],
            ..AppSettings::default()
        };
        let configured_args = vec!["/C".to_string(), "pi".to_string()];
        let (heuristic_id, _) =
            resolve_actual_agent("cmd", &configured_args, None, None, &settings);
        assert_eq!(heuristic_id.as_deref(), Some("unrelated-cmd"));

        let mut args = configured_args;
        assert!(!maybe_inject_pi_resume(
            Some(CodingAgentKind::Pi),
            None,
            false,
            "cmd",
            &mut args,
        ));
        assert_eq!(args, vec!["/C", "pi"]);
    }

    #[test]
    fn inject_antigravity_resume_prefixes_direct_agy_args() {
        let mut args = vec!["-m".to_string(), "gpt-5".to_string()];
        assert!(super::inject_antigravity_resume("agy", &mut args));
        assert_eq!(
            args,
            vec![
                "--continue".to_string(),
                "-m".to_string(),
                "gpt-5".to_string()
            ]
        );

        let mut args = vec!["-m".to_string(), "gpt-5".to_string()];
        assert!(super::inject_antigravity_resume("antigravity", &mut args));
        assert_eq!(
            args,
            vec![
                "--continue".to_string(),
                "-m".to_string(),
                "gpt-5".to_string()
            ]
        );
    }

    #[test]
    fn inject_antigravity_resume_inserts_into_cmd_tokenized_wrapper() {
        let mut args = vec![
            "/C".to_string(),
            "agy".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ];
        assert!(super::inject_antigravity_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec![
                "/C".to_string(),
                "agy".to_string(),
                "--continue".to_string(),
                "-m".to_string(),
                "gpt-5".to_string()
            ]
        );
    }

    #[test]
    fn inject_antigravity_resume_inserts_into_embedded_cmd_string() {
        let mut args = vec!["/K".to_string(), "git pull && agy -m gpt-5".to_string()];
        assert!(super::inject_antigravity_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec![
                "/K".to_string(),
                "git pull && agy --continue -m gpt-5".to_string()
            ]
        );
    }

    #[test]
    fn inject_antigravity_resume_skips_when_continue_or_conversation_present() {
        for skip_args in [
            vec!["--continue".to_string(), "gpt-5".to_string()],
            vec!["-c".to_string(), "gpt-5".to_string()],
            vec!["--conversation".to_string(), "abc123".to_string()],
            vec!["--conversation=abc123".to_string()],
            vec!["--CONTINUE".to_string(), "gpt-5".to_string()],
            vec!["--Conversation".to_string(), "abc123".to_string()],
        ] {
            let mut args = skip_args.clone();
            assert!(
                !super::inject_antigravity_resume("agy", &mut args),
                "args={skip_args:?}"
            );
            assert_eq!(args, skip_args);
        }

        // Tokenized cmd form also skips on a resume marker after the executable.
        let mut args = vec![
            "/C".to_string(),
            "agy".to_string(),
            "--conversation".to_string(),
            "abc123".to_string(),
        ];
        assert!(!super::inject_antigravity_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec![
                "/C".to_string(),
                "agy".to_string(),
                "--conversation".to_string(),
                "abc123".to_string()
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
            context_regex: None,
            backend: Default::default(),
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
            context_regex: None,
            backend: Default::default(),
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
        // to opt in to provider auto-resume; otherwise antigravity/codex/claude
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

    // ── (#747) carry_communication_for_restart: shared decision for the
    //    Branch-A reopen (restart_session_inner_with_activation) AND the
    //    startup wake arm (lib.rs restore loop), which reuses this helper with
    //    the persisted intent. This matrix pins the fresh gate for BOTH. ──

    #[test]
    fn carry_communication_for_restart_matrix() {
        let visible_hand = || {
            Some(crate::session::session::SessionCommunication {
                kind: crate::session::session::SessionCommunicationKind::RaiseHand,
                visible: true,
                updated_at: "2026-06-30T11:00:00+00:00".to_string(),
            })
        };
        let hidden_hand = || {
            Some(crate::session::session::SessionCommunication {
                kind: crate::session::session::SessionCommunicationKind::RaiseHand,
                visible: false,
                updated_at: "2026-06-30T11:00:00+00:00".to_string(),
            })
        };

        // (visible raise, fresh=false): the reopen resumes the conversation,
        // so the pending user-attention marker carries.
        let carried = super::carry_communication_for_restart(visible_hand(), false)
            .expect("visible hand must carry on a resuming reopen");
        assert!(carried.visible);
        assert_eq!(carried.updated_at, "2026-06-30T11:00:00+00:00");

        // (visible raise, fresh=true): a fresh restart abandons the old
        // conversation; the raise belonged to it and drops.
        assert!(super::carry_communication_for_restart(visible_hand(), true).is_none());

        // (hidden raise, fresh=false): non-visible payloads never carry.
        assert!(super::carry_communication_for_restart(hidden_hand(), false).is_none());

        // (None, fresh=false): nothing to carry.
        assert!(super::carry_communication_for_restart(None, false).is_none());
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

    #[test]
    fn local_resume_probe_env_layer_uses_config_dir_override() {
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-env");
        let cwd = "C:\\Users\\Test\\repo";

        let resolved =
            super::claude_projects_dir_for_config_dir(custom_base.to_str().unwrap(), cwd);

        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd));
        assert_eq!(resolved, expected);
    }

    #[test]
    fn local_resume_probe_env_layer_expands_powershell_envvar() {
        let var = "AC_TEST_894_CONFIG_DIR";
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-env");
        std::env::set_var(var, custom_base.to_str().unwrap());

        let resolved =
            super::claude_projects_dir_for_config_dir(&format!("$env:{}\\.claude", var), "C:\\x");

        std::env::remove_var(var);
        let expected = PathBuf::from(format!("{}\\.claude", custom_base.to_string_lossy()))
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

    // ── (#756) mirror_forces_fresh truth table ──

    #[test]
    fn mirror_forces_fresh_truth_table() {
        // Forces fresh ONLY when: resume requested + coordinator + mirror pending.
        assert!(super::mirror_forces_fresh(false, true, true));
        // Any other combination must not force.
        assert!(!super::mirror_forces_fresh(false, true, false));
        assert!(!super::mirror_forces_fresh(false, false, true));
        assert!(!super::mirror_forces_fresh(false, false, false));
        assert!(!super::mirror_forces_fresh(true, true, true));
        assert!(!super::mirror_forces_fresh(true, true, false));
        assert!(!super::mirror_forces_fresh(true, false, true));
        assert!(!super::mirror_forces_fresh(true, false, false));
    }

    // ── (#756) should_inject_fresh_session_id decision table ──

    #[test]
    fn should_inject_fresh_session_id_true_for_claude_skip_clean_argv() {
        assert!(super::should_inject_fresh_session_id(
            true,
            true,
            "claude --dangerously-skip-permissions"
        ));
    }

    #[test]
    fn should_inject_fresh_session_id_false_when_not_skipping_resume() {
        assert!(!super::should_inject_fresh_session_id(
            true,
            false,
            "claude --dangerously-skip-permissions"
        ));
    }

    #[test]
    fn should_inject_fresh_session_id_false_when_not_claude() {
        assert!(!super::should_inject_fresh_session_id(
            false,
            true,
            "codex --dangerously-bypass-approvals-and-sandbox"
        ));
    }

    #[test]
    fn should_inject_fresh_session_id_false_for_each_identity_veto_token() {
        // User-configured identity flags win; the rider must never stack onto
        // them (stacking is a hard CLI error, Q2-verified).
        for cmd in [
            "claude --session-id 7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f",
            "claude --session-id=7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f",
            "claude --resume",
            "claude --resume=abc",
            "claude -r",
            "claude --continue",
            "claude --continue=abc",
            "claude -c",
            "claude --fork-session",
            "claude --SESSION-ID x", // case-insensitive token match
            "claude --Continue",
        ] {
            assert!(
                !super::should_inject_fresh_session_id(true, true, cmd),
                "identity flag in {:?} must veto the rider",
                cmd
            );
        }
    }

    #[test]
    fn should_inject_fresh_session_id_true_when_unrelated_flag_substring() {
        // Token-equality fence: `--session-identity` is NOT `--session-id`,
        // `--recover` is NOT `-r`.
        assert!(super::should_inject_fresh_session_id(
            true,
            true,
            "claude --session-identity thing --recover"
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

    // ──────────────────────────────────────────────────────────────────────
    // §1295 gate tests (13-16)
    // ──────────────────────────────────────────────────────────────────────

    /// Test 13 (dispatch c): the impl creation gate blocks an unregistered cwd
    /// (zero residue) and accepts a registered existing root when forced with
    /// `Enforce`.
    #[tokio::test]
    async fn create_session_inner_gate_blocks_unregistered_cwd() {
        use crate::config::sessions_persistence::CreationGateEnforcement;

        // Use the strict-replica fixture so `pty_input_create_gate_key_from_cwd`
        // classifies the cwd as a known replica (returns None, not Err) rather
        // than erroring on a bare tempdir path.

        // Outside-roots cwd (empty project_paths) with Enforce -> rejected, no
        // row, no spawn.
        let (temp, first_cwd, _second) = strict_target_fixture();
        let outside_cwd = first_cwd;
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            AppSettings::default(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let err = super::create_session_inner_impl(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            outside_cwd,
            Some("gated".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
            None,
            None,
            None,
            None,
            false,
            None,
            CreationGateEnforcement::Enforce,
        )
        .await;
        let err = match err {
            Ok(_) => panic!("expected the gate to reject the unregistered cwd"),
            Err(e) => e,
        };
        assert!(err.contains("sessionCreateBlocked"), "{err}");
        assert!(err.contains("outside all registered projects"), "{err}");
        assert!(
            session_mgr.read().await.list_sessions().await.is_empty(),
            "rejected create leaves zero residue"
        );
        assert_eq!(
            backend.spawn_count.load(Ordering::SeqCst),
            0,
            "rejected create leaves zero residue"
        );
        close_test_coordinator(&app).await;

        // Registered existing root with Enforce -> Ok and a row is created.
        let project_root = temp.path().join("project");
        let reg_cwd = temp
            .path()
            .join("project")
            .join(".ac")
            .join("wg-1-team")
            .join("__agent_dev-one")
            .to_string_lossy()
            .to_string();
        std::fs::create_dir_all(&reg_cwd).unwrap();
        let settings = AppSettings {
            project_paths: vec![project_root.to_string_lossy().to_string()],
            ..AppSettings::default()
        };
        let session_mgr2 = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend2 = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr2 = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend2.clone(),
        )));
        let app2 = session_test_app(settings, Arc::clone(&session_mgr2), Arc::clone(&pty_mgr2));
        let reg_cwd = reg_cwd.to_string();
        let outcome = super::create_session_inner_impl(
            app2.handle(),
            &session_mgr2,
            &pty_mgr2,
            "codex".to_string(),
            Vec::new(),
            reg_cwd,
            Some("gated-ok".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
            None,
            None,
            None,
            None,
            false,
            None,
            CreationGateEnforcement::Enforce,
        )
        .await;
        assert!(
            outcome.is_ok(),
            "registered existing root must pass the gate"
        );
        assert_eq!(
            session_mgr2.read().await.list_sessions().await.len(),
            1,
            "row created"
        );
        close_test_coordinator(&app2).await;
    }

    /// Test 14 (B2/N4b): restart of a LIVE outside-roots session DECLINES before
    /// teardown when forced with `Enforce`: the row survives with its original
    /// status, the PTY handle count is unchanged, and no replacement spawns.
    #[tokio::test]
    async fn restart_of_live_outside_roots_declines_before_teardown() {
        use crate::config::sessions_persistence::CreationGateEnforcement;

        let temp = tempfile::tempdir().unwrap();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            test_settings(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        // Create a LIVE session at an unregistered cwd (gate Skip in the test
        // build lets the create through).
        let old =
            create_scripted_session(&app, &session_mgr, &pty_mgr, &temp.path().to_string_lossy())
                .await;
        let old_id = Uuid::parse_str(&old.id).unwrap();
        assert!(backend.has_session(old_id), "old PTY is live");
        let settings = app.state::<crate::config::settings::SettingsState>();

        let err = super::restart_session_inner_with_intent(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            settings.inner(),
            old_id,
            None,
            None,
            Some(true),
            true,
            crate::session::selection::TrustedRestartIntent::User,
            None,
            CreationGateEnforcement::Enforce,
        )
        .await
        .unwrap_err();
        assert!(err.contains("sessionCreateBlocked"), "{err}");

        // No teardown side effects: row survives with its original (non-Exited)
        // status, PTY untouched, no replacement spawn.
        let row = session_mgr
            .read()
            .await
            .get_session(old_id)
            .await
            .expect("row survives");
        assert!(
            !matches!(
                row.status,
                crate::session::session::SessionStatus::Exited(_)
            ),
            "declare must not flip the row to Exited"
        );
        assert!(backend.has_session(old_id), "PTY handle not torn down");
        assert_eq!(
            backend.spawn_count.load(Ordering::SeqCst),
            1,
            "no replacement spawn"
        );
        close_test_coordinator(&app).await;
    }

    /// Test 15 (N4d): the restore-wake gate exempts root-agent and archived-root
    /// cwds (existence waived) and refuses an outside-roots cwd, all through the
    /// same `enforce_creation_gate` the restore path runs.
    #[tokio::test]
    async fn restore_wake_gate_exceptions() {
        use crate::config::sessions_persistence::{enforce_creation_gate, CreationGateEnforcement};
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));

        // Root-agent cwd: allowed even when the dir is missing.
        let app = session_test_app(
            AppSettings::default(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        let root = crate::config::root_agent::root_agent_dir().expect("root dir resolves");
        assert!(
            enforce_creation_gate(app.handle(), &root, CreationGateEnforcement::Enforce)
                .await
                .is_ok(),
            "root-agent cwd is gate-exempt"
        );
        close_test_coordinator(&app).await;

        // Archived-root cwd: allowed with existence waived.
        let archived_dir = tempfile::tempdir().unwrap();
        let archived_root = archived_dir.path().to_string_lossy().to_string();
        let missing_under_archived = archived_dir
            .path()
            .join("not-there")
            .join("__agent_x")
            .to_string_lossy()
            .to_string();
        let session_mgr2 = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let app2 = session_test_app(
            AppSettings {
                archived_project_paths: vec![archived_root.clone()],
                ..AppSettings::default()
            },
            Arc::clone(&session_mgr2),
            Arc::clone(&pty_mgr),
        );
        assert!(
            enforce_creation_gate(
                app2.handle(),
                &missing_under_archived,
                CreationGateEnforcement::Enforce
            )
            .await
            .is_ok(),
            "archived-root restore-wake is gate-exempt"
        );
        close_test_coordinator(&app2).await;

        // Outside-roots cwd: refused.
        let session_mgr3 = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let app3 = session_test_app(
            AppSettings::default(),
            Arc::clone(&session_mgr3),
            Arc::clone(&pty_mgr),
        );
        let outside = archived_dir
            .path()
            .join("elsewhere")
            .to_string_lossy()
            .to_string();
        let err = enforce_creation_gate(app3.handle(), &outside, CreationGateEnforcement::Enforce)
            .await
            .unwrap_err();
        assert!(err.contains("sessionCreateBlocked"), "{err}");
        close_test_coordinator(&app3).await;
    }

    /// Test 16 (S5/N4): the Tauri `create_session` command with `cwd: None`
    /// resolves to the home dir and (in the test build, where the gate defaults
    /// to Skip) creates Ok. The production refusal of that default is covered by
    /// the pure-function predicate tests in sessions_persistence.
    #[tokio::test]
    async fn create_session_command_without_cwd_falls_back_to_home() {
        // §1295 S5: the command resolves cwd:None to the home dir (gate Skip in
        // the test build lets the inner create through). The production refusal
        // of the home-dir default is covered by the pure predicate test in
        // sessions_persistence.
        let resolved = super::resolve_create_session_cwd(None);
        let expected_home = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if !expected_home.is_empty() {
            assert_eq!(
                resolved,
                crate::path_utils::normalize_windows_verbatim_path(&expected_home),
                "cwd: None falls back to home"
            );
        }

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(
            AppSettings::default(),
            Arc::clone(&session_mgr),
            Arc::clone(&pty_mgr),
        );
        // The inner create (gate defaults to Skip in the test build) succeeds at
        // the home cwd, mirroring the `cwd: None` command path.
        super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "codex".to_string(),
            Vec::new(),
            resolved,
            Some("home-default".to_string()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .expect("create with home default succeeds in the test build");
        assert_eq!(session_mgr.read().await.list_sessions().await.len(), 1);
        close_test_coordinator(&app).await;
    }

    // --- #1271 configured default-shell propagation --------------------------

    /// Captures every `BackendSpawnSpec` the PTY manager hands to the backend,
    /// so tests can assert the resolved-agent host-shell snapshot propagated
    /// unchanged from the creation seam. Spawned ids count as live sessions so
    /// the finalized-create path can display them, mirroring
    /// `ScriptedSpawnBackend`.
    #[derive(Default)]
    struct CapturingSpawnBackend {
        specs: Mutex<Vec<crate::pty::backend::BackendSpawnSpec>>,
        live: Mutex<HashSet<Uuid>>,
    }

    impl PtyBackend for CapturingSpawnBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async move {
                self.live.lock().unwrap().insert(spec.id);
                self.specs.lock().unwrap().push(spec);
                Ok(())
            })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            id: Uuid,
            _data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            Err(crate::errors::AppError::SessionNotFound(id.to_string()))
        }

        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
            self.live.lock().unwrap().remove(&id);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    #[tokio::test]
    async fn configured_default_shell_snapshot_reaches_backend_for_resolved_agent() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let mut settings = test_settings();
        settings.default_shell =
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string();
        settings.default_shell_args = vec!["-NoProfile".to_string()];

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(CapturingSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

        // Drive the resolved-agent path exactly as `create_session` does: the
        // resolved agent stays the command-to-run, and the configured host shell
        // (copied from the same config snapshot) travels separately.
        let spawn = super::build_configured_agent_spawn_for_cwd(
            &test_settings(),
            "claude",
            &cwd,
            None,
        )
        .expect("resolve claude")
        .expect("claude is configured in test_settings");
        let host_shell = crate::pty::backend::ResolvedAgentHostShell {
            program: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string(),
            args: vec!["-NoProfile".to_string()],
        };
        let created = super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            cwd.clone(),
            Some("1271 propagation".to_string()),
            Some(spawn.trusted_agent_id.clone()),
            Some(spawn.trusted_agent_label.clone()),
            false,
            Vec::new(),
            true,
            Some(spawn),
            Some(host_shell),
            None,
            CreateSelectionIntent::User,
        )
        .await
        .expect("resolved-agent create succeeds");
        assert_eq!(created.agent_id.as_deref(), Some("claude"));

        {
            let specs = backend.specs.lock().unwrap();
            let spec = specs
                .last()
                .expect("the resolved-agent create reached the backend");
            assert_eq!(spec.cmd, "claude", "agent program stays the command-to-run");
            let host = spec
                .resolved_agent_host_shell
                .as_ref()
                .expect("host shell snapshot must reach the backend");
            assert_eq!(
                host.program,
                "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
            );
            assert_eq!(host.args, vec!["-NoProfile".to_string()]);
        }
        close_test_coordinator(&app).await;
    }

    /// #1271 - the observable session metadata seam (`set_pending_effective_shell_args`,
    /// surfaced as `effective_shell_args`) keeps the LOGICAL agent argv; the
    /// configured host-shell arguments travel only in the backend spec's
    /// `resolved_agent_host_shell`. The two must remain distinct: host-shell args
    /// are never written into the metadata channel.
    #[tokio::test]
    async fn configured_default_shell_metadata_keeps_logical_agent_argv_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let mut settings = test_settings();
        settings.default_shell =
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string();
        settings.default_shell_args = vec!["-NoProfile".to_string()];

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(CapturingSpawnBackend::default());
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

        let spawn = super::build_configured_agent_spawn_for_cwd(
            &test_settings(),
            "claude",
            &cwd,
            None,
        )
        .expect("resolve claude")
        .expect("claude is configured in test_settings");
        let host_shell = crate::pty::backend::ResolvedAgentHostShell {
            program: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string(),
            args: vec!["-NoProfile".to_string()],
        };
        let created = super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            cwd.clone(),
            Some("1271 metadata".to_string()),
            Some(spawn.trusted_agent_id.clone()),
            Some(spawn.trusted_agent_label.clone()),
            false,
            Vec::new(),
            true,
            Some(spawn),
            Some(host_shell),
            None,
            CreateSelectionIntent::User,
        )
        .await
        .expect("resolved-agent create succeeds");

        // The claude test agent gets auto-injected logical args (`--session-id`):
        // metadata must carry exactly that logical argv, never the configured
        // host-shell args.
        let effective = created
            .effective_shell_args
            .as_deref()
            .expect("effective args are captured before spawn");
        assert!(
            effective.iter().any(|arg| arg.starts_with("--session-id")),
            "session metadata carries the logical agent argv: {effective:?}"
        );
        assert!(
            !effective.iter().any(|arg| arg == "-NoProfile"),
            "host-shell args must never leak into session metadata: {effective:?}"
        );
        let session = session_mgr
            .read()
            .await
            .get_session(Uuid::parse_str(&created.id).unwrap())
            .await
            .expect("session exists");
        let stored = session
            .effective_shell_args
            .as_deref()
            .expect("stored effective args");
        assert!(
            !stored.iter().any(|arg| arg == "-NoProfile"),
            "the pending-effective metadata channel stays logical: {stored:?}"
        );

        {
            let specs = backend.specs.lock().unwrap();
            let spec = specs.last().expect("create reached the backend");
            assert_eq!(spec.args, effective, "backend args stay the logical agent argv");
            assert_eq!(
                spec.resolved_agent_host_shell
                    .as_ref()
                    .expect("host shell present")
                    .args,
                vec!["-NoProfile".to_string()],
                "host-shell args travel only in the paired snapshot"
            );
        }
        close_test_coordinator(&app).await;
    }

    /// #1271 - native twin of the web invalid-input postcondition test: the
    /// adapter rejection happens at the TOP of the REAL `spawn_sync`, before
    /// any spawn accounting or PTY acquisition, so a rejected configured host
    /// leaves no session, no pending/metadata record, no spawn record, no PTY
    /// map entry, and no output task. Windows-only (the adapter is the Windows
    /// host-shell branch); the backend seam proves the same postcondition with
    /// a known id in `adapter_spawn_sync_tests`.
    #[cfg(windows)]
    #[tokio::test]
    async fn configured_default_shell_invalid_input_leaves_no_session_state_native() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let mut settings = test_settings();
        // A conflicting/terminal configured option must fail before any PTY.
        settings.default_shell = "powershell.exe".to_string();
        settings.default_shell_args = vec!["-Command".to_string()];

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        // The real local backend: the invalid-input rejection lives inside its
        // spawn_sync, so a fake capturing backend would accept the spawn.
        let git_app = Box::leak(Box::new(
            crate::test_support::test_builder()
                .build(tauri::test::mock_context(tauri::test::noop_assets()))
                .expect("build native invalid-input git watcher app"),
        ));
        let git_watcher = crate::pty::git_watcher::GitWatcher::new(
            Arc::clone(&session_mgr),
            git_app.handle().clone(),
        );
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let output_senders: crate::telegram::manager::OutputSenderMap =
            Arc::new(Mutex::new(HashMap::new()));
        let backend = Arc::new(crate::pty::local_backend::LocalProcessBackend::new(
            output_senders,
            idle_detector,
            git_watcher,
            None,
        ));
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend,
        )));
        let app = session_test_app(settings, Arc::clone(&session_mgr), Arc::clone(&pty_mgr));

        let error = super::create_session_inner(
            app.handle(),
            &session_mgr,
            &pty_mgr,
            "claude".to_string(),
            Vec::new(),
            cwd.clone(),
            Some("1271 native invalid input".to_string()),
            None,
            None,
            false,
            Vec::new(),
            true,
            None,
            Some(crate::pty::backend::ResolvedAgentHostShell {
                program: "powershell.exe".to_string(),
                args: vec!["-Command".to_string()],
            }),
            None,
            CreateSelectionIntent::User,
        )
        .await
        .expect_err("invalid configured host must fail the create");
        assert!(error.contains("conflicting/terminal"), "{error}");
        assert!(error.contains("agent adapter owns command execution"), "{error}");

        assert!(
            session_mgr.read().await.list_sessions().await.is_empty(),
            "no session may appear in the session manager"
        );
        // The coordinator worker rolls the pending binding back asynchronously;
        // while it is still visible, the rejected id must show no launch
        // provenance, no PTY map entry, and no output task.
        let pending_ids = session_mgr
            .read()
            .await
            .aggregate_snapshot()
            .await
            .pending_ids;
        for id in &pending_ids {
            assert!(
                crate::pty::spawn_diagnostics::record_for(*id).is_none(),
                "no launch provenance may be recorded for the rejected session"
            );
            assert!(
                !pty_mgr.lock().unwrap().has_session(*id),
                "no PTY map entry may exist for the rejected session"
            );
            assert!(
                pty_mgr.lock().unwrap().get_pty_size(*id).is_none(),
                "no output task may be attached for the rejected session"
            );
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let pending_empty = session_mgr
                .read()
                .await
                .aggregate_snapshot()
                .await
                .pending_ids
                .is_empty();
            if pending_empty {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no pending session/metadata record may survive the rejection"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        close_test_coordinator(&app).await;
    }

}
