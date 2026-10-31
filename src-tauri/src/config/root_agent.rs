mod context_snapshots;
mod file_io;
mod legacy_contexts;
mod provisioning;
mod skill_files;
mod skill_snapshots;

pub use crate::config::ROOT_AGENT_DIR_NAME;
#[cfg(test)]
pub(crate) use context_snapshots::frozen_root_context_snapshots;
#[cfg(test)]
use context_snapshots::ROOT_CONTEXT_BEFORE_REPORTING_SCOPE_MD;
use context_snapshots::ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD;
pub(crate) use context_snapshots::{
    default_root_context_template, is_known_generated_root_context_template,
};
pub use file_io::read_last_coding_agent;
pub(crate) use file_io::{atomic_replace_existing, merge_root_agent_config};
use file_io::{
    cleanup_temp_role, create_default_skill_temp_file, display_path, normalize_role_text,
    paths_equivalent, publish_missing_default_skill_file, publish_missing_role_file,
    replace_role_file, sync_role_dir, unique_role_temp_path, validate_default_skill_file,
    write_role_file,
};
#[cfg(test)]
use file_io::{create_default_skill_temp_file_with, publish_missing_default_skill_file_with};
use legacy_contexts::{
    OLD_DEFERRED_MESSAGING_PARAGRAPH, OLD_ROOT_ROLE_MD, ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD,
    ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD, ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD,
    ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD, ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD,
    ROOT_COORDINATION_MESSAGING_PARAGRAPH,
    ROOT_COORDINATION_MESSAGING_PARAGRAPH_BEFORE_ROOM_RENAME, ROOT_ROLE_MD,
};
pub(crate) use provisioning::root_agent_path_requires_protection;
#[cfg(test)]
use provisioning::MINIMAL_ROOT_ROLE_MD;
pub(crate) use provisioning::{ensure_default_root_agent_skills_at, verify_live_root_agent_path};
pub use provisioning::{ensure_root_agent_dir, is_root_agent_dir_name, is_root_agent_path};
#[cfg(test)]
pub(crate) use provisioning::{ensure_root_agent_dir_at, is_root_agent_path_at};
use provisioning::{
    is_link_or_reparse, validate_default_skill_directory, validate_root_agent_root_path,
    validate_root_agent_skills_root,
};
#[cfg(test)]
use serde_json::Value;
use skill_files::ensure_default_skill_file;
#[cfg(test)]
use skill_files::{max_migratable_len, migrate_default_skill_file, SkillMigration, SkipReason};
#[cfg(test)]
pub(crate) use skill_snapshots::{agency_pre_yaml_fix_snapshot, default_root_skill_dir_names};
use skill_snapshots::{
    AGENCY_AGENTS_ROLES_BEFORE_TOKEN_MINIMIZATION, AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX,
    ROLE_SKILL_BOUNDARY_AUDIT_BEFORE_TOKEN_MINIMIZATION,
};
#[cfg(test)]
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};

pub(crate) const CONTEXT_TOKEN_GLOBAL: &str = "$AGENTSCOMMANDER_CONTEXT";

pub const ROOT_AGENT_SESSION_NAME: &str = "Root Agent";
pub const ROOT_AGENT_SENDER: &str = "agentscommander://root-agent";
pub const ROOT_AGENT_SHORT_NAME: &str = "root";
/// Canonical Root context order. The selected file owns all instructional prose.
const ROOT_AGENT_DEFAULT_CONTEXT: &[&str] = &["../Context.root-agent.md", "Role.md"];
/// Frozen pre-979 default, kept for exact legacy recognition during migration.
const ROOT_AGENT_PRE_979_DEFAULT_CONTEXT: &[&str] =
    &[CONTEXT_TOKEN_GLOBAL, "../Context.root-agent.md", "Role.md"];
/// Older default, kept because it must still upgrade to the new default (and so
/// gain the Root supplement) rather than degrade to a bare `["Role.md"]`.
const ROOT_AGENT_OLD_DEFAULT_CONTEXT: &[&str] = &[CONTEXT_TOKEN_GLOBAL, "Role.md"];
const ROOT_AGENT_SKILLS_DIR: &str = "skills";
const SKILL_MD_FILENAME: &str = "SKILL.md";
/// #909: slack added to the cap in `max_migratable_len`, covering the leading and trailing
/// whitespace that `normalize_role_text` trims away.
const MAX_DEFAULT_SKILL_TRIM_SLACK_BYTES: u64 = 4096;
#[cfg(test)]
static FAIL_ROOT_ROLE_WRITE_ONCE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
const FAIL_ROOT_ROLE_WRITE_MARKER: &str = "FAIL_ROOT_ROLE_WRITE_ONCE";

struct DefaultRootSkill {
    dir_name: &'static str,
    file_name: &'static str,
    content: &'static str,
    /// #909: frozen, full-text snapshots of versions of this file that we previously shipped and
    /// now consider defective. An on-disk file whose normalized text equals one of these is a
    /// pristine stale default, safe to rewrite. Anything else is a user edit and is never touched.
    ///
    /// An empty list means "never repair this skill", which is today's behavior exactly.
    legacy_snapshots: &'static [&'static str],
}

pub(crate) const SELF_HANDOFF_SKILL_CONTENT: &str =
    include_str!("default_skills/self-handoff/SKILL.md");

const SELF_HANDOFF_DEFAULT_SKILL: DefaultRootSkill = DefaultRootSkill {
    dir_name: "self-handoff",
    file_name: SKILL_MD_FILENAME,
    content: SELF_HANDOFF_SKILL_CONTENT,
    // First shipped version: future versions freeze their predecessors here.
    legacy_snapshots: &[],
};

/// Reuse the existing create-only publisher and pristine-version repair for projects.
pub(crate) fn ensure_project_self_handoff_skill_at(ac_root: &Path) -> Result<(), String> {
    let skills_root = ac_root.join(crate::config::shared_locations::PROJECT_SKILLS_DIR);
    provisioning::ensure_default_root_skill_file(ac_root, &skills_root, &SELF_HANDOFF_DEFAULT_SKILL)
}

const DEFAULT_ROOT_SKILLS: &[DefaultRootSkill] = &[
    DefaultRootSkill {
        dir_name: "role-skill-boundary-audit",
        file_name: SKILL_MD_FILENAME,
        content: include_str!("root_agent_defaults/role-skill-boundary-audit/SKILL.md"),
        legacy_snapshots: &[ROLE_SKILL_BOUNDARY_AUDIT_BEFORE_TOKEN_MINIMIZATION],
    },
    DefaultRootSkill {
        dir_name: "agency-agents-roles",
        file_name: SKILL_MD_FILENAME,
        content: include_str!("root_agent_defaults/agency-agents-roles/SKILL.md"),
        legacy_snapshots: &[
            AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX,
            AGENCY_AGENTS_ROLES_BEFORE_TOKEN_MINIMIZATION,
        ],
    },
    #[cfg(windows)]
    DefaultRootSkill {
        dir_name: "rtk-install-windows",
        file_name: SKILL_MD_FILENAME,
        content: include_str!("root_agent_defaults/rtk-install-windows/SKILL.md"),
        legacy_snapshots: &[],
    },
    SELF_HANDOFF_DEFAULT_SKILL,
];

/// Returns `true` iff `target` is the canonical Root Agent reply name.
///
/// Symmetric with `ROOT_AGENT_SENDER` (the `msg.from` value the Root Agent
/// writes when it sends): any peer that received that value as `from` MUST
/// be able to round-trip it back as `--to`.
pub fn is_root_agent_target(target: &str) -> bool {
    target == ROOT_AGENT_SENDER
}

static OLD_ROOT_CONTEXT_WITH_COORDINATION_MD: LazyLock<String> = LazyLock::new(|| {
    format!(
        concat!(
            r#"---
name: 'agents-commander'
description: 'Root coordinator for AgentsCommander sessions, workgroups, and agents.'
type: agent
---

# Agents Commander

You are the AgentsCommander Root Agent. You are the top-level coordinator for this "#,
            r#"AgentsCommander binary.

## Responsibility

Act as the top-level planning and oversight agent for sessions, workgroups, and agents available "#,
            r#"to this AgentsCommander instance. Help the user inspect available work, plan "#,
            r#"delegation, track status, and synthesize results. When direct peer "#,
            r#"messaging is unavailable, say so plainly and ask the user to route "#,
            r#"messages or wait for a future root messaging feature rather than "#,
            r#"claiming sends were performed.

## State

Your own canonical state lives in this `ac-root-agent` directory:

- `memory/`
- `plans/`
- `skills/`
- `Role.md`

You are not a workgroup replica and you do not have an origin Agent Matrix. Use this directory "#,
            r#"for your own durable state.

## Coordination

Use the AgentsCommander CLI only for commands that are valid from this root-agent directory. "#,
            r#"Follow the write restrictions in the common context "#,
            r#"exactly.

{ROOT_COORDINATION_MESSAGING_PARAGRAPH_BEFORE_ROOM_RENAME}
"#
        ),
        ROOT_COORDINATION_MESSAGING_PARAGRAPH_BEFORE_ROOM_RENAME =
            ROOT_COORDINATION_MESSAGING_PARAGRAPH_BEFORE_ROOM_RENAME
    )
});

pub fn root_agent_dir() -> Result<String, String> {
    static ROOT_DIR: OnceLock<String> = OnceLock::new();
    if let Some(cached) = ROOT_DIR.get() {
        return Ok(cached.clone());
    }

    let config_dir =
        super::config_dir().ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let root_dir = display_path(&config_dir.join(ROOT_AGENT_DIR_NAME));
    let _ = ROOT_DIR.set(root_dir.clone());
    Ok(root_dir)
}

/// One WARN per path per process; `debug!` thereafter.
///
/// We cannot enumerate the sticky failures. The `ReadOnly` classification catches the one we
/// measured, but a parent directory that denies `FILE_DELETE_CHILD` alongside a deny-`Delete` DACL
/// would fail `ReplaceFileW` forever and is invisible to the attribute check. So the memo lives at
/// the swallow point, not only at the read-only branch, and it closes the whole class rather than
/// one instance.
///
/// No information is lost: `discover_skill_index` keeps emitting its own per-build warning for any
/// skill it still cannot parse (`session_context.rs:45-47`). The user always learns the skill was
/// skipped; they learn our detailed cause once.
///
/// Keyed by path, not a global `AtomicBool`: `session_context.rs:38` gates on
/// `is_root_agent_dir_name`, a BASENAME comparison, so one process can seed several distinct roots.
///
/// **Suppressible at exactly one runtime level, and not the one you would guess.** Measured against
/// `LevelGateLogger` (`logging.rs:80-97`), which gates our own `agentscommander*` targets on
/// `(record.level() as u8) <= runtime_level`:
///
/// | runtime level | this `warn!` is logged | the repair's `info!` is logged |
/// |---|---|---|
/// | Error | **no** | no |
/// | Warn | yes | no |
/// | Info or lower | yes | yes |
///
/// `max_filter_for` (`logging.rs:66`) clamps the global max level to at least Warn, so the `warn!`
/// macro never short-circuits. That clamp exists to keep **third-party** warnings visible, and
/// those
/// take the `self.inner.enabled(..)` branch. Ours do not: at runtime level Error the gate rejects
/// them. A user who has selected Error learns nothing from a failed repair, not even from
/// `discover_skill_index`'s own per-build warning, which is a `warn!` on the same target and is
/// suppressed identically.
///
/// **This set is append-only, and two tests depend on that.** `warn_once_for_path` is its only
/// writer; there is no `remove`, `clear`, `drain`, or `retain` anywhere. That is what lets
/// `migration_skips_non_utf8_file` and the healthy branch of
/// `migration_skips_read_only_destination`
/// assert `!warned_for_path(..)` *after* the act without also asserting it before: a passing
/// post-condition implies the pre-condition only while the set never shrinks. Adding a
/// `#[cfg(test)]`
/// reset helper is a tempting and otherwise reasonable thing to want; it would silently weaken both
/// assertions, and both would then need an explicit pre-assertion, as the positive sites already
/// have.
static SKILL_REPAIR_WARNED: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

#[cfg(test)]
type PrePublishHook = Box<dyn Fn(&Path) -> Result<(), String> + Send>;

/// A MAP keyed by path, not a single slot. `cargo test` runs tests as parallel threads in one
/// process, so with one slot test B's arming overwrites test A's, A finds a path that does not
/// match, skips its hook, and publishes. Intermittent red, and intermittent reds get silenced. The
/// unique *path* never made the single *slot* safe.
///
/// `FAIL_ROOT_ROLE_WRITE_ONCE` is a global `AtomicBool` that is safe only because it is
/// additionally content-gated by a marker inside the content string. That gate is the whole of its
/// safety, and it does not generalize.
///
/// `Send` on `PrePublishHook` is mandatory: a `static` must be `Sync`, and `Mutex<T>: Sync`
/// requires
/// `T: Send`. Omitting it is E0277.
#[cfg(test)]
static PRE_PUBLISH_HOOKS: LazyLock<Mutex<HashMap<PathBuf, PrePublishHook>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
fn run_pre_publish_hook(path: &Path) -> Result<(), String> {
    // Take the closure OUT of the map before calling it, so a panicking hook cannot poison the lock
    // for every later test and a re-entrant hook cannot deadlock.
    let hook = {
        let mut guard = PRE_PUBLISH_HOOKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.remove(path)
    };
    match hook {
        Some(hook) => hook(path),
        None => Ok(()),
    }
}

/// RAII: a panicking test must not leave a closure armed for whatever test reuses the path.
#[cfg(test)]
struct ArmedHook(PathBuf);

#[cfg(test)]
impl ArmedHook {
    fn new(path: &Path, hook: PrePublishHook) -> Self {
        PRE_PUBLISH_HOOKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(path.to_path_buf(), hook);
        ArmedHook(path.to_path_buf())
    }
}

#[cfg(test)]
impl Drop for ArmedHook {
    fn drop(&mut self) {
        PRE_PUBLISH_HOOKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.0);
    }
}

#[cfg(test)]
mod tests;
