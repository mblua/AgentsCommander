//! Per-coding-agent profile — the single source of truth for behavior that
//! varies by coding agent (Claude Code, Codex CLI, Gemini CLI, Pi Coding Agent).
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
    Gemini,
    Pi,
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
    Gemini,
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
        } else if stem == "gemini" || (configured_wrapper && stem.starts_with("gemini")) {
            Some(Self::Gemini)
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
                | (Self::Gemini, Some(CodingAgentKind::Gemini))
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
    /// **Claude > Codex > Gemini**. That legacy prefix match remains deliberate
    /// for wrappers such as `claude-mb`, `codex-foo`, and `gemini-bar`.
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
        // Precedence claude > codex > gemini, scanning every token.
        if basenames.iter().any(|b| b.starts_with("claude")) {
            Some(CodingAgentKind::Claude)
        } else if basenames.iter().any(|b| b.starts_with("codex")) {
            Some(CodingAgentKind::Codex)
        } else if basenames.iter().any(|b| b.starts_with("gemini")) {
            Some(CodingAgentKind::Gemini)
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
            CodingAgentKind::Gemini => "gemini",
            CodingAgentKind::Pi => "pi",
        }
    }

    /// Resolve the full behavior profile for this kind.
    pub const fn profile(self) -> CodingAgentProfile {
        match self {
            CodingAgentKind::Claude => CLAUDE_PROFILE,
            CodingAgentKind::Codex => CODEX_PROFILE,
            CodingAgentKind::Gemini => GEMINI_PROFILE,
            CodingAgentKind::Pi => PI_PROFILE,
        }
    }
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
}

impl IdleTuning {
    /// Tuning for a plain shell / unrecognised agent. Also the per-field
    /// fallback when a session id is missing from the detector's tuning map.
    /// Values are identical to the pre-#260 `idle_detector.rs` constants.
    pub const DEFAULT: IdleTuning = IdleTuning {
        idle_threshold: Duration::from_millis(2500),
        resize_grace: Duration::from_millis(3000),
        seed_initial_activity: true,
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
    /// - Gemini: `["--resume", "latest"]` (prepended; the joined
    ///   `--resume=latest` form is handled by the Gemini stripper)
    /// - Pi: `["--continue"]` (inserted immediately after the executable)
    pub resume_tokens: &'static [&'static str],
    /// #930 - host credential file this agent reuses in a container (None = no
    /// copy-in for this agent).
    pub container_credential: Option<ContainerCredentialSource>,
    /// Whether generated auto-self-clear instructions are supported for this
    /// provider. This capability is an absolute outer gate on user settings.
    pub auto_self_clear_supported: bool,
}

// All four agents currently use `IdleTuning::DEFAULT`, identical to the
// pre-#260 hard-coded constants, which GUARANTEES zero behavior change. The
// per-profile `idle` field exists so a future agent can diverge (e.g. a
// longer `resize_grace` for a heavier TUI) without re-plumbing the detector.
const CLAUDE_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Claude,
    idle: IdleTuning::DEFAULT,
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
    idle: IdleTuning::DEFAULT,
    resume_tokens: &["resume", "--last"],
    // #930 follow-up (needs CODEX_HOME container wiring verified, Q3):
    // Some(ContainerCredentialSource { host_dir: ".codex", host_dir_env: Some("CODEX_HOME"),
    //     file: "auth.json", container_dir: ".codex" })
    container_credential: None,
    auto_self_clear_supported: true,
};
const GEMINI_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Gemini,
    idle: IdleTuning::DEFAULT,
    resume_tokens: &["--resume", "latest"],
    // #930 - no established container credential-file flow for Gemini.
    container_credential: None,
    auto_self_clear_supported: true,
};
const PI_PROFILE: CodingAgentProfile = CodingAgentProfile {
    kind: CodingAgentKind::Pi,
    idle: IdleTuning::DEFAULT,
    resume_tokens: &["--continue"],
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

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn detect_direct_claude_and_wrapper_basename() {
        assert_eq!(
            CodingAgentKind::detect("claude", &[]),
            Some(CodingAgentKind::Claude)
        );
        assert_eq!(
            CodingAgentKind::detect("claude-mb", &["--effort".into(), "max".into()]),
            Some(CodingAgentKind::Claude)
        );
    }

    #[test]
    fn detect_codex_and_gemini_direct() {
        assert_eq!(
            CodingAgentKind::detect("codex", &[]),
            Some(CodingAgentKind::Codex)
        );
        assert_eq!(
            CodingAgentKind::detect("gemini", &["-m".into(), "gpt-5".into()]),
            Some(CodingAgentKind::Gemini)
        );
    }

    #[test]
    fn locate_direct_pi_exact_allowlist_and_paths() {
        for shell in [
            "pi",
            "PI",
            "pi.exe",
            "Pi.CmD",
            "/usr/local/bin/pi",
            r"C:\tools\PI.EXE",
            r"\\server\share\pi.cmd",
            r"\\?\C:\Program Files\Pi\pi.exe",
        ] {
            let location = locate_pi_command(shell, &strings(&["--model", "claude-sonnet"]))
                .unwrap()
                .unwrap_or_else(|| panic!("expected Pi location for {shell:?}"));
            assert_eq!(
                location,
                PiCommandLocation {
                    option_tokens: strings(&["--model", "claude-sonnet"]),
                    insertion: PiInsertionPoint::Arg { index: 0 },
                },
                "shell={shell:?}"
            );
            assert_eq!(
                CodingAgentKind::detect(shell, &strings(&["--model", "claude-sonnet"])),
                Some(CodingAgentKind::Pi)
            );
        }
    }

    #[test]
    fn direct_pi_args_remain_literal_process_arguments() {
        let args = strings(&["&&", "\"", "dangling^", "line\r\nvalue", "工具"]);
        assert_eq!(
            locate_pi_command("pi", &args),
            Ok(Some(PiCommandLocation {
                option_tokens: args,
                insertion: PiInsertionPoint::Arg { index: 0 },
            }))
        );
    }

    #[test]
    fn runtime_shell_value_is_not_trimmed_or_unquoted() {
        for shell in [" pi ", "\"pi\"", r#""C:\tools\pi.cmd""#] {
            assert!(
                !matches!(locate_pi_command(shell, &[]), Ok(Some(_))),
                "shell={shell:?}"
            );
            assert_ne!(
                CodingAgentKind::detect(shell, &[]),
                Some(CodingAgentKind::Pi)
            );
        }

        let normalized = crate::config::agent_command::normalize_legacy_agent_command(
            r#""C:\Program Files\Pi\pi.cmd" --model claude-sonnet"#,
        )
        .unwrap();
        assert_eq!(normalized.shell, r"C:\Program Files\Pi\pi.cmd");
        assert_eq!(
            CodingAgentKind::detect(&normalized.shell, &normalized.shell_args),
            Some(CodingAgentKind::Pi)
        );
    }

    #[test]
    fn reserved_pi_extensions_fail_closed() {
        for shell in ["pi.md", "PI.BAT", r"C:\tools\pi.ps1"] {
            let args = strings(&["--model", "claude-sonnet"]);
            assert_eq!(
                locate_pi_command(shell, &args),
                Err(PiCommandParseError::UnsupportedPiCommandShape)
            );
            assert_eq!(CodingAgentKind::detect(shell, &args), None);
        }
    }

    #[test]
    fn locate_tokenized_cmd_pi_decodes_atoms_and_limits_first_segment() {
        let args = strings(&[
            "/C",
            r#""C:\Program Files\Pi\pi.cmd""#,
            "--model",
            "claude-sonnet",
            "&&",
            "echo",
            "done",
        ]);
        assert_eq!(
            locate_pi_command("CMD.EXE", &args),
            Ok(Some(PiCommandLocation {
                option_tokens: strings(&["--model", "claude-sonnet"]),
                insertion: PiInsertionPoint::Arg { index: 2 },
            }))
        );

        for head in ["pi", "p^i", "\"pi.cmd\""] {
            let args = strings(&["/K", head]);
            assert_eq!(
                locate_pi_command("cmd", &args),
                Ok(Some(PiCommandLocation {
                    option_tokens: Vec::new(),
                    insertion: PiInsertionPoint::Arg { index: 2 },
                }))
            );
        }

        let escaped = strings(&["/C", "pi", "\"--value&&literal\"", "x^&y", "&&", "echo"]);
        assert_eq!(
            locate_pi_command("cmd", &escaped)
                .unwrap()
                .unwrap()
                .option_tokens,
            strings(&["--value&&literal", "x&y"])
        );
    }

    #[test]
    fn locate_embedded_cmd_pi_reports_exact_utf8_ranges() {
        let text = " \t\"C:\\工具\\pi.cmd\"  --model claude-sonnet  &&echo done";
        let args = vec!["/K".to_string(), text.to_string()];
        let location = locate_pi_command("cmd.exe", &args).unwrap().unwrap();
        assert_eq!(
            location.option_tokens,
            strings(&["--model", "claude-sonnet"])
        );
        let PiInsertionPoint::CmdText {
            arg_index,
            executable_range,
            segment_range,
        } = location.insertion
        else {
            panic!("expected embedded insertion");
        };
        assert_eq!(arg_index, 1);
        assert_eq!(&text[executable_range.clone()], "\"C:\\工具\\pi.cmd\"");
        assert_eq!(segment_range.start, executable_range.start);
        assert_eq!(segment_range.end, text.find("&&").unwrap());
        for endpoint in [
            executable_range.start,
            executable_range.end,
            segment_range.start,
            segment_range.end,
        ] {
            assert!(text.is_char_boundary(endpoint));
        }
        assert_eq!(&text[segment_range.end..], "&&echo done");
    }

    #[test]
    fn embedded_quotes_carets_and_separators_decode_without_rewriting() {
        let text = r#"pi "--resume&&later" x^&y&&echo --continue"#;
        let location = locate_pi_command("cmd", &strings(&["/C", text]))
            .unwrap()
            .unwrap();
        assert_eq!(location.option_tokens, strings(&["--resume&&later", "x&y"]));
        let PiInsertionPoint::CmdText { segment_range, .. } = location.insertion else {
            panic!("expected embedded insertion");
        };
        assert_eq!(&text[segment_range.end..], "&&echo --continue");
    }

    #[test]
    fn pi_identity_precedes_legacy_provider_values() {
        for args in [
            strings(&["--model", "claude-sonnet"]),
            strings(&["--model", "codex-model"]),
            strings(&["--provider", "gemini-pro"]),
        ] {
            assert_eq!(
                CodingAgentKind::detect("pi", &args),
                Some(CodingAgentKind::Pi)
            );
        }
        assert_eq!(
            CodingAgentKind::detect(
                "cmd.exe",
                &strings(&["/C", "pi", "--model", "claude-sonnet"])
            ),
            Some(CodingAgentKind::Pi)
        );
        assert_eq!(
            CodingAgentKind::detect(
                "cmd.exe",
                &strings(&["/C", "pi --provider codex --model gemini-pro"])
            ),
            Some(CodingAgentKind::Pi)
        );
    }

    #[test]
    fn unsupported_pi_positions_and_syntax_fail_closed() {
        let cases = [
            strings(&["/C", "npx", "pi", "--model", "claude-sonnet"]),
            strings(&["/C", "call", "pi", "--provider", "codex"]),
            strings(&["/C", "start", "pi", "gemini-pro"]),
            strings(&["/S", "/C", "pi", "--model", "claude-sonnet"]),
            strings(&["/C", "echo before&&pi --model claude-sonnet"]),
            strings(&["/C", "(", "pi", ")", "--provider", "codex"]),
            strings(&["/C", "(pi) --provider codex"]),
            strings(&["/C", "pi>out --model claude-sonnet"]),
            strings(&["/C", "pi.bat --model claude-sonnet"]),
            strings(&["/C", "echo", "before", "&&", "pi", "gemini-pro"]),
            strings(&["/C", "pi", "--resume&&echo", "claude-sonnet"]),
            strings(&["/C", "pi&&echo", "--model", "claude-sonnet"]),
        ];
        for args in cases {
            assert_eq!(
                locate_pi_command("cmd.exe", &args),
                Err(PiCommandParseError::UnsupportedPiCommandShape),
                "args={args:?}"
            );
            assert_eq!(CodingAgentKind::detect("cmd.exe", &args), None);
        }

        let npx_args = strings(&["pi", "--model", "claude-sonnet"]);
        assert_eq!(
            locate_pi_command("npx.cmd", &npx_args),
            Err(PiCommandParseError::UnsupportedPiCommandShape)
        );
        assert_eq!(CodingAgentKind::detect("npx.cmd", &npx_args), None);
    }

    #[test]
    fn tokenized_attached_later_pi_heads_fail_closed() {
        let providers = [
            ("--model", "claude-sonnet"),
            ("--provider", "codex-model"),
            ("--model", "gemini-pro"),
        ];

        for separator in ["&", "&&", "|", "||"] {
            for executable in ["pi", "pi.md", "pi.bat"] {
                for (provider_flag, provider_value) in providers {
                    let cases = [
                        vec![
                            "/C".to_string(),
                            format!("echo{separator}{executable}"),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ],
                        vec![
                            "/C".to_string(),
                            format!("echo{separator}"),
                            executable.to_string(),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ],
                        vec![
                            "/C".to_string(),
                            "echo".to_string(),
                            format!("{separator}{executable}"),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ],
                        vec![
                            "/C".to_string(),
                            "echo".to_string(),
                            format!("value{separator}{executable}"),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ],
                    ];

                    for args in cases {
                        assert_eq!(
                            locate_pi_command("cmd.exe", &args),
                            Err(PiCommandParseError::UnsupportedPiCommandShape),
                            "separator={separator:?} executable={executable:?} args={args:?}"
                        );
                        assert_eq!(
                            CodingAgentKind::detect("cmd.exe", &args),
                            None,
                            "separator={separator:?} executable={executable:?} args={args:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn tokenized_grouped_attached_pi_boundaries_fail_closed() {
        let providers = [
            ("--model", "claude-sonnet"),
            ("--provider", "codex-model"),
            ("--model", "gemini-pro"),
        ];
        let wrappers = [
            "npx",
            "npx.exe",
            "npx.cmd",
            "call",
            "call.exe",
            "call.cmd",
            "start",
            "start.exe",
            "start.cmd",
        ];
        let mut cases = Vec::new();

        for separator in ["&", "&&", "|", "||"] {
            for groups in ["(", "((("] {
                for executable in ["pi", "pi.md", "pi.bat"] {
                    for &(provider_flag, provider_value) in &providers {
                        cases.push(vec![
                            "/C".to_string(),
                            format!("echo{separator}{groups}"),
                            executable.to_string(),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ]);
                        cases.push(vec![
                            "/C".to_string(),
                            format!("echo{separator}{groups}{executable}"),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ]);
                        cases.push(vec![
                            "/C".to_string(),
                            format!("echo{separator}"),
                            groups.to_string(),
                            executable.to_string(),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ]);

                        for wrapper in wrappers {
                            cases.push(vec![
                                "/C".to_string(),
                                format!("echo{separator}{groups}{wrapper}"),
                                executable.to_string(),
                                provider_flag.to_string(),
                                provider_value.to_string(),
                            ]);
                            cases.push(vec![
                                "/C".to_string(),
                                format!("echo{separator}{groups}"),
                                wrapper.to_string(),
                                executable.to_string(),
                                provider_flag.to_string(),
                                provider_value.to_string(),
                            ]);
                            cases.push(vec![
                                "/C".to_string(),
                                format!("echo{separator}"),
                                format!("{groups}{wrapper}"),
                                executable.to_string(),
                                provider_flag.to_string(),
                                provider_value.to_string(),
                            ]);
                        }
                    }
                }
            }
        }

        for args in cases {
            assert_eq!(
                locate_pi_command("cmd.exe", &args),
                Err(PiCommandParseError::UnsupportedPiCommandShape),
                "args={args:?}"
            );
            assert_eq!(
                CodingAgentKind::detect("cmd.exe", &args),
                None,
                "args={args:?}"
            );
        }
    }

    #[test]
    fn tokenized_attached_later_head_negative_controls_stay_non_pi() {
        let cases = [
            (
                strings(&["/C", "echo", "pi", "--model", "claude-sonnet"]),
                CodingAgentKind::Claude,
            ),
            (
                strings(&["/C", "echo", "pi&&later", "--provider", "codex-model"]),
                CodingAgentKind::Codex,
            ),
            (
                strings(&["/C", "echo", "value^&^&pi", "--model", "gemini-pro"]),
                CodingAgentKind::Gemini,
            ),
            (
                strings(&["/C", "echo&&other", "pi", "--model", "claude-sonnet"]),
                CodingAgentKind::Claude,
            ),
            (
                strings(&["/C", "echo", "\"value&&pi\"", "--model", "claude-sonnet"]),
                CodingAgentKind::Claude,
            ),
        ];

        for (args, expected_kind) in cases {
            assert_eq!(locate_pi_command("cmd.exe", &args), Ok(None));
            assert_eq!(
                CodingAgentKind::detect("cmd.exe", &args),
                Some(expected_kind)
            );
        }
    }

    #[test]
    fn tokenized_redirection_targets_stay_out_of_pi_command_positions() {
        let legacy_cases = [
            (vec!["claude"], CodingAgentKind::Claude),
            (vec!["--model", "claude-sonnet"], CodingAgentKind::Claude),
            (vec!["--provider", "codex-model"], CodingAgentKind::Codex),
            (vec!["--model", "gemini-pro"], CodingAgentKind::Gemini),
        ];

        for redirect in ["<", ">"] {
            for target in ["pi", "pi.md", "pi.bat"] {
                for (legacy_tokens, expected_kind) in &legacy_cases {
                    let mut layouts = vec![
                        vec![redirect.to_string(), target.to_string()],
                        vec![redirect.repeat(2), target.to_string()],
                        vec![format!("2{redirect}"), target.to_string()],
                        vec!["npx".to_string(), redirect.to_string(), target.to_string()],
                        vec!["call".to_string(), redirect.to_string(), target.to_string()],
                        vec![
                            "start".to_string(),
                            redirect.to_string(),
                            target.to_string(),
                        ],
                    ];
                    for separator in ["&", "&&", "|", "||"] {
                        layouts.push(vec![
                            "echo".to_string(),
                            separator.to_string(),
                            redirect.to_string(),
                            target.to_string(),
                        ]);
                        layouts.push(vec![
                            format!("echo{separator}{redirect}"),
                            target.to_string(),
                        ]);
                    }

                    for mut layout in layouts {
                        let mut args = vec!["/C".to_string()];
                        args.append(&mut layout);
                        args.extend(legacy_tokens.iter().map(|token| (*token).to_string()));

                        assert_eq!(
                            locate_pi_command("cmd.exe", &args),
                            Ok(None),
                            "redirect={redirect:?} target={target:?} args={args:?}"
                        );
                        assert_eq!(
                            CodingAgentKind::detect("cmd.exe", &args),
                            Some(*expected_kind),
                            "redirect={redirect:?} target={target:?} args={args:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn tokenized_group_and_control_position_transitions_stay_frozen() {
        let non_candidates = [
            (
                strings(&["/C", ")", "pi", "claude"]),
                CodingAgentKind::Claude,
            ),
            (
                strings(&["/C", "><", "pi.md", "--provider", "codex-model"]),
                CodingAgentKind::Codex,
            ),
            (
                strings(&["/C", "echo&&)", "pi.bat", "--model", "gemini-pro"]),
                CodingAgentKind::Gemini,
            ),
            (
                strings(&["/C", "npx", "()", "pi", "claude"]),
                CodingAgentKind::Claude,
            ),
            (
                strings(&["/C", ">", "npx", "pi", "claude"]),
                CodingAgentKind::Claude,
            ),
            (
                strings(&["/C", "echo&&>npx", "pi", "--provider", "codex-model"]),
                CodingAgentKind::Codex,
            ),
            (
                strings(&["/C", "npx", ">", "start", "pi", "--model", "gemini-pro"]),
                CodingAgentKind::Gemini,
            ),
            (
                strings(&["/C", "echo(", "pi", "--model", "claude-sonnet"]),
                CodingAgentKind::Claude,
            ),
            (
                strings(&["/C", "echo(npx", "pi", "--provider", "codex-model"]),
                CodingAgentKind::Codex,
            ),
            (
                strings(&["/C", "echo^&^&(", "pi", "--model", "gemini-pro"]),
                CodingAgentKind::Gemini,
            ),
            (
                strings(&["/C", "echo&&^(", "pi", "--model", "claude-sonnet"]),
                CodingAgentKind::Claude,
            ),
            (
                strings(&["/C", "echo&&\"(\"", "pi", "--provider", "codex-model"]),
                CodingAgentKind::Codex,
            ),
            (
                strings(&["/C", "echo&&(()", "pi", "--model", "gemini-pro"]),
                CodingAgentKind::Gemini,
            ),
            (
                strings(&["/C", "echo&&(<", "pi", "--model", "claude-sonnet"]),
                CodingAgentKind::Claude,
            ),
            (
                strings(&["/C", "echo", "(((", "pi", "--provider", "codex-model"]),
                CodingAgentKind::Codex,
            ),
            (
                strings(&["/C", "npx>pi", "--model", "claude-sonnet"]),
                CodingAgentKind::Claude,
            ),
            (
                strings(&["/C", "echo&&>npx(pi", "--provider", "codex-model"]),
                CodingAgentKind::Codex,
            ),
            (
                strings(&["/C", "echo&&)start(pi", "--model", "gemini-pro"]),
                CodingAgentKind::Gemini,
            ),
        ];
        for (args, expected_kind) in non_candidates {
            assert_eq!(
                locate_pi_command("cmd.exe", &args),
                Ok(None),
                "args={args:?}"
            );
            assert_eq!(
                CodingAgentKind::detect("cmd.exe", &args),
                Some(expected_kind),
                "args={args:?}"
            );
        }

        let candidates = [
            strings(&["/C", "echo&&", "(", "pi", "--model", "claude-sonnet"]),
            strings(&["/C", "npx", "(((", "pi.md", "--provider", "codex-model"]),
            strings(&["/C", ">", "out", "&&", "pi.bat", "--model", "gemini-pro"]),
            strings(&["/C", "echo)&", "pi", "--model", "claude-sonnet"]),
            strings(&["/C", "echo&&npx", "pi", "--provider", "codex-model"]),
        ];
        for args in candidates {
            assert_eq!(
                locate_pi_command("cmd.exe", &args),
                Err(PiCommandParseError::UnsupportedPiCommandShape),
                "args={args:?}"
            );
            assert_eq!(CodingAgentKind::detect("cmd.exe", &args), None);
        }
    }

    #[test]
    fn malformed_cmd_syntax_fails_closed_after_full_validation() {
        let cases = [
            strings(&["/C", "pi", "\"unterminated", "--model", "claude-sonnet"]),
            strings(&["/C", "pi", "dangling^", "codex-model"]),
            strings(&["/C", "echo", "\"unterminated", "claude-sonnet"]),
            strings(&["/C", "pi \"unterminated --model claude-sonnet"]),
            strings(&["/C", "pi dangling^"]),
            vec![
                "/C".to_string(),
                "pi".to_string(),
                "bad\0".to_string(),
                "claude-sonnet".to_string(),
            ],
            vec!["/C".to_string(), "pi\0 --model claude-sonnet".to_string()],
            vec!["/C".to_string(), "pi\r --provider codex".to_string()],
            vec!["/C".to_string(), "pi\n gemini-pro".to_string()],
        ];
        for args in cases {
            assert_eq!(
                locate_pi_command("cmd.exe", &args),
                Err(PiCommandParseError::MalformedCmdSyntax),
                "args={args:?}"
            );
            assert_eq!(CodingAgentKind::detect("cmd.exe", &args), None);
        }
    }

    #[test]
    fn genuine_non_pi_commands_retain_legacy_detection() {
        assert_eq!(
            CodingAgentKind::detect("claude", &strings(&["--model", "pi"])),
            Some(CodingAgentKind::Claude)
        );
        assert_eq!(
            CodingAgentKind::detect("cmd.exe", &strings(&["/C", "claude", "--model", "pi"])),
            Some(CodingAgentKind::Claude)
        );
        assert_eq!(
            CodingAgentKind::detect("cmd.exe", &strings(&["/C", "echo", "pi"])),
            None
        );
        for shell in ["my-pi", "pip", "pipx", "ping", "pixel"] {
            assert_eq!(CodingAgentKind::detect(shell, &[]), None, "shell={shell:?}");
        }
        assert_eq!(
            CodingAgentKind::detect("pip", &strings(&["--model", "claude-sonnet"])),
            Some(CodingAgentKind::Claude)
        );
    }

    #[test]
    fn embedded_redirection_and_environment_indirection_follow_contract() {
        let text = "pi  > out &&echo done";
        let location = locate_pi_command("cmd", &strings(&["/C", text]))
            .unwrap()
            .unwrap();
        assert_eq!(location.option_tokens, strings(&[">", "out"]));
        assert_eq!(
            locate_pi_command("cmd", &strings(&["/C", "%PI_EXE% --model claude-sonnet"])),
            Ok(None)
        );
        assert_eq!(
            locate_pi_command("cmd", &strings(&["/C", "pi>out"])),
            Err(PiCommandParseError::UnsupportedPiCommandShape)
        );
    }

    #[test]
    fn pi_first_compound_is_supported_and_later_selector_is_not_an_option() {
        for args in [
            strings(&["/C", "pi", "--model", "x", "&&", "echo", "--resume"]),
            strings(&["/C", "pi --model x&&echo --resume"]),
        ] {
            let location = locate_pi_command("cmd.exe", &args).unwrap().unwrap();
            assert_eq!(location.option_tokens, strings(&["--model", "x"]));
        }
    }

    #[test]
    fn legacy_cmd_detection_and_precedence_remain_unchanged_after_ok_none() {
        assert_eq!(
            CodingAgentKind::detect("cmd.exe", &strings(&["/C", "codex"])),
            Some(CodingAgentKind::Codex)
        );
        assert_eq!(
            CodingAgentKind::detect(
                "cmd.exe",
                &strings(&["/K", "git pull && gemini --resume latest"])
            ),
            Some(CodingAgentKind::Gemini)
        );
        assert_eq!(
            CodingAgentKind::detect("cmd.exe", &strings(&["/K", "codex && claude"])),
            Some(CodingAgentKind::Claude)
        );
    }

    #[test]
    fn adversarial_cmd_corpus_never_panics_and_ranges_are_valid() {
        let fragments = [
            "",
            " ",
            "\t",
            "\"",
            "^",
            "^^",
            "&",
            "&&",
            "|",
            "||",
            "<",
            ">",
            "(",
            ")",
            "λ",
            "工具",
            "\0",
            "\r",
            "\n",
            "\"quoted\"",
        ];
        for left in fragments {
            for right in fragments {
                let text = format!("{left}pi{right} --model claude-sonnet&&echo λ");
                let args = vec!["/C".to_string(), text.clone()];
                if let Ok(Some(location)) = locate_pi_command("cmd.exe", &args) {
                    if let PiInsertionPoint::CmdText {
                        executable_range,
                        segment_range,
                        ..
                    } = location.insertion
                    {
                        assert!(executable_range.start <= executable_range.end);
                        assert!(executable_range.end <= segment_range.end);
                        assert_eq!(segment_range.start, executable_range.start);
                        assert!(segment_range.end <= text.len());
                        for endpoint in [
                            executable_range.start,
                            executable_range.end,
                            segment_range.start,
                            segment_range.end,
                        ] {
                            assert!(text.is_char_boundary(endpoint));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pi_serde_and_profile_contract_are_stable() {
        assert_eq!(
            serde_json::to_string(&CodingAgentKind::Pi).unwrap(),
            "\"pi\""
        );
        assert_eq!(CodingAgentKind::Pi.as_str(), "pi");
        let profile = CodingAgentKind::Pi.profile();
        assert_eq!(profile.kind, CodingAgentKind::Pi);
        assert_eq!(profile.idle, IdleTuning::DEFAULT);
        assert_eq!(profile.resume_tokens, ["--continue"]);
        assert!(profile.container_credential.is_none());
        assert!(!profile.auto_self_clear_supported);
        for kind in [
            CodingAgentKind::Claude,
            CodingAgentKind::Codex,
            CodingAgentKind::Gemini,
        ] {
            assert!(kind.profile().auto_self_clear_supported);
        }
    }

    #[test]
    fn detect_plain_shell_and_literal_space_path_are_none() {
        assert_eq!(
            CodingAgentKind::detect("powershell.exe", &["-NoLogo".into()]),
            None
        );
        assert_eq!(CodingAgentKind::detect("cmd.exe", &[]), None);
    }

    #[test]
    fn privileged_detector_requires_executable_position() {
        assert_eq!(
            detect_pty_submission_agent("claude.exe", &[], Some(CodingAgentKind::Claude)),
            Some(PtySubmissionAgent::Claude)
        );
        assert_eq!(
            detect_pty_submission_agent(
                "cmd.exe",
                &["/C".into(), "codex".into(), "--quiet".into()],
                Some(CodingAgentKind::Codex)
            ),
            Some(PtySubmissionAgent::Codex)
        );
        assert_eq!(
            detect_pty_submission_agent("agent.exe", &[], None),
            Some(PtySubmissionAgent::CursorAgent)
        );
        assert_eq!(detect_pty_submission_agent("agentctl", &[], None), None);
        assert_eq!(
            detect_pty_submission_agent(
                "bash",
                &["-c".into(), "echo claude".into()],
                Some(CodingAgentKind::Claude)
            ),
            None
        );
    }

    #[test]
    fn privileged_detector_rejects_unconfigured_prefix_named_plain_shells() {
        assert_eq!(
            detect_pty_submission_agent("claudette.exe", &[], Some(CodingAgentKind::Claude),),
            None
        );
        let wrapper_args = ["/C".into(), "codex-shell.exe".into()];
        assert_eq!(
            detect_pty_submission_agent("cmd.exe", &wrapper_args, Some(CodingAgentKind::Codex),),
            None
        );
        assert_eq!(
            detect_configured_pty_submission_agent(
                "cmd.exe",
                &wrapper_args,
                Some(CodingAgentKind::Codex),
            ),
            Some(PtySubmissionAgent::Codex)
        );
    }

    #[test]
    fn privileged_detector_rejects_persistent_or_compound_cmd() {
        for args in [
            vec!["/K".into(), "codex".into()],
            vec!["/C".into(), "codex && whoami".into()],
            vec!["/C".into(), "CALL codex".into()],
            vec!["/C".into(), "codex %EXTRA%".into()],
        ] {
            assert_eq!(
                detect_pty_submission_agent("cmd.exe", &args, Some(CodingAgentKind::Codex)),
                None,
                "args={args:?}"
            );
        }
    }

    #[test]
    fn detect_strips_known_exe_extension() {
        // `file_stem` drops the `.exe` suffix the basename match relies on
        // (dev-rust R1.5).
        assert_eq!(
            CodingAgentKind::detect("claude.exe", &[]),
            Some(CodingAgentKind::Claude)
        );
    }

    #[test]
    fn detect_space_in_shell_path_treats_shell_as_one_token() {
        // #260 G3 — `detect` treats `shell` as a SINGLE token; it does NOT
        // whitespace-split it the way pre-#260 `create_session_inner` split
        // the joined command string. A space-containing shell path whose real
        // executable is not an agent therefore resolves to `None` (the more
        // correct result — the executable here is `runner.exe`).
        assert_eq!(
            CodingAgentKind::detect("C:\\codex tools\\runner.exe", &[]),
            None
        );
    }

    #[test]
    fn detect_strips_known_legacy_exe_extension() {
        assert_eq!(
            CodingAgentKind::detect("claude.exe", &[]),
            Some(CodingAgentKind::Claude)
        );
    }

    #[test]
    fn idle_profiles_keep_existing_defaults() {
        assert_eq!(idle_tuning_for(None), IdleTuning::DEFAULT);
        for kind in [
            CodingAgentKind::Claude,
            CodingAgentKind::Codex,
            CodingAgentKind::Gemini,
            CodingAgentKind::Pi,
        ] {
            assert!(kind.profile().idle.seed_initial_activity);
            assert_eq!(kind.profile().idle, IdleTuning::DEFAULT);
        }
        assert!(idle_tuning_for(None).seed_initial_activity);
    }
}
