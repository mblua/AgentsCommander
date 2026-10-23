/// Every `(file, child)` reference the guarded module is allowed to make under
/// `crate::`, sorted.
///
/// **Empty, and the emptiness is the contract.** Before #1273 this module
/// contained no `crate::` path at all, and it still does not: everything it
/// needs is one level up and is written `super::`. An empty equality therefore
/// refuses **every** `crate::`-anchored reference, not merely a reference to
/// `config::root_agent`, and that breadth is the point. The module's exposure is
/// that it may name any other knot member and fall straight back
/// in; `root_agent` is only today's spelling.
///
/// Adding the first row here is a decision about the crate's shape and must be
/// argued in the commit, not slipped in to get green.
pub(crate) const ALLOWED_GUARDED_CRATE_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the guarded module is allowed to make under
/// `super::`, sorted.
///
/// This is where this module actually lives, so this is the table that has to be
/// right. Seven pairs, one file:
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
/// - `instance_artifacts` is the artifact registry #1446 added, and unlike
///   `injected_messages` it is allowed in **production** code. The reason is a
///   property of the target, not of the position: that module has zero outgoing
///   arcs, so it is a trivial SCC whatever points at it, and an arc into it can
///   neither join nor create a cycle. `the_registry_names_nothing_at_all` below
///   is what holds that property, which is why this row and that test have to be
///   read together. The registry is where `required_rules` gets the artifact
///   table it derives every static rule from.
/// - `super` is the leading segment of every `super::super::...` path, reported by
///   the matcher as itself rather than dropped.
///
/// The pair is the contract, not the child on its own. Keying on the child alone
/// would make the observed set a union over every scanned file, so a reference
/// added to a future submodule of this module would leave the set unmoved and
/// pass.
pub(crate) const ALLOWED_GUARDED_SUPER_REFERENCES: [(&str, &str); 93] = [
    ("src/config/instance_gitignore.rs", "ROOT_AGENT_DIR_NAME"),
    ("src/config/instance_gitignore.rs", "agent_local_dir_name"),
    ("src/config/instance_gitignore.rs", "config_dir"),
    ("src/config/instance_gitignore.rs", "instance_artifacts"),
    ("src/config/instance_gitignore.rs", "naming_migration"),
    ("src/config/instance_gitignore/tests.rs", "*"),
    (
        "src/config/instance_gitignore/tests.rs",
        "ROOT_AGENT_DIR_NAME",
    ),
    (
        "src/config/instance_gitignore/tests.rs",
        "instance_artifacts",
    ),
    ("src/config/instance_gitignore/tests.rs", "super"),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "ArtifactKind",
    ),
    ("src/config/instance_gitignore/tests/ensure.rs", "Duration"),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "HISTORICAL_FIXED_RULES",
    ),
    ("src/config/instance_gitignore/tests/ensure.rs", "HashSet"),
    ("src/config/instance_gitignore/tests/ensure.rs", "Instant"),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "OpenOptions",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "ROOT_AGENT_DIR_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "TEST_AGENT_LOCAL_DIR",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "assert_git_ignore_status",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "assert_git_success",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "ensure_instance_gitignore_at",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "expected_fresh_bytes",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "ignore_rows",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "instance_artifacts",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "legacy_complete_bytes",
    ),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "missing_rule_indexes",
    ),
    ("src/config/instance_gitignore/tests/ensure.rs", "push_pair"),
    ("src/config/instance_gitignore/tests/ensure.rs", "render"),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "required_rules",
    ),
    ("src/config/instance_gitignore/tests/ensure.rs", "super"),
    (
        "src/config/instance_gitignore/tests/ensure.rs",
        "track_rows",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "ArtifactKind",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "ROOT_AGENT_DIR_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "TEST_AGENT_LOCAL_DIR",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "assert_git_ignore_status",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "assert_git_success",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "contains_exact_line",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "ensure_instance_gitignore_at",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "escape_gitignore_path_segment",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "fresh_file_bytes",
    ),
    ("src/config/instance_gitignore/tests/git_fixture.rs", "git"),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "ignore_rows",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "missing_rule_indexes",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "push_pair",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "required_rules",
    ),
    (
        "src/config/instance_gitignore/tests/git_fixture.rs",
        "super",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "AGENTS_INSTANCE_FILE_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "AGENTS_INSTANCE_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "BLOCKING_MENUS_LOCAL_FILE_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "BLOCKING_MENUS_LOCAL_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "BLOCKING_MENUS_REMOTE_CHECK_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "BLOCKING_MENUS_REMOTE_FILE_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "BLOCKING_MENUS_REMOTE_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "BLOCKING_MENUS_SHIPPED_FILE_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "BLOCKING_MENUS_SHIPPED_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "CODING_AGENTS_BASE_FILENAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "CODING_AGENTS_BASE_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "CODING_AGENTS_LOCAL_FILENAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "CODING_AGENTS_LOCAL_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "CODING_AGENTS_LOCK_FILENAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "CODING_AGENTS_LOCK_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "CODING_AGENTS_MIGRATION_BACKUP_FILENAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "CODING_AGENTS_MIGRATION_JOURNAL_FILENAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "CODING_AGENTS_PROJECT_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "LAYERED_NAME_PROBES",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "LAYER_DEFAULT",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "LAYER_INSTANCE",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "LAYER_PERSONAL",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "LAYER_PROJECT",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "LAYER_REMOTE",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "NO_GIT_MARKER",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "RETIRED_PAIRS",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_BACKUP_PREFIX",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_BACKUP_ROTATION_GLOB",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_BACKUP_ROTATION_TARGET_GLOB",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_BACKUP_SUFFIX",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_BACKUP_TARGET_PREFIX",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_FILE_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_LOCAL_OVERRIDE_FILE_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_LOCAL_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_LOCK_FILE_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_LOCK_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_MIGRATION_BACKUP_GLOB",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_MIGRATION_BACKUP_TARGET_GLOB",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "SETTINGS_TARGET_NAME",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "STATE_MARKER",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "TEST_AGENT_LOCAL_DIR",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "assert_git_ignore_status",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "assert_git_success",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "ensure_instance_gitignore_at",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "injected_messages",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "required_rules",
    ),
    (
        "src/config/instance_gitignore/tests/registry_names.rs",
        "super",
    ),
];

/// Every `(file, child)` reference the constant's home is allowed to make under
/// `crate::`, sorted.
///
/// **Empty.** `src/config.rs` measures zero outgoing arcs over the record, and
/// that measurement is the whole non-absorption argument of #1273: a module with
/// no way out cannot reach a knot member, so it cannot share an SCC with one, so
/// nothing that depends only on it can either.
pub(crate) const ALLOWED_HOST_CRATE_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the constant's home is allowed to make under
/// `super::`, sorted.
///
/// Empty. The test module's `use super::*;` moved to `src/config/tests.rs`,
/// which this observation does not read. From `src/config.rs`, `super::`
/// at module level would mean the crate root, so any row appearing here
/// is a reference from `config` up into the crate root's children,
/// which is the inversion this guard exists to refuse.
pub(crate) const ALLOWED_HOST_SUPER_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the guarded module is allowed to make under
/// `self::`, sorted.
///
/// **Empty, and for the same reason the `crate::` table is.** `self::x` and
/// `x` name the same item, so this anchor is a second spelling of every path
/// that begins at the module itself, and this module has no such path today.
///
/// #1273 review, entry 17: before this table existed, `self::` was scanned under
/// no anchor anywhere in this file, so it was a free spelling for reaching
/// anything in scope. An empty equality refuses the whole class rather than the
/// one name `root_agent`.
pub(crate) const ALLOWED_GUARDED_SELF_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the constant's home is allowed to make under
/// `self::`, sorted.
///
/// **Empty, and this is the one that was measured to matter.** From
/// `src/config.rs`, `self::` means `crate::config`, so
/// `use self::root_agent::...;` reaches a knot member from the module whose
/// zero-outgoing-arc property the whole of #1273 Section 4.3 rests on. Measured
/// during the #1273 review: that one line took the knot 87 to **91** and
/// `sccSize(config::instance_gitignore)` 1 to **91**, with the guard green,
/// because no anchor here was reading `self::` at all. Unlike entry 13 the
/// detector is **not** blind to it: `self::` resolves and the arc is recorded.
pub(crate) const ALLOWED_HOST_SELF_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the artifact registry is allowed to make
/// under `crate::`, sorted.
///
/// **Empty, and the emptiness is the whole reason the registry is safe to
/// depend on.** #1446 inverted the natural direction on purpose: the registry
/// declares the artifact names and the modules that write them alias it, rather
/// than the registry importing each owner's constant. The inverse would have
/// needed an arc in both directions with `api::message_store`, which is a
/// two-node cycle by construction. As written, every arc into this module
/// terminates in a node with out-degree zero, which is a trivial SCC no matter
/// its in-degree, so no arc into it can join, grow or create a cycle.
pub(crate) const ALLOWED_REGISTRY_CRATE_REFERENCES: [(&str, &str); 0] = [];

/// Every `(file, child)` reference the artifact registry is allowed to make
/// under `super::`, sorted.
///
/// One row, and it is the same shape as the constant home's: the `use super::*;`
/// inside the registry's own `#[cfg(test)] mod tests` (`instance_artifacts/tests.rs`),
/// where `super` is the registry itself. That glob resolves to the module under
/// test and contributes no arc to the record, which is emitted without test code,
/// so it does not cost the zero-outgoing-arc property. Any other row here is the
/// registry reaching into `crate::config`, which ends that property and with it
/// the reason `config::instance_gitignore` may name this module from production
/// code.
pub(crate) const ALLOWED_REGISTRY_SUPER_REFERENCES: [(&str, &str); 1] =
    [("src/config/instance_artifacts/tests.rs", "*")];

/// Every `(file, child)` reference the artifact registry is allowed to make
/// under `self::`, sorted.
///
/// **Empty, for the reason the guarded module's table is empty.** `self::x` and
/// `x` name the same item, so this anchor is only a second spelling, and leaving
/// it unasserted would leave that spelling free.
pub(crate) const ALLOWED_REGISTRY_SELF_REFERENCES: [(&str, &str); 0] = [];

/// The module #1273 cut this one away from, matched as a bare identifier under
/// every anchor and under none.
///
/// This is the one check in the file that does not depend on an anchor, and it
/// exists because of entry 8: a bare `root_agent::ROOT_AGENT_DIR_NAME` would
/// compile if the name were in scope, would rebuild the dependency, and would be
/// invisible to the arc record and to both equalities. Comments and literal
/// bodies are removed first, so the string `"ac-root-agent"` and the
/// `.gitignore` fixtures in this module's tests do not match it.
pub(crate) const FORBIDDEN_NAME: &str = "root_agent";

pub(crate) const CRATE_ANCHOR: &str = "crate::";
pub(crate) const SUPER_ANCHOR: &str = "super::";

/// The third anchor, added by the #1273 review to close entry 17.
///
/// `self::` is a legal prefix on any path that starts at the current module, so
/// leaving it unscanned left a spelling that reached every child of the module
/// under no anchor this file was reading. Both tests assert it against an empty
/// table, so it refuses the class and not merely `root_agent`.
pub(crate) const SELF_ANCHOR: &str = "self::";

/// The module this guard is written about, as path segments below `crate`.
pub(crate) const GUARDED_MODULE: [&str; 2] = ["config", "instance_gitignore"];

/// The module #1273 moved the constant into, as path segments below `crate`.
pub(crate) const HOST_MODULE: [&str; 1] = ["config"];

/// The artifact registry #1446 added, as path segments below `crate`.
pub(crate) const REGISTRY_MODULE: [&str; 2] = ["config", "instance_artifacts"];

/// The constant #1273 moved, and the one file that may define it.
///
/// `defines_the_constant` decides what counts as a definition, because the
/// re-export left behind in `src/config/root_agent.rs` names the same identifier
/// and must not be counted as one, while a `static` copy must be.
pub(crate) const CONSTANT_NAME: &str = "ROOT_AGENT_DIR_NAME";
pub(crate) const CONSTANT_HOME: &str = "src/config.rs";

/// The leading segment `children_under` reports for a glob import.
///
/// Counting globs means counting `super::`-anchored items whose leading segment
/// is this, taken **before** the observed set is deduplicated. See entry 9 of the
/// uncovered list for why the count exists at all.
///
/// #1273 review, entries 14 and 15: this count used to be the number of
/// occurrences of the literal text `use super::*;`. Neither
/// `use super::super::*;` nor `use super::{*};` contains that text, so both were
/// measured slipping past a count whose whole purpose was to be exactly 1, while
/// the children they report were already allowed rows. Counting structurally
/// closes both, and closes any other punctuation or nesting that reaches a glob
/// through `super::`, because `children_under` normalizes the path before it
/// reads it.
pub(crate) const GLOB_CHILD: &str = "*";

/// Whether literal bodies survive `scrub`.
///
/// They must survive when the text is about to be read for `path = "..."`,
/// and must not when it is about to be read for dependencies or for structure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Literals {
    Keep,
    Drop,
}

/// Whether a module's submodules are read with it.
///
/// `WithSubmodules` for the guarded module, so a reference cannot be parked in a
/// future child of it, and for the artifact registry, whose only children are
/// its `names` table and its tests, so a reference parked there is still seen.
/// `OwnFilesOnly` for the constant's home, because every child of `config` is a
/// separate module in the graph with its own arcs and most of them are knot
/// members.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    OwnFilesOnly,
    WithSubmodules,
}
