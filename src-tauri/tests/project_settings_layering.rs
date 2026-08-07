//! #1265 layering guard: `crate::commands::project_settings` may not name the
//! browser command dispatcher `crate::web::commands`, and the emitter module
//! `crate::web::event_broadcast` may not name anything but the WebSocket
//! fan-out.
//!
//! WHAT THIS GUARD IS, AND WHAT IT IS NOT.
//!
//! It is a net over the *spellings* a dependency can be written in, scanned out
//! of Rust source as text. It is not a proof that the dependency cannot return,
//! and it must not be read as one: it matches text, it does not resolve names,
//! so a spelling it does not know about passes it. The authoritative check is
//! the cycle detector run over the module graph, whose
//! `coverage.graphShape.cyclicSccs` must stay at 1 with the guarded module at
//! `sccSize 1`. A green result here means "no known spelling is present", never
//! "the cycle is impossible".
//!
//! WHY IT GUARDS TWO MODULES AND NOT ONE. #1265 took
//! `commands::project_settings` out of the knot by moving the emitter down into
//! `web::event_broadcast`, and the argument that the emitter module cannot be
//! absorbed by the knot rests on it having exactly one outgoing arc, to
//! `web::broadcast`, which itself has none. **That premise fails on an outgoing
//! arc, not an incoming one.** A single `use` in `src/web/event_broadcast.rs`
//! pointing at any knot member puts the guarded module straight back into the
//! knot and leaves the knot larger than it was before the change, and the
//! project-settings assertions below stay green throughout, because that file
//! never changed. So the emitter module is guarded too, by the same matcher
//! under three anchors: `crate::`, `web::` and `super::`.
//!
//! The third anchor is not symmetry, it is the emitter's neighbourhood. The
//! dispatcher `web::commands` is the emitter's SIBLING, so from inside
//! `src/web/` it is reachable as `super::commands` with no `web::` token
//! anywhere, which the first two anchors cannot see. That spelling is the idiom
//! the neighbouring file already uses (`src/web/commands.rs:12` writes
//! `use super::broadcast::WsBroadcaster;`), so it is the first thing a reader
//! of that directory would copy. `commands::project_settings` needs no such
//! anchor: it is not a sibling of anything under `web`, so every path from
//! there into the dispatcher must spell `web` followed by `::`, or rename a
//! group, which is refused by name.
//!
//! WHAT IT READS. Not a directory. The files it scans are resolved by walking
//! `mod` declarations down from `src/lib.rs`, honouring `path = "..."` in both
//! `#[path]` and `#[cfg_attr(..., path = ...)]`, and collecting **every**
//! declaration of a segment rather than the first, so a module declared twice
//! under opposite `cfg`s contributes both files. A directory walk decides from
//! names; the compiler decides from the module tree, and code lives in the gap.
//!
//! **This resolver is not rustc and does not claim to be.** It over-reads on
//! purpose: `cfg` is not evaluated, so both arms of a platform module are
//! scanned even though only one is compiled. Reading a file rustc does not
//! compile costs a false red, which is argued about; missing one costs a false
//! green, which is believed. Where it cannot over-read safely it refuses: two
//! candidate files for one declaration, or a `mod x;` nested inside an inline
//! `mod y { ... }` block, are hard failures naming the file rather than a guess.
//!
//! Comments and the bodies of string and character literals are removed before
//! anything is matched: neither can be a dependency, neither may hide a path
//! from the scan, and neither may feed one to it.
//!
//! Widening the net is the only thing a text scan can do, so this file is
//! written to be widened: the three `ALLOWED_*` tables are the whole contract,
//! and the spellings the scan is known to miss are listed below instead of being
//! left unsaid.
//!
//! KNOWN UNCOVERED SPELLINGS.
//!
//! This list is maintained by the review loop. When a reviewer proves a spelling
//! that reaches the browser command dispatcher from the guarded module and still
//! passes this file, it is appended here. Appending an entry is part of
//! reviewing #1265 and is expected; it changes nothing else.
//!
//! **This file is the canonical copy.** Section 5.5 of
//! `plans/1265-extract-project-settings-from-scc.md` quotes it verbatim, but that
//! quote is a snapshot taken when the plan was certified. The first appended
//! entry makes the two diverge, and that is expected: this file runs, the plan
//! does not. Append here and leave the plan alone.
//!
//! **"The detector still catches it" is not a closure.** Several entries below
//! say so and it is true and measured, but the whole reason this file exists is
//! that the detector is run by hand and is deliberately not wired to CI. An
//! entry the detector catches is still uncovered *here*, and still reaches
//! nobody until somebody remembers to run the instrument.
//!
//!   1. Re-export laundering. A third module writes
//!      `pub use crate::web::commands::broadcast_all;` and the guarded module
//!      imports from there. No `web::commands` token appears in the scanned
//!      files. The detector still catches it: the laundering module gains the
//!      arc, the guarded module reaches the knot through it, and the knot grows
//!      instead of shrinking.
//!   2. Macro-generated paths. A `macro_rules!` defined elsewhere, or any
//!      procedural macro, whose expansion contains the path. The text is not in
//!      the scanned files. Whether the detector resolves it has not been
//!      measured here, so do not assume it does.
//!   3. `include!`. A file textually included from outside the module tree is
//!      pulled in without a `mod` declaration, so walking the tree does not
//!      reach it.
//!   4. Runtime indirection. A trait object, function pointer or callback whose
//!      only implementor lives in the dispatcher and which is wired together
//!      outside the guarded module. No path text appears in the scanned files.
//!   5. `concat!` and friends. `concat!("crate::web", "::commands")` builds the
//!      path text out of fragments none of which contains the anchor, and the
//!      bodies of those literals are removed before the scan in any case.
//!   6. A `mod x;` declaration nested inside an inline `mod y { ... }` block.
//!      rustc resolves it against the inline module's own directory and this
//!      resolver does not, so it would scan a file rustc does not compile.
//!      **It used to pass silently whenever a file happened to exist at the
//!      path this resolver looks in; that was measured.** It is now refused:
//!      `module_body` rejects the whole file with a hard failure naming it. The
//!      spelling is still uncovered in the sense that the reference is not
//!      read, but it can no longer be read as green.
//!   7. NTFS alternate data streams. `#[path = "carrier.rs:evil"]` compiles from
//!      a stream that carries code the resolver does open by path, but a `mod`
//!      declaration hidden inside a stream of another file is not reachable.
//!      Git stores only the main stream, so a clone has no `:evil` and the build
//!      fails rather than hiding anything.
//!   8. Laundering through the PARENT module, FROM THE GUARDED MODULE ONLY.
//!      `commands/mod.rs` re-exports the dispatcher and
//!      `commands/project_settings.rs` reaches it as `super::<name>`, in a
//!      `use` declaration or in an expression path. No `web::` token appears
//!      there at all, and that file is read under two anchors only, so nothing
//!      matches. Measured green in both forms. **The emitter module is not
//!      exposed this way**: it is read under `super::` as well, so the same
//!      laundering from `src/web/event_broadcast.rs` is refused. The detector
//!      does catch it, in both forms, also measured: the arc
//!      `commands::project_settings -> commands` appears, and
//!      `web::commands -> commands::project_settings` closes the cycle, so the
//!      knot grows. See the note above about what that is worth.
//!   9. Aliasing the crate root or a parent, other than the two spellings
//!      `aliases_a_module_group` knows. `use crate as c;` and `use crate::web as
//!      w;` are refused by name; a rename reached some other way is not.
//!  10. A path assembled across a `cfg` boundary in a way the resolver
//!      over-reads into but the equality tables do not distinguish. This
//!      resolver scans both arms of a platform module, so a forbidden reference
//!      in either arm is caught, but which arm rustc compiled is not known here
//!      and the failure message cannot say.
//!  11. `broadcast_all_r` moving. After #1265 there are two dual-transport
//!      emitters in two modules: `broadcast_all` in `web::event_broadcast` and
//!      `broadcast_all_r` in `web::commands`. Section 3.2 of the plan closes the
//!      decision to leave `broadcast_all_r` where it is, because moving it would
//!      delete `commands::ac_discovery -> web::commands`, an arc nobody asked to
//!      remove. **Nothing in this file or in the suite enforces that.** The day
//!      somebody moves it "for symmetry", no test goes red.
//!  12. A reference inside a `#[cfg(test)]` region holding an equality up on its
//!      own. Whole files are read, test regions included, while the detector
//!      ignores them. Everywhere else that makes this guard stricter, which is
//!      the safe direction; here it makes it laxer. Measured: deleting the
//!      production `use crate::web::broadcast::WsBroadcaster;` from the emitter
//!      module leaves both of its equalities satisfied, because the test
//!      module's own `use crate::web::broadcast::{WsBroadcaster, WsOutMsg};`
//!      names the same child. **It is not exploitable as it stands**, because
//!      that deletion does not compile: the type is in `broadcast_all`'s
//!      signature. It is written down because the shrinking-set argument is the
//!      thing somebody will be trusting on the day it stops being true. The
//!      same asymmetry is recorded in `loops_layering.rs` for #1252.
//!  13. (append here: one entry per spelling a reviewer proves still passes)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Every `(file, child)` reference the guarded module is allowed to make into a
/// child of `crate::web`, sorted.
///
/// `broadcast` is the `WsBroadcaster` type in the signature of the
/// `update_project_groups` command and predates #1265. `event_broadcast` is the
/// emitter #1265 moved below both surfaces, and it is listed **because it must
/// be there**: this is an equality, so if that import silently disappears the
/// assertion fails rather than passing quieter.
///
/// The pair is the contract, not the child on its own. Keying on the child alone
/// would make the observed set a union over every scanned file, so a reference
/// added to a future submodule of this module would leave the set unmoved and
/// pass. Adding a row here is a deliberate decision to accept a new dependency
/// from this command onto a new part of the web transport.
const ALLOWED_WEB_REFERENCES: [(&str, &str); 2] = [
    ("src/commands/project_settings.rs", "broadcast"),
    ("src/commands/project_settings.rs", "event_broadcast"),
];

/// Every `(file, child)` reference the emitter module is allowed to make under
/// `crate::`, sorted.
///
/// One row. The emitter module's whole in-crate dependency is the `WsBroadcaster`
/// type in `broadcast_all`'s signature, and the non-absorption argument of the
/// plan's Section 4.3 is exactly the claim that this stays true.
const ALLOWED_EMITTER_CRATE_REFERENCES: [(&str, &str); 1] = [("src/web/event_broadcast.rs", "web")];

/// Every `(file, child)` reference the emitter module is allowed to make into a
/// child of `crate::web`, sorted.
///
/// Two equalities rather than one path, and this is deliberate: the `crate::`
/// table above pins the first segment and this one pins the second, and together
/// they admit `crate::web::broadcast` and nothing else. Expressing the contract
/// as one joined `web::broadcast` string would have needed a second matcher that
/// recurses through brace groups, where `children_under` already handles
/// `use crate::web::{broadcast::A, commands::B}` and `use crate::{web::A, x::B}`
/// correctly under each anchor. Reusing the audited matcher twice is worth more
/// than a prettier table.
const ALLOWED_EMITTER_WEB_REFERENCES: [(&str, &str); 1] =
    [("src/web/event_broadcast.rs", "broadcast")];

/// Every `(file, child)` reference the emitter module is allowed to make under
/// `super::`, sorted.
///
/// **This anchor exists only for the emitter module, and the asymmetry is the
/// point.** The dispatcher `web::commands` is the emitter's SIBLING: from inside
/// `src/web/`, `super::commands` reaches it without the text `web::` appearing
/// anywhere, so neither of the two tables above sees it. That is not an exotic
/// spelling, it is the idiom the neighbouring file already uses:
/// `src/web/commands.rs:12` writes `use super::broadcast::WsBroadcaster;`.
/// `commands::project_settings` needs no such anchor because it is not a sibling
/// of anything under `web`: every path from there into `web` must spell `web`
/// followed by `::`, or rename a group, which is refused separately.
///
/// The one allowed row is the test module reaching its own parent for the
/// function under test. **A glob is deliberately not allowed.** `use super::*;`
/// written at module level would pull `crate::web`'s children, `commands`
/// included, into scope under no name this scan could follow, and it is
/// indistinguishable by text from the same glob inside `mod tests`. That is why
/// Section 5.1 imports by name instead of globbing its parent.
const ALLOWED_EMITTER_SUPER_REFERENCES: [(&str, &str); 1] =
    [("src/web/event_broadcast.rs", "broadcast_all")];

/// The child #1265 removed, called out separately so its failure carries the
/// explanation of the cycle rather than the generic allowlist message.
const FORBIDDEN_WEB_CHILD: &str = "commands";

const ANCHOR: &str = "web::";
const CRATE_ANCHOR: &str = "crate::";
const SUPER_ANCHOR: &str = "super::";

/// The module this guard is written about, as path segments below `crate`.
const GUARDED_MODULE: [&str; 2] = ["commands", "project_settings"];

/// The module #1265 created to hold the emitter, as path segments below `crate`.
const EMITTER_MODULE: [&str; 2] = ["web", "event_broadcast"];

/// The emitter #1265 moved, and the one file that may define it.
///
/// The name only. `defines_emitter` decides what counts as a definition, because
/// `fn broadcast_all(` as a literal needle misses `fn broadcast_all (` and, more
/// to the point, misses a generic copy `fn broadcast_all<R: Runtime>(`, which is
/// the exact shape of the sibling `broadcast_all_r` that stays behind.
const EMITTER_NAME: &str = "fn broadcast_all";
const EMITTER_HOME: &str = "src/web/event_broadcast.rs";

/// Whether literal bodies survive `scrub`.
///
/// They must survive when the text is about to be read for `path = "..."`,
/// and must not when it is about to be read for dependencies or for structure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Literals {
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
fn scrub(body: &str, literals: Literals) -> Result<String, &'static str> {
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
fn normalized(body: &str) -> String {
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
fn aliases_a_module_group(body: &str) -> bool {
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
fn children_under(body: &str, anchor: &str) -> Result<Vec<String>, &'static str> {
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

/// Whether `body`, which must be scrubbed and normalized, defines the emitter.
///
/// The needle is the name, and what follows it decides. `fn broadcast_all_r(`
/// is not a definition of `broadcast_all`, so a following identifier character
/// disqualifies the hit; `fn broadcast_all (` and `fn broadcast_all<R: Runtime>(`
/// are definitions, so whitespace is skipped and both `(` and `<` count. A
/// generic copy is the shape that matters here: it is exactly how the sibling
/// `broadcast_all_r` is written, so it is the shape a copy would most naturally
/// take.
fn defines_emitter(body: &str) -> bool {
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(EMITTER_NAME) {
        let after = from + offset + EMITTER_NAME.len();
        let next = body[after..].trim_start().chars().next();
        if matches!(next, Some('(') | Some('<')) {
            return true;
        }
        from = after;
    }
    false
}

// ---------------------------------------------------------------------------
// Resolving what the compiler compiles
// ---------------------------------------------------------------------------

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn relative_of(path: &Path) -> String {
    path.strip_prefix(manifest_dir())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Read a file as text and remove its comments, keeping or dropping literals.
///
/// Bytes that are not valid UTF-8 are replaced rather than refused, so no file is
/// ever skipped for its encoding. A file that cannot be delimited afterwards is
/// still a hard failure.
fn scrubbed(path: &Path, literals: Literals) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", relative_of(path)))?;
    let text = String::from_utf8_lossy(&bytes);
    scrub(&text, literals).map_err(|reason| format!("{}: {reason}", relative_of(path)))
}

/// The directory rustc searches for the children of the module whose own file is
/// `file`: the file's own directory for a crate root or a `mod.rs`, and a
/// directory named after the file otherwise.
fn child_directory(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or(Path::new("."));
    let stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    if stem == "lib" || stem == "main" || stem == "mod" {
        parent.to_path_buf()
    } else {
        parent.join(stem)
    }
}

/// A file's two readings: `structure` has literal bodies removed and is what
/// braces and declarations are counted in; `with_literals` keeps them and is
/// what a `path = "..."` value is read out of.
struct ModuleBody {
    structure: String,
    with_literals: String,
}

/// Both readings of `owner`, after refusing the file outright if it declares a
/// module inside an inline module block.
///
/// The refusal is the honest answer to a case this resolver gets wrong: rustc
/// resolves `mod x;` inside `mod y { ... }` against `y`'s own directory, and
/// this resolver reads it as a child of the file. It used to resolve anyway,
/// to a different file, whenever one happened to exist at the path it looks in,
/// and then reported green having read the wrong file. A scanner that cannot
/// tell which file it should be reading has to say so.
fn module_body(owner: &Path) -> Result<ModuleBody, String> {
    let structure = normalized(&scrubbed(owner, Literals::Drop)?);
    if let Some(identifier) = nested_module_declaration(&structure) {
        return Err(format!(
            "{} declares `mod {identifier};` inside an inline `mod ... {{ ... }}` block. \
             rustc resolves that against the inline module's own directory and this resolver \
             does not, so the file it would scan is not the file rustc compiles. Refusing the \
             file rather than reading the wrong one: move the declaration to the top level of \
             its file.",
            relative_of(owner)
        ));
    }
    Ok(ModuleBody {
        structure,
        with_literals: normalized(&scrubbed(owner, Literals::Keep)?),
    })
}

/// The identifier of the first `mod <ident>;` that sits inside an inline
/// `mod ... { ... }` block, if there is one.
///
/// `body` must be scrubbed with `Literals::Drop` and normalized, because a brace
/// inside a string literal is not a block and would throw the depth off.
fn nested_module_declaration(body: &str) -> Option<String> {
    let mut depth = 0usize;
    for (index, character) in body.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 || !body[index..].starts_with("mod ") {
            continue;
        }
        let disqualified = body[..index]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if disqualified {
            continue;
        }
        let after = index + "mod ".len();
        let identifier: String = body[after..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '#')
            .collect();
        if !identifier.is_empty() && body[after + identifier.len()..].starts_with(';') {
            return Some(
                identifier
                    .strip_prefix("r#")
                    .unwrap_or(&identifier)
                    .to_string(),
            );
        }
    }
    None
}

/// Every byte offset at which `mod <segment>;` is declared in `body`, which must
/// be normalized. A preceding identifier character or quote disqualifies the hit,
/// so neither `submod x;` nor a `mod x;` sitting inside a string is read as one.
///
/// **All of them, not the first.** The standard per-platform module is two
/// declarations of one name under opposite `cfg`s, and reading only the first
/// means scanning the Unix file in a Windows build while reporting that the set
/// of files is the set rustc compiles.
fn find_declarations(body: &str, segment: &str) -> Vec<usize> {
    let needle = format!("mod {segment};");
    let mut found = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(&needle) {
        let at = from + offset;
        let disqualified = body[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"');
        if !disqualified {
            found.push(at);
        }
        from = at + needle.len();
    }
    found
}

/// The text of the item that ends at `declaration_at`, so its attributes can be
/// read without scanning back into the previous item.
fn attributes_before(body: &str, declaration_at: usize) -> &str {
    let start = body[..declaration_at]
        .rfind([';', '}', '{'])
        .map(|index| index + 1)
        .unwrap_or(0);
    &body[start..declaration_at]
}

/// Every file named by a `path = "..."` in the item's attributes, in order.
///
/// Both `#[path = "x.rs"]` and `#[cfg_attr(<cond>, path = "x.rs")]` are read.
/// Matching the bare key rather than the text `#[path` is what covers the second
/// form, which is otherwise invisible: the resolver would fall back to the
/// default candidates while rustc compiles the file the `cfg_attr` names.
fn path_attributes(attributes: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = attributes[from..].find("path") {
        let at = from + offset;
        let after = at + "path".len();
        let preceded_by_identifier = attributes[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let rest = attributes[after..].trim_start();
        if !preceded_by_identifier {
            if let Some(rest) = rest.strip_prefix('=') {
                if let Some(rest) = rest.trim_start().strip_prefix('"') {
                    if let Some(end) = rest.find('"') {
                        values.push(rest[..end].to_string());
                    }
                }
            }
        }
        from = after;
    }
    values
}

/// Every file rustc might compile for the child `segment` of the module whose
/// file is `owner`.
///
/// `owner_body` must be `ModuleBody::with_literals`, because the `path` value is
/// a literal.
///
/// Two rules earn their keep here and both are refusals rather than guesses:
///
/// - **A `path` value is resolved beside the owner file first.** For a `mod`
///   declaration at the top level of a file, rustc reads `path` relative to the
///   directory the file is in, not relative to the module's own subdirectory.
///   Trying the subdirectory first picks the file rustc does not compile
///   whenever both exist, which is the whole of a benign-decoy attack. Since
///   `module_body` refuses declarations nested in inline blocks, the case where
///   rustc would use the module directory cannot reach this function.
/// - **Two existing candidates for one declaration is a hard failure.** rustc
///   itself rejects `x.rs` and `x/mod.rs` both existing; for a `path` value, two
///   candidates is exactly the situation where this scan cannot know which file
///   it is meant to read, and the house rule is that it must then say so.
fn resolve_children(owner: &Path, owner_body: &str, segment: &str) -> Result<Vec<PathBuf>, String> {
    let declarations = find_declarations(owner_body, segment);
    if declarations.is_empty() {
        return Err(format!(
            "{} declares no `mod {segment};`",
            relative_of(owner)
        ));
    }

    let module_directory = child_directory(owner);
    let file_directory = owner.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut resolved = Vec::new();

    for at in declarations {
        let values = path_attributes(attributes_before(owner_body, at));
        if values.is_empty() {
            let named = module_directory.join(format!("{segment}.rs"));
            let directory = module_directory.join(segment).join("mod.rs");
            match (named.is_file(), directory.is_file()) {
                (true, true) => {
                    return Err(format!(
                        "{} declares `mod {segment};` and both {} and {} exist. rustc rejects \
                         that outright and this scan will not pick one.",
                        relative_of(owner),
                        relative_of(&named),
                        relative_of(&directory)
                    ))
                }
                (true, false) => resolved.push(named),
                (false, true) => resolved.push(directory),
                (false, false) => {
                    return Err(format!(
                        "{} declares `mod {segment};` but none of these files exists: {}, {}",
                        relative_of(owner),
                        relative_of(&named),
                        relative_of(&directory)
                    ))
                }
            }
            continue;
        }

        for value in values {
            let beside_the_file = file_directory.join(&value);
            let inside_the_module = module_directory.join(&value);
            let mut hits: Vec<PathBuf> = Vec::new();
            for candidate in [beside_the_file, inside_the_module] {
                if candidate.is_file() && !hits.contains(&candidate) {
                    hits.push(candidate);
                }
            }
            match hits.len() {
                1 => resolved.push(hits.remove(0)),
                0 => {
                    return Err(format!(
                        "{} declares `mod {segment};` with `path = \"{value}\"` and no file \
                         exists at {} or at {}",
                        relative_of(owner),
                        relative_of(&file_directory.join(&value)),
                        relative_of(&module_directory.join(&value))
                    ))
                }
                _ => {
                    return Err(format!(
                        "{} declares `mod {segment};` with `path = \"{value}\"` and both {} and \
                         {} exist. rustc compiles the one beside the file; this scan will not \
                         guess, because guessing wrong is how a forbidden reference stays \
                         unread. Remove one of them.",
                        relative_of(owner),
                        relative_of(&file_directory.join(&value)),
                        relative_of(&module_directory.join(&value))
                    ))
                }
            }
        }
    }

    resolved.sort();
    resolved.dedup();
    Ok(resolved)
}

/// Every `mod <ident>;` declared in `body`, which must be `ModuleBody::structure`,
/// deduplicated. `resolve_children` finds every declaration of each name, so one
/// entry per name is enough here.
fn declared_children(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find("mod ") {
        let at = from + offset;
        let after = at + "mod ".len();
        let disqualified = body[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"');
        if !disqualified {
            let identifier: String = body[after..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '#')
                .collect();
            if !identifier.is_empty() && body[after + identifier.len()..].starts_with(';') {
                found.push(
                    identifier
                        .strip_prefix("r#")
                        .unwrap_or(&identifier)
                        .to_string(),
                );
            }
        }
        from = after;
    }
    found.sort();
    found.dedup();
    found
}

/// The files rustc compiles for `module` and every module below it, resolved by
/// walking `mod` declarations down from the crate root.
///
/// The walk carries a frontier rather than a single file, because a segment can
/// be declared more than once under opposite `cfg`s and this resolver keeps both
/// arms. An error at any step is propagated rather than skipped: a module that
/// cannot be located is the one case where reading nothing must not look like
/// reading nothing forbidden.
fn sources_of(module: &[&str]) -> Result<Vec<PathBuf>, String> {
    let root = manifest_dir().join("src").join("lib.rs");
    if !root.is_file() {
        return Err(format!("{} does not exist", relative_of(&root)));
    }

    let mut frontier = vec![root];
    for segment in module {
        let mut next = Vec::new();
        for owner in &frontier {
            next.extend(resolve_children(
                owner,
                &module_body(owner)?.with_literals,
                segment,
            )?);
        }
        next.sort();
        next.dedup();
        frontier = next;
    }

    let mut files = Vec::new();
    let mut queue = frontier;
    while let Some(current) = queue.pop() {
        if files.contains(&current) {
            continue;
        }
        let body = module_body(&current)?;
        for child in declared_children(&body.structure) {
            queue.extend(resolve_children(&current, &body.with_literals, &child)?);
        }
        files.push(current);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Every file rustc compiles for the whole crate, as repository-relative paths.
///
/// Used for one question only, and computed lazily because it is the expensive
/// and failure-prone walk: when a file under `src/` cannot be delimited, is it a
/// file the compiler reads? A `.md` that is not in the module tree cannot hold a
/// definition of anything and must not turn a layering guard red. A `.rs` file
/// the compiler does read and this scan could not is the opposite, and must.
///
/// A file whose own body cannot be read is **recorded as reached and not
/// descended into**, which is the only sensible answer: the question being asked
/// is exactly "is this unreadable file in the tree", so failing the walk because
/// of it would refuse to answer the question it was called to answer. Its
/// children are lost, and that is stated rather than hidden.
fn crate_sources() -> Result<BTreeSet<String>, String> {
    let root = manifest_dir().join("src").join("lib.rs");
    if !root.is_file() {
        return Err(format!("{} does not exist", relative_of(&root)));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut queue = vec![root];
    while let Some(current) = queue.pop() {
        if files.contains(&current) {
            continue;
        }
        let Ok(body) = module_body(&current) else {
            files.push(current);
            continue;
        };
        for child in declared_children(&body.structure) {
            queue.extend(resolve_children(&current, &body.with_literals, &child)?);
        }
        files.push(current);
    }
    Ok(files.iter().map(|path| relative_of(path)).collect())
}

/// Every file under `root`, sorted, filtered by nothing.
///
/// **Do not add an extension filter here.** `rustc` decides what to compile from
/// the module tree; a filter decides from the name, and production code lives in
/// the gap between the two. On a case-insensitive filesystem `mod x;` resolves
/// `x.RS` while `"RS" == "rs"` is false, and `#[path = "carrier.inc"]` compiles a
/// file no extension filter matches. Reading every file closes both and is still
/// a pure text scan.
fn every_file_under(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read source directory") {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files.sort();
    files
}

/// What one module's files were observed to name.
///
/// `web` and `anchored` are `(file, child)` pairs under `web::` and under
/// `crate::` respectively; `aliases` is the files that rename a module group.
struct Observation {
    web: Vec<(String, String)>,
    anchored: Vec<(String, String)>,
    relative_up: Vec<(String, String)>,
    aliases: Vec<String>,
}

/// Read every file of `module` and report what it names.
///
/// A file reached through the module tree is a file rustc compiles, so a `scrub`
/// failure on one of them is fatal here and says so: it is source the compiler
/// reads and this scan could not.
fn observe(module: &[&str]) -> Observation {
    let files = sources_of(module).unwrap_or_else(|reason| {
        panic!(
            "the module {module:?} could not be resolved from the module tree, so this scan \
             proves nothing: {reason}\n\
             \n\
             WHY THIS IS A FAILURE AND NOT A SKIP: this guard exists to prove that a specific \
             dependency is absent. If the module cannot be located, the guard has read nothing \
             and must say so rather than pass. Rename or move the module and this message names \
             the file whose `mod` declaration no longer resolves; update GUARDED_MODULE or \
             EMITTER_MODULE, or the declaration, to match."
        )
    });
    assert!(
        !files.is_empty(),
        "the module {module:?} resolved to no files at all; the scan proves nothing"
    );

    let mut web = Vec::new();
    let mut anchored = Vec::new();
    let mut relative_up = Vec::new();
    let mut aliases = Vec::new();
    for path in &files {
        let relative = relative_of(path);
        let code = scrubbed(path, Literals::Drop).unwrap_or_else(|reason| {
            panic!(
                "{reason}\n\
                 \n\
                 This file is in the module tree, so rustc compiles it and this scan could not \
                 read it. That is a hard failure, not a skip."
            )
        });
        let body = normalized(&code);
        let name = |children: Result<Vec<String>, &'static str>| {
            children
                .unwrap_or_else(|reason| panic!("{relative}: {reason}"))
                .into_iter()
                .map(|child| (relative.clone(), child))
                .collect::<Vec<_>>()
        };
        web.extend(name(children_under(&body, ANCHOR)));
        anchored.extend(name(children_under(&body, CRATE_ANCHOR)));
        relative_up.extend(name(children_under(&body, SUPER_ANCHOR)));
        if aliases_a_module_group(&body) {
            aliases.push(relative.clone());
        }
    }
    web.sort();
    web.dedup();
    anchored.sort();
    anchored.dedup();
    relative_up.sort();
    relative_up.dedup();
    Observation {
        web,
        anchored,
        relative_up,
        aliases,
    }
}

fn expected(table: &[(&str, &str)]) -> Vec<(String, String)> {
    table
        .iter()
        .map(|(file, child)| ((*file).to_string(), (*child).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// The guards
// ---------------------------------------------------------------------------

/// #1265: `commands::project_settings` used to call
/// `crate::web::commands::broadcast_all`, which put the Tauri command and the
/// browser dispatcher in a mutual pair and held the command inside an 89 module
/// cyclic SCC. The emitter moved to `web::event_broadcast` so both surfaces
/// depend downward on it.
///
/// This test lives in `src-tauri/tests/`, which is a separate leaf crate the
/// detector marks `enabled: opts.includeTests` and the record is emitted with
/// `includeTests: false`. It therefore adds no arc and no module, is outside the
/// tree it reads, and never has to excise itself from its own scan. Whole files
/// are read, `#[cfg(test)]` regions included, which is stricter than the
/// detector: a false red is argued about, a false green is believed.
#[test]
fn project_settings_names_no_web_module_above_it() {
    let seen = observe(&GUARDED_MODULE);
    let observed = seen.web;
    let alias_offenders = seen.aliases;

    let dispatcher_offenders: Vec<String> = observed
        .iter()
        .filter(|(_, child)| child == FORBIDDEN_WEB_CHILD)
        .map(|(file, _)| file.clone())
        .collect();
    let allowed = expected(&ALLOWED_WEB_REFERENCES);
    let unlisted_offenders: Vec<String> = observed
        .iter()
        .filter(|pair| !allowed.contains(pair))
        .map(|(file, _)| file.clone())
        .collect();

    assert!(
        dispatcher_offenders.is_empty(),
        "commands::project_settings must not reference web::commands.\n\
         \n\
         WHY: `web::commands` is the browser IPC dispatcher and this module is a \
         Tauri IPC command. Two transport surfaces must not depend on each other, \
         and the dispatcher already depends on this module for its \
         `get_project_groups_inner`, `update_project_groups_inner`, \
         `project_groups_updated_payload` and `PROJECT_GROUPS_UPDATED_EVENT`. \
         Issue #1265 removed the one call going the other way, \
         `crate::web::commands::broadcast_all`, because that mutual pair was the \
         only thing holding this module inside the crate's 89 module cyclic SCC. \
         Any reference from here puts it back in.\n\
         \n\
         INSTEAD: emit through `crate::web::event_broadcast::broadcast_all`, which \
         the dispatcher and this module both depend on downward. If you need \
         something from the dispatcher that is not an emission, it belongs in a \
         module below both of them, never in either one.\n\
         \n\
         SCOPE: this is a net over the spellings of that reference, not a proof \
         that it cannot return. It matches text and does not resolve names, so a \
         spelling it does not know about passes it; the ones it is known to miss \
         are listed at the top of this file. The authoritative check is the cycle \
         detector, whose `coverage.graphShape.cyclicSccs` must stay at 1 with \
         `sccSize(agentscommander_lib::commands::project_settings) = 1`.\n\
         \n\
         OFFENDING FILES: {}",
        dispatcher_offenders.join(", ")
    );

    assert!(
        alias_offenders.is_empty(),
        "commands::project_settings must not rename the web module group or the \
         crate root.\n\
         \n\
         WHY: `use crate::web as <name>;`, `use crate::web::{{self as <name>}};` \
         and `use crate as <name>;` each put every module under `web`, \
         `web::commands` included, within reach under a name this scan cannot \
         follow. Following it would mean resolving names, which a text scan does \
         not do, so the rename is refused instead.\n\
         \n\
         INSTEAD: import the item you need by its real path, so this guard and \
         the cycle detector can both see it.\n\
         \n\
         OFFENDING FILES: {}",
        alias_offenders.join(", ")
    );

    assert_eq!(
        observed,
        allowed,
        "the set of web modules named from commands::project_settings moved.\n\
         \n\
         FILES NAMING SOMETHING UNLISTED: {}\n\
         \n\
         Each entry is a (file, child) pair, because the file is half of the \
         rule. Naming an allowed child from a different file of this module's \
         subtree is still a new dependency, so it fails here even though the set \
         of children on its own would not have moved.\n\
         \n\
         A LARGER SET means this command reached further into the web transport. \
         That is a decision, not a detail: remove it, or add its pair to \
         ALLOWED_WEB_REFERENCES and say in the commit why the new dependency is \
         acceptable.\n\
         \n\
         A SMALLER SET is the more dangerous failure, and it is why this is an \
         equality and not a denylist. `event_broadcast` is listed because #1265 \
         put it there: if that import silently disappears, the emitter has been \
         reached some other way and the reason this module is out of the cycle \
         has changed without anybody saying so. A shrinking set also means the \
         scan may have stopped seeing references it used to see, and a guard that \
         observes nothing passes everything. Comments and literal bodies are \
         removed before the scan so no amount of prose can hold this set up while \
         the real references disappear.",
        unlisted_offenders.join(", ")
    );
}

/// #1265 Section 4.3: the emitter module cannot be absorbed by the knot because
/// its only outgoing arc goes to `web::broadcast`, which has none of its own.
///
/// **That is a claim about outgoing arcs, and this test is the only thing that
/// holds it.** Measured on the arc record: adding one arc from
/// `web::event_broadcast` to any knot member takes the knot from 88 back to 90
/// and puts `commands::project_settings` back inside it, leaving the crate worse
/// than before #1265, while every assertion in the test above stays green,
/// because that module's own file did not change.
///
/// Two equalities, one per anchor. `crate::` pins the first segment and `web::`
/// pins the second, and together they admit `crate::web::broadcast` and nothing
/// else. The alias check is here for the same reason it is above: a renamed
/// group is a path this scan cannot follow.
#[test]
fn the_emitter_home_names_nothing_but_the_websocket_fan_out() {
    let seen = observe(&EMITTER_MODULE);
    let alias_offenders = seen.aliases;

    assert!(
        alias_offenders.is_empty(),
        "web::event_broadcast must not rename the web module group or the crate \
         root; see the same assertion for commands::project_settings.\n\
         \n\
         OFFENDING FILES: {}",
        alias_offenders.join(", ")
    );

    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_EMITTER_CRATE_REFERENCES),
        "the set of crate modules named from web::event_broadcast moved.\n\
         \n\
         WHY THIS MATTERS MORE THAN IT LOOKS: #1265 is only correct while this \
         module cannot reach the cyclic SCC. It has one in-crate dependency, the \
         `WsBroadcaster` type in `broadcast_all`'s signature, and that is what \
         makes the non-absorption argument of the plan's Section 4.3 true. One \
         `use` from here into any module of the knot puts \
         `commands::project_settings` back inside it and leaves the knot LARGER \
         than it was before #1265, and no other test in this repository would go \
         red.\n\
         \n\
         INSTEAD: if this module needs something else, that something belongs \
         below it, and the plan's Section 4.3 has to be rewritten before the \
         dependency is added. Adding a row to ALLOWED_EMITTER_CRATE_REFERENCES is \
         a decision about the crate's shape, not a detail.\n\
         \n\
         A SMALLER SET is a failure too: `web` is listed because the emitter's \
         signature needs `WsBroadcaster`, so if it disappears the module has \
         changed shape or the scan has stopped seeing it."
    );

    assert_eq!(
        seen.web,
        expected(&ALLOWED_EMITTER_WEB_REFERENCES),
        "the set of web modules named from web::event_broadcast moved.\n\
         \n\
         `broadcast` is the WebSocket fan-out and is the only child of `web` this \
         module may name. `commands` here would be the same cycle #1265 removed, \
         written from the other end: the emitter would depend on the dispatcher \
         that depends on the command that depends on the emitter.\n\
         \n\
         See the crate-anchored assertion above for why a change here is a \
         decision about the crate's shape."
    );

    assert_eq!(
        seen.relative_up,
        expected(&ALLOWED_EMITTER_SUPER_REFERENCES),
        "the set of names reached by `super::` from web::event_broadcast moved.\n\
         \n\
         WHY THIS ANCHOR EXISTS AT ALL: the dispatcher `web::commands` is this \
         module's SIBLING. From inside src/web/, `use super::commands::X;` \
         reaches it without the text `web::` appearing anywhere, so neither of \
         the two assertions above can see it, and it would rebuild the #1265 \
         cycle in silence. This is not an exotic spelling: the neighbouring \
         file writes `use super::broadcast::WsBroadcaster;` at line 12, so it \
         is the first thing a reader of src/web/ would copy.\n\
         \n\
         INSTEAD: name what you need by its `crate::` path, the way the two \
         production imports in this module already do. Then the arc it creates \
         is visible to the record, to the two assertions above, and to anyone \
         reading the file.\n\
         \n\
         A GLOB FAILS HERE ON PURPOSE. `use super::*;` at module level pulls \
         `crate::web`'s children, `commands` included, into scope under no name \
         a text scan can follow, and it is indistinguishable by text from the \
         same glob inside `mod tests`. The test module therefore imports \
         `broadcast_all` by name, which is the one row this table allows.\n\
         \n\
         `commands::project_settings` has no equivalent assertion because it is \
         not a sibling of anything under `web`: every path from there into the \
         dispatcher must spell `web` followed by `::`, or rename a group, which \
         is refused separately."
    );
}

/// #1265 criterion 8: the emitter moved, it was not copied. Two copies would
/// drift and the layering claim would be false while every arc assertion still
/// passed.
///
/// This reads every file under `src/`, filtered by nothing, because a duplicate
/// can be parked anywhere. Reading everything means reading files that are not
/// Rust, and `scrub` cannot delimit arbitrary text: a Markdown file with an odd
/// number of `"` is not a defect in this tree and must not turn a layering guard
/// red. So a file this scan cannot delimit is fatal only when the module tree
/// reaches it, which is to say only when rustc compiles it and this scan could
/// not read source the compiler reads. **Do not turn this into an extension
/// filter**: the reason for reading everything is in `every_file_under` and it
/// has not changed.
#[test]
fn the_dual_transport_emitter_is_defined_exactly_once() {
    let source_root = manifest_dir().join("src");
    let files = every_file_under(&source_root);
    assert!(
        !files.is_empty(),
        "no files found under src; the scan proves nothing"
    );

    let mut homes: Vec<String> = Vec::new();
    let mut unreadable: Vec<(String, String)> = Vec::new();
    for path in &files {
        let relative = relative_of(path);
        match scrubbed(path, Literals::Drop) {
            Ok(code) => {
                if defines_emitter(&normalized(&code)) {
                    homes.push(relative);
                }
            }
            Err(reason) => unreadable.push((relative, reason)),
        }
    }

    if !unreadable.is_empty() {
        let compiled = crate_sources().unwrap_or_else(|reason| {
            panic!(
                "a file under src could not be delimited, and the module tree could not be \
                 resolved to decide whether rustc compiles it, so this scan proves nothing: \
                 {reason}"
            )
        });
        let fatal: Vec<String> = unreadable
            .iter()
            .filter(|(relative, _)| compiled.contains(relative))
            .map(|(_, reason)| reason.clone())
            .collect();
        let none: Vec<String> = Vec::new();
        assert_eq!(
            fatal, none,
            "a file the compiler reads could not be delimited, so this scan did not read it \
             and cannot claim the emitter is defined exactly once.\n\
             \n\
             WHY THIS IS A FAILURE AND NOT A SKIP: an unread file that rustc compiles is \
             exactly where a second definition would survive. A scan that quietly skips what \
             it cannot parse passes for the wrong reason.\n\
             \n\
             WHAT IT USUALLY IS: an unterminated string, character literal or block comment. \
             A Rust file in that state does not compile either, so fix the file. Files under \
             `src/` that are NOT in the module tree, such as Markdown, are reported nowhere \
             and are not failures: they cannot define anything.\n\
             \n\
             **Do not add an extension filter to `every_file_under`**: rustc decides what to \
             compile from the module tree, a filter decides from the name, and production \
             code lives in the gap between the two.\n\
             \n\
             FILES THE COMPILER READS THAT COULD NOT BE DELIMITED: {fatal:?}"
        );
    }

    assert_eq!(
        homes,
        vec![EMITTER_HOME.to_string()],
        "the dual-transport emitter must be defined exactly once, in {EMITTER_HOME}.\n\
         \n\
         WHY: #1265 moved `broadcast_all` out of the browser command dispatcher \
         so that a Tauri command would stop depending on it. A move that left a \
         copy behind satisfies every arc assertion and is still wrong: the two \
         copies drift, and the claim that this module is the only home of the \
         emitter stops being true.\n\
         \n\
         WHAT COUNTS AS A DEFINITION: the name `broadcast_all` followed, after \
         any whitespace, by `(` or `<`. The generic form is included on purpose: \
         `fn broadcast_all<R: Runtime>(...)` is a copy of this emitter and it is \
         the exact shape of the sibling `broadcast_all_r` that stays in \
         `web::commands`, so it is the shape a copy would most naturally take.\n\
         \n\
         INSTEAD: keep one definition. If a second transport needs a variant, \
         give it a different name and a reason, in {EMITTER_HOME} beside this \
         one.\n\
         \n\
         MORE THAN ONE ENTRY means it was copied rather than moved. NO ENTRY \
         means it was renamed, deleted, or spelled in a way this scan does not \
         recognise, and a guard that finds nothing must fail rather than pass. \
         The list is asserted by equality and not counted: an equal count is not \
         an equal set.\n\
         \n\
         OBSERVED: {homes:?}"
    );
}
