use super::{
    SeededContextTemplateSpec, COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
    COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME,
    COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
    COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
    GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS, GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
    GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES, GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
    GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
    GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
    OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND,
    STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS,
};

pub(crate) fn project_specs() -> [SeededContextTemplateSpec; 5] {
    let [windows, linux, macos] = platform_specs();
    [
        SeededContextTemplateSpec {
            id: "global",
            filename: crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME,
            label: "AgentsCommander shared context",
            current_version: 6,
            current_content: crate::config::session_context::get_default_agent_template,
            is_known_generated: None,
            project_actionable: true,
            suppress_unknown_without_state: true,
            distribution_owned: true,
        },
        SeededContextTemplateSpec {
            id: "coordinator",
            filename: crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            label: "Orchestrator context",
            current_version: 6,
            current_content: crate::config::session_context::get_default_coordinator_template,
            is_known_generated: Some(is_known_generated_coordinator_template),
            project_actionable: true,
            suppress_unknown_without_state: false,
            distribution_owned: false,
        },
        windows,
        linux,
        macos,
    ]
}

/// Frozen Windows v1 bytes for exact generated-file migration.
pub(crate) const HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT: &str = concat!(
    r#"## Host Platform Rules

Windows host session: use `C:\Program Files\Git\bin\bash.exe` for all shell work and every "#,
    r#"AgentsCommander CLI invocation; from PowerShell wrap with `& 'C:\Program "#,
    r#"Files\Git\bin\bash.exe' -lc '...'`; never capture CLI output without `2>&1 | "#,
    r#"Out-String`."#
);

/// #1605: the three per-EXECUTION-platform `{{HOST_PLATFORM_RULES}}` files
/// (`Context.platform.<os>.md`), seeded absent-only in project `.ac` roots and
/// carried through `sync_one_template` with seeded/observed state, edit
/// preservation and a pending-update offer.
/// `suppress_unknown_without_state: true` keeps a pre-existing unowned file
/// preserved silently and never prompted.
pub(crate) fn platform_specs() -> [SeededContextTemplateSpec; 3] {
    [
        SeededContextTemplateSpec {
            id: "platform.windows",
            filename: crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS,
            label: "Windows host platform rules",
            current_version: 2,
            current_content: || crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
            is_known_generated: Some(is_known_generated_platform_windows),
            project_actionable: true,
            suppress_unknown_without_state: true,
            distribution_owned: false,
        },
        SeededContextTemplateSpec {
            id: "platform.linux",
            filename: crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
            label: "Linux host platform rules",
            current_version: 1,
            current_content: || crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
            is_known_generated: Some(is_known_generated_platform_linux),
            project_actionable: true,
            suppress_unknown_without_state: true,
            distribution_owned: false,
        },
        SeededContextTemplateSpec {
            id: "platform.macos",
            filename: crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
            label: "macOS host platform rules",
            current_version: 1,
            current_content: || crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
            is_known_generated: Some(is_known_generated_platform_macos),
            project_actionable: true,
            suppress_unknown_without_state: true,
            distribution_owned: false,
        },
    ]
}

pub(crate) fn root_spec() -> SeededContextTemplateSpec {
    SeededContextTemplateSpec {
        id: "rootAgent",
        filename: crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME,
        label: "Root agent context",
        current_version: 10,
        current_content: crate::config::root_agent::default_root_context_template,
        is_known_generated: Some(
            crate::config::root_agent::is_known_generated_root_context_template,
        ),
        project_actionable: false,
        suppress_unknown_without_state: false,
        distribution_owned: false,
    }
}

pub(crate) fn project_spec_by_filename(filename: &str) -> Option<SeededContextTemplateSpec> {
    project_specs()
        .into_iter()
        .find(|spec| spec.filename == filename)
}

pub(crate) fn actionable_project_spec_by_filename(
    filename: &str,
) -> Result<SeededContextTemplateSpec, String> {
    if filename.contains('/') || filename.contains('\\') {
        return Err("Context template filename is not managed by AgentsCommander".to_string());
    }
    let spec = project_spec_by_filename(filename)
        .ok_or_else(|| "Context template filename is not managed by AgentsCommander".to_string())?;
    if spec.distribution_owned {
        return Err(
            "Context template filename is managed by the distribution and cannot be overwritten by \
                hand"
                .to_string(),
        );
    }
    if !spec.project_actionable {
        return Err("Context template filename is not actionable for this project".to_string());
    }
    Ok(spec)
}

/// #979: exact recognition of a STANDALONE (app-config) generated global context.
///
/// True only for byte-for-byte UTF-8 equality with the current built-in default or
/// with the frozen 307-byte snapshot above. No normalization of whitespace, line
/// endings, BOMs, or trailing newlines, and seed-state hashes are never consulted:
/// a CRLF copy, an invalid-UTF-8 file, a one-byte edit, or a state entry claiming
/// ownership is UNKNOWN, and unknown content is backed up, never deleted.
///
/// #1748: the PROJECT `global` spec is distribution-owned and names no recognizer,
/// so this is the only recognizer for these bytes. It must never be pointed at
/// `project_specs()`: a second consumer would make a future edit to it silently
/// change project behavior.
pub(crate) fn is_known_generated_standalone_global_template(content: &str) -> bool {
    content == crate::config::session_context::get_default_agent_template()
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        || content == STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
}

/// #1605: per-platform generated recognizers — equality with the current
/// platform default or a frozen historical snapshot. A future default change MUST first freeze the
/// previous default as a snapshot const and extend the recognizer, so seeded
/// files auto-update and edited files are preserved with the pending-update
/// offer.
pub(crate) fn is_known_generated_platform_windows(content: &str) -> bool {
    content == crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS
        || content == HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT
}

pub(crate) fn is_known_generated_platform_linux(content: &str) -> bool {
    content == crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX
}

pub(crate) fn is_known_generated_platform_macos(content: &str) -> bool {
    content == crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS
}

pub(crate) fn is_known_generated_coordinator_template(content: &str) -> bool {
    content == crate::config::session_context::get_default_coordinator_template()
        || content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
        || content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        || content == OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND
        || content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME
        || content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
}
