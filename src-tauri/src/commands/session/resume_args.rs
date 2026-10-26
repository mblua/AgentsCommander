use super::agent_command::executable_basename;
use crate::config::agent_command::AgentSpawnCommand;
use crate::pty::backend::SessionBackendKind;
use crate::session::profile::{locate_pi_command, CodingAgentKind, PiInsertionPoint};

pub(crate) fn token_has_unclosed_quote(token: &str, quote: char) -> bool {
    token.chars().filter(|c| *c == quote).count() % 2 == 1
}

pub(crate) fn advance_past_config_value(tokens: &[&str], start: usize) -> usize {
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

pub(crate) fn codex_option_takes_value(token: &str) -> bool {
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

pub(crate) fn codex_has_explicit_subcommand(tokens: &[&str], start: usize) -> bool {
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

pub(crate) fn codex_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
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

pub(crate) fn antigravity_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
    tokens[start..].iter().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue"
            || lower == "-c"
            || lower == "--conversation"
            || lower.starts_with("--conversation=")
    })
}

pub(crate) fn inject_antigravity_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    // #260/#1482 — resume token sourced from the CodingAgentProfile (single
    // source of truth). G6: slice-pattern destructure, never index — a future
    // <1-element slice degrades gracefully instead of panicking in release.
    let &[resume_token] = CodingAgentKind::Antigravity.profile().resume_tokens else {
        debug_assert!(
            false,
            "Antigravity resume_tokens must have exactly 1 element"
        );
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
            let is_antigravity_executable =
                |arg: &str| matches!(executable_basename(arg).as_str(), "agy" | "antigravity");
            if let Some(idx) = shell_args
                .iter()
                .position(|arg| is_antigravity_executable(arg))
            {
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
                if let Some(idx) = tokens
                    .iter()
                    .position(|token| is_antigravity_executable(token))
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

pub(crate) fn inject_codex_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
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

pub(crate) fn pi_has_explicit_session_control(option_tokens: &[String]) -> bool {
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

pub(crate) fn pi_is_non_conversation_invocation(option_tokens: &[String]) -> bool {
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

pub(crate) fn inject_pi_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
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

pub(crate) fn maybe_inject_pi_resume(
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

/// #1873 - pure eligibility predicate for Muse workspace-latest auto-resume.
/// True only when EVERY condition holds:
/// - a resolved `AgentSpawnCommand` is present (configured-spawn provenance);
/// - the actual `shell`/`shell_args` equal that resolved recipe exactly;
/// - the resolved backend is `LocalProcess`;
/// - the compiled host is macOS or Linux;
/// - `Path::file_name` of the direct shell is the exact, case-sensitive `muse`;
/// - the configured argument vector is EMPTY.
///
/// Empty argv is the precedence rule: any configured argument (resume, --last,
/// UUID, prompt, --no-session-log, --workspace/--root, help/version, or a future
/// flag) wins and suppresses AC injection; AC neither validates nor rewrites it.
/// Ad-hoc launches, prefix names, wrappers, recipe mismatches, Windows, and
/// containers are all `false`. Shared with `loops::delivery` (an existing module
/// arc) so the cold-loop fresh rule and the launch seam agree.
pub(crate) fn trusted_muse_auto_resume_spawn(
    resolved_spawn: Option<&AgentSpawnCommand>,
    shell: &str,
    shell_args: &[String],
) -> bool {
    let Some(spawn) = resolved_spawn else {
        return false;
    };
    if spawn.shell != shell || spawn.shell_args != shell_args {
        return false;
    }
    if SessionBackendKind::from(&spawn.backend) != SessionBackendKind::LocalProcess {
        return false;
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        return false;
    }
    if std::path::Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        != Some("muse")
    {
        return false;
    }
    shell_args.is_empty()
}

/// #1873 - append the Muse profile's resume tokens to the LOCAL effective vector
/// only. Requires detected Muse, resume intent (`skip_auto_resume == false`) and
/// the trusted predicate above. Slice-destructures the profile tokens: a wrong
/// arity, or a second call (the vector is no longer empty), fails closed with no
/// mutation. `Session.shell_args` (configured) is never touched.
pub(crate) fn maybe_inject_muse_resume(
    agent_kind: Option<CodingAgentKind>,
    resolved_spawn: Option<&AgentSpawnCommand>,
    skip_auto_resume: bool,
    shell: &str,
    shell_args: &mut Vec<String>,
) -> bool {
    if agent_kind != Some(CodingAgentKind::Muse) || skip_auto_resume {
        return false;
    }
    if !trusted_muse_auto_resume_spawn(resolved_spawn, shell, shell_args) {
        return false;
    }
    let &[resume_subcmd, resume_flag] = CodingAgentKind::Muse.profile().resume_tokens else {
        debug_assert!(false, "Muse resume_tokens must have exactly 2 elements");
        return false;
    };
    shell_args.push(resume_subcmd.to_string());
    shell_args.push(resume_flag.to_string());
    true
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
pub(crate) fn expand_env_var_refs(input: &str) -> String {
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
