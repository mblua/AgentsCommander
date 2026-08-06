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
//! written to be widened: `ALLOWED_COMMAND_CHILDREN` is the whole contract, and
//! the spellings the scan is known to miss are listed below instead of being
//! left unsaid.
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
//!   6. (append here: one entry per spelling a reviewer proves still passes)

use std::path::{Path, PathBuf};

/// The children of `crate::commands` that `src/loops/` is allowed to name, sorted.
///
/// `loops::delivery` has referenced `commands::pty` and `commands::session` since
/// before #1252; neither is in a cycle. Every other child of `commands`, and
/// `loops` above all, is refused. Adding a name here is a deliberate decision to
/// accept a new upward arc from the domain into the IPC surface.
const ALLOWED_COMMAND_CHILDREN: [&str; 2] = ["pty", "session"];

/// The child #1252 removed, called out separately so its failure carries the
/// explanation of the cycle rather than the generic allowlist message.
const FORBIDDEN_COMMAND_CHILD: &str = "loops";

const ANCHOR: &str = "commands::";

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
fn leading_segment(item: &str) -> String {
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

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read source directory") {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
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
#[test]
fn no_loops_source_reaches_into_the_command_surface() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loops");
    let files = rust_sources(&root);
    assert!(
        !files.is_empty(),
        "no Rust sources found under src/loops; the scan proves nothing"
    );

    let mut observed: Vec<String> = Vec::new();
    let mut loops_offenders: Vec<String> = Vec::new();
    let mut unlisted_offenders: Vec<String> = Vec::new();
    let mut alias_offenders: Vec<String> = Vec::new();

    for path in &files {
        let relative = relative_of(path);
        let body = normalized(&std::fs::read_to_string(path).expect("read Rust source"));
        let children =
            command_children(&body).unwrap_or_else(|reason| panic!("{relative}: {reason}"));
        if children
            .iter()
            .any(|child| child == FORBIDDEN_COMMAND_CHILD)
        {
            loops_offenders.push(relative.clone());
        }
        if children
            .iter()
            .any(|child| !ALLOWED_COMMAND_CHILDREN.contains(&child.as_str()))
        {
            unlisted_offenders.push(relative.clone());
        }
        if aliases_the_command_group(&body) {
            alias_offenders.push(relative.clone());
        }
        observed.extend(children);
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

    let expected: Vec<String> = ALLOWED_COMMAND_CHILDREN
        .iter()
        .map(|child| (*child).to_string())
        .collect();
    assert_eq!(
        observed,
        expected,
        "the set of command modules named from src/loops moved.\n\
         \n\
         FILES NAMING SOMETHING UNLISTED: {}\n\
         \n\
         A LARGER SET means src/loops gained a dependency on the Tauri command \
         surface. The two that are allowed, `commands::pty` and \
         `commands::session` in loops/delivery.rs, predate #1252 and are in no \
         cycle. A third is a decision, not a detail: remove it, or add its name \
         to ALLOWED_COMMAND_CHILDREN and say in the commit why a new upward arc \
         from the domain into the IPC surface is acceptable.\n\
         \n\
         A SMALLER SET is the more dangerous failure. It usually means the scan \
         stopped seeing references it used to see, because `commands` was \
         renamed or moved or because this matcher was narrowed, and a guard that \
         observes nothing passes everything. The known references are therefore \
         asserted by membership and not counted: an equal count is not an equal \
         set.",
        unlisted_offenders.join(", ")
    );
}
