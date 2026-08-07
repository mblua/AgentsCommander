//! #1273 layering guard: `crate::config::instance_gitignore` may not name
//! `crate::config::root_agent`, nor anything else that can reach the crate's
//! cyclic SCC, and the module that now holds `ROOT_AGENT_DIR_NAME`,
//! `crate::config`, may not name anything at all.
//!
//! WHAT THIS GUARD IS, AND WHAT IT IS NOT.
//!
//! It is a net over the *spellings* a dependency can be written in, scanned out
//! of Rust source as text. It is not a proof that the dependency cannot return,
//! and it must not be read as one: it matches text, it does not resolve names,
//! so a spelling it does not know about passes it. The authoritative check is
//! the cycle detector run over the module graph, whose
//! `coverage.graphShape.cyclicSccs` must stay at 1 with
//! `sccSize(agentscommander_lib::config::instance_gitignore) = 1`. A green
//! result here means "no known spelling is present", never "the cycle is
//! impossible".
//!
//! WHY BOTH ANCHORS ARE ASSERTED, FROM DAY ONE. Every reference this module
//! makes is written `super::` or `super::super::`; it contains no `crate::` at
//! all. A guard that asserted only the `crate::`-anchored set would therefore
//! observe **nothing** in this module and pass everything, which is exactly the
//! failure `project_settings_layering.rs` records against itself as entry 13 of
//! its own uncovered list and issue #1268 tracks: adding
//! `use crate::session::manager::SessionManager;` to that guarded module moved
//! the knot 88 to 89 and `sccSize` 1 to 89 with that file green throughout,
//! because its `crate::`-anchored set was collected and never asserted. Here
//! the two sets are asserted by equality, both of them, and the `crate::` table
//! is deliberately **empty**: this module names nothing under `crate::` today
//! and any first entry is a decision about the crate's shape.
//!
//! WHY THE CONSTANT'S NEW HOME IS GUARDED TOO. #1273 took
//! `config::instance_gitignore` out of the knot by moving one `pub const` down
//! into `crate::config`, and the argument that the knot cannot absorb it rests
//! on `crate::config` having **zero outgoing arcs**, measured over the 976 of
//! `src-tauri/module-arcs.txt`. **That premise fails on an outgoing arc, not an
//! incoming one.** One `use crate::<any knot member>::...;` in `src/config/mod.rs`
//! puts `config` into the knot and drags this module straight back in with it,
//! and the assertions about `instance_gitignore` stay green throughout, because
//! that file never changed. The premise is load bearing either way: the arc
//! `config::instance_gitignore -> config` is not removed by #1273 and is not
//! removable, so `config` has to stay clean whatever else happens.
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
//! The guarded module is read together with every module below it. The constant's
//! home is read **shallow**, its own `src/config/mod.rs` and no descendant,
//! because every child of `config` is a separate module in the graph with its own
//! arcs and most of them are inside the knot; descending would assert that the
//! whole `config` subtree names nothing, which is neither true nor this issue's
//! business.
//!
//! Comments and the bodies of string and character literals are removed before
//! anything is matched: neither can be a dependency, neither may hide a path
//! from the scan, and neither may feed one to it. That is why the string
//! `"ac-root-agent"` and the many `.gitignore` fixtures containing it, in this
//! module's own tests, do not trip the forbidden-name check below.
//!
//! Widening the net is the only thing a text scan can do, so this file is
//! written to be widened: the four `ALLOWED_*` tables are the whole contract,
//! and the spellings the scan is known to miss are listed below instead of being
//! left unsaid.
//!
//! KNOWN UNCOVERED SPELLINGS.
//!
//! This list is maintained by the review loop. When a reviewer proves a spelling
//! that puts this module back within reach of the knot and still passes this
//! file, it is appended here. Appending an entry is part of reviewing #1273 and
//! is expected; it changes nothing else.
//!
//! **This file is the canonical copy.** Section 5.4 of
//! `plans/1273-extract-instance-gitignore-from-scc.md` quotes it verbatim, but
//! that quote is a snapshot taken when the plan was certified. The first appended
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
//!      `pub use crate::config::root_agent::SOMETHING;` and this module imports
//!      from there. No `root_agent` token appears in the scanned file. The
//!      detector still catches it: the laundering module carries the arc, this
//!      module reaches the knot through it, and the knot grows instead of
//!      thinning.
//!   2. Macro-generated paths. A `macro_rules!` defined elsewhere, or any
//!      procedural macro, whose expansion contains the path. The text is not in
//!      the scanned files. Whether the detector resolves it has not been
//!      measured here, so do not assume it does.
//!   3. `include!`. A file textually included from outside the module tree is
//!      pulled in without a `mod` declaration, so walking the tree does not
//!      reach it.
//!   4. Runtime indirection. A trait object, function pointer or callback whose
//!      only implementor lives in a knot member and which is wired together
//!      outside this module. No path text appears in the scanned files.
//!   5. `concat!` and friends. `concat!("root", "_agent")` builds the name out
//!      of fragments none of which contains it, and the bodies of those literals
//!      are removed before the scan in any case.
//!   6. A `mod x;` declaration nested inside an inline `mod y { ... }` block.
//!      rustc resolves it against the inline module's own directory and this
//!      resolver does not, so it would scan a file rustc does not compile. It is
//!      refused rather than read: `module_body` rejects the whole file with a
//!      hard failure naming it. The spelling is still uncovered in the sense
//!      that the reference is not read, but it cannot be read as green.
//!   7. NTFS alternate data streams. `#[path = "carrier.rs:evil"]` compiles from
//!      a stream that carries code, and a `mod` declaration hidden inside a
//!      stream of another file is not reachable. Git stores only the main
//!      stream, so a clone has no `:evil` and the build fails rather than hiding
//!      anything.
//!   8. **The fully unanchored path, and it is the important one.** The detector
//!      shares this blind spot and it is measured on production code:
//!      `src/lib.rs:1178` constructs `loops::scheduler::LoopScheduler::new()` and
//!      **no `lib -> loops` arc exists** among the 976. A path that begins with
//!      neither `crate::` nor `super::` is invisible to the record AND to both
//!      equalities here. Inside this module such a path needs a name in scope,
//!      which needs an import this guard does see, with one exception: the single
//!      `use super::*;` in the test module, entry 9. `names_the_replaced_module`
//!      closes the one spelling that matters today, a bare `root_agent`, and
//!      closes it under every anchor and none. It does not close the class.
//!   9. A second `use super::*;`, or the existing one moving to the top of the
//!      file. Written at module level rather than inside `mod tests`, that glob
//!      pulls `crate::config`'s children into scope under no name this scan can
//!      follow, `root_agent` among them, and a bare `root_agent::...` would then
//!      compile with no `super::` token anywhere. Because the observed set is
//!      deduplicated, a second identical glob would not move it. The count is
//!      therefore asserted separately, at exactly one. **Moving** the one glob is
//!      not caught by the count, and it is not exploitable as it stands, because
//!      `mod tests` reaches its parent's items through that glob and would stop
//!      compiling without it. It is written down because that is an argument
//!      about today's code, not a property of the matcher.
//!  10. Aliasing beyond the spellings `aliases_a_module_group` knows.
//!      `use crate as c;`, `use crate::config as c;` and `use super as s;` are
//!      refused by name; a rename reached some other way is not.
//!  11. A path assembled across a `cfg` boundary in a way the resolver
//!      over-reads into but the equality tables do not distinguish. This
//!      resolver scans both arms of a platform module, so a forbidden reference
//!      in either arm is caught, but which arm rustc compiled is not known here
//!      and the failure message cannot say.
//!  12. A `#[cfg(test)]` reference holding an equality up on its own, and here
//!      it is a live weakness rather than a theoretical one. Whole files are
//!      read, test regions included, while the detector is run with
//!      `includeTests: false`. Everywhere else that makes this guard stricter,
//!      which is the safe direction; here it makes it laxer. Six of the eight
//!      references to `ROOT_AGENT_DIR_NAME` in this module are inside
//!      `#[cfg(test)] mod tests`, so deleting the two production references at
//!      `required_rules` would leave the pair
//!      `("src/config/instance_gitignore.rs", "ROOT_AGENT_DIR_NAME")` standing
//!      and the equality green. Unlike the equivalent entry in
//!      `project_settings_layering.rs`, **that deletion can be made to compile**:
//!      hard-coding the directory name in `required_rules` does it. The
//!      shrinking-set argument is correspondingly weaker for this one pair, and
//!      the fourteen-rule behavioural tests in the module are what actually
//!      hold the production references up.
//!  13. An unanchored path in the constant's home. `src/config/mod.rs` already
//!      writes `profile::config_dir_name()` three times, a path beginning with
//!      neither `crate::` nor `super::`, which is why it creates no arc and why
//!      that module measures zero outgoing arcs. A new unanchored path from
//!      there into a knot member would be invisible to the detector and to this
//!      guard alike, while putting `config` and this module back into the knot
//!      the moment anybody anchored it. The two equalities on that file catch
//!      the anchored forms only.
//!  14. (append here: one entry per spelling a reviewer proves still passes)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Every `(file, child)` reference the guarded module is allowed to make under
/// `crate::`, sorted.
///
/// **Empty, and the emptiness is the contract.** Before #1273 this module
/// contained no `crate::` path at all, and it still does not: everything it
/// needs is one level up and is written `super::`. An empty equality therefore
/// refuses **every** `crate::`-anchored reference, not merely a reference to
/// `config::root_agent`, and that breadth is the point. The module's exposure is
/// that it may name any of the 87 remaining knot members and fall straight back
/// in; `root_agent` is only today's spelling.
///
/// Adding the first row here is a decision about the crate's shape and must be
/// argued in the commit, not slipped in to get green.
const ALLOWED_GUARDED_CRATE_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the guarded module is allowed to make under
/// `super::`, sorted.
///
/// This is where this module actually lives, so this is the table that has to be
/// right. Six pairs, one file:
///
/// - `*` is the single `use super::*;` inside `#[cfg(test)] mod tests`, where
///   `super` is this module itself. See entry 9 of the uncovered list for why
///   its count is asserted separately.
/// - `ROOT_AGENT_DIR_NAME` is the constant #1273 moved into `crate::config`,
///   reached as `super::ROOT_AGENT_DIR_NAME` from `required_rules` and as
///   `super::super::ROOT_AGENT_DIR_NAME` from the test module. **It is listed
///   because it must be there**: this is an equality, so if it silently
///   disappears the assertion fails rather than passing quieter. Its absence
///   would mean the name was reached some other way.
/// - `agent_local_dir_name` and `config_dir` are `crate::config`'s own
///   functions, called from `ensure_instance_gitignore`. They predate #1273 and
///   are the reason the arc `config::instance_gitignore -> config` exists and is
///   not removable.
/// - `injected_messages` is `super::super::injected_messages::...` in two tests.
///   It is a sibling module **inside the knot**, and it is allowed here only
///   because it is `#[cfg(test)]` and therefore contributes no arc to the record
///   (`includeTests: false`). If it ever appears in production code in this
///   module, the record gains `config::instance_gitignore ->
///   config::injected_messages` and this module is back in the knot. This guard
///   cannot tell the two positions apart; the detector can, and is the check
///   that decides.
/// - `super` is the leading segment of every `super::super::...` path, reported by
///   the matcher as itself rather than dropped.
///
/// The pair is the contract, not the child on its own. Keying on the child alone
/// would make the observed set a union over every scanned file, so a reference
/// added to a future submodule of this module would leave the set unmoved and
/// pass.
const ALLOWED_GUARDED_SUPER_REFERENCES: [(&str, &str); 6] = [
    ("src/config/instance_gitignore.rs", "*"),
    ("src/config/instance_gitignore.rs", "ROOT_AGENT_DIR_NAME"),
    ("src/config/instance_gitignore.rs", "agent_local_dir_name"),
    ("src/config/instance_gitignore.rs", "config_dir"),
    ("src/config/instance_gitignore.rs", "injected_messages"),
    ("src/config/instance_gitignore.rs", "super"),
];

/// Every `(file, child)` reference the constant's home is allowed to make under
/// `crate::`, sorted.
///
/// **Empty.** `src/config/mod.rs` measures zero outgoing arcs over the 976, and
/// that measurement is the whole non-absorption argument of #1273: a module with
/// no way out cannot reach a knot member, so it cannot share an SCC with one, so
/// nothing that depends only on it can either.
const ALLOWED_HOST_CRATE_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the constant's home is allowed to make under
/// `super::`, sorted.
///
/// One row, the `use super::*;` in that file's own `#[cfg(test)] mod tests`,
/// where `super` is `crate::config` itself. From `src/config/mod.rs`, `super::`
/// at module level would mean the crate root, so a row appearing here for any
/// other child is a reference from `config` up into the crate root's children,
/// which is the inversion this guard exists to refuse.
const ALLOWED_HOST_SUPER_REFERENCES: [(&str, &str); 1] = [("src/config/mod.rs", "*")];

/// The module #1273 cut this one away from, matched as a bare identifier under
/// every anchor and under none.
///
/// This is the one check in the file that does not depend on an anchor, and it
/// exists because of entry 8: a bare `root_agent::ROOT_AGENT_DIR_NAME` would
/// compile if the name were in scope, would rebuild the dependency, and would be
/// invisible to the arc record and to both equalities. Comments and literal
/// bodies are removed first, so the string `"ac-root-agent"` and the
/// `.gitignore` fixtures in this module's tests do not match it.
const FORBIDDEN_NAME: &str = "root_agent";

const CRATE_ANCHOR: &str = "crate::";
const SUPER_ANCHOR: &str = "super::";

/// The module this guard is written about, as path segments below `crate`.
const GUARDED_MODULE: [&str; 2] = ["config", "instance_gitignore"];

/// The module #1273 moved the constant into, as path segments below `crate`.
const HOST_MODULE: [&str; 1] = ["config"];

/// The constant #1273 moved, and the one file that may define it.
///
/// `defines_the_constant` decides what counts as a definition, because the
/// re-export left behind in `src/config/root_agent.rs` names the same identifier
/// and must not be counted as one, while a `static` copy must be.
const CONSTANT_NAME: &str = "ROOT_AGENT_DIR_NAME";
const CONSTANT_HOME: &str = "src/config/mod.rs";

/// The one glob import the guarded module is allowed to contain, counted rather
/// than merely observed. See entry 9 of the uncovered list.
const GLOB_IMPORT: &str = "use super::*;";

/// Whether literal bodies survive `scrub`.
///
/// They must survive when the text is about to be read for `path = "..."`,
/// and must not when it is about to be read for dependencies or for structure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Literals {
    Keep,
    Drop,
}

/// Whether a module's submodules are read with it.
///
/// `WithSubmodules` for the guarded module, so a reference cannot be parked in a
/// future child of it. `OwnFilesOnly` for the constant's home, because every
/// child of `config` is a separate module in the graph with its own arcs and
/// most of them are knot members.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    OwnFilesOnly,
    WithSubmodules,
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
/// after the real references are gone, and it is what keeps the many
/// `"ac-root-agent"` fixtures in the guarded module's tests from matching the
/// forbidden name.
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
/// out, as in `use crate as c;`, `use crate::config as c;` or `use super as s;`.
///
/// After such a rename `c::root_agent::...` reaches the cut module under a name no
/// text scan can follow, so the rename itself is refused instead of followed.
/// Anchored on the path punctuation in front of `config` so that English prose
/// about configuration does not trip it, and on `use crate`/`use super` rather
/// than the bare keywords for the same reason.
fn aliases_a_module_group(body: &str) -> bool {
    [
        "use crate as ",
        "::config as ",
        "{config as ",
        ",config as ",
        "config::{self as ",
        "use super as ",
        "use super::{self as ",
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
/// as `root_agent::{a, b}, config_dir` yields two items and not three.
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
/// `super::super::X` reports two children, `super` and `X`, because the scan
/// finds the anchor twice: once at the start and once immediately after it. That
/// is deliberate. `super` in the observed set is the marker that a path climbed
/// two levels, and dropping it would make `super::super::session::manager` and
/// `super::session::manager` indistinguishable.
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

/// Whether `body`, which must be scrubbed and normalized, names the module #1273
/// cut away from, as a bare identifier and under no anchor at all.
///
/// Both boundaries are checked, so `root_agent_defaults` and
/// `x_root_agent` are not hits and `r#root_agent` is.
fn names_the_replaced_module(body: &str) -> bool {
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
        if opens && closes {
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
fn defines_the_constant(body: &str) -> bool {
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
/// this resolver reads it as a child of the file. A scanner that cannot tell
/// which file it should be reading has to say so.
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

/// The files rustc compiles for `module`, and for every module below it when
/// `reach` says so, resolved by walking `mod` declarations down from the crate
/// root.
///
/// The walk carries a frontier rather than a single file, because a segment can
/// be declared more than once under opposite `cfg`s and this resolver keeps both
/// arms. An error at any step is propagated rather than skipped: a module that
/// cannot be located is the one case where reading nothing must not look like
/// reading nothing forbidden.
fn files_of(module: &[&str], reach: Reach) -> Result<Vec<PathBuf>, String> {
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

    if reach == Reach::OwnFilesOnly {
        return Ok(frontier);
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
/// of it would refuse to answer the question it was called to answer.
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
/// `anchored` and `relative_up` are `(file, child)` pairs under `crate::` and
/// under `super::` respectively; `aliases` is the files that rename a module
/// group; `forbidden` is the files that name `root_agent` under no anchor at
/// all; `globs` is the total number of `use super::*;` across the module.
struct Observation {
    anchored: Vec<(String, String)>,
    relative_up: Vec<(String, String)>,
    aliases: Vec<String>,
    forbidden: Vec<String>,
    globs: usize,
}

/// Read every file of `module` and report what it names.
///
/// A file reached through the module tree is a file rustc compiles, so a `scrub`
/// failure on one of them is fatal here and says so: it is source the compiler
/// reads and this scan could not.
fn observe(module: &[&str], reach: Reach) -> Observation {
    let files = files_of(module, reach).unwrap_or_else(|reason| {
        panic!(
            "the module {module:?} could not be resolved from the module tree, so this scan \
             proves nothing: {reason}\n\
             \n\
             WHY THIS IS A FAILURE AND NOT A SKIP: this guard exists to prove that a specific \
             dependency is absent. If the module cannot be located, the guard has read nothing \
             and must say so rather than pass. Rename or move the module and this message names \
             the file whose `mod` declaration no longer resolves; update GUARDED_MODULE or \
             HOST_MODULE, or the declaration, to match."
        )
    });
    assert!(
        !files.is_empty(),
        "the module {module:?} resolved to no files at all; the scan proves nothing"
    );

    let mut anchored = Vec::new();
    let mut relative_up = Vec::new();
    let mut aliases = Vec::new();
    let mut forbidden = Vec::new();
    let mut globs = 0usize;
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
        anchored.extend(name(children_under(&body, CRATE_ANCHOR)));
        relative_up.extend(name(children_under(&body, SUPER_ANCHOR)));
        if aliases_a_module_group(&body) {
            aliases.push(relative.clone());
        }
        if names_the_replaced_module(&body) {
            forbidden.push(relative.clone());
        }
        globs += body.matches(GLOB_IMPORT).count();
    }
    anchored.sort();
    anchored.dedup();
    relative_up.sort();
    relative_up.dedup();
    Observation {
        anchored,
        relative_up,
        aliases,
        forbidden,
        globs,
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

/// #1273: `config::instance_gitignore` used to name
/// `super::root_agent::ROOT_AGENT_DIR_NAME`, which made a 413-line filesystem
/// utility depend on a 3711-line module inside the crate's 88-member cyclic SCC
/// and held it inside that SCC. The constant moved down into `crate::config`, so
/// both sides depend downward on a module that depends on nothing.
///
/// This test lives in `src-tauri/tests/`, which is a separate leaf crate the
/// detector marks `enabled: opts.includeTests` and the record is emitted with
/// `includeTests: false`. It therefore adds no arc and no module, is outside the
/// tree it reads, and never has to excise itself from its own scan. Whole files
/// are read, `#[cfg(test)]` regions included, which is stricter than the
/// detector: a false red is argued about, a false green is believed.
#[test]
fn instance_gitignore_names_nothing_that_reaches_the_knot() {
    let seen = observe(&GUARDED_MODULE, Reach::WithSubmodules);

    assert!(
        seen.forbidden.is_empty(),
        "config::instance_gitignore must not name config::root_agent.\n\
         \n\
         WHY: this module writes the running instance's `.gitignore`. It is a \
         413 line filesystem utility that needs exactly one fact from \
         `config::root_agent`, the directory name `ac-root-agent`, and \
         `config::root_agent` is a 3711 line module inside the crate's 88 member \
         cyclic SCC. Naming it from here put this module inside that SCC too. \
         Issue #1273 moved the constant down into `crate::config`, which both \
         modules already depend on and which depends on nothing, so this module \
         now sits at level 1 below the knot instead of inside it.\n\
         \n\
         INSTEAD: read the name as `super::ROOT_AGENT_DIR_NAME`, which is where \
         it lives. If you need something from `config::root_agent` that is not a \
         name, it belongs in a module below both of them, never in either one.\n\
         \n\
         THIS CHECK IS DELIBERATELY ANCHORLESS. It matches the bare identifier \
         `root_agent` anywhere in the module's code, because the arc record \
         cannot see a path that begins with neither `crate::` nor `super::` \
         (measured: `src/lib.rs:1178` constructs `loops::scheduler::LoopScheduler` \
         and no `lib -> loops` arc exists among the 976). Comments and the bodies \
         of literals are removed first, so the string `\"ac-root-agent\"` and the \
         `.gitignore` fixtures in this module's own tests do not match.\n\
         \n\
         SCOPE: this is a net over the spellings of that reference, not a proof \
         that it cannot return. It matches text and does not resolve names, so a \
         spelling it does not know about passes it; the ones it is known to miss \
         are listed at the top of this file. The authoritative check is the cycle \
         detector, whose `coverage.graphShape.cyclicSccs` must stay at 1 with \
         `sccSize(agentscommander_lib::config::instance_gitignore) = 1`.\n\
         \n\
         OFFENDING FILES: {}",
        seen.forbidden.join(", ")
    );

    assert!(
        seen.aliases.is_empty(),
        "config::instance_gitignore must not rename the crate root or the config \
         module group.\n\
         \n\
         WHY: `use crate as <name>;`, `use crate::config as <name>;` and \
         `use super as <name>;` each put every module under `config`, \
         `config::root_agent` included, within reach under a name this scan \
         cannot follow. Following it would mean resolving names, which a text \
         scan does not do, so the rename is refused instead.\n\
         \n\
         INSTEAD: name the item you need by its real path, so this guard and the \
         cycle detector can both see it.\n\
         \n\
         OFFENDING FILES: {}",
        seen.aliases.join(", ")
    );

    assert_eq!(
        seen.globs, 1,
        "config::instance_gitignore must contain exactly one `use super::*;`.\n\
         \n\
         WHY: the one that exists is inside `#[cfg(test)] mod tests`, where \
         `super` is this module itself and the glob pulls in the functions under \
         test. Written at the top level of the file instead, the same three words \
         pull `crate::config`'s children into scope, `root_agent` among them, and \
         a bare `root_agent::ROOT_AGENT_DIR_NAME` would then compile with no \
         `super::` token anywhere: invisible to the arc record, invisible to both \
         equalities below, and the whole of #1273 undone. A text scan cannot tell \
         the two positions apart, and the observed set is deduplicated, so a \
         second identical glob would not move it. The count is asserted instead.\n\
         \n\
         INSTEAD: import what you need by name. If the test module genuinely \
         needs more of its parent, add the names to its existing glob's file, not \
         a second glob.\n\
         \n\
         OBSERVED: {} occurrences of `{GLOB_IMPORT}`",
        seen.globs
    );

    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_GUARDED_CRATE_REFERENCES),
        "the set of crate modules named from config::instance_gitignore moved.\n\
         \n\
         WHY THIS TABLE IS EMPTY: this module contains no `crate::` path at all, \
         before #1273 or after it. Everything it needs is one level up and is \
         written `super::`. An empty equality therefore refuses EVERY \
         `crate::`-anchored reference rather than only a reference to \
         `config::root_agent`, and that breadth is the point: this module's \
         exposure is that it may name any of the 87 remaining members of the knot \
         and fall straight back into it. `root_agent` is only today's spelling.\n\
         \n\
         THIS IS THE ASSERTION #1268 IS ABOUT. `project_settings_layering.rs` \
         collects the same set for its guarded module and never asserts it, and \
         the consequence was measured on this tree: adding \
         `use crate::session::manager::SessionManager;` to \
         `src/commands/project_settings.rs` moved the knot 88 to 89, \
         `sccSize` 1 to 89 and the arc count 976 to 977, with that file green \
         throughout, 3 passed 0 failed. This assertion is why the same three \
         words are red here.\n\
         \n\
         INSTEAD: if this module needs something from elsewhere in the crate, \
         that something belongs below it. Adding the first row to \
         ALLOWED_GUARDED_CRATE_REFERENCES is a decision about the crate's shape \
         and has to be argued in the commit."
    );

    assert_eq!(
        seen.relative_up,
        expected(&ALLOWED_GUARDED_SUPER_REFERENCES),
        "the set of names reached by `super::` from config::instance_gitignore \
         moved.\n\
         \n\
         WHY THIS ANCHOR IS THE LOAD BEARING ONE: every reference this module \
         makes is written `super::` or `super::super::`, so a `crate::`-only \
         guard would observe nothing here and pass everything. This table is the \
         real contract.\n\
         \n\
         Each entry is a (file, child) pair, because the file is half of the \
         rule. `super::super::X` reports two children, `super` and `X`, so a path \
         that climbs two levels is distinguishable from one that climbs one.\n\
         \n\
         A LARGER SET means this module reached further up. That is a decision, \
         not a detail: remove it, or add its pair and say in the commit why. \
         `injected_messages` is a knot member and is allowed only because its two \
         references are `#[cfg(test)]` and contribute no arc; a production \
         reference to it would put this module back in the knot and this guard \
         cannot tell the two apart, so the detector decides.\n\
         \n\
         A SMALLER SET is the more dangerous failure, and it is why this is an \
         equality and not a denylist. `ROOT_AGENT_DIR_NAME` is listed because \
         #1273 put it there: if it silently disappears, the name is being reached \
         some other way and the reason this module is out of the cycle has \
         changed without anybody saying so. A shrinking set also means the scan \
         may have stopped seeing references it used to see, and a guard that \
         observes nothing passes everything. Comments and literal bodies are \
         removed before the scan so no amount of prose can hold this set up while \
         the real references disappear."
    );
}

/// #1273 Section 4.3: the knot cannot absorb `config::instance_gitignore`
/// because, after the cut, everything it depends on is `crate::config`, and
/// `crate::config` has **zero outgoing arcs** over the 976 of
/// `src-tauri/module-arcs.txt`.
///
/// **That is a claim about outgoing arcs from `src/config/mod.rs`, and this test
/// is the only thing that holds it.** The arc
/// `config::instance_gitignore -> config` is not removed by #1273 and is not
/// removable: `ensure_instance_gitignore` calls `super::config_dir()` and
/// `super::agent_local_dir_name()`. So one `use crate::<knot member>::...;` in
/// `src/config/mod.rs` puts `config` into the knot and this module back in with
/// it, 49 other modules follow, and every assertion in the test above stays green
/// because the guarded module's own file did not change.
///
/// It reads `src/config/mod.rs` and nothing below it. Every child of `config` is
/// a separate module in the graph with its own arcs, and 21 of them are knot
/// members; descending would assert that the whole `config` subtree names
/// nothing, which is neither true nor #1273's business.
#[test]
fn the_constant_home_names_nothing_at_all() {
    let seen = observe(&HOST_MODULE, Reach::OwnFilesOnly);

    assert!(
        seen.aliases.is_empty(),
        "the constant's home must not rename the crate root or a module group; \
         see the same assertion for config::instance_gitignore.\n\
         \n\
         OFFENDING FILES: {}",
        seen.aliases.join(", ")
    );

    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_HOST_CRATE_REFERENCES),
        "the set of crate modules named from {CONSTANT_HOME} moved.\n\
         \n\
         WHY THIS MATTERS MORE THAN IT LOOKS: #1273 is only correct while this \
         module cannot reach the cyclic SCC. Measured over the 976 arcs of \
         `src-tauri/module-arcs.txt`, `agentscommander_lib::config` appears on \
         the left of the separator zero times and on the right 49 times: it is a \
         pure sink, and that is the entire non-absorption argument. A module with \
         no way out cannot reach a knot member, so it cannot share an SCC with \
         one, so `config::instance_gitignore`, which depends on it, cannot \
         either.\n\
         \n\
         One `use crate::<any knot member>::...;` in this file ends that. `config` \
         joins the knot, `config::instance_gitignore` follows it back in through \
         an arc #1273 never removed and cannot remove, and the 49 other modules \
         that depend on `config` follow too. No other test in this repository \
         would go red.\n\
         \n\
         INSTEAD: if this module needs something, that something belongs below \
         it. Adding the first row to ALLOWED_HOST_CRATE_REFERENCES is a decision \
         about the crate's shape and has to be argued in the commit, and Section \
         4.3 of `plans/1273-extract-instance-gitignore-from-scc.md` has to be \
         rewritten before the dependency is added."
    );

    assert_eq!(
        seen.relative_up,
        expected(&ALLOWED_HOST_SUPER_REFERENCES),
        "the set of names reached by `super::` from {CONSTANT_HOME} moved.\n\
         \n\
         The one allowed row is the `use super::*;` in that file's own \
         `#[cfg(test)] mod tests`, where `super` is `crate::config` itself. At \
         the top level of this file `super::` means the CRATE ROOT, so any other \
         row here is `config` reaching up into `crate`'s children, which is the \
         inversion this guard refuses. It would not even show up under the \
         `crate::` anchor above.\n\
         \n\
         See that assertion for why an outgoing arc from this file is the one \
         thing that undoes #1273."
    );
}

/// #1273 criterion 8: the constant moved, it was not copied. Two definitions
/// would drift, and `\"the arc is gone\"` would be satisfied while the fact the
/// arc was about had been duplicated instead of relocated.
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
fn the_root_agent_dir_name_constant_is_defined_exactly_once() {
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
                if defines_the_constant(&normalized(&code)) {
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
             and cannot claim the constant is defined exactly once.\n\
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
             FILES THE COMPILER READS THAT COULD NOT BE DELIMITED: {fatal:?}"
        );
    }

    assert_eq!(
        homes,
        vec![CONSTANT_HOME.to_string()],
        "{CONSTANT_NAME} must be defined exactly once, in {CONSTANT_HOME}.\n\
         \n\
         WHY: #1273 moved this constant out of `config::root_agent` so that \
         `config::instance_gitignore` would stop depending on a knot member for \
         one string. A move that left a copy behind satisfies every arc \
         assertion and is still wrong: the two copies drift, and the claim that \
         the name has one home stops being true. Arc absence alone is \
         satisfiable without fixing anything, which is why this test exists \
         beside the two above.\n\
         \n\
         WHAT COUNTS AS A DEFINITION: `const` or `static`, then the name, then \
         `:`. The re-export `pub use crate::config::ROOT_AGENT_DIR_NAME;` that \
         #1273 leaves in `src/config/root_agent.rs` names the identifier and \
         defines nothing, so it is deliberately not counted: 49 references \
         outside the guarded module still spell \
         `crate::config::root_agent::ROOT_AGENT_DIR_NAME` and resolve through \
         it.\n\
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
