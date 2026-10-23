//! Per-coding-agent profile — the single source of truth for behavior that
//! varies by coding agent (Claude Code, Codex CLI, Antigravity CLI, Pi Coding Agent).
//!
//! Before #260 this knowledge was scattered: three `is_claude`/`is_codex`/
//! `is_gemini` bools on `Session`/`SessionInfo` (#258), a duplicated
//! `starts_with` detector in `create_session_inner` and
//! `strip_auto_injected_args`, the `derive_reader` bool triple, and
//! hard-coded idle-detector thresholds. `CodingAgentProfile` consolidates it.
//!
//! Design (see _plans/260-coding-agent-profile.md §2): plain `Copy` data +
//! `const` lookup, not a trait — the agent set is small and closed and only
//! data varies, so a struct beats a `dyn` object (no vtables, no allocation,
//! usable in `const` context, exhaustive `match` on `CodingAgentKind`).

use std::ops::Range;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Identity of a coding agent. `Option<CodingAgentKind>` on a session: `None`
/// means "not a recognised coding agent" (a plain shell).
///
/// Mutual exclusion is **structural** — a session is exactly one kind or none.
/// This enum is what let #260 delete the `debug_assert!` that guarded the old
/// three-bool representation in `derive_reader`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentKind {
    Claude,
    Codex,
    Antigravity,
    Pi,
    /// #1873 - Muse Code. Detected only as a direct shell whose exact stem is
    /// `muse` (bare or absolute path); prefix names and argument-only identity
    /// inside `env`/shell/PowerShell/`cmd`/compound wrappers are never Muse.
    Muse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PiCommandLocation {
    pub(crate) option_tokens: Vec<String>,
    pub(crate) insertion: PiInsertionPoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PiInsertionPoint {
    Arg {
        index: usize,
    },
    CmdText {
        arg_index: usize,
        executable_range: Range<usize>,
        segment_range: Range<usize>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PiCommandParseError {
    MalformedCmdSyntax,
    UnsupportedPiCommandShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DecodedCmdAtom {
    text: String,
    has_unescaped_amp_or_pipe: bool,
    control_chunks: Vec<String>,
    unescaped_controls: Vec<char>,
    has_unescaped_control: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PiHeadClass {
    Exact,
    Unsupported,
    Other,
}

fn executable_leaf(value: &str) -> &str {
    value.rsplit(['/', '\\']).next().unwrap_or(value)
}

fn is_exact_pi_executable(value: &str) -> bool {
    matches!(
        executable_leaf(value).to_ascii_lowercase().as_str(),
        "pi" | "pi.exe" | "pi.cmd"
    )
}

/// #1873 - true only for a direct shell whose exact, case-sensitive file stem is
/// `muse` (`muse`, `/opt/muse/bin/muse`, `muse.exe`). Prefix names (`muse-agent`,
/// `musee`) and mixed case (`Muse`) are rejected; arguments are never inspected.
fn is_exact_muse_executable(shell: &str) -> bool {
    std::path::Path::new(shell)
        .file_stem()
        .and_then(|stem| stem.to_str())
        == Some("muse")
}

fn is_reserved_pi_executable(value: &str) -> bool {
    let leaf = executable_leaf(value).to_ascii_lowercase();
    leaf.starts_with("pi.") && !matches!(leaf.as_str(), "pi.exe" | "pi.cmd")
}

fn is_cmd_executable(value: &str) -> bool {
    matches!(
        executable_leaf(value).to_ascii_lowercase().as_str(),
        "cmd" | "cmd.exe"
    )
}

fn is_unsupported_wrapper(value: &str) -> bool {
    matches!(
        executable_leaf(value).to_ascii_lowercase().as_str(),
        "call"
            | "call.exe"
            | "call.cmd"
            | "start"
            | "start.exe"
            | "start.cmd"
            | "npx"
            | "npx.exe"
            | "npx.cmd"
    )
}

fn decode_cmd_atom(raw: &str) -> Result<DecodedCmdAtom, PiCommandParseError> {
    if raw.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n')) {
        return Err(PiCommandParseError::MalformedCmdSyntax);
    }

    let mut text = String::with_capacity(raw.len());
    let mut current_chunk = String::new();
    let mut control_chunks = Vec::new();
    let mut unescaped_controls = Vec::new();
    let mut in_quotes = false;
    let mut has_unescaped_amp_or_pipe = false;
    let mut has_unescaped_control = false;
    let mut chars = raw.chars();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                in_quotes = false;
            } else {
                text.push(ch);
                current_chunk.push(ch);
            }
            continue;
        }

        match ch {
            '"' => in_quotes = true,
            '^' => {
                let Some(escaped) = chars.next() else {
                    return Err(PiCommandParseError::MalformedCmdSyntax);
                };
                text.push(escaped);
                current_chunk.push(escaped);
            }
            '<' | '>' | '(' | ')' | '&' | '|' => {
                text.push(ch);
                control_chunks.push(std::mem::take(&mut current_chunk));
                unescaped_controls.push(ch);
                has_unescaped_control = true;
                has_unescaped_amp_or_pipe |= matches!(ch, '&' | '|');
            }
            _ => {
                text.push(ch);
                current_chunk.push(ch);
            }
        }
    }

    if in_quotes {
        return Err(PiCommandParseError::MalformedCmdSyntax);
    }
    control_chunks.push(current_chunk);

    Ok(DecodedCmdAtom {
        text,
        has_unescaped_amp_or_pipe,
        control_chunks,
        unescaped_controls,
        has_unescaped_control,
    })
}

fn classify_pi_head(atom: &DecodedCmdAtom) -> PiHeadClass {
    if is_exact_pi_executable(&atom.text) {
        return PiHeadClass::Exact;
    }
    if is_reserved_pi_executable(&atom.text) {
        return PiHeadClass::Unsupported;
    }
    if atom.has_unescaped_control {
        if let Some(chunk) = atom.control_chunks.iter().find(|chunk| !chunk.is_empty()) {
            if is_exact_pi_executable(chunk) || is_reserved_pi_executable(chunk) {
                return PiHeadClass::Unsupported;
            }
        }
    }
    PiHeadClass::Other
}

fn is_group_prefix(atom: &DecodedCmdAtom) -> bool {
    atom.has_unescaped_control && !atom.text.is_empty() && atom.text.chars().all(|ch| ch == '(')
}

fn segment_has_unsupported_pi_position(atoms: &[DecodedCmdAtom]) -> bool {
    let mut index = 0;
    let mut grouped = false;
    while atoms.get(index).is_some_and(is_group_prefix) {
        grouped = true;
        index += 1;
    }

    let Some(head) = atoms.get(index) else {
        return false;
    };
    if !matches!(classify_pi_head(head), PiHeadClass::Other) {
        return grouped || index == 0;
    }
    if !is_unsupported_wrapper(&head.text) {
        return false;
    }

    index += 1;
    while atoms.get(index).is_some_and(is_group_prefix) {
        index += 1;
    }
    atoms
        .get(index)
        .is_some_and(|atom| !matches!(classify_pi_head(atom), PiHeadClass::Other))
}

fn is_standalone_cmd_separator(raw: &str) -> bool {
    matches!(raw, "&&" | "&" | "||" | "|")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenizedCommandPosition {
    Head,
    WrapperArgument,
    Arguments,
}

fn tokenized_has_unsupported_pi_position(atoms: &[DecodedCmdAtom]) -> bool {
    let mut position = TokenizedCommandPosition::Head;
    let mut position_can_carry = true;

    for atom in atoms {
        let mut saw_syntax = false;
        for (index, chunk) in atom.control_chunks.iter().enumerate() {
            if !chunk.is_empty() {
                saw_syntax = true;
                match position {
                    TokenizedCommandPosition::Head => {
                        if is_exact_pi_executable(chunk) || is_reserved_pi_executable(chunk) {
                            return true;
                        }
                        if position_can_carry && is_unsupported_wrapper(chunk) {
                            position = TokenizedCommandPosition::WrapperArgument;
                        } else {
                            position = TokenizedCommandPosition::Arguments;
                            position_can_carry = false;
                        }
                    }
                    TokenizedCommandPosition::WrapperArgument => {
                        if position_can_carry
                            && (is_exact_pi_executable(chunk) || is_reserved_pi_executable(chunk))
                        {
                            return true;
                        }
                        position = TokenizedCommandPosition::Arguments;
                        position_can_carry = false;
                    }
                    TokenizedCommandPosition::Arguments => {
                        position_can_carry = false;
                    }
                }
            }

            if let Some(control) = atom.unescaped_controls.get(index) {
                saw_syntax = true;
                match control {
                    '&' | '|' => {
                        position = TokenizedCommandPosition::Head;
                        position_can_carry = true;
                    }
                    '<' | '>' | ')' => position_can_carry = false,
                    '(' => {}
                    _ => position_can_carry = false,
                }
            }
        }

        if !saw_syntax {
            position_can_carry = false;
        }
        // Candidate state crosses an argv boundary only when the suffix since
        // the last amp/pipe boundary is made of opening groups and, for the
        // wrapper state, one listed wrapper. Redirection, closing groups,
        // other literal chunks, and empty atoms consume that state.
        if matches!(
            position,
            TokenizedCommandPosition::Head | TokenizedCommandPosition::WrapperArgument
        ) && !position_can_carry
        {
            position = TokenizedCommandPosition::Arguments;
        }
    }

    false
}

fn locate_tokenized_pi_command(
    command_args: &[String],
    first_arg_index: usize,
) -> Result<Option<PiCommandLocation>, PiCommandParseError> {
    let decoded = command_args
        .iter()
        .map(|raw| decode_cmd_atom(raw))
        .collect::<Result<Vec<_>, _>>()?;

    let mut segments = Vec::new();
    let mut start = 0;
    for (index, raw) in command_args.iter().enumerate() {
        if is_standalone_cmd_separator(raw) {
            segments.push(start..index);
            start = index + 1;
        }
    }
    segments.push(start..command_args.len());

    let first = segments.first().cloned().unwrap_or(0..0);
    let Some(head) = decoded.get(first.start).filter(|_| first.start < first.end) else {
        return if tokenized_has_unsupported_pi_position(&decoded) {
            Err(PiCommandParseError::UnsupportedPiCommandShape)
        } else {
            Ok(None)
        };
    };

    match classify_pi_head(head) {
        PiHeadClass::Exact => {
            if decoded[first.start + 1..first.end]
                .iter()
                .any(|atom| atom.has_unescaped_amp_or_pipe)
            {
                return Err(PiCommandParseError::UnsupportedPiCommandShape);
            }
            Ok(Some(PiCommandLocation {
                option_tokens: decoded[first.start + 1..first.end]
                    .iter()
                    .map(|atom| atom.text.clone())
                    .collect(),
                insertion: PiInsertionPoint::Arg {
                    index: first_arg_index + 1,
                },
            }))
        }
        PiHeadClass::Unsupported => Err(PiCommandParseError::UnsupportedPiCommandShape),
        PiHeadClass::Other => {
            if tokenized_has_unsupported_pi_position(&decoded) {
                Err(PiCommandParseError::UnsupportedPiCommandShape)
            } else {
                Ok(None)
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CmdTextToken {
    raw_range: Range<usize>,
    atom: DecodedCmdAtom,
}

#[derive(Debug, Clone)]
struct CmdTextSegment {
    tokens: Vec<CmdTextToken>,
    end: usize,
}

#[derive(Default)]
struct CmdTextAtomBuilder {
    raw_start: Option<usize>,
    text: String,
    current_chunk: String,
    control_chunks: Vec<String>,
    unescaped_controls: Vec<char>,
    has_unescaped_control: bool,
}

impl CmdTextAtomBuilder {
    fn start(&mut self, index: usize) {
        self.raw_start.get_or_insert(index);
    }

    fn push_literal(&mut self, ch: char) {
        self.text.push(ch);
        self.current_chunk.push(ch);
    }

    fn push_control(&mut self, ch: char) {
        self.text.push(ch);
        self.control_chunks
            .push(std::mem::take(&mut self.current_chunk));
        self.unescaped_controls.push(ch);
        self.has_unescaped_control = true;
    }

    fn finish(&mut self, end: usize) -> Option<CmdTextToken> {
        let start = self.raw_start.take()?;
        self.control_chunks
            .push(std::mem::take(&mut self.current_chunk));
        let token = CmdTextToken {
            raw_range: start..end,
            atom: DecodedCmdAtom {
                text: std::mem::take(&mut self.text),
                has_unescaped_amp_or_pipe: false,
                control_chunks: std::mem::take(&mut self.control_chunks),
                unescaped_controls: std::mem::take(&mut self.unescaped_controls),
                has_unescaped_control: std::mem::take(&mut self.has_unescaped_control),
            },
        };
        Some(token)
    }
}

fn lex_cmd_text(text: &str) -> Result<Vec<CmdTextSegment>, PiCommandParseError> {
    if text.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n')) {
        return Err(PiCommandParseError::MalformedCmdSyntax);
    }

    let mut segments = Vec::new();
    let mut tokens = Vec::new();
    let mut builder = CmdTextAtomBuilder::default();
    let mut in_quotes = false;
    let mut chars = text.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        if in_quotes {
            if ch == '"' {
                in_quotes = false;
            } else {
                builder.push_literal(ch);
            }
            continue;
        }

        match ch {
            '"' => {
                builder.start(index);
                in_quotes = true;
            }
            '^' => {
                builder.start(index);
                let Some((_, escaped)) = chars.next() else {
                    return Err(PiCommandParseError::MalformedCmdSyntax);
                };
                builder.push_literal(escaped);
            }
            ' ' | '\t' => {
                if let Some(token) = builder.finish(index) {
                    tokens.push(token);
                }
            }
            '&' | '|' => {
                if let Some(token) = builder.finish(index) {
                    tokens.push(token);
                }
                segments.push(CmdTextSegment {
                    tokens: std::mem::take(&mut tokens),
                    end: index,
                });
                if chars.peek().is_some_and(|(_, next)| *next == ch) {
                    chars.next();
                }
            }
            '<' | '>' | '(' | ')' => {
                builder.start(index);
                builder.push_control(ch);
            }
            _ => {
                builder.start(index);
                builder.push_literal(ch);
            }
        }
    }

    if in_quotes {
        return Err(PiCommandParseError::MalformedCmdSyntax);
    }
    if let Some(token) = builder.finish(text.len()) {
        tokens.push(token);
    }
    segments.push(CmdTextSegment {
        tokens,
        end: text.len(),
    });
    Ok(segments)
}

fn text_segment_has_unsupported_pi_position(tokens: &[CmdTextToken]) -> bool {
    let atoms = tokens
        .iter()
        .map(|token| token.atom.clone())
        .collect::<Vec<_>>();
    segment_has_unsupported_pi_position(&atoms)
}

fn locate_embedded_pi_command(
    text: &str,
    arg_index: usize,
) -> Result<Option<PiCommandLocation>, PiCommandParseError> {
    let segments = lex_cmd_text(text)?;
    let Some(first_segment) = segments.first() else {
        return Ok(None);
    };
    let Some(head) = first_segment.tokens.first() else {
        if segments
            .iter()
            .skip(1)
            .any(|segment| text_segment_has_unsupported_pi_position(&segment.tokens))
        {
            return Err(PiCommandParseError::UnsupportedPiCommandShape);
        }
        return Ok(None);
    };

    match classify_pi_head(&head.atom) {
        PiHeadClass::Exact => Ok(Some(PiCommandLocation {
            option_tokens: first_segment.tokens[1..]
                .iter()
                .map(|token| token.atom.text.clone())
                .collect(),
            insertion: PiInsertionPoint::CmdText {
                arg_index,
                executable_range: head.raw_range.clone(),
                segment_range: head.raw_range.start..first_segment.end,
            },
        })),
        PiHeadClass::Unsupported => Err(PiCommandParseError::UnsupportedPiCommandShape),
        PiHeadClass::Other => {
            if text_segment_has_unsupported_pi_position(&first_segment.tokens)
                || segments
                    .iter()
                    .skip(1)
                    .any(|segment| text_segment_has_unsupported_pi_position(&segment.tokens))
            {
                Err(PiCommandParseError::UnsupportedPiCommandShape)
            } else {
                Ok(None)
            }
        }
    }
}

fn locate_cmd_command_args(
    command_args: &[String],
    first_arg_index: usize,
) -> Result<Option<PiCommandLocation>, PiCommandParseError> {
    if command_args.is_empty() {
        return Ok(None);
    }

    let tokenized = if command_args.len() > 1 {
        true
    } else {
        let atom = decode_cmd_atom(&command_args[0])?;
        classify_pi_head(&atom) == PiHeadClass::Exact && !atom.has_unescaped_amp_or_pipe
    };

    if tokenized {
        locate_tokenized_pi_command(command_args, first_arg_index)
    } else {
        locate_embedded_pi_command(&command_args[0], first_arg_index)
    }
}

pub(crate) fn locate_pi_command(
    shell: &str,
    args: &[String],
) -> Result<Option<PiCommandLocation>, PiCommandParseError> {
    if is_exact_pi_executable(shell) {
        return Ok(Some(PiCommandLocation {
            option_tokens: args.to_vec(),
            insertion: PiInsertionPoint::Arg { index: 0 },
        }));
    }
    if is_reserved_pi_executable(shell) {
        return Err(PiCommandParseError::UnsupportedPiCommandShape);
    }

    if is_unsupported_wrapper(shell)
        && args
            .first()
            .is_some_and(|arg| is_exact_pi_executable(arg) || is_reserved_pi_executable(arg))
    {
        return Err(PiCommandParseError::UnsupportedPiCommandShape);
    }

    if !is_cmd_executable(shell) {
        return Ok(None);
    }

    if args
        .first()
        .is_some_and(|arg| arg.eq_ignore_ascii_case("/C") || arg.eq_ignore_ascii_case("/K"))
    {
        return locate_cmd_command_args(&args[1..], 1);
    }

    if let Some(switch_index) = args
        .iter()
        .position(|arg| arg.eq_ignore_ascii_case("/C") || arg.eq_ignore_ascii_case("/K"))
    {
        let located = locate_cmd_command_args(&args[switch_index + 1..], switch_index + 1)?;
        if located.is_some() {
            return Err(PiCommandParseError::UnsupportedPiCommandShape);
        }
    }

    Ok(None)
}

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
fn detect_pty_submission_agent_with_provenance(
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

impl CodingAgentKind {
    /// Detect the coding agent from a spawn command (`shell` + `args`).
    ///
    /// First applies Pi's exact command-position parser. A supported Pi head
    /// wins over provider-looking option values, while malformed or reserved
    /// unsupported Pi shapes fail closed. Only a genuine non-Pi result reaches
    /// the legacy whitespace/basename prefix scan with precedence
    /// **Claude > Codex > Antigravity**. Claude and Codex keep their legacy
    /// prefix match for wrappers such as `claude-mb` and `codex-foo`;
    /// Antigravity matches the exact stems `agy` | `antigravity` on the
    /// already-stemmed lowercase basename (`agy.exe`/`agy.cmd`/`agy.bat`/
    /// `antigravity.exe` collapse via `file_stem`); prefix wrappers are not
    /// inferred. A user-defined `gemini*` command now gets generic behavior.
    ///
    /// THIS IS THE detector. `create_session_inner` (which stamps
    /// `Session::agent_kind`) and `strip_auto_injected_args` both call it, so
    /// the persisted recipe and the runtime identity can never disagree.
    pub fn detect(shell: &str, args: &[String]) -> Option<CodingAgentKind> {
        match locate_pi_command(shell, args) {
            Ok(Some(_)) => return Some(CodingAgentKind::Pi),
            Ok(None) => {}
            Err(_) => return None,
        }

        // #1873 - Muse is a direct-shell-only identity and outranks the legacy
        // token scans below, so a Codex-looking argument value such as
        // `muse --workspace /tmp/codex` stays Muse. Wrappers (`env muse`,
        // `cmd /C muse`) never reach here as Muse: the shell is the wrapper.
        if is_exact_muse_executable(shell) {
            return Some(CodingAgentKind::Muse);
        }

        // Mirror of `crate::commands::session::executable_basename`
        // (`session.rs:1506`, identical body). Deliberately NOT shared:
        // importing it would invert the dependency direction — the `session`
        // domain module would depend on the `commands` (IPC) layer (§2 D2).
        // ~6 trivial lines; do not "consolidate" into a layering violation
        // (dev-rust R1.4 #3).
        fn basename(token: &str) -> String {
            std::path::Path::new(token)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(token)
                .to_lowercase()
        }
        let basenames: Vec<String> = std::iter::once(shell.to_string())
            .chain(
                args.iter()
                    .flat_map(|a| a.split_whitespace().map(str::to_string)),
            )
            .map(|t| basename(&t))
            .collect();
        // Precedence claude > codex > antigravity, scanning every token.
        if basenames.iter().any(|b| b.starts_with("claude")) {
            Some(CodingAgentKind::Claude)
        } else if basenames.iter().any(|b| b.starts_with("codex")) {
            Some(CodingAgentKind::Codex)
        } else if basenames
            .iter()
            .any(|b| matches!(b.as_str(), "agy" | "antigravity"))
        {
            Some(CodingAgentKind::Antigravity)
        } else {
            None
        }
    }

    /// Stable lowercase name of the CLI, for logs and for keying diagnostics by the
    /// coding agent that is actually running (#942). Not the configured profile id.
    pub const fn as_str(self) -> &'static str {
        match self {
            CodingAgentKind::Claude => "claude",
            CodingAgentKind::Codex => "codex",
            CodingAgentKind::Antigravity => "agy",
            CodingAgentKind::Pi => "pi",
            CodingAgentKind::Muse => "muse",
        }
    }

    /// Resolve the full behavior profile for this kind.
    pub const fn profile(self) -> CodingAgentProfile {
        match self {
            CodingAgentKind::Claude => CLAUDE_PROFILE,
            CodingAgentKind::Codex => CODEX_PROFILE,
            CodingAgentKind::Antigravity => ANTIGRAVITY_PROFILE,
            CodingAgentKind::Pi => PI_PROFILE,
            CodingAgentKind::Muse => MUSE_PROFILE,
        }
    }
}

/// #2879 - which activity detector arbitrates a session's busy state. `Timed` is the
/// pre-#2879 silence clock alone; `CodexWorking` additionally keeps the session busy while
/// Codex's complete `Working (… • esc to interrupt)` indicator is visible. Selected per
/// profile only: reverting `CODEX_PROFILE` to `Timed` reverts the feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityDetectorKind {
    Timed,
    CodexWorking,
}

/// Per-session tuning for the PTY idle detector. Resolved from the session's
/// `CodingAgentProfile` (or `DEFAULT` for a plain shell) and handed to
/// `IdleDetector::register_session` at PTY spawn time.
///
/// Invariant: `resize_grace >= idle_threshold` — a resize repaint must not be
/// able to trigger a false busy→idle transition. `register_session`
/// `debug_assert!`s it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleTuning {
    /// PTY silence after which a session is reported idle / waiting-for-input.
    pub idle_threshold: Duration,
    /// Grace window after a resize during which PTY output is treated as
    /// prompt-repaint noise and does NOT reset the idle timer.
    pub resize_grace: Duration,
    /// #260 BUG FIX. When `true`, `IdleDetector::register_session` seeds
    /// `activity[id] = Instant::now()` at PTY spawn. Without this seed, a
    /// session whose entire visible output is suppressed (resize grace) or
    /// escape-only (SKIPPED) is never inserted into the detector's `activity`
    /// map, so the watcher thread — which only iterates `activity` — never
    /// evaluates it and `mark_idle` never fires. See plan §1.
    pub seed_initial_activity: bool,
    /// #2124 - byte total at which a pending output burst is confirmed as real
    /// work. `0` (or a zero `burst_window`) disables the filter for this
    /// session and restores the pre-#2124 behavior exactly. #2156 - ON by
    /// default for a recognised coding agent ([`IdleTuning::AGENT_DEFAULT`]);
    /// OFF for a plain shell ([`IdleTuning::DEFAULT`]).
    pub burst_max_bytes: u64,
    /// #2124 - longest gap a pending burst may keep spanning. A burst still
    /// unconfirmed `burst_window` after its first chunk is discarded.
    pub burst_window: Duration,
    /// #2124 - silence age a chunk must find before it may OPEN a burst
    /// candidate. Shorter silences count immediately, so input and message
    /// delivery still register right away.
    pub burst_prior_silence: Duration,
    /// #2879 - the activity detector for this session. Launch overrides only touch the burst
    /// fields, so this selector always survives `idle_tuning_for_launch`.
    pub(crate) activity_detector: ActivityDetectorKind,
}

/// #2124 defaults for an `idleBurst` object that omits a subfield.
const IDLE_BURST_DEFAULT_MAX_BYTES: u64 = 1024;
const IDLE_BURST_DEFAULT_MAX_SECS: f64 = 3.0;
const IDLE_BURST_DEFAULT_PRIOR_SILENCE_SECS: f64 = 60.0;
/// Caps that keep `Duration::from_secs_f64` panic-free for absurd catalog values.
const IDLE_BURST_MAX_WINDOW_SECS: f64 = 600.0;
const IDLE_BURST_MAX_PRIOR_SILENCE_SECS: f64 = 86_400.0;

impl IdleTuning {
    /// Tuning for a plain shell / unrecognised agent (`idle_tuning_for(None)`).
    /// Also the per-field fallback when a session id is missing from the
    /// detector's tuning map. Values are identical to the pre-#260
    /// `idle_detector.rs` constants, with the #2124 burst filter disabled.
    /// A recognised coding agent uses [`IdleTuning::AGENT_DEFAULT`] instead.
    pub const DEFAULT: IdleTuning = IdleTuning {
        idle_threshold: Duration::from_millis(2500),
        resize_grace: Duration::from_millis(3000),
        seed_initial_activity: true,
        burst_max_bytes: 0,
        burst_window: Duration::ZERO,
        burst_prior_silence: Duration::ZERO,
        activity_detector: ActivityDetectorKind::Timed,
    };

    /// #2156 - tuning for a recognised coding agent: `DEFAULT` plus the #2124
    /// burst filter ON by default. Values mirror the IDLE_BURST_DEFAULT_*
    /// constants (a test pins the two in sync).
    pub const AGENT_DEFAULT: IdleTuning = IdleTuning {
        burst_max_bytes: IDLE_BURST_DEFAULT_MAX_BYTES, // 1024
        burst_window: Duration::from_millis(3_000),    // IDLE_BURST_DEFAULT_MAX_SECS
        burst_prior_silence: Duration::from_secs(60),  // IDLE_BURST_DEFAULT_PRIOR_SILENCE_SECS
        ..IdleTuning::DEFAULT
    };
}

/// #930 - per-coding-agent host credential source for container copy-in.
/// Static data only (all `&'static str`), so it is usable in the `const`
/// profile table and carries no host-specific paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerCredentialSource {
    /// Default host config dir, relative to the user's home dir, e.g. ".claude".
    pub host_dir: &'static str,
    /// Optional env var whose value (an absolute path) overrides `host_dir` on
    /// the host, e.g. "CLAUDE_CONFIG_DIR" for Claude. Only the AgentsCommander
    /// process environment is consulted (NOT wrapper-script parsing).
    pub host_dir_env: Option<&'static str>,
    /// Credential filename within the config dir, e.g. ".credentials.json".
    pub file: &'static str,
    /// Container-side config dir relative to the bind-mount root (`host_root`),
    /// e.g. ".claude". Where the file is copied so the container reads it.
    pub container_dir: &'static str,
    /// #930 - first-run state stamped next to the copied credential so the
    /// agent's interactive TUI actually USES it instead of running its
    /// onboarding wizard. None = nothing is stamped for this agent.
    pub first_run: Option<ContainerFirstRunState>,
}

/// #930 - container-side first-run state a coding agent needs before its
/// interactive TUI will use a copied credential. Verified in a real container:
/// Claude Code gates its onboarding wizard on `hasCompletedOnboarding` in
/// `$CLAUDE_CONFIG_DIR/.claude.json`, and the folder-trust dialog on
/// `projects[<cwd>].hasTrustDialogAccepted`, checking NEITHER against the
/// credential file. So a valid copied token still lands on "Select login
/// method" unless these flags are set. Static data only (all `&'static str`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerFirstRunState {
    /// JSON config file inside `container_dir`, e.g. ".claude.json".
    pub file: &'static str,
    /// Top-level boolean gating the onboarding wizard, set to `true`, e.g.
    /// "hasCompletedOnboarding".
    pub onboarding_flag: &'static str,
    /// Key of the per-project map in that file, e.g. "projects".
    pub projects_key: &'static str,
    /// Booleans set to `true` under `<projects_key>[<container_workdir>]`, e.g.
    /// ["hasTrustDialogAccepted", "hasCompletedProjectOnboarding"]. Empty = no
    /// project entry is written.
    pub project_flags: &'static [&'static str],
}

/// All behavior that varies per coding agent. Plain `Copy` data (see §2 D1).
#[derive(Debug, Clone, Copy)]
pub struct CodingAgentProfile {
    pub kind: CodingAgentKind,
    /// Idle-detector tuning for sessions running this agent.
    pub idle: IdleTuning,
    /// Canonical argv tokens AC auto-injects to resume the agent's prior
    /// conversation, in argv order. Persistence stripping is provider-specific:
    /// Pi recipes are intentionally preserved because configured and injected
    /// `--continue` tokens have no persisted provenance.
    /// - Claude: `["--continue"]` (appended to argv)
    /// - Codex: `["resume", "--last"]` (prepended as a subcommand)
    /// - Antigravity: `["--continue"]` (prepended)
    /// - Pi: `["--continue"]` (inserted immediately after the executable)
    /// - Muse: `["resume", "--last"]` (appended to an EMPTY configured argv only;
    ///   any configured argument suppresses injection, #1873)
    pub resume_tokens: &'static [&'static str],
    /// #930 - host credential file this agent reuses in a container (None = no
    /// copy-in for this agent).
    pub container_credential: Option<ContainerCredentialSource>,
    /// Whether generated auto-self-clear instructions are supported for this
    /// provider. This capability is an absolute outer gate on user settings.
    pub auto_self_clear_supported: bool,
}

// #2156 - all five agents use `IdleTuning::AGENT_DEFAULT`: the pre-#260
// hard-coded idle constants plus the #2124 burst filter ON, so a recognised
// coding agent gets the filter regardless of what the persisted catalog
// contains. The per-profile `idle` field exists so a future agent can diverge
// (e.g. a longer `resize_grace` for a heavier TUI) without re-plumbing the
// detector.
const CLAUDE_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Claude,
    idle: IdleTuning::AGENT_DEFAULT,
    resume_tokens: &["--continue"],
    // #930 - verified end-to-end: host ~/.claude/.credentials.json copies to
    // <replica>/.claude/.credentials.json, read in-container as
    // /workspace/.claude/.credentials.json.
    container_credential: Some(ContainerCredentialSource {
        host_dir: ".claude",
        host_dir_env: Some("CLAUDE_CONFIG_DIR"),
        file: ".credentials.json",
        container_dir: ".claude",
        // #930 - without these, a valid copied token STILL shows the onboarding
        // wizard ("Select login method") and then the folder-trust dialog.
        // Reproduced in a real container; both prompts vanish once they are set.
        first_run: Some(ContainerFirstRunState {
            file: ".claude.json",
            onboarding_flag: "hasCompletedOnboarding",
            projects_key: "projects",
            project_flags: &["hasTrustDialogAccepted", "hasCompletedProjectOnboarding"],
        }),
    }),
    auto_self_clear_supported: true,
};
const CODEX_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Codex,
    // #2879 - the only profile that selects the Working-indicator detector.
    idle: IdleTuning {
        activity_detector: ActivityDetectorKind::CodexWorking,
        ..IdleTuning::AGENT_DEFAULT
    },
    resume_tokens: &["resume", "--last"],
    // #930 follow-up (needs CODEX_HOME container wiring verified, Q3):
    // Some(ContainerCredentialSource { host_dir: ".codex", host_dir_env: Some("CODEX_HOME"),
    //     file: "auth.json", container_dir: ".codex" })
    container_credential: None,
    auto_self_clear_supported: true,
};
const ANTIGRAVITY_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Antigravity,
    idle: IdleTuning::AGENT_DEFAULT,
    resume_tokens: &["--continue"],
    // #930 - no established container credential-file flow for Antigravity.
    container_credential: None,
    auto_self_clear_supported: true,
};
const PI_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Pi,
    idle: IdleTuning::AGENT_DEFAULT,
    resume_tokens: &["--continue"],
    container_credential: None,
    auto_self_clear_supported: false,
};
// #1873 - Muse Code. Workspace-latest resume (`muse resume --last`) is injected
// only by the trusted empty-argv local macOS/Linux seam in `commands::session`;
// no container credential flow, no auto-self-clear, no logical PTY submission.
const MUSE_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Muse,
    idle: IdleTuning::AGENT_DEFAULT,
    resume_tokens: &["resume", "--last"],
    container_credential: None,
    auto_self_clear_supported: false,
};

/// Idle-detector tuning for a session, given its (optional) agent kind.
/// `None` (plain shell / unrecognised agent) → `IdleTuning::DEFAULT`.
pub fn idle_tuning_for(kind: Option<CodingAgentKind>) -> IdleTuning {
    match kind {
        Some(k) => k.profile().idle,
        None => IdleTuning::DEFAULT,
    }
}

/// #2124 - the per-kind tuning plus the session's catalog `idleBurst` filter,
/// as raw primitives (`maxBytes`, `maxSecs`, `priorSilenceSecs`). `None` (no
/// catalog object) means NO OVERRIDE and equals `idle_tuning_for`, i.e. #2156
/// leaves the filter ON for a recognised agent and OFF for a plain shell.
/// Absent, non-finite or negative subfields take the documented defaults;
/// `maxSecs` is capped at 600 s and `priorSilenceSecs` at 86400 s so
/// `Duration::from_secs_f64` can never panic. `0` for `maxBytes` or `maxSecs`
/// disables the filter. Takes primitives on purpose: this module must not
/// depend on `config`.
pub fn idle_tuning_for_launch(
    kind: Option<CodingAgentKind>,
    burst: Option<(Option<u64>, Option<f64>, Option<f64>)>,
) -> IdleTuning {
    let mut tuning = idle_tuning_for(kind);
    let Some((max_bytes, max_secs, prior_silence_secs)) = burst else {
        return tuning;
    };
    tuning.burst_max_bytes = max_bytes.unwrap_or(IDLE_BURST_DEFAULT_MAX_BYTES);
    tuning.burst_window = Duration::from_secs_f64(
        sanitized_burst_secs(max_secs, IDLE_BURST_DEFAULT_MAX_SECS).min(IDLE_BURST_MAX_WINDOW_SECS),
    );
    tuning.burst_prior_silence = Duration::from_secs_f64(
        sanitized_burst_secs(prior_silence_secs, IDLE_BURST_DEFAULT_PRIOR_SILENCE_SECS)
            .min(IDLE_BURST_MAX_PRIOR_SILENCE_SECS),
    );
    tuning
}

/// A finite, non-negative subfield value, else `default`.
fn sanitized_burst_secs(value: Option<f64>, default: f64) -> f64 {
    value
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests;
