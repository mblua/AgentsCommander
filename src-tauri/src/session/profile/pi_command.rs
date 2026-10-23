use std::ops::Range;

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
pub(super) fn is_exact_muse_executable(shell: &str) -> bool {
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
