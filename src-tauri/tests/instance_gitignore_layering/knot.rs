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
         WHY THIS TABLE HAS ONE ROW: #3327 P04 permits the pair \
         src/config/instance_gitignore.rs -> filesystem for physical ignore IO \
         through crate::filesystem::instance_ignore. P05 enables the second pair, \
         src/config/naming_migration.rs -> filesystem, through \
         crate::filesystem::naming_journal. Both leaves use only std/OS bindings \
         and have no app path back. This equality permits only the first pair; \
         the second belongs to the separate naming_migration guard in P05.\n\
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

/// #3534 permits one std-only probe leaf instead of a host with zero outgoing arcs.
/// Exact host spelling and a leaf with no app references prevent an app return path.
/// Only the host file is observed; config children are separate graph modules.
#[test]
fn the_constant_home_names_nothing_at_all() {
    let seen = observe(&HOST_MODULE, Reach::OwnFilesOnly);

    assert!(
        seen.aliases.is_empty(),
        "the host must not rename crate/module groups: {}",
        seen.aliases.join(", ")
    );
    assert!(
        seen.forbidden.is_empty(),
        "the host must not use root_agent; only its mod declaration is exempt: {}",
        seen.forbidden.join(", ")
    );
    assert_eq!(
        seen.globs, 0,
        "{CONSTANT_HOME} must not glob-import crate children; observed {}",
        seen.globs
    );
    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_HOST_CRATE_REFERENCES),
        "the host may name only filesystem::config_write_probe, a leaf with no app return path"
    );
    assert_eq!(
        seen.relative_up,
        expected(&ALLOWED_HOST_SUPER_REFERENCES),
        "the host must not reach crate children through super::"
    );
    assert_eq!(
        seen.own_module,
        expected(&ALLOWED_HOST_SELF_REFERENCES),
        "the host must not reach config children through self::"
    );
}

fn host_has_only_probe_import(body: &str) -> bool {
    let canonical = "use crate::filesystem::config_write_probe;";
    let mut matches = body.match_indices(canonical);
    let Some((start, _)) = matches.next() else {
        return false;
    };
    if matches.next().is_some() {
        return false;
    }
    let rest = format!("{} {}", &body[..start], &body[start + canonical.len()..]);
    !rest
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .any(|token| token == "filesystem")
}

#[test]
fn the_constant_home_imports_only_the_probe_leaf() {
    let files = files_of(&HOST_MODULE, Reach::OwnFilesOnly).expect("resolve host module");
    assert!(
        !files.is_empty(),
        "host module must resolve to source files"
    );
    for path in files {
        let body = scrubbed(&path, Literals::Drop).expect("scrub host source");
        assert!(
            host_has_only_probe_import(&normalized(&body)),
            "host must have only the canonical probe import and no other filesystem token: {}",
            path.display()
        );
    }
}

#[test]
fn the_host_probe_import_guard_rejects_other_filesystem_spellings() {
    let canonical = "use crate::filesystem::config_write_probe;";
    let accepts = |body: &str| {
        host_has_only_probe_import(&normalized(
            &scrub(body, Literals::Drop).expect("scrub host fixture"),
        ))
    };
    assert!(accepts(canonical));
    assert!(!accepts(""));
    for extra in [
        canonical,
        "use crate::filesystem; filesystem::agent_files::f();",
        "use crate::{filesystem::agent_files};",
        "use crate::filesystem as f; f::agent_files::f();",
        "filesystem::agent_files::f();",
        "use crate::r#filesystem::agent_files;",
    ] {
        assert!(!accepts(&format!("{canonical} {extra}")), "{extra}");
    }
    assert!(accepts(&format!(
        "{canonical} /* filesystem */ let label = \"filesystem\"; // filesystem"
    )));
}

#[test]
fn the_configuration_probe_leaf_names_no_app_modules() {
    let seen = observe(&["filesystem", "config_write_probe"], Reach::WithSubmodules);
    assert!(
        seen.anchored.is_empty(),
        "probe leaf must not name crate modules"
    );
    assert!(
        seen.relative_up.is_empty(),
        "probe leaf must not name super modules"
    );
    assert!(
        seen.own_module.is_empty(),
        "probe leaf must not name self modules"
    );
    assert!(
        seen.aliases.is_empty(),
        "probe leaf must not alias app modules"
    );
    assert!(
        seen.forbidden.is_empty(),
        "probe leaf must not name root_agent"
    );
    assert_eq!(seen.globs, 0, "probe leaf must not glob-import app modules");
}
