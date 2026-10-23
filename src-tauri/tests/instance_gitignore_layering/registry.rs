use super::*;

/// #1446: `config::instance_artifacts` is the artifact registry that
/// `config::instance_gitignore` derives every static rule from, and the reason
/// that reference is allowed in production code is that this module has **zero
/// outgoing arcs**. A node with no way out is a trivial SCC however many modules
/// point at it, so every arc into the registry is safe regardless of whether its
/// source is inside the knot, and #1446 puts arcs into it from more than twenty
/// modules, many of them knot members.
///
/// **This test is what holds that property.** It is the same argument #1273 made
/// for `crate::config`, applied one level down, and it fails the same way: one
/// `use crate::<any knot member>::...;` in the registry and the registry joins
/// the knot, `config::instance_gitignore` follows it through a reference the
/// row in `ALLOWED_GUARDED_SUPER_REFERENCES` now permits, and nothing else in
/// this repository goes red.
///
/// It is read `WithSubmodules`, unlike the constant's home. The registry's only
/// children are its `names` table, which names nothing, and its tests, so the
/// deeper reach refuses a child that parks a reference out of sight.
#[test]
fn the_registry_names_nothing_at_all() {
    let seen = observe(&REGISTRY_MODULE, Reach::WithSubmodules);

    assert!(
        seen.aliases.is_empty(),
        "the artifact registry must not rename the crate root or a module group; \
         see the same assertion for config::instance_gitignore.\n\
         \n\
         OFFENDING FILES: {}",
        seen.aliases.join(", ")
    );

    assert!(
        seen.forbidden.is_empty(),
        "the artifact registry must not name config::root_agent.\n\
         \n\
         WHY: this module is a table of names with no outgoing arcs, and that is \
         the entire reason production code inside `config::instance_gitignore` is \
         allowed to reach it. A path from here into a knot member ends it.\n\
         \n\
         THIS CHECK IS DELIBERATELY ANCHORLESS, for the reason it is in the other \
         two tests: the arc record cannot see a path that begins with neither \
         `crate::` nor `super::`. Comments and literal bodies are removed first, \
         so the registry's own `\"ac-root-agent\"` row does not match.\n\
         \n\
         OFFENDING FILES: {}",
        seen.forbidden.join(", ")
    );

    assert_eq!(
        seen.globs, 1,
        "config::instance_artifacts and its children must contain exactly one glob import.\n\
         \n\
         WHY: the one that exists is the `use super::*;` inside the registry's own \
         `#[cfg(test)] mod tests`, where `super` is the registry itself, so it \
         reaches nothing and costs no arc. A SECOND test module, or a nested one, \
         would add a second glob that the deduplicated set below cannot see, which \
         is the hole this count exists to close. Written at the top level of the \
         file instead, the same three words mean `use crate::config::*;` and pull \
         every child of `config` into scope, `root_agent` among them.\n\
         \n\
         THE COUNT IS STRUCTURAL, NOT TEXTUAL: `use super::super::*;` and \
         `use super::{{*}};` are counted too. Entries 14 and 15.\n\
         \n\
         OBSERVED: {} `super::`-anchored glob imports",
        seen.globs
    );

    assert_eq!(
        seen.anchored,
        expected(&ALLOWED_REGISTRY_CRATE_REFERENCES),
        "the set of crate modules named from config::instance_artifacts moved.\n\
         \n\
         WHY THIS TABLE IS EMPTY: #1446 chose the direction deliberately. The \
         registry declares the artifact names; the modules that write those \
         artifacts alias it. The inverse, a registry that imports each owner's \
         constant, needs arcs in both directions with `api::message_store` and is \
         a two-node cycle by construction.\n\
         \n\
         An arc INTO this module is always safe because it terminates in a node \
         with out-degree zero. An arc OUT of it is what makes every one of those \
         incoming arcs dangerous at once, because more than twenty modules point \
         here after #1446 and many of them are knot members.\n\
         \n\
         INSTEAD: the registry is names and pure string logic. Anything it seems \
         to need from elsewhere in the crate belongs at the call site, not here. \
         Adding the first row is a decision about the crate's shape and has to be \
         argued in the commit."
    );

    assert_eq!(
        seen.relative_up,
        expected(&ALLOWED_REGISTRY_SUPER_REFERENCES),
        "the set of names reached by `super::` from config::instance_artifacts \
         moved.\n\
         \n\
         The one allowed row is the registry's own `#[cfg(test)] mod tests` glob, \
         where `super` is the registry itself. It contributes no arc because the \
         record is emitted without test code, which is why the spelling contract \
         and the arc contract legitimately differ on this one row.\n\
         \n\
         Any other row is the registry reaching up into `crate::config`, whose \
         children are mostly knot members. That is an outgoing arc and it ends the \
         property this whole test exists to hold.\n\
         \n\
         A SMALLER SET is a failure too: it means the scan stopped seeing the test \
         module, and a scan that observes nothing passes everything."
    );

    assert_eq!(
        seen.own_module,
        expected(&ALLOWED_REGISTRY_SELF_REFERENCES),
        "the set of names reached by `self::` from config::instance_artifacts \
         moved.\n\
         \n\
         THE TABLE IS EMPTY ON PURPOSE. `self::x` and `x` name the same item, so \
         this anchor is only a second spelling of a path that begins at this \
         module, and leaving it unasserted leaves that spelling free. Entry 17."
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
         defines nothing, so it is deliberately not counted: 17 references in \
         five files outside the guarded module still spell \
         `crate::config::root_agent::ROOT_AGENT_DIR_NAME` and resolve through \
         it, and the 31 bare `ROOT_AGENT_DIR_NAME` references inside \
         `root_agent.rs` itself resolve through it too. (This said 49 until the \
         #1273 review measured it. 49 is the number of references outside the \
         guarded module by ANY spelling, which is not the same set; plan Section \
         4.4 has the split right.)\n\
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

/// #2713: `config::naming_migration` is called from the crate root in B1b, and
/// the crate root is a member of the one cyclic SCC. A module the root calls
/// joins that SCC the moment it names any member, so this leaf may name
/// `super::instance_artifacts` and nothing else in the crate. Modelled on
/// `instance_gitignore_names_nothing_that_reaches_the_knot`.
pub(crate) const MIGRATION_MODULE: [&str; 2] = ["config", "naming_migration"];
pub(crate) const MIGRATION_FILE: &str = "src/config/naming_migration.rs";
pub(crate) const MIGRATION_IGNORE_ROWS_FILE: &str = "src/config/naming_migration/ignore_rows.rs";
pub(crate) const MIGRATION_TESTS_FILES: [&str; 3] = [
    "src/config/naming_migration/tests.rs",
    "src/config/naming_migration/tests/concurrency.rs",
    "src/config/naming_migration/tests/scenarios.rs",
];

/// The SCC members section 3.1 of the B1a plan names, plus the framework.
/// `super::<name>` and `crate::` are already refused by the equalities below;
/// this catches a bare `<name>::` path reached through an import this scan
/// cannot resolve.
pub(crate) const MIGRATION_FORBIDDEN_SEGMENTS: [&str; 7] = [
    "settings",
    "loops",
    "coding_agents_catalog",
    "local_config_io",
    "sessions_persistence",
    "commands",
    "tauri",
];

#[test]
fn naming_migration_names_nothing_that_reaches_the_knot() {
    let seen = observe(&MIGRATION_MODULE, Reach::WithSubmodules);

    assert!(
        seen.anchored.is_empty(),
        "config::naming_migration must not name any `crate::` path: the crate root \
         is in the cyclic SCC and B1b calls this module from it. OBSERVED: {:?}",
        seen.anchored
    );
    assert!(
        seen.aliases.is_empty(),
        "config::naming_migration must not rename the crate root or the config \
         group; the scan cannot follow a renamed path. OFFENDING FILES: {}",
        seen.aliases.join(", ")
    );
    assert!(
        seen.forbidden.is_empty(),
        "config::naming_migration names `root_agent`, a knot member. OFFENDING \
         FILES: {}",
        seen.forbidden.join(", ")
    );
    assert!(
        seen.own_module.is_empty(),
        "config::naming_migration uses a `self::` path; name items directly. \
         OBSERVED: {:?}",
        seen.own_module
    );
    assert_eq!(
        seen.relative_up,
        expected(&[
            (MIGRATION_FILE, "instance_artifacts"),
            (MIGRATION_IGNORE_ROWS_FILE, "CONFIG_STATE_TARGET_NAME"),
            (MIGRATION_IGNORE_ROWS_FILE, "LOOP_STATE_TARGET_NAME"),
            (MIGRATION_IGNORE_ROWS_FILE, "LOOP_STATE_TMP_TARGET_GLOB"),
            (MIGRATION_IGNORE_ROWS_FILE, "PROJECT_SETTINGS_TARGET_NAME"),
            (MIGRATION_IGNORE_ROWS_FILE, "SET_ASIDE_GLOB"),
            (MIGRATION_TESTS_FILES[0], "*"),
            (MIGRATION_TESTS_FILES[1], "*"),
            (MIGRATION_TESTS_FILES[2], "*"),
        ]),
        "the set of names reached by `super::` from config::naming_migration \
         moved. The only arc out of the module is its one arc, to \
         `config::instance_artifacts`, which has no outgoing arcs; the \
         `ignore_rows` child re-reads five of those names from this module, \
         and each `*` is a test module's `use super::*;`, whose `super` is \
         inside this module. Anything else must be argued as a change to the \
         arc budget of B1a."
    );
    assert_eq!(
        seen.globs,
        MIGRATION_TESTS_FILES.len(),
        "config::naming_migration must contain exactly one glob import per test \
         module, its `use super::*;`"
    );

    let files = files_of(&MIGRATION_MODULE, Reach::WithSubmodules).expect("module files");
    for path in &files {
        let code = normalized(&scrubbed(path, Literals::Drop).expect("scrub"));
        for segment in MIGRATION_FORBIDDEN_SEGMENTS {
            let needle = format!("{segment}::");
            let named = code.match_indices(&needle).any(|(at, _)| {
                !code[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|character| character.is_alphanumeric() || character == '_')
            });
            assert!(
                !named,
                "{} names `{segment}::`, a module inside the cyclic SCC or the \
                 framework; this leaf may name only super::instance_artifacts, \
                 std, serde, serde_json, chrono and the OS bindings",
                relative_of(path)
            );
        }
    }
}
