//! #1271 - which launcher AC interposes when Windows cannot execute an agent command
//! directly, and exactly how that launcher is invoked.
//!
//! Before #1271 `spawn_sync` hardcoded `cmd.exe /C` for every non-`.exe` command on
//! Windows, so Settings > General > SHELL said `powershell.exe` while the live process
//! tree said `cmd.exe`. This module owns the decision instead, as pure host-independent
//! functions: `decide_launcher` says WHICH launcher (or none), `build_launch` says HOW it
//! is invoked. `spawn_sync` applies the result and holds no policy.
//!
//! Two things the whole design turns on, both measured rather than assumed:
//!
//! 1. `CommandBuilder` already applies the MSVCRT `ArgvQuote` algorithm to the program and
//!    to every argument (`portable-pty-0.8.1/src/cmdbuilder.rs:653-698`), so today's
//!    baseline is good for quotes and empty tokens and bad only for unquoted `cmd.exe`
//!    metacharacters. Any replacement has to beat that baseline, not an imaginary one.
//! 2. `-EncodedCommand` defeats the three parsers above the PowerShell language parser but
//!    NOT the fourth one below it: when PowerShell invokes a native command it
//!    re-serializes the parsed arguments back into a single Win32 command line, and in
//!    Windows PowerShell 5.1 that step is lossy. `legacy_arg_escape` is what closes it.

use std::path::Path;

use base64::Engine as _;

/// The launcher families AC knows how to interpose on Windows. Everything else is refused
/// loudly (see `detect_launcher_family`) rather than supported unreliably.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherFamily {
    Cmd,
    PowerShell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLauncher {
    pub family: LauncherFamily,
    /// The program CommandBuilder is constructed with. For `Cmd` this is always the
    /// literal "cmd.exe"; for `PowerShell` it is the ABSOLUTE path the probe returned,
    /// e.g. C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe. Anyone diffing
    /// exec_argv against the configured string will see a different value; that is
    /// intended, not a bug.
    pub program: String,
}

/// Why AC fell back to `cmd.exe` instead of honouring the configured shell. Each one
/// raises exactly one session warning; none of them ever fails a spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherFallback {
    EmptyConfiguredShell,
    UnsupportedShellFamily,
    ConfiguredShellNotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherDecision {
    pub launcher: Option<ResolvedLauncher>,
    pub fallback: Option<LauncherFallback>,
}

/// What spawn_sync needs. `argv` is what CommandBuilder gets; `diagnostic_argv` is what
/// spawn_diagnostics records; `paint_floor` is the launcher-aware stall threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: String,
    pub argv: Vec<String>,
    pub diagnostic_argv: Vec<String>,
    pub paint_floor: u64,
}

/// The `cmd.exe` floor is the module-wide default, referenced rather than re-spelled so
/// the two cannot drift apart.
const CMD_PAINT_FLOOR_BYTES: u64 = crate::pty::spawn_diagnostics::Thresholds::DEFAULT.paint_floor;

/// Measured through a real ConPTY: PowerShell's own preamble puts 100 bytes (5.1) or 16
/// (pwsh) on the PTY, and a `CommandNotFoundException` banner alone puts 572 (5.1) or 380
/// (pwsh). 256 would therefore read a not-found agent as "painted" and silently disable
/// the #942 stall detector. 640 sits above both banners and below a painted TUI.
const POWERSHELL_PAINT_FLOOR_BYTES: u64 = 640;

/// The launcher's switch set is fixed per family and deliberately NOT user-configurable:
/// `default_shell_args` configures an INTERACTIVE shell, and a `-NoExit` or `/K` there
/// would leave the launcher alive after the agent exits and break exit propagation and
/// teardown.
const POWERSHELL_SWITCHES: [&str; 3] = ["-NoProfile", "-ExecutionPolicy", "Bypass"];

/// The `Cmd` launcher, spelled the same way on every fallback path so that a fallback
/// spawn is byte for byte what AC did before #1271, `exec_argv` included. A configured
/// `C:\Windows\System32\cmd.exe` is normalised to `cmd.exe` for the same reason.
fn cmd_fallback() -> ResolvedLauncher {
    ResolvedLauncher {
        family: LauncherFamily::Cmd,
        program: "cmd.exe".to_string(),
    }
}

fn no_launcher() -> LauncherDecision {
    LauncherDecision {
        launcher: None,
        fallback: None,
    }
}

fn fallback_to_cmd(fallback: LauncherFallback) -> LauncherDecision {
    LauncherDecision {
        launcher: Some(cmd_fallback()),
        fallback: Some(fallback),
    }
}

/// Pure. Basename file stem, case-insensitive, so `powershell`, `powershell.exe` and
/// `C:\Program Files\PowerShell\7\pwsh.exe` all match.
///
/// `bash`, `sh`, `zsh` and every other POSIX shell are deliberately `None` on Windows: a
/// Windows-native command line handed to an MSYS/Cygwin `bash.exe` is re-parsed by the
/// MSYS runtime's own translation layer before `bash -c` sees it, and AC cannot guarantee
/// argument fidelity across that layer.
pub fn detect_launcher_family(configured_shell: &str) -> Option<LauncherFamily> {
    // Both separators, always: this runs on Linux CI against Windows-shaped paths, where
    // `Path::file_stem` would treat the whole `C:\...\cmd.exe` as one component.
    let basename = configured_shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(configured_shell);
    let stem = match basename.rfind('.') {
        Some(dot) if dot > 0 => &basename[..dot],
        _ => basename,
    };
    match stem.to_ascii_lowercase().as_str() {
        "cmd" => Some(LauncherFamily::Cmd),
        "powershell" | "pwsh" => Some(LauncherFamily::PowerShell),
        _ => None,
    }
}

/// Impure probe. `which::which` for a bare name, `Path::is_file` for anything containing a
/// separator or absolute.
pub fn resolve_configured_shell_path(configured_shell: &str) -> Option<String> {
    let candidate = Path::new(configured_shell);
    if configured_shell.contains('/') || configured_shell.contains('\\') || candidate.is_absolute()
    {
        return candidate.is_file().then(|| configured_shell.to_string());
    }
    which::which(configured_shell)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

/// Pure and fully injectable. `probe` is `resolve_configured_shell_path` in production.
///
/// `is_agent_session` is the caller's `is_agent_owned_launch`, NOT `resolved_spawn
/// .is_some()`: on the restart path an agent whose profile no longer resolves falls back
/// to its stored command, and a `resolved_spawn` gate would classify that as a bare
/// terminal and silently put `cmd.exe` back in the tree while Settings still says
/// `powershell.exe`.
pub fn decide_launcher(
    host_is_windows: bool,
    backend_is_local: bool,
    is_agent_session: bool,
    configured_shell: &str,
    cmd: &str,
    probe: &dyn Fn(&str) -> Option<String>,
) -> LauncherDecision {
    // Container sessions never carry a host launcher.
    if !backend_is_local {
        return no_launcher();
    }
    // A bare terminal keeps its pre-#1271 behaviour byte for byte, stray `cmd.exe`
    // included, and raises no warning. Widening to it is a separate decision.
    if !is_agent_session {
        return no_launcher();
    }
    // The wrapper exists solely because Windows `CreateProcess` cannot execute an
    // extensionless `claude`. On POSIX `execvp` resolves it and runs its shebang, so
    // interposing anything would ADD a process level that does not exist today.
    if !host_is_windows {
        return no_launcher();
    }
    if is_direct_exe(cmd) {
        return no_launcher();
    }
    if configured_shell.trim().is_empty() {
        return fallback_to_cmd(LauncherFallback::EmptyConfiguredShell);
    }
    match detect_launcher_family(configured_shell) {
        None => fallback_to_cmd(LauncherFallback::UnsupportedShellFamily),
        // No probe: this is today's behaviour, and today's behaviour never probed.
        Some(LauncherFamily::Cmd) => LauncherDecision {
            launcher: Some(cmd_fallback()),
            fallback: None,
        },
        Some(LauncherFamily::PowerShell) => match probe(configured_shell) {
            Some(program) => LauncherDecision {
                launcher: Some(ResolvedLauncher {
                    family: LauncherFamily::PowerShell,
                    program,
                }),
                fallback: None,
            },
            None => fallback_to_cmd(LauncherFallback::ConfiguredShellNotFound),
        },
    }
}

/// Pure. The whole quoting, exit-code and diagnostics contract lives here.
pub fn build_launch(launcher: &ResolvedLauncher, cmd: &str, args: &[String]) -> LaunchPlan {
    match launcher.family {
        LauncherFamily::Cmd => {
            let mut argv = vec!["/C".to_string(), cmd.to_string()];
            argv.extend(args.iter().cloned());
            let mut diagnostic_argv = vec![launcher.program.clone()];
            diagnostic_argv.extend(argv.iter().cloned());
            LaunchPlan {
                program: launcher.program.clone(),
                argv,
                diagnostic_argv,
                paint_floor: CMD_PAINT_FLOOR_BYTES,
            }
        }
        LauncherFamily::PowerShell => {
            let encoded = encode_command(&powershell_script(cmd, args));
            let mut argv: Vec<String> = POWERSHELL_SWITCHES.iter().map(|s| s.to_string()).collect();
            argv.push("-EncodedCommand".to_string());
            argv.push(encoded.clone());

            // The payload is elided and the tokens it encodes are restored RAW after a
            // `--` separator: `SpawnRecord::scrub` redacts by plain substring replacement,
            // and a base64 blob defeats that completely. Recording the decoded script
            // instead is worse still, because it writes a full PowerShell program into
            // app.log and the script is exactly where a secret lives when the redactor
            // does not reach it.
            let mut diagnostic_argv = vec![launcher.program.clone()];
            diagnostic_argv.extend(POWERSHELL_SWITCHES.iter().map(|s| s.to_string()));
            diagnostic_argv.push("-EncodedCommand".to_string());
            diagnostic_argv.push(format!("<encoded payload elided: {} bytes>", encoded.len()));
            diagnostic_argv.push("--".to_string());
            diagnostic_argv.push(cmd.to_string());
            diagnostic_argv.extend(args.iter().cloned());

            LaunchPlan {
                program: launcher.program.clone(),
                argv,
                diagnostic_argv,
                paint_floor: POWERSHELL_PAINT_FLOOR_BYTES,
            }
        }
    }
}

/// The PowerShell payload, before base64. Every line is load-bearing; see the module
/// header and the notes below.
fn powershell_script(cmd: &str, args: &[String]) -> String {
    let quoted_cmd = ps_quote(cmd);

    // `Standard` (PowerShell 7.3+) passes arguments losslessly, so they go through
    // verbatim. Everything older uses the Legacy binder and needs the pre-escape.
    let standard_args: String = args
        .iter()
        .map(|arg| format!(" {}", ps_quote(arg)))
        .collect();
    let legacy_args: String = args
        .iter()
        .map(|arg| format!(" {}", ps_quote(&legacy_arg_escape(arg))))
        .collect();

    // `$null` and not `= 1`: a pre-seed of 1 turns "the agent ran fine but the target set
    // no exit code" into a reported crash, and npm-shaped `.ps1` shims are exactly that
    // shape. The `Get-Command` gate, not `$?`, separates "nothing ran" from "the target
    // ran and failed": measured, `$?` is True for a `.ps1` that failed with a
    // non-terminating error. `cmd` is q-quoted and NEVER v-escaped in either arm, because
    // PowerShell's command discovery resolves it rather than serializing it through the
    // native binder.
    format!(
        "$LASTEXITCODE = $null\n\
         if ($null -eq (Get-Command {quoted_cmd} -ErrorAction SilentlyContinue)) {{ exit 1 }}\n\
         if (Test-Path variable:PSNativeCommandArgumentPassing) {{\n\
         \x20 $PSNativeCommandArgumentPassing = 'Standard'\n\
         \x20 & {quoted_cmd}{standard_args}\n\
         }} else {{\n\
         \x20 & {quoted_cmd}{legacy_args}\n\
         }}\n\
         if ($null -eq $LASTEXITCODE) {{ exit 0 }}\n\
         exit $LASTEXITCODE"
    )
}

/// UTF-16LE, no BOM, standard base64: the alphabet `-EncodedCommand` accepts and the only
/// encoding it accepts.
fn encode_command(script: &str) -> String {
    let utf16le: Vec<u8> = script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    base64::engine::general_purpose::STANDARD.encode(utf16le)
}

/// Pure. PowerShell single-quoted literal: wrap in `'`, double any inner `'`.
///
/// Single-quoted strings perform no interpolation, no subexpression evaluation and no
/// escape processing, so `$`, backtick, `&`, `|`, `%`, `!`, `"` and whitespace all reach
/// the language parser verbatim and no token can break out of its quoting and become
/// script. Doubling the inner `'` is the complete and only escape rule at this stage.
pub fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push('\'');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Pure. Pre-compensates Windows PowerShell 5.1's Legacy native-argument binder.
///
/// 5.1's scanner does not understand `\"`. Scanning left to right, a `"` toggles an
/// internal quote state and whitespace only sets "needs quoting" while that state is
/// CLOSED; backslashes are invisible to it. So `a"b c` is emitted UNWRAPPED (the space is
/// seen as inside quotes) and the child's CRT then eats the bare `"`. That single rule
/// explains every 5.1 failure this fix had to clear, and it is why escaping the quote as
/// `\"` (the obvious repair) does not help.
///
/// The repair is to BALANCE the scanner rather than fight it: spell an inner quote `""`,
/// which is two toggles, so the state returns to open and interior spaces stay inside. And
/// `""` inside a quoted block is also the CRT / `CommandLineToArgvW` spelling of a literal
/// quote, so both parsers agree on the same byte sequence. The backslash rules are
/// `ArgvQuote`'s; only the spelling of the inner quote differs, and that difference is the
/// entire finding.
///
/// Every token is wrapped unconditionally, INCLUDING the empty string, which is what stops
/// an empty argument from being dropped and shifting the whole argv.
///
/// Applied ONLY in the Legacy arm of the payload: under `Standard` it would corrupt every
/// argument, which is why the arm is selected by the shell's own capability and never by a
/// build-time guess.
pub fn legacy_arg_escape(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(chars.len() + 2);
    out.push('"');
    let mut i = 0;
    while i < chars.len() {
        let mut backslashes = 0;
        while i < chars.len() && chars[i] == '\\' {
            backslashes += 1;
            i += 1;
        }
        if i == chars.len() {
            // A trailing run is doubled so the closing quote cannot escape.
            for _ in 0..(2 * backslashes) {
                out.push('\\');
            }
            break;
        }
        if chars[i] == '"' {
            for _ in 0..(2 * backslashes) {
                out.push('\\');
            }
            out.push_str("\"\"");
        } else {
            for _ in 0..backslashes {
                out.push('\\');
            }
            out.push(chars[i]);
        }
        i += 1;
    }
    out.push('"');
    out
}

/// Pure. Moved out of `spawn_sync` verbatim, not rewritten.
///
/// It is a STRING test, not a resolution: on a host where `claude` is a native installer
/// at `C:\Users\<u>\.local\bin\claude.exe`, `is_direct_exe("claude")` is still false and AC
/// interposes a shell it does not need. That is pre-#1271 behaviour, it is why the bug
/// reaches installs with no shim at all, and #1271 does not change it.
pub fn is_direct_exe(cmd: &str) -> bool {
    cmd.to_lowercase().ends_with(".exe")
        || std::path::Path::new(cmd)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The metacharacter class of the #1271 fixture matrix.
    const METACHARACTER_TOKENS: &[&str] = &[
        "a b",
        "a\"b",
        "a'b",
        "a$b",
        "a`b",
        "a&b",
        "a|b",
        "a%b",
        "a!b",
        "",
        "a\\b\\",
        "--flag=C:\\path with space\\x",
        "{\"mcpServers\":{\"x\":{\"command\":\"y\",\"args\":[\"--p\",\"1\"]}}}",
        "%PATH%",
        "^caret",
        "a>b",
        "a<b",
        "a^&b",
    ];

    /// The residual class: the shapes that only fail by SPLITTING a token, which a
    /// first-element-only comparison hides.
    const RESIDUAL_TOKENS: &[&str] = &[
        "a\"b c",
        "q\"and space\\",
        "{\"mcpServers\": {\"x\": \"y z\"}}",
        "\"leading quote and space",
        "trailing quote and space\"",
        "a\"b\tc",
        "a\"b c\\",
    ];

    pub(super) fn fixture_matrix() -> Vec<&'static str> {
        METACHARACTER_TOKENS
            .iter()
            .chain(RESIDUAL_TOKENS.iter())
            .copied()
            .collect()
    }

    fn never_probed(_shell: &str) -> Option<String> {
        panic!("the probe must not be called for the Cmd family");
    }

    fn probe_missing(_shell: &str) -> Option<String> {
        None
    }

    fn probe_resolves(_shell: &str) -> Option<String> {
        Some("C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string())
    }

    fn powershell_launcher() -> ResolvedLauncher {
        ResolvedLauncher {
            family: LauncherFamily::PowerShell,
            program: "C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string(),
        }
    }

    /// The `-EncodedCommand` payload, back to the script `build_launch` meant to run.
    fn decode_payload(payload: &str) -> String {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .expect("payload is standard base64");
        assert_eq!(bytes.len() % 2, 0, "UTF-16LE payload must be even-length");
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).expect("payload decodes as UTF-16")
    }

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    // --- detect_launcher_family (1-3) ---

    #[test]
    fn detect_launcher_family_matches_cmd() {
        for shell in [
            "cmd.exe",
            "CMD.EXE",
            "cmd",
            "C:\\Windows\\System32\\cmd.exe",
        ] {
            assert_eq!(
                detect_launcher_family(shell),
                Some(LauncherFamily::Cmd),
                "{shell}"
            );
        }
    }

    #[test]
    fn detect_launcher_family_matches_powershell() {
        for shell in [
            "powershell.exe",
            "PowerShell",
            "pwsh",
            "pwsh.exe",
            "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
        ] {
            assert_eq!(
                detect_launcher_family(shell),
                Some(LauncherFamily::PowerShell),
                "{shell}"
            );
        }
    }

    #[test]
    fn detect_launcher_family_refuses_everything_else() {
        for shell in [
            "bash",
            "bash.exe",
            "sh",
            "zsh",
            "C:\\Program Files\\Git\\bin\\bash.exe",
            "wsl.exe",
            "notepad.exe",
            "",
        ] {
            assert_eq!(detect_launcher_family(shell), None, "{shell}");
        }
    }

    // --- decide_launcher (4-13) ---

    #[test]
    fn container_backend_never_carries_a_launcher() {
        assert_eq!(
            decide_launcher(
                true,
                false,
                true,
                "powershell.exe",
                "claude",
                &probe_resolves
            ),
            LauncherDecision {
                launcher: None,
                fallback: None
            }
        );
    }

    #[test]
    fn bare_terminal_keeps_todays_behaviour() {
        assert_eq!(
            decide_launcher(true, true, false, "pwsh", "pwsh", &probe_resolves),
            LauncherDecision {
                launcher: None,
                fallback: None
            }
        );
    }

    #[test]
    fn bare_terminal_never_warns_about_an_unsupported_shell() {
        let decision = decide_launcher(true, true, false, "bash", "bash", &probe_resolves);
        assert_eq!(decision.launcher, None);
        assert_eq!(decision.fallback, None);
    }

    #[test]
    fn non_windows_hosts_launch_directly() {
        assert_eq!(
            decide_launcher(false, true, true, "/bin/bash", "claude", &probe_resolves),
            LauncherDecision {
                launcher: None,
                fallback: None
            }
        );
    }

    #[test]
    fn a_direct_exe_needs_no_launcher() {
        assert_eq!(
            decide_launcher(
                true,
                true,
                true,
                "powershell.exe",
                "C:\\t\\agent.exe",
                &probe_resolves
            ),
            LauncherDecision {
                launcher: None,
                fallback: None
            }
        );
    }

    #[test]
    fn configured_cmd_takes_the_cmd_family_without_probing() {
        let decision = decide_launcher(true, true, true, "cmd.exe", "claude", &never_probed);
        assert_eq!(decision.launcher, Some(cmd_fallback()));
        assert_eq!(decision.fallback, None);
    }

    #[test]
    fn empty_configured_shell_falls_back_to_cmd_with_a_warning() {
        let decision = decide_launcher(true, true, true, "   ", "claude", &probe_resolves);
        assert_eq!(decision.launcher, Some(cmd_fallback()));
        assert_eq!(
            decision.fallback,
            Some(LauncherFallback::EmptyConfiguredShell)
        );
    }

    #[test]
    fn unsupported_configured_shell_falls_back_to_cmd_with_a_warning() {
        let decision = decide_launcher(true, true, true, "bash", "claude", &probe_resolves);
        assert_eq!(decision.launcher, Some(cmd_fallback()));
        assert_eq!(
            decision.fallback,
            Some(LauncherFallback::UnsupportedShellFamily)
        );
    }

    #[test]
    fn missing_configured_shell_falls_back_to_cmd_with_a_warning() {
        let decision =
            decide_launcher(true, true, true, "powershell.exe", "claude", &probe_missing);
        assert_eq!(decision.launcher, Some(cmd_fallback()));
        assert_eq!(
            decision.fallback,
            Some(LauncherFallback::ConfiguredShellNotFound)
        );
    }

    #[test]
    fn resolved_powershell_becomes_the_launcher() {
        let decision = decide_launcher(
            true,
            true,
            true,
            "powershell.exe",
            "claude",
            &probe_resolves,
        );
        assert_eq!(decision.launcher, Some(powershell_launcher()));
        assert_eq!(decision.fallback, None);
    }

    // --- legacy_arg_escape (14-19) ---

    #[test]
    fn legacy_arg_escape_wraps_the_empty_string() {
        assert_eq!(legacy_arg_escape(""), "\"\"");
    }

    #[test]
    fn legacy_arg_escape_spells_an_inner_quote_twice() {
        assert_eq!(legacy_arg_escape("a\"b c"), "\"a\"\"b c\"");
        assert_eq!(legacy_arg_escape("a\"b"), "\"a\"\"b\"");
        assert_eq!(legacy_arg_escape("a'b"), "\"a'b\"");
    }

    #[test]
    fn legacy_arg_escape_doubles_only_a_trailing_backslash_run() {
        assert_eq!(legacy_arg_escape("a\\b\\"), "\"a\\b\\\\\"");
    }

    #[test]
    fn legacy_arg_escape_doubles_a_run_before_a_quote() {
        assert_eq!(legacy_arg_escape("a\\\"b"), "\"a\\\\\"\"b\"");
    }

    #[test]
    fn legacy_arg_escape_survives_the_reported_json_shape() {
        assert_eq!(
            legacy_arg_escape("{\"mcpServers\": {\"x\": \"y z\"}}"),
            "\"{\"\"mcpServers\"\": {\"\"x\"\": \"\"y z\"\"}}\""
        );
    }

    /// Section 9.1 item 19 asks for "the output never contains a `'`". Taken literally
    /// that contradicts item 15 in the same list, which pins `a'b` -> `"a'b"`, and `a'b`
    /// is itself a token of the fixture matrix. The property the escaper actually has, and
    /// the one the security argument needs, is that it INTRODUCES no `'`: it emits only
    /// `"` and `\` of its own and passes every other character through, so a `'` in the
    /// output can only be a `'` the caller already had, and `ps_quote` doubles those.
    #[test]
    fn legacy_arg_escape_introduces_no_single_quote() {
        for token in fixture_matrix() {
            let escaped = legacy_arg_escape(token);
            assert_eq!(
                escaped.matches('\'').count(),
                token.matches('\'').count(),
                "{token:?} gained or lost a single quote"
            );
        }
    }

    /// The composition that actually reaches PowerShell. A single-quoted literal ends at
    /// the first unpaired `'`, so this is what makes `ps_quote(legacy_arg_escape(s))`
    /// impossible to break out of.
    #[test]
    fn ps_quote_of_an_escaped_token_is_a_single_balanced_literal() {
        for token in fixture_matrix() {
            let literal = ps_quote(&legacy_arg_escape(token));
            let inner = &literal[1..literal.len() - 1];
            assert!(
                literal.starts_with('\'') && literal.ends_with('\''),
                "{token:?}"
            );
            // Every `'` strictly inside the literal belongs to a doubled pair, so none of
            // them can close it early. Asserted run by run rather than on the total count:
            // a parity check passes on `'a'b'c'`, which is two unpaired quotes and a
            // literal that ends four characters early.
            let mut rest = inner;
            while let Some(at) = rest.find('\'') {
                let run = rest[at..].len() - rest[at..].trim_start_matches('\'').len();
                assert_eq!(
                    run % 2,
                    0,
                    "{token:?} leaves a run of {run} unpaired quote(s) inside the literal"
                );
                rest = &rest[at + run..];
            }
        }
    }

    // --- ps_quote (20) ---

    #[test]
    fn ps_quote_doubles_inner_quotes_and_passes_everything_else() {
        assert_eq!(ps_quote("a'b"), "'a''b'");
        assert_eq!(ps_quote("a''b"), "'a''''b'");
        for token in fixture_matrix() {
            if token.contains('\'') {
                continue;
            }
            assert_eq!(ps_quote(token), format!("'{token}'"), "{token:?}");
        }
    }

    // --- build_launch, family Cmd (21-22) ---

    /// The byte-for-byte guard against regressing pre-#1271 behaviour.
    #[test]
    fn cmd_family_reproduces_todays_spawn_exactly() {
        let plan = build_launch(&cmd_fallback(), "claude", &owned(&["--a", "b c"]));
        assert_eq!(plan.program, "cmd.exe");
        assert_eq!(plan.argv, owned(&["/C", "claude", "--a", "b c"]));
        assert_eq!(
            plan.diagnostic_argv,
            owned(&["cmd.exe", "/C", "claude", "--a", "b c"])
        );
        assert_eq!(plan.paint_floor, 256);
    }

    #[test]
    fn cmd_family_with_no_arguments() {
        let plan = build_launch(&cmd_fallback(), "claude", &[]);
        assert_eq!(plan.argv, owned(&["/C", "claude"]));
    }

    // --- build_launch, family PowerShell (23-29) ---

    #[test]
    fn powershell_argv_is_a_fixed_prefix_and_one_payload() {
        let plan = build_launch(
            &powershell_launcher(),
            "claude",
            &owned(&["--a", "b c", "a\"b c"]),
        );
        assert_eq!(
            plan.argv[..4],
            owned(&[
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand"
            ])[..]
        );
        assert_eq!(plan.argv.len(), 5);
        assert_eq!(plan.paint_floor, 640);
        assert_eq!(plan.program, powershell_launcher().program);
    }

    #[test]
    fn powershell_payload_is_base64_only() {
        for args in [vec![], owned(&["--a", "b c", "a\"b c", "", "a\\b\\"])] {
            let plan = build_launch(&powershell_launcher(), "claude", &args);
            let payload = &plan.argv[4];
            assert!(!payload.is_empty());
            assert!(
                payload
                    .trim_end_matches('=')
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/'),
                "{payload}"
            );
            assert!(payload.len() - payload.trim_end_matches('=').len() <= 2);
        }
    }

    #[test]
    fn powershell_script_has_the_gate_before_the_branch_and_the_exit_after_it() {
        let plan = build_launch(&powershell_launcher(), "claude", &owned(&["--a"]));
        let script = decode_payload(&plan.argv[4]);

        assert!(script.starts_with("$LASTEXITCODE = $null\n"), "{script}");
        let gate = script
            .find("if ($null -eq (Get-Command 'claude' -ErrorAction SilentlyContinue)) { exit 1 }")
            .expect("Get-Command gate");
        let branch = script
            .find("if (Test-Path variable:PSNativeCommandArgumentPassing) {")
            .expect("capability branch");
        assert!(gate < branch, "the gate must run once, before both arms");
        assert!(script.contains("$PSNativeCommandArgumentPassing = 'Standard'"));
        assert!(script.contains("} else {"));
        assert!(
            script.ends_with("if ($null -eq $LASTEXITCODE) { exit 0 }\nexit $LASTEXITCODE"),
            "{script}"
        );
    }

    #[test]
    fn both_arms_are_built_for_the_same_input() {
        let plan = build_launch(
            &powershell_launcher(),
            "claude",
            &owned(&["--a", "b c", "a\"b c"]),
        );
        let script = decode_payload(&plan.argv[4]);

        assert!(
            script.contains("& 'claude' '--a' 'b c' 'a\"b c'"),
            "Standard arm missing: {script}"
        );
        assert!(
            script.contains("& 'claude' '\"--a\"' '\"b c\"' '\"a\"\"b c\"'"),
            "Legacy arm missing: {script}"
        );
    }

    #[test]
    fn both_arms_collapse_to_the_bare_command_with_no_arguments() {
        let plan = build_launch(&powershell_launcher(), "claude", &[]);
        let script = decode_payload(&plan.argv[4]);
        assert_eq!(script.matches("& 'claude'\n").count(), 2, "{script}");
    }

    #[test]
    fn payload_round_trips_through_utf16le_base64() {
        let plan = build_launch(&powershell_launcher(), "claude", &owned(&["a\"b c"]));
        let payload = &plan.argv[4];
        let reencoded = encode_command(&decode_payload(payload));
        assert_eq!(&reencoded, payload);
    }

    #[test]
    fn diagnostic_argv_keeps_every_token_raw_so_redaction_can_bite() {
        let args = owned(&["--a", "b c", "a\"b c", "", "sk-secret-value"]);
        let plan = build_launch(&powershell_launcher(), "claude", &args);

        assert!(!plan.diagnostic_argv.contains(&plan.argv[4]));
        assert!(plan
            .diagnostic_argv
            .iter()
            .any(|entry| entry.starts_with("<encoded payload elided:")));
        let separator = plan
            .diagnostic_argv
            .iter()
            .position(|entry| entry == "--")
            .expect("-- separator");
        assert_eq!(plan.diagnostic_argv[separator + 1], "claude");
        assert_eq!(&plan.diagnostic_argv[separator + 2..], &args[..]);
    }

    /// Property form of the same guarantee, over the whole fixture matrix: `scrub` matches
    /// by plain substring replacement, so an escaped token is a token it cannot redact.
    #[test]
    fn every_argument_appears_verbatim_in_diagnostic_argv() {
        let args: Vec<String> = fixture_matrix()
            .into_iter()
            .map(|token| token.to_string())
            .collect();
        let plan = build_launch(&powershell_launcher(), "claude", &args);
        let separator = plan
            .diagnostic_argv
            .iter()
            .position(|entry| entry == "--")
            .expect("-- separator");
        assert_eq!(&plan.diagnostic_argv[separator + 2..], &args[..]);
    }

    // --- is_direct_exe, moved verbatim ---

    #[test]
    fn is_direct_exe_is_a_string_test_not_a_resolution() {
        assert!(is_direct_exe("C:\\t\\agent.exe"));
        assert!(is_direct_exe("AGENT.EXE"));
        assert!(!is_direct_exe("claude"));
        assert!(!is_direct_exe("claude.cmd"));
    }

    // --- the restart side door (9.3) ---

    /// The codebase's single definition of "this launch belongs to an agent", copied from
    /// `create_session_inner_impl`. It is deliberately blind to whether the agent PROFILE
    /// still resolves.
    fn is_agent_owned_launch(
        agent_id: Option<&str>,
        agent_kind_detected: bool,
        is_root_agent: bool,
    ) -> bool {
        agent_id.is_some() || agent_kind_detected || is_root_agent
    }

    /// A restart whose agent profile no longer resolves (`resolved_spawn == None`) falls
    /// back to the STORED agent command. That session is still an agent session, and
    /// gating on `resolved_spawn.is_some()` instead would classify it as a bare terminal,
    /// put `cmd.exe` back in its process tree while Settings still says `powershell.exe`,
    /// and silently revert the fix. This pins the door shut.
    #[test]
    fn a_restart_with_an_unresolvable_profile_still_gets_the_launcher() {
        let resolved_spawn: Option<()> = None;
        let stored_agent_id = Some("agent_1782513272568_0");

        assert!(is_agent_owned_launch(stored_agent_id, true, false));
        assert!(
            resolved_spawn.is_none(),
            "the shape under test is the unresolvable-profile restart"
        );

        let decision = decide_launcher(
            true,
            true,
            is_agent_owned_launch(stored_agent_id, true, false),
            "powershell.exe",
            "claude",
            &probe_resolves,
        );
        assert_eq!(decision.launcher, Some(powershell_launcher()));

        // What the rejected gate would have produced, spelled out so the regression is
        // visible if anyone swaps the predicate back.
        let under_resolved_spawn_gate = decide_launcher(
            true,
            true,
            resolved_spawn.is_some(),
            "powershell.exe",
            "claude",
            &probe_resolves,
        );
        assert_eq!(under_resolved_spawn_gate.launcher, None);
    }
}

/// Section 9.2 - behavioural tests against the host's real shells.
///
/// `std::process::Command` rather than a PTY, so they are deterministic: every one of them
/// asserts on an exit code or on the argv a child actually received, neither of which
/// needs a console. They run against whatever PowerShell the host has, deliberately: that
/// is what makes this module the tripwire for a future PowerShell that drops
/// `$PSNativeCommandArgumentPassing` while keeping `Standard` semantics.
///
/// The argument-fidelity tests need a native argv printer and use `node`, which is what
/// the original measurements used. Where a tool is missing the test records why it
/// skipped rather than passing vacuously.
#[cfg(all(test, windows))]
mod windows_behaviour {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// The MSVCRT `ArgvQuote` algorithm, ported from
    /// `portable-pty-0.8.1/src/cmdbuilder.rs:653-698`. `CommandBuilder` applies exactly
    /// this to the program and to every argument before handing ONE command line to
    /// `CreateProcessW`, so a test that wants to reproduce a real AC spawn has to apply it
    /// too.
    ///
    /// WARNING, and this is the single most expensive mistake this change made once
    /// already: the `cmd.exe` BASELINE arm must be built with this and handed straight to
    /// `CreateProcessW`, with NO shell in between. Driving the baseline from PowerShell
    /// lets 5.1's own native binder mangle the tokens before `cmd.exe` ever sees them,
    /// which records 5.1's damage twice and blames `cmd.exe` for half of it. That produced
    /// a confident and completely wrong "no regression" verdict.
    fn append_quoted(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
            return arg.to_string();
        }
        let chars: Vec<char> = arg.chars().collect();
        let mut out = String::from("\"");
        let mut i = 0;
        while i < chars.len() {
            let mut backslashes = 0;
            while i < chars.len() && chars[i] == '\\' {
                backslashes += 1;
                i += 1;
            }
            if i == chars.len() {
                for _ in 0..(2 * backslashes) {
                    out.push('\\');
                }
                break;
            }
            if chars[i] == '"' {
                for _ in 0..(2 * backslashes + 1) {
                    out.push('\\');
                }
                out.push('"');
            } else {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                out.push(chars[i]);
            }
            i += 1;
        }
        out.push('"');
        out
    }

    struct Run {
        code: Option<i32>,
        stdout: String,
        bytes: usize,
    }

    /// One `CreateProcessW`, no shell interposed by the test itself. `raw_args` are
    /// appended to the command line verbatim, so the caller controls the quoting.
    fn run_raw(program: &str, raw_args: &[String], cwd: &Path, path: Option<&str>) -> Run {
        let mut command = Command::new(program);
        for arg in raw_args {
            command.raw_arg(arg);
        }
        command.current_dir(cwd);
        if let Some(path) = path {
            command.env("PATH", path);
        }
        let output = command.output().expect("spawn the launcher under test");
        Run {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            bytes: output.stdout.len() + output.stderr.len(),
        }
    }

    fn quoted(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| append_quoted(arg)).collect()
    }

    fn powershell() -> Option<ResolvedLauncher> {
        resolve_configured_shell_path("powershell.exe").map(|program| ResolvedLauncher {
            family: LauncherFamily::PowerShell,
            program,
        })
    }

    /// Run `cmd`/`args` through the real PowerShell launcher, quoting the plan's argv the
    /// way `CommandBuilder` would.
    fn run_launcher(
        launcher: &ResolvedLauncher,
        cmd: &str,
        args: &[&str],
        cwd: &Path,
        path: Option<&str>,
    ) -> Run {
        let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let plan = build_launch(launcher, cmd, &owned);
        let raw: Vec<String> = plan.argv.iter().map(|arg| append_quoted(arg)).collect();
        run_raw(&plan.program, &raw, cwd, path)
    }

    fn node() -> Option<PathBuf> {
        which::which("node").ok()
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write fixture");
        path
    }

    /// Prints the tokens it was given, so the test can compare what the child RECEIVED
    /// rather than what the command line looked like.
    fn argv_printer(dir: &Path) -> PathBuf {
        write(
            dir,
            "printargs.js",
            "process.stdout.write(JSON.stringify(process.argv.slice(2)));",
        )
    }

    fn parse_argv(stdout: &str) -> Option<Vec<String>> {
        serde_json::from_str::<Vec<String>>(stdout.trim()).ok()
    }

    fn prepend_path(dir: &Path) -> String {
        format!(
            "{};{}",
            dir.display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    // --- 30, 31: exit-code propagation ---

    #[test]
    fn exit_codes_propagate_through_the_powershell_launcher() {
        let Some(launcher) = powershell() else {
            eprintln!("SKIPPED: powershell.exe not resolvable on this host");
            return;
        };
        let Some(node) = node() else {
            eprintln!("SKIPPED: node not on PATH");
            return;
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let node = node.to_string_lossy().to_string();
        let exit42 = write(dir.path(), "exit42.js", "process.exit(42)");
        let exit0 = write(dir.path(), "exit0.js", "process.exit(0)");

        let run = run_launcher(
            &launcher,
            &node,
            &[&exit42.to_string_lossy()],
            dir.path(),
            None,
        );
        assert_eq!(run.code, Some(42));

        let run = run_launcher(
            &launcher,
            &node,
            &[&exit0.to_string_lossy()],
            dir.path(),
            None,
        );
        assert_eq!(run.code, Some(0));

        let run = run_launcher(&launcher, "ac-no-such-command-1271", &[], dir.path(), None);
        assert_ne!(run.code, Some(0), "a not-found command must fail the spawn");
    }

    /// Section 6.8 row 6 is a KNOWING deviation, pinned here so nobody "fixes" it by
    /// accident: a PowerShell-native target that fails without setting an exit code cannot
    /// be told apart from one that succeeds, and the `Get-Command` gate deliberately does
    /// not pretend otherwise. It is not reachable through a real npm or pnpm shim, whose
    /// template ends in an explicit `exit $ret`.
    #[test]
    fn a_ps1_that_returns_without_exit_reports_success_either_way() {
        let Some(launcher) = powershell() else {
            eprintln!("SKIPPED: powershell.exe not resolvable on this host");
            return;
        };
        let dir = tempfile::tempdir().expect("temp dir");
        write(dir.path(), "ac1271ok.ps1", "Write-Output 'ok'\n");
        write(
            dir.path(),
            "ac1271bad.ps1",
            "Write-Error 'boom' -ErrorAction Continue\n",
        );
        let path = prepend_path(dir.path());

        let run = run_launcher(&launcher, "ac1271ok", &[], dir.path(), Some(&path));
        assert_eq!(run.code, Some(0), "stdout={}", run.stdout);

        let run = run_launcher(&launcher, "ac1271bad", &[], dir.path(), Some(&path));
        assert_eq!(
            run.code,
            Some(0),
            "6.8 row 6: a failing .ps1 that never sets an exit code reads as success"
        );
    }

    // --- 32, 33: the argument differential ---

    /// The contract of 6.7, and the only assertion that stays objective: for every token,
    /// what the child observes under the configured launcher must equal what it observes
    /// under the `cmd.exe` baseline, or be better. NO ROW MAY REGRESS. It is differential
    /// rather than absolute because `cmd.exe` itself damages 7 of the 18 metacharacter
    /// tokens, so "everything survives" would fail on `main` and is not the right question.
    /// How many rows each arm damaged, so a caller that is pinning a property of the
    /// command SHAPE (test 32b) can assert on the counts as well. The no-regression rule
    /// is asserted here for every caller; the counts are evidence, never a hard-coded
    /// expectation, which is what keeps this objective on a host we did not measure on.
    struct Differential {
        rows: usize,
        baseline_damaged: usize,
        launcher_damaged: usize,
    }

    fn assert_no_row_regresses(
        label: &str,
        mut baseline: impl FnMut(&str) -> Run,
        mut launcher: impl FnMut(&str) -> Run,
    ) -> Differential {
        let mut report = String::new();
        let mut regressions = Vec::new();
        let mut summary = Differential {
            rows: 0,
            baseline_damaged: 0,
            launcher_damaged: 0,
        };
        for token in super::tests::fixture_matrix() {
            let want = vec![token.to_string()];
            let base = parse_argv(&baseline(token).stdout);
            let ps = parse_argv(&launcher(token).stdout);
            let base_ok = base.as_ref() == Some(&want);
            let ps_ok = ps.as_ref() == Some(&want);
            summary.rows += 1;
            summary.baseline_damaged += usize::from(!base_ok);
            summary.launcher_damaged += usize::from(!ps_ok);
            // The full observed argv, never just its first element: the residual class
            // fails by SPLITTING a token, which a first-element comparison hides.
            report.push_str(&format!(
                "  {token:?}\n    baseline={base:?} ok={base_ok}\n    launcher={ps:?} ok={ps_ok}\n"
            ));
            if base_ok && !ps_ok {
                regressions.push(format!("{token:?}: baseline={base:?} launcher={ps:?}"));
            }
        }
        eprintln!(
            "[#1271] argument differential ({label}): {} row(s), baseline damaged {}, launcher damaged {}\n{report}",
            summary.rows, summary.baseline_damaged, summary.launcher_damaged
        );
        assert!(
            regressions.is_empty(),
            "{label}: {} row(s) regressed against the cmd.exe baseline:\n{}",
            regressions.len(),
            regressions.join("\n")
        );
        summary
    }

    /// The command is the EXTENSIONLESS `node`, not an absolute path to it, because that
    /// is the shape AC really produces: `is_direct_exe` is a string test, so an agent is
    /// always launched by its bare name. It also keeps the baseline honest. `cmd /C` strips
    /// the outer pair of quotes off its command tail when that tail both starts and ends
    /// with one, so a quoted absolute program path plus a quoted final argument shreds the
    /// baseline for reasons that have nothing to do with the token under test.
    #[test]
    fn no_argument_row_regresses_against_the_cmd_baseline() {
        let Some(launcher) = powershell() else {
            eprintln!("SKIPPED: powershell.exe not resolvable on this host");
            return;
        };
        let Some(node) = node() else {
            eprintln!("SKIPPED: node not on PATH");
            return;
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let node_dir = node.parent().expect("node lives somewhere").to_path_buf();
        let script = argv_printer(dir.path()).to_string_lossy().to_string();
        let path = prepend_path(&node_dir);
        let cwd = dir.path().to_path_buf();

        assert_no_row_regresses(
            "native target",
            |token| {
                let args = quoted(&["/C", "node", &script, token]);
                run_raw("cmd.exe", &args, &cwd, Some(&path))
            },
            |token| run_launcher(&launcher, "node", &[&script, token], &cwd, Some(&path)),
        );
    }

    /// 32b. The same matrix again, with the agent command an ABSOLUTE PATH CONTAINING
    /// SPACES. `cmd /C` removes the first and the last quote of its whole command tail
    /// when that tail begins and ends with one (2.10, 15.5): `append_quoted` quotes a
    /// program path that contains a space, so the moment the final argument is quoted too,
    /// the outer pair goes and `cmd.exe` tries to run the first space-delimited chunk of
    /// the path. It is a property of the command SHAPE, not of any token, which is why no
    /// fixture built on a bare program name can see it, and why test 32 must never be
    /// "simplified" back into this one.
    ///
    /// This is a SECOND production defect the change fixes. A PowerShell-family launch
    /// never routes through `cmd.exe` and is not subject to the rule; a `Cmd`-family launch
    /// and every fallback keep it, deliberately, because that path is byte for byte
    /// unchanged by design (6.2).
    ///
    /// The target is `node` placed under a directory whose name contains a space, named
    /// `.com` rather than `.exe` on purpose: `is_direct_exe` is a string test, so an
    /// absolute `.exe` never reaches a launcher at all, and `.com` is therefore the shape
    /// in which AC really does interpose one on an absolute path. `CreateProcessW` loads
    /// the file from its PE header and does not care about the extension.
    #[test]
    fn the_absolute_path_with_spaces_shape_collapses_the_cmd_baseline_only() {
        let Some(launcher) = powershell() else {
            eprintln!("SKIPPED: powershell.exe not resolvable on this host");
            return;
        };
        let Some(node) = node() else {
            eprintln!("SKIPPED: node not on PATH");
            return;
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let spaced = dir.path().join("agent files");
        std::fs::create_dir_all(&spaced).expect("create a directory whose name has a space");
        let agent = spaced.join("ac1271agent.com");
        // A hard link keeps this cheap; a copy is the fallback across volumes.
        if std::fs::hard_link(&node, &agent).is_err() && std::fs::copy(&node, &agent).is_err() {
            eprintln!("SKIPPED: could not place a node image at a path containing spaces");
            return;
        }
        let agent = agent.to_string_lossy().to_string();
        assert!(
            agent.contains(' '),
            "the whole point of this fixture is the space: {agent:?}"
        );
        let script = argv_printer(dir.path()).to_string_lossy().to_string();
        let cwd = dir.path().to_path_buf();
        let path = prepend_path(&spaced);

        // Control, one spawn: the SAME target and the SAME token under test 32's shape,
        // where the program is a bare name and is therefore not quoted. The tail does not
        // end up wrapped and `a b` survives. So anything the spaced arm below loses is the
        // shape's doing, not the token's.
        let control = run_raw(
            "cmd.exe",
            &quoted(&["/C", "ac1271agent", &script, "a b"]),
            &cwd,
            Some(&path),
        );
        assert_eq!(
            parse_argv(&control.stdout),
            Some(vec!["a b".to_string()]),
            "control: the bare-name baseline must carry {:?} intact, stdout={:?}",
            "a b",
            control.stdout
        );

        // The defect itself, pinned deterministically rather than through a row count.
        let collapsed = run_raw(
            "cmd.exe",
            &quoted(&["/C", &agent, &script, "a b"]),
            &cwd,
            Some(&path),
        );
        assert_ne!(
            parse_argv(&collapsed.stdout),
            Some(vec!["a b".to_string()]),
            "cmd /C is expected to strip the outer quote pair of a tail that both starts \
             and ends with one, so this row must NOT survive the baseline; stdout={:?}",
            collapsed.stdout
        );

        let differential = assert_no_row_regresses(
            "absolute path with spaces",
            |token| {
                let args = quoted(&["/C", &agent, &script, token]);
                run_raw("cmd.exe", &args, &cwd, Some(&path))
            },
            |token| run_launcher(&launcher, &agent, &[&script, token], &cwd, Some(&path)),
        );

        // The launcher never routes through `cmd.exe`, so the tail rule cannot reach it and
        // the measured expectation of 6.7 is zero damage on this shape too.
        assert_eq!(
            differential.launcher_damaged, 0,
            "the PowerShell launcher must stay at 0 damaged rows on the absolute-path shape"
        );
        // Evidence, not an assertion: the plan measured the baseline degrading from 7 of 18
        // damaged on a bare program name to 13 of 18 here. A hard-coded cell value would
        // stop being objective on a host nobody measured on, so the count is reported and
        // the deterministic collapse above is what is asserted.
        assert!(
            differential.baseline_damaged > differential.launcher_damaged,
            "the cmd.exe baseline is expected to be strictly worse on this shape"
        );
    }

    /// The same matrix through an npm `cmd-shim`-shaped pair, which is what an
    /// npm-installed agent really is: `cmd.exe` selects the `.cmd`, the PowerShell
    /// launcher selects the `.ps1`. Both shims only forward their arguments to a native
    /// command, which is why the escaping composes through them.
    #[test]
    fn no_argument_row_regresses_through_an_npm_shim() {
        let Some(launcher) = powershell() else {
            eprintln!("SKIPPED: powershell.exe not resolvable on this host");
            return;
        };
        let Some(node) = node() else {
            eprintln!("SKIPPED: node not on PATH");
            return;
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let node = node.to_string_lossy().to_string();
        let script = argv_printer(dir.path()).to_string_lossy().to_string();
        write(
            dir.path(),
            "ac1271shim.cmd",
            &format!("@ECHO off\r\n\"{node}\" \"{script}\" %*\r\n"),
        );
        write(
            dir.path(),
            "ac1271shim.ps1",
            &format!("$ret=0\n& \"{node}\" \"{script}\" $args\n$ret=$LASTEXITCODE\nexit $ret\n"),
        );
        let path = prepend_path(dir.path());
        let cwd = dir.path().to_path_buf();

        assert_no_row_regresses(
            "npm cmd-shim",
            |token| {
                let args = quoted(&["/C", "ac1271shim", token]);
                run_raw("cmd.exe", &args, &cwd, Some(&path))
            },
            |token| run_launcher(&launcher, "ac1271shim", &[token], &cwd, Some(&path)),
        );
    }

    // --- 34: discovery order ---

    /// The capability the wrapper exists for, and the one real change of shim this fix
    /// makes: within one directory PowerShell prefers `.ps1` over every `PATHEXT` entry,
    /// including `.CMD` and including when `.PS1` is absent from `PATHEXT`.
    #[test]
    fn cmd_selects_the_cmd_shim_and_powershell_selects_the_ps1() {
        let Some(launcher) = powershell() else {
            eprintln!("SKIPPED: powershell.exe not resolvable on this host");
            return;
        };
        let dir = tempfile::tempdir().expect("temp dir");
        write(dir.path(), "ac1271probe.cmd", "@ECHO off\r\nECHO cmd\r\n");
        write(dir.path(), "ac1271probe.ps1", "Write-Output 'ps1'\n");
        let path = prepend_path(dir.path());
        let cwd = dir.path().to_path_buf();

        let baseline = run_raw(
            "cmd.exe",
            &quoted(&["/C", "ac1271probe"]),
            &cwd,
            Some(&path),
        );
        assert_eq!(baseline.stdout.trim(), "cmd");

        let through_launcher = run_launcher(&launcher, "ac1271probe", &[], &cwd, Some(&path));
        assert_eq!(through_launcher.stdout.trim(), "ps1");
    }

    // --- 36: the #942 stall detector still bites ---

    /// Regression guard for the threshold this change would otherwise break. Through pipes
    /// rather than a ConPTY, so the numbers here exclude ConPTY's own preamble; the
    /// deterministic half of this criterion is
    /// `spawn_diagnostics::powershell_not_found_is_still_stalled_at_the_launcher_floor`,
    /// which pins the floor against the measured banner sizes directly.
    #[test]
    fn a_not_found_command_stays_below_the_powershell_paint_floor() {
        let Some(launcher) = powershell() else {
            eprintln!("SKIPPED: powershell.exe not resolvable on this host");
            return;
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let run = run_launcher(&launcher, "ac-no-such-command-1271", &[], dir.path(), None);
        eprintln!(
            "[#1271] not-found under the PowerShell launcher: exit={:?} bytes={} floor={}",
            run.code, run.bytes, POWERSHELL_PAINT_FLOOR_BYTES
        );
        assert_ne!(run.code, Some(0));
        assert!(
            (run.bytes as u64) < POWERSHELL_PAINT_FLOOR_BYTES,
            "a not-found agent command must still read as a startup stall, got {} bytes",
            run.bytes
        );
    }
}
