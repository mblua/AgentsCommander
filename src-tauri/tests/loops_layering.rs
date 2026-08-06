//! #1252 layering guard: nothing under `src/loops/` may reach up into the Tauri
//! command surface, except through the references `loops::delivery` already
//! carried before that issue.
//!
//! WHAT THIS GUARD IS, AND WHAT IT IS NOT.
//!
//! It is a net over the *spellings* a dependency can be written in, scanned out
//! of Rust source as text. It is not a proof that the dependency cannot return,
//! and it must not be read as one: it matches text, it does not resolve names,
//! so a spelling it does not know about passes it. The authoritative check is
//! the cycle detector run over the module graph, whose
//! `coverage.graphShape.cyclicSccs` must stay at 1. A green result here means
//! "no known spelling is present", never "the cycle is impossible".
//!
//! Widening the net is the only thing a text scan can do, so this file is
//! written to be widened: `ALLOWED_COMMAND_REFERENCES` is the whole contract,
//! and the spellings the scan is known to miss are listed below instead of
//! being left unsaid.
//!
//! Comments and the bodies of string and character literals are removed before
//! anything is matched, so neither can hide a path from the scan nor feed one
//! to it. A dependency is code; it is never a comment and never the body of a
//! literal.
//!
//! KNOWN UNCOVERED SPELLINGS.
//!
//! This list is maintained by the review loop. When a reviewer proves a spelling
//! that reaches the command surface from `src/loops/` and still passes this
//! file, it is appended here. Appending an entry is part of reviewing #1252 and
//! is expected; it changes nothing else.
//!
//!   1. Re-export laundering. A module outside `src/loops/` writes
//!      `pub use crate::commands::loops::...` and `src/loops/` imports from there.
//!      No `commands` token appears in the scanned files. The detector still
//!      catches it: the laundering module joins the cycle, so `cyclicSccs` rises.
//!   2. Macro-generated paths. A `macro_rules!` defined outside `src/loops/`, or
//!      any procedural macro, whose expansion contains the path. The text is not
//!      in the scanned files. Whether the detector resolves it has not been
//!      measured here, so do not assume it does.
//!   3. `#[path = "..."]` modules. The scan walks the `src/loops/` directory, not
//!      the module tree. A `loops` submodule pointed by `#[path]` at a file
//!      outside that directory is never read.
//!   4. `include!`. A file textually included from outside `src/loops/` is never
//!      read, for the same reason.
//!   5. Runtime indirection. A trait object, function pointer or callback whose
//!      only implementor lives in `commands::loops` and which is wired together
//!      outside `src/loops/`. No path text appears in the scanned files.
//!   6. `concat!` and friends. `concat!("crate::commands", "::loops")` builds
//!      the path text out of fragments none of which contains the anchor, and
//!      the bodies of those literals are removed before the scan in any case.
//!   7. (append here: one entry per spelling a reviewer proves still passes)

use std::path::{Path, PathBuf};

/// Every `(file, child)` reference under `src/loops/` that is allowed to name a
/// child of `crate::commands`, sorted.
///
/// `loops::delivery` has referenced `commands::pty` and `commands::session` since
/// before #1252; neither is in a cycle. Every other child of `commands`, and
/// `loops` above all, is refused, and so is either of these two named from any
/// other file. Adding a row here is a deliberate decision to accept a new upward
/// arc from the domain into the IPC surface.
///
/// The pair is the contract, not the child on its own. Keying on the child alone
/// made the observed set a union over every file, so
/// `use crate::commands::session::X;` added to `scheduler.rs` left that set at
/// `["pty", "session"]` and passed: a new arc from the domain into the IPC
/// surface that this guard could not see, written in ordinary rustfmt-clean and
/// clippy-clean Rust. The same union hid a reference moving out of
/// `delivery.rs` into another file.
///
/// One asymmetry to know about before trusting this table. The scan reads whole
/// files, `#[cfg(test)]` regions included; the detector is run with
/// `includeTests: false` and ignores them. Everywhere else that makes this guard
/// stricter than the detector, which is the safe direction. Here it makes it
/// laxer: a reference inside a `#[cfg(test)]` region of `delivery.rs` would hold
/// its pair up on its own, so deleting the production reference would leave the
/// set unmoved and the membership assertion silent about a reference the module
/// graph no longer has. It does not apply today, because all four references in
/// `delivery.rs` are production code, and it is written here because the
/// membership check is the thing somebody will be trusting on the day it stops
/// being true.
const ALLOWED_COMMAND_REFERENCES: [(&str, &str); 2] = [
    ("src/loops/delivery.rs", "pty"),
    ("src/loops/delivery.rs", "session"),
];

/// The child #1252 removed, called out separately so its failure carries the
/// explanation of the cycle rather than the generic allowlist message.
const FORBIDDEN_COMMAND_CHILD: &str = "loops";

const ANCHOR: &str = "commands::";

/// Replace every comment and every string or character literal with a single
/// space, leaving only code behind.
///
/// A comment is whitespace to the Rust lexer, so `commands /* x */ ::loops` is
/// the same path as `commands::loops`. Normalization collapses whitespace but
/// never removed comments, so that spelling broke the `commands::` anchor and
/// passed, and it was measured reintroducing the whole #1252 cycle
/// (`cyclicSccs = 2`) with this guard, `cargo fmt --check` and
/// `cargo clippy -- -D warnings` all green. This is a class rather than one
/// spelling: block, line, nested and multi-line comments all do it, which is why
/// it is closed here instead of being listed above. A comment becomes a space
/// and not nothing, because `as/* g */c` is two tokens and must not be welded
/// into `asc`.
///
/// Literal bodies go the same way, for three reasons. Tracking them is what
/// makes comment removal correct at all: `"https://host"` carries a `//` that
/// would otherwise blank the rest of its line and hide whatever followed. It
/// stops prose or a string from holding the observed set at its expected value
/// after the real references are deleted, which is the failure the membership
/// assertion below exists to catch. And it removes the false red where an
/// unclosed `commands::{` inside a doc comment or a string made the scan report
/// a group it could not delimit.
///
/// A literal or comment that never closes is an error rather than a truncated
/// result, for the same reason an unclosed group is: a scanner that cannot
/// delimit what it is reading must say so.
fn code_only(body: &str) -> Result<String, &'static str> {
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
                    index = cursor;
                    out.push(' ');
                    continue;
                }
            }
        }

        if character == '"' {
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
            out.push(' ');
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
                index = cursor + 1;
                out.push(' ');
                continue;
            }
            if source.get(index + 2) == Some(&'\'') {
                index += 3;
                out.push(' ');
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
/// crate::commands::{loops::A, session::B};` does not contain the text
/// `commands::loops` at all: the braces are in the way. Reflowed across lines by
/// rustfmt it does not contain it either. After normalization every one of those
/// forms is the same text, and the use-tree can be read.
fn normalized(body: &str) -> String {
    let mut out = body.split_whitespace().collect::<Vec<_>>().join(" ");
    for token in ["::", "{", "}", ","] {
        out = out.replace(&format!(" {token}"), token);
        out = out.replace(&format!("{token} "), token);
    }
    out
}

/// Whether the source renames the whole command group, as in
/// `use crate::commands as c;`.
///
/// After such a rename `c::loops::...` reaches the forbidden module under a name
/// no text scan can follow, so the rename itself is refused instead of followed.
/// Anchored on the path punctuation in front of `commands` so that English prose
/// about commands does not trip it.
fn aliases_the_command_group(body: &str) -> bool {
    ["::commands as ", "{commands as ", ",commands as "]
        .iter()
        .any(|spelling| body.contains(spelling))
}

/// The leading identifier of a use-tree item: `loops` from `loops::{a, b}`, from
/// `loops as l` and from `loops`. A non-identifier item such as `*` is returned
/// as itself, so a glob is reported rather than silently dropped.
///
/// A leading `r#` is dropped first. `r#loops` is the raw-identifier spelling of
/// `loops` and names the same module, but reading it literally stopped at the
/// `#` and reported the child as `r`, so the reference was caught by the
/// membership assertion instead of by the #1252 message that explains it.
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
/// as `loops::{a, b}, session::c` yields two items and not three.
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

/// Every child of `commands` named anywhere in `body`, which must already be
/// normalized, in source order.
///
/// An unclosed group is an error rather than an empty result: a scanner that
/// cannot delimit what it is reading must say so, because the alternative is a
/// green result that proves nothing.
fn command_children(body: &str) -> Result<Vec<String>, &'static str> {
    let mut children = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(ANCHOR) {
        let anchor_at = from + offset;
        let after = anchor_at + ANCHOR.len();
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
                return Err("a `commands::{` group is never closed, so the scan cannot be trusted");
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

/// Every file under `root`, sorted, filtered by nothing.
///
/// **Do not add an extension filter here.** `rustc` decides what to compile from
/// the module tree; a filter decides from the name, and production code lives in
/// the gap between the two. The obvious filter, `extension == "rs"`, was measured
/// letting the module that caused #1252 out of the scan in two ways. On a
/// case-insensitive filesystem `mod scheduler;` resolves `scheduler.RS` while
/// `"RS" == "rs"` is false, so renaming that one file removed it from the scan
/// and the cycle came back whole with this guard green. And
/// `#[path = "reentry.inc"]` compiles a file sitting right here in this directory
/// under a name no extension filter matches. Reading every file closes both, is
/// still a pure text scan, and costs nothing: there are five files.
///
/// Case-insensitive matching would close only the first of those two, so it is
/// not the fix.
///
/// `#[path]` pointing *outside* this directory, and `include!`, are a different
/// problem: reaching them needs the module tree resolved, which no filter can do.
/// They stay in KNOWN UNCOVERED SPELLINGS.
fn rust_sources(root: &Path) -> Vec<PathBuf> {
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

fn relative_of(path: &Path) -> String {
    path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("source is below manifest directory")
        .to_string_lossy()
        .replace('\\', "/")
}

/// #1252: `loops::scheduler` used to call `crate::commands::loops::emit_loop_change`,
/// which made the domain depend on the Tauri command surface and put the two
/// modules in a 2 member cycle. The emitter moved to `loops::events` so both
/// sides depend downward.
///
/// This test lives outside `src/loops/` on purpose. The first version lived
/// inside the scanned tree, so it had to excise itself before scanning by
/// cutting each file at its first `#[cfg(test)]`; any production code below a
/// mid-file test helper was invisible to it. Scanning from outside removes the
/// need for that cut, so whole files are read, test regions included. That is
/// stricter than the detector, which ignores `#[cfg(test)]` items, and strictness
/// is the safe direction for a guard: a false red is argued about, a false green
/// is believed.
///
/// What is not read is comments and the bodies of literals, which `code_only`
/// removes first. That is not a narrowing: neither can be a dependency, and
/// leaving them in was what let a comment inside a path hide the reference and
/// let prose hold the observed set up after the real references were gone.
#[test]
fn no_loops_source_reaches_into_the_command_surface() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loops");
    let files = rust_sources(&root);
    assert!(
        !files.is_empty(),
        "no Rust sources found under src/loops; the scan proves nothing"
    );

    let mut observed: Vec<(String, String)> = Vec::new();
    let mut loops_offenders: Vec<String> = Vec::new();
    let mut unlisted_offenders: Vec<String> = Vec::new();
    let mut alias_offenders: Vec<String> = Vec::new();

    for path in &files {
        let relative = relative_of(path);
        let source = std::fs::read_to_string(path).expect("read Rust source");
        let code = code_only(&source).unwrap_or_else(|reason| panic!("{relative}: {reason}"));
        let body = normalized(&code);
        let children =
            command_children(&body).unwrap_or_else(|reason| panic!("{relative}: {reason}"));
        if children
            .iter()
            .any(|child| child == FORBIDDEN_COMMAND_CHILD)
        {
            loops_offenders.push(relative.clone());
        }
        if children.iter().any(|child| {
            !ALLOWED_COMMAND_REFERENCES
                .iter()
                .any(|(file, allowed)| *file == relative && allowed == child)
        }) {
            unlisted_offenders.push(relative.clone());
        }
        if aliases_the_command_group(&body) {
            alias_offenders.push(relative.clone());
        }
        observed.extend(children.into_iter().map(|child| (relative.clone(), child)));
    }
    observed.sort();
    observed.dedup();

    assert!(
        loops_offenders.is_empty(),
        "src/loops must not reference commands::loops.\n\
         \n\
         WHY: `loops` is domain logic and `commands` is the Tauri IPC surface. \
         The domain must not depend on the surface it is announced through. \
         Issue #1252 removed the one call that did, \
         `crate::commands::loops::emit_loop_change` in loops/scheduler.rs, \
         because it put those two modules in a dependency cycle: \
         commands::loops needs LoopScheduler, so the scheduler must not need \
         commands::loops back. Any reference from here rebuilds that cycle.\n\
         \n\
         INSTEAD: emit Loop events through \
         `crate::loops::events::emit_loop_change`, which the command layer and \
         the scheduler both depend on downward. If you need something from \
         commands::loops that is not an event, it belongs in a module below \
         both of them, never above.\n\
         \n\
         SCOPE: this is a net over the spellings of that reference, not a proof \
         that it cannot return. It matches text and does not resolve names, so a \
         spelling it does not know about passes it; the ones it is known to miss \
         are listed at the top of this file. The authoritative check is the cycle \
         detector, whose `coverage.graphShape.cyclicSccs` must stay at 1.\n\
         \n\
         OFFENDING FILES: {}",
        loops_offenders.join(", ")
    );

    assert!(
        alias_offenders.is_empty(),
        "src/loops must not rename the command module group.\n\
         \n\
         WHY: `use crate::commands as <name>;` puts every module under `commands`, \
         `commands::loops` included, within reach under a name this scan cannot \
         follow. Following it would mean resolving names, which a text scan does \
         not do, so the rename is refused instead.\n\
         \n\
         INSTEAD: import the item you need by its real path, so this guard and \
         the cycle detector can both see it.\n\
         \n\
         OFFENDING FILES: {}",
        alias_offenders.join(", ")
    );

    let expected: Vec<(String, String)> = ALLOWED_COMMAND_REFERENCES
        .iter()
        .map(|(file, child)| ((*file).to_string(), (*child).to_string()))
        .collect();
    assert_eq!(
        observed,
        expected,
        "the set of command modules named from src/loops moved.\n\
         \n\
         FILES NAMING SOMETHING UNLISTED: {}\n\
         \n\
         Each entry is a (file, child) pair, because the file is half of the \
         rule. The two that are allowed, `commands::pty` and `commands::session` \
         in loops/delivery.rs, predate #1252 and are in no cycle. Naming either \
         of them from a different file under src/loops is a new arc from the \
         domain into the IPC surface, so it fails here even though the set of \
         children on its own would not have moved.\n\
         \n\
         A LARGER SET means src/loops gained a dependency on the Tauri command \
         surface. A third one is a decision, not a detail: remove it, or add its \
         pair to ALLOWED_COMMAND_REFERENCES and say in the commit why a new \
         upward arc from the domain into the IPC surface is acceptable.\n\
         \n\
         A SMALLER SET is the more dangerous failure. It usually means the scan \
         stopped seeing references it used to see, because `commands` was \
         renamed or moved or because this matcher was narrowed, and a guard that \
         observes nothing passes everything. Comments and literal bodies are \
         removed before the scan so that no amount of prose can hold this set \
         up while the real references disappear. The known references are \
         asserted by membership and not counted: an equal count is not an equal \
         set.",
        unlisted_offenders.join(", ")
    );
}
