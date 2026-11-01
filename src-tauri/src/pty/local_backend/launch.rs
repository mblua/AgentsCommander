use portable_pty::CommandBuilder;

use crate::errors::AppError;
use crate::pty::backend::ResolvedAgentHostShell;

// ---------------------------------------------------------------------------
// #1271 - Windows host-shell adapter for resolved agent commands.
//
// Four supported protocol classes, selected from the lower-cased final path
// component of the configured default-shell program: Windows PowerShell
// (`powershell`/`powershell.exe`), PowerShell 7+ (`pwsh`/`pwsh.exe`), Command
// Prompt (`cmd`/`cmd.exe`), and any other basename as an explicitly declared
// POSIX-compatible custom shell. The exact configured program string is ALWAYS
// the launched program; a hard-coded `cmd.exe` is never substituted.
//
// PowerShell uses a managed `ProcessStartInfo` child instead of native `&` or
// `--%` because ordinary native invocation can alter empty values and embedded
// quotes, and `--%` is a physical-line parser hazard that would destroy the
// logical-argv boundary. The generated script resolves the agent as an external
// command only (never an alias/function/cmdlet) and starts a native
// `Application` with a standard Windows-encoded `Arguments` property; a
// resolved `.cmd`/`.bat` target runs through one explicit system-cmd child
// (`/D /V:OFF /S /C`) because a batch file is not a native argv consumer.
//
// cmd runs with delayed expansion disabled (`/V:OFF`) so `!` is literal data.
// All validation happens before any PTY is acquired; the configured host's own
// parser never sees a token the closed grammar (Section 4.3) did not classify.
// ---------------------------------------------------------------------------

/// One adapted launch: the exact program and argument vector handed to
/// `CommandBuilder`, which is also the `exec_argv` diagnostic provenance. Built
/// from ONE adapter decision so the two representations can never desynchronize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedLaunch {
    pub(super) program: String,
    pub(super) args: Vec<String>,
}

impl PreparedLaunch {
    pub(super) fn command_builder(&self) -> CommandBuilder {
        let mut command = CommandBuilder::new(&self.program);
        for arg in &self.args {
            command.arg(arg);
        }
        command
    }

    pub(super) fn exec_argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }
}

/// The four #1271 Windows host-shell protocol classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowsHostShellKind {
    PowerShell,
    Pwsh,
    Cmd,
    Posix,
}

/// True only for a command whose text ends in `.exe` or whose path extension
/// equals `.exe`, case-insensitively. Direct `.exe` agents never enter the
/// adapter; everything else on Windows is a candidate.
pub(super) fn is_direct_exe(command: &str) -> bool {
    command.to_lowercase().ends_with(".exe")
        || std::path::Path::new(command)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

/// Selects the adapter protocol from the lower-cased final Windows path
/// component of the configured program, accepting both extensionless and `.exe`
/// spellings. A custom path ending in a recognized name uses that adapter.
pub(super) fn configured_shell_kind(program: &str) -> WindowsHostShellKind {
    let basename = program
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    match basename.as_str() {
        "powershell" | "powershell.exe" => WindowsHostShellKind::PowerShell,
        "pwsh" | "pwsh.exe" => WindowsHostShellKind::Pwsh,
        "cmd" | "cmd.exe" => WindowsHostShellKind::Cmd,
        _ => WindowsHostShellKind::Posix,
    }
}

/// #1498 - Windows-only: `git-bash.exe` (the Git for Windows MinTTY launcher)
/// and `mintty.exe` detach from the console, so a ConPTY host would acquire
/// resources for a child that never attaches to them (blank PTY / detached
/// window). Rejected before any resource acquisition, naming the program and
/// the working alternative. Deliberately does NOT reuse `adapter_error`'s
/// resolved-agent phrasing: this rejection also applies to bare terminals and
/// direct launches, not only to resolved-agent sessions.
pub(super) fn reject_detached_launcher(program: &str) -> Result<(), AppError> {
    let basename = program
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    if basename == "git-bash.exe" || basename == "mintty.exe" {
        return Err(AppError::Other(format!(
            "'{}' is a detached launcher and cannot host a ConPTY terminal; \
             use Git for Windows bin\\bash.exe or a direct console shell",
            program
        )));
    }
    Ok(())
}

/// Standard #1271 error shape: names the configured default-shell program, the
/// offending token, and the rejection category, and states that the agent
/// adapter owns command execution (Section 4.3.1 item 6).
pub(super) fn adapter_error(host_program: &str, token: &str, category: &str) -> AppError {
    AppError::Other(format!(
        "Configured default shell '{}' cannot host a resolved agent: token '{}' is {}; \
         the agent adapter owns command execution for this session",
        host_program, token, category
    ))
}

/// Section 4.3 - the common rule, applied BEFORE adapter selection: NUL, CR,
/// and LF in the configured program, every configured argument, and every
/// logical agent argv element, plus the blank-program rejection.
pub(super) fn validate_common_adapter_input(
    command: &str,
    args: &[String],
    host_shell: &ResolvedAgentHostShell,
) -> Result<(), AppError> {
    if host_shell.program.trim().is_empty() {
        return Err(adapter_error(
            &host_shell.program,
            "(program)",
            "a blank configured default-shell program",
        ));
    }
    for (value, label) in [
        (
            host_shell.program.as_str(),
            "configured default-shell program",
        ),
        (command, "logical agent program"),
    ] {
        if value.contains('\0') || value.contains('\r') || value.contains('\n') {
            return Err(adapter_error(
                &host_shell.program,
                value,
                &format!("the {label} contains a forbidden line separator (NUL, CR, or LF)"),
            ));
        }
    }
    for argument in host_shell.args.iter().chain(args.iter()) {
        if argument.contains('\0') || argument.contains('\r') || argument.contains('\n') {
            return Err(adapter_error(
                &host_shell.program,
                argument,
                "contains a forbidden line separator (NUL, CR, or LF)",
            ));
        }
    }
    Ok(())
}

/// Duplicate detection shared by every closed grammar: a permitted option may
/// appear at most once, because host parsers disagree on duplicate handling
/// across versions and no configured duplicate is ever needed.
pub(super) fn mark_seen(
    seen: &mut Vec<String>,
    option: &str,
    host_program: &str,
    token: &str,
) -> Result<(), AppError> {
    if seen.iter().any(|s| s.eq_ignore_ascii_case(option)) {
        return Err(adapter_error(
            host_program,
            token,
            "a duplicate configured option",
        ));
    }
    seen.push(option.to_string());
    Ok(())
}

// --- Section 4.3.2 - PowerShell products -----------------------------------

pub(super) const POWERSHELL_FLAG_OPTIONS: &[&str] =
    &["NoLogo", "NoProfile", "NonInteractive", "Sta", "Mta"];
pub(super) const POWERSHELL_VALUE_OPTIONS: &[&str] = &[
    "ExecutionPolicy",
    "WindowStyle",
    "InputFormat",
    "OutputFormat",
    "PSConsoleFile",
];
pub(super) const PWSH_FLAG_OPTIONS: &[&str] = &["NoLogo", "NoProfile", "NonInteractive"];
pub(super) const PWSH_VALUE_OPTIONS: &[&str] = &[
    "ExecutionPolicy",
    "InputFormat",
    "OutputFormat",
    "WorkingDirectory",
    "ConfigurationName",
    "SettingsFile",
];
/// Command-ownership, keep-open, terminate-early, server/host-mode, and parser
/// terminators, rejected conservatively for BOTH PowerShell classes. Each
/// spelling may appear with `-` or `/`; `--` and `--%` are literal tokens.
pub(super) const POWERSHELL_CONFLICTING_OPTIONS: &[&str] = &[
    "Command",
    "c",
    "CommandWithArgs",
    "cwa",
    "File",
    "f",
    "EncodedCommand",
    "enc",
    "e",
    "NoExit",
    "noe",
    "Version",
    "v",
    "Help",
    "h",
    "?",
    "SSHServerMode",
    "ServerMode",
    "WindowsPowerShell",
    "Login",
    "Interactive",
];
/// powershell.exe-only spellings that pwsh does not define; rejected for pwsh
/// with the unknown/ambiguous category.
pub(super) const PWSH_UNKNOWN_OPTIONS: &[&str] = &["Sta", "Mta", "WindowStyle", "PSConsoleFile"];

/// Section 4.3.1 - the closed configured-host grammar for PowerShell products.
/// Exact, case-insensitive spelling matching only; every permitted value-bearing
/// option binds its operand attached (`-Name:value` / `-Name=value`) or as the
/// single immediately following non-option token.
pub(super) fn validate_powershell_arguments(
    host_program: &str,
    args: &[String],
    is_pwsh: bool,
) -> Result<(), AppError> {
    let flags = if is_pwsh {
        PWSH_FLAG_OPTIONS
    } else {
        POWERSHELL_FLAG_OPTIONS
    };
    let values = if is_pwsh {
        PWSH_VALUE_OPTIONS
    } else {
        POWERSHELL_VALUE_OPTIONS
    };
    let mut seen: Vec<String> = Vec::new();
    let mut pending_operand: Option<String> = None;
    for token in args {
        if let Some(option) = pending_operand.take() {
            if token.starts_with('-') || token.starts_with('/') {
                return Err(adapter_error(
                    host_program,
                    token,
                    "an option-shaped separated operand (value-bearing option has no bound value)",
                ));
            }
            mark_seen(&mut seen, &option, host_program, token)?;
            continue;
        }
        if token == "--" || token == "--%" {
            return Err(adapter_error(
                host_program,
                token,
                "a conflicting/terminal configured option",
            ));
        }
        if !token.starts_with('-') && !token.starts_with('/') {
            return Err(adapter_error(
                host_program,
                token,
                "an unknown or ambiguous token",
            ));
        }
        let body = &token[1..];
        // Conflicting/terminal spellings are matched against the FULL body before
        // any operand split: `-E:O` is not the `-e` spelling, so an attached
        // operand can never turn an unknown token into a terminal one.
        if POWERSHELL_CONFLICTING_OPTIONS
            .iter()
            .any(|option| option.eq_ignore_ascii_case(body))
        {
            return Err(adapter_error(
                host_program,
                token,
                "a conflicting/terminal configured option",
            ));
        }
        let (name, attached) = match body.find([':', '=']) {
            Some(idx) => (&body[..idx], Some(&body[idx + 1..])),
            None => (body, None),
        };
        if let Some(flag) = flags
            .iter()
            .find(|option| option.eq_ignore_ascii_case(name))
        {
            if attached.is_some() {
                return Err(adapter_error(
                    host_program,
                    token,
                    "an unknown or ambiguous token",
                ));
            }
            mark_seen(&mut seen, flag, host_program, token)?;
            continue;
        }
        if let Some(option) = values
            .iter()
            .find(|option| option.eq_ignore_ascii_case(name))
        {
            match attached {
                Some(value) if !value.is_empty() => {
                    mark_seen(&mut seen, option, host_program, token)?;
                }
                Some(_) => {
                    return Err(adapter_error(
                        host_program,
                        token,
                        "a missing or option-shaped operand",
                    ));
                }
                None => {
                    pending_operand = Some(option.to_string());
                }
            }
            continue;
        }
        if is_pwsh
            && PWSH_UNKNOWN_OPTIONS
                .iter()
                .any(|option| option.eq_ignore_ascii_case(name))
        {
            return Err(adapter_error(
                host_program,
                token,
                "an unknown or ambiguous token",
            ));
        }
        return Err(adapter_error(
            host_program,
            token,
            "an unknown or ambiguous token",
        ));
    }
    validate_powershell_pending_operand(host_program, pending_operand)
}

// --- Section 4.3.3 - cmd(.exe) ---------------------------------------------

pub(super) const CMD_FLAG_OPTIONS: &[&str] =
    &["A", "U", "Q", "D", "E:ON", "E:OFF", "F:ON", "F:OFF"];

/// Section 4.3.3 - closed configured-host grammar for cmd: `/`-prefixed exact
/// spellings only, `/T:<value>` as the sole attached-value form, no separated
/// operands, no `-` prefix.
pub(super) fn validate_cmd_arguments(host_program: &str, args: &[String]) -> Result<(), AppError> {
    let mut seen: Vec<String> = Vec::new();
    for token in args {
        if !token.starts_with('/') {
            return Err(adapter_error(
                host_program,
                token,
                "an unknown or ambiguous token",
            ));
        }
        let body = &token[1..];
        if body.eq_ignore_ascii_case("C")
            || body.eq_ignore_ascii_case("K")
            || body.eq_ignore_ascii_case("S")
            || body.eq_ignore_ascii_case("?")
        {
            return Err(adapter_error(
                host_program,
                token,
                "a conflicting/terminal configured option",
            ));
        }
        if body.eq_ignore_ascii_case("V:OFF") {
            mark_seen(&mut seen, "V", host_program, token)?;
            continue;
        }
        if body.to_ascii_uppercase().starts_with("V:") {
            // /V:ON and any other /V: value change `!` expansion semantics.
            return Err(adapter_error(
                host_program,
                token,
                "a conflicting/terminal configured option",
            ));
        }
        if body.to_ascii_uppercase().starts_with("T:") {
            let value = &body[2..];
            if value.is_empty() {
                return Err(adapter_error(
                    host_program,
                    token,
                    "a missing or option-shaped operand",
                ));
            }
            mark_seen(&mut seen, "T", host_program, token)?;
            continue;
        }
        if let Some(flag) = CMD_FLAG_OPTIONS
            .iter()
            .find(|option| option.eq_ignore_ascii_case(body))
        {
            let canonical = flag.split(':').next().unwrap_or(flag);
            mark_seen(&mut seen, canonical, host_program, token)?;
            continue;
        }
        return Err(adapter_error(
            host_program,
            token,
            "an unknown or ambiguous token",
        ));
    }
    Ok(())
}
// --- Section 4.5 - cmd payload domain --------------------------------------

/// Section 4.5 - the configured-cmd payload is the logical program plus args
/// joined with single ASCII spaces and NO per-token quoting: portable-pty's
/// `append_quoted` re-encodes every `CommandBuilder` argument with C-runtime
/// rules and cmd re-parses that string with its own grammar, so per-token
/// quoting cannot survive the round trip (plan Finding A). The accepted domain
/// is exactly the set for which `append_quoted` is inert apart from the outer
/// pair; every other form is rejected before PTY acquisition.
pub(super) fn cmd_payload(command: &str, args: &[String]) -> String {
    let mut payload = command.to_string();
    for arg in args {
        payload.push(' ');
        payload.push_str(arg);
    }
    payload
}

pub(super) fn validate_cmd_payload(
    host_program: &str,
    command: &str,
    args: &[String],
) -> Result<(), AppError> {
    if command.starts_with('@') {
        return Err(adapter_error(
            host_program,
            command,
            "an unsupported cmd payload character (program token starts with '@')",
        ));
    }
    for token in std::iter::once(command).chain(args.iter().map(|s| s.as_str())) {
        if token.is_empty() {
            return Err(adapter_error(
                host_program,
                "(empty)",
                "an unsupported cmd payload character (empty token)",
            ));
        }
        if token.contains('%') {
            return Err(adapter_error(
                host_program,
                token,
                "an unsupported cmd payload character ('%' expands in cmd)",
            ));
        }
        if token.chars().any(|c| c.is_whitespace()) {
            return Err(adapter_error(
                host_program,
                token,
                "an unsupported cmd payload character (whitespace splits tokens in cmd)",
            ));
        }
        if token.contains('"')
            || token.contains('&')
            || token.contains('|')
            || token.contains('<')
            || token.contains('>')
            || token.contains('(')
            || token.contains(')')
            || token.contains('^')
            || token.contains('=')
            || token.contains(',')
            || token.contains(';')
        {
            return Err(adapter_error(
                host_program,
                token,
                "an unsupported cmd payload character (cmd re-parses it as syntax)",
            ));
        }
    }
    if cmd_payload(command, args).ends_with('\\') {
        return Err(adapter_error(
            host_program,
            "(payload)",
            "an unsupported cmd payload character (payload-final backslash is doubled by \
                append_quoted)",
        ));
    }
    Ok(())
}

// --- Section 4.4 - PowerShell/pwsh protocol ---------------------------------

/// Standard Windows command-line-to-argv quote/backslash encoding (Section 4.5):
/// outer double quotes; `2n+1` backslashes before an embedded double quote; a
/// run unchanged before any other character; `2*trailing_run` backslashes
/// before the closing outer quote. An empty value encodes as `""`.
pub(super) fn windows_arg(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let mut run = 0;
        while i < chars.len() && chars[i] == '\\' {
            run += 1;
            i += 1;
        }
        if i < chars.len() {
            if chars[i] == '"' {
                out.extend(std::iter::repeat_n('\\', 2 * run + 1));
                out.push('"');
            } else {
                out.extend(std::iter::repeat_n('\\', run));
                out.push(chars[i]);
            }
            i += 1;
        } else {
            out.extend(std::iter::repeat_n('\\', 2 * run));
        }
    }
    out.push('"');
    out
}

/// Section 4.4 - the native `Application` branch's `ProcessStartInfo.Arguments`
/// string: logical arguments ONLY (never the resolved program), each
/// `windows_arg` encoded, joined with one ASCII space. Empty vector -> empty
/// string; one empty argument -> `""`.
pub(super) fn windows_raw_argv(args: &[String]) -> String {
    args.iter()
        .map(|arg| windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// PowerShell single-quoted literal: wrap in single quotes, double every
/// embedded single quote. Always ends with `'`, so the generated script can
/// never end with a backslash (the only generator invariant, plan Finding B).
pub(super) fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// POSIX single-quoted literal: wrap in single quotes, replace an embedded
/// single quote with the exact fragment `'"'"'`.
pub(super) fn posix_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// #1271 - the resolved batch child is launched through one explicit system-cmd
/// child (`/D /V:OFF /S /C`). Its payload tokens are `'"' + <raw value> + '"'`:
/// cmd's batch tokenizer is quote-aware but NOT backslash-aware (probe-proven),
/// so the C-runtime `windows_arg` trailing-run doubling would corrupt a value
/// ending in a backslash. Values containing `%` or `"` are already rejected
/// before this branch runs, so the raw quote wrapper is safe for the whole
/// batch domain.
/// Section 4.4 - the generated `-Command` script. Resolves the logical program
/// as an external command only (two-argument `GetCommand` with the
/// `Application | ExternalScript` filter, never a profile alias/function),
/// starts a native `Application` through `ProcessStartInfo` with separate
/// `FileName` and a standard Windows-encoded `Arguments` literal, runs a
/// resolved `.cmd`/`.bat` through one explicit system-cmd child, and keeps
/// literal invocation only for an `ExternalScript`. Contains no `--%` token and
/// never ends with a backslash (the trailing-run invariant).
pub(super) fn powershell_script(command: &str, args: &[String]) -> String {
    let batch_unsupported = args
        .iter()
        .any(|arg| arg.contains('%') || arg.contains('"'));
    let mut script = String::new();
    script.push_str("$global:LASTEXITCODE = $null;\n");
    script.push_str("$ac_batch_unsupported_logical_arg = $");
    script.push_str(if batch_unsupported { "true" } else { "false" });
    script.push_str(";\n");
    script.push_str(
        "$ac_kind = [System.Management.Automation.CommandTypes]::Application -bor \
         [System.Management.Automation.CommandTypes]::ExternalScript;\n",
    );
    script.push_str("$ac_command = $ExecutionContext.InvokeCommand.GetCommand(");
    script.push_str(&ps_literal(command));
    script.push_str(", $ac_kind);\n");
    script.push_str("if ($null -eq $ac_command) { exit 1 };\n");
    script.push_str(
        "if ($ac_command.CommandType -eq \
         [System.Management.Automation.CommandTypes]::Application) {\n",
    );
    script.push_str(
        "  if (([System.IO.Path]::GetExtension($ac_command.Path) -ieq '.cmd') -or \
         ([System.IO.Path]::GetExtension($ac_command.Path) -ieq '.bat')) {\n",
    );
    script.push_str(
        "    if ($ac_batch_unsupported_logical_arg -or $ac_command.Path.Contains('%') -or \
         $ac_command.Path.Contains([char]34)) { exit 1 };\n",
    );
    script.push_str("    $ac_batch_payload = '\"' + $ac_command.Path + '\"'");
    for arg in args {
        script.push_str(" + ' ' + '\"' + ");
        script.push_str(&ps_literal(arg));
        script.push_str(" + '\"'");
    }
    script.push_str(";\n");
    script.push_str("    $ac_start = New-Object System.Diagnostics.ProcessStartInfo;\n");
    script.push_str("    $ac_start.UseShellExecute = $false;\n");
    script.push_str("    $ac_start.RedirectStandardInput = $false;\n");
    script.push_str("    $ac_start.RedirectStandardOutput = $false;\n");
    script.push_str("    $ac_start.RedirectStandardError = $false;\n");
    script.push_str(
        "    $ac_start.FileName = \
         [System.IO.Path]::Combine([System.Environment]::SystemDirectory, 'cmd.exe');\n",
    );
    script.push_str("    $ac_start.Arguments = '/D /V:OFF /S /C \"' + $ac_batch_payload + '\"';\n");
    script.push_str(
        "    try { $ac_process = [System.Diagnostics.Process]::Start($ac_start) } \
        catch { exit 1 };\n",
    );
    script.push_str("    if ($null -eq $ac_process) { exit 1 };\n");
    script.push_str("    $ac_process.WaitForExit();\n");
    script.push_str("    exit $ac_process.ExitCode\n");
    script.push_str("  }\n");
    script.push_str("  $ac_start = New-Object System.Diagnostics.ProcessStartInfo;\n");
    script.push_str("  $ac_start.UseShellExecute = $false;\n");
    script.push_str("  $ac_start.RedirectStandardInput = $false;\n");
    script.push_str("  $ac_start.RedirectStandardOutput = $false;\n");
    script.push_str("  $ac_start.RedirectStandardError = $false;\n");
    script.push_str("  $ac_start.FileName = $ac_command.Path;\n");
    script.push_str("  $ac_start.Arguments = ");
    script.push_str(&ps_literal(&windows_raw_argv(args)));
    script.push_str(";\n");
    script.push_str(
        "  try { $ac_process = [System.Diagnostics.Process]::Start($ac_start) } \
        catch { exit 1 };\n",
    );
    script.push_str("  if ($null -eq $ac_process) { exit 1 };\n");
    script.push_str("  $ac_process.WaitForExit();\n");
    script.push_str("  exit $ac_process.ExitCode\n");
    script.push_str("}\n");
    script.push_str("& $ac_command.Path");
    for arg in args {
        script.push(' ');
        script.push_str(&ps_literal(arg));
    }
    script.push_str(";\n");
    script.push_str("$ac_succeeded = $?; $ac_exit_code = $LASTEXITCODE;\n");
    script.push_str("if ($null -ne $ac_exit_code) { exit $ac_exit_code };\n");
    script.push_str("if ($ac_succeeded) { exit 0 }; exit 1");
    script
}

// --- Section 4.6 - custom POSIX-compatible shell ----------------------------

/// #1498 - terminal-only flags a POSIX host may keep for a bare terminal but
/// that never reach a resolved-agent launch: the login/interactive flags and
/// the PowerShell `-NoLogo` relic that can ride along in migrated configs
/// (today's Windows default shell argument; #1499 moves fresh defaults to Git
/// Bash). Case-insensitive exact match only: payload and rc flags such as `-c`
/// or `--norc` are NOT terminal-only and stay rejected.
pub(super) fn is_terminal_only_posix_arg(arg: &str) -> bool {
    matches!(
        arg.to_ascii_lowercase().as_str(),
        "-l" | "--login" | "-i" | "-nologo"
    )
}

/// Section 4.6 - one `exec` command, so the custom shell replaces itself with
/// the agent and the agent owns the PTY child exit code directly.
pub(super) fn posix_script(command: &str, args: &[String]) -> String {
    let mut script = String::from("exec ");
    script.push_str(&posix_literal(command));
    for arg in args {
        script.push(' ');
        script.push_str(&posix_literal(arg));
    }
    script
}

// --- Section 4.1/4.4/4.5 - entry points ------------------------------------

/// Section 4.4 - a configured logical program that explicitly ends in `.cmd` or
/// `.bat` rejects `%` or `"` in ANY logical argument in Rust, before any PTY is
/// acquired. A bare name that resolves to batch only at runtime is handled by
/// the generated script instead (post-PTY, Section 4.4).
pub(super) fn validate_explicit_batch_args(
    host_program: &str,
    command: &str,
    args: &[String],
) -> Result<(), AppError> {
    let lower = command.to_ascii_lowercase();
    let explicit_batch = lower.ends_with(".cmd") || lower.ends_with(".bat");
    if explicit_batch
        && args
            .iter()
            .any(|arg| arg.contains('%') || arg.contains('"'))
    {
        return Err(adapter_error(
            host_program,
            "(batch target)",
            "an unsupported explicit-batch payload character ('%' or '\"' in a logical argument)",
        ));
    }
    Ok(())
}

/// The single private adapter seam (Section 4.1.4): logical agent argv plus the
/// optional host-shell snapshot in, executable plus argv and matching provenance
/// out. On Windows a resolved agent that is not a direct `.exe` enters the
/// adapter; a no-resolved-agent session keeps the historical `cmd.exe /C`
/// fallback byte-for-byte; non-Windows and direct `.exe` launches are unchanged.
pub(super) fn prepare_launch(
    command: &str,
    args: &[String],
    resolved_agent_host_shell: Option<&ResolvedAgentHostShell>,
) -> Result<PreparedLaunch, AppError> {
    if cfg!(windows) && !is_direct_exe(command) {
        if let Some(host_shell) = resolved_agent_host_shell {
            return prepare_windows_resolved_agent_launch(command, args, host_shell);
        }
        // #1271 - no resolved agent: keep the existing `cmd.exe /C` construction
        // byte-for-byte (a bare non-direct configured shell on Windows).
        let mut wrapped = vec!["/C".to_string(), command.to_string()];
        wrapped.extend(args.iter().cloned());
        return Ok(PreparedLaunch {
            program: "cmd.exe".to_string(),
            args: wrapped,
        });
    }
    // #1498 - bare terminals and direct `.exe` agents: a detached launcher
    // would acquire ConPTY resources for a child that never attaches, so
    // reject before any resource acquisition. The compile-time `cfg!(windows)`
    // guard keeps non-Windows behavior byte-identical (the branch is dead
    // at runtime on non-Windows but still compiled, so the helper must
    // compile on every platform).
    if cfg!(windows) {
        reject_detached_launcher(command)?;
    }
    Ok(PreparedLaunch {
        program: command.to_string(),
        args: args.to_vec(),
    })
}

pub(super) fn prepare_windows_resolved_agent_launch(
    command: &str,
    args: &[String],
    host_shell: &ResolvedAgentHostShell,
) -> Result<PreparedLaunch, AppError> {
    validate_common_adapter_input(command, args, host_shell)?;
    // #1498 - a configured host shell that is a detached launcher would
    // spawn a detached MinTTY child on ConPTY resources; reject before
    // `openpty` and before any spawn accounting.
    reject_detached_launcher(&host_shell.program)?;
    let mut launch_args = host_shell.args.clone();
    match configured_shell_kind(&host_shell.program) {
        WindowsHostShellKind::PowerShell => {
            validate_powershell_arguments(&host_shell.program, &host_shell.args, false)?;
            validate_explicit_batch_args(&host_shell.program, command, args)?;
            launch_args.push("-Command".to_string());
            launch_args.push(powershell_script(command, args));
        }
        WindowsHostShellKind::Pwsh => {
            validate_powershell_arguments(&host_shell.program, &host_shell.args, true)?;
            validate_explicit_batch_args(&host_shell.program, command, args)?;
            launch_args.push("-Command".to_string());
            launch_args.push(powershell_script(command, args));
        }
        WindowsHostShellKind::Cmd => {
            validate_cmd_arguments(&host_shell.program, &host_shell.args)?;
            validate_cmd_payload(&host_shell.program, command, args)?;
            launch_args.extend(["/V:OFF".to_string(), "/S".to_string(), "/C".to_string()]);
            launch_args.push(cmd_payload(command, args));
        }
        WindowsHostShellKind::Posix => {
            // #1498 - terminal-only configured flags (`-l`, `--login`, `-i`,
            // `-NoLogo`) may configure a bare terminal but never reach a
            // resolved-agent launch: the adapter owns only `-c` plus its
            // `exec` payload. Any other configured argument keeps the
            // historical conflicting/terminal rejection, naming the offending
            // token.
            for arg in &host_shell.args {
                if !is_terminal_only_posix_arg(arg) {
                    return Err(adapter_error(
                        &host_shell.program,
                        arg,
                        "a conflicting/terminal configured option (custom shell accepts no \
                            arguments)",
                    ));
                }
            }
            launch_args.clear();
            launch_args.push("-c".to_string());
            launch_args.push(posix_script(command, args));
        }
    }
    Ok(PreparedLaunch {
        program: host_shell.program.clone(),
        args: launch_args,
    })
}

fn validate_powershell_pending_operand(
    host_program: &str,
    pending_operand: Option<String>,
) -> Result<(), AppError> {
    if let Some(option) = pending_operand {
        return Err(adapter_error(
            host_program,
            &option,
            "a missing or option-shaped operand (value-bearing option has no operand)",
        ));
    }
    Ok(())
}
