use serde_json::Value;
#[cfg(test)]
use std::collections::HashMap;
use std::collections::HashSet;
// `Read` is for `Take::read_to_end` in `read_bounded`. Nobody may "fix" a missing-`Read` error by
// reverting the repair path to `std::fs::read_to_string`: that re-opens by path and enforces no
// bound at the handle.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";
pub const ROOT_AGENT_SESSION_NAME: &str = "Root Agent";
pub const ROOT_AGENT_SENDER: &str = "agentscommander://root-agent";
pub const ROOT_AGENT_SHORT_NAME: &str = "root";
const ROOT_AGENT_DEFAULT_CONTEXT: &[&str] = &[
    "$AGENTSCOMMANDER_CONTEXT",
    "../Context.root-agent.md",
    "Role.md",
];
const ROOT_AGENT_OLD_DEFAULT_CONTEXT: &[&str] = &["$AGENTSCOMMANDER_CONTEXT", "Role.md"];
const ROOT_AGENT_SKILLS_DIR: &str = "skills";
const SKILL_MD_FILENAME: &str = "SKILL.md";
/// #909: slack added to the cap in `max_migratable_len`, covering the leading and trailing
/// whitespace that `normalize_role_text` trims away.
const MAX_DEFAULT_SKILL_TRIM_SLACK_BYTES: u64 = 4096;
static ROOT_ROLE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
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

const DEFAULT_ROOT_SKILLS: &[DefaultRootSkill] = &[
    DefaultRootSkill {
        dir_name: "role-skill-boundary-audit",
        file_name: SKILL_MD_FILENAME,
        content: include_str!("root_agent_defaults/role-skill-boundary-audit/SKILL.md"),
        legacy_snapshots: &[],
    },
    DefaultRootSkill {
        dir_name: "agency-agents-roles",
        file_name: SKILL_MD_FILENAME,
        content: include_str!("root_agent_defaults/agency-agents-roles/SKILL.md"),
        legacy_snapshots: &[AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX],
    },
];

/// #909: the `agency-agents-roles/SKILL.md` we shipped from `00eca16` (PR #682) through
/// `646aeac`. Its `description` is an unquoted YAML plain scalar containing an inner
/// `": "`, so `serde_yaml` rejects it at line 2 column 105 and `discover_skill_index`
/// drops the skill entirely. Frozen here so a pristine copy can be recognized and
/// repaired without ever touching a user-edited file.
///
/// Generated, never transcribed:
///     git show 646aeac:src-tauri/src/config/root_agent_defaults/agency-agents-roles/SKILL.md
///
/// A raw string literal rather than an `include_str!` of a frozen `.md`, because
/// `.gitattributes` pins `*.rs` to `text eol=lf` but carries no rule for `*.md` (#914).
/// An `include_str!`ed `.md` would be CRLF on a Windows `autocrlf` checkout and LF on CI,
/// so its raw digest would differ per machine. This literal is LF everywhere.
///
/// Pinned by `agency_agents_roles_pre_yaml_fix_snapshot_is_byte_exact`, on both the raw
/// form (2576 bytes) and the normalized form. The normalized pin alone is blind to
/// exactly what normalization erases.
const AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX: &str = r#"---
name: agency-agents-roles
description: How the Root Agent offers Agency Agents role templates before creating any specialist agent: the mandatory offer, identifying Agency Agents from real local data (its source repo and cached templates, never invented), the bounded skip exceptions, the agency-templates CLI flow, and handling a missing local template cache.
when_to_use: Load before creating any new specialist agent, i.e. before any role-defined create-agent-matrix. Also whenever the user asks to add, create, or set up a new specialist role or agent.
---

# agency-agents-roles

## Mandatory offer before creating a specialist agent

Before you create any new specialist agent (any role-defined `create-agent-matrix`), you MUST first offer Agency Agents role templates. This is mandatory, not discretionary.

Skip the offer ONLY if, in this session, the user already declined Agency templates or explicitly asked for a custom or from-scratch role.

## Say what Agency Agents is, from real data only

When you offer, briefly say what Agency Agents is, but state ONLY what real local data supports. Never invent a description or recall one from memory.

- Agency Agents is a collection of tested, shareable role templates published in a source repository. The real source is the `repo` value reported by `agency-templates status` (and stored in the cache manifest), not a URL you guess.
- There is no local one-line project description to quote. Describe Agency Agents concretely by its source repo plus the actual templates available (their real names and 1-line descriptions from `agency-templates list`), not with invented prose.
- If the template cache is absent (status reports it unavailable), say so and offer to fetch it. Ask before downloading or updating, because it writes to the local template cache.

## On acceptance, use the CLI

Use the AgentsCommander CLI from `AGENTSCOMMANDER_BINARY_PATH`:

    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates update --ref main
    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates status --pretty
    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates list --pretty

`update` refreshes the local cache from the source repo (`--ref` selects the git ref, default `main`). `status` reports whether a cache is present and its repo, ref, and commit. `list` prints each cached template's real `id` and 1-line `description`.

Then present the candidate template(s) and create with `create-agent-matrix --role-template <id>`. Use only the IDs and descriptions that command returns; never invent template IDs or descriptions.
"#;

/// Test-only view of the shipped default skill table, so the indexer gates in
/// `session_context::tests` can assert coverage without widening the table's visibility.
/// `root_agent` already answers questions about its private constants through accessors;
/// see `default_root_context_template` and `is_known_generated_root_context_template`.
#[cfg(test)]
pub(crate) fn default_root_skill_dir_names() -> Vec<&'static str> {
    DEFAULT_ROOT_SKILLS
        .iter()
        .map(|skill| skill.dir_name)
        .collect()
}

/// Test-only. `session_context::tests` writes this to disk, with `\r\n`, to model the state
/// every broken install on disk is actually in.
#[cfg(test)]
pub(crate) fn agency_pre_yaml_fix_snapshot() -> &'static str {
    AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX
}

/// Returns `true` iff `target` is the canonical Root Agent reply name.
///
/// Symmetric with `ROOT_AGENT_SENDER` (the `msg.from` value the Root Agent
/// writes when it sends): any peer that received that value as `from` MUST
/// be able to round-trip it back as `--to`.
pub fn is_root_agent_target(target: &str) -> bool {
    target == ROOT_AGENT_SENDER
}

const OLD_DEFERRED_MESSAGING_PARAGRAPH: &str = "Direct file-based workgroup messaging is not available from the root-agent directory yet: `send --send` currently requires a workgroup replica root. Do not claim that you can autonomously message workgroup peers until a future root messaging feature adds explicit root-aware send instructions.";

const ROOT_COORDINATION_MESSAGING_PARAGRAPH: &str = r#"You may message verified workgroup coordinator replicas only. Before sending, run `list-peers-lean` with your `AGENTSCOMMANDER_*` credentials and use only the `name` values it returns. In Root Agent sessions this list omits origin coordinators and non-coordinator replicas.

Root messaging is file-based:

1. Write the message to `messaging/` inside this `ac-root-agent` directory.
2. Use a filename shaped like `YYYYMMDD-HHMMSS-root-to-<wgN>-<coordinator>-<slug>.md`.
3. Send it with:

```text
"<AGENTSCOMMANDER_BINARY_PATH>" send --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>" --to "<coordinator_name>" --send <filename> --mode wake
```

Never send to origin coordinators or non-coordinator specialist/member agents from this root session.

Coordinators may reply by sending to `agentscommander://root-agent`; their replies appear in this session as standard file notifications."#;

const OLD_ROOT_ROLE_MD: &str = r#"---
name: 'agents-commander'
description: 'Root coordinator for AgentsCommander sessions, workgroups, and agents.'
type: agent
---

# Agents Commander

You are the AgentsCommander Root Agent. You are the top-level coordinator for this AgentsCommander binary.

## Responsibility

Act as the top-level planning and oversight agent for sessions, workgroups, and agents available to this AgentsCommander instance. Help the user inspect available work, plan delegation, track status, and synthesize results. When direct peer messaging is unavailable, say so plainly and ask the user to route messages or wait for a future root messaging feature rather than claiming sends were performed.

## State

Your own canonical state lives in this `ac-root-agent` directory:

- `memory/`
- `plans/`
- `skills/`
- `Role.md`

You are not a workgroup replica and you do not have an origin Agent Matrix. Use this directory for your own durable state.

## Coordination

Use the AgentsCommander CLI only for commands that are valid from this root-agent directory. Follow the write restrictions in the common context exactly.

Direct file-based workgroup messaging is not available from the root-agent directory yet: `send --send` currently requires a workgroup replica root. Do not claim that you can autonomously message workgroup peers until a future root messaging feature adds explicit root-aware send instructions.
"#;

static OLD_ROOT_CONTEXT_WITH_COORDINATION_MD: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"---
name: 'agents-commander'
description: 'Root coordinator for AgentsCommander sessions, workgroups, and agents.'
type: agent
---

# Agents Commander

You are the AgentsCommander Root Agent. You are the top-level coordinator for this AgentsCommander binary.

## Responsibility

Act as the top-level planning and oversight agent for sessions, workgroups, and agents available to this AgentsCommander instance. Help the user inspect available work, plan delegation, track status, and synthesize results. When direct peer messaging is unavailable, say so plainly and ask the user to route messages or wait for a future root messaging feature rather than claiming sends were performed.

## State

Your own canonical state lives in this `ac-root-agent` directory:

- `memory/`
- `plans/`
- `skills/`
- `Role.md`

You are not a workgroup replica and you do not have an origin Agent Matrix. Use this directory for your own durable state.

## Coordination

Use the AgentsCommander CLI only for commands that are valid from this root-agent directory. Follow the write restrictions in the common context exactly.

{ROOT_COORDINATION_MESSAGING_PARAGRAPH}
"#
    )
});

const ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD: &str = r#"---
name: 'agents-commander'
description: 'Static supplemental root context for AgentsCommander.'
type: agent
---

# Agents Commander

You are the AgentsCommander Root Agent. You are the top-level coordinator for this AgentsCommander binary.

## Responsibility

Act as the top-level planning and oversight agent for sessions, workgroups, and agents available to this AgentsCommander instance. Help the user inspect available work, plan delegation, track status, and synthesize results.

## State

Your own durable state lives in the canonical `ac-root-agent` directory:

- `memory/`
- `plans/`
- `skills/`
- `Role.md`

You are not a workgroup replica and you do not have an origin Agent Matrix. Use the canonical root directory for your own durable state.

## Coordination

Coordinate across workgroups at a high level. Delegate specialized implementation work to the appropriate team coordinators and synthesize their results for the user.

## Team and workgroup setup

When asked to set up a new team for automation, use this order:

1. Create any missing agents with `create-agent-matrix`.
2. Create the team with `team create`, choosing one coordinator and the worker agents.
3. Activate a task workspace with `workgroup add` using only `--project`, `--team`, and `--title`.

Agents must exist before team creation. Team creation defines membership and repo access; workgroup activation uses the existing team definition.

## Agency Agents Roles

You may offer to download tested role templates from Agency Agents when the user wants a new specialist role. Ask before downloading or updating. If the user accepts, use the AgentsCommander CLI from `AGENTSCOMMANDER_BINARY_PATH`:

```text
"<AGENTSCOMMANDER_BINARY_PATH>" agency-templates update --ref main
"<AGENTSCOMMANDER_BINARY_PATH>" agency-templates status --pretty
"<AGENTSCOMMANDER_BINARY_PATH>" agency-templates list --pretty
```

Use only IDs returned by `agency-templates list` when creating agents with `create-agent-matrix --role-template <id>`. Do not invent Agency template IDs.
"#;

/// Frozen snapshot of the generated root context as it shipped BEFORE #648
/// moved the Agency Agents guidance into `skills/agency-agents-roles`. Kept so
/// an on-disk template generated by that release is recognized in
/// `migrate_root_context_template` / `migrate_root_role` and upgraded to the
/// current `ROOT_ROLE_MD`, instead of being treated as custom and preserved.
/// Do NOT edit: it must stay byte-identical to what that release wrote.
const ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD: &str = r#"---
name: 'agents-commander'
description: 'Static supplemental root context for AgentsCommander.'
type: agent
---

# Agents Commander

You are the AgentsCommander Root Agent. You are the top-level coordinator for this AgentsCommander binary.

## Responsibility

Act as the top-level planning and oversight agent for sessions, workgroups, and agents available to this AgentsCommander instance. Help the user inspect available work, plan delegation, track status, and synthesize results.

## State

Your own durable state lives in the canonical `ac-root-agent` directory:

- `memory/`
- `plans/`
- `skills/`
- `Role.md`

You are not a workgroup replica and you do not have an origin Agent Matrix. Use the canonical root directory for your own durable state.

## Coordination

Coordinate across workgroups at a high level. Delegate specialized implementation work to the appropriate team coordinators and synthesize their results for the user.

## Team and workgroup setup

When asked to set up a new team for automation, use this order:

1. Create any missing agents with `create-agent-matrix`.
2. Create the team with `team create`, choosing one coordinator and the worker agents.
3. Activate a task workspace with `workgroup add` using only `--project`, `--team`, and `--title`.

Agents must exist before team creation. Team creation defines membership and repo access; workgroup activation uses the existing team definition.

## Governance Boundary Audits

Before finalizing any work that creates, modifies, approves, or audits agents, `Role.md` files, skills, role templates, workflow instructions, or Agent Matrix structure, load and apply `skills/role-skill-boundary-audit/SKILL.md`.

Also apply that skill when a role grows unusually large, a role contains repeatable operational procedure, a skill contains authority or ownership language, similar instructions appear in multiple roles, someone proposes another agent for a bounded capability, or periodic matrix hygiene is requested.

The audit is a review lens. It should produce a structured recommendation before any refactor, not silently rewrite roles, skills, or agent boundaries.

## Agency Agents Roles

You may offer to download tested role templates from Agency Agents when the user wants a new specialist role. Ask before downloading or updating. If the user accepts, use the AgentsCommander CLI from `AGENTSCOMMANDER_BINARY_PATH`:

```text
"<AGENTSCOMMANDER_BINARY_PATH>" agency-templates update --ref main
"<AGENTSCOMMANDER_BINARY_PATH>" agency-templates status --pretty
"<AGENTSCOMMANDER_BINARY_PATH>" agency-templates list --pretty
```

Use only IDs returned by `agency-templates list` when creating agents with `create-agent-matrix --role-template <id>`. Do not invent Agency template IDs.
"#;

static ROOT_ROLE_MD: LazyLock<String> = LazyLock::new(|| {
    r#"---
name: 'agents-commander'
description: 'Static supplemental root context for AgentsCommander.'
type: agent
---

# Agents Commander

You are the AgentsCommander Root Agent. You are the top-level coordinator for this AgentsCommander binary.

## Responsibility

Act as the top-level planning and oversight agent for sessions, workgroups, and agents available to this AgentsCommander instance. Help the user inspect available work, plan delegation, track status, and synthesize results.

## State

Your own durable state lives in the canonical `ac-root-agent` directory:

- `memory/`
- `plans/`
- `skills/`
- `Role.md`

You are not a workgroup replica and you do not have an origin Agent Matrix. Use the canonical root directory for your own durable state.

## Coordination

Coordinate across workgroups at a high level. Delegate specialized implementation work to the appropriate team coordinators and synthesize their results for the user.

## Team and workgroup setup

When asked to set up a new team for automation, use this order:

1. Create any missing agents with `create-agent-matrix`.
2. Create the team with `team create`, choosing one coordinator and the worker agents.
3. Activate a task workspace with `workgroup add` using only `--project`, `--team`, and `--title`.

Agents must exist before team creation. Team creation defines membership and repo access; workgroup activation uses the existing team definition.

## Governance Boundary Audits

Before finalizing any work that creates, modifies, approves, or audits agents, `Role.md` files, skills, role templates, workflow instructions, or Agent Matrix structure, load and apply `skills/role-skill-boundary-audit/SKILL.md`.

Also apply that skill when a role grows unusually large, a role contains repeatable operational procedure, a skill contains authority or ownership language, similar instructions appear in multiple roles, someone proposes another agent for a bounded capability, or periodic matrix hygiene is requested.

The audit is a review lens. It should produce a structured recommendation before any refactor, not silently rewrite roles, skills, or agent boundaries.

## Agency Agents Roles

Before creating any new specialist agent (any role-defined `create-agent-matrix`), load and apply `skills/agency-agents-roles/SKILL.md`. It defines the mandatory offer of tested Agency Agents role templates, what to state about Agency Agents from real local data (never invented), the bounded skip exceptions, and the `agency-templates` CLI flow.
"#
    .to_string()
});

pub(crate) fn default_root_context_template() -> &'static str {
    ROOT_ROLE_MD.as_str()
}

pub(crate) fn is_known_generated_root_context_template(content: &str) -> bool {
    let normalized = normalize_role_text(content);
    let old_generated = [
        normalize_role_text(OLD_ROOT_ROLE_MD),
        normalize_role_text(&OLD_ROOT_CONTEXT_WITH_COORDINATION_MD),
        normalize_role_text(ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD),
        normalize_role_text(ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD),
        normalize_role_text(ROOT_ROLE_MD.as_str()),
    ];
    old_generated.contains(&normalized)
}

const MINIMAL_ROOT_ROLE_MD: &str = r#"# Role

You are the personal Root Agent for AgentsCommander.
"#;

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

pub fn is_root_agent_dir_name(cwd: &str) -> bool {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case(ROOT_AGENT_DIR_NAME))
        .unwrap_or(false)
}

pub fn is_root_agent_path(cwd: &str) -> bool {
    let Ok(root_dir) = root_agent_dir() else {
        return false;
    };
    paths_equivalent(Path::new(cwd), Path::new(&root_dir))
}

fn validate_root_agent_root_path(root_dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(root_dir) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                return Err(format!(
                    "Root agent directory {} exists but is not a regular directory",
                    root_dir.display()
                ));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "Failed to inspect root agent directory {}: {}",
            root_dir.display(),
            e
        )),
    }
}

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

pub fn ensure_root_agent_dir() -> Result<String, String> {
    let root_dir = root_agent_dir()?;
    ensure_root_agent_dir_at(Path::new(&root_dir))?;
    Ok(root_dir)
}

pub(crate) fn ensure_root_agent_dir_at(root_dir: &Path) -> Result<(), String> {
    validate_root_agent_root_path(root_dir)?;
    crate::commands::entity_creation::create_agent_matrix_layout(root_dir).map_err(
        |(sub, e)| {
            format!(
                "Failed to create root agent layout entry '{}' at {}: {}",
                sub,
                root_dir.display(),
                e
            )
        },
    )?;
    validate_root_agent_root_path(root_dir)?;
    ensure_default_root_agent_skills_at(root_dir)?;

    let messaging_dir = root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&messaging_dir).map_err(|e| {
        format!(
            "Failed to create root agent messaging directory at {}: {}",
            messaging_dir.display(),
            e
        )
    })?;

    let role_path = root_dir.join("Role.md");
    migrate_root_role(&role_path)?;

    merge_root_agent_config(&root_dir.join("config.json"))
}

pub(crate) fn ensure_default_root_agent_skills_at(root_dir: &Path) -> Result<(), String> {
    validate_root_agent_root_path(root_dir)?;
    let skills_root = root_dir.join(ROOT_AGENT_SKILLS_DIR);
    std::fs::create_dir_all(&skills_root).map_err(|e| {
        format!(
            "Failed to create root agent skills directory at {}: {}",
            skills_root.display(),
            e
        )
    })?;
    validate_root_agent_root_path(root_dir)?;

    validate_root_agent_skills_root(&skills_root)?;

    for skill in DEFAULT_ROOT_SKILLS {
        ensure_default_root_skill_file(root_dir, &skills_root, skill)?;
    }

    Ok(())
}

fn ensure_default_root_skill_file(
    root_dir: &Path,
    skills_root: &Path,
    skill: &DefaultRootSkill,
) -> Result<(), String> {
    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    let skill_dir = skills_root.join(skill.dir_name);
    match std::fs::symlink_metadata(&skill_dir) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                return Err(format!(
                    "Root agent default skill path {} exists but is not a regular directory",
                    skill_dir.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::create_dir(&skill_dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(format!(
                        "Failed to create root agent default skill directory {}: {}",
                        skill_dir.display(),
                        e
                    ));
                }
            }
        }
        Err(e) => {
            return Err(format!(
                "Failed to inspect root agent default skill directory {}: {}",
                skill_dir.display(),
                e
            ));
        }
    }

    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    let metadata = std::fs::symlink_metadata(&skill_dir).map_err(|e| {
        format!(
            "Failed to inspect root agent default skill directory {} after create: {}",
            skill_dir.display(),
            e
        )
    })?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "Root agent default skill path {} exists but is not a regular directory",
            skill_dir.display()
        ));
    }

    let skill_path = skill_dir.join(skill.file_name);
    ensure_default_skill_file(
        root_dir,
        skills_root,
        &skill_path,
        skill.content,
        skill.legacy_snapshots,
    )
}

fn validate_root_agent_skills_root(skills_root: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(skills_root).map_err(|e| {
        format!(
            "Failed to inspect root agent skills directory {}: {}",
            skills_root.display(),
            e
        )
    })?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "Root agent skills path {} exists but is not a regular directory",
            skills_root.display()
        ));
    }
    Ok(())
}

fn validate_default_skill_directory(skill_dir: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(skill_dir).map_err(|e| {
        format!(
            "Failed to inspect root agent default skill directory {}: {}",
            skill_dir.display(),
            e
        )
    })?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "Root agent default skill path {} exists but is not a regular directory",
            skill_dir.display()
        ));
    }
    Ok(())
}

fn migrate_root_role(role_path: &Path) -> Result<(), String> {
    let root_dir = role_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve root agent directory from {}",
            role_path.display()
        )
    })?;
    let config_dir = root_dir.parent().ok_or_else(|| {
        format!(
            "Could not resolve config directory from {}",
            role_path.display()
        )
    })?;
    crate::config::session_context::create_default_context_templates(config_dir)?;
    let context_template_path =
        config_dir.join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);

    migrate_root_context_template(&context_template_path)?;
    match create_missing_role(role_path, MINIMAL_ROOT_ROLE_MD)? {
        CreateMissingRole::Created => return Ok(()),
        CreateMissingRole::AlreadyExists => {}
    }

    let existing = std::fs::read_to_string(role_path)
        .map_err(|e| format!("Failed to read {}: {}", role_path.display(), e))?;
    let existing_normalized = normalize_role_text(&existing);
    let migrated = if existing_normalized == normalize_role_text(OLD_ROOT_ROLE_MD)
        || existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD)
        || existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD)
        || existing_normalized == normalize_role_text(&ROOT_ROLE_MD)
    {
        if existing_normalized != normalize_role_text(MINIMAL_ROOT_ROLE_MD) {
            Some(MINIMAL_ROOT_ROLE_MD.to_string())
        } else {
            None
        }
    } else if existing.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH) {
        Some(existing.replace(
            OLD_DEFERRED_MESSAGING_PARAGRAPH,
            ROOT_COORDINATION_MESSAGING_PARAGRAPH,
        ))
    } else {
        None
    };

    if let Some(content) = migrated {
        atomic_write_role(role_path, &content)?;
    }

    Ok(())
}

fn migrate_root_context_template(context_template_path: &Path) -> Result<(), String> {
    let config_dir = context_template_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve config directory from {}",
            context_template_path.display()
        )
    })?;
    crate::config::seeded_context_templates::ensure_root_context_template(config_dir)
}

fn atomic_write_role(role_path: &Path, content: &str) -> Result<(), String> {
    let parent = role_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for {}",
            role_path.display()
        )
    })?;
    let temp_path = unique_role_temp_path(role_path);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
    {
        Ok(file) => file,
        Err(e) => {
            return Err(format!(
                "Failed to create temporary role file {}: {}",
                temp_path.display(),
                e
            ))
        }
    };

    if let Err(e) = write_role_file(&mut file, role_path, content) {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    if let Err(e) = replace_role_file(&temp_path, role_path) {
        cleanup_temp_role(&temp_path);
        return Err(e);
    }

    if let Ok(dir) = std::fs::File::open(parent) {
        if let Err(e) = dir.sync_all() {
            log::warn!(
                "Failed to sync root agent role directory {}: {}",
                parent.display(),
                e
            );
        }
    }

    Ok(())
}

enum CreateMissingRole {
    Created,
    AlreadyExists,
}

fn create_missing_role(role_path: &Path, content: &str) -> Result<CreateMissingRole, String> {
    let parent = role_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for {}",
            role_path.display()
        )
    })?;
    let temp_path = unique_role_temp_path(role_path);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
    {
        Ok(file) => file,
        Err(e) => {
            return Err(format!(
                "Failed to create temporary role file {}: {}",
                temp_path.display(),
                e
            ))
        }
    };

    if let Err(e) = write_role_file(&mut file, role_path, content) {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    let published = match publish_missing_role_file(&temp_path, role_path) {
        Ok(published) => published,
        Err(e) => {
            cleanup_temp_role(&temp_path);
            return Err(e);
        }
    };

    cleanup_temp_role(&temp_path);

    if published {
        sync_role_dir(parent);
        Ok(CreateMissingRole::Created)
    } else {
        Ok(CreateMissingRole::AlreadyExists)
    }
}

/// Ensures the on-disk default skill file exists and, if it is a pristine copy of a defective
/// version we shipped ourselves, repairs it in place (#909).
///
/// The two arms are deliberately asymmetric and must stay that way. The **NotFound arm** creates the
/// file and publishes with `hard_link`, which is create-only, and it propagates `Err`, because a root
/// agent with no skill file is genuinely broken. The **exists arm** repairs through
/// `atomic_replace_existing`, and its failures are logged and swallowed, because a root agent with a
/// stale skill file merely keeps today's behavior until the next context build retries.
///
/// Do NOT unify the two arms onto one publish primitive: `hard_link` returns `AlreadyExists` against
/// an existing destination, so reusing it for the repair would make the migration a silent no-op on
/// every broken install.
fn ensure_default_skill_file(
    root_dir: &Path,
    skills_root: &Path,
    path: &Path,
    content: &str,
    legacy_snapshots: &[&str],
) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            // An inode-type violation is structural and stays fatal. The `?` is deliberate.
            validate_default_skill_file(path, &metadata)?;

            // A failed repair must never fail a session start, so log and continue.
            //
            // `warn_once_for_path`, not `log::warn!`: a sticky failure we cannot detect in advance
            // (a parent directory denying `FILE_DELETE_CHILD`, say) would otherwise emit a WARN on
            // every root-agent context build, forever, and bury real warnings. The read-only
            // pre-check inside `migrate_default_skill_file` only covers the one instance of that
            // class we could measure.
            //
            // `migrate_default_skill_file` takes no `Metadata` parameter. It stats for itself,
            // immediately before its own open.
            if let Err(e) =
                migrate_default_skill_file(root_dir, skills_root, path, content, legacy_snapshots)
            {
                warn_once_for_path(
                    path,
                    &format!(
                        "Could not repair root agent default skill {}: {}",
                        path.display(),
                        e
                    ),
                );
            }
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect root agent default skill file {}: {}",
                path.display(),
                e
            ));
        }
    }

    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for root agent default skill {}",
            path.display()
        )
    })?;
    validate_default_skill_directory(parent)?;
    let (temp_path, mut file) = create_default_skill_temp_file(path)?;

    let write_result = (|| -> Result<(), String> {
        file.write_all(content.as_bytes())
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush {}: {}", path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync {}: {}", path.display(), e))
    })();
    if let Err(e) = write_result {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    validate_default_skill_directory(parent)?;
    let published = match publish_missing_default_skill_file(&temp_path, path) {
        Ok(published) => published,
        Err(e) => {
            cleanup_temp_role(&temp_path);
            return Err(e);
        }
    };

    cleanup_temp_role(&temp_path);
    if published {
        sync_role_dir(parent);
    }
    Ok(())
}

// -------------------------------------------------------------------------------------------
// #909: repair of pristine stale default skill files.
// -------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum SkipReason {
    /// This default ships no frozen snapshot, so it is never repaired.
    NoSnapshots,
    /// Already `content`, modulo line endings and surrounding whitespace.
    AlreadyCurrent,
    /// Not any snapshot. Someone else's bytes. Never touched.
    UserEdit,
    /// Larger than anything that could normalize-equal a snapshot.
    TooLarge,
    /// Not valid UTF-8. Snapshots are `&'static str`, so this can never be repairable. Do not fold
    /// it into `UserEdit`: a merged variant is a lying enum.
    NotUtf8,
    /// Windows only: `FILE_ATTRIBUTE_READONLY` blocks the delete inside `ReplaceFileW`. On Unix,
    /// `rename(2)` checks the directory rather than the file mode, so this is never produced.
    #[cfg_attr(not(windows), allow(dead_code))]
    ReadOnly,
}

#[derive(Debug, PartialEq, Eq)]
enum SkillMigration {
    Skipped(SkipReason),
    Repaired,
}

enum Publish {
    Published,
    AlreadyCurrent,
}

/// Outcome of a bounded read. `Err` is reserved for genuine I/O failures, which are transient and
/// deserve a warning. `TooLarge` and `NotUtf8` are classifications, not errors: neither can ever be
/// repairable, so warning about them on every context build would be noise forever.
enum BoundedRead {
    Text(String),
    TooLarge,
    NotUtf8,
}

/// A conservative cap, NOT a provable bound: `trim` removes an unbounded number of bytes, so the
/// pre-image of a match is unbounded. Anything larger is declared a user edit.
///
/// The one consequence, stated plainly because the plan originally hid it: this is the only
/// mechanism in the design that can permanently strand a repairable pristine default. A
/// whitespace-padded copy above the cap matches semantically, is skipped as too large, and stays
/// broken forever. Nothing on disk is in that state.
///
/// Build-machine dependent, because `content.len()` is `include_str!`'s length, and `.gitattributes`
/// carries no `*.md` rule (#914). No fixture may hard-code the result.
fn max_migratable_len(content: &str, legacy_snapshots: &[&str]) -> u64 {
    let longest = legacy_snapshots
        .iter()
        .map(|snapshot| snapshot.len())
        .chain(std::iter::once(content.len()))
        .max()
        .unwrap_or(0);
    (longest as u64) * 2 + MAX_DEFAULT_SKILL_TRIM_SLACK_BYTES
}

/// Reads at most `bound + 1` bytes, relying on no prior `stat`.
///
/// The `+ 1` is what makes a truncated prefix unreachable. `Take::read_to_end` stops only at the
/// take limit or at EOF. If it stopped at the limit, `buf.len() == bound + 1 > bound` and we
/// classify `TooLarge`. So `buf.len() <= bound` implies EOF, which implies `buf` is the whole file,
/// and decoding therefore never sees a split multi-byte character.
///
/// `take(bound)` without the `+ 1` and the rejection is a DATA-LOSS BUG: `SNAPSHOT` + N spaces +
/// `MY NOTES` would read back as a prefix that trims to exactly the snapshot, match, and overwrite
/// the user's notes. Do not write it.
fn read_bounded(path: &Path, bound: u64) -> Result<BoundedRead, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
    let mut buf = Vec::new();
    file.take(bound.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    if buf.len() as u64 > bound {
        return Ok(BoundedRead::TooLarge);
    }
    match String::from_utf8(buf) {
        Ok(text) => Ok(BoundedRead::Text(text)),
        Err(_) => Ok(BoundedRead::NotUtf8),
    }
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
/// macro never short-circuits. That clamp exists to keep **third-party** warnings visible, and those
/// take the `self.inner.enabled(..)` branch. Ours do not: at runtime level Error the gate rejects
/// them. A user who has selected Error learns nothing from a failed repair, not even from
/// `discover_skill_index`'s own per-build warning, which is a `warn!` on the same target and is
/// suppressed identically.
///
/// **This set is append-only, and two tests depend on that.** `warn_once_for_path` is its only
/// writer; there is no `remove`, `clear`, `drain`, or `retain` anywhere. That is what lets
/// `migration_skips_non_utf8_file` and the healthy branch of `migration_skips_read_only_destination`
/// assert `!warned_for_path(..)` *after* the act without also asserting it before: a passing
/// post-condition implies the pre-condition only while the set never shrinks. Adding a `#[cfg(test)]`
/// reset helper is a tempting and otherwise reasonable thing to want; it would silently weaken both
/// assertions, and both would then need an explicit pre-assertion, as the positive sites already have.
static SKILL_REPAIR_WARNED: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn warn_once_for_path(path: &Path, message: &str) {
    let first = {
        let mut seen = SKILL_REPAIR_WARNED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        seen.insert(path.to_path_buf())
    };
    if first {
        log::warn!("[skills] {} (logged once per process)", message);
    } else {
        log::debug!("[skills] {}", message);
    }
}

fn short_sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    digest[..16].to_string()
}

/// #909: an on-disk default skill file that is a byte-exact copy (modulo line endings and
/// surrounding whitespace) of a previously shipped, defective version is repaired in place. Anything
/// else is treated as a user edit and left alone.
///
/// Classification and write live in this one function on purpose. Hoisting the branches into a
/// `classify()` that the caller acts on would turn `SkillMigration` into decoration and send the
/// tests back to proving nothing.
///
/// Best-effort by contract: every failure returns `Err` for the CALLER to log and swallow. A failed
/// repair must never fail a root-agent context build.
fn migrate_default_skill_file(
    root_dir: &Path,
    skills_root: &Path,
    path: &Path,
    content: &str,
    legacy_snapshots: &[&str],
) -> Result<SkillMigration, String> {
    if legacy_snapshots.is_empty() {
        return Ok(SkillMigration::Skipped(SkipReason::NoSnapshots));
    }

    // A fresh stat, taken as late as possible and immediately before the open. This narrows, and
    // does not close, the window in which a symlink or a FIFO can be planted at `path`. It also
    // supplies the read-only bit consumed below, which is why this function takes no `Metadata`
    // parameter: that parameter was proposed for the size guard (wrong window) and then for the
    // read-only check (wrong position), and neither use survived.
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to inspect {}: {}", path.display(), e))?;
    validate_default_skill_file(path, &metadata)?;

    let bound = max_migratable_len(content, legacy_snapshots);
    let existing = match read_bounded(path, bound)? {
        BoundedRead::TooLarge => return Ok(SkillMigration::Skipped(SkipReason::TooLarge)),
        BoundedRead::NotUtf8 => return Ok(SkillMigration::Skipped(SkipReason::NotUtf8)),
        BoundedRead::Text(text) => text,
    };

    let existing_normalized = normalize_role_text(&existing);
    let content_normalized = normalize_role_text(content);

    if existing_normalized == content_normalized {
        return Ok(SkillMigration::Skipped(SkipReason::AlreadyCurrent));
    }
    if !legacy_snapshots
        .iter()
        .any(|snapshot| normalize_role_text(snapshot) == existing_normalized)
    {
        return Ok(SkillMigration::Skipped(SkipReason::UserEdit));
    }

    // The file is stale and repairable. ONLY NOW does read-only mean anything: above this line it
    // would warn about healthy files, and the enum would stop distinguishing `ReadOnly` from
    // `AlreadyCurrent` and `UserEdit`, which is the exact defect `SkipReason` exists to kill.
    //
    // Measured, not reasoned: `ReplaceFileW` and `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` both
    // return os error 5 against a read-only destination under every flag combination, because
    // `ReplaceFileW` must delete the replaced file and `FILE_ATTRIBUTE_READONLY` blocks
    // `DeleteFileW`. It is an attribute, not a security descriptor, so no ACL grant and no
    // elevation bypasses it. A deny-write DACL does NOT block the repair (the delete is authorised
    // by the parent's `FILE_DELETE_CHILD`), so the attribute check covers the sticky case exactly.
    //
    // Advisory, not a correctness gate: a file that turns read-only after this stat still fails at
    // `ReplaceFileW`, is swallowed, and is warned once by `warn_once_for_path`.
    //
    // On Unix, `rename(2)` checks the DIRECTORY, not the file mode, so a read-only file repairs
    // fine and must not be rejected here.
    #[cfg(windows)]
    if metadata.permissions().readonly() {
        warn_once_for_path(
            path,
            &format!(
                "Root agent default skill {} is read-only; leaving it unrepaired",
                path.display()
            ),
        );
        return Ok(SkillMigration::Skipped(SkipReason::ReadOnly));
    }

    match atomic_write_default_skill(
        root_dir,
        skills_root,
        path,
        content,
        bound,
        &existing_normalized,
        &content_normalized,
    )? {
        Publish::AlreadyCurrent => Ok(SkillMigration::Skipped(SkipReason::AlreadyCurrent)),
        Publish::Published => {
            // This line requires the runtime log level at Info or lower. The default is Info
            // (`init_logger_inner`: `read_log_level_only().unwrap_or("info")`), so it normally
            // appears. But if the user has lowered verbosity to Warn or Error, the `info!` macro
            // short-circuits before the gate ever sees it, because `max_filter_for`
            // (`logging.rs:66`) leaves the global max level at Warn. The repair still happened; only
            // the record of it is gone. Do not read an absent line as a failed repair: check the
            // file, not the log.
            log::info!(
                "[skills] #909 repaired stale default skill {}: {} -> {}",
                path.display(),
                short_sha256(&existing_normalized),
                short_sha256(&content_normalized),
            );
            Ok(SkillMigration::Repaired)
        }
    }
}

/// Writes `content` VERBATIM (never the normalized form: four pre-existing tests assert
/// `read_to_string(seeded) == content` byte for byte) through a temp file, and publishes with
/// `atomic_replace_existing`, the #664 overwrite primitive.
fn atomic_write_default_skill(
    root_dir: &Path,
    skills_root: &Path,
    path: &Path,
    content: &str,
    bound: u64,
    expected_normalized: &str,
    content_normalized: &str,
) -> Result<Publish, String> {
    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for root agent default skill {}",
            path.display()
        )
    })?;
    validate_default_skill_directory(parent)?;

    let (temp_path, mut file) = create_default_skill_temp_file(path)?;

    let write_result = (|| -> Result<(), String> {
        file.write_all(content.as_bytes())
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush {}: {}", path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync {}: {}", path.display(), e))
    })();
    if let Err(e) = write_result {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    // Every `?` below would otherwise abandon the temp file, hence the closure and the
    // unconditional `cleanup_temp_role` after it.
    let published = (|| -> Result<Publish, String> {
        validate_root_agent_root_path(root_dir)?;
        validate_root_agent_skills_root(skills_root)?;
        validate_default_skill_directory(parent)?;

        // Simulates a writer that mutates the destination after our temp is durable and before we
        // publish. This is the window the re-read below exists to observe, so the hook MUST run
        // before that re-read: placed after it, the three tests it drives prove nothing and one of
        // them silently destroys a user edit inside its own scenario.
        #[cfg(test)]
        run_pre_publish_hook(path)?;

        // Re-verify the destination after the fsync, immediately before publishing. The plan's
        // central promise is "we never touch a user edit", and without this it is made before the
        // fsync and cashed after it. The window shrinks to `stat` + `open` + `ReplaceFileW`. It
        // cannot be closed: Windows has no compare-and-swap-file primitive.
        //
        // Re-validate the inode type too. The NotFound arm has this defence at
        // `publish_missing_default_skill_file_with`; the repair path validated the directory three
        // times and the file never. It cannot close the race, but it rejects a reparse point that
        // has PERSISTED since the stat above, which is the only variant an ordinary user produces
        // by accident.
        let fresh_metadata = std::fs::symlink_metadata(path)
            .map_err(|e| format!("Failed to re-inspect {}: {}", path.display(), e))?;
        validate_default_skill_file(path, &fresh_metadata)?;

        match read_bounded(path, bound)? {
            // Deliberately NOT symmetric with the first read, which classifies these silently. A
            // file that grows past the bound or turns non-UTF-8 mid-repair is an anomaly worth
            // exactly one warning, and it is self-limiting: the next context build classifies it
            // silently. Do not "harmonise" these.
            BoundedRead::TooLarge | BoundedRead::NotUtf8 => {
                return Err(format!(
                    "{} changed unreadably while it was being repaired; left alone",
                    path.display()
                ))
            }
            BoundedRead::Text(fresh) => {
                let fresh_normalized = normalize_role_text(&fresh);
                if fresh_normalized == content_normalized {
                    // Another writer got there first. Publishing would be a redundant
                    // `ReplaceFileW` over identical bytes.
                    return Ok(Publish::AlreadyCurrent);
                }
                if fresh_normalized != expected_normalized {
                    return Err(format!(
                        "{} changed while it was being repaired; left alone",
                        path.display()
                    ));
                }
            }
        }

        atomic_replace_existing(&temp_path, path)?;
        Ok(Publish::Published)
    })();

    cleanup_temp_role(&temp_path);
    let published = published?;
    if matches!(published, Publish::Published) {
        // NOTE: a silent no-op on Windows, where `File::open` on a directory fails. Do not claim
        // directory-entry durability here. `REPLACEFILE_WRITE_THROUGH` already flushed the file's
        // own data, which is what we need.
        sync_role_dir(parent);
    }
    Ok(published)
}

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
/// `Send` on `PrePublishHook` is mandatory: a `static` must be `Sync`, and `Mutex<T>: Sync` requires
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

fn write_role_file(
    file: &mut std::fs::File,
    role_path: &Path,
    content: &str,
) -> Result<(), String> {
    #[cfg(test)]
    if content.contains(FAIL_ROOT_ROLE_WRITE_MARKER)
        && FAIL_ROOT_ROLE_WRITE_ONCE.swap(false, Ordering::SeqCst)
    {
        return Err(format!(
            "Failed to write {}: injected failure",
            role_path.display()
        ));
    }

    file.write_all(content.as_bytes())
        .map_err(|e| format!("Failed to write {}: {}", role_path.display(), e))?;
    file.flush()
        .map_err(|e| format!("Failed to flush {}: {}", role_path.display(), e))?;
    file.sync_all()
        .map_err(|e| format!("Failed to sync {}: {}", role_path.display(), e))
}

fn unique_role_temp_path(role_path: &Path) -> std::path::PathBuf {
    let parent = role_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = role_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Role.md");
    let counter = ROOT_ROLE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ))
}

fn create_default_skill_temp_file(path: &Path) -> Result<(PathBuf, std::fs::File), String> {
    create_default_skill_temp_file_with(path, unique_default_skill_temp_path)
}

fn create_default_skill_temp_file_with<F>(
    path: &Path,
    mut next_temp_path: F,
) -> Result<(PathBuf, std::fs::File), String>
where
    F: FnMut(&Path) -> PathBuf,
{
    const TEMP_CREATE_ATTEMPTS: usize = 16;

    for _ in 0..TEMP_CREATE_ATTEMPTS {
        let temp_path = next_temp_path(path);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to create temporary root agent skill file {}: {}",
                    temp_path.display(),
                    e
                ));
            }
        }
    }

    Err(format!(
        "Failed to create temporary root agent skill file for {} after {} attempts",
        path.display(),
        TEMP_CREATE_ATTEMPTS
    ))
}

fn unique_default_skill_temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(SKILL_MD_FILENAME);
    let counter = ROOT_ROLE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ))
}

fn cleanup_temp_role(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "Failed to remove temporary role file {}: {}",
                path.display(),
                e
            );
        }
    }
}

fn sync_role_dir(parent: &Path) {
    if let Ok(dir) = std::fs::File::open(parent) {
        if let Err(e) = dir.sync_all() {
            log::warn!(
                "Failed to sync root agent role directory {}: {}",
                parent.display(),
                e
            );
        }
    }
}

fn publish_missing_role_file(temp_path: &Path, role_path: &Path) -> Result<bool, String> {
    match std::fs::hard_link(temp_path, role_path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(format!(
            "Failed to publish missing role file {} from {}: {}",
            role_path.display(),
            temp_path.display(),
            e
        )),
    }
}

fn validate_default_skill_file(path: &Path, metadata: &std::fs::Metadata) -> Result<(), String> {
    if is_link_or_reparse(metadata) || !metadata.is_file() {
        return Err(format!(
            "Root agent default skill file {} exists but is not a regular file",
            path.display()
        ));
    }
    Ok(())
}

fn publish_missing_default_skill_file(temp_path: &Path, path: &Path) -> Result<bool, String> {
    publish_missing_default_skill_file_with(temp_path, path, |temp_path, path| {
        std::fs::hard_link(temp_path, path)
    })
}

fn publish_missing_default_skill_file_with<F>(
    temp_path: &Path,
    path: &Path,
    publish: F,
) -> Result<bool, String>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    match publish(temp_path, path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path).map_err(|inspect_err| {
                format!(
                    "Root agent default skill file {} appeared during publish but could not be inspected: {}",
                    path.display(),
                    inspect_err
                )
            })?;
            validate_default_skill_file(path, &metadata)?;
            Ok(false)
        }
        Err(e) => Err(format!(
            "Failed to publish root agent default skill file {} from {}: {}",
            path.display(),
            temp_path.display(),
            e
        )),
    }
}

#[cfg(not(windows))]
fn replace_role_file(temp_path: &Path, role_path: &Path) -> Result<(), String> {
    std::fs::rename(temp_path, role_path).map_err(|e| {
        format!(
            "Failed to replace {} with {}: {}",
            role_path.display(),
            temp_path.display(),
            e
        )
    })
}

#[cfg(windows)]
fn replace_role_file(temp_path: &Path, role_path: &Path) -> Result<(), String> {
    if !role_path.exists() {
        return std::fs::rename(temp_path, role_path).map_err(|e| {
            format!(
                "Failed to publish {} from {}: {}",
                role_path.display(),
                temp_path.display(),
                e
            )
        });
    }

    replace_existing_file_windows(temp_path, role_path)
}

#[cfg(windows)]
fn replace_existing_file_windows(temp_path: &Path, role_path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let role_wide: Vec<u16> = role_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let temp_wide: Vec<u16> = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        ReplaceFileW(
            role_wide.as_ptr(),
            temp_wide.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "Failed to replace {} with {}: {}",
            role_path.display(),
            temp_path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// #664: shared atomic replace-existing primitive. Delegates to the vetted
/// role-file replace path (plain rename on Unix; rename-if-absent else
/// `ReplaceFileW(REPLACEFILE_WRITE_THROUGH)` on Windows). The CALLER is
/// responsible for creating, writing, fsyncing, dropping the temp handle, and
/// cleaning up `temp`; this only publishes `temp` -> `dest`.
pub(crate) fn atomic_replace_existing(temp: &Path, dest: &Path) -> Result<(), String> {
    replace_role_file(temp, dest)
}

fn normalize_role_text(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

pub(crate) fn merge_root_agent_config(config_path: &Path) -> Result<(), String> {
    crate::config::local_config_io::update_config_json_object(config_path, true, |obj| {
        obj.entry("tooling".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));

        let context = obj.get("context").and_then(|v| v.as_array());
        let context_is_old_default =
            context.is_some_and(|arr| context_array_matches(arr, ROOT_AGENT_OLD_DEFAULT_CONTEXT));
        if context.is_none_or(|arr| arr.is_empty()) || context_is_old_default {
            obj.insert(
                "context".to_string(),
                serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT),
            );
        }
        Ok(())
    })?;
    Ok(())
}

fn context_array_matches(arr: &[Value], expected: &[&str]) -> bool {
    arr.len() == expected.len()
        && arr
            .iter()
            .zip(expected)
            .all(|(value, expected)| value.as_str() == Some(*expected))
}

pub fn read_last_coding_agent(root_dir: &str) -> Option<String> {
    let config_path = Path::new(root_dir).join("config.json");
    let contents = std::fs::read_to_string(config_path).ok()?;
    let value: Value = serde_json::from_str(&contents).ok()?;
    value
        .get("tooling")
        .and_then(|tooling| tooling.get("lastCodingAgent"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => normalize_for_compare(&left) == normalize_for_compare(&right),
        _ => normalize_for_compare(left) == normalize_for_compare(right),
    }
}

fn normalize_for_compare(path: &Path) -> String {
    let mut s = display_path(path).replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

fn display_path(path: &Path) -> String {
    crate::path_utils::path_to_string_without_windows_verbatim_prefix(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn display_path_converts_verbatim_unc() {
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\ac-root-agent")),
            r"\\server\share\ac-root-agent"
        );
    }

    #[cfg(unix)]
    fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[cfg(unix)]
    fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[test]
    fn ensure_root_agent_dir_at_creates_layout_role_and_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);

        ensure_root_agent_dir_at(&root).expect("ensure root");

        for sub in ["memory", "plans", "skills", "inbox", "outbox", "messaging"] {
            assert!(root.join(sub).is_dir(), "missing {}", sub);
        }
        assert!(root.join("Role.md").is_file());
        assert!(ROOT_ROLE_MD.contains("You are the AgentsCommander Root Agent"));
        assert!(!ROOT_ROLE_MD.contains("verified workgroup coordinator replicas only"));
        assert!(!ROOT_ROLE_MD.contains("list-peers-lean"));
        assert!(!ROOT_ROLE_MD.contains("AGENTSCOMMANDER_TOKEN"));
        // #648: the Agency guidance moved into skills/agency-agents-roles; the
        // role now carries only a trigger pointer, not the inline CLI block.
        assert!(!ROOT_ROLE_MD.contains("agency-templates update"));
        assert!(!ROOT_ROLE_MD.contains("Do not invent Agency template IDs"));
        assert!(ROOT_ROLE_MD.contains("skills/agency-agents-roles/SKILL.md"));
        assert!(ROOT_ROLE_MD.contains("create-agent-matrix"));
        assert!(ROOT_ROLE_MD.contains("team create"));
        assert!(ROOT_ROLE_MD.contains("workgroup add"));
        assert!(ROOT_ROLE_MD.contains("Agents must exist before team creation"));
        assert!(!ROOT_ROLE_MD.contains("workgroup add --coordinator"));
        assert!(ROOT_ROLE_MD.contains("role-skill-boundary-audit"));
        assert!(ROOT_ROLE_MD.contains("`Role.md` files"));
        assert!(ROOT_ROLE_MD.contains("skills"));
        assert!(ROOT_ROLE_MD.contains("Agent Matrix structure"));
        let skill_path = root
            .join(ROOT_AGENT_SKILLS_DIR)
            .join("role-skill-boundary-audit")
            .join(SKILL_MD_FILENAME);
        assert!(skill_path.is_file());
        assert_eq!(
            std::fs::read_to_string(&skill_path).expect("read default skill"),
            DEFAULT_ROOT_SKILLS[0].content
        );
        let agency_skill_path = root
            .join(ROOT_AGENT_SKILLS_DIR)
            .join("agency-agents-roles")
            .join(SKILL_MD_FILENAME);
        assert!(agency_skill_path.is_file());
        assert_eq!(
            std::fs::read_to_string(&agency_skill_path).expect("read agency skill"),
            DEFAULT_ROOT_SKILLS[1].content
        );
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let global_template_path = temp
            .path()
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        let coordinator_template_path = temp
            .path()
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
        assert!(template_path.is_file());
        assert!(global_template_path.is_file());
        assert!(coordinator_template_path.is_file());
        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            MINIMAL_ROOT_ROLE_MD
        );
        assert_eq!(
            std::fs::read_to_string(template_path).expect("read template"),
            ROOT_ROLE_MD.as_str()
        );
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("config.json")).expect("read config"),
        )
        .expect("parse config");
        assert_eq!(config["tooling"], serde_json::json!({}));
        assert_eq!(
            config["context"],
            serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_preserves_existing_custom_template_and_seeds_minimal_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let global_template_path = temp
            .path()
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        let coordinator_template_path = temp
            .path()
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
        let custom_template = "# Custom Root Template\n\nUse this exact seed.\n";
        let custom_global = "# Custom Global Template\n\nKeep global.\n";
        let custom_coordinator = "# Custom Coordinator Template\n\nKeep coordinator.\n";
        std::fs::write(&template_path, custom_template).expect("write template");
        std::fs::write(&global_template_path, custom_global).expect("write global template");
        std::fs::write(&coordinator_template_path, custom_coordinator)
            .expect("write coordinator template");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            MINIMAL_ROOT_ROLE_MD
        );
        assert_eq!(
            std::fs::read_to_string(template_path).expect("read template"),
            custom_template
        );
        assert_eq!(
            std::fs::read_to_string(global_template_path).expect("read global template"),
            custom_global
        );
        assert_eq!(
            std::fs::read_to_string(coordinator_template_path).expect("read coordinator template"),
            custom_coordinator
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_migrates_old_root_template_defaults() {
        for old_default in [
            OLD_ROOT_ROLE_MD.to_string(),
            OLD_ROOT_CONTEXT_WITH_COORDINATION_MD.to_string(),
            ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD.to_string(),
            ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD.to_string(),
        ] {
            let temp = tempfile::tempdir().expect("tempdir");
            let root = temp.path().join(ROOT_AGENT_DIR_NAME);
            let template_path = temp
                .path()
                .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
            std::fs::write(&template_path, old_default).expect("write old template");

            ensure_root_agent_dir_at(&root).expect("ensure root");

            assert_eq!(
                std::fs::read_to_string(template_path).expect("read template"),
                ROOT_ROLE_MD.as_str()
            );
        }
    }

    #[test]
    fn missing_role_seed_uses_minimal_role_without_copying_custom_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let custom_template = format!(
            "# Custom Root Template\n\n{FAIL_ROOT_ROLE_WRITE_MARKER}\n\nComplete seed body.\n"
        );
        std::fs::write(&template_path, &custom_template).expect("write template");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            MINIMAL_ROOT_ROLE_MD
        );
        assert_eq!(
            std::fs::read_to_string(template_path).expect("read template"),
            custom_template
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_migrates_pre_boundary_audit_generated_root_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(&template_path, ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD)
            .expect("write old generated template");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        let migrated = std::fs::read_to_string(template_path).expect("read template");
        assert_eq!(migrated, ROOT_ROLE_MD.as_str());
        assert!(migrated.contains("role-skill-boundary-audit"));
    }

    #[test]
    fn ensure_root_agent_dir_at_preserves_custom_root_template_with_boundary_audit_text() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let custom = "# Custom Root Template\n\nrole-skill-boundary-audit stays custom.\n";
        std::fs::write(&template_path, custom).expect("write custom template");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(template_path).expect("read template"),
            custom
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_is_idempotent_and_preserves_custom_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(&template_path, "# Custom Template\n\nTemplate body.\n")
            .expect("write template");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("Role.md"), "custom role").expect("write role");

        ensure_root_agent_dir_at(&root).expect("first ensure");
        ensure_root_agent_dir_at(&root).expect("second ensure");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            "custom role"
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_preserves_existing_boundary_audit_skill() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let skill = root
            .join(ROOT_AGENT_SKILLS_DIR)
            .join("role-skill-boundary-audit")
            .join(SKILL_MD_FILENAME);
        std::fs::create_dir_all(skill.parent().expect("skill parent")).expect("create skill dir");
        std::fs::write(&skill, "custom boundary skill").expect("write custom skill");

        ensure_root_agent_dir_at(&root).expect("ensure root");
        ensure_root_agent_dir_at(&root).expect("ensure root again");

        assert_eq!(
            std::fs::read_to_string(&skill).expect("read skill"),
            "custom boundary skill"
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_recreates_missing_boundary_audit_skill() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        ensure_root_agent_dir_at(&root).expect("ensure root");

        let skill = root
            .join(ROOT_AGENT_SKILLS_DIR)
            .join("role-skill-boundary-audit")
            .join(SKILL_MD_FILENAME);
        std::fs::remove_file(&skill).expect("remove skill");

        ensure_root_agent_dir_at(&root).expect("ensure root again");

        assert_eq!(
            std::fs::read_to_string(&skill).expect("read skill"),
            DEFAULT_ROOT_SKILLS[0].content
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_recreates_missing_agency_agents_roles_skill() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        ensure_root_agent_dir_at(&root).expect("ensure root");

        let skill = root
            .join(ROOT_AGENT_SKILLS_DIR)
            .join("agency-agents-roles")
            .join(SKILL_MD_FILENAME);
        std::fs::remove_file(&skill).expect("remove skill");

        ensure_root_agent_dir_at(&root).expect("ensure root again");

        assert_eq!(
            std::fs::read_to_string(&skill).expect("read skill"),
            DEFAULT_ROOT_SKILLS[1].content
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_migrates_pre_agency_skill_generated_root_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(&template_path, ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD)
            .expect("write old generated template");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        let migrated = std::fs::read_to_string(template_path).expect("read template");
        assert_eq!(migrated, ROOT_ROLE_MD.as_str());
        assert!(migrated.contains("skills/agency-agents-roles/SKILL.md"));
        assert!(!migrated.contains("agency-templates update"));
    }

    #[test]
    fn ensure_root_agent_dir_at_reduces_pre_boundary_audit_generated_role_to_minimal_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("Role.md"), ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD)
            .expect("write generated role");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            MINIMAL_ROOT_ROLE_MD
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_rejects_default_skill_dir_as_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let skill_path = root
            .join(ROOT_AGENT_SKILLS_DIR)
            .join("role-skill-boundary-audit");
        std::fs::create_dir_all(skill_path.parent().expect("skill parent"))
            .expect("create skills root");
        std::fs::write(&skill_path, "not a directory").expect("write skill dir file");

        let err = ensure_root_agent_dir_at(&root).expect_err("skill dir file must fail");

        assert!(err.contains("not a regular directory"), "{err}");
        assert!(!skill_path.join(SKILL_MD_FILENAME).exists());
    }

    #[test]
    fn ensure_root_agent_dir_at_rejects_default_skill_entrypoint_as_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let skill_file = root
            .join(ROOT_AGENT_SKILLS_DIR)
            .join("role-skill-boundary-audit")
            .join(SKILL_MD_FILENAME);
        std::fs::create_dir_all(&skill_file).expect("create directory entrypoint");

        let err = ensure_root_agent_dir_at(&root).expect_err("skill file dir must fail");

        assert!(err.contains("not a regular file"), "{err}");
        assert!(skill_file.is_dir());
    }

    #[test]
    fn ensure_root_agent_dir_at_rejects_root_symlink_where_supported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target-root");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&target).expect("create target root");
        if try_symlink_dir(&target, &root).is_err() {
            return;
        }

        let err = ensure_root_agent_dir_at(&root).expect_err("root symlink must fail");

        assert!(err.contains("not a regular directory"), "{err}");
        assert!(!target.join("Role.md").exists());
    }

    #[test]
    fn ensure_root_agent_dir_at_rejects_default_skill_file_symlink_where_supported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let skill_file = root
            .join(ROOT_AGENT_SKILLS_DIR)
            .join("role-skill-boundary-audit")
            .join(SKILL_MD_FILENAME);
        let target = temp.path().join("target-skill.md");
        std::fs::create_dir_all(skill_file.parent().expect("skill parent"))
            .expect("create skill dir");
        std::fs::write(&target, "target skill").expect("write target");
        if try_symlink_file(&target, &skill_file).is_err() {
            return;
        }

        let err = ensure_root_agent_dir_at(&root).expect_err("skill symlink must fail");

        assert!(err.contains("not a regular file"), "{err}");
    }

    #[test]
    fn publish_missing_default_skill_file_revalidates_raced_existing_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(SKILL_MD_FILENAME);
        let temp_path = temp.path().join(".tmp-skill");
        std::fs::write(&temp_path, "default skill").expect("write temp");
        std::fs::create_dir(&path).expect("create invalid raced target");

        let err = publish_missing_default_skill_file_with(&temp_path, &path, |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "raced target",
            ))
        })
        .expect_err("invalid raced target must fail");

        assert!(err.contains("not a regular file"), "{err}");
    }

    #[test]
    fn create_default_skill_temp_file_retries_stale_temp_collision() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(SKILL_MD_FILENAME);
        let first = temp.path().join(".first.tmp");
        let second = temp.path().join(".second.tmp");
        std::fs::write(&first, "stale").expect("write stale temp");
        let mut calls = 0;

        let (created_path, file) = create_default_skill_temp_file_with(&path, |_| {
            calls += 1;
            if calls == 1 {
                first.clone()
            } else {
                second.clone()
            }
        })
        .expect("create temp after collision");
        drop(file);

        assert_eq!(created_path, second);
        assert_eq!(calls, 2);
        assert!(first.exists());
        assert!(second.exists());
    }

    #[test]
    fn ensure_root_agent_dir_at_reduces_current_builtin_role_to_minimal_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let custom_template = "# Custom Root Template\n\nReplace built-in text.\n";
        std::fs::write(&template_path, custom_template).expect("write template");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("Role.md"), ROOT_ROLE_MD.as_str()).expect("write current role");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            MINIMAL_ROOT_ROLE_MD
        );
        assert_eq!(
            std::fs::read_to_string(template_path).expect("read template"),
            custom_template
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_migrates_old_builtin_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("Role.md"), OLD_ROOT_ROLE_MD).expect("write old role");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        let migrated = std::fs::read_to_string(root.join("Role.md")).expect("read role");
        assert_eq!(
            normalize_role_text(&migrated),
            normalize_role_text(MINIMAL_ROOT_ROLE_MD)
        );
        assert!(!migrated.contains("verified workgroup coordinator replicas only"));
        assert!(!migrated.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH));
    }

    #[test]
    fn ensure_root_agent_dir_at_reduces_old_builtin_role_to_minimal_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let custom_template = "# Custom Root Template\n\nMigrate old default here.\n";
        std::fs::write(&template_path, custom_template).expect("write template");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("Role.md"), OLD_ROOT_ROLE_MD).expect("write old role");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            MINIMAL_ROOT_ROLE_MD
        );
        assert_eq!(
            std::fs::read_to_string(template_path).expect("read template"),
            custom_template
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_replaces_old_deferred_paragraph_in_custom_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root).expect("create root");
        let custom = format!(
            "# Custom Root\n\n{}\n\nKeep this custom tail.",
            OLD_DEFERRED_MESSAGING_PARAGRAPH
        );
        std::fs::write(root.join("Role.md"), custom).expect("write custom role");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        let migrated = std::fs::read_to_string(root.join("Role.md")).expect("read role");
        assert!(migrated.starts_with("# Custom Root"));
        assert!(migrated.contains("Keep this custom tail."));
        assert!(migrated.contains(ROOT_COORDINATION_MESSAGING_PARAGRAPH));
        assert!(!migrated.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH));
    }

    #[test]
    fn ensure_root_agent_dir_at_errors_when_root_template_is_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        std::fs::create_dir_all(&template_path).expect("create template directory");

        let err = ensure_root_agent_dir_at(&root).expect_err("directory template must fail");

        assert!(err.contains("not a regular file"), "{err}");
        assert!(!root.join("Role.md").exists());
    }

    #[test]
    fn ensure_root_agent_dir_at_errors_when_root_template_is_symlink() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let target = temp.path().join("target.md");
        std::fs::write(&target, "linked template").expect("write target");
        let Ok(()) = try_symlink_file(&target, &template_path) else {
            return;
        };

        let err = ensure_root_agent_dir_at(&root).expect_err("symlink template must fail");

        assert!(err.contains("not a regular file"), "{err}");
        assert!(!root.join("Role.md").exists());
    }

    #[test]
    fn create_default_context_templates_does_not_create_root_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_dir = temp.path().join(".ac");

        crate::config::session_context::create_default_context_templates(&workspace_dir)
            .expect("create default templates");

        assert!(workspace_dir
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(workspace_dir
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(!workspace_dir
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)
            .exists());
    }

    #[test]
    fn root_sender_uses_reserved_non_path_namespace() {
        assert_eq!(ROOT_AGENT_SENDER, "agentscommander://root-agent");
        assert_ne!(
            ROOT_AGENT_SENDER,
            crate::config::teams::agent_fqn_from_path("C:/tmp/agentscommander/_agent_root-agent")
        );
    }

    #[test]
    fn is_root_agent_target_recognizes_canonical_uri() {
        assert!(is_root_agent_target(ROOT_AGENT_SENDER));
        assert!(is_root_agent_target("agentscommander://root-agent"));
    }

    #[test]
    fn is_root_agent_target_rejects_partial_or_wrong_uris() {
        assert!(!is_root_agent_target(""));
        assert!(!is_root_agent_target("agentscommander://root"));
        assert!(!is_root_agent_target("root-agent"));
        assert!(!is_root_agent_target("agentscommander/root-agent"));
        assert!(!is_root_agent_target("agentscommander://ROOT-AGENT"));
    }

    #[test]
    fn merge_root_agent_config_preserves_tooling_and_unknown_fields() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{
  "tooling": {
    "lastCodingAgent": "codex",
    "codingAgents": {"codex": {"app": "Codex"}},
    "telegramBot": "ops"
  },
  "unknown": {"keep": true},
  "context": []
}"#,
        )
        .expect("write config");

        merge_root_agent_config(&config_path).expect("merge config");

        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read config"))
                .expect("parse config");
        assert_eq!(config["tooling"]["lastCodingAgent"], "codex");
        assert_eq!(config["tooling"]["telegramBot"], "ops");
        assert_eq!(config["unknown"]["keep"], true);
        assert_eq!(
            config["context"],
            serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)
        );
    }

    #[test]
    fn merge_root_agent_config_migrates_old_default_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{"tooling":{"lastCodingAgent":"codex"},"context":["$AGENTSCOMMANDER_CONTEXT","Role.md"]}"#,
        )
        .expect("write config");

        merge_root_agent_config(&config_path).expect("merge config");

        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read config"))
                .expect("parse config");
        assert_eq!(config["tooling"]["lastCodingAgent"], "codex");
        assert_eq!(
            config["context"],
            serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)
        );
    }

    #[test]
    fn merge_root_agent_config_preserves_custom_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{"context":["$AGENTSCOMMANDER_CONTEXT","custom.md","Role.md"]}"#,
        )
        .expect("write config");

        merge_root_agent_config(&config_path).expect("merge config");

        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read config"))
                .expect("parse config");
        assert_eq!(
            config["context"],
            serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "custom.md", "Role.md"])
        );
    }

    #[test]
    fn malformed_config_returns_error_without_rewriting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("config.json");
        std::fs::write(&config_path, "{not json").expect("write config");

        let err = merge_root_agent_config(&config_path).expect_err("must fail");

        assert!(err.contains("Failed to parse"));
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("read config"),
            "{not json"
        );
    }

    #[test]
    fn set_last_coding_agent_preserves_root_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        ensure_root_agent_dir_at(&root).expect("ensure root");

        crate::config::agent_config::set_last_coding_agent(
            &root.to_string_lossy(),
            "codex",
            "Codex",
            Some("session-1"),
        )
        .expect("set last coding agent");

        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("config.json")).expect("read config"),
        )
        .expect("parse config");
        assert_eq!(config["tooling"]["lastCodingAgent"], "codex");
        assert_eq!(
            config["context"],
            serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)
        );
    }

    #[test]
    fn read_last_coding_agent_reads_tooling_field_and_tolerates_bad_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(
            root.join("config.json"),
            r#"{"tooling":{"lastCodingAgent":"claude"}}"#,
        )
        .expect("write config");

        assert_eq!(
            read_last_coding_agent(&root.to_string_lossy()).as_deref(),
            Some("claude")
        );

        std::fs::write(root.join("config.json"), "{not json").expect("write bad config");
        assert!(read_last_coding_agent(&root.to_string_lossy()).is_none());
        assert!(read_last_coding_agent(&temp.path().join("missing").to_string_lossy()).is_none());
    }

    #[test]
    fn root_dir_name_detection_is_case_insensitive() {
        assert!(is_root_agent_dir_name("C:/tmp/AC-ROOT-AGENT"));
        assert!(!is_root_agent_dir_name("C:/tmp/not-root"));
    }

    // ---------------------------------------------------------------------------------------
    // #909: the frozen snapshot, and the two pins that keep it honest.
    // ---------------------------------------------------------------------------------------

    /// Look a default up by `dir_name`. Never index `DEFAULT_ROOT_SKILLS` positionally: the four
    /// pre-existing assertions do, and that is four too many.
    fn default_root_skill(dir_name: &str) -> &'static DefaultRootSkill {
        DEFAULT_ROOT_SKILLS
            .iter()
            .find(|skill| skill.dir_name == dir_name)
            .expect("default skill must be in the table")
    }

    #[test]
    fn agency_agents_roles_pre_yaml_fix_snapshot_is_byte_exact() {
        use sha2::{Digest, Sha256};

        // The raw pin. The normalized pin below is blind to exactly what normalization erases, and
        // `session_context::tests` writes this constant VERBATIM to disk as its real-world fixture,
        // so a snapshot that silently lost its trailing newline is no longer the file that test
        // claims to model.
        let lf = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.replace("\r\n", "\n");
        assert_eq!(
            lf.len(),
            2576,
            "snapshot must be the 646aeac blob, byte for byte"
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(lf.as_bytes())),
            "2b1643fc1a290fb2c04f490a27564fe23a155ccf17ed03e9c62f31a5b399cd22",
            "the frozen pre-fix snapshot changed; it must stay byte-identical to what shipped"
        );

        let normalized = normalize_role_text(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX);
        assert_eq!(
            format!("{:x}", Sha256::digest(normalized.as_bytes())),
            "78f498f0ead4ae5699b207358906f409b75d235738e4cfd2769b4f289089f10d",
            "the normalized snapshot changed; it is what every on-disk broken copy hashes to"
        );
    }

    /// Anti-drift. If anyone edits the shipped `SKILL.md` without adding a new snapshot, this fails
    /// loudly instead of silently orphaning the migration. Only valid because `when_to_use` stays an
    /// unquoted plain scalar (decision (a)); it has no inner `": "`.
    #[test]
    fn shipped_agency_skill_is_the_snapshot_with_a_quoted_description() {
        let snapshot = normalize_role_text(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX);
        let mut lines: Vec<String> = snapshot.lines().map(str::to_string).collect();
        // `.to_string()` is mandatory, not stylistic: `strip_prefix` borrows `lines[2]`, and the
        // next line takes a mutable borrow of `lines` while that borrow is live inside `format!`.
        let value = lines[2]
            .strip_prefix("description: ")
            .expect("snapshot line 3 is the description")
            .to_string();
        lines[2] = format!("description: \"{}\"", value);

        let agency = default_root_skill("agency-agents-roles");
        assert_eq!(lines.join("\n"), normalize_role_text(agency.content));
    }

    // ---------------------------------------------------------------------------------------
    // #909: the migration. Every enum assertion is paired with a content assertion. The enum
    // proves which branch ran; it does not prove no bytes were written. The pair is the proof.
    // ---------------------------------------------------------------------------------------

    struct AgencyFixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        skills_root: PathBuf,
        path: PathBuf,
    }

    fn seed_agency_skill(bytes: &[u8]) -> AgencyFixture {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let skills_root = root.join(ROOT_AGENT_SKILLS_DIR);
        let skill_dir = skills_root.join("agency-agents-roles");
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        let path = skill_dir.join(SKILL_MD_FILENAME);
        std::fs::write(&path, bytes).expect("seed skill file");
        AgencyFixture {
            _temp: temp,
            root,
            skills_root,
            path,
        }
    }

    fn agency_content() -> &'static str {
        default_root_skill("agency-agents-roles").content
    }

    fn migrate_agency(
        fixture: &AgencyFixture,
        legacy_snapshots: &[&str],
    ) -> Result<SkillMigration, String> {
        migrate_default_skill_file(
            &fixture.root,
            &fixture.skills_root,
            &fixture.path,
            agency_content(),
            legacy_snapshots,
        )
    }

    fn migrate_agency_with_snapshot(fixture: &AgencyFixture) -> Result<SkillMigration, String> {
        migrate_agency(fixture, &[AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX])
    }

    fn read_bytes(path: &Path) -> Vec<u8> {
        std::fs::read(path).expect("read skill file")
    }

    /// `warn_once_for_path` inserts on both the WARN and the DEBUG branch, so membership means
    /// exactly "we logged something about this path". This is how the no-WARN assertions are made
    /// deterministic without installing a global logger under parallel tests.
    fn warned_for_path(path: &Path) -> bool {
        SKILL_REPAIR_WARNED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(path)
    }

    fn stray_temp_files(skill_path: &Path) -> Vec<PathBuf> {
        let dir = skill_path.parent().expect("skill parent");
        std::fs::read_dir(dir)
            .expect("read skill dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".SKILL.md."))
            })
            .collect()
    }

    /// Reproduces `.agentscommander_ac2`, the one hand-repaired copy on disk, byte for byte.
    ///
    /// Built positionally rather than with `replacen`. `"template cache."` occurs TWICE in the
    /// file, on line 3 and again on line 21, so `str::replace` (which replaces all) would plant a
    /// stray apostrophe in the body and produce a file that exists nowhere. Rewriting line index 2
    /// cannot reach line 21 at all.
    fn user_edited_agency_skill() -> String {
        let mut lines: Vec<String> = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX
            .lines()
            .map(str::to_string)
            .collect();
        let value = lines[2]
            .strip_prefix("description: ")
            .expect("snapshot line 3 is the description")
            .to_string();
        lines[2] = format!("description: '{}'", value);
        format!("{}\n", lines.join("\n"))
    }

    /// The test that made the enum necessary, written the only way that proves anything.
    ///
    /// Asserting `Skipped(NoSnapshots)` against a junk fixture proves nothing, because junk also
    /// yields `Skipped(UserEdit)` under an empty list. The only fixture that isolates the early
    /// return is one that WOULD be repaired if the list were non-empty. Same fixture, same content,
    /// same path; the snapshot list is the only difference.
    #[test]
    fn empty_snapshot_list_is_what_prevents_the_repair() {
        let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());

        assert_eq!(
            migrate_agency(&fixture, &[]).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::NoSnapshots)
        );
        assert_eq!(
            read_bytes(&fixture.path),
            AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes()
        );

        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Repaired
        );
        assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
    }

    #[test]
    fn migrates_pristine_broken_agency_skill() {
        let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Repaired
        );
        assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
        assert!(stray_temp_files(&fixture.path).is_empty());
    }

    /// This is the case every broken install on disk is actually in, and the test that proves
    /// normalization earns its keep.
    #[test]
    fn migrates_pristine_broken_agency_skill_with_crlf() {
        let crlf = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.replace('\n', "\r\n");
        let fixture = seed_agency_skill(crlf.as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Repaired
        );
        assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
    }

    #[test]
    fn migrates_pristine_broken_agency_skill_with_trailing_whitespace() {
        let padded = format!("{}\n\n", AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX);
        let fixture = seed_agency_skill(padded.as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Repaired
        );
        assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
    }

    /// Pinned to the real file. If the fixture ever drifts away from `.agentscommander_ac2`, this
    /// stops being a test about the real world and nobody would notice.
    #[test]
    fn preserves_user_modified_agency_skill() {
        use sha2::{Digest, Sha256};

        let edited = user_edited_agency_skill();
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(normalize_role_text(&edited).as_bytes())
            ),
            "0c01c6f5c45092191f693e5c50c4d05da48deb9111990fa648214b659214d4c5",
            "fixture must stay byte-identical to the hand-repaired copy on disk"
        );

        let fixture = seed_agency_skill(edited.as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::UserEdit)
        );
        assert_eq!(read_bytes(&fixture.path), edited.as_bytes());
    }

    #[test]
    fn preserves_user_modified_agency_skill_with_appended_body() {
        let edited = format!("{}\nmy notes\n", AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX);
        let fixture = seed_agency_skill(edited.as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::UserEdit)
        );
        assert_eq!(read_bytes(&fixture.path), edited.as_bytes());
    }

    #[test]
    fn preserves_already_migrated_agency_skill() {
        let fixture = seed_agency_skill(agency_content().as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::AlreadyCurrent)
        );
        assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
        assert!(stray_temp_files(&fixture.path).is_empty());
    }

    #[test]
    fn migration_is_idempotent() {
        let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());

        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("first migration"),
            SkillMigration::Repaired
        );
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("second migration"),
            SkillMigration::Skipped(SkipReason::AlreadyCurrent)
        );
        assert_eq!(read_bytes(&fixture.path), agency_content().as_bytes());
    }

    /// Falsifiable ONLY because the padding is whitespace: `trim` removes it, so without the
    /// bounded read the padded file would normalize-equal the snapshot and be repaired. Padding
    /// with junk would be skipped as a user edit either way and would prove nothing.
    ///
    /// The size is taken from `max_migratable_len` at runtime. Hard-coding it fires on an LF build
    /// and silently misses on a Windows `autocrlf` build, where the bound is 66 bytes larger.
    #[test]
    fn migration_skips_oversized_file() {
        let snapshots = [AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX];
        let bound = max_migratable_len(agency_content(), &snapshots);
        let target = bound as usize + 1;
        let padding = target
            .checked_sub(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.len())
            .expect("bound must exceed the snapshot length");
        assert!(padding > 0);

        let oversized = format!(
            "{}{}",
            AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX,
            " ".repeat(padding)
        );
        assert!(oversized.len() as u64 > bound);
        // Without the guard this fixture WOULD be repaired: it trims to exactly the snapshot.
        assert_eq!(
            normalize_role_text(&oversized),
            normalize_role_text(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX)
        );

        let fixture = seed_agency_skill(oversized.as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::TooLarge)
        );
        assert_eq!(read_bytes(&fixture.path), oversized.as_bytes());
    }

    /// The data-loss input, made executable. This is the file that `take(bound)` without the `+ 1`
    /// and its rejection would have DESTROYED: a pristine snapshot, a long whitespace run, and then
    /// the user's real bytes sitting past the cut.
    ///
    /// Read to `bound + 1` and stopped there, the prefix is snapshot-plus-whitespace, which trims to
    /// exactly the snapshot, matches, and gets overwritten with `content`. `MY NOTES` is gone and
    /// nothing warns. The whole file, read honestly, is a user edit.
    ///
    /// Two things this fixture has that `migration_skips_oversized_file` does not. It is **the only
    /// fixture in the suite where `Take` stops strictly before EOF** (measured: every other call into
    /// `read_bounded` reads a file of at most `bound + 1` bytes, so the take limit and EOF coincide),
    /// which means the truncation path had no coverage at all until this test existed. And it is the
    /// only place where the data-loss consequence is executable rather than prose: a real user's
    /// bytes past the cut, destroyed, with the function reporting `Repaired`.
    ///
    /// What it does **not** add is coverage of the missing `+ 1`. `migration_skips_oversized_file`
    /// already catches that mutation: its fixture is exactly `bound + 1` bytes, so under `take(bound)`
    /// the read returns `bound` bytes, `buf.len() > bound` can never fire, the prefix trims to the
    /// snapshot, and it is `Repaired`. Both tests go red on it. The difference is the consequence:
    /// the old fixture's padding is pure whitespace, so the repair destroys nothing a user would miss
    /// and the test reports only a wrong classification. This one reports a destroyed file.
    ///
    /// Sized from `max_migratable_len` at runtime. A constant fires on an LF build and silently
    /// misses on a Windows `autocrlf` build, where the bound is 66 bytes larger.
    #[test]
    fn migration_skips_oversized_file_hiding_user_notes_past_the_cut() {
        let snapshots = [AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX];
        let bound = max_migratable_len(agency_content(), &snapshots);
        let cut = bound as usize + 1; // exactly what `read_bounded` will read

        // Enough whitespace that the cut lands strictly inside the run, so the truncated prefix is
        // snapshot + spaces and nothing else. The slack is what puts `MY NOTES` past the cut.
        let spaces = cut
            .checked_sub(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.len())
            .expect("bound must exceed the snapshot length")
            + 64;
        let notes = "MY NOTES\n";
        let hazard = format!(
            "{}{}{}",
            AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX,
            " ".repeat(spaces),
            notes
        );

        assert!(hazard.len() > cut, "the user's bytes must sit past the cut");
        assert!(hazard.is_char_boundary(cut));

        // The hazard, spelled out. Truncated at the cut, this file impersonates the snapshot.
        assert_eq!(
            normalize_role_text(&hazard[..cut]),
            normalize_role_text(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX),
            "the truncated prefix must be indistinguishable from a pristine stale default"
        );
        // Read honestly, it is nothing of the sort.
        assert_ne!(
            normalize_role_text(&hazard),
            normalize_role_text(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX)
        );
        assert!(normalize_role_text(&hazard).ends_with("MY NOTES"));

        let fixture = seed_agency_skill(hazard.as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::TooLarge)
        );
        assert_eq!(
            read_bytes(&fixture.path),
            hazard.as_bytes(),
            "the user's notes must survive, byte for byte"
        );
    }

    /// Classified, not an error. `Err` would mean a `log::warn!` on every root-agent context build
    /// forever, for a condition that is sticky and provably unrepairable: `legacy_snapshots` are
    /// `&'static str`, so no non-UTF-8 file can ever normalize-equal one.
    ///
    /// The no-WARN half is asserted through `ensure_default_root_agent_skills_at`, not through a
    /// direct call. `warn_once_for_path` lives at the caller's swallow point, so a direct call never
    /// has it in its call graph and `!warned_for_path` would be near-vacuous: the `Ok` return alone
    /// would carry it. Routed this way, the assertion means what its comment claims.
    #[test]
    fn migration_skips_non_utf8_file() {
        let invalid = [0xff_u8, 0xfe, 0x00, 0x41];
        let fixture = seed_agency_skill(&invalid);

        // The classification, asserted where the enum is visible.
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::NotUtf8)
        );

        // The absence of the warn, asserted where the warn would actually be emitted.
        ensure_default_root_agent_skills_at(&fixture.root)
            .expect("an unrepairable skill must never fail a session start");

        assert_eq!(read_bytes(&fixture.path), invalid);
        assert!(
            !warned_for_path(&fixture.path),
            "an unrepairable, sticky condition must not warn, on any build"
        );
    }

    /// `normalize_role_text` collapses `\r\n` only, and `str::trim` does not strip U+FEFF, which is
    /// not `White_Space`. So a BOM-prefixed copy is a user edit and stays broken. Pinned rather
    /// than asserted in prose.
    #[test]
    fn preserves_bom_prefixed_agency_skill() {
        let bom = format!("\u{FEFF}{}", AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX);
        let fixture = seed_agency_skill(bom.as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::UserEdit)
        );
        assert_eq!(read_bytes(&fixture.path), bom.as_bytes());
    }

    #[test]
    fn preserves_lone_cr_agency_skill() {
        let old_mac = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.replace('\n', "\r");
        let fixture = seed_agency_skill(old_mac.as_bytes());
        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::UserEdit)
        );
        assert_eq!(read_bytes(&fixture.path), old_mac.as_bytes());
    }

    /// Differential, and the differential is the point. A read-only file that is ALREADY CURRENT
    /// must classify as `AlreadyCurrent`, not `ReadOnly`: the read-only check sits below the
    /// classification returns precisely so it never warns about healthy files.
    #[cfg(windows)]
    #[test]
    fn migration_skips_read_only_destination() {
        fn set_readonly(path: &Path, readonly: bool) {
            let mut perms = std::fs::metadata(path).expect("metadata").permissions();
            perms.set_readonly(readonly);
            std::fs::set_permissions(path, perms).expect("set permissions");
        }

        // Healthy but read-only: classification wins, nothing is warned.
        let healthy = seed_agency_skill(agency_content().as_bytes());
        set_readonly(&healthy.path, true);
        assert_eq!(
            migrate_agency_with_snapshot(&healthy).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::AlreadyCurrent)
        );
        assert!(!warned_for_path(&healthy.path));
        set_readonly(&healthy.path, false);

        // Stale and read-only: `ReplaceFileW` would fail with os error 5, so we skip and warn once.
        let stale = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());
        set_readonly(&stale.path, true);
        // `SKILL_REPAIR_WARNED` is a process-global that is never cleared, so "a WARN was emitted"
        // below means the `first == true` branch ran only if this path was absent beforehand.
        // Assert that rather than inherit it from the tempdir being fresh.
        assert!(!warned_for_path(&stale.path));
        assert_eq!(
            migrate_agency_with_snapshot(&stale).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::ReadOnly)
        );
        assert_eq!(
            read_bytes(&stale.path),
            AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes()
        );
        assert!(warned_for_path(&stale.path));
        set_readonly(&stale.path, false);
    }

    /// The write side of the swallow. The read side is `migration_skips_non_utf8_file`; this is the
    /// side that can actually strand a user mid-session. Fault-injected rather than chmod-ed: under
    /// a root-uid CI container mode bits are ignored, the repair succeeds, and a chmod test goes red
    /// for a reason unrelated to the code.
    #[test]
    fn migration_write_failure_is_not_fatal() {
        let crlf = AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.replace('\n', "\r\n");
        let fixture = seed_agency_skill(crlf.as_bytes());

        let _armed = ArmedHook::new(
            &fixture.path,
            Box::new(|_| Err("injected publish failure".to_string())),
        );

        // See `migration_skips_read_only_destination`: the post-condition below means "a WARN was
        // emitted" only because this path was absent from the process-global memo beforehand.
        assert!(!warned_for_path(&fixture.path));

        ensure_default_root_agent_skills_at(&fixture.root)
            .expect("a failed repair must never fail a session start");

        assert_eq!(read_bytes(&fixture.path), crlf.as_bytes());
        assert!(warned_for_path(&fixture.path));
        assert!(stray_temp_files(&fixture.path).is_empty());
    }

    /// Another writer repaired the file inside our fsync window. We must observe that and publish
    /// nothing.
    ///
    /// The hook writes a byte-DIFFERENT but normalize-equal form, so "we skipped" and "we published"
    /// are distinguishable. Writing `content` itself would make the two outcomes identical on disk
    /// and the test would prove nothing.
    #[test]
    fn concurrent_repair_yields_already_current() {
        let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());
        let equivalent = format!("{}\n\n", normalize_role_text(agency_content()));
        assert_ne!(equivalent.as_bytes(), agency_content().as_bytes());

        let written = equivalent.clone();
        let _armed = ArmedHook::new(
            &fixture.path,
            Box::new(move |p: &Path| {
                std::fs::write(p, written.as_bytes()).map_err(|e| e.to_string())
            }),
        );

        assert_eq!(
            migrate_agency_with_snapshot(&fixture).expect("migration must not fail"),
            SkillMigration::Skipped(SkipReason::AlreadyCurrent)
        );
        assert_eq!(
            read_bytes(&fixture.path),
            equivalent.as_bytes(),
            "we must not have published over the other writer's bytes"
        );
        assert!(stray_temp_files(&fixture.path).is_empty());
    }

    /// THE test that proves the re-read earns its place. Delete the re-read block in
    /// `atomic_write_default_skill` and this goes red: the user's bytes are overwritten with
    /// `content`.
    #[test]
    fn destination_changed_under_us_is_not_written() {
        let fixture = seed_agency_skill(AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX.as_bytes());
        let user_edit = user_edited_agency_skill();

        let written = user_edit.clone();
        let _armed = ArmedHook::new(
            &fixture.path,
            Box::new(move |p: &Path| {
                std::fs::write(p, written.as_bytes()).map_err(|e| e.to_string())
            }),
        );

        let result = migrate_agency_with_snapshot(&fixture);
        assert!(
            result.is_err(),
            "a destination that changed mid-repair must abort, got {:?}",
            result
        );
        assert_eq!(
            read_bytes(&fixture.path),
            user_edit.as_bytes(),
            "the user's bytes must survive"
        );
        assert!(stray_temp_files(&fixture.path).is_empty());
    }
}
