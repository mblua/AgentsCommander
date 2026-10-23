use super::CodingAgentKind;

/// Coding-agent launch kinds that are safe for privileged exact PTY input.
/// Cursor uses the executable `agent` and therefore has no legacy
/// `CodingAgentKind` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtySubmissionAgent {
    Claude,
    Codex,
    Antigravity,
    CursorAgent,
}

impl PtySubmissionAgent {
    fn from_executable(token: &str, configured_wrapper: bool) -> Option<Self> {
        let trimmed = token.trim().trim_matches('"');
        let basename = trimmed
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(trimmed)
            .to_ascii_lowercase();
        let stem = basename
            .strip_suffix(".exe")
            .or_else(|| basename.strip_suffix(".cmd"))
            .or_else(|| basename.strip_suffix(".bat"))
            .unwrap_or(&basename);
        if stem == "claude" || (configured_wrapper && stem.starts_with("claude")) {
            Some(Self::Claude)
        } else if stem == "codex" || (configured_wrapper && stem.starts_with("codex")) {
            Some(Self::Codex)
        } else if stem == "agy" || stem == "antigravity" {
            // Exact stems only — no `configured_wrapper` prefix allowance for
            // Antigravity (plan #1482 §5.1).
            Some(Self::Antigravity)
        } else if stem == "agent" {
            Some(Self::CursorAgent)
        } else {
            None
        }
    }

    fn agrees_with_hint(self, hint: Option<CodingAgentKind>) -> bool {
        matches!(
            (self, hint),
            (Self::Claude, Some(CodingAgentKind::Claude))
                | (Self::Codex, Some(CodingAgentKind::Codex))
                | (Self::Antigravity, Some(CodingAgentKind::Antigravity))
                | (Self::CursorAgent, None)
                | (_, None)
        )
    }
}

/// Prove a trusted coding-agent executable at the executable position.
///
/// Direct executables and conservative `cmd.exe /C` wrappers are accepted.
/// Shell evaluators, `cmd /K`, expansion, control operators, and mere agent
/// mentions in arbitrary arguments are rejected.
pub(super) fn detect_pty_submission_agent_with_provenance(
    shell: &str,
    args: &[String],
    hint: Option<CodingAgentKind>,
    configured_wrapper: bool,
) -> Option<PtySubmissionAgent> {
    let shell_name = shell
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell.trim())
        .to_ascii_lowercase();
    let shell_stem = shell_name.strip_suffix(".exe").unwrap_or(&shell_name);

    let detected = if shell_stem == "cmd" {
        let mode = args.first()?;
        if !mode.eq_ignore_ascii_case("/c") || args.len() < 2 {
            return None;
        }
        let command = args[1..].join(" ");
        if command.chars().any(|c| {
            matches!(
                c,
                '\r' | '\n' | '&' | '|' | '<' | '>' | '(' | ')' | '^' | '%' | '!'
            )
        }) {
            return None;
        }
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut quoted = false;
        for ch in command.chars() {
            match ch {
                '"' => quoted = !quoted,
                c if c.is_whitespace() && !quoted => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            }
        }
        if quoted {
            return None;
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        let executable = tokens.first()?;
        if executable.eq_ignore_ascii_case("call") || executable.eq_ignore_ascii_case("start") {
            return None;
        }
        PtySubmissionAgent::from_executable(executable, configured_wrapper)?
    } else {
        // PowerShell, bash, sh, and other evaluators are not accepted merely
        // because an argument names an agent.
        PtySubmissionAgent::from_executable(shell, configured_wrapper)?
    };

    detected.agrees_with_hint(hint).then_some(detected)
}

/// Prove an exact built-in coding-agent executable at executable position.
/// Prefix wrappers are never accepted without retained configured-spawn proof.
pub fn detect_pty_submission_agent(
    shell: &str,
    args: &[String],
    hint: Option<CodingAgentKind>,
) -> Option<PtySubmissionAgent> {
    detect_pty_submission_agent_with_provenance(shell, args, hint, false)
}

/// Prove a wrapper that was resolved from the verified configured-spawn path.
/// Callers must retain that provenance and compare the current spawn recipe.
pub(crate) fn detect_configured_pty_submission_agent(
    shell: &str,
    args: &[String],
    hint: Option<CodingAgentKind>,
) -> Option<PtySubmissionAgent> {
    detect_pty_submission_agent_with_provenance(shell, args, hint, true)
}
