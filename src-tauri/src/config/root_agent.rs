use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, OnceLock};

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
}

const DEFAULT_ROOT_SKILLS: &[DefaultRootSkill] = &[DefaultRootSkill {
    dir_name: "role-skill-boundary-audit",
    file_name: SKILL_MD_FILENAME,
    content: include_str!("root_agent_defaults/role-skill-boundary-audit/SKILL.md"),
}];

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

You may offer to download tested role templates from Agency Agents when the user wants a new specialist role. Ask before downloading or updating. If the user accepts, use the AgentsCommander CLI from `AGENTSCOMMANDER_BINARY_PATH`:

```text
"<AGENTSCOMMANDER_BINARY_PATH>" agency-templates update --ref main
"<AGENTSCOMMANDER_BINARY_PATH>" agency-templates status --pretty
"<AGENTSCOMMANDER_BINARY_PATH>" agency-templates list --pretty
```

Use only IDs returned by `agency-templates list` when creating agents with `create-agent-matrix --role-template <id>`. Do not invent Agency template IDs.
"#
    .to_string()
});

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
    create_missing_default_skill_file(root_dir, skills_root, &skill_path, skill.content)
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
    let existing = match read_validated_template(context_template_path)? {
        Some(existing) => existing,
        None => {
            crate::config::session_context::write_template_if_missing(
                context_template_path,
                ROOT_ROLE_MD.as_str(),
            )?;
            return read_validated_template(context_template_path)?
                .map(|_| ())
                .ok_or_else(|| {
                    format!(
                        "Template missing immediately after write_template_if_missing: {}",
                        context_template_path.display()
                    )
                });
        }
    };

    let existing_normalized = normalize_role_text(&existing);
    let old_generated = [
        normalize_role_text(OLD_ROOT_ROLE_MD),
        normalize_role_text(&OLD_ROOT_CONTEXT_WITH_COORDINATION_MD),
        normalize_role_text(ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD),
    ];
    if old_generated.contains(&existing_normalized) {
        std::fs::write(context_template_path, ROOT_ROLE_MD.as_str()).map_err(|e| {
            format!(
                "Failed to migrate root agent context template {}: {}",
                context_template_path.display(),
                e
            )
        })?;
    } else if existing.contains(ROOT_COORDINATION_MESSAGING_PARAGRAPH)
        || existing.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH)
    {
        log::warn!(
            "Custom root agent context template {} appears to contain stale operational messaging prose; preserving custom content",
            context_template_path.display()
        );
    }

    Ok(())
}

fn read_validated_template(path: &Path) -> Result<Option<String>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Failed to inspect root agent context template {}: {}",
                path.display(),
                e
            ))
        }
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Root agent context template {} exists but is not a regular file",
            path.display()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "Failed to read root agent context template {}: {}",
            path.display(),
            e
        )
    })?;
    String::from_utf8(bytes).map(Some).map_err(|e| {
        format!(
            "Root agent context template {} is not valid UTF-8: {}",
            path.display(),
            e
        )
    })
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

fn create_missing_default_skill_file(
    root_dir: &Path,
    skills_root: &Path,
    path: &Path,
    content: &str,
) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_default_skill_file(path, &metadata)?;
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
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(ROOT_ROLE_MD.contains("agency-templates update"));
        assert!(ROOT_ROLE_MD.contains("agency-templates list"));
        assert!(ROOT_ROLE_MD.contains("Do not invent Agency template IDs"));
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
}
