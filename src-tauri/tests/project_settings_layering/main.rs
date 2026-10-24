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
//! `.ac/plans/1265-extract-project-settings-from-scc.md` quotes it verbatim, but that
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
//!  13. ANY `use crate::<knot member>::...` from the guarded module, and not
//!      only a reference to `web::commands`. The twelve entries above are all
//!      written about spellings of that one reference, but the cycle does not
//!      need it: `web::commands -> commands::project_settings` is alive by
//!      design, so naming any of the 88 knot members from here closes a cycle
//!      just as well. Measured on the real tree by adding three words,
//!      `use crate::session::manager::SessionManager;`, to
//!      `src/commands/project_settings.rs`: knot 88 to 89,
//!      `sccSize(commands::project_settings)` 1 to 89, level of `web::commands`
//!      3 back down to 2, arcs 976 to 977, and **this file green throughout:
//!      3 passed, 0 failed**. #1265 is undone and no test in the repository
//!      goes red.
//!
//!      CAUSE, and it is an asymmetry between the two guarded modules rather
//!      than a property of the matcher. The guarded module's test asserts only
//!      the equality of children under the `web::` anchor, plus the alias
//!      check. `seen.anchored`, the set of children named under `crate::`, is
//!      collected by `observe()` and then never asserted for `GUARDED_MODULE`.
//!      The emitter module does have that equality,
//!      `ALLOWED_EMITTER_CRATE_REFERENCES`, which is exactly why the
//!      equivalent spelling is red there and green here.
//!
//!      The authoritative check still catches it, as always: the detector puts
//!      the module straight back into the knot, and the numbers above are what
//!      it reports. Closing it in this file is cheap and symmetric with what
//!      already exists, assert `seen.anchored` for `GUARDED_MODULE` against an
//!      `ALLOWED_GUARDED_CRATE_REFERENCES` table the way the emitter module
//!      already does. That is tracked in **#1268** and is deliberately not
//!      done here: it would change the anchor contract of Section 4.3 of
//!      `.ac/plans/1265-extract-project-settings-from-scc.md`, which is
//!      specification, and appending here is what Section 9.3.5 contemplates.
//!  14. (append here: one entry per spelling a reviewer proves still passes)

mod module_sources;
mod source_text;

use module_sources::{
    crate_sources, every_file_under, manifest_dir, relative_of, scrubbed, sources_of,
};
use source_text::{aliases_a_module_group, children_under, normalized, Literals};

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
/// The production row pins `WsBroadcaster` in `broadcast_all`'s signature.
/// The child test file imports the same fan-out after its extraction in #3188.
const ALLOWED_EMITTER_CRATE_REFERENCES: [(&str, &str); 2] = [
    ("src/web/event_broadcast.rs", "web"),
    ("src/web/event_broadcast/tests.rs", "web"),
];

/// Every `(file, child)` reference the emitter module is allowed to make into a
/// child of `crate::web`, sorted.
/// Two equalities rather than one path, and this is deliberate: the `crate::`
/// table above pins the first segment and this one pins the second, and together
/// they admit `crate::web::broadcast` and nothing else. Expressing the contract
/// as one joined `web::broadcast` string would have needed a second matcher that
/// recurses through brace groups, where `children_under` already handles
/// `use crate::web::{broadcast::A, commands::B}` and `use crate::{web::A, x::B}`
/// correctly under each anchor. Reusing the audited matcher twice is worth more
/// than a prettier table.
const ALLOWED_EMITTER_WEB_REFERENCES: [(&str, &str); 2] = [
    ("src/web/event_broadcast.rs", "broadcast"),
    ("src/web/event_broadcast/tests.rs", "broadcast"),
];

/// Every `(file, child)` reference the emitter module is allowed to make under
/// `super::`, sorted.
/// **This anchor exists only for the emitter module, and the asymmetry is the
/// point.** The dispatcher `web::commands` is the emitter's SIBLING: from inside
/// `src/web/`, `super::commands` reaches it without the text `web::` appearing
/// anywhere, so neither of the two tables above sees it. That is not an exotic
/// spelling, it is the idiom the neighbouring file already uses:
/// `src/web/commands.rs:12` writes `use super::broadcast::WsBroadcaster;`.
/// `commands::project_settings` needs no such anchor because it is not a sibling
/// of anything under `web`: every path from there into `web` must spell `web`
/// followed by `::`, or rename a group, which is refused separately.
/// The one allowed row is the extracted test file reaching its own parent for the
/// function under test. **A glob is deliberately not allowed.** `use super::*;`
/// written at module level would pull `crate::web`'s children, `commands`
/// included, into scope under no name this scan could follow, and it is
/// indistinguishable by text from the same glob inside `mod tests`. That is why
/// Section 5.1 imports by name instead of globbing its parent.
const ALLOWED_EMITTER_SUPER_REFERENCES: [(&str, &str); 1] =
    [("src/web/event_broadcast/tests.rs", "broadcast_all")];

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
         same glob inside `mod tests`. The extracted test file imports \
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
