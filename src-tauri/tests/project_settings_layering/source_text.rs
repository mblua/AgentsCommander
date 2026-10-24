/// Whether literal bodies survive `scrub`.
///
/// They must survive when the text is about to be read for `path = "..."`,
/// and must not when it is about to be read for dependencies or for structure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Literals {
    Keep,
    Drop,
}

/// Replace every comment, and optionally every string or character literal, with
/// a single space, leaving code behind.
///
/// A comment is whitespace to the Rust lexer, so `web /* x */ ::commands` is the
/// same path as `web::commands`; collapsing whitespace alone would leave that
/// spelling intact and break the anchor. Tracking literals is what makes comment
/// removal correct at all: `"https://host"` carries a `//` that would otherwise
/// blank the rest of its line. Dropping literal bodies additionally stops prose
/// or a string from holding the observed set at its expected value after the real
/// references are gone, which is the failure the equality below exists to catch.
///
/// A comment or literal that never closes is an error rather than a truncated
/// result: a scanner that cannot delimit what it is reading must say so, because
/// the alternative is a green result that proves nothing.
pub(crate) fn scrub(body: &str, literals: Literals) -> Result<String, &'static str> {
    let source: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut index = 0usize;

    let emit = |out: &mut String, text: &[char]| {
        if literals == Literals::Keep {
            out.extend(text.iter());
        } else {
            out.push(' ');
        }
    };

    while index < source.len() {
        let character = source[index];
        let preceded_by_identifier = index
            .checked_sub(1)
            .map(|previous| source[previous])
            .is_some_and(|previous| previous.is_alphanumeric() || previous == '_');

        if character == '/' && source.get(index + 1) == Some(&'/') {
            while index < source.len() && source[index] != '\n' {
                index += 1;
            }
            out.push(' ');
            continue;
        }

        if character == '/' && source.get(index + 1) == Some(&'*') {
            let mut depth = 0usize;
            while index < source.len() {
                if source[index] == '/' && source.get(index + 1) == Some(&'*') {
                    depth += 1;
                    index += 2;
                } else if source[index] == '*' && source.get(index + 1) == Some(&'/') {
                    depth -= 1;
                    index += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    index += 1;
                }
            }
            if depth != 0 {
                return Err("a block comment is never closed, so the scan cannot be trusted");
            }
            out.push(' ');
            continue;
        }

        // `r"..."`, `r#"..."#`, `br"..."` and `br#"..."#`, only at a token
        // boundary so the `r` ending an identifier is not read as a prefix.
        if (character == 'r' || character == 'b') && !preceded_by_identifier {
            let mut cursor = index;
            if source[cursor] == 'b' {
                cursor += 1;
            }
            if source.get(cursor) == Some(&'r') {
                cursor += 1;
                let mut hashes = 0usize;
                while source.get(cursor) == Some(&'#') {
                    hashes += 1;
                    cursor += 1;
                }
                if source.get(cursor) == Some(&'"') {
                    cursor += 1;
                    let closing: Vec<char> = std::iter::once('"')
                        .chain(std::iter::repeat_n('#', hashes))
                        .collect();
                    let mut closed = false;
                    while cursor < source.len() {
                        if source[cursor..].starts_with(closing.as_slice()) {
                            cursor += closing.len();
                            closed = true;
                            break;
                        }
                        cursor += 1;
                    }
                    if !closed {
                        return Err("a raw string is never closed, so the scan cannot be trusted");
                    }
                    emit(&mut out, &source[index..cursor]);
                    index = cursor;
                    continue;
                }
            }
        }

        if character == '"' {
            let start = index;
            index += 1;
            let mut closed = false;
            while index < source.len() {
                match source[index] {
                    '\\' => index += 2,
                    '"' => {
                        index += 1;
                        closed = true;
                        break;
                    }
                    _ => index += 1,
                }
            }
            if !closed {
                return Err("a string literal is never closed, so the scan cannot be trusted");
            }
            emit(&mut out, &source[start..index]);
            continue;
        }

        // `'x'` and `'\n'` are literals; `'a` is a lifetime. Only a literal is
        // consumed, so a lifetime cannot swallow the code that follows it.
        if character == '\'' {
            if source.get(index + 1) == Some(&'\\') {
                let mut cursor = index + 3;
                while cursor < source.len() && source[cursor] != '\'' {
                    cursor += 1;
                }
                if cursor >= source.len() {
                    return Err(
                        "a character literal is never closed, so the scan cannot be trusted",
                    );
                }
                emit(&mut out, &source[index..cursor + 1]);
                index = cursor + 1;
                continue;
            }
            if source.get(index + 2) == Some(&'\'') {
                emit(&mut out, &source[index..index + 3]);
                index += 3;
                continue;
            }
        }

        out.push(character);
        index += 1;
    }

    Ok(out)
}

/// Collapse every run of ASCII whitespace (newlines included, so this is also
/// CRLF-safe) to one space, then delete the space on both sides of the
/// punctuation a Rust path or use-tree is built from.
///
/// This is what widens the net past a raw substring match. `use
/// crate::web::{commands::broadcast_all, broadcast::WsBroadcaster};` does not
/// contain the text `web::commands` at all: the braces are in the way. Reflowed
/// across lines by rustfmt it does not contain it either. After normalization
/// every one of those forms is the same text and the use-tree can be read.
///
/// `U+200E` and `U+200F` are replaced first because Rust's lexer treats them as
/// whitespace and `char::is_whitespace` does not, so `split_whitespace` would
/// leave `web<U+200E>::commands` intact and the anchor would never match a path
/// rustc compiles without a warning. They are the only two characters where the
/// two definitions disagree; `U+0085`, `U+2028` and `U+2029` are covered.
pub(crate) fn normalized(body: &str) -> String {
    let body = body.replace(['\u{200E}', '\u{200F}'], " ");
    let mut out = body.split_whitespace().collect::<Vec<_>>().join(" ");
    for token in ["::", "{", "}", ","] {
        out = out.replace(&format!(" {token}"), token);
        out = out.replace(&format!("{token} "), token);
    }
    out
}

/// Whether the source renames a module group this scan depends on being spelled
/// out, as in `use crate::web as w;`, `use crate::web::{self as w};` or
/// `use crate as c;`.
///
/// After such a rename `w::commands::...` or `c::web::commands::...` reaches the
/// forbidden module under a name no text scan can follow, so the rename itself is
/// refused instead of followed. Anchored on the path punctuation in front of
/// `web` so that English prose about the web does not trip it, and on `use crate`
/// rather than bare `crate` for the same reason.
pub(crate) fn aliases_a_module_group(body: &str) -> bool {
    [
        "::web as ",
        "{web as ",
        ",web as ",
        "web::{self as ",
        "use crate as ",
    ]
    .iter()
    .any(|spelling| body.contains(spelling))
}

/// The leading identifier of a use-tree item: `commands` from `commands::{a, b}`,
/// from `commands as c` and from `commands`. A non-identifier item such as `*` is
/// returned as itself, so a glob is reported rather than silently dropped.
///
/// A leading `r#` is dropped first: `r#commands` is the raw-identifier spelling
/// of `commands` and names the same module, but reading it literally stops at the
/// `#` and reports the child as `r`, so the reference would be caught by the
/// equality assertion instead of by the #1265 message that explains it.
fn leading_segment(item: &str) -> String {
    let item = item.strip_prefix("r#").unwrap_or(item);
    let mut segment: String = item
        .chars()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect();
    if segment.is_empty() {
        if let Some(character) = item.chars().next() {
            segment.push(character);
        }
    }
    segment
}

/// Split a brace group on the commas that belong to it, so a nested group such
/// as `commands::{a, b}, broadcast::c` yields two items and not three.
fn split_top_level(group: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in group.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                items.push(&group[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    items.push(&group[start..]);
    items
}

/// Every child named directly under `anchor` anywhere in `body`, which must
/// already be scrubbed and normalized, in source order.
///
/// `anchor` is `web::` for the dispatcher question and `crate::` for the emitter
/// module's own dependencies. Both go through the same brace-group handling, so
/// `use crate::{web::A, session::B}` reports `web` and `session` under
/// `crate::`, while `use crate::web::{broadcast::A, commands::B}` reports
/// `broadcast` and `commands` under `web::`.
///
/// An unclosed group is an error rather than an empty result, for the same reason
/// an unclosed comment is.
pub(crate) fn children_under(body: &str, anchor: &str) -> Result<Vec<String>, &'static str> {
    let mut children = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(anchor) {
        let anchor_at = from + offset;
        let after = anchor_at + anchor.len();
        let inside_longer_identifier = body[..anchor_at]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        if inside_longer_identifier {
            from = after;
            continue;
        }
        if body[after..].starts_with('{') {
            let mut depth = 0usize;
            let mut end = None;
            for (index, character) in body[after..].char_indices() {
                match character {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(after + index);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else {
                return Err("an anchored `{` group is never closed, so the scan cannot be trusted");
            };
            for item in split_top_level(&body[after + 1..end]) {
                if item.trim().is_empty() {
                    continue;
                }
                children.push(leading_segment(item));
            }
            from = end;
        } else {
            children.push(leading_segment(&body[after..]));
            from = after;
        }
    }
    Ok(children)
}
