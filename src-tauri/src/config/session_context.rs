mod agent_template;
mod builders;
mod default_context;
mod default_context_values;
mod legacy_detection;
mod materialize;
mod repos;
mod skill_index;
mod skill_sections;
mod templates;

use crate::config::instance_artifacts::CONTEXT_CACHE_DIR_NAME;
use agent_template::resolve_agent_context_with_activation;
pub(crate) use agent_template::PTY_INPUT_COORDINATOR_CONTEXT;
pub use agent_template::{get_default_agent_template, get_default_coordinator_template};
#[cfg(test)]
use agent_template::{
    render_agent_context_template_inner, render_default_agent_context, resolve_agent_context,
};
pub use builders::build_replica_context;
use builders::{
    build_direct_matrix_context, build_replica_context_with_activation,
    build_root_agent_context_with_auto, read_rendered_root_template, root_context_candidate,
    root_context_config_dir, verify_root_context_identity, write_root_context_cache,
};
#[cfg(test)]
use builders::{
    build_root_agent_context_at, render_root_skills_list, render_root_template_text,
    root_conditions, root_runtime_values,
};
#[cfg(all(test, target_os = "windows"))]
use default_context::WINDOWS_SHELL_ROUTING;
#[cfg(all(test, not(target_os = "windows")))]
use default_context::WINDOWS_SHELL_ROUTING;
#[cfg(test)]
use default_context::{
    host_platform_rules_default, host_platform_rules_filename, render_root_runtime_prologue_inner,
    CORE_CONCEPTS_SECTION,
};
use default_context::{
    render_host_platform_rules_block, render_inter_agent_messaging_block,
    render_project_shared_entry, render_room_shared_entry, render_write_restrictions_block,
    strip_legacy_self_maintenance, DefaultContextDynamicValues, DEFAULT_CLI_CONTEXT,
    DEFAULT_DELEGATED_TASK_REPORTING, DEFAULT_SESSION_CREDENTIALS, DIRECT_MATRIX_GIT_SCOPE,
    ROOT_AUTHORITY_SECTION, ROOT_GIT_SCOPE, ROOT_PROJECT_SCOPE_ENTRY,
    SELF_MAINTENANCE_AUTO_SECTION,
};
#[cfg(test)]
use default_context_values::{default_context, default_context_as_root};
use default_context_values::{default_context_dynamic_values, root_agency_cache_guidance};
use legacy_detection::{classify_legacy_rendered_default_context, LegacyRenderedDefaultContext};
#[cfg(test)]
use legacy_detection::{
    count_context_occurrences, current_legacy_rendered_default_context,
    is_known_legacy_default_heading, looks_like_generated_legacy_default_context,
    normalize_context_for_compat,
};
pub(crate) use materialize::materialize_agent_context_file_with_filename_activated;
use materialize::simple_hash;
#[cfg(test)]
use materialize::{
    is_generated_context_filename, is_link_or_reparse, resolve_session_context_content,
    resolve_session_context_content_with_activation,
};
pub use materialize::{
    materialize_agent_context_file, materialize_agent_context_file_with_filename,
    sweep_context_cache_at_startup, sweep_context_cache_dir,
};
#[cfg(test)]
use repos::agent_repos_header;
pub use repos::git_ceiling_directories_for_session_root;
pub(crate) use repos::is_canonical_agent_matrix_dir;
use repos::{render_agent_repos_string, resolved_paths_include_path, write_combined_context_file};
use skill_index::{
    collect_skill_index, discover_skill_index, display_path, resolve_skill_owner_root,
    sanitize_skill_metadata_for_context, truncate_chars, SkillIndex, SkillMetadata,
    GENERATED_SKILLS_SECTION_INTRO, GENERATED_SKILLS_SECTION_REPLICA_LINE,
    GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME,
    LEGACY_GENERATED_SKILLS_SECTION_INTRO, ROLE_MD_FILENAME, SKILL_INDEX_TOTAL_MAX_BYTES,
    SKILL_TRIGGER_TEXT_MAX_CHARS,
};
#[cfg(test)]
use skill_index::{SKILLS_DIR_NAME, SKILL_FRONTMATTER_MAX_BYTES, SKILL_MD_FILENAME};
#[cfg(test)]
use skill_sections::{
    accept_skill_team, canonical_skill_member, render_team_skills_section, replica_skill_team,
    resolve_project_skill_root,
};
use skill_sections::{
    append_budget_summary, project_skills_for_session, push_with_budget, render_skills_section,
    skill_trigger_text, team_skills_for_session,
};
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::io::Write;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
pub use templates::create_default_context_templates;
pub(crate) use templates::{
    atomically_replace_context_template, create_default_context_templates_with_publications,
    strip_yaml_frontmatter, write_template_if_missing_with_clock,
};
#[cfg(test)]
use templates::{
    atomically_replace_context_template_with, heal_stale_global_context_template_with_clock,
    mandatory_section_heading, migrate_legacy_agent_context_template_with,
    read_or_create_context_template_with_publications, read_or_create_context_template_with_sync,
    write_template_if_missing_with, ContextTemplateWriter,
};
use templates::{
    coarse_section_dedup_safe, heal_stale_global_recorded, managed_template_local_override,
    mandatory_placeholder_aliases, mandatory_section_present_inline, read_context_local_override,
    read_context_template, read_or_create_context_recorded, resolve_ac_root_context_dir,
    TemplateTokenSignals, MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS,
};

pub const GLOBAL_CONTEXT_TEMPLATE_FILENAME: &str =
    crate::config::instance_artifacts::GLOBAL_CONTEXT_TEMPLATE_FILENAME;
const LEGACY_AGENT_CONTEXT_TEMPLATE_FILENAME: &str = "Context.agent.md";
pub const COORDINATOR_CONTEXT_TEMPLATE_FILENAME: &str = "Context.coordinator.md";
pub const ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME: &str =
    crate::config::instance_artifacts::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME;

/// #1605: per-platform project `.ac` files whose content IS the rendered
/// `{{HOST_PLATFORM_RULES}}` block (including its `## Host Platform Rules`
/// heading). Project-level only; never seeded in the app-config directory.
pub const HOST_PLATFORM_RULES_FILENAME_WINDOWS: &str = "Context.platform.windows.md";
pub const HOST_PLATFORM_RULES_FILENAME_LINUX: &str = "Context.platform.linux.md";
pub const HOST_PLATFORM_RULES_FILENAME_MACOS: &str = "Context.platform.macos.md";

/// #1605: embedded default content for the `{{HOST_PLATFORM_RULES}}` block on
/// Windows host sessions. Single source for both the `.ac/Context.platform.windows.md` seed
/// and the render fallback.
pub(crate) const DEFAULT_HOST_PLATFORM_RULES_WINDOWS: &str = concat!(
    r#"## Host Platform Rules

Windows host session: invoke `C:\Program Files\Git\bin\bash.exe` directly for all shell work and "#,
    r#"every AgentsCommander CLI invocation; avoid PowerShell whenever Git Bash is available. "#,
    r#"Only if PowerShell is unavoidable, wrap with `& 'C:\Program Files\Git\bin\bash.exe' "#,
    r#"-lc '...'` and capture CLI output with `2>&1 | Out-String`."#
);

/// #1605: embedded default content for the `{{HOST_PLATFORM_RULES}}` block on
/// Linux host sessions (106 bytes; sha256
/// 848e6bd25a001b091dd419fb665d2114abc5e5c2a19429fc820144aaa6190790 at
/// 047248bc). Single source for both the `.ac/Context.platform.linux.md` seed
/// and the render fallback.
pub(crate) const DEFAULT_HOST_PLATFORM_RULES_LINUX: &str = r#"## Host Platform Rules

This session runs on a Linux host; no platform-specific shell routing rules apply."#;

/// #1605: embedded default content for the `{{HOST_PLATFORM_RULES}}` block on
/// macOS host sessions (106 bytes; sha256
/// a305af4fce8148ac80d8e197b02142738a6056959e9b72bee43689b3d01cdc4d at
/// 047248bc). Single source for both the `.ac/Context.platform.macos.md` seed
/// and the render fallback.
pub(crate) const DEFAULT_HOST_PLATFORM_RULES_MACOS: &str = r#"## Host Platform Rules

This session runs on a macOS host; no platform-specific shell routing rules apply."#;
static CONTEXT_TEMPLATE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateOnlyPublication {
    Published {
        published_at: chrono::DateTime<chrono::Utc>,
    },
    AlreadyPresent,
}

/// Map a managed project context-template filename to its canonical v1 seed-manifest
/// scope, or `None` for a non-project template (the Root agent context) that carries
/// no manifest row. The project scopes are the closed enum in plan section 4.2
/// (#1605 adds the per-platform `context:platform` scope for the three
/// `Context.platform.<os>.md` files).
pub(crate) fn manifest_scope_for_project_context_filename(filename: &str) -> Option<&'static str> {
    if filename == GLOBAL_CONTEXT_TEMPLATE_FILENAME {
        Some("context:agentscommander")
    } else if filename == COORDINATOR_CONTEXT_TEMPLATE_FILENAME {
        Some("context:coordinator")
    } else if matches!(
        filename,
        HOST_PLATFORM_RULES_FILENAME_WINDOWS
            | HOST_PLATFORM_RULES_FILENAME_LINUX
            | HOST_PLATFORM_RULES_FILENAME_MACOS
    ) {
        Some("context:platform")
    } else {
        None
    }
}

/// #1065 Stage F: record a context-template publication into the project seed
/// manifest under an already-held gate.
///
/// `published_at` must be the commit-point time carried by
/// [`crate::config::seeded_context_templates::ContextPublication`]; the recorder
/// never samples a later clock. A non-project filename (no scope), an invalid path,
/// or a degraded permit is a fail-soft no-op: the manifest never blocks or retracts
/// the context operation (plan sections 5.2/6.4).
pub(crate) fn record_project_context_publication(
    guard: &mut crate::config::seed_manifest::ProjectSeedManifestGuard,
    activation: &crate::config::seed_manifest::ManifestActivationToken,
    filename: &str,
    published_at: chrono::DateTime<chrono::Utc>,
) {
    let Some(scope) = manifest_scope_for_project_context_filename(filename) else {
        return;
    };
    let relative = Path::new(".ac").join(filename);
    let identity =
        match crate::config::seed_manifest::ManifestPathIdentity::from_relative_path(&relative) {
            Ok(identity) => identity,
            Err(error) => {
                log::warn!(
                    "[session_context] seed-manifest context row rejected filename={} error={}",
                    filename,
                    error
                );
                return;
            }
        };
    let row = match crate::config::seed_manifest::PublishedManifestRow::project_context(
        identity,
        scope,
        published_at,
    ) {
        Ok(row) => row,
        Err(error) => {
            log::warn!(
                "[session_context] seed-manifest context row rejected scope={} error={}",
                scope,
                error
            );
            return;
        }
    };
    let outcome = guard.publication_permit().record_file(activation, row);
    log::debug!(
        "[session_context] seed-manifest context publication scope={} outcome={:?}",
        scope,
        outcome
    );
}

/// Writes a per-agent copy of AgentsCommanderContext.md with the agent's own
/// root path interpolated into the GOLDEN RULE. For WG replicas, also exposes
/// the canonical Agent Matrix scope derived from config.json "identity". Uses a
/// deterministic filename based on the agent_root to prevent races between
/// concurrent session launches.
pub fn ensure_session_context(agent_root: &str) -> Result<String, String> {
    ensure_session_context_with_config(agent_root, None, None, None)
}

fn ensure_session_context_with_config(
    agent_root: &str,
    config: Option<&serde_json::Value>,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<String, String> {
    if root_context_candidate(agent_root) {
        verify_root_context_identity(agent_root)?;
        super::root_agent::ensure_default_root_agent_skills_at(Path::new(agent_root))?;
        let config_dir = root_context_config_dir()?;
        let path = config_dir.join(ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let content = read_rendered_root_template(
            agent_root,
            &path,
            &config_dir,
            config,
            repo_mounts,
            false,
        )?;
        return write_root_context_cache(agent_root, &config_dir, &content, "ac-context");
    }
    let config_dir =
        super::config_dir().ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let context_dir = config_dir.join(CONTEXT_CACHE_DIR_NAME);
    crate::filesystem::session_context::ensure_context_directory(&context_dir)
        .map_err(|e| format!("Failed to create context-cache dir: {}", e))?;

    // Canonicalize path for consistent display in the GOLDEN RULE text
    let canonical_root = crate::filesystem::session_context::canonical_context_source(agent_root)
        .map(|p| display_path(&p))
        .unwrap_or_else(|_| agent_root.to_string());
    let matrix_root = resolve_replica_matrix_root(agent_root)?;
    ensure_replica_shared_locations(agent_root, &canonical_root);
    let skill_owner_root = resolve_skill_owner_root(agent_root, matrix_root.as_deref());
    let skill_index = discover_skill_index(skill_owner_root.as_deref());
    let skills_section = render_skills_section(&skill_index);
    let project_section =
        project_skills_for_session(agent_root, matrix_root.as_deref(), &skill_index);

    for warning in &skill_index.warnings {
        log::warn!("[skills] {}", warning);
    }
    for skill in &skill_index.skills {
        for warning in &skill.metadata_warnings {
            log::warn!("[skills] {}: {}", skill.folder_name, warning);
        }
    }

    let hash = simple_hash(agent_root);
    let file_path = context_dir.join(format!("ac-context-{}.md", hash));

    // Raw and canonical Root identities return above without entering the global
    // resolver or creating project/coordinator/platform templates.
    let mut content = resolve_agent_context_with_activation(
        &canonical_root,
        matrix_root.as_deref(),
        &skills_section,
        Path::new(agent_root),
        config,
        repo_mounts,
        activation,
    )?;
    content.push_str(&project_section);
    content.push_str(&team_skills_for_session(agent_root, matrix_root.as_deref()));
    crate::filesystem::session_context::write_context_cache(&file_path, content)
        .map_err(|e| format!("Failed to write per-agent AgentsCommanderContext.md: {}", e))?;
    log::info!(
        "Refreshed per-agent AgentsCommanderContext.md for {} → {:?}",
        agent_root,
        file_path
    );

    Ok(file_path.to_string_lossy().to_string())
}

/// #1795 backstop for a project or a room that predates #1795. Bootstrap
/// (`config::seeded_context_templates`) and room creation (`commands::entity_creation`)
/// create these eagerly and hard-error on failure.
///
/// Takes BOTH the raw and the canonical agent root and applies the SAME two-name Root
/// guard as the content selection at :187-188. A Root Agent directory that is a junction
/// onto a replica path has a raw basename of `ac-root-agent` and a canonical basename of
/// `__agent_<name>`; guarding on one name only would create directories on that Root
/// Agent's behalf. `T10` pins this.
///
/// Resolves with the SAME `replica_shared_roots` the renderer uses, from the SAME
/// canonical root, so creation and rendering can never disagree about which directories
/// entry 5 and entry 6 name. `T13` pins that a session start creates them and `T6b` that
/// the resolver gates the creation.
///
/// Best-effort on purpose: refusing to start a session because a shared directory could
/// not be created would be a worse outcome than the grant it protects. A grant over a
/// directory that is missing fails at use time with an ordinary filesystem error, and
/// the warning below names the path. `T11` pins that a failure does not fail the session.
fn ensure_replica_shared_locations(agent_root: &str, canonical_root: &str) {
    if super::root_agent::is_root_agent_dir_name(agent_root)
        || super::root_agent::is_root_agent_dir_name(canonical_root)
    {
        return;
    }
    let Some((ac_root, room_root)) = replica_shared_roots(canonical_root) else {
        return;
    };
    if let Err(e) = super::shared_locations::create_project_shared_dirs(&ac_root) {
        log::warn!(
            "[session_context] #1795 could not create project shared dirs under {}: {}",
            ac_root.display(),
            e
        );
    }
    if let Err(e) = super::shared_locations::create_room_shared_dir(&room_root) {
        log::warn!(
            "[session_context] #1795 could not create room shared dir under {}: {}",
            room_root.display(),
            e
        );
    }
}

const MANAGED_CONTEXT_FILENAMES: &[&str] = &["last_ac_context.md", "CLAUDE.md", "AGENTS.md"];

#[derive(Debug, Clone, Copy)]
pub enum ManagedContextTarget {
    Claude,
    Antigravity,
    Codex,
    Pi,
}

impl ManagedContextTarget {
    /// `pub` since #529: the launch-time resolver in `agent_command.rs` and the
    /// detection fallback in `commands/session.rs` map a detected target to its
    /// filename.
    pub fn filename(self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE.md",
            Self::Antigravity => "AGENTS.md",
            Self::Codex => "AGENTS.md",
            Self::Pi => "AGENTS.md",
        }
    }
}

/// Special token in context[] that resolves to the global AgentsCommanderContext.md.
/// #979: `pub(crate)` so `root_agent.rs` migrates Root `context[]` arrays against this
/// exact constant instead of duplicating the literal.
pub(crate) use super::root_agent::CONTEXT_TOKEN_GLOBAL;

/// Special token in context[] that generates workspace repo info from the "repos" field.
const CONTEXT_TOKEN_REPOS: &str = "$REPOS_WORKSPACE_INFO";

/// #1369 (C4): the pre-rename spelling of `{{AGENT_REPOS}}`. These bytes are on
/// users' disks: the token was written into every seeded
/// `.ac/Context.AgentsCommander.md` from #658 through v2, and a customized or
/// user-ignored template is preserved by `sync_one_template` and never
/// auto-upgraded. The alias therefore has NO expiry. Never edit.
const LEGACY_AGENT_REPOS_PLACEHOLDER: &str = "{{WORKSPACE_REPOS}}";

/// Resolve the canonical Agent Matrix root for a WG replica from config.json "identity".
fn resolve_replica_matrix_root(replica_root: &str) -> Result<Option<String>, String> {
    if !is_replica_agent_dir(replica_root) {
        return Ok(None);
    }

    let replica_path = std::path::Path::new(replica_root);
    crate::config::replica_identity::read_and_repair_wg_replica_config(
        replica_path,
        crate::config::replica_identity::WG_REPLICA_REQUIRED_CONTEXT,
    )
    // Canonicalize so the embedded matrix path matches the canonicalized
    // replica root (`canonical_root` in `ensure_session_context_with_config`)
    // and the skills roots. Without this, a non-canonical base dir (e.g. CI
    // runner 8.3 short names or differing case) makes the matrix path diverge
    // from every other rendered path and from `assert_contains_canonical_path`.
    .map(|(_, identity)| Some(display_path(&canonical_or_original(&identity.matrix_dir))))
    .map_err(|e| {
        format!(
            "Invalid Room replica identity for '{}': {}",
            replica_path.display(),
            e
        )
    })
}

fn canonical_or_original(path: &std::path::Path) -> std::path::PathBuf {
    crate::filesystem::session_context::canonical_context_source(path)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn find_ac_root(path: &std::path::Path) -> Option<std::path::PathBuf> {
    crate::config::ac_root::find_ac_root_ancestor(path).map(|p| canonical_or_original(&p))
}

/// The project `.ac` root and the room root for an authoritative room replica, or
/// `None` for anything else.
///
/// #1795. This is deliberately STRICTER than `phone::messaging::workgroup_root`, which
/// gates Golden Rule branch A. `workgroup_root` walks ancestors and accepts any
/// descendant, at any depth, of any `room-<n>-`/`wg-<n>-` directory under any parent
/// (`phone/messaging.rs:146-157`, `:372-383`). Entry 4 can afford that, because it
/// renders a path INSIDE the room. Entry 5 renders paths in the room's PARENT, so the
/// loose predicate would grant read and write over `<anything>/plans`, `<anything>/tools`,
/// `<anything>/errors` and `<anything>/project-shared` for an input such as
/// `C:/x/room-1-t/__agent_dev/sub/nested` or `C:/x/room-1-t/notanagent`. Those are not
/// attacks; they are ordinary directories that exist and resolve, and round 1's resolver
/// would have granted them. This one fails closed instead.
///
/// The three checks below are exactly the first three of
/// `config::ac_root::wg_replica_layout_from_agent_dir` (`config/ac_root.rs:127-173`),
/// and they are pure NAME checks, so this function touches no disk and section 6's
/// deterministic byte budget is unaffected (3.2).
///
/// There is no Root Agent check here and none is needed: `ROOT_AGENT_DIR_NAME` is
/// `"ac-root-agent"` (`config.rs:49`), which never begins with `__agent_`, so check
/// one already rejects every Root Agent directory. The Root Agent exclusion that IS
/// load-bearing lives at the call sites: `session_context.rs:187-188` for rendering and
/// `ensure_replica_shared_locations` for creation, both testing the raw AND the
/// canonical basename.
fn replica_shared_roots(agent_root: &str) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let agent_path = Path::new(agent_root);

    // 1. the agent directory itself must be an `__agent_` replica
    if !agent_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_"))
        .unwrap_or(false)
    {
        return None;
    }

    // 2. its DIRECT parent must be the room root. Using the parent rather than
    //    `workgroup_root`'s ancestor walk is what rejects a nested non-replica.
    let room_root = agent_path.parent()?;
    if !crate::config::entity_prefix::has_entity_prefix(
        room_root.file_name().and_then(|name| name.to_str())?,
    ) {
        return None;
    }

    // 3. the room's parent must be a Project AC Root BY NAME (`.ac`).
    let ac_root = room_root.parent()?;
    if !super::ac_root::is_ac_root_name(ac_root.file_name().and_then(|name| name.to_str())?) {
        return None;
    }

    Some((ac_root.to_path_buf(), room_root.to_path_buf()))
}
const WORKGROUP_GIT_SCOPE: &str =
    "`room-*/` and `wg-*/` are gitignored; origin Agent Matrices are \
    not and can be tracked. Git discovery above replica and Matrix roots is blocked. \
    State-changing Git belongs in `repo-*`; read-only Git is allowed within scope.";
/// #1614 D8b: the exact `WORKGROUP_GIT_SCOPE` bytes shipped at d7008b34.
/// Read only by `legacy_rendered_default_context_for_generation`, which
/// reconstructs a pre-#1369 `Context.AgentsCommander.md` for byte
/// comparison. Never used for current runtime output. 220 bytes, sha256
/// A386B52D...566D (plan section 3.12 Table B); pinned by
/// `workgroup_git_scope_split_is_correct`. Never edit.
const WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME: &str = "`wg-*/` workgroups are gitignored; origin \
    Agent Matrices are not and can be tracked. Git discovery above replica and Matrix roots is \
    blocked. State-changing Git belongs in `repo-*`; read-only Git is allowed within scope.";

// Frozen warning bytes shipped before #1072. These constants are only for
// compatibility recognition and must never be used for current runtime output.
const LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072: &str = "Your replica directory and origin Agent \
    Matrix are typically inside a parent repository's `.ac/` folder, which is `.gitignore`d. Do \
    NOT run `git` commands that alter state (commit, branch, reset, etc.) from inside either \
    location, because that would affect the parent repo unintentionally. AgentsCommander blocks \
    Git repository discovery above these Project AC Root directories for agent sessions, but you \
    must still switch into the appropriate `repo-*` directory before running Git operations that \
    change repository state. `git status`, `git log`, and `git diff` are fine inside the allowed \
    roots.";
const LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072: &str = "Your agent directory is typically \
    inside a parent repository's `.ac/` folder, which is `.gitignore`d. Do NOT run `git` commands \
    that alter state (commit, branch, reset, etc.) from inside that directory, because that would \
    affect the parent repo unintentionally. AgentsCommander blocks Git repository discovery above \
    these Project AC Root directories for agent sessions, but you must still switch into the \
    appropriate `repo-*` directory before running Git operations that change repository state. \
    `git status`, `git log`, and `git diff` are fine inside the allowed roots.";

enum LegacyGitScopeGeneration {
    Current,
    Before1072,
}

fn legacy_rendered_default_context_for_compat(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
) -> String {
    legacy_rendered_default_context_for_generation(
        agent_root,
        matrix_root,
        skills_section,
        LegacyGitScopeGeneration::Current,
    )
}

fn pre_1072_legacy_rendered_default_context_for_compat(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
) -> String {
    legacy_rendered_default_context_for_generation(
        agent_root,
        matrix_root,
        skills_section,
        LegacyGitScopeGeneration::Before1072,
    )
}

fn legacy_rendered_default_context_for_generation(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
    git_scope_generation: LegacyGitScopeGeneration,
) -> String {
    enum MessagingContextMode {
        None,
        Workgroup(String),
        Root(String),
    }

    let allowed_places = "the entries listed below";
    let replica_usage =
        "   Use this for replica-local scratch, personal notes, inbox/outbox, role drafts, and session artifacts. Do NOT store canonical memory, plans, or skills here. Do NOT write into other agents' replica directories.";
    let matrix_section = match matrix_root {
        Some(matrix_root) => format!(
            "3. **Your origin Agent Matrix, but only for the canonical agent state listed below:**\n   ```\n   {matrix_root}\n   ```\n   Allowed there:\n   - `memory/`\n   - `plans/`\n   - `skills/`\n   - `Role.md`\n\n",
            matrix_root = matrix_root,
        ),
        None => String::new(),
    };
    let matrix_allowed = match matrix_root {
        Some(matrix_root) => format!(
            "- **Allowed**: Full read/write inside your origin Agent Matrix's `memory/`, `plans/`, `skills/`, and `Role.md` ({matrix_root})\n",
            matrix_root = matrix_root,
        ),
        None => String::new(),
    };
    let messaging_mode = if super::root_agent::is_root_agent_dir_name(agent_root) {
        MessagingContextMode::Root(display_path(
            &std::path::Path::new(agent_root).join(crate::phone::messaging::MESSAGING_DIR_NAME),
        ))
    } else {
        match crate::phone::messaging::workgroup_root(std::path::Path::new(agent_root)) {
            Ok(wg) => MessagingContextMode::Workgroup(display_path(
                &wg.join(crate::phone::messaging::MESSAGING_DIR_NAME),
            )),
            Err(_) => MessagingContextMode::None,
        }
    };
    let messaging_exception = match &messaging_mode {
        MessagingContextMode::Workgroup(path) => format!(
            "**Narrow exception — workgroup messaging directory:**\n\n\
             You MAY create message files inside this directory:\n\n\
             ```\n\
             {path}\n\
             ```\n\n\
             Strictly limited to canonical inter-agent message files whose name matches the pattern `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md` (the CLI rejects any other shape). Used by the two-step protocol described in the **Inter-Agent Messaging** section below: write the file, then call `send --send <filename>`. Do NOT modify or delete any message file once written. Do NOT write any other kind of file here.\n\n",
            path = path,
        ),
        MessagingContextMode::Root(path) => format!(
            "**Narrow exception — Root Agent messaging directory:**\n\n\
             You MAY create message files inside this directory:\n\n\
             ```\n\
             {path}\n\
             ```\n\n\
             Strictly limited to canonical Root Agent inter-agent message files whose name matches the pattern `YYYYMMDD-HHMMSS-root-to-<wgN>-<coordinator>-<slug>.md` (the CLI rejects any other shape). Used by the Root Agent coordinator-only protocol described in the **Inter-Agent Messaging** section below: write the file, then call `send --send <filename>`. Do NOT modify or delete any message file once written. Do NOT write any other kind of file here.\n\n",
            path = path,
        ),
        MessagingContextMode::None => String::new(),
    };
    let messaging_allowed = match &messaging_mode {
        MessagingContextMode::Workgroup(path) => format!(
            "- **Allowed (narrow)**: Create canonical inter-agent message files in your workgroup messaging directory ({path}). No other writes there.\n",
            path = path,
        ),
        MessagingContextMode::Root(path) => format!(
            "- **Allowed (narrow)**: Create canonical Root Agent inter-agent message files in your Root Agent messaging directory ({path}). No other writes there.\n",
            path = path,
        ),
        MessagingContextMode::None => String::new(),
    };
    let has_messaging_exception = !matches!(messaging_mode, MessagingContextMode::None);
    let ac_root_phrase = if has_messaging_exception {
        "the workspace root (other than the narrow messaging exception above)"
    } else {
        "the workspace root"
    };
    let forbidden_scope = if matrix_root.is_some() {
        format!(
            "the entries listed above — including other agents' replica directories, any other files inside the Agent Matrix, {ws}, parent project dirs, user home files, or arbitrary paths on disk",
            ws = ac_root_phrase,
        )
    } else {
        format!(
            "the entries listed above — including other agents' replica directories, {ws}, parent project dirs, user home files, or arbitrary paths on disk",
            ws = ac_root_phrase,
        )
    };
    let git_scope = match (git_scope_generation, matrix_root.is_some()) {
        (LegacyGitScopeGeneration::Current, true) => WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME,
        (LegacyGitScopeGeneration::Current, false) => DIRECT_MATRIX_GIT_SCOPE,
        (LegacyGitScopeGeneration::Before1072, true) => LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072,
        (LegacyGitScopeGeneration::Before1072, false) => {
            LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072
        }
    };
    let agency_cache_guidance = root_agency_cache_guidance(agent_root);
    let peer_name_format = match &messaging_mode {
        MessagingContextMode::Root(_) => "- **Root Agent sessions**: verified WG coordinator replicas only, shaped `<project>:<workgroup>/<agent>` — e.g. `agentscommander:wg-15-dev-team/tech-lead`.\n\nOrigin coordinators and non-coordinator WG replicas are not valid Root Agent targets in #277.".to_string(),
        _ => "- **WG replicas** (the common case): `<project>:<workgroup>/<agent>` — e.g. `agentscommander:wg-15-dev-team/dev-rust`.\n- **Origin agents**: `<project>/<agent>` — e.g. `agentscommander/architect`.".to_string(),
    };
    let send_message_instructions = match &messaging_mode {
        MessagingContextMode::Root(path) => format!(
            "Before sending, run `list-peers-lean`; in Root Agent sessions it returns verified WG coordinator replicas only. Use only the JSON `name` values returned by `list-peers-lean`.\n\n\
             Root messaging is **file-based** to avoid PTY truncation. Two steps:\n\n\
             1. Write your message to a new file in the Root Agent messaging directory:\n\n\
             ```\n\
             {path}\n\
             ```\n\n\
             Filename must follow the pattern `YYYYMMDD-HHMMSS-root-to-<wgN>-<coordinator>-<slug>.md` (UTC timestamp, sanitized kebab-case slug ≤50 chars).\n\
             2. Fire the send:\n\n\
             ```\n\
             \"<AGENTSCOMMANDER_BINARY_PATH>\" send --token <AGENTSCOMMANDER_TOKEN> --root \"<AGENTSCOMMANDER_ROOT>\" --to \"<coordinator_name>\" --send <filename> --mode wake\n\
             ```\n\n\
             **IMPORTANT: `--send` takes the filename ONLY — never a path.**\n\n\
             Origin coordinators and non-coordinator WG replicas are not valid Root Agent targets in #277.\n",
            path = path,
        ),
        _ => "Messaging is **file-based** to avoid PTY truncation. Two steps:\n\n\
             1. Write your message to a new file in the workgroup messaging directory. The\n\
                directory lives at `<workgroup-root>/messaging/` (walk up from your root\n\
                until you find the parent `wg-<N>-*` folder). Filename must follow the\n\
                pattern `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md` (UTC\n\
                timestamp, sanitized kebab-case slug ≤50 chars).\n\
             2. Fire the send:\n\n\
             ```\n\
             \"<AGENTSCOMMANDER_BINARY_PATH>\" send --token <AGENTSCOMMANDER_TOKEN> --root \"<AGENTSCOMMANDER_ROOT>\" --to \"<agent_name>\" --send <filename> --mode wake\n\
             ```\n\n\
             **IMPORTANT: `--send` takes the filename ONLY — never a path.**\n\n\
             - BAD:  `--send \"C:\\...\\messaging\\20260419-143052-wg3-you-to-wg3-peer-hello.md\"`\n\
             - GOOD: `--send \"20260419-143052-wg3-you-to-wg3-peer-hello.md\"`\n\n\
             The CLI resolves the filename against `<workgroup-root>/messaging/` automatically. Passing a path triggers `filename '...' contains path separators or traversal`.\n"
            .to_string(),
    };
    format!(
        r#"# AgentsCommander Context

You are running inside an AgentsCommander session — a terminal session manager that coordinates multiple AI agents.

## GOLDEN RULE — Repository Write Restrictions

**ABSOLUTE AND NON-NEGOTIABLE:** You may ONLY modify files in {allowed_places}:

1. **Repositories whose root folder name starts with `repo-`** (e.g. `repo-AgentsCommander`, `repo-myapp`). These are the working repos you are meant to edit.
2. **Your own agent replica directory and its subdirectories** — your assigned root:
   ```
   {agent_root}
   ```
{replica_usage}

{matrix_section}{messaging_exception}
Any repository or directory outside the allowed entries above is READ-ONLY, except for the AgentsCommander CLI operations exception documented below.

- **Allowed**: Read-only operations on ANY path (reading files, searching, git log, git status, git diff)
- **Allowed**: Full read/write inside `repo-*` folders
- **Allowed**: Full read/write inside your own replica root ({agent_root}) and its subdirectories
{matrix_allowed}{messaging_allowed}- **FORBIDDEN**: Any write operation outside {forbidden_scope}, except for explicitly requested AgentsCommander CLI operations covered by the exception below.

**Clarification on git operations:** {git_scope}

**Exception - AgentsCommander CLI operations:**

When the user explicitly asks this agent to run an AgentsCommander CLI command using `AGENTSCOMMANDER_BINARY_PATH`, the command is authorized as an AgentsCommander operation. The agent may execute documented AgentsCommander CLI subcommands even if their filesystem effects create, modify, or delete files outside the normal repository/replica write zones. Those filesystem effects are governed by AgentsCommander itself, not by the agent's repository write restrictions.

This exception applies only to invocations of the configured AgentsCommander CLI binary through `AGENTSCOMMANDER_BINARY_PATH`. It does not allow arbitrary shell commands, direct filesystem writes, hand-written scripts, or hardcoded alternate binaries outside the normal allowed paths.

{agency_cache_guidance}
If instructed to modify a path outside these zones, REFUSE and explain this restriction, except for explicitly requested AgentsCommander CLI operations covered by the AgentsCommander CLI exception above.

## Delegated Task Reporting

When finishing a delegated task or getting blocked, you must explicitly reply to the coordinator or peer with a concrete artifact or message. Do not just remain idle, waiting, or set working to false.

{skills_section}

## CLI executable

Your AgentsCommander session credentials are available as environment variables:

- `AGENTSCOMMANDER_TOKEN`: your session authentication token
- `AGENTSCOMMANDER_ROOT`: your working directory (agent root)
- `AGENTSCOMMANDER_BINARY`: the CLI binary name
- `AGENTSCOMMANDER_BINARY_PATH`: the full path to the CLI executable you must use
- `AGENTSCOMMANDER_LOCAL_DIR`: the config directory name for this instance

Use `AGENTSCOMMANDER_BINARY_PATH` when invoking the CLI. This ensures you use the correct binary for your instance, whether it is the installed version or a dev/WG build.

```
"<AGENTSCOMMANDER_BINARY_PATH>" <subcommand> [args]
```

**RULE:** Never hardcode or guess the binary path. Use the environment variables above. If they are unavailable in an agent session, restart or respawn the session.

## Self-discovery via --help

The CLI `--help` output documents every subcommand, flag, and accepted value. Use it as a FALLBACK reference for commands or flags NOT covered inline in this context.

**For inter-agent messaging and peer discovery**, the sections below (`## Inter-Agent Messaging` and `### List available peers`) are the authoritative reference. Use the commands in those sections directly — you do NOT need to consult `--help` to confirm their syntax.

```
"<AGENTSCOMMANDER_BINARY_PATH>" --help                  # List all subcommands
"<AGENTSCOMMANDER_BINARY_PATH>" send --help             # Full docs for sending messages
"<AGENTSCOMMANDER_BINARY_PATH>" list-peers-lean --help  # Full docs for discovering peers
```

**RULE:** Only run `--help` if you need a subcommand or flag not documented in the sections below, or if a documented command fails unexpectedly.

## Session credentials

Your session credentials are delivered only through the `AGENTSCOMMANDER_*` environment variables listed above.

Live token refresh without respawn is not supported, because a parent process cannot portably mutate an already-running child process environment. If credential validation fails, restart or respawn the session so AgentsCommander can create a new child process with fresh env values.

Your agent root is your current working directory.

## Inter-Agent Messaging

### Send a message to another agent

**MANDATORY**: Before sending any message, resolve the exact agent name via `list-peers-lean`. Never guess agent names.

**Peer name format** (canonical FQN, exactly what `list-peers-lean` emits in the `name` field):

{peer_name_format}

**The filesystem directory name is NEVER a valid `--to` value.** Replica dirs like `__agent_shipper` and matrix dirs like `_agent_architect` are on-disk paths only — they are not peer names. The `list-peers-lean` JSON `name` field is the only authoritative source. If `list-peers-lean` returns an empty array, do NOT fall back to scanning `__agent_*` siblings on disk — that produces invalid `--to` values. Stop and report the empty result instead.

{send_message_instructions}

The recipient receives a short notification pointing to your file's absolute
path and reads the content via filesystem. Do NOT use `--get-output` — it
blocks and is only for non-interactive sessions. After sending, stay idle and
wait for the reply.

### List available peers

```
"<AGENTSCOMMANDER_BINARY_PATH>" list-peers-lean --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>"
```
"#,
        agent_root = agent_root,
        allowed_places = allowed_places,
        replica_usage = replica_usage,
        matrix_section = matrix_section,
        matrix_allowed = matrix_allowed,
        messaging_exception = messaging_exception,
        messaging_allowed = messaging_allowed,
        forbidden_scope = forbidden_scope,
        git_scope = git_scope,
        agency_cache_guidance = agency_cache_guidance,
        skills_section = skills_section,
        peer_name_format = peer_name_format,
        send_message_instructions = send_message_instructions,
    )
}

pub(crate) fn is_replica_agent_dir(cwd: &str) -> bool {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_"))
        .unwrap_or(false)
}

// Thread-local configuration injection keeps successful Root acceptance tests
// inside isolated fixtures; it is absent from production builds. Identity still
// requires the supplied directory to match the configured canonical Root path.
#[cfg(test)]
thread_local! {
    static ROOT_CONTEXT_TEST_CONFIG: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Detect git branch for a given directory path.
fn detect_git_branch(dir: &str) -> Option<String> {
    crate::process::git::context::detect_session_branch(dir)
}

#[cfg(test)]
mod tests;
