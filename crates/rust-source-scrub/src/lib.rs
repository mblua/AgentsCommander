//! Comment and literal scrubbing for the layering guards in `src-tauri/tests/`.
//!
//! Each folder there with a `main.rs` is its own test crate, so the guards that
//! read Rust source as text share this one copy through `[dev-dependencies]`.

/// Whether literal bodies survive `scrub`.
///
/// They must survive when the text is about to be read for `path = "..."`,
/// and must not when it is about to be read for dependencies or for structure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Literals {
    Keep,
    Drop,
}

/// Replace every comment, and optionally every string or character literal, with
/// a single space, leaving code behind.
///
/// A comment is whitespace to the Rust lexer, so `super /* x */ ::root_agent` is
/// the same path as `super::root_agent`; collapsing whitespace alone would leave
/// that spelling intact and break the anchor. Tracking literals is what makes
/// comment removal correct at all: `"https://host"` carries a `//` that would
/// otherwise blank the rest of its line. Dropping literal bodies additionally
/// stops prose or a string from holding an observed set at its expected value
/// after the real references are gone, and it keeps string fixtures in a guarded
/// module's tests from matching a forbidden name.
///
/// A comment or literal that never closes is an error rather than a truncated
/// result: a scanner that cannot delimit what it is reading must say so, because
/// the alternative is a green result that proves nothing.
pub fn scrub(body: &str, literals: Literals) -> Result<String, &'static str> {
    let source: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut index = 0usize;

    while index < source.len() {
        let character = source[index];
        let preceded_by_identifier = index
            .checked_sub(1)
            .map(|previous| source[previous])
            .is_some_and(|previous| previous.is_alphanumeric() || previous == '_');

        if character == '/' && source.get(index + 1) == Some(&'/') {
            index = line_comment_end(&source, index);
            out.push(' ');
            continue;
        }

        if character == '/' && source.get(index + 1) == Some(&'*') {
            index = block_comment_end(&source, index)?;
            out.push(' ');
            continue;
        }

        // `r"..."`, `r#"..."#`, `br"..."` and `br#"..."#`, only at a token
        // boundary so the `r` ending an identifier is not read as a prefix.
        if (character == 'r' || character == 'b') && !preceded_by_identifier {
            if let Some(end) = raw_string_end(&source, index)? {
                emit(&mut out, &source[index..end], literals);
                index = end;
                continue;
            }
        }

        if character == '"' {
            let end = string_end(&source, index)?;
            emit(&mut out, &source[index..end], literals);
            index = end;
            continue;
        }

        // `'x'` and `'\n'` are literals; `'a` is a lifetime. Only a literal is
        // consumed, so a lifetime cannot swallow the code that follows it.
        if character == '\'' {
            if let Some(end) = char_literal_end(&source, index)? {
                emit(&mut out, &source[index..end], literals);
                index = end;
                continue;
            }
        }

        out.push(character);
        index += 1;
    }

    Ok(out)
}

/// Push `text` itself when literals are kept, or one space when they are dropped.
fn emit(out: &mut String, text: &[char], literals: Literals) {
    if literals == Literals::Keep {
        out.extend(text.iter());
    } else {
        out.push(' ');
    }
}

/// Index of the newline ending the `//` comment at `start`, or the end of input.
fn line_comment_end(source: &[char], start: usize) -> usize {
    let mut index = start;
    while index < source.len() && source[index] != '\n' {
        index += 1;
    }
    index
}

/// Index just past the `*/` closing the possibly nested `/*` comment at `start`.
fn block_comment_end(source: &[char], start: usize) -> Result<usize, &'static str> {
    let mut index = start;
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
    Ok(index)
}

/// Index just past the raw string starting at `start`, or `None` when the text
/// there is not a raw string prefix.
fn raw_string_end(source: &[char], start: usize) -> Result<Option<usize>, &'static str> {
    let mut cursor = start;
    if source[cursor] == 'b' {
        cursor += 1;
    }
    if source.get(cursor) != Some(&'r') {
        return Ok(None);
    }
    cursor += 1;
    let mut hashes = 0usize;
    while source.get(cursor) == Some(&'#') {
        hashes += 1;
        cursor += 1;
    }
    if source.get(cursor) != Some(&'"') {
        return Ok(None);
    }
    cursor += 1;
    let closing: Vec<char> = std::iter::once('"')
        .chain(std::iter::repeat_n('#', hashes))
        .collect();
    while cursor < source.len() {
        if source[cursor..].starts_with(closing.as_slice()) {
            return Ok(Some(cursor + closing.len()));
        }
        cursor += 1;
    }
    Err("a raw string is never closed, so the scan cannot be trusted")
}

/// Index just past the `"` closing the string literal opened at `start`.
fn string_end(source: &[char], start: usize) -> Result<usize, &'static str> {
    let mut index = start + 1;
    while index < source.len() {
        match source[index] {
            '\\' => index += 2,
            '"' => return Ok(index + 1),
            _ => index += 1,
        }
    }
    Err("a string literal is never closed, so the scan cannot be trusted")
}

/// Index just past the character literal at `start`, or `None` when the `'`
/// there opens a lifetime.
fn char_literal_end(source: &[char], start: usize) -> Result<Option<usize>, &'static str> {
    if source.get(start + 1) == Some(&'\\') {
        let mut cursor = start + 3;
        while cursor < source.len() && source[cursor] != '\'' {
            cursor += 1;
        }
        if cursor >= source.len() {
            return Err("a character literal is never closed, so the scan cannot be trusted");
        }
        return Ok(Some(cursor + 1));
    }
    if source.get(start + 2) == Some(&'\'') {
        return Ok(Some(start + 3));
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
