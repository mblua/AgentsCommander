use super::*;

/// Replace every comment, and optionally every string or character literal, with
/// a single space, leaving code behind.
///
/// A comment is whitespace to the Rust lexer, so `super /* x */ ::root_agent` is
/// the same path as `super::root_agent`; collapsing whitespace alone would leave
/// that spelling intact and break the anchor. Tracking literals is what makes
/// comment removal correct at all: `"https://host"` carries a `//` that would
/// otherwise blank the rest of its line. Dropping literal bodies additionally
/// stops prose or a string from holding an observed set at its expected value
/// after the real references are gone, and it is what keeps the many
/// `"ac-root-agent"` fixtures in the guarded module's tests from matching the
/// forbidden name.
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
/// super::{root_agent::ROOT_AGENT_DIR_NAME, config_dir};` does not contain the
/// text `super::root_agent` at all: the braces are in the way. Reflowed across
/// lines by rustfmt it does not contain it either. After normalization every one
/// of those forms is the same text and the use-tree can be read.
///
/// `U+200E` and `U+200F` are replaced first because Rust's lexer treats them as
/// whitespace and `char::is_whitespace` does not, so `split_whitespace` would
/// leave `super<U+200E>::root_agent` intact and the anchor would never match a
/// path rustc compiles without a warning. They are the only two characters where
/// the two definitions disagree; `U+0085`, `U+2028` and `U+2029` are covered.
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
/// out, as in `use crate as c;`, `use crate::config as c;` or `use super as s;`.
///
/// After such a rename `c::root_agent::...` reaches the cut module under a name no
/// text scan can follow, so the rename itself is refused instead of followed.
/// Anchored on the path punctuation in front of `config` so that English prose
/// about configuration does not trip it, and on `use crate`/`use super` rather
/// than the bare keywords for the same reason.
///
/// **`extern crate self as <name>;` is the strongest member of the family and was
/// the last one missing.** It compiles inside the guarded module, it is
/// rustfmt-stable, and it renames the **crate root** rather than one module
/// group, so the alias reaches every knot member and not only
/// `config::root_agent`. Measured during the #1273 review as one line plus one
/// production reference: guard green, `cargo fmt --check` exit 0, and the
/// detector does not resolve it either, so nothing in the repository noticed.
/// Entry 21.
pub(crate) fn aliases_a_module_group(body: &str) -> bool {
    [
        "use crate as ",
        "::config as ",
        "{config as ",
        ",config as ",
        "config::{self as ",
        "use super as ",
        "use super::{self as ",
        "extern crate self as ",
    ]
    .iter()
    .any(|spelling| body.contains(spelling))
}

/// The leading identifier of a use-tree item: `root_agent` from
/// `root_agent::{a, b}`, from `root_agent as r` and from `root_agent`. A
/// non-identifier item such as `*` is returned as itself, so a glob is reported
/// rather than silently dropped.
///
/// A leading `r#` is dropped first: `r#root_agent` is the raw-identifier
/// spelling of `root_agent` and names the same module, but reading it literally
/// stops at the `#` and reports the child as `r`, so the reference would be
/// caught by the equality assertion instead of by the #1273 message that
/// explains it.
pub(crate) fn leading_segment(item: &str) -> String {
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
/// as `root_agent::{a, b}, config_dir` yields two items and not three.
pub(crate) fn split_top_level(group: &str) -> Vec<&str> {
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
/// `super::super::X` reports two children, `super` and `X`, because the scan
/// finds the anchor twice: once at the start and once immediately after it. That
/// is deliberate. `super` in the observed set is the marker that a path climbed
/// two levels, and dropping it would make `super::super::session::manager` and
/// `super::session::manager` indistinguishable.
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

/// Whether `body`, which must be scrubbed and normalized, names the module #1273
/// cut away from, as a bare identifier and under no anchor at all.
///
/// Both boundaries are checked, so `root_agent_defaults` and
/// `x_root_agent` are not hits and `r#root_agent` is.
///
/// **`mod root_agent;` is not a hit, and that exemption is what makes this check
/// applicable to the constant's home at all.** `src/config/mod.rs` is
/// `config::root_agent`'s parent, so it is *required* to declare it: without the
/// exemption this check would be red on a correct tree, which is why it could
/// not simply be switched on for that file. The exemption is safe because a
/// `mod` declaration is not a dependency and creates no arc, measured and stated
/// in plan Section 2.7 and in this file's own header. Only the declaration is
/// exempt: any *use* of the name, `root_agent::X`, `use root_agent::X;` or a
/// `super::`/`crate::`/`self::`-anchored path, is a separate occurrence and is
/// still caught.
///
/// **`mod ` must be the whole keyword, and the character in front of it is what
/// decides.** `pub mod root_agent;`, `pub(crate) mod root_agent;` and
/// `#[cfg(unix)] mod root_agent;` keep the exemption, because a space, `)` or
/// `]` precedes the keyword. `xmod root_agent;` does not, and that distinction
/// is load bearing rather than pedantic: inside a macro invocation those are
/// tokens that compile, and the first version of this exemption skipped them,
/// which was a measured regression rather than a pre-existing blind spot. Entry
/// 20 records it. The three other `mod`-matching functions in this file make the
/// same test for the same reason.
pub(crate) fn names_the_replaced_module(body: &str) -> bool {
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(FORBIDDEN_NAME) {
        let at = from + offset;
        let after = at + FORBIDDEN_NAME.len();
        let opens = !body[..at]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        let closes = !body[after..]
            .chars()
            .next()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        // `mod ` has to be the whole keyword and not the tail of an identifier,
        // decided by the character in front of it exactly as `find_declarations`,
        // `declared_children` and `nested_module_declaration` decide it. Without
        // this clause `xmod root_agent;` was exempted too, and inside a macro
        // invocation that is a token sequence which compiles: entry 20.
        let keyword_is_whole = at.checked_sub("mod ".len()).is_some_and(|keyword_at| {
            !body[..keyword_at]
                .chars()
                .next_back()
                .is_some_and(|character| {
                    character.is_alphanumeric() || character == '_' || character == '"'
                })
        });
        let declares_it =
            body[..at].ends_with("mod ") && body[after..].starts_with(';') && keyword_is_whole;
        if opens && closes && !declares_it {
            return true;
        }
        from = after;
    }
    false
}

/// Whether `body`, which must be scrubbed and normalized, defines the constant.
///
/// The needle is the keyword and the name together, and what follows decides. A
/// following `:` is what makes it a definition, so `pub const ROOT_AGENT_DIR_NAME:
/// &str = ...` and `static ROOT_AGENT_DIR_NAME: &str = ...` both count while
/// `pub use crate::config::ROOT_AGENT_DIR_NAME;` does not: the re-export #1273
/// leaves in `src/config/root_agent.rs` names the identifier and defines nothing,
/// and counting it would make the "moved, not duplicated" assertion fail on the
/// very shape the plan requires.
pub(crate) fn defines_the_constant(body: &str) -> bool {
    for keyword in ["const ", "static "] {
        let needle = format!("{keyword}{CONSTANT_NAME}");
        let mut from = 0usize;
        while let Some(offset) = body[from..].find(&needle) {
            let at = from + offset;
            let after = at + needle.len();
            let preceded_by_identifier = body[..at]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_alphanumeric() || character == '_');
            if !preceded_by_identifier && body[after..].trim_start().starts_with(':') {
                return true;
            }
            from = after;
        }
    }
    false
}
