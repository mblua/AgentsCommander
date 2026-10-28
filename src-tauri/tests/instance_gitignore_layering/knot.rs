use super::*;

/// #1273: `config::instance_gitignore` used to name
/// `super::root_agent::ROOT_AGENT_DIR_NAME`, which made a 413-line filesystem
/// utility depend on a 3711-line module inside the crate's cyclic SCC
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
         `config::root_agent` is a 3711 line module inside the crate's \
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
         and no `lib -> loops` arc exists in the record). Comments and the bodies \
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
        "src/config/instance_gitignore.rs must contain exactly one glob \
         import.\n\
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
         THE COUNT IS STRUCTURAL, NOT TEXTUAL. It counts `super::`-anchored \
         items whose leading segment is `*`, taken before the observed set is \
         deduplicated, so `use super::super::*;` and `use super::{{*}};` are \
         counted too. Both were measured passing the old literal count while \
         adding a real arc: entries 14 and 15.\n\
         \n\
         OBSERVED: {} `super::`-anchored glob imports",
        seen.globs
    );

    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_GUARDED_CRATE_REFERENCES),
        "the set of crate modules named from config::instance_gitignore moved.\n\
         \n\
         WHY THIS TABLE HAS ONE ROW: #3327 delegates physical ignore IO through \
         crate::filesystem::instance_ignore. This leaf uses std/OS bindings and \
         has no app path back. Equality permits only the file/child pair \
         src/config/instance_gitignore.rs -> filesystem and rejects all others.\n\
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
         that something belongs below it. Changing the rows in \
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

    assert_eq!(
        seen.own_module,
        expected(&ALLOWED_GUARDED_SELF_REFERENCES),
        "the set of names reached by `self::` from config::instance_gitignore \
         moved.\n\
         \n\
         WHY THIS TABLE IS EMPTY: `self::x` and `x` name the same item, so this \
         anchor is a second spelling of every path that begins at this module, \
         and this module has no such path today. An empty equality refuses the \
         whole class rather than one name.\n\
         \n\
         WHY IT EXISTS AT ALL: until the #1273 review, `self::` was scanned under \
         no anchor anywhere in this file. It was measured taking the knot 87 to \
         91 from the constant's home; here it is closed at the same time, because \
         a spelling that is free in one guarded module is free in the other. \
         Entry 17.\n\
         \n\
         INSTEAD: name the item without the anchor, so the `super::` equality and \
         the anchorless check both see it."
    );
}

/// #1273 Section 4.3: the knot cannot absorb `config::instance_gitignore`
/// because, after the cut, everything it depends on is `crate::config`, and
/// `crate::config` has **zero outgoing arcs** over the record in
/// `src-tauri/module-arcs.txt`.
///
/// **That is a claim about outgoing arcs from `src/config.rs`, and this test
/// is the only thing that holds it.** The arc
/// `config::instance_gitignore -> config` is not removed by #1273 and is not
/// removable: `ensure_instance_gitignore` calls `super::config_dir()` and
/// `super::agent_local_dir_name()`. So one `use crate::<knot member>::...;` in
/// `src/config.rs` puts `config` into the knot and this module back in with
/// it, 49 other modules follow, and every assertion in the test above stays green
/// because the guarded module's own file did not change.
///
/// It reads `src/config.rs` and nothing below it. Every child of `config` is
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

    assert!(
        seen.forbidden.is_empty(),
        "the constant's home must not name config::root_agent.\n\
         \n\
         WHY: this file is where #1273 put `ROOT_AGENT_DIR_NAME`, precisely so \
         that nothing below the knot has to reach into it. A path from here into \
         `config::root_agent` puts `config` in the knot and drags \
         `config::instance_gitignore` back in with it through arc 547, which \
         #1273 never removed and cannot remove.\n\
         \n\
         WHY IT IS ASSERTED HERE NOW: this observation was computed and thrown \
         away until the #1273 review. The anchorless check therefore never ran \
         against this file at all, and `use self::root_agent::...;` here was \
         measured green while taking the knot 87 to 91. Entry 17.\n\
         \n\
         THIS CHECK IS DELIBERATELY ANCHORLESS, for the same reason it is in the \
         guarded module's test: the arc record cannot see a path that begins with \
         neither `crate::` nor `super::`. Comments and literal bodies are removed \
         first.\n\
         \n\
         `mod root_agent;` IS EXEMPT AND ONLY THAT. This file is \
         `config::root_agent`'s parent and must declare it, so without the \
         exemption this assertion would be red on a correct tree. A `mod` \
         declaration creates no arc (plan Section 2.7). Any USE of the name is a \
         separate occurrence and still fails here.\n\
         \n\
         OFFENDING FILES: {}",
        seen.forbidden.join(", ")
    );

    assert_eq!(
        seen.globs, 0,
        "{CONSTANT_HOME} must contain no glob import.\n\
         \n\
         WHY: the test module's `use super::*;` now lives in `src/config/tests.rs`, \
         a separate module this observation does not read. \
         Written at the TOP LEVEL of this file the same three words mean \
         `use crate::*;`, which puts every child of the crate root in scope and \
         gives `config` an outgoing arc to `agentscommander_lib`. That ends the \
         zero-outgoing-arc premise the whole of #1273 Section 4.3 rests on.\n\
         \n\
         THIS IS THE MOST DAMAGING SPELLING THE REVIEW FOUND. Measured: one line \
         added to this file took the knot 87 to 91 and \
         `sccSize(config::instance_gitignore)` 1 to 91, with `cargo fmt --check` \
         at exit 0 and this guard at 3 passed, because the allowed table below \
         contains the row for the test module's glob and a text scan cannot tell \
         the two positions apart. Entry 16. The guarded module's test has \
         asserted this count since day one; this test did not, and that \
         asymmetry was the hole.\n\
         \n\
         INSTEAD: import what you need by name, and keep the test module's glob \
         in `src/config/tests.rs`.\n\
         \n\
         OBSERVED: {} `super::`-anchored glob imports",
        seen.globs
    );

    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_HOST_CRATE_REFERENCES),
        "the set of crate modules named from {CONSTANT_HOME} moved.\n\
         \n\
         WHY THIS MATTERS MORE THAN IT LOOKS: #1273 is only correct while this \
         module cannot reach the cyclic SCC. Measured over the arcs of \
         `src-tauri/module-arcs.txt`, `agentscommander_lib::config` appears on \
         the left of the separator zero times and on the right many times: it is \
         a pure sink, and that is the entire non-absorption argument. A module \
         with no way out cannot reach a knot member, so it cannot share an SCC with \
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
         4.3 of `.ac/plans/1273-extract-instance-gitignore-from-scc.md` has to be \
         rewritten before the dependency is added."
    );

    assert_eq!(
        seen.relative_up,
        expected(&ALLOWED_HOST_SUPER_REFERENCES),
        "the set of names reached by `super::` from {CONSTANT_HOME} moved.\n\
         \n\
         The table is empty: the test module's `use super::*;` moved to \
         `src/config/tests.rs`. At \
         the top level of this file `super::` means the CRATE ROOT, so any \
         row here is `config` reaching up into `crate`'s children, which is the \
         inversion this guard refuses. It would not even show up under the \
         `crate::` anchor above.\n\
         \n\
         See that assertion for why an outgoing arc from this file is the one \
         thing that undoes #1273."
    );

    assert_eq!(
        seen.own_module,
        expected(&ALLOWED_HOST_SELF_REFERENCES),
        "the set of names reached by `self::` from {CONSTANT_HOME} moved.\n\
         \n\
         THIS IS THE ANCHOR THAT WAS NOT BEING READ. From this file `self::` \
         means `crate::config`, so `use self::root_agent::...;` reaches a member \
         of the knot from the one module whose zero-outgoing-arc property the \
         whole of #1273 Section 4.3 rests on. Measured during the review: that \
         one line took the knot 87 to 91 and \
         `sccSize(config::instance_gitignore)` 1 to 91, with this guard at 3 \
         passed and `cargo fmt --check` at exit 0, because `children_under` was \
         only ever called with `crate::` and `super::`. Entry 17.\n\
         \n\
         UNLIKE ENTRY 13, THE DETECTOR IS NOT BLIND TO THIS. `self::` resolves \
         and the arc is recorded, so this spelling is a real regression and not \
         merely a guard-blind one.\n\
         \n\
         THE TABLE IS EMPTY ON PURPOSE. `self::x` and `x` name the same item, so \
         every row here is a second spelling of a path that should be argued \
         under its own anchor. The class this closes is every knot member, not \
         `root_agent` alone, which is why widening the anchorless check would not \
         have been enough."
    );
}
